// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Cross-source ranking.
//!
//! Results arrive from four providers that cannot be compared directly:
//! pop-launcher ranks applications against its own corpus, [`crate::files`]
//! scores paths on an unbounded scale, windows have no score at all, and
//! plugins emit whatever order they like. Concatenating those lists and sorting
//! by position produces two specific failures, both observed:
//!
//! * **Weak matches crowd out strong ones.** pop-launcher returns a fixed eight
//!   results whether or not they are any good — querying `Cargo.toml` returned
//!   *Alpha Protocol* and *pwvucontrol*. Those filled the visible list.
//! * **Late providers never appear.** File search answers a few milliseconds
//!   after the launcher, so its results were appended below eight applications
//!   and fell off the bottom of the panel, however good they were.
//!
//! The fix is to score every item on one scale before ordering. Each provider's
//! contribution is normalised into `0.0..=1.0` from evidence that is comparable
//! across sources — chiefly *does the thing the user typed actually appear in
//! this result's title* — and only then is frecency applied.

use std::collections::HashMap;

use crate::frecency::Frecency;
use crate::model::{Item, Source};

/// Most results of any one kind, so a provider with many mediocre answers
/// cannot fill the panel and hide a better answer from somewhere else.
const PER_SOURCE_CAP: usize = 6;

/// Ceiling on the file-search score used when normalising it.
///
/// [`crate::files`] scores are unbounded in principle but sit around 3-4 for a
/// good match; anything above this is simply treated as perfect.
const FILE_SCORE_CEILING: f32 = 4.0;

/// Order `items` for display.
///
/// `query` is the text the user typed; `frecency` supplies usage history.
pub fn merge(mut items: Vec<Item>, query: &str, frecency: &Frecency) -> Vec<Item> {
    if items.is_empty() {
        return items;
    }

    let tokens: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|token| !token.is_empty())
        .collect();

    // Position within each source is a weak but real signal: pop-launcher's
    // first answer is more often right than its eighth.
    let mut seen_per_source: HashMap<u8, usize> = HashMap::new();

    for item in &mut items {
        let kind = source_kind(&item.source);
        let position = seen_per_source.entry(kind).or_default();
        let rank_penalty = (*position as f32) * 0.03;
        *position += 1;

        item.score = base_score(item, &tokens) - rank_penalty;
    }

    frecency.boost(&mut items);

    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    cap_per_source(items)
}

/// Numeric tag per source kind, for grouping without allocating.
fn source_kind(source: &Source) -> u8 {
    match source {
        Source::Window { .. } => 0,
        Source::Launcher => 1,
        Source::File { .. } => 2,
        Source::Plugin { .. } => 3,
        Source::Clipboard { .. } => 4,
        Source::System { .. } => 5,
        Source::Process { .. } => 6,
    }
}

/// Quality in roughly `0.0..=1.0`, comparable across providers.
fn base_score(item: &Item, tokens: &[String]) -> f32 {
    let title = item.title.to_lowercase();

    match &item.source {
        // A window only appears when its title or app id already matched, and
        // switching to something already open is nearly always the intent.
        // Scored just above a perfect application match so that it wins the tie
        // rather than depending on which provider answered first: typing
        // "firefox" while Firefox is open should focus it, not launch a second.
        Source::Window { .. } => 1.05,

        // Plugins are explicitly addressed by keyword, so if one produced a
        // result the user asked for it by name.
        Source::Plugin { .. } => 0.95,

        // Only ever produced behind the `clip` keyword, so the user asked for
        // history explicitly and nothing else should outrank it.
        Source::Clipboard { .. } => 1.1,

        // Already scored against a corpus of paths; normalise onto this scale.
        Source::File { .. } => (item.score / FILE_SCORE_CEILING).clamp(0.0, 1.0) * 0.9,

        // The important case. pop-launcher hands back a fixed-size list with no
        // quality signal, so the check is done here: an application whose name
        // does not contain what the user typed is a weak match no matter where
        // the service placed it.
        Source::Launcher => title_match(&title, tokens),

        // The provider already matched against its own keyword list — which
        // includes terms that are not in the title, like "wifi" for the
        // network page — and put its match quality in `score`. Re-deriving
        // from the title here would bury exactly those matches.
        Source::System { .. } => item.score.clamp(0.0, 1.0),

        // Only ever produced behind the `kill` keyword, where the whole list
        // is claimed and this scale is not consulted; a value keeps the match
        // exhaustive and honest if that ever changes.
        Source::Process { .. } => 1.0,
    }
}

/// How well a title matches the query tokens, in `0.0..=1.0`.
fn title_match(title: &str, tokens: &[String]) -> f32 {
    if tokens.is_empty() {
        return 0.5;
    }

    let matched = tokens
        .iter()
        .filter(|token| title.contains(token.as_str()))
        .count();

    if matched == 0 {
        // No part of the query appears in the name. pop-launcher matched it on
        // keywords, description, or executable — occasionally right, usually
        // noise, so it ranks below anything that matched by name.
        return 0.15;
    }

    let coverage = matched as f32 / tokens.len() as f32;

    // Where the match lands matters as much as whether it happened.
    let longest = tokens
        .iter()
        .max_by_key(|token| token.len())
        .map_or("", String::as_str);

    let placement = if title == longest {
        1.0
    } else if title.starts_with(longest) {
        0.9
    } else if title.contains(longest) {
        0.75
    } else {
        0.6
    };

    coverage * placement
}

/// Keep at most [`PER_SOURCE_CAP`] of each kind, preserving order.
///
/// Without this a query matching many files returns nothing but files, even
/// when an application further down was the better answer.
fn cap_per_source(items: Vec<Item>) -> Vec<Item> {
    let mut counts: HashMap<u8, usize> = HashMap::new();

    items
        .into_iter()
        .filter(|item| {
            let count = counts.entry(source_kind(&item.source)).or_default();
            *count += 1;
            *count <= PER_SOURCE_CAP
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemKey;
    use std::path::PathBuf;

    fn item(title: &str, source: Source, score: f32) -> Item {
        Item {
            key: ItemKey(format!("{title}:{}", source_kind(&source))),
            id: 0,
            title: title.to_owned(),
            subtitle: String::new(),
            icon: None,
            category_icon: None,
            window: None,
            source,
            autocomplete: None,
            score,
        }
    }

    #[test]
    fn strong_file_match_beats_unrelated_application() {
        // The exact failure observed: querying a filename returned unrelated
        // applications above the file itself.
        let items = vec![
            item("Alpha Protocol", Source::Launcher, 0.0),
            item("pwvucontrol", Source::Launcher, 0.0),
            item(
                "Cargo.toml",
                Source::File {
                    path: PathBuf::from("/home/u/project/Cargo.toml"),
                },
                3.5,
            ),
        ];

        let ranked = merge(items, "cargo.toml", &Frecency::default());
        assert_eq!(ranked[0].title, "Cargo.toml");
    }

    #[test]
    fn name_match_beats_keyword_match() {
        let items = vec![
            item("Garuda Toolbox", Source::Launcher, 0.0),
            item("Firefox", Source::Launcher, 0.0),
        ];

        let ranked = merge(items, "firefox", &Frecency::default());
        assert_eq!(ranked[0].title, "Firefox");
    }

    #[test]
    fn windows_outrank_launchables() {
        let items = vec![
            item("Firefox", Source::Launcher, 0.0),
            item(
                "Firefox — report.pdf",
                Source::Window {
                    identifier: "1".to_owned(),
                },
                0.0,
            ),
        ];

        let ranked = merge(items, "firefox", &Frecency::default());
        assert!(matches!(ranked[0].source, Source::Window { .. }));
    }

    #[test]
    fn one_source_cannot_fill_the_list() {
        let mut items: Vec<Item> = (0..20)
            .map(|i| {
                item(
                    &format!("file{i}.txt"),
                    Source::File {
                        path: PathBuf::from(format!("/home/u/file{i}.txt")),
                    },
                    3.0,
                )
            })
            .collect();
        items.push(item("Files", Source::Launcher, 0.0));

        let ranked = merge(items, "files", &Frecency::default());
        let files = ranked
            .iter()
            .filter(|item| matches!(item.source, Source::File { .. }))
            .count();

        assert_eq!(files, PER_SOURCE_CAP);
        assert!(ranked.iter().any(|item| item.source == Source::Launcher));
    }

    #[test]
    fn earlier_results_within_a_source_keep_their_edge() {
        let items = vec![
            item("Firefox", Source::Launcher, 0.0),
            item("Firefox Developer Edition", Source::Launcher, 0.0),
        ];

        let ranked = merge(items, "firefox", &Frecency::default());
        assert_eq!(ranked[0].title, "Firefox");
    }
}
