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
}

/// The open panel: its actions and which one is highlighted.
#[derive(Debug, Clone)]
pub struct Panel {
    pub actions: Vec<Action>,
    pub selected: usize,
}

impl Panel {
    /// Build the panel for `item`, or `None` when the only thing the item can
    /// do is what Enter already does — a panel with one redundant entry is
    /// noise, except for launcher items, whose context options arrive late.
    #[must_use]
    pub fn for_item(item: &Item) -> Option<Self> {
        let actions = actions_for(item);
        let worthwhile =
            actions.len() > 1 || matches!(item.source, Source::Launcher);
        worthwhile.then_some(Self {
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
                kind: Kind::LauncherContext {
                    item,
                    option: option.id,
                },
            }));
    }
}

/// The action set for one item, primary first.
fn actions_for(item: &Item) -> Vec<Action> {
    let primary = |label: String, icon| Action {
        label,
        icon,
        kind: Kind::Primary,
    };

    match &item.source {
        Source::File { path } => {
            let mut actions = vec![primary(fl!("action-open"), "document-open-symbolic")];

            if let Some(parent) = path.parent() {
                actions.push(Action {
                    label: fl!("action-open-folder"),
                    icon: "folder-open-symbolic",
                    kind: Kind::OpenFolder(parent.to_path_buf()),
                });
            }
            actions.push(Action {
                label: fl!("action-copy-path"),
                icon: "edit-copy-symbolic",
                kind: Kind::CopyText(path.display().to_string()),
            });
            if trash_available() {
                actions.push(Action {
                    label: fl!("action-trash"),
                    icon: "user-trash-symbolic",
                    kind: Kind::Trash(path.clone()),
                });
            }
            actions
        }

        Source::Window { .. } => vec![
            primary(fl!("action-switch-window"), "focus-windows-symbolic"),
            Action {
                label: fl!("action-close-window"),
                icon: "window-close-symbolic",
                kind: Kind::CloseWindow,
            },
        ],

        Source::Clipboard { text } => vec![Action {
            label: fl!("action-copy"),
            icon: "edit-copy-symbolic",
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
                kind: Kind::ForceKill { pid: *pid },
            },
        ],

        Source::Plugin { .. } | Source::System { .. } => {
            vec![primary(fl!("action-run"), "system-run-symbolic")]
        }
    }
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
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
    })
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
        let panel = Panel::for_item(&item(Source::File {
            path: PathBuf::from("/home/user/notes/todo.md"),
        }))
        .expect("files have a panel");

        assert!(matches!(panel.actions[0].kind, Kind::Primary));
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::OpenFolder(PathBuf::from("/home/user/notes")))
        );
        assert!(panel.actions.iter().any(
            |action| action.kind == Kind::CopyText("/home/user/notes/todo.md".to_owned())
        ));
    }

    #[test]
    fn windows_offer_close() {
        let panel = Panel::for_item(&item(Source::Window {
            identifier: "w1".into(),
        }))
        .expect("windows have a panel");
        assert!(
            panel
                .actions
                .iter()
                .any(|action| action.kind == Kind::CloseWindow)
        );
    }

    #[test]
    fn single_action_sources_get_no_panel() {
        assert!(
            Panel::for_item(&item(Source::System {
                id: "dark-mode".into()
            }))
            .is_none()
        );
    }

    #[test]
    fn launcher_items_get_a_panel_for_late_context_options() {
        let mut panel =
            Panel::for_item(&item(Source::Launcher)).expect("launcher items have a panel");
        assert_eq!(panel.actions.len(), 1);

        panel.extend_with_context(
            7,
            [jump_core::ContextOption {
                id: 0,
                name: "New Window".into(),
            }],
        );
        assert_eq!(panel.actions.len(), 2);
        assert!(matches!(
            panel.actions[1].kind,
            Kind::LauncherContext { item: 7, option: 0 }
        ));
    }

    #[test]
    fn processes_offer_force_kill() {
        let panel = Panel::for_item(&item(Source::Process { pid: 1234 }))
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
        let mut panel = Panel::for_item(&item(Source::Window {
            identifier: "w1".into(),
        }))
        .expect("panel");

        panel.shift(-3);
        assert_eq!(panel.selected, 0);
        panel.shift(10);
        assert_eq!(panel.selected, panel.actions.len() - 1);
    }
}
