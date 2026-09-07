// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! The overlay surface.
//!
//! `jump` maps a single full-screen `wlr-layer-shell` surface on the overlay
//! layer and draws the panel *inside* it, centred, rather than sizing the
//! surface to the panel. That choice drives most of the feel:
//!
//! * The panel can scale, translate, and fade during the open transition. A
//!   layer surface cannot be transformed by its client, and resizing it every
//!   frame would round-trip to the compositor for a new buffer configure — so
//!   anything animated has to live in surface-local coordinates.
//! * Clicks anywhere outside the panel land on our surface, giving click-away
//!   dismissal without a separate input region.
//! * The blur region is set to the panel rectangle only, so the frosted-glass
//!   effect tracks the panel instead of blurring the whole screen.
//!
//! The cost is that we paint a full-screen transparent buffer. That is cheap on
//! the GPU and is what every compositor-side overlay does anyway.

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{
    IcedMargin, SctkLayerSurfaceSettings,
};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced::{Rectangle, Task, window};

use jump::config::GridLayout;

/// Width of the panel in logical pixels.
///
/// 720 is wider than cosmic-launcher's 600: at 600 the subtitle of a file
/// result truncates constantly, and Spotlight's proportions are closer to this
/// relative to a typical display.
pub const PANEL_WIDTH: f32 = 800.0;

/// Height of the query input row.
pub const INPUT_HEIGHT: f32 = 76.0;

/// Height of one result row.
pub const ROW_HEIGHT: f32 = 60.0;

/// Most rows shown before the list scrolls.
pub const MAX_VISIBLE_ROWS: usize = 8;

/// Breathing room below the last row, so content does not touch the panel edge.
pub const LIST_PADDING: f32 = 12.0;

/// Horizontal inset of the result rows inside the panel.
///
/// The selection highlight is drawn on the row, so this is what turns it from a
/// full-bleed bar into an inset pill — the single biggest difference between
/// "a list widget" and the way Spotlight looks.
pub const ROW_INSET: f32 = 10.0;

/// Fraction of the screen height above the panel.
///
/// Slightly above centre, matching Spotlight — an optically centred panel sits
/// higher than a geometrically centred one.
const VERTICAL_BIAS: f32 = 0.28;

/// Corner radius of the panel, used for both the drawn shape and the blur
/// region so the frosted backdrop does not square off the corners.
pub const PANEL_RADIUS: f32 = 22.0;

/// Columns in the panel-layout Launchpad grid.
pub const GRID_COLUMNS: usize = 7;

/// Rows shown before the panel-layout grid scrolls.
pub const GRID_ROWS: usize = 4;

/// Geometry of a Launchpad grid, resolved for the current display and config.
///
/// Cell width and height are separate. A square cell looks wrong here: the label
/// sits under the icon and needs two lines of room, so a square cell either
/// crops long names or wastes horizontal space. Keeping them independent also
/// lets the columns spread to fill the display while the row pitch stays tied to
/// the icon size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    /// Edge length of the icon inside a cell.
    pub icon: f32,
    pub columns: usize,
    /// Rows that fit on screen before the grid scrolls.
    pub visible_rows: usize,
}

impl GridMetrics {
    /// Resolve grid metrics for a display and configuration.
    ///
    /// In full-screen layout the column count is derived from the available
    /// width rather than fixed, so the grid adapts to a 1080p laptop and a 3440
    /// px ultrawide without either becoming unusable.
    #[must_use]
    pub fn resolve(screen: cosmic::iced::Size, config: &jump::config::Config) -> Self {
        let cell = config.cell();

        match config.grid_layout {
            GridLayout::Panel => Self {
                cell_width: cell,
                cell_height: cell,
                icon: cell * ICON_RATIO,
                columns: GRID_COLUMNS,
                visible_rows: GRID_ROWS,
            },
            GridLayout::Fullscreen => {
                let usable_width = screen.width * config.grid_max_width.clamp(0.3, 1.0);

                // Columns are capped low on purpose. Deriving the count purely
                // from the display width gives 12+ columns on an ultrawide,
                // which reads as a dense table rather than a launcher. Launchpad
                // keeps 7-8 columns at any size and lets the cells grow.
                let columns = ((usable_width / cell).floor() as usize).clamp(4, MAX_COLUMNS);

                // Cells then divide the available width evenly, so the icons
                // spread across the display instead of clustering at a fixed
                // pitch with dead space at the edges.
                let cell_width = usable_width / columns as f32;
                let icon = (cell_width * ICON_RATIO).clamp(48.0, 128.0);
                // Icon, gap, and two lines of label.
                let cell_height = icon + LABEL_BLOCK;

                let usable_height = screen.height - INPUT_HEIGHT - FULLSCREEN_MARGIN * 2.0;
                let visible_rows = ((usable_height / cell_height).floor() as usize).max(1);

                Self {
                    cell_width,
                    cell_height,
                    icon,
                    columns,
                    visible_rows,
                }
            }
        }
    }

    /// How many rows `count` applications occupy.
    #[must_use]
    pub fn rows_for(&self, count: usize) -> usize {
        count.div_ceil(self.columns)
    }

    /// Applications shown on one page.
    #[must_use]
    pub fn page_size(&self) -> usize {
        (self.columns * self.visible_rows).max(1)
    }
}

/// Space kept clear above and below the full-screen grid.
pub const FULLSCREEN_MARGIN: f32 = 72.0;

/// Most columns the full-screen grid will use, however wide the display is.
const MAX_COLUMNS: usize = 8;

/// Icon edge as a fraction of cell width.
const ICON_RATIO: f32 = 0.46;

/// Vertical room reserved under the icon for the gap and two label lines.
///
/// Fixed rather than derived from the text so that a one-line name and a
/// two-line name produce the same row pitch — otherwise rows visibly step out
/// of alignment wherever a long application name appears.
pub const LABEL_BLOCK: f32 = 58.0;

/// What the overlay is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Query field with a ranked result list.
    Search,
    /// Launchpad: every installed application in a grid.
    ///
    /// This is what an empty query shows. pop-launcher returns nothing for an
    /// empty query, so the alternative was a blank panel; showing the full
    /// application grid instead turns the launcher's resting state into
    /// something useful and folds Launchpad into the same surface.
    Grid,
}

/// Create the overlay surface.
///
/// Anchored to all four edges so the compositor stretches it to the full
/// output; `exclusive_zone(-1)` opts out of panel/dock reservation so the
/// surface genuinely covers the screen rather than the work area.
pub fn open(id: window::Id) -> Task<()> {
    get_layer_surface(SctkLayerSurfaceSettings {
        id,
        // Overlay rather than Top: the launcher must appear above the COSMIC
        // panel and any fullscreen window, the same as the built-in launcher.
        layer: Layer::Overlay,
        // The launcher owns the keyboard while open. Without Exclusive the
        // compositor keeps focus on the previously active window and typing
        // goes there instead.
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
        anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        namespace: "jump".into(),
        margin: IcedMargin::default(),
        // Anchored on all sides, so the compositor decides both dimensions.
        size: Some((None, None)),
        // Negative: do not let panels shrink us to the work area.
        exclusive_zone: -1,
        // Accept input across the whole surface so clicks outside the panel
        // dismiss. `None` means "all input".
        input_zone: None,
        ..Default::default()
    })
}

/// Tear the surface down.
pub fn close(id: window::Id) -> Task<()> {
    destroy_layer_surface(id)
}

/// Height the first-run hint adds to the panel.
///
/// The panel is drawn into a fixed rectangle, so anything the view puts in
/// the column has to be accounted for here too — a hint that the geometry
/// does not know about is simply clipped, which is exactly what happened
/// the first time this was tried.
pub const HINT_HEIGHT: f32 = 26.0;

/// Where the panel sits inside the full-screen surface.
///
/// `rows` is the number of result rows currently visible, which lets the panel
/// grow downward as results arrive instead of reserving space for a list that
/// may be empty. `hint` adds room for the first-run line.
#[must_use]
pub fn panel_rect(
    screen: cosmic::iced::Size,
    mode: Mode,
    rows: usize,
    metrics: GridMetrics,
    layout: GridLayout,
    hint: bool,
) -> Rectangle {
    // Full-screen Launchpad is the whole output: there is no panel to place, and
    // the blur region becomes the entire surface, which is what gives the
    // macOS-style frosted desktop rather than a floating card.
    if mode == Mode::Grid && layout == GridLayout::Fullscreen {
        return Rectangle {
            x: 0.0,
            y: 0.0,
            width: screen.width,
            height: screen.height,
        };
    }

    let (width, height) = match mode {
        Mode::Search => {
            let rows = rows.min(MAX_VISIBLE_ROWS);
            let height = if rows == 0 {
                INPUT_HEIGHT
            } else {
                INPUT_HEIGHT + (rows as f32 * ROW_HEIGHT) + LIST_PADDING
            };
            (PANEL_WIDTH, height)
        }
        Mode::Grid => {
            // `rows` is the number of grid rows actually populated, so a system
            // with few applications gets a panel that fits them rather than a
            // fixed block of empty space.
            let rows = rows.clamp(1, metrics.visible_rows);
            (
                metrics.columns as f32 * metrics.cell_width + LIST_PADDING * 2.0,
                INPUT_HEIGHT + (rows as f32 * metrics.cell_height) + LIST_PADDING,
            )
        }
    };

    let height = height + if hint { HINT_HEIGHT } else { 0.0 };

    let width = width.min(screen.width - 32.0);
    let height = height.min(screen.height - 32.0);

    // Grid mode is tall, so biasing it upward the way the search panel is biased
    // would push it off the bottom of shorter displays. Centre it instead.
    let y = match mode {
        Mode::Search => screen.height * VERTICAL_BIAS - INPUT_HEIGHT / 2.0,
        Mode::Grid => (screen.height - height) / 2.0,
    };

    Rectangle {
        x: ((screen.width - width) / 2.0).max(0.0),
        y: y.max(16.0),
        width,
        height,
    }
}

/// Ask the compositor to blur the backdrop behind the panel.
///
/// Uses `ext-background-effect-v1`, which COSMIC 1.5 implements. libcosmic
/// silently no-ops when the compositor does not advertise the protocol or its
/// blur capability, so this needs no fallback path — on a compositor without
/// blur the panel simply renders as a plain translucent surface.
///
/// The region is expressed in surface-local coordinates, so it must be updated
/// whenever the panel geometry changes.
pub fn set_blur(id: window::Id, panel: Rectangle, enabled: bool) -> Task<()> {
    let regions = if enabled { Some(vec![panel]) } else { None };
    cosmic::iced::platform_specific::shell::commands::blur::blur(id, regions)
}
