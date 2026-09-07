// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Calculator and unit conversion, over Qalculate's `qalc`.
//!
//! This is the one place jump deliberately duplicates something pop-launcher
//! ships, and the reason is that pop-launcher's version does not work.
//! Measured on pop-launcher 1.2.7 against Qalculate 5.12.0 by driving the
//! plugin binary directly: every expression comes back as the input with
//! `x = ?` appended — `= 15*3` answers `15*3 x = ?`, and unit conversions do
//! the same. `qalc -t "15*3"` prints `45` on the same machine, so the fault
//! is a version skew inside that plugin rather than in Qalculate.
//!
//! Rather than pattern-match another program's broken output, jump asks
//! `qalc` itself. The contract is small enough that owning it is cheaper
//! than depending on the skew being fixed.
//!
//! ## Talking to qalc
//!
//! * `-t` is terse: the answer alone, with no echo of the input.
//! * `-m 500` is qalc's *own* millisecond budget, so it gives up internally
//!   rather than being killed halfway through and leaving a half-written
//!   answer. The deadline here is a second line of defence around it.
//! * `-e` is **not** passed. It means `-exrates`, which fetches currency
//!   rates over the network — not something a keystroke should do.
//!
//! Qalculate answers almost anything, and its exit status is `0` even for
//! input it could not make sense of (`nonsense zzz` evaluates to `0 s²`), so
//! success cannot be read from the status. The only answer rejected here is
//! one that merely restates the question, which is what qalc does when it
//! cannot evaluate at all.

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// Prefix that addresses the calculator, matching pop-launcher's own syntax
/// so muscle memory carries over.
pub const PREFIX: char = '=';

/// Qalculate's internal evaluation budget, and the shape of ours.
///
/// Half a second is far longer than arithmetic needs and still short enough
/// that a pathological expression cannot hold the result list. The outer
/// deadline is deliberately larger so qalc's own timeout wins first and can
/// report what it managed.
const QALC_BUDGET_MS: u64 = 500;
const DEADLINE: Duration = Duration::from_millis(1200);

/// The expression a query addresses to the calculator, if it does.
///
/// `= 15*3` and `=15*3` both work; a bare `=` does not, because there is
/// nothing to evaluate and an empty expression makes qalc print its prompt.
#[must_use]
pub fn claims(query: &str) -> Option<&str> {
    let rest = query.strip_prefix(PREFIX)?.trim();
    (!rest.is_empty()).then_some(rest)
}

/// Whether Qalculate is installed.
///
/// Checked by looking for the binary rather than running it: this decides
/// whether to offer the provider at all, and spawning a process to find out
/// would cost more than the answer.
#[must_use]
pub fn available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("qalc").is_file())
        })
    })
}

/// Evaluate `expression`, returning the answer as Qalculate renders it.
///
/// `None` when qalc is missing, fails, times out, or answers with nothing
/// more than a restatement of the question.
pub async fn evaluate(expression: &str) -> Option<String> {
    if !available() {
        return None;
    }
    let expression = expression.trim();
    if expression.is_empty() {
        return None;
    }

    let child = Command::new("qalc")
        .arg("-t")
        .arg("-m")
        .arg(QALC_BUDGET_MS.to_string())
        // `--` so an expression beginning with a dash is an expression and
        // not a flag: `= -5+3` must not be read as an option.
        .arg("--")
        .arg(expression)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();

    let output = match tokio::time::timeout(DEADLINE, child).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            tracing::warn!(%error, "qalc failed to run");
            return None;
        }
        Err(_) => {
            tracing::warn!(expression, "qalc exceeded its deadline");
            return None;
        }
    };

    let answer = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if answer.is_empty() {
        return None;
    }
    // qalc restates the question when it cannot evaluate — `1/0` answers
    // `1 / 0`. Spacing differs, so the comparison ignores whitespace.
    if squeeze(&answer) == squeeze(expression) {
        return None;
    }
    Some(answer)
}

/// Lowercase with all whitespace removed, for comparing an answer against
/// the question that produced it.
fn squeeze(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_claims_with_or_without_a_space() {
        assert_eq!(claims("=15*3"), Some("15*3"));
        assert_eq!(claims("= 15*3"), Some("15*3"));
        assert_eq!(claims("=  5 km to miles  "), Some("5 km to miles"));
    }

    #[test]
    fn a_bare_prefix_and_ordinary_queries_claim_nothing() {
        // A bare `=` would make qalc print its prompt rather than an answer.
        assert_eq!(claims("="), None);
        assert_eq!(claims("=   "), None);
        assert_eq!(claims("firefox"), None);
        assert_eq!(claims(""), None);
    }

    #[test]
    fn a_restatement_is_not_an_answer() {
        assert_eq!(squeeze("1 / 0"), squeeze("1/0"));
        assert_ne!(squeeze("45"), squeeze("15*3"));
    }

    /// Runs the real qalc. Ignored by default so CI without Qalculate stays
    /// green; run with `cargo test -- --ignored qalc`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs qalc installed"]
    async fn qalc_answers_arithmetic_and_conversions() {
        assert_eq!(evaluate("15*3").await.as_deref(), Some("45"));

        let converted = evaluate("5 km to miles").await.expect("a conversion");
        assert!(converted.contains("mi"), "got {converted}");

        // Restatements are suppressed rather than shown as answers.
        assert_eq!(evaluate("1/0").await, None);
        assert_eq!(evaluate("").await, None);
    }
}
