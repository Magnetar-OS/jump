// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Settings window.
//!
//! A separate binary rather than a page inside the launcher, for two reasons.
//! The launcher has no ordinary window at all — it is a layer-shell overlay with
//! exclusive keyboard focus — so hosting a settings form inside it would mean
//! building a second kind of surface into a process whose whole design is
//! "one overlay". And COSMIC Settings has no mechanism for third-party pages, so
//! there is nowhere else for this to live.
//!
//! The two processes never talk to each other. This window writes
//! `cosmic-config`; the launcher subscribes to the same store and applies
//! changes live. That is the entire integration, and it is why every control
//! here takes effect the moment it is touched, with no apply button.

use std::sync::LazyLock;

use cosmic::app::{Core, Settings, Task};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Length, Subscription};
use cosmic::widget::{self, settings};
use cosmic::{Apply, Element};
use jump::config::{Config, GridLayout, ScrollMode};
use jump::fl;
use jump_core::PluginHost;

const ID: &str = "com.magnetaros.JumpSettings";

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,jump_settings=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Before the application is constructed: `fl!` is evaluated during `init`,
    // and the dropdown label statics resolve on first view.
    jump::localize::localize();

    let settings = Settings::default()
        .size(cosmic::iced::Size::new(680.0, 720.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(480.0)
                .min_height(400.0),
        );

    cosmic::app::run::<App>(settings, ())
}

struct App {
    core: Core,
    /// Handle on the store. `None` when cosmic-config is unavailable, in which
    /// case the window still renders but cannot persist anything.
    handle: Option<cosmic_config::Config>,
    config: Config,
    /// Discovered jump plugins, listed so each can be switched off. Kept
    /// current by watching the plugin directories.
    plugins: Vec<(String, String, Option<String>)>,
}

#[derive(Debug, Clone)]
enum Message {
    Blur(bool),
    ReduceMotion(bool),
    /// Turn one search provider on or off.
    Provider(Provider, bool),
    Opacity(f32),
    BackdropOpacity(f32),
    Layout(usize),
    Scroll(usize),
    CellSize(f32),
    GridWidth(f32),
    FilesEnabled(bool),
    ExternalDrives(bool),
    Content(bool),
    ContentMaxMb(f32),
    RefreshHours(f32),
    /// Enable or disable the plugin with this id.
    PluginEnabled(String, bool),
    /// Override the keyword the plugin with this id answers to. An empty
    /// string clears the override, restoring the manifest's own keyword.
    PluginKeyword(String, String),
    /// Unpin the favorite with this result key.
    UnpinFavorite(String),
    /// The store changed — here, in the launcher (a pin), or by hand.
    ConfigChanged(Box<Config>),
    /// A plugin was installed, removed or edited.
    PluginsDiscovered(PluginHost),
}

/// Which provider a [`Message::Provider`] refers to.
///
/// A small enum rather than six message variants: the update arm is then one
/// match instead of six near-identical assignments, and adding a provider is
/// one line in each of three places rather than a new message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Windows,
    System,
    Devices,
    Clipboard,
    Emoji,
    Web,
    Calculator,
}

/// Labels for the layout dropdown, in the order the variants are offered.
///
/// `widget::dropdown` borrows its label slice for the lifetime of the element
/// it returns, so these cannot be built inside `view`. A `LazyLock` resolved
/// after `localize()` has run is the idiom that works.
static LAYOUTS: LazyLock<Vec<String>> =
    LazyLock::new(|| vec![fl!("layout-fullscreen"), fl!("layout-panel")]);
static SCROLLS: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        fl!("scroll-continuous"),
        fl!("scroll-pages-horizontal"),
        fl!("scroll-pages-vertical"),
    ]
});

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, (): Self::Flags) -> (Self, Task<Self::Message>) {
        let handle = cosmic_config::Config::new(jump::APP_ID, Config::VERSION).ok();
        let config = Config::load();

        let plugins = listed(&PluginHost::discover());

        (
            Self {
                core,
                handle,
                config,
                plugins,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let mut next = self.config.clone();
        match message {
            Message::Blur(value) => next.blur = value,
            Message::ReduceMotion(value) => next.reduce_motion = value,
            Message::Provider(provider, value) => {
                let providers = &mut next.providers;
                match provider {
                    Provider::Windows => providers.windows = value,
                    Provider::System => providers.system = value,
                    Provider::Devices => providers.devices = value,
                    Provider::Clipboard => providers.clipboard = value,
                    Provider::Emoji => providers.emoji = value,
                    Provider::Web => providers.web = value,
                    Provider::Calculator => providers.calculator = value,
                }
            }
            Message::Opacity(value) => next.opacity = value,
            Message::BackdropOpacity(value) => next.fullscreen_opacity = value,
            Message::CellSize(value) => next.cell_size = value,
            Message::GridWidth(value) => next.grid_max_width = value,
            Message::Layout(index) => {
                next.grid_layout = match index {
                    1 => GridLayout::Panel,
                    _ => GridLayout::Fullscreen,
                };
            }
            Message::Scroll(index) => {
                next.scroll = match index {
                    0 => ScrollMode::Continuous,
                    2 => ScrollMode::PageVertical,
                    _ => ScrollMode::PageHorizontal,
                };
            }
            Message::FilesEnabled(value) => next.files.enabled = value,
            Message::ExternalDrives(value) => next.files.external_drives = value,
            Message::Content(value) => next.files.content = value,
            Message::ContentMaxMb(value) => next.files.content_max_mb = value as u64,
            Message::RefreshHours(value) => next.files.refresh_hours = value as u64,
            Message::PluginKeyword(id, keyword) => {
                let keyword = keyword.trim().to_owned();
                // The same rule `jump plugin lint` applies to a manifest: a
                // keyword with a space in it could never match, so the space
                // is not taken rather than saved.
                if !keyword.is_empty() && !jump_core::plugin::is_keyword(&keyword) {
                    return Task::none();
                }
                next.plugin_keywords.retain(|(entry, _)| entry != &id);
                // An empty field means "no override", not "no keyword": the
                // manifest's own keyword comes back rather than the plugin
                // silently starting to answer every keystroke.
                if !keyword.is_empty() {
                    next.plugin_keywords.push((id, keyword));
                }
            }

            Message::UnpinFavorite(key) => {
                next.favorites.retain(|entry| entry != &key);
            }

            Message::PluginEnabled(id, enabled) => {
                if enabled {
                    next.disabled_plugins.retain(|entry| entry != &id);
                } else if !next.disabled_plugins.contains(&id) {
                    next.disabled_plugins.push(id);
                }
            }
            // Kept current so each change starts from what is stored now, not
            // from what was stored when the window opened. Nothing to write.
            Message::ConfigChanged(config) => {
                self.config = *config;
                return Task::none();
            }
            Message::PluginsDiscovered(host) => {
                self.plugins = listed(&host);
                return Task::none();
            }
        }

        self.save(next);
        Task::none()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            self.core()
                .watch_config::<Config>(jump::APP_ID)
                .map(|update| {
                    for error in update.errors {
                        tracing::warn!(%error, "ignoring an unreadable setting");
                    }
                    Message::ConfigChanged(Box::new(update.config))
                }),
            Subscription::run(jump::plugins::watch).map(Message::PluginsDiscovered),
        ])
    }

    // One section builder per settings group; splitting it would only scatter
    // the window's layout. The same allowance every COSMIC app makes.
    #[allow(clippy::too_many_lines)]
    fn view(&self) -> Element<'_, Self::Message> {
        let appearance = settings::section()
            .title(fl!("section-appearance"))
            .add(settings::item(
                fl!("background-blur"),
                widget::toggler(self.config.blur).on_toggle(Message::Blur),
            ))
            .add(settings::item(
                fl!("reduce-motion"),
                widget::toggler(self.config.reduce_motion).on_toggle(Message::ReduceMotion),
            ))
            .add(settings::item(
                fl!("panel-opacity"),
                slider(self.config.opacity, 0.15, 0.95, Message::Opacity),
            ))
            .add(settings::item(
                fl!("backdrop-opacity"),
                slider(
                    self.config.fullscreen_opacity,
                    0.15,
                    0.95,
                    Message::BackdropOpacity,
                ),
            ));

        let launchpad = settings::section()
            .title(fl!("section-launchpad"))
            .add(settings::item(
                fl!("layout"),
                widget::dropdown(
                    LAYOUTS.as_slice(),
                    Some(match self.config.grid_layout {
                        GridLayout::Fullscreen => 0,
                        GridLayout::Panel => 1,
                    }),
                    Message::Layout,
                ),
            ))
            .add(settings::item(
                fl!("scrolling"),
                widget::dropdown(
                    SCROLLS.as_slice(),
                    Some(match self.config.scroll {
                        ScrollMode::Continuous => 0,
                        ScrollMode::PageHorizontal => 1,
                        ScrollMode::PageVertical => 2,
                    }),
                    Message::Scroll,
                ),
            ))
            .add(settings::item(
                fl!("icon-size"),
                slider(self.config.cell_size, 96.0, 260.0, Message::CellSize),
            ))
            .add(settings::item(
                fl!("grid-width"),
                slider(self.config.grid_max_width, 0.3, 1.0, Message::GridWidth),
            ));

        let files = settings::section()
            .title(fl!("section-files"))
            .add(settings::item(
                fl!("files-by-name"),
                widget::toggler(self.config.files.enabled).on_toggle(Message::FilesEnabled),
            ))
            .add(settings::item(
                fl!("files-external-drives"),
                widget::toggler(self.config.files.external_drives)
                    .on_toggle(Message::ExternalDrives),
            ))
            .add(settings::item(
                fl!("files-content"),
                widget::toggler(self.config.files.content).on_toggle(Message::Content),
            ))
            .add(settings::item(
                fl!("files-content-limit"),
                slider(
                    self.config.files.content_max_mb as f32,
                    64.0,
                    4096.0,
                    Message::ContentMaxMb,
                ),
            ))
            .add(settings::item(
                fl!("files-refresh-hours"),
                slider(
                    self.config.files.refresh_hours as f32,
                    1.0,
                    72.0,
                    Message::RefreshHours,
                ),
            ));

        let plugins = if self.plugins.is_empty() {
            settings::section().title(fl!("section-plugins")).add(
                widget::column::with_children(vec![
                    widget::text::body(fl!("plugins-none")).into(),
                    widget::text::caption(fl!("plugins-hint")).into(),
                ])
                .spacing(4),
            )
        } else {
            self.plugins.iter().fold(
                settings::section().title(fl!("section-plugins")),
                |section, (id, name, keyword)| {
                    let enabled = !self.config.disabled_plugins.contains(id);
                    // The override if the user set one, else the field is
                    // empty and the manifest's keyword shows as a placeholder.
                    let override_value = self
                        .config
                        .plugin_keywords
                        .iter()
                        .find(|(entry, _)| entry == id)
                        .map(|(_, keyword)| keyword.clone())
                        .unwrap_or_default();

                    let keyword_id = id.clone();
                    let toggle_id = id.clone();
                    section.add(settings::item(
                        name.clone(),
                        widget::row::with_children(vec![
                            widget::text_input(
                                keyword.clone().unwrap_or_else(|| fl!("keyword-none")),
                                override_value,
                            )
                            .on_input(move |value| {
                                Message::PluginKeyword(keyword_id.clone(), value)
                            })
                            .width(Length::Fixed(140.0))
                            .into(),
                            widget::toggler(enabled)
                                .on_toggle(move |value| {
                                    Message::PluginEnabled(toggle_id.clone(), value)
                                })
                                .into(),
                        ])
                        .spacing(12)
                        .align_y(cosmic::iced::Alignment::Center),
                    ))
                },
            )
        };

        // File search has its own section: its switch also governs indexing,
        // which is work that happens whether or not the launcher is open.
        let providers = [
            (
                Provider::Windows,
                fl!("provider-windows"),
                self.config.providers.windows,
            ),
            (
                Provider::System,
                fl!("provider-system"),
                self.config.providers.system,
            ),
            (
                Provider::Devices,
                fl!("provider-devices"),
                self.config.providers.devices,
            ),
            (
                Provider::Clipboard,
                fl!("provider-clipboard"),
                self.config.providers.clipboard,
            ),
            (
                Provider::Emoji,
                fl!("provider-emoji"),
                self.config.providers.emoji,
            ),
            (
                Provider::Web,
                fl!("provider-web"),
                self.config.providers.web,
            ),
            (
                Provider::Calculator,
                fl!("provider-calculator"),
                self.config.providers.calculator,
            ),
        ]
        .into_iter()
        .fold(
            settings::section().title(fl!("section-providers")),
            |section, (provider, label, enabled)| {
                section.add(settings::item(
                    label,
                    widget::toggler(enabled)
                        .on_toggle(move |value| Message::Provider(provider, value)),
                ))
            },
        );

        let favorites = if self.config.favorites.is_empty() {
            settings::section().title(fl!("section-favorites")).add(
                widget::column::with_children(vec![
                    widget::text::body(fl!("favorites-none")).into(),
                    widget::text::caption(fl!("favorites-hint")).into(),
                ])
                .spacing(4),
            )
        } else {
            self.config.favorites.iter().fold(
                settings::section().title(fl!("section-favorites")),
                |section, key| {
                    let remove = key.clone();
                    section.add(settings::item(
                        favorite_label(key),
                        widget::button::text(fl!("favorites-remove"))
                            .class(cosmic::theme::Button::Destructive)
                            .on_press(Message::UnpinFavorite(remove)),
                    ))
                },
            )
        };

        let note = widget::text::caption(if self.handle.is_some() {
            fl!("note-live")
        } else {
            fl!("note-unavailable")
        });

        widget::column::with_children(vec![
            appearance.into(),
            launchpad.into(),
            files.into(),
            providers.into(),
            plugins.into(),
            favorites.into(),
            note.into(),
        ])
        .spacing(20)
        .padding(24)
        .apply(widget::scrollable)
        .height(Length::Fill)
        .into()
    }
}

impl App {
    /// Apply `next` and persist it.
    ///
    /// Written on every change rather than behind an apply button: the launcher
    /// reloads live, so a change the user makes is visible the moment they make
    /// it, and an apply button would only be a way to make that not happen.
    fn save(&mut self, next: Config) {
        match self.handle.as_ref() {
            Some(handle) => {
                for error in persist(&mut self.config, handle, next) {
                    tracing::error!(%error, "could not save settings");
                }
            }
            None => self.config = next,
        }
    }
}

/// Make `current` into `next`, writing only the keys that differ.
///
/// Never the whole entry: this window's copy of every other key is a
/// snapshot from when it opened, and writing it back would undo whatever
/// changed since — a result the launcher pinned, a key edited by hand. The
/// destructuring is exhaustive so a new field cannot be left unsaved.
fn persist(
    current: &mut Config,
    handle: &cosmic_config::Config,
    next: Config,
) -> Vec<cosmic_config::Error> {
    let Config {
        scroll,
        reduce_motion,
        blur,
        opacity,
        fullscreen_opacity,
        grid_layout,
        cell_size,
        grid_max_width,
        disabled_plugins,
        files,
        providers,
        quicklinks,
        fallbacks,
        favorites,
        plugin_keywords,
    } = next;
    [
        current.set_scroll(handle, scroll),
        current.set_reduce_motion(handle, reduce_motion),
        current.set_blur(handle, blur),
        current.set_opacity(handle, opacity),
        current.set_fullscreen_opacity(handle, fullscreen_opacity),
        current.set_grid_layout(handle, grid_layout),
        current.set_cell_size(handle, cell_size),
        current.set_grid_max_width(handle, grid_max_width),
        current.set_disabled_plugins(handle, disabled_plugins),
        current.set_files(handle, files),
        current.set_providers(handle, providers),
        current.set_quicklinks(handle, quicklinks),
        current.set_fallbacks(handle, fallbacks),
        current.set_favorites(handle, favorites),
        current.set_plugin_keywords(handle, plugin_keywords),
    ]
    .into_iter()
    .filter_map(Result::err)
    .collect()
}

/// A pinned result's key rendered for a person.
///
/// Keys are internal addresses — `entry:Firefox\u{1f}Web Browser`,
/// `system:dark-mode` — so the source prefix becomes a plain word and the
/// unit-separator between an entry's name and description becomes a dash.
/// The alternative is showing the user a control-character-laden string and
/// expecting them to recognise what they pinned.
/// The plugins as this window lists them: id, name and manifest keyword.
/// The host's discovery, minus the parts only the launcher needs — this
/// window lists plugins, it does not run them.
fn listed(host: &PluginHost) -> Vec<(String, String, Option<String>)> {
    host.all()
        .map(|(plugin, _)| {
            (
                plugin.id.clone(),
                plugin.manifest.name.clone(),
                plugin.manifest.keyword.clone(),
            )
        })
        .collect()
}

fn favorite_label(key: &str) -> String {
    let (kind, rest) = key.split_once(':').unwrap_or(("", key));
    let rest = rest.replace('\u{1f}', " — ");

    let kind = match kind {
        "entry" => fl!("favorite-kind-application"),
        "file" => fl!("favorite-kind-file"),
        "window" => fl!("favorite-kind-window"),
        "system" => fl!("favorite-kind-command"),
        "plugin" => fl!("favorite-kind-plugin"),
        "clip" => fl!("favorite-kind-clipboard"),
        "quicklink" | "fallback" => fl!("favorite-kind-link"),
        _ => return rest,
    };
    format!("{rest}  ·  {kind}")
}

/// A slider that reports continuously, so dragging shows live feedback.
fn slider<'a>(
    value: f32,
    min: f32,
    max: f32,
    message: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    widget::slider(min..=max, value, message)
        .width(Length::Fixed(240.0))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::cosmic_config::ConfigSet;

    #[test]
    fn a_change_here_does_not_undo_one_made_elsewhere() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = || {
            cosmic_config::Config::with_custom_path(
                jump::APP_ID,
                Config::VERSION,
                root.path().into(),
            )
            .expect("store")
        };

        // The window opened with this snapshot...
        let mut window = Config::get_entry(&store()).expect("readable store");
        // ...then the launcher pinned a result, and a key was edited by hand.
        store()
            .set("favorites", vec!["entry:Firefox".to_owned()])
            .expect("pin");
        store().set("cell_size", 200.0_f32).expect("hand edit");

        // Toggling blur in the window writes blur, and only blur.
        let mut next = window.clone();
        next.blur = !next.blur;
        assert!(persist(&mut window, &store(), next).is_empty());

        let stored = Config::get_entry(&store()).expect("readable store");
        assert_eq!(stored.favorites, ["entry:Firefox"]);
        assert!((stored.cell_size - 200.0).abs() < f32::EPSILON);
        assert_eq!(stored.blur, window.blur);
    }
}
