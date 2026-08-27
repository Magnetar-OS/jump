// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Search engine, ranking, and plugin host for the jump launcher.
//!
//! This crate holds everything a launcher frontend needs that is not drawing:
//! talking to the pop-launcher service, ranking results by usage, and running
//! user plugins. It has no UI dependencies, which is what lets the COSMIC
//! (libcosmic) and future GNOME (GTK4) frontends share one engine rather than
//! drifting into two half-implementations.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use jump_core::{Launcher, Frecency, PluginHost};
//!
//! let (launcher, mut events, _guard) = Launcher::spawn()?;
//! let plugins = PluginHost::discover();
//! let frecency = Frecency::load();
//!
//! launcher.search("fire");
//! while let Some(event) = events.recv().await {
//!     if let jump_core::Event::Update { items, .. } = event {
//!         // One scale for every provider, then usage as a tiebreak.
//!         let ranked = jump_core::rank::merge(items, "fire", &frecency);
//!         let _ = ranked;
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod content;
pub mod emoji;
pub mod files;
pub mod frecency;
pub mod launcher;
pub mod model;
pub mod plugin;
pub mod process;
pub mod rank;
pub mod web;

pub use content::Content;
pub use files::Files;
pub use frecency::Frecency;
pub use launcher::{Error, Event, Launcher, LauncherGuard};
pub use model::{Icon, Item, ItemKey, Results, Source};
pub use plugin::{Manifest, Plugin, PluginHost};

// Re-exported so frontends can speak the protocol's vocabulary (activation
// indices, GPU preference for desktop entries) without depending on
// pop-launcher directly.
pub use pop_launcher::{ContextOption, GpuPreference, Indice};
