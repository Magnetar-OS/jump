// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Emoji search over a compiled-in data set.
//!
//! The whole Unicode emoji list ships inside the binary (the `emojis` crate),
//! so this provider needs no daemon, no index and no I/O — it is a pure
//! function from query to matches, which is why it lives in the engine.
//!
//! Matching is against the CLDR name and the shortcodes, ranked by where the
//! query lands: a name that *starts* with the query beats one that merely
//! contains it, and a shortcode hit ranks between the two — people who type
//! `:tada:` shortcodes know exactly what they are asking for.

/// One matching emoji.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The emoji itself, e.g. `🎉`.
    pub emoji: &'static str,
    /// CLDR name, e.g. `party popper`.
    pub name: &'static str,
    /// Primary GitHub-style shortcode, e.g. `tada`, when one exists.
    pub shortcode: Option<&'static str>,
}

/// Emoji matching `query`, best first, at most `limit`.
///
/// An empty query returns the first emoji in Unicode ordering — the smileys —
/// so the bare keyword shows something to pick from rather than nothing.
#[must_use]
pub fn matching(query: &str, limit: usize) -> Vec<Match> {
    let needle = query.trim().trim_matches(':').to_lowercase();

    if needle.is_empty() {
        return emojis::iter().take(limit).map(to_match).collect();
    }

    let mut scored: Vec<(u8, Match)> = emojis::iter()
        .filter_map(|emoji| {
            let name = emoji.name();
            let rank = if name.eq_ignore_ascii_case(&needle) {
                0
            } else if name.to_lowercase().starts_with(&needle) {
                1
            } else if emoji.shortcodes().any(|code| code.starts_with(&needle)) {
                2
            } else if name.to_lowercase().contains(&needle) {
                3
            } else if emoji.shortcodes().any(|code| code.contains(&needle)) {
                4
            } else {
                return None;
            };
            Some((rank, to_match(emoji)))
        })
        .collect();

    // Stable sort: within a rank, Unicode order — which groups related emoji —
    // is more useful than alphabetical.
    scored.sort_by_key(|(rank, _)| *rank);
    scored
        .into_iter()
        .take(limit)
        .map(|(_, matched)| matched)
        .collect()
}

fn to_match(emoji: &'static emojis::Emoji) -> Match {
    Match {
        emoji: emoji.as_str(),
        name: emoji.name(),
        shortcode: emoji.shortcode(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_match_finds_the_obvious_emoji() {
        let results = matching("party popper", 12);
        assert_eq!(results.first().map(|m| m.emoji), Some("🎉"));
    }

    #[test]
    fn shortcode_match_works_with_and_without_colons() {
        assert_eq!(matching("tada", 12)[0].emoji, "🎉");
        assert_eq!(matching(":tada:", 12)[0].emoji, "🎉");
    }

    #[test]
    fn prefix_beats_substring() {
        let results = matching("cat", 40);
        // "cat face" (name starts with the query) must rank above emoji whose
        // names merely contain "cat" somewhere.
        let first = results.first().expect("cat matches something");
        assert!(
            first.name.starts_with("cat"),
            "expected a cat-prefixed name first, got {}",
            first.name
        );
    }

    #[test]
    fn empty_query_shows_a_default_page() {
        let results = matching("", 12);
        assert_eq!(results.len(), 12);
    }

    #[test]
    fn limit_is_respected_and_nonsense_matches_nothing() {
        assert!(matching("smile", 3).len() <= 3);
        assert!(matching("qqqxyzzy", 12).is_empty());
    }
}
