//! Watchdog for quotes that have stopped moving while a market is open.
//!
//! CORRECTION (2026-09-20): this module originally cited a "lost trading day"
//! on 2026-09-19. That was wrong. 2026-09-19 was a **Saturday**: the zero price
//! changes and absent decision reports that day were correct weekend behaviour,
//! and the persisted scheduler records say `market_closed` and Markov idle.
//! Friday 2026-09-18 ran normally -- four reports, three Markov runs, 548
//! recorded price changes -- as did every session from 09-14 to 09-18. The
//! scheduler did terminate with exit=1 at 2026-09-19T08:09:09Z, and the pods
//! carry 8 restarts, but no trading was missed and that restart is a separate
//! question. The original commit message (a2b5d62) carries the same error.
//!
//! The check is kept because the failure class it guards is real even though
//! the incident was not. Every other integrity check asks whether an action
//! went wrong; none asks whether an action happened at all, which is how the
//! end-of-day reflection stayed dead for seven days and the realised-sell panel
//! never rendered. A frozen feed never errors -- it simply stops, and an
//! absence passes every test written for errors.
//!
//! So this inverts the question: not "did the last quote fetch fail" but "have
//! prices moved at all on an exchange the calendar says is open". It is a guard
//! against a plausible failure, not a postmortem of an observed one, and it has
//! never yet fired in production.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value as JsonValue, json};

/// How long an open market may show no price change at all before this is an
/// error rather than a quiet stretch.
///
/// Individual symbols go minutes without printing, so the test is across every
/// held position on an open exchange at once. On 2026-09-18 the book logged 548
/// price changes over the session; the chance that a working feed shows zero
/// across all of them for half an hour is not worth pricing.
pub(crate) const DEFAULT_STALE_QUOTE_MINUTES: i64 = 30;

/// One position's price at the start and end of the observed window.
struct Observed {
    first: f64,
    last: f64,
    changed: bool,
}

/// Verdict on whether quotes are moving for positions on open exchanges.
///
/// `rows` are position snapshots carrying `recorded_at`, `symbol` and
/// `price_local`, newest or oldest first -- ordering inside the window does not
/// matter because only the endpoints and the change flag are used.
pub(crate) fn quote_freshness_verdict(
    rows: &[JsonValue],
    open_exchange_codes: &HashSet<String>,
    stale_after_minutes: i64,
) -> JsonValue {
    if open_exchange_codes.is_empty() {
        return verdict(
            "not_applicable",
            "No configured exchange is open.",
            0,
            0,
            None,
        );
    }

    // Staleness is measured over the trailing `stale_after_minutes` only. The
    // caller fetches a wider span so the window is always fully covered, and
    // evaluating that whole span instead would silently weaken the check: one
    // print 85 minutes ago would clear a feed that has been dead for 30.
    let newest = rows
        .iter()
        .filter_map(|row| row.get("recorded_at").and_then(JsonValue::as_str))
        .max()
        .map(str::to_string);
    let Some(newest) = newest else {
        return verdict(
            "unknown",
            "No position snapshots were available, so quote freshness could not be measured.",
            0,
            0,
            None,
        );
    };
    let cutoff = rfc3339_minus_minutes(&newest, stale_after_minutes);

    let mut observed: BTreeMap<String, Observed> = BTreeMap::new();
    let mut rows_in_window = 0_usize;
    let mut earliest: Option<String> = None;
    let mut latest: Option<String> = None;

    for row in rows {
        let (Some(symbol), Some(price), Some(recorded_at)) = (
            row.get("symbol").and_then(JsonValue::as_str),
            row.get("price_local").and_then(JsonValue::as_f64),
            row.get("recorded_at").and_then(JsonValue::as_str),
        ) else {
            continue;
        };
        if !price.is_finite() || price <= 0.0 {
            continue;
        }
        if cutoff.as_deref().is_some_and(|cutoff| recorded_at < cutoff) {
            continue;
        }
        rows_in_window += 1;
        let exchange = symbol.split_once(':').map(|(_, code)| code).unwrap_or("");
        if !open_exchange_codes.contains(&exchange.to_ascii_lowercase()) {
            continue;
        }
        if earliest.as_deref().is_none_or(|value| recorded_at < value) {
            earliest = Some(recorded_at.to_string());
        }
        if latest.as_deref().is_none_or(|value| recorded_at > value) {
            latest = Some(recorded_at.to_string());
        }
        observed
            .entry(symbol.to_string())
            .and_modify(|entry| {
                if entry.last != price {
                    entry.changed = true;
                }
                entry.last = price;
            })
            .or_insert(Observed {
                first: price,
                last: price,
                changed: false,
            });
    }

    let watched = observed.len();
    if watched == 0 {
        // Holding nothing on the open exchange is a real answer; having no
        // snapshot rows at all is missing evidence. Collapsing the two would
        // reproduce the exact defect this check exists to catch -- a degraded
        // read that looks identical to a healthy one -- and the caller's
        // `unwrap_or_default()` on a failed query lands in the second case.
        return if rows_in_window == 0 {
            verdict(
                "unknown",
                "No position snapshot was found in the window, so quote freshness could not \
                 be measured.",
                0,
                0,
                None,
            )
        } else {
            verdict(
                "not_applicable",
                "No held position trades on an open exchange.",
                0,
                0,
                None,
            )
        };
    }

    // `changed` catches a price that moved and came back within the window;
    // comparing only the endpoints would call that static.
    let moved = observed
        .values()
        .filter(|entry| entry.changed || entry.first != entry.last)
        .count();

    let window_minutes = match (earliest.as_deref(), latest.as_deref()) {
        (Some(first), Some(last)) => minutes_between(first, last),
        _ => None,
    };

    // Too short a window is not evidence of anything: a fresh restart or a
    // pruned history would otherwise report a dead feed on its first cycle.
    let Some(window_minutes) = window_minutes else {
        return verdict(
            "unknown",
            "Snapshot timestamps could not be read, so staleness was not measured.",
            watched,
            moved,
            None,
        );
    };
    if window_minutes < stale_after_minutes {
        return verdict(
            "ok",
            "The observed window is shorter than the staleness threshold.",
            watched,
            moved,
            Some(window_minutes),
        );
    }

    if moved > 0 {
        return verdict(
            "ok",
            "Quotes are moving.",
            watched,
            moved,
            Some(window_minutes),
        );
    }

    verdict(
        "error",
        "No held position on an open exchange has changed price for the whole window. \
         The quote feed is not updating and every downstream decision is running on \
         frozen prices.",
        watched,
        moved,
        Some(window_minutes),
    )
}

fn verdict(
    status: &str,
    message: &str,
    watched: usize,
    moved: usize,
    window_minutes: Option<i64>,
) -> JsonValue {
    json!({
        "status": status,
        "message": message,
        "watched_positions": watched,
        "positions_with_price_change": moved,
        "window_minutes": window_minutes,
    })
}

/// `timestamp` shifted back by `minutes`, kept as an RFC3339 string so the
/// comparison stays lexical and matches how the rows are stored.
fn rfc3339_minus_minutes(timestamp: &str, minutes: i64) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(
        (parsed - chrono::Duration::minutes(minutes.max(0)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

fn minutes_between(first: &str, last: &str) -> Option<i64> {
    let parse = |value: &str| chrono::DateTime::parse_from_rfc3339(value).ok();
    let (first, last) = (parse(first)?, parse(last)?);
    Some((last - first).num_minutes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(recorded_at: &str, symbol: &str, price: f64) -> JsonValue {
        json!({"recorded_at": recorded_at, "symbol": symbol, "price_local": price})
    }

    fn open(codes: &[&str]) -> HashSet<String> {
        codes.iter().map(|code| code.to_string()).collect()
    }

    /// An open market where nothing printed for the whole trailing window.
    /// Snapshots arrive about every ten minutes, so a frozen half hour is
    /// several identical rows per symbol rather than two distant ones.
    #[test]
    fn an_open_market_with_no_price_change_at_all_is_an_error() {
        let mut rows = Vec::new();
        for minute in ["00", "10", "20", "30", "40"] {
            rows.push(row(
                &format!("2026-09-18T12:{minute}:00Z"),
                "CHEMM:xcse",
                512.0,
            ));
            rows.push(row(
                &format!("2026-09-18T12:{minute}:00Z"),
                "DFDS:xcse",
                163.6,
            ));
        }

        let verdict = quote_freshness_verdict(&rows, &open(&["xcse"]), 30);
        assert_eq!(verdict["status"], "error");
        assert_eq!(verdict["watched_positions"], 2);
        assert_eq!(verdict["positions_with_price_change"], 0);
        assert_eq!(verdict["window_minutes"], 30);
    }

    /// Regression for a defect found in review: the caller fetches a wider span
    /// than the threshold so the window is always covered, and the first
    /// version measured that whole span. A single print 85 minutes ago then
    /// cleared a feed that had been dead for 30 -- the advertised threshold and
    /// the enforced one were different numbers.
    #[test]
    fn movement_older_than_the_threshold_does_not_clear_a_frozen_window() {
        let mut rows = vec![
            row("2026-09-18T11:00:00Z", "CHEMM:xcse", 505.0),
            row("2026-09-18T11:10:00Z", "CHEMM:xcse", 512.0), // the last real print
        ];
        for minute in ["00", "10", "20", "30", "40"] {
            rows.push(row(
                &format!("2026-09-18T12:{minute}:00Z"),
                "CHEMM:xcse",
                512.0,
            ));
        }

        let verdict = quote_freshness_verdict(&rows, &open(&["xcse"]), 30);
        assert_eq!(
            verdict["status"], "error",
            "a print 90 minutes ago says nothing about the last 30: {verdict}"
        );
    }

    /// A degraded or failed snapshot read must not look like a healthy feed.
    /// The caller falls back to an empty row set when the query errors, and
    /// reporting that as `ok` would rebuild the silent-pass defect this check
    /// exists to remove.
    #[test]
    fn missing_snapshot_evidence_is_unknown_rather_than_healthy() {
        let verdict = quote_freshness_verdict(&[], &open(&["xcse"]), 30);
        assert_eq!(verdict["status"], "unknown");

        // Holding nothing on the open exchange is a real answer, not a gap.
        let held_elsewhere = vec![row("2026-09-18T12:40:00Z", "CHEMM:xcse", 512.0)];
        assert_eq!(
            quote_freshness_verdict(&held_elsewhere, &open(&["xnys"]), 30)["status"],
            "not_applicable"
        );
    }

    /// One symbol printing is enough: the claim being tested is that the feed
    /// is alive, not that every instrument is liquid.
    #[test]
    fn a_single_moving_price_clears_the_watchdog() {
        let rows = vec![
            row("2026-09-18T08:10:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-18T08:10:00Z", "DFDS:xcse", 163.6),
            row("2026-09-18T12:40:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-18T12:40:00Z", "DFDS:xcse", 164.2),
        ];

        assert_eq!(
            quote_freshness_verdict(&rows, &open(&["xcse"]), 30)["status"],
            "ok"
        );
    }

    /// A price that moves and returns to where it started is a live feed. Only
    /// comparing the endpoints of the window would file it as frozen.
    #[test]
    fn a_round_trip_back_to_the_opening_price_still_counts_as_movement() {
        let rows = vec![
            row("2026-09-18T08:10:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-18T10:00:00Z", "CHEMM:xcse", 515.0),
            row("2026-09-18T12:40:00Z", "CHEMM:xcse", 512.0),
        ];

        assert_eq!(
            quote_freshness_verdict(&rows, &open(&["xcse"]), 30)["status"],
            "ok"
        );
    }

    /// Static prices outside trading hours are the normal state of the world
    /// and must never raise anything -- a watchdog that cries every evening is
    /// one the operator learns to ignore, which is how the end-of-day alert
    /// managed to fire only on weekends while the real outage ran silently.
    #[test]
    fn a_closed_market_is_never_reported_as_frozen() {
        let rows = vec![
            row("2026-09-20T08:10:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-20T12:40:00Z", "CHEMM:xcse", 512.0),
        ];

        assert_eq!(
            quote_freshness_verdict(&rows, &HashSet::new(), 30)["status"],
            "not_applicable"
        );
        assert_eq!(
            quote_freshness_verdict(&rows, &open(&["xnys"]), 30)["status"],
            "not_applicable",
            "positions on a closed exchange are not evidence about an open one"
        );
    }

    /// A window shorter than the threshold proves nothing. Without this a fresh
    /// restart would report a dead feed on its first cycle -- and a restart is
    /// exactly the situation this check exists to survive.
    #[test]
    fn a_window_shorter_than_the_threshold_does_not_accuse_the_feed() {
        let rows = vec![
            row("2026-09-18T08:10:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-18T08:20:00Z", "CHEMM:xcse", 512.0),
        ];

        let verdict = quote_freshness_verdict(&rows, &open(&["xcse"]), 30);
        assert_eq!(verdict["status"], "ok");
        assert_eq!(verdict["window_minutes"], 10);
    }
}
