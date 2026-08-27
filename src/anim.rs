// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! The animation layer.
//!
//! Two different mechanisms, because they solve different problems:
//!
//! * The open/close transition uses iced's [`Animation`], which is backed by
//!   `lilt`. The reason is interruption: hitting Escape while the panel is still
//!   opening must reverse from wherever it currently is, not snap to fully-open
//!   and then close. Getting that right by hand means tracking the current
//!   interpolated value and re-basing the clock, which `lilt` already does.
//!
//! There is no scale component. iced's `Float` is the only widget that applies
//! a transform, and it does so only when scaling *above* 1.0 — a 0.96 -> 1.0
//! entrance would silently render at full size. The panel's entrance is carried
//! by opacity and a short vertical rise instead, which is close to what
//! Spotlight actually does.
//!
//! * Row staggering is computed arithmetically from a single [`Instant`]. Rows
//!   come and go on every keystroke, so allocating an `Animation` per row would
//!   mean building and dropping a dozen of them per frame. One timestamp plus a
//!   per-index delay gives the same cascade for free.

use std::time::{Duration, Instant};

use cosmic::iced::animation::{Animation, Easing};

/// How long the panel takes to appear.
///
/// Short enough not to delay the user, long enough for the eye to register
/// motion. Spotlight sits around this mark; below ~150 ms the transition reads
/// as a flicker rather than a movement.
const OPEN_DURATION: Duration = Duration::from_millis(220);

/// Closing is faster than opening. Dismissal should feel like it already
/// happened — waiting for a symmetrical fade-out feels sluggish.
const CLOSE_DURATION: Duration = Duration::from_millis(140);

/// Vertical offset, in logical pixels, the panel rises through as it opens.
const RISE: f32 = 10.0;

/// Delay added per row index in the result cascade.
///
/// Eight rows at 18 ms finish 126 ms after the first, overlapping the tail of
/// the panel transition so the whole thing lands as one motion.
const ROW_STAGGER: Duration = Duration::from_millis(18);

/// How long an individual row takes to fade and slide in.
const ROW_DURATION: Duration = Duration::from_millis(190);

/// Distance, in logical pixels, a result row travels as it fades in.
const ROW_RISE: f32 = 8.0;

/// How long a Launchpad page takes to slide in.
///
/// Longer than a row fade because the eye is tracking a whole screen of icons
/// travelling, and a page that snaps is disorienting rather than fast.
const PAGE_DURATION: Duration = Duration::from_millis(260);

/// Drives the panel's open/close transition.
pub struct Panel {
    /// Target state: `true` open, `false` closed.
    ///
    /// `bool` rather than `f32` because iced only exposes `interpolate` on the
    /// boolean animation — which suits a two-state transition anyway.
    progress: Animation<bool>,
    /// When the current result set was installed. Drives the row cascade.
    results_changed: Instant,
    /// When the visible Launchpad page last changed, and which way it moved.
    page_changed: Instant,
    page_forward: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // EaseOutExpo decelerates hard, which is what makes a panel feel
            // like it settles into place rather than coasting to a stop.
            progress: Animation::new(false)
                .easing(Easing::EaseOutExpo)
                .duration(OPEN_DURATION),
            results_changed: Instant::now(),
            page_changed: Instant::now() - PAGE_DURATION,
            page_forward: true,
        }
    }

    /// Begin opening. Safe to call while closing — the transition reverses.
    pub fn open(&mut self, now: Instant) {
        self.progress = std::mem::replace(&mut self.progress, Animation::new(false))
            .duration(OPEN_DURATION)
            .easing(Easing::EaseOutExpo);
        self.progress.go_mut(true, now);
    }

    /// Begin closing. Safe to call while opening.
    pub fn close(&mut self, now: Instant) {
        self.progress = std::mem::replace(&mut self.progress, Animation::new(false))
            .duration(CLOSE_DURATION)
            // Symmetrical deceleration on the way out would leave the panel
            // hanging around at low opacity; accelerating away reads as crisper.
            .easing(Easing::EaseInQuad);
        self.progress.go_mut(false, now);
    }

    /// Note that the Launchpad page changed, restarting the slide.
    pub fn page_changed(&mut self, now: Instant, forward: bool) {
        self.page_changed = now;
        self.page_forward = forward;
    }

    /// Offset of the page as it slides in, in logical pixels along the paging
    /// axis. Zero once the transition has finished.
    ///
    /// `distance` is the travel, normally the width or height of the grid, so a
    /// page enters from exactly one screen away rather than a fixed nudge.
    #[must_use]
    pub fn page_offset(&self, now: Instant, distance: f32) -> f32 {
        let elapsed = now.saturating_duration_since(self.page_changed);
        if elapsed >= PAGE_DURATION {
            return 0.0;
        }

        let linear = elapsed.as_secs_f32() / PAGE_DURATION.as_secs_f32();
        let remaining = 1.0 - Easing::EaseOutCubic.value(linear);
        let direction = if self.page_forward { 1.0 } else { -1.0 };

        remaining * distance * direction
    }

    /// Whether the page slide is still running.
    #[must_use]
    pub fn page_animating(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.page_changed) < PAGE_DURATION
    }

    /// Note that the result list changed, restarting the row cascade.
    ///
    /// Only call this when the set actually differs — restarting the cascade on
    /// every keystroke that returns the same rows makes the list strobe.
    pub fn results_changed(&mut self, now: Instant) {
        self.results_changed = now;
    }

    /// Whether a redraw is still needed.
    ///
    /// Used to decide whether to subscribe to frame callbacks. Returning false
    /// when idle is what keeps the launcher from burning a core while the user
    /// reads the results.
    #[must_use]
    pub fn is_animating(&self, now: Instant, rows: usize) -> bool {
        if self.progress.is_animating(now) || self.page_animating(now) {
            return true;
        }
        // The cascade runs on its own clock, so it has to be checked separately.
        let elapsed = now.saturating_duration_since(self.results_changed);
        elapsed < ROW_DURATION + ROW_STAGGER * u32::try_from(rows).unwrap_or(u32::MAX)
    }

    /// Panel opacity in 0.0..=1.0.
    #[must_use]
    pub fn opacity(&self, now: Instant) -> f32 {
        self.progress.interpolate(0.0, 1.0, now)
    }

    /// Vertical offset in logical pixels; positive moves the panel down.
    #[must_use]
    pub fn offset_y(&self, now: Instant) -> f32 {
        self.progress.interpolate(RISE, 0.0, now)
    }

    /// Whether the close transition has finished, meaning the surface can be
    /// destroyed. Destroying it earlier truncates the fade-out.
    #[must_use]
    pub fn is_closed(&self, now: Instant) -> bool {
        !self.progress.is_animating(now) && !self.progress.value()
    }

    /// Fade and offset for the result row at `index`.
    ///
    /// Returns `(opacity, offset_y)`. Rows past the visible window still get a
    /// value so that scrolling to them does not restart their entrance.
    #[must_use]
    pub fn row(&self, now: Instant, index: usize) -> (f32, f32) {
        let elapsed = now.saturating_duration_since(self.results_changed);
        let delay = ROW_STAGGER * u32::try_from(index).unwrap_or(u32::MAX);

        let Some(active) = elapsed.checked_sub(delay) else {
            // Not started yet: fully transparent and displaced.
            return (0.0, ROW_RISE);
        };

        let linear = (active.as_secs_f32() / ROW_DURATION.as_secs_f32()).clamp(0.0, 1.0);
        let eased = Easing::EaseOutCubic.value(linear);

        (eased, ROW_RISE * (1.0 - eased))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cascade_in_order() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.results_changed(start);

        let sample = start + Duration::from_millis(40);
        let (first, _) = panel.row(sample, 0);
        let (second, _) = panel.row(sample, 1);
        let (fifth, _) = panel.row(sample, 5);

        assert!(first > second, "earlier rows lead the cascade");
        assert!(second > fifth, "later rows trail further behind");
    }

    #[test]
    fn unstarted_rows_are_hidden() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.results_changed(start);

        // Row 20 is delayed well past this sample point.
        let (opacity, _) = panel.row(start + Duration::from_millis(1), 20);
        assert_eq!(opacity, 0.0);
    }

    #[test]
    fn cascade_completes() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.results_changed(start);

        let done = start + ROW_DURATION + ROW_STAGGER * 8 + Duration::from_millis(10);
        let (opacity, offset) = panel.row(done, 8);
        assert_eq!(opacity, 1.0);
        assert_eq!(offset, 0.0);
        assert!(!panel.is_animating(done, 8));
    }

    #[test]
    fn opening_reaches_full_opacity() {
        let mut panel = Panel::new();
        let start = Instant::now();
        panel.open(start);

        let settled = start + OPEN_DURATION + Duration::from_millis(10);
        assert!((panel.opacity(settled) - 1.0).abs() < f32::EPSILON);
        assert!(panel.offset_y(settled).abs() < f32::EPSILON);
    }
}
