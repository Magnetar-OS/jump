// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Persisted settings, stored through `cosmic-config`.
//!
//! Previously a hand-rolled TOML file. `cosmic-config` is what every other
//! COSMIC application uses, and moving to it buys two things a private file
//! cannot:
//!
//! * **Live reload.** Changes arrive over a subscription and are applied without
//!   restarting, which matters for a process expected to stay running for the
//!   whole session.
//! * **A predictable home.** Settings land under `~/.config/cosmic/<APP_ID>/v1/`
//!   alongside everything else, so the same tools back them up and inspect them.
//!
//! It is also as close to control-center integration as this can get: COSMIC
//! Settings has no mechanism for third-party pages, so living in the right
//! config store is the available form of "integrated".
//!
//! Each field is stored as its own RON file, which is why the types here are
//! plain serialisable structs rather than one blob.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

/// How Launchpad mode lays itself out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GridLayout {
    /// Icons spread across the whole display, macOS Launchpad style.
    #[default]
    Fullscreen,
    /// A centred panel, matching the search surface.
    ///
    /// Better on ultrawides, where a full-screen grid spreads icons so far
    /// apart that the eye has to travel across the whole display.
    Panel,
}

/// How the Launchpad grid moves through applications that do not fit on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollMode {
    /// One long scrolling surface.
    Continuous,
    /// Discrete pages that slide horizontally, like macOS Launchpad.
    #[default]
    PageHorizontal,
    /// Discrete pages that slide vertically.
    PageVertical,
}

impl ScrollMode {
    #[must_use]
    pub fn is_paged(self) -> bool {
        !matches!(self, Self::Continuous)
    }
}

/// File-search settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileConfig {
    pub enabled: bool,
    /// Also index mounted external volumes — NTFS drives and the like.
    ///
    /// Off by default because these are routinely multi-terabyte: indexing one
    /// takes minutes and produces a database far larger than a home directory's.
    pub external_drives: bool,
    /// Hours before the index is considered stale and rebuilt.
    pub refresh_hours: u64,
    /// Extra trees to index, in addition to the home directory.
    pub roots: Vec<std::path::PathBuf>,
    /// Directory names never to index, on top of the built-in list.
    pub ignore_names: Vec<String>,
    /// Absolute paths never to index.
    pub ignore_paths: Vec<std::path::PathBuf>,
    /// If non-empty, only these extensions are returned.
    pub include_extensions: Vec<String>,
    /// Extensions never returned.
    pub exclude_extensions: Vec<String>,
    /// Also index the *contents* of text files, not just their names.
    ///
    /// Off by default. Extraction is far more expensive than listing paths, and
    /// a launcher that silently starts reading every document in a home
    /// directory is doing something the user did not ask for.
    pub content: bool,
    /// Extensions whose contents are indexed. Empty means the built-in list.
    pub content_extensions: Vec<String>,
    /// Ceiling on the content index, in megabytes.
    ///
    /// Indexing stops once the database reaches this size. Because candidates
    /// are indexed in priority order, the cap costs the *least* useful documents
    /// rather than whichever happened to come last.
    pub content_max_mb: u64,
    /// Largest individual file whose contents are read, in kilobytes.
    pub content_max_file_kb: u64,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            external_drives: false,
            refresh_hours: 6,
            roots: Vec::new(),
            ignore_names: Vec::new(),
            ignore_paths: Vec::new(),
            include_extensions: Vec::new(),
            exclude_extensions: Vec::new(),
            content: false,
            content_extensions: Vec::new(),
            content_max_mb: 512,
            content_max_file_kb: 2048,
        }
    }
}

impl FileConfig {
    /// Translate into the engine's configuration.
    #[must_use]
    pub fn to_core(&self) -> jump_core::files::Config {
        let mut roots: Vec<std::path::PathBuf> = dirs::home_dir().into_iter().collect();
        roots.extend(self.roots.iter().cloned());

        jump_core::files::Config {
            enabled: self.enabled,
            roots,
            ignore_names: self.ignore_names.clone(),
            ignore_paths: self.ignore_paths.clone(),
            include_extensions: self.include_extensions.clone(),
            exclude_extensions: self.exclude_extensions.clone(),
            external_drives: self.external_drives,
            refresh: std::time::Duration::from_secs(self.refresh_hours.max(1) * 3600),
        }
    }

    /// Limits handed to the content indexer.
    #[must_use]
    pub fn content_limits(&self) -> jump_core::content::Limits {
        jump_core::content::Limits {
            max_index_bytes: self.content_max_mb.max(16) * 1024 * 1024,
            max_file_bytes: self.content_max_file_kb.max(4) * 1024,
        }
    }
}

/// Which result providers answer a query.
///
/// File search has its own switch under [`FileConfig`] because it also
/// governs indexing, which is work that happens whether or not the launcher
/// is open. These six only decide whether a provider is consulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Providers {
    /// Open windows, matched by title and application id.
    pub windows: bool,
    /// Built-in commands: dark mode, COSMIC Settings pages, media, power.
    pub system: bool,
    /// Bluetooth devices and Wi-Fi networks.
    pub devices: bool,
    /// Clipboard history behind the `clip` keyword.
    pub clipboard: bool,
    /// Emoji behind the `emoji` keyword.
    pub emoji: bool,
    /// Quicklinks and the fallback web searches.
    pub web: bool,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            windows: true,
            system: true,
            devices: true,
            clipboard: true,
            emoji: true,
            web: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, CosmicConfigEntry)]
#[version = 1]
pub struct Config {
    /// Paging behaviour of the Launchpad grid.
    pub scroll: ScrollMode,
    /// Collapse the entrance, row cascade and page slide to plain fades.
    ///
    /// jump's own setting rather than a desktop-wide one: COSMIC 1.5 exposes
    /// no reduced-motion preference. When it does, this becomes the fallback
    /// for it rather than the only source.
    pub reduce_motion: bool,
    /// Ask the compositor to blur the backdrop behind the surface.
    ///
    /// Turning this off leaves the surface merely translucent, which shows the
    /// desktop behind it far more legibly — blur is what makes a translucent
    /// panel read as frosted glass rather than as a window you can see through.
    pub blur: bool,
    /// Fill opacity of the panel, 0.0..=1.0. Lower shows more of the desktop.
    pub opacity: f32,
    /// Fill opacity of the full-screen Launchpad backdrop.
    pub fullscreen_opacity: f32,
    pub grid_layout: GridLayout,
    /// Target width of one grid cell, in logical pixels.
    pub cell_size: f32,
    /// Fraction of the display width the full-screen grid may occupy.
    pub grid_max_width: f32,
    /// Ids of jump plugins the user has switched off.
    ///
    /// Stored as a list of directory names rather than per-plugin files so a
    /// deleted plugin leaves no orphaned setting behind.
    pub disabled_plugins: Vec<String>,
    /// File-search settings.
    pub files: FileConfig,
    /// Which providers are consulted at all.
    pub providers: Providers,
    /// Keyworded URL templates. `yt cats` opens the `yt` link's template with
    /// `{query}` replaced by `cats`, claiming the query the way a plugin
    /// keyword does.
    pub quicklinks: Vec<jump_core::web::Link>,
    /// Web searches appended below the results of an ordinary search, so a
    /// query that matched little still ends somewhere useful. The `keyword`
    /// field is ignored here.
    pub fallbacks: Vec<jump_core::web::Link>,
    /// Pinned result keys. When a pinned result matches a query at all, it
    /// ranks above everything unpinned. Written by the action panel's
    /// Pin/Unpin entries.
    pub favorites: Vec<String>,
    /// Per-plugin keyword overrides, `(plugin id, keyword)` — the alias
    /// mechanism. Replaces the manifest's keyword without editing the
    /// plugin; an empty keyword removes it, so the plugin runs on every
    /// query.
    pub plugin_keywords: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scroll: ScrollMode::default(),
            reduce_motion: false,
            blur: true,
            opacity: 0.42,
            fullscreen_opacity: 0.48,
            grid_layout: GridLayout::default(),
            cell_size: 158.0,
            grid_max_width: 0.72,
            disabled_plugins: Vec::new(),
            files: FileConfig::default(),
            providers: Providers::default(),
            quicklinks: Vec::new(),
            fallbacks: vec![
                jump_core::web::Link {
                    name: "DuckDuckGo".to_owned(),
                    keyword: String::new(),
                    template: "https://duckduckgo.com/?q={query}".to_owned(),
                },
                jump_core::web::Link {
                    name: "Wikipedia".to_owned(),
                    keyword: String::new(),
                    template: "https://en.wikipedia.org/wiki/Special:Search?search={query}"
                        .to_owned(),
                },
            ],
            favorites: Vec::new(),
            plugin_keywords: Vec::new(),
        }
    }
}

impl Config {
    /// Open the config store and read the current settings.
    ///
    /// A missing or partially-invalid store is never fatal: `get_entry` hands
    /// back defaults for whatever it could not read, and the launcher must still
    /// open.
    #[must_use]
    pub fn load() -> Self {
        let Ok(handle) = cosmic_config::Config::new(crate::APP_ID, Self::VERSION) else {
            tracing::warn!("cosmic-config unavailable; using default settings");
            return Self::default();
        };

        match Self::get_entry(&handle) {
            Ok(config) => config,
            Err((errors, config)) => {
                for error in errors {
                    tracing::warn!(%error, "using the default for an unreadable setting");
                }
                config
            }
        }
    }

    /// Clamp the configured cell size into a range that still lays out sanely.
    #[must_use]
    pub fn cell(&self) -> f32 {
        self.cell_size.clamp(96.0, 260.0)
    }

    /// Panel fill opacity, clamped so the surface can never become invisible
    /// or fully opaque.
    #[must_use]
    pub fn panel_opacity(&self) -> f32 {
        self.opacity.clamp(0.15, 0.95)
    }

    /// Full-screen backdrop opacity, clamped the same way.
    #[must_use]
    pub fn backdrop_opacity(&self) -> f32 {
        self.fullscreen_opacity.clamp(0.15, 0.95)
    }

    /// Persist the favorites list.
    ///
    /// One key, written directly: the daemon's own `watch_config`
    /// subscription then delivers the change back like any settings-window
    /// edit, so there is exactly one path by which configuration changes.
    pub fn write_favorites(&self) {
        use cosmic_config::ConfigSet;
        let Ok(handle) = cosmic_config::Config::new(crate::APP_ID, Self::VERSION) else {
            tracing::error!("cosmic-config unavailable; favorites not saved");
            return;
        };
        if let Err(error) = handle.set("favorites", self.favorites.clone()) {
            tracing::error!(%error, "could not write favorites");
        }
    }
}
