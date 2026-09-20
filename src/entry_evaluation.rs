//! Reproducible forward evaluation of every entry this system has made.
//!
//! Built 2026-09-20 after a week of ad-hoc analyses reached conclusions that
//! did not survive review. Those analyses were wrong in ways that had nothing
//! to do with arithmetic, and each mistake is a rule here:
//!
//! **The unit of observation is a BUY fill, not a sale.** Measuring from the
//! sale ledger silently conditions on having sold, which excludes every entry
//! still open and shapes the population by survivorship. A 26.6% hit rate
//! computed that way says nothing about entries that are still running.
//!
//! **Horizons are declared before any result is computed.** `HORIZONS` is a
//! constant and every report emits all of them. "Twenty entries positive at
//! some horizon" is trivially satisfiable when the horizon is chosen after
//! seeing the data, so the choice is removed from whoever reads the output.
//!
//! **An unelapsed horizon is `Pending`, never absent.** Dropping entries whose
//! window has not closed reintroduces the survivorship this module exists to
//! remove, and dropping them quietly makes the denominator a lie. Pending and
//! Unavailable are separate from each other and from a zero return.
//!
//! **Costs are applied identically everywhere or not at all.** A gross figure
//! and a net figure are different claims about different things, and comparing
//! one against the other -- shadow candidates measured after costs against
//! executed entries measured gross -- was one of the errors under review. Both
//! are reported side by side, never mixed.
//!
//! **Epochs are declared with their evidence.** The prompt, model, Markov
//! scaling and commission ceiling all changed inside the measured period, so a
//! pooled number describes no strategy that ever ran. Epoch boundaries are
//! source-controlled with the reason for each, and each entry also carries the
//! model actually observed so the declaration can be audited rather than
//! trusted.
//!
//! What this module deliberately does not do: prove causation, model intraday
//! paths, or simulate an exit policy. It measures what happened after each
//! entry, at fixed horizons, with stated costs.

use std::collections::BTreeMap;

use chrono::Datelike;
use serde_json::{Value as JsonValue, json};

/// Forward horizons in trading sessions, fixed before the first result was read.
///
/// Chosen to span the strategy's own stated intent -- "near-term opportunities
/// for the next 2 weeks, medium-term 1-3 months" -- rather than to flatter a
/// result: 1 session for immediate drift, 5 and 10 for the swing horizon the
/// gates are tuned to, 20 for the medium-term claim. Every report emits all
/// four. Adding a horizon after seeing an outcome is the failure this list
/// prevents, so a change here needs a reason recorded beside it.
pub(crate) const HORIZONS: &[i64] = &[1, 5, 10, 20];

/// Bumped whenever a change alters what a number here means -- horizons,
/// epochs, the cost model, the benchmark alignment.
///
/// Stored with every snapshot because the series is only comparable within one
/// version, and a row that silently changed meaning is worse than a missing
/// one. v1 measured the benchmark off by one session, matched it by array
/// position across two different calendars, and used the manager's acceptable-
/// commission ceiling as if it were the broker's fee; none of its numbers are
/// comparable with what follows.
pub(crate) const METHODOLOGY_VERSION: &str = "v2-2026-09-20";

/// A declared strategy epoch: entries inside it ran under the same rules.
///
/// Boundaries are commit-dated facts, not guesses. Pooling across them was
/// how a week of "the strategy loses" arrived without noticing that the
/// universe the model could see quadrupled on 2026-09-04.
pub(crate) struct Epoch {
    pub key: &'static str,
    pub start_utc: &'static str,
    pub reason: &'static str,
}

pub(crate) const EPOCHS: &[Epoch] = &[
    Epoch {
        key: "a_pre_bootstrap",
        start_utc: "2000-01-01T00:00:00Z",
        reason: "Book before the 2026-07-16 broker bootstrap; different account, not comparable.",
    },
    Epoch {
        key: "b_daily_markov",
        start_utc: "2026-07-16T00:00:00Z",
        reason: "Current book opens. Daily Markov bars; block alphabetical and capped at 80 of ~200.",
    },
    Epoch {
        key: "c_hourly_markov",
        start_utc: "2026-08-31T00:00:00Z",
        reason: "Markov moved to hourly bars with pre-report refresh slots and a recalibrated gate \
                 (40d6d29, 54f8b03, 0bb74e9). Split out in review: pooling this with the daily-bar \
                 weeks described a signal model that had already been replaced.",
    },
    Epoch {
        key: "d_scaled_markov",
        start_utc: "2026-09-02T00:00:00Z",
        reason: "Each signal persists the exchange-specific scaling it was computed under \
                 (e354432), so horizon and window are comparable across venues for the first time.",
    },
    Epoch {
        key: "e_widened_universe",
        start_utc: "2026-09-04T00:00:00Z",
        reason: "Markov block ranked by conviction and raised to the full 200 (47c9731, 40b8be9); \
                 commission ceiling 0.003 -> 0.004 (8f770da); decision model moved to \
                 gemini-flash-latest.",
    },
];

/// How many missed weeks the scheduler will still record.
///
/// Bounded because a backfilled week is computed from today's prices and would
/// silently claim to be what that week knew. Two weeks covers a holiday
/// outage; beyond that the gap is more honest than the reconstruction.
pub(crate) const SNAPSHOT_RECOVERY_WEEKS: i64 = 2;

/// The oldest unrecorded week whose snapshot instant has passed, if any.
///
/// Weekly rather than daily because the horizons are 1 to 20 sessions: a daily
/// snapshot mostly re-reports the same resolved cells and buries the change
/// that matters. The mark is a weekday and time rather than "a week since the
/// last run", which drifts later every week until it lands mid-session.
///
/// Recovery is why this returns a week rather than a yes/no. The first version
/// asked "is today at or after the configured weekday", which is false on
/// Monday -- `num_days_from_monday` is 0 for Monday and 6 for Sunday -- and by
/// then the ISO week has already advanced, so a missed Sunday was lost for
/// good. That was the opposite of the stated guarantee. Walking back over
/// recent weeks and returning the oldest one still owed makes the recovery
/// real: a scheduler down all weekend records the missed week on Monday.
pub(crate) fn due_week_key(
    now_local: chrono::NaiveDateTime,
    weekday: chrono::Weekday,
    at_or_after: chrono::NaiveTime,
    existing_weeks: &[String],
    lookback_weeks: i64,
) -> Option<String> {
    // Every week inside the lookback whose mark has already passed, oldest
    // first. ISO keys are "YYYY-Www", which sorts chronologically as text.
    let mut passed: Vec<String> = (0..=lookback_weeks.max(0))
        .filter_map(|weeks_back| {
            let candidate = now_local.date() - chrono::Duration::weeks(weeks_back);
            let offset = i64::from(candidate.weekday().num_days_from_monday())
                - i64::from(weekday.num_days_from_monday());
            let mark_day = candidate - chrono::Duration::days(offset);
            let mark = mark_day.and_time(at_or_after);
            (now_local >= mark).then(|| {
                let week = mark_day.iso_week();
                format!("{}-W{:02}", week.year(), week.week())
            })
        })
        .collect();
    passed.sort();
    passed.dedup();

    // With nothing recorded there is nothing to recover. A first run waits for
    // its own mark rather than backfilling: a week rebuilt from today's prices
    // and filed under an earlier date is a fiction, and one indistinguishable
    // from real evidence once it sits in the series.
    let Some(latest_recorded) = existing_weeks.iter().max() else {
        let this_week = {
            let week = now_local.date().iso_week();
            format!("{}-W{:02}", week.year(), week.week())
        };
        return passed.into_iter().find(|week| *week == this_week);
    };
    passed.into_iter().find(|week| week > latest_recorded)
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        // One row per ISO week. The week is the primary key because the value
        // of this series is comparability across runs, and two snapshots of the
        // same week -- taken hours apart, resolving different numbers of
        // horizons -- would silently become two different answers to one
        // question. Re-running a week updates it rather than appending.
        "CREATE TABLE IF NOT EXISTS entry_evaluation_snapshots (
            iso_week TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            entry_count INTEGER NOT NULL,
            summary_json TEXT NOT NULL
        )",
    ]
}

/// The ISO year-week a timestamp falls in, e.g. `2026-W38`.
///
/// Weeks rather than dates because the run is weekly and a date key would make
/// a Monday rerun look like a new observation.
pub(crate) fn iso_week_key(now: chrono::DateTime<chrono::Utc>) -> String {
    let week = now.iso_week();
    format!("{}-W{:02}", week.year(), week.week())
}

/// The regional proxy an entry is compared against.
///
/// Matched by listing venue rather than by the instrument's own sector so the
/// comparison is a market the entry actually competed with. A venue with no
/// declared proxy returns the global reference rather than silently dropping
/// the comparison.
pub(crate) fn benchmark_key_for_exchange(exchange: &str) -> &'static str {
    match exchange.trim().to_ascii_lowercase().as_str() {
        "xnas" => "us_technology",
        "xnys" => "us_large_cap",
        "xlon" => "uk_large_cap",
        "xcse" | "xetr" | "xosl" | "xhel" | "xsto" | "xome" | "xpar" | "xbru" | "xams" | "xmil"
        | "xmad" | "xswx" | "xwbo" | "xlis" => "europe_broad",
        _ => "global_equity",
    }
}

/// One entry's outcome at one horizon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HorizonStatus {
    /// The window closed and a price was found at both ends.
    Resolved,
    /// The window has not elapsed yet. Not a zero, not a loss, not excluded.
    Pending,
    /// The window elapsed but no price is stored for it.
    Unavailable,
}

impl HorizonStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Pending => "pending",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Saxo's commission rate per side, matching the fill-cost fallback in
/// `saxo_order::commission_dkk_for_order`.
///
/// The first version of this module used
/// `trading_manager::DEFAULT_MAX_COMMISSION_PCT_PER_SIDE` (0.004), which is the
/// *maximum commission burden the manager will accept on a clip* -- a gate
/// threshold, and five times the real fee. That forced a 0.8% round-trip floor
/// onto every entry and manufactured the "costs dominate at short horizons"
/// finding the first run reported. A ceiling is not a price.
pub(crate) const COMMISSION_RATE_PER_SIDE: f64 = 0.0008;

/// Round-trip cost as a fraction of notional, applied identically to every
/// entry so that net figures are comparable across symbols and venues.
///
/// Both sides are charged at the exchange minimum commission or the rate,
/// whichever binds -- the same shape the fill ledger books -- which is why a
/// small clip on an expensive venue can be uneconomic however good the signal.
/// Tax is excluded on purpose: Danish share income uses the average method
/// across the whole book and cannot be attributed to one entry without
/// inventing a number.
pub(crate) fn round_trip_cost_fraction(
    notional_local: f64,
    min_commission_local: f64,
    pct_per_side: f64,
) -> f64 {
    if !notional_local.is_finite() || notional_local <= 0.0 {
        return 0.0;
    }
    let per_side = (notional_local * pct_per_side).max(min_commission_local);
    2.0 * per_side / notional_local
}

/// The declared epoch an entry belongs to, by fill time.
pub(crate) fn epoch_for(filled_at: &str) -> &'static str {
    EPOCHS
        .iter()
        .rev()
        .find(|epoch| filled_at >= epoch.start_utc)
        .map(|epoch| epoch.key)
        .unwrap_or("unclassified")
}

/// One evaluated entry.
pub(crate) struct Entry<'a> {
    pub symbol: &'a str,
    pub filled_at: &'a str,
    pub fill_price_local: f64,
    pub quantity: f64,
    pub model: &'a str,
    /// `(date, close)` for this symbol strictly after the fill date, oldest
    /// first. The horizon is counted in rows of this series, which is the
    /// symbol's own session calendar.
    pub forward_closes: &'a [(String, f64)],
    /// `(date, close)` for the benchmark, oldest first, starting on or before
    /// the fill date. Matched to the stock **by date**, never by position:
    /// the indicator series carries a 2026-09-07 row for US symbols (Labor
    /// Day, repeating the 09-04 close) that the benchmark series does not, so
    /// positional matching silently compares different intervals from there on.
    pub benchmark_closes: &'a [(String, f64)],
    pub min_commission_local: f64,
    pub rate_per_side: f64,
}

/// The benchmark close on or before `date`.
///
/// "On or before" rather than "exactly on" because the two series keep
/// different calendars: a venue holiday, a missing observation, or a repeated
/// close appears in one and not the other. Carrying the last known close
/// forward compares the same interval on both sides; requiring an exact match
/// would discard the comparison on precisely the days it differs.
fn benchmark_close_at(series: &[(String, f64)], date: &str) -> Option<f64> {
    series
        .iter()
        .filter(|(observed, close)| observed.as_str() <= date && close.is_finite() && *close > 0.0)
        .next_back()
        .map(|(_, close)| *close)
}

fn horizon_outcome(
    entry: &Entry<'_>,
    horizon: i64,
    sessions_elapsed: i64,
    fill_day: &str,
) -> JsonValue {
    let index = usize::try_from(horizon - 1).unwrap_or(usize::MAX);
    let status = if sessions_elapsed < horizon {
        HorizonStatus::Pending
    } else if entry.fill_price_local <= 0.0 || entry.forward_closes.get(index).is_none() {
        HorizonStatus::Unavailable
    } else {
        HorizonStatus::Resolved
    };

    if status != HorizonStatus::Resolved {
        return json!({
            "horizon_sessions": horizon,
            "status": status.as_str(),
            "gross_return": JsonValue::Null,
            "net_return": JsonValue::Null,
            "benchmark_return": JsonValue::Null,
            "excess_return": JsonValue::Null,
        });
    }

    let (horizon_date, close) = entry.forward_closes[index].clone();
    let gross = close / entry.fill_price_local - 1.0;
    let cost = round_trip_cost_fraction(
        entry.fill_price_local * entry.quantity,
        entry.min_commission_local,
        entry.rate_per_side,
    );
    let net = gross - cost;

    // Both legs are looked up by date. The first version measured the
    // benchmark from `closes[h-1] / closes[0]`, which at one session divided
    // the base by itself and reported every one-session comparison as exactly
    // zero, and at every other horizon compared h-1 benchmark sessions against
    // h stock sessions.
    let benchmark = benchmark_close_at(entry.benchmark_closes, fill_day).and_then(|base| {
        benchmark_close_at(entry.benchmark_closes, &horizon_date)
            .map(|at_horizon| at_horizon / base - 1.0)
    });

    json!({
        "horizon_sessions": horizon,
        "status": status.as_str(),
        "horizon_date": horizon_date,
        "gross_return": gross,
        "net_return": net,
        "round_trip_cost_fraction": cost,
        "benchmark_return": benchmark,
        "excess_return": benchmark.map(|value| net - value),
    })
}

/// Evaluates one entry at every declared horizon.
pub(crate) fn evaluate_entry(entry: &Entry<'_>, sessions_elapsed: i64) -> JsonValue {
    json!({
        "symbol": entry.symbol,
        "filled_at": entry.filled_at,
        "epoch": epoch_for(entry.filled_at),
        "model": entry.model,
        "fill_price_local": entry.fill_price_local,
        "quantity": entry.quantity,
        "notional_local": entry.fill_price_local * entry.quantity,
        "sessions_elapsed": sessions_elapsed,
        "horizons": HORIZONS
            .iter()
            .map(|horizon| {
                horizon_outcome(
                    entry,
                    *horizon,
                    sessions_elapsed,
                    entry.filled_at.get(..10).unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>(),
    })
}

/// Aggregates evaluated entries by epoch and horizon.
///
/// Every cell reports its own denominator. A hit rate over four resolved
/// entries and one over forty are different claims, and a table that hides
/// which is which invites the reader to treat them alike.
pub(crate) fn summarize(entries: &[JsonValue]) -> JsonValue {
    let mut cells: BTreeMap<(String, i64), Vec<(f64, f64, Option<f64>)>> = BTreeMap::new();
    let mut pending: BTreeMap<(String, i64), i64> = BTreeMap::new();
    let mut unavailable: BTreeMap<(String, i64), i64> = BTreeMap::new();
    let mut epoch_entry_count: BTreeMap<String, i64> = BTreeMap::new();

    for entry in entries {
        let epoch = entry
            .get("epoch")
            .and_then(JsonValue::as_str)
            .unwrap_or("unclassified")
            .to_string();
        *epoch_entry_count.entry(epoch.clone()).or_default() += 1;
        for outcome in entry
            .get("horizons")
            .and_then(JsonValue::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let horizon = outcome
                .get("horizon_sessions")
                .and_then(JsonValue::as_i64)
                .unwrap_or_default();
            let key = (epoch.clone(), horizon);
            match outcome.get("status").and_then(JsonValue::as_str) {
                Some("resolved") => {
                    let gross = outcome
                        .get("gross_return")
                        .and_then(JsonValue::as_f64)
                        .unwrap_or_default();
                    let net = outcome
                        .get("net_return")
                        .and_then(JsonValue::as_f64)
                        .unwrap_or_default();
                    let excess = outcome.get("excess_return").and_then(JsonValue::as_f64);
                    cells.entry(key).or_default().push((gross, net, excess));
                }
                Some("pending") => *pending.entry(key).or_default() += 1,
                _ => *unavailable.entry(key).or_default() += 1,
            }
        }
    }

    let mut rows = Vec::new();
    for epoch in EPOCHS.iter().map(|epoch| epoch.key) {
        for horizon in HORIZONS {
            let key = (epoch.to_string(), *horizon);
            let observations = cells.get(&key).cloned().unwrap_or_default();
            let resolved = observations.len() as i64;
            let mean = |values: Vec<f64>| {
                (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
            };
            let excess: Vec<f64> = observations.iter().filter_map(|row| row.2).collect();
            rows.push(json!({
                "epoch": epoch,
                "horizon_sessions": horizon,
                "resolved": resolved,
                "pending": pending.get(&key).copied().unwrap_or_default(),
                "unavailable": unavailable.get(&key).copied().unwrap_or_default(),
                "mean_gross_return": mean(observations.iter().map(|row| row.0).collect()),
                "mean_net_return": mean(observations.iter().map(|row| row.1).collect()),
                "net_positive_rate": (resolved > 0).then(|| {
                    observations.iter().filter(|row| row.1 > 0.0).count() as f64 / resolved as f64
                }),
                "benchmark_compared": excess.len() as i64,
                "mean_excess_return": mean(excess),
            }));
        }
    }

    json!({
        "horizons": HORIZONS,
        "epochs": EPOCHS.iter().map(|epoch| json!({
            "key": epoch.key,
            "start_utc": epoch.start_utc,
            "reason": epoch.reason,
        })).collect::<Vec<_>>(),
        "entry_count": entries.len(),
        "entries_by_epoch": epoch_entry_count,
        "cells": rows,
        "methodology_version": METHODOLOGY_VERSION,
        "definitions": {
            "unit": "one BUY fill, including entries whose position is still open",
            "out_of_sample": "none of it yet. The horizons, epochs and cost model were all \
                              chosen with this data visible, and entries recorded after that \
                              choice were still selected by a strategy tuned before it. A \
                              snapshot is out-of-sample only for entries filled after the \
                              methodology_version it was measured under.",
            "reproducibility": "a snapshot records the methodology version it used, but \
                                re-running the endpoint today cannot reconstruct what an \
                                earlier snapshot knew: the price series it reads has since \
                                been extended and its pending cells have resolved.",
            "gross_return": "close at horizon / fill price - 1, local currency, no costs",
            "net_return": "gross minus modelled round-trip commission as a fraction of notional",
            "benchmark_return": "same-horizon return of the matched regional proxy",
            "pending": "horizon has not elapsed; excluded from every mean, never counted as zero",
            "tax": "excluded: Danish share income uses the average method across the book",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(
        symbol: &'a str,
        filled_at: &'a str,
        price: f64,
        forward: &'a [(String, f64)],
        bench: &'a [(String, f64)],
    ) -> Entry<'a> {
        Entry {
            symbol,
            filled_at,
            fill_price_local: price,
            quantity: 10.0,
            model: "test-model",
            forward_closes: forward,
            benchmark_closes: bench,
            min_commission_local: 3.0,
            rate_per_side: COMMISSION_RATE_PER_SIDE,
        }
    }

    /// Dated fixture series. Calendar days are enough to order these rows.
    fn dated(start_day: u32, closes: &[f64]) -> Vec<(String, f64)> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| (format!("2026-09-{:02}", start_day + index as u32), *close))
            .collect()
    }

    /// The defect this module exists to remove: an entry whose window has not
    /// closed is not a zero and not a loss, and it must not vanish from the
    /// denominator. Measuring only closed positions is what produced a hit rate
    /// that described sold trades and was read as describing the strategy.
    #[test]
    fn an_unelapsed_horizon_is_pending_and_never_a_return() {
        let forward = dated(11, &[101.0, 102.0]);
        let bench = dated(10, &[50.0, 50.5, 51.0]);
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-09-18T08:00:00Z", 100.0, &forward, &bench),
            2,
        );
        let statuses: Vec<&str> = evaluated["horizons"]
            .as_array()
            .expect("horizons")
            .iter()
            .map(|row| row["status"].as_str().expect("status"))
            .collect();

        assert_eq!(statuses, vec!["resolved", "pending", "pending", "pending"]);
        for row in evaluated["horizons"]
            .as_array()
            .expect("horizons")
            .iter()
            .skip(1)
        {
            assert!(
                row["gross_return"].is_null(),
                "pending must carry no return: {row}"
            );
        }

        let summary = summarize(&[evaluated]);
        let cell = summary["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .find(|row| row["horizon_sessions"] == 5 && row["epoch"] == "e_widened_universe")
            .expect("the 5-session cell");
        assert_eq!(cell["resolved"], 0);
        assert_eq!(cell["pending"], 1);
        assert!(
            cell["mean_gross_return"].is_null(),
            "a pending cell has no mean"
        );
    }

    /// An elapsed window with no stored price is missing evidence, which is a
    /// different claim from "the horizon has not arrived". Collapsing the two
    /// would let a gap in the price series read as a young entry.
    #[test]
    fn an_elapsed_horizon_without_a_price_is_unavailable_not_pending() {
        let forward = dated(11, &[101.0]);
        let bench = dated(10, &[50.0, 50.5]);
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-08-01T08:00:00Z", 100.0, &forward, &bench),
            20,
        );
        let statuses: Vec<&str> = evaluated["horizons"]
            .as_array()
            .expect("horizons")
            .iter()
            .map(|row| row["status"].as_str().expect("status"))
            .collect();
        assert_eq!(
            statuses,
            vec!["resolved", "unavailable", "unavailable", "unavailable"]
        );
    }

    /// Gross and net are separate columns because comparing one against the
    /// other is a real mistake this project already made: shadow candidates
    /// were quoted after costs beside executed entries quoted gross, and the
    /// two were read as equivalent.
    #[test]
    fn gross_and_net_are_reported_separately_and_costs_bind_on_small_clips() {
        // 10 shares at 100 = 1,000 notional. 0.12% per side is 1.20, under the
        // 3.00 exchange minimum, so the minimum binds: 6.00 round trip = 0.6%.
        let forward = dated(11, &[110.0]);
        let bench = dated(10, &[50.0, 50.0]);
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-08-01T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let first = &evaluated["horizons"][0];
        assert!((first["gross_return"].as_f64().expect("gross") - 0.10).abs() < 1e-9);
        assert!((first["round_trip_cost_fraction"].as_f64().expect("cost") - 0.006).abs() < 1e-9);
        assert!((first["net_return"].as_f64().expect("net") - 0.094).abs() < 1e-9);

        // A large enough clip and the rate binds instead of the floor.
        assert!(
            (round_trip_cost_fraction(100_000.0, 3.0, COMMISSION_RATE_PER_SIDE) - 0.0016).abs()
                < 1e-9,
            "the rate takes over once it exceeds the floor"
        );
        // The rate is the broker's, not the manager's acceptable-burden
        // ceiling. Using the ceiling forced a 0.8% round-trip floor onto every
        // entry and manufactured the first run's "costs dominate" finding.
        assert!(
            COMMISSION_RATE_PER_SIDE < crate::trading_manager::DEFAULT_MAX_COMMISSION_PCT_PER_SIDE,
            "a gate threshold is not a price"
        );
    }

    /// Regression for a defect found in review. The benchmark leg was indexed
    /// `closes[h-1] / closes[0]`, so a one-session comparison divided the base
    /// by itself and reported exactly zero, and every other horizon compared
    /// h-1 benchmark sessions against h stock sessions.
    #[test]
    fn the_benchmark_measures_the_same_interval_as_the_entry() {
        let forward = dated(11, &[105.0, 110.0]);
        // Base 100 on the fill day, then +1% per session.
        let bench = dated(10, &[100.0, 101.0, 102.0]);
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-09-10T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let first = &evaluated["horizons"][0];
        assert!(
            (first["benchmark_return"].as_f64().expect("benchmark") - 0.01).abs() < 1e-9,
            "one session of benchmark, not zero: {first}"
        );
    }

    /// The two series keep different calendars. US indicator rows carry a
    /// 2026-09-07 entry (Labor Day, repeating the 09-04 close) that the
    /// benchmark series does not, so matching by array position silently shifts
    /// the comparison by a session from there on.
    #[test]
    fn a_calendar_gap_on_one_side_does_not_shift_the_comparison() {
        let forward = vec![
            ("2026-09-07".to_string(), 100.0), // holiday row, repeated close
            ("2026-09-08".to_string(), 110.0),
        ];
        // The benchmark has no 09-07 row at all.
        let bench = vec![
            ("2026-09-04".to_string(), 100.0),
            ("2026-09-08".to_string(), 102.0),
        ];
        let evaluated = evaluate_entry(
            &entry("A:xnas", "2026-09-04T14:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let second = &evaluated["horizons"]
            .as_array()
            .expect("horizons")
            .iter()
            .find(|row| row["horizon_sessions"] == 1)
            .cloned()
            .expect("first horizon");
        // Horizon 1 lands on the 09-07 holiday row; the benchmark has nothing
        // that day, so the last close on or before it is the 09-04 base: 0%.
        assert!(
            second["benchmark_return"]
                .as_f64()
                .is_some_and(|value| value.abs() < 1e-9),
            "a benchmark with no row that day carries the base forward: {second}"
        );
    }

    /// Pooling across a rule change describes no strategy that ever ran. The
    /// 2026-09-04 boundary quadrupled the universe the model could see, moved
    /// the commission ceiling and changed the model, so entries either side of
    /// it are different experiments.
    #[test]
    fn entries_are_separated_by_the_declared_epoch_they_ran_under() {
        assert_eq!(epoch_for("2026-06-01T00:00:00Z"), "a_pre_bootstrap");
        assert_eq!(epoch_for("2026-08-20T00:00:00Z"), "b_daily_markov");
        // Review caught these two hiding inside one epoch: the hourly-bar move
        // and the exchange-specific scaling both changed the signal the gate
        // reads, so entries either side of them are different experiments.
        assert_eq!(epoch_for("2026-09-01T08:00:00Z"), "c_hourly_markov");
        assert_eq!(epoch_for("2026-09-03T08:00:00Z"), "d_scaled_markov");
        assert_eq!(epoch_for("2026-09-04T08:00:00Z"), "e_widened_universe");

        let forward = dated(11, &[110.0]);
        let bench = dated(10, &[50.0, 50.0]);
        let old = evaluate_entry(
            &entry("A:xcse", "2026-08-20T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let new = evaluate_entry(
            &entry("A:xcse", "2026-09-10T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let summary = summarize(&[old, new]);
        let resolved_at = |epoch: &str| {
            summary["cells"]
                .as_array()
                .expect("cells")
                .iter()
                .find(|row| row["epoch"] == epoch && row["horizon_sessions"] == 1)
                .expect("cell")["resolved"]
                .as_i64()
                .expect("count")
        };
        assert_eq!(resolved_at("b_daily_markov"), 1);
        assert_eq!(resolved_at("e_widened_universe"), 1);
    }

    /// Every declared horizon is emitted on every run. Reporting only the
    /// horizon that happened to look good is the failure the fixed list
    /// prevents, and a reader who never sees the others cannot notice.
    #[test]
    fn every_declared_horizon_appears_in_the_output() {
        let forward = dated(11, &[101.0, 102.0, 103.0, 104.0, 105.0]);
        let bench = dated(10, &[50.0; 6]);
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-09-10T08:00:00Z", 100.0, &forward, &bench),
            3,
        );
        let horizons: Vec<i64> = evaluated["horizons"]
            .as_array()
            .expect("horizons")
            .iter()
            .map(|row| row["horizon_sessions"].as_i64().expect("horizon"))
            .collect();
        assert_eq!(horizons, HORIZONS.to_vec());

        let summary = summarize(&[evaluated]);
        assert_eq!(
            summary["cells"].as_array().expect("cells").len(),
            EPOCHS.len() * HORIZONS.len(),
            "every epoch/horizon cell is present even when empty"
        );
    }

    /// A missing benchmark must not discard an otherwise measurable entry.
    /// Excess goes null; gross and net still stand on their own.
    #[test]
    fn a_missing_benchmark_leaves_the_entry_measurable() {
        let forward = dated(11, &[110.0]);
        let bench: Vec<(String, f64)> = Vec::new();
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-09-10T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let first = &evaluated["horizons"][0];
        assert_eq!(first["status"], "resolved");
        assert!(first["gross_return"].as_f64().is_some());
        assert!(first["excess_return"].is_null());

        let summary = summarize(&[evaluated]);
        let cell = summary["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .find(|row| row["epoch"] == "e_widened_universe" && row["horizon_sessions"] == 1)
            .expect("cell");
        assert_eq!(cell["resolved"], 1);
        assert_eq!(cell["benchmark_compared"], 0);
        assert!(cell["mean_excess_return"].is_null());
    }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use chrono::{NaiveDate, Weekday};

    fn at(day: u32, hour: u32, minute: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, day)
            .expect("date")
            .and_hms_opt(hour, minute, 0)
            .expect("time")
    }

    fn mark() -> chrono::NaiveTime {
        chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("time")
    }

    /// 2026-09-20 is a Sunday, 09-21 a Monday, and ISO week 38 runs Mon 09-14
    /// to Sun 09-20. Spelled out because misreading a weekday is how a quiet
    /// Saturday became a reported outage in this project.
    #[test]
    fn the_snapshot_waits_for_its_mark_then_records_once() {
        let recorded = ["2026-W37".to_string()];
        assert_eq!(
            due_week_key(at(20, 21, 59), Weekday::Sun, mark(), &recorded, 4),
            None,
            "before the mark"
        );
        assert_eq!(
            due_week_key(at(20, 22, 0), Weekday::Sun, mark(), &recorded, 4).as_deref(),
            Some("2026-W38")
        );
        let recorded = ["2026-W37".to_string(), "2026-W38".to_string()];
        assert_eq!(
            due_week_key(at(20, 23, 30), Weekday::Sun, mark(), &recorded, 4),
            None,
            "the ISO-week key makes a restart idempotent"
        );
    }

    /// Regression for a defect found in review. The old gate asked whether
    /// today's weekday was at or after Sunday; on Monday that is false --
    /// `num_days_from_monday` is 0 for Monday and 6 for Sunday -- and the ISO
    /// week has already turned, so a missed Sunday was lost for good. That was
    /// the opposite of the recovery this was documented as providing.
    #[test]
    fn a_missed_sunday_is_recorded_on_monday_not_lost() {
        let recorded = ["2026-W37".to_string()];
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &recorded, 4).as_deref(),
            Some("2026-W38"),
            "Monday is inside W39, and W38 is still owed"
        );

        let recorded = ["2026-W37".to_string(), "2026-W38".to_string()];
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &recorded, 4),
            None,
            "and once recorded it owes nothing until its own Sunday"
        );
    }

    /// A weekend of downtime owes the oldest missed week first, so the series
    /// fills in order rather than skipping to the present and leaving a hole
    /// that is invisible once later snapshots land beside it.
    #[test]
    fn the_oldest_unrecorded_week_is_recovered_first() {
        let recorded = ["2026-W36".to_string()];
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &recorded, 4).as_deref(),
            Some("2026-W37")
        );
        let recorded = ["2026-W36".to_string(), "2026-W37".to_string()];
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &recorded, 4).as_deref(),
            Some("2026-W38")
        );
    }

    /// A first run waits for its own mark. Backfilling would file weeks the
    /// system was never up for under earlier dates, each rebuilt from today's
    /// prices and then indistinguishable from real evidence in the series.
    #[test]
    fn a_first_run_does_not_backfill_weeks_that_were_never_owed() {
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &[], 4),
            None,
            "Monday of W39, whose own mark is the coming Sunday"
        );
        assert_eq!(
            due_week_key(at(20, 22, 0), Weekday::Sun, mark(), &[], 4).as_deref(),
            Some("2026-W38"),
            "but its own mark, once passed, is owed"
        );
    }

    /// Recovery is bounded: a long outage fills recent weeks, not the whole
    /// history, because an old week rebuilt from today's prices is a fiction.
    #[test]
    fn recovery_does_not_reach_back_indefinitely() {
        let recorded = ["2026-W30".to_string()];
        assert_eq!(
            due_week_key(at(21, 9, 0), Weekday::Sun, mark(), &recorded, 2).as_deref(),
            Some("2026-W37"),
            "only weeks inside the lookback are owed"
        );
    }

    /// Weeks are the key, not dates: two runs on different days of one week are
    /// one observation.
    #[test]
    fn the_week_key_is_iso_and_stable_across_the_week() {
        let monday = chrono::DateTime::parse_from_rfc3339("2026-09-14T08:00:00Z")
            .expect("ts")
            .with_timezone(&chrono::Utc);
        let friday = chrono::DateTime::parse_from_rfc3339("2026-09-18T20:00:00Z")
            .expect("ts")
            .with_timezone(&chrono::Utc);
        assert_eq!(iso_week_key(monday), iso_week_key(friday));
        assert_eq!(iso_week_key(monday), "2026-W38");
    }
}
