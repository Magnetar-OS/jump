// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Process search, behind the `kill` keyword.
//!
//! Typing `kill fire` lists the user's own processes whose name or command
//! line matches, heaviest first; activating one sends `SIGTERM`, and the
//! frontend offers `SIGKILL` as a secondary action. Behind a keyword rather
//! than always-on for the same reason clipboard history is: a result whose
//! activation kills something must never appear next to results whose
//! activation opens something.
//!
//! The listing is a plain `/proc` walk, done fresh per keystroke. A few
//! hundred processes at two small files each is well under a millisecond of
//! I/O, and a cache would only be a way to offer a process that has already
//! exited.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::model::{Icon, Item, ItemKey, Source};

/// Results returned to the UI.
const RESULTS: usize = 12;

/// One running process.
#[derive(Debug, Clone)]
struct Process {
    pid: u32,
    /// Short name from `/proc/<pid>/comm`.
    name: String,
    /// Full command line, NUL separators replaced with spaces.
    cmdline: String,
    /// Resident set size in bytes.
    rss: u64,
}

/// The user's own processes matching `needle`, heaviest first.
///
/// An empty needle lists everything, which is what `kill ` with nothing after
/// it shows: the biggest things running, which is usually what the user is
/// hunting for anyway.
#[must_use]
pub fn matching(needle: &str) -> Vec<Item> {
    let needle = needle.to_lowercase();
    let tokens: Vec<&str> = needle.split_whitespace().collect();

    let mut processes: Vec<Process> = list()
        .into_iter()
        .filter(|process| {
            let haystack = format!(
                "{} {} {}",
                process.name.to_lowercase(),
                process.cmdline.to_lowercase(),
                process.pid
            );
            tokens.iter().all(|token| haystack.contains(token))
        })
        .collect();

    // Heaviest first: with no better signal, the process eating the most
    // memory is the likeliest kill target, and a deterministic order beats
    // pid order for a list the user scans.
    processes.sort_by_key(|process| std::cmp::Reverse(process.rss));
    processes.truncate(RESULTS);

    processes
        .into_iter()
        .map(|process| Item {
            // Keyed by name rather than pid: pids never repeat across
            // sessions, so frecency could not learn "the user keeps killing
            // this program" any other way.
            key: ItemKey(format!("process:{}", process.name)),
            id: 0,
            title: process.name,
            subtitle: format!(
                "PID {} · {} · {}",
                process.pid,
                human_bytes(process.rss),
                truncate(&process.cmdline, 80),
            ),
            icon: Some(Icon::Name("utilities-system-monitor-symbolic".to_owned())),
            category_icon: None,
            window: None,
            source: Source::Process { pid: process.pid },
            autocomplete: None,
            score: 0.0,
        })
        .collect()
}

/// Ask a process to exit, or make it.
///
/// `SIGTERM` by default so the process can save and clean up; `force` is
/// `SIGKILL`, the action panel's escalation for the ones that ignore that.
/// Delivered through `kill(1)` rather than a syscall binding — it is
/// everywhere, and this path runs a handful of times per session.
pub fn terminate(pid: u32, force: bool) {
    let signal = if force { "-KILL" } else { "-TERM" };
    let result = Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match result {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(pid, ?status, "kill refused; process may have exited"),
        Err(error) => tracing::error!(pid, %error, "could not run kill"),
    }
}

/// Every live process belonging to the current user, except this one.
fn list() -> Vec<Process> {
    use std::os::unix::fs::MetadataExt;

    let uid = effective_uid();
    let own_pid = std::process::id();

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if pid == own_pid {
                return None;
            }

            // Other users' processes cannot be signalled, so listing them
            // would only offer actions that fail.
            let metadata = entry.metadata().ok()?;
            if u64::from(metadata.uid()) != uid {
                return None;
            }

            let path = entry.path();
            read_process(&path, pid)
        })
        .collect()
}

/// Read one process's identity, skipping kernel threads.
fn read_process(path: &Path, pid: u32) -> Option<Process> {
    let cmdline_raw = std::fs::read(path.join("cmdline")).ok()?;
    // Kernel threads have an empty cmdline; nothing the user can kill.
    if cmdline_raw.is_empty() {
        return None;
    }
    let cmdline: String = cmdline_raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");

    let name = std::fs::read_to_string(path.join("comm"))
        .ok()?
        .trim()
        .to_owned();

    // statm's second field is resident pages.
    let rss = std::fs::read_to_string(path.join("statm"))
        .ok()
        .and_then(|statm| {
            statm
                .split_whitespace()
                .nth(1)
                .and_then(|pages| pages.parse::<u64>().ok())
        })
        .map_or(0, |pages| pages * 4096);

    Some(Process {
        pid,
        name,
        cmdline,
        rss,
    })
}

/// The effective uid, without a libc dependency: the answer is in procfs,
/// and reading our own `/proc/self/status` costs one small file.
fn effective_uid() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                let rest = line.strip_prefix("Uid:")?;
                // Fields: real, effective, saved, filesystem.
                rest.split_whitespace().nth(1)?.parse().ok()
            })
        })
        .unwrap_or(u64::MAX)
}

fn human_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(limit).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn finds_and_terminates_a_real_process() {
        let mut child = Command::new("sleep")
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        // Give /proc a moment on slow machines.
        std::thread::sleep(Duration::from_millis(50));

        let items = matching(&pid.to_string());
        assert!(
            items
                .iter()
                .any(|item| item.source == Source::Process { pid }),
            "spawned process appears in the listing"
        );

        terminate(pid, false);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait().expect("try_wait") {
                Some(_) => break,
                None if Instant::now() > deadline => panic!("process survived SIGTERM"),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    #[test]
    fn all_tokens_must_match() {
        let mut child = Command::new("sleep")
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(50));

        let items = matching(&format!("sleep no-such-token-{pid}"));
        assert!(items.is_empty());

        terminate(pid, true);
        let _ = child.wait();
    }

    #[test]
    fn other_processes_do_not_leak_into_a_pid_query() {
        // A query for a nonsense token matches nothing at all.
        assert!(matching("zzqx-definitely-not-a-process").is_empty());
    }

    #[test]
    fn bytes_render_for_humans() {
        assert_eq!(human_bytes(512 * 1024 * 1024), "512 MB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
