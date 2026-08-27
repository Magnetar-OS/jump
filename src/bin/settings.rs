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
use cosmic::iced::Length;
use cosmic::widget::{self, settings};
use cosmic::{Apply, Element};
use jump::config::{Config, GridLayout, ScrollMode};
use jump::fl;
use jump_core::PluginHost;

const ID: &str = "dev.entro314labs.JumpSettings";

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
    /// Discovered jump plugins, listed so each can be switched off. Read once
    /// at startup; installing a plugin means reopening this window.
    plugins: Vec<(String, String, Option<String>)>,
}

#[derive(Debug, Clone)]
enum Message {
    Blur(bool),
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

        // The host's discovery, minus the parts only the launcher needs: this
        // window lists plugins, it does not run them.
        let plugins = PluginHost::discover()
            .all()
            .map(|(plugin, _)| {
                (
                    plugin.id.clone(),
                    plugin.manifest.name.clone(),
                    plugin.manifest.keyword.clone(),
                )
            })
            .collect();

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
        match message {
            Message::Blur(value) => self.config.blur = value,
            Message::Opacity(value) => self.config.opacity = value,
            Message::BackdropOpacity(value) => self.config.fullscreen_opacity = value,
            Message::CellSize(value) => self.config.cell_size = value,
            Message::GridWidth(value) => self.config.grid_max_width = value,
            Message::Layout(index) => {
                self.config.grid_layout = match index {
                    1 => GridLayout::Panel,
                    _ => GridLayout::Fullscreen,
                };
            }
            Message::Scroll(index) => {
                self.config.scroll = match index {
                    0 => ScrollMode::Continuous,
                    2 => ScrollMode::PageVertical,
                    _ => ScrollMode::PageHorizontal,
                };
            }
            Message::FilesEnabled(value) => self.config.files.enabled = value,
            Message::ExternalDrives(value) => self.config.files.external_drives = value,
            Message::Content(value) => self.config.files.content = value,
            Message::ContentMaxMb(value) => self.config.files.content_max_mb = value as u64,
            Message::RefreshHours(value) => self.config.files.refresh_hours = value as u64,
            Message::PluginEnabled(id, enabled) => {
                if enabled {
                    self.config.disabled_plugins.retain(|entry| entry != &id);
                } else if !self.config.disabled_plugins.contains(&id) {
                    self.config.disabled_plugins.push(id);
                }
            }
        }

        self.save();
        Task::none()
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
                    let label = match keyword {
                        Some(keyword) => format!("{name} · {keyword}"),
                        None => name.clone(),
                    };
                    let enabled = !self.config.disabled_plugins.contains(id);
                    let id = id.clone();
                    section.add(settings::item(
                        label,
                        widget::toggler(enabled)
                            .on_toggle(move |value| Message::PluginEnabled(id.clone(), value)),
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
            plugins.into(),
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
    /// Persist the whole entry.
    ///
    /// Written on every change rather than behind an apply button: the launcher
    /// reloads live, so a change the user makes is visible the moment they make
    /// it, and an apply button would only be a way to make that not happen.
    fn save(&self) {
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        if let Err(error) = self.config.write_entry(handle) {
            tracing::error!(%error, "could not save settings");
        }
    }
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
