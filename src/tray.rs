// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Status-area icon, via `StatusNotifierItem`.
//!
//! COSMIC has no tray protocol of its own; `cosmic-applet-status-area` owns
//! `org.kde.StatusNotifierWatcher` on the session bus and renders whatever
//! registers with it — verified live, alongside other applications already
//! doing exactly this. That makes SNI the portable choice rather than a
//! COSMIC-specific one: the same code appears in KDE's tray and in GNOME's via
//! the AppIndicator extension, with no per-desktop branches here.
//!
//! The alternative would be a `cosmic-panel` applet, which is a separate binary
//! the user must add to their panel configuration by hand. That is more native
//! and considerably less discoverable.

use tokio::sync::mpsc;

/// What the user asked for from the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show the launcher, or hide it if already up.
    Toggle,
    /// Show Launchpad specifically.
    ShowLaunchpad,
    /// Open the settings window.
    Settings,
    /// Rebuild the file index now rather than waiting for the refresh interval.
    RebuildIndex,
    /// Forget everything in clipboard history.
    ClearClipboard,
    /// Shut the daemon down.
    Quit,
}

struct Tray {
    actions: mpsc::UnboundedSender<Action>,
}

impl Tray {
    fn send(&self, action: Action) {
        if self.actions.send(action).is_err() {
            tracing::debug!("tray action dropped; the application is shutting down");
        }
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        jump::APP_ID.to_owned()
    }

    fn title(&self) -> String {
        jump::fl!("app-title")
    }

    fn icon_name(&self) -> String {
        // Symbolic so the panel can recolour it for light and dark themes.
        "system-search-symbolic".to_owned()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: jump::fl!("app-title"),
            description: jump::fl!("app-description"),
            icon_name: "system-search-symbolic".to_owned(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Toggle);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};

        vec![
            StandardItem {
                label: jump::fl!("tray-search"),
                icon_name: "system-search-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Toggle)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: jump::fl!("tray-applications"),
                icon_name: "view-app-grid-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::ShowLaunchpad)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: jump::fl!("tray-settings"),
                icon_name: "preferences-system-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Settings)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: jump::fl!("tray-rebuild-index"),
                icon_name: "view-refresh-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::RebuildIndex)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: jump::fl!("tray-clear-clipboard"),
                icon_name: "edit-clear-all-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::ClearClipboard)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: jump::fl!("tray-quit"),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Register the tray icon and stream the actions it produces.
///
/// Returns `None` when no status-notifier host is running, which is the normal
/// case on a bare compositor with no panel. The launcher works identically
/// without it, so this is not treated as an error.
pub async fn spawn() -> Option<mpsc::UnboundedReceiver<Action>> {
    use ksni::TrayMethods;

    let (actions, receiver) = mpsc::unbounded_channel();

    match (Tray { actions }).spawn().await {
        Ok(handle) => {
            // The handle owns the D-Bus registration; dropping it would remove
            // the icon, so it is deliberately leaked for the process lifetime.
            std::mem::forget(handle);
            tracing::info!("status area icon registered");
            Some(receiver)
        }
        Err(error) => {
            tracing::info!(%error, "no status notifier host; tray icon unavailable");
            None
        }
    }
}
