// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! The result model shared by every frontend.
//!
//! The one design decision that matters here is [`ItemKey`]. pop-launcher
//! addresses results by [`Indice`], but that index is only meaningful for the
//! query that produced it — "Firefox" can be id 0 for `fire` and id 3 for `fi`.
//! A frontend that keys its list on the indice therefore sees every row change
//! identity on every keystroke, which forces a full list rebuild and makes
//! animating the difference impossible. That is exactly why cosmic-launcher
//! swaps its list instantly instead of transitioning.
//!
//! [`ItemKey`] is derived from properties that survive re-querying, so the same
//! program keeps the same key across searches. The view can then diff two result
//! sets and animate rows that moved, faded in, or faded out.

use std::borrow::Cow;

use pop_launcher::{Generation, IconSource as PopIcon, Indice, SearchResult};
use serde::{Deserialize, Serialize};

/// Identity of a result that is stable across queries.
///
/// Two [`Item`]s produced by different searches compare equal when they refer
/// to the same underlying thing, which is what lets the UI animate between
/// result sets rather than replacing them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemKey(pub String);

impl ItemKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ItemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a result came from. Determines how it is activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Produced by the pop-launcher service; activate by [`Indice`].
    Launcher,
    /// An open window, activated through the compositor rather than by
    /// launching anything. Carries only the compositor's identifier string, so
    /// the engine stays free of Wayland types.
    Window { identifier: String },
    /// A file on disk, opened with the user's default handler.
    File { path: std::path::PathBuf },
    /// A clipboard history entry; activating it copies the text back.
    Clipboard { text: String },
    /// Produced by a jump plugin; activate by running the plugin's action.
    Plugin {
        /// Directory name of the plugin that produced this item.
        plugin: String,
        /// Opaque payload handed back to the plugin on activation.
        arg: String,
        /// Alfred-style `variables`, exported into the activation command's
        /// environment. Sorted, so two equal items compare equal.
        variables: Vec<(String, String)>,
        /// Alternate actions on modifier+Enter, Alfred's `mods`.
        mods: Vec<Mod>,
    },
    /// A built-in command provided by the frontend — toggling dark mode,
    /// opening a settings page. Carries only an identifier so the engine stays
    /// free of desktop specifics; the frontend maps it to an action.
    System {
        /// Stable command identifier, e.g. `dark-mode`.
        id: String,
    },
    /// A running process; activating it sends `SIGTERM`.
    Process {
        /// Process id, valid only for the listing that produced it.
        pid: u32,
    },
    /// A web link — a quicklink or a fallback search; activating it opens the
    /// URL with the user's default handler.
    Url {
        /// Fully-expanded URL, query already encoded in.
        url: String,
    },
}

/// An alternate action on a plugin item, Alfred's `mods`.
///
/// Held on the item rather than looked up at activation time because the
/// plugin process that produced it is long gone by then: the payload has to
/// travel with the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mod {
    /// Modifier that selects it — `ctrl`, `alt`, `shift`, `super`. Kept as
    /// the plugin wrote it, lowercased, so the frontend owns the display
    /// spelling and the keybinding.
    pub modifier: String,
    /// Row text describing what this alternate does.
    pub subtitle: String,
    /// Payload handed to the plugin instead of the item's own `arg`.
    pub arg: String,
    /// Variables for this alternate, merged the same way the item's are.
    pub variables: Vec<(String, String)>,
}

/// An icon to render, resolved by the frontend against the active icon theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    /// An icon-theme name or an absolute path to an image.
    Name(String),
    /// A mime type whose generic icon should be shown.
    Mime(String),
}

impl Icon {
    fn from_pop(icon: PopIcon) -> Self {
        match icon {
            PopIcon::Name(name) => Self::Name(into_string(name)),
            PopIcon::Mime(mime) => Self::Mime(into_string(mime)),
        }
    }
}

fn into_string(cow: Cow<'static, str>) -> String {
    match cow {
        Cow::Borrowed(s) => s.to_owned(),
        Cow::Owned(s) => s,
    }
}

/// A single row in the result list.
#[derive(Debug, Clone)]
pub struct Item {
    /// Stable identity across queries. See the module docs.
    pub key: ItemKey,
    /// pop-launcher's per-query index. Only valid for the query that produced
    /// this item, so it is refreshed on every update rather than remembered.
    pub id: Indice,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<Icon>,
    /// Icon representing the plugin/category, shown trailing.
    pub category_icon: Option<Icon>,
    /// Set when the result refers to an open window rather than a launchable.
    pub window: Option<(Generation, Indice)>,
    pub source: Source,
    /// Text that replaces the query on Tab, Alfred's `autocomplete`.
    pub autocomplete: Option<String>,
    /// Ranking score after frecency is applied. Higher sorts first.
    pub score: f32,
}

impl Item {
    /// Build an [`Item`] from a pop-launcher search result.
    #[must_use]
    pub fn from_search_result(result: SearchResult) -> Self {
        let SearchResult {
            id,
            name,
            description,
            icon,
            category_icon,
            window,
        } = result;

        let key = Self::derive_key(&name, &description, window);

        Self {
            key,
            id,
            title: name,
            subtitle: description,
            icon: icon.map(Icon::from_pop),
            category_icon: category_icon.map(Icon::from_pop),
            window,
            source: Source::Launcher,
            // pop-launcher completion goes through Request::Complete instead.
            autocomplete: None,
            score: 0.0,
        }
    }

    /// Windows are keyed by their compositor handle because two windows of the
    /// same application share a title and description; everything else is keyed
    /// by name plus description, which together identify a desktop entry well
    /// enough to survive re-querying.
    fn derive_key(name: &str, description: &str, window: Option<(Generation, Indice)>) -> ItemKey {
        match window {
            Some((generation, indice)) => ItemKey(format!("window:{generation}:{indice}")),
            None => ItemKey(format!("entry:{name}\u{1f}{description}")),
        }
    }

    /// Whether activating this item should dismiss the launcher.
    ///
    /// Calculator results are the notable exception: the user usually wants to
    /// copy the value and keep going.
    #[must_use]
    pub fn dismisses_on_activate(&self) -> bool {
        true
    }
}

/// A complete result set for one query, plus the query that produced it.
#[derive(Debug, Clone, Default)]
pub struct Results {
    /// Monotonic query generation. Used to discard responses that arrive after
    /// the user has already typed something newer.
    pub seq: u64,
    pub query: String,
    pub items: Vec<Item>,
}

impl Results {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Position of `key` in this result set, if present.
    ///
    /// The view uses this to decide whether a row moved (animate to the new
    /// position) or is new (fade in).
    #[must_use]
    pub fn position_of(&self, key: &ItemKey) -> Option<usize> {
        self.items.iter().position(|item| &item.key == key)
    }
}
