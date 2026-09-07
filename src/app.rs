// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Application state and the update loop.
//!
//! `jump` runs as a daemon rather than a process spawned per keypress. The
//! expensive parts of opening a launcher — spawning the pop-launcher service,
//! building its desktop-entry cache, discovering plugins, creating a wgpu
//! device — all happen once at startup. A second invocation reaches the running
//! instance over D-Bus (the `single-instance` feature) and only has to map a
//! surface, which is the difference between a launcher that appears instantly
//! and one that appears eventually.

use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::platform_specific::shell::commands::activation::request_token;
use cosmic::iced::{Size, Subscription, event, keyboard, mouse, window};
use cosmic::widget::{icon, text_input};
use jump_core::{Content, Files, Frecency, Item, Launcher, PluginHost, Source};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::apps::{self, App as AppEntry};
use crate::clipboard::{self, Clipboard, Entry};
use crate::launch::Launch;
use crate::toplevel::{self, Toplevels, Window};
use crate::tray;
use jump::config::Config;

use crate::actions;
use crate::anim::Panel;
use crate::surface::{self, GridMetrics, Mode};
use crate::system;
use crate::view;
use jump::fl;

/// Identifier for the query input, so focus can be restored on every open.
pub const INPUT_ID: &str = "jump-query";

/// Startup flags.
///
/// `run_single_instance` requires this to implement [`CosmicFlags`] because it
/// forwards a second invocation's argv to the running daemon: a bare `jump`
/// arrives as a plain activation (the toggle gesture), and `jump show <query>`
/// arrives as an `ActivateAction` carrying the query.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    /// Start without showing the overlay.
    ///
    /// Autostart runs the daemon at login purely to keep it warm; mapping the
    /// launcher over the user's session the moment they log in would be
    /// actively hostile. A later invocation with no flag toggles it.
    pub daemon: bool,
    /// Subcommand forwarded to the running daemon — `show`, today.
    pub action: Option<String>,
    /// The subcommand's arguments; for `show`, the query to open with.
    pub args: Vec<String>,
}

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;

    fn action(&self) -> Option<&String> {
        self.action.as_ref()
    }

    fn args(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }
}

/// The `jump show <query>` subcommand: open with the query pre-filled.
///
/// This is what makes per-command hotkeys possible without owning any global
/// keybinding state — COSMIC's own custom shortcuts can bind `jump show clip`
/// and land straight in clipboard history.
pub const ACTION_SHOW: &str = "show";

#[derive(Debug, Clone)]
pub enum Message {
    /// The pop-launcher bridge came up and handed back its write handle.
    LauncherReady(Launcher),
    /// Something arrived from the pop-launcher service.
    Launcher(jump_core::Event),
    /// Plugin results for `query`, which may already be stale. `rerun` is how
    /// soon the plugins that asked to stream want the same query again.
    PluginResults {
        query: String,
        items: Vec<Item>,
        rerun: Option<Duration>,
    },
    /// A plugin's `rerun` timer fired: run `query` against the plugins again
    /// if the user is still looking at it.
    RerunPlugins(String),
    /// File-search results for `query`, which may already be stale.
    FileResults { query: String, items: Vec<Item> },
    /// The file index finished (re)building.
    IndexReady(Files),
    /// The content index is open and ready to query.
    ContentReady(Arc<Mutex<Content>>),
    /// Full-text results for `query`, which may already be stale.
    ContentResults { query: String, items: Vec<Item> },
    /// The compositor's window list changed.
    Windows(Vec<Window>),
    /// The current MPRIS track, fetched when the overlay opens.
    NowPlaying(Option<String>),
    /// A calculator answer for `query`, which may already be stale. `None`
    /// when qalc could not evaluate the expression.
    CalcResult {
        query: String,
        answer: Option<String>,
    },
    /// Bluetooth devices and Wi-Fi networks, snapshotted when the overlay
    /// opens — see [`crate::devices`] for why it is a snapshot.
    Devices(Vec<crate::devices::Device>),
    /// The window-switcher backend came up.
    ToplevelsReady(Toplevels),
    /// Clipboard history changed.
    Clips(Vec<Entry>),
    /// The user chose something from the status-area menu.
    Tray(tray::Action),
    /// Settings changed on disk and were re-read.
    ConfigChanged(Config),
    /// The pointer wheel moved. Positive is down or right, i.e. toward later
    /// pages, matching how the backend reports both axes.
    Scrolled(f32),
    /// The clipboard backend came up.
    ClipboardReady(Clipboard),
    /// The query text changed.
    InputChanged(String),
    /// Move the highlighted row by a signed offset, saturating at the ends.
    MoveSelection(i32),
    /// Move the selection a whole Launchpad page, turning the page with it.
    ///
    /// Separate from [`Message::MoveSelection`], which interprets its argument
    /// as an arrow-key direction and normalises the magnitude away — passing a
    /// page-sized delta there moved the highlight by one cell.
    MovePage(i32),
    /// Highlight a specific row, e.g. from a mouse hover.
    Select(usize),
    /// Run the highlighted result.
    Activate,
    /// Run a specific row, e.g. from a click.
    ActivateAt(usize),
    /// Run the highlighted result's alternate for this modifier, Alfred's
    /// `mods`. Falls back to plain activation when the item has none.
    ActivateMod(String),
    /// The compositor answered our activation-token request; start the
    /// application the token was requested for.
    Launch(Launch, Option<String>),
    /// Tab-complete from the highlighted result.
    Complete,
    /// Open or close the action panel for the highlighted result.
    ToggleActions,
    /// Highlight a specific action, e.g. from a mouse hover.
    SelectAction(usize),
    /// Run a specific action, e.g. from a click.
    RunAction(usize),
    /// Close the highlighted window, if the selection is one.
    CloseSelected,
    /// Begin dismissing. The surface outlives this until the fade finishes.
    Dismiss,
    /// Show the launcher, or dismiss it if already open.
    Toggle,
    /// A frame callback while animating.
    Frame(Instant),
    /// The overlay surface was configured at this size.
    Configured(Size),
    /// Nothing to do. Used to discard the result of infallible platform tasks.
    None,
}

pub struct App {
    core: Core,
    /// Present only while the overlay is mapped.
    surface: Option<window::Id>,
    /// Size of the overlay surface, i.e. the output. Drives panel placement.
    screen: Size,

    input: String,
    results: Vec<Item>,
    /// Icon handle per entry in `results`, resolved when the results are
    /// installed rather than while drawing.
    ///
    /// Resolving a name means a freedesktop icon-theme lookup, which walks the
    /// theme search path and, on a miss, retries against progressively shorter
    /// prefixes of the name. Doing that in the view would repeat it for every
    /// visible row on every frame — and the open transition redraws at the
    /// display's refresh rate.
    result_icons: Vec<icon::Handle>,
    /// Every installed application, for Launchpad mode. Loaded once at startup.
    apps: Vec<AppEntry>,
    /// Live window list from the compositor.
    windows: Vec<Window>,
    /// `None` when the compositor does not implement the toplevel protocols.
    toplevels: Option<Toplevels>,
    /// `None` when file search is disabled in configuration.
    files: Option<Files>,
    /// Full-text index over file contents; `None` unless enabled.
    ///
    /// Shared behind a mutex because indexing mutates it from a background task
    /// while queries read it — SQLite handles the concurrency, but the handle
    /// itself is not `Sync`.
    content: Option<Arc<Mutex<Content>>>,
    /// Clipboard history, newest first.
    clips: Vec<Entry>,
    /// What the most relevant player is playing, fetched when the overlay
    /// opens. A snapshot, not a subscription: it captions the media commands
    /// for the seconds the overlay is up, and re-fetching per open is cheaper
    /// than tracking every player's property changes all session.
    now_playing: Option<String>,
    /// Connectable Bluetooth devices and saved Wi-Fi networks, snapshotted
    /// per open for the same reason.
    devices: Vec<crate::devices::Device>,
    /// `None` when the compositor does not implement wlr-data-control.
    clipboard: Option<Clipboard>,
    selected: usize,
    /// The action panel, open on the selected result. Closed by Escape, by
    /// typing, and by anything that changes what is selected.
    actions: Option<actions::Panel>,

    /// `None` until the bridge finishes starting.
    launcher: Option<Launcher>,
    frecency: Frecency,
    plugins: PluginHost,

    config: Config,
    panel: Panel,
    /// Set while the close transition plays, so input is ignored on the way out.
    dismissing: bool,
    /// Wheel travel accumulated since the last page turn.
    ///
    /// A touchpad emits a stream of small deltas, so acting on each one would
    /// fly through every page at the lightest gesture. Travel is accumulated and
    /// spent a page at a time.
    scroll_travel: f32,
    /// Whether the blur region has been sent since the surface last drew.
    ///
    /// The region is double-buffered surface-local state, so it only takes
    /// effect against a committed buffer. Sending it during `Configured` — before
    /// anything has rendered — is silently dropped. Search mode hid this because
    /// arriving results re-sent the region after the first frame; Launchpad with
    /// an empty query never updates, so it never got a second chance and showed
    /// no frost at all.
    blur_settled: bool,
    /// The surface has been requested but has not been configured yet.
    ///
    /// The open transition is started when this clears rather than when the
    /// surface is requested: creating a layer surface, negotiating a buffer, and
    /// getting the first frame callback takes long enough that a 220 ms
    /// transition started at request time is over ~98% of the way through before
    /// anything is on screen. Measured, not assumed.
    awaiting_first_configure: bool,
}

impl App {
    /// What the overlay is showing.
    ///
    /// Derived from the query rather than stored: an empty query *is* Launchpad
    /// mode, so keeping a separate field would just be a second source of truth
    /// that could disagree with the input.
    fn mode(&self) -> Mode {
        if self.input.is_empty() {
            Mode::Grid
        } else {
            Mode::Search
        }
    }

    /// Index of the Launchpad page the selection currently sits on.
    ///
    /// Derived from the selection rather than stored: moving the highlight past
    /// the edge of a page *is* turning the page, so a separate page field would
    /// only be able to disagree with it.
    fn page(&self) -> usize {
        if !self.config.scroll.is_paged() {
            return 0;
        }
        self.selected / self.metrics().page_size()
    }

    /// Grid geometry for the current display and configuration.
    fn metrics(&self) -> GridMetrics {
        GridMetrics::resolve(self.screen, &self.config)
    }

    /// Number of selectable entries in the current mode.
    fn selectable(&self) -> usize {
        match self.mode() {
            Mode::Search => self.results.len(),
            Mode::Grid => self.apps.len(),
        }
    }

    /// Clipboard entries for a `clip …` query.
    ///
    /// Behind a keyword rather than always-on: history contains passwords and
    /// tokens, and surfacing those next to application results — where a stray
    /// Enter would paste them somewhere — is a bad default.
    fn matching_clips(&self, query: &str) -> Vec<Item> {
        if !self.config.providers.clipboard {
            return Vec::new();
        }
        let Some(rest) = query.strip_prefix(CLIP_KEYWORD) else {
            return Vec::new();
        };
        let needle = match rest.strip_prefix(' ') {
            Some(rest) => rest.trim_start().to_lowercase(),
            None if rest.is_empty() => String::new(),
            None => return Vec::new(),
        };

        self.clips
            .iter()
            .filter(|entry| needle.is_empty() || entry.text.to_lowercase().contains(&needle))
            .take(12)
            .map(|entry| Item {
                key: jump_core::ItemKey(format!("clip:{}", entry.text)),
                id: 0,
                title: entry.preview(),
                subtitle: entry.describe(),
                icon: Some(jump_core::Icon::Name("edit-paste-symbolic".to_owned())),
                category_icon: Some(jump_core::Icon::Name("edit-paste-symbolic".to_owned())),
                window: None,
                source: Source::Clipboard {
                    text: entry.text.clone(),
                },
                autocomplete: None,
                score: 0.0,
            })
            .collect()
    }

    /// Quicklink rows for a query one of the configured keywords claims.
    ///
    /// Several links may share a keyword; all of them answer, and the user
    /// picks. The URL is expanded here so activation is a plain open.
    fn matching_quicklinks(&self, query: &str) -> Vec<Item> {
        if !self.config.providers.web {
            return Vec::new();
        }
        self.config
            .quicklinks
            .iter()
            .filter_map(|link| {
                let rest = link.match_query(query)?;
                let url = link.url_for(rest);
                Some(Item {
                    key: jump_core::ItemKey(format!("quicklink:{}", link.name)),
                    id: 0,
                    title: if rest.is_empty() {
                        link.name.clone()
                    } else {
                        fl!("web-search-for", name = link.name.as_str(), query = rest)
                    },
                    // Where Enter goes, so a typo in the template is visible
                    // before it is opened.
                    subtitle: url.clone(),
                    icon: Some(jump_core::Icon::Name("web-browser-symbolic".to_owned())),
                    category_icon: Some(jump_core::Icon::Name("web-browser-symbolic".to_owned())),
                    window: None,
                    source: Source::Url { url },
                    autocomplete: None,
                    score: 1.0,
                })
            })
            .collect()
    }

    /// Fallback web searches, appended below an ordinary search's results.
    ///
    /// Appended rather than merged: they are not an answer to the query, they
    /// are where to go when the answers were not it, so they belong at the
    /// bottom in configuration order regardless of scores.
    fn fallback_items(&self, query: &str) -> Vec<Item> {
        if !self.config.providers.web {
            return Vec::new();
        }
        self.config
            .fallbacks
            .iter()
            .map(|link| Item {
                key: jump_core::ItemKey(format!("fallback:{}", link.name)),
                id: 0,
                title: fl!("web-search-for", name = link.name.as_str(), query = query),
                subtitle: fl!("web-search-subtitle"),
                icon: Some(jump_core::Icon::Name("web-browser-symbolic".to_owned())),
                category_icon: Some(jump_core::Icon::Name("web-browser-symbolic".to_owned())),
                window: None,
                source: Source::Url {
                    url: link.url_for(query),
                },
                autocomplete: None,
                score: 0.0,
            })
            .collect()
    }

    /// Windows whose title or application id matches `query`.
    ///
    /// Matching is a simple case-insensitive substring test rather than the
    /// fuzzy ranking pop-launcher applies to applications. There are rarely more
    /// than a couple of dozen windows, and a user reaching for one is usually
    /// typing the words they can literally see in its title bar.
    fn matching_windows(&self, query: &str) -> Vec<Item> {
        if query.is_empty() || !self.config.providers.windows {
            return Vec::new();
        }
        let needle = query.to_lowercase();

        self.windows
            .iter()
            .filter(|window| {
                window.title.to_lowercase().contains(&needle)
                    || window.app_id.to_lowercase().contains(&needle)
            })
            .map(|window| Item {
                key: jump_core::ItemKey(format!("window:{}", window.identifier)),
                id: 0,
                title: window.title.clone(),
                subtitle: fl!("window-subtitle", app = window.app_id.as_str()),
                // Reuse the application's icon so a window looks like the app it
                // belongs to rather than a generic placeholder.
                icon: Some(jump_core::Icon::Name(window.app_id.clone())),
                category_icon: Some(jump_core::Icon::Name("focus-windows-symbolic".to_owned())),
                window: None,
                source: Source::Window {
                    identifier: window.identifier.clone(),
                },
                autocomplete: None,
                score: 0.0,
            })
            .collect()
    }

    /// Replace the visible results, restarting the row cascade only when the
    /// set actually changed.
    ///
    /// The comparison is on [`jump_core::ItemKey`], which is stable across
    /// queries. Typing a character that narrows `fire` to `firef` usually
    /// leaves the same top results in the same order; restarting the cascade
    /// then would make the list strobe on every keystroke.
    fn set_results(&mut self, mut items: Vec<Item>) {
        // `kill …` claims the query outright: results whose activation kills
        // something must never sit next to results whose activation opens
        // something.
        if let Some(needle) = keyword_rest(&self.input, KILL_KEYWORD) {
            self.install(jump_core::process::matching(needle));
            return;
        }

        // `emoji …` likewise: a page of emoji next to application results
        // would be noise in both directions.
        if self.config.providers.emoji
            && let Some(needle) = keyword_rest(&self.input, EMOJI_KEYWORD)
        {
            self.install(emoji_items(needle));
            return;
        }

        // A quicklink keyword claims the query the way a plugin keyword does.
        let quicklinks = self.matching_quicklinks(&self.input);
        if !quicklinks.is_empty() {
            self.install(quicklinks);
            return;
        }

        let clips = self.matching_clips(&self.input);
        // The keyword takes the query over entirely, the same way a plugin
        // keyword does.
        if !clips.is_empty() {
            self.install(clips);
            return;
        }

        let mut merged = self.matching_windows(&self.input);
        if self.config.providers.system {
            merged.append(&mut system::matching(
                &self.input,
                self.now_playing.as_deref(),
            ));
        }
        if self.config.providers.devices {
            merged.append(&mut crate::devices::matching(&self.input, &self.devices));
        }
        merged.append(&mut items);

        // One scale for every provider. See `jump_core::rank` — concatenating
        // provider lists and sorting by position is what buried file results
        // below eight mediocre application matches.
        let mut items = jump_core::rank::merge(merged, &self.input, &self.frecency);

        // Pinned results, when they matched at all, sit above everything.
        let favorites = &self.config.favorites;
        if !favorites.is_empty() {
            jump_core::rank::promote_pinned(&mut items, |key| {
                favorites.iter().any(|entry| entry == key.as_str())
            });
        }

        // Fallback searches close the list on every real query, so even a
        // query that matched nothing ends somewhere useful.
        if !self.input.is_empty() && !self.plugins.is_claimed(&self.input) {
            items.extend(self.fallback_items(&self.input));
        }
        self.install(items);
    }

    /// Swap in a new result set, preserving the highlight and cascade rules.
    fn install(&mut self, items: Vec<Item>) {
        let unchanged = items.len() == self.results.len()
            && items
                .iter()
                .zip(&self.results)
                .all(|(new, old)| new.key == old.key);

        if !unchanged {
            self.panel.results_changed(Instant::now());
        }

        // Keep the highlight on the same item where possible, so a result the
        // user is about to hit does not slide out from under them.
        let previously_selected = self.results.get(self.selected).map(|item| item.key.clone());
        self.results = items;
        self.result_icons = self
            .results
            .iter()
            .map(|item| view::icon_handle(item.icon.as_ref()))
            .collect();
        self.selected = previously_selected
            .and_then(|key| self.results.iter().position(|item| item.key == key))
            .unwrap_or(0);
    }

    /// Run a query against both the launcher service and the plugin host.
    fn search(&mut self, query: String) -> Task<Message> {
        // A `kill …` query is answered here and now: it must not fan out to
        // the other providers, and not only for tidiness. The file search
        // spawns `plocate` with the query text in its argv — a process that
        // then *matches the kill query it was spawned by*, out-weighs the
        // real target, and eats the signal. Measured, not theoretical.
        if let Some(needle) = keyword_rest(&query, KILL_KEYWORD) {
            let items = jump_core::process::matching(needle);
            tracing::debug!(needle, count = items.len(), "kill query claimed");
            if let Some(launcher) = self.launcher.as_ref() {
                launcher.interrupt();
            }
            self.install(items);
            return self.refresh_blur();
        }

        // `= …` claims the query and is answered by a subprocess, so unlike
        // the other claims it cannot be resolved here; the row arrives with
        // `CalcResult`. The service is still interrupted immediately so the
        // list does not fill with applications in the meantime.
        if self.config.providers.calculator
            && let Some(expression) = jump_core::calc::claims(&query)
        {
            if let Some(launcher) = self.launcher.as_ref() {
                launcher.interrupt();
            }
            let expression = expression.to_owned();
            let claimed = query.clone();
            // Nothing stale should sit under a `=` query while qalc answers.
            self.install(Vec::new());
            return Task::perform(
                async move {
                    let answer = jump_core::calc::evaluate(&expression).await;
                    (claimed, answer)
                },
                |(query, answer)| cosmic::action::app(Message::CalcResult { query, answer }),
            );
        }

        // Emoji and quicklinks are answered locally the same way: no fanout,
        // and the service is interrupted rather than racing to fill the list.
        if self.config.providers.emoji
            && let Some(needle) = keyword_rest(&query, EMOJI_KEYWORD)
        {
            if let Some(launcher) = self.launcher.as_ref() {
                launcher.interrupt();
            }
            self.install(emoji_items(needle));
            return self.refresh_blur();
        }

        let quicklinks = self.matching_quicklinks(&query);
        if !quicklinks.is_empty() {
            if let Some(launcher) = self.launcher.as_ref() {
                launcher.interrupt();
            }
            self.install(quicklinks);
            return self.refresh_blur();
        }

        let Some(launcher) = self.launcher.clone() else {
            // The bridge is still starting. Dropping the query is correct: the
            // user is mid-keystroke and the next one will re-query.
            return Task::none();
        };

        // A keyworded plugin takes over the query, so the service is told to
        // stop rather than racing to fill the list with app results.
        if self.plugins.is_claimed(&query) {
            launcher.interrupt();
            self.results.clear();
            self.result_icons.clear();
        } else {
            launcher.search(query.clone());
        }

        let mut tasks = Vec::new();

        if !self.plugins.is_empty() {
            tasks.push(query_plugins(self.plugins.clone(), query.clone()));
        }

        let query_for_content = query.clone();

        // File search runs concurrently with the launcher rather than after it,
        // so its latency overlaps instead of adding.
        if let Some(files) = self.files.clone()
            && !self.plugins.is_claimed(&query)
        {
            let text = query;
            tasks.push(Task::perform(
                async move {
                    let items = files.search(&text).await;
                    (text, items)
                },
                |(query, items)| cosmic::action::app(Message::FileResults { query, items }),
            ));
        }

        if let Some(content) = self.content.clone() {
            let text = query_for_content;
            tasks.push(Task::perform(
                async move {
                    let items = content.lock().await.search(&text).unwrap_or_else(|error| {
                        tracing::warn!(%error, "content search failed");
                        Vec::new()
                    });
                    (text, items)
                },
                |(query, items)| cosmic::action::app(Message::ContentResults { query, items }),
            ));
        }

        Task::batch(tasks)
    }

    /// Re-send the blur region so it matches the panel's current geometry.
    ///
    /// The region is double-buffered surface-local state, so it must be resent
    /// whenever the panel grows or shrinks — otherwise the frosted backdrop
    /// stays sized to whatever the panel was when the surface was configured.
    fn refresh_blur(&self) -> Task<Message> {
        let Some(id) = self.surface else {
            return Task::none();
        };
        let mode = self.mode();
        let metrics = self.metrics();
        let rows = match mode {
            Mode::Search => self.results.len(),
            Mode::Grid => metrics.rows_for(self.apps.len()),
        };
        let rect = surface::panel_rect(self.screen, mode, rows, metrics, self.config.grid_layout);

        // Two switches, both of which have to be on. `frosted_system_interface`
        // is the desktop-wide one every COSMIC surface obeys, so ignoring it
        // would leave the launcher frosted on a session the user has explicitly
        // asked to be flat; our own setting only narrows that further.
        let frosted = self.core.system_theme().cosmic().frosted_system_interface;
        surface::set_blur(id, rect, self.config.blur && frosted)
            .map(|()| cosmic::action::app(Message::None))
    }

    /// Open the content index and bring it up to date.
    ///
    /// Runs only after the path index exists, because the candidate list comes
    /// from it. Extraction is the expensive half of file search, so it is kept
    /// entirely off the interactive path and behind an explicit setting.
    fn start_content_index(&self, files: Files) -> Task<Message> {
        if !self.config.files.content {
            return Task::none();
        }
        let Some(path) = dirs::data_dir().map(|dir| dir.join("jump").join("content.db")) else {
            return Task::none();
        };
        let extensions = self.config.files.content_extensions.clone();
        let limits = self.config.files.content_limits();

        Task::future(async move {
            let configured = if extensions.is_empty() {
                jump_core::content::DEFAULT_EXTENSIONS
                    .iter()
                    .map(|extension| (*extension).to_owned())
                    .collect()
            } else {
                extensions
            };

            let content = match Content::open(&path, configured.clone(), limits) {
                Ok(content) => Arc::new(Mutex::new(content)),
                Err(error) => {
                    tracing::warn!(%error, "content index unavailable");
                    return cosmic::action::app(Message::None);
                }
            };

            // Indexing runs detached so the index is queryable *while* it is
            // being built. Waiting for it to finish first meant a first run
            // silently had no content search at all for several minutes.
            let background = Arc::clone(&content);
            tokio::spawn(async move {
                let candidates = files.content_candidates(&configured).await;
                tracing::info!(candidates = candidates.len(), "indexing file contents");

                {
                    let mut guard = background.lock().await;
                    guard.prune_missing();
                }

                // The lock is taken per chunk rather than for the whole pass:
                // holding it across 24 000 documents would block every query
                // behind the indexer.
                let mut indexed = 0;
                for chunk in candidates.chunks(CONTENT_CHUNK) {
                    let mut guard = background.lock().await;
                    // `index` reads files and writes SQLite — blocking work. Run
                    // under `block_in_place` so the runtime moves other tasks to
                    // a different worker instead of stalling them behind it.
                    indexed += tokio::task::block_in_place(|| guard.index(chunk));
                    drop(guard);

                    if background.lock().await.is_full() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                tracing::info!(indexed, "content indexing finished");
            });

            cosmic::action::app(Message::ContentReady(content))
        })
    }

    /// Show the overlay.
    fn open(&mut self) -> Task<Message> {
        if self.surface.is_some() {
            // Already mapped; just make sure the transition is running forward.
            self.dismissing = false;
            self.panel.open(Instant::now());
            return text_input::focus(cosmic::widget::Id::new(INPUT_ID));
        }

        let id = window::Id::unique();
        self.surface = Some(id);
        self.dismissing = false;
        self.input.clear();
        self.results.clear();
        self.result_icons.clear();
        self.selected = 0;
        // Deliberately not started here — see `awaiting_first_configure`.
        self.awaiting_first_configure = true;

        Task::batch([
            surface::open(id).map(|()| cosmic::action::app(Message::None)),
            text_input::focus(cosmic::widget::Id::new(INPUT_ID)),
            // pop-launcher answers an empty query with an empty list, so this
            // does not populate anything — it exists to reset the service's
            // per-interaction state so the first real keystroke is a clean
            // search rather than a continuation of the previous session.
            self.search(String::new()),
            // Snapshot the current track off the frame, so the media
            // commands can caption themselves with what is playing.
            Task::perform(system::now_playing(), |track| {
                cosmic::action::app(Message::NowPlaying(track))
            }),
            // Same for the device lists the system bus owns — skipped
            // entirely when the provider is off, so a user who does not want
            // it does not pay two bus round trips per open either.
            if self.config.providers.devices {
                Task::perform(crate::devices::snapshot(), |devices| {
                    cosmic::action::app(Message::Devices(devices))
                })
            } else {
                Task::none()
            },
        ])
    }

    /// Show the overlay with `query` already in the field — the
    /// `jump show <query>` deep link.
    fn open_with_query(&mut self, query: String) -> Task<Message> {
        let open = self.open();
        self.input = query.clone();
        self.selected = 0;
        let search = self.search(query);
        Task::batch([open, search])
    }

    /// Begin hiding the overlay. The surface is destroyed later, in
    /// [`Message::Closed`], so the fade-out is not cut short.
    fn dismiss(&mut self) -> Task<Message> {
        self.dismiss_with(Release::Service)
    }

    /// Begin hiding the overlay, choosing whether pop-launcher is told.
    fn dismiss_with(&mut self, release: Release) -> Task<Message> {
        if self.surface.is_none() || self.dismissing {
            return Task::none();
        }

        self.dismissing = true;
        self.actions = None;
        self.blur_settled = false;
        self.panel.close(Instant::now());

        if release == Release::Service
            && let Some(launcher) = self.launcher.as_ref()
        {
            // Release the service's per-interaction state but keep the process
            // warm for the next open.
            launcher.dismissed();
        }
        self.frecency.save();

        Task::none()
    }

    /// Destroy the surface once the close transition has played out.
    fn finish_close(&mut self) -> Task<Message> {
        let Some(id) = self.surface.take() else {
            return Task::none();
        };
        self.dismissing = false;
        self.input.clear();
        self.results.clear();
        self.result_icons.clear();
        self.selected = 0;

        surface::close(id).map(|()| cosmic::action::app(Message::None))
    }

    /// Ask the compositor for an activation token, then start `launch`.
    ///
    /// The token is bound to the overlay's own surface, which is what tells the
    /// compositor the new window was asked for by the user through us rather
    /// than appearing on its own. Requesting it is a round trip, so the launch
    /// happens in [`Message::Launch`] once the answer arrives.
    fn spawn(&self, launch: Launch) -> Task<Message> {
        request_token(Some(jump::APP_ID.to_owned()), self.surface)
            .map(move |token| cosmic::action::app(Message::Launch(launch.clone(), token)))
    }

    /// Launch the application at `index` in the Launchpad grid.
    fn activate_app(&mut self, index: usize) -> Task<Message> {
        let Some(app) = self.apps.get(index) else {
            return Task::none();
        };

        let key = app.key();
        let Some(launch) = app.launch() else {
            // Nothing to start, but the click still deserves an answer, and
            // leaving the overlay up over a tile that does nothing is worse
            // than closing it.
            return self.dismiss();
        };

        self.frecency.record(&key);
        Task::batch([self.spawn(launch), self.dismiss()])
    }

    /// Run the result at `index`.
    fn activate(&mut self, index: usize) -> Task<Message> {
        // Enter reaches here twice — once through the input's `on_submit` and
        // once through the global key listener that exists so navigation works
        // regardless of focus. The first call starts the dismissal, so the
        // guard turns the second into a no-op. Without it every activation ran
        // twice: invisible for launching (the second launch raced the token),
        // corrupting for frecency (double counts), and visibly wrong for
        // anything that toggles.
        if self.dismissing {
            return Task::none();
        }

        if self.mode() == Mode::Grid {
            return self.activate_app(index);
        }

        let Some(item) = self.results.get(index) else {
            tracing::debug!(
                index,
                results = self.results.len(),
                "activate on empty results"
            );
            return Task::none();
        };
        tracing::debug!(index, key = %item.key, "activating");

        self.frecency.record(&item.key);

        // Activating a launcher result leaves a request in flight, and the
        // answer is what tells us to start the application. Everything else
        // acts here and now.
        let mut release = Release::Service;

        match &item.source {
            Source::Launcher => {
                if let Some(launcher) = self.launcher.as_ref() {
                    launcher.activate(item.id);
                    release = Release::Nothing;
                }
            }
            Source::Window { identifier } => {
                if let Some(toplevels) = self.toplevels.as_ref() {
                    toplevels.activate(identifier);
                }
            }
            Source::File { path } => {
                jump_core::files::open(path);
            }
            Source::Clipboard { text } => {
                if let Some(clipboard) = self.clipboard.as_ref() {
                    clipboard.copy(text);
                }
            }
            Source::Plugin {
                plugin,
                arg,
                variables,
                ..
            } => {
                if let Some(plugin) = self.plugins.get(plugin) {
                    plugin.activate(arg, variables);
                }
            }
            Source::Process { pid } => {
                jump_core::process::terminate(*pid, false);
            }
            Source::Url { url } => {
                jump_core::web::open(url);
            }
            Source::Calc { answer } => {
                // The answer is already on screen; what is left to do with it
                // is take it somewhere else.
                if let Some(clipboard) = self.clipboard.as_ref() {
                    clipboard.copy(answer);
                }
            }
            Source::System { id } => match system::run(id) {
                Some(system::Outcome::Launch(launch)) => {
                    // A program launch goes through the same activation-token
                    // path as everything else.
                    return Task::batch([self.spawn(launch), self.dismiss_with(release)]);
                }
                Some(system::Outcome::Background(task)) => {
                    // A bus round trip must not run on the frame; dismiss now
                    // and let the call finish behind the fade-out.
                    return Task::batch([
                        Task::future(async move {
                            system::run_background(task).await;
                            cosmic::action::app(Message::None)
                        }),
                        self.dismiss_with(release),
                    ]);
                }
                Some(system::Outcome::Done) | None => {}
            },
        }

        // Dismiss immediately rather than waiting for the service to confirm.
        // Waiting would leave the panel visibly hanging over the window that is
        // about to appear; the activation token is what makes that window take
        // focus, not our still being on screen.
        self.dismiss_with(release)
    }

    /// One line saying why `item` sits where it does — the frecency
    /// inspector, answering "why is this ranked here?" at the moment the
    /// question arises rather than in a separate window.
    ///
    /// Score is shown to two decimals because the differences that decide
    /// order are frequently in the second one.
    fn explain_ranking(&self, item: &Item) -> Option<String> {
        let score = format!("{:.2}", item.score);
        let Some(usage) = self.frecency.explain(&item.key) else {
            return Some(fl!("ranking-never-used", score = score.as_str()));
        };

        let days = usage.age.as_secs() / 86_400;
        let recency = format!("{:.2}", usage.recency_weight);
        Some(fl!(
            "ranking-used",
            score = score.as_str(),
            count = usage.count,
            days = days,
            recency = recency.as_str()
        ))
    }

    /// Desktop-action display names for the application `item` refers to, in
    /// the order pop-launcher addresses them by index.
    ///
    /// Empty unless exactly one installed application carries the result's
    /// title: two entries can share a name — this machine has two "Document
    /// Viewer"s — and labelling one application's actions with another's
    /// would be worse than showing the raw ids the fallback humanises.
    fn action_names_for(&self, item: &Item) -> Vec<String> {
        let mut matching = self.apps.iter().filter(|app| app.name == item.title);
        match (matching.next(), matching.next()) {
            (Some(app), None) => app.actions.clone(),
            _ => Vec::new(),
        }
    }

    /// Run action `index` of the open panel.
    fn run_action(&mut self, index: usize) -> Task<Message> {
        let Some(panel) = self.actions.take() else {
            return Task::none();
        };
        let Some(action) = panel.actions.get(index) else {
            return Task::none();
        };

        tracing::debug!(index, kind = ?action.kind, "running action");
        match action.kind.clone() {
            actions::Kind::Primary => self.activate(self.selected),
            actions::Kind::OpenFolder(path) => {
                jump_core::files::open(&path);
                self.dismiss()
            }
            actions::Kind::CopyText(text) => {
                match self.clipboard.as_ref() {
                    Some(clipboard) => clipboard.copy(&text),
                    None => tracing::warn!("no clipboard backend; copy dropped"),
                }
                self.dismiss()
            }
            actions::Kind::Trash(path) => {
                actions::trash(&path);
                self.dismiss()
            }
            actions::Kind::CloseWindow => {
                // Stays open, like Ctrl+W: closing a window is housekeeping,
                // not the end of the interaction.
                if let (Some(item), Some(toplevels)) =
                    (self.results.get(self.selected), self.toplevels.as_ref())
                    && let Source::Window { identifier } = &item.source
                {
                    toplevels.close(identifier);
                }
                Task::none()
            }
            actions::Kind::ForceKill { pid } => {
                jump_core::process::terminate(pid, true);
                self.dismiss()
            }
            actions::Kind::PluginMod {
                plugin,
                arg,
                variables,
            } => {
                if let Some(plugin) = self.plugins.get(&plugin) {
                    plugin.activate(&arg, &variables);
                }
                self.dismiss()
            }
            actions::Kind::Window(command) => {
                // Dismiss afterwards: the change happens behind a full-screen
                // overlay, and staying open would hide exactly the thing the
                // user just asked to see happen.
                if let (Some(item), Some(toplevels)) =
                    (self.results.get(self.selected), self.toplevels.as_ref())
                    && let Source::Window { identifier } = &item.source
                {
                    match command {
                        actions::WindowCommand::Maximize => toplevels.maximize(identifier),
                        actions::WindowCommand::Unmaximize => toplevels.unmaximize(identifier),
                        actions::WindowCommand::Minimize => toplevels.minimize(identifier),
                        actions::WindowCommand::Fullscreen => toplevels.fullscreen(identifier),
                        actions::WindowCommand::Unfullscreen => toplevels.unfullscreen(identifier),
                    }
                }
                self.dismiss()
            }
            actions::Kind::TogglePin { key } => {
                // Housekeeping like Close Window: the launcher stays open and
                // shows the new order immediately.
                match self
                    .config
                    .favorites
                    .iter()
                    .position(|entry| entry == key.as_str())
                {
                    Some(position) => {
                        self.config.favorites.remove(position);
                    }
                    None => self.config.favorites.push(key.as_str().to_owned()),
                }
                self.config.write_favorites();

                let mut items = std::mem::take(&mut self.results);
                let favorites = &self.config.favorites;
                jump_core::rank::promote_pinned(&mut items, |key| {
                    favorites.iter().any(|entry| entry == key.as_str())
                });
                self.install(items);
                Task::none()
            }
            actions::Kind::LauncherContext { item, option } => {
                if let Some(launcher) = self.launcher.as_ref() {
                    launcher.activate_context(item, option);
                }
                // The reply may be a DesktopEntry to launch, so the service
                // must not be told the interaction is over.
                self.dismiss_with(Release::Nothing)
            }
        }
    }
}

/// Whether hiding the overlay also ends the pop-launcher interaction.
///
/// `Activate` and `Close` written back to back lose the response: pop-launcher
/// 1.2.7 drops the activation still in flight when the client says it has
/// closed, and that response is the desktop entry we are supposed to launch.
/// Measured against the installed service — the reply survives with a delay
/// between the two writes and disappears without one, which makes it a race
/// rather than something to tune a sleep against.
///
/// Skipping `Close` is safe because opening the overlay already sends an empty
/// search, which resets the same per-interaction state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// Tell pop-launcher the interaction is over.
    Service,
    /// Leave the service alone; something of ours is still in flight.
    Nothing,
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = jump::APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = Config::load();

        // The overlay is shell chrome, not a window: `AppType::System` makes
        // libcosmic's frosted and corner-radius decisions follow the
        // system-interface settings, the same as the panel and the built-in
        // launcher.
        core.set_app_type(cosmic::core::AppType::System);
        // Our own subscription handles Escape and the navigation keys, and the
        // query field is the only focusable widget — leaving libcosmic's
        // keyboard navigation on would interpret Tab, Escape and Ctrl+F a
        // second time in parallel.
        core.set_keyboard_nav(false);

        let mut app = Self {
            core,
            surface: None,
            screen: Size::new(1920.0, 1080.0),
            input: String::new(),
            results: Vec::new(),
            result_icons: Vec::new(),
            apps: apps::load(),
            windows: Vec::new(),
            toplevels: None,
            files: None,
            content: None,
            clips: Vec::new(),
            now_playing: None,
            devices: Vec::new(),
            clipboard: None,
            selected: 0,
            actions: None,
            launcher: None,
            frecency: Frecency::load(),
            plugins: {
                let mut plugins = PluginHost::discover();
                plugins.set_disabled(config.disabled_plugins.iter().cloned());
                plugins.set_keyword_overrides(config.plugin_keywords.iter().cloned());
                plugins
            },
            panel: {
                let mut panel = Panel::new();
                panel.set_reduced_motion(config.reduce_motion);
                panel
            },
            config,
            dismissing: false,
            scroll_travel: 0.0,
            blur_settled: false,
            awaiting_first_configure: false,
        };

        // Normally the process is started *by* the keybinding, so the user is
        // already waiting and the overlay should map immediately. Under
        // `--daemon` it starts hidden and waits to be toggled. `show` on the
        // *first* invocation lands here too — when there is no daemon yet to
        // forward it to, this process is the one that honours it.
        let open = if let Some(query) = flags
            .action
            .as_deref()
            .filter(|action| *action == ACTION_SHOW)
            .map(|_| flags.args.join(" "))
        {
            app.open_with_query(query)
        } else if flags.daemon {
            tracing::info!("started in daemon mode; waiting to be activated");
            Task::none()
        } else {
            app.open()
        };

        // Index maintenance runs in the background. Building it takes seconds
        // for a home directory and minutes for an external drive, so it must
        // never be on the path between the keybind and the first frame.
        let index = match Files::new(app.config.files.to_core()) {
            Some(mut files) => Task::future(async move {
                if files.needs_refresh() {
                    tracing::info!("rebuilding file index");
                    files.rebuild().await;
                }
                cosmic::action::app(Message::IndexReady(files))
            }),
            None => Task::none(),
        };

        (app, Task::batch([open, index]))
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::None => Task::none(),

            Message::LauncherReady(launcher) => {
                self.launcher = Some(launcher);
                // Re-run whatever the user has typed while the bridge was
                // starting, so the first query is not swallowed.
                let query = self.input.clone();
                self.search(query)
            }

            Message::Launcher(event) => match event {
                jump_core::Event::Update { query, items, .. } => {
                    // Stale-response guard: the service carries no request id,
                    // so a response is only trusted when the query that
                    // produced it still matches what the user has typed. A
                    // mismatch means a newer response is already in flight, and
                    // keeping the current list is better than flashing.
                    if query == self.input {
                        self.set_results(items);
                        return self.refresh_blur();
                    }
                    Task::none()
                }
                jump_core::Event::Fill(text) => {
                    self.input = text.clone();
                    self.search(text)
                }
                // pop-launcher does not start applications itself. Activating a
                // desktop-entry result makes it hand the entry back and leave
                // the launching to the frontend, which is what lets an
                // activation token and the right GPU environment be attached.
                jump_core::Event::DesktopEntry {
                    path,
                    gpu_preference,
                    action_name,
                } => {
                    let Some(launch) =
                        Launch::from_desktop_file(path.clone(), action_name, gpu_preference)
                    else {
                        tracing::warn!(path = %path.display(), "desktop entry could not be read");
                        return self.dismiss();
                    };

                    Task::batch([self.spawn(launch), self.dismiss()])
                }
                jump_core::Event::Close => self.dismiss(),
                jump_core::Event::Context { id, options } => {
                    // Only meaningful while the panel is open on the item that
                    // asked; a stale answer for a different row is dropped.
                    let addressed = self
                        .results
                        .get(self.selected)
                        .is_some_and(|item| item.source == Source::Launcher && item.id == id);
                    if !addressed {
                        return Task::none();
                    }

                    // Resolved before the panel is borrowed mutably.
                    let names = self
                        .results
                        .get(self.selected)
                        .map(|item| self.action_names_for(item))
                        .unwrap_or_default();

                    if let Some(panel) = self.actions.as_mut() {
                        panel.extend_with_context(id, options, &names);
                    }
                    Task::none()
                }
                jump_core::Event::Disconnected => {
                    tracing::error!("pop-launcher exited; results are unavailable");
                    self.launcher = None;
                    Task::none()
                }
            },

            Message::PluginResults {
                query,
                items,
                rerun,
            } => {
                if query != self.input {
                    return Task::none();
                }
                if self.plugins.is_claimed(&query) {
                    // The plugin owns this query outright.
                    self.set_results(items);
                } else if !items.is_empty() {
                    // Otherwise plugin results join the service's, above them:
                    // the user typed a keyword, so that intent outranks a fuzzy
                    // application match.
                    let mut merged = items;
                    merged.append(&mut self.results);
                    self.set_results(merged);
                }

                let refresh = self.refresh_blur();
                // A plugin that asked to stream gets the same query again
                // after its interval — each answer schedules at most one
                // rerun, so the cadence is the plugin's, not a runaway loop.
                let Some(delay) = rerun else {
                    return refresh;
                };
                Task::batch([
                    refresh,
                    Task::future(async move {
                        tokio::time::sleep(delay).await;
                        cosmic::action::app(Message::RerunPlugins(query))
                    }),
                ])
            }

            Message::RerunPlugins(query) => {
                // Only while the user is still looking at exactly that query.
                if query != self.input || self.surface.is_none() || self.dismissing {
                    return Task::none();
                }
                query_plugins(self.plugins.clone(), query)
            }

            Message::IndexReady(files) => {
                let ready = files.is_ready();
                tracing::info!(ready, "file index available");
                let content = self.start_content_index(files.clone());
                self.files = Some(files);
                content
            }

            Message::ContentReady(content) => {
                self.content = Some(content);
                Task::none()
            }

            Message::ContentResults { query, items } => {
                if query != self.input || items.is_empty() {
                    return Task::none();
                }
                let mut merged = std::mem::take(&mut self.results);
                merged.extend(items);
                self.set_results(merged);
                self.refresh_blur()
            }

            Message::FileResults { query, items } => {
                // Same staleness rule as every other provider: a result set is
                // only trusted while it still describes what the user has typed.
                if query != self.input || items.is_empty() {
                    return Task::none();
                }
                let mut merged = std::mem::take(&mut self.results);
                merged.extend(items);
                self.set_results(merged);
                self.refresh_blur()
            }

            Message::Scrolled(delta) => {
                // Only paged Launchpad needs this. A continuous grid is a real
                // `scrollable`, which consumes the wheel itself, and the search
                // list likewise — intercepting there would scroll twice.
                if self.mode() != Mode::Grid || !self.config.scroll.is_paged() {
                    return Task::none();
                }

                self.scroll_travel += delta;
                if self.scroll_travel.abs() < SCROLL_PER_PAGE {
                    return Task::none();
                }

                // The backend reports scrolling down and scrolling right as
                // positive, and both mean "further into the list".
                let direction = if self.scroll_travel > 0.0 { 1 } else { -1 };
                self.scroll_travel = 0.0;

                self.update(Message::MovePage(direction))
            }

            Message::ConfigChanged(config) => {
                // Applied live rather than at next start. Geometry and blur are
                // derived from configuration on every frame, so the only thing
                // that needs doing here is resending the blur region, which is
                // compositor state rather than something the view can express.
                let files_changed = config.files != self.config.files;
                self.plugins
                    .set_disabled(config.disabled_plugins.iter().cloned());
                self.plugins
                    .set_keyword_overrides(config.plugin_keywords.iter().cloned());
                self.panel.set_reduced_motion(config.reduce_motion);
                self.config = config;
                tracing::info!(files_changed, "settings updated");
                self.refresh_blur()
            }

            Message::Tray(action) => match action {
                tray::Action::Toggle => self.update(Message::Toggle),
                tray::Action::ShowLaunchpad => {
                    // Launchpad *is* the empty query, so clearing the input is
                    // all "show applications" means.
                    self.input.clear();
                    self.selected = 0;
                    self.update(Message::Toggle)
                }
                tray::Action::Settings => {
                    // A separate process, so the launcher does not have to grow
                    // a second kind of surface. It writes the same config store
                    // this daemon subscribes to, so changes arrive live.
                    if let Err(error) = std::process::Command::new("jump-settings").spawn() {
                        tracing::error!(%error, "could not start the settings window");
                    }
                    Task::none()
                }

                tray::Action::RebuildIndex => {
                    let Some(mut files) = self.files.clone() else {
                        return Task::none();
                    };
                    Task::future(async move {
                        tracing::info!("rebuilding file index on request");
                        files.rebuild().await;
                        cosmic::action::app(Message::IndexReady(files))
                    })
                }
                tray::Action::ClearClipboard => {
                    self.clips.clear();
                    clipboard::clear_history();
                    Task::none()
                }
                tray::Action::Quit => {
                    self.frecency.save();
                    cosmic::iced::exit()
                }
            },

            Message::ClipboardReady(clipboard) => {
                self.clipboard = Some(clipboard);
                Task::none()
            }

            Message::Clips(clips) => {
                self.clips = clips;
                Task::none()
            }

            Message::ToplevelsReady(toplevels) => {
                self.toplevels = Some(toplevels);
                Task::none()
            }

            Message::CalcResult { query, answer } => {
                // Same staleness rule as every other provider.
                if query != self.input {
                    return Task::none();
                }
                let items = answer.map(calc_item).into_iter().collect();
                self.install(items);
                self.refresh_blur()
            }

            Message::NowPlaying(track) => {
                let changed = track != self.now_playing;
                self.now_playing = track;
                // Media command subtitles are baked into the built results, so
                // a visible search has to be re-run for the caption to appear.
                if changed && !self.input.is_empty() {
                    let query = self.input.clone();
                    return self.search(query);
                }
                Task::none()
            }

            Message::Devices(devices) => {
                let changed = devices != self.devices;
                self.devices = devices;
                // Device rows are baked into the built results, so a visible
                // search has to be re-run for them to appear.
                if changed && !self.input.is_empty() {
                    let query = self.input.clone();
                    return self.search(query);
                }
                Task::none()
            }

            Message::Windows(windows) => {
                self.windows = windows;
                // A window opening or closing while the launcher is up should be
                // reflected immediately, but only when windows are on screen.
                if self.mode() == Mode::Search && !self.input.is_empty() {
                    let query = self.input.clone();
                    return self.search(query);
                }
                Task::none()
            }

            Message::InputChanged(input) => {
                self.actions = None;
                let was = self.mode();
                self.input = input.clone();
                let mode_changed = self.mode() != was;
                if mode_changed {
                    // Index spaces are unrelated between modes, so carrying the
                    // old selection over would land on an arbitrary entry.
                    self.selected = 0;
                    self.panel.results_changed(Instant::now());
                }

                let search = self.search(input);
                if mode_changed {
                    // Grid and search panels differ in both dimensions, so the
                    // frosted region has to be resent or it keeps the shape of
                    // the mode we just left.
                    Task::batch([search, self.refresh_blur()])
                } else {
                    search
                }
            }

            Message::MoveSelection(delta) => {
                if let Some(panel) = self.actions.as_mut() {
                    // Left/Right arrive as ±2 for the grid's benefit; in a
                    // one-column list they mean a single step.
                    panel.shift(delta.signum());
                    return Task::none();
                }
                let count = self.selectable();
                if count == 0 {
                    return Task::none();
                }
                // In grid mode a vertical step is a whole row; horizontal steps
                // arrive as +/-1 from the left/right keys.
                let step = if self.mode() == Mode::Grid && delta.abs() == 1 {
                    delta * self.metrics().columns as i32
                } else {
                    delta.signum()
                };

                let last = count - 1;
                let previous_page = self.page();
                self.selected = if step.is_negative() {
                    self.selected.saturating_sub(step.unsigned_abs() as usize)
                } else {
                    (self.selected + step as usize).min(last)
                };

                let page = self.page();
                if page != previous_page {
                    self.panel
                        .page_changed(Instant::now(), page > previous_page);
                }
                Task::none()
            }

            Message::MovePage(direction) => {
                let count = self.selectable();
                if count == 0 || direction == 0 {
                    return Task::none();
                }

                let page_size = self.metrics().page_size() as i32;
                let previous_page = self.page();
                let target = self.selected as i32 + direction * page_size;
                self.selected = target.clamp(0, count as i32 - 1) as usize;

                let page = self.page();
                if page != previous_page {
                    self.panel
                        .page_changed(Instant::now(), page > previous_page);
                }
                Task::none()
            }

            Message::Select(index) => {
                if index != self.selected {
                    self.actions = None;
                }
                if index < self.selectable() {
                    self.selected = index;
                }
                Task::none()
            }

            Message::Activate => {
                if let Some(panel) = self.actions.as_ref() {
                    let index = panel.selected;
                    return self.run_action(index);
                }
                self.activate(self.selected)
            }

            Message::ActivateMod(modifier) => {
                // Modifier+Enter on the highlighted row runs that alternate
                // directly — the action panel is the discoverable path to the
                // same thing, not a required one.
                let Some(kind) = self
                    .results
                    .get(self.selected)
                    .and_then(|item| actions::mod_for(item, &modifier))
                else {
                    // No alternate for this modifier: fall back to the plain
                    // activation, so Ctrl+Enter never does nothing.
                    return self.activate(self.selected);
                };
                self.actions = Some(actions::Panel {
                    actions: vec![actions::Action {
                        label: String::new(),
                        icon: "system-run-symbolic",
                        shortcut: None,
                        kind,
                    }],
                    selected: 0,
                    // Never drawn: this panel exists for exactly one
                    // `run_action` call and is dropped on the next line.
                    ranking: None,
                });
                self.run_action(0)
            }
            Message::ActivateAt(index) => self.activate(index),

            Message::Launch(launch, token) => Task::future(async move {
                launch.run(token).await;
                cosmic::action::app(Message::None)
            }),

            Message::Complete => {
                let Some(item) = self.results.get(self.selected) else {
                    return Task::none();
                };

                // A plugin item that carries `autocomplete` completes locally,
                // Alfred-style: the text replaces the query and re-searches.
                if let Some(text) = item.autocomplete.clone() {
                    self.input = text.clone();
                    return self.search(text);
                }

                if let Some(launcher) = self.launcher.as_ref()
                    && item.source == Source::Launcher
                {
                    launcher.complete(item.id);
                }
                Task::none()
            }

            Message::ToggleActions => {
                tracing::debug!(open = self.actions.is_some(), "action panel toggle");
                if self.actions.take().is_some() {
                    return Task::none();
                }
                // Search results only: a grid tile's one action is Enter.
                if self.mode() != Mode::Search {
                    return Task::none();
                }
                let Some(item) = self.results.get(self.selected) else {
                    return Task::none();
                };

                let pinned = self
                    .config
                    .favorites
                    .iter()
                    .any(|key| key == item.key.as_str());
                // The window's live state decides Maximize against Restore.
                let window = if let Source::Window { identifier } = &item.source {
                    self.windows
                        .iter()
                        .find(|window| &window.identifier == identifier)
                } else {
                    None
                };
                let ranking = self.explain_ranking(item);
                self.actions = actions::Panel::for_item(item, pinned, window)
                    .map(|panel| panel.explaining(ranking));
                tracing::debug!(
                    actions = self.actions.as_ref().map_or(0, |panel| panel.actions.len()),
                    "action panel opened"
                );

                // An application's extra actions are pop-launcher's context
                // options; ask for them and append when the answer arrives.
                if self.actions.is_some()
                    && item.source == Source::Launcher
                    && let Some(launcher) = self.launcher.as_ref()
                {
                    launcher.context(item.id);
                }
                Task::none()
            }

            Message::SelectAction(index) => {
                if let Some(panel) = self.actions.as_mut() {
                    panel.selected = index.min(panel.actions.len().saturating_sub(1));
                }
                Task::none()
            }

            Message::RunAction(index) => self.run_action(index),

            Message::CloseSelected => {
                // Only meaningful on a window result; everything else ignores it
                // rather than doing something surprising.
                if let (Some(item), Some(toplevels)) =
                    (self.results.get(self.selected), self.toplevels.as_ref())
                    && let Source::Window { identifier } = &item.source
                {
                    toplevels.close(identifier);
                }
                Task::none()
            }

            Message::Dismiss => {
                // Escape peels one layer: panel first, launcher second.
                if self.actions.take().is_some() {
                    return Task::none();
                }
                self.dismiss()
            }

            Message::Toggle => {
                if self.surface.is_some() && !self.dismissing {
                    self.dismiss()
                } else {
                    self.open()
                }
            }

            Message::Configured(size) => {
                self.screen = size;

                self.blur_settled = false;
                if self.awaiting_first_configure {
                    self.awaiting_first_configure = false;
                    // The surface exists and is about to draw, so the transition
                    // now has somewhere to play.
                    self.panel.open(Instant::now());
                }

                Task::batch([
                    self.refresh_blur(),
                    // Focus has to be (re)claimed here rather than at creation
                    // time: when `open` runs, the surface does not exist yet and
                    // the text input has never been laid out, so the focus task
                    // finds nothing to focus.
                    text_input::focus(cosmic::widget::Id::new(INPUT_ID)),
                ])
            }

            Message::Frame(now) => {
                if self.dismissing && self.panel.is_closed(now) {
                    return self.finish_close();
                }

                // A frame callback means a buffer has been committed, so the
                // blur region now has something to attach to.
                if !self.blur_settled {
                    self.blur_settled = true;
                    return self.refresh_blur();
                }
                // The frame subscription itself is what triggers the redraw;
                // nothing else needs to happen here.
                Task::none()
            }
        }
    }

    fn view(&self) -> cosmic::Element<'_, Self::Message> {
        // The overlay has no main window, so this is only reached if one is
        // somehow created. Render nothing rather than panicking.
        cosmic::widget::text("").into()
    }

    fn view_window(&self, id: window::Id) -> cosmic::Element<'_, Self::Message> {
        if Some(id) != self.surface {
            return cosmic::widget::text("").into();
        }

        view::overlay(
            &self.panel,
            Instant::now(),
            self.screen,
            self.mode(),
            &self.input,
            &self.results,
            &self.result_icons,
            &self.apps,
            self.selected,
            self.metrics(),
            &self.config,
            self.page(),
            self.actions.as_ref(),
            // Nothing has ever been activated, so the launcher has never
            // actually been used.
            self.frecency.is_empty(),
        )
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subscriptions = vec![
            // Owns the pop-launcher child for the lifetime of the daemon.
            Subscription::run(launcher_stream),
            // Owns the Wayland connection that watches open windows.
            Subscription::run(toplevel_stream),
            // Owns the Wayland connection that watches the clipboard.
            Subscription::run(clipboard_stream),
            // Status-area icon, when a notifier host is running.
            Subscription::run(tray_stream),
            // Settings changes, delivered by cosmic-config without a restart.
            // `watch_config` already filters to the struct's own keys and only
            // emits when a value actually changed; with the `dbus-config`
            // feature it goes through cosmic-settings-daemon rather than
            // polling inotify.
            self.core()
                .watch_config::<Config>(jump::APP_ID)
                .map(|update| {
                    for error in update.errors {
                        tracing::warn!(%error, "ignoring an unreadable setting");
                    }
                    Message::ConfigChanged(update.config)
                }),
            // Keyboard handling lives here rather than on the text input so
            // that navigation keys work regardless of which widget has focus.
            event::listen_with(|event, _status, _id| match event {
                event::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    match key {
                        Key::Named(Named::Escape) => Some(Message::Dismiss),
                        Key::Named(Named::Enter) => {
                            // A plugin item's alternates are reached with
                            // modifier+Enter; everything else ignores the
                            // modifier and activates normally.
                            if modifiers.control() {
                                Some(Message::ActivateMod("ctrl".to_owned()))
                            } else if modifiers.alt() {
                                Some(Message::ActivateMod("alt".to_owned()))
                            } else if modifiers.shift() {
                                Some(Message::ActivateMod("shift".to_owned()))
                            } else if modifiers.logo() {
                                Some(Message::ActivateMod("super".to_owned()))
                            } else {
                                Some(Message::Activate)
                            }
                        }
                        Key::Named(Named::ArrowDown) => Some(Message::MoveSelection(1)),
                        Key::Named(Named::ArrowUp) => Some(Message::MoveSelection(-1)),
                        // Grid mode reads these as one column; the list treats
                        // them as a single step, which is harmless there.
                        Key::Named(Named::ArrowRight) => Some(Message::MoveSelection(2)),
                        Key::Named(Named::ArrowLeft) => Some(Message::MoveSelection(-2)),
                        Key::Named(Named::Tab) => Some(Message::Complete),
                        // Ctrl+N / Ctrl+P, for the emacs-fingered.
                        Key::Character(ref c) if modifiers.control() && c.as_str() == "n" => {
                            Some(Message::MoveSelection(1))
                        }
                        Key::Character(ref c) if modifiers.control() && c.as_str() == "p" => {
                            Some(Message::MoveSelection(-1))
                        }
                        // Close the highlighted window without leaving the
                        // launcher, the way a switcher is expected to behave.
                        Key::Character(ref c) if modifiers.control() && c.as_str() == "w" => {
                            Some(Message::CloseSelected)
                        }
                        // The action panel, on Raycast's muscle memory.
                        Key::Character(ref c) if modifiers.control() && c.as_str() == "k" => {
                            Some(Message::ToggleActions)
                        }
                        _ => None,
                    }
                }
                event::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                    // Both axes matter. A mouse with a thumb wheel — an MX
                    // Master, say — reports the horizontal wheel as `x` with `y`
                    // at zero, so reading only `y` makes that wheel dead. The
                    // dominant axis wins, which also stops a slightly diagonal
                    // touchpad flick from counting twice.
                    //
                    // Line and pixel deltas come from different devices, so they
                    // are normalised to comparable distances.
                    let (x, y) = match delta {
                        mouse::ScrollDelta::Lines { x, y } => (x * LINE_HEIGHT, y * LINE_HEIGHT),
                        mouse::ScrollDelta::Pixels { x, y } => (x, y),
                    };
                    let travel = if x.abs() > y.abs() { x } else { y };
                    Some(Message::Scrolled(travel))
                }
                event::Event::Window(window::Event::Opened { size, .. }) => {
                    Some(Message::Configured(size))
                }
                event::Event::Window(window::Event::Resized(size)) => {
                    Some(Message::Configured(size))
                }
                _ => None,
            }),
        ];

        // Only ask for frame callbacks while something is actually moving.
        // A launcher that redraws at display rate while the user reads the
        // results is a launcher that shows up in power measurements.
        if self.surface.is_some()
            && (self.awaiting_first_configure
                || self.panel.is_animating(Instant::now(), self.selectable()))
        {
            subscriptions.push(window::frames().map(|(_, at)| Message::Frame(at)));
        }

        Subscription::batch(subscriptions)
    }

    fn on_escape(&mut self) -> Task<Self::Message> {
        self.dismiss()
    }

    fn dbus_activation(&mut self, msg: cosmic::dbus_activation::Message) -> Task<Self::Message> {
        match msg.msg {
            // `jump show <query>` from a second invocation: open with the
            // query pre-filled, and re-fill it when the overlay was already
            // up — the user asked for that view, not for a toggle.
            cosmic::dbus_activation::Details::ActivateAction { action, args }
                if action == ACTION_SHOW =>
            {
                self.open_with_query(args.join(" "))
            }
            // Re-running `jump` while the daemon is up is the toggle gesture,
            // so this is what a keybinding ends up calling.
            _ => self.update(Message::Toggle),
        }
    }
}

/// Streams actions from the status-area icon.
fn tray_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(8, async move |mut output| {
        use cosmic::iced::futures::SinkExt;

        let Some(mut actions) = tray::spawn().await else {
            return;
        };

        while let Some(action) = actions.recv().await {
            if output.send(Message::Tray(action)).await.is_err() {
                break;
            }
        }
    })
}

/// Documents indexed between yields, so queries are not blocked behind the
/// background indexer.
const CONTENT_CHUNK: usize = 200;

/// Wheel travel, in logical pixels, that turns one Launchpad page.
const SCROLL_PER_PAGE: f32 = 120.0;

/// Assumed height of one wheel line, for devices reporting line deltas.
const LINE_HEIGHT: f32 = 40.0;

/// Keyword that switches the query into clipboard history.
const CLIP_KEYWORD: &str = "clip";

/// Keyword for process search. Not localised: it is a command vocabulary the
/// same way `clip` is, and muscle memory should survive a locale change.
const KILL_KEYWORD: &str = "kill";

/// Keyword for emoji search, claiming the query the same way `clip` does.
const EMOJI_KEYWORD: &str = "emoji";

/// Most emoji shown for one query. Twelve list rows is the panel's budget; a
/// denser grid presentation is the planned home for the rest.
const EMOJI_LIMIT: usize = 12;

/// The single row a calculator answer produces.
///
/// The answer is the title because the view renders that row in large type —
/// the number is the result, not a label for it.
fn calc_item(answer: String) -> Item {
    Item {
        key: jump_core::ItemKey("calc".to_owned()),
        id: 0,
        title: answer.clone(),
        subtitle: fl!("calc-copy-subtitle"),
        icon: Some(jump_core::Icon::Name("accessories-calculator".to_owned())),
        category_icon: None,
        window: None,
        source: Source::Calc { answer },
        autocomplete: None,
        score: 1.0,
    }
}

/// Emoji rows for an `emoji …` query. The glyph rides in the title — emoji
/// have no icon-theme icons, and the text renderer already draws them.
fn emoji_items(needle: &str) -> Vec<Item> {
    jump_core::emoji::matching(needle, EMOJI_LIMIT)
        .into_iter()
        .map(|matched| Item {
            key: jump_core::ItemKey(format!("emoji:{}", matched.emoji)),
            id: 0,
            title: format!("{}  {}", matched.emoji, matched.name),
            subtitle: matched.shortcode.map_or_else(
                || fl!("emoji-copy-subtitle"),
                |code| format!(":{code}: — {}", fl!("emoji-copy-subtitle")),
            ),
            icon: None,
            category_icon: None,
            window: None,
            // Copying is exactly what activating a clipboard entry does, so
            // emoji reuse that source rather than growing a parallel one.
            source: Source::Clipboard {
                text: matched.emoji.to_owned(),
            },
            autocomplete: None,
            score: 1.0,
        })
        .collect()
}

/// Query the plugin host off the frame, tagging the answer with the query
/// that produced it and any requested rerun interval.
fn query_plugins(plugins: PluginHost, query: String) -> Task<Message> {
    Task::perform(
        async move {
            let results = plugins.query(&query).await;
            (query, results)
        },
        |(query, results)| {
            cosmic::action::app(Message::PluginResults {
                query,
                items: results.items,
                rerun: results.rerun,
            })
        },
    )
}

/// The query text after `keyword`, when the query is addressed to it.
///
/// The keyword must be followed by a space or end the query, so `killarney`
/// is not read as `kill` + `arney`.
fn keyword_rest<'a>(query: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = query.strip_prefix(keyword)?;
    match rest.strip_prefix(' ') {
        Some(rest) => Some(rest.trim_start()),
        None if rest.is_empty() => Some(""),
        None => None,
    }
}

/// Long-lived stream that owns the clipboard Wayland connection.
fn clipboard_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(16, async move |mut output| {
        use cosmic::iced::futures::SinkExt;

        let Some((clipboard, mut history)) = clipboard::spawn() else {
            return;
        };

        if output
            .send(Message::ClipboardReady(clipboard))
            .await
            .is_err()
        {
            return;
        }

        while let Some(entries) = history.recv().await {
            if output.send(Message::Clips(entries)).await.is_err() {
                break;
            }
        }
    })
}

/// Long-lived stream that owns the window-switcher Wayland connection.
fn toplevel_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(16, async move |mut output| {
        use cosmic::iced::futures::SinkExt;

        let Some((toplevels, mut windows)) = toplevel::spawn() else {
            return;
        };

        if output
            .send(Message::ToplevelsReady(toplevels))
            .await
            .is_err()
        {
            return;
        }

        while let Some(snapshot) = windows.recv().await {
            if output.send(Message::Windows(snapshot)).await.is_err() {
                break;
            }
        }
    })
}

/// Long-lived stream that owns the pop-launcher child process.
fn launcher_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(64, async move |mut output| {
        use cosmic::iced::futures::SinkExt;

        let (launcher, mut events, guard) = match Launcher::spawn() {
            Ok(parts) => parts,
            Err(error) => {
                tracing::error!(%error, "could not start pop-launcher");
                return;
            }
        };

        if output.send(Message::LauncherReady(launcher)).await.is_err() {
            guard.shutdown().await;
            return;
        }

        while let Some(event) = events.recv().await {
            if output.send(Message::Launcher(event)).await.is_err() {
                break;
            }
        }

        guard.shutdown().await;
    })
}
