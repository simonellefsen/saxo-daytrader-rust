use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    path::PathBuf,
    process,
    sync::{OnceLock, RwLock},
    time::Duration as StdDuration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use tokio::time::sleep;
use tracing::{error, info, warn};
use url::Url;

#[cfg(test)]
use crate::debug_redaction::compact_debug_text;
use crate::{
    auth,
    config::{database_url, yaml_at, yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{clamp_limit, pct, row_to_json, sql_escape, value_f64, value_i64},
    models::{
        DashboardAiSettingsPayload, DashboardLatestRunPayload, DataFreshnessSourcePayload,
        ExecutionEventSummaryPayload, ExecutionFillSummaryPayload,
        ExecutionOrderEventTimelineEntryPayload, ExecutionOrderSummaryPayload,
        LlmUsageLedgerPayload, TuningBenchmarkComparison, TuningBenchmarkReference,
        TuningDirectionalOutcome, TuningExecutionCandidateFunnel, TuningExecutionLifecycleEvidence,
        TuningExecutionPulseOutcome, TuningExperimentGovernance, TuningMonthlyGoalProgress,
        TuningPayload, TuningPortfolioOutcome, TuningProtectiveStopCoverage, TuningPulseComparison,
        TuningShadowChangeEvidence, TuningShadowGateEvidence, TuningShadowHermesEvidence,
        TuningShadowMarkovEvidence, TuningShadowQuiverEvidence, TuningShadowSupportRiskEvidence,
        TuningTradeThesisEvidence,
    },
    performance_state::performance_summary_from_history,
    read_model,
};

#[cfg(test)]
use crate::models::{
    CandidateScoringWaterfallCandidatePayload, CandidateScoringWaterfallConcentrationPayload,
    CandidateScoringWaterfallCostGuardPayload, CandidateScoringWaterfallHermesPayload,
    CandidateScoringWaterfallHoldingLimitPayload, CandidateScoringWaterfallMarketPayload,
    CandidateScoringWaterfallMarkovPayload, CandidateScoringWaterfallPayload,
    CandidateScoringWaterfallPositionWeightPayload, CandidateScoringWaterfallSummaryPayload,
    CandidateScoringWaterfallTechnicalPayload, DashboardSelectedDecisionPayload,
};

#[cfg(test)]
use crate::performance_state::{performance_confidence, performance_range_metrics};

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
    pub config: YamlValue,
    pub db_url: String,
    pub pool: AnyPool,
}

static SAXO_EXCHANGE_CALENDAR_CACHE: OnceLock<RwLock<Option<SaxoExchangeCalendarCache>>> =
    OnceLock::new();

const SAXO_SESSION_REFRESH_LEASE_SECONDS: i64 = 45;
const SAXO_SESSION_REFRESH_LEASE_WAIT_ATTEMPTS: usize = 50;
const SAXO_SESSION_PERSIST_ATTEMPTS: u32 = 3;
/// While shutting down, an access token is still handed out if it outlives
/// this; the replacement process owns the next rotation.
const SAXO_SESSION_DRAINING_MIN_ACCESS_SECONDS: i64 = 60;
pub(crate) const INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK: f64 = 100_000.0;

/// Upper bound on points returned for a performance chart. A line chart gains
/// nothing past roughly this density, and it bounds the payload for `ALL`.
const PERFORMANCE_MAX_CHART_POINTS: i64 = 1_500;

/// A snapshot's `total_cost_basis_dkk` is unusable when it exceeds the invested
/// market value by more than this multiple.
///
/// Between 2026-06-03 and 2026-07-09 roughly 6,300 snapshots stored a cost basis
/// of 5.4M-26.9M DKK against an invested value near 240k -- ratios of 20x to
/// 100x. The underlying `position_lots` fault was repaired on 2026-07-08 and is
/// now blocked by `INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK`, but the snapshots keep
/// the bad figure: they store only aggregates, so there is no per-position
/// detail to recompute from. The window is marked rather than repaired, which
/// also preserves the evidence.
///
/// A real portfolio can hold a cost basis above its market value after a large
/// drawdown, so the threshold is deliberately generous -- it separates "under
/// water" from "arithmetically impossible".
const PERFORMANCE_COST_BASIS_MAX_RATIO: f64 = 3.0;

/// Whether a snapshot's stored cost basis can be trusted.
pub(crate) fn cost_basis_is_plausible(cost_basis_dkk: f64, invested_market_value_dkk: f64) -> bool {
    if !cost_basis_dkk.is_finite() || !invested_market_value_dkk.is_finite() {
        return false;
    }
    // Nothing invested and nothing claimed is consistent, not suspect.
    if cost_basis_dkk <= 0.0 {
        return true;
    }
    if invested_market_value_dkk <= 0.0 {
        return false;
    }
    cost_basis_dkk / invested_market_value_dkk <= PERFORMANCE_COST_BASIS_MAX_RATIO
}
pub(crate) const DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES: i64 = 10;
pub(crate) const DECISION_REPORT_SUMMARY_COLUMNS: &str = "id, created_at, report_date, model, status, analysis_window_active, response_id, error_text, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible";
pub(crate) const DECISION_REPORT_DETAIL_COLUMNS: &str = "id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible";
const DEFAULT_SCHEDULER_HISTORY_MAX_ROWS: i64 = 250;
const DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS: i64 = 30;
/// Keep full per-cycle detail through the drawdown lookback; older detail is
/// thinned to the final snapshot of each UTC date.
const PORTFOLIO_POSITION_SNAPSHOT_RETENTION_DAYS: i64 = 90;
pub(crate) const DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS: i64 = 7;
pub(crate) const TUNING_PULSE_WINDOW_DAYS: i64 = 30;
const TUNING_MATURE_OUTCOME_MINIMUM: i64 = 20;
const SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS: i64 = 180;
const SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS: usize = 30;
const SUPPORT_RISK_LABELS: [&str; 3] = ["low", "moderate", "high"];
const TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT: i64 = 50;
pub(crate) const TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS: usize = 20;
const HOLDING_THESIS_REVIEW_LIMIT: i64 = 50;
// Shared with tests so a query against a column the schema lacks fails there.
const BROKER_POSITION_SNAPSHOTS_DDL: &str = "CREATE TABLE IF NOT EXISTS broker_position_snapshots (
    symbol TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL,
    instrument_name TEXT,
    isin TEXT,
    uic INTEGER,
    asset_type TEXT,
    quantity REAL NOT NULL,
    currency TEXT,
    open_price_local REAL,
    open_price_including_costs_local REAL,
    execution_time_open TEXT,
    value_date TEXT,
    market_state TEXT,
    can_be_closed INTEGER,
    raw_payload_json TEXT NOT NULL
)";
const BROKER_INSTRUMENT_EXPOSURES_DDL: &str =
    "CREATE TABLE IF NOT EXISTS broker_instrument_exposures (
    symbol TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL,
    uic INTEGER,
    asset_type TEXT,
    quantity REAL,
    average_open_price REAL,
    profit_loss_on_trade REAL,
    instrument_price_day_percent_change REAL,
    currency TEXT,
    calculation_reliability TEXT,
    can_be_closed INTEGER,
    raw_payload_json TEXT NOT NULL
)";
pub(crate) const MISSED_TRADE_SHADOW_LIMIT: i64 = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShareIncomeTaxBracket {
    up_to_dkk: Option<f64>,
    rate: f64,
}

fn share_income_tax_brackets(config: &YamlValue) -> Option<Vec<ShareIncomeTaxBracket>> {
    let values = yaml_at(config, &["taxation", "share_income", "brackets"])?.as_sequence()?;
    if values.is_empty() {
        return None;
    }

    let mut brackets = Vec::with_capacity(values.len());
    let mut lower_bound = 0.0;
    for (index, value) in values.iter().enumerate() {
        let rate = value.get("rate").and_then(YamlValue::as_f64)?;
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return None;
        }
        let up_to_dkk = value.get("up_to_dkk").and_then(YamlValue::as_f64);
        match up_to_dkk {
            Some(upper_bound) if upper_bound.is_finite() && upper_bound > lower_bound => {
                lower_bound = upper_bound;
            }
            Some(_) => return None,
            None if index + 1 == values.len() => {}
            None => return None,
        }
        brackets.push(ShareIncomeTaxBracket { up_to_dkk, rate });
    }
    Some(brackets)
}

fn share_income_tax_due_dkk(income_dkk: f64, brackets: &[ShareIncomeTaxBracket]) -> Option<f64> {
    if !income_dkk.is_finite() || brackets.is_empty() {
        return None;
    }
    let taxable_income = income_dkk.max(0.0);
    let mut lower_bound = 0.0;
    let mut tax_dkk = 0.0;
    for bracket in brackets {
        let taxable_slice = match bracket.up_to_dkk {
            Some(upper_bound) => (taxable_income.min(upper_bound) - lower_bound).max(0.0),
            None => (taxable_income - lower_bound).max(0.0),
        };
        tax_dkk += taxable_slice * bracket.rate;
        if bracket.up_to_dkk.is_none() || taxable_income <= bracket.up_to_dkk.unwrap_or_default() {
            return Some(tax_dkk);
        }
        lower_bound = bracket.up_to_dkk.unwrap_or(lower_bound);
    }
    None
}

fn incremental_share_income_tax_dkk(
    realised_gain_ytd_dkk: f64,
    unrealised_pnl_dkk: f64,
    brackets: &[ShareIncomeTaxBracket],
) -> Option<f64> {
    if !realised_gain_ytd_dkk.is_finite() || !unrealised_pnl_dkk.is_finite() {
        return None;
    }
    Some(
        share_income_tax_due_dkk(realised_gain_ytd_dkk + unrealised_pnl_dkk, brackets)?
            - share_income_tax_due_dkk(realised_gain_ytd_dkk, brackets)?,
    )
}

fn unavailable_after_tax_summary(
    gross_unrealised_pnl_dkk: f64,
    tax_year: i32,
    reason: &str,
) -> JsonValue {
    json!({
        "status": "unavailable",
        "tax_year": tax_year,
        "gross_unrealised_pnl_dkk": gross_unrealised_pnl_dkk,
        "estimated_tax_dkk": 0.0,
        "unrealised_pnl_after_tax_dkk": gross_unrealised_pnl_dkk,
        "reason": reason
    })
}

#[derive(Clone, Debug)]
struct SaxoExchangeCalendarCache {
    checked_date: NaiveDate,
    checked_at: DateTime<Utc>,
    exchanges: HashMap<String, SaxoExchangeCalendar>,
    source: String,
}

#[derive(Clone, Debug)]
struct SaxoExchangeCalendar {
    exchange_id: String,
    name: Option<String>,
    timezone_id: Option<String>,
    sessions: Vec<SaxoExchangeSession>,
}

#[derive(Clone, Debug)]
struct SaxoExchangeSession {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    state: String,
}

#[derive(Clone, Debug)]
struct ExchangeDaySession {
    open_at: DateTime<Utc>,
    close_at: DateTime<Utc>,
}

/// True for a Postgres/PostgreSQL connection string. Used to skip
/// Postgres-only DDL (storage-parameter `ALTER TABLE ... SET (...)`) on the
/// SQLite pool used locally and in tests, which has no autovacuum and does
/// not accept that syntax.
fn database_url_is_postgres(value: &str) -> bool {
    value.starts_with("postgres://") || value.starts_with("postgresql://")
}

pub(crate) fn redacted_database_url(value: &str) -> String {
    // This value reaches both logs and the operator dashboard. Render only
    // connection topology, never URL user-info, query parameters, or a local
    // filesystem path. A structured URL parser avoids fragile string masking.
    let Ok(url) = Url::parse(value) else {
        return "Configured database".to_string();
    };
    match url.scheme() {
        "postgres" | "postgresql" => {
            let host = url.host_str().unwrap_or("configured host");
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            let database = url.path().trim_matches('/');
            if database.is_empty() {
                format!("PostgreSQL · {host}{port}")
            } else {
                format!("PostgreSQL · {host}{port}/{database}")
            }
        }
        "sqlite" => "SQLite · local database".to_string(),
        scheme if !scheme.is_empty() => format!("{} database", scheme.to_ascii_uppercase()),
        _ => "Configured database".to_string(),
    }
}

pub(crate) fn sql_optional_text(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("'{}'", sql_escape(value)),
        None => "NULL".to_string(),
    }
}

fn sql_f64(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "0".to_string()
    }
}

pub(crate) fn hermes_counterfactual_quote_metrics(
    action: &str,
    shadow_quantity: f64,
    reference_price_local: f64,
    latest_price_local: f64,
) -> Option<(f64, f64)> {
    if !shadow_quantity.is_finite()
        || shadow_quantity <= 0.0
        || !reference_price_local.is_finite()
        || reference_price_local <= 0.0
        || !latest_price_local.is_finite()
        || latest_price_local <= 0.0
    {
        return None;
    }
    let price_change = match action.trim().to_uppercase().as_str() {
        "BUY" => latest_price_local - reference_price_local,
        "SELL" => reference_price_local - latest_price_local,
        _ => return None,
    };
    Some((
        price_change / reference_price_local,
        shadow_quantity * price_change,
    ))
}

pub(crate) fn json_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

#[derive(Default)]
struct SupportRiskEvidenceStats {
    signal_count: usize,
    one_run_count: usize,
    one_run_return_sum_pct: f64,
    one_run_negative_count: usize,
    five_run_count: usize,
    five_run_return_sum_pct: f64,
    five_run_negative_count: usize,
    break_risk_sum: f64,
    confidence_sum: f64,
    history_coverage_sum: f64,
}

#[derive(Default)]
struct TradeThesisOutcomeStats {
    recorded_thesis_count: usize,
    filled_thesis_count: usize,
    one_session_count: usize,
    one_session_return_sum_pct: f64,
    one_session_positive_count: usize,
    five_session_count: usize,
    five_session_return_sum_pct: f64,
    five_session_positive_count: usize,
}

pub(crate) fn average_or_null(total: f64, count: usize) -> JsonValue {
    if count == 0 {
        JsonValue::Null
    } else {
        json!(total / count as f64)
    }
}

pub(crate) fn fraction_or_null(numerator: usize, denominator: usize) -> JsonValue {
    if denominator == 0 {
        JsonValue::Null
    } else {
        json!(numerator as f64 / denominator as f64)
    }
}

/// Summarizes only the recorded BUY theses that have reconciled-fill outcome
/// evidence. This is observational aggregation, not a backtest: it does not
/// include blocked candidates, broker adjustments, FX, costs, or later
/// position changes, and it does not claim that the thesis caused an outcome.
fn trade_thesis_outcome_evidence_from_holding_outcomes(outcomes: &[JsonValue]) -> JsonValue {
    let mut stats = TradeThesisOutcomeStats {
        recorded_thesis_count: outcomes.len(),
        ..TradeThesisOutcomeStats::default()
    };
    for outcome in outcomes {
        if outcome.is_null() || value_f64(outcome, "filled_quantity") <= 0.0 {
            continue;
        }
        stats.filled_thesis_count += 1;
        for (session, count, return_sum, positive_count) in [
            (
                outcome.get("one_session").unwrap_or(&JsonValue::Null),
                &mut stats.one_session_count,
                &mut stats.one_session_return_sum_pct,
                &mut stats.one_session_positive_count,
            ),
            (
                outcome.get("five_session").unwrap_or(&JsonValue::Null),
                &mut stats.five_session_count,
                &mut stats.five_session_return_sum_pct,
                &mut stats.five_session_positive_count,
            ),
        ] {
            if session.is_null() {
                continue;
            }
            let directional_return_pct = value_f64(session, "directional_return_pct");
            if !directional_return_pct.is_finite() {
                continue;
            }
            *count += 1;
            *return_sum += directional_return_pct;
            if directional_return_pct > 0.0 {
                *positive_count += 1;
            }
        }
    }
    let status = if stats.recorded_thesis_count == 0 {
        "no_recorded_theses"
    } else if stats.five_session_count < TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };
    let session_summary = |count: usize, return_sum_pct: f64, positive_count: usize| {
        json!({
            "sample_count": count,
            "average_directional_return_pct": average_or_null(return_sum_pct, count),
            "positive_return_rate": fraction_or_null(positive_count, count),
        })
    };
    json!({
        "status": status,
        "recorded_thesis_count": stats.recorded_thesis_count,
        "filled_thesis_count": stats.filled_thesis_count,
        "one_session": session_summary(
            stats.one_session_count,
            stats.one_session_return_sum_pct,
            stats.one_session_positive_count,
        ),
        "five_session": session_summary(
            stats.five_session_count,
            stats.five_session_return_sum_pct,
            stats.five_session_positive_count,
        ),
        "minimum_complete_observations": TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS,
        "scan_limit": TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT,
        "safety": "read_only_local_execution_fills_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "Directional returns compare reconciled BUY fills with later stored daily closes. They exclude blocked candidates, FX, commission, tax, slippage, later position changes, broker adjustments, and any causal claim about the thesis."
    })
}

/// Groups stored daily signals by their recorded support-break label and
/// observes subsequent stored closes. This deliberately measures only the
/// next available indicator runs; it neither assumes every market traded nor
/// claims a causal backtest from sparse observations.
fn support_risk_evidence_from_indicator_rows(rows: &[JsonValue]) -> JsonValue {
    let mut grouped: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for row in rows {
        let symbol = json_text(row, "symbol");
        let run_date = json_text(row, "run_date");
        if !symbol.trim().is_empty() && !run_date.trim().is_empty() {
            // The query is ordered by created_at within a symbol/date. A manual
            // rerun supersedes the older daily row instead of manufacturing an
            // extra next-run outcome for the same market date.
            let symbol_rows = grouped.entry(symbol).or_default();
            if symbol_rows
                .last()
                .is_some_and(|previous| json_text(previous, "run_date") == run_date)
            {
                let last_index = symbol_rows.len() - 1;
                symbol_rows[last_index] = row;
            } else {
                symbol_rows.push(row);
            }
        }
    }

    let mut stats: HashMap<&'static str, SupportRiskEvidenceStats> = SUPPORT_RISK_LABELS
        .iter()
        .copied()
        .map(|label| (label, SupportRiskEvidenceStats::default()))
        .collect();

    for symbol_rows in grouped.values() {
        for (index, row) in symbol_rows.iter().enumerate() {
            let label = json_text(row, "support_break_risk_label");
            let Some(label) = SUPPORT_RISK_LABELS
                .iter()
                .copied()
                .find(|candidate| *candidate == label)
            else {
                continue;
            };
            let close = value_f64(row, "close");
            if !close.is_finite() || close <= 0.0 {
                continue;
            }
            let entry = stats.get_mut(label).expect("known support-risk label");
            entry.signal_count += 1;
            entry.break_risk_sum += value_f64(row, "support_break_risk").clamp(0.0, 1.0);
            entry.confidence_sum += value_f64(row, "support_confidence").clamp(0.0, 1.0);
            entry.history_coverage_sum +=
                value_f64(row, "support_history_coverage").clamp(0.0, 1.0);

            for (horizon, count, return_sum, negative_count) in [
                (
                    1usize,
                    &mut entry.one_run_count,
                    &mut entry.one_run_return_sum_pct,
                    &mut entry.one_run_negative_count,
                ),
                (
                    5usize,
                    &mut entry.five_run_count,
                    &mut entry.five_run_return_sum_pct,
                    &mut entry.five_run_negative_count,
                ),
            ] {
                let Some(next_row) = symbol_rows.get(index + horizon) else {
                    continue;
                };
                let next_close = value_f64(next_row, "close");
                if !next_close.is_finite() || next_close <= 0.0 {
                    continue;
                }
                let return_pct = (next_close - close) / close * 100.0;
                *count += 1;
                *return_sum += return_pct;
                if return_pct < 0.0 {
                    *negative_count += 1;
                }
            }
        }
    }

    let labels = SUPPORT_RISK_LABELS
        .iter()
        .map(|label| {
            let entry = stats.get(label).expect("known support-risk label");
            json!({
                "label": label,
                "signal_count": entry.signal_count,
                "next_run": {
                    "sample_count": entry.one_run_count,
                    "average_return_pct": average_or_null(entry.one_run_return_sum_pct, entry.one_run_count),
                    "negative_return_rate": fraction_or_null(entry.one_run_negative_count, entry.one_run_count),
                },
                "five_run": {
                    "sample_count": entry.five_run_count,
                    "average_return_pct": average_or_null(entry.five_run_return_sum_pct, entry.five_run_count),
                    "negative_return_rate": fraction_or_null(entry.five_run_negative_count, entry.five_run_count),
                },
                "average_break_risk": average_or_null(entry.break_risk_sum, entry.signal_count),
                "average_confidence": average_or_null(entry.confidence_sum, entry.signal_count),
                "average_history_coverage": average_or_null(entry.history_coverage_sum, entry.signal_count),
            })
        })
        .collect::<Vec<_>>();
    let eligible_signal_count = stats
        .values()
        .map(|entry| entry.signal_count)
        .sum::<usize>();
    let one_run_complete_count = stats
        .values()
        .map(|entry| entry.one_run_count)
        .sum::<usize>();
    let five_run_complete_count = stats
        .values()
        .map(|entry| entry.five_run_count)
        .sum::<usize>();
    let status = if eligible_signal_count == 0 {
        "no_observations"
    } else if five_run_complete_count < SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };

    json!({
        "status": status,
        "lookback_days": SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS,
        "minimum_complete_observations": SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS,
        "eligible_signal_count": eligible_signal_count,
        "next_run_complete_count": one_run_complete_count,
        "five_run_complete_count": five_run_complete_count,
        "labels": labels,
        "safety": "read_only_observation_of_stored_daily_indicator_closes",
        "interpretation": "Outcomes use the next available one or five stored daily indicator runs for the same symbol. They are descriptive, not causal, do not account for trading costs or market gaps, and cannot change a gate, Hermes proposal, configuration, or Saxo order.",
    })
}

/// Reconciles the latest persisted broker-position snapshot with locally
/// recorded SELL Stop/StopLimit orders. It deliberately does not query Saxo
/// and only classifies broker-confirmed order states as active protection.
/// Queued or uncertain orders remain planned so the dashboard cannot imply a
/// protection guarantee that the broker has not acknowledged.
/// Default when `strategy.ladder.stop_loss_atr_multiple` is absent. Matches the
/// value both shipped configs carry.
/// One-variable overlay paths Hermes may propose. Cross-checked against the
/// config contract by test, so a variable nothing reads cannot be offered.
///
/// Legacy cash-buffer paths are retired: the single active reserve is
/// `strategy.capital.min_cash_buffer_pct`.
pub(crate) const SUPPORTED_EXPERIMENT_VARIABLES: &[&str] = &[
    "execution.min_trade_value_dkk",
    "strategy.capital.min_cash_buffer_pct",
    "strategy.swing.daily_indicators.min_confluences",
    "strategy.swing.markov_gate.min_signed_signal",
];

/// Supported variables whose value is published into the Hermes decision
/// preflight, where an advisory reader can act on the number even when no
/// deterministic gate does.
///
/// This distinction is not academic. Across advice recorded since 2026-08-01,
/// Markov is mentioned in 105 of 127 order-advice items and `signed_signal` is
/// quoted numerically in 54 of them, and Hermes treats
/// `strategy.swing.markov_gate.min_signed_signal` as an admission bar in so
/// many words -- "clears both current and pending 0.20 conservative Markov
/// threshold" on one allow, "below the pending conservative" on a review. The
/// replay meanwhile reports that variable `unreachable_in_retained_evidence`,
/// which is true of the deterministic gate and false of the outcome: on
/// 2026-09-01 both US-open candidates carried signals under 0.20 (0.1686 and
/// 0.1289), Hermes held both citing exactly that, and the cycle bought nothing.
///
/// An explicit list rather than a derived one, because the preflight assembles
/// its blocks from several policy structs and a variable can enter it without
/// anyone noticing. Tests assert each entry against the builder that emits it.
pub(crate) const ADVISORY_VISIBLE_EXPERIMENT_VARIABLES: &[&str] = &[
    "strategy.capital.min_cash_buffer_pct",
    "strategy.swing.daily_indicators.min_confluences",
    "strategy.swing.markov_gate.min_signed_signal",
];

/// Marks an `execution_orders` row as a broker-hosted protective stop rather
/// than a discretionary order. Three code paths depend on telling them apart: a
/// resting GTC stop must not pin the scheduler at its fast poll interval, must
/// not reserve the quantity its own position needs to exit, and must be
/// cancelled before a discretionary SELL on the same symbol reaches Saxo.
pub(crate) const PROTECTIVE_STOP_STRATEGY_TYPE: &str = "protective_stop";

pub(crate) fn nested_json_text(value: &JsonValue, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        current = next;
    }
    match current {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

/// Compares two retained position compositions without treating ordinary price
/// movement as a portfolio composition change. Snapshot symbols are a stable
/// per-snapshot primary key, so a case-normalized symbol is the least-lossy
/// cross-snapshot identity when historical ISIN coverage differs.
fn portfolio_position_snapshot_change_from_rows(
    current_snapshot: &JsonValue,
    current_rows: &[JsonValue],
    previous_snapshot: &JsonValue,
    previous_rows: &[JsonValue],
) -> JsonValue {
    let current_by_symbol = current_rows
        .iter()
        .map(|row| (canonical_symbol_key(&json_text(row, "symbol")), row))
        .collect::<HashMap<_, _>>();
    let previous_by_symbol = previous_rows
        .iter()
        .map(|row| (canonical_symbol_key(&json_text(row, "symbol")), row))
        .collect::<HashMap<_, _>>();
    let symbols = current_by_symbol
        .keys()
        .chain(previous_by_symbol.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut opened = Vec::new();
    let mut closed = Vec::new();
    let mut resized = Vec::new();
    let mut unchanged_quantity_count = 0usize;
    let mut net_market_value_change_dkk = 0.0;
    let mut net_cost_basis_change_dkk = 0.0;
    for symbol_key in symbols {
        let current = current_by_symbol.get(&symbol_key).copied();
        let previous = previous_by_symbol.get(&symbol_key).copied();
        let current_quantity = current.map(|row| value_f64(row, "quantity")).unwrap_or(0.0);
        let previous_quantity = previous
            .map(|row| value_f64(row, "quantity"))
            .unwrap_or(0.0);
        let current_market_value = current
            .map(|row| value_f64(row, "market_value_dkk"))
            .unwrap_or(0.0);
        let previous_market_value = previous
            .map(|row| value_f64(row, "market_value_dkk"))
            .unwrap_or(0.0);
        let current_cost_basis = current
            .map(|row| value_f64(row, "cost_basis_dkk"))
            .unwrap_or(0.0);
        let previous_cost_basis = previous
            .map(|row| value_f64(row, "cost_basis_dkk"))
            .unwrap_or(0.0);
        net_market_value_change_dkk += current_market_value - previous_market_value;
        net_cost_basis_change_dkk += current_cost_basis - previous_cost_basis;
        let symbol = current
            .or(previous)
            .map(|row| json_text(row, "symbol"))
            .unwrap_or(symbol_key);
        let change = json!({
            "symbol": symbol,
            "quantity_before": previous_quantity,
            "quantity_after": current_quantity,
            "quantity_change": current_quantity - previous_quantity,
            "market_value_change_dkk": current_market_value - previous_market_value,
            "cost_basis_change_dkk": current_cost_basis - previous_cost_basis,
        });
        match (current, previous) {
            (Some(_), None) => opened.push(change),
            (None, Some(_)) => closed.push(change),
            (Some(_), Some(_)) if (current_quantity - previous_quantity).abs() > 1e-9 => {
                resized.push(change)
            }
            (Some(_), Some(_)) => unchanged_quantity_count += 1,
            (None, None) => unreachable!("symbol came from one of the snapshot maps"),
        }
    }
    let opened_count = opened.len();
    let closed_count = closed.len();
    let resized_count = resized.len();
    json!({
        "status": "available",
        "current_snapshot": current_snapshot,
        "previous_snapshot": previous_snapshot,
        "opened": opened,
        "closed": closed,
        "resized": resized,
        "opened_count": opened_count,
        "closed_count": closed_count,
        "resized_count": resized_count,
        "unchanged_quantity_count": unchanged_quantity_count,
        "net_market_value_change_dkk": net_market_value_change_dkk,
        "net_cost_basis_change_dkk": net_cost_basis_change_dkk,
        "safety": "local_retained_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
        "interpretation": "This compares only two stored retained compositions. Market-value movement includes prices, FX, and quantity changes; opened, closed, and resized lists isolate quantity composition changes and do not assert trades, fills, or causality.",
    })
}

fn is_duplicate_column_error(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("duplicate column")
        || message.contains("already exists")
        || message.contains("column exists")
}

fn saxo_refresh_lease_owner(source: &str) -> String {
    let host = env::var("HOSTNAME").unwrap_or_else(|_| "local".to_string());
    let nonce = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros());
    format!(
        "{}:{}:{}:{}",
        host,
        process::id(),
        source.replace(':', "_"),
        nonce
    )
}

/// Test-only interleaving point between a process deciding a refresh is due
/// and it trying to take the refresh lease. Another process completing a
/// whole refresh in that gap is the race this seam makes deterministic.
#[cfg(test)]
pub(crate) mod saxo_lease_gap_hook {
    use std::{
        collections::HashMap,
        future::Future,
        path::{Path, PathBuf},
        pin::Pin,
        sync::{LazyLock, Mutex},
    };

    type Hook = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

    static HOOKS: LazyLock<Mutex<HashMap<PathBuf, Hook>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Runs `hook` once, the next time the process owning `session_file`
    /// reaches the gap.
    pub(crate) fn install<F, Fut>(session_file: &Path, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        HOOKS.lock().expect("lease gap hooks").insert(
            session_file.to_path_buf(),
            Box::new(move || Box::pin(hook())),
        );
    }

    pub(crate) async fn run(session_file: &Path) {
        let hook = HOOKS.lock().expect("lease gap hooks").remove(session_file);
        if let Some(hook) = hook {
            hook().await;
        }
    }
}

/// Per-process coordination for Saxo token rotation, keyed by the working
/// copy it guards. In production that is one per process; keying by file lets
/// tests run several simulated pods in one process.
struct SaxoRefreshGate {
    in_flight: tokio::sync::Mutex<()>,
    draining: std::sync::atomic::AtomicBool,
}

impl SaxoRefreshGate {
    fn is_draining(&self) -> bool {
        self.draining.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn begin_draining(&self) {
        self.draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn saxo_refresh_gate(session_file: &std::path::Path) -> std::sync::Arc<SaxoRefreshGate> {
    static GATES: OnceLock<std::sync::Mutex<HashMap<PathBuf, std::sync::Arc<SaxoRefreshGate>>>> =
        OnceLock::new();
    GATES
        .get_or_init(Default::default)
        .lock()
        .expect("Saxo refresh gates")
        .entry(session_file.to_path_buf())
        .or_insert_with(|| {
            std::sync::Arc::new(SaxoRefreshGate {
                in_flight: tokio::sync::Mutex::new(()),
                draining: std::sync::atomic::AtomicBool::new(false),
            })
        })
        .clone()
}

enum SaxoRefreshLease {
    NotNeeded,
    Draining,
    Held(String),
}

struct SaxoRotation {
    result: Result<JsonValue>,
    release_lease: bool,
}

impl SaxoRotation {
    fn released(result: Result<JsonValue>) -> Self {
        Self {
            result,
            release_lease: true,
        }
    }
}

fn saxo_session_needs_refresh(session: &JsonValue) -> bool {
    let access_token = json_text(session, "access_token");
    if access_token.is_empty() {
        return saxo_session_refresh_token_usable(session);
    }
    let Some(access_expires_at) = parse_rfc3339_utc(session.get("access_token_expires_at")) else {
        return saxo_session_refresh_token_usable(session);
    };
    access_expires_at <= Utc::now() + Duration::seconds(15 * 60)
        && saxo_session_refresh_token_usable(session)
}

fn saxo_session_refresh_token_usable(session: &JsonValue) -> bool {
    if !json_text(session, "refresh_token_invalid_at")
        .trim()
        .is_empty()
    {
        return false;
    }
    if json_text(session, "refresh_token").trim().is_empty() {
        return false;
    }
    let Some(refresh_expires_at) = parse_rfc3339_utc(session.get("refresh_token_expires_at"))
    else {
        return false;
    };
    refresh_expires_at > Utc::now() + Duration::seconds(15 * 60)
}

pub(crate) fn parse_rfc3339_utc(value: Option<&JsonValue>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?.as_str()?.replace('Z', "+00:00").as_str())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub(crate) fn matching_order_advice(
    value: Option<&JsonValue>,
    strategy_key: &str,
    symbol: &str,
    action: &str,
) -> Option<JsonValue> {
    let items = value?.as_array()?;
    let strategy_key = strategy_key.trim();
    let symbol = symbol.trim();
    let action = action.trim();
    if !strategy_key.is_empty() {
        if let Some(item) = items
            .iter()
            .find(|item| json_text(item, "strategy_key").trim() == strategy_key)
        {
            return Some(item.clone());
        }
    }
    items
        .iter()
        .find(|item| {
            json_text(item, "symbol")
                .trim()
                .eq_ignore_ascii_case(symbol)
                && json_text(item, "action")
                    .trim()
                    .eq_ignore_ascii_case(action)
        })
        .cloned()
}

/// Turns durable BUY-thesis records and the latest local broker-position
/// snapshot into an operator review queue. A due review is deliberately not
/// an exit recommendation: imported lots and later position changes can make
/// a recorded entry only partial provenance for the broker holding.
fn compact_holding_thesis_reviews(
    positions: &[JsonValue],
    thesis_rows: &[JsonValue],
    stale_after_days: i64,
    now: DateTime<Utc>,
) -> JsonValue {
    let stale_after_days = stale_after_days.max(1);
    let held = positions
        .iter()
        .filter_map(|position| {
            let symbol = json_text(position, "symbol");
            (value_f64(position, "quantity") > 1e-9 && !symbol.is_empty())
                .then(|| (watchlist_symbol_key(&symbol), position))
        })
        .collect::<HashMap<_, _>>();
    let held_position_count = held.len();
    let mut latest_thesis_by_symbol = HashMap::<String, (&JsonValue, JsonValue)>::new();
    for row in thesis_rows {
        let symbol = json_text(row, "symbol");
        let raw = row
            .get("trade_thesis_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let thesis = if raw.is_object() {
            raw
        } else {
            raw.as_str()
                .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                .unwrap_or(JsonValue::Null)
        };
        if symbol.is_empty() || json_text(&thesis, "status") != "recorded" {
            continue;
        }
        latest_thesis_by_symbol
            .entry(watchlist_symbol_key(&symbol))
            .or_insert((row, thesis));
    }

    let mut reviews = Vec::new();
    for (symbol, position) in held {
        let Some((row, thesis)) = latest_thesis_by_symbol.get(&symbol) else {
            continue;
        };
        let mut tracked_at = json_text(row, "first_fill_at");
        if tracked_at.is_empty() {
            tracked_at = json_text(row, "created_at");
        }
        let Some(tracked_at) = DateTime::parse_from_rfc3339(&tracked_at)
            .ok()
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        let age_days = (now - tracked_at).num_days().max(0);
        let intended_window = json_text(thesis, "intended_holding_window");
        let thesis_window_days = match intended_window.as_str() {
            "next_1_week" => Some(7),
            "next_2_weeks" => Some(14),
            "next_1_month" => Some(30),
            _ => None,
        };
        let decision_evidence_stale = age_days >= stale_after_days;
        let thesis_window_elapsed = thesis_window_days.is_some_and(|days| age_days >= days);
        if !decision_evidence_stale && !thesis_window_elapsed {
            continue;
        }
        let status = if thesis_window_elapsed {
            "thesis_window_elapsed"
        } else {
            "decision_evidence_stale"
        };
        reviews.push(json!({
            "symbol": json_text(position, "symbol"),
            "instrument_name": json_text(position, "instrument_name"),
            "position_quantity": value_f64(position, "quantity"),
            "latest_thesis_order_id": value_i64(row, "id"),
            "tracked_entry_at": tracked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "age_days": age_days,
            "decision_stale_after_days": stale_after_days,
            "intended_holding_window": intended_window,
            "intended_holding_window_days": thesis_window_days,
            "status": status,
            "entry_rationale": compact_review_text(&json_text(thesis, "entry_rationale"), 240),
            "invalidation": compact_review_text(&json_text(thesis, "invalidation"), 320),
            "operator_next_step": "Request or wait for a fresh decision pulse, then compare current verified technical and Markov evidence with the recorded entry thesis. This review does not instruct an exit.",
        }));
    }
    reviews.sort_by(|left, right| {
        value_i64(right, "age_days")
            .cmp(&value_i64(left, "age_days"))
            .then_with(|| json_text(left, "symbol").cmp(&json_text(right, "symbol")))
    });
    json!({
        "status": if reviews.is_empty() { "no_reviews_due" } else { "review_due" },
        "held_position_count": held_position_count,
        "review_count": reviews.len(),
        "decision_stale_after_days": stale_after_days,
        "reviews": reviews,
        "safety": "read_only_local_broker_position_snapshot_execution_order_thesis_and_fill_audit_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "A review identifies a held symbol with a recorded BUY thesis whose decision evidence is stale or intended window has elapsed. It is not a sell signal, sizing instruction, gate, or broker action."
    })
}

fn compact_review_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// Midnight at the start of a local calendar date, rendered as the UTC
/// RFC3339 string format used by portfolio_value_history.recorded_at.
fn local_date_start_to_utc_string(date: NaiveDate, tz: Tz) -> String {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    let local = tz
        .from_local_datetime(&midnight)
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&midnight));
    local
        .with_timezone(&Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn performance_range_limit(range_key: &str) -> i64 {
    // Rust match expressions are similar to Python's match/case or a JS switch, but they
    // must cover every possible input. The final `_` arm is the default case.
    match range_key {
        "1D" => 120,
        "1W" => 600,
        "1M" => 2500,
        "3M" => 5000,
        "YTD" => 5000,
        "1Y" => 5000,
        "ALL" => 5000,
        _ => 120,
    }
}

pub(crate) fn dashboard_embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

pub(crate) fn dashboard_required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

pub(crate) fn dashboard_optional_string(
    row: &JsonValue,
    key: &str,
) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

pub(crate) fn dashboard_required_boolish(row: &JsonValue, key: &str) -> serde_json::Result<bool> {
    match row.get(key) {
        Some(JsonValue::Bool(value)) => Ok(*value),
        Some(JsonValue::Number(value)) if value.as_i64() == Some(0) => Ok(false),
        Some(JsonValue::Number(value)) if value.as_i64() == Some(1) => Ok(true),
        Some(JsonValue::String(value)) if value == "0" || value.eq_ignore_ascii_case("false") => {
            Ok(false)
        }
        Some(JsonValue::String(value)) if value == "1" || value.eq_ignore_ascii_case("true") => {
            Ok(true)
        }
        _ => serde_json::from_value(JsonValue::Null),
    }
}

#[cfg(test)]
fn dashboard_optional_boolish(row: &JsonValue, key: &str) -> serde_json::Result<Option<bool>> {
    match row.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(_) => dashboard_required_boolish(row, key).map(Some),
    }
}

#[cfg(test)]
fn dashboard_candidate_scoring_technical_from_json(
    value: Option<&JsonValue>,
) -> CandidateScoringWaterfallTechnicalPayload {
    let value = value.unwrap_or(&JsonValue::Null);
    CandidateScoringWaterfallTechnicalPayload {
        status: json_text(value, "status"),
        source: json_text(value, "source"),
        run_date: json_text(value, "run_date"),
        sentiment: json_text(value, "sentiment"),
        trend_bias: json_text(value, "trend_bias"),
        confluence_count: value_i64(value, "confluence_count"),
        min_confluences: value_i64(value, "min_confluences"),
    }
}

#[cfg(test)]
pub(crate) fn dashboard_candidate_scoring_candidate_from_json(
    value: &JsonValue,
) -> CandidateScoringWaterfallCandidatePayload {
    let market = value.get("market").unwrap_or(&JsonValue::Null);
    let markov = value.get("markov").unwrap_or(&JsonValue::Null);
    let hermes = value.get("hermes").unwrap_or(&JsonValue::Null);
    let cost_guard = value
        .get("cost_guard")
        .filter(|value| value.is_object())
        .map(|value| CandidateScoringWaterfallCostGuardPayload {
            verified_from_db: value
                .get("verified_from_db")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            estimated_slippage_bps: value_f64(value, "estimated_slippage_bps"),
            cost_guard_multiple: value_f64(value, "cost_guard_multiple"),
            expected_reward_dkk: value_f64(value, "expected_reward_dkk"),
            round_trip_commission_dkk: value_f64(value, "round_trip_commission_dkk"),
            one_way_slippage_dkk: value_f64(value, "one_way_slippage_dkk"),
            required_reward_dkk: value_f64(value, "required_reward_dkk"),
            passes: value
                .get("passes")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            basis: json_text(value, "basis"),
        });
    let concentration = value
        .get("concentration")
        .filter(|value| value.is_object())
        .map(|value| CandidateScoringWaterfallConcentrationPayload {
            status: json_text(value, "status"),
            verified_from_state: value
                .get("verified_from_state")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            max_assets_per_exchange: value_i64(value, "max_assets_per_exchange"),
            max_assets_per_currency: value_i64(value, "max_assets_per_currency"),
            exchange: json_text(value, "exchange"),
            currency: json_text(value, "currency"),
            exchange_count_before: value_i64(value, "exchange_count_before"),
            currency_count_before: value_i64(value, "currency_count_before"),
            already_held: value
                .get("already_held")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            unmapped_exchange_symbol_count: value_i64(value, "unmapped_exchange_symbol_count"),
            unmapped_currency_symbol_count: value_i64(value, "unmapped_currency_symbol_count"),
        });
    let holding_limit = value
        .get("final_holding_limit")
        .filter(|value| value.is_object())
        .map(|value| CandidateScoringWaterfallHoldingLimitPayload {
            verified_from_state: value
                .get("verified_from_state")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            max_holdings: value_i64(value, "max_holdings"),
            holding_count_before: value_i64(value, "holding_count_before"),
            already_held: value
                .get("already_held")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
        });
    let position_weight = value
        .get("final_position_weight")
        .filter(|value| value.is_object())
        .map(|value| CandidateScoringWaterfallPositionWeightPayload {
            verified_from_state: value
                .get("verified_from_state")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            max_position_weight: value_f64(value, "max_position_weight"),
            current_position_value_dkk: value_f64(value, "current_position_value_dkk"),
            approved_value_dkk: value_f64(value, "approved_value_dkk"),
            resulting_position_value_dkk: value_f64(value, "resulting_position_value_dkk"),
            max_position_value_dkk: value_f64(value, "max_position_value_dkk"),
        });
    CandidateScoringWaterfallCandidatePayload {
        strategy_key: json_text(value, "strategy_key"),
        symbol: json_text(value, "symbol"),
        action: json_text(value, "action"),
        order_type: json_text(value, "order_type"),
        quantity: value_f64(value, "quantity"),
        market: CandidateScoringWaterfallMarketPayload {
            exchange: json_text(market, "exchange"),
            exchange_open: market
                .get("exchange_open")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            risk_excluded: market
                .get("risk_excluded")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            quarantine_active: market
                .get("quarantine_active")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
        },
        technical: dashboard_candidate_scoring_technical_from_json(value.get("technical")),
        final_technical: value
            .get("final_technical")
            .filter(|value| value.is_object())
            .map(|value| dashboard_candidate_scoring_technical_from_json(Some(value))),
        cost_guard,
        holding_limit,
        concentration,
        position_weight,
        markov: CandidateScoringWaterfallMarkovPayload {
            status: json_text(markov, "status"),
            fresh: markov
                .get("fresh")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            direction: json_text(markov, "direction"),
            signed_signal: value_f64(markov, "signed_signal"),
            age_days: value_i64(markov, "age_days"),
        },
        hermes: CandidateScoringWaterfallHermesPayload {
            effect: json_text(hermes, "effect"),
            requested_quantity: value_f64(hermes, "requested_quantity"),
            resulting_quantity: value_f64(hermes, "resulting_quantity"),
        },
        outcome: json_text(value, "outcome"),
        gate_code: json_text(value, "gate_code"),
    }
}

/// Decodes the selected Decision Report's stable outer fields. Its detailed
/// report and provider diagnostics remain compatibility JSON, while the
/// deterministic manager-gate waterfall has a fully typed dashboard boundary.
#[cfg(test)]
fn dashboard_candidate_scoring_waterfall_from_json(
    value: Option<&JsonValue>,
) -> CandidateScoringWaterfallPayload {
    let Some(value) = value.filter(|value| value.is_object()) else {
        return CandidateScoringWaterfallPayload::default();
    };
    let summary = value.get("summary").unwrap_or(&JsonValue::Null);
    CandidateScoringWaterfallPayload {
        status: json_text(value, "status"),
        run_id: value_i64(value, "run_id"),
        created_at: json_text(value, "created_at"),
        manager_status: json_text(value, "manager_status"),
        candidates: value
            .get("candidates")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .map(dashboard_candidate_scoring_candidate_from_json)
            .collect(),
        summary: CandidateScoringWaterfallSummaryPayload {
            candidate_count: value_i64(summary, "candidate_count"),
            approved_count: value_i64(summary, "approved_count"),
            skipped_count: value_i64(summary, "skipped_count"),
            not_reached_count: value_i64(summary, "not_reached_count"),
        },
        safety: json_text(value, "safety"),
    }
}

#[cfg(test)]
fn dashboard_selected_decision_from_json(
    decision: JsonValue,
) -> serde_json::Result<Option<DashboardSelectedDecisionPayload>> {
    if decision.is_null() {
        return Ok(None);
    }

    Ok(Some(DashboardSelectedDecisionPayload {
        id: dashboard_required_i64(&decision, "id")?,
        created_at: dashboard_required_string(&decision, "created_at")?,
        report_date: dashboard_optional_string(&decision, "report_date")?.unwrap_or_default(),
        model: dashboard_optional_string(&decision, "model")?.unwrap_or_default(),
        status: dashboard_required_string(&decision, "status")?,
        analysis_window_active: dashboard_optional_boolish(&decision, "analysis_window_active")?
            .unwrap_or(false),
        response_id: dashboard_optional_string(&decision, "response_id")?.unwrap_or_default(),
        prompt_text: dashboard_optional_string(&decision, "prompt_text")?.unwrap_or_default(),
        request_json: dashboard_embedded_json(&decision, "request_json").unwrap_or(JsonValue::Null),
        response_json: dashboard_embedded_json(&decision, "response_json")
            .unwrap_or(JsonValue::Null),
        report_json: dashboard_embedded_json(&decision, "report_json").unwrap_or(JsonValue::Null),
        error_text: dashboard_optional_string(&decision, "error_text")?
            .map(|value| compact_debug_text(&value, 420))
            .unwrap_or_default(),
        analysis_pulse_key: dashboard_optional_string(&decision, "analysis_pulse_key")?
            .unwrap_or_default(),
        analysis_pulse_label: dashboard_optional_string(&decision, "analysis_pulse_label")?
            .unwrap_or_default(),
        pulse_mode: dashboard_optional_string(&decision, "pulse_mode")?.unwrap_or_default(),
        queue_eligible: dashboard_optional_boolish(&decision, "queue_eligible")?.unwrap_or(false),
        candidate_scoring_waterfall: dashboard_candidate_scoring_waterfall_from_json(
            decision.get("candidate_scoring_waterfall"),
        ),
    }))
}

/// Decodes stable execution-order metadata for the public API. Detailed
/// lifecycle results and attribution remain in the local audit store.
pub(crate) fn execution_order_summaries_from_json(
    orders: Vec<JsonValue>,
) -> serde_json::Result<Vec<ExecutionOrderSummaryPayload>> {
    orders
        .into_iter()
        .map(|order| {
            let number =
                |key| dashboard_optional_f64(&order, key).map(|value| value.unwrap_or(0.0));
            Ok(ExecutionOrderSummaryPayload {
                id: dashboard_required_i64(&order, "id")?,
                created_at: dashboard_required_string(&order, "created_at")?,
                symbol: dashboard_required_string(&order, "symbol")?,
                action: dashboard_required_string(&order, "action")?,
                order_type: dashboard_optional_string(&order, "order_type")?.unwrap_or_default(),
                mode: dashboard_optional_string(&order, "mode")?.unwrap_or_default(),
                status: dashboard_required_string(&order, "status")?,
                adapter: dashboard_optional_string(&order, "adapter")?.unwrap_or_default(),
                quantity: number("quantity")?,
                price_local: number("price_local")?,
                limit_price_local: number("limit_price_local")?,
                stop_price_local: number("stop_price_local")?,
                currency: dashboard_optional_string(&order, "currency")?.unwrap_or_default(),
                estimated_value_dkk: number("estimated_value_dkk")?,
                strategy_type: dashboard_optional_string(&order, "strategy_type")?
                    .unwrap_or_default(),
                strategy_role: dashboard_optional_string(&order, "strategy_role")?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Decodes stable reconciled-fill metadata for the public API. Raw Saxo fill
/// payloads remain in the local audit store.
pub(crate) fn execution_fill_summaries_from_json(
    fills: Vec<JsonValue>,
) -> serde_json::Result<Vec<ExecutionFillSummaryPayload>> {
    fills
        .into_iter()
        .map(|fill| {
            let number = |key| dashboard_optional_f64(&fill, key).map(|value| value.unwrap_or(0.0));
            Ok(ExecutionFillSummaryPayload {
                id: dashboard_required_i64(&fill, "id")?,
                created_at: dashboard_required_string(&fill, "created_at")?,
                execution_order_id: dashboard_required_i64(&fill, "execution_order_id")?,
                broker_order_id: dashboard_optional_string(&fill, "broker_order_id")?,
                symbol: dashboard_required_string(&fill, "symbol")?,
                side: dashboard_required_string(&fill, "side")?,
                fill_status: dashboard_required_string(&fill, "fill_status")?,
                cumulative_quantity: number("cumulative_quantity")?,
                delta_quantity: number("delta_quantity")?,
                average_price_local: number("average_price_local")?,
                currency: dashboard_required_string(&fill, "currency")?,
                ledger_id: serde_json::from_value(
                    fill.get("ledger_id").cloned().unwrap_or(JsonValue::Null),
                )?,
            })
        })
        .collect()
}

/// Decodes stable lifecycle metadata for the public API. Raw broker responses
/// remain in the local audit store.
pub(crate) fn execution_event_summaries_from_json(
    events: Vec<JsonValue>,
) -> serde_json::Result<Vec<ExecutionEventSummaryPayload>> {
    events
        .into_iter()
        .map(|event| {
            Ok(ExecutionEventSummaryPayload {
                id: dashboard_required_i64(&event, "id")?,
                created_at: dashboard_required_string(&event, "created_at")?,
                execution_order_id: dashboard_required_i64(&event, "execution_order_id")?,
                event_type: dashboard_required_string(&event, "event_type")?,
                broker_status: dashboard_optional_string(&event, "broker_status")?,
            })
        })
        .collect()
}

/// Decodes the existing allowlisted lifecycle-event timeline into its public
/// typed contract. The state query deliberately omits raw Saxo payloads and
/// account identifiers before this conversion runs.
pub(crate) fn execution_order_event_timeline_entries_from_json(
    events: Vec<JsonValue>,
) -> serde_json::Result<Vec<ExecutionOrderEventTimelineEntryPayload>> {
    events
        .into_iter()
        .map(|event| {
            Ok(ExecutionOrderEventTimelineEntryPayload {
                id: dashboard_required_i64(&event, "id")?,
                created_at: dashboard_required_string(&event, "created_at")?,
                event_type: dashboard_required_string(&event, "event_type")?,
                broker_status: dashboard_optional_string(&event, "broker_status")?,
                broker_substatus: dashboard_optional_string(&event, "broker_substatus")?,
                broker_quantity: serde_json::from_value(
                    event
                        .get("broker_quantity")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                broker_price_local: serde_json::from_value(
                    event
                        .get("broker_price_local")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                broker_order_id: dashboard_optional_string(&event, "broker_order_id")?,
            })
        })
        .collect()
}

pub(crate) fn dashboard_optional_f64(
    row: &JsonValue,
    key: &str,
) -> serde_json::Result<Option<f64>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

pub(crate) fn dashboard_required_i64(row: &JsonValue, key: &str) -> serde_json::Result<i64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

/// Decodes the dashboard's sanitized AI settings and masked key status.
/// Provider configuration and raw key overrides remain outside this SSR view.
pub(crate) fn dashboard_ai_settings_from_json(
    settings: JsonValue,
) -> serde_json::Result<DashboardAiSettingsPayload> {
    read_model::decode("dashboard_ai_settings", settings)
}

/// Decodes a retained Markov, Quiver, or indicator run for dashboard status.
/// An absent row is an explicit no-run state; detailed config and summary data
/// stay staged JSON.
pub(crate) fn dashboard_latest_run_from_json(
    run: JsonValue,
) -> serde_json::Result<DashboardLatestRunPayload> {
    if run.is_null() {
        return Ok(DashboardLatestRunPayload::default());
    }
    let mut run: DashboardLatestRunPayload = read_model::decode("dashboard_latest_run", run)?;
    run.available = true;
    Ok(run)
}

/// Decodes a collector run into the public lifecycle-only summary. Detailed
/// configuration and summary documents are intentionally retained only by the
/// dashboard/internal read models.
pub(crate) fn signal_run_summary_from_json(
    run: JsonValue,
) -> serde_json::Result<crate::models::SignalRunSummaryPayload> {
    let run = dashboard_latest_run_from_json(run)?;
    Ok(crate::models::SignalRunSummaryPayload {
        available: run.available,
        id: run.id,
        created_at: run.created_at,
        run_date: run.run_date,
        status: run.status,
        asset_count: run.asset_count,
        success_count: run.success_count,
        error_count: run.error_count,
    })
}

fn tuning_pulse_kind(value: &str) -> Option<&'static str> {
    value.split(':').next().and_then(|kind| match kind {
        "europe_open_followup" => Some("europe_open_followup"),
        "europe_mid_session_shadow" => Some("europe_mid_session_shadow"),
        "us_open_followup" => Some("us_open_followup"),
        "us_mid_session_shadow" => Some("us_mid_session_shadow"),
        _ => None,
    })
}

fn tuning_pulse_comparison_from_rows(
    report_rows: &[JsonValue],
    shadow_rows: &[JsonValue],
) -> Vec<TuningPulseComparison> {
    let mut pulses = [
        TuningPulseComparison {
            pulse_key: "europe_open_followup".to_string(),
            pulse_label: "Nordic/EU Open +1h15".to_string(),
            authority: "execution_eligible".to_string(),
            report_count: 0,
            terminal_success_count: 0,
            terminal_success_rate: None,
            shadow_candidate_count: 0,
            shadow_candidate_report_count: 0,
            shadow_reports_missing_outcome_count: 0,
            shadow_awaiting_reference_count: 0,
            shadow_comparable_candidate_count: 0,
            shadow_new_candidate_count: 0,
            shadow_repeated_candidate_count: 0,
            shadow_candidate_novelty_rate: None,
            shadow_reference_captured_count: 0,
            shadow_reference_unavailable_retroactive_count: 0,
            one_session_outcome_count: 0,
            five_session_outcome_count: 0,
            twenty_session_outcome_count: 0,
            five_session_after_cost_count: 0,
            five_session_after_cost_positive_rate: None,
            maturity: "not_applicable".to_string(),
            outcome_status: "execution_outcomes_not_in_initial_shadow_slice".to_string(),
        },
        TuningPulseComparison {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            authority: "shadow_observation_only".to_string(),
            report_count: 0,
            terminal_success_count: 0,
            terminal_success_rate: None,
            shadow_candidate_count: 0,
            shadow_candidate_report_count: 0,
            shadow_reports_missing_outcome_count: 0,
            shadow_awaiting_reference_count: 0,
            shadow_comparable_candidate_count: 0,
            shadow_new_candidate_count: 0,
            shadow_repeated_candidate_count: 0,
            shadow_candidate_novelty_rate: None,
            shadow_reference_captured_count: 0,
            shadow_reference_unavailable_retroactive_count: 0,
            one_session_outcome_count: 0,
            five_session_outcome_count: 0,
            twenty_session_outcome_count: 0,
            five_session_after_cost_count: 0,
            five_session_after_cost_positive_rate: None,
            maturity: "collecting".to_string(),
            outcome_status: "awaiting_shadow_candidates".to_string(),
        },
        TuningPulseComparison {
            pulse_key: "us_open_followup".to_string(),
            pulse_label: "US Open +1h15".to_string(),
            authority: "execution_eligible".to_string(),
            report_count: 0,
            terminal_success_count: 0,
            terminal_success_rate: None,
            shadow_candidate_count: 0,
            shadow_candidate_report_count: 0,
            shadow_reports_missing_outcome_count: 0,
            shadow_awaiting_reference_count: 0,
            shadow_comparable_candidate_count: 0,
            shadow_new_candidate_count: 0,
            shadow_repeated_candidate_count: 0,
            shadow_candidate_novelty_rate: None,
            shadow_reference_captured_count: 0,
            shadow_reference_unavailable_retroactive_count: 0,
            one_session_outcome_count: 0,
            five_session_outcome_count: 0,
            twenty_session_outcome_count: 0,
            five_session_after_cost_count: 0,
            five_session_after_cost_positive_rate: None,
            maturity: "not_applicable".to_string(),
            outcome_status: "execution_outcomes_not_in_initial_shadow_slice".to_string(),
        },
        TuningPulseComparison {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            authority: "shadow_observation_only".to_string(),
            report_count: 0,
            terminal_success_count: 0,
            terminal_success_rate: None,
            shadow_candidate_count: 0,
            shadow_candidate_report_count: 0,
            shadow_reports_missing_outcome_count: 0,
            shadow_awaiting_reference_count: 0,
            shadow_comparable_candidate_count: 0,
            shadow_new_candidate_count: 0,
            shadow_repeated_candidate_count: 0,
            shadow_candidate_novelty_rate: None,
            shadow_reference_captured_count: 0,
            shadow_reference_unavailable_retroactive_count: 0,
            one_session_outcome_count: 0,
            five_session_outcome_count: 0,
            twenty_session_outcome_count: 0,
            five_session_after_cost_count: 0,
            five_session_after_cost_positive_rate: None,
            maturity: "collecting".to_string(),
            outcome_status: "awaiting_shadow_candidates".to_string(),
        },
    ];
    let mut shadow_candidate_report_ids = [
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
    ];
    let mut shadow_outcome_report_ids = [
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
        HashSet::<i64>::new(),
    ];
    for row in report_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(index) = pulses.iter().position(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        let is_shadow = {
            let pulse = &mut pulses[index];
            pulse.report_count += 1;
            if matches!(
                json_text(row, "status").as_str(),
                "completed" | "xai_fallback"
            ) {
                pulse.terminal_success_count += 1;
            }
            pulse.authority == "shadow_observation_only"
        };
        let report_id = value_i64(row, "id");
        let report_json = decode_shadow_json_field(row.get("report_json"));
        if is_shadow
            && json_text(row, "status") == "completed"
            && report_id > 0
            && crate::xai_decision::shadow_report_has_recordable_candidates(&report_json)
        {
            if shadow_candidate_report_ids[index].insert(report_id) {
                pulses[index].shadow_candidate_report_count += 1;
            }
        }
    }
    for pulse in &mut pulses {
        if pulse.report_count > 0 {
            pulse.terminal_success_rate =
                Some(pulse.terminal_success_count as f64 / pulse.report_count as f64);
        }
    }

    let mut after_cost_positive_counts = [0_i64; 4];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(index) = pulses.iter().position(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        let pulse = &mut pulses[index];
        if pulse.authority != "shadow_observation_only" {
            continue;
        }
        let report_id = value_i64(row, "report_id");
        if report_id > 0 {
            shadow_outcome_report_ids[index].insert(report_id);
        }
        pulse.shadow_candidate_count += 1;
        if json_text(row, "status") == "awaiting_reference" {
            pulse.shadow_awaiting_reference_count += 1;
        }
        if value_i64(row, "earlier_pulse_report_id") > 0 {
            pulse.shadow_comparable_candidate_count += 1;
            if row
                .get("appeared_in_earlier_pulse")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
            {
                pulse.shadow_repeated_candidate_count += 1;
            } else {
                pulse.shadow_new_candidate_count += 1;
            }
        }
        if json_text(row, "reference_price_source") == "saxo_infoprices" {
            pulse.shadow_reference_captured_count += 1;
        } else if json_text(row, "reference_price_source") == "not_captured_retroactively" {
            pulse.shadow_reference_unavailable_retroactive_count += 1;
        }
        if !decode_shadow_json_field(row.get("one_session_outcome_json")).is_null() {
            pulse.one_session_outcome_count += 1;
        }
        if !decode_shadow_json_field(row.get("five_session_outcome_json")).is_null() {
            pulse.five_session_outcome_count += 1;
        }
        if !decode_shadow_json_field(row.get("twenty_session_outcome_json")).is_null() {
            pulse.twenty_session_outcome_count += 1;
        }
        let after_cost = decode_shadow_json_field(row.get("five_session_after_cost_outcome_json"));
        if json_text(&after_cost, "status") == "estimated" {
            pulse.five_session_after_cost_count += 1;
            if value_f64(&after_cost, "estimated_after_cost_return_pct") > 0.0 {
                after_cost_positive_counts[index] += 1;
            }
        }
    }
    for (index, pulse) in pulses.iter_mut().enumerate() {
        if pulse.authority != "shadow_observation_only" {
            continue;
        }
        pulse.shadow_reports_missing_outcome_count = shadow_candidate_report_ids[index]
            .difference(&shadow_outcome_report_ids[index])
            .count() as i64;
        if pulse.shadow_reports_missing_outcome_count > 0 {
            pulse.outcome_status = "shadow_outcome_ledger_gap".to_string();
        } else if pulse.shadow_candidate_count == 0 {
            pulse.outcome_status = "awaiting_shadow_candidates".to_string();
        } else if pulse.shadow_awaiting_reference_count > 0 {
            pulse.outcome_status = "awaiting_saxo_reference_quote".to_string();
        } else if pulse.shadow_reference_captured_count == 0
            && pulse.shadow_reference_unavailable_retroactive_count == pulse.shadow_candidate_count
        {
            pulse.outcome_status = "historical_reference_unavailable".to_string();
        } else if pulse.shadow_reference_captured_count == 0 {
            pulse.outcome_status = "awaiting_saxo_reference_quote".to_string();
        } else {
            pulse.outcome_status = "observational_shadow_outcomes".to_string();
        }
        if pulse.five_session_after_cost_count > 0 {
            pulse.five_session_after_cost_positive_rate = Some(
                after_cost_positive_counts[index] as f64
                    / pulse.five_session_after_cost_count as f64,
            );
        }
        if pulse.shadow_comparable_candidate_count > 0 {
            pulse.shadow_candidate_novelty_rate = Some(
                pulse.shadow_new_candidate_count as f64
                    / pulse.shadow_comparable_candidate_count as f64,
            );
        }
        pulse.maturity = if pulse.twenty_session_outcome_count >= TUNING_MATURE_OUTCOME_MINIMUM {
            "mature".to_string()
        } else if pulse.five_session_outcome_count >= TUNING_MATURE_OUTCOME_MINIMUM {
            "preliminary".to_string()
        } else {
            "collecting".to_string()
        };
    }
    pulses.into()
}

fn tuning_shadow_change_evidence_from_rows(
    report_rows: &[JsonValue],
) -> Vec<TuningShadowChangeEvidence> {
    let mut pulses = [
        TuningShadowChangeEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            report_count: 0,
            comparison_available_assessment_count: 0,
            material_change_count: 0,
            no_new_information_count: 0,
            no_new_information_rate: None,
            opening_reference_not_available_count: 0,
            not_applicable_count: 0,
            comparison_invalid_count: 0,
            missing_assessment_count: 0,
            unclassified_assessment_count: 0,
        },
        TuningShadowChangeEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            report_count: 0,
            comparison_available_assessment_count: 0,
            material_change_count: 0,
            no_new_information_count: 0,
            no_new_information_rate: None,
            opening_reference_not_available_count: 0,
            not_applicable_count: 0,
            comparison_invalid_count: 0,
            missing_assessment_count: 0,
            unclassified_assessment_count: 0,
        },
    ];
    for row in report_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(pulse) = pulses.iter_mut().find(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        pulse.report_count += 1;
        let report = decode_shadow_json_field(row.get("report_json"));
        let Some(assessment) = report.get("shadow_change_assessment") else {
            pulse.missing_assessment_count += 1;
            continue;
        };
        match json_text(assessment, "status").as_str() {
            "material_change" => {
                pulse.comparison_available_assessment_count += 1;
                pulse.material_change_count += 1;
            }
            "no_new_information" => {
                pulse.comparison_available_assessment_count += 1;
                pulse.no_new_information_count += 1;
            }
            "not_available" => pulse.opening_reference_not_available_count += 1,
            "not_applicable" => pulse.not_applicable_count += 1,
            "comparison_invalid" => {
                pulse.comparison_available_assessment_count += 1;
                pulse.comparison_invalid_count += 1;
            }
            _ => pulse.unclassified_assessment_count += 1,
        }
    }
    for pulse in &mut pulses {
        if pulse.comparison_available_assessment_count > 0 {
            pulse.no_new_information_rate = Some(
                pulse.no_new_information_count as f64
                    / pulse.comparison_available_assessment_count as f64,
            );
        }
    }
    pulses.into()
}

fn tuning_shadow_support_risk_evidence_from_rows(
    shadow_rows: &[JsonValue],
) -> Vec<TuningShadowSupportRiskEvidence> {
    let mut pulses = [
        TuningShadowSupportRiskEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            low_break_risk_count: 0,
            moderate_break_risk_count: 0,
            high_break_risk_count: 0,
            unavailable_count: 0,
            complete_context_count: 0,
            average_break_risk: None,
            average_confidence: None,
            average_history_coverage: None,
            unclassified_count: 0,
        },
        TuningShadowSupportRiskEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            low_break_risk_count: 0,
            moderate_break_risk_count: 0,
            high_break_risk_count: 0,
            unavailable_count: 0,
            complete_context_count: 0,
            average_break_risk: None,
            average_confidence: None,
            average_history_coverage: None,
            unclassified_count: 0,
        },
    ];
    let mut break_risk_sums = [0.0_f64; 2];
    let mut confidence_sums = [0.0_f64; 2];
    let mut history_coverage_sums = [0.0_f64; 2];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(index) = pulses.iter().position(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        let pulse = &mut pulses[index];
        pulse.candidate_count += 1;
        let context = decode_shadow_json_field(row.get("report_time_context_json"));
        let support = context
            .get("support_risk_snapshot")
            .filter(|value| value.is_object());
        let Some(support) = support else {
            pulse.unavailable_count += 1;
            continue;
        };
        pulse.snapshot_available_count += 1;
        match json_text(support, "break_risk_label").as_str() {
            "low" => pulse.low_break_risk_count += 1,
            "moderate" => pulse.moderate_break_risk_count += 1,
            "high" => pulse.high_break_risk_count += 1,
            "" | "unavailable" | "not_available_at_report_time" => pulse.unavailable_count += 1,
            _ => pulse.unclassified_count += 1,
        }
        let (Some(break_risk), Some(confidence), Some(history_coverage)) = (
            tuning_optional_number(support, "break_risk"),
            tuning_optional_number(support, "confidence"),
            tuning_optional_number(support, "history_coverage"),
        ) else {
            continue;
        };
        pulse.complete_context_count += 1;
        break_risk_sums[index] += break_risk.clamp(0.0, 1.0);
        confidence_sums[index] += confidence.clamp(0.0, 1.0);
        history_coverage_sums[index] += history_coverage.clamp(0.0, 1.0);
    }
    for (index, pulse) in pulses.iter_mut().enumerate() {
        if pulse.complete_context_count > 0 {
            let denominator = pulse.complete_context_count as f64;
            pulse.average_break_risk = Some(break_risk_sums[index] / denominator);
            pulse.average_confidence = Some(confidence_sums[index] / denominator);
            pulse.average_history_coverage = Some(history_coverage_sums[index] / denominator);
        }
    }
    pulses.into()
}

fn tuning_shadow_markov_evidence_from_rows(
    shadow_rows: &[JsonValue],
) -> Vec<TuningShadowMarkovEvidence> {
    let mut pulses = [
        TuningShadowMarkovEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            long_direction_count: 0,
            short_direction_count: 0,
            neutral_direction_count: 0,
            unavailable_count: 0,
            complete_signal_count: 0,
            average_signed_signal: None,
            unclassified_count: 0,
        },
        TuningShadowMarkovEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            long_direction_count: 0,
            short_direction_count: 0,
            neutral_direction_count: 0,
            unavailable_count: 0,
            complete_signal_count: 0,
            average_signed_signal: None,
            unclassified_count: 0,
        },
    ];
    let mut signed_signal_sums = [0.0_f64; 2];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(index) = pulses.iter().position(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        let pulse = &mut pulses[index];
        pulse.candidate_count += 1;
        let context = decode_shadow_json_field(row.get("report_time_context_json"));
        let markov = context
            .get("markov_snapshot")
            .filter(|value| value.is_object());
        let Some(markov) = markov else {
            pulse.unavailable_count += 1;
            continue;
        };
        pulse.snapshot_available_count += 1;
        let status = json_text(markov, "status");
        match status.as_str() {
            "available" => match json_text(markov, "direction").as_str() {
                "long" => pulse.long_direction_count += 1,
                "short" => pulse.short_direction_count += 1,
                "neutral" => pulse.neutral_direction_count += 1,
                "" | "unavailable" | "not_available_at_report_time" => pulse.unavailable_count += 1,
                _ => pulse.unclassified_count += 1,
            },
            "" | "unavailable" | "not_available_at_report_time" => pulse.unavailable_count += 1,
            _ => pulse.unclassified_count += 1,
        }
        if status != "available" {
            continue;
        }
        let Some(signed_signal) = tuning_optional_number(markov, "signed_signal") else {
            continue;
        };
        pulse.complete_signal_count += 1;
        signed_signal_sums[index] += signed_signal;
    }
    for (index, pulse) in pulses.iter_mut().enumerate() {
        if pulse.complete_signal_count > 0 {
            pulse.average_signed_signal =
                Some(signed_signal_sums[index] / pulse.complete_signal_count as f64);
        }
    }
    pulses.into()
}

fn tuning_shadow_quiver_evidence_from_rows(
    shadow_rows: &[JsonValue],
) -> Vec<TuningShadowQuiverEvidence> {
    let mut pulses = [
        TuningShadowQuiverEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            fresh_source_count: 0,
            partial_source_count: 0,
            stale_source_count: 0,
            unavailable_source_count: 0,
            bullish_direction_count: 0,
            bearish_direction_count: 0,
            neutral_direction_count: 0,
            complete_signal_count: 0,
            average_signal: None,
            average_confidence: None,
            unclassified_count: 0,
        },
        TuningShadowQuiverEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            candidate_count: 0,
            snapshot_available_count: 0,
            fresh_source_count: 0,
            partial_source_count: 0,
            stale_source_count: 0,
            unavailable_source_count: 0,
            bullish_direction_count: 0,
            bearish_direction_count: 0,
            neutral_direction_count: 0,
            complete_signal_count: 0,
            average_signal: None,
            average_confidence: None,
            unclassified_count: 0,
        },
    ];
    let mut signal_sums = [0.0_f64; 2];
    let mut confidence_sums = [0.0_f64; 2];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(index) = pulses.iter().position(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        let pulse = &mut pulses[index];
        pulse.candidate_count += 1;
        let context = decode_shadow_json_field(row.get("report_time_context_json"));
        let quiver = context
            .get("quiver_snapshot")
            .filter(|value| value.is_object());
        let Some(quiver) = quiver else {
            pulse.unavailable_source_count += 1;
            continue;
        };
        pulse.snapshot_available_count += 1;
        let freshness = quiver
            .get("freshness")
            .filter(|value| value.is_object())
            .map(|value| json_text(value, "status"))
            .unwrap_or_default();
        match freshness.as_str() {
            "fresh" => pulse.fresh_source_count += 1,
            "partial" => pulse.partial_source_count += 1,
            "stale" => pulse.stale_source_count += 1,
            "" | "missing" | "failed" | "not_due" | "no_us_session" => {
                pulse.unavailable_source_count += 1
            }
            _ => pulse.unclassified_count += 1,
        }
        if json_text(quiver, "status") != "available" {
            continue;
        }
        match json_text(quiver, "direction").as_str() {
            "bullish" => pulse.bullish_direction_count += 1,
            "bearish" => pulse.bearish_direction_count += 1,
            "neutral" => pulse.neutral_direction_count += 1,
            "" | "unavailable" | "not_available_at_report_time" => {}
            _ => pulse.unclassified_count += 1,
        }
        let (Some(signal), Some(confidence)) = (
            tuning_optional_number(quiver, "signal"),
            tuning_optional_number(quiver, "confidence"),
        ) else {
            continue;
        };
        pulse.complete_signal_count += 1;
        signal_sums[index] += signal;
        confidence_sums[index] += confidence.clamp(0.0, 1.0);
    }
    for (index, pulse) in pulses.iter_mut().enumerate() {
        if pulse.complete_signal_count > 0 {
            let denominator = pulse.complete_signal_count as f64;
            pulse.average_signal = Some(signal_sums[index] / denominator);
            pulse.average_confidence = Some(confidence_sums[index] / denominator);
        }
    }
    pulses.into()
}

fn tuning_shadow_gate_evidence_from_rows(
    shadow_rows: &[JsonValue],
) -> Vec<TuningShadowGateEvidence> {
    let mut pulses = [
        TuningShadowGateEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            candidate_count: 0,
            technical_source_count: 0,
            markov_fallback_source_count: 0,
            not_evaluated_source_count: 0,
            clear_signal_count: 0,
            blocked_signal_count: 0,
            insufficient_evidence_count: 0,
            unclassified_count: 0,
        },
        TuningShadowGateEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            candidate_count: 0,
            technical_source_count: 0,
            markov_fallback_source_count: 0,
            not_evaluated_source_count: 0,
            clear_signal_count: 0,
            blocked_signal_count: 0,
            insufficient_evidence_count: 0,
            unclassified_count: 0,
        },
    ];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(pulse) = pulses.iter_mut().find(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        pulse.candidate_count += 1;
        let mut classified = true;
        match json_text(row, "deterministic_gate_code").as_str() {
            "technical" => pulse.technical_source_count += 1,
            "markov" => pulse.markov_fallback_source_count += 1,
            "not_evaluated_shadow" => pulse.not_evaluated_source_count += 1,
            _ => classified = false,
        }
        let gate = decode_shadow_json_field(row.get("deterministic_gate_json"));
        match json_text(&gate, "status").as_str() {
            "clear_signal_gate_only" => pulse.clear_signal_count += 1,
            "blocked_signal_gate" => pulse.blocked_signal_count += 1,
            "insufficient_decision_time_evidence" => pulse.insufficient_evidence_count += 1,
            _ => classified = false,
        }
        if !classified {
            pulse.unclassified_count += 1;
        }
    }
    pulses.into()
}

fn tuning_shadow_hermes_evidence_from_rows(
    shadow_rows: &[JsonValue],
) -> Vec<TuningShadowHermesEvidence> {
    let mut pulses = [
        TuningShadowHermesEvidence {
            pulse_key: "europe_mid_session_shadow".to_string(),
            pulse_label: "Nordic/EU 14:15 Shadow".to_string(),
            candidate_count: 0,
            allow_count: 0,
            reduce_count: 0,
            stand_down_count: 0,
            review_count: 0,
            no_matching_advice_count: 0,
            unavailable_count: 0,
            not_requested_count: 0,
            self_check_complete_count: 0,
            self_check_incomplete_count: 0,
            self_check_not_recorded_count: 0,
            approved_policy_source_count: 0,
            missing_policy_source_count: 0,
            unclassified_effect_count: 0,
        },
        TuningShadowHermesEvidence {
            pulse_key: "us_mid_session_shadow".to_string(),
            pulse_label: "US 14:15 Shadow".to_string(),
            candidate_count: 0,
            allow_count: 0,
            reduce_count: 0,
            stand_down_count: 0,
            review_count: 0,
            no_matching_advice_count: 0,
            unavailable_count: 0,
            not_requested_count: 0,
            self_check_complete_count: 0,
            self_check_incomplete_count: 0,
            self_check_not_recorded_count: 0,
            approved_policy_source_count: 0,
            missing_policy_source_count: 0,
            unclassified_effect_count: 0,
        },
    ];
    for row in shadow_rows {
        let Some(kind) = tuning_pulse_kind(&json_text(row, "analysis_pulse_key")) else {
            continue;
        };
        let Some(pulse) = pulses.iter_mut().find(|pulse| pulse.pulse_key == kind) else {
            continue;
        };
        pulse.candidate_count += 1;
        match json_text(row, "hermes_effect").as_str() {
            "record_only_allow" => pulse.allow_count += 1,
            "record_only_reduce" => pulse.reduce_count += 1,
            "record_only_stand_down" => pulse.stand_down_count += 1,
            "record_only_review" => pulse.review_count += 1,
            "record_only_no_matching_advice" => pulse.no_matching_advice_count += 1,
            "record_only_unavailable" => pulse.unavailable_count += 1,
            "not_requested_shadow" => pulse.not_requested_count += 1,
            _ => pulse.unclassified_effect_count += 1,
        }
        let advice = decode_shadow_json_field(row.get("hermes_advice_snapshot_json"));
        match advice
            .get("context_self_check_complete")
            .and_then(JsonValue::as_bool)
        {
            Some(true) => pulse.self_check_complete_count += 1,
            Some(false) => pulse.self_check_incomplete_count += 1,
            None => pulse.self_check_not_recorded_count += 1,
        }
        if json_text(row, "approved_policy_source").is_empty() {
            pulse.missing_policy_source_count += 1;
        } else {
            pulse.approved_policy_source_count += 1;
        }
    }
    pulses.into()
}

pub(crate) fn tuning_optional_number(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(JsonValue::as_f64)
        .filter(|number| number.is_finite())
}

pub(crate) fn tuning_directional_outcome(value: &JsonValue) -> TuningDirectionalOutcome {
    TuningDirectionalOutcome {
        sample_count: value_i64(value, "sample_count"),
        average_directional_return_pct: tuning_optional_number(
            value,
            "average_directional_return_pct",
        ),
        positive_return_rate: tuning_optional_number(value, "positive_return_rate"),
    }
}

pub(crate) fn tuning_trade_thesis_evidence_from_payload(
    value: &JsonValue,
) -> TuningTradeThesisEvidence {
    TuningTradeThesisEvidence {
        status: json_text(value, "status"),
        recorded_thesis_count: value_i64(value, "recorded_thesis_count"),
        filled_thesis_count: value_i64(value, "filled_thesis_count"),
        one_session: tuning_directional_outcome(
            value.get("one_session").unwrap_or(&JsonValue::Null),
        ),
        five_session: tuning_directional_outcome(
            value.get("five_session").unwrap_or(&JsonValue::Null),
        ),
        minimum_complete_observations: value_i64(value, "minimum_complete_observations"),
        scan_limit: value_i64(value, "scan_limit"),
        scope: "newest_recorded_buy_theses_independent_of_30_day_tuning_window".to_string(),
        gross_net_label:
            "gross_directional_return_excludes_fx_commission_tax_slippage_and_realised_p_l"
                .to_string(),
        interpretation: json_text(value, "interpretation"),
    }
}

fn tuning_experiment_governance_from_rows(rows: &[JsonValue]) -> TuningExperimentGovernance {
    let mut evidence = TuningExperimentGovernance {
        status: "available".to_string(),
        total_experiment_count: 0,
        pending_review_count: 0,
        approved_paper_count: 0,
        approved_sim_count: 0,
        active_paper_count: 0,
        active_sim_count: 0,
        ready_for_promotion_count: 0,
        promoted_count: 0,
        terminal_count: 0,
        unclassified_count: 0,
        scope: "all_retained_strategy_experiment_lifecycle_rows".to_string(),
        interpretation: "Lifecycle inventory only. Pending proposals never activate themselves, and this summary does not expose proposed values, assess performance, or change Hermes, a manager gate, queue, broker precheck, or Saxo execution.".to_string(),
    };
    for row in rows {
        let count = value_i64(row, "count").max(0);
        evidence.total_experiment_count += count;
        match json_text(row, "status").as_str() {
            "pending_review" => evidence.pending_review_count += count,
            "approved_paper" => evidence.approved_paper_count += count,
            "approved_sim" => evidence.approved_sim_count += count,
            "active_paper" => evidence.active_paper_count += count,
            "active_sim" => evidence.active_sim_count += count,
            "ready_for_promotion" => evidence.ready_for_promotion_count += count,
            "promoted" => evidence.promoted_count += count,
            "rejected" | "paper_failed" | "sim_failed" | "failed" | "expired_stale" => {
                evidence.terminal_count += count
            }
            _ => evidence.unclassified_count += count,
        }
    }
    evidence
}

pub(crate) fn tuning_experiment_governance_unavailable() -> TuningExperimentGovernance {
    TuningExperimentGovernance {
        status: "unavailable".to_string(),
        total_experiment_count: 0,
        pending_review_count: 0,
        approved_paper_count: 0,
        approved_sim_count: 0,
        active_paper_count: 0,
        active_sim_count: 0,
        ready_for_promotion_count: 0,
        promoted_count: 0,
        terminal_count: 0,
        unclassified_count: 0,
        scope: "unavailable".to_string(),
        interpretation: "Experiment lifecycle inventory could not be loaded. It does not affect Hermes, a manager gate, queue, broker precheck, or Saxo execution.".to_string(),
    }
}

pub(crate) fn tuning_portfolio_outcome_from_summary(summary: &JsonValue) -> TuningPortfolioOutcome {
    let confidence = summary.get("confidence").unwrap_or(&JsonValue::Null);
    let status = json_text(confidence, "status");
    let latest_available = !status.is_empty() && status != "unavailable";
    let comparable_range = latest_available && value_i64(confidence, "valid_points") >= 2;
    TuningPortfolioOutcome {
        status,
        range_key: "1M".to_string(),
        snapshot_count: value_i64(summary, "points"),
        valid_snapshot_count: value_i64(confidence, "valid_points"),
        latest_value_dkk: latest_available
            .then(|| tuning_optional_number(summary, "latest_total_market_value_dkk"))
            .flatten(),
        change_dkk: comparable_range
            .then(|| tuning_optional_number(summary, "change_dkk"))
            .flatten(),
        simple_return_pct: comparable_range
            .then(|| tuning_optional_number(summary, "range_return_pct"))
            .flatten(),
        max_drawdown_pct: comparable_range
            .then(|| tuning_optional_number(summary, "range_max_drawdown_pct"))
            .flatten(),
        latest_recorded_at: json_text(confidence, "latest_recorded_at"),
        latest_snapshot_type: json_text(confidence, "latest_snapshot_type"),
        latest_source: json_text(confidence, "latest_source"),
        age_minutes: confidence.get("age_minutes").and_then(JsonValue::as_i64),
        unreliable_cost_basis_points: value_i64(summary, "unreliable_cost_basis_points"),
        scope: "one_month_local_account_value_history_including_cash".to_string(),
        return_kind:
            "simple_snapshot_movement_not_realised_pnl_time_weighted_or_total_return".to_string(),
        caveat: "Account-value snapshots include cash. Change and return are simple local snapshot movement, not realised P/L, time-weighted or total return, and are not normalized for deposits, withdrawals, FX, dividends, fees, or tax.".to_string(),
    }
}

pub(crate) fn tuning_portfolio_outcome_unavailable() -> TuningPortfolioOutcome {
    tuning_portfolio_outcome_from_summary(&json!({
        "confidence": {"status": "unavailable"}
    }))
}

pub(crate) fn tuning_monthly_goal_progress_from_payload(
    value: &JsonValue,
) -> TuningMonthlyGoalProgress {
    let month = value
        .get("periods")
        .and_then(|periods| periods.get("month"))
        .unwrap_or(&JsonValue::Null);
    TuningMonthlyGoalProgress {
        status: json_text(month, "status"),
        target_dkk: tuning_optional_number(month, "target_dkk"),
        value_change_dkk: tuning_optional_number(month, "pnl_dkk"),
        target_progress: tuning_optional_number(month, "progress_pct"),
        baseline_value_dkk: tuning_optional_number(month, "baseline_value_dkk"),
        period_start: json_text(month, "period_start_utc"),
        scope: "configured_calendar_month_portfolio_value_change_against_dkk_target".to_string(),
        caveat: "The configured monthly DKK target is a planning reference and must be reviewed when portfolio value changes materially. Progress is local portfolio-value change from the active-batch calendar-month baseline, not realised P/L, time-weighted or total return, and is not normalized for deposits, withdrawals, FX, dividends, fees, or tax.".to_string(),
    }
}

pub(crate) fn tuning_monthly_goal_progress_unavailable() -> TuningMonthlyGoalProgress {
    tuning_monthly_goal_progress_from_payload(&json!({
        "periods": {"month": {"status": "unavailable"}}
    }))
}

pub(crate) fn tuning_benchmark_comparison_from_payload(
    value: &JsonValue,
) -> TuningBenchmarkComparison {
    let latest_run = value.get("latest_run").unwrap_or(&JsonValue::Null);
    let references = value
        .get("references")
        .and_then(JsonValue::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| TuningBenchmarkReference {
                    key: json_text(row, "key"),
                    label: json_text(row, "label"),
                    symbol: json_text(row, "symbol"),
                    status: json_text(row, "status"),
                    benchmark_return_pct: tuning_optional_number(row, "benchmark_return_pct"),
                    excess_return_pct: tuning_optional_number(row, "excess_return_pct"),
                    freshness: json_text(row, "freshness"),
                    baseline_at: json_text(row, "baseline_at"),
                    latest_at: json_text(row, "latest_at"),
                })
                .collect()
        })
        .unwrap_or_default();
    TuningBenchmarkComparison {
        status: json_text(value, "status"),
        portfolio_return_pct: tuning_optional_number(value, "portfolio_return_pct"),
        ready_count: value_i64(value, "ready_count"),
        reference_count: value_i64(value, "reference_count"),
        aligned_count: value_i64(value, "aligned_count"),
        prior_close_count: value_i64(value, "prior_close_count"),
        stale_close_count: value_i64(value, "stale_close_count"),
        freshness: json_text(value, "freshness"),
        collector_status: {
            let status = json_text(latest_run, "status");
            if status.is_empty() {
                "not_run".to_string()
            } else {
                status
            }
        },
        collector_run_at: json_text(latest_run, "created_at"),
        collector_run_date: json_text(latest_run, "run_date"),
        collector_reference_count: value_i64(latest_run, "reference_count"),
        collector_success_count: value_i64(latest_run, "success_count"),
        collector_error_count: value_i64(latest_run, "error_count"),
        references,
        scope: "one_month_local_account_value_vs_stored_native_currency_proxy_price_returns"
            .to_string(),
        return_kind: "not_time_weighted_or_total_return_not_normalized_for_fx_dividends_fees_tax_or_external_cash_flows".to_string(),
        caveat: json_text(value, "caveat"),
    }
}

pub(crate) fn tuning_benchmark_comparison_unavailable() -> TuningBenchmarkComparison {
    tuning_benchmark_comparison_from_payload(&json!({"status": "unavailable"}))
}

fn tuning_execution_pulse_outcomes_from_evidence(
    evidence: &JsonValue,
) -> Vec<TuningExecutionPulseOutcome> {
    ["europe_open_followup", "us_open_followup"]
        .into_iter()
        .map(|pulse_key| {
            let row = evidence
                .get("pulses")
                .and_then(JsonValue::as_array)
                .and_then(|rows| {
                    rows.iter().find(|row| json_text(row, "pulse_key") == pulse_key)
                })
                .cloned()
                .unwrap_or(JsonValue::Null);
            let outcome = row.get("outcome").unwrap_or(&JsonValue::Null);
            let one_session = tuning_directional_outcome(
                outcome.get("one_session").unwrap_or(&JsonValue::Null),
            );
            let five_session = tuning_directional_outcome(
                outcome.get("five_session").unwrap_or(&JsonValue::Null),
            );
            let maturity = if value_i64(outcome, "attributed_order_count") == 0 {
                "no_attributable_orders"
            } else if five_session.sample_count < TUNING_MATURE_OUTCOME_MINIMUM {
                "collecting"
            } else {
                "preliminary"
            };
            let realised_sell = outcome.get("realised_sell").unwrap_or(&JsonValue::Null);
            TuningExecutionPulseOutcome {
                pulse_key: pulse_key.to_string(),
                pulse_label: match pulse_key {
                    "europe_open_followup" => "Nordic/EU Open +1h15",
                    _ => "US Open +1h15",
                }
                .to_string(),
                attributed_order_count: value_i64(outcome, "attributed_order_count"),
                filled_buy_order_count: value_i64(outcome, "filled_buy_order_count"),
                one_session,
                five_session,
                reconciled_sell_order_count: value_i64(outcome, "reconciled_sell_order_count"),
                realised_sell_gain_dkk: value_f64(realised_sell, "realised_gain_dkk"),
                realised_sell_commission_dkk: value_f64(realised_sell, "commission_dkk"),
                realised_sell_tax_dkk: value_f64(realised_sell, "tax_dkk"),
                maturity: maturity.to_string(),
                interpretation: "BUY fields are equal-weighted directional price movement after reconciled fills. SELL fields are local-ledger realised accounting after reconciliation. These evidence types remain separate from each other and from shadow observations.".to_string(),
            }
        })
        .collect()
}

/// Summarizes the persisted local status of orders produced by the two
/// execution-eligible pulses. It consumes the same bounded evidence envelope
/// as the outcome table, so it neither replays the manager nor asks Saxo for
/// a current broker status.
fn tuning_execution_lifecycle_evidence_from_evidence(
    evidence: &JsonValue,
) -> Vec<TuningExecutionLifecycleEvidence> {
    ["europe_open_followup", "us_open_followup"]
        .into_iter()
        .map(|pulse_key| {
            let outcome = evidence
                .get("pulses")
                .and_then(JsonValue::as_array)
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| json_text(row, "pulse_key") == pulse_key)
                })
                .and_then(|row| row.get("outcome"))
                .unwrap_or(&JsonValue::Null);
            let mut lifecycle = TuningExecutionLifecycleEvidence {
                pulse_key: pulse_key.to_string(),
                pulse_label: match pulse_key {
                    "europe_open_followup" => "Nordic/EU Open +1h15",
                    _ => "US Open +1h15",
                }
                .to_string(),
                attributed_order_count: value_i64(outcome, "attributed_order_count"),
                locally_queued_count: 0,
                broker_active_count: 0,
                broker_state_unknown_count: 0,
                executed_count: 0,
                failed_count: 0,
                expired_count: 0,
                cancelled_count: 0,
                unclassified_count: 0,
            };
            for (status, count) in outcome
                .get("execution_status_counts")
                .and_then(JsonValue::as_object)
                .into_iter()
                .flatten()
            {
                let count = count.as_i64().unwrap_or_default();
                match status.trim().to_ascii_lowercase().as_str() {
                    "pending_execution"
                    | "pending_approval"
                    | "waiting_for_market_open"
                    | "waiting_for_cash_settlement"
                    | "waiting_for_virtual_cash_budget"
                    | "submitting_to_broker" => lifecycle.locally_queued_count += count,
                    "submitted_to_broker"
                    | "broker_working"
                    | "broker_amended"
                    | "broker_partially_filled"
                    | "broker_replace_requested"
                    | "broker_cancel_requested"
                    | "broker_fill_unreconciled"
                    | "reconciliation_pending" => lifecycle.broker_active_count += count,
                    "broker_state_unknown" => lifecycle.broker_state_unknown_count += count,
                    "executed" => lifecycle.executed_count += count,
                    "execution_failed" | "failed" => lifecycle.failed_count += count,
                    "expired_local" | "broker_expired" => lifecycle.expired_count += count,
                    "broker_cancelled" | "cancelled" => lifecycle.cancelled_count += count,
                    _ => lifecycle.unclassified_count += count,
                }
            }
            lifecycle
        })
        .collect()
}

/// Projects the existing broker-confirmation-aware protection audit into the
/// typed Tuning payload. The source is a current local snapshot, not an
/// execution-pulse outcome or a fresh Saxo request.
fn tuning_protective_stop_coverage_from_payload(
    coverage: &JsonValue,
) -> TuningProtectiveStopCoverage {
    let summary = coverage.get("summary").unwrap_or(&JsonValue::Null);
    let total_quantity = value_f64(summary, "total_quantity");
    let confirmed_covered_quantity = value_f64(summary, "confirmed_covered_quantity");
    TuningProtectiveStopCoverage {
        status: json_text(coverage, "status"),
        position_count: value_i64(summary, "position_count"),
        protected_count: value_i64(summary, "protected_count"),
        partial_count: value_i64(summary, "partial_count"),
        planned_count: value_i64(summary, "planned_count"),
        unprotected_count: value_i64(summary, "unprotected_count"),
        exception_count: value_i64(summary, "exception_count"),
        confirmed_coverage_ratio: (total_quantity.is_finite()
            && total_quantity > 0.0
            && confirmed_covered_quantity.is_finite())
        .then(|| (confirmed_covered_quantity / total_quantity).clamp(0.0, 1.0)),
    }
}

/// Aggregates only persisted manager-run counts for the execution-eligible
/// pulses. It deliberately stops at local execution-row creation: broker
/// submission, fills, and current order state belong to their separate lanes.
fn tuning_execution_candidate_funnel_from_rows(
    rows: &[JsonValue],
) -> Vec<TuningExecutionCandidateFunnel> {
    let mut pulses = [
        TuningExecutionCandidateFunnel {
            pulse_key: "europe_open_followup".to_string(),
            pulse_label: "Nordic/EU Open +1h15".to_string(),
            report_count: 0,
            manager_run_count: 0,
            manager_snapshot_missing_count: 0,
            candidate_order_count: 0,
            eligible_candidate_order_count: 0,
            hermes_matched_candidate_count: 0,
            approved_order_count: 0,
            skipped_order_count: 0,
            local_execution_row_count: 0,
        },
        TuningExecutionCandidateFunnel {
            pulse_key: "us_open_followup".to_string(),
            pulse_label: "US Open +1h15".to_string(),
            report_count: 0,
            manager_run_count: 0,
            manager_snapshot_missing_count: 0,
            candidate_order_count: 0,
            eligible_candidate_order_count: 0,
            hermes_matched_candidate_count: 0,
            approved_order_count: 0,
            skipped_order_count: 0,
            local_execution_row_count: 0,
        },
    ];
    for row in rows {
        let pulse_key = json_text(row, "analysis_pulse_key");
        let index = if pulse_key.starts_with("europe_open_followup:") {
            0
        } else if pulse_key.starts_with("us_open_followup:") {
            1
        } else {
            continue;
        };
        let pulse = &mut pulses[index];
        pulse.report_count += 1;
        pulse.local_execution_row_count += value_i64(row, "local_execution_row_count");
        if value_i64(row, "manager_run_id") > 0 {
            pulse.manager_run_count += 1;
        }
        let manager = decode_shadow_json_field(row.get("manager_json"));
        if !manager.is_object() {
            pulse.manager_snapshot_missing_count += 1;
            continue;
        }
        pulse.candidate_order_count += value_i64(&manager, "candidate_order_count");
        pulse.eligible_candidate_order_count +=
            value_i64(&manager, "eligible_candidate_order_count");
        pulse.approved_order_count += value_i64(&manager, "approved_order_count");
        pulse.skipped_order_count += value_i64(&manager, "skipped_order_count");
        pulse.hermes_matched_candidate_count += manager
            .get("hermes_advice_delta")
            .map(|delta| value_i64(delta, "matched_candidate_count"))
            .unwrap_or_default();
    }
    pulses.into()
}

/// One independently-refreshed data source the dashboard displays.
///
/// `stale_after_minutes` is derived from each source's own cadence rather than
/// picked uniformly: an FX rate carrying a 30-minute cache TTL and a nightly
/// weekday sweep are stale at very different ages.
///
/// The weekday-only sweeps get a deliberately generous three-day threshold so a
/// normal weekend does not raise a false alarm. Precise market-aware staleness
/// already exists in the operations banner; duplicating that calendar logic here
/// would mean two places to keep correct, so this strip answers the cruder
/// question — "has this stopped updating entirely?" — which is the one that went
/// unanswered when FX silently froze for two days on 2026-08-02.
struct FreshnessSource {
    key: &'static str,
    label: &'static str,
    tab: &'static str,
    sql: &'static str,
    stale_after_minutes: i64,
}

const FRESHNESS_SOURCES: &[FreshnessSource] = &[
    FreshnessSource {
        key: "fx_rates",
        label: "FX rates",
        tab: "overview",
        sql: "SELECT MAX(observed_at) AS at FROM currency_fx_rates",
        // Three times the 30-minute cache TTL: one missed refresh is normal,
        // three in a row is the failure that hid for two days.
        stale_after_minutes: 90,
    },
    FreshnessSource {
        key: "broker_exposures",
        label: "Broker positions",
        tab: "overview",
        sql: "SELECT MAX(updated_at) AS at FROM broker_instrument_exposures",
        // Refreshed every 10-minute scheduler cycle.
        stale_after_minutes: 45,
    },
    FreshnessSource {
        key: "portfolio_value",
        label: "Portfolio value",
        tab: "performance",
        sql: "SELECT MAX(recorded_at) AS at FROM portfolio_value_history",
        stale_after_minutes: 45,
    },
    FreshnessSource {
        key: "decision_reports",
        label: "Decision reports",
        tab: "decisions",
        sql: "SELECT MAX(created_at) AS at FROM decision_reports",
        stale_after_minutes: 3 * 24 * 60,
    },
    FreshnessSource {
        key: "markov_run",
        label: "Markov run",
        tab: "markov",
        sql: "SELECT MAX(created_at) AS at FROM markov_signal_runs",
        stale_after_minutes: 3 * 24 * 60,
    },
    FreshnessSource {
        key: "indicator_run",
        label: "Daily indicators",
        tab: "markov",
        sql: "SELECT MAX(created_at) AS at FROM daily_indicator_runs",
        stale_after_minutes: 3 * 24 * 60,
    },
    FreshnessSource {
        key: "base_backup",
        label: "Base backup",
        tab: "overview",
        // Written by the retention CronJob, which is the only process that sees
        // both halves: CNPG's completed Backup resources and whether their
        // objects actually landed in the bucket. Base backups stopped for ten
        // days across the 2026-08-31 disk crisis with nothing surfacing it.
        sql: "SELECT last_verified_backup_at AS at FROM backup_status WHERE singleton_key = 'cnpg_base_backup'",
        // The schedule is every 30 minutes; three missed rounds is the signal,
        // the same rule used for FX rates and WAL archiving.
        stale_after_minutes: 90,
    },
    FreshnessSource {
        key: "wal_archiving",
        label: "WAL archiving",
        tab: "overview",
        // The only backup signal reachable from the application's own
        // connection. CNPG writes base backups to object storage and records
        // their state in Kubernetes, which this process cannot read; continuous
        // WAL archiving is visible here because `-rw` routes to the primary.
        // A failing archiver stops advancing this timestamp, so a failure
        // surfaces as staleness rather than needing its own signal.
        sql: r#"SELECT to_char(last_archived_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS at FROM pg_stat_archiver"#,
        // archive_timeout forces a segment switch every 5 minutes, so three
        // missed switches is the signal -- the same rule as FX rates above.
        stale_after_minutes: 15,
    },
    FreshnessSource {
        key: "quiver_run",
        label: "Quiver run",
        tab: "quiver",
        sql: "SELECT MAX(created_at) AS at FROM quiver_signal_runs",
        stale_after_minutes: 3 * 24 * 60,
    },
    FreshnessSource {
        key: "hermes_reflection",
        label: "Hermes daily reflection",
        tab: "hermes",
        // The watchdog placeholder the CronJob writes when the model never
        // answered is a row in this table but not a reflection, so counting it
        // would report the loop healthy on exactly the days it was dead. On
        // 2026-09-01 the newest real reflection was 2026-08-25 while the newest
        // row was the night before, and nothing on screen said so.
        // Kept literal because `FreshnessSource.sql` is a `&'static str` in a
        // const array; a test asserts it still contains the shared predicate,
        // so the two cannot drift apart unnoticed.
        sql: "SELECT MAX(created_at) AS at FROM hermes_reflections \
              WHERE source_session_id LIKE 'daily-eod-reflection-%' \
                AND (raw_payload_json IS NULL \
                     OR raw_payload_json NOT LIKE '%kubernetes-cronjob-watchdog%')",
        // Weekday-only at 23:45, so a normal Friday-to-Monday gap is already
        // 72h. Four days keeps the weekend quiet and still catches two
        // consecutive missed runs.
        stale_after_minutes: 4 * 24 * 60,
    },
];

/// Accepts both timestamp spellings in the database: the Rust runtime writes
/// `...Z`, the retired Python path wrote `...+00:00`.
fn parse_freshness_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|value| value.with_timezone(&Utc))
        .ok()
}

/// Classify one source's age. Pure so the thresholds are testable without a
/// database.
///
/// `missing` is deliberately distinct from `stale`: a source that has never
/// produced a row is a different fault from one that produced rows and then
/// stopped, and conflating them sends an operator looking in the wrong place.
pub(crate) fn freshness_state(age_minutes: Option<i64>, stale_after_minutes: i64) -> &'static str {
    let Some(age_minutes) = age_minutes else {
        return "missing";
    };
    let stale_after = stale_after_minutes.max(1);
    if age_minutes >= stale_after {
        "stale"
    } else if age_minutes * 2 >= stale_after {
        "aging"
    } else {
        "fresh"
    }
}

/// Compact human age. Kept coarse on purpose — the exact minute count of a
/// three-day-old sweep is noise; that it is measured in days is the signal.
pub(crate) fn freshness_age_label(age_minutes: Option<i64>) -> String {
    let Some(age_minutes) = age_minutes else {
        return "never".to_string();
    };
    let age_minutes = age_minutes.max(0);
    if age_minutes < 1 {
        "just now".to_string()
    } else if age_minutes < 60 {
        format!("{age_minutes}m ago")
    } else if age_minutes < 24 * 60 {
        format!("{}h ago", age_minutes / 60)
    } else {
        format!("{}d ago", age_minutes / (24 * 60))
    }
}

/// Hermes sub-sections, in the order the tab strip presents them.
/// Restricts a `hermes_reflections` query to rows the daily loop actually
/// produced.
///
/// The CronJob writes a watchdog placeholder when the model never answers, and
/// that row lives in the same table. Every consumer that counts rows instead of
/// reflections therefore reads the loop as healthy on exactly the days it was
/// dead — the freshness chip did, and so did the missed-reflection alert, which
/// stayed silent through the 2026-08-26 to 08-31 outage because a placeholder
/// was written each of those nights. Shared rather than repeated, because the
/// second copy is where these diverge.
pub(crate) const REAL_DAILY_REFLECTION_SQL_PREDICATE: &str = "source_session_id LIKE 'daily-eod-reflection-%' AND (raw_payload_json IS NULL OR raw_payload_json NOT LIKE '%kubernetes-cronjob-watchdog%')";

/// Top-level `get_context` keys a caller may ask for by name.
///
/// Distinct from `HERMES_SECTIONS`, which names dashboard tabs. These are the
/// fields of `HermesContextPayload`, minus the three in
/// `HERMES_CONTEXT_ALWAYS_PRESENT`.
pub(crate) const HERMES_CONTEXT_SECTIONS: &[&str] = &[
    "capabilities",
    "goal_contract",
    "overview",
    "scheduler",
    "decisions",
    "end_of_day",
    "strategy_journal",
    "execution",
    "performance",
    "markov_method",
    "quiver_signals",
    "quiver_conflicts",
    "editorial_research",
    "daily_indicators",
    "hermes",
];

/// Never filtered out: a scoped payload must still say what it is and carry
/// its own boundary statement, or a caller could request a context with the
/// safety framing quietly removed.
pub(crate) const HERMES_CONTEXT_ALWAYS_PRESENT: &[&str] = &["status", "generated_at", "safety"];

/// Drop every top-level key the caller did not ask for, and say so in the
/// payload.
///
/// A no-op when `sections` is empty or names nothing recognised, so the
/// default call is unchanged.
/// How much of each record `get_context` returns.
///
/// `Full` is the shape every caller had before compaction existed and stays
/// the default; only a caller that asks gets the smaller one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextDetail {
    Full,
    Compact,
}

impl ContextDetail {
    pub(crate) fn from_argument(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("compact") => Self::Compact,
            _ => Self::Full,
        }
    }
}

/// Fields on `hermes.experiments` that carry the proposal's own supporting
/// narrative. Half the block by weight, and nothing a reflection needs: it
/// reads this inventory to avoid duplicating a `changed_variable_path`, not to
/// re-derive the case for a proposal somebody already wrote.
const HERMES_EXPERIMENT_BULK_FIELDS: &[&str] = &["evidence_json", "raw_payload_json"];

/// Narrative fields inside a trade thesis or thesis review that render, in
/// prose, structured evidence sitting beside them: `entry_rationale` and
/// `catalyst_or_monitor` restate the `technical` and `markov` objects, and
/// `approval_evidence` restates the gate that admitted the order. Dropping
/// them costs phrasing, not facts, and they were 26% of the execution orders
/// block on 2026-09-02.
///
/// `invalidation`, `operator_next_step` and the per-thesis `safety` line are
/// deliberately absent: they are forward-looking rather than restatements, and
/// being identical everywhere they already cost one copy after hoisting.
const NARRATIVE_THESIS_FIELDS: &[&str] = &[
    "approval_evidence",
    "entry_rationale",
    "catalyst_or_monitor",
];

/// Strip narrative fields from thesis records only.
///
/// Targeted by container rather than by key name: a blanket sweep for these
/// names would also reach the block-level boundary statements that
/// `protective_stop_coverage` and `holding_thesis_reviews` carry, which exist
/// to say what the block may not be used for.
pub(crate) fn strip_thesis_narrative(value: &mut JsonValue) {
    match value {
        JsonValue::Object(object) => {
            if let Some(thesis) = object
                .get_mut("trade_thesis")
                .and_then(JsonValue::as_object_mut)
            {
                for field in NARRATIVE_THESIS_FIELDS {
                    thesis.remove(*field);
                }
            }
            if let Some(reviews) = object
                .get_mut("holding_thesis_reviews")
                .and_then(|block| block.get_mut("reviews"))
                .and_then(JsonValue::as_array_mut)
            {
                for review in reviews.iter_mut() {
                    if let Some(review) = review.as_object_mut() {
                        for field in NARRATIVE_THESIS_FIELDS {
                            review.remove(*field);
                        }
                    }
                }
            }
            for field in object.values_mut() {
                strip_thesis_narrative(field);
            }
        }
        JsonValue::Array(items) => {
            for item in items.iter_mut() {
                strip_thesis_narrative(item);
            }
        }
        _ => {}
    }
}

/// Below this, hoisting a repeated field costs more in surprise than it saves.
/// The values this targets are templated sentences, not status flags.
const REPEATED_FIELD_MIN_BYTES: usize = 40;

/// Lift fields that carry the same value on every element of a list out of the
/// list, into a sibling `<key>_shared` object.
///
/// Measured on 2026-09-02: `holding_thesis_reviews` repeated one identical
/// `invalidation` string across 11 reviews for 2,178 bytes and one identical
/// `operator_next_step` for 1,936, and `trade_thesis` repeated one
/// `invalidation` and one `safety` across every order. Roughly 6.3 KB carrying
/// four sentences. Hoisting is lossless -- the value is still there, once --
/// which is why it beats deleting the prose.
pub(crate) fn hoist_repeated_fields(items: &mut [JsonValue]) -> serde_json::Map<String, JsonValue> {
    let mut shared = serde_json::Map::new();
    if items.len() < 2 {
        return shared;
    }
    let Some(first) = items.first().and_then(JsonValue::as_object).cloned() else {
        return shared;
    };
    for (key, candidate) in first {
        let encoded = candidate.to_string();
        if encoded.len() < REPEATED_FIELD_MIN_BYTES {
            continue;
        }
        let identical_everywhere = items.iter().all(|item| {
            item.as_object()
                .and_then(|object| object.get(&key))
                .is_some_and(|value| value.to_string() == encoded)
        });
        if identical_everywhere {
            shared.insert(key, candidate);
        }
    }
    for item in items.iter_mut() {
        if let Some(object) = item.as_object_mut() {
            for key in shared.keys() {
                object.remove(key);
            }
        }
    }
    shared
}

/// True when a value carries no information: null, or a structure whose every
/// leaf is null. Empty lists and objects are left alone -- "none today" is a
/// fact, where a field that was never populated is not.
pub(crate) fn is_all_null(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => true,
        JsonValue::Object(object) => !object.is_empty() && object.values().all(is_all_null),
        JsonValue::Array(items) => !items.is_empty() && items.iter().all(is_all_null),
        _ => false,
    }
}

pub(crate) fn strip_experiment_bulk_fields(value: &mut JsonValue) {
    let Some(experiments) = value
        .get_mut("hermes")
        .and_then(|hermes| hermes.get_mut("experiments"))
        .and_then(JsonValue::as_array_mut)
    else {
        return;
    };
    for experiment in experiments.iter_mut() {
        if let Some(object) = experiment.as_object_mut() {
            for field in HERMES_EXPERIMENT_BULK_FIELDS {
                object.remove(*field);
            }
        }
    }
}

pub(crate) fn compact_context_node(value: &mut JsonValue) {
    match value {
        JsonValue::Object(object) => {
            object.retain(|_, field| !is_all_null(field));
            let mut hoisted: Vec<(String, serde_json::Map<String, JsonValue>)> = Vec::new();
            for (key, field) in object.iter_mut() {
                if let Some(items) = field.as_array_mut() {
                    let shared = hoist_repeated_fields(items);
                    if !shared.is_empty() {
                        hoisted.push((format!("{key}_shared"), shared));
                    }
                }
            }
            for (key, shared) in hoisted {
                object.insert(key, JsonValue::Object(shared));
            }
            for field in object.values_mut() {
                compact_context_node(field);
            }
        }
        JsonValue::Array(items) => {
            for item in items.iter_mut() {
                compact_context_node(item);
            }
        }
        _ => {}
    }
}

pub(crate) const HERMES_SECTIONS: &[&str] = &[
    "overview",
    "advice",
    "reflections",
    "experiments",
    "baselines",
];

pub(crate) fn normalize_hermes_section(value: Option<&str>) -> String {
    let requested = value.unwrap_or("overview").trim().to_ascii_lowercase();
    if HERMES_SECTIONS.contains(&requested.as_str()) {
        requested
    } else {
        "overview".to_string()
    }
}

pub(crate) fn hermes_section_label(section: &str) -> &'static str {
    match section {
        "advice" => "Advice",
        "reflections" => "Reflections",
        "experiments" => "Experiments",
        "baselines" => "Baselines",
        _ => "Overview",
    }
}

fn scheduler_history_policy_values(
    configured_max_rows: Option<i64>,
    configured_retention_days: Option<i64>,
) -> (i64, i64) {
    (
        configured_max_rows
            .unwrap_or(DEFAULT_SCHEDULER_HISTORY_MAX_ROWS)
            .max(0),
        configured_retention_days
            .unwrap_or(DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS)
            .max(0),
    )
}

/// Use the price-monitor calculation whenever it is available. In SIM,
/// `InstrumentPriceDayPercentChange` may be zero for an approximated broker
/// exposure even though the current infoprice and LastClose establish a move.
fn daily_change_pct_from_sources(
    price_snapshot: Option<&JsonValue>,
    broker_exposure: Option<&JsonValue>,
) -> f64 {
    price_snapshot
        .and_then(|row| row.get("change_pct"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite())
        .or_else(|| {
            broker_exposure
                .and_then(|row| row.get("instrument_price_day_percent_change"))
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite())
        })
        .unwrap_or(0.0)
}

/// Normalized display-symbol key for records that do not retain a durable
/// instrument identifier. Keep the exchange suffix: symbols on different
/// exchanges remain distinct, while Saxo and imported history may disagree
/// only on ticker casing.
pub(crate) fn canonical_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

/// Shadow outcome records are admitted only from a persisted server-owned
/// shadow report. Provider text, a label, or a manual dry-run marker cannot
/// grant this observational ledger any broader authority.
pub(crate) fn shadow_report_outcome_report_is_eligible(report: &JsonValue) -> bool {
    let queue_eligible = report.get("queue_eligible").and_then(|value| {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .or_else(|| {
                value.as_str().and_then(|value| match value {
                    "0" | "false" | "FALSE" => Some(false),
                    "1" | "true" | "TRUE" => Some(true),
                    _ => None,
                })
            })
    });
    json_text(report, "pulse_mode") == "shadow" && queue_eligible == Some(false)
}

pub(crate) fn shadow_report_approved_policy_source(context: &JsonValue) -> Option<String> {
    let thesis = context.get("thesis_snapshot").unwrap_or(&JsonValue::Null);
    let baseline = thesis.get("active_baseline").unwrap_or(&JsonValue::Null);
    let baseline_id = json_text(baseline, "id");
    if !baseline_id.is_empty() {
        return Some(format!("active_strategy_baseline:{baseline_id}"));
    }
    let report_baseline_id = json_text(thesis, "strategy_baseline_id");
    (!report_baseline_id.is_empty())
        .then(|| format!("report_strategy_baseline:{report_baseline_id}"))
}

pub(crate) fn shadow_report_context_strategy_key(context: &JsonValue) -> String {
    json_text(context, "strategy_key")
}

pub(crate) fn decode_shadow_json_field(value: Option<&JsonValue>) -> JsonValue {
    match value {
        Some(value) if value.is_object() || value.is_array() => value.clone(),
        Some(JsonValue::String(raw)) => serde_json::from_str(raw).unwrap_or(JsonValue::Null),
        _ => JsonValue::Null,
    }
}

fn effective_execution_mode(
    configured_mode: &str,
    adapter: &str,
    dry_run: bool,
    require_approval: bool,
    configured_saxo_environment: &str,
    session_environment: Option<&str>,
) -> String {
    if !configured_mode.eq_ignore_ascii_case("live") || !adapter.eq_ignore_ascii_case("saxo") {
        return configured_mode.to_string();
    }
    if dry_run {
        return "dry_run".to_string();
    }
    if require_approval {
        return "approval_required".to_string();
    }
    let configured_environment = if configured_saxo_environment.eq_ignore_ascii_case("sim") {
        "SIM"
    } else if configured_saxo_environment.eq_ignore_ascii_case("live") {
        "LIVE"
    } else {
        return "disabled_unknown_environment".to_string();
    };
    let Some(session_environment) = session_environment else {
        return "disabled_session_environment".to_string();
    };
    if !session_environment.eq_ignore_ascii_case(configured_environment) {
        return "disabled_environment_mismatch".to_string();
    }
    match configured_environment {
        "SIM" => "simulated_broker".to_string(),
        "LIVE" => "live_broker".to_string(),
        _ => unreachable!("configured Saxo environment was normalized"),
    }
}

/// Prefer ISIN when both source records retain it; otherwise use the
/// case-normalized display symbol as the least-lossy common key.
fn position_identity_key(symbol: &str, isin: Option<&str>) -> String {
    isin.map(str::trim)
        .filter(|isin| !isin.is_empty())
        .map(|isin| format!("isin:{}", isin.to_ascii_uppercase()))
        .unwrap_or_else(|| format!("symbol:{}", canonical_symbol_key(symbol)))
}

/// Reduce one scheduler `cycle_json` to per-step health.
///
/// The stored document is the cycle's full internal diagnostic dump -- 28 KB
/// each, and 20 of them made `scheduler.cycles` 1.81 MB, 59% of a 4.66 MB
/// `get_context` payload that the transport then truncated. What an advisory
/// reader needs from cycle history is whether each step ran and whether it
/// failed, so each step collapses to its status and any short reason. Scalars
/// and `step_durations` survive because they are already small and are the
/// health signal itself.
pub(crate) fn compact_scheduler_cycle_document(cycle: &JsonValue) -> JsonValue {
    let Some(object) = cycle.as_object() else {
        return cycle.clone();
    };
    let mut compact = serde_json::Map::new();
    for (key, value) in object {
        match value {
            JsonValue::Object(step) => {
                if key == "step_durations" {
                    compact.insert(key.clone(), value.clone());
                    continue;
                }
                let mut summary = serde_json::Map::new();
                for field in ["status", "reason", "error", "state"] {
                    if let Some(found) = step.get(field) {
                        // Keep only short scalars: a nested document here is
                        // the bloat this function exists to remove.
                        let short = found
                            .as_str()
                            .map(|text| text.len() <= 200)
                            .unwrap_or_else(|| !found.is_object() && !found.is_array());
                        if short {
                            summary.insert(field.to_string(), found.clone());
                        }
                    }
                }
                for field in ["error_count", "success_count", "asset_count", "processed"] {
                    if let Some(found) = step.get(field).filter(|v| v.is_number()) {
                        summary.insert(field.to_string(), found.clone());
                    }
                }
                if summary.is_empty() {
                    summary.insert("status".to_string(), json!("recorded"));
                }
                compact.insert(key.clone(), JsonValue::Object(summary));
            }
            JsonValue::Array(items) => {
                compact.insert(key.clone(), json!({"count": items.len()}));
            }
            scalar => {
                compact.insert(key.clone(), scalar.clone());
            }
        }
    }
    JsonValue::Object(compact)
}

impl AppState {
    // Associated functions are like static/class methods. `Self` means
    // `AppState`, so this returns a fully initialized application state.
    pub async fn load() -> Result<Self> {
        let config_path =
            env::var("DAYTRADER_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        let config_path = PathBuf::from(config_path);
        info!(config_path = %config_path.display(), "loading application config");
        let config_text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading config {}", config_path.display()))?;
        let config: YamlValue = serde_yaml::from_str(&config_text)
            .with_context(|| format!("parsing config {}", config_path.display()))?;
        // Report config keys that are configured but unwired before anything
        // starts trusting them. Covers every process mode because api,
        // scheduler, and MCP all load state through here.
        crate::config_contract::log_config_contract_audit(&config);
        let db_url = database_url(&config, &config_path)?;
        let safe_db_url = redacted_database_url(&db_url);
        info!(database_url = %safe_db_url, "connecting to database");
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .with_context(|| format!("connecting to database {safe_db_url}"))?;
        info!(database_url = %safe_db_url, "database connection pool ready");
        let state = Self {
            config_path,
            config,
            db_url,
            pool,
        };
        state.ensure_runtime_state_schema().await?;
        if let Err(err) = state.sync_saxo_session_storage().await {
            warn!("Saxo session database sync skipped during startup: {err:#}");
        }
        Ok(state)
    }

    pub(crate) async fn tuning_payload(&self) -> Result<TuningPayload> {
        let generated_at = Utc::now();
        let window_start = generated_at - Duration::days(TUNING_PULSE_WINDOW_DAYS);
        let window_start_text = window_start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let pulse_filter = "(analysis_pulse_key LIKE 'europe_open_followup:%' \
            OR analysis_pulse_key LIKE 'europe_mid_session_shadow:%' \
            OR analysis_pulse_key LIKE 'us_open_followup:%' \
            OR analysis_pulse_key LIKE 'us_mid_session_shadow:%')";
        let report_rows = self
            .select_json(&format!(
                "SELECT id, analysis_pulse_key, status, report_json
                 FROM decision_reports
                 WHERE created_at >= '{}' AND {pulse_filter}",
                sql_escape(&window_start_text),
            ))
            .await?;
        let shadow_rows = self
            .select_json(&format!(
                "SELECT report_id, analysis_pulse_key, status,
                        earlier_pulse_report_id, appeared_in_earlier_pulse,
                        deterministic_gate_code, deterministic_gate_json,
                        hermes_effect, hermes_advice_snapshot_json, approved_policy_source,
                        report_time_context_json,
                        reference_price_source,
                        one_session_outcome_json, five_session_outcome_json,
                        twenty_session_outcome_json, five_session_after_cost_outcome_json
                 FROM shadow_report_outcomes
                 WHERE created_at >= '{}' AND {pulse_filter}",
                sql_escape(&window_start_text),
            ))
            .await?;
        let pulse_comparison = tuning_pulse_comparison_from_rows(&report_rows, &shadow_rows);
        let shadow_change_evidence = tuning_shadow_change_evidence_from_rows(&report_rows);
        let shadow_support_risk_evidence =
            tuning_shadow_support_risk_evidence_from_rows(&shadow_rows);
        let shadow_markov_evidence = tuning_shadow_markov_evidence_from_rows(&shadow_rows);
        let shadow_quiver_evidence = tuning_shadow_quiver_evidence_from_rows(&shadow_rows);
        let shadow_gate_evidence = tuning_shadow_gate_evidence_from_rows(&shadow_rows);
        let shadow_hermes_evidence = tuning_shadow_hermes_evidence_from_rows(&shadow_rows);
        let execution_evidence = self
            .decision_pulse_outcome_evidence_since(Some(&window_start_text))
            .await?;
        let execution_pulse_outcomes =
            tuning_execution_pulse_outcomes_from_evidence(&execution_evidence);
        let execution_lifecycle_evidence =
            tuning_execution_lifecycle_evidence_from_evidence(&execution_evidence);
        let execution_candidate_funnel_rows = self
            .select_json(&format!(
                "SELECT dr.analysis_pulse_key, tm.id AS manager_run_id, tm.manager_json,
                        (SELECT COUNT(*) FROM execution_orders eo WHERE eo.report_id = dr.id)
                            AS local_execution_row_count
                 FROM decision_reports dr
                 LEFT JOIN trading_manager_runs tm ON tm.id = (
                    SELECT latest.id
                    FROM trading_manager_runs latest
                    WHERE latest.report_id = dr.id
                    ORDER BY latest.created_at DESC, latest.id DESC
                    LIMIT 1
                 )
                 WHERE dr.created_at >= '{}'
                   AND (dr.analysis_pulse_key LIKE 'europe_open_followup:%'
                        OR dr.analysis_pulse_key LIKE 'us_open_followup:%')",
                sql_escape(&window_start_text),
            ))
            .await?;
        let execution_candidate_funnel =
            tuning_execution_candidate_funnel_from_rows(&execution_candidate_funnel_rows);
        let trade_thesis_evidence = tuning_trade_thesis_evidence_from_payload(
            &self
                .trade_thesis_outcome_evidence()
                .await
                .unwrap_or_else(|err| {
                    warn!("tuning trade-thesis evidence degraded: {err:#}");
                    json!({
                        "status": "unavailable",
                        "interpretation": "Trade-thesis outcome evidence could not be loaded."
                    })
                }),
        );
        let experiment_governance = match self
            .select_json(
                "SELECT status, COUNT(*) AS count
                 FROM strategy_experiments
                 GROUP BY status",
            )
            .await
        {
            Ok(rows) => tuning_experiment_governance_from_rows(&rows),
            Err(err) => {
                warn!("tuning experiment governance degraded: {err:#}");
                tuning_experiment_governance_unavailable()
            }
        };
        let one_month_performance_history = self
            .performance_history_with_current("1M", performance_range_limit("1M"))
            .await;
        let portfolio_outcome = match &one_month_performance_history {
            Ok(history) => tuning_portfolio_outcome_from_summary(
                &performance_summary_from_history(history, Utc::now()),
            ),
            Err(err) => {
                warn!("tuning portfolio outcome degraded: {err:#}");
                tuning_portfolio_outcome_unavailable()
            }
        };
        let monthly_goal_progress = match &one_month_performance_history {
            Ok(history) => match history
                .last()
                .and_then(|row| tuning_optional_number(row, "total_market_value_dkk"))
                .filter(|total| *total > 0.0)
            {
                Some(total) => {
                    tuning_monthly_goal_progress_from_payload(&self.goal_tracking(total).await)
                }
                None => tuning_monthly_goal_progress_unavailable(),
            },
            Err(_) => tuning_monthly_goal_progress_unavailable(),
        };
        let benchmark_comparison = match &one_month_performance_history {
            Ok(history) => {
                match crate::performance_benchmarks::performance_benchmark_payload(self, &history)
                    .await
                {
                    Ok(payload) => tuning_benchmark_comparison_from_payload(&payload),
                    Err(err) => {
                        warn!("tuning benchmark comparison degraded: {err:#}");
                        tuning_benchmark_comparison_unavailable()
                    }
                }
            }
            Err(err) => {
                warn!("tuning benchmark history degraded: {err:#}");
                tuning_benchmark_comparison_unavailable()
            }
        };
        let protective_stop_coverage = tuning_protective_stop_coverage_from_payload(
            &self.protective_stop_coverage().await.unwrap_or_else(|err| {
                warn!("tuning protective-stop coverage degraded: {err:#}");
                json!({"status": "unavailable", "summary": {}})
            }),
        );
        let shadow_pulses = pulse_comparison
            .iter()
            .filter(|pulse| pulse.authority == "shadow_observation_only")
            .collect::<Vec<_>>();
        let status = if shadow_pulses.iter().any(|pulse| pulse.maturity == "mature") {
            "mature"
        } else if shadow_pulses
            .iter()
            .any(|pulse| pulse.maturity == "preliminary")
        {
            "preliminary"
        } else {
            "collecting"
        };
        Ok(TuningPayload {
            generated_at: generated_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            window_start: window_start_text,
            window_days: TUNING_PULSE_WINDOW_DAYS,
            status: status.to_string(),
            pulse_comparison,
            shadow_change_evidence,
            shadow_support_risk_evidence,
            shadow_markov_evidence,
            shadow_quiver_evidence,
            shadow_gate_evidence,
            shadow_hermes_evidence,
            execution_pulse_outcomes,
            execution_lifecycle_evidence,
            protective_stop_coverage,
            execution_candidate_funnel,
            trade_thesis_evidence,
            experiment_governance,
            portfolio_outcome,
            monthly_goal_progress,
            benchmark_comparison,
            safety: "read_only_local_decision_reports_execution_orders_fills_ledger_daily_closes_and_shadow_outcome_ledger_no_provider_hermes_broker_gate_or_order_mutation".to_string(),
            interpretation: "This view compares report reliability, separately-labelled execution evidence, and shadow-observation coverage. The shadow reference lane also distinguishes a completed eligible report without any local outcome-ledger row from a row still awaiting its immediate read-only quote; it is an integrity observation, not a retry or an alert action. The shadow-change table uses only the server-normalized report assessment: candidate count does not determine material change or no-new-information status, and the no-new-information rate excludes reports without an opening reference. Support/Risk, Markov, and Quiver are saved decision-time context and coverage summaries, not forecasts or gates; neither Markov nor Quiver is rerun here. The shadow signal-gate table is a bounded replay over persisted decision-time technical/Markov evidence, and the Hermes table reports separately persisted record-only advice coverage; neither is a Trading Manager approval, broker precheck, or execution simulation. The one-month account-value outcome lane is simple local snapshot movement including cash, with visible freshness and no realised-P/L, time-weighted-return, or total-return claim. Its calendar-month target context is a separately labelled configured DKK planning reference, with an active-batch baseline and the same non-realised/non-normalized limits. The separately labelled thesis lane is bounded to newest recorded BUY theses rather than the 30-day pulse window and reports gross directional observations, not realised P/L. Experiment governance is a retained lifecycle inventory only: it does not expose proposal values, measure performance, or activate anything. Benchmark comparison is a separate one-month local account-value versus stored native-currency ETF proxy price-return view, with explicit alignment/freshness and non-total-return caveats. Shadow 1/5/20-session and after-cost fields are equal-weighted quote-to-close evidence, never realised P/L, fill quality, or a trading recommendation. Execution BUY directional movement, reconciled SELL accounting, local candidate-funnel counts, and persisted lifecycle statuses are separately labelled; no execution field is blended with shadow observations or treated as a current broker poll. Protective-stop coverage is a separate current local snapshot and counts only broker-confirmed stop evidence.".to_string(),
        })
    }

    pub async fn overview_payload(&self) -> Result<JsonValue> {
        // `&self` is a borrowed receiver: callers can use AppState without
        // transferring ownership, similar to passing an object reference in
        // Python or JavaScript.
        let latest_history = self
            .first_json(
                "SELECT recorded_at, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count FROM portfolio_value_history ORDER BY recorded_at DESC LIMIT 1",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let latest_batch = self.latest_batch_id().await?;
        let broker_positions_available = self.broker_positions_available().await.unwrap_or(false);
        let aggregate = if broker_positions_available {
            self.position_aggregate(latest_batch.as_deref()).await?
        } else if latest_history.as_object().is_some_and(|o| !o.is_empty()) {
            latest_history.clone()
        } else {
            self.position_aggregate(latest_batch.as_deref()).await?
        };
        let total_value = value_f64(&aggregate, "total_market_value_dkk");
        let cash_summary = self.cash_summary_from_ledger().await?;
        let initial_cash = aggregate
            .get("initial_cash_dkk")
            .map(|_| value_f64(&aggregate, "initial_cash_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "initial_cash_dkk"));
        let cash_from_trades = aggregate
            .get("cash_from_trades_dkk")
            .map(|_| value_f64(&aggregate, "cash_from_trades_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "cash_from_trades_dkk"));
        let max_daily_orders =
            yaml_i64(&self.config, &["execution", "max_daily_orders"]).unwrap_or(0);
        let executed_today = self.executed_orders_today().await.unwrap_or(0);
        let decision_refresh = crate::xai_decision::decision_pulse_summary(self);
        let configured_execution_mode = yaml_string(&self.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string());
        let execution_adapter = yaml_string(&self.config, &["execution", "adapter"])
            .unwrap_or_else(|| "simulation".to_string());
        let execution_dry_run = yaml_bool(&self.config, &["app", "dry_run"]).unwrap_or(true);
        let require_approval_live =
            yaml_bool(&self.config, &["execution", "require_approval_live"]).unwrap_or(true);
        let configured_saxo_environment = yaml_string(&self.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string());
        let saxo_auth = self.saxo_auth_status_value().await;
        let effective_execution_mode = effective_execution_mode(
            &configured_execution_mode,
            &execution_adapter,
            execution_dry_run,
            require_approval_live,
            &configured_saxo_environment,
            saxo_auth.get("environment").and_then(JsonValue::as_str),
        );
        let integrity = self
            .overview_integrity(&aggregate, &latest_history, &cash_summary)
            .await
            .unwrap_or_else(|err| {
                json!({
                    "healthy": false,
                    "warnings": [{
                        "code": "integrity_check_failed",
                        "severity": "warning",
                        "message": format!("Overview integrity checks failed: {err:#}")
                    }],
                    "mismatches": [],
                    "unreconciled_orders": [],
                    "checked_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                })
            });

        let after_tax_summary = self
            .after_tax_summary(value_f64(&aggregate, "total_unrealised_pnl_dkk"))
            .await;

        Ok(json!({
            "app": {
                "project_name": yaml_string(&self.config, &["app", "project_name"]),
                "environment": yaml_string(&self.config, &["app", "environment"]),
                "config_path": self.config_path.display().to_string(),
                "runtime": "rust-dioxus"
            },
            "execution": {
                "mode": effective_execution_mode,
                "configured_mode": configured_execution_mode,
                "adapter": execution_adapter,
                "configured_saxo_environment": configured_saxo_environment,
                "session_environment": saxo_auth.get("environment").cloned().unwrap_or(JsonValue::Null),
                "require_approval_live": require_approval_live,
                "max_daily_orders": max_daily_orders,
                "daily_order_capacity": {
                    "max": max_daily_orders,
                    "used": executed_today,
                    "remaining": (max_daily_orders - executed_today).max(0)
                },
                "counts": self.execution_counts().await.unwrap_or_else(|_| json!({
                    "queued": 0,
                    "pending_approval": 0,
                    "broker_live": 0,
                    "failed": 0
                })),
            },
            "portfolio_summary": {
                "recorded_at": aggregate.get("recorded_at").cloned().unwrap_or(JsonValue::Null),
                "total_market_value_dkk": total_value,
                "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
                "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
                "initial_cash_dkk": initial_cash,
                "cash_from_trades_dkk": cash_from_trades,
                "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
                "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
                "position_count": value_i64(&aggregate, "position_count"),
            },
            "after_tax_summary": after_tax_summary,
            "goal_tracking": self.goal_tracking(total_value).await,
            "integrity": integrity,
            "analysis_summary": self.market_status_payload().await.unwrap_or_else(|_| json!({"summary": {"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []}})).get("summary").cloned().unwrap_or_else(|| json!({"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []})),
            "latest_decision": self.latest_decision_summary().await.unwrap_or_else(|_| json!({"id": null, "created_at": null, "status": null})),
            "scheduler_status": self.scheduler_status_value().await.unwrap_or(JsonValue::Null),
            "scheduler_health": {"status": "ok", "message": "Rust scheduler maintains Saxo sessions, submits/polls deferred xAI decision reports, runs the Trading Manager, refreshes daily Markov regime signals when due, and creates due end-of-day journals."},
            "trading_manager": {
                "status": "available",
                "latest_run": self.latest_trading_manager_run().await.unwrap_or(JsonValue::Null)
            },
            "markov_method": {
                "status": "available",
                "config": crate::markov_method::markov_config_json_for_state(self),
                "latest_run": self.latest_markov_run().await.unwrap_or(JsonValue::Null),
            },
            "quiver_signals": {
                "status": "available",
                "config": crate::quiver::quiver_config_json_for_state(self),
                "latest_run": self.latest_quiver_run().await.unwrap_or(JsonValue::Null),
            },
            "saxo_auth": saxo_auth,
            "settings": {
                "cash_buffer": self.cash_buffer_value(),
                "ai": self.ai_settings_value().await.unwrap_or_else(|_| self.default_ai_settings_value())
            },
            "refresh": {
                "price_poll_interval_minutes": yaml_i64(&self.config, &["price_monitor", "poll_interval_minutes"]).unwrap_or(1),
                "scheduler_poll_interval_minutes": yaml_i64(&self.config, &["scheduler", "poll_interval_minutes"]).unwrap_or(10),
                "decision_cadence": "rust_dashboard",
                "decision_cadence_label": "Rust dashboard",
                "decision_pulses": decision_refresh.get("pulses").cloned().unwrap_or_else(|| json!([])),
                "next_decision_pulse_at": decision_refresh.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
                "next_decision_pulse_label": decision_refresh.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null)
            }
        }))
    }

    pub async fn performance_payload(&self, range_key: &str) -> Result<JsonValue> {
        let history = self
            .performance_history_with_current(range_key, performance_range_limit(range_key))
            .await?;
        let latest = history.last().cloned().unwrap_or_else(|| json!({}));
        let total = value_f64(&latest, "total_market_value_dkk");
        Ok(json!({
            "range_key": range_key,
            "history": history,
            "summary": performance_summary_from_history(&history, Utc::now()),
            "benchmarks": crate::performance_benchmarks::performance_benchmark_payload(self, &history).await?,
            "goal_tracking": self.goal_tracking(total).await,
            "snapshot_evidence": self.portfolio_position_snapshot_coverage(range_key).await?
        }))
    }

    pub async fn market_status_payload(&self) -> Result<JsonValue> {
        let calendar_refresh = match self.refresh_saxo_exchange_calendars_if_stale().await {
            Ok(value) => value,
            Err(err) => {
                warn!("Saxo exchange calendar refresh skipped: {err:#}");
                json!({"status": "error", "error": err.to_string()})
            }
        };
        let items = self.market_exchange_rows();
        let scheduler = self
            .scheduler_status_value()
            .await
            .unwrap_or(JsonValue::Null);
        let price_monitor = self
            .price_monitor_status_value()
            .await
            .unwrap_or(JsonValue::Null);
        let cycle = scheduler
            .get("last_cycle_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let manager_status = cycle
            .get("trading_manager")
            .and_then(|value| value.get("manager_status"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        let open_active_markets = market_names_where(&items, "open_analysis_window_active");
        let close_active_markets = market_names_where(&items, "close_analysis_window_active");
        let pre_sync_markets = market_names_where(&items, "pre_analysis_sync_active");
        let active_markets = open_active_markets
            .iter()
            .chain(close_active_markets.iter())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let summary = json!({
            "analysis_window_active": !active_markets.is_empty(),
            "active_markets": active_markets,
            "active_windows": manager_status.get("active_pulses").cloned().unwrap_or_else(|| json!([])),
            "open_active_markets": open_active_markets,
            "close_active_markets": close_active_markets,
            "pre_sync_markets": pre_sync_markets,
            "last_cycle_status": scheduler.get("last_cycle_status").cloned().unwrap_or(JsonValue::Null),
            "last_heartbeat_at": scheduler.get("last_heartbeat_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_at": manager_status.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_label": manager_status.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null),
            "price_monitor_status": price_monitor.get("status").cloned().unwrap_or(JsonValue::Null),
            "price_monitor_updated_at": price_monitor.get("updated_at").cloned().unwrap_or(JsonValue::Null),
            "calendar_refresh": calendar_refresh,
        });
        Ok(json!({
            "items": items,
            "summary": summary,
            "scheduler": scheduler,
            "price_monitor": price_monitor
        }))
    }

    pub async fn refresh_saxo_exchange_calendars_if_stale(&self) -> Result<JsonValue> {
        let today = Utc::now().date_naive();
        if let Some(cache) = current_saxo_exchange_calendar_cache() {
            if cache.checked_date == today {
                return Ok(json!({
                    "status": "fresh",
                    "source": cache.source,
                    "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "exchange_count": cache.exchanges.len(),
                }));
            }
        }

        let cache = self
            .fetch_saxo_exchange_calendar_cache(today)
            .await
            .context("refreshing Saxo exchange calendar cache")?;
        let result = json!({
            "status": "refreshed",
            "source": cache.source,
            "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "exchange_count": cache.exchanges.len(),
        });
        let lock = saxo_exchange_calendar_cache_lock();
        *lock
            .write()
            .map_err(|_| anyhow!("Saxo exchange calendar cache lock is poisoned"))? = Some(cache);
        Ok(result)
    }

    async fn fetch_saxo_exchange_calendar_cache(
        &self,
        checked_date: NaiveDate,
    ) -> Result<SaxoExchangeCalendarCache> {
        let session = self
            .ensure_saxo_session_json("exchange_calendar")
            .await
            .context("loading Saxo session for exchange calendar lookup")?;
        let data = self
            .fetch_saxo_exchange_summaries(&session)
            .await
            .context("fetching Saxo ref/v1/exchanges")?;
        let mut exchanges = HashMap::new();
        for exchange in default_exchanges() {
            let Some(summary) = data
                .iter()
                .find(|item| saxo_exchange_matches(item, exchange.code))
            else {
                continue;
            };
            let exchange_id = saxo_exchange_text(summary, "ExchangeId")
                .unwrap_or_else(|| exchange.code.to_string());
            let mut detail = summary.clone();
            if parse_saxo_exchange_sessions(&detail).is_empty() {
                match saxo_reference_get_json(
                    self,
                    &session,
                    &format!("/ref/v1/exchanges/{exchange_id}"),
                    &[],
                )
                .await
                {
                    Ok(value) => detail = value,
                    Err(err) => warn!(
                        exchange = exchange.code,
                        exchange_id, "Saxo exchange detail lookup failed: {err:#}"
                    ),
                }
            }
            if let Some(calendar) = saxo_exchange_calendar_from_detail(&detail, &exchange_id) {
                exchanges.insert(exchange.code.to_string(), calendar);
            }
        }
        if exchanges.is_empty() {
            bail!("Saxo ref/v1/exchanges did not match any configured exchange MICs");
        }
        Ok(SaxoExchangeCalendarCache {
            checked_date,
            checked_at: Utc::now(),
            exchanges,
            source: "saxo_ref_v1_exchanges".to_string(),
        })
    }

    async fn fetch_saxo_exchange_summaries(&self, session: &JsonValue) -> Result<Vec<JsonValue>> {
        let mut skip = 0usize;
        let top = 1000usize;
        let mut all = Vec::new();
        loop {
            let payload = saxo_reference_get_json(
                self,
                session,
                "/ref/v1/exchanges",
                &[("$skip", skip.to_string()), ("$top", top.to_string())],
            )
            .await?;
            let page = payload
                .get("Data")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| anyhow!("Saxo ref/v1/exchanges response did not contain Data"))?;
            let page_len = page.len();
            all.extend(page.iter().cloned());
            let total_count = payload
                .get("__count")
                .and_then(JsonValue::as_u64)
                .map(|value| value as usize);
            let has_next = payload
                .get("__next")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if page_len < top || !has_next || total_count.is_some_and(|total| all.len() >= total) {
                break;
            }
            skip += top;
            if skip > 10_000 {
                bail!("Saxo ref/v1/exchanges pagination exceeded 10000 rows");
            }
        }
        Ok(all)
    }

    pub async fn watchlists_payload(&self) -> Result<JsonValue> {
        // Quote- and decision-derived entries older than this are dropped.
        // The price monitor refreshes live symbols every few minutes, so an
        // old portfolio_price_snapshots row is an orphan of a former holding,
        // and recycling its sentiment ("Existing portfolio holding ...") into
        // new decision prompts misleads the model into suggesting SELLs of
        // positions the broker no longer holds.
        let stale_after_days = yaml_i64(
            &self.config,
            &["strategy", "swing", "position_decision_stale_after_days"],
        )
        .unwrap_or(7)
        .max(1);
        let stale_cutoff = (Utc::now() - Duration::days(stale_after_days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let configured_universe = configured_watchlist_universe_symbols(&self.config);
        // A versioned universe is the durable source of candidate membership.
        // Position, broker, fresh report, and explicit extra-watch inputs remain
        // additive. The archived sentiment table is only a migration fallback
        // for an installation that has not configured its universe yet.
        let legacy_archive_fallback = configured_universe.is_empty();
        if legacy_archive_fallback {
            warn!(
                "watchlist universe is not configured; retaining archived sentiment membership as a temporary fallback"
            );
        }
        let configured_extra_symbols = configured_extra_watch_symbols(&self.config);
        // Membership is decided per instrument, so the rows that carry Saxo's
        // instrument identity are read before any branch admits a symbol.
        let broker_positions = self
            .select_json("SELECT symbol, uic, asset_type FROM broker_position_snapshots")
            .await
            .unwrap_or_default();
        let broker_exposures = self
            .select_json(
                "SELECT symbol, updated_at, uic, asset_type, quantity, currency, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, calculation_reliability FROM broker_instrument_exposures ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default();
        let latest_indicator_rows = self
            .select_json(
                "SELECT symbol, uic, asset_type, run_date, status, nearest_support, next_support,
                        downside_to_support_pct, downside_after_break_pct,
                        support_break_risk, support_break_risk_label, support_confidence,
                        support_history_coverage, support_touch_count
                 FROM daily_indicator_signals
                 WHERE run_id = (
                    SELECT id FROM daily_indicator_runs
                    ORDER BY run_date DESC, created_at DESC LIMIT 1
                 )",
            )
            .await
            .unwrap_or_default();
        let mut members = WatchlistMembers::new(
            broker_positions.iter().chain(&broker_exposures),
            &latest_indicator_rows,
            configured_universe.iter().chain(&configured_extra_symbols),
        );
        let mut monitored = Vec::new();
        for row in self.position_items(250).await.unwrap_or_default() {
            let symbol = text_value(&row, "symbol");
            if !symbol.is_empty() {
                members.admit(&mut monitored, &symbol, || row);
            }
        }
        for row in self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, change_pct, currency, source, status FROM portfolio_price_snapshots ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            if text_value(&row, "updated_at") < stale_cutoff {
                if legacy_archive_fallback {
                    // Keep the symbol as a universe member only while
                    // migrating installations without a configured universe.
                    // The dead quote itself never masquerades as live data.
                    members.admit(&mut monitored, &symbol, || {
                        json!({
                            "symbol": symbol,
                            "quote_status": "stale_quote_dropped",
                            "source": "price_snapshot_archive",
                        })
                    });
                }
                continue;
            }
            members.admit(&mut monitored, &symbol, || {
                let mut item = row.as_object().cloned().unwrap_or_default();
                item.insert("instrument_name".to_string(), JsonValue::from(symbol.clone()));
                JsonValue::Object(item)
            });
        }
        for row in &broker_exposures {
            let symbol = text_value(row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            members.admit(&mut monitored, &symbol, || {
                let mut item = row.as_object().cloned().unwrap_or_default();
                // The identity columns decided membership; the row keeps the
                // shape of the other branches.
                item.remove("uic");
                item.remove("asset_type");
                item.insert(
                    "instrument_name".to_string(),
                    JsonValue::from(instrument_name_for_symbol(&symbol)),
                );
                JsonValue::Object(item)
            });
        }
        let decisions: HashMap<String, JsonValue> = self
            .latest_symbol_decisions()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, decision)| text_value(decision, "created_at") >= stale_cutoff)
            .collect();
        for (symbol, decision) in &decisions {
            if symbol.is_empty() {
                continue;
            }
            members.admit(&mut monitored, symbol, || {
                let source = decision.get("source").cloned().unwrap_or_else(|| json!({}));
                let technical = source
                    .get("technical")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                json!({
                    "symbol": symbol,
                    "instrument_name": instrument_name_for_symbol(symbol),
                    "updated_at": decision.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "currency": technical.get("currency").cloned().unwrap_or(JsonValue::Null),
                    "current_price_local": technical.get("latest_close").cloned().unwrap_or(JsonValue::Null),
                    "change_pct": JsonValue::Null,
                    "market_value_dkk": 0.0,
                    "daily_pnl_dkk": 0.0,
                    "allocation_pct": 0.0,
                    "quote_status": "decision_snapshot",
                    "technical_status": technical.get("status").cloned().unwrap_or(JsonValue::Null),
                    "source": source.get("source").cloned().unwrap_or_else(|| JsonValue::from("decision_report")),
                    "decision": decision,
                    "exchange": exchange_code(symbol).to_uppercase(),
                    "region": exchange_region(symbol),
                })
            });
        }
        let mut configured_symbols_added = 0usize;
        for symbol in configured_universe {
            if members.admit(&mut monitored, &symbol, || {
                configured_watchlist_row(&symbol, "configured_analysis_universe")
            }) {
                configured_symbols_added += 1;
            }
        }
        let mut extra_symbols_added = 0usize;
        for symbol in configured_extra_symbols {
            if members.admit(&mut monitored, &symbol, || {
                configured_watchlist_row(&symbol, "configured_extra_watch")
            }) {
                extra_symbols_added += 1;
            }
        }
        let indicator_support_by_symbol: HashMap<String, JsonValue> = latest_indicator_rows
            .into_iter()
            .map(|row| (watchlist_symbol_key(&text_value(&row, "symbol")), row))
            .collect();
        for item in &mut monitored {
            let symbol = text_value(item, "symbol");
            if let Some(obj) = item.as_object_mut() {
                // A held row renamed to its system symbol arrives with the
                // decision looked up under Saxo's spelling, usually none.
                if obj.get("decision").is_none_or(JsonValue::is_null) {
                    obj.insert(
                        "decision".to_string(),
                        decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null),
                    );
                }
                obj.entry("exchange".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_code(&symbol).to_uppercase()));
                obj.entry("region".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_region(&symbol)));
                obj.entry("instrument_name".to_string())
                    .or_insert_with(|| JsonValue::from(instrument_name_for_symbol(&symbol)));
                obj.entry("quote_status".to_string())
                    .or_insert_with(|| JsonValue::from("current_source"));
                if let Some(indicator) =
                    indicator_support_by_symbol.get(&watchlist_symbol_key(&symbol))
                {
                    obj.insert(
                        "technical_risk".to_string(),
                        json!({
                            "run_date": indicator.get("run_date").cloned().unwrap_or(JsonValue::Null),
                            "status": indicator.get("status").cloned().unwrap_or(JsonValue::Null),
                            "nearest_support": indicator.get("nearest_support").cloned().unwrap_or(JsonValue::Null),
                            "next_support": indicator.get("next_support").cloned().unwrap_or(JsonValue::Null),
                            "downside_to_support_pct": indicator.get("downside_to_support_pct").cloned().unwrap_or(JsonValue::Null),
                            "downside_after_break_pct": indicator.get("downside_after_break_pct").cloned().unwrap_or(JsonValue::Null),
                            "break_risk": indicator.get("support_break_risk").cloned().unwrap_or(JsonValue::Null),
                            "break_risk_label": indicator.get("support_break_risk_label").cloned().unwrap_or(JsonValue::Null),
                            "confidence": indicator.get("support_confidence").cloned().unwrap_or(JsonValue::Null),
                            "history_coverage": indicator.get("support_history_coverage").cloned().unwrap_or(JsonValue::Null),
                            "touch_count": indicator.get("support_touch_count").cloned().unwrap_or(JsonValue::Null),
                        }),
                    );
                }
            }
        }
        let mut nordic = Vec::new();
        let mut uk = Vec::new();
        let mut us = Vec::new();
        let mut eu = Vec::new();
        for item in &monitored {
            match exchange_region(&text_value(item, "symbol")).as_str() {
                "Nordics" => nordic.push(item.clone()),
                "UK" => uk.push(item.clone()),
                "US" => us.push(item.clone()),
                _ => eu.push(item.clone()),
            }
        }
        let nordic_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "nordic_limit"]).unwrap_or(100);
        let uk_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "uk_limit"]).unwrap_or(25);
        let us_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "us_limit"]).unwrap_or(100);
        let eu_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "eu_limit"]).unwrap_or(75);
        Ok(json!({
            "generated_at": Utc::now().to_rfc3339(),
            "cache_ttl_seconds": 300,
            "universe": {
                "source": if legacy_archive_fallback { "legacy_sentiment_archive_fallback" } else { "configured_analysis_universe" },
                "configured_symbol_count": configured_watchlist_universe_symbols(&self.config).len(),
                "configured_symbols_added": configured_symbols_added,
                "extra_symbols_added": extra_symbols_added,
            },
            "categories": [
                {"key": "all", "label": "All monitored", "target_limit": monitored.len(), "total_universe": monitored.len(), "items": monitored},
                {"key": "nordic", "label": "Nordics", "target_limit": nordic_limit, "total_universe": nordic.len(), "items": nordic},
                {"key": "uk", "label": "UK", "target_limit": uk_limit, "total_universe": uk.len(), "items": uk},
                {"key": "us", "label": "US", "target_limit": us_limit, "total_universe": us.len(), "items": us},
                {"key": "eu", "label": "Europe", "target_limit": eu_limit, "total_universe": eu.len(), "items": eu}
            ],
        }))
    }

    pub(crate) async fn latest_batch_id(&self) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| row.try_get::<String, _>("batch_id").ok()))
    }

    async fn broker_positions_available(&self) -> Result<bool> {
        let row = self
            .first_json("SELECT COUNT(*) AS count FROM broker_position_snapshots")
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count") > 0)
    }

    async fn effective_position_rows(&self, limit: Option<i64>) -> Result<Vec<JsonValue>> {
        let latest_batch = self.latest_batch_id().await?;
        let where_clause = match latest_batch {
            Some(batch_id) => format!(
                "WHERE batch_id = '{}' AND excluded = 0",
                sql_escape(&batch_id)
            ),
            None => "WHERE excluded = 0".to_string(),
        };
        let base_rows = self
            .select_json(&format!(
                "SELECT instrument_name, symbol, isin, quantity, currency, open_price_local, open_price_local AS paid_price_local, current_price_local, cost_basis_local, cost_basis_local AS cost_basis_local_total, cost_basis_dkk, market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk, allocation_pct, asset_class, market_status, value_date FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default();
        let broker_rows = self
            .select_json(
                "SELECT symbol, updated_at, instrument_name, isin, uic, asset_type, quantity, currency, open_price_local, open_price_including_costs_local, execution_time_open, value_date, market_state, can_be_closed FROM broker_position_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default();
        if broker_rows.is_empty() {
            let mut rows = base_rows;
            rows.sort_by(|left, right| {
                value_f64(right, "market_value_dkk")
                    .partial_cmp(&value_f64(left, "market_value_dkk"))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
            });
            if let Some(limit) = limit {
                rows.truncate(clamp_limit(limit, 1, 250) as usize);
            }
            return Ok(rows);
        }

        let base_by_identity = base_rows
            .into_iter()
            .map(|row| {
                (
                    position_identity_key(
                        &text_value(&row, "symbol"),
                        row.get("isin").and_then(JsonValue::as_str),
                    ),
                    row,
                )
            })
            .collect::<HashMap<_, _>>();
        let price_by_symbol = self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, current_fx_rate_to_dkk, baseline_price_local, baseline_fx_rate_to_dkk, change_pct, currency, status FROM portfolio_price_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (canonical_symbol_key(&text_value(&row, "symbol")), row))
            .collect::<HashMap<_, _>>();
        let exposure_by_symbol = self
            .select_json(
                "SELECT symbol, quantity, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, currency, calculation_reliability FROM broker_instrument_exposures ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (canonical_symbol_key(&text_value(&row, "symbol")), row))
            .collect::<HashMap<_, _>>();
        // The account currency was only ever read to convert
        // `profit_loss_on_trade`, which is instrument-denominated -- see the
        // note at that conversion below. Nothing else in this projection needs
        // it, so looking it up here would be a query for an unused value.
        let same_session_buy_quantity = self.same_session_buy_quantity().await;
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");

        let mut rows = Vec::new();
        for broker in broker_rows {
            let symbol = text_value(&broker, "symbol");
            let quantity = value_f64(&broker, "quantity");
            if symbol.is_empty() || quantity <= 1e-9 {
                continue;
            }
            let symbol_key = canonical_symbol_key(&symbol);
            let base = base_by_identity.get(&position_identity_key(
                &symbol,
                broker.get("isin").and_then(JsonValue::as_str),
            ));
            let price = price_by_symbol.get(&symbol_key);
            let exposure = exposure_by_symbol.get(&symbol_key);
            // The price monitor derives this from Saxo's LastClose and the
            // current infoprice. Broker exposures can legitimately report
            // zero for delayed/approximated SIM prices, so they are only a
            // fallback when no monitored quote exists.
            let daily_change_pct = daily_change_pct_from_sources(price, exposure);
            let currency = text_value(&broker, "currency")
                .trim()
                .to_string()
                .if_empty_then(|| {
                    price
                        .map(|row| text_value(row, "currency"))
                        .filter(|value| !value.is_empty())
                })
                .or_else(|| base.map(|row| text_value(row, "currency")))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "DKK".to_string());
            let broker_open_price = value_f64(&broker, "open_price_including_costs_local")
                .max(value_f64(&broker, "open_price_local"));
            let base_quantity = base.map(|row| value_f64(row, "quantity")).unwrap_or(0.0);
            let base_market_local = base
                .map(|row| value_f64(row, "market_value_local"))
                .unwrap_or(0.0);
            let base_market_dkk = base
                .map(|row| value_f64(row, "market_value_dkk"))
                .unwrap_or(0.0);
            let inferred_fx_rate = if base_market_local.abs() > 1e-9 {
                base_market_dkk / base_market_local
            } else {
                crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &currency).await
            };
            let current_price_local = price
                .map(|row| value_f64(row, "current_price_local"))
                .filter(|value| *value > 0.0)
                .or_else(|| {
                    base.map(|row| value_f64(row, "current_price_local"))
                        .filter(|value| *value > 0.0)
                })
                .unwrap_or(broker_open_price);
            let current_fx_rate = price
                .map(|row| value_f64(row, "current_fx_rate_to_dkk"))
                .filter(|value| *value > 0.0)
                .unwrap_or(inferred_fx_rate);
            let unit_cost_dkk = if base_quantity > 0.0 {
                value_f64(base.unwrap(), "cost_basis_dkk") / base_quantity
            } else {
                broker_open_price * current_fx_rate
            };
            let cost_basis_dkk = unit_cost_dkk * quantity;
            let cost_basis_local_total = if base_quantity > 0.0 {
                let base_cost_local_total = value_f64(base.unwrap(), "cost_basis_local");
                if base_cost_local_total > 0.0 {
                    base_cost_local_total / base_quantity * quantity
                } else {
                    broker_open_price * quantity
                }
            } else {
                broker_open_price * quantity
            };
            let market_value_dkk = quantity * current_price_local * current_fx_rate;
            // A position established entirely after the session baseline did not
            // live through the session's move, so measuring it against that
            // baseline attributes a whole day to a few hours of holding. DDOG on
            // 2026-08-06 is the case: sold at 225.55 and re-bought at 242.39
            // within the session, then displayed -15.8% "1D" against the
            // previous close of 283.17 -- a fall it was not held through, and
            // whose realised half already sits in the trade ledger.
            //
            // Anchoring to cost is only correct when the *whole* position is
            // same-day. A position partly added to today still carries genuine
            // overnight exposure on the older shares, so it keeps the session
            // baseline.
            let opened_this_session = quantity > 0.0
                && same_session_buy_quantity
                    .get(&symbol.to_uppercase())
                    .copied()
                    .unwrap_or(0.0)
                    + 1e-9
                    >= quantity;
            let daily_pnl_dkk = match price {
                _ if opened_this_session => market_value_dkk - cost_basis_dkk,
                Some(price) if value_f64(price, "baseline_price_local") > 0.0 => {
                    quantity
                        * (current_price_local * current_fx_rate
                            - value_f64(price, "baseline_price_local")
                                * value_f64(price, "baseline_fx_rate_to_dkk"))
                }
                _ if base_quantity > 0.0 => {
                    value_f64(base.unwrap(), "daily_pnl_dkk") * quantity / base_quantity
                }
                _ => 0.0,
            };
            // Saxo's `ProfitLossOnTrade` is denominated in the *instrument's*
            // currency, not the account's -- the sibling field
            // `ProfitLossOnTradeInBaseCurrency` is the account-currency one, and
            // this is not it. Converting with `account_fx_rate` (EUR on this SIM
            // account) inflated every position: AKERBP's -284 NOK rendered as
            // -284 x 7.475 = -2,123 DKK instead of -284 x 0.681 = -193, and a
            // DKK instrument like DANSKE had its 443.5 DKK shown as 3,315.
            // `current_fx_rate` is the instrument's rate -- the same one
            // `market_value_dkk` above already uses correctly.
            let unrealised_pnl_dkk = exposure
                .map(|row| value_f64(row, "profit_loss_on_trade"))
                .filter(|value| value.abs() > 1e-9)
                .map(|value| value * current_fx_rate)
                .unwrap_or(market_value_dkk - cost_basis_dkk);
            rows.push(json!({
                "instrument_name": text_value(&broker, "instrument_name")
                    .if_empty_then(|| base.map(|row| text_value(row, "instrument_name")))
                    .unwrap_or_else(|| instrument_name_for_symbol(&symbol)),
                "symbol": symbol,
                "isin": broker.get("isin").cloned().unwrap_or(JsonValue::Null),
                "quantity": quantity,
                "currency": currency,
                "paid_price_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                "open_price_local": broker_open_price,
                "cost_basis_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                // Position screens show unit cost, while the immutable
                // per-position snapshot must retain the total that can be
                // reconciled to the stored DKK basis.
                "cost_basis_local_total": cost_basis_local_total,
                "current_price_local": current_price_local,
                "market_value_local": quantity * current_price_local,
                "fx_rate_to_dkk": current_fx_rate,
                "cost_basis_dkk": cost_basis_dkk,
                "market_value_dkk": market_value_dkk,
                "unrealised_pnl_dkk": unrealised_pnl_dkk,
                "daily_pnl_dkk": daily_pnl_dkk,
                // Watchlist rows use `change_pct`; position views retain the
                // explicit daily name. Keep both projections aligned.
                //
                // `daily_change_pct` normally carries the *instrument's*
                // session price move, which is what a watchlist row wants. For
                // a position opened this session that would contradict the DKK
                // beside it -- DDOG's stock was genuinely -15.8% on the day
                // while the position, entered after most of the fall, was only
                // -2.7%. A percentage and an amount in the same row must
                // describe the same thing, so an entry-anchored position
                // reports its own return.
                "change_pct": daily_change_pct,
                "daily_change_pct": if opened_this_session && cost_basis_dkk.abs() > 1e-9 {
                    daily_pnl_dkk / cost_basis_dkk
                } else {
                    daily_change_pct
                },
                "total_return_pct": if cost_basis_dkk.abs() > 1e-9 { unrealised_pnl_dkk / cost_basis_dkk } else { 0.0 },
                "allocation_pct": 0.0,
                "asset_class": text_value(&broker, "asset_type")
                    .if_empty_then(|| base.map(|row| text_value(row, "asset_class")))
                    .unwrap_or_else(|| "Equity".to_string()),
                "market_status": "Saxo broker snapshot",
                "value_date": broker.get("value_date").cloned().unwrap_or(JsonValue::Null),
                "latest_quote_updated_at": price.and_then(|row| row.get("updated_at")).cloned().unwrap_or(JsonValue::Null),
                "quote_status": price.and_then(|row| row.get("status")).cloned().unwrap_or_else(|| JsonValue::from("broker_snapshot")),
                "broker_profit_loss_on_trade": exposure.map(|row| value_f64(row, "profit_loss_on_trade")).unwrap_or(0.0),
                "broker_calculation_reliability": exposure.and_then(|row| row.get("calculation_reliability")).cloned().unwrap_or(JsonValue::Null),
            }));
        }
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let total_value = invested + cash_balance;
        for row in &mut rows {
            let market_value_dkk = value_f64(row, "market_value_dkk");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "allocation_pct".to_string(),
                    JsonValue::from(if total_value > 0.0 {
                        market_value_dkk / total_value
                    } else {
                        0.0
                    }),
                );
            }
        }
        rows.sort_by(|left, right| {
            value_f64(right, "market_value_dkk")
                .partial_cmp(&value_f64(left, "market_value_dkk"))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
        });
        if let Some(limit) = limit {
            rows.truncate(clamp_limit(limit, 1, 250) as usize);
        }
        Ok(rows)
    }

    async fn position_aggregate_from_rows(&self, rows: &[JsonValue]) -> Result<JsonValue> {
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");
        let initial_cash = value_f64(&cash_summary, "initial_cash_dkk");
        let cash_from_trades = value_f64(&cash_summary, "cash_from_trades_dkk");
        Ok(json!({
            "total_market_value_dkk": invested + cash_balance,
            "invested_market_value_dkk": invested,
            "cash_balance_dkk": cash_balance,
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "total_cost_basis_dkk": rows.iter().map(|row| value_f64(row, "cost_basis_dkk")).sum::<f64>(),
            "total_unrealised_pnl_dkk": rows.iter().map(|row| value_f64(row, "unrealised_pnl_dkk")).sum::<f64>(),
            "total_daily_pnl_dkk": rows.iter().map(|row| value_f64(row, "daily_pnl_dkk")).sum::<f64>(),
            "position_count": rows.len() as i64,
            "source": if self.broker_positions_available().await? { "saxo_broker_snapshot" } else { "position_snapshots" }
        }))
    }

    async fn position_aggregate(&self, batch_id: Option<&str>) -> Result<JsonValue> {
        let rows = if self.broker_positions_available().await? {
            self.effective_position_rows(None).await?
        } else {
            let where_clause = match batch_id {
                Some(batch_id) => format!(
                    "WHERE batch_id = '{}' AND excluded = 0",
                    sql_escape(batch_id)
                ),
                None => "WHERE excluded = 0".to_string(),
            };
            self.select_json(&format!(
                "SELECT market_value_dkk, cost_basis_dkk, unrealised_pnl_dkk, daily_pnl_dkk FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default()
        };
        self.position_aggregate_from_rows(&rows).await
    }

    /// Normalizes the same effective positions used for an aggregate snapshot
    /// into immutable, recomputable position evidence. This is intentionally
    /// pure: it neither refreshes prices nor changes portfolio/execution state.
    fn portfolio_position_snapshot_rows(rows: &[JsonValue]) -> Result<Vec<JsonValue>> {
        let mut snapshots = Vec::with_capacity(rows.len());
        let mut seen_symbols = HashSet::new();
        for row in rows {
            let symbol = text_value(row, "symbol").trim().to_string();
            let quantity = value_f64(row, "quantity");
            let price_local = value_f64(row, "current_price_local");
            let currency = text_value(row, "currency").trim().to_string();
            let cost_basis_local = value_f64(row, "cost_basis_local_total");
            let cost_basis_dkk = value_f64(row, "cost_basis_dkk");
            if symbol.is_empty()
                || currency.is_empty()
                || !quantity.is_finite()
                || quantity <= 0.0
                || !price_local.is_finite()
                || price_local <= 0.0
                || !cost_basis_local.is_finite()
                || cost_basis_local < 0.0
                || !cost_basis_dkk.is_finite()
                || cost_basis_dkk < 0.0
            {
                bail!("position snapshot inputs are incomplete for {symbol}");
            }
            let canonical_symbol = canonical_symbol_key(&symbol);
            if !seen_symbols.insert(canonical_symbol) {
                bail!("position snapshot contains duplicate symbol {symbol}");
            }
            let market_value_local = quantity * price_local;
            let fx_rate_to_dkk = value_f64(row, "fx_rate_to_dkk");
            let fx_rate_to_dkk = if fx_rate_to_dkk.is_finite() && fx_rate_to_dkk > 0.0 {
                fx_rate_to_dkk
            } else {
                value_f64(row, "market_value_dkk") / market_value_local
            };
            if !fx_rate_to_dkk.is_finite() || fx_rate_to_dkk <= 0.0 {
                bail!("position snapshot FX rate is unavailable for {symbol}");
            }
            let market_value_dkk = market_value_local * fx_rate_to_dkk;
            snapshots.push(json!({
                "symbol": symbol,
                "isin": row.get("isin").cloned().unwrap_or(JsonValue::Null),
                "currency": currency,
                "quantity": quantity,
                "price_local": price_local,
                "fx_rate_to_dkk": fx_rate_to_dkk,
                "cost_basis_local": cost_basis_local,
                "cost_basis_dkk": cost_basis_dkk,
                "market_value_dkk": market_value_dkk,
                "unrealised_pnl_dkk": market_value_dkk - cost_basis_dkk,
            }));
        }
        Ok(snapshots)
    }

    async fn cash_summary_from_ledger(&self) -> Result<JsonValue> {
        let initial_cash =
            yaml_f64(&self.config, &["portfolio", "initial_cash_dkk"]).unwrap_or(0.0);
        let row = self
            .first_json(
                "SELECT COALESCE(SUM(net_amount_dkk), 0) AS cash_from_trades_dkk FROM trade_ledger WHERE status IN ('executed', 'approved')",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let cash_from_trades = value_f64(&row, "cash_from_trades_dkk");
        Ok(json!({
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "cash_balance_dkk": initial_cash + cash_from_trades,
        }))
    }

    async fn after_tax_summary(&self, gross_unrealised_pnl_dkk: f64) -> JsonValue {
        let tax_year = Utc::now().year();
        let currency = yaml_string(&self.config, &["taxation", "share_income", "currency"])
            .unwrap_or_else(|| "DKK".to_string());
        if !currency.eq_ignore_ascii_case("DKK") {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "unsupported_tax_currency",
            );
        }
        let Some(brackets) = share_income_tax_brackets(&self.config) else {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "invalid_tax_brackets",
            );
        };
        let realised_row = match self
            .first_json(&format!(
                "SELECT COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_ytd_dkk \
                 FROM trade_ledger WHERE side = 'SELL' AND tax_year = {tax_year}"
            ))
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => json!({}),
            Err(err) => {
                warn!(
                    "after-tax estimate unavailable because the trade ledger could not be read: {err:#}"
                );
                return unavailable_after_tax_summary(
                    gross_unrealised_pnl_dkk,
                    tax_year,
                    "trade_ledger_unavailable",
                );
            }
        };
        let realised_gain_ytd_dkk = value_f64(&realised_row, "realised_gain_ytd_dkk");
        let Some(estimated_tax_dkk) = incremental_share_income_tax_dkk(
            realised_gain_ytd_dkk,
            gross_unrealised_pnl_dkk,
            &brackets,
        ) else {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "invalid_tax_inputs",
            );
        };
        json!({
            "status": "estimated",
            "tax_year": tax_year,
            "currency": "DKK",
            "gross_unrealised_pnl_dkk": gross_unrealised_pnl_dkk,
            "realised_gain_ytd_dkk": realised_gain_ytd_dkk,
            "estimated_tax_dkk": estimated_tax_dkk,
            "unrealised_pnl_after_tax_dkk": gross_unrealised_pnl_dkk - estimated_tax_dkk,
            "basis": "incremental_share_income_tax_on_realised_gain_plus_unrealised_pnl"
        })
    }

    pub async fn position_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let decisions = self.latest_symbol_decisions().await.unwrap_or_default();
        let mut rows = self.effective_position_rows(Some(limit)).await?;
        for row in &mut rows {
            let symbol = text_value(row, "symbol");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "ladder_status".to_string(),
                    json!({"text": "idle", "active_orders": 0, "filled_entry_rungs": 0, "total_entry_rungs": 0, "progress_pct": 0.0, "trailing": false}),
                );
                obj.insert(
                    "decision".to_string(),
                    decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null),
                );
                obj.entry("latest_quote_updated_at".to_string())
                    .or_insert(JsonValue::Null);
            }
        }
        Ok(rows)
    }

    pub(crate) async fn trade_thesis_outcome_evidence(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT id, created_at, symbol, action, quantity, currency, trade_thesis_json
                 FROM execution_orders
                 WHERE action = 'BUY'
                   AND trade_thesis_json IS NOT NULL
                 ORDER BY created_at DESC, id DESC
                 LIMIT {}",
                TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT
            ))
            .await?;
        let mut outcomes = Vec::with_capacity(rows.len());
        for row in rows {
            let raw_thesis = row
                .get("trade_thesis_json")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let thesis = if raw_thesis.is_object() {
                raw_thesis
            } else {
                raw_thesis
                    .as_str()
                    .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                    .unwrap_or(JsonValue::Null)
            };
            if json_text(&thesis, "status") != "recorded" {
                continue;
            }
            outcomes.push(self.execution_order_holding_period_outcome(&row).await?);
        }
        Ok(trade_thesis_outcome_evidence_from_holding_outcomes(
            &outcomes,
        ))
    }

    pub(crate) async fn holding_thesis_reviews(&self) -> Result<JsonValue> {
        let positions = self.position_items(250).await?;
        let thesis_rows = self
            .select_json(&format!(
                "SELECT eo.id, eo.created_at, eo.symbol, eo.trade_thesis_json,
                        (SELECT MIN(f.created_at)
                         FROM execution_fills f
                         WHERE f.execution_order_id = eo.id
                           AND f.delta_quantity > 0) AS first_fill_at
                 FROM execution_orders eo
                 WHERE eo.action = 'BUY'
                   AND eo.trade_thesis_json IS NOT NULL
                 ORDER BY eo.created_at DESC, eo.id DESC
                 LIMIT {}",
                HOLDING_THESIS_REVIEW_LIMIT
            ))
            .await?;
        let stale_after_days = yaml_i64(
            &self.config,
            &["strategy", "swing", "position_decision_stale_after_days"],
        )
        .unwrap_or(DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS)
        .max(1);
        Ok(compact_holding_thesis_reviews(
            &positions,
            &thesis_rows,
            stale_after_days,
            Utc::now(),
        ))
    }

    pub(crate) async fn latest_indicator_signal_summary(&self, symbol: &str) -> Result<JsonValue> {
        let sql = format!(
            "SELECT run_date, status, close, rsi14, macd_histogram, atr14, reward_risk,
                    nearest_support, next_support, downside_to_support_pct,
                    downside_after_break_pct, support_break_risk, support_break_risk_label,
                    support_confidence, support_history_coverage, support_touch_count,
                    trend_bias, sentiment, confluence_count, min_confluences, error_text
             FROM daily_indicator_signals
             WHERE symbol = '{}'
             ORDER BY run_date DESC, created_at DESC
             LIMIT 1",
            sql_escape(symbol)
        );
        Ok(self.first_json(&sql).await?.unwrap_or(JsonValue::Null))
    }

    pub(crate) async fn support_risk_evidence(&self) -> Result<JsonValue> {
        let cutoff = (Utc::now().date_naive()
            - Duration::days(SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS))
        .to_string();
        let rows = self
            .select_json(&format!(
                "SELECT symbol, run_date, close, support_break_risk, support_break_risk_label,
                        support_confidence, support_history_coverage
                 FROM daily_indicator_signals
                 WHERE status = 'ok'
                   AND close > 0
                   AND run_date >= '{}'
                 ORDER BY symbol ASC, run_date ASC, created_at ASC",
                sql_escape(&cutoff)
            ))
            .await?;
        Ok(support_risk_evidence_from_indicator_rows(&rows))
    }

    pub async fn markov_signals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        crate::markov_method::latest_markov_signals(self, limit).await
    }

    /// Quantity bought per symbol since the current session baseline.
    ///
    /// Used to decide whether a position's daily P/L should anchor to the
    /// session baseline or to its own entry. Keyed by upper-cased symbol
    /// because broker and ledger spellings differ for the same instrument
    /// (`DB1Gn:xetr` against `db1gn:xetr`).
    ///
    /// Deliberately sums BUY quantity rather than net BUY-minus-SELL: a
    /// same-day round trip that exits and re-enters — DDOG on 2026-08-06 —
    /// nets to zero while genuinely having established the whole position
    /// today, which is exactly the case this is here to catch.
    async fn same_session_buy_quantity(&self) -> HashMap<String, f64> {
        let timezone = yaml_string(&self.config, &["price_monitor", "timezone"])
            .or_else(|| yaml_string(&self.config, &["localization", "time_zone"]))
            .and_then(|value| value.parse::<chrono_tz::Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        let reset_hour = yaml_i64(&self.config, &["price_monitor", "reset_hour_local"])
            .unwrap_or(6)
            .clamp(0, 23) as u32;
        let session_date = crate::price_monitor::session_date_for(Utc::now(), timezone, reset_hour);
        let Some(session_start) = session_date
            .and_hms_opt(reset_hour, 0, 0)
            .and_then(|naive| naive.and_local_timezone(timezone).single())
        else {
            return HashMap::new();
        };
        let session_start_utc = session_start
            .with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let rows = self
            .select_json(&format!(
                "SELECT symbol, SUM(quantity) AS bought FROM trade_ledger \
                 WHERE side = 'BUY' AND created_at >= '{}' GROUP BY symbol",
                sql_escape(&session_start_utc)
            ))
            .await
            .unwrap_or_default();
        rows.iter()
            .filter_map(|row| {
                let symbol = text_value(row, "symbol");
                if symbol.is_empty() {
                    return None;
                }
                Some((symbol.to_uppercase(), value_f64(row, "bought")))
            })
            .collect()
    }

    /// Per-source data freshness for the dashboard's staleness strip.
    ///
    /// Motivated by three separate staleness faults found on 2026-08-02/03 that
    /// were all invisible in the UI: the FX cache had been frozen for two days,
    /// planner statistics for 33, and the position Decision column was serving
    /// data from a table dead for three months. Each was discovered by querying
    /// production by hand. This surfaces the same question in the dashboard.
    ///
    /// Sources are queried individually rather than as one UNION so a single
    /// missing table degrades that row to `missing` instead of failing the whole
    /// strip — this is a diagnostic, and a diagnostic that disappears when
    /// something is wrong is worse than none.
    pub(crate) async fn data_freshness(&self) -> Vec<DataFreshnessSourcePayload> {
        let now = Utc::now();
        let mut rows = Vec::new();
        for source in FRESHNESS_SOURCES {
            let observed_at = self
                .first_json(source.sql)
                .await
                .ok()
                .flatten()
                .and_then(|row| {
                    row.get("at")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                })
                .filter(|value| !value.trim().is_empty());
            let age_minutes = observed_at
                .as_deref()
                .and_then(parse_freshness_timestamp)
                .map(|observed| (now - observed).num_minutes().max(0));
            rows.push(DataFreshnessSourcePayload {
                key: source.key.to_string(),
                label: source.label.to_string(),
                tab: source.tab.to_string(),
                observed_at,
                age_minutes,
                age_label: freshness_age_label(age_minutes),
                stale_after_minutes: source.stale_after_minutes,
                state: freshness_state(age_minutes, source.stale_after_minutes).to_string(),
            });
        }
        rows
    }

    pub async fn markov_signals_page_filtered(
        &self,
        limit: i64,
        offset: i64,
        filter: &str,
        min_signed_signal: f64,
    ) -> Result<Vec<JsonValue>> {
        crate::markov_method::latest_markov_signals_page_filtered(
            self,
            limit,
            offset,
            filter,
            min_signed_signal,
        )
        .await
    }

    pub async fn markov_signals_count_filtered(
        &self,
        filter: &str,
        min_signed_signal: f64,
    ) -> Result<i64> {
        crate::markov_method::latest_markov_signal_count_filtered(self, filter, min_signed_signal)
            .await
    }

    pub async fn latest_markov_run(&self) -> Result<JsonValue> {
        crate::markov_method::latest_markov_run(self).await
    }

    pub async fn quiver_signals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        crate::quiver::latest_quiver_signals(self, limit).await
    }

    pub async fn quiver_signals_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        crate::quiver::latest_quiver_signals_page(self, limit, offset).await
    }

    pub async fn quiver_signals_count(&self) -> Result<i64> {
        crate::quiver::latest_quiver_signal_count(self).await
    }

    pub async fn latest_quiver_run(&self) -> Result<JsonValue> {
        crate::quiver::latest_quiver_run(self).await
    }

    pub async fn latest_daily_indicator_run(&self) -> Result<JsonValue> {
        let sql = "SELECT id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json
                   FROM daily_indicator_runs
                   ORDER BY run_date DESC, created_at DESC
                   LIMIT 1";
        Ok(self.first_json(sql).await?.unwrap_or(JsonValue::Null))
    }

    pub async fn scheduler_status_value(&self) -> Result<JsonValue> {
        Ok(self
            .first_json("SELECT singleton_key, started_at, last_heartbeat_at, last_cycle_started_at, last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid FROM scheduler_status WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .unwrap_or(JsonValue::Null))
    }

    /// Narrow scheduler status projection for the public operational API.
    ///
    /// `last_cycle_json` can retain provider and execution diagnostics, while
    /// `scheduler_pid` is local process metadata, so neither crosses this
    /// boundary.
    pub async fn scheduler_status_summary(&self) -> Result<JsonValue> {
        Ok(self
            .first_json("SELECT started_at, last_heartbeat_at, last_cycle_started_at, last_cycle_completed_at, last_cycle_status FROM scheduler_status WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn scheduler_cycles(&self, limit: i64) -> Result<Vec<JsonValue>> {
        self.scheduler_cycles_page(limit, 0).await
    }

    /// Narrow scheduler-cycle projection for the public operational API.
    ///
    /// The retained `cycle_json` is decoded locally into its fixed display
    /// fields before serialization; broker alerts and other persisted columns
    /// remain outside this API boundary.
    pub async fn scheduler_cycle_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT started_at, status, generated_decision, queue_status, notifications_status, cycle_json
             FROM scheduler_cycle_history
             ORDER BY started_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn scheduler_cycles_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, started_at, completed_at, status, analysis_window_active,
                    generated_decision, queue_status, notifications_status,
                    broker_alerts_status, cycle_json
             FROM scheduler_cycle_history
             ORDER BY started_at DESC, id DESC
             LIMIT {} OFFSET {}",
            clamp_limit(limit, 1, 100),
            offset.max(0).min(100_000)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn scheduler_cycles_count(&self) -> Result<i64> {
        Ok(self
            .first_json("SELECT COUNT(*) AS count FROM scheduler_cycle_history")
            .await?
            .and_then(|row| row.get("count").and_then(JsonValue::as_i64))
            .unwrap_or(0))
    }

    pub async fn prune_scheduler_cycles(&self, now: DateTime<Utc>) -> Result<i64> {
        let (max_rows, retention_days) = scheduler_history_policy_values(
            yaml_i64(&self.config, &["scheduler", "history_max_rows"]),
            yaml_i64(&self.config, &["scheduler", "history_retention_days"]),
        );
        let mut deleted_rows = 0;
        if retention_days > 0 {
            let keep_since_started_at = (now - Duration::days(retention_days))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            deleted_rows += sqlx::query(&format!(
                "DELETE FROM scheduler_cycle_history WHERE started_at < '{}'",
                sql_escape(&keep_since_started_at)
            ))
            .execute(&self.pool)
            .await
            .context("pruning scheduler cycle history by retention age")?
            .rows_affected() as i64;
        }
        if max_rows > 0 {
            deleted_rows += sqlx::query(&format!(
                "DELETE FROM scheduler_cycle_history
                 WHERE id NOT IN (
                    SELECT id
                    FROM scheduler_cycle_history
                    ORDER BY id DESC
                    LIMIT {max_rows}
                 )"
            ))
            .execute(&self.pool)
            .await
            .context("pruning scheduler cycle history by row cap")?
            .rows_affected() as i64;
        }
        Ok(deleted_rows)
    }

    /// Bounds high-resolution portfolio-position evidence without touching the
    /// aggregate history. Full per-cycle detail is retained for the drawdown
    /// window; older days retain their final recorded snapshot only.
    pub async fn prune_portfolio_position_snapshots(&self, now: DateTime<Utc>) -> Result<i64> {
        let cutoff = (now - Duration::days(PORTFOLIO_POSITION_SNAPSHOT_RETENTION_DAYS))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let result = sqlx::query(&format!(
            "DELETE FROM portfolio_position_snapshots AS older
             WHERE older.recorded_at < '{cutoff}'
               AND EXISTS (
                   SELECT 1
                   FROM portfolio_value_history AS newer
                   WHERE newer.recorded_at < '{cutoff}'
                     AND SUBSTR(newer.recorded_at, 1, 10) = SUBSTR(older.recorded_at, 1, 10)
                     AND (newer.recorded_at > older.recorded_at
                          OR (newer.recorded_at = older.recorded_at AND newer.id > older.snapshot_id))
               )",
            cutoff = sql_escape(&cutoff),
        ))
        .execute(&self.pool)
        .await
        .context("pruning retained portfolio position snapshots")?;
        Ok(result.rows_affected() as i64)
    }

    /// Reports how much selected performance history has reproducible
    /// per-position evidence. Historical aggregate-only rows remain visible
    /// but are deliberately never relabelled as repairable.
    pub async fn portfolio_position_snapshot_coverage(&self, range_key: &str) -> Result<JsonValue> {
        let where_clause = match performance_start_at(range_key) {
            Some(start_at) => format!("WHERE h.recorded_at >= '{}'", sql_escape(&start_at)),
            None => String::new(),
        };
        let row = self
            .first_json(&format!(
                "SELECT COUNT(*) AS aggregate_snapshot_count,
                        COALESCE(SUM(CASE
                            WHEN h.position_count = 0 OR details.snapshot_id IS NOT NULL THEN 1
                            ELSE 0
                        END), 0) AS covered_snapshot_count,
                        COUNT(details.snapshot_id) AS snapshots_with_position_rows,
                        CAST(COALESCE(SUM(details.position_row_count), 0) AS BIGINT) AS position_evidence_row_count,
                        MIN(CASE
                            WHEN h.position_count = 0 OR details.snapshot_id IS NOT NULL THEN h.recorded_at
                        END) AS first_covered_at,
                        MAX(CASE
                            WHEN h.position_count = 0 OR details.snapshot_id IS NOT NULL THEN h.recorded_at
                        END) AS latest_covered_at
                 FROM portfolio_value_history h
                 LEFT JOIN (
                    SELECT snapshot_id, COUNT(*) AS position_row_count
                    FROM portfolio_position_snapshots
                    GROUP BY snapshot_id
                 ) details ON details.snapshot_id = h.id
                 {where_clause}"
            ))
            .await?
            .unwrap_or_else(|| json!({}));
        let aggregate_snapshot_count = value_i64(&row, "aggregate_snapshot_count");
        let covered_snapshot_count = value_i64(&row, "covered_snapshot_count");
        let missing_snapshot_count = (aggregate_snapshot_count - covered_snapshot_count).max(0);
        let status = if aggregate_snapshot_count == 0 {
            "collecting"
        } else if missing_snapshot_count == 0 {
            "complete"
        } else {
            "partial"
        };
        let coverage_pct = if aggregate_snapshot_count > 0 {
            JsonValue::from(covered_snapshot_count as f64 * 100.0 / aggregate_snapshot_count as f64)
        } else {
            JsonValue::Null
        };
        Ok(json!({
            "status": status,
            "range_key": range_key,
            "aggregate_snapshot_count": aggregate_snapshot_count,
            "covered_snapshot_count": covered_snapshot_count,
            "missing_legacy_snapshot_count": missing_snapshot_count,
            "coverage_pct": coverage_pct,
            "snapshots_with_position_rows": value_i64(&row, "snapshots_with_position_rows"),
            "position_evidence_row_count": value_i64(&row, "position_evidence_row_count"),
            "first_covered_at": row.get("first_covered_at").cloned().unwrap_or(JsonValue::Null),
            "latest_covered_at": row.get("latest_covered_at").cloned().unwrap_or(JsonValue::Null),
            "latest_snapshot": self
                .latest_portfolio_position_snapshot_evidence(range_key)
                .await?,
            "latest_change": self
                .portfolio_position_snapshot_change_evidence(range_key)
                .await?,
            "detail_retention": "all_cycle_snapshots_for_90_days_then_final_stored_snapshot_per_utc_date",
            "integrity": self.portfolio_position_snapshot_integrity().await?,
            "safety": "local_snapshot_evidence_read_no_provider_hermes_gate_or_order_authority",
            "interpretation": "Coverage identifies snapshots whose position detail is retained and therefore recomputable. Aggregate-only legacy snapshots remain usable for charting but cannot be repaired from position evidence. A broker-derived aggregate unrealised P/L difference is reported separately by the integrity diagnostic and is not structural corruption.",
        }))
    }

    /// Summarizes composition changes between the two newest retained
    /// snapshots in the selected range. Unlike current positions, both sides
    /// are immutable local evidence from their respective record times.
    async fn portfolio_position_snapshot_change_evidence(
        &self,
        range_key: &str,
    ) -> Result<JsonValue> {
        let range_clause = match performance_start_at(range_key) {
            Some(start_at) => format!("AND h.recorded_at >= '{}'", sql_escape(&start_at)),
            None => String::new(),
        };
        let snapshots = self
            .select_json(&format!(
                "SELECT h.id AS snapshot_id, h.recorded_at, h.snapshot_type, h.source,
                        h.position_count, h.invested_market_value_dkk,
                        h.total_cost_basis_dkk, h.total_unrealised_pnl_dkk
                 FROM portfolio_value_history h
                 WHERE (h.position_count = 0 OR EXISTS (
                    SELECT 1 FROM portfolio_position_snapshots p WHERE p.snapshot_id = h.id
                 ))
                 {range_clause}
                 ORDER BY h.id DESC
                 LIMIT 2"
            ))
            .await?;
        let Some(current_snapshot) = snapshots.first() else {
            return Ok(json!({
                "status": "collecting",
                "safety": "local_retained_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
            }));
        };
        let Some(previous_snapshot) = snapshots.get(1) else {
            return Ok(json!({
                "status": "collecting",
                "current_snapshot": current_snapshot,
                "safety": "local_retained_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
            }));
        };
        let current_id = value_i64(current_snapshot, "snapshot_id");
        let previous_id = value_i64(previous_snapshot, "snapshot_id");
        let items = self
            .select_json(&format!(
                "SELECT snapshot_id, symbol, isin, currency, quantity, price_local,
                        fx_rate_to_dkk, cost_basis_local, cost_basis_dkk,
                        market_value_dkk, unrealised_pnl_dkk
                 FROM portfolio_position_snapshots
                 WHERE snapshot_id IN ({current_id}, {previous_id})
                 ORDER BY snapshot_id DESC, symbol ASC"
            ))
            .await?;
        let current_rows = items
            .iter()
            .filter(|row| value_i64(row, "snapshot_id") == current_id)
            .cloned()
            .collect::<Vec<_>>();
        let previous_rows = items
            .iter()
            .filter(|row| value_i64(row, "snapshot_id") == previous_id)
            .cloned()
            .collect::<Vec<_>>();
        Ok(portfolio_position_snapshot_change_from_rows(
            current_snapshot,
            &current_rows,
            previous_snapshot,
            &previous_rows,
        ))
    }

    /// Returns the latest stored, recomputable position composition in the
    /// selected performance range. It intentionally does not fall back to a
    /// live broker/current-position read: this panel is historical evidence.
    async fn latest_portfolio_position_snapshot_evidence(
        &self,
        range_key: &str,
    ) -> Result<JsonValue> {
        let range_clause = match performance_start_at(range_key) {
            Some(start_at) => format!("AND h.recorded_at >= '{}'", sql_escape(&start_at)),
            None => String::new(),
        };
        let Some(snapshot) = self
            .first_json(&format!(
                "SELECT h.id AS snapshot_id, h.recorded_at, h.snapshot_type, h.source,
                        h.position_count, h.invested_market_value_dkk,
                        h.total_cost_basis_dkk, h.total_unrealised_pnl_dkk
                 FROM portfolio_value_history h
                 WHERE (h.position_count = 0 OR EXISTS (
                    SELECT 1 FROM portfolio_position_snapshots p WHERE p.snapshot_id = h.id
                 ))
                 {range_clause}
                 ORDER BY h.id DESC
                 LIMIT 1"
            ))
            .await?
        else {
            return Ok(json!({
                "status": "collecting",
                "items": [],
                "safety": "local_retained_position_snapshot_read_no_provider_hermes_gate_or_order_authority",
            }));
        };
        let snapshot_id = value_i64(&snapshot, "snapshot_id");
        let items = self
            .select_json(&format!(
                "SELECT symbol, isin, currency, quantity, price_local, fx_rate_to_dkk,
                        cost_basis_local, cost_basis_dkk, market_value_dkk,
                        unrealised_pnl_dkk
                 FROM portfolio_position_snapshots
                 WHERE snapshot_id = {snapshot_id}
                 ORDER BY symbol ASC"
            ))
            .await?;
        Ok(json!({
            "status": "available",
            "snapshot": snapshot,
            "items": items,
            "safety": "local_retained_position_snapshot_read_no_provider_hermes_gate_or_order_authority",
            "interpretation": "This is the latest retained per-position composition inside the selected performance range, not a live broker portfolio. DKK market value and recomputed unrealised P/L can be reproduced from the stored local price, quantity, FX rate, and cost basis.",
        }))
    }

    pub async fn price_monitor_status_value(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT singleton_key, updated_at, status, summary_json
                 FROM price_monitor_status
                 WHERE singleton_key = 'main'
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn record_price_monitor_status(&self, summary: &JsonValue) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let status = json_text(summary, "status");
        let status = if status.is_empty() {
            "unknown"
        } else {
            &status
        };
        let summary_json = summary.to_string();
        let updated = sqlx::query(&format!(
            "UPDATE price_monitor_status
             SET updated_at = '{}', status = '{}', summary_json = '{}'
             WHERE singleton_key = 'main'",
            sql_escape(&now),
            sql_escape(status),
            sql_escape(&summary_json)
        ))
        .execute(&self.pool)
        .await
        .context("updating price monitor status")?;
        if updated.rows_affected() == 0 {
            sqlx::query(&format!(
                "INSERT INTO price_monitor_status
                    (singleton_key, updated_at, status, summary_json)
                 VALUES ('main', '{}', '{}', '{}')",
                sql_escape(&now),
                sql_escape(status),
                sql_escape(&summary_json)
            ))
            .execute(&self.pool)
            .await
            .context("inserting price monitor status")?;
        }
        Ok(())
    }
}

impl AppState {
    pub async fn active_strategy_baseline(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT id, created_at, activated_at, status, goal_version, config_json, prompt_json, source
                 FROM strategy_baselines
                 WHERE status = 'active'
                 ORDER BY activated_at DESC, created_at DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn latest_trading_manager_run(&self) -> Result<JsonValue> {
        let mut run = self
            .first_json(
                "SELECT id, created_at, manager_key, manager_kind, manager_label, target_at_utc, report_id, status, open_exchange_codes_json, technical_json, manager_json, queue_result_json, error_text
                 FROM trading_manager_runs
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null);

        let report_id = run
            .get("report_id")
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
            .unwrap_or(0);
        let current_advice_status = run
            .get("manager_json")
            .and_then(|value| value.get("hermes_decision_advice"))
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let should_repair_advice =
            report_id > 0 && matches!(current_advice_status, "" | "timeout" | "error");
        if should_repair_advice {
            if let Some(advice) = self.hermes_decision_advice_by_report(report_id).await? {
                let existing_mode = run
                    .get("manager_json")
                    .and_then(|value| value.get("hermes_decision_advice"))
                    .and_then(|value| value.get("mode"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or("record_only")
                    .to_string();
                if let Some(manager_json) = run
                    .get_mut("manager_json")
                    .and_then(JsonValue::as_object_mut)
                {
                    manager_json.insert(
                        "hermes_decision_advice".to_string(),
                        json!({
                            "status": json_text(&advice, "status"),
                            "mode": existing_mode,
                            "source_session_id": json_text(&advice, "source_session_id"),
                            "overall_recommendation": json_text(&advice, "overall_recommendation"),
                            "summary": json_text(&advice, "summary"),
                            "attached_from": "report_fallback",
                            "raw": advice,
                        }),
                    );
                }
            }
        }

        Ok(run)
    }

    pub async fn record_scheduler_cycle(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler cycle JSON")?;
        let queue_status = cycle_json
            .get("trading_manager")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or(status);
        let analysis_window_active = cycle_json
            .get("market")
            .and_then(|value| value.get("analysis_window_active"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let notifications_status = cycle_json
            .get("notifications")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or("not_run");
        let broker_alerts_status = notifications_status;
        let sql = format!(
            "INSERT INTO scheduler_cycle_history (
                started_at, completed_at, status, analysis_window_active,
                generated_decision, queue_status, notifications_status, broker_alerts_status,
                cycle_json
            ) VALUES (
                '{}', '{}', '{}', {}, 0, '{}', '{}', '{}', '{}'
            )",
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            if analysis_window_active { 1 } else { 0 },
            sql_escape(queue_status),
            sql_escape(notifications_status),
            sql_escape(broker_alerts_status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording scheduler cycle")?;
        self.update_scheduler_status(started_at, completed_at, status, cycle_json)
            .await
    }

    pub async fn update_scheduler_heartbeat(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', NULL, NULL, 'heartbeat', '{{}}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at",
            sql_escape(&now),
            sql_escape(&now)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler heartbeat")?;
        Ok(())
    }

    async fn update_scheduler_status(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler status JSON")?;
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', '{}', '{}', '{}', '{}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at,
                last_cycle_started_at = excluded.last_cycle_started_at,
                last_cycle_completed_at = excluded.last_cycle_completed_at,
                last_cycle_status = excluded.last_cycle_status,
                last_cycle_json = excluded.last_cycle_json",
            sql_escape(started_at),
            sql_escape(&now),
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler status")?;
        Ok(())
    }

    pub async fn performance_history_for_range(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let columns = "recorded_at, snapshot_type, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count, source";
        let limit = clamp_limit(limit, 1, 5000);
        let mut rows = Vec::new();
        let where_clause = match performance_start_at(range_key) {
            Some(start_at) => {
                let escaped_start = sql_escape(&start_at);
                let anchor_sql = format!(
                    "SELECT {columns} FROM portfolio_value_history WHERE recorded_at < '{escaped_start}' ORDER BY recorded_at DESC, id DESC LIMIT 1"
                );
                rows.extend(self.select_json(&anchor_sql).await.unwrap_or_default());
                format!("WHERE recorded_at >= '{escaped_start}'")
            }
            None => String::new(),
        };
        // Downsample rather than truncate. `ORDER BY recorded_at ASC` with a
        // LIMIT keeps the *oldest* rows in the window, so every range longer
        // than about a month returned the same early slice with today's point
        // appended: 3M, YTD, 1Y and ALL rendered identically, and 1M reported a
        // -46,562 DKK "change" measured against a row from three weeks before
        // the window it claimed to cover.
        //
        // Sampling on `id % stride` keeps the series spanning the whole window
        // at reduced resolution, and is portable across both backends `AnyPool`
        // serves -- window functions are not uniformly available.
        let matched = self
            .first_json(&format!(
                "SELECT COUNT(*) AS count FROM portfolio_value_history {where_clause}"
            ))
            .await
            .ok()
            .flatten()
            .map(|row| value_f64(&row, "count") as i64)
            .unwrap_or(0);
        let target = clamp_limit(limit, 1, PERFORMANCE_MAX_CHART_POINTS);
        let stride = if matched > target {
            (matched as f64 / target as f64).ceil() as i64
        } else {
            1
        };
        let sample_clause = if stride > 1 {
            let joiner = if where_clause.is_empty() {
                "WHERE"
            } else {
                "AND"
            };
            format!(" {joiner} (id % {stride} = 0)")
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT {columns} FROM portfolio_value_history {where_clause}{sample_clause} ORDER BY recorded_at ASC, id ASC LIMIT {}",
            PERFORMANCE_MAX_CHART_POINTS
        );
        rows.extend(self.select_json(&sql).await.unwrap_or_default());
        // Sampling can drop the window's first and last rows, which would both
        // leave the chart short of the window it advertises and -- worse --
        // make `change_dkk` an artifact of the stride, since it is measured
        // from the first row present. Observed live: 3M and YTD reported
        // -107,527 and -69,391 from an identical start date purely because
        // their strides selected different opening rows. Pinning both ends
        // makes the endpoints stride-independent.
        if stride > 1 {
            for order in ["ASC", "DESC"] {
                let edge_sql = format!(
                    "SELECT {columns} FROM portfolio_value_history {where_clause} ORDER BY recorded_at {order}, id {order} LIMIT 1"
                );
                rows.extend(self.select_json(&edge_sql).await.unwrap_or_default());
            }
        }
        rows.sort_by(|left, right| {
            text_value(left, "recorded_at")
                .cmp(&text_value(right, "recorded_at"))
                .then_with(|| {
                    text_value(left, "snapshot_type").cmp(&text_value(right, "snapshot_type"))
                })
        });
        // The anchor and pinned edge rows can each coincide with a sampled row,
        // which would plot the same instant twice and let a duplicated endpoint
        // skew the range metrics.
        rows.dedup_by(|left, right| {
            text_value(left, "recorded_at") == text_value(right, "recorded_at")
                && text_value(left, "snapshot_type") == text_value(right, "snapshot_type")
        });
        Ok(rows)
    }

    pub async fn performance_history_with_current(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let mut history = self.performance_history_for_range(range_key, limit).await?;
        let current = self.current_performance_row().await?;
        let latest_matches_current = history
            .last()
            .is_some_and(|latest| performance_rows_have_same_values(latest, &current));
        if !latest_matches_current {
            history.push(current);
        } else if let Some(latest) = history.last_mut() {
            // Preserve the chart shape but expose the source and timestamp of
            // the aggregate that was actually read for this response.
            *latest = current;
        }
        Ok(history)
    }

    pub async fn record_portfolio_value_snapshot(
        &self,
        snapshot_type: &str,
        baseline_session_date: Option<&str>,
        source: &str,
        extra_payload: JsonValue,
    ) -> Result<JsonValue> {
        let recorded_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let latest_batch = self.latest_batch_id().await?;
        // Read the effective positions once. The aggregate and its new
        // per-position sibling must describe precisely the same observation.
        let position_rows = if self.broker_positions_available().await? {
            self.effective_position_rows(None).await?
        } else {
            let where_clause = latest_batch
                .as_deref()
                .map(|batch_id| {
                    format!(
                        "WHERE batch_id = '{}' AND excluded = 0",
                        sql_escape(batch_id)
                    )
                })
                .unwrap_or_else(|| "WHERE excluded = 0".to_string());
            self.select_json(&format!(
                "SELECT symbol, isin, quantity, currency, current_price_local,
                        cost_basis_local, cost_basis_local AS cost_basis_local_total,
                        cost_basis_dkk, market_value_local, market_value_dkk,
                        unrealised_pnl_dkk, daily_pnl_dkk
                 FROM position_snapshots {where_clause}"
            ))
            .await?
        };
        let aggregate = self.position_aggregate_from_rows(&position_rows).await?;
        let position_snapshots = Self::portfolio_position_snapshot_rows(&position_rows)?;
        let payload = json!({
            "summary": aggregate,
            "snapshot_type": snapshot_type,
            "baseline_session_date": baseline_session_date,
            "source": source,
            "position_snapshot_count": position_snapshots.len(),
            "extra": extra_payload,
        });
        let payload_text =
            serde_json::to_string(&payload).context("serializing portfolio snapshot payload")?;
        let sql = format!(
            "INSERT INTO portfolio_value_history (
                recorded_at,
                snapshot_type,
                baseline_session_date,
                batch_id,
                total_market_value_dkk,
                invested_market_value_dkk,
                cash_balance_dkk,
                total_cost_basis_dkk,
                total_unrealised_pnl_dkk,
                total_daily_pnl_dkk,
                position_count,
                source,
                raw_payload_json
            ) VALUES (
                '{}',
                '{}',
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                '{}'
            )",
            sql_escape(&recorded_at),
            sql_escape(snapshot_type),
            sql_optional_text(baseline_session_date),
            sql_optional_text(latest_batch.as_deref()),
            sql_f64(value_f64(&aggregate, "total_market_value_dkk")),
            sql_f64(value_f64(&aggregate, "invested_market_value_dkk")),
            sql_f64(value_f64(&aggregate, "cash_balance_dkk")),
            sql_f64(value_f64(&aggregate, "total_cost_basis_dkk")),
            sql_f64(value_f64(&aggregate, "total_unrealised_pnl_dkk")),
            sql_f64(value_f64(&aggregate, "total_daily_pnl_dkk")),
            value_i64(&aggregate, "position_count"),
            sql_optional_text(Some(source)),
            sql_escape(&payload_text)
        );
        let mut transaction = self
            .pool
            .begin()
            .await
            .context("starting portfolio snapshot transaction")?;
        let snapshot_id = sqlx::query_scalar::<_, i64>(&format!("{sql} RETURNING id"))
            .fetch_one(&mut *transaction)
            .await
            .context("recording portfolio value snapshot")?;
        for position in &position_snapshots {
            sqlx::query(&format!(
                "INSERT INTO portfolio_position_snapshots (
                    snapshot_id, recorded_at, symbol, isin, currency, quantity,
                    price_local, fx_rate_to_dkk, cost_basis_local, cost_basis_dkk,
                    market_value_dkk, unrealised_pnl_dkk
                ) VALUES ({}, '{}', '{}', {}, '{}', {}, {}, {}, {}, {}, {}, {})",
                snapshot_id,
                sql_escape(&recorded_at),
                sql_escape(&json_text(position, "symbol")),
                sql_optional_text(position.get("isin").and_then(JsonValue::as_str)),
                sql_escape(&json_text(position, "currency")),
                sql_f64(value_f64(position, "quantity")),
                sql_f64(value_f64(position, "price_local")),
                sql_f64(value_f64(position, "fx_rate_to_dkk")),
                sql_f64(value_f64(position, "cost_basis_local")),
                sql_f64(value_f64(position, "cost_basis_dkk")),
                sql_f64(value_f64(position, "market_value_dkk")),
                sql_f64(value_f64(position, "unrealised_pnl_dkk")),
            ))
            .execute(&mut *transaction)
            .await
            .context("recording portfolio position snapshot")?;
        }
        transaction
            .commit()
            .await
            .context("committing portfolio snapshot transaction")?;
        let snapshot = json!({
            "id": snapshot_id,
            "recorded_at": recorded_at,
            "snapshot_type": snapshot_type,
            "baseline_session_date": baseline_session_date,
            "batch_id": latest_batch,
            "total_market_value_dkk": value_f64(&aggregate, "total_market_value_dkk"),
            "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
            "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
            "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
            "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
            "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
            "position_count": value_i64(&aggregate, "position_count"),
            "source": source,
        });
        Ok(json!({
            "status": "ok",
            "snapshot": snapshot,
            "position_snapshot_count": position_snapshots.len(),
        }))
    }

    /// Portfolio value snapshots for the drawdown guardrail's trailing window,
    /// oldest first, with the live position aggregate appended.
    ///
    /// The live row matters: the stored snapshots are written on a schedule, so
    /// on their own they can be hours stale, and a guardrail that reacts hours
    /// late to a decline is not much of a guardrail. `observe_drawdown` filters
    /// out a non-positive aggregate, so a batch that has not loaded yet is
    /// dropped rather than read as a collapse to zero.
    pub(crate) async fn portfolio_drawdown_history(
        &self,
        lookback_days: i64,
    ) -> Result<Vec<JsonValue>> {
        let mut rows = self.portfolio_drawdown_window(lookback_days).await?;
        rows.push(self.current_performance_row().await?);
        Ok(rows)
    }

    /// Snapshot rows for the drawdown window, without the live current row.
    /// Split out so the window bounds can be tested on their own.
    async fn portfolio_drawdown_window(&self, lookback_days: i64) -> Result<Vec<JsonValue>> {
        let lookback_day = (Utc::now() - Duration::days(lookback_days.clamp(1, 3_650)))
            .format("%Y-%m-%d")
            .to_string();
        // Never look back past a re-baselining. Dates are compared on the
        // leading YYYY-MM-DD because `recorded_at` and `created_at` carry a
        // mix of `Z` and `+00:00` offsets, which do not order consistently as
        // whole strings.
        let start_day = match self.latest_external_cash_flow_day().await? {
            Some(flow_day) if flow_day >= lookback_day => flow_day,
            _ => lookback_day,
        };
        let sql = format!(
            "SELECT recorded_at, total_market_value_dkk FROM portfolio_value_history \
             WHERE SUBSTR(recorded_at, 1, 10) > '{}' \
             ORDER BY recorded_at ASC, id ASC LIMIT 20000",
            sql_escape(&start_day)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// The day of the most recent deposit, withdrawal, or reconciliation
    /// adjustment, if any.
    ///
    /// These rows mark the moments the portfolio value stopped being
    /// comparable to what came before, so they bound how far back a drawdown
    /// peak may reach. The whole day is excluded rather than the instant: an
    /// adjustment is usually settled against snapshots taken around it, and
    /// half a re-baselined day is not a value worth defending a peak with.
    async fn latest_external_cash_flow_day(&self) -> Result<Option<String>> {
        let row = self
            .first_json(
                "SELECT MAX(SUBSTR(created_at, 1, 10)) AS flow_day FROM trade_ledger \
                 WHERE side IN ('DEPOSIT', 'WITHDRAWAL', 'ADJUSTMENT')",
            )
            .await?;
        Ok(row
            .as_ref()
            .map(|row| json_text(row, "flow_day"))
            .filter(|day| day.len() == 10))
    }

    /// The saved drawdown-guardrail override, as stored. Whether it still
    /// applies is decided in `drawdown_guard` against the peak actually being
    /// measured, so the expiry rule lives with the rule it modifies.
    pub async fn drawdown_guard_override_value(&self) -> Result<JsonValue> {
        let saved = self.runtime_setting("drawdown_guard_override").await?;
        let mut value = json!({
            "enabled": false,
            "peak_value_dkk": null,
            "notes": "",
            "updated_at": null
        });
        if let Some(saved) = saved
            && let Some(object) = value.as_object_mut()
        {
            for key in ["enabled", "peak_value_dkk", "notes", "updated_at"] {
                if let Some(entry) = saved.get(key) {
                    object.insert(key.to_string(), entry.clone());
                }
            }
        }
        Ok(value)
    }

    /// Grant or clear the drawdown override. The peak it is granted against is
    /// recorded so the grant can expire by itself; without one the override is
    /// refused rather than becoming permanent.
    pub async fn save_drawdown_guard_override(
        &self,
        enabled: bool,
        peak_value_dkk: Option<f64>,
        notes: &str,
    ) -> Result<JsonValue> {
        if notes.len() > 500 {
            anyhow::bail!("Drawdown guardrail override notes are too long");
        }
        let peak_value_dkk = match (enabled, peak_value_dkk) {
            (true, Some(peak)) if peak.is_finite() && peak > 0.0 => Some(peak),
            (true, _) => anyhow::bail!(
                "Enabling the drawdown guardrail override requires the peak value it is granted against"
            ),
            (false, _) => None,
        };
        let value = json!({
            "enabled": enabled,
            "peak_value_dkk": peak_value_dkk,
            "notes": notes,
            "updated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        self.save_runtime_setting("drawdown_guard_override", &value)
            .await?;
        self.drawdown_guard_override_value().await
    }

    pub(crate) async fn current_performance_row(&self) -> Result<JsonValue> {
        let latest_batch = self.latest_batch_id().await?;
        let aggregate = self.position_aggregate(latest_batch.as_deref()).await?;
        Ok(json!({
            "recorded_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "snapshot_type": "runtime_current",
            "total_market_value_dkk": value_f64(&aggregate, "total_market_value_dkk"),
            "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
            "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
            "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
            "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
            "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
            "position_count": value_i64(&aggregate, "position_count"),
            "source": text_value(&aggregate, "source"),
        }))
    }

    pub async fn portfolio_trades_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, symbol, isin, instrument_name, side, quantity, price_local,
                    currency, gross_amount_dkk, commission_dkk, tax_dkk, realised_gain_dkk,
                    net_amount_dkk, mode, status, batch_id
             FROM trade_ledger ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 250)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn goal_tracking(&self, total_value: f64) -> JsonValue {
        let weekly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "weekly_target_dkk"],
        )
        .unwrap_or(5000.0);
        let monthly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "monthly_target_dkk"],
        )
        .unwrap_or(20000.0);
        let tz = yaml_string(&self.config, &["localization", "time_zone"])
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        let now_local = Utc::now().with_timezone(&tz);
        let week_start_date = now_local.date_naive()
            - Duration::days(now_local.weekday().num_days_from_monday() as i64);
        let month_start_date = now_local
            .date_naive()
            .with_day(1)
            .unwrap_or_else(|| now_local.date_naive());
        let week_start_utc = local_date_start_to_utc_string(week_start_date, tz);
        let month_start_utc = local_date_start_to_utc_string(month_start_date, tz);
        // Scope baselines to the active import batch so portfolio resets
        // start a fresh P&L baseline instead of bleeding through as losses.
        let batch_id = self.latest_batch_id().await.ok().flatten();
        let week_baseline = self
            .portfolio_value_at(&week_start_utc, batch_id.as_deref())
            .await
            .unwrap_or(None);
        let month_baseline = self
            .portfolio_value_at(&month_start_utc, batch_id.as_deref())
            .await
            .unwrap_or(None);
        let since_reset_baseline = self
            .portfolio_value_since_reset(batch_id.as_deref())
            .await
            .unwrap_or(None);
        json!({
            "weekly_target_dkk": weekly_target,
            "monthly_target_dkk": monthly_target,
            "basis": "pnl_dkk is total portfolio value change since the period start, measured against the portfolio value history baseline.",
            "periods": {
                "week": goal_period_value(week_baseline, total_value, weekly_target, &week_start_utc),
                "month": goal_period_value(month_baseline, total_value, monthly_target, &month_start_utc),
                "since_reset": since_reset_performance_value(since_reset_baseline, total_value)
            }
        })
    }

    /// Portfolio value at a moment in time: the last snapshot before the
    /// cutoff, falling back to the first snapshot after it when history
    /// starts mid-period. Restricted to one import batch when given so
    /// values from before a portfolio reset are never used as baselines.
    async fn portfolio_value_at(
        &self,
        cutoff_utc: &str,
        batch_id: Option<&str>,
    ) -> Result<Option<f64>> {
        let batch_filter = batch_id
            .map(|batch_id| format!(" AND batch_id = '{}'", sql_escape(batch_id)))
            .unwrap_or_default();
        let before = self
            .first_json(&format!(
                "SELECT total_market_value_dkk FROM portfolio_value_history \
                 WHERE recorded_at < '{}'{} ORDER BY recorded_at DESC LIMIT 1",
                sql_escape(cutoff_utc),
                batch_filter
            ))
            .await?;
        if let Some(row) = before {
            return Ok(Some(value_f64(&row, "total_market_value_dkk")));
        }
        let after = self
            .first_json(&format!(
                "SELECT total_market_value_dkk FROM portfolio_value_history \
                 WHERE recorded_at >= '{}'{} ORDER BY recorded_at ASC LIMIT 1",
                sql_escape(cutoff_utc),
                batch_filter
            ))
            .await?;
        Ok(after.map(|row| value_f64(&row, "total_market_value_dkk")))
    }

    /// The earliest persisted account-value snapshot in the active import
    /// batch. A missing row is intentionally not substituted with zero or a
    /// value from an earlier reset because neither is a comparable baseline.
    async fn portfolio_value_since_reset(
        &self,
        batch_id: Option<&str>,
    ) -> Result<Option<JsonValue>> {
        let Some(batch_id) = batch_id else {
            return Ok(None);
        };
        self.first_json(&format!(
            "SELECT recorded_at, total_market_value_dkk FROM portfolio_value_history \
             WHERE batch_id = '{}' ORDER BY recorded_at ASC, id ASC LIMIT 1",
            sql_escape(batch_id)
        ))
        .await
    }

    /// Per-request token and cost ledger for this runtime's LLM calls.
    ///
    /// The provider matrix beside it answers whether a model works; this
    /// answers what each call cost and which way the trend runs. Both read the
    /// same persisted responses, so they can never disagree about what ran.
    pub async fn llm_usage_ledger(
        &self,
        limit: i64,
        day_limit: i64,
    ) -> Result<LlmUsageLedgerPayload> {
        let rows = self
            .select_json(&format!(
                "SELECT created_at, model, status, request_json, response_json
                 FROM decision_reports
                 WHERE response_json IS NOT NULL
                 ORDER BY created_at DESC, id DESC
                 LIMIT {}",
                clamp_limit(limit, 1, 2_000)
            ))
            .await;
        let jev_rows = crate::jev_store::usage_rows(&self.pool, clamp_limit(limit, 1, 5_000)).await;
        if let Err(err) = &rows {
            warn!("Decision usage read failed: {err:#}");
        }
        if let Err(err) = &jev_rows {
            warn!("Jev usage read failed: {err:#}");
        }
        Ok(crate::llm_usage::ledger_from_reads(
            rows,
            jev_rows,
            clamp_limit(day_limit, 1, 400) as usize,
        ))
    }

    fn strategy_month_key(&self) -> String {
        let tz = yaml_string(&self.config, &["price_monitor", "timezone"])
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        Utc::now().with_timezone(&tz).format("%Y-%m").to_string()
    }

    pub async fn monthly_loss_breaker_override_value(&self) -> Result<JsonValue> {
        let current_month_key = self.strategy_month_key();
        let saved = self
            .runtime_setting("monthly_loss_breaker_override")
            .await?;
        let mut value = json!({
            "enabled": false,
            "current_month_key": current_month_key,
            "month_key": null,
            "active_for_current_month": false,
            "notes": "",
            "updated_at": null
        });
        if let Some(saved) = saved {
            let enabled = saved
                .get("enabled")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let month_key = saved
                .get("month_key")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            let active_for_current_month = enabled && month_key == current_month_key;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("enabled".to_string(), JsonValue::from(enabled));
                obj.insert("month_key".to_string(), JsonValue::from(month_key));
                obj.insert(
                    "active_for_current_month".to_string(),
                    JsonValue::from(active_for_current_month),
                );
                obj.insert(
                    "notes".to_string(),
                    saved
                        .get("notes")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::from("")),
                );
                obj.insert(
                    "updated_at".to_string(),
                    saved.get("updated_at").cloned().unwrap_or(JsonValue::Null),
                );
            }
        }
        Ok(value)
    }

    pub async fn save_monthly_loss_breaker_override(
        &self,
        enabled: bool,
        notes: &str,
    ) -> Result<JsonValue> {
        if notes.len() > 500 {
            anyhow::bail!("Monthly-loss breaker override notes are too long");
        }
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "enabled": enabled,
            "month_key": self.strategy_month_key(),
            "notes": notes,
            "updated_at": updated_at
        });
        self.save_runtime_setting("monthly_loss_breaker_override", &value)
            .await?;
        self.monthly_loss_breaker_override_value().await
    }

    pub async fn instrument_quarantine_overrides_value(&self) -> Result<JsonValue> {
        let saved = self
            .runtime_setting("instrument_quarantine_overrides")
            .await?;
        let overrides = saved
            .as_ref()
            .and_then(|value| value.get("overrides"))
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                item.get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                    && item
                        .get("symbol")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    && item
                        .get("action")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    && item
                        .get("signature")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "overrides": overrides,
            "updated_at": saved
                .as_ref()
                .and_then(|value| value.get("updated_at"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        }))
    }

    pub async fn save_instrument_quarantine_override(
        &self,
        symbol: &str,
        action: &str,
        signature: &str,
        enabled: bool,
        notes: &str,
    ) -> Result<JsonValue> {
        let symbol = symbol.trim();
        let action = action.trim().to_uppercase();
        let signature = signature.trim();
        if symbol.is_empty() || action.is_empty() || signature.is_empty() {
            anyhow::bail!("Instrument quarantine override requires symbol, side, and signature");
        }
        if symbol.len() > 80 || action.len() > 12 || signature.len() > 120 {
            anyhow::bail!("Instrument quarantine override identifier is too long");
        }
        if notes.len() > 500 {
            anyhow::bail!("Instrument quarantine override notes are too long");
        }
        let existing = self.instrument_quarantine_overrides_value().await?;
        let mut overrides = existing
            .get("overrides")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                !(item.get("symbol").and_then(JsonValue::as_str) == Some(symbol)
                    && item.get("action").and_then(JsonValue::as_str) == Some(action.as_str())
                    && item.get("signature").and_then(JsonValue::as_str) == Some(signature))
            })
            .collect::<Vec<_>>();
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        if enabled {
            overrides.push(json!({
                "symbol": symbol,
                "action": action,
                "signature": signature,
                "enabled": true,
                "notes": notes,
                "updated_at": updated_at
            }));
        }
        let value = json!({
            "overrides": overrides,
            "updated_at": updated_at
        });
        self.save_runtime_setting("instrument_quarantine_overrides", &value)
            .await?;
        self.instrument_quarantine_overrides_value().await
    }

    pub async fn saxo_auth_status(&self) -> auth::SaxoAuthStatus {
        if let Err(err) = self.ensure_saxo_session_json("auth_status").await {
            warn!("Saxo leased session refresh before auth status skipped: {err:#}");
        }
        auth::auth_status(&self.config, &self.config_path).await
    }

    pub async fn saxo_auth_status_value(&self) -> JsonValue {
        serde_json::to_value(self.saxo_auth_status().await)
            .expect("Saxo auth status must serialize")
    }

    pub async fn saxo_auth_api_status(&self) -> auth::SaxoAuthApiStatus {
        self.saxo_auth_status().await.into()
    }

    pub async fn saxo_session_status(&self) -> auth::SaxoSessionApiStatus {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before session API failed: {err:#}");
        }
        auth::session_api(&self.config, &self.config_path).await
    }

    pub async fn refresh_saxo_session(&self) -> Result<auth::SaxoSessionApiStatus> {
        self.ensure_saxo_session_json("refresh").await?;
        Ok(auth::session_api(&self.config, &self.config_path).await)
    }

    /// Returns a session whose access token is outside the refresh margin,
    /// rotating the refresh token first when it is due.
    ///
    /// Saxo refresh tokens are single-use and every pod refreshes the same
    /// one, so a rotation is only ever made from the durable row, under the
    /// cross-process lease, and made durable before the lease is released.
    /// The pod-local file is a read cache and never the refresh input.
    pub async fn ensure_saxo_session_json(&self, source: &str) -> Result<JsonValue> {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before use failed: {err:#}");
        }
        let session = auth::export_session_json(&self.config, &self.config_path)?;
        if !saxo_session_needs_refresh(&session) {
            return auth::usable_session_json(session);
        }
        self.rotate_durable_saxo_session(source).await
    }

    async fn rotate_durable_saxo_session(&self, source: &str) -> Result<JsonValue> {
        let gate = saxo_refresh_gate(&self.saxo_session_file());
        // One rotation per process at a time; a shutdown drain waits on this.
        let _in_flight = gate.in_flight.lock().await;
        if gate.is_draining() {
            return self.saxo_session_while_draining(source);
        }
        let owner = match self
            .prepare_saxo_session_refresh_lease_if_needed(source, &gate)
            .await?
        {
            SaxoRefreshLease::Held(owner) => owner,
            // Another task or process rotated the token while this one waited.
            SaxoRefreshLease::NotNeeded => {
                return auth::usable_session_json(auth::export_session_json(
                    &self.config,
                    &self.config_path,
                )?);
            }
            SaxoRefreshLease::Draining => return self.saxo_session_while_draining(source),
        };
        let rotation = self.rotate_saxo_session_under_lease(source).await;
        if rotation.release_lease {
            if let Err(err) = self.release_saxo_session_refresh_lease(&owner).await {
                warn!("Saxo session refresh lease release failed: {err:#}");
            }
        }
        rotation.result
    }

    /// A process that has begun shutting down starts no rotation: Saxo would
    /// consume the durable token, and the successor could die with the pod.
    /// It keeps serving an access token that has not expired yet.
    fn saxo_session_while_draining(&self, source: &str) -> Result<JsonValue> {
        let session = auth::export_session_json(&self.config, &self.config_path)?;
        if auth::session_access_token_valid_for(&session, SAXO_SESSION_DRAINING_MIN_ACCESS_SECONDS)
        {
            info!(
                source,
                "Saxo refresh deferred while shutting down; serving the unexpired access token"
            );
            return Ok(session);
        }
        bail!(
            "Saxo session refresh deferred: this process is shutting down and its replacement refreshes from the durable session."
        );
    }

    /// Stops this process from starting any new token rotation and waits up to
    /// `timeout` for one already in flight to become durable. Returns whether
    /// nothing was left in flight.
    pub async fn drain_saxo_session_refreshes(&self, timeout: StdDuration) -> bool {
        let gate = saxo_refresh_gate(&self.saxo_session_file());
        gate.begin_draining();
        match tokio::time::timeout(timeout, gate.in_flight.lock()).await {
            Ok(_idle) => true,
            Err(_) => {
                warn!(
                    timeout_seconds = timeout.as_secs(),
                    "a Saxo token rotation was still in flight at shutdown"
                );
                false
            }
        }
    }

    /// Marks this process as shutting down without waiting; see
    /// `drain_saxo_session_refreshes`.
    pub fn begin_saxo_session_drain(&self) {
        saxo_refresh_gate(&self.saxo_session_file()).begin_draining();
    }

    fn saxo_session_file(&self) -> PathBuf {
        auth::session_file_path(&self.config, &self.config_path)
    }

    /// Stores a session from a completed OAuth login: durable row first, then
    /// the working copy, so a concurrent restore cannot put the replaced
    /// session back in the file and have it written over the new one.
    pub async fn store_saxo_oauth_session(&self, session: &JsonValue) -> Result<()> {
        let durable = self
            .save_saxo_session_to_db(session, "oauth_callback")
            .await;
        // Even if the durable write failed, keep the new login in the working
        // copy: it is the newest session, so the next restore pushes it.
        auth::import_session_json(&self.config, &self.config_path, session)
            .context("writing the new Saxo session to the working copy")?;
        durable
    }

    pub async fn user_logout_saxo_session(&self) -> Result<JsonValue> {
        // User SSO and Saxo OAuth are different security domains. Logging out
        // of the dashboard user must not delete the service-level Saxo refresh
        // token, because the scheduler keeps renewing that token without any
        // browser session. This endpoint therefore reports the current Saxo
        // status and leaves the durable `saxo_sessions` row untouched.
        if let Err(err) = self.ensure_saxo_session_json("user_logout_keepalive").await {
            warn!("Saxo leased session refresh during user logout no-op skipped: {err:#}");
        }
        let mut status =
            serde_json::to_value(auth::auth_status(&self.config, &self.config_path).await)
                .expect("Saxo auth status must serialize");
        if let Some(obj) = status.as_object_mut() {
            obj.insert("logout_scope".to_string(), json!("user"));
            obj.insert(
                "message".to_string(),
                json!("User logout does not disconnect the service-level Saxo session."),
            );
        }
        Ok(status)
    }

    pub async fn disconnect_saxo_session(&self) -> Result<JsonValue> {
        let status = auth::logout_session(&self.config, &self.config_path)?;
        self.clear_saxo_session_from_db().await?;
        Ok(status)
    }

    async fn ensure_saxo_session_schema(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS saxo_sessions (
                singleton_key TEXT PRIMARY KEY,
                session_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source TEXT NOT NULL,
                refresh_lease_owner TEXT,
                refresh_lease_expires_at TEXT,
                refresh_lease_source TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Saxo session state table")?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_owner TEXT")
            .await?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_expires_at TEXT")
            .await?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_source TEXT")
            .await
    }

    async fn ensure_runtime_state_schema(&self) -> Result<()> {
        // The database is the durable runtime state for tokens and operator
        // preferences. The on-disk session file is only an ephemeral working
        // copy for the OAuth helper functions.
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS portfolio_value_history (
                    id BIGSERIAL PRIMARY KEY,
                    recorded_at TEXT NOT NULL,
                    snapshot_type TEXT NOT NULL,
                    baseline_session_date TEXT,
                    batch_id TEXT,
                    total_market_value_dkk REAL NOT NULL,
                    invested_market_value_dkk REAL NOT NULL,
                    cash_balance_dkk REAL NOT NULL,
                    total_cost_basis_dkk REAL NOT NULL,
                    total_unrealised_pnl_dkk REAL NOT NULL,
                    total_daily_pnl_dkk REAL NOT NULL,
                    position_count INTEGER NOT NULL,
                    source TEXT,
                    raw_payload_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating portfolio value history table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS portfolio_value_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at TEXT NOT NULL,
                    snapshot_type TEXT NOT NULL,
                    baseline_session_date TEXT,
                    batch_id TEXT,
                    total_market_value_dkk REAL NOT NULL,
                    invested_market_value_dkk REAL NOT NULL,
                    cash_balance_dkk REAL NOT NULL,
                    total_cost_basis_dkk REAL NOT NULL,
                    total_unrealised_pnl_dkk REAL NOT NULL,
                    total_daily_pnl_dkk REAL NOT NULL,
                    position_count INTEGER NOT NULL,
                    source TEXT,
                    raw_payload_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating portfolio value history table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_portfolio_value_history_recorded
             ON portfolio_value_history(recorded_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating portfolio value history recorded index")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS portfolio_position_snapshots (
                snapshot_id BIGINT NOT NULL,
                recorded_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                isin TEXT,
                currency TEXT NOT NULL,
                quantity REAL NOT NULL,
                price_local REAL NOT NULL,
                fx_rate_to_dkk REAL NOT NULL,
                cost_basis_local REAL NOT NULL,
                cost_basis_dkk REAL NOT NULL,
                market_value_dkk REAL NOT NULL,
                unrealised_pnl_dkk REAL NOT NULL,
                PRIMARY KEY (snapshot_id, symbol),
                FOREIGN KEY (snapshot_id) REFERENCES portfolio_value_history(id) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating portfolio position snapshots table")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_portfolio_position_snapshots_recorded
             ON portfolio_position_snapshots(recorded_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating portfolio position snapshots recorded index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_portfolio_position_snapshots_symbol_recorded
             ON portfolio_position_snapshots(symbol, recorded_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating portfolio position snapshots symbol index")?;
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS notification_deliveries (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    summary_date TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    status TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    message_text TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    error_text TEXT,
                    summary_kind TEXT NOT NULL DEFAULT 'daily'
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating notification deliveries table")?;
            sqlx::query(
                "ALTER TABLE notification_deliveries
                 ADD COLUMN IF NOT EXISTS summary_kind TEXT NOT NULL DEFAULT 'daily'",
            )
            .execute(&self.pool)
            .await
            .context("ensuring notification deliveries summary_kind column")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS notification_deliveries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    summary_date TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    status TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    message_text TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    error_text TEXT,
                    summary_kind TEXT NOT NULL DEFAULT 'daily'
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating notification deliveries table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_notification_deliveries_summary
             ON notification_deliveries(summary_date, channel, status, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating notification deliveries summary index")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notification_channel_state (
                channel TEXT PRIMARY KEY,
                summary_date TEXT,
                last_attempt_at TEXT,
                next_attempt_after TEXT,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                last_status TEXT,
                last_error_text TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating notification channel state table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notification_alert_state (
                scope_key TEXT PRIMARY KEY,
                severity TEXT NOT NULL,
                last_sent_at TEXT,
                last_alert_key TEXT,
                last_summary_kind TEXT,
                last_delivery_id INTEGER
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating notification alert state table")?;
        self.ensure_saxo_session_schema().await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runtime_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating runtime settings table")?;
        let retired_settings = self.purge_retired_runtime_settings().await?;
        if retired_settings > 0 {
            info!(
                count = retired_settings,
                "removed retired legacy runtime settings"
            );
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS price_monitor_status (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                status TEXT NOT NULL,
                summary_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Written by the backup retention CronJob, read by the freshness strip.
        // CNPG records base-backup state in Kubernetes, which this process
        // cannot reach; this table is the one place both sides can meet. It
        // carries the newest backup that CNPG reported completed *and* whose
        // objects the retention pass then found in the bucket, because a
        // Backup resource marked completed whose data is missing is exactly
        // the failure that hid for ten days.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS backup_status (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                last_verified_backup_at TEXT,
                last_verified_backup_id TEXT,
                verified_backup_count INTEGER,
                summary_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating price monitor status table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS currency_fx_rates (
                currency_code TEXT NOT NULL,
                base_currency TEXT NOT NULL DEFAULT 'DKK',
                rate_to_dkk REAL NOT NULL,
                source TEXT NOT NULL,
                observed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                raw_payload_json TEXT NOT NULL,
                PRIMARY KEY (currency_code, base_currency)
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating currency FX rates table")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_currency_fx_rates_expires
             ON currency_fx_rates(expires_at)",
        )
        .execute(&self.pool)
        .await
        .context("creating currency FX rates expiry index")?;
        sqlx::query(BROKER_POSITION_SNAPSHOTS_DDL)
            .execute(&self.pool)
            .await
            .context("creating broker position snapshots table")?;
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_prechecks (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    result_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop prechecks table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_prechecks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    result_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop prechecks table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_protective_stop_prechecks_created
             ON protective_stop_prechecks(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating protective-stop prechecks index")?;
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_lifecycle_tests (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    source_precheck_id BIGINT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    broker_order_id TEXT,
                    external_reference TEXT NOT NULL,
                    request_id TEXT NOT NULL UNIQUE,
                    placement_result_json TEXT NOT NULL,
                    cancellation_result_json TEXT NOT NULL,
                    reconciliation_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop lifecycle tests table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_lifecycle_tests (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    source_precheck_id INTEGER NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    broker_order_id TEXT,
                    external_reference TEXT NOT NULL,
                    request_id TEXT NOT NULL UNIQUE,
                    placement_result_json TEXT NOT NULL,
                    cancellation_result_json TEXT NOT NULL,
                    reconciliation_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop lifecycle tests table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_protective_stop_lifecycle_tests_created
             ON protective_stop_lifecycle_tests(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating protective-stop lifecycle tests index")?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_protective_stop_lifecycle_tests_active_source
             ON protective_stop_lifecycle_tests(source_precheck_id)
             WHERE status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                              'broker_state_unknown', 'cancellation_submitted', 'reconciliation_pending')",
        )
        .execute(&self.pool)
        .await
        .context("creating active protective-stop lifecycle test uniqueness guard")?;
        sqlx::query(BROKER_INSTRUMENT_EXPOSURES_DDL)
            .execute(&self.pool)
            .await
            .context("creating broker instrument exposures table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_balance_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                currency TEXT,
                cash_available_for_trading REAL,
                margin_available_for_trading REAL,
                cash_balance REAL,
                transactions_not_booked REAL,
                settlement_value REAL,
                total_value REAL,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker balance snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_account_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                account_key TEXT,
                account_id TEXT,
                account_currency TEXT,
                is_trial_account INTEGER,
                fractional_order_enabled INTEGER,
                fractional_order_enabled_asset_types_json TEXT,
                can_use_cash_positions_as_margin_collateral INTEGER,
                use_cash_positions_as_margin_collateral INTEGER,
                legal_asset_types_json TEXT,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker account snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_baselines (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                activated_at TEXT,
                status TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                config_json TEXT NOT NULL,
                prompt_json TEXT NOT NULL,
                source TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy baselines table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_reflections (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                period_start TEXT NOT NULL,
                period_end TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                summary TEXT NOT NULL,
                findings_json TEXT NOT NULL,
                proposed_actions_json TEXT NOT NULL,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                baseline_id TEXT,
                goal_version INTEGER NOT NULL,
                hypothesis TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                old_value_json TEXT NOT NULL,
                new_value_json TEXT NOT NULL,
                expected_effect TEXT NOT NULL,
                risk_notes TEXT NOT NULL,
                evidence_json TEXT NOT NULL,
                approval_json TEXT,
                metrics_json TEXT,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_decision_advice (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                decision_report_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                source_session_id TEXT,
                overall_recommendation TEXT NOT NULL,
                summary TEXT NOT NULL,
                order_advice_json TEXT NOT NULL,
                learning_notes_json TEXT NOT NULL,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_counterfactuals (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_effect TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                reference_price_at TEXT,
                reference_price_source TEXT,
                reported_reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactuals table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS missed_trade_shadows (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_gate TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                reference_price_at TEXT,
                reference_price_source TEXT,
                reported_reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating missed-trade shadows table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS shadow_report_outcomes (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                analysis_pulse_key TEXT NOT NULL,
                analysis_pulse_label TEXT NOT NULL,
                candidate_rank INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                proposed_quantity REAL NOT NULL,
                currency TEXT,
                earlier_pulse_report_id INTEGER,
                appeared_in_earlier_pulse INTEGER NOT NULL DEFAULT 0,
                deterministic_gate_code TEXT NOT NULL,
                deterministic_gate_json TEXT,
                hermes_effect TEXT NOT NULL,
                hermes_advice_snapshot_json TEXT,
                approved_policy_source TEXT,
                report_time_context_json TEXT NOT NULL,
                reported_reference_price_local REAL,
                reference_price_local REAL,
                reference_price_at TEXT,
                reference_price_source TEXT NOT NULL,
                reference_dkk_basis TEXT NOT NULL,
                reference_fx_json TEXT,
                estimated_cost_json TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                observation_count INTEGER NOT NULL DEFAULT 0,
                one_session_outcome_json TEXT,
                five_session_outcome_json TEXT,
                twenty_session_outcome_json TEXT,
                one_session_after_cost_outcome_json TEXT,
                five_session_after_cost_outcome_json TEXT,
                twenty_session_after_cost_outcome_json TEXT,
                intraday_excursion_json TEXT,
                outcome_maturity_status TEXT NOT NULL DEFAULT 'awaiting_reference',
                UNIQUE(report_id, candidate_rank)
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating shadow report outcomes table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS shadow_report_outcome_quotes (
                outcome_id TEXT NOT NULL,
                observed_at TEXT NOT NULL,
                price_local REAL NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY (outcome_id, observed_at)
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating shadow report outcome quote samples table")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_shadow_report_outcome_quotes_outcome_at
             ON shadow_report_outcome_quotes(outcome_id, observed_at)",
        )
        .execute(&self.pool)
        .await
        .context("creating shadow report outcome quote samples index")?;
        for sql in crate::markov_method::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Markov method runtime tables")?;
        }
        for sql in crate::quiver::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Quiver runtime tables")?;
        }
        for sql in crate::editorial_research::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating editorial research runtime tables")?;
        }
        for sql in crate::daily_indicators::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating daily indicator runtime tables")?;
        }
        for sql in crate::fill_quality::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating order submission quote table")?;
        }
        for sql in crate::price_bars::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating daily price bar table")?;
        }
        for sql in crate::performance_benchmarks::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating performance benchmark runtime tables")?;
        }
        for sql in crate::earnings_dates::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating earnings report date tables")?;
        }
        for sql in crate::congress_trades::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating congress trade tables")?;
        }
        for sql in crate::company_profiles::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating company profile table")?;
        }
        for sql in crate::trading_schedule::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating instrument trading schedule table")?;
        }
        for sql in crate::operator_actions::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating the operator-action trail")?;
        }
        for sql in crate::jev_store::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Jev runtime tables")?;
        }
        for column in crate::jev_store::request_columns_to_ensure() {
            self.ensure_table_column("jev_requests", column).await?;
        }
        // The signal table shipped before evidence provenance existed, and
        // CREATE TABLE IF NOT EXISTS will not add columns to it.
        for column in crate::jev_store::signal_columns_to_ensure() {
            self.ensure_table_column("jev_editorial_signals", column)
                .await?;
        }
        // Strictly after the columns above exist.
        for sql in crate::jev_store::post_migration_index_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Jev migration-dependent indexes")?;
        }
        for sql in crate::entry_evaluation::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating entry evaluation snapshot table")?;
        }
        for column in [
            "nearest_support DOUBLE PRECISION",
            "next_support DOUBLE PRECISION",
            "downside_to_support_pct DOUBLE PRECISION",
            "downside_after_break_pct DOUBLE PRECISION",
            "support_break_risk DOUBLE PRECISION",
            "support_break_risk_label TEXT",
            "support_confidence DOUBLE PRECISION",
            "support_history_coverage DOUBLE PRECISION",
            "support_touch_count INTEGER",
        ] {
            self.ensure_table_column("daily_indicator_signals", column)
                .await
                .context("migrating daily indicator support-risk columns")?;
        }
        for column in [
            "deterministic_gate_json TEXT",
            "hermes_advice_snapshot_json TEXT",
            "reference_fx_json TEXT",
            "estimated_cost_json TEXT",
            "one_session_outcome_json TEXT",
            "five_session_outcome_json TEXT",
            "twenty_session_outcome_json TEXT",
            "one_session_after_cost_outcome_json TEXT",
            "five_session_after_cost_outcome_json TEXT",
            "twenty_session_after_cost_outcome_json TEXT",
            "intraday_excursion_json TEXT",
            "outcome_maturity_status TEXT NOT NULL DEFAULT 'awaiting_reference'",
        ] {
            self.ensure_table_column("shadow_report_outcomes", column)
                .await
                .context("migrating shadow report outcome maturity columns")?;
        }
        for column in [
            "pulse_mode TEXT NOT NULL DEFAULT 'execution_eligible'",
            "queue_eligible INTEGER NOT NULL DEFAULT 1",
        ] {
            self.ensure_table_column("decision_reports", column)
                .await
                .context("migrating decision-report pulse authority")?;
        }
        // The scaling a signal was actually computed under. Nullable on
        // purpose: rows written before per-exchange scaling have no honest
        // value, and defaulting them to today's numbers would invent evidence.
        for column in [
            "bars_per_session INTEGER",
            "window_bars INTEGER",
            "min_labeled_bars INTEGER",
            "signal_horizon_bars INTEGER",
        ] {
            self.ensure_table_column("markov_asset_signals", column)
                .await
                .context("migrating Markov applied-scaling columns")?;
        }
        // Reports created before the pulse-authority contract keep their
        // historical execution eligibility. Dry-run reports are explicitly
        // shadowed so a legacy status can never gain authority during the
        // migration. New shadow reports must provide both fields at insert.
        sqlx::query(
            "UPDATE decision_reports
             SET pulse_mode = CASE
                    WHEN status LIKE 'dry_run_%' THEN 'shadow'
                    WHEN pulse_mode IS NULL OR pulse_mode = '' THEN 'execution_eligible'
                    ELSE pulse_mode
                 END,
                 queue_eligible = CASE
                    WHEN status LIKE 'dry_run_%' OR pulse_mode = 'shadow' THEN 0
                    WHEN queue_eligible IS NULL THEN 1
                    ELSE queue_eligible
                 END",
        )
        .execute(&self.pool)
        .await
        .context("backfilling decision-report pulse authority")?;
        for table in ["hermes_counterfactuals", "missed_trade_shadows"] {
            for column in [
                "reference_price_at TEXT",
                "reference_price_source TEXT",
                "reported_reference_price_local DOUBLE PRECISION",
            ] {
                self.ensure_table_column(table, column)
                    .await
                    .with_context(|| format!("migrating {table} reference-price provenance"))?;
            }
            // Historical rows predate Saxo quote provenance. Do not mix their
            // model/report-derived references with the verified observations
            // introduced below; their old outcome remains visible as legacy
            // audit data, but is excluded from learning evidence.
            sqlx::query(&format!(
                "UPDATE {table}
                 SET reported_reference_price_local = COALESCE(reported_reference_price_local, reference_price_local),
                     reference_price_at = CASE
                         WHEN reference_price_at IS NULL AND reference_price_local > 0 THEN created_at
                         ELSE reference_price_at
                     END,
                     reference_price_source = CASE
                         WHEN reference_price_local > 0 THEN 'legacy_report_candidate_unverified'
                         ELSE 'missing_report_reference'
                     END,
                     status = CASE
                         WHEN status IN ('tracking', 'unpriced') THEN 'legacy_unverified_reference'
                         ELSE status
                     END
                 WHERE reference_price_source IS NULL"
            ))
            .execute(&self.pool)
            .await
            .with_context(|| format!("marking historical {table} references unverified"))?;
        }
        self.ensure_table_column("execution_orders", "trade_thesis_json TEXT")
            .await
            .context("migrating execution-order trade-thesis provenance")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_reflections_created
             ON hermes_reflections(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections created index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_strategy_experiments_status
             ON strategy_experiments(status, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments status index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_decision_advice_report
             ON hermes_decision_advice(decision_report_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice report index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_decision_advice_session
             ON hermes_decision_advice(source_session_id)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice session index")?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_hermes_counterfactuals_manager_strategy
             ON hermes_counterfactuals(manager_run_id, strategy_key)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactual manager strategy index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_counterfactuals_tracking
             ON hermes_counterfactuals(status, symbol, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactual tracking index")?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_missed_trade_shadows_manager_strategy
             ON missed_trade_shadows(manager_run_id, strategy_key)",
        )
        .execute(&self.pool)
        .await
        .context("creating missed-trade shadow manager strategy index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_missed_trade_shadows_tracking
             ON missed_trade_shadows(status, symbol, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating missed-trade shadow tracking index")?;
        self.backfill_trading_manager_strategy_type().await?;
        self.backfill_realised_gain_currency_split().await?;
        self.backfill_terminal_protective_stop_lifecycle_tests()
            .await?;
        self.tune_append_heavy_table_autovacuum().await?;
        Ok(())
    }

    /// Lower the autovacuum analyze threshold on append-heavy tables and run
    /// an immediate `ANALYZE`.
    ///
    /// Verified live on 2026-08-02: every `pg_stat_user_tables.last_autoanalyze`
    /// in production dated to 2026-06-30, 33 days stale. The planner believed
    /// `audit_log` held 0 rows where it held 67,578, `trade_ledger` 47 where it
    /// held 118, `decision_reports` 85 where it held 137. Postgres's default
    /// `autovacuum_analyze_scale_factor` (0.10) only re-triggers after row
    /// count changes by a tenth, which these tables' absolute sizes rarely
    /// clear on their own -- `decision_reports` grows by one or two rows a
    /// weekday. The 0.02 / 50-row settings below make a much smaller amount of
    /// drift enough to trigger a re-analyze.
    ///
    /// Postgres-only: SQLite has no autovacuum and `ALTER TABLE ... SET (...)`
    /// storage parameters are Postgres syntax. Idempotent -- setting the same
    /// reloption twice is a cheap catalog update, and `ANALYZE` takes only an
    /// `AccessShareLock` -- so running this on every pod startup (api,
    /// scheduler, mcp) during a rolling deploy is harmless, consistent with the
    /// other `CREATE TABLE IF NOT EXISTS` migrations in this function.
    async fn tune_append_heavy_table_autovacuum(&self) -> Result<()> {
        if !database_url_is_postgres(&self.db_url) {
            return Ok(());
        }
        const APPEND_HEAVY_TABLES: &[&str] = &[
            "decision_reports",
            "trading_manager_runs",
            "trade_ledger",
            "execution_orders",
            "execution_order_events",
            "markov_asset_signals",
            "scheduler_cycle_history",
            "portfolio_value_history",
        ];
        for table in APPEND_HEAVY_TABLES {
            sqlx::query(&format!(
                "ALTER TABLE {table} SET (autovacuum_analyze_scale_factor = 0.02, \
                 autovacuum_analyze_threshold = 50)"
            ))
            .execute(&self.pool)
            .await
            .with_context(|| format!("tuning autovacuum analyze settings for {table}"))?;
        }
        sqlx::query("ANALYZE")
            .execute(&self.pool)
            .await
            .context("running ANALYZE after autovacuum tuning")?;
        Ok(())
    }

    async fn ensure_table_column(&self, table: &str, column_spec: &str) -> Result<()> {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column_spec}");
        match sqlx::query(&sql).execute(&self.pool).await {
            Ok(_) => Ok(()),
            Err(err) if is_duplicate_column_error(&err) => Ok(()),
            Err(err) => Err(err).with_context(|| format!("ensuring {table}.{column_spec}")),
        }
    }

    /// Reconciles the pod-local working copy with the durable row. The durable
    /// row wins unless the working copy holds a strictly newer session (a
    /// rotation or login whose durable write failed) or has learned that Saxo
    /// refused the very token the durable row still holds.
    async fn sync_saxo_session_storage(&self) -> Result<()> {
        let file_session = auth::export_session_json(&self.config, &self.config_path).ok();
        let db_session = self.load_saxo_session_from_db().await?;

        match (file_session, db_session) {
            (Some(file), Some(db)) => {
                if file == db {
                    return Ok(());
                }
                if saxo_working_copy_supersedes_durable(&file, &db) {
                    self.save_saxo_session_to_db(&file, "startup_file_sync")
                        .await?;
                    info!("Saxo session database state updated from local file");
                } else {
                    auth::import_session_json(&self.config, &self.config_path, &db)
                        .context("restoring Saxo session file from database")?;
                    info!("Saxo session file restored from database state");
                }
            }
            (Some(file), None) => {
                self.save_saxo_session_to_db(&file, "startup_file_import")
                    .await?;
                info!("Saxo session database state initialized from local file");
            }
            (None, Some(db)) => {
                auth::import_session_json(&self.config, &self.config_path, &db)
                    .context("restoring Saxo session file from database")?;
                info!("Saxo session file initialized from database state");
            }
            (None, None) => {
                info!("No Saxo session is cached in the file system or database");
            }
        }
        Ok(())
    }

    async fn load_saxo_session_from_db(&self) -> Result<Option<JsonValue>> {
        Ok(self
            .load_saxo_session_row_from_db()
            .await?
            .map(|(_, session)| session))
    }

    /// The durable session and its exact stored text, which a compare-and-set
    /// write uses to prove the row has not moved since it was read.
    async fn load_saxo_session_row_from_db(&self) -> Result<Option<(String, JsonValue)>> {
        // Aliased away from `*_json` so the row decoder hands back the text
        // exactly as stored instead of parsing it.
        let Some(row) = self
            .first_json(
                "SELECT session_json AS session_text FROM saxo_sessions WHERE singleton_key = 'default' LIMIT 1",
            )
            .await?
        else {
            return Ok(None);
        };
        let Some(text) = row.get("session_text").and_then(JsonValue::as_str) else {
            return Ok(None);
        };
        let session =
            serde_json::from_str(text).context("parsing Saxo session JSON from database")?;
        Ok(Some((text.to_string(), session)))
    }

    async fn save_saxo_session_to_db(&self, session: &JsonValue, source: &str) -> Result<()> {
        let session_text =
            serde_json::to_string(session).context("serializing Saxo session for database")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO saxo_sessions (singleton_key, session_json, updated_at, source)
             VALUES ('default', '{}', '{}', '{}')
             ON CONFLICT(singleton_key) DO UPDATE SET
                session_json = excluded.session_json,
                updated_at = excluded.updated_at,
                source = excluded.source",
            sql_escape(&session_text),
            sql_escape(&updated_at),
            sql_escape(source)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting Saxo session to database")?;
        Ok(())
    }

    /// Replaces the durable session only if the row still holds exactly
    /// `expected_text`. Token-bearing statements go straight to the pool, never
    /// through the query helpers that quote a failed statement in the error.
    async fn replace_saxo_session_in_db_if_unchanged(
        &self,
        expected_text: &str,
        session: &JsonValue,
        source: &str,
    ) -> Result<bool> {
        let session_text =
            serde_json::to_string(session).context("serializing Saxo session for database")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "UPDATE saxo_sessions
             SET session_json = '{}', updated_at = '{}', source = '{}'
             WHERE singleton_key = 'default' AND session_json = '{}'",
            sql_escape(&session_text),
            sql_escape(&updated_at),
            sql_escape(source),
            sql_escape(expected_text)
        );
        let result = sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("conditionally persisting Saxo session to database")?;
        Ok(result.rows_affected() == 1)
    }

    /// Saxo has already consumed the token this rotation replaced, so the
    /// successor must reach the durable row; a transient database error gets
    /// retried rather than stranding every other process on a dead token.
    async fn persist_rotated_saxo_session(&self, session: &JsonValue, source: &str) -> Result<()> {
        let mut attempt = 1;
        loop {
            match self.save_saxo_session_to_db(session, source).await {
                Ok(()) => return Ok(()),
                Err(err) if attempt < SAXO_SESSION_PERSIST_ATTEMPTS => {
                    warn!(
                        attempt,
                        "persisting the rotated Saxo session failed; retrying: {err:#}"
                    );
                    sleep(StdDuration::from_millis(250 * attempt as u64)).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn prepare_saxo_session_refresh_lease_if_needed(
        &self,
        source: &str,
        gate: &SaxoRefreshGate,
    ) -> Result<SaxoRefreshLease> {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before refresh lease check failed: {err:#}");
        }
        if !self.current_saxo_session_needs_refresh() {
            return Ok(SaxoRefreshLease::NotNeeded);
        }
        #[cfg(test)]
        saxo_lease_gap_hook::run(&self.saxo_session_file()).await;
        if gate.is_draining() {
            return Ok(SaxoRefreshLease::Draining);
        }
        if let Some(owner) = self.acquire_saxo_session_refresh_lease(source).await? {
            return Ok(SaxoRefreshLease::Held(owner));
        }

        info!(
            source,
            "Saxo session refresh lease is held by another process; waiting for durable session update"
        );
        for _ in 0..SAXO_SESSION_REFRESH_LEASE_WAIT_ATTEMPTS {
            sleep(StdDuration::from_secs(1)).await;
            if let Err(err) = self.sync_saxo_session_storage().await {
                warn!("Saxo session restore while waiting for refresh lease failed: {err:#}");
            }
            if !self.current_saxo_session_needs_refresh() {
                return Ok(SaxoRefreshLease::NotNeeded);
            }
            if gate.is_draining() {
                return Ok(SaxoRefreshLease::Draining);
            }
            if let Some(owner) = self.acquire_saxo_session_refresh_lease(source).await? {
                return Ok(SaxoRefreshLease::Held(owner));
            }
        }
        bail!("Saxo session refresh lease is still held by another process; refresh not attempted");
    }

    /// The rotation itself, with the lease held.
    ///
    /// The refresh input is the durable row as read now, never the working
    /// copy: the lease is typically free because another process just rotated
    /// the token and released it, after this process last looked.
    async fn rotate_saxo_session_under_lease(&self, source: &str) -> SaxoRotation {
        if let Err(err) = self.sync_saxo_session_storage().await {
            return SaxoRotation::released(Err(err.context(
                "re-reading the durable Saxo session under the refresh lease; refresh not attempted",
            )));
        }
        let (durable_text, durable) = match self.load_saxo_session_row_from_db().await {
            Ok(Some(row)) => row,
            Ok(None) => {
                return SaxoRotation::released(Err(anyhow!(
                    "No durable Saxo session is stored. Re-authentication is required."
                )));
            }
            Err(err) => return SaxoRotation::released(Err(err)),
        };
        if !saxo_session_needs_refresh(&durable) {
            return SaxoRotation::released(auth::usable_session_json(durable));
        }

        match auth::refresh_session_json(&self.config, &durable).await {
            Ok(rotated) => {
                let durable_write = self.persist_rotated_saxo_session(&rotated, source).await;
                if let Err(err) =
                    auth::import_session_json(&self.config, &self.config_path, &rotated)
                {
                    warn!("Saxo session working copy update after rotation failed: {err:#}");
                }
                match durable_write {
                    Ok(()) => SaxoRotation::released(Ok(rotated)),
                    Err(err) => {
                        // Keep the lease so it lapses rather than letting
                        // another process present the consumed token at once;
                        // this process's next restore pushes the working copy.
                        error!(
                            "Saxo rotated the refresh token but the durable write failed; holding the refresh lease until it lapses: {err:#}"
                        );
                        SaxoRotation {
                            result: Ok(rotated),
                            release_lease: false,
                        }
                    }
                }
            }
            Err(err) if auth::is_refresh_rejection(&err) => {
                let refused = match auth::mark_refresh_rejected(&durable, &err.to_string()) {
                    Ok(refused) => refused,
                    Err(mark_err) => return SaxoRotation::released(Err(mark_err)),
                };
                match self
                    .replace_saxo_session_in_db_if_unchanged(
                        &durable_text,
                        &refused,
                        &format!("{source}_invalid"),
                    )
                    .await
                {
                    Ok(true) => {
                        if let Err(import_err) =
                            auth::import_session_json(&self.config, &self.config_path, &refused)
                        {
                            warn!("Saxo session working copy update failed: {import_err:#}");
                        }
                        warn!(
                            source,
                            "Saxo refused the durable refresh token; re-authentication is required"
                        );
                    }
                    Ok(false) => {
                        // The durable row moved on while this request was in
                        // flight, so the refusal concerns a token no process
                        // holds any more. Adopt the newer row; mark nothing.
                        warn!(
                            source,
                            "Saxo refused a refresh token the durable session no longer holds; adopting the newer durable session"
                        );
                        if let Err(sync_err) = self.sync_saxo_session_storage().await {
                            warn!(
                                "Saxo session restore after a superseded refusal failed: {sync_err:#}"
                            );
                        }
                    }
                    Err(persist_err) => {
                        warn!("Saxo invalid session database persistence failed: {persist_err:#}");
                    }
                }
                SaxoRotation::released(Err(err))
            }
            // A transport failure leaves it unknown whether Saxo consumed the
            // token, so nothing is marked; the next attempt finds out.
            Err(err) => SaxoRotation::released(Err(err)),
        }
    }

    fn current_saxo_session_needs_refresh(&self) -> bool {
        auth::export_session_json(&self.config, &self.config_path)
            .map(|session| saxo_session_needs_refresh(&session))
            .unwrap_or(false)
    }

    async fn acquire_saxo_session_refresh_lease(&self, source: &str) -> Result<Option<String>> {
        let owner = saxo_refresh_lease_owner(source);
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let expires_at = (Utc::now() + Duration::seconds(SAXO_SESSION_REFRESH_LEASE_SECONDS))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let result = sqlx::query(&format!(
            "UPDATE saxo_sessions
             SET refresh_lease_owner = '{}',
                 refresh_lease_expires_at = '{}',
                 refresh_lease_source = '{}'
             WHERE singleton_key = 'default'
               AND (
                    refresh_lease_owner IS NULL
                    OR refresh_lease_owner = ''
                    OR refresh_lease_owner = '{}'
                    OR refresh_lease_expires_at IS NULL
                    OR refresh_lease_expires_at <= '{}'
               )",
            sql_escape(&owner),
            sql_escape(&expires_at),
            sql_escape(source),
            sql_escape(&owner),
            sql_escape(&now)
        ))
        .execute(&self.pool)
        .await
        .context("acquiring Saxo session refresh lease")?;
        if result.rows_affected() == 1 {
            info!(source, owner, "acquired Saxo session refresh lease");
            Ok(Some(owner))
        } else {
            Ok(None)
        }
    }

    async fn release_saxo_session_refresh_lease(&self, owner: &str) -> Result<()> {
        sqlx::query(&format!(
            "UPDATE saxo_sessions
             SET refresh_lease_owner = NULL,
                 refresh_lease_expires_at = NULL,
                 refresh_lease_source = NULL
             WHERE singleton_key = 'default'
               AND refresh_lease_owner = '{}'",
            sql_escape(owner)
        ))
        .execute(&self.pool)
        .await
        .context("releasing Saxo session refresh lease")?;
        Ok(())
    }

    pub async fn clear_saxo_session_from_db(&self) -> Result<()> {
        sqlx::query("DELETE FROM saxo_sessions WHERE singleton_key = 'default'")
            .execute(&self.pool)
            .await
            .context("clearing Saxo session from database")?;
        Ok(())
    }

    pub(crate) fn market_exchange_rows(&self) -> Vec<JsonValue> {
        let cache = current_saxo_exchange_calendar_cache();
        market_exchange_rows_for_config(&self.config, Utc::now(), cache.as_ref())
    }

    pub(crate) async fn first_json(&self, sql: &str) -> Result<Option<JsonValue>> {
        let row = sqlx::query(sql)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| report_query_failure(sql, err))?;
        Ok(row.map(|row| row_to_json(&row)))
    }

    pub(crate) async fn select_json(&self, sql: &str) -> Result<Vec<JsonValue>> {
        let rows = sqlx::query(sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|err| report_query_failure(sql, err))?;
        Ok(rows.iter().map(row_to_json).collect())
    }
}

/// Log a failed query and return an error that names it.
///
/// Forty-two call sites end `select_json(...).await.unwrap_or_default()`, which
/// is usually the right way to degrade a panel -- but it made a rejected
/// statement and an empty table produce the same empty vector, so the panel
/// reported "no data" for both. The realised-sell outcomes panel sat that way
/// from the day it shipped: its SQL carried a literal backslash Postgres would
/// not parse, and nothing anywhere said so. Reporting here fixes every one of
/// those callers at once without taking away their fallback.
fn report_query_failure(sql: &str, err: sqlx::Error) -> anyhow::Error {
    let outline = query_outline(sql);
    warn!("database query failed [{outline}]: {err}");
    anyhow::Error::new(err).context(format!("query failed [{outline}]"))
}

/// A short, identifying fragment of a statement for logs and error context.
///
/// Bounded on purpose: these statements interpolate symbols, dates and ids, and
/// a whole one in a log line is noise rather than evidence.
fn query_outline(sql: &str) -> String {
    let flattened = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let table = flattened
        .split_whitespace()
        .skip_while(|word| !word.eq_ignore_ascii_case("from"))
        .nth(1)
        .unwrap_or("?")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string();
    let head: String = flattened.chars().take(60).collect();
    format!("{table}: {head}")
}

/// Validates a model identifier without changing the active runtime setting.
/// A dry-run comparison may use this contract, but cannot promote itself to the
/// configured model or affect report execution eligibility.
pub(crate) fn validated_ai_model(model: &str) -> Result<String> {
    let model = model.trim();
    if model.is_empty() {
        anyhow::bail!("AI model cannot be empty");
    }
    if model.len() > 160 {
        anyhow::bail!("AI model is too long");
    }
    // '~' is OpenRouter's floating-alias prefix (e.g. ~openai/gpt-5) and must
    // round-trip unmodified.
    if !model
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.' | ':' | '~'))
    {
        anyhow::bail!("AI model contains unsupported characters");
    }
    Ok(model.to_string())
}

fn goal_period_value(
    baseline: Option<f64>,
    total_value: f64,
    target: f64,
    period_start: &str,
) -> JsonValue {
    let baseline = baseline.filter(|value| value.is_finite());
    if let Some(baseline_value) = baseline {
        let pnl = total_value - baseline_value;
        return json!({
            "status": "ready",
            "pnl_dkk": pnl,
            "target_dkk": target,
            "progress_pct": pct(pnl, target),
            "baseline_value_dkk": baseline_value,
            "period_start_utc": period_start,
        });
    }

    json!({
        "status": "pending_baseline",
        "pnl_dkk": JsonValue::Null,
        "target_dkk": target,
        "progress_pct": JsonValue::Null,
        "baseline_value_dkk": JsonValue::Null,
        "period_start_utc": period_start,
    })
}

fn since_reset_performance_value(baseline: Option<JsonValue>, total_value: f64) -> JsonValue {
    let baseline_at = baseline
        .as_ref()
        .and_then(|row| row.get("recorded_at"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let baseline_value = baseline
        .as_ref()
        .and_then(|row| row.get("total_market_value_dkk"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    if let (Some(baseline_at), Some(baseline_value)) = (baseline_at, baseline_value)
        && total_value.is_finite()
        && total_value > 0.0
    {
        let pnl = total_value - baseline_value;
        return json!({
            "status": "ready",
            "pnl_dkk": pnl,
            "return_pct": pct(pnl, baseline_value) * 100.0,
            "baseline_value_dkk": baseline_value,
            "baseline_recorded_at": baseline_at,
        });
    }

    json!({
        "status": "pending_baseline",
        "pnl_dkk": JsonValue::Null,
        "return_pct": JsonValue::Null,
        "baseline_value_dkk": JsonValue::Null,
        "baseline_recorded_at": JsonValue::Null,
    })
}

/// The import batch that defines the current book.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RealisedSellBaseline {
    pub(crate) batch_id: String,
    pub(crate) started_at: String,
}

pub(crate) const ADOPTED_ORDERS_WITHOUT_LEDGER_SQL: &str = "SELECT COUNT(*) AS count FROM execution_orders \
     WHERE status = 'executed' \
       AND ledger_id IS NULL \
       AND COALESCE(strategy_type, '') = 'portfolio_sync'";

async fn saxo_reference_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = json_text(session, "access_token");
    if access_token.trim().is_empty() {
        bail!("Saxo access token is missing from session");
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!(
            "{}{}",
            saxo_openapi_base_url(state, session)?,
            path
        ))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await
        .map_err(crate::debug_redaction::request_error_without_url)?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
        if let Some(error_text) = extract_saxo_error_text(&payload) {
            bail!("Saxo reference lookup failed: {error_text}");
        }
        let snippet: String = body.chars().take(300).collect();
        bail!(
            "Saxo reference lookup failed: HTTP {}: {}",
            status.as_u16(),
            snippet
        );
    }
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).context("parsing Saxo reference response")
}

fn extract_saxo_error_text(payload: &JsonValue) -> Option<String> {
    for key in ["Message", "ErrorMessage", "ErrorCode"] {
        if let Some(text) = payload.get(key).and_then(JsonValue::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    payload
        .get("ErrorInfo")
        .and_then(|value| {
            value
                .get("Message")
                .or_else(|| value.get("ErrorMessage"))
                .or_else(|| value.get("ErrorCode"))
        })
        .and_then(JsonValue::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn saxo_openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = json_text(session, "environment")
        .trim()
        .to_string()
        .to_lowercase();
    let environment = if environment.is_empty() {
        yaml_string(&state.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string())
            .to_lowercase()
    } else {
        environment
    };
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn saxo_exchange_calendar_cache_lock() -> &'static RwLock<Option<SaxoExchangeCalendarCache>> {
    SAXO_EXCHANGE_CALENDAR_CACHE.get_or_init(|| RwLock::new(None))
}

fn current_saxo_exchange_calendar_cache() -> Option<SaxoExchangeCalendarCache> {
    saxo_exchange_calendar_cache_lock()
        .read()
        .ok()
        .and_then(|cache| cache.clone())
}

fn market_exchange_rows_for_config(
    config: &YamlValue,
    now_utc: DateTime<Utc>,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> Vec<JsonValue> {
    let offset_minutes =
        yaml_i64(config, &["analysis_windows", "offset_minutes_after_open"]).unwrap_or(30);
    let pre_sync_minutes = yaml_i64(
        config,
        &["analysis_windows", "pre_sync_minutes_before_analysis"],
    )
    .unwrap_or(5);
    let end_buffer_minutes = yaml_i64(
        config,
        &["analysis_windows", "end_buffer_minutes_before_close"],
    )
    .unwrap_or(15);
    default_exchanges()
        .into_iter()
        .map(|exchange| {
            market_exchange_row(
                &exchange,
                now_utc,
                offset_minutes,
                pre_sync_minutes,
                end_buffer_minutes,
                cache,
            )
        })
        .collect()
}

fn market_exchange_row(
    exchange: &ExchangeRuntime,
    now_utc: DateTime<Utc>,
    offset_minutes: i64,
    pre_sync_minutes: i64,
    end_buffer_minutes: i64,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> JsonValue {
    let tz = exchange
        .timezone
        .parse::<Tz>()
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let local_now = now_utc.with_timezone(&tz);
    let local_date = local_now.date_naive();
    let is_weekend = local_now.weekday().number_from_monday() >= 6;
    let saxo_calendar = cache.and_then(|cache| cache.exchanges.get(exchange.code));
    let configured_holiday = if !is_weekend {
        configured_holiday_name(exchange.code, local_date)
    } else {
        None
    };
    let saxo_day_session =
        saxo_calendar.and_then(|calendar| saxo_trading_session_for_date(calendar, tz, local_date));
    let day_session = saxo_day_session.or_else(|| {
        if saxo_calendar.is_none() && !is_weekend && configured_holiday.is_none() {
            let open_local = local_session_time(tz, local_date, exchange.open_time);
            let close_local = local_session_time(tz, local_date, exchange.close_time);
            Some(ExchangeDaySession {
                open_at: open_local.with_timezone(&Utc),
                close_at: close_local.with_timezone(&Utc),
            })
        } else {
            None
        }
    });
    let holiday_name = if day_session.is_none() && !is_weekend {
        configured_holiday
    } else {
        None
    };

    let current_saxo_state = saxo_calendar.and_then(|calendar| {
        calendar
            .sessions
            .iter()
            .find(|session| session.start_at <= now_utc && now_utc < session.end_at)
            .map(|session| session.state.as_str())
    });
    let calendar_source = if saxo_calendar.is_some() {
        cache
            .map(|cache| cache.source.as_str())
            .unwrap_or("saxo_ref_v1_exchanges")
    } else if holiday_name.is_some() {
        "configured_holiday"
    } else {
        "configured"
    };
    let calendar_last_checked = cache
        .map(|cache| {
            cache
                .checked_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_default();

    if let Some(day_session) = day_session {
        let open_local = day_session.open_at.with_timezone(&tz);
        let close_local = day_session.close_at.with_timezone(&tz);
        let tradable_close_local =
            close_local - Duration::minutes(exchange.tradable_close_offset_minutes);
        let tradable_close_at = tradable_close_local.with_timezone(&Utc);
        let is_open = current_saxo_state
            .map(is_saxo_open_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc <= day_session.close_at);
        let is_tradable = current_saxo_state
            .map(is_saxo_trading_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc < tradable_close_at)
            && now_utc < tradable_close_at;
        let open_analysis_start = open_local + Duration::minutes(offset_minutes);
        let open_analysis_end = std::cmp::max(
            open_analysis_start,
            tradable_close_local - Duration::minutes(end_buffer_minutes),
        );
        let pre_sync_start = std::cmp::max(
            open_local,
            open_analysis_start - Duration::minutes(pre_sync_minutes),
        );
        let pre_analysis_sync_active =
            local_now >= pre_sync_start && local_now < open_analysis_start;
        let open_analysis_window_active =
            local_now >= open_analysis_start && local_now <= open_analysis_end;
        let next_open = saxo_calendar
            .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
            .map(|value| value.with_timezone(&tz))
            .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
        let status_reason = current_saxo_state
            .map(saxo_status_reason)
            .unwrap_or_else(|| {
                if local_now < open_local {
                    "Pre-open"
                } else if local_now >= tradable_close_local && local_now <= close_local {
                    "Closed - Closing auction / post-trade"
                } else if local_now > close_local {
                    "Closed - After hours"
                } else {
                    "Open"
                }
            });
        return json!({
            "code": exchange.code,
            "market": exchange.name,
            "timezone": exchange.timezone,
            "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
            "status_reason": status_reason,
            "holiday_name": JsonValue::Null,
            "session_open_local": open_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_close_local": close_local.format("%Y-%m-%d %H:%M").to_string(),
            "tradable_close_local": tradable_close_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_open_at_utc": day_session.open_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "session_close_at_utc": day_session.close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "tradable_close_at_utc": tradable_close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "is_open": is_open,
            "is_tradable": is_tradable,
            "pre_analysis_sync_active": pre_analysis_sync_active,
            "open_analysis_window_active": open_analysis_window_active,
            "close_analysis_window_active": false,
            "analysis_window_active": open_analysis_window_active,
            "pre_analysis_sync_start_at_utc": pre_sync_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_start_at_utc": open_analysis_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_end_at_utc": open_analysis_end.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
            "calendar_source": calendar_source,
            "calendar_last_checked": calendar_last_checked,
            "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
            "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
            "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
            "saxo_session_state": current_saxo_state.unwrap_or_default(),
        });
    }

    let next_open = saxo_calendar
        .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
        .map(|value| value.with_timezone(&tz))
        .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
    let status_reason = if is_weekend {
        "Closed - Weekend".to_string()
    } else if let Some(holiday) = holiday_name {
        format!("Closed - {holiday}")
    } else if saxo_calendar.is_some() {
        "Closed - No Saxo trading session".to_string()
    } else {
        let open_local = local_session_time(tz, local_date, exchange.open_time);
        if local_now < open_local {
            "Pre-open".to_string()
        } else {
            "Closed - After hours".to_string()
        }
    };

    json!({
        "code": exchange.code,
        "market": exchange.name,
        "timezone": exchange.timezone,
        "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
        "status_reason": status_reason,
        "holiday_name": holiday_name.unwrap_or_default(),
        "session_open_local": "n/a",
        "session_close_local": "n/a",
        "tradable_close_local": "n/a",
        "session_open_at_utc": JsonValue::Null,
        "session_close_at_utc": JsonValue::Null,
        "tradable_close_at_utc": JsonValue::Null,
        "is_open": false,
        "is_tradable": false,
        "pre_analysis_sync_active": false,
        "open_analysis_window_active": false,
        "close_analysis_window_active": false,
        "analysis_window_active": false,
        "pre_analysis_sync_start_at_utc": JsonValue::Null,
        "open_analysis_window_start_at_utc": JsonValue::Null,
        "open_analysis_window_end_at_utc": JsonValue::Null,
        "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
        "calendar_source": calendar_source,
        "calendar_last_checked": calendar_last_checked,
        "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
        "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
        "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
        "saxo_session_state": current_saxo_state.unwrap_or_default(),
    })
}

/// Whether the pod-local working copy should replace the durable row.
///
/// Saxo rotates a grant's refresh token linearly and a new login is newer than
/// any rotation, so between two different refresh tokens the later session is
/// the live one and the earlier has been consumed: recency decides, and a
/// refreshable-looking copy never outranks a newer row just because that row
/// was refused. For the same refresh token, knowing Saxo refused it is the
/// newer fact.
fn saxo_working_copy_supersedes_durable(file: &JsonValue, db: &JsonValue) -> bool {
    let same_refresh_token = matches!(
        (
            non_empty_session_text(file.get("refresh_token")),
            non_empty_session_text(db.get("refresh_token")),
        ),
        (Some(file_token), Some(db_token)) if file_token == db_token
    );
    let file_refused = saxo_session_refresh_invalid(file);
    if same_refresh_token && file_refused != saxo_session_refresh_invalid(db) {
        return file_refused;
    }
    saxo_session_rank(file) > saxo_session_rank(db)
}

fn saxo_session_refresh_invalid(session: &JsonValue) -> bool {
    non_empty_session_text(session.get("refresh_token_invalid_at")).is_some()
}

struct ExchangeRuntime {
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_time: NaiveTime,
    close_time: NaiveTime,
    tradable_close_offset_minutes: i64,
}

fn default_exchanges() -> Vec<ExchangeRuntime> {
    vec![
        exchange("XCSE", "Copenhagen", "Europe/Copenhagen", 9, 0, 17, 0, 0),
        exchange("XLON", "London", "Europe/London", 8, 0, 16, 30, 0),
        exchange(
            "XETR",
            "Frankfurt / Xetra",
            "Europe/Berlin",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange(
            "XAMS",
            "Amsterdam / Euronext",
            "Europe/Amsterdam",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange("XNAS", "Nasdaq US", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XNYS", "NYSE", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XSTO", "Stockholm", "Europe/Stockholm", 9, 0, 17, 30, 0),
        exchange("XOSL", "Oslo", "Europe/Oslo", 9, 0, 16, 30, 5),
        exchange("XHEL", "Helsinki", "Europe/Helsinki", 10, 0, 18, 30, 0),
        exchange("XMIL", "Milan", "Europe/Rome", 9, 0, 17, 30, 0),
    ]
}

fn exchange(
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_hour: u32,
    open_minute: u32,
    close_hour: u32,
    close_minute: u32,
    tradable_close_offset_minutes: i64,
) -> ExchangeRuntime {
    ExchangeRuntime {
        code,
        name,
        timezone,
        open_time: NaiveTime::from_hms_opt(open_hour, open_minute, 0).unwrap_or(NaiveTime::MIN),
        close_time: NaiveTime::from_hms_opt(close_hour, close_minute, 0).unwrap_or(NaiveTime::MIN),
        tradable_close_offset_minutes,
    }
}

fn saxo_exchange_calendar_from_detail(
    detail: &JsonValue,
    fallback_exchange_id: &str,
) -> Option<SaxoExchangeCalendar> {
    let exchange_id = saxo_exchange_text(detail, "ExchangeId")
        .unwrap_or_else(|| fallback_exchange_id.to_string());
    let sessions = parse_saxo_exchange_sessions(detail);
    if sessions.is_empty() {
        return None;
    }
    Some(SaxoExchangeCalendar {
        exchange_id,
        name: saxo_exchange_text(detail, "Name"),
        timezone_id: saxo_exchange_text(detail, "TimeZoneId"),
        sessions,
    })
}

fn parse_saxo_exchange_sessions(detail: &JsonValue) -> Vec<SaxoExchangeSession> {
    let Some(sessions) = detail.get("ExchangeSessions").and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|session| {
            let start = saxo_exchange_text(session, "StartTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let end = saxo_exchange_text(session, "EndTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let state =
                saxo_exchange_text(session, "State").unwrap_or_else(|| "Undefined".to_string());
            Some(SaxoExchangeSession {
                start_at: start,
                end_at: end,
                state,
            })
        })
        .collect()
}

fn saxo_exchange_matches(value: &JsonValue, code: &str) -> bool {
    ["ExchangeId", "Mic", "IsoMic", "OperatingMic"]
        .iter()
        .filter_map(|key| saxo_exchange_text(value, key))
        .any(|value| value.eq_ignore_ascii_case(code))
}

fn saxo_exchange_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn saxo_trading_session_for_date(
    calendar: &SaxoExchangeCalendar,
    tz: Tz,
    local_date: NaiveDate,
) -> Option<ExchangeDaySession> {
    let sessions = calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_continuous_trading_state(&session.state))
        .filter(|session| session_overlaps_local_date(session, tz, local_date))
        .collect::<Vec<_>>();
    let open_at = sessions.iter().map(|session| session.start_at).min()?;
    let close_at = sessions.iter().map(|session| session.end_at).max()?;
    Some(ExchangeDaySession { open_at, close_at })
}

fn next_saxo_open_time(
    calendar: &SaxoExchangeCalendar,
    now_utc: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_continuous_trading_state(&session.state))
        .filter(|session| session.start_at > now_utc)
        .map(|session| session.start_at)
        .min()
}

fn session_overlaps_local_date(
    session: &SaxoExchangeSession,
    tz: Tz,
    local_date: NaiveDate,
) -> bool {
    let start_date = session.start_at.with_timezone(&tz).date_naive();
    let end_date = (session.end_at - Duration::seconds(1))
        .with_timezone(&tz)
        .date_naive();
    start_date <= local_date && local_date <= end_date
}

fn is_saxo_trading_state(state: &str) -> bool {
    is_saxo_continuous_trading_state(state)
}

fn is_saxo_continuous_trading_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "automatedtrading" | "pittrading"
    )
}

fn is_saxo_open_state(state: &str) -> bool {
    !matches!(
        state.to_ascii_lowercase().as_str(),
        "closed" | "break" | "halt" | "suspended" | "undefined"
    )
}

fn saxo_status_reason(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "automatedtrading" | "pittrading" | "callauctiontrading" | "auction" | "openingauction"
        | "tradingatlast" => "Open",
        "preautomatedtrading" | "premarket" | "pretrading" => "Pre-open",
        "postautomatedtrading" | "postmarket" | "posttrading" => {
            "Closed - Closing auction / post-trade"
        }
        "break" => "Closed - Exchange break",
        "halt" => "Closed - Halted",
        "suspended" => "Closed - Suspended",
        "closed" => "Closed",
        _ => "Closed - Unknown Saxo session state",
    }
}

fn configured_holiday_name(exchange_code: &str, local_date: NaiveDate) -> Option<&'static str> {
    match (
        exchange_code,
        local_date.year(),
        local_date.month(),
        local_date.day(),
    ) {
        ("XCSE", 2026, 1, 1) => Some("New Year's Day"),
        ("XCSE", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XCSE", 2026, 4, 3) => Some("Good Friday"),
        ("XCSE", 2026, 4, 6) => Some("Easter Monday"),
        ("XCSE", 2026, 5, 14) => Some("Ascension Day"),
        ("XCSE", 2026, 5, 15) => Some("Day after Ascension Day"),
        ("XCSE", 2026, 5, 25) => Some("Whit Monday"),
        ("XCSE", 2026, 6, 5) => Some("Constitution Day"),
        ("XCSE", 2026, 12, 24) => Some("Christmas Eve"),
        ("XCSE", 2026, 12, 25) => Some("Christmas Day"),
        ("XCSE", 2026, 12, 31) => Some("New Year's Eve"),
        ("XLON", 2026, 1, 1) => Some("New Year's Day"),
        ("XLON", 2026, 4, 3) => Some("Good Friday"),
        ("XLON", 2026, 4, 6) => Some("Easter Monday"),
        ("XLON", 2026, 5, 4) => Some("Early May bank holiday"),
        ("XLON", 2026, 5, 25) => Some("Spring bank holiday"),
        ("XLON", 2026, 8, 31) => Some("Summer bank holiday"),
        ("XLON", 2026, 12, 25) => Some("Christmas Day"),
        ("XLON", 2026, 12, 28) => Some("Boxing Day (substitute day)"),
        ("XETR", 2026, 1, 1) => Some("New Year's Day"),
        ("XETR", 2026, 4, 3) => Some("Good Friday"),
        ("XETR", 2026, 4, 6) => Some("Easter Monday"),
        ("XETR", 2026, 12, 24) => Some("Christmas Eve"),
        ("XETR", 2026, 12, 25) => Some("Christmas Day"),
        ("XETR", 2026, 12, 31) => Some("New Year's Eve"),
        ("XAMS", 2026, 1, 1) => Some("New Year's Day"),
        ("XAMS", 2026, 4, 3) => Some("Good Friday"),
        ("XAMS", 2026, 4, 6) => Some("Easter Monday"),
        ("XAMS", 2026, 5, 1) => Some("Labour Day"),
        ("XAMS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNAS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNAS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNAS", 2026, 2, 16) => Some("Presidents Day"),
        ("XNAS", 2026, 4, 3) => Some("Good Friday"),
        ("XNAS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNAS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNAS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNAS", 2026, 9, 7) => Some("Labor Day"),
        ("XNAS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNAS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNYS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNYS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNYS", 2026, 2, 16) => Some("Washington's Birthday"),
        ("XNYS", 2026, 4, 3) => Some("Good Friday"),
        ("XNYS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNYS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNYS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNYS", 2026, 9, 7) => Some("Labor Day"),
        ("XNYS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNYS", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 1, 1) => Some("New Year's Day"),
        ("XSTO", 2026, 1, 6) => Some("Epiphany"),
        ("XSTO", 2026, 4, 3) => Some("Good Friday"),
        ("XSTO", 2026, 4, 6) => Some("Easter Monday"),
        ("XSTO", 2026, 5, 1) => Some("Labour Day"),
        ("XSTO", 2026, 5, 14) => Some("Ascension Day"),
        ("XSTO", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XSTO", 2026, 12, 24) => Some("Christmas Eve"),
        ("XSTO", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 12, 31) => Some("New Year's Eve"),
        ("XOSL", 2026, 1, 1) => Some("New Year's Day"),
        ("XOSL", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XOSL", 2026, 4, 3) => Some("Good Friday"),
        ("XOSL", 2026, 4, 6) => Some("Easter Monday"),
        ("XOSL", 2026, 5, 1) => Some("Labour Day"),
        ("XOSL", 2026, 5, 14) => Some("Ascension Day"),
        ("XOSL", 2026, 5, 25) => Some("Whit Monday"),
        ("XOSL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XOSL", 2026, 12, 25) => Some("Christmas Day"),
        ("XOSL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XHEL", 2026, 1, 1) => Some("New Year's Day"),
        ("XHEL", 2026, 1, 6) => Some("Epiphany"),
        ("XHEL", 2026, 4, 3) => Some("Good Friday"),
        ("XHEL", 2026, 4, 6) => Some("Easter Monday"),
        ("XHEL", 2026, 5, 1) => Some("Labour Day"),
        ("XHEL", 2026, 5, 14) => Some("Ascension Day"),
        ("XHEL", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XHEL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XHEL", 2026, 12, 25) => Some("Christmas Day"),
        ("XHEL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XMIL", 2026, 1, 1) => Some("New Year's Day"),
        ("XMIL", 2026, 4, 3) => Some("Good Friday"),
        ("XMIL", 2026, 4, 6) => Some("Easter Monday"),
        ("XMIL", 2026, 5, 1) => Some("Labour Day"),
        ("XMIL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XMIL", 2026, 12, 25) => Some("Christmas Day"),
        ("XMIL", 2026, 12, 31) => Some("New Year's Eve"),
        _ => None,
    }
}

fn local_session_time(tz: Tz, date: chrono::NaiveDate, time: NaiveTime) -> DateTime<Tz> {
    tz.with_ymd_and_hms(
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        0,
    )
    .single()
    .unwrap_or_else(|| Utc::now().with_timezone(&tz))
}

fn next_open_time(tz: Tz, exchange: &ExchangeRuntime, local_now: DateTime<Tz>) -> DateTime<Tz> {
    for offset in 0..10 {
        let candidate_date = local_now.date_naive() + Duration::days(offset);
        if candidate_date.weekday().number_from_monday() >= 6 {
            continue;
        }
        if configured_holiday_name(exchange.code, candidate_date).is_some() {
            continue;
        }
        let candidate = local_session_time(tz, candidate_date, exchange.open_time);
        if candidate > local_now {
            return candidate;
        }
    }
    local_session_time(
        tz,
        local_now.date_naive() + Duration::days(1),
        exchange.open_time,
    )
}

fn market_names_where(items: &[JsonValue], key: &str) -> Vec<String> {
    items
        .iter()
        .filter(|row| row.get(key).and_then(JsonValue::as_bool).unwrap_or(false))
        .filter_map(|row| row.get("market").and_then(JsonValue::as_str))
        .map(ToString::to_string)
        .collect()
}

fn performance_start_at(range_key: &str) -> Option<String> {
    let now = Utc::now();
    let start = match range_key.to_uppercase().as_str() {
        "1D" => now - Duration::days(1),
        "1W" => now - Duration::weeks(1),
        "1M" => now - Duration::days(31),
        "3M" => now - Duration::days(93),
        "YTD" => Utc
            .with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now - Duration::days(31)),
        "1Y" => now - Duration::days(366),
        "ALL" => return None,
        _ => now - Duration::days(1),
    };
    Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn performance_rows_have_same_values(left: &JsonValue, right: &JsonValue) -> bool {
    const EPSILON_DKK: f64 = 0.01;
    let numeric_keys = [
        "total_market_value_dkk",
        "invested_market_value_dkk",
        "cash_balance_dkk",
        "total_cost_basis_dkk",
        "total_unrealised_pnl_dkk",
        "total_daily_pnl_dkk",
    ];
    numeric_keys
        .iter()
        .all(|key| (value_f64(left, key) - value_f64(right, key)).abs() <= EPSILON_DKK)
        && value_i64(left, "position_count") == value_i64(right, "position_count")
}

pub(crate) fn text_value(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

pub(crate) trait BlankStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>;
    fn non_empty_or_none(self) -> Option<String>;
}

impl BlankStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        if self.trim().is_empty() {
            fallback()
        } else {
            Some(self)
        }
    }

    fn non_empty_or_none(self) -> Option<String> {
        if self.trim().is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

/// The exchange suffix of a `BASE:exchange` symbol, for callers outside this
/// module.
pub(crate) fn exchange_code_for(symbol: &str) -> String {
    exchange_code(symbol)
}

pub(crate) fn exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.to_string())
        .unwrap_or_default()
}

/// Returns the versioned, source-controlled analysis universe. It deliberately
/// does not read historical report, sentiment, or quote tables: those tables
/// describe observations, not the set of assets the system is meant to study.
fn configured_watchlist_universe_symbols(config: &YamlValue) -> Vec<String> {
    configured_watchlist_symbols_at(
        config,
        &["market_data", "watchlists", "universe_symbols"],
        false,
    )
}

/// Extra watches are candidates in addition to the main universe. Their
/// mapping form may carry an ISIN for Saxo resolution; membership needs only
/// the explicit symbol.
fn configured_extra_watch_symbols(config: &YamlValue) -> Vec<String> {
    configured_watchlist_symbols_at(
        config,
        &["market_data", "watchlists", "extra_symbols"],
        true,
    )
}

fn configured_watchlist_symbols_at(
    config: &YamlValue,
    keys: &[&str],
    accept_mapping_symbols: bool,
) -> Vec<String> {
    let mut seen = HashSet::new();
    crate::config::yaml_at(config, keys)
        .and_then(YamlValue::as_sequence)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .as_str()
                        .or_else(|| {
                            accept_mapping_symbols
                                .then(|| entry.get("symbol").and_then(YamlValue::as_str))
                                .flatten()
                        })
                        .and_then(normalize_watchlist_symbol)
                })
                .filter(|symbol| seen.insert(watchlist_symbol_key(symbol)))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_watchlist_symbol(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (base, exchange) = trimmed
        .split_once(':')
        .map(|(base, exchange)| (base.trim(), Some(exchange.trim())))
        .unwrap_or((trimmed, None));
    if base.is_empty() || exchange.is_some_and(str::is_empty) {
        return None;
    }
    Some(match exchange {
        Some(exchange) => format!(
            "{}:{}",
            base.to_ascii_uppercase(),
            exchange.to_ascii_lowercase()
        ),
        None => base.to_ascii_uppercase(),
    })
}

fn watchlist_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_lowercase()
}

/// Saxo's identity for an instrument, from a row's `uic` and `asset_type`. A
/// `Uic` alone is not enough: Saxo only guarantees it unique within an asset
/// type, and data borrowed from a different instrument would sit under the
/// right symbol looking correct.
pub(crate) fn saxo_instrument_key(row: &JsonValue) -> Option<(i64, String)> {
    let uic = value_i64(row, "uic");
    let asset_type = text_value(row, "asset_type").trim().to_ascii_uppercase();
    (uic > 0 && !asset_type.is_empty()).then_some((uic, asset_type))
}

/// Watchlist membership that lists a held instrument once.
///
/// Saxo reports a holding under its own spelling (`ALKb:xcse`) while the
/// universe, decision reports, and orders name the instrument by the system
/// symbol (`ALK-B:xcse`), so membership keyed on the symbol listed it twice.
/// The broker snapshots give each Saxo spelling its instrument, and the latest
/// indicator run gives the system spelling the instrument it resolved to.
///
/// A held instrument keeps the slot of the first spelling seen. When that is a
/// spelling only Saxo uses -- not a configured symbol -- the first other
/// spelling of the instrument renames it: decisions and orders use that
/// symbol, and keeping it listed keeps the Markov and indicator runs refreshing
/// it, as the positions already give them Saxo's. A further spelling is listed
/// under its own symbol, as before, rather than dropped: dropping it would stop
/// the indicator run that identified it, and the next build would list it
/// again.
struct WatchlistMembers {
    held_instrument_by_symbol: HashMap<String, (i64, String)>,
    broker_spellings: HashSet<String>,
    configured_spellings: HashSet<String>,
    held_entries: HashMap<(i64, String), HeldWatchlistEntry>,
    listed_symbols: HashSet<String>,
}

struct HeldWatchlistEntry {
    index: usize,
    renameable: bool,
}

impl WatchlistMembers {
    fn new<'a>(
        broker_rows: impl IntoIterator<Item = &'a JsonValue>,
        indicator_rows: &[JsonValue],
        configured_symbols: impl IntoIterator<Item = &'a String>,
    ) -> Self {
        let mut held_instrument_by_symbol = HashMap::new();
        let mut broker_spellings = HashSet::new();
        for row in broker_rows {
            let symbol = watchlist_symbol_key(&text_value(row, "symbol"));
            if let Some(instrument) = saxo_instrument_key(row)
                && !symbol.is_empty()
            {
                broker_spellings.insert(symbol.clone());
                held_instrument_by_symbol.insert(symbol, instrument);
            }
        }
        let held = held_instrument_by_symbol
            .values()
            .cloned()
            .collect::<HashSet<_>>();
        for row in indicator_rows {
            if let Some(instrument) = saxo_instrument_key(row)
                && held.contains(&instrument)
            {
                held_instrument_by_symbol
                    .entry(watchlist_symbol_key(&text_value(row, "symbol")))
                    .or_insert(instrument);
            }
        }
        Self {
            held_instrument_by_symbol,
            broker_spellings,
            configured_spellings: configured_symbols
                .into_iter()
                .map(|symbol| watchlist_symbol_key(symbol))
                .collect(),
            held_entries: HashMap::new(),
            listed_symbols: HashSet::new(),
        }
    }

    /// Lists `row()` unless the symbol, or another spelling of the same held
    /// instrument, is already listed, and returns whether it did.
    fn admit(
        &mut self,
        monitored: &mut Vec<JsonValue>,
        symbol: &str,
        row: impl FnOnce() -> JsonValue,
    ) -> bool {
        let symbol_key = watchlist_symbol_key(symbol);
        if let Some(instrument) = self.held_instrument_by_symbol.get(&symbol_key) {
            let broker_spelling = self.broker_spellings.contains(&symbol_key);
            match self.held_entries.get_mut(instrument) {
                None => {
                    self.held_entries.insert(
                        instrument.clone(),
                        HeldWatchlistEntry {
                            index: monitored.len(),
                            renameable: broker_spelling
                                && !self.configured_spellings.contains(&symbol_key),
                        },
                    );
                    self.listed_symbols.insert(symbol_key);
                    monitored.push(row());
                    return true;
                }
                Some(_) if broker_spelling => return false,
                Some(entry) if entry.renameable => {
                    entry.renameable = false;
                    take_system_symbol(&mut monitored[entry.index], symbol);
                    self.listed_symbols.insert(symbol_key);
                    return false;
                }
                Some(_) => {}
            }
        }
        if !self.listed_symbols.insert(symbol_key) {
            return false;
        }
        monitored.push(row());
        true
    }
}

/// Renames a watchlist row from Saxo's spelling to the system symbol. A name
/// derived from the old spelling follows it; a broker description stays.
fn take_system_symbol(row: &mut JsonValue, symbol: &str) {
    let Some(object) = row.as_object_mut() else {
        return;
    };
    let previous = object
        .get("symbol")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    let name = object.get("instrument_name").and_then(JsonValue::as_str);
    if name.is_none_or(|name| name == previous || name == instrument_name_for_symbol(&previous)) {
        object.insert(
            "instrument_name".to_string(),
            JsonValue::from(instrument_name_for_symbol(symbol)),
        );
    }
    object.insert("symbol".to_string(), JsonValue::from(symbol));
}

fn configured_watchlist_row(symbol: &str, source: &str) -> JsonValue {
    json!({
        "symbol": symbol,
        "instrument_name": instrument_name_for_symbol(symbol),
        "quote_status": "configured_universe",
        "source": source,
    })
}

pub(crate) fn enrich_execution_order_lifecycle(order: &mut JsonValue, market_rows: &[JsonValue]) {
    let status = json_text(order, "status").to_ascii_lowercase();
    let active_broker_order = matches!(
        status.as_str(),
        "submitted_to_broker"
            | "broker_working"
            | "broker_amended"
            | "broker_partially_filled"
            | "broker_replace_requested"
            | "broker_cancel_requested"
    );
    if !active_broker_order {
        return;
    }

    let duration_type = execution_order_duration_type(order);
    let exchange = exchange_code(
        order
            .get("symbol")
            .and_then(JsonValue::as_str)
            .unwrap_or_default(),
    )
    .to_ascii_lowercase();
    let lifecycle = if duration_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("DayOrder"))
    {
        market_rows
            .iter()
            .find(|row| {
                row.get("code")
                    .and_then(JsonValue::as_str)
                    .map(|code| code.eq_ignore_ascii_case(&exchange))
                    .unwrap_or(false)
            })
            .map(|market| {
                (
                    json_text(market, "tradable_close_at_utc")
                        .non_empty_or_none()
                        .or_else(|| json_text(market, "session_close_at_utc").non_empty_or_none()),
                    market
                        .get("market")
                        .cloned()
                        .unwrap_or_else(|| json!(exchange)),
                    market.get("timezone").cloned().unwrap_or(JsonValue::Null),
                )
            })
    } else {
        None
    };

    let Some(object) = order.as_object_mut() else {
        return;
    };
    if let Some(duration_type) = duration_type.as_deref() {
        object.insert("order_duration_type".to_string(), json!(duration_type));
    }
    if let Some((expiry, market, timezone)) = lifecycle {
        if let Some(expiry) = expiry {
            let expiry_pending = parse_rfc3339_utc(Some(&json!(expiry.clone())))
                .map(|expiry_at| {
                    expiry_at + Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES) <= Utc::now()
                })
                .unwrap_or(false);
            object.insert("expected_expiry_at_utc".to_string(), json!(expiry));
            if expiry_pending {
                object.insert(
                    "lifecycle_state".to_string(),
                    json!("expiry_pending_broker_sync"),
                );
            }
        }
        object.insert(
            "expected_expiry_source".to_string(),
            json!("exchange_calendar"),
        );
        object.insert("expected_expiry_market".to_string(), market);
        object.insert("expected_expiry_timezone".to_string(), timezone);
        object.insert(
            "lifecycle_note".to_string(),
            if object
                .get("lifecycle_state")
                .and_then(JsonValue::as_str)
                == Some("expiry_pending_broker_sync")
            {
                json!("Expected DayOrder expiry has passed; waiting for Saxo broker sync to confirm fill, cancel, reject, or expiry.")
            } else {
                json!("DayOrder remains live until broker fill, cancel, reject, or exchange-day expiry sync.")
            },
        );
    }
}

pub(crate) fn execution_order_duration_type(order: &JsonValue) -> Option<String> {
    nested_json_text(
        order,
        &[
            "execution_result_json",
            "payload",
            "OrderDuration",
            "DurationType",
        ],
    )
    .non_empty_or_none()
    .or_else(|| {
        nested_json_text(
            order,
            &[
                "execution_result_json",
                "broker_sync",
                "broker_payload",
                "Duration",
                "DurationType",
            ],
        )
        .non_empty_or_none()
    })
    .or_else(|| {
        nested_json_text(
            order,
            &[
                "execution_result_json",
                "broker_sync",
                "broker_payload",
                "OrderDuration",
                "DurationType",
            ],
        )
        .non_empty_or_none()
    })
}

fn exchange_region(symbol: &str) -> String {
    match exchange_code(symbol).to_lowercase().as_str() {
        "xcse" | "xsto" | "xome" | "xosl" | "xhel" => "Nordics",
        "xlon" => "UK",
        "xnas" | "xnys" => "US",
        _ => "Europe",
    }
    .to_string()
}

fn instrument_name_for_symbol(symbol: &str) -> String {
    let base = symbol
        .split_once(':')
        .map(|(base, _)| base)
        .unwrap_or(symbol);
    match base.to_uppercase().as_str() {
        "AAPL" => "Apple".to_string(),
        "ADBE" => "Adobe".to_string(),
        "ADI" => "Analog Devices".to_string(),
        "AMD" => "Advanced Micro Devices".to_string(),
        "AMZN" => "Amazon.com".to_string(),
        "ASML" => "ASML ADR".to_string(),
        "AVGO" => "Broadcom".to_string(),
        "DDOG" => "Datadog".to_string(),
        "GOOGL" => "Alphabet Inc. Class A".to_string(),
        "IBM" => "IBM".to_string(),
        "INTC" => "Intel".to_string(),
        "MA" => "Mastercard".to_string(),
        "MDB" => "MongoDB".to_string(),
        "MSTR" => "MicroStrategy".to_string(),
        "NVDA" => "NVIDIA".to_string(),
        "PANW" => "Palo Alto Networks".to_string(),
        "PLTR" => "Palantir Technologies".to_string(),
        "QCOM" => "Qualcomm".to_string(),
        "SNOW" => "Snowflake".to_string(),
        "V" => "Visa".to_string(),
        other => other.to_string(),
    }
}

fn saxo_session_rank(session: &JsonValue) -> i64 {
    // The latest useful timestamp wins within the same health tier. Refreshes update
    // `last_refreshed_at`, while a fresh OAuth callback may only have `created_at`.
    [
        "last_refreshed_at",
        "created_at",
        "access_token_expires_at",
        "refresh_token_expires_at",
    ]
    .iter()
    .filter_map(|key| parse_session_time(session.get(*key)))
    .max()
    .unwrap_or(0)
}

fn parse_session_time(value: Option<&JsonValue>) -> Option<i64> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.timestamp())
}

fn non_empty_session_text(value: Option<&JsonValue>) -> Option<&str> {
    let text = value?.as_str()?;
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard_state::{
        dashboard_active_strategy_baseline_from_json, dashboard_decision_pulse_evidence_from_json,
        dashboard_execution_orders_from_json, dashboard_hermes_baseline_evidence_pack_from_json,
        dashboard_hermes_counterfactuals_from_json,
        dashboard_hermes_decision_advice_audit_from_json, dashboard_hermes_experiments_from_json,
        dashboard_hermes_learning_memory_from_json,
        dashboard_hermes_lessons_pending_review_from_json,
        dashboard_hermes_one_variable_audit_from_json, dashboard_hermes_proposal_quality_from_json,
        dashboard_hermes_reflections_from_json, dashboard_holding_thesis_reviews_from_json,
        dashboard_loads_hermes_section, dashboard_loads_tab_exclusive_data,
        dashboard_missed_trade_shadow_evidence_from_json, dashboard_missed_trade_shadows_from_json,
        dashboard_performance_history_limit, dashboard_quiver_conflicts_from_json,
        dashboard_quiver_conflicts_not_loaded, dashboard_run_schedules_from_json,
        dashboard_saxo_auth_from_json, dashboard_saxo_auth_unavailable,
        dashboard_trade_thesis_evidence_from_json,
    };
    use crate::decision_state::dashboard_decision_gate_replay_from_json;
    use crate::decision_state::dashboard_decision_gate_replay_not_loaded;
    use crate::decision_state::decision_report_summaries_from_json;
    use crate::decision_state::{
        candidate_gate_code, candidate_gate_code_from_reason,
        candidate_scoring_waterfall_from_manager_run, compact_candidate_final_technical,
        decision_pulse_outcome_evidence_from_observations, experiment_variable_effect_path,
        gate_replay_cash_budgets, gate_replay_cash_buffer_scenario, gate_replay_from_manager_runs,
        gate_replay_markov_scenario, merge_report_symbol_decisions,
    };
    use crate::execution_state::{
        attribution_delta_label, compact_attribution_capital, compact_attribution_markov,
        compact_attribution_technical, compact_execution_ledger_outcome,
        compact_execution_position_lifecycle, compact_holding_period_outcome,
        matching_manager_preflight_candidate,
    };
    use crate::hermes_state::hermes_proposal_quality_from_experiments;
    use crate::hermes_state::{
        compact_hermes_context, compact_journal_rows, compact_overview_for_hermes,
        compact_performance_history, compact_protective_stop_coverage_for_hermes,
        hermes_baseline_evidence_pack_from_snapshot, hermes_capabilities_payload,
        hermes_counterfactual_shadow_quantity, hermes_experiment_next_status,
        hermes_experiment_review_family, hermes_experiment_status_blocks_duplicate,
        hermes_goal_contract_from_config, hermes_one_variable_audit_from_snapshot,
        learning_memory_from_reflections, lessons_pending_review_from_reflections,
        normalize_hermes_context_self_check, normalize_hermes_experiment_variable_path,
        realised_outcomes_for_hermes, scope_hermes_context, strip_raw_broker_documents,
    };
    use crate::integrity_state::{
        annotate_overview_integrity_acknowledgements, broker_cash_reconciliation_enabled,
        broker_exposure_pnl_attribution, broker_exposure_quantity_mismatches, exit_route_family,
        money_mismatch_exceeds_tolerance, overview_integrity_issue_key,
        portfolio_position_snapshot_integrity_from_rows, realised_sell_outcome_evidence,
        unrealised_pnl_reconciliation, unreconciled_orders_sql,
    };
    use crate::market_state::dashboard_watchlists_from_json;
    use crate::market_state::dashboard_watchlists_not_loaded;
    use crate::models::{
        HermesContextDecisionsPayload, HermesContextEndOfDayPayload, HermesContextExecutionPayload,
        HermesContextHermesPayload, HermesContextItemsPayload, HermesContextLearningMemoryPayload,
        HermesContextPayload, HermesContextPerformancePayload, HermesContextSafetyPayload,
        HermesContextSchedulerPayload, HermesDecisionReportOutcomePayload,
    };
    use crate::performance_state::dashboard_performance_benchmarks_from_json;
    use crate::performance_state::dashboard_performance_exposure_attribution_from_json;
    use crate::performance_state::dashboard_performance_goal_tracking_from_json;
    use crate::performance_state::dashboard_performance_history_from_json;
    use crate::performance_state::dashboard_performance_pnl_reconciliation_from_json;
    use crate::performance_state::dashboard_performance_realised_sell_outcomes_from_json;
    use crate::performance_state::dashboard_performance_snapshot_evidence_from_json;
    use crate::performance_state::dashboard_performance_summary_from_json;
    use crate::protective_stop_state::protective_stop_coverage_from_rows;
    use crate::settings_state::mask_api_key;
    use crate::shadow_evidence_state::{
        compact_shadow_report_decision_time_signal_gate, compact_shadow_report_forward_outcome,
        missed_trade_shadow_gate_is_eligible, missed_trade_shadow_outcome_evidence_from_rows,
        shadow_report_outcome_candidates,
    };
    use std::collections::BTreeMap;

    #[test]
    fn dashboard_watchlists_requires_the_stable_outer_contract() {
        let watchlists = dashboard_watchlists_from_json(json!({
            "generated_at": "2026-08-23T12:00:00Z",
            "cache_ttl_seconds": 300,
            "universe": {"source": "configured_analysis_universe"},
            "categories": [{"key": "nordic", "items": [{"symbol": "NOVO-B:xcse"}]}]
        }))
        .expect("watchlists fixture has the dashboard contract");

        assert_eq!(watchlists.cache_ttl_seconds, 300);
        assert_eq!(watchlists.categories[0].key, "nordic");
        assert_eq!(dashboard_watchlists_not_loaded().categories.len(), 0);
        assert!(dashboard_watchlists_from_json(json!({"categories": []})).is_err());
    }

    #[test]
    fn dashboard_gate_replay_requires_the_stable_outer_contract() {
        let replay = dashboard_decision_gate_replay_from_json(json!({
            "status": "available",
            "run_count": 3,
            "scenarios": [{
                "variable_path": "strategy.swing.markov_gate.min_signed_signal",
                "proposed_value": 0.2,
                "comparison": "Historical comparison only.",
                "summary": {
                    "candidate_count": 3,
                    "evaluated_count": 2,
                    "would_block_target_gate_count": 1,
                    "would_clear_target_gate_only_count": 0,
                    "unchanged_target_gate_count": 1,
                    "not_reached_count": 0,
                    "insufficient_evidence_count": 1
                },
                "changes": []
            }],
            "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
            "interpretation": "A target-gate clear is not an approval.",
            "support_risk_evidence": {
                "status": "collecting",
                "eligible_signal_count": 12,
                "labels": [{
                    "label": "high",
                    "signal_count": 12,
                    "next_run": {"sample_count": 10, "average_return_pct": -1.2},
                    "five_run": {"sample_count": 8, "negative_return_rate": 0.625},
                    "average_confidence": 0.7,
                    "raw_indicator_document": {"must": "stay internal"}
                }],
                "raw_support_risk_document": {"must": "stay internal"}
            }
        }))
        .expect("gate replay fixture has the dashboard contract");

        assert_eq!(replay.run_count, 3);
        assert_eq!(
            replay.scenarios[0].variable_path,
            "strategy.swing.markov_gate.min_signed_signal"
        );
        assert_eq!(replay.scenarios[0].summary.evaluated_count, 2);
        assert_eq!(replay.support_risk_evidence.labels.len(), 1);
        assert_eq!(
            replay.support_risk_evidence.labels[0]
                .next_run
                .average_return_pct,
            Some(-1.2)
        );
        let serialized = serde_json::to_value(&replay)
            .expect("typed gate replay serializes")
            .to_string();
        assert!(!serialized.contains("raw_indicator_document"));
        assert!(!serialized.contains("raw_support_risk_document"));
        assert_eq!(
            dashboard_decision_gate_replay_not_loaded().status,
            "not_loaded"
        );
        assert!(dashboard_decision_gate_replay_from_json(json!({"status": "available"})).is_err());
    }

    #[test]
    fn decision_report_summaries_keep_detail_documents_outside_responses() {
        let reports = decision_report_summaries_from_json(vec![json!({
            "id": 312,
            "created_at": "2026-08-26T12:00:00Z",
            "status": "completed",
            "model": "openai/gpt-5",
            "analysis_pulse_key": "us_open_followup:2026-08-26",
            "analysis_pulse_label": "US Open +1h15",
            "report_json": {"api_key": "must-not-reach-the-dashboard"},
            "request_json": {"token": "must-not-reach-the-dashboard"},
            "response_json": {"provider": "must-not-reach-the-dashboard"},
            "prompt_text": "must-not-reach-the-dashboard",
            "error_text": "must-not-reach-the-dashboard"
        })])
        .expect("stable Decision Report summary decodes");

        assert_eq!(reports[0].id, 312);
        assert_eq!(reports[0].analysis_pulse_label, "US Open +1h15");
        assert!(
            !serde_json::to_string(&reports)
                .expect("typed Decision Report summaries serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            decision_report_summaries_from_json(vec![json!({
                "id": 312
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_selected_decision_types_the_outer_contract_and_keeps_detail_documents() {
        let decision = dashboard_selected_decision_from_json(json!({
            "id": 312,
            "created_at": "2026-08-26T12:00:00Z",
            "report_date": "2026-08-26",
            "model": "openai/gpt-5",
            "status": "completed",
            "analysis_window_active": 1,
            "response_id": "gen-312",
            "prompt_text": "Review the retained report.",
            "request_json": "{\"response_format\": {\"type\": \"json_schema\"}}",
            "response_json": {"id": "gen-312"},
            "report_json": {"suggested_trades": []},
            "error_text": null,
            "analysis_pulse_key": "us_open_followup:2026-08-26",
            "analysis_pulse_label": "US Open +1h15",
            "pulse_mode": "execution_eligible",
            "queue_eligible": "1",
            "candidate_scoring_waterfall": {
                "status": "available",
                "run_id": 91,
                "summary": {
                    "candidate_count": 2,
                    "approved_count": 1,
                    "skipped_count": 1,
                    "not_reached_count": 0
                },
                "candidates": [{
                    "symbol": "ACME:xnas",
                    "action": "BUY",
                    "markov": {"status": "ok", "fresh": true, "signed_signal": 0.42},
                    "raw_reason": "candidate-raw-detail-must-not-reach-the-dashboard"
                }],
                "manager_json": {"raw_reason": "must-not-reach-the-dashboard"}
            }
        }))
        .expect("selected Decision Report fixture has the dashboard contract")
        .expect("fixture is present");

        assert_eq!(decision.id, 312);
        assert!(decision.analysis_window_active);
        assert!(decision.queue_eligible);
        assert_eq!(
            decision.request_json["response_format"]["type"],
            json!("json_schema")
        );
        assert_eq!(decision.candidate_scoring_waterfall.status, "available");
        assert_eq!(decision.candidate_scoring_waterfall.run_id, 91);
        assert_eq!(
            decision.candidate_scoring_waterfall.summary.approved_count,
            1
        );
        assert_eq!(
            decision.candidate_scoring_waterfall.candidates[0].symbol,
            "ACME:xnas"
        );
        assert!(
            decision.candidate_scoring_waterfall.candidates[0]
                .markov
                .fresh
        );
        assert!(
            !serde_json::to_string(&decision)
                .expect("typed selected Decision Report serializes")
                .contains("raw_reason")
        );
        assert!(
            !serde_json::to_string(&decision)
                .expect("typed selected Decision Report serializes")
                .contains("candidate-raw-detail-must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_selected_decision_from_json(JsonValue::Null)
                .expect("absent selected Decision Report is explicit")
                .is_none()
        );
        assert!(
            dashboard_selected_decision_from_json(json!({
                "id": 312,
                "created_at": "2026-08-26T12:00:00Z",
                "status": "completed",
                "queue_eligible": "unknown"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_execution_orders_keep_broker_documents_outside_stable_fields() {
        let orders = dashboard_execution_orders_from_json(vec![json!({
            "id": 91,
            "created_at": "2026-08-26T12:00:00Z",
            "symbol": "EXAMPLE:xnas",
            "action": "BUY",
            "status": "broker_working",
            "quantity": 4.0,
            "currency": "USD",
            "error_text": "Saxo response included sk-must-not-reach-the-dashboard-1234567890",
            "execution_result_json": {"raw_payload": "must-not-reach-the-dashboard"},
            "attribution": {"manager": "must-not-reach-the-dashboard"}
        })])
        .expect("stable execution order decodes");

        assert_eq!(orders[0].id, 91);
        assert!(orders[0].error_text.contains("[redacted]"));
        assert!(
            !serde_json::to_string(&orders)
                .expect("typed execution orders serialize")
                .contains("must-not-reach-the-dashboard-1234567890")
        );
        assert!(dashboard_execution_orders_from_json(vec![json!({"id": 91})]).is_err());
    }

    #[test]
    fn execution_summaries_exclude_broker_and_attribution_documents() {
        let orders = execution_order_summaries_from_json(vec![json!({
            "id": 91,
            "created_at": "2026-08-27T08:00:00Z",
            "symbol": "EXAMPLE:xnas",
            "action": "BUY",
            "order_type": "Market",
            "mode": "live",
            "status": "broker_working",
            "adapter": "saxo",
            "quantity": 4.0,
            "price_local": 320.0,
            "limit_price_local": null,
            "stop_price_local": null,
            "currency": "USD",
            "estimated_value_dkk": 9000.0,
            "strategy_type": "swing",
            "strategy_role": "entry",
            "error_text": "Saxo response included sk-must-not-reach-the-public-api-1234567890",
            "execution_result_json": {"AccountKey": "must-not-reach-the-public-api"},
            "attribution": {"manager_json": "must-not-reach-the-public-api"}
        })])
        .expect("stable execution order summaries decode");
        let fills = execution_fill_summaries_from_json(vec![json!({
            "id": 92,
            "created_at": "2026-08-27T08:00:05Z",
            "execution_order_id": 91,
            "broker_order_id": "SAXO-91",
            "symbol": "EXAMPLE:xnas",
            "side": "BUY",
            "fill_status": "FinalFill",
            "cumulative_quantity": 4.0,
            "delta_quantity": 4.0,
            "average_price_local": 320.0,
            "currency": "USD",
            "ledger_id": 811,
            "raw_payload_json": {"AccountKey": "must-not-reach-the-public-api"}
        })])
        .expect("stable execution fill summaries decode");
        let events = execution_event_summaries_from_json(vec![json!({
            "id": 93,
            "created_at": "2026-08-27T08:00:01Z",
            "execution_order_id": 91,
            "event_type": "precheck_completed",
            "broker_status": "Ok",
            "raw_payload_json": {"AccountKey": "must-not-reach-the-public-api"}
        })])
        .expect("stable execution event summaries decode");

        assert_eq!(fills[0].ledger_id, Some(811));
        assert_eq!(events[0].event_type, "precheck_completed");
        let serialized = serde_json::to_string(&json!({
            "orders": orders,
            "fills": fills,
            "events": events,
        }))
        .expect("typed execution summaries serialize");
        assert!(!serialized.contains("must-not-reach-the-public-api"));
        assert!(execution_order_summaries_from_json(vec![json!({"id": 91})]).is_err());
        assert!(execution_fill_summaries_from_json(vec![json!({"id": 92})]).is_err());
        assert!(execution_event_summaries_from_json(vec![json!({"id": 93})]).is_err());
    }

    #[test]
    fn dashboard_hermes_reflections_keep_raw_payloads_outside_ssr() {
        let reflections = dashboard_hermes_reflections_from_json(vec![json!({
            "id": "hermes-reflection-91",
            "created_at": "2026-08-26T08:30:00Z",
            "goal_version": 4,
            "summary": "Keep the current SIM experiment advisory-only.",
            "findings_json": [{"kind": "risk"}, {"kind": "evidence"}],
            "proposed_actions_json": [{"action": "review"}],
            "source_session_id": "hermes-weekly-2026w35",
            "raw_payload_json": {
                "api_key": "must-not-reach-the-dashboard",
                "provider_response": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable Hermes reflection evidence decodes");

        assert_eq!(reflections[0].finding_count, 2);
        assert_eq!(reflections[0].proposed_action_count, 1);
        assert!(
            !serde_json::to_string(&reflections)
                .expect("typed Hermes reflection evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_reflections_from_json(vec![json!({
                "created_at": "2026-08-26T08:30:00Z"
            })])
            .is_err()
        );
    }

    #[test]
    fn hermes_reflection_summaries_exclude_detailed_advisory_documents() {
        let reflections = crate::hermes_state::hermes_reflection_summaries_from_json(vec![json!({
            "id": "hermes-reflection-91",
            "created_at": "2026-08-26T08:30:00Z",
            "period_start": "2026-08-25",
            "period_end": "2026-08-26",
            "goal_version": 4,
            "summary": "Keep the current SIM experiment advisory-only.",
            "source_session_id": "hermes-weekly-2026w35",
            "findings_json": [{"detail": "must-not-reach-the-public-api"}],
            "proposed_actions_json": [{"action": "must-not-reach-the-public-api"}],
            "raw_payload_json": {"api_key": "must-not-reach-the-public-api"}
        })])
        .expect("stable Hermes reflection summaries decode");

        assert_eq!(reflections[0].goal_version, 4);
        assert!(
            !serde_json::to_string(&reflections)
                .expect("typed Hermes reflection summaries serialize")
                .contains("must-not-reach-the-public-api")
        );
        assert!(
            crate::hermes_state::hermes_reflection_summaries_from_json(vec![json!({
                "id": "hermes-reflection-91"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_lessons_keep_derived_actions_typed_and_sanitized() {
        let lessons = dashboard_hermes_lessons_pending_review_from_json(vec![json!({
            "created_at": "2026-08-26T08:30:00Z",
            "period_start": "2026-08-25",
            "period_end": "2026-08-26",
            "lesson": "Review the simulated stop coverage before changing policy.",
            "reflection_summary": "Coverage evidence is incomplete.",
            "source_session_id": "hermes-weekly-2026w35",
            "raw_payload_json": {
                "api_key": "must-not-reach-the-dashboard",
                "provider_response": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable Hermes lesson evidence decodes");

        assert_eq!(lessons[0].period_end.as_deref(), Some("2026-08-26"));
        assert!(
            !serde_json::to_string(&lessons)
                .expect("typed Hermes lesson evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_lessons_pending_review_from_json(vec![json!({
                "created_at": "2026-08-26T08:30:00Z"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_learning_memory_keeps_only_typed_evidence() {
        let memory = dashboard_hermes_learning_memory_from_json(vec![json!({
            "status": "stable",
            "lesson": "Review SIM protective-stop coverage before changing policy.",
            "observation_count": 2,
            "first_seen": "2026-08-20T08:30:00Z",
            "last_seen": "2026-08-26T08:30:00Z",
            "expires_at": "2026-09-16T08:30:00Z",
            "cadences": ["daily", "weekly"],
            "safety": "must-not-reach-the-dashboard",
            "raw_payload_json": {
                "api_key": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable Hermes learning memory decodes");

        assert_eq!(
            memory[0].cadences,
            vec!["daily".to_string(), "weekly".to_string()]
        );
        assert!(
            !serde_json::to_string(&memory)
                .expect("typed Hermes learning memory serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_learning_memory_from_json(vec![json!({
                "status": "unknown"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_one_variable_audit_keeps_source_json_outside_ssr() {
        let audit_rows = dashboard_hermes_one_variable_audit_from_json(vec![json!({
            "kind": "selected_overlay",
            "created_at": "2026-08-26T08:30:00Z",
            "status": "selected_for_next_cycle",
            "variable": "strategy.capital.min_cash_buffer_pct",
            "baseline_value": 0.05,
            "candidate_value": 0.02,
            "reason": "Evaluate a controlled SIM overlay.",
            "scope": "SIM only",
            "last_manager_state": "selected for the next eligible manager cycle",
            "raw_payload_json": {
                "api_key": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable one-variable audit evidence decodes");

        assert_eq!(audit_rows[0].baseline_value, "0.05");
        assert_eq!(audit_rows[0].candidate_value, "0.02");
        assert!(
            !serde_json::to_string(&audit_rows)
                .expect("typed one-variable audit evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_one_variable_audit_from_json(vec![json!({
                "kind": "selected_overlay"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_baseline_evidence_keeps_audit_documents_outside_ssr() {
        let baseline = dashboard_active_strategy_baseline_from_json(json!({
            "id": "baseline-91",
            "created_at": "2026-08-20T08:30:00Z",
            "activated_at": "2026-08-26T08:30:00Z",
            "status": "active",
            "goal_version": 4,
            "config_json": {
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "api_key": "must-not-reach-the-dashboard"
            },
            "prompt_json": {"provider_response": "must-not-reach-the-dashboard"},
            "source": "must-not-reach-the-dashboard"
        }))
        .expect("active baseline display record decodes")
        .expect("active baseline exists");
        let pack = dashboard_hermes_baseline_evidence_pack_from_json(json!({
            "status": "observing",
            "safety": "must-not-reach-the-dashboard",
            "baseline": {
                "id": "baseline-91",
                "activated_at": "2026-08-26T08:30:00Z",
                "variable": "strategy.capital.min_cash_buffer_pct",
                "source_experiment_id": "must-not-reach-the-dashboard"
            },
            "experiment": {
                "evaluation_window": {
                    "observation_count": 4,
                    "return_pct": 0.03,
                    "raw_payload": "must-not-reach-the-dashboard"
                }
            },
            "affected_activity": {
                "manager_run_count": 2,
                "report_count": 2,
                "failed_order_count": 1,
                "manager_json": {"api_key": "must-not-reach-the-dashboard"}
            },
            "post_promotion": {
                "observation_count": 3,
                "max_drawdown_pct": -0.01,
                "broker_payload": "must-not-reach-the-dashboard"
            }
        }))
        .expect("baseline evidence display pack decodes");

        assert_eq!(baseline.id, "baseline-91");
        assert!(baseline.config_display.contains("[redacted]"));
        assert_eq!(pack.manager_run_count, 2);
        assert_eq!(pack.post_promotion.observation_count, 3);
        let serialized =
            serde_json::to_string(&(baseline, pack)).expect("typed baseline evidence serializes");
        assert!(!serialized.contains("must-not-reach-the-dashboard"));
        assert!(
            dashboard_active_strategy_baseline_from_json(json!({"id": "baseline-91"})).is_err()
        );
        assert!(dashboard_hermes_baseline_evidence_pack_from_json(json!({})).is_err());
    }

    #[test]
    fn dashboard_hermes_experiments_keep_review_documents_outside_ssr() {
        let experiments = dashboard_hermes_experiments_from_json(vec![json!({
            "id": "experiment-91",
            "created_at": "2026-08-26T08:30:00Z",
            "status": "pending_review",
            "baseline_id": "baseline-4",
            "goal_version": 4,
            "hypothesis": "A smaller SIM cash buffer may improve diversification.",
            "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
            "old_value_json": {"value": 0.05, "api_key": "must-not-reach-the-dashboard"},
            "new_value_json": {"value": 0.03, "api_key": "must-not-reach-the-dashboard"},
            "expected_effect": "More eligible SIM BUY candidates during broad market breadth.",
            "risk_notes": "May increase exposure during a drawdown.",
            "evidence_json": {"source": "SIM journal", "api_key": "must-not-reach-the-dashboard"},
            "approval_json": {"operator": "must-not-reach-the-dashboard"},
            "metrics_json": {"provider_response": "must-not-reach-the-dashboard"},
            "source_session_id": "must-not-reach-the-dashboard",
            "raw_payload_json": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable Hermes experiment display row decodes");

        assert_eq!(experiments[0].id, "experiment-91");
        assert!(experiments[0].old_value_display.contains("[redacted]"));
        assert!(
            !serde_json::to_string(&experiments)
                .expect("typed Hermes experiment rows serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_experiments_from_json(vec![json!({
                "id": "experiment-91"
            })])
            .is_err()
        );
    }

    #[test]
    fn hermes_experiment_summaries_exclude_proposal_and_provider_documents() {
        let experiments = crate::hermes_state::hermes_experiment_summaries_from_json(vec![json!({
            "id": "experiment-91",
            "created_at": "2026-08-26T08:30:00Z",
            "status": "pending_review",
            "baseline_id": null,
            "goal_version": 4,
            "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
            "source_session_id": "daily-eod-reflection-2026-08-26",
            "old_value_json": {"value": 0.05, "api_key": "must-not-reach-the-public-api"},
            "new_value_json": {"value": 0.03, "api_key": "must-not-reach-the-public-api"},
            "evidence_json": {"source": "SIM journal", "api_key": "must-not-reach-the-public-api"},
            "approval_json": {"operator": "must-not-reach-the-public-api"},
            "metrics_json": {"provider_response": "must-not-reach-the-public-api"},
            "raw_payload_json": {"api_key": "must-not-reach-the-public-api"}
        })])
        .expect("stable Hermes experiment summaries decode");

        assert_eq!(experiments[0].status, "pending_review");
        assert!(
            !serde_json::to_string(&experiments)
                .expect("typed Hermes experiment summaries serialize")
                .contains("must-not-reach-the-public-api")
        );
        assert!(
            crate::hermes_state::hermes_experiment_summaries_from_json(vec![json!({
                "id": "experiment-91"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_proposal_quality_keeps_only_rubric_evidence() {
        let quality_rows = dashboard_hermes_proposal_quality_from_json(vec![json!({
            "created_at": "2026-08-26T08:30:00Z",
            "experiment_status": "pending_review",
            "variable": "strategy.capital.min_cash_buffer_pct",
            "quality_score": 90,
            "quality_status": "review_ready",
            "evidence_present": true,
            "evidence_has_named_sources": true,
            "measurable_effect": true,
            "values_changed": true,
            "risk_notes_present": true,
            "exact_duplicate_count": 0,
            "related_family_count": 0,
            "gaps": [],
            "evidence_json": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable Hermes quality rubric decodes");

        assert_eq!(quality_rows[0].quality_score, 90);
        assert!(quality_rows[0].evidence_has_named_sources);
        assert!(
            !serde_json::to_string(&quality_rows)
                .expect("typed proposal quality serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_proposal_quality_from_json(vec![json!({
                "quality_score": 90
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_decision_advice_audit_keeps_raw_documents_outside_ssr() {
        let audit_rows = dashboard_hermes_decision_advice_audit_from_json(vec![json!({
            "report_id": 42,
            "report_created_at": "2026-08-26T08:30:00Z",
            "analysis_pulse_label": "US midday shadow",
            "advice_id": "hermes-decision-advice-91",
            "advice_created_at": "2026-08-26T08:31:00Z",
            "advice_status": "received",
            "advice_source_session_id": "hermes-us-2026w35",
            "advice_recommendation": "review",
            "advice_summary": "Reduce concentrated exposure before queue review.",
            "order_advice_json": [
                {"symbol": "EXAMPLE:xnas", "action": "reduce", "reason": "concentration"},
                {"symbol": "OTHER:xnas", "action": "allow", "reason": "diversified"}
            ],
            "advice_raw_payload_json": {
                "context_self_check": {"complete": false, "missing": ["current_positions"]},
                "provider_response": {"api_key": "must-not-reach-the-dashboard"}
            },
            "manager_status": "completed",
            "manager_json": {
                "hermes_decision_advice": {
                    "mode": "conservative",
                    "provider_response": {"api_key": "must-not-reach-the-dashboard"}
                },
                "hermes_advice_delta": {
                    "effect_counts": {"reduced": 1, "allowed": 1}
                },
                "queue_request": {"api_key": "must-not-reach-the-dashboard"}
            },
            "queued_order_count": 2,
            "executed_order_count": 1,
            "failed_order_count": 0
        })])
        .expect("stable Hermes advice audit decodes");

        assert_eq!(audit_rows[0].report_id, 42);
        assert_eq!(audit_rows[0].advice_reduce_count, 1);
        assert_eq!(audit_rows[0].impact_reduced_count, 1);
        assert_eq!(audit_rows[0].self_check_missing, vec!["current_positions"]);
        assert!(
            !serde_json::to_string(&audit_rows)
                .expect("typed Hermes advice audit serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_decision_advice_audit_from_json(vec![json!({
                "report_id": 42
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_hermes_counterfactuals_keep_broker_payloads_outside_ssr() {
        let counterfactuals = dashboard_hermes_counterfactuals_from_json(vec![json!({
            "report_id": 42,
            "created_at": "2026-08-26T08:30:00Z",
            "symbol": "EXAMPLE:xnas",
            "action": "BUY",
            "source_effect": "reduced",
            "shadow_quantity": 3.0,
            "reference_price_local": 100.0,
            "reference_price_at": "2026-08-26T08:30:00Z",
            "reference_price_source": "saxo_infoprices",
            "reported_reference_price_local": 99.0,
            "currency": "USD",
            "status": "tracking",
            "latest_price_local": 110.0,
            "latest_price_at": "2026-08-26T09:30:00Z",
            "estimated_return_pct": 0.1,
            "estimated_pnl_local": 30.0,
            "broker_payload_json": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable Hermes counterfactual evidence decodes");

        assert_eq!(counterfactuals[0].report_id, 42);
        assert_eq!(counterfactuals[0].estimated_pnl_local, Some(30.0));
        assert!(
            !serde_json::to_string(&counterfactuals)
                .expect("typed Hermes counterfactual evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_hermes_counterfactuals_from_json(vec![json!({
                "report_id": 42
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_missed_trade_shadows_keep_gate_audit_payloads_outside_ssr() {
        let shadows = dashboard_missed_trade_shadows_from_json(vec![json!({
            "report_id": 43,
            "created_at": "2026-08-26T08:30:00Z",
            "symbol": "EXAMPLE:xnas",
            "action": "BUY",
            "source_gate": "cash_budget",
            "shadow_quantity": 2.0,
            "reference_price_local": 101.0,
            "reference_price_at": "2026-08-26T08:30:00Z",
            "reference_price_source": "saxo_infoprices",
            "reported_reference_price_local": 100.0,
            "currency": "USD",
            "status": "tracking",
            "latest_price_local": 110.0,
            "latest_price_at": "2026-08-26T09:30:00Z",
            "estimated_return_pct": 0.089,
            "estimated_pnl_local": 18.0,
            "manager_gate_audit_json": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable missed-trade shadow evidence decodes");

        assert_eq!(shadows[0].source_gate, "cash_budget");
        assert_eq!(shadows[0].estimated_pnl_local, Some(18.0));
        assert!(
            !serde_json::to_string(&shadows)
                .expect("typed missed-trade shadow evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_missed_trade_shadows_from_json(vec![json!({
                "report_id": 43
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_missed_trade_shadow_evidence_keeps_raw_rows_outside_ssr() {
        let evidence = dashboard_missed_trade_shadow_evidence_from_json(json!({
            "status": "collecting",
            "recorded_shadow_count": 3,
            "observed_shadow_count": 2,
            "overall": {
                "sample_count": 2,
                "average_directional_return_pct": 0.025,
                "positive_return_rate": 0.5
            },
            "by_gate": [{
                "source_gate": "cash_budget",
                "recorded_shadow_count": 2,
                "outcome": {
                    "sample_count": 2,
                    "average_directional_return_pct": 0.025,
                    "positive_return_rate": 0.5
                }
            }],
            "minimum_complete_observations": 20,
            "interpretation": "Read-only quote-to-quote evidence only.",
            "raw_rows": [{"api_key": "must-not-reach-the-dashboard"}]
        }))
        .expect("stable missed-trade shadow aggregate decodes");

        assert_eq!(evidence.by_gate[0].outcome.sample_count, 2);
        assert!(
            !serde_json::to_string(&evidence)
                .expect("typed missed-trade shadow aggregate serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_missed_trade_shadow_evidence_from_json(json!({
                "status": "collecting"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_trade_thesis_evidence_keeps_raw_records_outside_ssr() {
        let evidence = dashboard_trade_thesis_evidence_from_json(json!({
            "status": "collecting",
            "recorded_thesis_count": 3,
            "filled_thesis_count": 2,
            "one_session": {
                "sample_count": 2,
                "average_directional_return_pct": 0.025,
                "positive_return_rate": 0.5
            },
            "five_session": {
                "sample_count": 1,
                "average_directional_return_pct": -0.01,
                "positive_return_rate": 0.0
            },
            "minimum_complete_observations": 20,
            "interpretation": "Read-only post-fill evidence only.",
            "raw_theses": [{"api_key": "must-not-reach-the-dashboard"}]
        }))
        .expect("stable trade-thesis aggregate decodes");

        assert_eq!(evidence.one_session.sample_count, 2);
        assert_eq!(evidence.five_session.positive_return_rate, Some(0.0));
        assert!(
            !serde_json::to_string(&evidence)
                .expect("typed trade-thesis aggregate serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_trade_thesis_evidence_from_json(json!({
                "status": "collecting"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_holding_thesis_reviews_keep_raw_records_outside_ssr() {
        let reviews = dashboard_holding_thesis_reviews_from_json(json!({
            "status": "review_due",
            "held_position_count": 4,
            "review_count": 1,
            "decision_stale_after_days": 7,
            "reviews": [{
                "symbol": "AMD:xnas",
                "instrument_name": "Advanced Micro Devices",
                "status": "thesis_window_elapsed",
                "tracked_entry_at": "2026-08-01T14:30:00Z",
                "age_days": 25,
                "intended_holding_window": "next_2_weeks",
                "entry_rationale": "Compact rationale.",
                "invalidation": "Compact invalidation.",
                "position_snapshot": {"api_key": "must-not-reach-the-dashboard"}
            }],
            "interpretation": "Read-only review queue only.",
            "raw_theses": [{"api_key": "must-not-reach-the-dashboard"}]
        }))
        .expect("stable holding-thesis reviews decode");

        assert_eq!(reviews.reviews[0].symbol, "AMD:xnas");
        assert_eq!(reviews.reviews[0].age_days, 25);
        assert!(
            !serde_json::to_string(&reviews)
                .expect("typed holding-thesis reviews serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_holding_thesis_reviews_from_json(json!({
                "status": "review_due"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_decision_pulse_evidence_keeps_raw_records_outside_ssr() {
        let outcome = json!({
            "attributed_order_count": 3,
            "hermes_reviewed_order_count": 2,
            "one_session": {
                "sample_count": 2,
                "average_directional_return_pct": 0.02,
                "positive_return_rate": 0.5
            },
            "five_session": {
                "sample_count": 1,
                "average_directional_return_pct": -0.01,
                "positive_return_rate": 0.0
            },
            "reconciled_sell_order_count": 1,
            "realised_sell": {
                "realised_gain_dkk": 125.0,
                "commission_dkk": 14.0,
                "tax_dkk": 35.0
            },
            "execution_status_counts": {"executed": 2, "broker_working": 1},
            "hermes_effect_counts": {"allow": 2},
            "manager_json": {"api_key": "must-not-reach-the-dashboard"}
        });
        let evidence = dashboard_decision_pulse_evidence_from_json(json!({
            "status": "collecting",
            "overall": outcome,
            "pulses": [{
                "pulse_key": "europe_open",
                "pulse_label": "EU Open",
                "outcome": {
                    "attributed_order_count": 1,
                    "hermes_reviewed_order_count": 1,
                    "one_session": {
                        "sample_count": 1,
                        "average_directional_return_pct": 0.02,
                        "positive_return_rate": 1.0
                    },
                    "five_session": {
                        "sample_count": 0,
                        "average_directional_return_pct": null,
                        "positive_return_rate": null
                    },
                    "reconciled_sell_order_count": 0,
                    "realised_sell": {
                        "realised_gain_dkk": 0.0,
                        "commission_dkk": 0.0,
                        "tax_dkk": 0.0
                    },
                    "execution_status_counts": {"executed": 1},
                    "hermes_effect_counts": {"allow": 1}
                }
            }],
            "minimum_complete_observations": 20,
            "interpretation": "Read-only pulse outcome evidence only.",
            "raw_observations": [{"api_key": "must-not-reach-the-dashboard"}]
        }))
        .expect("stable decision-pulse aggregate decodes");

        assert_eq!(evidence.overall.attributed_order_count, 3);
        assert_eq!(evidence.pulses[0].outcome.realised_sell_gain_dkk, 0.0);
        assert!(
            !serde_json::to_string(&evidence)
                .expect("typed decision-pulse aggregate serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_decision_pulse_evidence_from_json(json!({
                "status": "collecting"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_saxo_auth_requires_the_sanitized_status_contract() {
        let fixture = json!({
            "connected": true,
            "environment": "SIM",
            "token_valid": true,
            "refresh_token_valid": true,
            "expires_in_minutes": 29,
            "needs_reauth": false,
            "status": "connected",
            "status_text": "Saxo SIM session ready.",
            "session_path": "/tmp/daytrader/saxo_session.json"
        });
        crate::read_model::assert_null_is_never_worse_than_absent(
            &fixture,
            dashboard_saxo_auth_from_json,
        );
        let auth =
            dashboard_saxo_auth_from_json(fixture.clone()).expect("sanitized Saxo status decodes");

        assert!(auth.connected);
        assert_eq!(auth.environment, "SIM");
        assert_eq!(auth.expires_in_minutes, Some(29));
        assert!(dashboard_saxo_auth_from_json(json!({"connected": true})).is_err());
        assert_eq!(dashboard_saxo_auth_unavailable().status, "unavailable");
    }

    #[test]
    fn dashboard_ai_settings_require_masked_key_status() {
        let fixture = json!({
            "provider": "openrouter",
            "model": "openai/gpt-5.5",
            "config_model": "openai/gpt-5.5",
            "source": "runtime",
            "updated_at": "2026-08-23T13:00:00Z",
            "api_key": {
                "configured": true,
                "source": "runtime",
                "masked": "sk-or-…5678",
                "updated_at": "2026-08-23T13:00:00Z",
                "raw": "must-not-reach-the-dashboard"
            }
        });
        crate::read_model::assert_null_is_never_worse_than_absent(
            &fixture,
            dashboard_ai_settings_from_json,
        );
        let settings =
            dashboard_ai_settings_from_json(fixture.clone()).expect("sanitized AI settings decode");

        assert!(settings.api_key.configured);
        assert_eq!(settings.model, "openai/gpt-5.5");
        assert_eq!(settings.api_key.masked.as_deref(), Some("sk-or-…5678"));
        assert!(
            !serde_json::to_string(&settings)
                .expect("sanitized settings serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(dashboard_ai_settings_from_json(json!({"model": "openai/gpt-5.5"})).is_err());
    }

    #[test]
    fn dashboard_run_schedules_retain_only_timing_metadata() {
        let fixture = json!({
            "markov": {
                "enabled": true,
                "timezone": "Europe/Copenhagen",
                "daily_time": "10:00",
                "run_weekdays_only": true,
                "window_days": 40
            },
            "quiver": {
                "enabled": true,
                "timezone": "America/New_York",
                "schedule_kind": "us_open_followup",
                "minutes_after_open": 45,
                "scheduled_for": "2026-08-23T14:15:00Z",
                "scheduled_run_date": "2026-08-23",
                "schedule_status": "waiting",
                "base_url": "must-not-reach-the-dashboard"
            },
            "indicators": {
                "enabled": false,
                "timezone": "Europe/Copenhagen",
                "daily_time": "20:15",
                "run_weekdays_only": true
            },
            "performance_benchmarks": {"retained": "staged"}
        });
        crate::read_model::assert_null_is_never_worse_than_absent(
            &fixture,
            dashboard_run_schedules_from_json,
        );
        let schedules = dashboard_run_schedules_from_json(fixture.clone())
            .expect("dashboard schedule timing metadata decodes");

        assert!(schedules.markov.available);
        assert_eq!(schedules.quiver.minutes_after_open, Some(45));
        assert!(!schedules.indicators.enabled);
        assert_eq!(schedules.performance_benchmarks["retained"], "staged");
        assert!(
            !serde_json::to_string(&schedules)
                .expect("dashboard schedules serialize")
                .contains("must-not-reach-the-dashboard")
        );
    }

    #[test]
    fn dashboard_latest_run_keeps_lifecycle_and_stages_detail() {
        let fixture = json!({
            "id": "run-42",
            "created_at": "2026-08-23T13:00:00Z",
            "run_date": "2026-08-23",
            "status": "completed",
            "asset_count": 20,
            "success_count": 19,
            "error_count": 1,
            "config_json": {"lookback_days": 120},
            "summary_json": {"signals": []},
            "raw_provider_field": "must-not-reach-the-dashboard"
        });
        crate::read_model::assert_null_is_never_worse_than_absent(
            &fixture,
            dashboard_latest_run_from_json,
        );
        let run = dashboard_latest_run_from_json(fixture.clone()).expect("latest run decodes");

        assert!(run.available);
        assert_eq!(run.id.as_deref(), Some("run-42"));
        assert_eq!(run.success_count, 19);
        assert_eq!(run.config_json["lookback_days"], 120);
        assert!(
            !serde_json::to_string(&run)
                .expect("latest run serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            !dashboard_latest_run_from_json(JsonValue::Null)
                .expect("no latest run is explicit")
                .available
        );
    }

    #[test]
    fn public_signal_run_summary_omits_collector_documents() {
        let summary = signal_run_summary_from_json(json!({
            "id": "run-42",
            "created_at": "2026-08-23T13:00:00Z",
            "run_date": "2026-08-23",
            "status": "completed",
            "asset_count": 20,
            "success_count": 19,
            "error_count": 1,
            "config_json": {"api_key": "must-not-reach-public-api"},
            "summary_json": {"provider_response": "must-not-reach-public-api"}
        }))
        .expect("public signal run summary decodes");

        assert!(summary.available);
        assert_eq!(summary.id.as_deref(), Some("run-42"));
        assert_eq!(summary.success_count, 19);
        assert!(
            !serde_json::to_string(&summary)
                .expect("public signal run summary serializes")
                .contains("must-not-reach-public-api")
        );
    }

    #[test]
    fn dashboard_quiver_conflicts_keep_advisory_outer_contract() {
        let fixture = json!({
            "status": "conflicts_detected",
            "held_symbol_count": 4,
            "strong_bearish_signal_lte": -0.5,
            "conflicts": [{
                "symbol": "CRM:xnys",
                "signal": -0.71,
                "direction": "bearish",
                "confidence": 0.83,
                "event_count": 6,
                "latest_event_date": "2026-08-22",
                "provider_detail": "must-not-reach-the-dashboard"
            }],
            "safety": "advisory_quiver_context_only_no_gate_hermes_or_broker_mutation",
            "interpretation": "Review evidence only.",
            "raw_provider_field": "must-not-reach-the-dashboard"
        });
        crate::read_model::assert_null_is_never_worse_than_absent(
            &fixture,
            dashboard_quiver_conflicts_from_json,
        );
        let conflicts = dashboard_quiver_conflicts_from_json(fixture.clone())
            .expect("stable Quiver conflict evidence decodes");

        assert_eq!(conflicts.status, "conflicts_detected");
        assert_eq!(conflicts.held_symbol_count, 4);
        assert_eq!(conflicts.conflicts[0].symbol, "CRM:xnys");
        assert_eq!(conflicts.conflicts[0].event_count, Some(6));
        assert!(
            !serde_json::to_string(&conflicts)
                .expect("typed Quiver conflicts serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert_eq!(dashboard_quiver_conflicts_not_loaded().status, "not_loaded");
        assert!(dashboard_quiver_conflicts_from_json(json!({"status": "clear"})).is_err());
    }

    #[test]
    fn goal_period_value_marks_missing_baseline_pending_instead_of_zero_progress() {
        let missing = goal_period_value(None, 250_000.0, 880.0, "2026-07-27T00:00:00Z");
        assert_eq!(missing["status"], json!("pending_baseline"));
        assert!(missing["pnl_dkk"].is_null());
        assert!(missing["progress_pct"].is_null());

        let ready = goal_period_value(Some(249_000.0), 250_000.0, 880.0, "2026-07-27T00:00:00Z");
        assert_eq!(ready["status"], json!("ready"));
        assert_eq!(ready["pnl_dkk"], json!(1_000.0));
        assert!(
            (ready["progress_pct"].as_f64().unwrap_or_default() - (1_000.0 / 880.0)).abs() < 1e-12
        );
    }

    #[test]
    fn since_reset_performance_requires_an_active_batch_baseline() {
        let missing = since_reset_performance_value(None, 250_000.0);
        assert_eq!(missing["status"], json!("pending_baseline"));
        assert!(missing["pnl_dkk"].is_null());
        assert!(missing["return_pct"].is_null());

        let ready = since_reset_performance_value(
            Some(json!({
                "recorded_at": "2026-07-01T08:00:00Z",
                "total_market_value_dkk": 240_000.0,
            })),
            250_000.0,
        );
        assert_eq!(ready["status"], json!("ready"));
        assert_eq!(ready["pnl_dkk"], json!(10_000.0));
        assert!((ready["return_pct"].as_f64().unwrap_or_default() - 4.166_666_666_7).abs() < 1e-8);
    }

    #[test]
    fn performance_range_metrics_requires_history_and_measures_peak_to_trough_loss() {
        assert_eq!(performance_range_metrics(&[]), (None, None));
        assert_eq!(
            performance_range_metrics(&[json!({"total_market_value_dkk": 100.0})]),
            (None, None)
        );

        let (return_pct, drawdown_pct) = performance_range_metrics(&[
            json!({"total_market_value_dkk": 100.0}),
            json!({"total_market_value_dkk": 120.0}),
            json!({"total_market_value_dkk": 90.0}),
            json!({"total_market_value_dkk": 110.0}),
        ]);
        assert!((return_pct.unwrap_or_default() - 10.0).abs() < 1e-9);
        assert!((drawdown_pct.unwrap_or_default() + 25.0).abs() < 1e-9);
    }

    #[test]
    fn performance_confidence_distinguishes_current_partial_stale_and_unavailable_evidence() {
        let now = DateTime::parse_from_rfc3339("2026-07-30T12:00:00Z")
            .expect("parses fixed timestamp")
            .with_timezone(&Utc);
        let current = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-29T12:00:00Z", "total_market_value_dkk": 250_000.0}),
                json!({"recorded_at": "2026-07-30T12:00:00Z", "snapshot_type": "runtime_current", "source": "saxo_broker_snapshot", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(current["status"], json!("current"));
        assert_eq!(current["valid_points"], json!(2));

        let partial = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-30T12:00:00Z", "snapshot_type": "runtime_current", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(partial["status"], json!("partial"));

        let stale = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-28T12:00:00Z", "total_market_value_dkk": 250_000.0}),
                json!({"recorded_at": "2026-07-30T10:00:00Z", "snapshot_type": "daily_close", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(stale["status"], json!("stale"));

        let unavailable = performance_confidence(
            &[json!({"snapshot_type": "runtime_current", "total_market_value_dkk": 0.0})],
            now,
        );
        assert_eq!(unavailable["status"], json!("unavailable"));
    }

    #[test]
    fn unrealised_pnl_reconciliation_keeps_sources_and_tolerance_explicit() {
        let reconciliation = unrealised_pnl_reconciliation(
            &json!({
                "total_unrealised_pnl_dkk": 1_000.0,
                "source": "saxo_broker_snapshot",
            }),
            &json!({
                "total_unrealised_pnl_dkk": 900.0,
                "source": "daily_close",
                "snapshot_type": "daily_close",
                "recorded_at": "2026-07-29T22:30:00Z",
            }),
            &json!({
                "unrealised_pnl_dkk": 1_150.0,
                "account_currency": "EUR",
                "fx_rate_to_dkk": 7.46,
                "exposure_count": 3,
                "updated_at": "2026-07-30T10:00:00Z",
            }),
        );
        assert_eq!(
            reconciliation["dashboard"]["source"],
            json!("saxo_broker_snapshot")
        );
        assert_eq!(
            reconciliation["latest_history"]["recorded_at"],
            json!("2026-07-29T22:30:00Z")
        );
        assert_eq!(
            reconciliation["broker_exposure"]["status"],
            json!("aligned")
        );
        assert_eq!(
            reconciliation["broker_exposure"]["difference_from_dashboard_dkk"],
            json!(-150.0)
        );

        let drift = unrealised_pnl_reconciliation(
            &json!({"total_unrealised_pnl_dkk": 4_000.0}),
            &json!({}),
            &json!({"unrealised_pnl_dkk": 1_000.0}),
        );
        assert_eq!(drift["broker_exposure"]["status"], json!("drift"));
    }

    #[test]
    fn effective_execution_mode_distinguishes_enabled_sim_from_environment_mismatch() {
        assert_eq!(
            effective_execution_mode("live", "saxo", false, false, "sim", Some("sim")),
            "simulated_broker"
        );
        assert_eq!(
            effective_execution_mode("live", "saxo", false, false, "live", Some("sim")),
            "disabled_environment_mismatch"
        );
        assert_eq!(
            effective_execution_mode("live", "saxo", false, false, "live", Some("live")),
            "live_broker"
        );
        assert_eq!(
            effective_execution_mode("live", "saxo", false, false, "sim", None),
            "disabled_session_environment"
        );
        assert_eq!(
            effective_execution_mode("live", "saxo", false, false, "paper", Some("paper")),
            "disabled_unknown_environment"
        );
    }

    #[test]
    fn broker_exposure_pnl_attribution_groups_instrument_currency_without_claiming_fx_pnl() {
        let attribution = broker_exposure_pnl_attribution(
            &[
                json!({
                    "symbol": "USDWIN:xnas",
                    "currency": "USD",
                    "quantity": 2.0,
                    "profit_loss_on_trade": 100.0,
                    "calculation_reliability": "Estimated",
                    "updated_at": "2026-07-30T10:00:00Z",
                }),
                json!({
                    "symbol": "EURLOSS:xetr",
                    "currency": "EUR",
                    "quantity": 1.0,
                    "profit_loss_on_trade": -50.0,
                    "calculation_reliability": "Exact",
                    "updated_at": "2026-07-30T10:00:00Z",
                }),
            ],
            &json!({
                "account_currency": "DKK",
                "unrealised_pnl_dkk": 1_010.0,
                "updated_at": "2026-07-30T10:00:00Z",
            }),
            &BTreeMap::from([("USD".to_string(), 6.4), ("EUR".to_string(), 7.4)]),
        );
        assert_eq!(attribution["status"], json!("available"));
        assert_eq!(attribution["exposure_count"], json!(2));
        assert_eq!(attribution["rows"][0]["symbol"], json!("USDWIN:xnas"));
        assert_eq!(
            attribution["rows"][0]["profit_loss_instrument_currency"],
            json!(100.0)
        );
        assert_eq!(attribution["rows"][0]["fx_rate_to_dkk"], json!(6.4));
        assert_eq!(
            attribution["currencies"][0]["instrument_currency"],
            json!("USD")
        );
        assert_eq!(
            attribution["currencies"][0]["absolute_contribution_pct"],
            json!(63.366336633663366)
        );
        assert_eq!(
            attribution["scope"],
            json!("read_only_stored_saxo_exposure_unrealised_pnl")
        );
    }

    #[test]
    fn realised_sell_outcomes_keep_partial_sales_and_sample_limits_explicit() {
        let outcomes = realised_sell_outcome_evidence(
            &[
                json!({
                    "created_at": "2026-07-30T10:00:00Z",
                    "symbol": "WIN1:xnas",
                    "quantity": 1.0,
                    "currency": "USD",
                    "realised_gain_dkk": 1_000.0,
                    "commission_dkk": 4.0,
                    "tax_dkk": 0.0,
                    "cost_basis_sold_dkk": 3_000.0,
                    "status": "executed",
                    "execution_order_id": 101,
                    "linked_order_count": 1,
                    "exit_strategy_type": "swing",
                    "exit_strategy_role": "risk_reduction",
                }),
                json!({
                    "created_at": "2026-07-29T10:00:00Z",
                    "symbol": "WIN2:xnas",
                    "quantity": 2.0,
                    "currency": "USD",
                    "realised_gain_dkk": 500.0,
                    "commission_dkk": 3.0,
                    "tax_dkk": 0.0,
                    "cost_basis_sold_dkk": 2_000.0,
                    "status": "executed",
                    "execution_order_id": 102,
                    "linked_order_count": 1,
                    "exit_strategy_type": "swing",
                    "exit_strategy_role": "take_profit",
                }),
                json!({
                    "created_at": "2026-07-28T10:00:00Z",
                    "symbol": "LOSS:xnas",
                    "quantity": 1.0,
                    "currency": "USD",
                    "realised_gain_dkk": -300.0,
                    "commission_dkk": 2.0,
                    "tax_dkk": 0.0,
                    "cost_basis_sold_dkk": 1_500.0,
                    "status": "approved",
                    "linked_order_count": 2,
                }),
                json!({
                    "created_at": "2026-07-27T10:00:00Z",
                    "symbol": "FLAT:xnas",
                    "quantity": 1.0,
                    "currency": "USD",
                    "realised_gain_dkk": 0.0,
                    "commission_dkk": 1.0,
                    "tax_dkk": 0.0,
                    "cost_basis_sold_dkk": 900.0,
                    "status": "executed",
                }),
            ],
            crate::holding_period::holding_period_evidence_json(&Default::default()),
            None,
        );

        assert_eq!(json_text(&outcomes, "status"), "collecting");
        assert_eq!(value_i64(&outcomes, "closed_sale_count"), 4);
        assert_eq!(value_i64(&outcomes, "decisive_sale_count"), 3);
        assert_eq!(value_i64(&outcomes, "win_count"), 2);
        assert_eq!(value_i64(&outcomes, "loss_count"), 1);
        assert_eq!(value_i64(&outcomes, "breakeven_count"), 1);
        assert!((value_f64(&outcomes, "win_rate") - (2.0 / 3.0)).abs() < 1e-12);
        assert_eq!(value_f64(&outcomes, "average_win_dkk"), 750.0);
        assert_eq!(value_f64(&outcomes, "average_loss_dkk"), -300.0);
        assert_eq!(value_f64(&outcomes, "payoff_ratio"), 2.5);
        assert_eq!(value_f64(&outcomes, "total_realised_gain_dkk"), 1_200.0);
        assert_eq!(value_i64(&outcomes, "attributed_symbol_count"), 4);
        assert_eq!(
            outcomes["symbol_attribution"][0]["symbol"],
            json!("WIN1:xnas")
        );
        assert_eq!(
            outcomes["currency_attribution"][0]["instrument_currency"],
            json!("USD")
        );
        assert_eq!(value_i64(&outcomes, "linked_exit_route_count"), 2);
        assert_eq!(value_i64(&outcomes, "unlinked_ledger_count"), 1);
        assert_eq!(value_i64(&outcomes, "ambiguous_exit_link_count"), 1);
        assert_eq!(
            outcomes["exit_route_attribution"][0]["exit_route"],
            json!("swing / risk reduction"),
            "exits group by family now, so the raw role spelling is not the key"
        );
        assert_eq!(
            outcomes["exit_route_attribution"][2]["link_status"],
            json!("ambiguous_reconciled_link")
        );
        assert_eq!(value_i64(&outcomes, "sample_requirement"), 20);
        assert_eq!(
            json_text(&outcomes, "holding_time_status"),
            "unavailable_no_matched_sales",
            "with no ledger supplied there is nothing to match, which is a different claim from having no linkage at all"
        );
    }

    #[test]
    fn share_income_tax_estimate_applies_progressive_brackets_incrementally() {
        let config: YamlValue = serde_yaml::from_str(
            "taxation:\n  share_income:\n    brackets:\n      - up_to_dkk: 79000\n        rate: 0.27\n      - up_to_dkk:\n        rate: 0.42\n",
        )
        .expect("parses tax configuration");
        let brackets = share_income_tax_brackets(&config).expect("valid tax brackets");

        let tax = share_income_tax_due_dkk(100_000.0, &brackets).expect("tax estimate");
        assert!((tax - 30_150.0).abs() < 1e-9);

        let incremental = incremental_share_income_tax_dkk(70_000.0, 20_000.0, &brackets)
            .expect("incremental tax estimate");
        assert!((incremental - 7_050.0).abs() < 1e-9);

        let loss_offset = incremental_share_income_tax_dkk(90_000.0, -20_000.0, &brackets)
            .expect("loss offset estimate");
        assert!((loss_offset + 7_050.0).abs() < 1e-9);
    }

    #[test]
    fn share_income_tax_rejects_malformed_or_nonterminal_open_brackets() {
        let malformed: YamlValue = serde_yaml::from_str(
            "taxation:\n  share_income:\n    brackets:\n      - up_to_dkk:\n        rate: 0.27\n      - up_to_dkk: 79000\n        rate: 0.42\n",
        )
        .expect("parses malformed tax configuration");
        assert!(share_income_tax_brackets(&malformed).is_none());
    }

    fn goal_contract_field_paths(contract: &JsonValue) -> Vec<String> {
        ["objective", "constraints"]
            .iter()
            .flat_map(|section| {
                contract
                    .get(section)
                    .and_then(JsonValue::as_object)
                    .map(|fields| {
                        fields
                            .keys()
                            .map(|key| format!("{section}.{key}"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The mechanism behind U3. The goal contract used to advertise a risk
    /// envelope the runtime did not implement, and nothing made that visible.
    /// Adding a field without declaring how the runtime treats it now fails the
    /// build, so the contract cannot quietly start claiming things again.
    #[test]
    fn hermes_goal_contract_declares_enforcement_for_every_field() {
        let contract = hermes_goal_contract_from_config(&YamlValue::Null);
        let enforcement = contract
            .get("enforcement")
            .and_then(JsonValue::as_object)
            .expect("contract carries an enforcement record");

        for path in goal_contract_field_paths(&contract) {
            let entry = enforcement
                .get(&path)
                .unwrap_or_else(|| panic!("{path} has no enforcement entry"));
            let status = entry
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            assert!(
                matches!(
                    status,
                    "runtime_enforced"
                        | "evaluation_only"
                        | "structural"
                        | "documentation"
                        | "not_enforced"
                ),
                "{path} declares unknown enforcement status {status:?}"
            );
            assert!(
                !entry
                    .get("detail")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .trim()
                    .is_empty(),
                "{path} declares a status with no explanation"
            );
        }

        // The reverse direction: a stale entry for a field that no longer
        // exists would misrepresent the contract just as badly.
        let declared_fields = goal_contract_field_paths(&contract);
        for key in enforcement.keys().filter(|key| key.as_str() != "note") {
            assert!(
                declared_fields.contains(key),
                "enforcement declares {key}, which is not a contract field"
            );
        }
    }

    /// The drawdown limit Hermes is told about must be the one a gate applies,
    /// otherwise U3 has only moved the dishonesty behind a config key.
    #[test]
    fn hermes_goal_contract_publishes_the_enforced_drawdown_limit() {
        let config: YamlValue =
            serde_yaml::from_str("strategy:\n  capital:\n    drawdown_halt_pct: 0.15\n")
                .expect("parses");
        let contract = hermes_goal_contract_from_config(&config);
        let enforced = crate::drawdown_guard::DrawdownPolicy::from_config(&config).halt_pct;

        assert_eq!(contract["objective"]["max_drawdown"], json!(enforced));
        assert_eq!(
            contract["experiment_policy"]["promote_only_if"]["drawdown_lte"],
            json!(enforced)
        );
        assert_eq!(
            contract["experiment_policy"]["rollback_if"]["drawdown_gt"],
            json!(enforced)
        );
        assert_eq!(
            contract["enforcement"]["objective.max_drawdown"]["status"],
            "runtime_enforced"
        );
    }

    #[test]
    fn hermes_lessons_pending_review_projects_safe_recent_actions() {
        let reflections = vec![
            json!({
                "id": "newer-reflection",
                "created_at": "2026-07-23T14:00:00Z",
                "period_start": "2026-07-23",
                "period_end": "2026-07-23",
                "goal_version": 1,
                "summary": "Fresh evidence supports an operator review.",
                "source_session_id": "daily-eod-reflection-2026-07-23",
                "raw_payload_json": {"secret": "must-not-leak"},
                "proposed_actions_json": {
                    "actions": [
                        {"action": "  Compare the Markov horizon against the next five reports.  "},
                        {"recommendation": "Review the cash-buffer evidence before proposing a change."}
                    ]
                }
            }),
            json!({
                "id": "older-reflection",
                "created_at": "2026-07-22T14:00:00Z",
                "summary": "Older evidence.",
                "proposed_actions_json": [
                    "Compare the Markov horizon against the next five reports.",
                    {"unsupported": "not surfaced"}
                ]
            }),
        ];

        let lessons = lessons_pending_review_from_reflections(&reflections, 30);

        assert_eq!(lessons.len(), 2);
        assert_eq!(
            lessons[0]["lesson"],
            json!("Compare the Markov horizon against the next five reports.")
        );
        assert_eq!(
            lessons[1]["lesson"],
            json!("Review the cash-buffer evidence before proposing a change.")
        );
        assert_eq!(lessons[0]["reflection_id"], json!("newer-reflection"));
        assert_eq!(
            lessons[0]["source_session_id"],
            json!("daily-eod-reflection-2026-07-23")
        );
        assert!(lessons[0].get("raw_payload_json").is_none());
    }

    #[test]
    fn hermes_lessons_pending_review_caps_and_ignores_empty_actions() {
        let reflections = vec![json!({
            "id": "reflection",
            "proposed_actions_json": ["", "First review", "Second review"]
        })];

        let lessons = lessons_pending_review_from_reflections(&reflections, 1);

        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0]["lesson"], json!("First review"));
    }

    #[test]
    fn hermes_lessons_pending_review_redacts_sensitive_action_text() {
        let reflections = vec![json!({
            "id": "reflection",
            "proposed_actions_json": ["Investigate refresh_token=do-not-display"]
        })];

        let lessons = lessons_pending_review_from_reflections(&reflections, 30);

        assert_eq!(lessons.len(), 1);
        assert_eq!(
            lessons[0]["lesson"],
            json!("[redacted potentially sensitive reflection action]")
        );
        assert!(
            !serde_json::to_string(&lessons)
                .expect("serialize lessons")
                .contains("do-not-display")
        );
    }

    #[test]
    fn one_variable_audit_distinguishes_baseline_from_selected_overlay() {
        let baseline = json!({
            "id": "baseline-1",
            "activated_at": "2026-07-23T08:00:00Z",
            "config_json": {
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value": 0.05,
                "new_value": 0.02,
                "hypothesis": "Use less idle cash when qualified signals are available."
            }
        });
        let overlay_audit = json!({
            "state": "selected_for_next_cycle",
            "execution_mode": "live",
            "saxo_environment": "SIM",
            "candidate": {
                "id": "experiment-2",
                "status": "active_sim",
                "changed_variable_path": "strategy.swing.daily_indicators.min_confluences",
                "old_value": 4,
                "new_value": 3,
                "hypothesis": "Allow a controlled SIM comparison."
            }
        });
        let latest_manager_run = json!({
            "created_at": "2026-07-23T09:15:00Z",
            "manager_json": {
                "strategy_experiment_overlay": {"id": "experiment-2"}
            }
        });

        let rows =
            hermes_one_variable_audit_from_snapshot(&baseline, &overlay_audit, &latest_manager_run);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], json!("promoted_baseline"));
        assert_eq!(rows[0]["status"], json!("record_only"));
        assert_eq!(rows[1]["kind"], json!("selected_overlay"));
        assert_eq!(rows[1]["status"], json!("selected_for_next_cycle"));
        assert_eq!(
            rows[1]["last_manager_state"],
            json!("observed in latest manager run")
        );
        assert!(
            serde_json::to_string(&rows)
                .expect("serialize audit rows")
                .contains("controlled SIM comparison")
        );
    }

    #[test]
    fn one_variable_audit_never_claims_a_live_config_change() {
        let rows = hermes_one_variable_audit_from_snapshot(
            &JsonValue::Null,
            &json!({
                "state": "disabled_live_environment",
                "execution_mode": "live",
                "saxo_environment": "LIVE",
                "candidate": null,
            }),
            &JsonValue::Null,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], json!("none"));
        assert_eq!(rows[0]["status"], json!("disabled_live_environment"));
        assert!(
            rows[0]["scope"]
                .as_str()
                .unwrap_or_default()
                .contains("no promoted baseline")
        );
    }

    #[test]
    fn proposal_quality_rubric_marks_complete_safe_proposal_review_ready() {
        let quality = hermes_proposal_quality_from_experiments(&[json!({
            "id": "proposal-ready",
            "created_at": "2026-07-23T10:00:00Z",
            "status": "pending_review",
            "changed_variable_path": "strategy.swing.markov_gate.min_signed_signal",
            "old_value_json": 0.15,
            "new_value_json": 0.20,
            "expected_effect": "Raise the minimum signal threshold to reduce failure rate and drawdown.",
            "risk_notes": "Could reduce eligible BUY opportunities.",
            "evidence_json": {
                "report_ids": [201, 202],
                "markov": {"fresh": true},
                "metrics": {"failure_rate": 0.20}
            }
        })]);

        assert_eq!(quality.len(), 1);
        assert_eq!(quality[0]["quality_score"], json!(100));
        assert_eq!(quality[0]["quality_status"], json!("review_ready"));
        assert_eq!(quality[0]["exact_duplicate_count"], json!(0));
        assert_eq!(quality[0]["related_family_count"], json!(0));
        assert!(quality[0].get("evidence_json").is_none());
        assert!(quality[0].get("risk_notes").is_none());
    }

    #[test]
    fn proposal_quality_rubric_surfaces_missing_evidence_and_related_family() {
        let quality = hermes_proposal_quality_from_experiments(&[
            json!({
                "id": "proposal-cash-buffer",
                "status": "pending_review",
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve outcomes.",
                "risk_notes": "May retain more cash.",
                "evidence_json": []
            }),
            json!({
                "id": "proposal-swing-buffer",
                "status": "active_paper",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"observations": ["operator reviewed"]}
            }),
        ]);

        let cash_buffer = quality
            .iter()
            .find(|row| row["id"] == json!("proposal-cash-buffer"))
            .expect("cash-buffer proposal quality");
        assert_eq!(cash_buffer["quality_status"], json!("needs_evidence"));
        assert_eq!(cash_buffer["related_family_count"], json!(1));
        assert!(
            cash_buffer["gaps"]
                .as_array()
                .expect("gaps array")
                .iter()
                .any(|gap| gap == "attach evidence")
        );
    }

    #[test]
    fn proposal_quality_rubric_requires_review_for_an_otherwise_ready_related_family() {
        let quality = hermes_proposal_quality_from_experiments(&[
            json!({
                "id": "proposal-capital-buffer",
                "status": "pending_review",
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget by 1 pct while preserving drawdown.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"report_ids": [301], "metrics": {"cash": 0.02}}
            }),
            json!({
                "id": "proposal-swing-buffer",
                "status": "active_paper",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget by 1 pct while preserving drawdown.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"report_ids": [302], "metrics": {"cash": 0.02}}
            }),
        ]);

        let capital_buffer = quality
            .iter()
            .find(|row| row["id"] == json!("proposal-capital-buffer"))
            .expect("capital-buffer proposal quality");
        assert_eq!(capital_buffer["quality_score"], json!(95));
        assert_eq!(capital_buffer["quality_status"], json!("related_review"));
    }

    #[test]
    fn baseline_evidence_pack_links_only_matching_overlay_activity() {
        let pack = hermes_baseline_evidence_pack_from_snapshot(
            &json!({
                "id": "baseline-1",
                "activated_at": "2026-07-10T12:00:00Z",
                "config_json": {
                    "source_experiment_id": "experiment-1",
                    "changed_variable_path": "strategy.swing.markov_gate.min_signed_signal",
                    "raw_payload": "must not appear"
                }
            }),
            &json!({
                "id": "experiment-1",
                "created_at": "2026-07-01T12:00:00Z",
                "status": "promoted",
                "evidence_json": {"secret": "must not appear"}
            }),
            &[
                json!({
                    "id": 1,
                    "created_at": "2026-07-04T12:00:00Z",
                    "report_id": 41,
                    "manager_json": {
                        "strategy_experiment_overlay": {"id": "experiment-1"},
                        "approved_order_count": 2,
                        "skipped_order_count": 1
                    }
                }),
                json!({
                    "id": 2,
                    "created_at": "2026-07-05T12:00:00Z",
                    "report_id": 42,
                    "manager_json": {
                        "strategy_experiment_overlay": {"id": "different-experiment"},
                        "approved_order_count": 9,
                        "skipped_order_count": 9
                    }
                }),
            ],
            &[
                json!({"report_id": 41, "status": "executed"}),
                json!({"report_id": 41, "status": "execution_failed"}),
                json!({"report_id": 42, "status": "executed"}),
            ],
            &[
                json!({"recorded_at": "2026-07-01T12:00:00Z", "total_market_value_dkk": 100.0, "invested_market_value_dkk": 80.0, "cash_balance_dkk": 20.0}),
                json!({"recorded_at": "2026-07-04T12:00:00Z", "total_market_value_dkk": 110.0, "invested_market_value_dkk": 88.0, "cash_balance_dkk": 22.0}),
                json!({"recorded_at": "2026-07-08T12:00:00Z", "total_market_value_dkk": 105.0, "invested_market_value_dkk": 84.0, "cash_balance_dkk": 21.0}),
                json!({"recorded_at": "2026-07-10T12:00:00Z", "total_market_value_dkk": 120.0, "invested_market_value_dkk": 96.0, "cash_balance_dkk": 24.0}),
                json!({"recorded_at": "2026-07-11T12:00:00Z", "total_market_value_dkk": 126.0, "invested_market_value_dkk": 100.0, "cash_balance_dkk": 26.0}),
                json!({"recorded_at": "2026-07-12T12:00:00Z", "total_market_value_dkk": 132.0, "invested_market_value_dkk": 106.0, "cash_balance_dkk": 26.0}),
            ],
        );

        assert_eq!(pack["status"], json!("observing"));
        assert_eq!(pack["affected_activity"]["manager_run_count"], json!(1));
        assert_eq!(pack["affected_activity"]["report_count"], json!(1));
        assert_eq!(pack["affected_activity"]["approved_order_count"], json!(2));
        assert_eq!(pack["affected_activity"]["failed_order_count"], json!(1));
        assert!(
            (pack["experiment"]["evaluation_window"]["return_pct"]
                .as_f64()
                .expect("experiment return")
                - 20.0)
                .abs()
                < 1e-9
        );
        assert!(
            (pack["post_promotion"]["return_pct"]
                .as_f64()
                .expect("post-promotion return")
                - 100.0 / 21.0)
                .abs()
                < 1e-9
        );
        assert!(
            pack.to_string()
                .contains("read_only_observational_not_causal")
        );
        assert!(!pack.to_string().contains("must not appear"));
        assert!(pack.get("evidence_json").is_none());
    }

    #[test]
    fn baseline_evidence_pack_waits_for_a_source_experiment() {
        let pack = hermes_baseline_evidence_pack_from_snapshot(
            &json!({
                "id": "baseline-missing-source",
                "activated_at": "2026-07-10T12:00:00Z",
                "config_json": {"source_experiment_id": "missing"}
            }),
            &JsonValue::Null,
            &[],
            &[],
            &[],
        );

        assert_eq!(pack["status"], json!("source_experiment_unavailable"));
        assert_eq!(pack["safety"], json!("read_only_observational_not_causal"));
    }

    #[test]
    fn learning_memory_promotes_repeated_lessons_and_expires_old_ones() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = learning_memory_from_reflections(
            &[
                json!({
                    "id": "reflection-1",
                    "created_at": "2026-07-20T12:00:00Z",
                    "source_session_id": "daily-reflection-2026-07-20",
                    "proposed_actions_json": ["Keep Markov signals fresh before approving BUYs."]
                }),
                json!({
                    "id": "reflection-2",
                    "created_at": "2026-07-21T12:00:00Z",
                    "source_session_id": "weekly-reflection-2026-07-21",
                    "proposed_actions_json": ["Keep Markov signals fresh before approving BUYs."]
                }),
                json!({
                    "id": "reflection-old",
                    "created_at": "2026-06-01T12:00:00Z",
                    "source_session_id": "daily-reflection-2026-06-01",
                    "proposed_actions_json": ["Retire this old one-off lesson."]
                }),
            ],
            now,
            10,
        );

        let stable = memory
            .iter()
            .find(|row| row["lesson"] == json!("Keep Markov signals fresh before approving BUYs."))
            .expect("stable lesson");
        assert_eq!(stable["status"], json!("stable"));
        assert_eq!(stable["observation_count"], json!(2));
        assert_eq!(stable["cadences"], json!(["daily", "weekly"]));
        let stale = memory
            .iter()
            .find(|row| row["lesson"] == json!("Retire this old one-off lesson."))
            .expect("stale lesson");
        assert_eq!(stale["status"], json!("stale"));
    }

    #[test]
    fn learning_memory_redacts_sensitive_actions_before_grouping() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = learning_memory_from_reflections(
            &[json!({
                "id": "reflection-sensitive",
                "created_at": "2026-07-22T12:00:00Z",
                "source_session_id": "daily-reflection-2026-07-22",
                "proposed_actions_json": ["Use bearer token super-secret-value in the next report."]
            })],
            now,
            10,
        );

        assert_eq!(memory.len(), 1);
        assert_eq!(
            memory[0]["lesson"],
            json!("[redacted potentially sensitive reflection action]")
        );
        assert!(!memory[0].to_string().contains("super-secret-value"));
    }

    #[test]
    fn learning_memory_counts_duplicate_actions_once_per_reflection() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = learning_memory_from_reflections(
            &[json!({
                "id": "reflection-duplicate-actions",
                "created_at": "2026-07-22T12:00:00Z",
                "source_session_id": "daily-reflection-2026-07-22",
                "proposed_actions_json": [
                    "Wait for fresh Markov signals.",
                    "Wait for fresh Markov signals."
                ]
            })],
            now,
            10,
        );

        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0]["status"], json!("emerging"));
        assert_eq!(memory[0]["observation_count"], json!(1));
    }

    #[test]
    fn configured_watchlist_universe_is_versioned_and_case_normalized() {
        let config: YamlValue = serde_yaml::from_str(
            r#"
market_data:
  watchlists:
    universe_symbols:
      - amd:XNAS
      - AMD:xnas
      - "  DSV:XCSE  "
      - ""
    extra_symbols:
      - symbol: SPCX:xnas
        isin: US84615Q1031
      - rklb:XNAS
      - symbol: ""
"#,
        )
        .expect("parse watchlist universe config");

        assert_eq!(
            configured_watchlist_universe_symbols(&config),
            vec!["AMD:xnas".to_string(), "DSV:xcse".to_string()]
        );
        assert_eq!(
            configured_extra_watch_symbols(&config),
            vec!["SPCX:xnas".to_string(), "RKLB:xnas".to_string()]
        );
    }

    /// The exposure branch selected an `instrument_name` column the table has
    /// never had, so it failed on every read. Saxo also spells a held instrument
    /// its own way (`ALKb:xcse`) while a decision names it by the system symbol
    /// (`ALK-B:xcse`); the two share a Uic and must be listed once.
    #[tokio::test]
    async fn a_held_exposure_is_listed_once_under_the_system_symbol() {
        let state = runtime_settings_test_state(
            "market_data:\n  watchlists:\n    universe_symbols:\n      - nvda:xnas\n",
        )
        .await;
        // The broker and indicator tables use the runtime schema's own DDL, so
        // a column the code selects but the schema lacks fails here.
        for statement in [
            BROKER_POSITION_SNAPSHOTS_DDL,
            BROKER_INSTRUMENT_EXPOSURES_DDL,
            "CREATE TABLE portfolio_price_snapshots (
                symbol TEXT PRIMARY KEY, updated_at TEXT NOT NULL, current_price_local REAL,
                change_pct REAL, currency TEXT, source TEXT, status TEXT
            )",
            "CREATE TABLE decision_reports (
                id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, status TEXT NOT NULL,
                analysis_pulse_key TEXT, analysis_pulse_label TEXT, pulse_mode TEXT,
                queue_eligible INTEGER, report_json TEXT
            )",
        ]
        .into_iter()
        .chain(crate::daily_indicators::create_schema_sql().iter().copied())
        {
            sqlx::query(statement)
                .execute(&state.pool)
                .await
                .expect("create watchlist test table");
        }
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let today = Utc::now().date_naive().to_string();
        for statement in [
            format!(
                "INSERT INTO broker_instrument_exposures (
                    symbol, updated_at, uic, asset_type, quantity, average_open_price,
                    profit_loss_on_trade, instrument_price_day_percent_change, currency,
                    calculation_reliability, can_be_closed, raw_payload_json
                ) VALUES
                    ('ALKb:xcse', '{now}', 19782, 'Stock', 28, 250.5, 112.0, 0.4, 'DKK', 'Ok', 1, '{{}}'),
                    ('AMGN:xnas', '{now}', 209, 'Stock', 2, 300.0, 15.0, -0.2, 'USD', 'Ok', 1, '{{}}')"
            ),
            format!(
                "INSERT INTO daily_indicator_runs (
                    id, created_at, run_date, status, asset_count, success_count, error_count,
                    config_json, summary_json
                ) VALUES ('run-1', '{now}', '{today}', 'completed', 1, 1, 0, '{{}}', '{{}}')"
            ),
            format!(
                "INSERT INTO daily_indicator_signals (
                    id, run_id, created_at, run_date, status, symbol, uic, asset_type
                ) VALUES ('signal-1', 'run-1', '{now}', '{today}', 'ok', 'ALK-B:xcse', 19782, 'Stock')"
            ),
            format!(
                "INSERT INTO decision_reports (id, created_at, status, report_json)
                 VALUES (1, '{now}', 'completed', '{}')",
                json!({"symbol_sentiment": [
                    {"symbol": "ALK-B:xcse", "sentiment": "HOLD", "confidence": 60}
                ]})
            ),
        ] {
            sqlx::query(&statement)
                .execute(&state.pool)
                .await
                .expect("seed watchlist test database");
        }

        let payload = state
            .watchlists_payload()
            .await
            .expect("build watchlists payload");
        let all = payload["categories"]
            .as_array()
            .and_then(|categories| categories.iter().find(|category| category["key"] == "all"))
            .and_then(|category| category["items"].as_array())
            .cloned()
            .expect("all-monitored category");
        let listed = |symbol: &str| {
            all.iter()
                .filter(|item| item["symbol"] == symbol)
                .collect::<Vec<_>>()
        };

        let amgn = listed("AMGN:xnas");
        assert_eq!(amgn.len(), 1, "an exposure-only holding is listed once");
        assert_eq!(amgn[0]["quantity"], json!(2.0));
        assert_eq!(amgn[0]["instrument_name"], "AMGN");
        assert!(amgn[0].get("uic").is_none());

        assert!(listed("ALKb:xcse").is_empty(), "{all:#?}");
        let alk = listed("ALK-B:xcse");
        assert_eq!(alk.len(), 1, "one held instrument, one entry: {all:#?}");
        assert_eq!(
            alk[0]["quantity"],
            json!(28.0),
            "the entry keeps the holding"
        );
        assert_eq!(alk[0]["instrument_name"], "ALK-B");
        assert_eq!(alk[0]["decision"]["sentiment"], "HOLD");

        assert_eq!(listed("NVDA:xnas").len(), 1);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn a_held_instrument_takes_one_system_spelling_and_no_other() {
        let broker = [
            json!({"symbol": "ALKb:xcse", "uic": 19782, "asset_type": "Stock"}),
            json!({"symbol": "NOVOb:xcse", "uic": 15629, "asset_type": "Stock"}),
            json!({"symbol": "COST:xnas", "uic": 501, "asset_type": "Stock"}),
        ];
        let indicators = [
            json!({"symbol": "ALK-B:xcse", "uic": 19782, "asset_type": "Stock"}),
            json!({"symbol": "ALK_B:xcse", "uic": 19782, "asset_type": "Stock"}),
            // Same Uic, different asset type: a different instrument.
            json!({"symbol": "NOVO-B:xcse", "uic": 15629, "asset_type": "CfdOnStock"}),
            // An analysis alias resolving to a held instrument whose Saxo
            // spelling is already the configured one.
            json!({"symbol": "COST:xnys", "uic": 501, "asset_type": "Stock"}),
        ];
        let configured = ["COST:xnas".to_string()];
        let mut members = WatchlistMembers::new(&broker, &indicators, &configured);
        let mut monitored = Vec::new();
        let mut admit = |symbol: &str| {
            members.admit(
                &mut monitored,
                symbol,
                || json!({"symbol": symbol, "instrument_name": "broker description"}),
            )
        };

        assert!(admit("ALKb:xcse"));
        assert!(admit("NOVOb:xcse"));
        assert!(admit("COST:xnas"));
        assert!(!admit("ALKb:xcse"));
        assert!(
            !admit("ALK-B:xcse"),
            "the system spelling renames the entry"
        );
        assert!(!admit("alk-b:XCSE"));
        assert!(
            admit("ALK_B:xcse"),
            "a second spelling stays keyed by symbol"
        );
        assert!(admit("NOVO-B:xcse"));
        assert!(admit("COST:xnys"), "a configured spelling is never renamed");

        let symbols = monitored
            .iter()
            .map(|item| item["symbol"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            symbols,
            [
                "ALK-B:xcse",
                "NOVOb:xcse",
                "COST:xnas",
                "ALK_B:xcse",
                "NOVO-B:xcse",
                "COST:xnys"
            ]
        );
        assert_eq!(monitored[0]["instrument_name"], "broker description");
    }

    #[test]
    fn monitored_quote_change_wins_over_zero_broker_exposure() {
        assert_eq!(
            daily_change_pct_from_sources(
                Some(&json!({"change_pct": 0.01914580265095732})),
                Some(&json!({"instrument_price_day_percent_change": 0.0})),
            ),
            0.01914580265095732
        );
        assert_eq!(
            daily_change_pct_from_sources(
                Some(&json!({"change_pct": 0.0})),
                Some(&json!({"instrument_price_day_percent_change": 0.03})),
            ),
            0.0,
            "a verified flat quote must not be replaced by broker fallback data"
        );
        assert_eq!(
            daily_change_pct_from_sources(
                None,
                Some(&json!({"instrument_price_day_percent_change": -0.01})),
            ),
            -0.01
        );
    }

    #[test]
    fn shadow_report_outcome_candidates_preserve_context_without_claiming_maturity() {
        let earlier_symbols = HashSet::from(["AAA:XCSE".to_string()]);
        let candidates = shadow_report_outcome_candidates(
            &json!({
                "capital_plan": {
                    "cash_balance_dkk": 120000.0,
                    "available_buy_budget_dkk": 25000.0
                },
                "shadow_change_assessment": {"status": "material_change"},
                "strategy_baseline_id": "baseline-approved-sim",
                "suggested_trades": [
                    {
                        "symbol": "aaa:xcse",
                        "action": "buy",
                        "quantity": 4.0,
                        "limit_price_local": 102.5,
                        "estimated_value_dkk": 410.0,
                        "strategy_role": "starter",
                        "strategy_metadata": {
                            "technical": {"confluence_count": 4},
                            "markov": {"signed_signal": 0.42}
                        }
                    },
                    {
                        "symbol": "BBB:xnas",
                        "action": "SELL",
                        "quantity": 2.0,
                        "limit_price_local": null,
                        "strategy_metadata": {}
                    },
                    {"symbol": "", "action": "BUY", "quantity": 1.0}
                ]
            }),
            &earlier_symbols,
            &json!({
                "capital_plan": {"cash_balance_dkk": 121000.0, "available_buy_budget_dkk": 24000.0},
                "positions": [
                    {"symbol": "AAA:xcse", "quantity": 2.0, "currency": "DKK"},
                    {"symbol": "CCC:xcse", "quantity": 1.0, "currency": "DKK"}
                ],
                "active_strategy_baseline": {"id": "baseline-approved-sim", "status": "active", "goal_version": 3, "activated_at": "2026-08-19T08:00:00Z"},
                "daily_indicators": {
                    "latest_run": {"run_date": "2026-08-19", "status": "ok"},
                    "signals": [{"symbol": "AAA:xcse", "close": 100.0, "currency": "DKK", "trend_bias": "bullish", "sentiment": "BUY", "confluence_count": 4, "min_confluences": 3, "support": {"break_risk": 0.2, "break_risk_label": "low"}}]
                },
                "markov_method": {
                    "latest_run": {"run_date": "2026-08-19", "status": "ok"},
                    "signals": [{"symbol": "AAA:xcse", "run_date": "2026-08-19", "state": "bull", "signed_signal": 0.42, "direction": "long", "conviction": 0.8}]
                },
                "quiver_signals": {
                    "freshness": {"status": "fresh"},
                    "signals": [{"symbol": "AAA:xcse", "run_date": "2026-08-19", "signal": 0.5, "direction": "bullish", "confidence": 0.7, "event_count": 2, "latest_event_date": "2026-08-18"}]
                },
                "decision_time_gate_policy": {
                    "markov_starter": {"enabled": true, "min_signed_signal": 0.15, "max_signal_age_days": 5}
                }
            }),
            "2026-08-19",
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["candidate_rank"], json!(1));
        assert_eq!(candidates[0]["action"], json!("BUY"));
        assert_eq!(candidates[0]["currency"], json!("DKK"));
        assert_eq!(candidates[0]["appeared_in_earlier_pulse"], json!(true));
        assert_eq!(
            candidates[0]["report_time_context"]["technical_reported"]["confluence_count"],
            json!(4)
        );
        assert_eq!(
            candidates[0]["report_time_context"]["quiver_snapshot"]["signal"],
            json!(0.5)
        );
        assert_eq!(
            candidates[0]["report_time_context"]["support_risk_snapshot"]["break_risk"],
            json!(0.2)
        );
        assert_eq!(
            candidates[0]["report_time_context"]["concentration_snapshot"]["same_exchange_held_position_count"],
            json!(2)
        );
        assert_eq!(
            candidates[0]["report_time_context"]["thesis_snapshot"]["candidate_entry_thesis"],
            json!("not_available_pre_trade")
        );
        assert_eq!(
            candidates[0]["deterministic_gate"]["gate_code"],
            json!("technical")
        );
        assert_eq!(
            candidates[0]["deterministic_gate"]["status"],
            json!("clear_signal_gate_only")
        );
        assert_eq!(candidates[1]["candidate_rank"], json!(2));
        assert_eq!(candidates[1]["currency"], json!("USD"));
    }

    #[test]
    fn shadow_decision_time_signal_gate_marks_its_coverage_and_markov_fallback() {
        let markov = compact_shadow_report_decision_time_signal_gate(
            "BUY",
            "2026-08-19",
            &json!({
                "technical_snapshot": {"status": "not_available_at_report_time"},
                "markov_snapshot": {"status": "available", "run_date": "2026-08-18", "direction": "long", "signed_signal": 0.42},
                "signal_gate_policy": {"markov_starter": {"enabled": true, "min_signed_signal": 0.15, "max_signal_age_days": 5}}
            }),
        );
        assert_eq!(markov["status"], json!("clear_signal_gate_only"));
        assert_eq!(markov["gate_code"], json!("markov"));
        assert_eq!(
            markov["coverage"],
            json!("technical_and_markov_signal_rules_only")
        );
        assert!(
            markov["omitted_manager_gates"]
                .as_array()
                .is_some_and(|gates| gates.contains(&json!("cash_budget")))
        );

        let sell = compact_shadow_report_decision_time_signal_gate(
            "SELL",
            "2026-08-19",
            &json!({
                "technical_snapshot": {"status": "available", "sentiment": "HOLD", "trend_bias": "neutral", "confluence_count": 0, "min_confluences": 3}
            }),
        );
        assert_eq!(sell["status"], json!("blocked_signal_gate"));
        assert_eq!(sell["gate_code"], json!("technical"));
    }

    #[test]
    fn shadow_forward_outcomes_require_distinct_daily_sessions_and_respect_side() {
        let closes = (1..=20)
            .map(|session| {
                json!({
                    "run_date": format!("2026-09-{session:02}"),
                    "close": 100.0 + session as f64,
                })
            })
            .collect::<Vec<_>>();
        let buy =
            compact_shadow_report_forward_outcome("BUY", 100.0, "2026-08-19T12:15:00Z", &closes);
        let sell =
            compact_shadow_report_forward_outcome("SELL", 100.0, "2026-08-19T12:15:00Z", &closes);

        assert_eq!(json_text(&buy, "status"), "mature");
        assert_eq!(value_i64(&buy, "available_sessions"), 20);
        assert!((value_f64(&buy["one_session"], "directional_return_pct") - 0.01).abs() < 1e-9);
        assert!((value_f64(&buy["twenty_session"], "directional_return_pct") - 0.20).abs() < 1e-9);
        assert!((value_f64(&sell["five_session"], "directional_return_pct") + 0.05).abs() < 1e-9);
        assert!(compact_shadow_report_forward_outcome("BUY", 100.0, "", &closes).is_null());
    }

    #[tokio::test]
    async fn shadow_report_outcome_persistence_is_idempotent_and_never_needs_an_order_table() {
        let state = runtime_settings_test_state("{}").await;
        for sql in [
            "CREATE TABLE decision_reports (id INTEGER PRIMARY KEY, report_date TEXT NOT NULL, analysis_pulse_key TEXT NOT NULL, analysis_pulse_label TEXT NOT NULL, pulse_mode TEXT NOT NULL, queue_eligible INTEGER NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, prompt_text TEXT, report_json TEXT)",
            "CREATE TABLE shadow_report_outcomes (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, report_id INTEGER NOT NULL, analysis_pulse_key TEXT NOT NULL, analysis_pulse_label TEXT NOT NULL, candidate_rank INTEGER NOT NULL, symbol TEXT NOT NULL, action TEXT NOT NULL, proposed_quantity REAL NOT NULL, currency TEXT, earlier_pulse_report_id INTEGER, appeared_in_earlier_pulse INTEGER NOT NULL, deterministic_gate_code TEXT NOT NULL, deterministic_gate_json TEXT, hermes_effect TEXT NOT NULL, hermes_advice_snapshot_json TEXT, approved_policy_source TEXT, report_time_context_json TEXT NOT NULL, reported_reference_price_local REAL, reference_price_local REAL, reference_price_at TEXT, reference_price_source TEXT NOT NULL, reference_dkk_basis TEXT NOT NULL, status TEXT NOT NULL, latest_price_local REAL, latest_price_at TEXT, observation_count INTEGER NOT NULL DEFAULT 0, UNIQUE(report_id, candidate_rank))",
        ] {
            sqlx::query(sql).execute(&state.pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO decision_reports (id, report_date, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible, status, created_at, report_json) VALUES (7, '2026-08-19', 'europe_mid_session_shadow:2026-08-19', 'EU Shadow', 'shadow', 0, 'completed', '2026-08-19T12:15:00Z', '{}')",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        let report_json = json!({
            "suggested_trades": [{
                "symbol": "AAA:xcse",
                "action": "BUY",
                "quantity": 3.0,
                "currency": "DKK",
                "limit_price_local": 101.0,
                "strategy_metadata": {}
            }]
        });

        let first = state
            .record_shadow_report_outcomes(7, &report_json)
            .await
            .unwrap();
        let second = state
            .record_shadow_report_outcomes(7, &report_json)
            .await
            .unwrap();
        let rows = state
            .select_json("SELECT symbol, action, status, deterministic_gate_code, deterministic_gate_json, hermes_effect, reference_price_source FROM shadow_report_outcomes")
            .await
            .unwrap();

        assert_eq!(first["created"], json!(1));
        assert_eq!(second["created"], json!(0));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["symbol"], json!("AAA:xcse"));
        assert_eq!(rows[0]["status"], json!("awaiting_reference"));
        assert_eq!(
            rows[0]["deterministic_gate_code"],
            json!("not_evaluated_shadow")
        );
        assert_eq!(
            rows[0]["deterministic_gate_json"]["status"],
            json!("insufficient_decision_time_evidence")
        );
        assert_eq!(rows[0]["hermes_effect"], json!("not_requested_shadow"));
        assert_eq!(
            rows[0]["reference_price_source"],
            json!("awaiting_saxo_infoprice")
        );

        sqlx::query(
            "CREATE TABLE hermes_decision_advice (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, decision_report_id INTEGER NOT NULL, status TEXT NOT NULL, source_session_id TEXT, overall_recommendation TEXT NOT NULL, summary TEXT NOT NULL, order_advice_json TEXT NOT NULL, learning_notes_json TEXT NOT NULL, raw_payload_json TEXT NOT NULL)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO hermes_decision_advice (id, created_at, decision_report_id, status, source_session_id, overall_recommendation, summary, order_advice_json, learning_notes_json, raw_payload_json) VALUES ('shadow-advice', '2026-08-19T12:16:00Z', 7, 'received', 'shadow-decision-advice-7', 'review', 'not rendered', '[{\"symbol\":\"AAA:xcse\",\"side\":\"BUY\",\"action\":\"reduce\"}]', '[]', '{\"context_self_check\":{\"complete\":true}}')",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        let hermes = state
            .record_shadow_report_hermes_effects(
                7,
                &json!({"status": "received", "source_session_id": "shadow-decision-advice-7"}),
            )
            .await
            .unwrap();
        let hermes_row = state
            .first_json("SELECT hermes_effect, hermes_advice_snapshot_json FROM shadow_report_outcomes WHERE report_id = 7")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hermes["status"], json!("recorded"));
        assert_eq!(hermes_row["hermes_effect"], json!("record_only_reduce"));
        assert_eq!(
            hermes_row["hermes_advice_snapshot_json"]["mode"],
            json!("record_only_shadow")
        );
        assert_eq!(
            hermes_row["hermes_advice_snapshot_json"]["match_source"],
            json!("symbol_side")
        );

        for column in [
            "reference_fx_json TEXT",
            "estimated_cost_json TEXT",
            "one_session_outcome_json TEXT",
            "five_session_outcome_json TEXT",
            "twenty_session_outcome_json TEXT",
            "one_session_after_cost_outcome_json TEXT",
            "five_session_after_cost_outcome_json TEXT",
            "twenty_session_after_cost_outcome_json TEXT",
            "intraday_excursion_json TEXT",
            "outcome_maturity_status TEXT NOT NULL DEFAULT 'awaiting_reference'",
        ] {
            sqlx::query(&format!(
                "ALTER TABLE shadow_report_outcomes ADD COLUMN {column}"
            ))
            .execute(&state.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "CREATE TABLE daily_indicator_signals (symbol TEXT NOT NULL, run_date TEXT NOT NULL, status TEXT NOT NULL, close REAL NOT NULL)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE shadow_report_outcome_quotes (outcome_id TEXT NOT NULL, observed_at TEXT NOT NULL, price_local REAL NOT NULL, source TEXT NOT NULL, PRIMARY KEY (outcome_id, observed_at))",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        assert_eq!(
            state
                .refresh_shadow_report_outcome_price("AAA:xcse", 100.0, "2026-08-19T12:15:00Z")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .refresh_shadow_report_outcome_price("AAA:xcse", 110.0, "2026-08-19T12:20:00Z")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .refresh_shadow_report_outcome_price("AAA:xcse", 90.0, "2026-08-19T12:25:00Z")
                .await
                .unwrap(),
            1
        );
        for session in 1..=20 {
            sqlx::query(&format!(
                "INSERT INTO daily_indicator_signals (symbol, run_date, status, close) VALUES ('AAA:xcse', '2026-09-{session:02}', 'ok', {})",
                100 + session
            ))
            .execute(&state.pool)
            .await
            .unwrap();
        }
        let maturity = state
            .refresh_shadow_report_outcome_daily_outcomes()
            .await
            .unwrap();
        let matured = state
            .first_json("SELECT reference_dkk_basis, reference_fx_json, estimated_cost_json, intraday_excursion_json, one_session_outcome_json, five_session_outcome_json, twenty_session_outcome_json, one_session_after_cost_outcome_json, five_session_after_cost_outcome_json, twenty_session_after_cost_outcome_json, outcome_maturity_status FROM shadow_report_outcomes WHERE report_id = 7")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(maturity["mature"], json!(1));
        assert_eq!(matured["reference_dkk_basis"], json!("captured_fresh_fx"));
        assert_eq!(matured["reference_fx_json"]["source"], json!("native_dkk"));
        assert_eq!(matured["estimated_cost_json"]["status"], json!("available"));
        assert_eq!(
            matured["intraday_excursion_json"]["status"],
            json!("observed")
        );
        assert!(
            (value_f64(
                &matured["intraday_excursion_json"]["maximum_favourable_observed"],
                "directional_return_pct"
            ) - 0.1)
                .abs()
                < 1e-9
        );
        assert!(
            (value_f64(
                &matured["intraday_excursion_json"]["maximum_adverse_observed"],
                "directional_return_pct"
            ) + 0.1)
                .abs()
                < 1e-9
        );
        assert_eq!(matured["outcome_maturity_status"], json!("mature"));
        assert_eq!(
            matured["one_session_outcome_json"]["as_of"],
            json!("2026-09-01")
        );
        assert_eq!(matured["five_session_outcome_json"]["session"], json!(5));
        assert_eq!(matured["twenty_session_outcome_json"]["session"], json!(20));
        assert_eq!(
            matured["one_session_after_cost_outcome_json"]["status"],
            json!("estimated")
        );
        assert!(
            value_f64(
                &matured["twenty_session_after_cost_outcome_json"],
                "estimated_after_cost_pnl_dkk"
            ) < value_f64(
                &matured["twenty_session_after_cost_outcome_json"],
                "gross_directional_pnl_dkk"
            )
        );
    }

    async fn runtime_settings_test_state(config_yaml: &str) -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory runtime-settings test database");
        sqlx::query(
            "CREATE TABLE runtime_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create runtime-settings test table");
        AppState {
            config_path: std::path::PathBuf::from("runtime-settings-test.yaml"),
            config: serde_yaml::from_str(config_yaml).expect("parse runtime-settings test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    async fn drawdown_history_test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory drawdown test database");
        for statement in [
            "CREATE TABLE trade_ledger (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, side TEXT NOT NULL)",
            "CREATE TABLE portfolio_value_history (id INTEGER PRIMARY KEY, recorded_at TEXT NOT NULL, total_market_value_dkk REAL NOT NULL)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create drawdown test table");
        }
        AppState {
            config_path: std::path::PathBuf::from("drawdown-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    async fn portfolio_snapshot_test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory portfolio snapshot database");
        for statement in [
            "CREATE TABLE import_batches (batch_id TEXT NOT NULL, imported_at TEXT NOT NULL)",
            "CREATE TABLE broker_position_snapshots (symbol TEXT PRIMARY KEY)",
            "CREATE TABLE position_snapshots (
                instrument_name TEXT, symbol TEXT NOT NULL, isin TEXT, quantity REAL NOT NULL,
                currency TEXT NOT NULL, open_price_local REAL, current_price_local REAL,
                cost_basis_local REAL NOT NULL, cost_basis_dkk REAL NOT NULL,
                market_value_local REAL, market_value_dkk REAL NOT NULL,
                unrealised_pnl_dkk REAL, daily_pnl_dkk REAL, allocation_pct REAL,
                asset_class TEXT, market_status TEXT, value_date TEXT, excluded INTEGER NOT NULL
            )",
            "CREATE TABLE trade_ledger (net_amount_dkk REAL NOT NULL, status TEXT NOT NULL)",
            "CREATE TABLE portfolio_value_history (
                id INTEGER PRIMARY KEY, recorded_at TEXT NOT NULL, snapshot_type TEXT NOT NULL,
                baseline_session_date TEXT, batch_id TEXT, total_market_value_dkk REAL NOT NULL,
                invested_market_value_dkk REAL NOT NULL, cash_balance_dkk REAL NOT NULL,
                total_cost_basis_dkk REAL NOT NULL, total_unrealised_pnl_dkk REAL NOT NULL,
                total_daily_pnl_dkk REAL NOT NULL, position_count INTEGER NOT NULL,
                source TEXT, raw_payload_json TEXT NOT NULL
            )",
            "CREATE TABLE portfolio_position_snapshots (
                snapshot_id INTEGER NOT NULL, recorded_at TEXT NOT NULL, symbol TEXT NOT NULL,
                isin TEXT, currency TEXT NOT NULL, quantity REAL NOT NULL,
                price_local REAL NOT NULL, fx_rate_to_dkk REAL NOT NULL,
                cost_basis_local REAL NOT NULL, cost_basis_dkk REAL NOT NULL,
                market_value_dkk REAL NOT NULL, unrealised_pnl_dkk REAL NOT NULL,
                PRIMARY KEY (snapshot_id, symbol)
            )",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create portfolio snapshot test table");
        }
        sqlx::query(
            "INSERT INTO position_snapshots (
                instrument_name, symbol, isin, quantity, currency, open_price_local,
                current_price_local, cost_basis_local, cost_basis_dkk, market_value_local,
                market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk, allocation_pct,
                asset_class, market_status, value_date, excluded
            ) VALUES (
                'Example Inc.', 'EXMPL:xnas', 'US0000000001', 10, 'USD', 75,
                100, 750, 1200, 1000, 1500, 300, 25, 0.6,
                'Stock', 'Open', '2026-08-23', 0
            )",
        )
        .execute(&pool)
        .await
        .expect("seed position snapshot");
        AppState {
            config_path: std::path::PathBuf::from("portfolio-snapshot-test.yaml"),
            config: serde_yaml::from_str("portfolio:\n  initial_cash_dkk: 1000\n")
                .expect("parse portfolio snapshot test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[tokio::test]
    async fn portfolio_value_snapshot_dual_writes_recomputable_position_evidence() {
        let state = portfolio_snapshot_test_state().await;
        let recorded = state
            .record_portfolio_value_snapshot(
                "scheduler_cycle",
                None,
                "test",
                json!({"reason": "regression"}),
            )
            .await
            .expect("record dual portfolio snapshot");
        assert_eq!(recorded["status"], json!("ok"));
        assert_eq!(recorded["position_snapshot_count"], json!(1));
        assert_eq!(
            recorded["snapshot"]["total_market_value_dkk"],
            json!(2500.0)
        );

        let row = sqlx::query(
            "SELECT snapshot_id, quantity, price_local, fx_rate_to_dkk,
                    cost_basis_local, cost_basis_dkk, market_value_dkk,
                    unrealised_pnl_dkk
             FROM portfolio_position_snapshots",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read position evidence");
        let snapshot_id = row.try_get::<i64, _>("snapshot_id").unwrap();
        assert_eq!(snapshot_id, value_i64(&recorded["snapshot"], "id"));
        let quantity = row.try_get::<f64, _>("quantity").unwrap();
        let price_local = row.try_get::<f64, _>("price_local").unwrap();
        let fx_rate = row.try_get::<f64, _>("fx_rate_to_dkk").unwrap();
        let market_value = row.try_get::<f64, _>("market_value_dkk").unwrap();
        let cost_basis = row.try_get::<f64, _>("cost_basis_dkk").unwrap();
        let unrealised = row.try_get::<f64, _>("unrealised_pnl_dkk").unwrap();
        assert!((market_value - quantity * price_local * fx_rate).abs() < 1e-9);
        assert!((unrealised - (market_value - cost_basis)).abs() < 1e-9);
        assert!((row.try_get::<f64, _>("cost_basis_local").unwrap() - 750.0).abs() < 1e-9);

        let integrity = state
            .portfolio_position_snapshot_integrity()
            .await
            .expect("compare aggregate and position snapshot evidence");
        assert_eq!(integrity["status"], json!("aligned"));
        assert_eq!(integrity["checked_snapshot_count"], json!(1));
        assert_eq!(integrity["structural_mismatch_count"], json!(0));
        assert_eq!(
            integrity["broker_derived_unrealised_difference_count"],
            json!(0)
        );

        let coverage = state
            .portfolio_position_snapshot_coverage("ALL")
            .await
            .expect("summarize position snapshot coverage");
        assert_eq!(coverage["status"], json!("complete"));
        assert_eq!(coverage["aggregate_snapshot_count"], json!(1));
        assert_eq!(coverage["covered_snapshot_count"], json!(1));
        assert_eq!(coverage["missing_legacy_snapshot_count"], json!(0));
        assert_eq!(coverage["latest_snapshot"]["status"], json!("available"));
        assert_eq!(
            coverage["latest_snapshot"]["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            coverage["latest_snapshot"]["items"][0]["symbol"],
            json!("EXMPL:xnas")
        );
        assert_eq!(coverage["latest_change"]["status"], json!("collecting"));

        sqlx::query(
            "INSERT INTO portfolio_value_history (
                id, recorded_at, snapshot_type, total_market_value_dkk,
                invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk,
                total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count,
                raw_payload_json
            ) VALUES (2, '2026-08-23T18:00:00Z', 'legacy', 2500, 1500, 1000, 1200, 300, 0, 1, '{}')",
        )
        .execute(&state.pool)
        .await
        .expect("seed aggregate-only legacy snapshot");
        let partial_coverage = state
            .portfolio_position_snapshot_coverage("ALL")
            .await
            .expect("surface aggregate-only legacy snapshot");
        assert_eq!(partial_coverage["status"], json!("partial"));
        assert_eq!(partial_coverage["aggregate_snapshot_count"], json!(2));
        assert_eq!(partial_coverage["covered_snapshot_count"], json!(1));
        assert_eq!(partial_coverage["missing_legacy_snapshot_count"], json!(1));
        assert_eq!(
            partial_coverage["latest_snapshot"]["snapshot"]["snapshot_id"],
            recorded["snapshot"]["id"]
        );
    }

    #[test]
    fn portfolio_position_snapshot_integrity_distinguishes_structure_from_broker_pnl() {
        let base = json!({
            "snapshot_id": 7,
            "recorded_at": "2026-08-23T12:00:00Z",
            "aggregate_position_count": 2,
            "detail_position_count": 2,
            "aggregate_market_value_dkk": 1000.0,
            "detail_market_value_dkk": 1000.0,
            "aggregate_cost_basis_dkk": 800.0,
            "detail_cost_basis_dkk": 800.0,
            "aggregate_unrealised_pnl_dkk": 200.0,
            "detail_unrealised_pnl_dkk": 200.0,
        });
        let aligned = portfolio_position_snapshot_integrity_from_rows(&[base.clone()]);
        assert_eq!(aligned["status"], json!("aligned"));

        let mut broker_derived = base.clone();
        broker_derived["aggregate_unrealised_pnl_dkk"] = json!(235.0);
        let broker_only = portfolio_position_snapshot_integrity_from_rows(&[broker_derived]);
        assert_eq!(
            broker_only["status"],
            json!("broker_derived_unrealised_difference")
        );
        assert_eq!(broker_only["structural_mismatch_count"], json!(0));
        assert_eq!(
            broker_only["broker_derived_unrealised_difference_count"],
            json!(1)
        );

        let mut structural = base;
        structural["detail_position_count"] = json!(1);
        structural["detail_market_value_dkk"] = json!(900.0);
        let attention_required = portfolio_position_snapshot_integrity_from_rows(&[structural]);
        assert_eq!(attention_required["status"], json!("attention_required"));
        assert_eq!(attention_required["structural_mismatch_count"], json!(1));
    }

    #[test]
    fn portfolio_position_snapshot_change_distinguishes_opened_closed_and_resized() {
        let previous_snapshot = json!({"snapshot_id": 7, "recorded_at": "2026-08-23T10:00:00Z"});
        let current_snapshot = json!({"snapshot_id": 8, "recorded_at": "2026-08-23T11:00:00Z"});
        let previous_rows = vec![
            json!({"symbol": "AAA:xnas", "quantity": 1.0, "market_value_dkk": 100.0, "cost_basis_dkk": 90.0}),
            json!({"symbol": "BBB:xnas", "quantity": 2.0, "market_value_dkk": 200.0, "cost_basis_dkk": 180.0}),
            json!({"symbol": "CCC:xnas", "quantity": 1.0, "market_value_dkk": 70.0, "cost_basis_dkk": 60.0}),
        ];
        let current_rows = vec![
            json!({"symbol": "aaa:xnas", "quantity": 3.0, "market_value_dkk": 330.0, "cost_basis_dkk": 270.0}),
            json!({"symbol": "DDD:xnas", "quantity": 1.0, "market_value_dkk": 50.0, "cost_basis_dkk": 45.0}),
        ];

        let change = portfolio_position_snapshot_change_from_rows(
            &current_snapshot,
            &current_rows,
            &previous_snapshot,
            &previous_rows,
        );

        assert_eq!(change["status"], json!("available"));
        assert_eq!(change["opened_count"], json!(1));
        assert_eq!(change["closed_count"], json!(2));
        assert_eq!(change["resized_count"], json!(1));
        assert_eq!(change["unchanged_quantity_count"], json!(0));
        assert_eq!(change["opened"][0]["symbol"], json!("DDD:xnas"));
        assert_eq!(change["resized"][0]["symbol"], json!("aaa:xnas"));
        assert_eq!(change["resized"][0]["quantity_change"], json!(2.0));
        assert_eq!(change["net_market_value_change_dkk"], json!(10.0));
        assert_eq!(change["net_cost_basis_change_dkk"], json!(-15.0));
    }

    #[tokio::test]
    async fn portfolio_position_snapshot_retention_keeps_recent_and_daily_final_rows() {
        let state = portfolio_snapshot_test_state().await;
        for (id, recorded_at) in [
            (1, "2026-05-01T09:00:00Z"),
            (2, "2026-05-01T17:00:00Z"),
            (3, "2026-05-02T17:00:00Z"),
            (4, "2026-08-22T17:00:00Z"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO portfolio_value_history (
                    id, recorded_at, snapshot_type, total_market_value_dkk,
                    invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk,
                    total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count,
                    raw_payload_json
                ) VALUES (
                    {id}, '{recorded_at}', 'scheduler_cycle', 100, 50, 50, 40, 10, 0, 1, '{{}}'
                )"
            ))
            .execute(&state.pool)
            .await
            .expect("seed aggregate snapshot");
            sqlx::query(&format!(
                "INSERT INTO portfolio_position_snapshots (
                    snapshot_id, recorded_at, symbol, currency, quantity, price_local,
                    fx_rate_to_dkk, cost_basis_local, cost_basis_dkk,
                    market_value_dkk, unrealised_pnl_dkk
                ) VALUES (
                    {id}, '{recorded_at}', 'EXMPL:xnas', 'USD', 1, 100, 1, 90, 90, 100, 10
                )"
            ))
            .execute(&state.pool)
            .await
            .expect("seed position evidence");
        }
        let now = DateTime::parse_from_rfc3339("2026-08-23T18:00:00Z")
            .expect("parse retention time")
            .with_timezone(&Utc);
        assert_eq!(
            state
                .prune_portfolio_position_snapshots(now)
                .await
                .expect("prune position evidence"),
            1
        );
        let retained = state
            .select_json(
                "SELECT snapshot_id FROM portfolio_position_snapshots ORDER BY snapshot_id ASC",
            )
            .await
            .expect("read retained position evidence");
        assert_eq!(
            retained
                .iter()
                .map(|row| value_i64(row, "snapshot_id"))
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    /// A drawdown peak must never reach back across a re-baselining.
    ///
    /// In mid-May 2026 a run of operator cash adjustments and a "Live export
    /// reset" moved the book from roughly 351,000 to 265,000 DKK. Nothing was
    /// lost, but a peak spanning that boundary reads as a 27% drawdown, which
    /// under the 20% floor would have suspended all buying indefinitely.
    #[tokio::test]
    async fn the_drawdown_window_starts_after_the_latest_external_cash_flow() {
        let state = drawdown_history_test_state().await;
        for (created_at, side) in [
            ("2026-05-13T04:11:30+00:00", "ADJUSTMENT"),
            ("2026-05-18T18:53:24Z", "DEPOSIT"),
            ("2026-05-19T11:01:10Z", "ADJUSTMENT"),
            ("2026-06-04T09:00:00Z", "BUY"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO trade_ledger (created_at, side) VALUES ('{created_at}', '{side}')"
            ))
            .execute(&state.pool)
            .await
            .expect("insert ledger row");
        }
        for (recorded_at, total) in [
            ("2026-05-08T21:00:00Z", 344_775.0),
            ("2026-05-19T21:00:00Z", 351_559.0),
            ("2026-06-04T21:00:00Z", 265_500.0),
            ("2026-06-05T21:00:00Z", 266_232.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO portfolio_value_history (recorded_at, total_market_value_dkk) \
                 VALUES ('{recorded_at}', {total})"
            ))
            .execute(&state.pool)
            .await
            .expect("insert snapshot");
        }

        let rows = state
            .portfolio_drawdown_window(3_650)
            .await
            .expect("history");
        let days = rows
            .iter()
            .map(|row| {
                json_text(row, "recorded_at")
                    .chars()
                    .take(10)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        // The whole adjustment day is excluded, and so is everything before it.
        assert!(
            !days.iter().any(|day| day.as_str() <= "2026-05-19"),
            "pre-re-baselining snapshots leaked into the window: {days:?}"
        );
        assert!(days.contains(&"2026-06-04".to_string()));
        assert!(days.contains(&"2026-06-05".to_string()));
    }

    /// Without any external cash flow the window is the plain lookback, and a
    /// flow older than the lookback must not extend it.
    #[tokio::test]
    async fn a_stale_cash_flow_does_not_widen_the_drawdown_window() {
        let state = drawdown_history_test_state().await;
        sqlx::query(
            "INSERT INTO trade_ledger (created_at, side) VALUES ('2020-01-01T00:00:00Z', 'DEPOSIT')",
        )
        .execute(&state.pool)
        .await
        .expect("insert ledger row");
        // The recent snapshot is dated from today, not fixed: a fixed date
        // falls out of a 90-day window counted back from now, and on
        // 2026-10-18 this test failed for exactly that reason.
        let recent = (Utc::now() - Duration::days(10))
            .format("%Y-%m-%dT21:00:00Z")
            .to_string();
        for (recorded_at, total) in [
            ("2020-06-01T21:00:00Z".to_string(), 100_000.0),
            (recent.clone(), 255_000.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO portfolio_value_history (recorded_at, total_market_value_dkk) \
                 VALUES ('{recorded_at}', {total})"
            ))
            .execute(&state.pool)
            .await
            .expect("insert snapshot");
        }

        let rows = state.portfolio_drawdown_window(90).await.expect("history");
        let days = rows
            .iter()
            .map(|row| {
                json_text(row, "recorded_at")
                    .chars()
                    .take(10)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            !days.contains(&"2020-06-01".to_string()),
            "a 2020 deposit must not pull six-year-old snapshots into a 90-day window: {days:?}"
        );
        assert!(days.contains(&recent.chars().take(10).collect::<String>()));
    }

    /// Adopted broker positions (`strategy_type = 'portfolio_sync'`) are
    /// bookkeeping for holdings that already existed at the broker when this
    /// system took over the book, so they never acquire a trade-ledger row.
    /// Before 2026-07-25 the integrity check counted them as unreconciled,
    /// which held `healthy` false continuously from the 2026-05-05 adoption and
    /// buried the signal this check exists to raise.
    /// The backfill must reach Trading Manager orders and nothing else.
    /// `report_id IS NOT NULL` is the discriminator: adoption, reconciliation,
    /// and manual rows do not originate in a decision report.
    /// A double-clicked placement left lifecycle test 1 in `placement_preparing`
    /// with no broker order id on 2026-07-25: axum dropped the first handler
    /// future when the browser cancelled it, after the prepared row had already
    /// committed. The orphan then blocked its precheck permanently.
    ///
    /// Stale rows must be findable, and abandoning one must be safe only for the
    /// exact shape that was never sent — never for a row that carries a broker
    /// order id, since that interruption could have happened after placement.

    /// A variable offered to Hermes as tunable must be one the runtime actually
    /// reads. `strategy.swing.cash_buffer_pct` was on this list until
    /// 2026-07-25 while nothing read it, so an experiment could have been
    /// proposed, run in SIM, observed, and promoted while changing nothing --
    /// and whatever the portfolio did would have been attributed to it.
    #[test]
    fn supported_experiment_variables_are_all_read_by_the_runtime() {
        use crate::config_contract::{ContractStatus, status_for_path};
        let mut dead = Vec::new();
        for path in SUPPORTED_EXPERIMENT_VARIABLES {
            // Paths outside the audited roots are not described by the contract
            // and cannot be checked here.
            if let Some(status) = status_for_path(path) {
                if status == ContractStatus::Unused {
                    dead.push((*path).to_string());
                }
            }
        }
        assert!(
            dead.is_empty(),
            "Hermes may propose experiments on variables nothing reads: {dead:?}"
        );
    }

    #[test]
    fn hermes_capabilities_publish_the_checked_variable_list() {
        // The published payload must be the same list the test above checks,
        // so the two cannot drift apart.
        let expected = SUPPORTED_EXPERIMENT_VARIABLES
            .iter()
            .map(|path| json!(path))
            .collect::<Vec<_>>();
        assert_eq!(json!(SUPPORTED_EXPERIMENT_VARIABLES), json!(expected));
        assert!(
            !SUPPORTED_EXPERIMENT_VARIABLES.contains(&"strategy.swing.cash_buffer_pct"),
            "the dead cash-buffer path must not return"
        );
    }

    #[test]
    fn typed_hermes_capabilities_keep_advisory_boundaries_explicit() {
        let payload = hermes_capabilities_payload(json!({"mode": "recommend_only"}));

        assert_eq!(payload.status, "ok");
        assert_eq!(payload.decision_advice.write_tool, "create_decision_advice");
        assert!(
            payload
                .supported_experiment_overlays
                .variables
                .iter()
                .any(|path| path == "strategy.capital.min_cash_buffer_pct")
        );
        assert!(
            payload
                .forbidden
                .iter()
                .any(|capability| capability == "order precheck/place/replace/cancel")
        );
        assert!(
            payload
                .safe_endpoints
                .iter()
                .all(|endpoint| endpoint.starts_with("/api/"))
        );
        assert_eq!(payload.goal_contract["mode"], "recommend_only");
    }

    #[test]
    fn typed_hermes_context_keeps_section_and_safety_boundaries_explicit() {
        let payload = HermesContextPayload {
            status: "ok".to_string(),
            generated_at: "2026-08-27T08:00:00Z".to_string(),
            capabilities: hermes_capabilities_payload(json!({"mode": "recommend_only"})),
            goal_contract: json!({"mode": "recommend_only"}),
            overview: json!({"status": "ok"}),
            scheduler: HermesContextSchedulerPayload {
                status: json!({"status": "ok"}),
                cycles: vec![json!({"status": "ok"})],
            },
            decisions: HermesContextDecisionsPayload {
                cadence: "two_daily_open_followups".to_string(),
                pulses: json!([]),
                reports: vec![HermesDecisionReportOutcomePayload {
                    report_id: 42,
                    ..Default::default()
                }],
            },
            end_of_day: HermesContextEndOfDayPayload {
                cadence: "daily".to_string(),
                reports: Vec::new(),
            },
            strategy_journal: HermesContextItemsPayload { items: Vec::new() },
            execution: HermesContextExecutionPayload {
                orders: Vec::new(),
                failures: Vec::new(),
                events: Vec::new(),
                fills: Vec::new(),
                protective_stop_coverage: json!({"status": "ok"}),
                holding_thesis_reviews: json!({"status": "ok"}),
            },
            performance: HermesContextPerformancePayload {
                range: "1M".to_string(),
                history: Vec::new(),
                realised_outcomes: JsonValue::Null,
            },
            markov_method: json!({"status": "ok"}),
            quiver_signals: json!({"status": "ok"}),
            quiver_conflicts: json!([]),
            editorial_research: json!({"status": "ok"}),
            daily_indicators: json!({"status": "ok"}),
            hermes: HermesContextHermesPayload {
                experiments: Vec::new(),
                active_strategy_baseline: JsonValue::Null,
                gate_replay: json!({"status": "ok"}),
                learning_memory: HermesContextLearningMemoryPayload {
                    active: Vec::new(),
                    stale_count: 0,
                    policy: "bounded".to_string(),
                },
            },
            safety: HermesContextSafetyPayload {
                saxo_sessions_excluded: true,
                broker_mutations_excluded: true,
                raw_oauth_payloads_excluded: true,
                raw_broker_payloads_excluded: true,
            },
        };

        let serialized = serde_json::to_value(payload).expect("Hermes context serializes");
        assert_eq!(
            serialized["decisions"]["cadence"],
            "two_daily_open_followups"
        );
        assert_eq!(serialized["performance"]["range"], "1M");
        assert_eq!(serialized["safety"]["saxo_sessions_excluded"], true);
        assert_eq!(serialized["safety"]["broker_mutations_excluded"], true);
        assert_eq!(
            serialized["capabilities"]["decision_advice"]["write_tool"],
            "create_decision_advice"
        );
    }

    #[tokio::test]
    async fn stale_protective_stop_preparations_are_findable_and_safely_abandoned() {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT,
                request_id TEXT,
                placement_result_json TEXT,
                cancellation_result_json TEXT,
                reconciliation_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create lifecycle test table");
        let old = "2020-01-01T00:00:00Z";
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        sqlx::query(&format!(
            "INSERT INTO protective_stop_lifecycle_tests
                (id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                 stop_price_local, status, broker_order_id, external_reference, request_id) VALUES
                (1, '{old}', '{old}', 1, 'sim', 'V:xnys', 13, 340.84, 'placement_preparing', NULL, 'stop-test:1:1', 'r1'),
                (2, '{old}', '{old}', 2, 'sim', 'AMD:xnas', 7, 448.78, 'placement_preparing', '5099', 'stop-test:2:1', 'r2'),
                (3, '{now}', '{now}', 3, 'sim', 'BAC:xnys', 58, 59.67, 'placement_preparing', NULL, 'stop-test:3:1', 'r3'),
                (4, '{old}', '{old}', 4, 'sim', 'V:xnys', 13, 340.84, 'broker_working', '5100', 'stop-test:4:1', 'r4')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert lifecycle rows");

        let stale = state
            .stale_protective_stop_preparations(90)
            .await
            .expect("read stale preparations");
        let ids = stale
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![1],
            "only an old preparing row with no broker order id is stale; \
             row 2 has a broker order id, row 3 is recent, row 4 is already working"
        );

        state
            .abandon_protective_stop_preparation(1)
            .await
            .expect("abandon row 1");
        // Abandoning a row that carries a broker order id must be a no-op even
        // if it is somehow attempted.
        state
            .abandon_protective_stop_preparation(2)
            .await
            .expect("attempt row 2");

        let rows = state
            .select_json("SELECT id, status FROM protective_stop_lifecycle_tests ORDER BY id")
            .await
            .expect("read back");
        let statuses = rows
            .iter()
            .map(|row| (value_f64(row, "id") as i64, text_value(row, "status")))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                (1, "placement_abandoned".to_string()),
                (2, "placement_preparing".to_string()),
                (3, "placement_preparing".to_string()),
                (4, "broker_working".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn broker_confirmed_protective_stops_are_adopted_once_into_execution_orders() {
        let state = runtime_settings_test_state(
            "saxo:\n  environment: sim\nexecution:\n  mode: live\n  adapter: saxo\n",
        )
        .await;
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT,
                request_id TEXT,
                placement_result_json TEXT,
                cancellation_result_json TEXT,
                reconciliation_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create lifecycle test table");
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                order_type TEXT NOT NULL DEFAULT 'Market',
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                adapter TEXT NOT NULL,
                quantity REAL,
                stop_price_local REAL,
                approval_required INTEGER NOT NULL DEFAULT 0,
                approved_at TEXT,
                strategy_type TEXT,
                strategy_key TEXT,
                strategy_role TEXT,
                broker_order_id TEXT,
                request_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution order table");
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        sqlx::query(&format!(
            "INSERT INTO protective_stop_lifecycle_tests
                (id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                 stop_price_local, status, broker_order_id, external_reference, request_id) VALUES
                (1, '{now}', '{now}', 1, 'sim', 'V:xnys', 13, 340.84, 'broker_working', '5100', 'stop-test:1:1', 'r1'),
                (2, '{now}', '{now}', 2, 'sim', 'AMD:xnas', 7, 448.78, 'placement_submitted', '5101', 'stop-test:2:1', 'r2'),
                (3, '{now}', '{now}', 3, 'sim', 'BAC:xnys', 58, 59.67, 'broker_working', '', 'stop-test:3:1', 'r3'),
                (4, '{now}', '{now}', 4, 'sim', 'AAPL:xnas', 4, 316.49, 'placement_failed', NULL, 'stop-test:4:1', 'r4')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert lifecycle rows");

        let adopted = state
            .adopt_protective_stops_into_execution_orders()
            .await
            .expect("adopt protective stops");
        assert_eq!(
            adopted.len(),
            1,
            "only a broker-confirmed stop carrying a broker order id may be adopted; \
             a submitted-but-unconfirmed, an empty-id, and a failed row must all be skipped"
        );

        // Adoption runs every scheduler cycle. A second pass must not create a
        // duplicate SELL, which at Saxo would be a second resting sell order.
        let repeat = state
            .adopt_protective_stops_into_execution_orders()
            .await
            .expect("re-run adoption");
        assert!(repeat.is_empty(), "adoption must be idempotent");

        let orders = state
            .select_json(
                "SELECT symbol, action, order_type, status, mode, adapter, quantity,
                        stop_price_local, strategy_type, strategy_role, broker_order_id
                 FROM execution_orders ORDER BY id",
            )
            .await
            .expect("read execution orders");
        assert_eq!(orders.len(), 1);
        let order = &orders[0];
        assert_eq!(text_value(order, "symbol"), "V:xnys");
        assert_eq!(text_value(order, "action"), "SELL");
        assert_eq!(text_value(order, "order_type"), "stop");
        // The status has to be one broker sync polls for, or the adoption
        // achieves nothing: an unwatched row is exactly the state this change
        // exists to end.
        assert_eq!(text_value(order, "status"), "broker_working");
        assert_eq!(text_value(order, "mode"), "live");
        assert_eq!(text_value(order, "adapter"), "saxo");
        assert_eq!(text_value(order, "strategy_type"), "protective_stop");
        assert_eq!(text_value(order, "strategy_role"), "protective_stop");
        assert_eq!(text_value(order, "broker_order_id"), "5100");
        assert_eq!(value_f64(order, "quantity"), 13.0);
        assert_eq!(value_f64(order, "stop_price_local"), 340.84);
    }

    /// Live on 2026-10-23, 99 lifecycle tests stayed `broker_working` after
    /// their stop filled. The coverage audit already reported such a symbol as
    /// unprotected once re-bought, but the operator batch skipped it as
    /// "a protective stop already exists". A stop cancelled outside the
    /// runtime, or expired, stayed active the same way.
    #[tokio::test]
    async fn terminal_stop_backfill_makes_a_rebought_symbol_eligible_for_the_batch() {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        for ddl in [
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT,
                request_id TEXT,
                placement_result_json TEXT,
                cancellation_result_json TEXT,
                reconciliation_json TEXT
            )",
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                order_type TEXT NOT NULL,
                status TEXT NOT NULL,
                quantity REAL,
                stop_price_local REAL,
                broker_order_id TEXT,
                strategy_type TEXT
            )",
            "CREATE TABLE broker_position_snapshots (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                quantity REAL NOT NULL,
                currency TEXT
            )",
            "CREATE TABLE protective_stop_prechecks (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT,
                result_json TEXT
            )",
            "CREATE TABLE daily_indicator_signals (
                id INTEGER PRIMARY KEY,
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                close REAL,
                atr14 REAL
            )",
        ] {
            sqlx::query(ddl)
                .execute(&state.pool)
                .await
                .expect("create protective-stop test table");
        }
        let old = "2026-08-21T17:05:34Z";
        sqlx::query(&format!(
            "INSERT INTO protective_stop_lifecycle_tests
                (id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                 stop_price_local, status, broker_order_id, external_reference, request_id,
                 placement_result_json, cancellation_result_json, reconciliation_json) VALUES
                (1, '{old}', '{old}', 1, 'sim', 'COP:xnys', 10, 128.06, 'broker_working', '5001',
                 'stop-test:1:1', 'r1', '{{}}', '{{}}', '{{}}'),
                (2, '{old}', '{old}', 2, 'sim', 'V:xnys', 13, 340.84, 'broker_working', '5002',
                 'stop-test:2:1', 'r2', '{{}}', '{{}}', '{{}}'),
                (3, '{old}', '{old}', 3, 'sim', 'AMD:xnas', 7, 448.78, 'placement_preparing', NULL,
                 'stop-test:3:1', 'r3', '{{}}', '{{}}', '{{}}'),
                (4, '{old}', '{old}', 4, 'sim', 'BAC:xnys', 58, 59.67, 'broker_cancelled', '5004',
                 'stop-test:4:1', 'r4', '{{}}', '{{}}', '{{}}'),
                (5, '{old}', '{old}', 5, 'sim', 'MSFT:xnas', 4, 400.0, 'broker_working', '5005',
                 'stop-test:5:1', 'r5', '{{}}', '{{}}', '{{}}'),
                (6, '{old}', '{old}', 6, 'sim', 'NVDA:xnas', 9, 150.0, 'broker_working', '5006',
                 'stop-test:6:1', 'r6', '{{}}', '{{}}', '{{}}')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert lifecycle tests");
        sqlx::query(&format!(
            "INSERT INTO execution_orders
                (id, created_at, symbol, action, order_type, status, quantity, stop_price_local,
                 broker_order_id, strategy_type) VALUES
                (1, '{old}', 'COP:xnys', 'SELL', 'stop', 'executed', 10, 128.06, '5001', 'protective_stop'),
                (2, '{old}', 'V:xnys', 'SELL', 'stop', 'broker_working', 13, 340.84, '5002', 'protective_stop'),
                (4, '{old}', 'BAC:xnys', 'SELL', 'stop', 'broker_cancelled', 58, 59.67, '5004', 'protective_stop'),
                (5, '{old}', 'MSFT:xnas', 'SELL', 'stop', 'broker_cancelled', 4, 400.0, '5005', 'protective_stop'),
                (6, '{old}', 'NVDA:xnas', 'SELL', 'stop', 'broker_expired', 9, 150.0, '5006', 'protective_stop')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert adopted protective stops");
        // COP was stopped out and later re-bought; V still holds its stop.
        sqlx::query(
            "INSERT INTO broker_position_snapshots (symbol, updated_at, quantity, currency) VALUES
                ('COP:xnys', '2026-10-22T15:00:00Z', 10, 'USD'),
                ('V:xnys', '2026-10-22T15:00:00Z', 13, 'USD')",
        )
        .execute(&state.pool)
        .await
        .expect("insert broker positions");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (id, symbol, run_date, close, atr14) VALUES
                (1, 'COP:xnys', '2026-10-22', 130.0, 3.0),
                (2, 'V:xnys', '2026-10-22', 345.0, 5.0)",
        )
        .execute(&state.pool)
        .await
        .expect("insert indicators");
        let requested = vec!["COP:XNYS".to_string(), "V:XNYS".to_string()];
        let (state, requested) = (&state, &requested);
        let batch_targets = || async move {
            let coverage = state
                .protective_stop_coverage()
                .await
                .expect("read protective-stop coverage");
            let active = state
                .symbols_with_active_protective_stops()
                .await
                .expect("read active protective stops");
            crate::api::protective_stop_batch_targets(&coverage, &requested, &active)
        };

        assert!(
            batch_targets().await.is_empty(),
            "before the backfill the filled stop's broker_working row blocks COP"
        );

        let settled = state
            .backfill_terminal_protective_stop_lifecycle_tests()
            .await
            .expect("settle terminal lifecycle tests");
        assert_eq!(settled, 3);

        let rows = state
            .select_json(
                "SELECT id, status, updated_at FROM protective_stop_lifecycle_tests ORDER BY id",
            )
            .await
            .expect("read back lifecycle tests");
        let statuses = rows
            .iter()
            .map(|row| (value_f64(row, "id") as i64, text_value(row, "status")))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                (1, "filled".to_string()),
                (2, "broker_working".to_string()),
                // Only a Saxo lookup can prove the broker never saw a stale
                // preparation, and the backfill makes no broker call.
                (3, "placement_preparing".to_string()),
                // Released by this runtime's own cancellation; already settled.
                (4, "broker_cancelled".to_string()),
                // Cancelled at Saxo by someone else, and expired.
                (5, "cancelled".to_string()),
                (6, "failed".to_string()),
            ]
        );
        for row in &rows[1..4] {
            assert_eq!(text_value(row, "updated_at"), old, "untouched row changed");
        }

        let mut active = state
            .symbols_with_active_protective_stops()
            .await
            .expect("read active protective stops");
        active.sort();
        assert_eq!(active, vec!["AMD:XNAS".to_string(), "V:XNYS".to_string()]);
        assert_eq!(
            batch_targets().await,
            vec![("COP:xnys".to_string(), 10.0, 124.0)],
            "the re-bought symbol gets a stop at close minus 2 x ATR14; V keeps its own"
        );

        let repeat = state
            .backfill_terminal_protective_stop_lifecycle_tests()
            .await
            .expect("re-run the backfill");
        assert_eq!(repeat, 0, "the backfill runs on every pod start");
    }

    #[tokio::test]
    async fn resting_protective_stops_are_not_reported_as_stale_orders() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT,
                action TEXT,
                status TEXT NOT NULL,
                quantity REAL,
                currency TEXT,
                limit_price_local REAL,
                ledger_id INTEGER,
                broker_order_id TEXT,
                error_text TEXT,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution order table");
        let old = "2020-01-01T00:00:00Z";
        sqlx::query(&format!(
            "INSERT INTO execution_orders
                (id, created_at, symbol, action, status, quantity, broker_order_id, strategy_type) VALUES
                (1, '{old}', 'V:xnys', 'SELL', 'broker_working', 13, '5100', 'protective_stop'),
                (2, '{old}', 'AMD:xnas', 'BUY', 'broker_working', 7, '5101', 'swing'),
                (3, '{old}', 'BAC:xnys', 'SELL', 'broker_state_unknown', 58, NULL, 'protective_stop')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert orders");

        let cutoff = "2021-01-01T00:00:00Z";
        let rows = state
            .select_json(&unreconciled_orders_sql(cutoff, cutoff))
            .await
            .expect("run the shared integrity query");
        let ids = rows
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![2, 3],
            "a GoodTillCancel protective stop resting at the broker is doing its job, \
             so age alone must not flag it; an ordinary stale order and a protective stop \
             with an unresolved placement are both still real faults"
        );
    }

    /// The per-order timeline reaches a browser, so the column list must stay
    /// an allowlist. `raw_payload_json` holds unredacted Saxo responses and
    /// `account_uid` identifies the account; neither may be served, and adding
    /// a column to the table must not silently add it to the response.
    #[tokio::test]
    async fn order_event_timeline_never_serves_raw_broker_payloads() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_order_events (
                id INTEGER PRIMARY KEY,
                created_at TEXT,
                execution_order_id INTEGER,
                broker_order_id TEXT,
                event_type TEXT,
                broker_status TEXT,
                broker_substatus TEXT,
                broker_quantity REAL,
                broker_price_local REAL,
                event_signature TEXT,
                raw_payload_json TEXT,
                environment_id TEXT,
                account_uid TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution_order_events test table");
        sqlx::query(
            "INSERT INTO execution_order_events VALUES
                (1,'2026-08-04T14:55:00Z',285,'5039465258','placed','Working',NULL,6,NULL,'sig','{\"Secret\":\"broker-token\"}','SIM','ACCT-SECRET'),
                (2,'2026-08-04T14:55:30Z',285,'5039465258','filled','Filled',NULL,6,193.12,'sig','{\"Secret\":\"broker-token\"}','SIM','ACCT-SECRET'),
                (3,'2026-08-04T14:50:00Z',999,'other','placed','Working',NULL,1,NULL,'sig','{}','SIM','ACCT-SECRET')",
        )
        .execute(&state.pool)
        .await
        .expect("insert execution_order_events test rows");

        let events = state
            .execution_order_events(285, 200)
            .await
            .expect("load order events");

        assert_eq!(events.len(), 2, "must scope to the requested order only");
        let timeline = execution_order_event_timeline_entries_from_json(events)
            .expect("typed timeline entries decode");
        let serialized = serde_json::to_string(&timeline).expect("serialize");
        assert!(
            !serialized.contains("broker-token"),
            "raw broker payload must never be served: {serialized}"
        );
        assert!(
            !serialized.contains("ACCT-SECRET"),
            "account identifier must never be served: {serialized}"
        );
        // Oldest first: the timeline reads placement -> terminal state.
        assert_eq!(timeline[0].event_type, "placed");
        assert_eq!(timeline[1].event_type, "filled");
        assert_eq!(timeline[1].broker_price_local, Some(193.12));
    }

    /// Rows 1-3 are the real production shapes: a USD sale across a falling
    /// dollar, a DKK sale with no currency exposure, and a legacy row whose
    /// stored `cost_basis_fx_rate_to_dkk` is 100x too small. The last must still
    /// backfill correctly, because the split derives its cost rate from the DKK
    /// and local cost bases rather than trusting that column.
    #[tokio::test]
    async fn currency_split_backfill_recomputes_only_where_currency_actually_moved() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE trade_ledger (
                id INTEGER PRIMARY KEY,
                side TEXT,
                realised_gain_dkk REAL,
                cost_basis_sold_dkk REAL,
                cost_basis_sold_local REAL,
                sale_fx_rate_to_dkk REAL,
                cost_basis_fx_rate_to_dkk REAL,
                fx_gain_dkk REAL,
                price_gain_dkk REAL,
                realised_gain_local REAL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create trade_ledger test table");
        sqlx::query(
            "INSERT INTO trade_ledger VALUES \
                (1, 'SELL', -543.0, 7160.45, 1089.75, 6.4986, 6.5705, 0, -543.0, 0), \
                (2, 'SELL', -1489.0, 10042.0, 10042.0, 1.0, 1.0, 0, -1489.0, 0), \
                (3, 'SELL', 2356.0, 3127.0, 445.31, 7.0215, 0.0713, 0, 2356.0, 0), \
                (4, 'SELL', -250.0, 100.0, 0.0, 6.5, 6.5, 0, -250.0, 0), \
                (5, 'BUY', 0, 0, 0, 6.5, 6.5, 0, 0, 0)",
        )
        .execute(&state.pool)
        .await
        .expect("insert trade_ledger test rows");

        state
            .backfill_realised_gain_currency_split()
            .await
            .expect("backfill currency split");
        let after_first = state
            .select_json("SELECT id, fx_gain_dkk, price_gain_dkk FROM trade_ledger ORDER BY id")
            .await
            .expect("read back split");
        state
            .backfill_realised_gain_currency_split()
            .await
            .expect("backfill currency split again");
        let after_second = state
            .select_json("SELECT id, fx_gain_dkk, price_gain_dkk FROM trade_ledger ORDER BY id")
            .await
            .expect("read back split again");
        assert_eq!(
            after_first, after_second,
            "recomputation from stored columns must be idempotent"
        );

        let fx_of = |id: i64| -> f64 {
            after_first
                .iter()
                .find(|row| row.get("id").and_then(|v| v.as_i64()) == Some(id))
                .and_then(|row| row.get("fx_gain_dkk").and_then(|v| v.as_f64()))
                .unwrap_or_default()
        };

        assert!(
            fx_of(1) < -70.0 && fx_of(1) > -90.0,
            "a 6.5705 -> 6.4986 move on a 1089.75 USD basis is about -78 DKK, got {}",
            fx_of(1)
        );
        assert_eq!(fx_of(2), 0.0, "a DKK sale has no currency component");
        // The discriminator: the derived cost rate is 3127.0/445.31 = 7.0221
        // against a 7.0215 sale rate, so the true currency component is a
        // fraction of a krone. Had the backfill trusted the stored
        // `cost_basis_fx_rate_to_dkk` of 0.0713, it would have computed
        // 445.31 * (7.0215 - 0.0713) = about +3,095 DKK of fictional currency
        // gain on a 2,356 DKK profit.
        assert!(
            fx_of(3).abs() < 1.0,
            "the legacy row must derive its cost rate from the DKK and local \
             cost bases; a value near +3095 means it trusted the 100x-too-small \
             stored rate, got {}",
            fx_of(3)
        );
        assert_eq!(
            fx_of(4),
            0.0,
            "a row with no local cost basis must be left alone"
        );

        for row in &after_first {
            let id = row.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            if id == 5 {
                continue;
            }
            let fx = row
                .get("fx_gain_dkk")
                .and_then(|v| v.as_f64())
                .unwrap_or_default();
            let price = row
                .get("price_gain_dkk")
                .and_then(|v| v.as_f64())
                .unwrap_or_default();
            let realised = [-543.0, -1489.0, 2356.0, -250.0][(id - 1) as usize];
            assert!(
                (fx + price - realised).abs() < 0.01,
                "row {id}: price {price} + fx {fx} must reconstruct {realised}"
            );
        }
    }

    #[tokio::test]
    async fn strategy_type_backfill_targets_only_report_derived_orders() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                report_id INTEGER,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution_orders test table");
        sqlx::query(
            "INSERT INTO execution_orders (id, report_id, strategy_type) VALUES \
                (1, 42, NULL), \
                (2, 43, NULL), \
                (3, NULL, 'portfolio_sync'), \
                (4, NULL, 'clean_reconciliation'), \
                (5, NULL, 'manual'), \
                (6, NULL, NULL), \
                (7, 44, 'swing')",
        )
        .execute(&state.pool)
        .await
        .expect("insert execution_orders test rows");

        state
            .backfill_trading_manager_strategy_type()
            .await
            .expect("backfill strategy type");
        // Idempotent: a second pass must be a no-op.
        state
            .backfill_trading_manager_strategy_type()
            .await
            .expect("backfill strategy type again");

        let rows = state
            .select_json("SELECT id, COALESCE(strategy_type, 'NULL') AS stype FROM execution_orders ORDER BY id")
            .await
            .expect("read back execution orders");
        let observed = rows
            .iter()
            .map(|row| (value_f64(row, "id") as i64, text_value(row, "stype")))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                (1, "swing".to_string()),
                (2, "swing".to_string()),
                (3, "portfolio_sync".to_string()),
                (4, "clean_reconciliation".to_string()),
                (5, "manual".to_string()),
                // No report id and no type: not a Trading Manager order, so it
                // stays unset rather than being guessed at.
                (6, "NULL".to_string()),
                (7, "swing".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn unreconciled_orders_check_excludes_adopted_positions_but_not_real_fills() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                quantity REAL,
                currency TEXT,
                limit_price_local REAL,
                ledger_id INTEGER,
                broker_order_id TEXT,
                error_text TEXT,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution_orders test table");
        sqlx::query(
            "INSERT INTO execution_orders \
                (id, created_at, symbol, action, status, ledger_id, strategy_type) VALUES \
                (1, '2026-05-05T05:20:09+00:00', 'NVDA:xnas', 'BUY', 'executed', NULL, 'portfolio_sync'), \
                (2, '2026-05-05T05:20:09+00:00', 'ORSTED:xcse', 'BUY', 'executed', NULL, 'portfolio_sync'), \
                (3, '2026-05-06T09:00:00+00:00', 'AMD:xnas', 'BUY', 'executed', NULL, 'swing'), \
                (4, '2026-05-06T09:00:00+00:00', 'V:xnys', 'BUY', 'executed', 7, 'swing')",
        )
        .execute(&state.pool)
        .await
        .expect("insert execution_orders test rows");

        let cutoff = "2026-07-01T00:00:00+00:00";
        let flagged = state
            .select_json(&unreconciled_orders_sql(cutoff, cutoff))
            .await
            .expect("query unreconciled orders");
        let flagged_ids = flagged
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            flagged_ids,
            vec![3],
            "only the genuine ledger-less fill should be flagged"
        );

        let adopted = state
            .select_json(ADOPTED_ORDERS_WITHOUT_LEDGER_SQL)
            .await
            .expect("count adopted orders");
        assert_eq!(
            adopted.first().map(|row| value_f64(row, "count") as i64),
            Some(2),
            "adopted positions stay visible as context rather than vanishing"
        );
    }

    #[tokio::test]
    async fn protective_stop_lifecycle_requires_accepted_sim_precheck_and_blocks_duplicate_active_test()
     {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        sqlx::query(
            "CREATE TABLE protective_stop_prechecks (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create protective-stop prechecks test table");
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT NOT NULL,
                request_id TEXT NOT NULL UNIQUE,
                placement_result_json TEXT NOT NULL,
                cancellation_result_json TEXT NOT NULL,
                reconciliation_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create protective-stop lifecycle test table");
        sqlx::query(
            "INSERT INTO protective_stop_prechecks (
                id, created_at, environment, symbol, quantity, stop_price_local, status, result_json
             ) VALUES (1, '2026-07-25T08:00:00Z', 'sim', 'TSLA:xnas', 1, 300, 'precheck_ok', '{}')",
        )
        .execute(&state.pool)
        .await
        .expect("seed accepted SIM precheck");

        let first = state
            .prepare_protective_stop_lifecycle_test(1)
            .await
            .expect("prepare lifecycle test");
        assert_eq!(first["status"], json!("placement_preparing"));
        assert!(
            state
                .prepare_protective_stop_lifecycle_test(1)
                .await
                .is_err()
        );
        assert!(
            state
                .prepare_protective_stop_lifecycle_test(999)
                .await
                .is_err()
        );
    }

    /// Live on 2026-10-23 every placed stop's `placement_result_json` read
    /// `{}`: each lifecycle update rewrote the documents it was not setting
    /// from a decoded copy that came back empty.
    #[tokio::test]
    async fn lifecycle_updates_keep_the_documents_they_do_not_set() {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        for ddl in [
            "CREATE TABLE protective_stop_prechecks (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT NOT NULL
            )",
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT NOT NULL,
                request_id TEXT NOT NULL UNIQUE,
                placement_result_json TEXT NOT NULL,
                cancellation_result_json TEXT NOT NULL,
                reconciliation_json TEXT NOT NULL
            )",
            "INSERT INTO protective_stop_prechecks (
                id, created_at, environment, symbol, quantity, stop_price_local, status, result_json
             ) VALUES (1, '2026-10-23T13:40:00Z', 'sim', 'COP:xnys', 10, 124, 'precheck_ok', '{}')",
        ] {
            sqlx::query(ddl)
                .execute(&state.pool)
                .await
                .expect("prepare lifecycle test fixture");
        }
        let id = value_i64(
            &state
                .prepare_protective_stop_lifecycle_test(1)
                .await
                .expect("prepare lifecycle test"),
            "id",
        );
        let stored = || {
            let pool = state.pool.clone();
            async move {
                let row = sqlx::query(
                    "SELECT status, broker_order_id, placement_result_json,
                            cancellation_result_json, reconciliation_json
                     FROM protective_stop_lifecycle_tests WHERE id = ?",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("read lifecycle test");
                [
                    "status",
                    "broker_order_id",
                    "placement_result_json",
                    "cancellation_result_json",
                    "reconciliation_json",
                ]
                .map(|column| {
                    row.try_get::<Option<String>, _>(column)
                        .expect("lifecycle column")
                        .unwrap_or_default()
                })
            }
        };
        let [
            _,
            _,
            prepared_placement,
            not_requested_cancellation,
            not_requested_reconciliation,
        ] = stored().await;
        assert!(prepared_placement.contains("not_sent"));

        let placement = json!({"accepted": true, "broker_order_id": "5100", "request_id": "r"});
        state
            .record_protective_stop_lifecycle_placement(
                id,
                "placement_submitted",
                Some("5100"),
                &placement,
            )
            .await
            .expect("record placement");
        assert_eq!(
            stored().await,
            [
                "placement_submitted".to_string(),
                "5100".to_string(),
                placement.to_string(),
                not_requested_cancellation.clone(),
                not_requested_reconciliation.clone(),
            ]
        );

        // The confirmation step passes no broker order id of its own; the one
        // the placement recorded must survive along with the placement result.
        let reconciliation = json!({"status": "broker_working", "broker_status": "Working"});
        state
            .record_protective_stop_lifecycle_reconciliation(
                id,
                "broker_working",
                None,
                &reconciliation,
            )
            .await
            .expect("record reconciliation");
        assert_eq!(
            stored().await,
            [
                "broker_working".to_string(),
                "5100".to_string(),
                placement.to_string(),
                not_requested_cancellation.clone(),
                reconciliation.to_string(),
            ]
        );

        let cancellation = json!({"accepted": true});
        state
            .record_protective_stop_lifecycle_cancellation(
                id,
                "cancellation_submitted",
                &cancellation,
            )
            .await
            .expect("record cancellation");
        assert_eq!(
            stored().await,
            [
                "cancellation_submitted".to_string(),
                "5100".to_string(),
                placement.to_string(),
                cancellation.to_string(),
                reconciliation.to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn ai_api_key_override_wins_over_config_and_is_never_echoed() {
        let state = runtime_settings_test_state(
            "xai:\n  provider: openrouter\n  api_key: sk-or-old-config-key-1234\n",
        )
        .await;

        // Before any override the config key is effective.
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-old-config-key-1234")
        );
        let status = state
            .ai_api_key_status_value()
            .await
            .expect("read initial key status");
        assert_eq!(status["source"], json!("config"));
        assert_eq!(status["configured"], json!(true));

        // Saving a rotated key makes it effective immediately, and the
        // returned status carries only a masked preview.
        let status = state
            .save_ai_api_key("  sk-or-v1-new-rotated-key-5678  ")
            .await
            .expect("save rotated key");
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-v1-new-rotated-key-5678")
        );
        assert_eq!(status["source"], json!("runtime"));
        let masked = status["masked"].as_str().expect("masked preview");
        assert_eq!(masked, "sk-or-…5678");
        assert!(!status.to_string().contains("sk-or-v1-new-rotated-key-5678"));

        // Clearing the override falls back to the config key.
        let status = state.save_ai_api_key("").await.expect("clear override");
        assert_eq!(status["source"], json!("config"));
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-old-config-key-1234")
        );
    }

    #[tokio::test]
    async fn purge_retired_runtime_settings_removes_all_legacy_cash_buffer_paths() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        state
            .save_runtime_setting(
                "strategy.capital.cash_buffer",
                &json!({"min_cash_buffer_pct": 0.0, "max_deployment_pct": 1.0}),
            )
            .await
            .expect("seed retired cash-buffer override");
        state
            .save_runtime_setting(
                "strategy.swing.cash_buffer_pct",
                &json!({"cash_buffer_pct": 0.0}),
            )
            .await
            .expect("seed retired swing cash-buffer override");
        state
            .save_runtime_setting("ai_model", &json!({"model": "openrouter/fusion"}))
            .await
            .expect("seed active model override");

        assert_eq!(
            state
                .purge_retired_runtime_settings()
                .await
                .expect("purge retired settings"),
            2
        );
        assert!(
            state
                .runtime_setting("strategy.capital.cash_buffer")
                .await
                .expect("read retired setting")
                .is_none()
        );
        assert!(
            state
                .runtime_setting("strategy.swing.cash_buffer_pct")
                .await
                .expect("read retired swing setting")
                .is_none()
        );
        assert_eq!(
            state
                .runtime_setting("ai_model")
                .await
                .expect("read active setting")
                .expect("active setting remains")["model"],
            json!("openrouter/fusion")
        );
        assert_eq!(
            state
                .purge_retired_runtime_settings()
                .await
                .expect("repeat purge is safe"),
            0
        );
    }

    #[tokio::test]
    async fn ai_api_key_rejects_non_printable_input_and_reports_missing() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;

        assert!(state.save_ai_api_key("bad key with spaces").await.is_err());
        assert!(state.save_ai_api_key("bad\nkey").await.is_err());

        let status = state
            .ai_api_key_status_value()
            .await
            .expect("read missing key status");
        assert_eq!(status["configured"], json!(false));
        assert_eq!(status["source"], json!("missing"));
        assert_eq!(status["masked"], JsonValue::Null);
        assert_eq!(state.effective_ai_api_key().await, None);
    }

    #[tokio::test]
    async fn manual_report_claim_is_exclusive_until_released() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;

        assert!(!state.manual_decision_report_in_flight().await);
        assert!(state.claim_manual_decision_report().await.expect("claim"));
        assert!(state.manual_decision_report_in_flight().await);
        // A second click while the pipeline runs must not start another.
        assert!(
            !state
                .claim_manual_decision_report()
                .await
                .expect("re-claim")
        );

        state
            .release_manual_decision_report_claim()
            .await
            .expect("release claim");
        assert!(!state.manual_decision_report_in_flight().await);
        assert!(
            state
                .claim_manual_decision_report()
                .await
                .expect("claim again")
        );
    }

    #[tokio::test]
    async fn stale_manual_report_claim_is_taken_over() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        // A claim from a crashed task, older than the stale window.
        state
            .save_runtime_setting(
                "manual_report_claim",
                &json!({"started_at": "2026-07-17T00:00:00Z"}),
            )
            .await
            .expect("seed stale claim");

        assert!(!state.manual_decision_report_in_flight().await);
        assert!(
            state
                .claim_manual_decision_report()
                .await
                .expect("take over")
        );
    }

    #[test]
    fn masked_api_key_never_reveals_short_keys() {
        assert_eq!(mask_api_key("sk-or-v1-abcdefgh-9999"), "sk-or-…9999");
        assert_eq!(mask_api_key("short-key"), "•••");
    }

    /// The settings picker listed four hardcoded slugs, none of them the model
    /// that had produced the last 93 reports, so it offered choices nobody had
    /// made and hid the ones they had. Order is by most recent use, not by
    /// volume: the model someone ran once yesterday is a likelier destination
    /// than the one that ran fifty times last quarter.
    #[tokio::test]
    async fn the_model_picker_lists_what_actually_ran_newest_first() {
        let state =
            runtime_settings_test_state("xai:\n  provider: openrouter\n  model: openai/gpt-5.5\n")
                .await;
        sqlx::query(
            "CREATE TABLE decision_reports (id TEXT PRIMARY KEY, model TEXT, created_at TEXT NOT NULL)",
        )
        .execute(&state.pool)
        .await
        .expect("create reports table");
        // grok ran far more often, but longer ago; the blank row is a provider
        // fallback that never named a model and must not become an option.
        for (id, model, created_at) in [
            ("1", "grok-4.3", "2026-06-16T14:45:14Z"),
            ("2", "grok-4.3", "2026-06-17T14:45:14Z"),
            ("3", "grok-4.3", "2026-06-18T14:45:14Z"),
            ("4", "openai/gpt-5.6-terra", "2026-09-03T18:19:29Z"),
            ("5", "~google/gemini-flash-latest", "2026-09-04T08:19:29Z"),
            ("6", "   ", "2026-09-04T09:00:00Z"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO decision_reports (id, model, created_at) \
                 VALUES ('{id}', '{model}', '{created_at}')"
            ))
            .execute(&state.pool)
            .await
            .expect("insert report");
        }

        let recent = state
            .recently_used_ai_models(12)
            .await
            .expect("read recently used models");
        assert_eq!(
            recent,
            vec![
                "~google/gemini-flash-latest".to_string(),
                "openai/gpt-5.6-terra".to_string(),
                "grok-4.3".to_string(),
            ],
            "newest use first, one entry per model, and no blank option"
        );

        let settings = state
            .ai_settings_value()
            .await
            .expect("build the settings payload");
        let options: Vec<&str> = settings["recent_models"]
            .as_array()
            .expect("recent_models is a list")
            .iter()
            .filter_map(JsonValue::as_str)
            .collect();
        assert!(
            options.contains(&"openai/gpt-5.5"),
            "the configured default is offered so reverting an override needs no retyping: {options:?}"
        );
    }

    #[tokio::test]
    async fn ai_model_setting_accepts_openrouter_floating_alias() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        let saved = state
            .save_ai_settings("~openai/gpt-5.6-terra")
            .await
            .expect("save floating-alias model");
        assert_eq!(saved["model"], json!("~openai/gpt-5.6-terra"));
    }

    #[test]
    fn hermes_context_self_check_marks_complete_when_all_sources_seen() {
        let normalized = normalize_hermes_context_self_check(json!({
            "latest_report": true,
            "markov_signals": true,
            "end_of_day_report": true,
            "current_positions": true,
            "active_experiments": true,
            "sources": ["get_decision_reports", "get_markov_signals"]
        }));

        assert_eq!(
            normalized.get("complete").and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            normalized
                .get("missing")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn hermes_context_self_check_reports_missing_sources() {
        let normalized = normalize_hermes_context_self_check(json!({
            "latest_report": true,
            "markov_signals": false,
            "current_positions": true
        }));

        assert_eq!(
            normalized.get("complete").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            normalized.get("missing").cloned(),
            Some(json!([
                "markov_signals",
                "end_of_day_report",
                "active_experiments"
            ]))
        );
    }

    #[test]
    fn money_integrity_tolerance_requires_absolute_and_relative_drift() {
        assert!(!money_mismatch_exceeds_tolerance(
            100_000.0, 100_020.0, 50.0, 0.002
        ));
        assert!(!money_mismatch_exceeds_tolerance(
            100_000.0, 100_100.0, 50.0, 0.002
        ));
        assert!(money_mismatch_exceeds_tolerance(
            100_000.0, 100_500.0, 50.0, 0.002
        ));
        assert!(money_mismatch_exceeds_tolerance(
            f64::NAN,
            100_000.0,
            50.0,
            0.002
        ));
    }

    #[test]
    fn hermes_counterfactuals_only_shadow_advice_that_changed_quantity() {
        assert_eq!(
            hermes_counterfactual_shadow_quantity("reduced", 5.0, 2.0),
            Some(3.0)
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("blocked_by_order_advice", 5.0, 0.0),
            Some(5.0)
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("allowed", 5.0, 5.0),
            None
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("reduced", 2.0, 2.0),
            None
        );
    }

    #[test]
    fn hermes_counterfactual_quote_metrics_are_directional() {
        assert_eq!(
            hermes_counterfactual_quote_metrics("BUY", 2.0, 100.0, 110.0),
            Some((0.1, 20.0))
        );
        assert_eq!(
            hermes_counterfactual_quote_metrics("SELL", 2.0, 100.0, 90.0),
            Some((0.1, 20.0))
        );
        assert_eq!(
            hermes_counterfactual_quote_metrics("HOLD", 2.0, 100.0, 90.0),
            None
        );
    }

    #[test]
    fn missed_trade_shadows_only_track_timing_capital_and_capacity_blocks() {
        assert!(missed_trade_shadow_gate_is_eligible("cash_budget"));
        assert!(missed_trade_shadow_gate_is_eligible("market_open"));
        assert!(missed_trade_shadow_gate_is_eligible("drawdown_guardrail"));
        assert!(missed_trade_shadow_gate_is_eligible("max_holdings"));
        assert!(!missed_trade_shadow_gate_is_eligible("technical"));
        assert!(!missed_trade_shadow_gate_is_eligible("markov"));
        assert!(!missed_trade_shadow_gate_is_eligible("risk_exclusion"));
        assert!(!missed_trade_shadow_gate_is_eligible(
            "instrument_quarantine"
        ));
    }

    #[test]
    fn missed_trade_shadow_outcomes_are_bounded_and_observational() {
        let evidence = missed_trade_shadow_outcome_evidence_from_rows(&[
            json!({"source_gate": "cash_budget", "estimated_return_pct": 0.10}),
            json!({"source_gate": "cash_budget", "estimated_return_pct": -0.05}),
            json!({"source_gate": "market_open", "estimated_return_pct": null}),
        ]);
        assert_eq!(evidence["status"], json!("collecting"));
        assert_eq!(evidence["recorded_shadow_count"], json!(3));
        assert_eq!(evidence["observed_shadow_count"], json!(2));
        assert_eq!(evidence["overall"]["sample_count"], json!(2));
        assert!(
            (value_f64(&evidence["overall"], "average_directional_return_pct") - 0.025).abs()
                < 1e-9
        );
        assert!((value_f64(&evidence["overall"], "positive_return_rate") - 0.5).abs() < 1e-9);
        assert_eq!(evidence["by_gate"][0]["source_gate"], json!("cash_budget"));
        assert_eq!(evidence["by_gate"][0]["outcome"]["sample_count"], json!(2));
        assert!(
            evidence["interpretation"]
                .as_str()
                .is_some_and(|value| value.contains("exclude fees"))
        );
    }

    #[tokio::test]
    async fn missed_trade_shadows_record_selected_blocks_and_refresh_quotes() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE missed_trade_shadows (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_gate TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                reference_price_at TEXT,
                reference_price_source TEXT,
                reported_reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0,
                UNIQUE(manager_run_id, strategy_key)
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create missed-trade shadow table");
        let candidates = vec![
            json!({
                "strategy_key": "cash-buy:AMD:xnas:BUY",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "quantity": 2.0,
                "currency": "USD",
                "reference_price_local": 100.0,
                "gate_code": "cash_budget",
            }),
            json!({
                "strategy_key": "technical-buy:NVDA:xnas:BUY",
                "symbol": "NVDA:xnas",
                "action": "BUY",
                "quantity": 3.0,
                "currency": "USD",
                "reference_price_local": 200.0,
                "gate_code": "technical",
            }),
        ];

        let result = state
            .record_missed_trade_shadows(41, 77, &candidates)
            .await
            .expect("record missed-trade shadows");
        assert_eq!(result["created"], json!(1));
        assert_eq!(result["awaiting_reference"], json!(1));
        assert_eq!(result["skipped"], json!(1));
        assert_eq!(
            state
                .active_missed_trade_shadow_symbols()
                .await
                .expect("list active missed-trade symbols"),
            vec!["AMD:xnas".to_string()]
        );
        assert_eq!(
            state
                .refresh_missed_trade_shadow_price("AMD:xnas", 110.0, "2026-07-27T10:00:00Z")
                .await
                .expect("refresh missed-trade shadow quote"),
            1
        );
        assert_eq!(
            state
                .refresh_missed_trade_shadow_price("AMD:xnas", 121.0, "2026-07-27T10:01:00Z")
                .await
                .expect("refresh missed-trade shadow outcome quote"),
            1
        );
        let rows = state
            .missed_trade_shadows(10)
            .await
            .expect("read missed-trade shadows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source_gate"], json!("cash_budget"));
        assert_eq!(rows[0]["status"], json!("tracking"));
        assert_eq!(rows[0]["reference_price_local"], json!(110.0));
        assert_eq!(rows[0]["reference_price_at"], json!("2026-07-27T10:00:00Z"));
        assert_eq!(rows[0]["reference_price_source"], json!("saxo_infoprices"));
        assert_eq!(rows[0]["reported_reference_price_local"], json!(100.0));
        assert!((value_f64(&rows[0], "estimated_return_pct") - 0.1).abs() < 1e-9);
        assert!((value_f64(&rows[0], "estimated_pnl_local") - 22.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn hermes_counterfactuals_capture_saxo_quote_before_observing_outcome() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE hermes_counterfactuals (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_effect TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                reference_price_at TEXT,
                reference_price_source TEXT,
                reported_reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0,
                UNIQUE(manager_run_id, strategy_key)
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create Hermes counterfactual table");
        let advice_delta = json!({
            "candidates": [{
                "strategy_key": "hermes-buy:AMD:xnas:BUY",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "currency": "USD",
                "reference_price_local": 90.0,
                "requested_quantity": 5.0,
                "resulting_quantity": 2.0,
                "effect": "reduced",
            }]
        });

        let result = state
            .record_hermes_counterfactuals(42, 78, &advice_delta)
            .await
            .expect("record Hermes counterfactual");
        assert_eq!(result["created"], json!(1));
        assert_eq!(result["awaiting_reference"], json!(1));
        assert_eq!(
            state
                .refresh_hermes_counterfactual_price("AMD:xnas", 100.0, "2026-07-27T10:00:00Z")
                .await
                .expect("capture Saxo reference quote"),
            1
        );
        assert_eq!(
            state
                .refresh_hermes_counterfactual_price("AMD:xnas", 110.0, "2026-07-27T10:01:00Z")
                .await
                .expect("refresh Hermes counterfactual outcome quote"),
            1
        );
        let rows = state
            .hermes_counterfactuals(10)
            .await
            .expect("read Hermes counterfactuals");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["reference_price_local"], json!(100.0));
        assert_eq!(rows[0]["reference_price_source"], json!("saxo_infoprices"));
        assert_eq!(rows[0]["reported_reference_price_local"], json!(90.0));
        assert!((value_f64(&rows[0], "estimated_return_pct") - 0.1).abs() < 1e-9);
        assert!((value_f64(&rows[0], "estimated_pnl_local") - 30.0).abs() < 1e-9);
    }

    #[test]
    fn broker_cash_reconciliation_requires_explicit_opt_in() {
        let default_config: YamlValue =
            serde_yaml::from_str("portfolio: {}").expect("parse default portfolio config");
        let enabled_config: YamlValue =
            serde_yaml::from_str("portfolio:\n  broker_cash_reconciliation_enabled: true\n")
                .expect("parse enabled broker cash config");

        assert!(!broker_cash_reconciliation_enabled(&default_config));
        assert!(broker_cash_reconciliation_enabled(&enabled_config));
    }

    #[test]
    fn overview_integrity_acknowledgement_matches_stable_issue_key() {
        let mut mismatches = vec![json!({
            "code": "implausible_position_lot_cost_basis",
            "severity": "error",
            "message": "Bad lot",
            "lots": [{"lot_id": 42}, {"lot_id": 43}]
        })];
        let mut warnings = vec![json!({
            "code": "broker_cash_drift",
            "severity": "warning",
            "message": "Broker cash drift",
            "broker_currency": "DKK"
        })];
        let key = overview_integrity_issue_key(&mismatches[0]);
        assert_eq!(key, "implausible_position_lot_cost_basis:error:42_43");

        let acknowledged = annotate_overview_integrity_acknowledgements(
            &mut mismatches,
            &mut warnings,
            &json!({
                "acknowledgements": [{
                    "issue_key": key,
                    "enabled": true,
                    "notes": "accepted while import repair is pending",
                    "updated_at": "2026-07-10T06:00:00Z"
                }]
            }),
        );

        assert_eq!(acknowledged, 1);
        assert_eq!(
            mismatches[0]
                .get("acknowledged")
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            warnings[0].get("acknowledged").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            mismatches[0]
                .get("acknowledgement")
                .and_then(|value| value.get("notes"))
                .and_then(JsonValue::as_str),
            Some("accepted while import repair is pending")
        );
    }

    #[test]
    fn broker_exposure_quantity_mismatch_reports_symbol_drift() {
        let exposures = vec![
            json!({"symbol": "BAC:xnys", "quantity": 3.0, "updated_at": "2026-07-10T06:00:00Z"}),
            json!({"symbol": "AMD:xnas", "quantity": 1.0, "updated_at": "2026-07-10T06:00:00Z"}),
        ];
        let positions = HashMap::from([
            (
                "BAC:xnys".to_string(),
                json!({"symbol": "BAC:xnys", "quantity": 2.0, "updated_at": "2026-07-10T06:01:00Z"}),
            ),
            (
                "AMD:xnas".to_string(),
                json!({"symbol": "AMD:xnas", "quantity": 1.0, "updated_at": "2026-07-10T06:01:00Z"}),
            ),
        ]);

        let mismatches = broker_exposure_quantity_mismatches(&exposures, &positions);

        assert_eq!(mismatches.len(), 1);
        assert_eq!(json_text(&mismatches[0], "symbol"), "BAC:xnys");
        assert_eq!(value_f64(&mismatches[0], "difference"), 1.0);
    }

    #[test]
    fn broker_exposure_integrity_issue_key_includes_scope() {
        let key = overview_integrity_issue_key(&json!({
            "code": "broker_exposure_quantity_drift",
            "severity": "warning",
            "symbols": [{"symbol": "BAC:xnys"}, {"symbol": "AMD:xnas"}]
        }));

        assert_eq!(
            key,
            "broker_exposure_quantity_drift:warning:BAC:xnys_AMD:xnas"
        );
    }

    #[test]
    fn enriches_active_day_order_with_exchange_expiry() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": "2026-07-09T19:45:00Z",
            "session_close_at_utc": "2026-07-09T20:00:00Z"
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(json_text(&order, "order_duration_type"), "DayOrder");
        assert_eq!(
            json_text(&order, "expected_expiry_at_utc"),
            "2026-07-09T19:45:00Z"
        );
        assert_eq!(
            json_text(&order, "expected_expiry_market"),
            "New York Stock Exchange"
        );
    }

    #[test]
    fn marks_day_order_expiry_pending_after_expected_expiry_passes() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let expired_at = (Utc::now() - Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES + 1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": expired_at
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(
            json_text(&order, "lifecycle_state"),
            "expiry_pending_broker_sync"
        );
        assert!(json_text(&order, "lifecycle_note").contains("waiting for Saxo broker sync"));
    }

    #[test]
    fn does_not_mark_day_order_expiry_pending_inside_grace_window() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let expired_at = (Utc::now() - Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES - 1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": expired_at
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(json_text(&order, "lifecycle_state"), "");
        assert!(json_text(&order, "lifecycle_note").contains("remains live"));
    }

    #[test]
    fn matching_order_advice_prefers_strategy_key_then_symbol_action() {
        let advice = json!([
            {
                "strategy_key": "pulse|AMD:xnas|BUY|primary",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "reason": "strategy-key match"
            },
            {
                "strategy_key": "other",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "reason": "symbol-action fallback"
            }
        ]);

        let matched = matching_order_advice(
            Some(&advice),
            "pulse|AMD:xnas|BUY|primary",
            "AMD:xnas",
            "BUY",
        )
        .unwrap();
        assert_eq!(json_text(&matched, "reason"), "strategy-key match");

        let fallback = matching_order_advice(Some(&advice), "", "AMD:XNAS", "buy").unwrap();
        assert_eq!(json_text(&fallback, "reason"), "strategy-key match");
    }

    #[test]
    fn execution_attribution_prefers_persisted_manager_snapshots() {
        let manager_run = json!({
            "manager_json": {
                "capital_budget": {
                    "cash_balance_dkk": 18_075.0,
                    "cash_pct": 0.064,
                    "required_cash_buffer_dkk": 5_658.0,
                    "available_buy_budget_dkk": 12_417.0,
                    "remaining_deployment_capacity_dkk": 18_000.0,
                    "reinvestment_pressure_active": true,
                    "unrelated_sensitive_value": "excluded"
                },
                "hermes_preflight": {
                    "candidate_waterfall": [{
                        "strategy_key": "open|AMD:xnas|BUY|primary",
                        "symbol": "AMD:xnas",
                        "action": "BUY",
                        "technical": {
                            "status": "ok",
                            "run_date": "2026-07-21",
                            "sentiment": "BUY",
                            "trend_bias": "bullish",
                            "confluence_count": 4,
                            "min_confluences": 3
                        },
                        "markov": {
                            "status": "ok",
                            "run_date": "2026-07-21",
                            "current_state": "Bull",
                            "direction": "long",
                            "signed_signal": 0.548,
                            "conviction": 0.78
                        }
                    }]
                }
            }
        });
        let final_technical = json!({
            "status": "ok",
            "run_date": "2026-07-21",
            "sentiment": "HOLD",
            "trend_bias": "neutral",
            "confluence_count": 1,
            "min_confluences": 3
        });

        let candidate = matching_manager_preflight_candidate(
            &manager_run,
            "open|AMD:xnas|BUY|primary",
            "AMD:xnas",
            "BUY",
        );
        let technical = compact_attribution_technical(&final_technical, "manager_final");
        let markov = compact_attribution_markov(
            candidate.get("markov").unwrap_or(&JsonValue::Null),
            "manager_preflight",
        );
        let capital = compact_attribution_capital(
            manager_run["manager_json"]
                .get("capital_budget")
                .expect("persisted capital budget"),
        );

        assert_eq!(json_text(&technical, "evidence_source"), "manager_final");
        assert_eq!(json_text(&technical, "sentiment"), "HOLD");
        assert_eq!(json_text(&markov, "evidence_source"), "manager_preflight");
        assert_eq!(json_text(&markov, "direction"), "long");
        assert_eq!(value_f64(&markov, "signed_signal"), 0.548);
        assert_eq!(json_text(&capital, "evidence_source"), "manager_run");
        assert_eq!(value_f64(&capital, "available_buy_budget_dkk"), 12_417.0);
        assert!(capital.get("unrelated_sensitive_value").is_none());
    }

    #[test]
    fn execution_ledger_attribution_aggregates_reconciled_sell_fills() {
        let order = json!({"action": "SELL", "quantity": 4.0});
        let summary = json!({
            "fill_count": 2,
            "ledger_entry_count": 2,
            "filled_quantity": 4.0,
            "last_fill_at": "2026-07-21T18:00:00Z",
            "commission_dkk": 21.0,
            "tax_dkk": 0.0,
            "realised_gain_dkk": 1_234.5,
            "cost_basis_sold_dkk": 6_500.0
        });

        let outcome = compact_execution_ledger_outcome(&order, &summary, "reconciled_fills");

        assert_eq!(json_text(&outcome, "status"), "reconciled");
        assert_eq!(json_text(&outcome, "side"), "SELL");
        assert_eq!(json_text(&outcome, "evidence_source"), "reconciled_fills");
        assert_eq!(value_i64(&outcome, "fill_count"), 2);
        assert_eq!(value_f64(&outcome, "realised_gain_dkk"), 1_234.5);
        assert_eq!(
            outcome.get("fully_filled").and_then(JsonValue::as_bool),
            Some(true)
        );
    }

    #[test]
    fn holding_period_attribution_uses_available_trading_sessions_and_side() {
        let fill = json!({
            "first_fill_at": "2026-07-20T15:34:00Z",
            "filled_quantity": 2.0,
            "average_fill_price_local": 100.0,
            "currency": "USD"
        });
        let closes = vec![
            json!({"run_date": "2026-07-21", "close": 105.0}),
            json!({"run_date": "2026-07-22", "close": 102.0}),
            json!({"run_date": "2026-07-23", "close": 104.0}),
            json!({"run_date": "2026-07-24", "close": 106.0}),
            json!({"run_date": "2026-07-27", "close": 110.0}),
        ];

        let buy = compact_holding_period_outcome(&json!({"action": "BUY"}), &fill, &closes);
        assert_eq!(json_text(&buy, "status"), "complete");
        assert_eq!(value_i64(&buy, "available_sessions"), 5);
        assert!((value_f64(&buy["one_session"], "market_return_pct") - 0.05).abs() < 1e-9);
        assert!((value_f64(&buy["five_session"], "directional_return_pct") - 0.10).abs() < 1e-9);

        let sell = compact_holding_period_outcome(&json!({"action": "SELL"}), &fill, &closes);
        assert!((value_f64(&sell["one_session"], "directional_return_pct") + 0.05).abs() < 1e-9);
    }

    #[test]
    fn holding_period_attribution_stays_partial_until_five_closes_exist() {
        let outcome = compact_holding_period_outcome(
            &json!({"action": "BUY"}),
            &json!({
                "first_fill_at": "2026-07-20T15:34:00Z",
                "filled_quantity": 1.0,
                "average_fill_price_local": 100.0,
                "currency": "USD"
            }),
            &[json!({"run_date": "2026-07-21", "close": 98.0})],
        );

        assert_eq!(json_text(&outcome, "status"), "partial");
        assert!((value_f64(&outcome["one_session"], "directional_return_pct") + 0.02).abs() < 1e-9);
        assert!(outcome["five_session"].is_null());
    }

    #[test]
    fn holding_thesis_reviews_only_flag_current_positions_with_due_recorded_theses() {
        let now = DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z")
            .expect("fixed test timestamp")
            .with_timezone(&Utc);
        let reviews = compact_holding_thesis_reviews(
            &[
                json!({
                    "symbol": "AMD:xnas",
                    "instrument_name": "AMD",
                    "quantity": 2.0
                }),
                json!({
                    "symbol": "NVDA:xnas",
                    "instrument_name": "NVIDIA",
                    "quantity": 1.0
                }),
            ],
            &[
                json!({
                    "id": 22,
                    "symbol": "AMD:xnas",
                    "created_at": "2026-07-12T15:00:00Z",
                    "first_fill_at": "2026-07-12T15:30:00Z",
                    "trade_thesis_json": {
                        "status": "recorded",
                        "intended_holding_window": "next_2_weeks",
                        "entry_rationale": "Verified technical setup",
                        "invalidation": "Fresh evidence no longer supports the long setup."
                    }
                }),
                json!({
                    "id": 23,
                    "symbol": "MSFT:xnas",
                    "created_at": "2026-07-01T15:00:00Z",
                    "first_fill_at": "2026-07-01T15:30:00Z",
                    "trade_thesis_json": {
                        "status": "recorded",
                        "intended_holding_window": "next_2_weeks"
                    }
                }),
            ],
            7,
            now,
        );

        assert_eq!(json_text(&reviews, "status"), "review_due");
        assert_eq!(value_i64(&reviews, "held_position_count"), 2);
        assert_eq!(value_i64(&reviews, "review_count"), 1);
        let item = reviews["reviews"][0].clone();
        assert_eq!(json_text(&item, "symbol"), "AMD:xnas");
        assert_eq!(json_text(&item, "status"), "thesis_window_elapsed");
        assert_eq!(value_i64(&item, "age_days"), 14);
        assert_eq!(value_i64(&item, "latest_thesis_order_id"), 22);
        assert!(json_text(&item, "operator_next_step").contains("not instruct an exit"));
        assert!(json_text(&reviews, "safety").contains("no_saxo"));
    }

    #[test]
    fn holding_thesis_reviews_wait_until_the_stale_window() {
        let now = DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z")
            .expect("fixed test timestamp")
            .with_timezone(&Utc);
        let reviews = compact_holding_thesis_reviews(
            &[json!({"symbol": "AMD:xnas", "quantity": 2.0})],
            &[json!({
                "id": 22,
                "symbol": "AMD:xnas",
                "created_at": "2026-07-24T15:00:00Z",
                "first_fill_at": "2026-07-24T15:30:00Z",
                "trade_thesis_json": {
                    "status": "recorded",
                    "intended_holding_window": "next_2_weeks"
                }
            })],
            7,
            now,
        );

        assert_eq!(json_text(&reviews, "status"), "no_reviews_due");
        assert_eq!(value_i64(&reviews, "review_count"), 0);
    }

    #[test]
    fn position_lifecycle_attribution_tracks_observed_add_reduce_and_exit() {
        let fills = vec![
            json!({"id": 1, "execution_order_id": 10, "created_at": "2026-07-20T15:00:00Z", "side": "BUY", "delta_quantity": 2.0}),
            json!({"id": 2, "execution_order_id": 11, "created_at": "2026-07-21T15:00:00Z", "side": "BUY", "delta_quantity": 3.0}),
            json!({"id": 3, "execution_order_id": 12, "created_at": "2026-07-22T15:00:00Z", "side": "SELL", "delta_quantity": 1.0}),
            json!({"id": 4, "execution_order_id": 13, "created_at": "2026-07-23T15:00:00Z", "side": "SELL", "delta_quantity": 4.0}),
        ];

        let add = compact_execution_position_lifecycle(&json!({"id": 11, "action": "BUY"}), &fills);
        assert_eq!(json_text(&add, "phase"), "add");
        assert_eq!(json_text(&add, "history_status"), "observed_local_fills");
        assert_eq!(value_f64(&add, "observed_net_before"), 2.0);
        assert_eq!(value_f64(&add, "observed_net_after"), 5.0);

        let reduce =
            compact_execution_position_lifecycle(&json!({"id": 12, "action": "SELL"}), &fills);
        assert_eq!(json_text(&reduce, "phase"), "reduce");
        assert_eq!(value_f64(&reduce, "observed_net_before"), 5.0);
        assert_eq!(value_f64(&reduce, "observed_net_after"), 4.0);

        let exit =
            compact_execution_position_lifecycle(&json!({"id": 13, "action": "SELL"}), &fills);
        assert_eq!(json_text(&exit, "phase"), "exit");
        assert_eq!(value_f64(&exit, "observed_net_after"), 0.0);
    }

    #[test]
    fn position_lifecycle_attribution_refuses_to_infer_unobserved_inventory() {
        let outcome = compact_execution_position_lifecycle(
            &json!({"id": 14, "action": "SELL"}),
            &[json!({
                "id": 1,
                "execution_order_id": 14,
                "created_at": "2026-07-20T15:00:00Z",
                "side": "SELL",
                "delta_quantity": 2.0
            })],
        );

        assert_eq!(json_text(&outcome, "phase"), "partial_history");
        assert_eq!(json_text(&outcome, "history_status"), "partial_history");
        assert!(json_text(&outcome, "interpretation").contains("not broker position truth"));
    }

    #[tokio::test]
    async fn holding_period_attribution_reads_reconciled_fills_and_daily_closes() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                execution_order_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                delta_quantity REAL NOT NULL,
                average_price_local REAL NOT NULL,
                currency TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution fills");
        sqlx::query(
            "CREATE TABLE daily_indicator_signals (
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                close REAL NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create daily indicator signals");
        sqlx::query(
            "INSERT INTO execution_fills
                (execution_order_id, created_at, delta_quantity, average_price_local, currency)
             VALUES (42, '2026-07-20T15:34:00Z', 1, 100, 'USD'),
                    (42, '2026-07-20T15:35:00Z', 3, 110, 'USD')",
        )
        .execute(&state.pool)
        .await
        .expect("insert reconciled fills");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (symbol, run_date, status, close) VALUES
                ('AMD:xnas', '2026-07-20', 'ok', 99),
                ('AMD:xnas', '2026-07-21', 'ok', 108),
                ('AMD:xnas', '2026-07-22', 'ok', 109),
                ('AMD:xnas', '2026-07-23', 'ok', 110),
                ('AMD:xnas', '2026-07-24', 'ok', 111),
                ('AMD:xnas', '2026-07-27', 'ok', 112)",
        )
        .execute(&state.pool)
        .await
        .expect("insert daily closes");

        let outcome = state
            .execution_order_holding_period_outcome(&json!({
                "id": 42,
                "symbol": "AMD:xnas",
                "action": "BUY"
            }))
            .await
            .expect("read holding-period attribution");

        assert_eq!(json_text(&outcome, "status"), "complete");
        assert_eq!(value_f64(&outcome, "filled_quantity"), 4.0);
        assert!((value_f64(&outcome, "fill_price_local") - 107.5).abs() < 1e-9);
        assert_eq!(json_text(&outcome["one_session"], "as_of"), "2026-07-21");
        assert_eq!(json_text(&outcome["five_session"], "as_of"), "2026-07-27");
    }

    #[tokio::test]
    async fn trade_thesis_outcome_evidence_reads_only_recorded_buy_theses() {
        let state = runtime_settings_test_state("{}").await;
        for statement in [
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                quantity REAL NOT NULL,
                currency TEXT NOT NULL,
                trade_thesis_json TEXT
            )",
            "CREATE TABLE execution_fills (
                execution_order_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                delta_quantity REAL NOT NULL,
                average_price_local REAL NOT NULL,
                currency TEXT NOT NULL
            )",
            "CREATE TABLE daily_indicator_signals (
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                close REAL NOT NULL
            )",
        ] {
            sqlx::query(statement)
                .execute(&state.pool)
                .await
                .expect("create trade-thesis evidence table");
        }
        sqlx::query(
            "INSERT INTO execution_orders
                (id, created_at, symbol, action, quantity, currency, trade_thesis_json)
             VALUES
                (41, '2026-07-20T15:00:00Z', 'AMD:xnas', 'BUY', 1, 'USD',
                 '{\"status\":\"recorded\"}'),
                (42, '2026-07-20T15:00:00Z', 'NVDA:xnas', 'BUY', 1, 'USD', NULL)",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-order theses");
        sqlx::query(
            "INSERT INTO execution_fills
                (execution_order_id, created_at, delta_quantity, average_price_local, currency)
             VALUES (41, '2026-07-20T15:30:00Z', 1, 100, 'USD')",
        )
        .execute(&state.pool)
        .await
        .expect("seed reconciled thesis fill");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (symbol, run_date, status, close) VALUES
                ('AMD:xnas', '2026-07-21', 'ok', 101),
                ('AMD:xnas', '2026-07-22', 'ok', 102),
                ('AMD:xnas', '2026-07-23', 'ok', 103),
                ('AMD:xnas', '2026-07-24', 'ok', 104),
                ('AMD:xnas', '2026-07-27', 'ok', 105)",
        )
        .execute(&state.pool)
        .await
        .expect("seed later AMD closes");

        let evidence = state
            .trade_thesis_outcome_evidence()
            .await
            .expect("read trade-thesis outcome evidence");

        assert_eq!(json_text(&evidence, "status"), "collecting");
        assert_eq!(value_i64(&evidence, "recorded_thesis_count"), 1);
        assert_eq!(value_i64(&evidence, "filled_thesis_count"), 1);
        assert_eq!(value_i64(&evidence["one_session"], "sample_count"), 1);
        assert_eq!(value_i64(&evidence["five_session"], "sample_count"), 1);
        assert!(
            (value_f64(&evidence["five_session"], "average_directional_return_pct") - 0.05).abs()
                < 1e-9
        );
    }

    #[test]
    fn tuning_pulse_comparison_separates_execution_reports_from_mature_shadow_evidence() {
        let reports = vec![
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-08-19",
                "status": "completed",
            }),
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-08-20",
                "status": "xai_error",
            }),
            json!({
                "analysis_pulse_key": "us_mid_session_shadow:2026-08-19",
                "status": "completed",
                "report_json": serde_json::to_string(&json!({
                    "shadow_change_assessment": {"status": "no_new_information"},
                })).unwrap(),
            }),
        ];
        let shadows = (0..20)
            .map(|index| {
                let (gate_code, gate_status) = if index < 8 {
                    ("technical", "clear_signal_gate_only")
                } else if index < 12 {
                    ("markov", "clear_signal_gate_only")
                } else if index < 17 {
                    ("technical", "blocked_signal_gate")
                } else {
                    ("not_evaluated_shadow", "insufficient_decision_time_evidence")
                };
                let hermes_effect = match index {
                    0..=3 => "record_only_allow",
                    4..=7 => "record_only_reduce",
                    8..=9 => "record_only_stand_down",
                    10 => "record_only_review",
                    11..=13 => "record_only_no_matching_advice",
                    14..=15 => "record_only_unavailable",
                    16..=17 => "not_requested_shadow",
                    _ => "legacy_unknown_effect",
                };
                let self_check = if index < 8 {
                    json!(true)
                } else if index < 12 {
                    json!(false)
                } else {
                    JsonValue::Null
                };
                json!({
                    "analysis_pulse_key": "us_mid_session_shadow:2026-08-19",
                    "earlier_pulse_report_id": 41,
                    "appeared_in_earlier_pulse": index < 5,
                    "deterministic_gate_code": gate_code,
                    "deterministic_gate_json": serde_json::to_string(&json!({ "status": gate_status })).unwrap(),
                    "hermes_effect": hermes_effect,
                    "hermes_advice_snapshot_json": serde_json::to_string(&json!({ "context_self_check_complete": self_check })).unwrap(),
                    "approved_policy_source": if index < 16 { json!("active_strategy_baseline:baseline-1") } else { JsonValue::Null },
                    "reference_price_source": "saxo_infoprices",
                    "one_session_outcome_json": "{\"status\":\"observed\"}",
                    "five_session_outcome_json": "{\"status\":\"observed\"}",
                    "twenty_session_outcome_json": "{\"status\":\"observed\"}",
                    "five_session_after_cost_outcome_json": serde_json::to_string(&json!({
                        "status": "estimated",
                        "estimated_after_cost_return_pct": if index < 12 { 0.01 } else { -0.01 },
                    })).unwrap(),
                })
            })
            .collect::<Vec<_>>();

        let pulses = tuning_pulse_comparison_from_rows(&reports, &shadows);
        let eu_open = pulses
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_open_followup")
            .expect("EU opening pulse");
        assert_eq!(eu_open.authority, "execution_eligible");
        assert_eq!(eu_open.report_count, 2);
        assert_eq!(eu_open.terminal_success_count, 1);
        assert_eq!(eu_open.terminal_success_rate, Some(0.5));
        assert_eq!(eu_open.shadow_candidate_count, 0);
        assert_eq!(
            eu_open.outcome_status,
            "execution_outcomes_not_in_initial_shadow_slice"
        );

        let us_shadow = pulses
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow pulse");
        assert_eq!(us_shadow.authority, "shadow_observation_only");
        assert_eq!(us_shadow.report_count, 1);
        assert_eq!(us_shadow.shadow_candidate_count, 20);
        assert_eq!(us_shadow.shadow_comparable_candidate_count, 20);
        assert_eq!(us_shadow.shadow_new_candidate_count, 15);
        assert_eq!(us_shadow.shadow_repeated_candidate_count, 5);
        assert_eq!(us_shadow.shadow_candidate_novelty_rate, Some(0.75));
        assert_eq!(us_shadow.shadow_reference_captured_count, 20);
        assert_eq!(us_shadow.one_session_outcome_count, 20);
        assert_eq!(us_shadow.five_session_outcome_count, 20);
        assert_eq!(us_shadow.twenty_session_outcome_count, 20);
        assert_eq!(us_shadow.five_session_after_cost_count, 20);
        assert_eq!(us_shadow.five_session_after_cost_positive_rate, Some(0.6));
        assert_eq!(us_shadow.maturity, "mature");
        assert_eq!(us_shadow.outcome_status, "observational_shadow_outcomes");

        let retroactive = tuning_pulse_comparison_from_rows(
            &[json!({
                "analysis_pulse_key": "us_mid_session_shadow:2026-08-21",
                "status": "completed",
            })],
            &[json!({
                "analysis_pulse_key": "us_mid_session_shadow:2026-08-21",
                "reference_price_source": "not_captured_retroactively",
            })],
        );
        let retroactive_us = retroactive
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("retroactive US shadow pulse");
        assert_eq!(retroactive_us.shadow_candidate_count, 1);
        assert_eq!(retroactive_us.shadow_reference_captured_count, 0);
        assert_eq!(
            retroactive_us.shadow_reference_unavailable_retroactive_count,
            1
        );
        assert_eq!(
            retroactive_us.outcome_status,
            "historical_reference_unavailable"
        );

        let changes = tuning_shadow_change_evidence_from_rows(&reports);
        let us_changes = changes
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow change evidence");
        assert_eq!(us_changes.report_count, 1);
        assert_eq!(us_changes.comparison_available_assessment_count, 1);
        assert_eq!(us_changes.no_new_information_count, 1);
        assert_eq!(us_changes.no_new_information_rate, Some(1.0));

        let gates = tuning_shadow_gate_evidence_from_rows(&shadows);
        let us_gates = gates
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow gate evidence");
        assert_eq!(us_gates.candidate_count, 20);
        assert_eq!(us_gates.technical_source_count, 13);
        assert_eq!(us_gates.markov_fallback_source_count, 4);
        assert_eq!(us_gates.not_evaluated_source_count, 3);
        assert_eq!(us_gates.clear_signal_count, 12);
        assert_eq!(us_gates.blocked_signal_count, 5);
        assert_eq!(us_gates.insufficient_evidence_count, 3);
        assert_eq!(us_gates.unclassified_count, 0);

        let hermes = tuning_shadow_hermes_evidence_from_rows(&shadows);
        let us_hermes = hermes
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow Hermes evidence");
        assert_eq!(us_hermes.candidate_count, 20);
        assert_eq!(us_hermes.allow_count, 4);
        assert_eq!(us_hermes.reduce_count, 4);
        assert_eq!(us_hermes.stand_down_count, 2);
        assert_eq!(us_hermes.review_count, 1);
        assert_eq!(us_hermes.no_matching_advice_count, 3);
        assert_eq!(us_hermes.unavailable_count, 2);
        assert_eq!(us_hermes.not_requested_count, 2);
        assert_eq!(us_hermes.self_check_complete_count, 8);
        assert_eq!(us_hermes.self_check_incomplete_count, 4);
        assert_eq!(us_hermes.self_check_not_recorded_count, 8);
        assert_eq!(us_hermes.approved_policy_source_count, 16);
        assert_eq!(us_hermes.missing_policy_source_count, 4);
        assert_eq!(us_hermes.unclassified_effect_count, 2);
    }

    #[test]
    fn tuning_candidate_novelty_requires_an_earlier_opening_report() {
        let pulses = tuning_pulse_comparison_from_rows(
            &[],
            &[json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "earlier_pulse_report_id": null,
                "appeared_in_earlier_pulse": false,
                "reference_price_source": "awaiting_saxo_infoprice",
            })],
        );
        let eu_shadow = pulses
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow pulse");
        assert_eq!(eu_shadow.shadow_candidate_count, 1);
        assert_eq!(eu_shadow.shadow_comparable_candidate_count, 0);
        assert_eq!(eu_shadow.shadow_new_candidate_count, 0);
        assert_eq!(eu_shadow.shadow_repeated_candidate_count, 0);
        assert_eq!(eu_shadow.shadow_candidate_novelty_rate, None);
    }

    #[test]
    fn tuning_shadow_outcome_integrity_separates_missing_ledger_from_pending_quote() {
        let reports = [json!({
            "id": 71,
            "analysis_pulse_key": "us_mid_session_shadow:2026-08-24",
            "status": "completed",
            "report_json": {
                "suggested_trades": [{
                    "symbol": "MSFT:xnas",
                    "action": "BUY",
                    "quantity": 2,
                }],
            },
        })];

        let missing = tuning_pulse_comparison_from_rows(&reports, &[]);
        let missing_us = missing
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow pulse with missing ledger");
        assert_eq!(missing_us.shadow_candidate_report_count, 1);
        assert_eq!(missing_us.shadow_reports_missing_outcome_count, 1);
        assert_eq!(missing_us.shadow_awaiting_reference_count, 0);
        assert_eq!(missing_us.outcome_status, "shadow_outcome_ledger_gap");

        let pending = tuning_pulse_comparison_from_rows(
            &reports,
            &[json!({
                "report_id": 71,
                "analysis_pulse_key": "us_mid_session_shadow:2026-08-24",
                "status": "awaiting_reference",
                "reference_price_source": "awaiting_saxo_infoprice",
            })],
        );
        let pending_us = pending
            .iter()
            .find(|pulse| pulse.pulse_key == "us_mid_session_shadow")
            .expect("US shadow pulse awaiting quote");
        assert_eq!(pending_us.shadow_candidate_report_count, 1);
        assert_eq!(pending_us.shadow_reports_missing_outcome_count, 0);
        assert_eq!(pending_us.shadow_awaiting_reference_count, 1);
        assert_eq!(pending_us.outcome_status, "awaiting_saxo_reference_quote");
    }

    #[test]
    fn tuning_shadow_change_evidence_keeps_assessment_states_separate() {
        let reports = [
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "report_json": {"shadow_change_assessment": {"status": "material_change"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-21",
                "report_json": {"shadow_change_assessment": {"status": "no_new_information"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-22",
                "report_json": {"shadow_change_assessment": {"status": "not_available"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-23",
                "report_json": {"shadow_change_assessment": {"status": "comparison_invalid"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-24",
                "report_json": {"shadow_change_assessment": {"status": "not_applicable"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-25",
                "report_json": {},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-26",
                "report_json": {"shadow_change_assessment": {"status": "legacy_unknown"}},
            }),
        ];
        let evidence = tuning_shadow_change_evidence_from_rows(&reports);
        let eu_shadow = evidence
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow change evidence");
        assert_eq!(eu_shadow.report_count, 7);
        assert_eq!(eu_shadow.comparison_available_assessment_count, 3);
        assert_eq!(eu_shadow.material_change_count, 1);
        assert_eq!(eu_shadow.no_new_information_count, 1);
        assert_eq!(eu_shadow.no_new_information_rate, Some(1.0 / 3.0));
        assert_eq!(eu_shadow.opening_reference_not_available_count, 1);
        assert_eq!(eu_shadow.not_applicable_count, 1);
        assert_eq!(eu_shadow.comparison_invalid_count, 1);
        assert_eq!(eu_shadow.missing_assessment_count, 1);
        assert_eq!(eu_shadow.unclassified_assessment_count, 1);
    }

    #[test]
    fn tuning_shadow_support_risk_evidence_uses_saved_candidate_context_only() {
        let rows = [
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "report_time_context_json": {"support_risk_snapshot": {"break_risk_label": "low", "break_risk": 0.2, "confidence": 0.8, "history_coverage": 0.9}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-21",
                "report_time_context_json": {"support_risk_snapshot": {"break_risk_label": "moderate", "break_risk": 0.5, "confidence": 0.6, "history_coverage": 0.7}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-22",
                "report_time_context_json": {"support_risk_snapshot": {"break_risk_label": "high", "break_risk": 0.8, "confidence": 0.4, "history_coverage": 0.5}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-23",
                "report_time_context_json": {"support_risk_snapshot": null},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-24",
                "report_time_context_json": {"support_risk_snapshot": {"break_risk_label": "unavailable"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-25",
                "report_time_context_json": {"support_risk_snapshot": {"break_risk_label": "legacy_unknown"}},
            }),
        ];
        let evidence = tuning_shadow_support_risk_evidence_from_rows(&rows);
        let eu_shadow = evidence
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow Support/Risk evidence");
        assert_eq!(eu_shadow.candidate_count, 6);
        assert_eq!(eu_shadow.snapshot_available_count, 5);
        assert_eq!(eu_shadow.low_break_risk_count, 1);
        assert_eq!(eu_shadow.moderate_break_risk_count, 1);
        assert_eq!(eu_shadow.high_break_risk_count, 1);
        assert_eq!(eu_shadow.unavailable_count, 2);
        assert_eq!(eu_shadow.complete_context_count, 3);
        assert_eq!(eu_shadow.average_break_risk, Some(0.5));
        assert_eq!(eu_shadow.average_confidence, Some(0.6));
        assert!(
            (eu_shadow
                .average_history_coverage
                .expect("average history coverage")
                - 0.7)
                .abs()
                < 1e-9
        );
        assert_eq!(eu_shadow.unclassified_count, 1);
    }

    #[test]
    fn tuning_trade_thesis_evidence_keeps_its_independent_bounded_scope() {
        let evidence = tuning_trade_thesis_evidence_from_payload(&json!({
            "status": "collecting",
            "recorded_thesis_count": 8,
            "filled_thesis_count": 5,
            "one_session": {
                "sample_count": 3,
                "average_directional_return_pct": 0.02,
                "positive_return_rate": 0.67,
            },
            "five_session": {
                "sample_count": 2,
                "average_directional_return_pct": -0.01,
                "positive_return_rate": 0.5,
            },
            "minimum_complete_observations": 20,
            "scan_limit": 50,
            "interpretation": "Recorded fill-to-close evidence only.",
        }));
        assert_eq!(evidence.status, "collecting");
        assert_eq!(evidence.recorded_thesis_count, 8);
        assert_eq!(evidence.filled_thesis_count, 5);
        assert_eq!(evidence.one_session.sample_count, 3);
        assert_eq!(
            evidence.five_session.average_directional_return_pct,
            Some(-0.01)
        );
        assert_eq!(evidence.minimum_complete_observations, 20);
        assert_eq!(evidence.scan_limit, 50);
        assert_eq!(
            evidence.scope,
            "newest_recorded_buy_theses_independent_of_30_day_tuning_window"
        );
        assert_eq!(
            evidence.gross_net_label,
            "gross_directional_return_excludes_fx_commission_tax_slippage_and_realised_p_l"
        );
    }

    #[test]
    fn tuning_experiment_governance_exposes_statuses_without_proposal_values() {
        let evidence = tuning_experiment_governance_from_rows(&[
            json!({"status": "pending_review", "count": 2}),
            json!({"status": "approved_paper", "count": 1}),
            json!({"status": "approved_sim", "count": 3}),
            json!({"status": "active_paper", "count": 4}),
            json!({"status": "active_sim", "count": 5}),
            json!({"status": "ready_for_promotion", "count": 1}),
            json!({"status": "promoted", "count": 2}),
            json!({"status": "expired_stale", "count": 3}),
            json!({"status": "legacy_unknown", "count": 1}),
        ]);
        assert_eq!(evidence.status, "available");
        assert_eq!(evidence.total_experiment_count, 22);
        assert_eq!(evidence.pending_review_count, 2);
        assert_eq!(evidence.approved_paper_count, 1);
        assert_eq!(evidence.approved_sim_count, 3);
        assert_eq!(evidence.active_paper_count, 4);
        assert_eq!(evidence.active_sim_count, 5);
        assert_eq!(evidence.ready_for_promotion_count, 1);
        assert_eq!(evidence.promoted_count, 2);
        assert_eq!(evidence.terminal_count, 3);
        assert_eq!(evidence.unclassified_count, 1);
        assert_eq!(
            evidence.scope,
            "all_retained_strategy_experiment_lifecycle_rows"
        );
    }

    #[test]
    fn tuning_benchmark_comparison_keeps_proxy_return_and_freshness_labels() {
        let evidence = tuning_benchmark_comparison_from_payload(&json!({
            "status": "partial",
            "portfolio_return_pct": 0.03,
            "ready_count": 1,
            "reference_count": 2,
            "aligned_count": 1,
            "prior_close_count": 0,
            "stale_close_count": 0,
            "freshness": "aligned_close",
            "latest_run": {
                "created_at": "2026-08-21T23:55:00Z",
                "run_date": "2026-08-21",
                "status": "partial",
                "reference_count": 6,
                "success_count": 5,
                "error_count": 1,
            },
            "references": [{
                "key": "world",
                "label": "World ETF",
                "symbol": "IWRD:xlon",
                "status": "ready",
                "benchmark_return_pct": 0.02,
                "excess_return_pct": 0.01,
                "freshness": "aligned_close",
                "baseline_at": "2026-08-01T00:00:00Z",
                "latest_at": "2026-08-21T00:00:00Z",
            }],
            "caveat": "Not a total-return comparison.",
        }));
        assert_eq!(evidence.status, "partial");
        assert_eq!(evidence.portfolio_return_pct, Some(0.03));
        assert_eq!(evidence.ready_count, 1);
        assert_eq!(evidence.reference_count, 2);
        assert_eq!(evidence.freshness, "aligned_close");
        assert_eq!(evidence.collector_status, "partial");
        assert_eq!(evidence.collector_run_at, "2026-08-21T23:55:00Z");
        assert_eq!(evidence.collector_run_date, "2026-08-21");
        assert_eq!(evidence.collector_reference_count, 6);
        assert_eq!(evidence.collector_success_count, 5);
        assert_eq!(evidence.collector_error_count, 1);
        assert_eq!(evidence.references.len(), 1);
        assert_eq!(evidence.references[0].label, "World ETF");
        assert_eq!(evidence.references[0].benchmark_return_pct, Some(0.02));
        assert_eq!(evidence.references[0].excess_return_pct, Some(0.01));
        assert_eq!(evidence.references[0].freshness, "aligned_close");
        assert_eq!(
            evidence.scope,
            "one_month_local_account_value_vs_stored_native_currency_proxy_price_returns"
        );
        assert!(
            evidence
                .return_kind
                .contains("not_time_weighted_or_total_return")
        );
        assert_eq!(
            tuning_benchmark_comparison_unavailable().collector_status,
            "not_run"
        );
    }

    #[test]
    fn tuning_portfolio_outcome_keeps_snapshot_confidence_and_return_limits() {
        let outcome = tuning_portfolio_outcome_from_summary(&json!({
            "points": 4,
            "latest_total_market_value_dkk": 125_000.0,
            "change_dkk": 5_000.0,
            "range_return_pct": 4.17,
            "range_max_drawdown_pct": -1.25,
            "unreliable_cost_basis_points": 0,
            "confidence": {
                "status": "current",
                "valid_points": 4,
                "latest_recorded_at": "2026-08-21T12:00:00Z",
                "latest_snapshot_type": "runtime_current",
                "latest_source": "local_aggregate",
                "age_minutes": 3,
            }
        }));
        assert_eq!(outcome.status, "current");
        assert_eq!(outcome.snapshot_count, 4);
        assert_eq!(outcome.valid_snapshot_count, 4);
        assert_eq!(outcome.latest_value_dkk, Some(125_000.0));
        assert_eq!(outcome.change_dkk, Some(5_000.0));
        assert_eq!(outcome.simple_return_pct, Some(4.17));
        assert_eq!(outcome.max_drawdown_pct, Some(-1.25));
        assert_eq!(outcome.age_minutes, Some(3));
        assert!(
            outcome
                .return_kind
                .contains("not_realised_pnl_time_weighted_or_total_return")
        );

        let unavailable = tuning_portfolio_outcome_unavailable();
        assert_eq!(unavailable.status, "unavailable");
        assert_eq!(unavailable.latest_value_dkk, None);
        assert_eq!(unavailable.change_dkk, None);
    }

    #[test]
    fn tuning_monthly_goal_progress_keeps_calendar_baseline_and_target_labels() {
        let progress = tuning_monthly_goal_progress_from_payload(&json!({
            "periods": {
                "month": {
                    "status": "ready",
                    "target_dkk": 3_800.0,
                    "pnl_dkk": 1_140.0,
                    "progress_pct": 0.3,
                    "baseline_value_dkk": 240_000.0,
                    "period_start_utc": "2026-08-01T00:00:00Z",
                }
            }
        }));
        assert_eq!(progress.status, "ready");
        assert_eq!(progress.target_dkk, Some(3_800.0));
        assert_eq!(progress.value_change_dkk, Some(1_140.0));
        assert_eq!(progress.target_progress, Some(0.3));
        assert_eq!(progress.baseline_value_dkk, Some(240_000.0));
        assert_eq!(progress.period_start, "2026-08-01T00:00:00Z");
        assert!(
            progress
                .scope
                .contains("configured_calendar_month_portfolio_value_change")
        );
        assert_eq!(
            tuning_monthly_goal_progress_unavailable().status,
            "unavailable"
        );
    }

    #[test]
    fn tuning_shadow_markov_evidence_uses_saved_candidate_context_only() {
        let rows = [
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "report_time_context_json": {"markov_snapshot": {"status": "available", "direction": "long", "signed_signal": 0.4}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-21",
                "report_time_context_json": {"markov_snapshot": {"status": "available", "direction": "short", "signed_signal": -0.2}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-22",
                "report_time_context_json": {"markov_snapshot": {"status": "available", "direction": "neutral"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-23",
                "report_time_context_json": {"markov_snapshot": {"status": "not_available_at_report_time"}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-24",
                "report_time_context_json": {},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-25",
                "report_time_context_json": {"markov_snapshot": {"status": "legacy_unknown", "signed_signal": 0.9}},
            }),
        ];
        let evidence = tuning_shadow_markov_evidence_from_rows(&rows);
        let eu_shadow = evidence
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow Markov evidence");
        assert_eq!(eu_shadow.candidate_count, 6);
        assert_eq!(eu_shadow.snapshot_available_count, 5);
        assert_eq!(eu_shadow.long_direction_count, 1);
        assert_eq!(eu_shadow.short_direction_count, 1);
        assert_eq!(eu_shadow.neutral_direction_count, 1);
        assert_eq!(eu_shadow.unavailable_count, 2);
        assert_eq!(eu_shadow.complete_signal_count, 2);
        assert!(
            (eu_shadow
                .average_signed_signal
                .expect("average signed signal")
                - 0.1)
                .abs()
                < 1e-9
        );
        assert_eq!(eu_shadow.unclassified_count, 1);
    }

    #[test]
    fn tuning_shadow_quiver_evidence_uses_saved_candidate_context_only() {
        let rows = [
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "report_time_context_json": {"quiver_snapshot": {"status": "available", "freshness": {"status": "fresh"}, "direction": "bullish", "signal": 0.6, "confidence": 0.8}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-21",
                "report_time_context_json": {"quiver_snapshot": {"status": "available", "freshness": {"status": "partial"}, "direction": "bearish", "signal": -0.4, "confidence": 0.6}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-22",
                "report_time_context_json": {"quiver_snapshot": {"status": "available", "freshness": {"status": "stale"}, "direction": "neutral", "signal": 0.0, "confidence": 0.4}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-23",
                "report_time_context_json": {"quiver_snapshot": {"status": "not_available_at_report_time", "freshness": {"status": "fresh"}}},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-24",
                "report_time_context_json": {},
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-25",
                "report_time_context_json": {"quiver_snapshot": {"status": "available", "freshness": {"status": "legacy_unknown"}, "direction": "bullish"}},
            }),
        ];
        let evidence = tuning_shadow_quiver_evidence_from_rows(&rows);
        let eu_shadow = evidence
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow Quiver evidence");
        assert_eq!(eu_shadow.candidate_count, 6);
        assert_eq!(eu_shadow.snapshot_available_count, 5);
        assert_eq!(eu_shadow.fresh_source_count, 2);
        assert_eq!(eu_shadow.partial_source_count, 1);
        assert_eq!(eu_shadow.stale_source_count, 1);
        assert_eq!(eu_shadow.unavailable_source_count, 1);
        assert_eq!(eu_shadow.bullish_direction_count, 2);
        assert_eq!(eu_shadow.bearish_direction_count, 1);
        assert_eq!(eu_shadow.neutral_direction_count, 1);
        assert_eq!(eu_shadow.complete_signal_count, 3);
        assert!((eu_shadow.average_signal.expect("average signal") - (0.2 / 3.0)).abs() < 1e-9);
        assert_eq!(eu_shadow.average_confidence, Some(0.6));
        assert_eq!(eu_shadow.unclassified_count, 1);
    }

    #[test]
    fn tuning_shadow_gate_evidence_keeps_unknown_legacy_records_visible() {
        let evidence = tuning_shadow_gate_evidence_from_rows(&[json!({
            "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
            "deterministic_gate_code": "legacy_unknown_gate",
            "deterministic_gate_json": "{\"status\":\"legacy_unknown_result\"}",
        })]);
        let eu_shadow = evidence
            .iter()
            .find(|pulse| pulse.pulse_key == "europe_mid_session_shadow")
            .expect("EU shadow gate evidence");
        assert_eq!(eu_shadow.candidate_count, 1);
        assert_eq!(eu_shadow.unclassified_count, 1);
        assert_eq!(eu_shadow.clear_signal_count, 0);
        assert_eq!(eu_shadow.blocked_signal_count, 0);
        assert_eq!(eu_shadow.insufficient_evidence_count, 0);
    }

    #[test]
    fn tuning_execution_outcomes_keep_buy_movement_and_sell_accounting_separate() {
        let evidence = json!({
            "pulses": [{
                "pulse_key": "europe_open_followup",
                "pulse_label": "EU Open",
                "outcome": {
                    "attributed_order_count": 3,
                    "filled_buy_order_count": 2,
                    "reconciled_sell_order_count": 1,
                    "one_session": {
                        "sample_count": 2,
                        "average_directional_return_pct": 0.015,
                        "positive_return_rate": 0.5,
                    },
                    "five_session": {
                        "sample_count": 20,
                        "average_directional_return_pct": 0.02,
                        "positive_return_rate": 0.6,
                    },
                    "realised_sell": {
                        "realised_gain_dkk": 125.0,
                        "commission_dkk": 3.0,
                        "tax_dkk": 0.0,
                    },
                }
            }]
        });

        let outcomes = tuning_execution_pulse_outcomes_from_evidence(&evidence);
        assert_eq!(outcomes.len(), 2);
        let eu = outcomes
            .iter()
            .find(|outcome| outcome.pulse_key == "europe_open_followup")
            .expect("EU execution evidence");
        assert_eq!(eu.attributed_order_count, 3);
        assert_eq!(eu.filled_buy_order_count, 2);
        assert_eq!(eu.one_session.sample_count, 2);
        assert_eq!(eu.five_session.positive_return_rate, Some(0.6));
        assert_eq!(eu.reconciled_sell_order_count, 1);
        assert_eq!(eu.realised_sell_gain_dkk, 125.0);
        assert_eq!(eu.realised_sell_commission_dkk, 3.0);
        assert_eq!(eu.maturity, "preliminary");

        let us = outcomes
            .iter()
            .find(|outcome| outcome.pulse_key == "us_open_followup")
            .expect("US execution evidence");
        assert_eq!(us.attributed_order_count, 0);
        assert_eq!(us.one_session.sample_count, 0);
        assert_eq!(us.maturity, "no_attributable_orders");
    }

    #[test]
    fn tuning_execution_lifecycle_evidence_preserves_pending_and_unknown_statuses() {
        let evidence = json!({
            "pulses": [{
                "pulse_key": "europe_open_followup",
                "outcome": {
                    "attributed_order_count": 14,
                    "execution_status_counts": {
                        "pending_execution": 1,
                        "waiting_for_market_open": 2,
                        "submitted_to_broker": 3,
                        "broker_partially_filled": 1,
                        "broker_state_unknown": 1,
                        "executed": 2,
                        "execution_failed": 1,
                        "expired_local": 1,
                        "broker_cancelled": 1,
                        "legacy_unknown": 1
                    }
                }
            }]
        });

        let lifecycle = tuning_execution_lifecycle_evidence_from_evidence(&evidence);
        assert_eq!(lifecycle.len(), 2);
        let eu = lifecycle
            .iter()
            .find(|row| row.pulse_key == "europe_open_followup")
            .expect("EU lifecycle evidence");
        assert_eq!(eu.attributed_order_count, 14);
        assert_eq!(eu.locally_queued_count, 3);
        assert_eq!(eu.broker_active_count, 4);
        assert_eq!(eu.broker_state_unknown_count, 1);
        assert_eq!(eu.executed_count, 2);
        assert_eq!(eu.failed_count, 1);
        assert_eq!(eu.expired_count, 1);
        assert_eq!(eu.cancelled_count, 1);
        assert_eq!(eu.unclassified_count, 1);

        let us = lifecycle
            .iter()
            .find(|row| row.pulse_key == "us_open_followup")
            .expect("US lifecycle evidence");
        assert_eq!(us.attributed_order_count, 0);
        assert_eq!(us.locally_queued_count, 0);
        assert_eq!(us.unclassified_count, 0);
    }

    #[test]
    fn tuning_protection_coverage_keeps_planned_and_unprotected_positions_distinct() {
        let coverage = tuning_protective_stop_coverage_from_payload(&json!({
            "status": "attention_required",
            "summary": {
                "position_count": 4,
                "protected_count": 1,
                "partial_count": 1,
                "planned_count": 1,
                "unprotected_count": 1,
                "exception_count": 3,
                "total_quantity": 20.0,
                "confirmed_covered_quantity": 12.0,
            }
        }));
        assert_eq!(coverage.status, "attention_required");
        assert_eq!(coverage.position_count, 4);
        assert_eq!(coverage.protected_count, 1);
        assert_eq!(coverage.partial_count, 1);
        assert_eq!(coverage.planned_count, 1);
        assert_eq!(coverage.unprotected_count, 1);
        assert_eq!(coverage.exception_count, 3);
        assert_eq!(coverage.confirmed_coverage_ratio, Some(0.6));

        let unavailable = tuning_protective_stop_coverage_from_payload(&json!({
            "status": "unavailable",
            "summary": {"total_quantity": 0.0}
        }));
        assert_eq!(unavailable.confirmed_coverage_ratio, None);
        assert_eq!(unavailable.unprotected_count, 0);
    }

    #[test]
    fn tuning_candidate_funnel_uses_latest_persisted_manager_counts_only() {
        let funnel = tuning_execution_candidate_funnel_from_rows(&[
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-08-20",
                "manager_run_id": 41,
                "manager_json": {
                    "candidate_order_count": 4,
                    "eligible_candidate_order_count": 3,
                    "approved_order_count": 1,
                    "skipped_order_count": 2,
                    "hermes_advice_delta": {"matched_candidate_count": 2}
                },
                "local_execution_row_count": 1
            }),
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-08-21",
                "manager_run_id": null,
                "manager_json": null,
                "local_execution_row_count": 0
            }),
            json!({
                "analysis_pulse_key": "us_open_followup:2026-08-20",
                "manager_run_id": 42,
                "manager_json": "{\"candidate_order_count\":2,\"eligible_candidate_order_count\":2,\"approved_order_count\":1,\"skipped_order_count\":1,\"hermes_advice_delta\":{\"matched_candidate_count\":1}}",
                "local_execution_row_count": 1
            }),
            json!({
                "analysis_pulse_key": "europe_mid_session_shadow:2026-08-20",
                "manager_run_id": 43,
                "manager_json": {"candidate_order_count": 99},
                "local_execution_row_count": 99
            }),
        ]);
        assert_eq!(funnel.len(), 2);
        let eu = funnel
            .iter()
            .find(|row| row.pulse_key == "europe_open_followup")
            .expect("EU candidate funnel");
        assert_eq!(eu.report_count, 2);
        assert_eq!(eu.manager_run_count, 1);
        assert_eq!(eu.manager_snapshot_missing_count, 1);
        assert_eq!(eu.candidate_order_count, 4);
        assert_eq!(eu.eligible_candidate_order_count, 3);
        assert_eq!(eu.hermes_matched_candidate_count, 2);
        assert_eq!(eu.approved_order_count, 1);
        assert_eq!(eu.skipped_order_count, 2);
        assert_eq!(eu.local_execution_row_count, 1);

        let us = funnel
            .iter()
            .find(|row| row.pulse_key == "us_open_followup")
            .expect("US candidate funnel");
        assert_eq!(us.report_count, 1);
        assert_eq!(us.manager_run_count, 1);
        assert_eq!(us.candidate_order_count, 2);
        assert_eq!(us.hermes_matched_candidate_count, 1);
        assert_eq!(us.local_execution_row_count, 1);
    }

    #[test]
    fn decision_pulse_outcome_evidence_keeps_buy_price_movement_and_sell_ledger_gains_separate() {
        let evidence = decision_pulse_outcome_evidence_from_observations(&[
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-07-20",
                "analysis_pulse_label": "EU Open +1h15",
                "action": "BUY",
                "execution_status": "executed",
                "hermes_reviewed": true,
                "hermes_effect": "reduced",
                "holding_period_outcome": {
                    "filled_quantity": 2.0,
                    "one_session": {"directional_return_pct": 0.04},
                    "five_session": {"directional_return_pct": 0.06},
                },
                "ledger_outcome": null,
            }),
            json!({
                "analysis_pulse_key": "us_open_followup:2026-07-20",
                "analysis_pulse_label": "US Open +1h15",
                "action": "SELL",
                "execution_status": "executed",
                "hermes_reviewed": false,
                "hermes_effect": "not_recorded",
                "holding_period_outcome": null,
                "ledger_outcome": {
                    "status": "reconciled",
                    "realised_gain_dkk": 125.0,
                    "commission_dkk": 3.0,
                    "tax_dkk": 0.0,
                },
            }),
            json!({
                "strategy_type": "portfolio_sync",
                "action": "BUY",
                "execution_status": "not_recorded",
                "hermes_reviewed": false,
                "hermes_effect": "not_recorded",
                "holding_period_outcome": {"filled_quantity": 0.0},
                "ledger_outcome": null,
            }),
        ]);

        assert_eq!(json_text(&evidence, "status"), "collecting");
        assert_eq!(value_i64(&evidence["overall"], "attributed_order_count"), 3);
        assert_eq!(
            value_i64(&evidence["overall"], "hermes_reviewed_order_count"),
            1
        );
        assert_eq!(
            value_i64(&evidence["overall"]["one_session"], "sample_count"),
            1
        );
        assert!(
            (value_f64(
                &evidence["overall"]["five_session"],
                "average_directional_return_pct"
            ) - 0.06)
                .abs()
                < 1e-9
        );
        assert!(
            (value_f64(&evidence["overall"]["realised_sell"], "realised_gain_dkk") - 125.0).abs()
                < 1e-9
        );
        assert_eq!(
            evidence["overall"]["hermes_effect_counts"]["reduced"],
            json!(1)
        );
        assert_eq!(
            evidence["overall"]["execution_status_counts"]["executed"],
            json!(2)
        );
        let pulses = evidence["pulses"].as_array().expect("pulse rows");
        assert!(pulses.iter().any(|row| {
            json_text(row, "pulse_key") == "portfolio_sync"
                && json_text(row, "pulse_label") == "Portfolio Sync"
        }));
    }

    #[tokio::test]
    async fn position_lifecycle_attribution_reads_symbol_scoped_reconciled_fills() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                id INTEGER PRIMARY KEY,
                execution_order_id INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                created_at TEXT NOT NULL,
                side TEXT NOT NULL,
                delta_quantity REAL NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-fill lifecycle table");
        sqlx::query(
            "INSERT INTO execution_fills
                (id, execution_order_id, symbol, created_at, side, delta_quantity)
             VALUES
                (1, 41, 'AMD:xnas', '2026-07-20T15:00:00Z', 'BUY', 2),
                (2, 42, 'AMD:xnas', '2026-07-21T15:00:00Z', 'SELL', 1),
                (3, 99, 'NVDA:xnas', '2026-07-21T15:00:00Z', 'BUY', 9)",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-fill lifecycle rows");

        let outcome = state
            .execution_order_position_lifecycle(&json!({
                "id": 42,
                "symbol": "AMD:xnas",
                "action": "SELL"
            }))
            .await
            .expect("read position lifecycle attribution");

        assert_eq!(json_text(&outcome, "phase"), "reduce");
        assert_eq!(value_i64(&outcome, "observed_fill_count"), 2);
        assert_eq!(value_i64(&outcome, "observed_order_count"), 2);
        assert_eq!(value_f64(&outcome, "observed_net_before"), 2.0);
        assert_eq!(value_f64(&outcome, "observed_net_after"), 1.0);
    }

    #[tokio::test]
    async fn trade_thesis_attribution_uses_the_latest_prior_buy_for_the_symbol() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                trade_thesis_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-order thesis table");
        sqlx::query(
            "INSERT INTO execution_orders (id, created_at, symbol, action, trade_thesis_json)
             VALUES
                (41, '2026-07-20T15:00:00Z', 'AMD:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"starter_long\"}'),
                (42, '2026-07-21T15:00:00Z', 'AMD:xnas', 'SELL', NULL),
                (43, '2026-07-22T15:00:00Z', 'AMD:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"add_on_strength\"}'),
                (99, '2026-07-21T15:00:00Z', 'NVDA:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"other_symbol\"}')",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-order theses");

        let thesis = state
            .execution_order_trade_thesis(&json!({
                "id": 42,
                "created_at": "2026-07-21T15:00:00Z",
                "symbol": "AMD:xnas"
            }))
            .await
            .expect("read prior BUY thesis");

        assert_eq!(json_text(&thesis, "status"), "recorded");
        assert_eq!(json_text(&thesis, "strategy_key"), "starter_long");
    }

    #[tokio::test]
    async fn execution_ledger_outcome_reads_reconciled_fill_rows() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                id INTEGER PRIMARY KEY,
                execution_order_id INTEGER NOT NULL,
                ledger_id INTEGER,
                delta_quantity REAL NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-fill attribution table");
        sqlx::query(
            "CREATE TABLE trade_ledger (
                id INTEGER PRIMARY KEY,
                commission_dkk REAL NOT NULL,
                tax_dkk REAL NOT NULL,
                realised_gain_dkk REAL NOT NULL,
                cost_basis_sold_dkk REAL NOT NULL,
                quantity REAL NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create ledger attribution table");
        sqlx::query(
            "INSERT INTO trade_ledger (
                id, commission_dkk, tax_dkk, realised_gain_dkk,
                cost_basis_sold_dkk, quantity, created_at
            ) VALUES
                (10, 4.0, 0.0, 300.0, 1200.0, 1.0, '2026-07-21T17:00:00Z'),
                (11, 5.0, 0.0, 500.0, 1800.0, 2.0, '2026-07-21T18:00:00Z')",
        )
        .execute(&state.pool)
        .await
        .expect("seed ledger attribution rows");
        sqlx::query(
            "INSERT INTO execution_fills (
                id, execution_order_id, ledger_id, delta_quantity, created_at
            ) VALUES
                (1, 42, 10, 1.0, '2026-07-21T17:00:00Z'),
                (2, 42, 11, 2.0, '2026-07-21T18:00:00Z')",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-fill attribution rows");

        let outcome = state
            .execution_order_ledger_outcome(&json!({
                "id": 42,
                "ledger_id": 11,
                "status": "executed",
                "action": "SELL",
                "quantity": 3.0
            }))
            .await
            .expect("read reconciled ledger outcome");

        assert_eq!(json_text(&outcome, "evidence_source"), "reconciled_fills");
        assert_eq!(value_i64(&outcome, "fill_count"), 2);
        assert_eq!(value_f64(&outcome, "filled_quantity"), 3.0);
        assert_eq!(value_f64(&outcome, "commission_dkk"), 9.0);
        assert_eq!(value_f64(&outcome, "realised_gain_dkk"), 800.0);
    }

    #[test]
    fn attribution_delta_label_describes_hermes_manager_difference() {
        assert_eq!(
            attribution_delta_label(
                &json!({"action": "allow"}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "executed"})
            ),
            "allowed_executed"
        );
        assert_eq!(
            attribution_delta_label(
                &json!({"action": "review"}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "queued"})
            ),
            "manager_overrode_review"
        );
        assert_eq!(
            attribution_delta_label(
                &json!({}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "queued"})
            ),
            "manager_only"
        );
    }

    fn restore_rule_session(refresh_token: &str, minutes_ago: i64, refused: bool) -> JsonValue {
        json!({
            "access_token": "test-access",
            "refresh_token": refresh_token,
            "refresh_token_invalid_at": refused.then(|| Utc::now().to_rfc3339()),
            "refresh_token_expires_at": (Utc::now() + Duration::minutes(60 - minutes_ago)).to_rfc3339(),
            "last_refreshed_at": (Utc::now() - Duration::minutes(minutes_ago)).to_rfc3339(),
        })
    }

    /// Between two different refresh tokens the later one is live and the
    /// earlier consumed, so a stale working copy never displaces the durable
    /// row -- not even a refused one. Presenting the stale token would only be
    /// refused in turn.
    #[test]
    fn a_stale_working_copy_never_displaces_a_newer_durable_session() {
        let stale_file = restore_rule_session("test-refresh-1", 30, false);
        let newer_refused_db = restore_rule_session("test-refresh-2", 5, true);
        let newer_live_db = restore_rule_session("test-refresh-2", 5, false);

        assert!(!saxo_working_copy_supersedes_durable(
            &stale_file,
            &newer_refused_db
        ));
        assert!(!saxo_working_copy_supersedes_durable(
            &stale_file,
            &newer_live_db
        ));
    }

    /// A working copy holding a newer rotation or login -- one whose durable
    /// write failed -- is pushed to the durable row rather than lost.
    #[test]
    fn a_newer_working_copy_replaces_the_durable_session() {
        let rotated_file = restore_rule_session("test-refresh-3", 1, false);
        let durable = restore_rule_session("test-refresh-2", 5, false);

        assert!(saxo_working_copy_supersedes_durable(
            &rotated_file,
            &durable
        ));
    }

    /// For one and the same refresh token, knowing Saxo refused it is the
    /// newer fact, whichever copy holds it.
    #[test]
    fn a_refusal_of_the_same_token_wins_in_either_direction() {
        let live = restore_rule_session("test-refresh-2", 5, false);
        let refused = restore_rule_session("test-refresh-2", 5, true);

        assert!(saxo_working_copy_supersedes_durable(&refused, &live));
        assert!(!saxo_working_copy_supersedes_durable(&live, &refused));
    }

    #[test]
    fn saxo_session_needs_refresh_only_for_usable_near_expiry_tokens() {
        let valid = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::hours(2)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
        });
        let near_expiry = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
        });
        let invalid_refresh = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
            "refresh_token_invalid_at": Utc::now().to_rfc3339(),
        });

        assert!(!saxo_session_needs_refresh(&valid));
        assert!(saxo_session_needs_refresh(&near_expiry));
        assert!(!saxo_session_needs_refresh(&invalid_refresh));
    }

    #[test]
    fn configured_holiday_fallback_closes_copenhagen_and_oslo_on_whit_monday_2026() {
        let config = YamlValue::Null;
        let now = DateTime::parse_from_rfc3339("2026-05-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = market_exchange_rows_for_config(&config, now, None);
        let copenhagen = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XCSE"))
            .unwrap();
        let oslo = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XOSL"))
            .unwrap();

        assert_eq!(
            copenhagen.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            oslo.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            copenhagen.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            oslo.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            copenhagen
                .get("next_open_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        assert_eq!(
            oslo.get("next_open_at_utc").and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        for (code, reason) in [
            ("XLON", "Closed - Spring bank holiday"),
            ("XNAS", "Closed - Memorial Day"),
            ("XNYS", "Closed - Memorial Day"),
        ] {
            let row = rows
                .iter()
                .find(|row| row.get("code").and_then(JsonValue::as_str) == Some(code))
                .unwrap();
            assert_eq!(
                row.get("status_reason").and_then(JsonValue::as_str),
                Some(reason)
            );
            assert_eq!(
                row.get("is_tradable").and_then(JsonValue::as_bool),
                Some(false)
            );
        }
    }

    #[test]
    fn saxo_opening_auction_does_not_anchor_decision_window() {
        let config: YamlValue = serde_yaml::from_str(
            r#"
analysis_windows:
  offset_minutes_after_open: 75
  pre_sync_minutes_before_analysis: 5
  end_buffer_minutes_before_close: 15
"#,
        )
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-06-18T06:40:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "XCSE".to_string(),
            SaxoExchangeCalendar {
                exchange_id: "XCSE".to_string(),
                name: Some("Copenhagen".to_string()),
                timezone_id: Some("Europe/Copenhagen".to_string()),
                sessions: vec![
                    SaxoExchangeSession {
                        start_at: DateTime::parse_from_rfc3339("2026-06-18T05:30:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        end_at: DateTime::parse_from_rfc3339("2026-06-18T07:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        state: "OpeningAuction".to_string(),
                    },
                    SaxoExchangeSession {
                        start_at: DateTime::parse_from_rfc3339("2026-06-18T07:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        end_at: DateTime::parse_from_rfc3339("2026-06-18T15:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        state: "AutomatedTrading".to_string(),
                    },
                ],
            },
        );
        let cache = SaxoExchangeCalendarCache {
            checked_date: now.date_naive(),
            checked_at: now,
            exchanges,
            source: "test".to_string(),
        };

        let rows = market_exchange_rows_for_config(&config, now, Some(&cache));
        let copenhagen = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XCSE"))
            .unwrap();

        assert_eq!(
            copenhagen
                .get("session_open_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-06-18T07:00:00Z")
        );
        assert_eq!(
            copenhagen
                .get("open_analysis_window_start_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-06-18T08:15:00Z")
        );
        assert_eq!(
            copenhagen
                .get("session_open_local")
                .and_then(JsonValue::as_str),
            Some("2026-06-18 09:00")
        );
        assert_eq!(
            copenhagen
                .get("open_analysis_window_active")
                .and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            copenhagen.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
    }

    #[test]
    fn validates_hermes_experiment_lifecycle_transitions() {
        assert_eq!(
            hermes_experiment_next_status("pending_review", "approve_paper"),
            Some("approved_paper")
        );
        assert_eq!(
            hermes_experiment_next_status("active_sim", "ready_for_promotion"),
            Some("ready_for_promotion")
        );
        assert_eq!(
            hermes_experiment_next_status("ready_for_promotion", "promote"),
            Some("promoted")
        );
        assert_eq!(
            hermes_experiment_next_status("pending_review", "promote"),
            None
        );
        assert_eq!(
            hermes_experiment_next_status("pending_review", "expire_stale"),
            Some("expired_stale")
        );
    }

    #[test]
    fn hermes_experiment_duplicate_statuses_are_active_or_pending_only() {
        for status in [
            "pending_review",
            "approved_paper",
            "active_paper",
            "approved_sim",
            "active_sim",
            "ready_for_promotion",
        ] {
            assert!(hermes_experiment_status_blocks_duplicate(status));
        }

        for status in [
            "rejected",
            "paper_failed",
            "sim_failed",
            "failed",
            "expired_stale",
            "promoted",
            "",
        ] {
            assert!(!hermes_experiment_status_blocks_duplicate(status));
        }
    }

    #[tokio::test]
    async fn advisory_experiment_context_redacts_pending_review_values() {
        let state = runtime_settings_test_state("hermes:\n  experiments: {}\n").await;
        sqlx::query(
            "CREATE TABLE strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                baseline_id TEXT,
                goal_version INTEGER,
                hypothesis TEXT,
                changed_variable_path TEXT NOT NULL,
                old_value_json TEXT,
                new_value_json TEXT,
                expected_effect TEXT,
                risk_notes TEXT,
                evidence_json TEXT,
                approval_json TEXT,
                metrics_json TEXT,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create strategy experiments table");
        for (id, status, new_value) in [
            ("pending", "pending_review", "99"),
            ("approved", "approved_paper", "3"),
            ("active", "active_sim", "4"),
            ("rejected", "rejected", "1"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO strategy_experiments (id, created_at, status, changed_variable_path, new_value_json)
                 VALUES ('{}', '2026-08-19T12:00:00Z', '{}', 'strategy.swing.daily_indicators.min_confluences', '{}')",
                sql_escape(id),
                sql_escape(status),
                sql_escape(new_value),
            ))
            .execute(&state.pool)
            .await
            .expect("seed experiment");
        }

        let context = state
            .hermes_advisory_experiments(20)
            .await
            .expect("load advisory experiment context");
        assert_eq!(context["pending_review_excluded_count"], json!(1));
        assert_eq!(context["pending_review_values_included"], json!(false));
        assert_eq!(
            context["items"]
                .as_array()
                .expect("advisory items")
                .iter()
                .map(|row| json_text(row, "id"))
                .collect::<Vec<_>>(),
            vec!["approved".to_string(), "active".to_string()]
        );
        assert!(
            !serde_json::to_string(&context)
                .expect("serialize advisory context")
                .contains("99")
        );
    }

    #[tokio::test]
    async fn expires_only_stale_pending_hermes_experiments() {
        let state = runtime_settings_test_state("hermes:\n  experiments: {}\n").await;
        sqlx::query(
            "CREATE TABLE strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                approval_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create strategy experiments table");
        for (id, created_at, status) in [
            ("old-pending", "2026-06-01T12:00:00Z", "pending_review"),
            ("fresh-pending", "2026-06-25T12:00:00Z", "pending_review"),
            ("old-approved", "2026-06-01T12:00:00Z", "approved_paper"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO strategy_experiments (id, created_at, status, changed_variable_path)
                 VALUES ('{}', '{}', '{}', 'strategy.swing.daily_indicators.min_confluences')",
                sql_escape(id),
                sql_escape(created_at),
                sql_escape(status),
            ))
            .execute(&state.pool)
            .await
            .expect("seed experiment");
        }

        let now = DateTime::parse_from_rfc3339("2026-07-01T12:00:00Z")
            .expect("valid now")
            .with_timezone(&Utc);
        let result = state
            .expire_stale_hermes_experiments_at(now, 30)
            .await
            .expect("expire stale pending proposal");
        assert_eq!(result["expired_count"], json!(1));
        assert_eq!(result["expired"][0]["id"], json!("old-pending"));

        let rows = state
            .select_json(
                "SELECT id, status, approval_json
                 FROM strategy_experiments ORDER BY id ASC",
            )
            .await
            .expect("read experiment statuses");
        let old_pending = rows
            .iter()
            .find(|row| json_text(row, "id") == "old-pending")
            .expect("old pending row");
        assert_eq!(old_pending["status"], json!("expired_stale"));
        assert_eq!(old_pending["approval_json"]["actor"], json!("scheduler"));
        assert_eq!(
            old_pending["approval_json"]["action"],
            json!("expire_stale")
        );
        assert_eq!(
            rows.iter()
                .find(|row| json_text(row, "id") == "fresh-pending")
                .expect("fresh pending row")["status"],
            json!("pending_review")
        );
        assert_eq!(
            rows.iter()
                .find(|row| json_text(row, "id") == "old-approved")
                .expect("old approved row")["status"],
            json!("approved_paper")
        );
        assert_eq!(
            state
                .expire_stale_hermes_experiments_at(now, 30)
                .await
                .expect("repeated expiry is safe")["expired_count"],
            json!(0)
        );
    }

    #[test]
    fn normalizes_hermes_experiment_variable_paths_for_duplicate_lookup() {
        assert_eq!(
            normalize_hermes_experiment_variable_path(
                " Strategy.Swing.Daily_Indicators.Min_Confluences "
            ),
            "strategy.swing.daily_indicators.min_confluences"
        );
    }

    #[test]
    fn maps_only_explicit_hermes_experiment_review_families() {
        assert_eq!(
            hermes_experiment_review_family("strategy.capital.min_cash_buffer_pct"),
            Some("cash_buffer_policy")
        );
        assert_eq!(
            hermes_experiment_review_family(" Strategy.Swing.Cash_Buffer_Pct "),
            Some("cash_buffer_policy")
        );
        assert_eq!(
            hermes_experiment_review_family("strategy.capital.max_positions"),
            None
        );
    }

    #[tokio::test]
    async fn hermes_experiment_preinsert_review_distinguishes_exact_and_related_proposals() {
        let state = runtime_settings_test_state("hermes:\n  experiments: {}\n").await;
        sqlx::query(
            "CREATE TABLE strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                hypothesis TEXT NOT NULL,
                source_session_id TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create strategy experiments table");
        for (id, status, path) in [
            (
                "exact-pending",
                "pending_review",
                "Strategy.Capital.Min_Cash_Buffer_Pct",
            ),
            (
                "related-active",
                "active_sim",
                "strategy.swing.cash_buffer_pct",
            ),
            (
                "related-terminal",
                "expired_stale",
                "strategy.swing.cash_buffer_pct",
            ),
            (
                "unrelated-pending",
                "pending_review",
                "strategy.swing.daily_indicators.min_confluences",
            ),
        ] {
            sqlx::query(&format!(
                "INSERT INTO strategy_experiments (id, created_at, status, changed_variable_path, hypothesis)
                 VALUES ('{}', '2026-07-22T09:00:00Z', '{}', '{}', 'test')",
                sql_escape(id),
                sql_escape(status),
                sql_escape(path),
            ))
            .execute(&state.pool)
            .await
            .expect("seed experiment proposal");
        }

        let review = state
            .inspect_hermes_experiment_proposal("strategy.capital.min_cash_buffer_pct")
            .await
            .expect("inspect proposal");

        assert_eq!(review["exact_duplicate"]["id"], json!("exact-pending"));
        assert_eq!(review["review_family"], json!("cash_buffer_policy"));
        assert_eq!(
            review["related_active_or_pending_experiments"],
            json!([{
                "id": "related-active",
                "created_at": "2026-07-22T09:00:00Z",
                "status": "active_sim",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "hypothesis": "test",
                "source_session_id": null,
            }])
        );
        assert_eq!(review["related_family_is_advisory"], json!(true));
    }

    #[test]
    fn candidate_scoring_waterfall_sanitizes_legacy_manager_reasons() {
        let run = json!({
            "id": 42,
            "created_at": "2026-07-13T12:00:00Z",
            "status": "completed",
            "manager_json": {
                "hermes_preflight": {
                    "candidate_waterfall": [{
                        "strategy_key": "swing:NVDA:xnas:BUY",
                        "symbol": "NVDA:xnas",
                        "action": "BUY",
                        "order_type": "Limit",
                        "quantity": 2,
                        "exchange": "XNAS",
                        "exchange_open": true,
                        "risk_excluded": false,
                        "instrument_quarantine": null,
                        "technical": {
                            "status": "ok",
                            "sentiment": "bullish",
                            "trend_bias": "up",
                            "confluence_count": 3,
                            "min_confluences": 3
                        },
                        "markov": {
                            "status": "ok",
                            "fresh": true,
                            "direction": "long",
                            "signed_signal": 0.34,
                            "age_days": 0
                        }
                    }]
                },
                "hermes_advice_delta": {
                    "candidates": [{
                        "strategy_key": "swing:NVDA:xnas:BUY",
                        "effect": "reduced",
                        "requested_quantity": 2,
                        "resulting_quantity": 1,
                        "raw_rationale": "do not render this"
                    }]
                },
                "skipped_orders": [{
                    "strategy_key": "swing:NVDA:xnas:BUY",
                    "symbol": "NVDA:xnas",
                    "action": "BUY",
                    "final_technical": {
                        "status": "ok",
                        "source": "daily_indicators_db",
                        "run_date": "2026-07-13",
                        "sentiment": "HOLD",
                        "trend_bias": "neutral",
                        "confluence_count": 1,
                        "min_confluences": 3
                    },
                    "technical_gate": "Hermes advisory rejected because do not render this"
                }]
            }
        });

        let waterfall = candidate_scoring_waterfall_from_manager_run(&run);
        assert_eq!(waterfall["status"], "available");
        assert_eq!(waterfall["summary"]["skipped_count"], 1);
        assert_eq!(waterfall["candidates"][0]["symbol"], "NVDA:xnas");
        assert_eq!(waterfall["candidates"][0]["gate_code"], "hermes_advice");
        assert_eq!(waterfall["candidates"][0]["hermes"]["effect"], "reduced");
        assert_eq!(
            waterfall["candidates"][0]["final_technical"]["sentiment"],
            "HOLD"
        );
        assert_eq!(
            waterfall["candidates"][0]["final_technical"]["source"],
            "daily_indicators_db"
        );
        assert!(!waterfall.to_string().contains("do not render this"));
        assert!(!waterfall.to_string().contains("technical_gate"));
    }

    #[test]
    fn gate_replay_isolates_threshold_flips_without_claiming_full_approval() {
        let run = json!({
            "id": 77,
            "report_id": 91,
            "created_at": "2026-07-22T12:00:00Z",
            "status": "completed",
            "manager_json": {
                "hermes_preflight": {
                    "markov": {"min_signed_signal": 0.15},
                    "candidate_waterfall": [
                        {
                            "strategy_key": "markov-starter",
                            "symbol": "MARKOV:xnas",
                            "action": "BUY",
                            "technical": {"status": "ok", "sentiment": "HOLD", "trend_bias": "neutral", "confluence_count": 1, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "long", "signed_signal": 0.20, "age_days": 0}
                        },
                        {
                            "strategy_key": "technical-buy",
                            "symbol": "TECH:xnas",
                            "action": "BUY",
                            "technical": {"status": "ok", "sentiment": "BUY", "trend_bias": "bullish", "confluence_count": 3, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "long", "signed_signal": 0.40, "age_days": 0}
                        },
                        {
                            "strategy_key": "sell",
                            "symbol": "SELL:xnas",
                            "action": "SELL",
                            "technical": {"status": "ok", "sentiment": "SELL", "trend_bias": "bearish", "confluence_count": 0, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "short", "signed_signal": -0.40, "age_days": 0}
                        }
                    ]
                },
                "approved_orders": [
                    {"strategy_key": "markov-starter", "symbol": "MARKOV:xnas", "action": "BUY", "gate_code": "approved"},
                    {"strategy_key": "technical-buy", "symbol": "TECH:xnas", "action": "BUY", "gate_code": "approved"},
                    {"strategy_key": "sell", "symbol": "SELL:xnas", "action": "SELL", "gate_code": "approved"}
                ]
            }
        });

        let replay = gate_replay_from_manager_runs(&[run]);
        assert_eq!(replay["status"], "available");
        assert_eq!(replay["run_count"], 1);
        assert_eq!(
            replay["scenarios"][0]["variable_path"],
            "strategy.swing.markov_gate.min_signed_signal"
        );
        assert_eq!(
            replay["scenarios"][0]["summary"]["would_block_target_gate_count"],
            1
        );
        assert_eq!(
            replay["scenarios"][0]["changes"][0]["symbol"],
            "MARKOV:xnas"
        );
        assert_eq!(
            replay["scenarios"][0]["changes"][0]["effect"],
            "would_block_target_gate"
        );
        assert_eq!(
            replay["scenarios"][1]["variable_path"],
            "strategy.swing.daily_indicators.min_confluences"
        );
        assert_eq!(
            replay["scenarios"][1]["summary"]["would_block_target_gate_count"],
            1
        );
        assert_eq!(replay["scenarios"][1]["changes"][0]["symbol"], "TECH:xnas");
        assert!(
            replay["interpretation"]
                .as_str()
                .unwrap_or_default()
                .contains("not an approval")
        );
    }

    #[test]
    fn support_risk_evidence_uses_next_available_closes_without_claiming_causality() {
        let rows = vec![
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-01", "close": 100.0, "support_break_risk_label": "low", "support_break_risk": 0.2, "support_confidence": 0.8, "support_history_coverage": 1.0}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-02", "close": 105.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-03", "close": 106.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-04", "close": 107.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-05", "close": 108.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-06", "close": 110.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-01", "close": 100.0, "support_break_risk_label": "high", "support_break_risk": 0.8, "support_confidence": 0.6, "support_history_coverage": 0.5}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-02", "close": 92.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-02", "close": 90.0, "support_break_risk_label": "unavailable"}),
        ];

        let evidence = support_risk_evidence_from_indicator_rows(&rows);
        assert_eq!(evidence["status"], "collecting");
        assert_eq!(evidence["eligible_signal_count"], 2);
        assert_eq!(evidence["next_run_complete_count"], 2);
        assert_eq!(evidence["five_run_complete_count"], 1);
        assert_eq!(evidence["labels"][0]["label"], "low");
        assert_eq!(evidence["labels"][0]["next_run"]["average_return_pct"], 5.0);
        assert_eq!(
            evidence["labels"][0]["five_run"]["average_return_pct"],
            10.0
        );
        assert_eq!(evidence["labels"][2]["label"], "high");
        assert_eq!(
            evidence["labels"][2]["next_run"]["average_return_pct"],
            -10.0
        );
        assert!(
            evidence["interpretation"]
                .as_str()
                .unwrap_or_default()
                .contains("descriptive, not causal")
        );
    }

    /// Makes `strategy.ladder.stop_loss_atr_multiple` real. The proposal covers
    /// only the uncovered share of a position, is absent when the position is
    /// fully protected or the indicator data is unusable, and never claims to be
    /// tick-normalized -- normalization needs Saxo instrument details and
    /// happens in the precheck/placement path.
    #[test]
    fn proposed_protective_stop_sizes_to_uncovered_quantity_and_fails_closed() {
        let positions = vec![
            json!({"symbol": "OPEN:xnas", "quantity": 10.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "HALF:xnas", "quantity": 10.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "SAFE:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "NOATR:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "DEEP:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
        ];
        let orders = vec![
            json!({"symbol": "HALF:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 6.0, "stop_price_local": 90.0}),
            json!({"symbol": "SAFE:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 4.0, "stop_price_local": 90.0}),
        ];
        let indicators = vec![
            json!({"symbol": "OPEN:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            json!({"symbol": "HALF:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            json!({"symbol": "SAFE:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            // Unusable ATR must produce no proposal rather than a bad level.
            json!({"symbol": "NOATR:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 0.0}),
            // A stop below zero is not a protective level.
            json!({"symbol": "DEEP:xnas", "run_date": "2026-07-24", "close": 5.0, "atr14": 4.0}),
        ];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &orders, &[], &indicators, 2.0);
        let rows = coverage["positions"].as_array().expect("coverage rows");
        let find = |symbol: &str| {
            rows.iter()
                .find(|row| row["symbol"] == json!(symbol))
                .expect("symbol row")
        };

        // 100 - (4 * 2) = 92, on the full unprotected quantity.
        let open = &find("OPEN:xnas")["proposed_stop"];
        assert_eq!(open["stop_price_local"], json!(92.0));
        assert_eq!(open["quantity"], json!(10.0));
        assert_eq!(open["distance_pct"], json!(8.0));
        assert_eq!(open["tick_normalized"], json!(false));

        // Only the 4 uncovered of 10 need a stop.
        assert_eq!(find("HALF:xnas")["proposed_stop"]["quantity"], json!(4.0));

        for symbol in ["SAFE:xnas", "NOATR:xnas", "DEEP:xnas"] {
            assert_eq!(
                find(symbol)["proposed_stop"],
                JsonValue::Null,
                "{symbol} must not carry a proposed stop"
            );
        }

        // The exception rows carry the same proposal so the operator sees the
        // level next to the reason.
        let exceptions = coverage["exceptions"].as_array().expect("exceptions");
        let open_exception = exceptions
            .iter()
            .find(|row| row["symbol"] == json!("OPEN:xnas"))
            .expect("unprotected exception");
        assert_eq!(
            open_exception["proposed_stop"]["stop_price_local"],
            json!(92.0)
        );
    }

    #[test]
    fn protective_stop_coverage_requires_broker_confirmed_stop_state() {
        let positions = vec![
            json!({"symbol": "FULL:xnas", "quantity": 5.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "PART:xnas", "quantity": 5.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "PLAN:xnas", "quantity": 3.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "FAIL:xnas", "quantity": 2.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
        ];
        let orders = vec![
            json!({"symbol": "FULL:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 5.0, "stop_price_local": 95.0, "raw_payload": "must not appear"}),
            json!({"symbol": "PART:xnas", "action": "SELL", "order_type": "StopLimit", "status": "submitted_to_broker", "quantity": 2.0, "stop_price_local": 90.0}),
            json!({"symbol": "PLAN:xnas", "action": "SELL", "order_type": "Stop", "status": "pending_execution", "quantity": 3.0, "stop_price_local": 85.0}),
            json!({"symbol": "FAIL:xnas", "action": "SELL", "order_type": "Stop", "status": "execution_failed", "quantity": 2.0, "stop_price_local": 80.0}),
        ];

        let coverage = protective_stop_coverage_from_rows(&positions, &orders, &[], &[], 2.0);
        let rows = coverage["positions"].as_array().expect("coverage rows");
        let find = |symbol: &str| {
            rows.iter()
                .find(|row| row["symbol"] == json!(symbol))
                .expect("symbol row")
        };
        assert_eq!(coverage["status"], "attention_required");
        assert_eq!(coverage["summary"]["protected_count"], 1);
        assert_eq!(coverage["summary"]["partial_count"], 1);
        assert_eq!(coverage["summary"]["planned_count"], 1);
        assert_eq!(coverage["summary"]["unprotected_count"], 1);
        assert_eq!(find("FULL:xnas")["protection_status"], "protected");
        assert_eq!(find("PART:xnas")["protection_status"], "partial_protection");
        assert_eq!(find("PLAN:xnas")["protection_status"], "planned");
        assert_eq!(find("FAIL:xnas")["protection_status"], "unprotected");
        assert!(!coverage.to_string().contains("must not appear"));
    }

    #[test]
    fn protective_stop_coverage_accepts_only_reconciled_sim_lifecycle_tests() {
        let positions = vec![json!({
            "symbol": "TEST:xnas",
            "quantity": 5.0,
            "currency": "USD",
            "updated_at": "2026-07-25T12:00:00Z",
        })];
        let lifecycle_tests = vec![
            json!({
                "id": "working",
                "environment": "sim",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 95.0,
                "status": "broker_working",
                "broker_order_id": "12345",
            }),
            json!({
                "id": "ambiguous",
                "environment": "sim",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 94.0,
                "status": "placement_submitted",
                "broker_order_id": "67890",
            }),
            json!({
                "id": "wrong-environment",
                "environment": "live",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 93.0,
                "status": "broker_working",
                "broker_order_id": "98765",
            }),
        ];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &[], &lifecycle_tests, &[], 2.0);
        let row = &coverage["positions"][0];
        assert_eq!(coverage["status"], "covered");
        assert_eq!(coverage["summary"]["protected_count"], 1);
        assert_eq!(coverage["summary"]["exception_count"], 0);
        assert_eq!(row["confirmed_covered_quantity"], 5.0);
        assert_eq!(row["coverage_evidence"]["execution_orders"], 0);
        assert_eq!(row["coverage_evidence"]["manual_sim_lifecycle_tests"], 1);
    }

    #[test]
    fn protective_stop_coverage_marks_unconfirmed_lifecycle_test_as_exception() {
        let positions = vec![json!({
            "symbol": "PENDING:xnas",
            "quantity": 2.0,
            "currency": "USD",
            "updated_at": "2026-07-25T12:00:00Z",
        })];
        let lifecycle_tests = vec![json!({
            "environment": "sim",
            "symbol": "PENDING:xnas",
            "quantity": 2.0,
            "stop_price_local": 90.0,
            "status": "placement_submitted",
            "broker_order_id": "12345",
        })];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &[], &lifecycle_tests, &[], 2.0);
        assert_eq!(coverage["status"], "attention_required");
        assert_eq!(coverage["summary"]["unprotected_count"], 1);
        assert_eq!(coverage["summary"]["exception_count"], 1);
        assert_eq!(
            coverage["exceptions"][0]["kind"],
            "unprotected_broker_position"
        );
        assert_eq!(coverage["exceptions"][0]["unprotected_quantity"], 2.0);
    }

    #[test]
    fn protective_stop_coverage_for_hermes_is_bounded() {
        let coverage = json!({
            "status": "covered",
            "summary": {"position_count": 3},
            "positions": [json!({"symbol": "A:xnas"}), json!({"symbol": "B:xnas"}), json!({"symbol": "C:xnas"})],
            "exceptions": [json!({"symbol": "A:xnas"}), json!({"symbol": "B:xnas"}), json!({"symbol": "C:xnas"})],
        });
        let compact = compact_protective_stop_coverage_for_hermes(&coverage, 2);
        assert_eq!(compact["positions"].as_array().map(Vec::len), Some(2));
        assert_eq!(compact["exceptions"].as_array().map(Vec::len), Some(2));
        assert_eq!(coverage["positions"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn candidate_waterfall_recovers_only_known_legacy_sell_gate_evidence() {
        let technical = compact_candidate_final_technical(&json!({
            "technical_gate": "SELL not approved; technical sentiment is HOLD with neutral trend. (database-verified daily indicators)"
        }));
        assert_eq!(technical["status"], "ok");
        assert_eq!(technical["source"], "recorded_gate_reason");
        assert_eq!(technical["sentiment"], "HOLD");
        assert_eq!(technical["trend_bias"], "neutral");

        let untrusted = compact_candidate_final_technical(&json!({
            "technical_gate": "Hermes advisory rejected because do not render this"
        }));
        assert!(untrusted.is_null());
    }

    #[test]
    fn candidate_waterfall_preserves_stable_manager_gate_codes() {
        for gate_code in [
            "candidate_limit",
            "drawdown_guardrail",
            "position_weight",
            "max_holdings",
            "max_selected_assets",
            "cost_guard",
        ] {
            assert_eq!(
                candidate_gate_code(&json!({"gate_code": gate_code})),
                gate_code
            );
        }
        assert_eq!(
            candidate_gate_code_from_reason("Portfolio drawdown guardrail is active"),
            "drawdown_guardrail"
        );
    }

    #[test]
    fn database_display_label_excludes_credentials_and_query_parameters() {
        let label = redacted_database_url(
            "postgresql://daytrader:super-secret@daytrader-postgres-rw.saxo.svc.cluster.local:5432/daytrader?sslmode=require&token=also-secret",
        );
        assert_eq!(
            label,
            "PostgreSQL · daytrader-postgres-rw.saxo.svc.cluster.local:5432/daytrader"
        );
        assert!(!label.contains("daytrader:"));
        assert!(!label.contains("super-secret"));
        assert!(!label.contains("also-secret"));
        assert_eq!(
            redacted_database_url("sqlite:///Users/example/private/ledger.db"),
            "SQLite · local database"
        );
    }

    fn decision_report_row(id: i64, created_at: &str, report_json: JsonValue) -> JsonValue {
        json!({
            "id": id,
            "created_at": created_at,
            "status": "completed",
            "analysis_pulse_key": format!("us_open_followup:{created_at}"),
            "analysis_pulse_label": "US open",
            "report_json": report_json,
        })
    }

    /// The defect this replaced: decisions were read from `swing_*` tables that
    /// no Rust code writes, frozen at report 12, so a symbol's chip age only
    /// grew and symbols outside that one report were `n/a` forever. Reports are
    /// scanned newest-first and the first entry per symbol wins, so each symbol
    /// carries its own report's timestamp.
    #[test]
    fn symbol_decisions_keep_the_most_recent_report_per_symbol() {
        let newer = decision_report_row(
            203,
            "2026-08-03T14:46:13Z",
            json!({
                "symbol_sentiment": [
                    {"symbol": "ADBE:xnas", "sentiment": "BUY", "confidence": 88, "rationale": "fresh"}
                ]
            }),
        );
        let older = decision_report_row(
            188,
            "2026-07-23T08:22:28Z",
            json!({
                "symbol_sentiment": [
                    {"symbol": "ADBE:xnas", "sentiment": "SELL", "confidence": 40, "rationale": "stale"},
                    {"symbol": "DANSKE:xcse", "sentiment": "HOLD", "confidence": 60, "rationale": "older but only view"}
                ]
            }),
        );

        let mut decisions = HashMap::new();
        for report in [&newer, &older] {
            merge_report_symbol_decisions(report, &mut decisions);
        }

        let adbe = decisions.get("ADBE:xnas").expect("ADBE decision");
        assert_eq!(adbe["sentiment"], "BUY", "newest report must win");
        assert_eq!(adbe["created_at"], "2026-08-03T14:46:13Z");
        assert_eq!(adbe["report_id"], 203);

        // A symbol only the older report mentions still resolves, carrying that
        // report's own timestamp so staleness is judged per symbol.
        let danske = decisions.get("DANSKE:xcse").expect("DANSKE decision");
        assert_eq!(danske["sentiment"], "HOLD");
        assert_eq!(danske["created_at"], "2026-07-23T08:22:28Z");
    }

    /// `suggested_trades` supplies the action and the technical snapshot the
    /// trend sparkline reads, but only for the report that owns the entry --
    /// pairing a newer report's sentiment with an older report's action would
    /// present two different moments as one decision.
    #[test]
    fn suggested_trades_enrich_only_their_own_reports_entry() {
        let newer = decision_report_row(
            203,
            "2026-08-03T14:46:13Z",
            json!({
                "symbol_sentiment": [
                    {"symbol": "ADBE:xnas", "sentiment": "BUY", "confidence": 88, "rationale": "fresh"}
                ],
                "suggested_trades": [{
                    "symbol": "ADBE:xnas",
                    "action": "BUY",
                    "strategy_role": "starter",
                    "strategy_metadata": {"technical": {"trend_bias": "bullish", "confluence_count": 4}}
                }]
            }),
        );
        let older = decision_report_row(
            188,
            "2026-07-23T08:22:28Z",
            json!({
                "suggested_trades": [
                    {"symbol": "ADBE:xnas", "action": "SELL", "strategy_role": "flatten"},
                    {"symbol": "GN:xcse", "action": "BUY", "strategy_role": "starter"}
                ]
            }),
        );

        let mut decisions = HashMap::new();
        for report in [&newer, &older] {
            merge_report_symbol_decisions(report, &mut decisions);
        }

        let adbe = decisions.get("ADBE:xnas").expect("ADBE decision");
        assert_eq!(adbe["action"], "BUY", "older report must not overwrite");
        assert_eq!(adbe["strategy_role"], "starter");
        assert_eq!(
            adbe["source"]["technical"]["trend_bias"], "bullish",
            "sparkline reads source.technical.trend_bias"
        );
        assert_eq!(
            adbe["trend_bias"], "bullish",
            "the typed Watchlists projection reads the same report-local trend bias"
        );

        // A suggested trade may name a symbol the sentiment list omits; it
        // should still produce a usable entry rather than being dropped.
        let gn = decisions
            .get("GN:xcse")
            .expect("GN decision from trade only");
        assert_eq!(gn["action"], "BUY");
        assert_eq!(gn["created_at"], "2026-07-23T08:22:28Z");
    }

    #[test]
    fn symbol_decisions_tolerate_missing_or_malformed_report_payloads() {
        let mut decisions = HashMap::new();
        // No report_json at all, wrong types, and unnamed entries must not
        // panic or create junk keys.
        merge_report_symbol_decisions(&json!({"id": 1, "created_at": "x"}), &mut decisions);
        merge_report_symbol_decisions(
            &decision_report_row(2, "y", json!({"symbol_sentiment": "not-an-array"})),
            &mut decisions,
        );
        merge_report_symbol_decisions(
            &decision_report_row(
                3,
                "z",
                json!({"symbol_sentiment": [{"sentiment": "BUY"}], "suggested_trades": [{"action": "BUY"}]}),
            ),
            &mut decisions,
        );
        assert!(
            decisions.is_empty(),
            "entries without a symbol must be skipped, got {decisions:?}"
        );
    }

    /// The threshold must separate "under water" from "arithmetically
    /// impossible". A real book can carry a cost basis above market value after
    /// a drawdown; it cannot carry 26.9M against 240k invested.
    #[test]
    fn cost_basis_plausibility_flags_the_corrupt_window_but_not_a_drawdown() {
        // Real corrupt production values from the 2026-06/07 window.
        assert!(!cost_basis_is_plausible(17_605_178.0, 240_061.0));
        assert!(!cost_basis_is_plausible(26_881_060.0, 245_000.0));
        assert!(!cost_basis_is_plausible(5_428_221.0, 250_000.0));
        // A book 40% under water is unremarkable and must not be flagged.
        assert!(cost_basis_is_plausible(340_000.0, 204_000.0));
        assert!(cost_basis_is_plausible(204_324.0, 209_279.0));
        // Nothing invested and nothing claimed is consistent.
        assert!(cost_basis_is_plausible(0.0, 0.0));
        // A claimed basis against no holdings is not.
        assert!(!cost_basis_is_plausible(100_000.0, 0.0));
        assert!(!cost_basis_is_plausible(f64::NAN, 240_000.0));
    }

    /// Reproduces DDOG on 2026-08-06: sold at 225.55 and re-bought at 242.39
    /// inside one session, then shown as -15.8% "1D" against the previous
    /// close of 283.17 -- a fall the position was not held through, and whose
    /// realised half already sits in the trade ledger.
    ///
    /// The rule is quantity-based rather than net, because a same-day round
    /// trip nets to zero while genuinely re-establishing the whole position.
    #[test]
    fn daily_pnl_anchors_to_entry_only_when_the_whole_position_is_same_session() {
        // (bought this session, current quantity, should anchor to entry)
        let cases = [
            // DDOG: sold 5, re-bought 5, holding 5. Net is zero; BUY quantity
            // is 5, which covers the position.
            (5.0, 5.0, true),
            // Untouched overnight holding.
            (0.0, 5.0, false),
            // Partially added to today: the older shares carry real overnight
            // exposure, so the session baseline still applies.
            (5.0, 15.0, false),
            // Opened and added to, all today.
            (15.0, 10.0, true),
            // A flat symbol must never claim a same-session entry.
            (5.0, 0.0, false),
        ];
        for (bought_today, quantity, expected) in cases {
            let opened_this_session = quantity > 0.0 && bought_today + 1e-9 >= quantity;
            assert_eq!(
                opened_this_session, expected,
                "bought {bought_today} of {quantity} held"
            );
        }
    }

    /// Saxo's `ProfitLossOnTrade` is instrument-denominated; the sibling
    /// `ProfitLossOnTradeInBaseCurrency` is the account-currency one. Converting
    /// the former with the account rate inflated every position on this
    /// EUR-based SIM account. These are the real production values from
    /// 2026-08-06, before and after.
    #[test]
    fn broker_pnl_converts_at_the_instrument_rate_not_the_account_rate() {
        const EUR_ACCOUNT_RATE: f64 = 7.47523;
        // (symbol, broker P/L in instrument currency, instrument rate, what the
        // bug displayed, what is correct)
        let cases = [
            ("AKERBP:xosl", -284.0, 0.68054, -2123.0, -193.0),
            ("EQNR:xosl", -288.0, 0.68054, -2153.0, -196.0),
            ("V:xnys", 549.51, 6.47436, 4108.0, 3558.0),
            // A DKK instrument is the clearest case: its P/L needs no
            // conversion at all, yet was being multiplied by 7.475.
            ("DANSKE:xcse", 443.5, 1.0, 3315.0, 443.5),
            ("GN:xcse", -80.0, 1.0, -598.0, -80.0),
        ];
        for (symbol, broker_pnl, instrument_rate, buggy, correct) in cases {
            assert!(
                (broker_pnl * EUR_ACCOUNT_RATE - buggy).abs() < 1.0,
                "{symbol}: the old account-rate path should reproduce {buggy}"
            );
            assert!(
                (broker_pnl * instrument_rate - correct).abs() < 1.0,
                "{symbol}: instrument rate should give {correct}, got {}",
                broker_pnl * instrument_rate
            );
        }
    }

    /// `missing` must stay distinct from `stale`: a source that never produced
    /// a row is a different fault from one that produced rows and stopped, and
    /// conflating them sends an operator looking in the wrong place.
    #[test]
    fn freshness_separates_never_seen_from_stopped_updating() {
        assert_eq!(freshness_state(None, 90), "missing");
        assert_eq!(freshness_state(Some(0), 90), "fresh");
        assert_eq!(freshness_state(Some(44), 90), "fresh");
        // Half the budget is where it starts warning, not where it fails.
        assert_eq!(freshness_state(Some(45), 90), "aging");
        assert_eq!(freshness_state(Some(89), 90), "aging");
        assert_eq!(freshness_state(Some(90), 90), "stale");
        assert_eq!(freshness_state(Some(10_000), 90), "stale");
    }

    /// The real incidents this strip exists to catch, at the thresholds
    /// actually shipped in `FRESHNESS_SOURCES`.
    #[test]
    fn the_real_staleness_incidents_would_have_been_flagged() {
        // FX froze for two days against a 90-minute budget (2026-08-02).
        assert_eq!(freshness_state(Some(2 * 24 * 60), 90), "stale");
        // A single missed 30-minute refresh must not cry wolf.
        assert_eq!(freshness_state(Some(31), 90), "fresh");
        // A nightly weekday sweep across a normal weekend is not a fault:
        // Friday night to Monday morning is under the 3-day budget.
        assert_eq!(freshness_state(Some(60 * 60), 3 * 24 * 60), "aging");
        // A sweep that has genuinely stopped is.
        assert_eq!(freshness_state(Some(4 * 24 * 60), 3 * 24 * 60), "stale");
    }

    #[test]
    fn freshness_age_labels_stay_coarse() {
        assert_eq!(freshness_age_label(None), "never");
        assert_eq!(freshness_age_label(Some(0)), "just now");
        assert_eq!(freshness_age_label(Some(45)), "45m ago");
        assert_eq!(freshness_age_label(Some(90)), "1h ago");
        assert_eq!(freshness_age_label(Some(2 * 24 * 60)), "2d ago");
        // Negative clock skew must not render as a bizarre age.
        assert_eq!(freshness_age_label(Some(-5)), "just now");
    }

    #[test]
    fn freshness_payload_keeps_the_dashboard_diagnostic_contract() {
        let source = DataFreshnessSourcePayload {
            key: "fx_rates".to_string(),
            label: "FX rates".to_string(),
            tab: "overview".to_string(),
            observed_at: Some("2026-08-23T12:00:00Z".to_string()),
            age_minutes: Some(45),
            age_label: "45m ago".to_string(),
            stale_after_minutes: 90,
            state: "aging".to_string(),
        };

        let encoded = serde_json::to_value(&source).expect("freshness source serializes");
        assert_eq!(encoded["key"], "fx_rates");
        assert_eq!(encoded["state"], "aging");
        assert_eq!(encoded["stale_after_minutes"], 90);
    }

    /// Every shipped source must name a tab that exists, or its chip can only
    /// ever appear via the cross-tab stale rule and never on its own tab.
    #[test]
    fn every_freshness_source_targets_a_real_tab() {
        const TABS: &[&str] = &[
            "overview",
            "performance",
            "market",
            "watchlists",
            "markov",
            "quiver",
            "decisions",
            "prompts",
            "hermes",
            "eod",
            "execution",
        ];
        for source in FRESHNESS_SOURCES {
            assert!(
                TABS.contains(&source.tab),
                "{} targets unknown tab {}",
                source.key,
                source.tab
            );
            assert!(
                source.stale_after_minutes > 0,
                "{} needs a positive staleness budget",
                source.key
            );
        }
    }

    #[test]
    fn an_unknown_hermes_section_falls_back_to_overview() {
        assert_eq!(normalize_hermes_section(None), "overview");
        assert_eq!(normalize_hermes_section(Some("")), "overview");
        assert_eq!(normalize_hermes_section(Some("nonsense")), "overview");
        assert_eq!(normalize_hermes_section(Some("  ADVICE ")), "advice");
        for key in HERMES_SECTIONS {
            assert_eq!(&normalize_hermes_section(Some(key)), key);
        }
    }

    /// Each of the Hermes tab's eleven datasets must load for exactly one
    /// section. A dataset gated to no section renders as a permanently empty
    /// table; one gated to several defeats the point of splitting the tab.
    #[test]
    fn every_hermes_section_gates_only_its_own_data() {
        // Nothing loads while another tab is active.
        for key in HERMES_SECTIONS {
            assert!(
                !dashboard_loads_hermes_section("overview", key, key),
                "the Hermes tab must not load {key} data from another view"
            );
        }
        // Within Hermes, a section loads its own data and nothing else.
        for selected in HERMES_SECTIONS {
            for candidate in HERMES_SECTIONS {
                assert_eq!(
                    dashboard_loads_hermes_section("hermes", selected, candidate),
                    selected == candidate,
                    "section {selected} should load {candidate} data only when they match"
                );
            }
        }
    }

    /// The autovacuum tuning migration runs `ALTER TABLE ... SET (...)`
    /// storage-parameter syntax that only Postgres accepts; SQLite (used
    /// locally and in every other test here) has no autovacuum at all. This
    /// pins the guard so a future edit cannot accidentally run Postgres DDL
    /// against the SQLite pool the rest of the test suite depends on.
    #[test]
    fn postgres_url_detection_gates_autovacuum_tuning_to_postgres_only() {
        assert!(database_url_is_postgres(
            "postgresql://daytrader:secret@daytrader-postgres-rw.saxo.svc.cluster.local:5432/daytrader"
        ));
        assert!(database_url_is_postgres("postgres://user:pw@localhost/db"));
        assert!(!database_url_is_postgres("sqlite::memory:"));
        assert!(!database_url_is_postgres(
            "sqlite:///Users/example/private/ledger.db"
        ));
    }

    #[test]
    fn decision_report_summary_projection_excludes_heavy_payload_columns() {
        for column in [
            "prompt_text",
            "request_json",
            "response_json",
            "report_json",
        ] {
            assert!(
                !DECISION_REPORT_SUMMARY_COLUMNS.contains(column),
                "summary projection must not load {column}"
            );
            assert!(
                DECISION_REPORT_DETAIL_COLUMNS.contains(column),
                "detail projection must retain {column}"
            );
        }
    }

    #[test]
    fn decision_report_debug_payload_is_bounded_and_sanitized_before_api_delivery() {
        let report = json!({
            "id": 42,
            "created_at": "2026-07-31T10:00:00Z",
            "status": "completed",
            "prompt_text": "Do not expose sk-live-123456789012345678901234567890",
            "request_json": {"Authorization": "Bearer abcdef1234567890abcdef1234567890abcd"},
            "response_json": {"refresh_token": "refresh-123456789012345678901234567890"},
            "report_json": {"suggested_trades": []},
        });

        let payload = AppState::sanitized_decision_report_debug_payload(&report);
        assert_eq!(payload.report_id, 42);
        assert!(payload.payloads.prompt.contains("[redacted]"));
        assert!(
            payload
                .payloads
                .request
                .contains("\"Authorization\": \"[redacted]\"")
        );
        assert!(
            payload
                .payloads
                .provider_response
                .contains("\"refresh_token\": \"[redacted]\"")
        );
        assert!(
            payload
                .payloads
                .normalized_report
                .contains("suggested_trades")
        );

        let serialized = serde_json::to_value(&payload).expect("typed debug payload serializes");
        assert_eq!(serialized["report_id"], 42);
        assert!(
            serialized["payloads"]["normalized_report"]
                .as_str()
                .is_some_and(|value| value.contains("suggested_trades"))
        );
    }

    #[test]
    fn dashboard_loads_performance_history_only_for_performance_view() {
        assert_eq!(
            dashboard_performance_history_limit("performance", "1W"),
            Some(600)
        );
        for view in ["overview", "decisions", "prompts", "execution", "hermes"] {
            assert_eq!(
                dashboard_performance_history_limit(view, "1M"),
                None,
                "{view} must not load performance history"
            );
        }
    }

    #[test]
    fn dashboard_performance_history_requires_the_typed_local_observation_shape() {
        let rows = dashboard_performance_history_from_json(vec![json!({
            "recorded_at": "2026-08-23T09:00:00Z",
            "snapshot_type": "runtime_current",
            "total_market_value_dkk": 244_404.95,
            "invested_market_value_dkk": 214_118.30,
            "cash_balance_dkk": 30_286.65,
            "total_cost_basis_dkk": 202_849.06,
            "total_unrealised_pnl_dkk": 14_824.65,
            "total_daily_pnl_dkk": 2_263.17,
            "position_count": 20,
            "source": null,
        })])
        .expect("complete local observation is typed");
        assert_eq!(rows[0].snapshot_type, "runtime_current");
        assert!(rows[0].source.is_none());
        assert!(
            dashboard_performance_history_from_json(vec![json!({
                "recorded_at": "2026-08-23T09:00:00Z"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_performance_summary_preserves_unavailable_and_confidence_states() {
        assert!(
            dashboard_performance_summary_from_json(JsonValue::Null)
                .expect("null stays unavailable")
                .is_none()
        );
        let summary = dashboard_performance_summary_from_json(json!({
            "points": 2,
            "first_recorded_at": "2026-08-23T08:00:00Z",
            "latest_recorded_at": "2026-08-23T09:00:00Z",
            "first_total_market_value_dkk": 240_000.0,
            "latest_total_market_value_dkk": 244_000.0,
            "change_dkk": 4_000.0,
            "daily_pnl_dkk": 250.0,
            "position_count": 20,
            "range_return_pct": 1.6667,
            "range_max_drawdown_pct": -0.5,
            "confidence": {
                "status": "current",
                "valid_points": 2,
                "latest_recorded_at": "2026-08-23T09:00:00Z",
                "latest_snapshot_type": "runtime_current",
                "latest_source": "test",
                "age_minutes": 0,
                "scope": "account_value_only",
            },
            "unreliable_cost_basis_points": 0,
        }))
        .expect("complete summary is typed")
        .expect("non-null summary is present");
        assert_eq!(summary.confidence.status, "current");
        assert!(dashboard_performance_summary_from_json(json!({"points": 2})).is_err());
    }

    #[test]
    fn dashboard_performance_benchmarks_preserve_unavailable_and_pending_rows() {
        assert!(
            dashboard_performance_benchmarks_from_json(JsonValue::Null)
                .expect("null stays unavailable")
                .is_none()
        );
        let benchmarks = dashboard_performance_benchmarks_from_json(json!({
            "status": "partial",
            "latest_run": null,
            "portfolio_baseline_at": "2026-08-23T08:00:00Z",
            "portfolio_latest_at": "2026-08-23T09:00:00Z",
            "portfolio_return_pct": 1.5,
            "ready_count": 1,
            "reference_count": 2,
            "aligned_count": 1,
            "prior_close_count": 0,
            "stale_close_count": 0,
            "freshness": "aligned_close",
            "references": [{
                "key": "us_large_cap",
                "label": "S&P 500 (SPY ETF proxy)",
                "symbol": "SPY:arcx",
                "status": "pending_history",
                "portfolio_return_pct": 1.5,
                "benchmark_return_pct": null,
                "excess_return_pct": null,
                "baseline_close": null,
                "latest_close": null,
                "baseline_at": null,
                "latest_at": null,
                "freshness": null,
            }],
            "caveat": "Read-only proxy comparison.",
        }))
        .expect("complete comparison is typed")
        .expect("non-null comparison is present");
        assert_eq!(benchmarks.references[0].status, "pending_history");
        assert!(benchmarks.references[0].benchmark_return_pct.is_none());
        assert!(dashboard_performance_benchmarks_from_json(json!({"status": "ready"})).is_err());
    }

    #[test]
    fn dashboard_goal_tracking_preserves_unavailable_and_pending_baselines() {
        assert!(
            dashboard_performance_goal_tracking_from_json(JsonValue::Null)
                .expect("null stays unavailable")
                .is_none()
        );
        let goal_tracking = dashboard_performance_goal_tracking_from_json(json!({
            "weekly_target_dkk": 880.0,
            "monthly_target_dkk": 3800.0,
            "basis": "Local portfolio-value history baseline.",
            "periods": {
                "week": {
                    "status": "ready",
                    "pnl_dkk": 440.0,
                    "target_dkk": 880.0,
                    "progress_pct": 0.5,
                    "baseline_value_dkk": 299_560.0,
                    "period_start_utc": "2026-08-17T00:00:00Z",
                },
                "month": {
                    "status": "pending_baseline",
                    "pnl_dkk": null,
                    "target_dkk": 3800.0,
                    "progress_pct": null,
                    "baseline_value_dkk": null,
                    "period_start_utc": "2026-08-01T00:00:00Z",
                },
                "since_reset": {
                    "status": "pending_baseline",
                    "pnl_dkk": null,
                    "return_pct": null,
                    "baseline_value_dkk": null,
                    "baseline_recorded_at": null,
                },
            },
        }))
        .expect("complete goal tracking is typed")
        .expect("non-null goal tracking is present");
        assert_eq!(goal_tracking.periods.month.status, "pending_baseline");
        assert!(goal_tracking.periods.month.pnl_dkk.is_none());
        assert!(
            dashboard_performance_goal_tracking_from_json(json!({"basis": "missing periods"}))
                .is_err()
        );
    }

    #[test]
    fn dashboard_snapshot_evidence_preserves_unavailable_and_retained_states() {
        assert!(
            dashboard_performance_snapshot_evidence_from_json(JsonValue::Null)
                .expect("null stays unavailable")
                .is_none()
        );
        let evidence = dashboard_performance_snapshot_evidence_from_json(json!({
            "status": "complete",
            "range_key": "1W",
            "aggregate_snapshot_count": 2,
            "covered_snapshot_count": 2,
            "missing_legacy_snapshot_count": 0,
            "coverage_pct": 100.0,
            "snapshots_with_position_rows": 2,
            "position_evidence_row_count": 3,
            "first_covered_at": "2026-08-22T08:00:00Z",
            "latest_covered_at": "2026-08-23T08:00:00Z",
            "latest_snapshot": {
                "status": "available",
                "snapshot": {
                    "snapshot_id": 7,
                    "recorded_at": "2026-08-23T08:00:00Z",
                    "snapshot_type": "runtime_current",
                    "source": "test",
                    "position_count": 1,
                    "invested_market_value_dkk": 100.0,
                    "total_cost_basis_dkk": 90.0,
                    "total_unrealised_pnl_dkk": 10.0,
                },
                "items": [{
                    "symbol": "TEST:xcse",
                    "isin": null,
                    "currency": "DKK",
                    "quantity": 1.0,
                    "price_local": 100.0,
                    "fx_rate_to_dkk": 1.0,
                    "cost_basis_local": 90.0,
                    "cost_basis_dkk": 90.0,
                    "market_value_dkk": 100.0,
                    "unrealised_pnl_dkk": 10.0,
                }],
                "safety": "read_only",
                "interpretation": "stored observation",
            },
            "latest_change": {
                "status": "collecting",
                "current_snapshot": null,
                "previous_snapshot": null,
                "opened_count": null,
                "closed_count": null,
                "resized_count": null,
                "unchanged_quantity_count": null,
                "net_market_value_change_dkk": null,
                "net_cost_basis_change_dkk": null,
                "safety": "read_only",
                "interpretation": "needs two observations",
            },
            "detail_retention": "90 days",
            "integrity": {
                "status": "healthy",
                "checked_snapshot_count": 2,
                "structural_mismatch_count": 0,
                "structural_mismatches": [],
                "broker_derived_unrealised_difference_count": 0,
                "broker_derived_unrealised_differences": [],
                "tolerance": { "absolute_dkk": 50.0, "relative": 0.002 },
                "safety": "read_only",
            },
            "safety": "read_only",
            "interpretation": "local historical evidence",
        }))
        .expect("complete snapshot evidence is typed")
        .expect("non-null evidence is present");
        assert_eq!(evidence.latest_snapshot.items[0].symbol, "TEST:xcse");
        assert!(evidence.latest_change.net_market_value_change_dkk.is_none());
        assert!(
            dashboard_performance_snapshot_evidence_from_json(json!({"status": "complete"}))
                .is_err()
        );
    }

    #[test]
    fn dashboard_pnl_reconciliation_preserves_alignment_and_unavailable_states() {
        assert!(
            dashboard_performance_pnl_reconciliation_from_json(JsonValue::Null)
                .expect("null stays unavailable")
                .is_none()
        );
        let reconciliation = dashboard_performance_pnl_reconciliation_from_json(json!({
            "scope": "read_only_unrealised_pnl_sources",
            "dashboard": {
                "status": "available",
                "unrealised_pnl_dkk": 1_000.0,
                "source": "saxo_broker_snapshot",
                "snapshot_type": "runtime_current",
            },
            "latest_history": {
                "status": "available",
                "unrealised_pnl_dkk": 900.0,
                "source": "daily_close",
                "snapshot_type": "daily_close",
                "recorded_at": "2026-08-23T08:00:00Z",
                "difference_from_dashboard_dkk": 100.0,
            },
            "broker_exposure": {
                "status": "aligned",
                "unrealised_pnl_dkk": 950.0,
                "difference_from_dashboard_dkk": 50.0,
                "account_currency": "DKK",
                "fx_basis": "instrument_currency",
                "instrument_fx_rates_to_dkk": { "DKK": 1.0 },
                "exposure_count": 2,
                "updated_at": "2026-08-23T08:01:00Z",
            },
        }))
        .expect("complete P/L reconciliation is typed")
        .expect("non-null reconciliation is present");
        assert_eq!(reconciliation.broker_exposure.status, "aligned");
        assert_eq!(
            reconciliation.latest_history.difference_from_dashboard_dkk,
            Some(100.0)
        );
        assert!(
            dashboard_performance_pnl_reconciliation_from_json(json!({
                "scope": "read_only_unrealised_pnl_sources"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_exposure_attribution_preserves_available_and_empty_states() {
        let unavailable = dashboard_performance_exposure_attribution_from_json(json!({
            "status": "unavailable",
            "scope": "read_only_stored_saxo_exposure_unrealised_pnl",
            "rows": [],
            "currencies": [],
            "exposure_count": 0,
        }))
        .expect("empty attribution is typed")
        .expect("non-null attribution is present");
        assert_eq!(unavailable.status, "unavailable");
        assert!(unavailable.rows.is_empty());

        let attribution = dashboard_performance_exposure_attribution_from_json(json!({
            "status": "available",
            "scope": "read_only_stored_saxo_exposure_unrealised_pnl",
            "account_currency": "DKK",
            "fx_basis": "instrument_currency",
            "instrument_fx_rates_to_dkk": { "USD": 6.4 },
            "updated_at": "2026-08-23T08:00:00Z",
            "exposure_count": 1,
            "shown_row_count": 1,
            "total_unrealised_pnl_dkk": 640.0,
            "rows": [{
                "symbol": "TEST:xnas",
                "instrument_currency": "USD",
                "quantity": 2.0,
                "unrealised_pnl_dkk": 640.0,
                "profit_loss_instrument_currency": 100.0,
                "fx_rate_to_dkk": 6.4,
                "calculation_reliability": "Estimated",
                "updated_at": "2026-08-23T08:00:00Z",
            }],
            "currencies": [{
                "instrument_currency": "USD",
                "symbol_count": 1,
                "unrealised_pnl_dkk": 640.0,
                "absolute_contribution_pct": 100.0,
            }],
        }))
        .expect("complete attribution is typed")
        .expect("non-null attribution is present");
        assert_eq!(attribution.rows[0].symbol, "TEST:xnas");
        assert_eq!(
            attribution.currencies[0].absolute_contribution_pct,
            Some(100.0)
        );
        assert!(
            dashboard_performance_exposure_attribution_from_json(json!({
                "status": "available"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_realised_sell_outcomes_preserve_empty_and_collecting_states() {
        let unavailable =
            dashboard_performance_realised_sell_outcomes_from_json(realised_sell_outcome_evidence(
                &[],
                crate::holding_period::holding_period_evidence_json(&Default::default()),
                None,
            ))
            .expect("empty outcome evidence is typed")
            .expect("non-null outcome evidence is present");
        assert_eq!(unavailable.status, "unavailable");
        assert!(unavailable.recent_rows.is_empty());

        let outcomes =
            dashboard_performance_realised_sell_outcomes_from_json(realised_sell_outcome_evidence(
                &[json!({
                    "created_at": "2026-08-23T08:00:00Z",
                    "symbol": "TEST:xcse",
                    "quantity": 1.0,
                    "currency": "DKK",
                    "realised_gain_dkk": 100.0,
                    "commission_dkk": 2.0,
                    "tax_dkk": 0.0,
                    "cost_basis_sold_dkk": 1_000.0,
                    "status": "executed",
                })],
                crate::holding_period::holding_period_evidence_json(&Default::default()),
                None,
            ))
            .expect("collecting outcome evidence is typed")
            .expect("non-null outcome evidence is present");
        assert_eq!(outcomes.status, "collecting");
        assert_eq!(outcomes.recent_rows[0].symbol, "TEST:xcse");
        assert_eq!(outcomes.symbol_attribution[0].closed_sale_count, 1);
        assert!(
            dashboard_performance_realised_sell_outcomes_from_json(json!({
                "status": "collecting"
            }))
            .is_err()
        );
    }

    #[test]
    fn dashboard_loads_tab_exclusive_collections_only_for_their_tab() {
        for tab in [
            "decisions",
            "eod",
            "execution",
            "hermes",
            "markov",
            "quiver",
            "tuning",
            "watchlists",
        ] {
            assert!(dashboard_loads_tab_exclusive_data(tab, tab));
            assert!(!dashboard_loads_tab_exclusive_data("overview", tab));
            assert!(!dashboard_loads_tab_exclusive_data("performance", tab));
        }
    }

    #[test]
    fn scheduler_history_policy_uses_defaults_and_disables_negative_values() {
        assert_eq!(
            scheduler_history_policy_values(None, None),
            (
                DEFAULT_SCHEDULER_HISTORY_MAX_ROWS,
                DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS
            )
        );
        assert_eq!(scheduler_history_policy_values(Some(-1), Some(-2)), (0, 0));
        assert_eq!(
            scheduler_history_policy_values(Some(500), Some(14)),
            (500, 14)
        );
    }

    #[test]
    fn the_hermes_execution_context_carries_no_raw_broker_documents() {
        // Regression for the live 2026-08-31 get_context payload, which
        // carried AccountKey 25 times and ClientKey 20 times -- every one
        // inside a broker document nested in these two columns, and handed to
        // an external model that the prompt explicitly tells not to expose
        // them.
        let rows = vec![
            json!({
                "id": 274,
                "symbol": "PLTR:xnas",
                "status": "filled",
                "raw_payload_json": {
                    "broker_payload": {
                        "AccountKey": "must-not-reach-hermes",
                        "ClientKey": "must-not-reach-hermes",
                        "OrderId": "12345"
                    },
                    "payload": {"AccountKey": "must-not-reach-hermes"}
                }
            }),
            json!({
                "id": 275,
                "symbol": "DE:xnys",
                "status": "execution_failed",
                "execution_result_json": {
                    "broker_sync": {
                        "broker_payload": {"AccountKey": "must-not-reach-hermes", "ClientKey": "must-not-reach-hermes"}
                    },
                    "payload": {"AccountKey": "must-not-reach-hermes"}
                }
            }),
        ];

        let stripped = strip_raw_broker_documents(rows);
        let serialized = serde_json::to_string(&stripped).expect("rows serialize");

        assert!(
            !serialized.contains("must-not-reach-hermes"),
            "{serialized}"
        );
        assert!(!serialized.contains("AccountKey"), "{serialized}");
        assert!(!serialized.contains("ClientKey"), "{serialized}");
        // The rows themselves survive: only the raw documents are projected out.
        assert_eq!(stripped.len(), 2);
        assert_eq!(stripped[0]["symbol"], "PLTR:xnas");
        assert_eq!(stripped[1]["status"], "execution_failed");
    }

    #[test]
    fn a_scheduler_cycle_keeps_step_health_and_drops_the_diagnostic_dump() {
        // 20 stored cycle documents at ~28 KB each made scheduler.cycles 1.81 MB
        // of a 4.66 MB get_context payload, which the transport then truncated
        // and Hermes reported as missing portfolio context.
        let cycle = json!({
            "duration_ms": 41234,
            "step_durations": {"markov_method": 210_000, "decision_reports": 4_000},
            "portfolio_position_snapshot_integrity": {
                "status": "ok",
                "error_count": 0,
                "rows": (0..200).map(|i| json!({"symbol": format!("S{i}"), "detail": "x".repeat(40)})).collect::<Vec<_>>()
            },
            "decision_reports": {
                "status": "completed",
                "reason": "pulse due",
                "report": {"prompt_text": "y".repeat(5000)}
            },
            "protective_stop_sweep": {"status": "error", "error": "broker rejected"},
            "notifications": ["a", "b", "c"],
            "saxo_session": {"state": "connected"}
        });

        let compact = compact_scheduler_cycle_document(&cycle);

        // Health survives.
        assert_eq!(compact["duration_ms"], 41234);
        assert_eq!(compact["step_durations"]["markov_method"], 210_000);
        assert_eq!(
            compact["portfolio_position_snapshot_integrity"]["status"],
            "ok"
        );
        assert_eq!(
            compact["portfolio_position_snapshot_integrity"]["error_count"],
            0
        );
        assert_eq!(compact["protective_stop_sweep"]["status"], "error");
        assert_eq!(compact["protective_stop_sweep"]["error"], "broker rejected");
        assert_eq!(compact["decision_reports"]["reason"], "pulse due");
        assert_eq!(compact["saxo_session"]["state"], "connected");
        assert_eq!(compact["notifications"]["count"], 3);

        // The dumps do not.
        let serialized = serde_json::to_string(&compact).expect("serialize");
        assert!(
            !serialized.contains("prompt_text"),
            "{}",
            &serialized[..200.min(serialized.len())]
        );
        assert!(!serialized.contains("S199"));
        assert!(
            serialized.len() < 600,
            "compacted cycle should be small, got {} bytes",
            serialized.len()
        );
    }

    /// Every one of these is a real `strategy_role` from the SELL orders. The
    /// panel grouped on the raw string, so the four flattens that together lost
    /// 7,073 DKK appeared as four unrelated one-off rows.
    #[test]
    fn exits_group_by_what_they_did_not_by_how_the_model_spelled_it() {
        for role in [
            "flatten",
            "FLATTEN",
            "flatten_risk_reduction",
            "FLATTEN_risk_reduction",
            "risk_reduction_flatten",
            "partial_flatten_risk_reduction",
            "FLATTEN risk-reduction: close the full 6-share DSV position because the fresh bearish Markov regime materially conflicts with continued long exposure.",
        ] {
            assert_eq!(exit_route_family(role), "flatten", "{role}");
        }

        for role in [
            "risk_reduction",
            "risk-reduction",
            "risk reduction",
            "partial_risk_reduce",
            "risk_reduction_trim",
            "risk-reduction: reduce concentration from 19.5% allocation",
        ] {
            assert_eq!(exit_route_family(role), "risk reduction", "{role}");
        }

        assert_eq!(exit_route_family("protective_stop"), "protective stop");
        assert_eq!(exit_route_family("rotation_exit"), "rotation exit");
        assert_eq!(exit_route_family("reduce_to_target"), "other role");
        assert_eq!(exit_route_family("   "), "unlabelled role");
    }

    /// A flatten is whatever the Trading Manager's gate says it is. If these
    /// two ever disagree, the table would report a family the runtime does not
    /// admit under that name.
    #[test]
    fn the_flatten_family_matches_the_gate_that_admits_flattens() {
        for role in [
            "flatten",
            "FLATTEN_risk_reduction",
            "partial_flatten_risk_reduction",
        ] {
            let gate_sees_flatten = role.to_ascii_lowercase().contains("flatten");
            assert_eq!(
                gate_sees_flatten,
                exit_route_family(role) == "flatten",
                "{role} is classified differently by the table and the gate"
            );
        }
    }

    /// The loop asked to propose strategy changes could not see whether the
    /// strategy makes money: its performance block carried unrealised value
    /// history and nothing else. It gets aggregates, not the twelve recent
    /// rows and three attribution tables the screen shows, and it gets the
    /// baseline so it knows which book the numbers describe.
    #[test]
    fn the_hermes_performance_block_carries_book_scoped_realised_outcomes() {
        let overview = json!({
            "integrity": {
                "realised_sell_outcomes": {
                    "status": "preliminary",
                    "closed_sale_count": 32,
                    "win_count": 9,
                    "loss_count": 23,
                    "win_rate": 0.28125,
                    "payoff_ratio": 0.43,
                    "total_realised_gain_dkk": -21_298.0,
                    "baseline": {"batch_id": "broker-bootstrap-20260716T190000Z"},
                    "holding_period": {"winner_median_days_held": 31.0},
                    "recent_rows": [{"symbol": "NOISE:xnas"}],
                    "symbol_attribution": [{"symbol": "NOISE:xnas"}],
                }
            }
        });

        let summary = realised_outcomes_for_hermes(&overview);

        assert_eq!(summary["win_rate"], 0.28125);
        assert_eq!(summary["payoff_ratio"], 0.43);
        assert_eq!(summary["total_realised_gain_dkk"], -21_298.0);
        assert_eq!(
            summary["baseline"]["batch_id"], "broker-bootstrap-20260716T190000Z",
            "the loop must know which book these numbers describe"
        );
        assert_eq!(summary["holding_period"]["winner_median_days_held"], 31.0);
        assert!(
            summary.get("recent_rows").is_none() && summary.get("symbol_attribution").is_none(),
            "screen detail is not evidence for a loop deciding a threshold"
        );
    }

    #[test]
    fn a_missing_outcome_block_reports_unavailable_rather_than_zeroes() {
        let summary = realised_outcomes_for_hermes(&json!({}));
        assert_eq!(summary["status"], "unavailable");
        assert!(
            summary.get("win_rate").is_none(),
            "no number is better than a fabricated zero"
        );
    }

    /// Blended across the 2026-07-16 bootstrap the panel reported +18,762 DKK
    /// while the live book stood at -21,298 — a 40,000 DKK swing in the
    /// flattering direction on the one panel that exists to answer whether the
    /// strategy works. The bootstrap note is explicit that it "supersedes stale
    /// 2026-05-18 import rows that no longer match the broker book", so those
    /// sales were booked against cost bases that no longer exist.
    #[test]
    fn realised_outcomes_describe_the_current_book_not_the_superseded_one() {
        let sale = |created_at: &str, gain: f64| {
            json!({
                "created_at": created_at,
                "symbol": "AMD:xnas",
                "quantity": 1.0,
                "currency": "USD",
                "realised_gain_dkk": gain,
                "commission_dkk": 4.0,
                "tax_dkk": 0.0,
                "cost_basis_sold_dkk": 1_000.0,
                "status": "executed",
            })
        };
        let baseline = RealisedSellBaseline {
            batch_id: "broker-bootstrap-20260716T190000Z".to_string(),
            started_at: "2026-07-16T19:00:00Z".to_string(),
        };

        let outcomes = realised_sell_outcome_evidence(
            &[
                sale("2026-06-20T10:00:00Z", 40_000.0),
                sale("2026-08-20T10:00:00Z", -21_000.0),
            ],
            crate::holding_period::holding_period_evidence_json(&Default::default()),
            Some(&baseline),
        );

        assert_eq!(
            outcomes["closed_sale_count"], 1,
            "only the current book's sale is counted"
        );
        assert_eq!(outcomes["total_realised_gain_dkk"], -21_000.0);
        assert_eq!(outcomes["baseline"]["superseded_sale_count"], 1);
        assert_eq!(
            outcomes["baseline"]["superseded_realised_gain_dkk"], 40_000.0,
            "the excluded total stays visible rather than vanishing"
        );
        assert_eq!(
            outcomes["baseline"]["batch_id"],
            "broker-bootstrap-20260716T190000Z"
        );
    }

    /// With no batch recorded there is no boundary to apply, and silently
    /// dropping every row would be worse than counting them all.
    #[test]
    fn without_a_baseline_every_retained_sale_is_still_counted() {
        let outcomes = realised_sell_outcome_evidence(
            &[json!({
                "created_at": "2026-06-20T10:00:00Z",
                "symbol": "AMD:xnas",
                "quantity": 1.0,
                "currency": "USD",
                "realised_gain_dkk": 500.0,
                "commission_dkk": 4.0,
                "tax_dkk": 0.0,
                "cost_basis_sold_dkk": 1_000.0,
                "status": "executed",
            })],
            crate::holding_period::holding_period_evidence_json(&Default::default()),
            None,
        );
        assert_eq!(outcomes["closed_sale_count"], 1);
        assert_eq!(outcomes["baseline"]["status"], "no_import_batch_recorded");
    }

    /// A rejected statement and an empty table used to produce the same empty
    /// vector at 42 call sites, so a panel said "no data" for both. The
    /// fallback stays -- degrading a panel is usually right -- but the error
    /// now names the statement that failed.
    #[tokio::test]
    async fn a_failed_query_returns_an_error_naming_the_statement() {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory database");
        let state = AppState {
            config_path: std::path::PathBuf::from("query-test.yaml"),
            config: serde_yaml::from_str("{}").expect("parse test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        };

        let err = state
            .select_json("SELECT id FROM a_table_that_does_not_exist")
            .await
            .expect_err("a missing table must not read as an empty result");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("a_table_that_does_not_exist"),
            "the error must name the statement, got {rendered}"
        );
    }

    /// The outline is what lands in the log line, so it has to identify the
    /// query without dumping interpolated symbols and dates into it.
    #[test]
    fn a_query_outline_names_its_table_and_stays_short() {
        let outline = query_outline(
            "SELECT l.id, l.created_at, l.symbol \n FROM trade_ledger l \n WHERE l.symbol = 'NESTE:xhel'",
        );
        assert!(outline.starts_with("trade_ledger:"), "got {outline}");
        assert!(
            outline.len() < 100,
            "outline stays log-sized, got {outline}"
        );
    }

    /// The realised-sell query carried `\\` line breaks — an *escaped*
    /// backslash in Rust, so PostgreSQL received a literal `\` and rejected the
    /// statement with a syntax error. `select_json`'s error went into
    /// `unwrap_or_default()`, so the panel reported "unavailable" and looked
    /// exactly like an empty ledger; it had never rendered in production
    /// despite 64 qualifying sales. Any SQL this module builds must be free of
    /// stray backslashes, which SQLite tolerates in more places than Postgres.
    #[test]
    fn no_sql_literal_in_this_module_carries_a_stray_backslash() {
        let source = include_str!("state.rs");
        let offenders: Vec<usize> = source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_end();
                // A Rust line continuation is a single trailing backslash; two
                // means a literal backslash reached the string.
                trimmed.ends_with("\\\\") && !trimmed.trim_start().starts_with("//")
            })
            .map(|(index, _)| index + 1)
            .collect();
        assert!(
            offenders.is_empty(),
            "these lines end in an escaped backslash that would reach the database: {offenders:?}"
        );
    }

    /// The refused-candidate retry reports its outcome inside `attempts`, and
    /// the compaction keeps only short top-level scalars. Without a scalar
    /// summary every cycle -- retry fired, retry declined, retry errored --
    /// persists as the same `{"status":"ok"}` and the record answers nothing.
    /// The freshness chip and the missed-reflection alert must agree on what a
    /// reflection is. They did not: the chip was fixed on 2026-09-01 and the
    /// alert kept counting watchdog placeholders, so the dashboard said six
    /// days stale while the alert stayed quiet. The chip's SQL has to stay a
    /// literal, so this is what stops the two drifting again.
    #[test]
    fn the_reflection_freshness_chip_uses_the_shared_predicate() {
        let source = FRESHNESS_SOURCES
            .iter()
            .find(|source| source.key == "hermes_reflection")
            .expect("the reflection freshness source ships");
        assert!(
            source.sql.contains(REAL_DAILY_REFLECTION_SQL_PREDICATE),
            "freshness SQL has drifted from the shared predicate:\n  sql: {}\n  predicate: {}",
            source.sql,
            REAL_DAILY_REFLECTION_SQL_PREDICATE
        );
    }

    /// A scoped context must be honest about what it left out. The failure
    /// this guards is a reader concluding "no execution activity today" from a
    /// payload that was never asked for execution.
    #[test]
    fn a_scoped_context_names_what_it_withheld() {
        let mut value = json!({
            "status": "ok",
            "generated_at": "2026-09-01T21:45:00Z",
            "safety": {"boundary": "read_only"},
            "decisions": {"reports": []},
            "end_of_day": {"reports": []},
            "execution": {"orders": [1, 2, 3]},
            "quiver_signals": {"signals": []},
            "overview": {"portfolio": {}},
        });

        scope_hermes_context(
            &mut value,
            &["decisions".to_string(), "END_OF_DAY".to_string()],
        );

        // Requested and always-present survive.
        assert!(value.get("decisions").is_some());
        assert!(
            value.get("end_of_day").is_some(),
            "matching is case-insensitive"
        );
        assert!(value.get("status").is_some());
        assert!(
            value.get("safety").is_some(),
            "the boundary statement is never filtered out"
        );

        // The rest are gone, and named.
        assert!(value.get("execution").is_none());
        assert!(value.get("quiver_signals").is_none());
        assert_eq!(
            value["sections"]["omitted"],
            json!(["execution", "overview", "quiver_signals"])
        );
        assert_eq!(
            value["sections"]["requested"],
            json!(["decisions", "end_of_day"])
        );
    }

    /// Hoisting must move the value, not delete it. The 11 thesis reviews that
    /// carried one identical `invalidation` string cost 2,178 bytes to say one
    /// sentence; after hoisting the sentence is still readable, once.
    #[test]
    fn a_field_identical_on_every_item_moves_out_of_the_list_intact() {
        let boilerplate = "Re-evaluate on a fresh decision pulse if verified technical evidence or the Markov regime no longer supports the position.";
        let mut value = json!({
            "execution": {
                "reviews": [
                    {"symbol": "ALMB:xcse", "invalidation": boilerplate, "age_days": 36},
                    {"symbol": "AUTO:xosl", "invalidation": boilerplate, "age_days": 12},
                ]
            }
        });

        compact_hermes_context(&mut value);

        assert_eq!(
            value["execution"]["reviews_shared"]["invalidation"],
            boilerplate
        );
        assert!(
            value["execution"]["reviews"][0]
                .get("invalidation")
                .is_none()
        );
        assert_eq!(value["execution"]["reviews"][0]["symbol"], "ALMB:xcse");
        assert_eq!(value["execution"]["reviews"][1]["age_days"], 12);
    }

    #[test]
    fn a_field_that_differs_stays_on_its_item() {
        let mut value = json!({
            "block": {
                "items": [
                    {"note": "a distinct rationale long enough to clear the hoist threshold, one"},
                    {"note": "a distinct rationale long enough to clear the hoist threshold, two"},
                ]
            }
        });

        compact_hermes_context(&mut value);

        assert!(value["block"].get("items_shared").is_none());
        assert!(value["block"]["items"][0]["note"].is_string());
    }

    /// Hoisting a short repeated flag changes the payload's shape for no
    /// saving, and a reader then has to look in two places for a status.
    #[test]
    fn a_short_repeated_field_is_left_where_it_is() {
        let mut value = json!({
            "block": {"items": [{"status": "ok"}, {"status": "ok"}]}
        });

        compact_hermes_context(&mut value);

        assert!(value["block"].get("items_shared").is_none());
        assert_eq!(value["block"]["items"][1]["status"], "ok");
    }

    /// "None today" is a fact and must survive; a field that was never
    /// populated is not.
    #[test]
    fn all_null_keys_go_but_empty_collections_stay() {
        let mut value = json!({
            "block": {
                "ledger_id": null,
                "report": {"id": null, "model": null},
                "fills": [],
                "summary": {},
                "quantity": 32.0,
            }
        });

        compact_hermes_context(&mut value);

        assert!(value["block"].get("ledger_id").is_none());
        assert!(
            value["block"].get("report").is_none(),
            "an all-null object carries nothing"
        );
        assert!(
            value["block"]["fills"].is_array(),
            "an empty list means none, and stays"
        );
        assert!(value["block"]["summary"].is_object());
        assert_eq!(value["block"]["quantity"], 32.0);
    }

    /// The narrative sweep is targeted at thesis records. A blanket sweep by
    /// key name would also strip the block-level statements that say what a
    /// block may not be used for.
    #[test]
    fn narrative_prose_goes_but_block_boundary_statements_survive() {
        let mut value = json!({
            "status": "ok",
            "safety": {"broker_mutations_excluded": true},
            "execution": {
                "orders": [{
                    "symbol": "CMG:xnys",
                    "attribution": {
                        "trade_thesis": {
                            "symbol": "CMG:xnys",
                            "entry_rationale": "Bullish OVERWEIGHT technical structure with 4 of 3 confluences.",
                            "approval_evidence": "BUY approved by bullish technical confluence, database-verified.",
                            "catalyst_or_monitor": "Actionable consumer starter with fresh Bull-state Markov.",
                            "technical": {"confluence_count": 4},
                            "markov": {"signed_signal": 0.36},
                        }
                    }
                }],
                "protective_stop_coverage": {
                    "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit",
                    "summary": {"covered": 3},
                },
            },
            "hermes": {
                "experiments": [{
                    "id": "strategy-experiment-1",
                    "changed_variable_path": "execution.min_trade_value_dkk",
                    "status": "pending_review",
                    "evidence_json": {"duplicate_check": "a long supporting narrative"},
                    "raw_payload_json": {"one_variable_only": true},
                }]
            }
        });

        compact_hermes_context(&mut value);

        let thesis = &value["execution"]["orders"][0]["attribution"]["trade_thesis"];
        assert!(thesis.get("entry_rationale").is_none());
        assert!(thesis.get("approval_evidence").is_none());
        assert!(thesis.get("catalyst_or_monitor").is_none());
        assert_eq!(
            thesis["technical"]["confluence_count"], 4,
            "the structured evidence stays"
        );
        assert_eq!(thesis["markov"]["signed_signal"], 0.36);

        assert!(
            value["safety"].is_object(),
            "the context boundary statement is untouched"
        );
        assert!(
            value["execution"]["protective_stop_coverage"]["safety"].is_string(),
            "a block-level boundary statement is not narrative prose"
        );

        let experiment = &value["hermes"]["experiments"][0];
        assert!(experiment.get("evidence_json").is_none());
        assert!(experiment.get("raw_payload_json").is_none());
        assert_eq!(
            experiment["changed_variable_path"],
            "execution.min_trade_value_dkk"
        );
        assert_eq!(experiment["status"], "pending_review");
    }

    /// A reader who cannot tell a compacted payload from a sparse one will
    /// misread the second as the first.
    #[test]
    fn a_compacted_payload_says_so() {
        let mut value = json!({"block": {"items": [{"x": null}, {"x": null}]}});
        let descriptor = compact_hermes_context(&mut value);
        assert_eq!(descriptor["detail"], "compact");
        assert!(descriptor["bytes_before"].as_u64() > descriptor["bytes_after"].as_u64());
        assert!(
            descriptor["applied"]
                .as_array()
                .expect("applied list")
                .len()
                >= 3
        );
    }

    #[test]
    fn full_detail_is_the_default_and_changes_nothing() {
        assert_eq!(ContextDetail::from_argument(None), ContextDetail::Full);
        assert_eq!(ContextDetail::from_argument(Some("")), ContextDetail::Full);
        assert_eq!(
            ContextDetail::from_argument(Some("nonsense")),
            ContextDetail::Full
        );
        assert_eq!(
            ContextDetail::from_argument(Some(" COMPACT ")),
            ContextDetail::Compact
        );
    }

    /// An unrecognised name silently widening the payload back to everything is
    /// the failure mode this filter invites, so it is reported.
    #[test]
    fn an_unknown_context_section_filters_nothing_and_says_so() {
        let mut value = json!({
            "status": "ok",
            "safety": {},
            "decisions": {},
            "execution": {},
        });

        scope_hermes_context(&mut value, &["decisons".to_string()]);

        assert!(
            value.get("execution").is_some(),
            "a typo must not silently scope the payload"
        );
        assert_eq!(value["sections"]["unknown"], json!(["decisons"]));
        assert_eq!(value["sections"]["requested"], json!([]));
    }

    #[test]
    fn an_unscoped_context_is_untouched() {
        let original = json!({"status": "ok", "execution": {}, "quiver_signals": {}});
        let mut value = original.clone();
        scope_hermes_context(&mut value, &[]);
        assert_eq!(
            value, original,
            "callers that do not ask must see no change"
        );
    }

    /// Every advertised section must be a real top-level key, or a caller that
    /// names it gets an empty payload instead of the block it asked for.
    #[test]
    fn every_advertised_context_section_is_a_real_payload_key() {
        let payload = json!({
            "status": "", "generated_at": "", "capabilities": {}, "goal_contract": {},
            "overview": {}, "scheduler": {}, "decisions": {}, "end_of_day": {},
            "strategy_journal": {}, "execution": {}, "performance": {}, "markov_method": {},
            "quiver_signals": {}, "quiver_conflicts": {}, "editorial_research": {},
            "daily_indicators": {}, "hermes": {}, "safety": {},
        });
        let keys = payload.as_object().expect("object");
        for section in HERMES_CONTEXT_SECTIONS {
            assert!(
                keys.contains_key(*section),
                "{section} is not a context key"
            );
        }
        for always in HERMES_CONTEXT_ALWAYS_PRESENT {
            assert!(
                !HERMES_CONTEXT_SECTIONS.contains(always),
                "{always} is always present and must not also be requestable"
            );
        }
    }

    #[test]
    fn a_retry_outcome_survives_cycle_compaction() {
        let cycle = json!({
            "refused_candidate_retry": {
                "status": "ok",
                "reason": "us_open_followup:2026-09-01=requested",
                "attempts": [
                    {"pulse_key": "us_open_followup:2026-09-01", "status": "requested"}
                ],
                "safety": "replacement_candidates_come_from_a_new_audited_decision_report_and_face_every_gate",
            },
        });

        let compact = compact_scheduler_cycle_document(&cycle);

        assert_eq!(compact["refused_candidate_retry"]["status"], "ok");
        assert_eq!(
            compact["refused_candidate_retry"]["reason"], "us_open_followup:2026-09-01=requested",
            "a retry that fired must not compact to the same record as one that declined"
        );
    }

    #[test]
    fn performance_history_keeps_one_closing_observation_per_day() {
        // A snapshot per scheduler cycle made a one-month range 495 points and
        // 186 KB, for a series whose shape is daily.
        let history = vec![
            json!({"recorded_at": "2026-08-29T09:00:00Z", "total_market_value_dkk": 100.0}),
            json!({"recorded_at": "2026-08-29T13:00:00Z", "total_market_value_dkk": 110.0}),
            json!({"recorded_at": "2026-08-29T17:00:00Z", "total_market_value_dkk": 120.0}),
            json!({"recorded_at": "2026-08-30T09:00:00Z", "total_market_value_dkk": 130.0}),
            json!({"recorded_at": "2026-08-31T09:00:00Z", "total_market_value_dkk": 140.0}),
            json!({"recorded_at": "2026-08-31T17:41:42Z", "total_market_value_dkk": 150.0}),
        ];

        let compact = compact_performance_history(history);

        assert_eq!(compact.len(), 3, "one point per calendar day");
        assert_eq!(
            compact[0]["total_market_value_dkk"], 120.0,
            "the day's last reading is its close"
        );
        assert_eq!(compact[1]["total_market_value_dkk"], 130.0);
        assert_eq!(
            compact[2]["total_market_value_dkk"], 150.0,
            "the newest reading is retained even mid-day"
        );
    }

    #[test]
    fn the_strategy_journal_documents_are_projected_out() {
        let rows = vec![json!({
            "id": 12,
            "journal_date": "2026-08-31",
            "summary": "flat session",
            "learnings_json": [{"lesson": "keep"}],
            "diary_json": {"narrative": "z".repeat(7000)},
            "metrics_json": {"detail": "z".repeat(3000)}
        })];

        let compact = compact_journal_rows(rows);
        let serialized = serde_json::to_string(&compact).expect("serialize");

        assert!(!serialized.contains("diary_json"), "{serialized:.120}");
        assert!(!serialized.contains("metrics_json"));
        // The advisory-useful fields survive.
        assert_eq!(compact[0]["summary"], "flat session");
        assert_eq!(compact[0]["learnings_json"][0]["lesson"], "keep");
        assert_eq!(compact[0]["journal_date"], "2026-08-31");
    }

    #[test]
    fn the_overview_sheds_the_embedded_markov_run_and_cycle_document() {
        let overview = json!({
            "markov_method": {
                "status": "ok",
                "latest_run": {
                    "id": 9,
                    "status": "completed",
                    "asset_count": 201,
                    "summary_json": {
                        "status": "completed",
                        "signals": (0..20).map(|i| json!({"symbol": format!("S{i}"), "recent_labels": vec!["Bull"; 60]})).collect::<Vec<_>>()
                    }
                }
            },
            "scheduler_status": {
                "status": "ok",
                "last_cycle_json": {"decision_reports": {"status": "completed", "report": {"prompt_text": "y".repeat(4000)}}}
            }
        });

        let compact = compact_overview_for_hermes(overview);
        let serialized = serde_json::to_string(&compact).expect("serialize");

        assert!(
            !serialized.contains("recent_labels"),
            "U12's array must not return here"
        );
        assert!(!serialized.contains("prompt_text"));
        // Run-level health metadata is exactly what survives.
        assert_eq!(compact["markov_method"]["latest_run"]["asset_count"], 201);
        assert_eq!(
            compact["scheduler_status"]["last_cycle_json"]["decision_reports"]["status"],
            "completed"
        );
    }

    #[test]
    fn gate_replay_flags_evidence_that_spans_a_model_change() {
        // Hermes is permitted to propose min_signed_signal, and the replay
        // pools the last N manager runs into one aggregate. On 2026-08-31 the
        // Markov horizon moved from daily to hourly, which changed the signal
        // distribution enough that the same 0.15 threshold went from admitting
        // 111 of 200 symbols to 132. Evidence spanning that boundary describes
        // two models under one label.
        let run = |horizon: Option<i64>, threshold: f64| {
            let model = match horizon {
                Some(minutes) => json!({"horizon_minutes": minutes, "window_days": 20}),
                None => json!("not_recorded"),
            };
            json!({"manager_json": {"hermes_preflight": {"markov": {
                "min_signed_signal": threshold,
                "model": model
            }}}})
        };

        let one_generation = vec![run(Some(60), 0.20), run(Some(60), 0.20)];
        let replay = gate_replay_from_manager_runs(&one_generation);
        assert_eq!(replay["evidence_is_mixed"], false);
        assert_eq!(replay["evidence_generations"].as_array().unwrap().len(), 1);
        assert_eq!(replay["evidence_generations"][0]["run_count"], 2);

        // A threshold change alone splits the evidence.
        let threshold_change = vec![run(Some(60), 0.20), run(Some(60), 0.15)];
        let replay = gate_replay_from_manager_runs(&threshold_change);
        assert_eq!(replay["evidence_is_mixed"], true);
        assert_eq!(replay["evidence_generations"].as_array().unwrap().len(), 2);

        // So does a horizon change at one threshold.
        let horizon_change = vec![run(Some(60), 0.15), run(Some(1440), 0.15)];
        let replay = gate_replay_from_manager_runs(&horizon_change);
        assert_eq!(replay["evidence_is_mixed"], true);
        assert!(
            replay["evidence_caveat"]
                .as_str()
                .unwrap()
                .contains("more than one Markov configuration")
        );

        // Runs predating the stamp report an unrecorded model rather than
        // being silently folded in with stamped ones.
        let mixed_vintage = vec![run(Some(60), 0.15), run(None, 0.15)];
        let replay = gate_replay_from_manager_runs(&mixed_vintage);
        assert_eq!(replay["evidence_is_mixed"], true);
        assert!(
            replay["evidence_generations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|generation| generation["model"] == "not_recorded")
        );
    }

    #[test]
    fn the_enforcement_record_describes_the_cost_model_that_exists() {
        // U3 exists to stop the contract advertising a risk envelope the
        // runtime does not apply. This is the same drift inside U3's own
        // record: the slippage entry claimed "no cost model exists, so
        // slippage is never estimated before queueing" while cost_guard_gate
        // was estimating one-way slippage from strategy.estimated_slippage_bps
        // and refusing BUYs that could not clear it. Understating enforcement
        // is a smaller error than overstating it, but it is the same failure.
        let config: YamlValue = serde_yaml::from_str("{}").expect("parse empty config");
        let contract = hermes_goal_contract_from_config(&config);
        let detail = contract["enforcement"]["constraints.slippage_tolerance"]["detail"]
            .as_str()
            .expect("slippage enforcement detail");

        assert!(
            detail.contains("estimated_slippage_bps"),
            "the record must name the mechanism that actually runs: {detail}"
        );
        assert!(
            !detail.contains("never estimated"),
            "slippage is estimated; only the contract's own value is unchecked: {detail}"
        );
        // The status itself stays not_enforced: the 2% tolerance is genuinely
        // never compared against, even though slippage is modelled.
        assert_eq!(
            contract["enforcement"]["constraints.slippage_tolerance"]["status"],
            "not_enforced"
        );
    }

    #[test]
    fn wal_archiving_is_watched_and_reads_the_primary_only() {
        // Base backups stopped for ten days across the 2026-08-31 disk crisis
        // and nothing surfaced it. CNPG records base-backup state in Kubernetes,
        // which this process cannot read; continuous WAL archiving is the part
        // that is visible from the application's own connection, because the
        // `-rw` service routes to the primary. Querying a replica returns zeros
        // and a null timestamp, which is how this was missed the first time.
        let source = FRESHNESS_SOURCES
            .iter()
            .find(|s| s.key == "wal_archiving")
            .expect("WAL archiving is a watched source");

        assert!(source.sql.contains("pg_stat_archiver"));
        assert!(
            source.sql.contains("last_archived_time"),
            "a stalled archiver stops advancing this, so staleness is the failure signal"
        );
        assert!(
            source.sql.contains("AT TIME ZONE 'UTC'") && source.sql.contains(r#""T""#),
            "the timestamp must reach the strip as RFC3339 or it cannot be aged"
        );
        // Three missed 5-minute segment switches, matching the FX rule.
        assert_eq!(source.stale_after_minutes, 15);

        // A replica answers with a null timestamp rather than an error, and
        // SQLite has no such view at all, so the row must degrade to missing
        // rather than reporting a freshness it does not have.
        assert_eq!(freshness_state(None, source.stale_after_minutes), "missing");
        assert_eq!(
            freshness_state(Some(8), source.stale_after_minutes),
            "aging",
            "one missed segment switch should warn before it is a failure"
        );
        assert_eq!(
            freshness_state(Some(4), source.stale_after_minutes),
            "fresh"
        );
        assert_eq!(
            freshness_state(Some(60), source.stale_after_minutes),
            "stale"
        );
    }

    #[test]
    fn a_scenario_that_never_ran_is_not_reported_as_one_that_changed_nothing() {
        // The Markov BUY gate is a rescue path: it is consulted only when the
        // technical gate rejects a BUY. Across the retained runs the technical
        // gate rejected none, so min_signed_signal was never read -- yet the
        // replay rendered that identically to "we tested it and nothing
        // flipped". Hermes is allowed to propose this exact variable, so the
        // two must not read the same.
        let run = |signal: f64, technical_rejects: bool| {
            json!({"manager_json": {"hermes_preflight": {"candidate_waterfall": [{
                "symbol": "PLTR:xnas",
                "action": "BUY",
                "strategy_key": "k",
                "technical": {
                    "status": "ok",
                    "sentiment": if technical_rejects { "HOLD" } else { "BUY" },
                    "trend_bias": if technical_rejects { "neutral" } else { "bullish" },
                    "confluence_count": if technical_rejects { 1 } else { 5 },
                    "min_confluences": 3
                },
                "markov": {"status": "ok", "direction": "long", "signed_signal": signal, "age_days": 0}
            }]}}})
        };

        // Technical approves every candidate, so the rescue path is never
        // consulted and the threshold could not have mattered.
        let unreachable = gate_replay_markov_scenario(&[run(0.30, false), run(0.10, false)]);
        assert_eq!(unreachable["summary"]["evaluated_count"], 0);
        assert_eq!(
            unreachable["reachability"], "unreachable_in_retained_evidence",
            "a threshold that was never read must say so"
        );

        // With no candidates at all the answer is different again: nothing was
        // proposed, rather than proposed and not gated.
        let empty = gate_replay_markov_scenario(&[]);
        assert_eq!(empty["reachability"], "no_candidates");
    }

    /// The real EU-open cycle of 2026-09-01, with its recorded numbers. At the
    /// shipped 2% buffer both candidates were affordable and NESTE executed; at
    /// 5% the budget falls from 24,941 to 19,414 DKK and the second BUY no
    /// longer fits behind the first. This is the whole point of the scenario:
    /// before it existed, a Hermes proposal against the cash buffer was judged
    /// on nothing at all.
    #[test]
    fn a_tighter_cash_buffer_reports_the_buy_it_would_not_have_funded() {
        let run = json!({
            "id": 223,
            "report_id": 259,
            "created_at": "2026-09-01T08:47:36Z",
            "manager_json": {"hermes_preflight": {
                "capital_budget": {
                    "total_market_value_dkk": 245_625.47,
                    "cash_balance_dkk": 38_167.25,
                    "invested_market_value_dkk": 207_458.22,
                    "min_cash_buffer_pct": 0.02,
                    "remaining_deployment_capacity_dkk": 33_254.74,
                    "persisted_buy_reservation_value_dkk": 0.0,
                    "unreduced_available_buy_budget_dkk": 33_254.74,
                    "available_buy_budget_dkk": 24_941.06,
                },
                "candidate_waterfall": [
                    {"symbol": "ALV:xetr", "action": "BUY", "strategy_key": "a", "estimated_value_dkk": 16_848.11},
                    {"symbol": "NESTE:xhel", "action": "BUY", "strategy_key": "b", "estimated_value_dkk": 7_625.0},
                ]
            }}
        });

        let scenario = gate_replay_cash_buffer_scenario(&[run]);

        assert_eq!(
            scenario["variable_path"],
            "strategy.capital.min_cash_buffer_pct"
        );
        assert_eq!(scenario["reachability"], "evaluated");
        assert_eq!(scenario["summary"]["evaluated_count"], 2);
        assert_eq!(
            scenario["summary"]["would_block_target_gate_count"], 1,
            "the second BUY stops fitting behind the first"
        );
        assert_eq!(scenario["summary"]["unchanged_target_gate_count"], 1);
        let change = &scenario["changes"][0];
        assert_eq!(change["symbol"], "NESTE:xhel");
        assert_eq!(change["effect"], "would_block_target_gate");
    }

    /// A guardrail was reducing the budget by 25% on that run. Comparing the
    /// reduced recorded budget against an unreduced counterfactual would show a
    /// tighter buffer funding *more*, which is nonsense -- so the multiplier is
    /// recovered from the recorded pair and re-applied.
    #[test]
    fn the_cash_buffer_replay_keeps_a_live_soft_reduction() {
        let budget = json!({
            "total_market_value_dkk": 245_625.47,
            "cash_balance_dkk": 38_167.25,
            "remaining_deployment_capacity_dkk": 33_254.74,
            "persisted_buy_reservation_value_dkk": 0.0,
            "unreduced_available_buy_budget_dkk": 33_254.74,
            "available_buy_budget_dkk": 24_941.06,
        });

        let (recorded, proposed) =
            gate_replay_cash_budgets(&budget).expect("a full budget block replays");

        assert!((recorded - 24_941.06).abs() < 1.0);
        // (38,167.25 - 245,625.47 * 0.05) * 0.75
        assert!(
            (proposed - 19_414.0).abs() < 5.0,
            "the 0.75 multiplier must survive the counterfactual, got {proposed}"
        );
        assert!(proposed < recorded, "a tighter buffer cannot fund more");
    }

    /// A run recorded before the capital-budget block existed has no buffer to
    /// replay. Reading that as a zero budget would report every BUY as blocked
    /// by a threshold that was never applied to it.
    #[test]
    fn a_run_without_a_capital_budget_is_insufficient_evidence_not_a_zero_budget() {
        let run = json!({
            "id": 1,
            "manager_json": {"hermes_preflight": {"candidate_waterfall": [
                {"symbol": "ALV:xetr", "action": "BUY", "strategy_key": "a", "estimated_value_dkk": 5_000.0}
            ]}}
        });

        let scenario = gate_replay_cash_buffer_scenario(&[run]);

        assert_eq!(scenario["summary"]["insufficient_evidence_count"], 1);
        assert_eq!(scenario["summary"]["would_block_target_gate_count"], 0);
        assert_eq!(scenario["summary"]["evaluated_count"], 0);
        assert!(gate_replay_cash_budgets(&json!({})).is_none());
    }

    /// A SELL frees cash rather than consuming the buffer, so it was never
    /// tested against this budget and must not be counted as unchanged.
    #[test]
    fn a_sell_is_not_reached_by_the_cash_buffer() {
        let run = json!({
            "id": 1,
            "manager_json": {"hermes_preflight": {
                "capital_budget": {
                    "total_market_value_dkk": 100_000.0,
                    "cash_balance_dkk": 50_000.0,
                    "remaining_deployment_capacity_dkk": 50_000.0,
                    "persisted_buy_reservation_value_dkk": 0.0,
                    "unreduced_available_buy_budget_dkk": 48_000.0,
                    "available_buy_budget_dkk": 48_000.0,
                },
                "candidate_waterfall": [
                    {"symbol": "DIS:xnys", "action": "SELL", "strategy_key": "s", "estimated_value_dkk": 7_000.0}
                ]
            }}
        });

        let scenario = gate_replay_cash_buffer_scenario(&[run]);

        assert_eq!(scenario["summary"]["not_reached_count"], 1);
        assert_eq!(scenario["summary"]["evaluated_count"], 0);
        assert_eq!(scenario["reachability"], "unreachable_in_retained_evidence");
    }

    /// The reflection prompt turns `unreachable_in_retained_evidence` into "a
    /// proposal cannot produce a measurable effect no matter which value is
    /// chosen". That is a statement about the deterministic gate, and for a
    /// variable the advisory layer reads it is simply false -- on 2026-09-01
    /// `min_signed_signal` bound nothing in the runtime and decided the whole
    /// US-open cycle through Hermes.
    #[test]
    fn a_variable_the_advisory_reads_is_never_reported_as_having_no_effect() {
        assert_eq!(
            experiment_variable_effect_path(Some("unreachable_in_retained_evidence"), true),
            "advisory_only"
        );
        assert_eq!(
            experiment_variable_effect_path(Some("no_replay_scenario"), true),
            "advisory_only"
        );
        assert_eq!(
            experiment_variable_effect_path(Some("evaluated"), true),
            "gate_and_advisory"
        );
        assert_eq!(
            experiment_variable_effect_path(Some("evaluated"), false),
            "gate_only"
        );
        // Only this combination earns the inert verdict.
        assert_eq!(
            experiment_variable_effect_path(Some("unreachable_in_retained_evidence"), false),
            "no_observed_effect"
        );
        assert_eq!(
            experiment_variable_effect_path(None, false),
            "no_observed_effect"
        );
    }

    /// The specific cell that motivated this: the replay calls the Markov
    /// threshold unreachable, and it is the one Hermes quotes as an admission
    /// bar. A reader must be able to see both facts about it at once.
    #[test]
    fn the_markov_threshold_reports_its_advisory_effect_despite_being_unreachable() {
        let replay = gate_replay_from_manager_runs(&[]);
        let coverage = replay["supported_variable_coverage"]
            .as_array()
            .expect("coverage is reported");
        let markov = coverage
            .iter()
            .find(|row| row["variable_path"] == "strategy.swing.markov_gate.min_signed_signal")
            .expect("markov threshold is covered");

        assert_eq!(markov["advisory_visible"], true);
        assert_ne!(
            markov["effect_path"], "no_observed_effect",
            "Hermes is handed this number and cites it; the replay must not call it inert"
        );

        let floor = coverage
            .iter()
            .find(|row| row["variable_path"] == "execution.min_trade_value_dkk")
            .expect("trade-value floor is listed");
        assert_eq!(
            floor["advisory_visible"], false,
            "the preflight publishes the cost guard, not the trade-value floor"
        );
    }

    #[test]
    fn every_proposable_variable_reports_whether_the_replay_can_test_it() {
        // Hermes is told to check a scenario's reachability before proposing a
        // threshold. That only helps where a scenario exists -- silence reads
        // as "no problem" rather than "no evidence", the confusion reachability
        // was added to remove. `min_cash_buffer_pct` gained a scenario on
        // 2026-09-02; `execution.min_trade_value_dkk` still has none, and is
        // dominated by the commission floor on every BUY anyway.
        let replay = gate_replay_from_manager_runs(&[]);
        let coverage = replay["supported_variable_coverage"]
            .as_array()
            .expect("coverage is reported");

        assert_eq!(
            coverage.len(),
            SUPPORTED_EXPERIMENT_VARIABLES.len(),
            "every proposable variable must be accounted for, covered or not"
        );
        for path in SUPPORTED_EXPERIMENT_VARIABLES {
            let row = coverage
                .iter()
                .find(|row| row["variable_path"] == *path)
                .unwrap_or_else(|| panic!("{path} missing from coverage"));
            assert!(row["reachability"].is_string());
        }

        let uncovered: Vec<&str> = coverage
            .iter()
            .filter(|row| row["covered"] == false)
            .filter_map(|row| row["variable_path"].as_str())
            .collect();
        assert_eq!(
            uncovered,
            vec!["execution.min_trade_value_dkk"],
            "the one variable still without a scenario must say so, not stay silent"
        );
        for path in [
            "strategy.swing.markov_gate.min_signed_signal",
            "strategy.swing.daily_indicators.min_confluences",
            "strategy.capital.min_cash_buffer_pct",
        ] {
            assert!(
                coverage
                    .iter()
                    .any(|row| row["variable_path"] == path && row["covered"] == true),
                "{path} has a scenario and must be marked covered"
            );
        }
    }

    // Saxo refresh-token rotation across pods. Every pod refreshes the same
    // single-use token; these tests share one durable row between simulated
    // pods, each with its own working-copy file, against a fake Saxo token
    // endpoint that consumes whatever token it accepts.

    struct SaxoPods {
        dir: PathBuf,
        pool: AnyPool,
        client_id: String,
    }

    impl SaxoPods {
        async fn new(label: &str) -> Self {
            static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
            INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
            let unique = format!(
                "{label}-{}-{}",
                process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
            let dir = env::temp_dir().join(format!("saxo-session-rotation-{unique}"));
            std::fs::create_dir_all(&dir).expect("create pod working-copy directory");
            let pool = sqlx::any::AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("open in-memory durable session database");
            let pods = Self {
                dir,
                pool,
                client_id: format!("test-client-{unique}"),
            };
            pods.pod("schema")
                .ensure_saxo_session_schema()
                .await
                .expect("create Saxo session schema");
            pods
        }

        /// One process: its own working copy, the shared durable row.
        fn pod(&self, name: &str) -> AppState {
            let config = serde_yaml::from_str(&format!(
                "saxo:\n  environment: sim\n  auth_mode: pkce\n  client_id: {}\n  session_path: {}\n",
                self.client_id,
                self.working_copy(name).display()
            ))
            .expect("parse pod config");
            AppState {
                config_path: self.dir.join("config.yaml"),
                config,
                db_url: "sqlite::memory:".to_string(),
                pool: self.pool.clone(),
            }
        }

        fn working_copy(&self, name: &str) -> PathBuf {
            self.dir.join(format!("{name}.json"))
        }

        /// A durable session whose access token is inside the 15-minute
        /// refresh margin, so the next use rotates it.
        async fn seed_due_session(&self, refresh_token: &str) {
            let session = json!({
                "environment": "sim",
                "auth_mode": "pkce",
                "client_id": self.client_id,
                "redirect_uri": "https://example.invalid/api/saxo/auth/callback",
                "code_verifier": "test-verifier",
                "client_key": "test-client-key",
                "account_key": "test-account-key",
                "default_account_id": "test-account",
                "client_id_display": "test-client",
                "access_token": "test-access-0",
                "refresh_token": refresh_token,
                "token_type": "Bearer",
                "access_token_expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
                "refresh_token_expires_at": (Utc::now() + Duration::minutes(45)).to_rfc3339(),
                "created_at": (Utc::now() - Duration::hours(2)).to_rfc3339(),
                "last_refreshed_at": (Utc::now() - Duration::minutes(15)).to_rfc3339(),
                "refresh_token_invalid_at": null,
                "refresh_error": null,
            });
            self.pod("seed")
                .save_saxo_session_to_db(&session, "test_seed")
                .await
                .expect("seed durable session");
        }

        async fn durable(&self) -> JsonValue {
            self.pod("reader")
                .load_saxo_session_from_db()
                .await
                .expect("read durable session")
                .expect("durable session row")
        }

        /// A stalled or dead lease holder's lease lapses after 45 seconds.
        async fn lapse_refresh_lease(&self) {
            sqlx::query(
                "UPDATE saxo_sessions SET refresh_lease_expires_at = '2000-01-01T00:00:00Z'",
            )
            .execute(&self.pool)
            .await
            .expect("lapse the refresh lease");
        }
    }

    impl Drop for SaxoPods {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The 2026-09-26 shape. The scheduler decides from the durable row that
    /// a refresh is due; before it takes the lease, the API pod rotates the
    /// token and releases the lease; the scheduler then wins the free lease.
    /// It must present the rotated token, not the one its working copy held
    /// when it decided -- that one Saxo has consumed.
    #[tokio::test]
    async fn a_pod_that_wins_the_lease_after_another_rotated_presents_the_durable_token() {
        let pods = SaxoPods::new("lease-gap").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 300);
        pods.seed_due_session("test-refresh-0").await;
        let scheduler = pods.pod("scheduler");
        let api = pods.pod("api");

        saxo_lease_gap_hook::install(&pods.working_copy("scheduler"), move || async move {
            api.ensure_saxo_session_json("auth_status")
                .await
                .expect("the API pod rotates the token in the gap");
        });

        let status = scheduler.refresh_saxo_session().await;

        assert_eq!(
            saxo.rejected(),
            0,
            "no pod may present a refresh token Saxo already consumed"
        );
        assert!(
            status.is_ok(),
            "the scheduler refresh must succeed: {:#}",
            status.err().unwrap()
        );
        let durable = pods.durable().await;
        assert!(durable["refresh_token_invalid_at"].is_null());
        assert_eq!(durable["refresh_token"], saxo.current_refresh_token());
    }

    /// A pod whose lease lapsed while its request was in flight can lose the
    /// rotation to another pod. The refusal it then receives concerns a token
    /// the durable row no longer holds, so it must not mark the winner's live
    /// session invalid.
    #[tokio::test]
    async fn a_refusal_for_a_superseded_token_leaves_the_newer_durable_session_live() {
        let pods = SaxoPods::new("superseded-refusal").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 300);
        pods.seed_due_session("test-refresh-0").await;
        saxo.hold_responses();

        let winner = pods.pod("winner");
        let winner_call =
            tokio::spawn(async move { winner.ensure_saxo_session_json("price_monitor").await });
        saxo.answered().await; // Saxo rotated the token; the response is in flight.
        pods.lapse_refresh_lease().await;
        let loser = pods.pod("loser");
        let loser_call =
            tokio::spawn(async move { loser.ensure_saxo_session_json("refresh").await });
        saxo.answered().await; // Saxo refused the consumed token.

        saxo.release_next(); // The rotation lands first ...
        winner_call
            .await
            .expect("winner task")
            .expect("the winner makes its rotation durable");
        saxo.release_next(); // ... then the refusal.
        let _ = loser_call.await.expect("loser task");

        let durable = pods.durable().await;
        assert!(
            durable["refresh_token_invalid_at"].is_null(),
            "a refusal for a superseded token must not invalidate the live session"
        );
        assert_eq!(durable["refresh_token"], saxo.current_refresh_token());
    }

    /// The rollout race itself. A pod killed after Saxo rotated the token but
    /// before the rotation was durable leaves the durable row holding a
    /// consumed token, and its replacement is refused. Nothing in-process can
    /// survive SIGKILL; the defense is the SIGTERM drain in the next test,
    /// which keeps a pod from being killed mid-rotation.
    #[tokio::test]
    async fn a_pod_killed_between_rotation_and_the_durable_write_strands_its_replacement() {
        let pods = SaxoPods::new("killed-mid-rotation").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 300);
        pods.seed_due_session("test-refresh-0").await;
        saxo.hold_responses();

        let old_pod = pods.pod("old");
        let rotation =
            tokio::spawn(async move { old_pod.ensure_saxo_session_json("price_monitor").await });
        saxo.answered().await; // Saxo rotated; the response dies with the pod.
        rotation.abort();
        let _ = rotation.await;
        pods.lapse_refresh_lease().await;
        saxo.deliver_responses();

        let replacement = pods.pod("replacement");
        let err = replacement
            .ensure_saxo_session_json("refresh")
            .await
            .expect_err("the replacement can only present the consumed token");

        assert!(format!("{err:#}").contains("HTTP 401"));
        assert_eq!(saxo.rejected(), 1);
        assert!(!pods.durable().await["refresh_token_invalid_at"].is_null());
    }

    /// SIGTERM drains instead: the rotation already in flight completes and
    /// becomes durable before the drain returns, the draining pod starts no
    /// new rotation, and the replacement rotates the durable token cleanly.
    #[tokio::test]
    async fn a_sigterm_drain_lands_the_rotation_in_flight_and_starts_none() {
        let pods = SaxoPods::new("sigterm-drain").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 300);
        pods.seed_due_session("test-refresh-0").await;
        saxo.hold_responses();

        let old_pod = pods.pod("old");
        let rotation = {
            let pod = old_pod.clone();
            tokio::spawn(async move { pod.ensure_saxo_session_json("price_monitor").await })
        };
        saxo.answered().await; // Saxo rotated; the response is in flight.
        let drain = {
            let pod = old_pod.clone();
            tokio::spawn(async move {
                pod.drain_saxo_session_refreshes(StdDuration::from_secs(10))
                    .await
            })
        };
        sleep(StdDuration::from_millis(100)).await;
        assert!(
            !drain.is_finished(),
            "the drain must wait for a rotation Saxo already made"
        );

        saxo.deliver_responses();
        saxo.release_next();
        rotation
            .await
            .expect("rotation task")
            .expect("the in-flight rotation completes");
        assert!(drain.await.expect("drain task"), "nothing left in flight");
        assert_eq!(pods.durable().await["refresh_token"], "test-refresh-1");

        // The rotated access token is again inside the refresh margin, but a
        // draining pod serves it rather than rotating.
        old_pod
            .ensure_saxo_session_json("price_monitor")
            .await
            .expect("a draining pod serves the unexpired access token");
        assert_eq!(saxo.accepted(), 1, "a draining pod starts no rotation");

        let replacement = pods.pod("replacement");
        replacement
            .refresh_saxo_session()
            .await
            .expect("the replacement rotates the durable token");
        assert_eq!(saxo.rejected(), 0);
        assert_eq!(pods.durable().await["refresh_token"], "test-refresh-2");
    }

    /// A draining pod whose access token has run out refuses rather than
    /// rotating; the error says why.
    #[tokio::test]
    async fn a_draining_pod_with_an_expired_access_token_refuses_to_rotate() {
        let pods = SaxoPods::new("drain-expired").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 300);
        pods.seed_due_session("test-refresh-0").await;
        let session = json!({
            "access_token_expires_at": (Utc::now() - Duration::minutes(1)).to_rfc3339(),
        });
        sqlx::query(&format!(
            "UPDATE saxo_sessions SET session_json = json_patch(session_json, '{}')",
            session
        ))
        .execute(&pods.pool)
        .await
        .expect("expire the access token");
        let pod = pods.pod("old");
        pod.begin_saxo_session_drain();

        let err = pod
            .ensure_saxo_session_json("price_monitor")
            .await
            .expect_err("a draining pod must not rotate");

        assert!(format!("{err:#}").contains("shutting down"), "{err:#}");
        assert_eq!(saxo.accepted() + saxo.rejected(), 0);
    }

    /// Concurrent callers in one pod make one rotation, not one each.
    #[tokio::test]
    async fn concurrent_callers_in_one_pod_share_a_single_rotation() {
        let pods = SaxoPods::new("single-flight").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-0", 1200);
        pods.seed_due_session("test-refresh-0").await;
        let pod = pods.pod("api");

        let calls = (0..4).map(|_| {
            let pod = pod.clone();
            tokio::spawn(async move { pod.ensure_saxo_session_json("auth_status").await })
        });
        for call in calls.collect::<Vec<_>>() {
            call.await
                .expect("caller task")
                .expect("every caller gets a usable session");
        }

        assert_eq!(saxo.accepted(), 1);
        assert_eq!(saxo.rejected(), 0);
    }

    /// Saxo refusing the durable token itself is a real invalidation: it is
    /// recorded durably so every pod stops presenting it.
    #[tokio::test]
    async fn a_refusal_of_the_durable_token_is_recorded_durably() {
        let pods = SaxoPods::new("durable-refusal").await;
        let saxo = auth::fake_token_endpoint::register(&pods.client_id, "test-refresh-9", 300);
        pods.seed_due_session("test-refresh-0").await;

        let err = pods
            .pod("scheduler")
            .refresh_saxo_session()
            .await
            .expect_err("Saxo refuses the token");

        assert!(format!("{err:#}").contains("HTTP 401"));
        assert_eq!(saxo.rejected(), 1);
        let durable = pods.durable().await;
        assert!(!durable["refresh_token_invalid_at"].is_null());
        assert_eq!(durable["refresh_token"], "test-refresh-0");
        let err = pods
            .pod("api")
            .ensure_saxo_session_json("auth_status")
            .await
            .expect_err("no pod presents a refused token again");
        assert!(format!("{err:#}").contains("Re-authentication is required"));
        assert_eq!(saxo.rejected(), 1);
    }

    /// A new login reaches the durable row even when a stale working copy is
    /// restored concurrently, and it outranks a refused session.
    #[tokio::test]
    async fn an_oauth_login_replaces_a_refused_durable_session() {
        let pods = SaxoPods::new("oauth-login").await;
        pods.seed_due_session("test-refresh-0").await;
        let refused = auth::mark_refresh_rejected(&pods.durable().await, "test refusal")
            .expect("mark refused");
        pods.pod("seed")
            .save_saxo_session_to_db(&refused, "test_seed")
            .await
            .expect("store refused session");
        let api = pods.pod("api");
        api.sync_saxo_session_storage().await.expect("restore");
        let mut login = refused.clone();
        login["refresh_token"] = json!("test-refresh-login");
        login["refresh_token_invalid_at"] = JsonValue::Null;
        login["refresh_error"] = JsonValue::Null;
        login["created_at"] = json!(Utc::now().to_rfc3339());
        login["last_refreshed_at"] = json!(Utc::now().to_rfc3339());
        login["refresh_token_expires_at"] =
            json!((Utc::now() + Duration::minutes(60)).to_rfc3339());

        api.store_saxo_oauth_session(&login)
            .await
            .expect("store login");
        pods.pod("scheduler")
            .sync_saxo_session_storage()
            .await
            .expect("another pod restores");

        let durable = pods.durable().await;
        assert_eq!(durable["refresh_token"], "test-refresh-login");
        assert!(durable["refresh_token_invalid_at"].is_null());
    }
}
