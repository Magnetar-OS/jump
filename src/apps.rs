// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! The installed-application list behind Launchpad mode.
//!
//! This deliberately does not go through pop-launcher. The service answers an
//! empty query with an empty list — verified, not assumed — because it is a
//! *search* service: it ranks against a query and has nothing to say without
//! one. Launchpad needs the opposite, the complete set in a stable order, so it
//! reads desktop entries directly.
//!
//! It also lives in the frontend rather than in `jump-core`, because
//! `cosmic::desktop` already implements XDG entry precedence, locale handling,
//! `OnlyShowIn`/`NotShowIn` filtering, and `Exec` field-code expansion. A GTK
//! frontend would reach for gio's equivalent rather than share this.

use cosmic::desktop::{DesktopEntryData, IconSourceExt, load_applications};
use cosmic::widget::icon;
use jump_core::{GpuPreference, ItemKey};

use crate::launch::Launch;

/// One launchable application.
#[derive(Debug, Clone)]
pub struct App {
    /// Desktop file id, e.g. `firefox.desktop`.
    pub id: String,
    pub name: String,
    /// Pre-resolved icon handle. Resolving happens once at load rather than per
    /// frame — the grid draws every installed application at once, so doing it
    /// in the view would mean hundreds of theme lookups per redraw.
    pub icon: icon::Handle,
    exec: Option<String>,
    terminal: bool,
    /// Whether the entry asks for the discrete GPU.
    prefers_dgpu: bool,
}

impl App {
    /// Key matching the one [`jump_core`] derives for search results, so a
    /// launch from the grid feeds the same frecency history as a launch from
    /// the result list.
    #[must_use]
    pub fn key(&self) -> ItemKey {
        ItemKey(format!("app:{}", self.id))
    }

    /// Describe how to start this application.
    ///
    /// Starting it is deliberately not done here: it needs an activation token,
    /// which only the frontend can request because it has to be bound to the
    /// overlay's surface. See [`crate::launch`].
    ///
    /// `None` when the entry has no `Exec` line, which is legal for entries
    /// that exist only to be a MIME handler target.
    #[must_use]
    pub fn launch(&self) -> Option<Launch> {
        let Some(exec) = self.exec.clone() else {
            tracing::warn!(app = %self.id, "desktop entry has no Exec line");
            return None;
        };

        Some(Launch {
            app_id: self.id.clone(),
            exec,
            terminal: self.terminal,
            gpu: if self.prefers_dgpu {
                GpuPreference::NonDefault
            } else {
                GpuPreference::Default
            },
        })
    }
}

/// Load every visible application, sorted by name.
///
/// Sorting is by lowercased name so the grid does not put every capitalised
/// entry ahead of the rest, which is what a plain byte-order sort does.
#[must_use]
pub fn load() -> Vec<App> {
    let locales = cosmic::desktop::fde::get_languages_from_env();
    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();

    let mut apps: Vec<App> = load_applications(&locales, false, current_desktop.as_deref())
        .map(App::from_entry)
        .collect();

    apps.sort_by_key(|app| app.name.to_lowercase());
    tracing::info!(count = apps.len(), "loaded applications");
    apps
}

impl App {
    fn from_entry(entry: DesktopEntryData) -> Self {
        Self {
            icon: entry.icon.as_cosmic_icon(),
            id: entry.id,
            name: entry.name,
            exec: entry.exec,
            terminal: entry.terminal,
            prefers_dgpu: entry.prefers_dgpu,
        }
    }
}
