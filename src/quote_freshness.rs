//! Watchdog for quotes that have stopped moving while a market is open.
//!
//! On 2026-09-19 the scheduler pod terminated with exit=1 at 08:09:09 and
//! restarted 25 seconds later. It came back reporting a healthy Saxo session
//! and refreshed the broker read model, but the price monitor acquired no
//! polling lease for the rest of the session. Quotes froze: 548 price changes
//! were recorded on 09-18 and zero on 09-19. Every downstream pulse went with
//! them -- no decision reports, no Markov run, no stop re-evaluation -- and a
//! full trading day was lost.
//!
//! Nothing raised an error, because nothing had failed. `last_cycle_status`
//! stayed `ok` and `integrity.healthy` stayed true throughout, since every
//! check asks whether an action went wrong and none asks whether an action
//! happened at all. This is the same shape as the end-of-day reflection that
//! was dead for seven days and the realised-sell panel that never rendered:
//! the failure is an absence, and absences pass every test written for errors.
//!
//! So this check inverts the question. It does not ask whether the last quote
//! fetch failed; it asks whether prices have moved at all on an exchange the
//! calendar says is open. A market that is open and completely static is not a
//! calm market -- across 14 positions it is a dead feed.

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

    let mut observed: BTreeMap<String, Observed> = BTreeMap::new();
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
        return verdict(
            "not_applicable",
            "No held position trades on an open exchange.",
            0,
            0,
            None,
        );
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

    /// The 2026-09-19 shape: an open market, positions held on it, and not one
    /// price change for the whole session. Every existing check passed that day
    /// because nothing errored -- the feed simply stopped, and a stopped feed
    /// looks exactly like a calm one to anything that only watches for failures.
    #[test]
    fn an_open_market_with_no_price_change_at_all_is_an_error() {
        let rows = vec![
            row("2026-09-19T08:10:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-19T08:10:00Z", "DFDS:xcse", 163.6),
            row("2026-09-19T12:40:00Z", "CHEMM:xcse", 512.0),
            row("2026-09-19T12:40:00Z", "DFDS:xcse", 163.6),
        ];

        let verdict = quote_freshness_verdict(&rows, &open(&["xcse"]), 30);
        assert_eq!(verdict["status"], "error");
        assert_eq!(verdict["watched_positions"], 2);
        assert_eq!(verdict["positions_with_price_change"], 0);
        assert_eq!(verdict["window_minutes"], 270);
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
