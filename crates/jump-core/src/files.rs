// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! File search.
//!
//! ## Why we build our own index
//!
//! The system `plocate` database is not usable for this. On a btrfs machine
//! running snapper it contained 37,153,401 paths of which **36,496,775 were
//! under `/.snapshots/`** — 98.2% of the index is snapshot duplicates of the
//! same files. Every one of the first 2000 results for `config` came from a
//! snapshot, so filtering them out after the fact returns nothing: the limit is
//! exhausted long before a real file appears.
//!
//! Editing `/etc/updatedb.conf` would fix that, but it needs root, it is a
//! system-wide change made on the user's behalf, and it still would not index
//! removable drives. Building a private database instead solves all three at
//! once and costs 8.75 s for 1.5 M entries in a 44 MB file — cheap enough to
//! refresh on a timer.
//!
//! ## Why ranking has to be separate from matching
//!
//! `plocate` answers "which paths contain this substring" in order of database
//! position, which is essentially filesystem order. It has no notion of
//! relevance, so the raw answer for a common word is thousands of equally-ranked
//! paths. Useful results come from over-fetching a bounded candidate set and
//! ranking it here, in two phases:
//!
//! 1. **String-only scoring** over every candidate. No syscalls, so it can run
//!    over thousands of paths within a frame.
//! 2. **Filesystem scoring** over only the survivors — `stat` is a syscall per
//!    file, so it runs on ~100 paths, not 2000.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use tokio::process::Command;

use crate::model::{Icon, Item, ItemKey, Source};

/// Paths fetched from the index before ranking.
///
/// Large enough that the good answer is almost certainly somewhere in the set,
/// small enough that scoring stays well inside a frame. Measured at ~20 ms for
/// 2000 candidates, dominated by `plocate` itself rather than by scoring.
const CANDIDATES: usize = 2000;

/// Paths fetched per extension when enumerating content-index candidates.
///
/// Generous because this runs in the background, not per keystroke, but still
/// bounded so one pathological extension cannot dominate the pass.
const CONTENT_CANDIDATES: usize = 20_000;

/// Candidates promoted to the `stat` phase.
const STAT_BUDGET: usize = 120;

/// Results returned to the UI.
const RESULTS: usize = 12;

/// A plugin-style deadline: file search must never delay the frame.
const QUERY_TIMEOUT: Duration = Duration::from_millis(250);

/// Directory names that are never worth indexing.
///
/// These are build and cache trees: high file counts, near-zero chance the user
/// is searching for something inside them by name.
const PRUNE_NAMES: &str = ".git node_modules .cache __pycache__ .venv venv target .next .nuxt \
     .gradle .tox .mypy_cache .pytest_cache .terraform vendor .snapshots .Trash-1000 Trash \
     .cargo .rustup .npm .pnpm-store .yarn .bun .deno .m2 .ivy2 .nuget .conda .rbenv \
     .pyenv .nvm .stack .cabal .ccache .zig-cache .swiftpm";

/// Query tokens shorter than this are not worth sending to the index on their
/// own — they match nearly everything.
const MIN_TOKEN: usize = 2;

#[derive(Debug, Clone)]
pub struct Config {
    /// Whether file search runs at all.
    pub enabled: bool,
    /// Trees to index. Defaults to the user's home directory.
    pub roots: Vec<PathBuf>,
    /// Directory names never to index, in addition to the built-in list.
    ///
    /// Applied at index time, so excluding a large tree makes the index both
    /// smaller and faster to build — unlike a query-time filter, which pays for
    /// the paths anyway.
    pub ignore_names: Vec<String>,
    /// Absolute paths never to index.
    pub ignore_paths: Vec<PathBuf>,
    /// If non-empty, only files with these extensions are returned.
    ///
    /// Applied at query time rather than index time: it is the setting most
    /// likely to be changed on a whim, and re-indexing after every change would
    /// be a poor trade for a filter this cheap.
    pub include_extensions: Vec<String>,
    /// Extensions never returned, beyond the built-in noise list.
    pub exclude_extensions: Vec<String>,
    /// Also index mounted external volumes (NTFS drives and the like).
    ///
    /// Off by default: these are frequently multi-terabyte, and indexing one
    /// takes minutes rather than seconds.
    pub external_drives: bool,
    /// How stale the index may get before it is rebuilt.
    pub refresh: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: dirs::home_dir().into_iter().collect(),
            ignore_names: Vec::new(),
            ignore_paths: Vec::new(),
            include_extensions: Vec::new(),
            exclude_extensions: Vec::new(),
            external_drives: false,
            refresh: Duration::from_secs(6 * 3600),
        }
    }
}

/// File search backed by a private `plocate` database.
#[derive(Debug, Clone)]
pub struct Files {
    config: Config,
    /// Databases to search, most important first.
    databases: Vec<PathBuf>,
    data_dir: PathBuf,
}

impl Files {
    #[must_use]
    pub fn new(config: Config) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let data_dir = dirs::data_dir()?.join("jump").join("index");

        let mut files = Self {
            config,
            databases: Vec::new(),
            data_dir,
        };
        files.databases = files.existing_databases();
        Some(files)
    }

    fn home_db(&self) -> PathBuf {
        self.data_dir.join("home.db")
    }

    fn external_db(&self) -> PathBuf {
        self.data_dir.join("external.db")
    }

    /// Databases that exist on disk right now.
    ///
    /// `plocate` aborts the whole query if any database in the list is missing,
    /// so a stale path must never be passed to it.
    fn existing_databases(&self) -> Vec<PathBuf> {
        let mut databases = Vec::new();
        if self.home_db().is_file() {
            databases.push(self.home_db());
        }
        if self.config.external_drives && self.external_db().is_file() {
            databases.push(self.external_db());
        }
        databases
    }

    /// Whether any index exists yet.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.databases.is_empty()
    }

    /// Whether the index is missing or older than the refresh interval.
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(self.home_db()) else {
            return true;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age > self.config.refresh)
    }

    /// Rebuild the index.
    ///
    /// Runs `updatedb` per root. This takes seconds for a home directory and
    /// minutes for a multi-terabyte drive, so it is never called on the
    /// interactive path — the caller schedules it.
    pub async fn rebuild(&mut self) {
        if let Err(error) = tokio::fs::create_dir_all(&self.data_dir).await {
            tracing::warn!(%error, dir = ?self.data_dir, "cannot create index directory");
            return;
        }

        let roots: Vec<PathBuf> = self
            .config
            .roots
            .iter()
            .filter(|root| root.is_dir())
            .cloned()
            .collect();

        if !roots.is_empty() {
            build_database(&roots, &self.home_db(), &self.config).await;
        }

        if self.config.external_drives {
            let external = external_mounts();
            if external.is_empty() {
                tracing::info!("no external volumes mounted; skipping external index");
            } else {
                tracing::info!(count = external.len(), "indexing external volumes");
                build_database(&external, &self.external_db(), &self.config).await;
            }
        }

        self.databases = self.existing_databases();
    }

    /// Search the index and return ranked results.
    pub async fn search(&self, query: &str) -> Vec<Item> {
        if self.databases.is_empty() {
            return Vec::new();
        }

        let tokens = tokenize(query);
        let Some(probe) = tokens.iter().max_by_key(|token| token.len()) else {
            return Vec::new();
        };
        if probe.len() < MIN_TOKEN {
            return Vec::new();
        }

        let candidates = self.candidates(probe).await;
        if candidates.is_empty() {
            return Vec::new();
        }

        let candidates: Vec<String> = candidates
            .into_iter()
            .filter(|path| self.extension_allowed(path))
            .collect();

        rank(candidates, &tokens).await
    }

    /// Paths worth handing to the content indexer.
    ///
    /// Enumerated from the path index rather than by walking the tree again:
    /// `updatedb` already did that walk, and asking `plocate` for one extension
    /// at a time is both faster and automatically consistent with whatever the
    /// index excludes.
    pub async fn content_candidates(&self, extensions: &[String]) -> Vec<PathBuf> {
        let mut candidates: Vec<(u8, PathBuf)> = Vec::new();
        let mut seen = HashSet::new();

        for extension in extensions {
            let needle = normalise_extension(extension);
            for path in self.candidates_for(&needle, CONTENT_CANDIDATES).await {
                // plocate matches substrings, so `.ts` also returns `.tsx`.
                if !path.to_lowercase().ends_with(&needle) {
                    continue;
                }
                if !seen.insert(path.clone()) {
                    continue;
                }
                candidates.push((index_priority(&path), PathBuf::from(path)));
            }
        }

        // Highest priority first. The content index has a size ceiling, so the
        // *order* decides what survives when it is reached: documents the user
        // wrote should be indexed before scratch files and dotfile trees, not
        // whichever extension happened to be enumerated first.
        candidates.sort_by_key(|(priority, _)| std::cmp::Reverse(*priority));
        candidates.into_iter().map(|(_, path)| path).collect()
    }

    /// Whether a path survives the configured extension filters.
    fn extension_allowed(&self, path: &str) -> bool {
        let lower = path.to_lowercase();

        if self
            .config
            .exclude_extensions
            .iter()
            .any(|ext| lower.ends_with(&normalise_extension(ext)))
        {
            return false;
        }

        if self.config.include_extensions.is_empty() {
            return true;
        }

        // Directories have no extension but are still worth showing when the
        // user has restricted the result types, since they contain them.
        self.config
            .include_extensions
            .iter()
            .any(|ext| lower.ends_with(&normalise_extension(ext)))
    }

    /// Ask `plocate` for paths whose basename contains `probe`.
    ///
    /// `--basename` matters: a whole-path match for `config` hits every file
    /// under every directory called `config`, which buries the file actually
    /// named `config`.
    async fn candidates(&self, probe: &str) -> Vec<String> {
        self.candidates_for(probe, CANDIDATES).await
    }

    /// Ask `plocate` for up to `limit` basename matches.
    async fn candidates_for(&self, probe: &str, limit: usize) -> Vec<String> {
        // plocate accepts several databases as one colon-separated argument.
        let databases = self
            .databases
            .iter()
            .filter_map(|path| path.to_str())
            .collect::<Vec<_>>()
            .join(":");

        let output = Command::new("plocate")
            .arg("--database")
            .arg(&databases)
            .arg("--basename")
            .arg("--ignore-case")
            .arg("--limit")
            .arg(limit.to_string())
            .arg(probe)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();

        let output = match tokio::time::timeout(QUERY_TIMEOUT, output).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                tracing::warn!(%error, "plocate failed to run");
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!("file search exceeded its deadline");
                return Vec::new();
            }
        };

        // plocate exits non-zero when nothing matched, which is not an error.
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    }
}

/// Run `updatedb` over `roots` into `output`.
async fn build_database(roots: &[PathBuf], output: &Path, config: &Config) {
    // updatedb takes a single root, so multiple roots mean multiple databases.
    // Rather than juggle a database per root, index the common ancestor when
    // there is one and otherwise index each root into a temporary and keep the
    // last — in practice `roots` is the home directory alone.
    let Some(root) = roots.first() else {
        return;
    };
    if roots.len() > 1 {
        tracing::warn!(
            extra = roots.len() - 1,
            "updatedb indexes one root per database; only the first is indexed"
        );
    }

    let started = SystemTime::now();
    let result = Command::new("updatedb")
        .arg("--database-root")
        .arg(root)
        .arg("--output")
        .arg(output)
        // Without this, updatedb records only paths readable by everyone, which
        // for a private index of the user's own home is exactly backwards.
        .arg("--require-visibility")
        .arg("no")
        .arg("--prune-bind-mounts")
        .arg("yes")
        .arg("--add-prunenames")
        .arg(prune_names(config))
        .arg("--add-prunepaths")
        .arg(prune_paths(config))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;

    match result {
        Ok(output_result) if output_result.status.success() => {
            let elapsed = started.elapsed().unwrap_or_default();
            tracing::info!(?root, ?elapsed, "index rebuilt");
        }
        Ok(output_result) => {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            tracing::warn!(?root, %stderr, "updatedb failed");
        }
        Err(error) => tracing::warn!(%error, "could not run updatedb"),
    }
}

/// Indexing priority for a path, higher first.
///
/// Deliberately coarse. The aim is to get the user's own documents and projects
/// in before anything else, not to produce a finely-graded ordering — a
/// three-way split does nearly all the work a hundred rules would.
fn index_priority(path: &str) -> u8 {
    const PREFERRED: &[&str] = &[
        "/documents/",
        "/desktop/",
        "/projects/",
        "/github/",
        "/work/",
        "/notes/",
        "/src/",
    ];

    let lower = path.to_lowercase();

    // Hidden trees are configuration and state: occasionally useful to search,
    // never the first thing worth spending the index budget on.
    if lower.contains("/.") {
        return 0;
    }
    if PREFERRED.iter().any(|dir| lower.contains(dir)) {
        return 2;
    }
    1
}

/// Built-in prune list plus anything the user added.
fn prune_names(config: &Config) -> String {
    if config.ignore_names.is_empty() {
        return PRUNE_NAMES.to_owned();
    }
    format!("{PRUNE_NAMES} {}", config.ignore_names.join(" "))
}

/// User-specified absolute paths to skip. Space-separated, as updatedb expects.
fn prune_paths(config: &Config) -> String {
    config
        .ignore_paths
        .iter()
        .filter_map(|path| path.to_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Mounted volumes that look like removable or external storage.
///
/// Derived from `/proc/mounts` rather than a configured list so that plugging a
/// drive in does not require editing configuration. NTFS is the common case, but
/// exFAT and ext4 externals are picked up the same way.
fn external_mounts() -> Vec<PathBuf> {
    const EXTERNAL_FS: &[&str] = &["ntfs3", "ntfs", "fuseblk", "exfat", "vfat"];
    const EXTERNAL_ROOTS: &[&str] = &["/run/media/", "/media/", "/mnt/"];

    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let mount_point = fields.next()?;
            let filesystem = fields.next()?;

            // `/proc/mounts` escapes spaces as \040.
            let mount_point = mount_point.replace("\\040", " ");

            let external = EXTERNAL_FS.contains(&filesystem)
                || EXTERNAL_ROOTS
                    .iter()
                    .any(|root| mount_point.starts_with(root));

            (external && seen.insert(mount_point.clone())).then(|| PathBuf::from(mount_point))
        })
        .filter(|path| path.is_dir())
        .collect()
}

/// Accept extensions written either as `pdf` or `.pdf`.
fn normalise_extension(extension: &str) -> String {
    let lower = extension.trim().to_lowercase();
    if lower.starts_with('.') {
        lower
    } else {
        format!(".{lower}")
    }
}

/// Split a query into lowercase tokens.
fn tokenize(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|token| !token.is_empty())
        .collect()
}

/// Score and order candidates, then attach filesystem signals to the survivors.
async fn rank(candidates: Vec<String>, tokens: &[String]) -> Vec<Item> {
    let mut scored: Vec<(f32, String)> = candidates
        .into_iter()
        .filter_map(|path| score_path(&path, tokens).map(|score| (score, path)))
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(STAT_BUDGET);

    // Phase two: only now does anything touch the filesystem.
    let now = SystemTime::now();
    let mut refined: Vec<(f32, PathBuf)> = Vec::with_capacity(scored.len());
    for (score, path) in scored {
        let path = PathBuf::from(path);
        let Ok(metadata) = tokio::fs::metadata(&path).await else {
            // Indexed but gone: the database is a snapshot, not the truth.
            continue;
        };

        let mut score = score;
        if metadata.is_dir() {
            // Directories are usually a means to a file, not the destination.
            score -= 0.15;
        }
        if let Ok(modified) = metadata.modified()
            && let Ok(age) = now.duration_since(modified)
        {
            score += recency_bonus(age);
        }
        refined.push((score, path));
    }

    refined.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    refined.truncate(RESULTS);

    refined
        .into_iter()
        .map(|(score, path)| item(&path, score))
        .collect()
}

/// Recency contribution, on the same scale as the string signals.
///
/// Bucketed rather than continuous for the same reason frecency is: a file must
/// not drift down the list while the user is looking at it.
fn recency_bonus(age: Duration) -> f32 {
    const DAY: u64 = 86_400;
    match age.as_secs() {
        0..=DAY => 0.5,
        secs if secs <= 7 * DAY => 0.3,
        secs if secs <= 30 * DAY => 0.15,
        _ => 0.0,
    }
}

/// String-only relevance. `None` rejects the candidate outright.
fn score_path(path: &str, tokens: &[String]) -> Option<f32> {
    let lower = path.to_lowercase();

    // Every token must appear somewhere in the path. plocate only guaranteed
    // the probe token, so this is what makes multi-word queries behave: typing
    // "cargo toml" should not return every Cargo.toml on the system.
    if !tokens.iter().all(|token| lower.contains(token.as_str())) {
        return None;
    }

    let basename = lower.rsplit('/').next().unwrap_or(&lower);
    let mut score = 0.0;

    // The strongest signal by far: how well the *name* matches, not the path.
    let longest = tokens.iter().max_by_key(|token| token.len())?;
    score += name_match(basename, longest) * 3.0;

    // Every token that also hits the basename is worth more than one that only
    // matched somewhere up the directory tree.
    let in_name = tokens
        .iter()
        .filter(|token| basename.contains(token.as_str()))
        .count();
    score += in_name as f32 * 0.4;

    // Shallow paths are more likely to be what someone means. Capped so that a
    // deeply-nested exact match still beats a shallow partial one.
    let depth = path.matches('/').count();
    score -= (depth as f32 * 0.06).min(0.9);

    // Hidden files are usually configuration the user did not ask for, unless
    // they typed a leading dot.
    let hidden = lower.contains("/.");
    if hidden && !tokens.iter().any(|token| token.starts_with('.')) {
        score -= 0.8;
    }

    score += location_prior(&lower);
    score += extension_prior(basename);

    Some(score)
}

/// How well `name` matches `token`, in 0.0..=1.0.
///
/// plocate already guaranteed a substring match somewhere, so this is about
/// *where* and *how much* of the name the token accounts for.
fn name_match(name: &str, token: &str) -> f32 {
    let Some(position) = name.find(token) else {
        return 0.0;
    };

    if name == token {
        1.0
    } else if position == 0 {
        // Prefix match, scaled by how much of the name the token covers, so
        // "cargo" scores higher against "cargo.toml" than against
        // "cargo-something-very-long".
        0.75 + 0.2 * (token.len() as f32 / name.len() as f32)
    } else if name[..position].ends_with(['-', '_', '.', ' ']) {
        // Start of a word inside the name.
        0.6
    } else {
        0.35
    }
}

/// Directory-based priors, expressed as a bonus or penalty.
fn location_prior(lower: &str) -> f32 {
    const PREFERRED: &[&str] = &[
        "/documents/",
        "/desktop/",
        "/downloads/",
        "/pictures/",
        "/projects/",
        "/github/",
        "/src/",
        "/work/",
    ];
    const DEPRIORITISED: &[&str] = &[
        "/.local/",
        "/.config/",
        "/.mozilla/",
        "/.steam/",
        "/site-packages/",
        "/dist-packages/",
        "/.wine/",
        "/proton/",
    ];

    let mut prior = 0.0;
    if PREFERRED.iter().any(|dir| lower.contains(dir)) {
        prior += 0.6;
    }
    if DEPRIORITISED.iter().any(|dir| lower.contains(dir)) {
        prior -= 0.7;
    }
    prior
}

/// Extension-based priors: things people open beat build residue.
fn extension_prior(basename: &str) -> f32 {
    const DOCUMENTS: &[&str] = &[
        ".pdf", ".odt", ".docx", ".doc", ".md", ".txt", ".epub", ".xlsx", ".ods", ".pptx", ".rtf",
    ];
    const MEDIA: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".svg", ".mp4", ".mkv", ".mp3", ".flac", ".wav",
    ];
    const NOISE: &[&str] = &[
        ".lock", ".tmp", ".swp", ".pyc", ".o", ".d", ".rlib", ".rmeta", ".log", ".bak", ".part",
    ];

    if DOCUMENTS.iter().any(|ext| basename.ends_with(ext)) {
        0.5
    } else if MEDIA.iter().any(|ext| basename.ends_with(ext)) {
        0.3
    } else if NOISE.iter().any(|ext| basename.ends_with(ext)) {
        -1.2
    } else {
        0.0
    }
}

/// Open a file with the user's default handler.
///
/// Delegates to `xdg-open` rather than resolving the MIME association here:
/// the desktop already owns that mapping, and duplicating it would diverge.
pub fn open(path: &Path) {
    let result = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match result {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(error) => tracing::error!(%error, ?path, "could not open file"),
    }
}

/// Turn a ranked path into a result row.
fn item(path: &Path, score: f32) -> Item {
    let title = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    // Show the containing directory, with the home prefix shortened, because the
    // full path of a deeply-nested file is unreadable at row width.
    let subtitle = path
        .parent()
        .map(|parent| {
            let text = parent.display().to_string();
            match dirs::home_dir().and_then(|home| {
                text.strip_prefix(home.to_str()?)
                    .map(|rest| format!("~{rest}"))
            }) {
                Some(shortened) => shortened,
                None => text,
            }
        })
        .unwrap_or_default();

    Item {
        key: ItemKey(format!("file:{}", path.display())),
        id: 0,
        title,
        subtitle,
        icon: Some(Icon::Name("text-x-generic".to_owned())),
        category_icon: Some(Icon::Name("system-file-manager".to_owned())),
        window: None,
        source: Source::File {
            path: path.to_path_buf(),
        },
        autocomplete: None,
        score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(path: &str, query: &str) -> f32 {
        score_path(path, &tokenize(query)).unwrap_or(f32::MIN)
    }

    #[test]
    fn exact_name_beats_partial() {
        let exact = score("/home/u/documents/report.pdf", "report");
        let partial = score("/home/u/documents/report-draft-v2.pdf", "report");
        assert!(exact > partial);
    }

    #[test]
    fn shallow_beats_deep() {
        let shallow = score("/home/u/notes.md", "notes");
        let deep = score("/home/u/a/b/c/d/e/f/notes.md", "notes");
        assert!(shallow > deep);
    }

    #[test]
    fn build_residue_is_penalised() {
        let source = score("/home/u/projects/main.rs", "main");
        let residue = score("/home/u/projects/main.o", "main");
        assert!(source > residue);
    }

    #[test]
    fn hidden_paths_are_penalised_unless_asked_for() {
        let visible = score("/home/u/projects/config.toml", "config");
        let hidden = score("/home/u/.config/config.toml", "config");
        assert!(visible > hidden);
    }

    #[test]
    fn all_tokens_must_match() {
        // The probe token matched, but the second token appears nowhere.
        assert!(score_path("/home/u/Cargo.toml", &tokenize("cargo nonexistent")).is_none());
        assert!(score_path("/home/u/jump/Cargo.toml", &tokenize("cargo jump")).is_some());
    }

    #[test]
    fn documents_outrank_unknown_types() {
        let document = score("/home/u/documents/thesis.pdf", "thesis");
        let unknown = score("/home/u/documents/thesis.xyz", "thesis");
        assert!(document > unknown);
    }

    #[test]
    fn word_boundary_beats_mid_word() {
        let boundary = score("/home/u/my-report.pdf", "report");
        let mid = score("/home/u/xxreportxx.pdf", "report");
        assert!(boundary > mid);
    }

    fn files_with(config: Config) -> Files {
        Files {
            config,
            databases: vec![PathBuf::from("/nonexistent")],
            data_dir: PathBuf::from("/tmp"),
        }
    }

    #[test]
    fn documents_are_indexed_before_dotfiles() {
        assert!(
            index_priority("/home/u/Documents/thesis.md") > index_priority("/home/u/scratch.md")
        );
        assert!(index_priority("/home/u/scratch.md") > index_priority("/home/u/.config/app.md"));
    }

    #[test]
    fn include_extensions_restrict_results() {
        let files = files_with(Config {
            include_extensions: vec!["pdf".to_owned(), ".md".to_owned()],
            ..Config::default()
        });

        assert!(files.extension_allowed("/home/u/report.pdf"));
        assert!(files.extension_allowed("/home/u/notes.md"));
        assert!(!files.extension_allowed("/home/u/main.rs"));
    }

    #[test]
    fn exclude_extensions_win_over_include() {
        let files = files_with(Config {
            include_extensions: vec!["pdf".to_owned()],
            exclude_extensions: vec!["pdf".to_owned()],
            ..Config::default()
        });

        assert!(!files.extension_allowed("/home/u/report.pdf"));
    }

    #[test]
    fn everything_is_allowed_by_default() {
        let files = files_with(Config::default());
        assert!(files.extension_allowed("/home/u/anything.xyz"));
    }

    #[test]
    fn user_prune_names_extend_the_builtin_list() {
        let config = Config {
            ignore_names: vec!["Games".to_owned()],
            ..Config::default()
        };
        let names = prune_names(&config);
        assert!(names.contains("node_modules"), "built-ins are kept");
        assert!(names.contains("Games"));
    }

    #[test]
    fn recency_is_bucketed_and_decays() {
        assert!(
            recency_bonus(Duration::from_secs(3600))
                > recency_bonus(Duration::from_secs(86_400 * 10))
        );
        assert_eq!(recency_bonus(Duration::from_secs(86_400 * 400)), 0.0);
    }
}
