// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Rendering.
//!
//! ## How fading works
//!
//! iced has no generic opacity wrapper — only `image` and `svg` expose one — so
//! there is no way to wrap the panel and fade it as a unit. Instead every colour
//! this module produces is passed through [`fade`], which scales its alpha by
//! the current transition progress. It is more bookkeeping than a compositor
//! opacity would be, but it is the only approach that actually fades text,
//! borders, and backgrounds together.
//!
//! Themed icons are the one exception: a full-colour application icon has no
//! tint parameter, so rows rely on their vertical slide for entrance motion and
//! their icons appear at full opacity. In practice this is invisible, because
//! the panel background beneath them is fading in at the same time.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit};
use cosmic::iced::widget::{keyed_column, pin};
use cosmic::iced::{Alignment, Background, Color, Length, Size};
use cosmic::widget::{column, container, icon, mouse_area, row, scrollable, text, text_input};
use cosmic::{Apply, Element};
use jump_core::{Icon, Item};

use crate::actions;
use crate::anim::Panel;
use crate::app::{INPUT_ID, Message};
use crate::apps::App;
use crate::surface::{
    self, FULLSCREEN_MARGIN, GridMetrics, INPUT_HEIGHT, LIST_PADDING, MAX_VISIBLE_ROWS, Mode,
    PANEL_RADIUS, ROW_HEIGHT, ROW_INSET,
};
use jump::config::{Config, GridLayout};

/// Edge length of a result row's leading icon.
const ICON_SIZE: u16 = 40;

/// Size icons are looked up at, as opposed to drawn at.
///
/// Icon themes ship discrete bitmap sizes; asking for the drawn size leaves a
/// 2x display scaling a 40 px bitmap up. 64 is the largest size every theme is
/// expected to provide.
const ICON_RESOLUTION: u16 = 64;

/// Corner radius of the selection pill.
const ROW_RADIUS: f32 = 14.0;

/// Diameter of a page indicator dot.
const DOT_SIZE: f32 = 8.0;

/// Gap between page indicator dots.
const DOT_SPACING: f32 = 10.0;

/// Height reserved for the page indicator row, kept constant so the grid does
/// not shift when the dots appear or disappear.
const DOT_ROW_HEIGHT: f32 = 28.0;

/// Gap between a tile's icon and its label.
const LABEL_GAP: f32 = 10.0;

/// Vertical padding inside a tile, which is what turns a dense grid into an
/// evenly-spaced one.
const CELL_PADDING: f32 = 14.0;

/// Fill opacity of the selection highlight on a dark theme.
///
/// Tinted with the user's accent rather than a flat grey, but kept low so the
/// application icon stays the brightest thing in the row.
const SELECTION_OPACITY_DARK: f32 = 0.34;

/// The same, on a light theme.
///
/// Higher, which is the opposite of what "the light theme looks too strong"
/// suggests — the measurement says otherwise. A theme's light and dark accents
/// are unrelated colours (this machine derives a light blue for dark mode and
/// a crimson for light), so a single alpha lands at a different perceived
/// strength in each. Measured against the shipped theme as WCAG contrast
/// between the composited row and the background behind it: dark at 0.34 gives
/// 2.12, light at 0.34 gives only 1.71 — the light selection was the *mushier*
/// of the two. 0.47 brings light to 2.13, so the highlight carries the same
/// weight in both modes.
///
/// The hue is deliberately not touched. It is whatever accent the user chose,
/// and overriding that would be substituting our taste for their setting.
const SELECTION_OPACITY_LIGHT: f32 = 0.47;

/// Gap between the action panel's card and the panel's corner.
const ACTION_PANEL_MARGIN: u16 = 14;

/// Width of the action panel card.
const ACTION_PANEL_WIDTH: f32 = 300.0;

/// Height of one action row.
const ACTION_ROW_HEIGHT: f32 = 40.0;

/// Shown when a result carries no icon, so rows keep a consistent left edge.
const FALLBACK_ICON: &str = "application-x-executable";

fn fallback_icon<'a, Message: 'a>() -> Element<'a, Message> {
    icon::from_name(FALLBACK_ICON).size(ICON_SIZE).icon().into()
}

/// Scale a colour's alpha, for transition fades.
fn fade(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha.clamp(0.0, 1.0),
        ..color
    }
}

/// `keyed_column` requires a `Copy` key, and [`jump_core::ItemKey`] is a
/// string. Hashing it preserves the property that matters — the same item keeps
/// the same key across queries, so iced reuses its widget state instead of
/// rebuilding the row.
fn row_key(item: &Item) -> u64 {
    let mut hasher = DefaultHasher::new();
    item.key.hash(&mut hasher);
    hasher.finish()
}

/// The whole overlay: a transparent full-screen layer with the panel pinned
/// inside it.
#[allow(clippy::too_many_arguments)]
pub fn overlay<'a>(
    panel: &Panel,
    now: Instant,
    screen: Size,
    mode: Mode,
    query: &'a str,
    results: &'a [Item],
    result_icons: &'a [icon::Handle],
    apps: &'a [App],
    selected: usize,
    metrics: GridMetrics,
    config: &Config,
    page: usize,
    actions: Option<&'a actions::Panel>,
) -> Element<'a, Message> {
    let layout = config.grid_layout;
    let alpha = panel.opacity(now);
    let rows = match mode {
        Mode::Search => results.len(),
        Mode::Grid => metrics.rows_for(apps.len()),
    };
    let rect = surface::panel_rect(screen, mode, rows, metrics, layout);
    let fullscreen = mode == Mode::Grid && layout == GridLayout::Fullscreen;

    let content = panel_body(
        panel,
        now,
        alpha,
        mode,
        query,
        results,
        result_icons,
        apps,
        selected,
        rect,
        metrics,
        fullscreen,
        config,
        page,
    );

    // The action panel floats over the results, anchored to the bottom-right
    // corner the way Raycast's does — near the hand, over nothing important.
    let content: Element<'a, Message> = match actions {
        Some(panel) if mode == Mode::Search => cosmic::iced::widget::Stack::with_children(vec![
            content,
            container(action_panel(panel, alpha))
                .width(Length::Fixed(rect.width))
                .height(Length::Fixed(rect.height))
                .align_x(Alignment::End)
                .align_y(Alignment::End)
                .padding(ACTION_PANEL_MARGIN)
                .into(),
        ])
        .width(Length::Fixed(rect.width))
        .height(Length::Fixed(rect.height))
        .into(),
        _ => content,
    };

    let pinned = pin(content)
        .x(rect.x)
        // The rise is applied here rather than as a widget transform: moving the
        // pin origin costs nothing, whereas animating padding would force a
        // relayout of every row on every frame.
        // Full-screen Launchpad does not translate: sliding the whole display
        // would expose an unpainted edge.
        .y(rect.y + if fullscreen { 0.0 } else { panel.offset_y(now) });

    // The backdrop covers the output and swallows clicks, giving click-away
    // dismissal without a separate input region.
    mouse_area(
        container(pinned)
            .width(Length::Fill)
            .height(Length::Fill)
            .class(cosmic::theme::Container::Transparent),
    )
    .on_press(Message::Dismiss)
    .into()
}

/// The panel itself: query field above a result list.
#[allow(clippy::too_many_arguments)]
fn panel_body<'a>(
    panel: &Panel,
    now: Instant,
    alpha: f32,
    mode: Mode,
    query: &'a str,
    results: &'a [Item],
    result_icons: &'a [icon::Handle],
    apps: &'a [App],
    selected: usize,
    rect: cosmic::iced::Rectangle,
    metrics: GridMetrics,
    fullscreen: bool,
    config: &Config,
    page: usize,
) -> Element<'a, Message> {
    // Full screen centres a fixed-width field rather than stretching it to the
    // display: a 3440 px wide text field looks absurd and puts the caret miles
    // from the results.
    let field_width = if fullscreen {
        surface::PANEL_WIDTH
    } else {
        rect.width
    };

    let field = query_field(query, alpha, field_width)
        .apply(container)
        .width(Length::Fixed(rect.width))
        .align_x(Alignment::Center)
        // Full screen, the field would otherwise sit flush against the top edge
        // with the first icon row immediately under it.
        .padding(if fullscreen {
            [
                FULLSCREEN_MARGIN as u16,
                0,
                (FULLSCREEN_MARGIN / 2.0) as u16,
                0,
            ]
        } else {
            [0, 0, 0, 0]
        })
        .apply(Element::from);

    let mut children: Vec<Element<'a, Message>> = vec![field];

    match mode {
        Mode::Search if !results.is_empty() => {
            children.push(result_list(
                panel,
                now,
                alpha,
                results,
                result_icons,
                selected,
            ));
        }
        Mode::Grid if !apps.is_empty() => {
            children.push(app_grid(
                panel, now, alpha, apps, selected, rect, metrics, fullscreen, config, page,
            ));
        }
        _ => {}
    }

    let fill = if fullscreen {
        config.backdrop_opacity()
    } else {
        config.panel_opacity()
    };

    column::with_children(children)
        .width(Length::Fixed(rect.width))
        .apply(container)
        // Pinned to the same rectangle the blur region uses, so the frosted
        // backdrop lines up with the panel exactly and the last row cannot be
        // clipped by a parent that sized itself differently.
        .width(Length::Fixed(rect.width))
        .height(Length::Fixed(rect.height))
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();

            // The compositor is blurring what is behind this rectangle via
            // ext-background-effect-v1. That effect is only visible through the
            // panel, so the fill has to stay well short of opaque — at 0.72 the
            // blur was there but invisible, which read as a flat dark panel.
            let mut background: Color = cosmic.background(false).base.into();
            background.a = fill;

            container::Style {
                background: Some(Background::Color(fade(background, alpha))),
                border: if fullscreen {
                    // No border or radius: the surface *is* the screen.
                    cosmic::iced::Border::default()
                } else {
                    cosmic::iced::Border {
                        radius: PANEL_RADIUS.into(),
                        width: 1.0,
                        // A light hairline rather than the theme divider: on a
                        // frosted surface it reads as the edge catching the
                        // light, which keeps the panel from looking pasted on.
                        color: fade(Color::from_rgba(1.0, 1.0, 1.0, 0.14), alpha),
                    }
                },
                shadow: if fullscreen {
                    cosmic::iced::Shadow::default()
                } else {
                    cosmic::iced::Shadow {
                        color: fade(Color::from_rgba(0.0, 0.0, 0.0, 0.55), alpha),
                        offset: cosmic::iced::Vector::new(0.0, 18.0),
                        blur_radius: 64.0,
                    }
                },
                text_color: Some(fade(cosmic.background(false).on.into(), alpha)),
                icon_color: None,
                snap: false,
            }
        }))
        .into()
}

/// Appearance of the query field: no fill, no border, in every state.
///
/// The selection fill for the active theme, before the animation's fade.
///
/// One helper rather than the same two lines at each of the three selection
/// sites, so the light/dark split cannot be applied to two of them and
/// forgotten at the third.
fn selection_tint(cosmic: &cosmic::cosmic_theme::Theme) -> Color {
    let mut tint: Color = cosmic.accent.base.into();
    tint.a = if cosmic.is_dark {
        SELECTION_OPACITY_DARK
    } else {
        SELECTION_OPACITY_LIGHT
    };
    tint
}

/// Every built-in class — `Search` included — draws a 2 px accent focus ring
/// when focused. A launcher must not have one: the field is the only focusable
/// thing on screen, so the ring communicates nothing and reads as a form
/// control. `TextInput::Custom` is the only way to opt out entirely.
fn query_appearance(theme: &cosmic::Theme, alpha: f32) -> text_input::Appearance {
    let cosmic = theme.cosmic();
    let on: Color = cosmic.background(false).on.into();

    text_input::Appearance {
        background: Color::TRANSPARENT.into(),
        border_radius: 0.0.into(),
        border_offset: None,
        border_width: 0.0,
        border_color: Color::TRANSPARENT,
        label_color: fade(on, alpha),
        placeholder_color: fade(Color { a: 0.4, ..on }, alpha),
        selected_text_color: fade(cosmic.on_accent_color().into(), alpha),
        selected_fill: fade(cosmic.accent_color().into(), alpha),
        icon_color: None,
        text_color: Some(fade(on, alpha)),
    }
}

/// The query input.
fn query_field<'a>(query: &'a str, alpha: f32, width: f32) -> Element<'a, Message> {
    // The same appearance in every state, so focusing changes nothing visually.
    let appearance = move |theme: &cosmic::Theme| query_appearance(theme, alpha);

    text_input::text_input(jump::fl!("type-to-search"), query)
        .id(cosmic::widget::Id::new(INPUT_ID))
        // The field is the only focusable thing on the surface, so it is always
        // the one typing should reach. Saying so directly also removes the race
        // where the focus task fires before the input has been laid out.
        .always_active()
        .on_input(Message::InputChanged)
        .on_submit(|_| Message::Activate)
        .style(cosmic::theme::TextInput::Custom {
            active: Box::new(appearance),
            error: Box::new(appearance),
            hovered: Box::new(appearance),
            focused: Box::new(appearance),
            disabled: Box::new(appearance),
        })
        // Large enough to read as the primary object on screen rather than a
        // form field.
        .size(28.0)
        .width(Length::Fixed(width))
        .padding([0, 26])
        .apply(container)
        .height(Length::Fixed(INPUT_HEIGHT))
        .align_y(Alignment::Center)
        .class(cosmic::theme::Container::Transparent)
        .into()
}

/// The scrollable result list.
fn result_list<'a>(
    panel: &Panel,
    now: Instant,
    alpha: f32,
    results: &'a [Item],
    result_icons: &'a [icon::Handle],
    selected: usize,
) -> Element<'a, Message> {
    let rows = results.iter().enumerate().map(|(index, item)| {
        let (row_alpha, offset) = panel.row(now, index);
        (
            row_key(item),
            result_row(
                item,
                result_icons.get(index),
                index,
                index == selected,
                alpha * row_alpha,
                offset,
            ),
        )
    });

    let visible = results.len().min(MAX_VISIBLE_ROWS) as f32 * ROW_HEIGHT;

    scrollable(keyed_column(rows))
        .height(Length::Fixed(visible))
        .apply(container)
        .padding([0, ROW_INSET as u16, LIST_PADDING as u16, ROW_INSET as u16])
        .into()
}

/// Launchpad: every installed application, in a scrollable grid.
#[allow(clippy::too_many_arguments)]
fn app_grid<'a>(
    panel: &Panel,
    now: Instant,
    alpha: f32,
    apps: &'a [App],
    selected: usize,
    rect: cosmic::iced::Rectangle,
    metrics: GridMetrics,
    fullscreen: bool,
    config: &Config,
    page: usize,
) -> Element<'a, Message> {
    let total = apps.len();

    // In paged mode only the current page is built. Slicing here rather than
    // scrolling means the widget tree stays the size of one screen of icons
    // regardless of how many applications are installed.
    let (apps, first_index) = if config.scroll.is_paged() {
        let size = metrics.page_size();
        let start = (page * size).min(apps.len());
        let end = (start + size).min(apps.len());
        (&apps[start..end], start)
    } else {
        (apps, 0)
    };

    // Chunked into explicit rows rather than handed to a wrapping layout: the
    // cascade delay is per row, and keyboard navigation moves by
    // `GRID_COLUMNS`, so both need the row boundaries to be known here rather
    // than decided during layout.
    let rows = apps
        .chunks(metrics.columns)
        .enumerate()
        .map(|(row, chunk)| {
            let (row_alpha, offset) = panel.row(now, row);
            let cells = chunk.iter().enumerate().map(|(column, app)| {
                let index = first_index + row * metrics.columns + column;
                app_cell(app, index, index == selected, alpha * row_alpha, metrics)
            });

            (
                row as u64,
                row::with_children(cells.collect::<Vec<_>>())
                    .apply(container)
                    .padding([offset as u16, 0, 0, 0])
                    .into(),
            )
        });

    // Centre the fixed-width block of columns in the available space, so a
    // partly-filled last row stays aligned with the rows above it.
    // The page slides in along the configured axis. Travel is a full page, so a
    // page enters from exactly offscreen rather than nudging into place.
    let (slide_x, slide_y) = match config.scroll {
        jump::config::ScrollMode::Continuous => (0.0, 0.0),
        jump::config::ScrollMode::PageHorizontal => (
            panel.page_offset(now, metrics.columns as f32 * metrics.cell_width),
            0.0,
        ),
        jump::config::ScrollMode::PageVertical => (
            0.0,
            panel.page_offset(now, metrics.visible_rows as f32 * metrics.cell_height),
        ),
    };

    let grid = keyed_column(rows)
        .apply(container)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .apply(|grid| {
            if slide_x == 0.0 && slide_y == 0.0 {
                Element::from(grid)
            } else {
                pin(grid).x(slide_x).y(slide_y).into()
            }
        });

    // Sized to a whole number of rows so the viewport never cuts one in half.
    let height = if fullscreen {
        metrics.visible_rows as f32 * metrics.cell_height
    } else {
        rect.height - INPUT_HEIGHT - LIST_PADDING
    };
    let height = Length::Fixed(height.max(metrics.cell_height));

    // A paged grid holds exactly one screenful, so it must not scroll — a
    // scrollbar there would let the view and the page counter disagree. Paging
    // is driven by the selection and by the wheel; the dots below are the only
    // signal that further pages exist, since there is no scrollbar to imply it.
    let body: Element<'a, Message> = if config.scroll.is_paged() {
        let pages = total.div_ceil(metrics.page_size().max(1));
        column::with_children(vec![
            container(grid).height(height).into(),
            page_dots(pages, page, alpha),
        ])
        .into()
    } else {
        scrollable(grid).height(height).into()
    };

    container(body)
        .padding([
            0,
            LIST_PADDING as u16,
            LIST_PADDING as u16,
            LIST_PADDING as u16,
        ])
        .into()
}

/// Row of dots showing how many pages there are and which one is showing.
fn page_dots<'a>(pages: usize, current: usize, alpha: f32) -> Element<'a, Message> {
    // One page is not a set of pages; drawing a lone dot just adds noise.
    if pages <= 1 {
        return cosmic::widget::Space::new()
            .height(Length::Fixed(DOT_ROW_HEIGHT))
            .into();
    }

    let dots = (0..pages).map(|page| {
        let active = page == current;
        container(
            cosmic::widget::Space::new()
                .width(Length::Fixed(DOT_SIZE))
                .height(Length::Fixed(DOT_SIZE)),
        )
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            let mut colour: Color = cosmic.background(false).on.into();
            colour.a = if active { 0.85 } else { 0.3 };

            container::Style {
                background: Some(Background::Color(fade(colour, alpha))),
                border: cosmic::iced::Border {
                    radius: (DOT_SIZE / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
    });

    row::with_children(dots.collect::<Vec<_>>())
        .spacing(DOT_SPACING as u16)
        .apply(container)
        .width(Length::Fill)
        .height(Length::Fixed(DOT_ROW_HEIGHT))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

/// One application tile.
fn app_cell<'a>(
    app: &'a App,
    index: usize,
    is_selected: bool,
    alpha: f32,
    metrics: GridMetrics,
) -> Element<'a, Message> {
    let label = text::caption(app.name.as_str())
        .center()
        .class(cosmic::theme::Text::Color(alpha_text(alpha, true)))
        .apply(container)
        // A fixed label box keeps every row on the same pitch regardless of how
        // many lines a name wraps to. Without it, one long name in a row pushes
        // that row taller and the whole grid stops lining up.
        .height(Length::Fixed(surface::LABEL_BLOCK - LABEL_GAP))
        .width(Length::Fill)
        .align_x(Alignment::Center);

    let tile = column::with_children(vec![
        icon::icon(app.icon.clone())
            .size(metrics.icon as u16)
            .into(),
        label.into(),
    ])
    .spacing(LABEL_GAP as u16)
    .align_x(Alignment::Center)
    .apply(container)
    .width(Length::Fixed(metrics.cell_width))
    .height(Length::Fixed(metrics.cell_height))
    .padding([CELL_PADDING as u16, 4])
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .class(cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        container::Style {
            background: is_selected.then(|| Background::Color(fade(selection_tint(cosmic), alpha))),
            border: cosmic::iced::Border {
                radius: ROW_RADIUS.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }));

    mouse_area(tile)
        .on_enter(Message::Select(index))
        .on_press(Message::ActivateAt(index))
        .into()
}

/// The floating card of actions for the selected result.
fn action_panel(panel: &actions::Panel, alpha: f32) -> Element<'_, Message> {
    let rows = panel.actions.iter().enumerate().map(|(index, action)| {
        let is_selected = index == panel.selected;

        let mut cells: Vec<Element<'_, Message>> = vec![
            icon::from_name(action.icon).size(16).icon().into(),
            text::body(action.label.as_str())
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                .class(cosmic::theme::Text::Color(alpha_text(alpha, true)))
                // Filling here is what pushes the key hint to the trailing
                // edge; libcosmic re-exports no spacer widget.
                .width(Length::Fill)
                .into(),
        ];
        // The key hint sits at the trailing edge, dimmed: it is how anyone
        // discovers the shortcut exists, and it must never compete with the
        // label for attention.
        if let Some(shortcut) = action.shortcut.as_deref() {
            cells.push(
                text::caption(shortcut)
                    .class(cosmic::theme::Text::Color(alpha_text(alpha, false)))
                    .into(),
            );
        }

        let content = row::with_children(cells)
            .spacing(10)
            .align_y(Alignment::Center)
            .apply(container)
            .padding([0, 12])
            .width(Length::Fill)
            .height(Length::Fixed(ACTION_ROW_HEIGHT))
            .align_y(Alignment::Center)
            .class(cosmic::theme::Container::custom(move |theme| {
                let cosmic = theme.cosmic();
                container::Style {
                    background: is_selected
                        .then(|| Background::Color(fade(selection_tint(cosmic), alpha))),
                    border: cosmic::iced::Border {
                        radius: (ROW_RADIUS - 6.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }));

        mouse_area(content)
            .on_enter(Message::SelectAction(index))
            .on_press(Message::RunAction(index))
            .into()
    });

    column::with_children(rows.collect::<Vec<_>>())
        .apply(container)
        .padding(6)
        .width(Length::Fixed(ACTION_PANEL_WIDTH))
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();

            // Nearly opaque on purpose: this card sits over the result list,
            // and legibility beats letting the rows ghost through it.
            let mut background: Color = cosmic.background(false).base.into();
            background.a = 0.97;

            container::Style {
                background: Some(Background::Color(fade(background, alpha))),
                border: cosmic::iced::Border {
                    radius: (ROW_RADIUS - 2.0).into(),
                    width: 1.0,
                    color: fade(cosmic.bg_divider().into(), alpha),
                },
                shadow: cosmic::iced::Shadow {
                    color: fade(Color::from_rgba(0.0, 0.0, 0.0, 0.4), alpha),
                    offset: cosmic::iced::Vector::new(0.0, 6.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            }
        }))
        .into()
}

/// One result row.
fn result_row<'a>(
    item: &'a Item,
    handle: Option<&icon::Handle>,
    index: usize,
    is_selected: bool,
    alpha: f32,
    offset: f32,
) -> Element<'a, Message> {
    let leading: Element<'a, Message> = match handle {
        Some(handle) => icon::icon(handle.clone()).size(ICON_SIZE).into(),
        // Only reachable if the handle list and the result list disagree, which
        // would be a bug rather than a state to render specially.
        None => fallback_icon(),
    };

    // Titles come from desktop entries and subtitles from file paths, so both
    // are arbitrarily long while the row's height is fixed. Without an explicit
    // limit a long one wraps, and the extra lines are drawn outside the row.
    let label = column::with_children(vec![
        text::body(item.title.as_str())
            .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
            .class(cosmic::theme::Text::Color(alpha_text(alpha, true)))
            .into(),
        text::caption(item.subtitle.as_str())
            .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
            .class(cosmic::theme::Text::Color(alpha_text(alpha, false)))
            .into(),
    ])
    .spacing(1)
    // Ellipsizing needs a bound to ellipsize against: without this the column
    // shrinks to its content and the text has no width to overflow.
    .width(Length::Fill);

    let content = row::with_children(vec![leading, label.into()])
        .spacing(12)
        .align_y(Alignment::Center)
        // The slide-in is expressed as leading padding rather than a transform,
        // because a translated row would still reserve its final position and
        // leave a visible gap while it travels.
        .padding([0, 16, 0, 16 + offset as u16])
        .apply(container)
        .height(Length::Fixed(ROW_HEIGHT))
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .class(cosmic::theme::Container::custom(move |theme| {
            let cosmic = theme.cosmic();
            container::Style {
                background: is_selected
                    .then(|| Background::Color(fade(selection_tint(cosmic), alpha))),
                border: cosmic::iced::Border {
                    radius: ROW_RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }));

    mouse_area(content)
        .on_enter(Message::Select(index))
        .on_press(Message::ActivateAt(index))
        .into()
}

/// Text colour at the given fade, dimmed for subtitles.
fn alpha_text(alpha: f32, primary: bool) -> Color {
    // From the theme, not a literal: the panel is filled with the theme's
    // background colour, so hardcoded white text disappears on a light theme.
    let on: Color = cosmic::theme::active().cosmic().background(false).on.into();
    let base = if primary { 1.0 } else { 0.65 };
    Color {
        a: base * alpha.clamp(0.0, 1.0),
        ..on
    }
}

/// Resolve a [`jump_core::Icon`] to a handle.
///
/// Called when results are installed rather than while drawing — see
/// `App::result_icons` — because a name lookup walks the icon theme.
///
/// Resolved at a larger size than rows draw at so a scaled display gets the
/// bitmap variant it needs rather than a scaled-up small one, and with an
/// explicit fallback chain: the default is to retry against ever-shorter
/// prefixes of the name, which for `com.example.App` silently lands on whatever
/// `com` happens to be.
pub fn icon_handle(source: Option<&Icon>) -> icon::Handle {
    let named = match source {
        // pop-launcher hands back either an icon-theme name or an absolute
        // path, and the distinction is not encoded in the variant.
        Some(Icon::Name(name)) if name.starts_with('/') => {
            return icon::from_path(std::path::PathBuf::from(name.clone()));
        }
        Some(Icon::Name(name)) => icon::from_name(name.clone()),
        Some(Icon::Mime(mime)) => icon::from_name(mime.replace('/', "-")),
        None => icon::from_name(FALLBACK_ICON.to_owned()),
    };

    named
        .prefer_svg(true)
        .size(ICON_RESOLUTION)
        .fallback(Some(icon::IconFallback::Names(vec![
            "application-default".into(),
            FALLBACK_ICON.into(),
        ])))
        .handle()
}
