// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Usage-weighted re-ranking.
//!
//! pop-launcher sorts by match quality alone, so the app you launch twenty
//! times a day sits below a closer string match you have never opened. This
//! module records activations and boosts results accordingly.
//!
//! The scoring is Mozilla's frecency shape: a visit count weighted by how
//! recently each visit happened, so a burst of use decays rather than pinning an
//! entry to the top forever. Bucketed decay is used instead of a continuous
//! exponential because it is stable — a result does not drift down the list
//! while the user is looking at it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::model::{Item, ItemKey};

/// How much a maximally-used entry may add to its match quality.
///
/// Base scores from [`crate::rank`] are normalised into 0.0..=1.0, so half a
/// point lets usage reorder results of *similar* quality — the app you open
/// daily rises above one you never touch — without letting a favourite outrank
/// something that matched the query far better.
const MAX_BOOST: f32 = 0.5;

/// Visits older than this contribute nothing.
const MAX_AGE_DAYS: f64 = 90.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Usage {
    /// Number of times this entry was activated.
    count: u32,
    /// Unix timestamp, seconds, of the most recent activation.
    last_used: u64,
}

impl Usage {
    /// Weight in 0.0..=1.0 from recency alone.
    fn recency_weight(&self, now: u64) -> f32 {
        let age_days = (now.saturating_sub(self.last_used) as f64) / 86_400.0;
        // Mozilla-style buckets: same day, this week, this month, this quarter.
        let weight = if age_days < 1.0 {
            1.0
        } else if age_days < 7.0 {
            0.7
        } else if age_days < 30.0 {
            0.4
        } else if age_days < MAX_AGE_DAYS {
            0.2
        } else {
            0.0
        };
        weight as f32
    }

    fn score(&self, now: u64) -> f32 {
        // Saturating log growth: the difference between 1 and 5 launches should
        // matter far more than between 200 and 400.
        let volume = (f64::from(self.count) + 1.0).ln() as f32;
        volume * self.recency_weight(now)
    }
}

/// Persistent activation history.
#[derive(Debug, Default)]
pub struct Frecency {
    entries: HashMap<ItemKey, Usage>,
    path: Option<PathBuf>,
    /// Set when `entries` has changed since the last successful save.
    dirty: bool,
}

impl Frecency {
    /// Load history from the user's data directory, or start empty when the
    /// file is missing or unreadable. A corrupt history is never fatal — the
    /// launcher must still open.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            tracing::warn!("no data directory; frecency will not persist");
            return Self::default();
        };

        let entries = match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
                tracing::warn!(%error, ?path, "discarding unreadable frecency history");
                HashMap::new()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => {
                tracing::warn!(%error, ?path, "failed to read frecency history");
                HashMap::new()
            }
        };

        Self {
            entries,
            path: Some(path),
            dirty: false,
        }
    }

    fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("jump").join("frecency.json"))
    }

    /// Record an activation.
    pub fn record(&mut self, key: &ItemKey) {
        let entry = self.entries.entry(key.clone()).or_default();
        entry.count = entry.count.saturating_add(1);
        entry.last_used = now_secs();
        self.dirty = true;
    }

    /// Add a usage boost to each item's existing score.
    ///
    /// Deliberately additive rather than authoritative: match quality is
    /// computed by [`crate::rank`] and this only nudges the order. An earlier
    /// version *set* the score from list position, which silently discarded the
    /// file-search ranking and pushed good results off the panel.
    pub fn boost(&self, items: &mut [Item]) {
        if items.is_empty() {
            return;
        }

        let now = now_secs();

        // Normalise against the usage present in *this* result set, so the
        // boost is relative to the candidates on screen rather than to the
        // user's most-used application overall.
        let max_usage = items
            .iter()
            .filter_map(|item| self.entries.get(&item.key))
            .map(|usage| usage.score(now))
            .fold(0.0_f32, f32::max);

        if max_usage <= 0.0 {
            return;
        }

        for item in items.iter_mut() {
            item.score += self
                .entries
                .get(&item.key)
                .map_or(0.0, |usage| usage.score(now) / max_usage)
                * MAX_BOOST;
        }
    }

    /// Persist history if it changed. Cheap to call on dismiss.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.path.as_ref() else {
            return;
        };

        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(%error, ?parent, "failed to create data directory");
            return;
        }

        let Ok(contents) = serde_json::to_string(&self.entries) else {
            return;
        };

        // Write-then-rename so a crash mid-write cannot truncate the history.
        let temporary = path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temporary, contents) {
            tracing::warn!(%error, ?temporary, "failed to write frecency history");
            return;
        }
        if let Err(error) = std::fs::rename(&temporary, path) {
            tracing::warn!(%error, ?path, "failed to replace frecency history");
            return;
        }

        self.dirty = false;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn item(name: &str) -> Item {
        Item {
            key: ItemKey(format!("entry:{name}")),
            id: 0,
            title: name.to_owned(),
            subtitle: String::new(),
            icon: None,
            category_icon: None,
            window: None,
            source: Source::Launcher,
            autocomplete: None,
            score: 0.0,
        }
    }

    #[test]
    fn usage_breaks_ties_between_equal_matches() {
        let mut frecency = Frecency::default();
        frecency.record(&ItemKey("entry:Firefox".to_owned()));
        frecency.record(&ItemKey("entry:Firefox".to_owned()));

        // Both matched the query equally well.
        let mut items = vec![item("Files"), item("Firefox")];
        for entry in &mut items {
            entry.score = 0.6;
        }
        frecency.boost(&mut items);

        assert!(
            items[1].score > items[0].score,
            "the used entry gains a boost"
        );
    }

    #[test]
    fn usage_cannot_override_a_clearly_better_match() {
        let mut frecency = Frecency::default();
        for _ in 0..50 {
            frecency.record(&ItemKey("entry:Firefox".to_owned()));
        }

        let mut items = vec![item("Files"), item("Firefox")];
        // "Files" matched the query exactly; "Firefox" barely matched.
        items[0].score = 1.0;
        items[1].score = 0.15;
        frecency.boost(&mut items);

        assert!(items[0].score > items[1].score);
    }

    #[test]
    fn scores_are_untouched_without_history() {
        let frecency = Frecency::default();
        let mut items = vec![item("Files"), item("Firefox")];
        items[0].score = 0.7;
        items[1].score = 0.3;
        frecency.boost(&mut items);

        assert!((items[0].score - 0.7).abs() < f32::EPSILON);
        assert!((items[1].score - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn stale_usage_stops_contributing() {
        let usage = Usage {
            count: 100,
            last_used: 0,
        };
        // Epoch timestamps are far past MAX_AGE_DAYS, so the weight is zero.
        assert_eq!(usage.score(now_secs()), 0.0);
    }
}
