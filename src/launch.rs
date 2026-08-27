// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Starting applications.
//!
//! Both ways of launching something — a tile in Launchpad and a desktop entry
//! pop-launcher hands back — end up here, because both need two things a plain
//! spawn does not do:
//!
//! * An **XDG activation token**. Without one the compositor sees a window
//!   appear from a process it has no reason to believe the user asked for, and
//!   is entitled to apply focus-stealing prevention. The token is requested
//!   against the overlay's own surface, which is what ties the new window back
//!   to the click or keystroke that asked for it.
//! * The **discrete-GPU environment** for entries marked
//!   `PrefersNonDefaultGPU`. Which variables those are depends on the vendor,
//!   so they come from `switcheroo-control` rather than being hardcoded.
//!
//! `spawn_desktop_exec` does the rest: it resolves the user's configured
//! terminal for `Terminal=true` entries, expands `Exec` field codes, and places
//! the child in its own systemd scope so it neither dies with the daemon nor
//! inherits its cgroup.

use std::path::PathBuf;

use jump_core::GpuPreference;

/// A desktop entry resolved to the point where it can be started.
#[derive(Debug, Clone)]
pub struct Launch {
    /// Desktop file id, passed through so the compositor can match the window
    /// it maps back to the entry that asked for it.
    pub app_id: String,
    pub exec: String,
    pub terminal: bool,
    pub gpu: GpuPreference,
}

impl Launch {
    /// Resolve the entry at `path`.
    ///
    /// `action_name` selects one of the entry's desktop actions — "New Window",
    /// "New Private Window", and friends — which is how pop-launcher reports a
    /// match against an action rather than against the application itself.
    /// Without one the entry's own `Exec` is used.
    #[must_use]
    pub fn from_desktop_file(
        path: PathBuf,
        action_name: Option<String>,
        gpu: GpuPreference,
    ) -> Option<Self> {
        let locales = cosmic::desktop::fde::get_languages_from_env();
        let entry = cosmic::desktop::load_desktop_file(&locales, path)?;

        let exec = match action_name {
            Some(name) => entry
                .desktop_actions
                .into_iter()
                .find(|action| action.name == name)
                .map(|action| action.exec),
            None => entry.exec,
        }?;

        Some(Self {
            app_id: entry.id,
            exec,
            terminal: entry.terminal,
            gpu,
        })
    }

    /// Start the application, handing `token` to it as the activation token.
    ///
    /// A missing token is not an error: xdg-activation is optional, and a
    /// compositor that does not implement it simply focuses the new window on
    /// its own terms.
    pub async fn run(self, token: Option<String>) {
        let mut env: Vec<(String, String)> = Vec::new();

        if let Some(token) = token {
            // Wayland clients read the first; XWayland clients and anything
            // going through startup notification read the second. Both are set
            // because a launcher cannot know which the child will be.
            env.push(("XDG_ACTIVATION_TOKEN".into(), token.clone()));
            env.push(("DESKTOP_STARTUP_ID".into(), token));
        }

        env.extend(gpu_env(self.gpu).await);

        cosmic::desktop::spawn_desktop_exec(self.exec, env, Some(&self.app_id), self.terminal)
            .await;
    }
}

/// Environment that steers a process onto the requested GPU.
///
/// Empty on a single-GPU machine, and empty when `switcheroo-control` is not
/// running — in both cases the default GPU is the only sensible answer, and
/// forcing PRIME variables at a machine without a discrete card would be worse
/// than doing nothing.
async fn gpu_env(preference: GpuPreference) -> Vec<(String, String)> {
    let Some(gpus) = gpus().await else {
        return Vec::new();
    };

    let gpu = match preference {
        GpuPreference::Default => gpus.into_iter().find(|gpu| gpu.default),
        GpuPreference::NonDefault => gpus.into_iter().find(|gpu| !gpu.default),
        GpuPreference::SpecificIdx(index) => gpus.into_iter().nth(index as usize),
    };

    gpu.map(|gpu| gpu.environment.into_iter().collect())
        .unwrap_or_default()
}

/// Ask `switcheroo-control` what GPUs are available.
///
/// Queried per launch rather than cached at startup: a machine can gain or lose
/// an external GPU while the daemon is running, and the call costs one system
/// bus round trip on a path that is already spawning a process.
async fn gpus() -> Option<Vec<switcheroo_control::Gpu>> {
    let connection = zbus::Connection::system().await.ok()?;

    // Property caching is turned off deliberately. It is populated eagerly when
    // the proxy is built, and on the majority of machines — where nothing
    // provides `net.hadess.SwitcherooControl` — that eager call fails and zbus
    // logs a warning about it. Two lines of noise per launch, to prime a cache
    // for two properties read once.
    let proxy = switcheroo_control::SwitcherooControlProxy::builder(&connection)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .ok()?;

    if !proxy.has_dual_gpu().await.ok()? {
        return None;
    }

    let gpus = proxy.get_gpus().await.ok()?;
    (!gpus.is_empty()).then_some(gpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a desktop file and hand back its path.
    ///
    /// Named per test so a failure leaves the fixture behind to look at, and so
    /// two tests cannot collide on the same path.
    fn entry(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("jump-launch-{name}.desktop"));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn desktop_entry_resolves_to_something_launchable() {
        let path = entry(
            "plain",
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Probe\n\
             Exec=/usr/bin/probe --flag\n\
             Terminal=true\n",
        );

        let launch = Launch::from_desktop_file(path.clone(), None, GpuPreference::Default)
            .expect("entry is launchable");

        assert_eq!(launch.exec, "/usr/bin/probe --flag");
        assert!(launch.terminal);
        assert_eq!(launch.app_id, "jump-launch-plain");

        std::fs::remove_file(path).ok();
    }

    /// pop-launcher reports a match against a desktop *action* — "New Window"
    /// and friends — by naming it alongside the entry. Launching the entry's
    /// own `Exec` there would open the application instead of doing what the
    /// user picked.
    #[test]
    fn a_named_action_wins_over_the_entry() {
        let path = entry(
            "action",
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Probe\n\
             Exec=/usr/bin/probe\n\
             Actions=new-window;\n\
             \n\
             [Desktop Action new-window]\n\
             Name=New Window\n\
             Exec=/usr/bin/probe --new-window\n",
        );

        let launch = Launch::from_desktop_file(
            path.clone(),
            Some("New Window".to_owned()),
            GpuPreference::Default,
        )
        .expect("action is launchable");

        assert_eq!(launch.exec, "/usr/bin/probe --new-window");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn an_unknown_action_is_not_launchable() {
        let path = entry(
            "unknown-action",
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Probe\n\
             Exec=/usr/bin/probe\n",
        );

        assert!(
            Launch::from_desktop_file(
                path.clone(),
                Some("No Such Action".to_owned()),
                GpuPreference::Default,
            )
            .is_none()
        );

        std::fs::remove_file(path).ok();
    }

    /// Legal for an entry that exists only to be a MIME handler target.
    #[test]
    fn an_entry_without_exec_is_not_launchable() {
        let path = entry(
            "no-exec",
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Probe\n",
        );

        assert!(Launch::from_desktop_file(path.clone(), None, GpuPreference::Default).is_none());

        std::fs::remove_file(path).ok();
    }
}
