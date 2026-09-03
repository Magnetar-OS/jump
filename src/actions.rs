// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! The action panel: what else a result can do.
//!
//! Enter runs a result's obvious action. The panel is the second dimension —
//! Ctrl+K on a selected result opens a small list of everything else it can
//! do: reveal a file, copy its path, trash it, close a window, run one of an
//! application's desktop actions. This is the interaction that separates a
//! launcher from a command surface, and it is deliberately keyboard-first:
//! Ctrl+K, arrows, Enter, Escape.
//!
//! Actions are derived from the item's [`Source`], so every provider gets a
//! sensible set with no per-provider UI code. Application results additionally
//! pull pop-launcher's context options — its desktop actions, "New Window" and
//! friends — which arrive asynchronously and are appended to an open panel.

use std::path::{Path, PathBuf};

use jump_core::{Indice, Item, Source};

use jump::fl;

/// One entry in the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub label: String,
    /// Symbolic icon-theme name.
    pub icon: &'static str,
    /// Key hint shown at the trailing edge of the row — "Enter", "Ctrl ↵".
    /// How anybody discovers the shortcut exists at all.
    pub shortcut: Option<String>,
    pub kind: Kind,
}

/// What running an action does. Interpreted by the update loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Whatever Enter on the row would have done.
    Primary,
    /// Open the directory containing this file.
    OpenFolder(PathBuf),
    /// Put text on the clipboard.
    CopyText(String),
    /// Move a file to the trash.
    Trash(PathBuf),
    /// Close the window this result refers to.
    CloseWindow,
    /// One of pop-launcher's context options for this item — an application's
    /// desktop actions, mostly.
    LauncherContext { item: Indice, option: Indice },
    /// `SIGKILL` the process this result refers to — the escalation for one
    /// that ignored the polite request.
    ForceKill { pid: u32 },
    /// Pin this result above everything unpinned, or unpin it.
    TogglePin { key: jump_core::ItemKey },
    /// A compositor request against the window this result refers to.
    Window(WindowCommand),
    /// A plugin item's alternate action, Alfred's `mods`.
    PluginMod {
        plugin: String,
        arg: String,
        variables: Vec<(String, String)>,
    },
}

/// Management requests the compositor honours for a window, chosen from its
/// current state — the panel offers Restore on a maximized window, never a
/// blind toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    Maximize,
    Unmaximize,
    Minimize,
    Fullscreen,
    Unfullscreen,
}

/// The open panel: its actions and which one is highlighted.
#[derive(Debug, Clone)]
pub struct Panel {
    pub actions: Vec<Action>,
    pub selected: usize,
}

impl Panel {
    /// Build the panel for `item`. `pinned` is whether the item is currently
    /// a favorite, which flips the Pin entry to Unpin; `window` is the
    /// current compositor state when the item is a window, which decides
    /// Maximize against Restore.
    ///
    /// Every panel now carries at least Pin next to the primary action, so —
    /// unlike earlier versions — no source is left without one.
    #[must_use]
    pub fn for_item(
        item: &Item,
        pinned: bool,
        window: Option<&crate::toplevel::Window>,
    ) -> Option<Self> {
        let mut actions = actions_for(item, window);
        actions.push(Action {
            label: if pinned {
                fl!("action-unpin")
            } else {
                fl!("action-pin")
            },
            icon: "pin-symbolic",
            shortcut: None,
            kind: Kind::TogglePin {
                key: item.key.clone(),
            },
        });
        Some(Self {
            actions,
            selected: 0,
        })
    }

    /// Move the highlight by a signed step, saturating at the ends.
    pub fn shift(&mut self, delta: i32) {
        let last = self.actions.len().saturating_sub(1);
        self.selected = if delta.is_negative() {
            self.selected.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (self.selected + delta as usize).min(last)
        };
    }

    /// Append pop-launcher context options as they arrive.
    pub fn extend_with_context(
        &mut self,
        item: Indice,
        options: impl IntoIterator<Item = jump_core::ContextOption>,
    ) {
        self.actions
            .extend(options.into_iter().map(|option| Action {
                label: option.name,
                icon: "view-more-symbolic",
                shortcut: None,
                kind: Kind::LauncherContext {
                    item,
                    option: option.id,
                },
            }));
    }
}

/// The action set for one item, primary first.
fn actions_for(item: &Item, window: Option<&crate::toplevel::Window>) -> Vec<Action> {
    let primary = |label: String, icon| Action {
        label,
        icon,
        shortcut: Some(fl!("key-enter")),
        kind: Kind::Primary,
    };

    match &item.source {
        Source::File { path } => {
            let mut actions = vec![primary(fl!("action-open"), "document-open-symbolic")];

            if let Some(parent) = path.parent() {
                actions.push(Action {
                    label: fl!("action-open-folder"),
                    icon: "folder-open-symbolic",
                    shortcut: None,
                    kind: Kind::OpenFolder(parent.to_path_buf()),
                });
            }
            actions.push(Action {
                label: fl!("action-copy-path"),
                icon: "edit-copy-symbolic",
                shortcut: None,
                kind: Kind::CopyText(path.display().to_string()),
            });
            if trash_available() {
                actions.push(Action {
                    label: fl!("action-trash"),
                    icon: "user-trash-symbolic",
                    shortcut: None,
                    kind: Kind::Trash(path.clone()),
                });
            }
            actions
        }

        Source::Window { .. } => {
            let mut actions = vec![primary(
                fl!("action-switch-window"),
                "focus-windows-symbolic",
            )];

            // State-aware management entries. Offered only when the state is
            // known — a compositor speaking just the foreign-toplevel list
            // shows windows but cannot act on them, and offering requests it
            // would drop is worse than a shorter panel.
            if let Some(window) = window {
                if window.maximized {
                    actions.push(Action {
                        label: fl!("action-restore-window"),
                        icon: "window-restore-symbolic",
                        shortcut: None,
                        kind: Kind::Window(WindowCommand::Unmaximize),
                    });
                } else {
                    actions.push(Action {
                        label: fl!("action-maximize-window"),
                        icon: "window-maximize-symbolic",
                        shortcut: None,
                        kind: Kind::Window(WindowCommand::Maximize),
                    });
                }
                actions.push(Action {
                    label: fl!("action-minimize-window"),
                    icon: "window-minimize-symbolic",
                    shortcut: None,
                    kind: Kind::Window(WindowCommand::Minimize),
                });
                if window.fullscreen {
                    actions.push(Action {
                        label: fl!("action-exit-fullscreen"),
                        icon: "view-restore-symbolic",
                        shortcut: None,
                        kind: Kind::Window(WindowCommand::Unfullscreen),
                    });
                } else {
                    actions.push(Action {
                        label: fl!("action-fullscreen"),
                        icon: "view-fullscreen-symbolic",
                        shortcut: None,
                        kind: Kind::Window(WindowCommand::Fullscreen),
                    });
                }
            }

            actions.push(Action {
                label: fl!("action-close-window"),
                icon: "window-close-symbolic",
                shortcut: None,
                kind: Kind::CloseWindow,
            });
            actions
        }

        Source::Clipboard { text } => vec![Action {
            label: fl!("action-copy"),
            icon: "edit-copy-symbolic",
            shortcut: None,
            kind: Kind::CopyText(text.clone()),
        }],

        // Context options are requested when the panel opens and appended as
        // pop-launcher answers.
        Source::Launcher => vec![primary(fl!("action-open"), "document-open-symbolic")],

        Source::Process { pid } => vec![
            primary(fl!("action-end-process"), "process-stop-symbolic"),
            Action {
                label: fl!("action-force-kill"),
                icon: "edit-delete-symbolic",
                shortcut: None,
                kind: Kind::ForceKill { pid: *pid },
            },
        ],

        Source::Url { url } => vec![
            primary(fl!("action-open"), "web-browser-symbolic"),
            Action {
                label: fl!("action-copy-link"),
                icon: "edit-copy-symbolic",
                shortcut: None,
                kind: Kind::CopyText(url.clone()),
            },
        ],

        Source::Plugin { plugin, mods, .. } => {
            let mut actions = vec![primary(fl!("action-run"), "system-run-symbolic")];
            // Alternates the plugin declared, each addressable by its own
            // modifier as well as from this list.
            actions.extend(mods.iter().map(|alternate| Action {
                label: if alternate.subtitle.is_empty() {
                    fl!("action-run")
                } else {
                    alternate.subtitle.clone()
                },
                icon: "system-run-symbolic",
                shortcut: Some(modifier_label(&alternate.modifier)),
                kind: Kind::PluginMod {
                    plugin: plugin.clone(),
                    arg: alternate.arg.clone(),
                    variables: alternate.variables.clone(),
                },
            }));
            actions
        }

        Source::System { .. } => vec![primary(fl!("action-run"), "system-run-symbolic")],
    }
}

/// The key hint for a plugin modifier, e.g. `ctrl` → "Ctrl ↵".
fn modifier_label(modifier: &str) -> String {
    let name = match modifier {
        "ctrl" => "Ctrl",
        "alt" => "Alt",
        "shift" => "Shift",
        "super" => "Super",
        other => other,
    };
    format!("{name} ↵")
}

/// The alternate a modifier selects on this item, if any.
///
/// This is the modifier+Enter path: the panel need not be open for an
/// alternate to be reachable, which is the whole point of `mods`.
#[must_use]
pub fn mod_for(item: &Item, modifier: &str) -> Option<Kind> {
    let Source::Plugin { plugin, mods, .. } = &item.source else {
        return None;
    };
    mods.iter()
        .find(|alternate| alternate.modifier == modifier)
        .map(|alternate| Kind::PluginMod {
            plugin: plugin.clone(),
            arg: alternate.arg.clone(),
            variables: alternate.variables.clone(),
        })
}

/// Whether `gio trash` is available. Checked once: the answer cannot change
/// without the user installing software mid-session, and a stale `false` costs
/// one menu entry until restart.
fn trash_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| which("gio"))
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Move `path` to the trash, detached from the frame.
pub fn trash(path: &Path) {
    let result = std::process::Command::new("gio")
        .arg("trash")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::error!(%error, ?path, "could not trash file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jump_core::ItemKey;

    fn item(source: Source) -> Item {
        Item {
            key: ItemKey("test".into()),
            id: 0,
            title: "Test".into(),
            subtitle: String::new(),
            icon: None,
            category_icon: None,
            window: None,
            source,
            autocomplete: None,
            score: 0.0,
        }
    }

    #[test]
    fn files_offer_reveal_and_copy_path() {
        let panel = Panel::for_item(
            &item(Source::File {
                path: PathBuf::from("/home/user/notes/todo.md"),
            }),
            false,
            None,
        )
        .expect("files have a panel");

        assert!(matches!(panel.actions[0].kind, Kind::Primary));
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::OpenFolder(PathBuf::from("/home/user/notes")))
        );
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::CopyText("/home/user/notes/todo.md".to_owned()))
        );
    }

    #[test]
    fn windows_offer_close() {
        let window = crate::toplevel::Window {
            identifier: "w1".into(),
            title: "Report".into(),
            app_id: "org.example".into(),
            maximized: false,
            fullscreen: false,
        };
        let panel = Panel::for_item(
            &item(Source::Window {
                identifier: "w1".into(),
            }),
            false,
            Some(&window),
        )
        .expect("windows have a panel");
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::CloseWindow)
        );
    }

    #[test]
    fn window_commands_follow_the_window_state() {
        let win = |maximized, fullscreen| crate::toplevel::Window {
            identifier: "w1".into(),
            title: "Report".into(),
            app_id: "org.example".into(),
            maximized,
            fullscreen,
        };
        let result = item(Source::Window {
            identifier: "w1".into(),
        });

        let plain = Panel::for_item(&result, false, Some(&win(false, false))).expect("panel");
        assert!(
            plain
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Maximize))
        );
        assert!(
            plain
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Fullscreen))
        );
        assert!(
            plain
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Minimize))
        );

        let maxed = Panel::for_item(&result, false, Some(&win(true, true))).expect("panel");
        assert!(
            maxed
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Unmaximize))
        );
        assert!(
            maxed
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Unfullscreen))
        );
        assert!(
            !maxed
                .actions
                .iter()
                .any(|a| a.kind == Kind::Window(WindowCommand::Maximize))
        );

        // Unknown state: only switch and close — no blind toggles.
        let unknown = Panel::for_item(&result, false, None).expect("panel");
        assert!(
            !unknown
                .actions
                .iter()
                .any(|a| matches!(a.kind, Kind::Window(_)))
        );
    }

    #[test]
    fn every_panel_offers_pin_and_pinned_items_offer_unpin() {
        let system = item(Source::System {
            id: "dark-mode".into(),
        });
        let panel = Panel::for_item(&system, false, None).expect("system items have a panel");
        assert!(matches!(panel.actions[0].kind, Kind::Primary));
        assert!(
            panel.actions.iter().any(
                |action| matches!(&action.kind, Kind::TogglePin { key } if key == &system.key)
            )
        );

        // Same entry, different label when already pinned; the kind is what
        // the update loop acts on either way.
        let pinned = Panel::for_item(&system, true, None).expect("panel");
        assert!(
            pinned
                .actions
                .iter()
                .any(|action| matches!(&action.kind, Kind::TogglePin { .. }))
        );
    }

    #[test]
    fn launcher_items_get_a_panel_for_late_context_options() {
        let mut panel = Panel::for_item(&item(Source::Launcher), false, None)
            .expect("launcher items have a panel");
        assert_eq!(panel.actions.len(), 2); // primary + pin

        panel.extend_with_context(
            7,
            [jump_core::ContextOption {
                id: 0,
                name: "New Window".into(),
            }],
        );
        assert_eq!(panel.actions.len(), 3);
        assert!(matches!(
            panel.actions[2].kind,
            Kind::LauncherContext { item: 7, option: 0 }
        ));
    }

    #[test]
    fn processes_offer_force_kill() {
        let panel = Panel::for_item(&item(Source::Process { pid: 1234 }), false, None)
            .expect("processes have a panel");
        assert!(matches!(panel.actions[0].kind, Kind::Primary));
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::ForceKill { pid: 1234 })
        );
    }

    #[test]
    fn shift_saturates_at_both_ends() {
        let mut panel = Panel::for_item(
            &item(Source::Window {
                identifier: "w1".into(),
            }),
            false,
            None,
        )
        .expect("panel");

        panel.shift(-3);
        assert_eq!(panel.selected, 0);
        panel.shift(10);
        assert_eq!(panel.selected, panel.actions.len() - 1);
    }
}
