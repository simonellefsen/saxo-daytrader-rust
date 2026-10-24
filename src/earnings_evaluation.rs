//! What earnings reports did to prices, and to the book.
//!
//! The 2026-10-24 month review left one question open before an earnings
//! blackout is worth building: has holding through reports cost this book
//! money? Two measurements answer it, both from stored data only:
//!
//! - **Gaps.** For every US stock with stored report dates and price bars,
//!   the open-versus-previous-close gap on each report's reaction session
//!   against every other session. A stop rests at the broker, so the gap is
//!   the part of a report's move no stop can avoid.
//! - **The book.** Every report the book held a position through since the
//!   current book opened, with that session's gap applied to the quantity
//!   held.
//!
//! A report's timing decides its reaction session: the report day itself for
//! one before the open, the next session for one after the close. A report
//! whose timing is unknown is left out of both samples, and the sessions it
//! could have moved are kept out of the comparison sample too.
//!
//! Read-only. Nothing here gates, prompts, sizes or reaches the broker.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use chrono::Utc;
use serde_json::Value as JsonValue;

use crate::{
    db::value_f64,
    models::{
        EarningsCoveragePayload, EarningsEvaluationPayload, EarningsGapSamplePayload,
        EarningsHeldReportPayload,
    },
    state::{AppState, json_text},
};

/// An open more than this far from the previous close counts as a big gap.
pub(crate) const BIG_GAP_THRESHOLD_PCT: f64 = 5.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bar {
    pub date: String,
    pub open: Option<f64>,
    pub close: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Report {
    pub reported_date: String,
    pub report_time: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Trade {
    pub date: String,
    pub side: String,
    pub quantity: f64,
    pub currency: String,
}

/// The index of the session that first reacts to a report, oldest-first bars.
pub(crate) fn reaction_index(bars: &[Bar], report: &Report) -> Option<usize> {
    let date = report.reported_date.as_str();
    let index = match report.report_time.as_str() {
        "pre-market" => bars.partition_point(|bar| bar.date.as_str() < date),
        "post-market" => bars.partition_point(|bar| bar.date.as_str() <= date),
        _ => return None,
    };
    // A reaction a week after the report means the bars have a hole there.
    (index < bars.len() && days_between(date, &bars[index].date).is_some_and(|days| days <= 5))
        .then_some(index)
}

fn days_between(from: &str, to: &str) -> Option<i64> {
    let parse = |value: &str| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok();
    Some((parse(to)? - parse(from)?).num_days())
}

/// The open against the previous close, in percent.
pub(crate) fn gap_pct(bars: &[Bar], index: usize) -> Option<f64> {
    let previous = bars.get(index.checked_sub(1)?)?;
    let open = bars.get(index)?.open?;
    (previous.close > 0.0 && open > 0.0).then(|| (open / previous.close - 1.0) * 100.0)
}

fn close_change_pct(bars: &[Bar], index: usize) -> Option<f64> {
    let previous = bars.get(index.checked_sub(1)?)?;
    let close = bars.get(index)?.close;
    (previous.close > 0.0).then(|| (close / previous.close - 1.0) * 100.0)
}

/// Sessions a report could have moved but whose timing is unknown: the
/// report day and the next session.
fn ambiguous_indices(bars: &[Bar], report: &Report) -> Vec<usize> {
    let first = bars.partition_point(|bar| bar.date.as_str() < report.reported_date.as_str());
    [first, first + 1]
        .into_iter()
        .filter(|index| *index < bars.len())
        .collect()
}

/// Quantity held going into `date`: every trade on an earlier day.
pub(crate) fn quantity_before(trades: &[Trade], date: &str) -> f64 {
    trades
        .iter()
        .filter(|trade| trade.date.as_str() < date)
        .map(|trade| {
            if trade.side == "SELL" {
                -trade.quantity
            } else {
                trade.quantity
            }
        })
        .sum::<f64>()
        .max(0.0)
}

fn percentile(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let position = (fraction * (sorted.len() - 1) as f64).round() as usize;
    sorted.get(position).copied()
}

fn gap_sample(sample: &str, gaps: &[f64]) -> EarningsGapSamplePayload {
    let mut magnitudes = gaps.iter().map(|gap| gap.abs()).collect::<Vec<_>>();
    magnitudes.sort_by(f64::total_cmp);
    let big = magnitudes
        .iter()
        .filter(|gap| **gap > BIG_GAP_THRESHOLD_PCT)
        .count();
    EarningsGapSamplePayload {
        sample: sample.to_string(),
        sessions: magnitudes.len() as i64,
        median_abs_gap_pct: percentile(&magnitudes, 0.5),
        p90_abs_gap_pct: percentile(&magnitudes, 0.9),
        share_big_gaps: (!magnitudes.is_empty()).then(|| big as f64 / magnitudes.len() as f64),
    }
}

/// Everything measured for one stock.
#[derive(Default)]
pub(crate) struct SymbolMeasurement {
    pub reaction_gaps: Vec<f64>,
    pub other_gaps: Vec<f64>,
    pub measured_reports: usize,
    pub held: Vec<EarningsHeldReportPayload>,
}

pub(crate) fn measure_symbol(
    symbol: &str,
    bars: &[Bar],
    reports: &[Report],
    trades: &[Trade],
    book_since: &str,
) -> SymbolMeasurement {
    let mut measurement = SymbolMeasurement::default();
    let mut reaction = HashSet::new();
    let mut ambiguous = HashSet::new();
    for report in reports {
        if !matches!(report.report_time.as_str(), "pre-market" | "post-market") {
            ambiguous.extend(ambiguous_indices(bars, report));
            continue;
        }
        let Some(index) = reaction_index(bars, report) else {
            continue;
        };
        if !reaction.insert(index) {
            continue;
        }
        let Some(gap) = gap_pct(bars, index) else {
            continue;
        };
        measurement.measured_reports += 1;
        measurement.reaction_gaps.push(gap);
        let reaction_date = bars[index].date.clone();
        if reaction_date.as_str() < book_since {
            continue;
        }
        let quantity = quantity_before(trades, &reaction_date);
        if quantity <= 0.0 {
            continue;
        }
        let previous_close = bars[index - 1].close;
        let currency = trades
            .iter()
            .map(|trade| trade.currency.clone())
            .find(|currency| !currency.is_empty())
            .unwrap_or_default();
        measurement.held.push(EarningsHeldReportPayload {
            symbol: symbol.to_string(),
            reported_date: report.reported_date.clone(),
            report_time: report.report_time.clone(),
            reaction_date: reaction_date.clone(),
            gap_pct: Some(gap),
            close_change_pct: close_change_pct(bars, index),
            quantity,
            currency,
            gap_pnl_local: Some(quantity * previous_close * gap / 100.0),
            gap_pnl_dkk: None,
            sold_on_reaction_session: trades
                .iter()
                .any(|trade| trade.side == "SELL" && trade.date == reaction_date),
        });
    }
    for index in 1..bars.len() {
        if reaction.contains(&index) || ambiguous.contains(&index) {
            continue;
        }
        if let Some(gap) = gap_pct(bars, index) {
            measurement.other_gaps.push(gap);
        }
    }
    measurement
}

fn definitions() -> BTreeMap<String, String> {
    [
        (
            "reaction_session",
            "the report day for a report before the open, the next session for one after the close; a report with unknown timing is excluded, and so are the two sessions it could have moved",
        ),
        (
            "gap_pct",
            "the session open against the previous close: the part of a move a resting stop cannot avoid",
        ),
        (
            "held_reports",
            "reports whose reaction session fell inside a position the book held at the previous close, since the current book opened",
        ),
        (
            "gap_pnl_dkk",
            "quantity held times the gap, valued at today's FX rate rather than the rate on the day",
        ),
        (
            "blackout",
            "record-only: each queued US BUY whose next report reacts within window_sessions weekday sessions is noted as one a blackout would have stopped; the BUY still goes ahead",
        ),
        (
            "scope",
            "read-only measurement over stored report dates and daily bars; it gates nothing",
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

pub(crate) async fn earnings_evaluation_payload(
    state: &AppState,
) -> Result<EarningsEvaluationPayload> {
    let book_since = crate::entry_evaluation::EPOCHS
        .iter()
        .find(|epoch| epoch.key == "b_daily_markov")
        .map(|epoch| epoch.start_utc.get(..10).unwrap_or_default().to_string())
        .unwrap_or_default();
    let mut reports = HashMap::<String, Vec<Report>>::new();
    for row in state
        .select_json(
            "SELECT symbol, reported_date, report_time FROM earnings_report_dates
             ORDER BY symbol, reported_date",
        )
        .await?
    {
        reports
            .entry(json_text(&row, "symbol"))
            .or_default()
            .push(Report {
                reported_date: json_text(&row, "reported_date"),
                report_time: json_text(&row, "report_time"),
            });
    }
    let mut trades = HashMap::<String, Vec<Trade>>::new();
    for row in state
        .select_json(
            "SELECT symbol, created_at, side, quantity, currency FROM trade_ledger
             WHERE status = 'executed' ORDER BY created_at",
        )
        .await?
    {
        trades
            .entry(json_text(&row, "symbol"))
            .or_default()
            .push(Trade {
                date: json_text(&row, "created_at")
                    .get(..10)
                    .unwrap_or_default()
                    .to_string(),
                side: json_text(&row, "side").to_uppercase(),
                quantity: value_f64(&row, "quantity"),
                currency: json_text(&row, "currency"),
            });
    }

    let mut reaction_gaps = Vec::new();
    let mut other_gaps = Vec::new();
    let mut held = Vec::new();
    let mut measured = 0usize;
    for (symbol, symbol_reports) in &reports {
        let bars = state
            .select_json(&format!(
                "SELECT bar_date, open, close FROM daily_price_bars
                 WHERE symbol = '{}' ORDER BY bar_date",
                crate::db::sql_escape(symbol)
            ))
            .await?
            .iter()
            .filter_map(|row| {
                let close = value_f64(row, "close");
                (close > 0.0).then(|| Bar {
                    date: json_text(row, "bar_date"),
                    open: row
                        .get("open")
                        .and_then(JsonValue::as_f64)
                        .filter(|v| *v > 0.0),
                    close,
                })
            })
            .collect::<Vec<_>>();
        let measurement = measure_symbol(
            symbol,
            &bars,
            symbol_reports,
            trades.get(symbol).map(Vec::as_slice).unwrap_or_default(),
            &book_since,
        );
        measured += measurement.measured_reports;
        reaction_gaps.extend(measurement.reaction_gaps);
        other_gaps.extend(measurement.other_gaps);
        held.extend(measurement.held);
    }
    for row in &mut held {
        let rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, &row.currency).await;
        row.gap_pnl_dkk = row.gap_pnl_local.map(|pnl| pnl * rate);
    }
    held.sort_by(|left, right| right.reaction_date.cmp(&left.reaction_date));

    let coverage_json = crate::earnings_dates::coverage(state).await?;
    let stored = reports.values().map(Vec::len).sum::<usize>();
    let with_timing = reports
        .values()
        .flatten()
        .filter(|report| matches!(report.report_time.as_str(), "pre-market" | "post-market"))
        .count();
    let big = |gaps: &[f64]| {
        gaps.iter()
            .filter(|gap| gap.abs() > BIG_GAP_THRESHOLD_PCT)
            .count() as i64
    };
    let held_report_gap_pnl_dkk =
        (!held.is_empty()).then(|| held.iter().filter_map(|row| row.gap_pnl_dkk).sum::<f64>());
    Ok(EarningsEvaluationPayload {
        generated_at: Utc::now().to_rfc3339(),
        coverage: EarningsCoveragePayload {
            us_symbols: coverage_json["us_symbols"].as_i64().unwrap_or(0),
            with_report_dates: coverage_json["with_report_dates"].as_i64().unwrap_or(0),
            by_last_status: coverage_json["by_last_status"]
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .map(|(key, value)| (key.clone(), value.as_i64().unwrap_or(0)))
                        .collect()
                })
                .unwrap_or_default(),
            reports_stored: stored as i64,
            reports_with_timing: with_timing as i64,
            reports_measured: measured as i64,
        },
        big_gap_threshold_pct: BIG_GAP_THRESHOLD_PCT,
        big_gaps_after_reports: big(&reaction_gaps),
        big_gaps_otherwise: big(&other_gaps),
        gaps: vec![
            gap_sample("report_reaction", &reaction_gaps),
            gap_sample("other_sessions", &other_gaps),
        ],
        book_since,
        held_reports: held,
        held_report_gap_pnl_dkk,
        blackout: crate::earnings_blackout::blackout_payload(state).await?,
        definitions: definitions(),
        status: None,
        unavailable_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(rows: &[(&str, f64, f64)]) -> Vec<Bar> {
        rows.iter()
            .map(|(date, open, close)| Bar {
                date: date.to_string(),
                open: Some(*open),
                close: *close,
            })
            .collect()
    }

    fn report(date: &str, time: &str) -> Report {
        Report {
            reported_date: date.to_string(),
            report_time: time.to_string(),
        }
    }

    fn trade(date: &str, side: &str, quantity: f64) -> Trade {
        Trade {
            date: date.to_string(),
            side: side.to_string(),
            quantity,
            currency: "USD".to_string(),
        }
    }

    /// A report after the close moves the next session; one before the open
    /// moves its own day; one with unknown timing is not placed.
    #[test]
    fn the_reaction_session_follows_the_report_timing() {
        let bars = bars(&[
            ("2026-08-31", 100.0, 100.0),
            ("2026-09-01", 100.0, 101.0),
            ("2026-09-02", 87.0, 88.0),
        ]);
        assert_eq!(
            reaction_index(&bars, &report("2026-09-01", "post-market")),
            Some(2)
        );
        assert_eq!(
            reaction_index(&bars, &report("2026-09-01", "pre-market")),
            Some(1)
        );
        assert_eq!(reaction_index(&bars, &report("2026-09-01", "")), None);
        assert_eq!(
            reaction_index(&bars, &report("2026-09-02", "post-market")),
            None
        );
        let gap = gap_pct(&bars, 2).unwrap();
        assert!((gap - (87.0 / 101.0 - 1.0) * 100.0).abs() < 1e-9);
    }

    #[test]
    fn quantity_held_counts_only_earlier_days() {
        let trades = vec![
            trade("2026-09-01", "BUY", 3.0),
            trade("2026-09-10", "SELL", 3.0),
            trade("2026-09-20", "BUY", 2.0),
        ];
        assert_eq!(
            quantity_before(&trades, "2026-09-01"),
            0.0,
            "bought after the open"
        );
        assert_eq!(quantity_before(&trades, "2026-09-02"), 3.0);
        assert_eq!(
            quantity_before(&trades, "2026-09-10"),
            3.0,
            "held into the sale day"
        );
        assert_eq!(quantity_before(&trades, "2026-09-11"), 0.0);
        assert_eq!(quantity_before(&trades, "2026-09-21"), 2.0);
    }

    /// A held report is costed at the gap, the stop sale that day is flagged,
    /// and a report with unknown timing keeps its two sessions out of the
    /// comparison sample.
    #[test]
    fn a_held_report_is_costed_at_its_gap_and_ambiguous_sessions_stay_out() {
        let bars = bars(&[
            ("2026-08-28", 100.0, 100.0),
            ("2026-08-31", 100.0, 100.0),
            ("2026-09-01", 100.0, 100.0),
            ("2026-09-02", 80.0, 82.0),
            ("2026-09-03", 82.0, 83.0),
            ("2026-09-04", 90.0, 90.0),
            ("2026-09-07", 90.0, 91.0),
        ]);
        let reports = vec![
            report("2026-09-01", "post-market"),
            report("2026-09-04", ""),
            report("2026-06-01", "post-market"),
        ];
        let trades = vec![
            trade("2026-08-28", "BUY", 4.0),
            trade("2026-09-02", "SELL", 4.0),
        ];
        let measured = measure_symbol("MDB:xnas", &bars, &reports, &trades, "2026-07-16");
        assert_eq!(measured.measured_reports, 1, "June has no bars to react on");
        assert_eq!(measured.held.len(), 1);
        let held = &measured.held[0];
        assert_eq!(held.reaction_date, "2026-09-02");
        assert_eq!(held.quantity, 4.0);
        assert!(held.sold_on_reaction_session);
        assert!((held.gap_pnl_local.unwrap() - 4.0 * 100.0 * -0.2).abs() < 1e-9);
        // Sessions 1..=6 minus the reaction (index 3) and the ambiguous
        // report day and next session (indices 5 and 6).
        assert_eq!(measured.other_gaps.len(), 3);
    }

    #[test]
    fn gap_samples_summarise_magnitudes() {
        let sample = gap_sample("report_reaction", &[-12.0, 1.0, 3.0, 6.0]);
        assert_eq!(sample.sessions, 4);
        assert_eq!(sample.share_big_gaps, Some(0.5));
        assert_eq!(sample.p90_abs_gap_pct, Some(12.0));
        let empty = gap_sample("other_sessions", &[]);
        assert_eq!(empty.sessions, 0);
        assert_eq!(empty.median_abs_gap_pct, None);
    }
}
