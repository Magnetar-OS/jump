// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Alfred-style plugin host.
//!
//! pop-launcher already has a plugin system, and it keeps working — anything
//! installed under `/usr/lib/pop-launcher/plugins` shows up for free through the
//! service. But writing one means implementing the full stateful IPC protocol
//! (indices, activation, context menus, cancellation), which is far more than a
//! shell script should have to do.
//!
//! This host covers the other end of that range: a manifest plus an executable
//! that receives the query on argv and prints JSON. It is deliberately a subset
//! of Alfred's Script Filter format so existing workflow scripts mostly port by
//! changing the manifest.
//!
//! ## Layout
//!
//! ```text
//! ~/.local/share/jump/plugins/github/
//!   manifest.toml
//!   search.sh
//! ```
//!
//! ```toml
//! name = "GitHub"
//! description = "Search your repositories"
//! keyword = "gh"          # optional; omit to run on every query
//! query = "./search.sh"   # invoked as: query <text>
//! activate = "./open.sh"  # invoked as: activate <arg>
//! icon = "github"         # optional icon-theme name or path
//! timeout_ms = 1000       # optional; up to 3000 for slow keyworded plugins
//! ```
//!
//! Items may carry Alfred's `uid` (a stable identity, which is what feeds the
//! launcher's frecency ranking) and `autocomplete` (text that replaces the
//! query on Tab).
//!
//! ## Isolation
//!
//! A plugin is an arbitrary program on the interactive path, so it is held to a
//! hard deadline. Exceeding [`QUERY_TIMEOUT`] kills the process and drops its
//! results rather than delaying the frame — a broken plugin degrades its own
//! results and nothing else.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

use crate::model::{Icon, Item, ItemKey, Source};

/// A plugin that has not answered by this point is abandoned for this query.
///
/// Chosen to sit under a 100 ms interaction budget while leaving room for a
/// script that shells out to a local index. A plugin that genuinely needs
/// longer — a network search behind a keyword — raises it with `timeout_ms`,
/// up to [`MAX_QUERY_TIMEOUT`]; stale results are discarded by the frontend's
/// query guard, so a slow answer can be late without being wrong.
const QUERY_TIMEOUT: Duration = Duration::from_millis(180);

/// Hard ceiling on a manifest's `timeout_ms`.
///
/// Three seconds is generous for anything interactive; past it a plugin is not
/// answering a keystroke, it is running a job.
const MAX_QUERY_TIMEOUT: Duration = Duration::from_millis(3000);

/// Ceiling on a plugin's stdout, so a runaway script cannot exhaust memory.
const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// Parsed `manifest.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Human-readable name, shown as the result category.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// When set, the plugin only runs for queries starting with this word.
    /// When absent the plugin runs on every query and must be fast.
    #[serde(default)]
    pub keyword: Option<String>,
    /// Executable invoked with the query text as a single argument.
    pub query: String,
    /// Executable invoked with the selected item's `arg` on activation.
    /// Without it, activation falls back to the `query` program with
    /// `--activate`.
    #[serde(default)]
    pub activate: Option<String>,
    /// Icon-theme name or path used for items that do not specify their own.
    #[serde(default)]
    pub icon: Option<String>,
    /// Query deadline override in milliseconds, clamped to a hard ceiling.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Manifest {
    /// The deadline this plugin's query command runs under.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout_ms.map_or(QUERY_TIMEOUT, |ms| {
            Duration::from_millis(ms).clamp(Duration::from_millis(50), MAX_QUERY_TIMEOUT)
        })
    }
}

/// One item emitted by a plugin. A subset of Alfred's Script Filter schema.
#[derive(Debug, Clone, Deserialize)]
struct PluginItem {
    title: String,
    #[serde(default)]
    subtitle: String,
    /// Payload handed back on activation. Defaults to the title.
    #[serde(default)]
    arg: Option<String>,
    /// Icon-theme name or path. Alfred nests this under an object; both the
    /// bare string and `{ "path": "..." }` forms are accepted.
    #[serde(default)]
    icon: Option<PluginIcon>,
    /// Alfred marks unselectable rows with `valid: false`.
    #[serde(default = "default_valid")]
    valid: bool,
    /// Stable identity across queries, Alfred's `uid`. When present it keys
    /// the row — which is what lets frecency learn plugin items — and rows
    /// with the same uid keep their widget state across keystrokes.
    #[serde(default)]
    uid: Option<String>,
    /// Text that replaces the query on Tab, Alfred's `autocomplete`.
    #[serde(default)]
    autocomplete: Option<String>,
    /// State carried from query to activation, Alfred's `variables`: exported
    /// as environment variables to the activation command. Merged over the
    /// response's top-level `variables`, the item's own winning.
    #[serde(default)]
    variables: std::collections::HashMap<String, String>,
}

const fn default_valid() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PluginIcon {
    Named(String),
    Path {
        #[serde(alias = "name")]
        path: String,
    },
}

impl PluginIcon {
    fn into_icon(self) -> Icon {
        match self {
            Self::Named(name) | Self::Path { path: name } => Icon::Name(name),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PluginResponse {
    items: Vec<PluginItem>,
    /// Variables applied to every item, Alfred's top-level `variables`.
    #[serde(default)]
    variables: std::collections::HashMap<String, String>,
    /// Seconds after which the same query should run again, Alfred's `rerun`
    /// — how a progress-reporting or polling plugin streams updates.
    #[serde(default)]
    rerun: Option<f64>,
}

/// Bounds on a plugin's `rerun` interval. Alfred allows 0.1–5.0 s; the floor
/// here is higher because every rerun forks a process, and a launcher that
/// respawns scripts ten times a second shows up in power measurements.
const RERUN_RANGE: std::ops::RangeInclusive<f64> = 0.5..=5.0;

/// A completed plugin query: the items, plus how long until the plugins that
/// asked to be re-run should see the same query again.
#[derive(Debug, Clone, Default)]
pub struct QueryResults {
    pub items: Vec<Item>,
    /// Soonest requested rerun across the plugins that answered.
    pub rerun: Option<Duration>,
}

/// The item's variables over the response's, sorted for a stable identity —
/// the result lives inside [`Source::Plugin`], which is compared by `Eq`.
fn merge_variables(
    response: &std::collections::HashMap<String, String>,
    item: std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut merged = response.clone();
    merged.extend(item);
    let mut variables: Vec<(String, String)> = merged.into_iter().collect();
    variables.sort();
    variables
}

/// A discovered plugin: its manifest plus the directory it lives in.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    pub directory: PathBuf,
    /// Directory name, used as the stable plugin identifier.
    pub id: String,
}

impl Plugin {
    /// Resolve a manifest command against the plugin directory so `./search.sh`
    /// works regardless of the launcher's working directory.
    fn resolve(&self, command: &str) -> PathBuf {
        let path = Path::new(command);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.directory.join(path)
        }
    }

    /// Whether this plugin should run for `query`, and the text it should see.
    ///
    /// A keyworded plugin receives the query with its keyword stripped, matching
    /// Alfred: typing `gh jump` hands the plugin `jump`. The host passes the
    /// *effective* keyword, so a user override replaces the manifest's here.
    #[must_use]
    pub fn match_query_as<'a>(&self, keyword: Option<&str>, query: &'a str) -> Option<&'a str> {
        match keyword {
            None => Some(query),
            Some(keyword) => {
                let rest = query.strip_prefix(keyword)?;
                match rest.strip_prefix(' ') {
                    Some(rest) => Some(rest.trim_start()),
                    // Bare keyword with nothing after it: run with an empty
                    // query so the plugin can show a default list.
                    None if rest.is_empty() => Some(""),
                    None => None,
                }
            }
        }
    }

    /// [`Self::match_query_as`] with the manifest's own keyword.
    #[must_use]
    pub fn match_query<'a>(&self, query: &'a str) -> Option<&'a str> {
        self.match_query_as(self.manifest.keyword.as_deref(), query)
    }

    /// Run the plugin's query command and parse its results, plus the rerun
    /// interval it asked for, if any.
    async fn query(&self, text: &str) -> (Vec<Item>, Option<Duration>) {
        let program = self.resolve(&self.manifest.query);

        let child = Command::new(&program)
            .arg(text)
            .current_dir(&self.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();

        let deadline = self.manifest.timeout();
        let output = match tokio::time::timeout(deadline, child).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                tracing::warn!(plugin = %self.id, ?program, %error, "plugin failed to run");
                return (Vec::new(), None);
            }
            Err(_) => {
                tracing::warn!(
                    plugin = %self.id,
                    timeout_ms = deadline.as_millis(),
                    "plugin exceeded its deadline; dropping results"
                );
                return (Vec::new(), None);
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(plugin = %self.id, status = ?output.status, %stderr, "plugin exited non-zero");
            return (Vec::new(), None);
        }

        if output.stdout.len() > MAX_OUTPUT_BYTES {
            tracing::warn!(plugin = %self.id, bytes = output.stdout.len(), "plugin output too large");
            return (Vec::new(), None);
        }

        let response: PluginResponse = match serde_json::from_slice(&output.stdout) {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(plugin = %self.id, %error, "plugin emitted invalid JSON");
                return (Vec::new(), None);
            }
        };

        let fallback_icon = self.manifest.icon.as_ref().map(|n| Icon::Name(n.clone()));
        let PluginResponse {
            items,
            variables: response_variables,
            rerun,
        } = response;

        // NaN and absurd values from a script must not become a timer.
        let rerun = rerun.filter(|seconds| seconds.is_finite()).map(|seconds| {
            Duration::from_secs_f64(seconds.clamp(*RERUN_RANGE.start(), *RERUN_RANGE.end()))
        });

        let items = items
            .into_iter()
            .filter(|item| item.valid)
            .map(|item| {
                let arg = item.arg.unwrap_or_else(|| item.title.clone());
                let variables = merge_variables(&response_variables, item.variables);
                // Prefer the uid: an item whose arg changes with the query — a
                // search URL, say — would otherwise get a new identity every
                // keystroke, and frecency could never learn it.
                let identity = item.uid.as_deref().unwrap_or(&arg);
                Item {
                    // Namespaced by plugin id so two plugins returning the same
                    // title stay distinct rows.
                    key: ItemKey(format!("plugin:{}:{identity}", self.id)),
                    // Plugin items are not addressable by pop-launcher indice.
                    id: 0,
                    title: item.title,
                    subtitle: if item.subtitle.is_empty() {
                        self.manifest.description.clone()
                    } else {
                        item.subtitle
                    },
                    icon: item
                        .icon
                        .map(PluginIcon::into_icon)
                        .or_else(|| fallback_icon.clone()),
                    category_icon: fallback_icon.clone(),
                    window: None,
                    source: Source::Plugin {
                        plugin: self.id.clone(),
                        arg,
                        variables,
                    },
                    autocomplete: item.autocomplete,
                    score: 0.0,
                }
            })
            .collect();

        (items, rerun)
    }

    /// Run the plugin's activation command for `arg`.
    ///
    /// `variables` — the item's merged `variables`, Alfred-style — are
    /// exported into the command's environment, which is how a plugin carries
    /// state from query to activation without encoding it all into `arg`.
    ///
    /// Spawned detached: the launcher dismisses immediately and does not wait
    /// for the action to finish.
    pub fn activate(&self, arg: &str, variables: &[(String, String)]) {
        let (program, extra) = match self.manifest.activate.as_deref() {
            Some(activate) => (self.resolve(activate), Vec::new()),
            None => (
                self.resolve(&self.manifest.query),
                vec!["--activate".to_owned()],
            ),
        };

        let result = Command::new(&program)
            .envs(variables.iter().map(|(key, value)| (key, value)))
            .args(extra)
            .arg(arg)
            .current_dir(&self.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        match result {
            Ok(mut child) => {
                // Reap in the background so the child does not become a zombie.
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
            }
            Err(error) => {
                tracing::error!(plugin = %self.id, ?program, %error, "failed to activate plugin item");
            }
        }
    }
}

/// All discovered plugins.
#[derive(Debug, Default, Clone)]
pub struct PluginHost {
    plugins: Vec<Plugin>,
    /// Plugin ids the user has switched off. They stay discovered — the
    /// settings window needs to list them — but never run.
    disabled: std::collections::HashSet<String>,
    /// Per-plugin keyword overrides: the alias mechanism. An entry replaces
    /// the manifest's keyword without editing the plugin; an empty string
    /// removes the keyword, making the plugin run on every query.
    keyword_overrides: std::collections::HashMap<String, String>,
}

impl PluginHost {
    /// Discover plugins in the user's data directory and the system data
    /// directories (`$XDG_DATA_DIRS`, so distro packages under
    /// `/usr/share/jump/plugins` are picked up with no configuration).
    ///
    /// The user's directory is searched first and wins on an id collision, so
    /// copying a packaged plugin into `~/.local/share/jump/plugins` to modify
    /// it shadows the packaged one rather than duplicating it.
    #[must_use]
    pub fn discover() -> Self {
        let mut roots: Vec<PathBuf> = dirs::data_dir()
            .map(|dir| dir.join("jump").join("plugins"))
            .into_iter()
            .collect();

        let system = std::env::var_os("XDG_DATA_DIRS")
            .filter(|value| !value.is_empty())
            .map_or_else(
                || {
                    vec![
                        PathBuf::from("/usr/local/share"),
                        PathBuf::from("/usr/share"),
                    ]
                },
                |value| std::env::split_paths(&value).collect(),
            );
        roots.extend(
            system
                .into_iter()
                .map(|dir| dir.join("jump").join("plugins")),
        );

        Self::discover_in(&roots)
    }

    /// Discover plugins under `roots`, earlier roots shadowing later ones.
    ///
    /// Directories without a readable manifest are skipped with a warning; a
    /// malformed plugin must never prevent the launcher from starting.
    #[must_use]
    pub fn discover_in(roots: &[PathBuf]) -> Self {
        let mut plugins: Vec<Plugin> = Vec::new();

        for root in roots {
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%error, ?root, "failed to read plugin directory");
                    continue;
                }
            };

            for entry in entries.flatten() {
                let directory = entry.path();
                if !directory.is_dir() {
                    continue;
                }

                let Some(id) = directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };

                if plugins.iter().any(|plugin| plugin.id == id) {
                    tracing::debug!(plugin = %id, ?directory, "shadowed by an earlier root");
                    continue;
                }

                let manifest_path = directory.join("manifest.toml");
                let contents = match std::fs::read_to_string(&manifest_path) {
                    Ok(contents) => contents,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        tracing::warn!(%error, ?manifest_path, "failed to read plugin manifest");
                        continue;
                    }
                };

                let manifest: Manifest = match toml::from_str(&contents) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        tracing::warn!(%error, ?manifest_path, "invalid plugin manifest");
                        continue;
                    }
                };

                tracing::info!(plugin = %id, keyword = ?manifest.keyword, "loaded plugin");
                plugins.push(Plugin {
                    manifest,
                    directory,
                    id,
                });
            }
        }

        Self {
            plugins,
            disabled: std::collections::HashSet::new(),
            keyword_overrides: std::collections::HashMap::new(),
        }
    }

    /// Replace the set of switched-off plugins, e.g. when settings change.
    pub fn set_disabled<I, S>(&mut self, ids: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disabled = ids.into_iter().map(Into::into).collect();
    }

    /// Replace the keyword overrides, e.g. when settings change.
    pub fn set_keyword_overrides<I, K, V>(&mut self, overrides: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.keyword_overrides = overrides
            .into_iter()
            .map(|(id, keyword)| (id.into(), keyword.into()))
            .collect();
    }

    /// The keyword `plugin` currently answers to: the user's override when
    /// one exists (empty meaning "no keyword"), else the manifest's.
    #[must_use]
    pub fn effective_keyword<'a>(&'a self, plugin: &'a Plugin) -> Option<&'a str> {
        match self.keyword_overrides.get(&plugin.id) {
            Some(keyword) if keyword.is_empty() => None,
            Some(keyword) => Some(keyword.as_str()),
            None => plugin.manifest.keyword.as_deref(),
        }
    }

    /// Every discovered plugin, the switched-off ones included, with whether
    /// each is currently enabled. This is the settings window's list.
    pub fn all(&self) -> impl Iterator<Item = (&Plugin, bool)> {
        self.plugins
            .iter()
            .map(|plugin| (plugin, !self.disabled.contains(&plugin.id)))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Look up an *enabled* plugin. Activation goes through here, so a plugin
    /// disabled between query and click does nothing rather than running.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .filter(|plugin| !self.disabled.contains(&plugin.id))
            .find(|plugin| plugin.id == id)
    }

    /// Query every plugin that matches, concurrently.
    ///
    /// Plugins run in parallel and share the same deadline, so total latency is
    /// the slowest matching plugin rather than the sum.
    pub async fn query(&self, text: &str) -> QueryResults {
        let matching: Vec<_> = self
            .plugins
            .iter()
            .filter(|plugin| !self.disabled.contains(&plugin.id))
            .filter_map(|plugin| {
                plugin
                    .match_query_as(self.effective_keyword(plugin), text)
                    .map(|query| (plugin, query))
            })
            .collect();

        if matching.is_empty() {
            return QueryResults::default();
        }

        // A keyworded plugin takes over the query entirely: once the user types
        // `gh …` they are addressing that plugin, not searching their apps.
        let keyworded: Vec<_> = matching
            .iter()
            .filter(|(plugin, _)| self.effective_keyword(plugin).is_some())
            .collect();

        let selected: Vec<_> = if keyworded.is_empty() {
            matching.iter().collect()
        } else {
            keyworded
        };

        let futures = selected
            .into_iter()
            .map(|(plugin, query)| plugin.query(query));

        let mut results = QueryResults::default();
        for (items, rerun) in futures::future::join_all(futures).await {
            results.items.extend(items);
            // The soonest rerun wins: a plugin that asked to stream must not
            // be held to a slower neighbour's interval.
            results.rerun = match (results.rerun, rerun) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        results
    }

    /// Whether a keyworded plugin claims this query, meaning pop-launcher
    /// results should be suppressed.
    #[must_use]
    pub fn is_claimed(&self, text: &str) -> bool {
        self.plugins
            .iter()
            .filter(|plugin| !self.disabled.contains(&plugin.id))
            .any(|plugin| {
                let keyword = self.effective_keyword(plugin);
                keyword.is_some() && plugin.match_query_as(keyword, text).is_some()
            })
    }
}

/// What `lint` found. Errors stop the plugin working; warnings do not.
#[derive(Debug, Default)]
pub struct LintReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Items the sample query produced, when it ran at all.
    pub items: usize,
}

impl LintReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Check a plugin directory: the manifest, the commands it names, and the
/// output of one sample query against the item schema.
///
/// This *runs* the plugin's query command — that is the point: the schema
/// violations that break a plugin live in its output, not its manifest.
pub async fn lint(directory: &Path, sample_query: &str) -> LintReport {
    let mut report = LintReport::default();

    let manifest_path = directory.join("manifest.toml");
    let contents = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(error) => {
            report
                .errors
                .push(format!("cannot read {}: {error}", manifest_path.display()));
            return report;
        }
    };

    let manifest: Manifest = match toml::from_str(&contents) {
        Ok(manifest) => manifest,
        Err(error) => {
            report
                .errors
                .push(format!("manifest does not parse: {error}"));
            return report;
        }
    };

    if let Some(keyword) = manifest.keyword.as_deref() {
        if keyword.is_empty() || keyword.contains(char::is_whitespace) {
            report.errors.push(format!(
                "keyword {keyword:?} must be one non-empty word — it is matched \
                 up to the first space of the query"
            ));
        }
    } else {
        report.warnings.push(
            "no keyword: the plugin runs on every keystroke and must answer \
             well inside 180 ms"
                .to_owned(),
        );
    }

    if let Some(ms) = manifest.timeout_ms {
        let clamped = manifest.timeout().as_millis();
        if u128::from(ms) != clamped {
            report
                .warnings
                .push(format!("timeout_ms = {ms} is clamped to {clamped} ms"));
        }
    }

    let check_command = |report: &mut LintReport, label: &str, command: &str| {
        let path = if Path::new(command).is_absolute() {
            PathBuf::from(command)
        } else {
            directory.join(command)
        };
        if !path.is_file() {
            report
                .errors
                .push(format!("{label} command {} does not exist", path.display()));
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = path
                .metadata()
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            if !executable {
                report.errors.push(format!(
                    "{label} command {} is not executable (chmod +x)",
                    path.display()
                ));
            }
        }
    };
    check_command(&mut report, "query", &manifest.query);
    if let Some(activate) = manifest.activate.as_deref() {
        check_command(&mut report, "activate", activate);
    }
    if !report.errors.is_empty() {
        return report;
    }

    // Run the sample query exactly the way the launcher would.
    let plugin = Plugin {
        manifest,
        directory: directory.to_path_buf(),
        id: directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plugin")
            .to_owned(),
    };
    let deadline = plugin.manifest.timeout();
    let program = plugin.directory.join(&plugin.manifest.query);
    let output = match tokio::time::timeout(
        deadline,
        Command::new(&program)
            .arg(sample_query)
            .current_dir(&plugin.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            report
                .errors
                .push(format!("query command failed to run: {error}"));
            return report;
        }
        Err(_) => {
            report.errors.push(format!(
                "query command exceeded its {} ms deadline and was killed",
                deadline.as_millis()
            ));
            return report;
        }
    };

    if !output.status.success() {
        report.errors.push(format!(
            "query command exited {:?}; stderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return report;
    }

    match serde_json::from_slice::<PluginResponse>(&output.stdout) {
        Ok(response) => {
            report.items = response.items.len();
            if response.items.is_empty() {
                report
                    .warnings
                    .push("sample query produced no items".to_owned());
            }
            for item in &response.items {
                if item.uid.is_none() {
                    report.warnings.push(format!(
                        "item {:?} has no uid — without one, usage ranking \
                         cannot learn it",
                        item.title
                    ));
                }
            }
        }
        Err(error) => {
            report
                .errors
                .push(format!("output is not valid script-filter JSON: {error}"));
        }
    }

    report
}

/// Write a runnable plugin skeleton into `directory`.
///
/// Refuses to touch a directory that already exists — a scaffolder that can
/// overwrite a real plugin is a footgun, not a convenience.
pub fn scaffold(directory: &Path, name: &str) -> std::io::Result<()> {
    if directory.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", directory.display()),
        ));
    }
    std::fs::create_dir_all(directory)?;

    let keyword = name.to_lowercase().replace(char::is_whitespace, "-");
    std::fs::write(
        directory.join("manifest.toml"),
        format!(
            "name = \"{name}\"\n\
             description = \"Describe what this plugin searches\"\n\
             keyword = \"{keyword}\"\n\
             query = \"./search.sh\"\n\
             # activate = \"./open.sh\"   # separate action command, optional\n\
             icon = \"system-search-symbolic\"\n\
             # timeout_ms = 1000          # up to 3000 for slow keyworded plugins\n"
        ),
    )?;

    let script = directory.join("search.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             # jump runs this with the query as $1 and reads script-filter\n\
             # JSON from stdout. Without an `activate` command in the\n\
             # manifest, activation re-runs this script as: search.sh\n\
             # --activate <arg>.\n\
             if [ \"$1\" = \"--activate\" ]; then\n\
             \t# \"$2\" is the chosen item's arg.\n\
             \texit 0\n\
             fi\n\
             \n\
             query=\"$1\"\n\
             printf '{{\"items\":[{{\"uid\":\"hello\",\"title\":\"Hello %s\",\
             \"subtitle\":\"{name}\",\"arg\":\"%s\"}}]}}' \
             \"${{query:-world}}\" \"$query\"\n"
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(keyword: Option<&str>) -> Plugin {
        Plugin {
            manifest: Manifest {
                name: "Test".to_owned(),
                description: String::new(),
                keyword: keyword.map(ToOwned::to_owned),
                query: "./q".to_owned(),
                activate: None,
                icon: None,
                timeout_ms: None,
            },
            directory: PathBuf::from("/tmp"),
            id: "test".to_owned(),
        }
    }

    #[test]
    fn keywordless_plugin_sees_whole_query() {
        assert_eq!(plugin(None).match_query("hello there"), Some("hello there"));
    }

    #[test]
    fn keyword_is_stripped() {
        assert_eq!(plugin(Some("gh")).match_query("gh jump"), Some("jump"));
    }

    #[test]
    fn bare_keyword_yields_empty_query() {
        assert_eq!(plugin(Some("gh")).match_query("gh"), Some(""));
    }

    #[test]
    fn keyword_requires_word_boundary() {
        // "ghost" must not be read as the "gh" keyword plus "ost".
        assert_eq!(plugin(Some("gh")).match_query("ghost"), None);
    }

    #[test]
    fn non_matching_keyword_is_skipped() {
        assert_eq!(plugin(Some("gh")).match_query("firefox"), None);
    }

    #[test]
    fn timeout_is_clamped_to_the_ceiling() {
        let mut manifest = plugin(None).manifest;
        assert_eq!(manifest.timeout(), Duration::from_millis(180));

        manifest.timeout_ms = Some(60_000);
        assert_eq!(manifest.timeout(), Duration::from_millis(3000));

        manifest.timeout_ms = Some(1);
        assert_eq!(manifest.timeout(), Duration::from_millis(50));
    }

    #[test]
    fn uid_and_autocomplete_parse_from_alfred_json() {
        let response: PluginResponse = serde_json::from_str(
            r#"{"items":[
                {"uid":"repo","title":"jump","arg":"https://example.com/?q=jump",
                 "autocomplete":"gh jump ", "icon":{"path":"/tmp/icon.png"}},
                {"title":"plain"}
            ]}"#,
        )
        .expect("alfred json parses");

        assert_eq!(response.items[0].uid.as_deref(), Some("repo"));
        assert_eq!(response.items[0].autocomplete.as_deref(), Some("gh jump "));
        assert_eq!(response.items[1].uid, None);
    }

    #[test]
    fn rerun_parses_and_is_clamped_sanely() {
        let parse = |json: &str| -> Option<f64> {
            serde_json::from_str::<PluginResponse>(json)
                .expect("json parses")
                .rerun
        };
        assert_eq!(parse(r#"{"items":[]}"#), None);
        assert_eq!(parse(r#"{"items":[],"rerun":1.5}"#), Some(1.5));

        // The clamp applied where the value becomes a timer.
        let clamp = |seconds: f64| seconds.clamp(*RERUN_RANGE.start(), *RERUN_RANGE.end());
        assert_eq!(clamp(0.1), 0.5);
        assert_eq!(clamp(60.0), 5.0);
        assert_eq!(clamp(2.0), 2.0);
    }

    #[test]
    fn item_variables_win_over_response_variables() {
        let response: PluginResponse = serde_json::from_str(
            r#"{"variables":{"SESSION":"abc","MODE":"list"},
                "items":[{"title":"one","variables":{"MODE":"open"}},
                         {"title":"two"}]}"#,
        )
        .expect("alfred json parses");

        let merged = merge_variables(&response.variables, response.items[0].variables.clone());
        assert_eq!(
            merged,
            vec![
                ("MODE".to_owned(), "open".to_owned()),
                ("SESSION".to_owned(), "abc".to_owned()),
            ]
        );

        // An item without its own variables inherits the response's.
        let inherited = merge_variables(&response.variables, response.items[1].variables.clone());
        assert_eq!(inherited.len(), 2);
        assert!(inherited.contains(&("MODE".to_owned(), "list".to_owned())));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scaffold_produces_a_plugin_that_lints_clean() {
        let root = tempfile::tempdir().expect("tempdir");
        let directory = root.path().join("demo");

        scaffold(&directory, "Demo").expect("scaffold succeeds");
        // Refuses to overwrite what it just made.
        assert!(scaffold(&directory, "Demo").is_err());

        let report = lint(&directory, "hello").await;
        assert!(report.is_clean(), "unexpected errors: {:?}", report.errors);
        assert_eq!(report.items, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lint_reports_missing_and_broken_pieces() {
        let root = tempfile::tempdir().expect("tempdir");

        // No manifest at all.
        let report = lint(root.path(), "x").await;
        assert!(!report.is_clean());

        // A manifest naming a command that does not exist.
        let directory = root.path().join("broken");
        std::fs::create_dir_all(&directory).expect("dir");
        std::fs::write(
            directory.join("manifest.toml"),
            "name = \"Broken\"\nkeyword = \"two words\"\nquery = \"./missing.sh\"\n",
        )
        .expect("manifest");
        let report = lint(&directory, "x").await;
        assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
    }

    #[test]
    fn discovery_walks_roots_in_order_and_user_shadows_system() {
        let user = tempfile::tempdir().expect("tempdir");
        let system = tempfile::tempdir().expect("tempdir");

        let write = |root: &std::path::Path, id: &str, name: &str| {
            let dir = root.join(id);
            std::fs::create_dir_all(&dir).expect("plugin dir");
            std::fs::write(
                dir.join("manifest.toml"),
                format!("name = \"{name}\"\nquery = \"./q\"\n"),
            )
            .expect("manifest");
        };

        write(user.path(), "shared", "User copy");
        write(user.path(), "mine", "User only");
        write(system.path(), "shared", "System copy");
        write(system.path(), "packaged", "System only");
        // A directory without a manifest is not a plugin.
        std::fs::create_dir_all(system.path().join("junk")).expect("junk dir");

        let host =
            PluginHost::discover_in(&[user.path().to_path_buf(), system.path().to_path_buf()]);

        let mut ids: Vec<&str> = host.all().map(|(plugin, _)| plugin.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["mine", "packaged", "shared"]);

        // The user's copy shadowed the packaged one.
        let shared = host.get("shared").expect("shared plugin");
        assert_eq!(shared.manifest.name, "User copy");
    }

    #[test]
    fn keyword_overrides_replace_and_remove_the_manifest_keyword() {
        let mut host = PluginHost {
            plugins: vec![plugin(Some("gh"))],
            disabled: std::collections::HashSet::new(),
            keyword_overrides: std::collections::HashMap::new(),
        };

        // Manifest keyword applies untouched.
        assert!(host.is_claimed("gh jump"));

        // An override is a rename: the old keyword stops answering.
        host.set_keyword_overrides([("test", "g")]);
        assert!(host.is_claimed("g jump"));
        assert!(!host.is_claimed("gh jump"));

        // An empty override removes the keyword entirely: the plugin runs on
        // every query and claims none.
        host.set_keyword_overrides([("test", "")]);
        assert!(!host.is_claimed("g jump"));
        assert!(!host.is_claimed("gh jump"));
        let plugin = &host.plugins[0];
        assert_eq!(host.effective_keyword(plugin), None);

        // Overrides for unknown plugins are inert.
        host.set_keyword_overrides([("other", "x")]);
        assert!(host.is_claimed("gh jump"));
    }

    #[test]
    fn disabled_plugins_neither_run_nor_activate() {
        let mut host = PluginHost {
            plugins: vec![plugin(Some("gh"))],
            disabled: std::collections::HashSet::new(),
            keyword_overrides: std::collections::HashMap::new(),
        };

        assert!(host.is_claimed("gh jump"));
        assert!(host.get("test").is_some());

        host.set_disabled(["test"]);
        assert!(!host.is_claimed("gh jump"));
        assert!(host.get("test").is_none());
        // Still listed for the settings window, marked disabled.
        assert_eq!(host.all().map(|(_, on)| on).collect::<Vec<_>>(), [false]);
    }
}
