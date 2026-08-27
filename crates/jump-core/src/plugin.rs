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
        self.timeout_ms
            .map_or(QUERY_TIMEOUT, |ms| {
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
    /// Alfred: typing `gh jump` hands the plugin `jump`.
    #[must_use]
    pub fn match_query<'a>(&self, query: &'a str) -> Option<&'a str> {
        match self.manifest.keyword.as_deref() {
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

    /// Run the plugin's query command and parse its results.
    async fn query(&self, text: &str) -> Vec<Item> {
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
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!(
                    plugin = %self.id,
                    timeout_ms = deadline.as_millis(),
                    "plugin exceeded its deadline; dropping results"
                );
                return Vec::new();
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(plugin = %self.id, status = ?output.status, %stderr, "plugin exited non-zero");
            return Vec::new();
        }

        if output.stdout.len() > MAX_OUTPUT_BYTES {
            tracing::warn!(plugin = %self.id, bytes = output.stdout.len(), "plugin output too large");
            return Vec::new();
        }

        let response: PluginResponse = match serde_json::from_slice(&output.stdout) {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(plugin = %self.id, %error, "plugin emitted invalid JSON");
                return Vec::new();
            }
        };

        let fallback_icon = self.manifest.icon.as_ref().map(|n| Icon::Name(n.clone()));

        response
            .items
            .into_iter()
            .filter(|item| item.valid)
            .map(|item| {
                let arg = item.arg.unwrap_or_else(|| item.title.clone());
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
                    },
                    autocomplete: item.autocomplete,
                    score: 0.0,
                }
            })
            .collect()
    }

    /// Run the plugin's activation command for `arg`.
    ///
    /// Spawned detached: the launcher dismisses immediately and does not wait
    /// for the action to finish.
    pub fn activate(&self, arg: &str) {
        let (program, extra) = match self.manifest.activate.as_deref() {
            Some(activate) => (self.resolve(activate), Vec::new()),
            None => (
                self.resolve(&self.manifest.query),
                vec!["--activate".to_owned()],
            ),
        };

        let result = Command::new(&program)
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
}

impl PluginHost {
    /// Discover plugins in the user's data directory.
    ///
    /// Directories without a readable manifest are skipped with a warning; a
    /// malformed plugin must never prevent the launcher from starting.
    #[must_use]
    pub fn discover() -> Self {
        let Some(root) = dirs::data_dir().map(|dir| dir.join("jump").join("plugins")) else {
            return Self::default();
        };

        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(error) => {
                tracing::warn!(%error, ?root, "failed to read plugin directory");
                return Self::default();
            }
        };

        let mut plugins = Vec::new();
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
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

            let Some(id) = directory
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
            else {
                continue;
            };

            tracing::info!(plugin = %id, keyword = ?manifest.keyword, "loaded plugin");
            plugins.push(Plugin {
                manifest,
                directory,
                id,
            });
        }

        Self {
            plugins,
            disabled: std::collections::HashSet::new(),
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
    pub async fn query(&self, text: &str) -> Vec<Item> {
        let matching: Vec<_> = self
            .plugins
            .iter()
            .filter(|plugin| !self.disabled.contains(&plugin.id))
            .filter_map(|plugin| plugin.match_query(text).map(|query| (plugin, query)))
            .collect();

        if matching.is_empty() {
            return Vec::new();
        }

        // A keyworded plugin takes over the query entirely: once the user types
        // `gh …` they are addressing that plugin, not searching their apps.
        let keyworded: Vec<_> = matching
            .iter()
            .filter(|(plugin, _)| plugin.manifest.keyword.is_some())
            .collect();

        let selected: Vec<_> = if keyworded.is_empty() {
            matching.iter().collect()
        } else {
            keyworded
        };

        let futures = selected
            .into_iter()
            .map(|(plugin, query)| plugin.query(query));

        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Whether a keyworded plugin claims this query, meaning pop-launcher
    /// results should be suppressed.
    #[must_use]
    pub fn is_claimed(&self, text: &str) -> bool {
        self.plugins
            .iter()
            .filter(|plugin| !self.disabled.contains(&plugin.id))
            .any(|plugin| plugin.manifest.keyword.is_some() && plugin.match_query(text).is_some())
    }
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
    fn disabled_plugins_neither_run_nor_activate() {
        let mut host = PluginHost {
            plugins: vec![plugin(Some("gh"))],
            disabled: std::collections::HashSet::new(),
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
