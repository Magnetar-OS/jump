// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Performance budgets for the work that happens between a keystroke and a
//! painted frame.
//!
//! A launcher is judged on latency, so the latency claims have to be checked
//! rather than remembered. These are not micro-benchmarks chasing a number:
//! each one wraps a stage that sits on the interactive path, and each budget
//! is set well above the measured cost so it catches an *order of magnitude*
//! regression — an accidental clone-per-item, an O(n²) merge — without
//! failing CI because a shared runner was busy.
//!
//! Measured on a release build (Ryzen desktop, 2026-09-03), for scale. Two
//! runs minutes apart differed by up to 1.7x depending on what else the
//! machine was doing, which is exactly why the budgets sit ~30x above these
//! rather than beside them:
//!
//! | Stage | Mean |
//! |---|---|
//! | `rank::merge`, 180 items including the clone | 29-35 µs |
//! | `rank::promote_pinned`, 40 favorites | 27-31 µs |
//! | `frecency::boost`, 500 recorded entries | 21-26 µs |
//! | `emoji::matching`, full-table miss | 83-144 µs |
//! | `web::Link::url_for` | 127-197 ns |
//!
//! The whole cross-source ranking pipeline is therefore under 100 µs against
//! pop-launcher's own 0.5-1.5 ms warm query: ranking costs a rounding error
//! of the thing it ranks, which is the property worth defending.
//!
//! `just bench` runs them in release with the timings printed. Under
//! `cargo test` (a debug build) the same code runs for its correctness but
//! the budgets are *not* enforced — an unoptimised build misses them by an
//! order of magnitude, and a test that only passes in one profile is worse
//! than no test.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use jump_core::frecency::Frecency;
use jump_core::model::{Item, ItemKey, Source};

/// Run `body` `iterations` times and report the mean.
fn measure(name: &str, iterations: u32, mut body: impl FnMut()) -> Duration {
    // One untimed pass so first-touch costs — lazily built tables, cold
    // allocator arenas — are not billed to the first measured iteration.
    body();

    let start = Instant::now();
    for _ in 0..iterations {
        body();
    }
    let mean = start.elapsed() / iterations;
    println!("{name}: {mean:?} mean over {iterations} iterations");
    mean
}

fn assert_within(name: &str, measured: Duration, budget: Duration) {
    if cfg!(debug_assertions) {
        // Debug builds are an order of magnitude slower and say nothing about
        // shipped latency. The measurement still ran, and the numbers are
        // still printed; only the assertion is skipped.
        println!("{name}: budget {budget:?} not enforced in a debug build");
        return;
    }

    assert!(
        measured <= budget,
        "{name} took {measured:?}, over its {budget:?} budget — this is an \
         order-of-magnitude check, so a failure here means something on the \
         interactive path got structurally slower, not that the machine is busy"
    );
}

fn item(title: &str, source: Source, score: f32) -> Item {
    Item {
        key: ItemKey(format!("bench:{title}")),
        id: 0,
        title: title.to_owned(),
        subtitle: "subtitle text of a realistic length".to_owned(),
        icon: None,
        category_icon: None,
        window: None,
        source,
        autocomplete: None,
        score,
    }
}

/// A result set the size of a busy query: every provider answering at once.
fn corpus() -> Vec<Item> {
    let mut items = Vec::new();
    for index in 0..60 {
        items.push(item(&format!("Application {index}"), Source::Launcher, 0.0));
        items.push(item(
            &format!("document-{index}.md"),
            Source::File {
                path: PathBuf::from(format!("/home/user/notes/document-{index}.md")),
            },
            3.0,
        ));
        items.push(item(
            &format!("Window {index}"),
            Source::Window {
                identifier: format!("w{index}"),
            },
            0.0,
        ));
    }
    items
}

#[test]
fn ranking_a_full_result_set_is_cheap() {
    let frecency = Frecency::default();
    let items = corpus();
    println!("corpus: {} items", items.len());

    let mean = measure("rank::merge", 200, || {
        let ranked = jump_core::rank::merge(items.clone(), "document", &frecency);
        std::hint::black_box(ranked);
    });

    // The clone of 180 items is inside the measurement, so this covers more
    // than the merge itself. pop-launcher answers a warm query in 0.5-1.5 ms;
    // ranking must stay far below the thing it is ranking.
    assert_within("rank::merge", mean, Duration::from_micros(1000));
}

#[test]
fn promoting_favorites_does_not_scan_quadratically() {
    let favorites: Vec<String> = (0..40).map(|i| format!("bench:Application {i}")).collect();
    let items = corpus();

    let mean = measure("rank::promote_pinned", 200, || {
        let mut items = items.clone();
        jump_core::rank::promote_pinned(&mut items, |key| {
            favorites.iter().any(|entry| entry == key.as_str())
        });
        std::hint::black_box(items);
    });

    assert_within("rank::promote_pinned", mean, Duration::from_micros(1000));
}

#[test]
fn emoji_search_scans_the_whole_table_in_time_for_a_keystroke() {
    // The worst case is a query that matches almost nothing but still visits
    // every emoji and every shortcode.
    let mean = measure("emoji::matching", 50, || {
        std::hint::black_box(jump_core::emoji::matching("zzqq", 12));
    });
    assert_within("emoji::matching (miss)", mean, Duration::from_millis(5));

    let mean = measure("emoji::matching (hit)", 50, || {
        std::hint::black_box(jump_core::emoji::matching("smile", 12));
    });
    assert_within("emoji::matching (hit)", mean, Duration::from_millis(5));
}

#[test]
fn expanding_a_quicklink_template_is_free() {
    let link = jump_core::web::Link {
        name: "DuckDuckGo".to_owned(),
        keyword: "ddg".to_owned(),
        template: "https://duckduckgo.com/?q={query}".to_owned(),
    };

    let mean = measure("web::Link::url_for", 2000, || {
        std::hint::black_box(link.url_for("a moderately long query with spaces & symbols"));
    });
    assert_within("web::Link::url_for", mean, Duration::from_micros(10));
}

#[test]
fn frecency_boost_scales_with_the_visible_list() {
    let mut frecency = Frecency::default();
    for index in 0..500 {
        frecency.record(&ItemKey(format!("bench:Application {index}")));
    }
    let items = corpus();

    let mean = measure("frecency::boost", 200, || {
        let mut items = items.clone();
        frecency.boost(&mut items);
        std::hint::black_box(items);
    });

    assert_within("frecency::boost", mean, Duration::from_micros(1000));
}
