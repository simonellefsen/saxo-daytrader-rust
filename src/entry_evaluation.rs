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
        key: "b_post_bootstrap",
        start_utc: "2026-07-16T00:00:00Z",
        reason: "Current book opens. Markov block alphabetical and capped at 80 of ~200 symbols.",
    },
    Epoch {
        key: "c_widened_universe",
        start_utc: "2026-09-04T00:00:00Z",
        reason: "Markov block ranked by conviction and raised to the full 200 (40b8be9, 47c9731); \
                 commission ceiling 0.003 -> 0.004; decision model moved to gemini-flash-latest.",
    },
];

/// Whether this week's snapshot is due at `now_local`.
///
/// Weekly rather than daily because the horizons are 1 to 20 sessions: a daily
/// snapshot would mostly re-report the same resolved cells and bury the change
/// that matters. Gated on weekday-and-time reaching the configured mark, with
/// the ISO-week primary key doing the real idempotency -- a scheduler that
/// restarts ten times on Sunday evening still records one snapshot.
///
/// Deliberately not "run if a week has passed since the last one": that drifts
/// later every week and eventually lands mid-session, where a snapshot would
/// mix a partial trading day into the series.
pub(crate) fn snapshot_due(
    now_local: chrono::NaiveDateTime,
    weekday: chrono::Weekday,
    at_or_after: chrono::NaiveTime,
    existing_weeks: &[String],
    week_key: &str,
) -> bool {
    if existing_weeks.iter().any(|week| week == week_key) {
        return false;
    }
    // Later in the week is still due: a scheduler that was down on Sunday must
    // not silently skip the week, because a gap in the series is invisible
    // once the next snapshot lands beside it.
    let day_reached = now_local.weekday().num_days_from_monday() >= weekday.num_days_from_monday();
    let time_reached = now_local.weekday().num_days_from_monday() > weekday.num_days_from_monday()
        || now_local.time() >= at_or_after;
    day_reached && time_reached
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

/// Round-trip cost as a fraction of notional, applied identically to every
/// entry so that net figures are comparable across symbols and venues.
///
/// Both sides are charged at the exchange minimum commission or the percentage
/// fee, whichever binds -- which is how the Trading Manager's own cost guard
/// reasons, and why a small clip on an expensive venue can be uneconomic
/// however good the signal. Tax is excluded on purpose: Danish share income
/// uses the average method across the whole book and cannot be attributed to
/// one entry without inventing a number.
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
    /// Closes for this symbol strictly after the fill date, oldest first.
    pub forward_closes: &'a [f64],
    /// Benchmark closes over the same sessions, oldest first. Same length
    /// contract as `forward_closes`; a short series yields `Unavailable`
    /// rather than a silently shorter comparison.
    pub benchmark_closes: &'a [f64],
    pub min_commission_local: f64,
    pub pct_per_side: f64,
}

fn horizon_outcome(entry: &Entry<'_>, horizon: i64, sessions_elapsed: i64) -> JsonValue {
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

    let close = entry.forward_closes[index];
    let gross = close / entry.fill_price_local - 1.0;
    let cost = round_trip_cost_fraction(
        entry.fill_price_local * entry.quantity,
        entry.min_commission_local,
        entry.pct_per_side,
    );
    let net = gross - cost;
    // The benchmark is optional evidence: its absence must not discard an
    // otherwise measurable entry, so excess is null while gross and net stand.
    let benchmark = entry
        .benchmark_closes
        .get(index)
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .and_then(|close_h| {
            entry
                .benchmark_closes
                .first()
                .copied()
                .filter(|base| base.is_finite() && *base > 0.0)
                .map(|base| close_h / base - 1.0)
        });

    json!({
        "horizon_sessions": horizon,
        "status": status.as_str(),
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
            .map(|horizon| horizon_outcome(entry, *horizon, sessions_elapsed))
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
        "definitions": {
            "unit": "one BUY fill, including entries whose position is still open",
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
        forward: &'a [f64],
        bench: &'a [f64],
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
            pct_per_side: 0.0012,
        }
    }

    /// The defect this module exists to remove: an entry whose window has not
    /// closed is not a zero and not a loss, and it must not vanish from the
    /// denominator. Measuring only closed positions is what produced a hit rate
    /// that described sold trades and was read as describing the strategy.
    #[test]
    fn an_unelapsed_horizon_is_pending_and_never_a_return() {
        let forward = [101.0, 102.0];
        let bench = [50.0, 50.5];
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
            .find(|row| row["horizon_sessions"] == 5 && row["epoch"] == "c_widened_universe")
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
        let forward = [101.0];
        let bench = [50.0, 50.5];
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
        let forward = [110.0];
        let bench = [50.0, 50.0];
        let evaluated = evaluate_entry(
            &entry("A:xcse", "2026-08-01T08:00:00Z", 100.0, &forward, &bench),
            5,
        );
        let first = &evaluated["horizons"][0];
        assert!((first["gross_return"].as_f64().expect("gross") - 0.10).abs() < 1e-9);
        assert!((first["round_trip_cost_fraction"].as_f64().expect("cost") - 0.006).abs() < 1e-9);
        assert!((first["net_return"].as_f64().expect("net") - 0.094).abs() < 1e-9);

        // Ten times the clip and the percentage fee binds instead.
        assert!(
            (round_trip_cost_fraction(10_000.0, 3.0, 0.0012) - 0.0024).abs() < 1e-9,
            "the percentage takes over once it exceeds the floor"
        );
    }

    /// Pooling across a rule change describes no strategy that ever ran. The
    /// 2026-09-04 boundary quadrupled the universe the model could see, moved
    /// the commission ceiling and changed the model, so entries either side of
    /// it are different experiments.
    #[test]
    fn entries_are_separated_by_the_declared_epoch_they_ran_under() {
        assert_eq!(epoch_for("2026-06-01T00:00:00Z"), "a_pre_bootstrap");
        assert_eq!(epoch_for("2026-08-20T00:00:00Z"), "b_post_bootstrap");
        assert_eq!(epoch_for("2026-09-04T08:00:00Z"), "c_widened_universe");

        let forward = [110.0];
        let bench = [50.0, 50.0];
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
        assert_eq!(resolved_at("b_post_bootstrap"), 1);
        assert_eq!(resolved_at("c_widened_universe"), 1);
    }

    /// Every declared horizon is emitted on every run. Reporting only the
    /// horizon that happened to look good is the failure the fixed list
    /// prevents, and a reader who never sees the others cannot notice.
    #[test]
    fn every_declared_horizon_appears_in_the_output() {
        let forward = [101.0, 102.0, 103.0, 104.0, 105.0];
        let bench = [50.0; 5];
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
        let forward = [110.0];
        let bench: [f64; 0] = [];
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
            .find(|row| row["epoch"] == "c_widened_universe" && row["horizon_sessions"] == 1)
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

    /// 2026-09-20 is a Sunday; 09-21 Monday. Written out because misreading a
    /// weekday is how a quiet Saturday became a reported outage in this project
    /// three days ago.
    #[test]
    fn the_snapshot_waits_for_its_weekday_and_time_then_runs_once() {
        let mark = chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("time");
        let week = "2026-W38";

        assert!(
            !snapshot_due(at(20, 21, 59), Weekday::Sun, mark, &[], week),
            "before the configured time"
        );
        assert!(snapshot_due(at(20, 22, 0), Weekday::Sun, mark, &[], week));
        assert!(
            !snapshot_due(at(20, 23, 0), Weekday::Sun, mark, &[week.to_string()], week),
            "the ISO-week key makes a restart idempotent"
        );
    }

    /// A missed Sunday must still record the week. A silent gap in the series
    /// is invisible once the next snapshot lands beside it, and this series
    /// exists precisely to show change between runs.
    #[test]
    fn a_missed_day_still_records_the_week_rather_than_skipping_it() {
        let mark = chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("time");
        // Configured for Wednesday; it is now Friday and nothing ran.
        assert!(snapshot_due(
            at(18, 9, 0),
            Weekday::Wed,
            mark,
            &[],
            "2026-W38"
        ));
    }

    /// Weeks are the key, not dates: two runs on different days of the same
    /// week are one observation, and the second must not append a row that
    /// resolves a different number of horizons to the same question.
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
