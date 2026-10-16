//! Read-only Decision Report dashboard projections.
//!
//! These typed summaries narrow retained report metadata and offline gate-replay
//! evidence. They cannot invoke a provider, change a manager gate, queue an
//! order, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::{
    db::{clamp_limit, sql_escape},
    debug_redaction::{DEBUG_PAYLOAD_MAX_CHARS, compact_json_redacted},
    models::{DecisionReportDebugPayload, DecisionReportDebugPayloads},
    state::{
        ADVISORY_VISIBLE_EXPERIMENT_VARIABLES, AppState, DECISION_REPORT_DETAIL_COLUMNS,
        DECISION_REPORT_SUMMARY_COLUMNS, SUPPORTED_EXPERIMENT_VARIABLES,
        TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS, average_or_null, fraction_or_null,
        matching_order_advice, text_value,
    },
};
use crate::{
    db::{value_f64, value_i64},
    debug_redaction::compact_debug_text,
    models::{
        CandidateScoringWaterfallCandidatePayload, CandidateScoringWaterfallConcentrationPayload,
        CandidateScoringWaterfallCostGuardPayload, CandidateScoringWaterfallHermesPayload,
        CandidateScoringWaterfallHoldingLimitPayload, CandidateScoringWaterfallMarketPayload,
        CandidateScoringWaterfallMarkovPayload, CandidateScoringWaterfallPayload,
        CandidateScoringWaterfallPositionWeightPayload, CandidateScoringWaterfallSummaryPayload,
        CandidateScoringWaterfallTechnicalPayload, DashboardDecisionReportSummaryPayload,
        DashboardSelectedDecisionPayload, DecisionGateReplayPayload, DecisionPulseStatusPayload,
        LatestDecisionStatusPayload, SupportRiskEvidencePayload,
    },
    read_model,
};
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use tracing::warn;

/// Decodes compact latest-report metadata used outside the detailed Decisions
/// view. Full Decision Report detail remains staged JSON.
pub(crate) fn dashboard_latest_decision_from_json(
    decision: JsonValue,
) -> serde_json::Result<LatestDecisionStatusPayload> {
    if decision.is_null() {
        Ok(LatestDecisionStatusPayload::default())
    } else {
        read_model::decode("dashboard_latest_decision", decision)
    }
}

/// Decodes compact report-pulse lifecycle cards. Detailed report payloads stay
/// on lazy-loaded views, so unexpected fields cannot cross this SSR boundary.
pub(crate) fn dashboard_decision_pulse_statuses_from_json(
    statuses: Vec<JsonValue>,
) -> serde_json::Result<Vec<DecisionPulseStatusPayload>> {
    read_model::decode_each("dashboard_decision_pulse_statuses", statuses)
}

/// Decodes the stable Decision Gate Replay envelope used by the Decisions tab.
/// Scenario and support-risk evidence remain staged historical-analysis JSON.
pub(crate) fn dashboard_decision_gate_replay_from_json(
    replay: JsonValue,
) -> serde_json::Result<DecisionGateReplayPayload> {
    read_model::decode("dashboard_decision_gate_replay", replay)
}

/// Supplies the explicit, offline-only state for views that do not load gate
/// replay evidence. It cannot create a report, change a manager gate, or reach
/// Saxo.
pub(crate) fn dashboard_decision_gate_replay_not_loaded() -> DecisionGateReplayPayload {
    DecisionGateReplayPayload {
        status: "not_loaded".to_string(),
        run_count: 0,
        evidence_generations: Vec::new(),
        evidence_is_mixed: false,
        evidence_caveat: String::new(),
        supported_variable_coverage: Vec::new(),
        scenarios: Vec::new(),
        safety: "not_loaded_outside_decisions_tab".to_string(),
        interpretation: String::new(),
        support_risk_evidence: SupportRiskEvidencePayload::default(),
    }
}

/// Decodes bounded Decision Report summaries for the dashboard and public API.
/// Detailed report/provider documents remain on selected-report and lazy debug
/// paths.
pub(crate) fn decision_report_summaries_from_json(
    reports: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardDecisionReportSummaryPayload>> {
    reports
        .into_iter()
        .map(|report| {
            Ok(DashboardDecisionReportSummaryPayload {
                id: required_i64(&report, "id")?,
                created_at: required_string(&report, "created_at")?,
                status: required_string(&report, "status")?,
                model: optional_string(&report, "model")?.unwrap_or_default(),
                analysis_pulse_key: optional_string(&report, "analysis_pulse_key")?
                    .unwrap_or_default(),
                analysis_pulse_label: optional_string(&report, "analysis_pulse_label")?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_string(row: &JsonValue, key: &str) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_i64(row: &JsonValue, key: &str) -> serde_json::Result<i64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

/// Decodes the selected Decision Report's stable outer fields. Detailed report
/// and provider diagnostics remain compatibility JSON, while the deterministic
/// manager-gate waterfall has a typed dashboard boundary.
pub(crate) fn dashboard_selected_decision_from_json(
    decision: JsonValue,
) -> serde_json::Result<Option<DashboardSelectedDecisionPayload>> {
    if decision.is_null() {
        return Ok(None);
    }
    Ok(Some(DashboardSelectedDecisionPayload {
        id: required_i64(&decision, "id")?,
        created_at: required_string(&decision, "created_at")?,
        report_date: optional_string(&decision, "report_date")?.unwrap_or_default(),
        model: optional_string(&decision, "model")?.unwrap_or_default(),
        status: required_string(&decision, "status")?,
        analysis_window_active: optional_boolish(&decision, "analysis_window_active")?
            .unwrap_or(false),
        response_id: optional_string(&decision, "response_id")?.unwrap_or_default(),
        prompt_text: optional_string(&decision, "prompt_text")?.unwrap_or_default(),
        request_json: embedded_json(&decision, "request_json").unwrap_or(JsonValue::Null),
        response_json: embedded_json(&decision, "response_json").unwrap_or(JsonValue::Null),
        report_json: embedded_json(&decision, "report_json").unwrap_or(JsonValue::Null),
        error_text: optional_string(&decision, "error_text")?
            .map(|value| compact_debug_text(&value, 420))
            .unwrap_or_default(),
        analysis_pulse_key: optional_string(&decision, "analysis_pulse_key")?.unwrap_or_default(),
        analysis_pulse_label: optional_string(&decision, "analysis_pulse_label")?
            .unwrap_or_default(),
        pulse_mode: optional_string(&decision, "pulse_mode")?.unwrap_or_default(),
        queue_eligible: optional_boolish(&decision, "queue_eligible")?.unwrap_or(false),
        candidate_scoring_waterfall: candidate_scoring_waterfall_from_json(
            decision.get("candidate_scoring_waterfall"),
        ),
    }))
}

fn candidate_scoring_waterfall_from_json(
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
            .map(candidate_scoring_candidate_from_json)
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

fn candidate_scoring_candidate_from_json(
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
        technical: candidate_scoring_technical_from_json(value.get("technical")),
        final_technical: value
            .get("final_technical")
            .filter(|value| value.is_object())
            .map(|value| candidate_scoring_technical_from_json(Some(value))),
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

fn candidate_scoring_technical_from_json(
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

fn embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}
fn optional_boolish(row: &JsonValue, key: &str) -> serde_json::Result<Option<bool>> {
    match row.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(value)) => Ok(Some(*value)),
        Some(JsonValue::Number(value)) if value.as_i64() == Some(0) => Ok(Some(false)),
        Some(JsonValue::Number(value)) if value.as_i64() == Some(1) => Ok(Some(true)),
        Some(JsonValue::String(value)) if value == "0" || value.eq_ignore_ascii_case("false") => {
            Ok(Some(false))
        }
        Some(JsonValue::String(value)) if value == "1" || value.eq_ignore_ascii_case("true") => {
            Ok(Some(true))
        }
        _ => serde_json::from_value::<bool>(JsonValue::Null).map(Some),
    }
}
fn json_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

const GATE_REPLAY_MAX_CHANGE_ROWS: usize = 30;

const GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL: f64 = 0.25;

const GATE_REPLAY_MIN_CONFLUENCES: i64 = 4;

/// One plausible tightening step from the shipped 0.02, chosen because it is
/// reachable rather than round: on 2026-09-01 it would have cut the EU-open
/// budget from 24,941 to 19,415 DKK, which is the difference between funding
/// both candidates in that report and funding one.
const GATE_REPLAY_MIN_CASH_BUFFER_PCT: f64 = 0.05;

const DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT: i64 = 50;

/// How many recent decision reports `latest_symbol_decisions` scans for
/// per-symbol sentiment. Two weekday pulses put this at roughly four weeks.
const SYMBOL_DECISION_REPORT_SCAN: i64 = 40;

/// Fold one decision report's per-symbol view into `decisions`, newest-first.
///
/// Existing entries are never overwritten: callers iterate reports in
/// descending id order, so the first writer for a symbol is its most recent
/// view. Both halves of a symbol's decision come from the same report, so a
/// symbol mentioned in an older report cannot have its sentiment paired with a
/// newer report's action.
pub(crate) fn merge_report_symbol_decisions(
    report: &JsonValue,
    decisions: &mut HashMap<String, JsonValue>,
) {
    let report_json = report
        .get("report_json")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let base = |symbol: &str| {
        json!({
            "symbol": symbol,
            "report_id": report.get("id").cloned().unwrap_or(JsonValue::Null),
            "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
            "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
            "pulse_key": report.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
            "pulse_label": report.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
            "pulse_mode": report.get("pulse_mode").cloned().unwrap_or(JsonValue::Null),
            "queue_eligible": report.get("queue_eligible").cloned().unwrap_or(JsonValue::Null),
        })
    };

    for entry in report_json
        .get("symbol_sentiment")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let symbol = text_value(entry, "symbol");
        if symbol.is_empty() || decisions.contains_key(&symbol) {
            continue;
        }
        let mut decision = base(&symbol);
        if let Some(object) = decision.as_object_mut() {
            object.insert(
                "sentiment".to_string(),
                entry.get("sentiment").cloned().unwrap_or(JsonValue::Null),
            );
            object.insert(
                "confidence".to_string(),
                JsonValue::from(value_f64(entry, "confidence")),
            );
            object.insert(
                "rationale".to_string(),
                entry.get("rationale").cloned().unwrap_or(JsonValue::Null),
            );
        }
        decisions.insert(symbol, decision);
    }

    // A suggested trade carries the concrete action and the technical snapshot
    // the sparkline reads. It may name a symbol the sentiment list omits, so it
    // can create an entry as well as enrich one -- but only within this report.
    for trade in report_json
        .get("suggested_trades")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let symbol = text_value(trade, "symbol");
        if symbol.is_empty() {
            continue;
        }
        let owns_entry = match decisions.get(&symbol) {
            // Only enrich an entry this same report created; a newer report's
            // entry must not absorb this older report's action.
            Some(existing) => existing.get("report_id") == report.get("id"),
            None => true,
        };
        if !owns_entry {
            continue;
        }
        let entry = decisions
            .entry(symbol.clone())
            .or_insert_with(|| base(&symbol));
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        object.insert(
            "action".to_string(),
            trade.get("action").cloned().unwrap_or(JsonValue::Null),
        );
        object.insert(
            "strategy_role".to_string(),
            trade
                .get("strategy_role")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        if let Some(technical) = trade
            .get("strategy_metadata")
            .and_then(|metadata| metadata.get("technical"))
            .cloned()
        {
            object.insert(
                "trend_bias".to_string(),
                JsonValue::from(text_value(&technical, "trend_bias")),
            );
            object.insert("source".to_string(), json!({ "technical": technical }));
        }
    }
}

fn candidate_scoring_key(value: &JsonValue) -> String {
    let strategy_key = json_text(value, "strategy_key");
    if !strategy_key.trim().is_empty() {
        return format!("strategy:{strategy_key}");
    }
    format!(
        "order:{}:{}",
        json_text(value, "symbol"),
        json_text(value, "action").to_ascii_uppercase()
    )
}

pub(crate) fn candidate_gate_code_from_reason(reason: &str) -> &'static str {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.starts_with("hermes context") {
        "hermes_context"
    } else if normalized.starts_with("hermes advisory") {
        "hermes_advice"
    } else if normalized.starts_with("candidate limit") {
        "candidate_limit"
    } else if normalized.starts_with("exchange concentration cap") {
        "concentration_exchange"
    } else if normalized.starts_with("currency concentration cap") {
        "concentration_currency"
    } else if normalized.starts_with("concentration cap configuration")
        || normalized.starts_with("concentration cap requires")
    {
        "concentration"
    } else if normalized.starts_with("exchange ") {
        "market_open"
    } else if normalized.starts_with("symbol is excluded") {
        "risk_exclusion"
    } else if normalized.starts_with("instrument quarantine") {
        "instrument_quarantine"
    } else if normalized.starts_with("order quantity") {
        "quantity"
    } else if normalized.starts_with("unsupported order")
        || normalized.contains("orders require")
        || normalized.contains("order shape")
    {
        "order_shape"
    } else if normalized.starts_with("monthly-loss circuit breaker") {
        "monthly_loss_breaker"
    } else if normalized.starts_with("portfolio drawdown guardrail") {
        "drawdown_guardrail"
    } else if normalized.starts_with("buy would exceed available cash budget") {
        "cash_budget"
    } else if normalized.contains("risk-per-trade") {
        "risk_per_trade"
    } else if normalized.contains("position-weight cap") {
        "position_weight"
    } else if normalized.contains("holding cap") {
        "max_holdings"
    } else if normalized.starts_with("selection cap") {
        "max_selected_assets"
    } else if normalized.starts_with("cost guard") {
        "cost_guard"
    } else if normalized.contains("commission-efficiency floor") {
        "commission_floor"
    } else if normalized.starts_with("estimated trade value") {
        "minimum_trade_value"
    } else if normalized.starts_with("no broker-authoritative sellable") {
        "sellable_quantity"
    } else if normalized.contains("markov") {
        "markov"
    } else if normalized.contains("technical") || normalized.starts_with("only ") {
        "technical"
    } else {
        "other"
    }
}

pub(crate) fn candidate_gate_code(value: &JsonValue) -> String {
    let configured = json_text(value, "gate_code");
    if matches!(
        configured.as_str(),
        "approved"
            | "hermes_context"
            | "hermes_advice"
            | "candidate_limit"
            | "market_open"
            | "risk_exclusion"
            | "instrument_quarantine"
            | "quantity"
            | "order_shape"
            | "monthly_loss_breaker"
            | "drawdown_guardrail"
            | "cash_budget"
            | "risk_per_trade"
            | "position_weight"
            | "max_holdings"
            | "concentration"
            | "concentration_exchange"
            | "concentration_currency"
            | "max_selected_assets"
            | "cost_guard"
            | "commission_floor"
            | "minimum_trade_value"
            | "sellable_quantity"
            | "markov"
            | "technical"
            | "other"
    ) {
        return configured;
    }
    candidate_gate_code_from_reason(&json_text(value, "technical_gate")).to_string()
}

fn compact_candidate_technical(value: &JsonValue) -> JsonValue {
    let technical = value.get("technical").unwrap_or(&JsonValue::Null);
    json!({
        "status": json_text(technical, "status"),
        "sentiment": json_text(technical, "sentiment"),
        "trend_bias": json_text(technical, "trend_bias"),
        "confluence_count": value_i64(technical, "confluence_count"),
        "min_confluences": value_i64(technical, "min_confluences"),
    })
}

pub(crate) fn compact_candidate_final_technical(value: &JsonValue) -> JsonValue {
    let technical = value.get("final_technical").unwrap_or(&JsonValue::Null);
    if technical.is_object() {
        return json!({
            "status": json_text(technical, "status"),
            "source": json_text(technical, "source"),
            "run_date": json_text(technical, "run_date"),
            "sentiment": json_text(technical, "sentiment"),
            "trend_bias": json_text(technical, "trend_bias"),
            "confluence_count": value_i64(technical, "confluence_count"),
            "min_confluences": value_i64(technical, "min_confluences"),
        });
    }
    legacy_final_technical_from_gate_reason(value).unwrap_or(JsonValue::Null)
}

fn compact_candidate_cost_guard(value: &JsonValue) -> JsonValue {
    let guard = value.get("final_cost_guard").unwrap_or(&JsonValue::Null);
    if !guard.is_object() {
        return JsonValue::Null;
    }
    json!({
        "verified_from_db": guard.get("verified_from_db").and_then(JsonValue::as_bool).unwrap_or(false),
        "estimated_slippage_bps": value_f64(guard, "estimated_slippage_bps"),
        "cost_guard_multiple": value_f64(guard, "cost_guard_multiple"),
        "expected_reward_dkk": value_f64(guard, "expected_reward_dkk"),
        "round_trip_commission_dkk": value_f64(guard, "round_trip_commission_dkk"),
        "one_way_slippage_dkk": value_f64(guard, "one_way_slippage_dkk"),
        "required_reward_dkk": value_f64(guard, "required_reward_dkk"),
        "passes": guard.get("passes").and_then(JsonValue::as_bool).unwrap_or(false),
        "basis": json_text(guard, "basis"),
    })
}

fn compact_candidate_holding_limit(value: &JsonValue) -> JsonValue {
    let holding_limit = value.get("final_holding_limit").unwrap_or(&JsonValue::Null);
    if !holding_limit.is_object() {
        return JsonValue::Null;
    }
    json!({
        "verified_from_state": holding_limit.get("verified_from_state").and_then(JsonValue::as_bool).unwrap_or(false),
        "max_holdings": value_i64(holding_limit, "max_holdings"),
        "holding_count_before": value_i64(holding_limit, "holding_count_before"),
        "already_held": holding_limit.get("already_held").and_then(JsonValue::as_bool).unwrap_or(false),
    })
}

fn compact_candidate_concentration(value: &JsonValue) -> JsonValue {
    let concentration = value.get("final_concentration").unwrap_or(&JsonValue::Null);
    if !concentration.is_object() {
        return JsonValue::Null;
    }
    json!({
        "status": json_text(concentration, "status"),
        "verified_from_state": concentration.get("verified_from_state").and_then(JsonValue::as_bool).unwrap_or(false),
        "max_assets_per_exchange": value_i64(concentration, "max_assets_per_exchange"),
        "max_assets_per_currency": value_i64(concentration, "max_assets_per_currency"),
        "exchange": json_text(concentration, "exchange"),
        "currency": json_text(concentration, "currency"),
        "exchange_count_before": value_i64(concentration, "exchange_count_before"),
        "currency_count_before": value_i64(concentration, "currency_count_before"),
        "already_held": concentration.get("already_held").and_then(JsonValue::as_bool).unwrap_or(false),
        "unmapped_exchange_symbol_count": value_i64(concentration, "unmapped_exchange_symbol_count"),
        "unmapped_currency_symbol_count": value_i64(concentration, "unmapped_currency_symbol_count"),
    })
}

fn compact_candidate_position_weight(value: &JsonValue) -> JsonValue {
    let position_weight = value
        .get("final_position_weight")
        .unwrap_or(&JsonValue::Null);
    if !position_weight.is_object() {
        return JsonValue::Null;
    }
    json!({
        "verified_from_state": position_weight.get("verified_from_state").and_then(JsonValue::as_bool).unwrap_or(false),
        "max_position_weight": value_f64(position_weight, "max_position_weight"),
        "current_position_value_dkk": value_f64(position_weight, "current_position_value_dkk"),
        "approved_value_dkk": value_f64(position_weight, "approved_value_dkk"),
        "resulting_position_value_dkk": value_f64(position_weight, "resulting_position_value_dkk"),
        "max_position_value_dkk": value_f64(position_weight, "max_position_value_dkk"),
    })
}

/// Recover only the known, deterministic SELL gate result from pre-persistence
/// manager runs. This never exposes the stored reason text to the dashboard.
fn legacy_final_technical_from_gate_reason(value: &JsonValue) -> Option<JsonValue> {
    let reason = json_text(value, "technical_gate");
    let normalized = reason.trim().to_ascii_lowercase();
    let remainder = normalized.strip_prefix("sell not approved; technical sentiment is ")?;
    let (sentiment, trend) = remainder.split_once(" with ")?;
    let (trend, _) = trend.split_once(" trend.")?;
    let sentiment = match sentiment {
        "sell" => "SELL",
        "underweight" => "UNDERWEIGHT",
        "hold" => "HOLD",
        "overweight" => "OVERWEIGHT",
        "buy" => "BUY",
        _ => return None,
    };
    if !matches!(trend, "bullish" | "neutral" | "bearish") {
        return None;
    }
    Some(json!({
        "status": "ok",
        "source": "recorded_gate_reason",
        "run_date": "",
        "sentiment": sentiment,
        "trend_bias": trend,
        "confluence_count": 0,
        "min_confluences": 0,
    }))
}

fn compact_candidate_markov(value: &JsonValue) -> JsonValue {
    let markov = value.get("markov").unwrap_or(&JsonValue::Null);
    json!({
        "status": json_text(markov, "status"),
        "fresh": markov.get("fresh").and_then(JsonValue::as_bool).unwrap_or(false),
        "direction": json_text(markov, "direction"),
        "signed_signal": value_f64(markov, "signed_signal"),
        "age_days": value_i64(markov, "age_days"),
    })
}

fn candidate_recorded_technical(candidate: &JsonValue) -> &JsonValue {
    let final_technical = candidate.get("final_technical").unwrap_or(&JsonValue::Null);
    if json_text(final_technical, "status") == "ok" {
        final_technical
    } else {
        candidate.get("technical").unwrap_or(&JsonValue::Null)
    }
}

fn replay_buy_technical_passes(candidate: &JsonValue, min_confluences: i64) -> Option<bool> {
    if json_text(candidate, "action").to_ascii_uppercase() != "BUY" {
        return None;
    }
    let technical = candidate_recorded_technical(candidate);
    if json_text(technical, "status") != "ok" {
        return None;
    }
    let sentiment = json_text(technical, "sentiment").to_ascii_uppercase();
    let trend_bias = json_text(technical, "trend_bias").to_ascii_lowercase();
    Some(
        matches!(sentiment.as_str(), "BUY" | "OVERWEIGHT")
            && trend_bias == "bullish"
            && value_i64(technical, "confluence_count") >= min_confluences.max(1),
    )
}

fn replay_recorded_min_confluences(candidate: &JsonValue) -> Option<i64> {
    let minimum = value_i64(candidate_recorded_technical(candidate), "min_confluences");
    (minimum > 0).then_some(minimum)
}

fn replay_recorded_markov_minimum(run: &JsonValue) -> Option<f64> {
    run.get("manager_json")
        .and_then(|value| value.get("hermes_preflight"))
        .and_then(|value| value.get("markov"))
        .and_then(|value| value.get("min_signed_signal"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn gate_replay_change_row(
    run: &JsonValue,
    candidate: &JsonValue,
    effect: &str,
    recorded_value: JsonValue,
    proposed_value: JsonValue,
) -> JsonValue {
    json!({
        "manager_run_id": value_i64(run, "id"),
        "report_id": value_i64(run, "report_id"),
        "created_at": json_text(run, "created_at"),
        "symbol": json_text(candidate, "symbol"),
        "action": json_text(candidate, "action"),
        "recorded_outcome": json_text(candidate, "outcome"),
        "recorded_gate": json_text(candidate, "gate_code"),
        "effect": effect,
        "recorded_value": recorded_value,
        "proposed_value": proposed_value,
    })
}

pub(crate) fn gate_replay_markov_scenario(runs: &[JsonValue]) -> JsonValue {
    let mut candidate_count = 0usize;
    let mut evaluated_count = 0usize;
    let mut would_block_count = 0usize;
    let mut would_clear_target_gate_only_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut insufficient_evidence_count = 0usize;
    let mut changes = Vec::new();

    for run in runs {
        let waterfall = candidate_scoring_waterfall_from_manager_run(run);
        for candidate in waterfall
            .get("candidates")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            candidate_count += 1;
            if json_text(candidate, "action").to_ascii_uppercase() != "BUY" {
                not_reached_count += 1;
                continue;
            }
            let Some(recorded_minimum) = replay_recorded_markov_minimum(run) else {
                insufficient_evidence_count += 1;
                continue;
            };
            let Some(technical_passes) = replay_buy_technical_passes(
                candidate,
                replay_recorded_min_confluences(candidate).unwrap_or(1),
            ) else {
                insufficient_evidence_count += 1;
                continue;
            };
            if technical_passes {
                not_reached_count += 1;
                continue;
            }
            let markov = candidate.get("markov").unwrap_or(&JsonValue::Null);
            if json_text(markov, "status") != "ok"
                || !markov
                    .get("fresh")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                || json_text(markov, "direction") != "long"
            {
                insufficient_evidence_count += 1;
                continue;
            }
            evaluated_count += 1;
            let signal = value_f64(markov, "signed_signal");
            let recorded_passes = signal >= recorded_minimum;
            let proposed_passes = signal >= GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL;
            let effect = match (recorded_passes, proposed_passes) {
                (true, false) => "would_block_target_gate",
                (false, true) => "would_clear_target_gate_only",
                _ => "unchanged_target_gate",
            };
            match effect {
                "would_block_target_gate" => would_block_count += 1,
                "would_clear_target_gate_only" => would_clear_target_gate_only_count += 1,
                _ => unchanged_count += 1,
            }
            if effect != "unchanged_target_gate" && changes.len() < GATE_REPLAY_MAX_CHANGE_ROWS {
                changes.push(gate_replay_change_row(
                    run,
                    candidate,
                    effect,
                    json!({"min_signed_signal": recorded_minimum, "signed_signal": signal}),
                    json!({"min_signed_signal": GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL, "signed_signal": signal}),
                ));
            }
        }
    }

    json!({
        "variable_path": "strategy.swing.markov_gate.min_signed_signal",
        "proposed_value": GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL,
        "comparison": "fresh long starter fallback only; historical technical-gate evidence must first reject the BUY",
        "summary": {
            "candidate_count": candidate_count,
            "evaluated_count": evaluated_count,
            "would_block_target_gate_count": would_block_count,
            "would_clear_target_gate_only_count": would_clear_target_gate_only_count,
            "unchanged_target_gate_count": unchanged_count,
            "not_reached_count": not_reached_count,
            "insufficient_evidence_count": insufficient_evidence_count,
        },
        "reachability": if candidate_count == 0 {
            "no_candidates"
        } else if evaluated_count == 0 {
            // The gate's precondition never held, so the threshold was never
            // consulted. Distinct from "evaluated and nothing changed", and a
            // proposal built on this is reasoning about a value that did
            // nothing during the whole retained window.
            "unreachable_in_retained_evidence"
        } else {
            "evaluated"
        },
        "changes": changes,
    })
}

fn gate_replay_technical_scenario(runs: &[JsonValue]) -> JsonValue {
    let mut candidate_count = 0usize;
    let mut evaluated_count = 0usize;
    let mut would_block_count = 0usize;
    let mut would_clear_target_gate_only_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut insufficient_evidence_count = 0usize;
    let mut changes = Vec::new();

    for run in runs {
        let waterfall = candidate_scoring_waterfall_from_manager_run(run);
        for candidate in waterfall
            .get("candidates")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            candidate_count += 1;
            let Some(recorded_minimum) = replay_recorded_min_confluences(candidate) else {
                insufficient_evidence_count += 1;
                continue;
            };
            let Some(recorded_passes) = replay_buy_technical_passes(candidate, recorded_minimum)
            else {
                if json_text(candidate, "action").to_ascii_uppercase() == "BUY" {
                    insufficient_evidence_count += 1;
                } else {
                    not_reached_count += 1;
                }
                continue;
            };
            evaluated_count += 1;
            let proposed_passes =
                replay_buy_technical_passes(candidate, GATE_REPLAY_MIN_CONFLUENCES)
                    .unwrap_or(false);
            let effect = match (recorded_passes, proposed_passes) {
                (true, false) => "would_block_target_gate",
                (false, true) => "would_clear_target_gate_only",
                _ => "unchanged_target_gate",
            };
            match effect {
                "would_block_target_gate" => would_block_count += 1,
                "would_clear_target_gate_only" => would_clear_target_gate_only_count += 1,
                _ => unchanged_count += 1,
            }
            if effect != "unchanged_target_gate" && changes.len() < GATE_REPLAY_MAX_CHANGE_ROWS {
                changes.push(gate_replay_change_row(
                    run,
                    candidate,
                    effect,
                    json!({
                        "min_confluences": recorded_minimum,
                        "confluence_count": value_i64(candidate_recorded_technical(candidate), "confluence_count"),
                    }),
                    json!({
                        "min_confluences": GATE_REPLAY_MIN_CONFLUENCES,
                        "confluence_count": value_i64(candidate_recorded_technical(candidate), "confluence_count"),
                    }),
                ));
            }
        }
    }

    json!({
        "variable_path": "strategy.swing.daily_indicators.min_confluences",
        "proposed_value": GATE_REPLAY_MIN_CONFLUENCES,
        "comparison": "BUY technical gate only; SELL rules do not use the confluence threshold",
        "summary": {
            "candidate_count": candidate_count,
            "evaluated_count": evaluated_count,
            "would_block_target_gate_count": would_block_count,
            "would_clear_target_gate_only_count": would_clear_target_gate_only_count,
            "unchanged_target_gate_count": unchanged_count,
            "not_reached_count": not_reached_count,
            "insufficient_evidence_count": insufficient_evidence_count,
        },
        "reachability": if candidate_count == 0 {
            "no_candidates"
        } else if evaluated_count == 0 {
            // The gate's precondition never held, so the threshold was never
            // consulted. Distinct from "evaluated and nothing changed", and a
            // proposal built on this is reasoning about a value that did
            // nothing during the whole retained window.
            "unreachable_in_retained_evidence"
        } else {
            "evaluated"
        },
        "changes": changes,
    })
}

/// Read-only counterfactual projection over persisted manager snapshots. It
/// isolates each threshold and never re-runs a decision report, calls Saxo, or
/// changes runtime configuration.
/// Distinct Markov configurations represented in the retained runs.
///
/// A replay pools the last N manager runs into one aggregate, which is only
/// meaningful while every run was produced by the same model under the same
/// threshold. The 2026-08-31 move from daily to hourly bars changed the signal
/// distribution, so evidence spanning that boundary describes two models under
/// one label -- and Hermes is explicitly permitted to propose
/// `strategy.swing.markov_gate.min_signed_signal`, which makes silently mixed
/// evidence a live hazard rather than a presentation nit.
///
/// Runs predating the fingerprint report their threshold with an unrecorded
/// model, which is itself the useful signal: the evidence is older than the
/// stamp.
fn gate_replay_evidence_generations(runs: &[JsonValue]) -> Vec<JsonValue> {
    let mut generations: Vec<(JsonValue, JsonValue, usize)> = Vec::new();
    for run in runs {
        let markov = run
            .get("manager_json")
            .and_then(|manager| manager.get("hermes_preflight"))
            .and_then(|preflight| preflight.get("markov"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        let model = markov
            .get("model")
            .cloned()
            .unwrap_or(JsonValue::String("not_recorded".to_string()));
        let threshold = markov
            .get("min_signed_signal")
            .cloned()
            .unwrap_or(JsonValue::Null);
        match generations
            .iter_mut()
            .find(|(seen_model, seen_threshold, _)| {
                *seen_model == model && *seen_threshold == threshold
            }) {
            Some((_, _, count)) => *count += 1,
            None => generations.push((model, threshold, 1)),
        }
    }
    generations
        .into_iter()
        .map(|(model, min_signed_signal, run_count)| {
            json!({"model": model, "min_signed_signal": min_signed_signal, "run_count": run_count})
        })
        .collect()
}

/// Which proposable variables the replay can actually speak to.
///
/// Hermes is told to check a scenario's reachability before proposing a
/// threshold, which only helps for variables a scenario covers. Two of the four
/// supported variables have no scenario at all, and silence there reads like
/// absence of a problem rather than absence of evidence -- the same confusion
/// `reachability` exists to remove. Reporting the gap keeps "we did not test
/// this" distinct from "this is fine".
fn gate_replay_variable_coverage(scenarios: &[JsonValue]) -> Vec<JsonValue> {
    SUPPORTED_EXPERIMENT_VARIABLES
        .iter()
        .map(|path| {
            let scenario = scenarios.iter().find(|scenario| {
                scenario.get("variable_path").and_then(JsonValue::as_str) == Some(*path)
            });
            let advisory_visible = ADVISORY_VISIBLE_EXPERIMENT_VARIABLES.contains(path);
            let reachability = scenario
                .and_then(|scenario| scenario.get("reachability"))
                .and_then(JsonValue::as_str)
                .unwrap_or("no_replay_scenario");
            json!({
                "variable_path": path,
                "covered": scenario.is_some(),
                "reachability": reachability,
                // Whether Hermes is handed this number in its decision
                // preflight. A variable can bind nothing deterministically and
                // still shape every candidate through the advisory.
                "advisory_visible": advisory_visible,
                "effect_path": experiment_variable_effect_path(Some(reachability), advisory_visible),
            })
        })
        .collect()
}

/// What a different cash buffer would have funded.
///
/// `strategy.capital.min_cash_buffer_pct` is one of the four variables Hermes
/// may propose and, until now, one of the two with no replay scenario at all --
/// so a proposal against it was judged on nothing. It is also the more
/// consequential of that pair: it shapes the BUY budget on every cycle, where
/// `execution.min_trade_value_dkk` is dominated by the commission floor on
/// every BUY.
///
/// The counterfactual is exact rather than modelled, because the buffer enters
/// the runtime at exactly one place:
///
/// ```text
/// required_cash_buffer     = total_market_value * min_cash_buffer_pct
/// available_cash_above     = (cash - required_cash_buffer - reservations).max(0)
/// available_buy_budget     = min(available_cash_above, remaining_deployment_capacity)
/// ```
///
/// Every input is retained on the run, and `remaining_deployment_capacity` does
/// not depend on the buffer, so it carries over unchanged. The soft multipliers
/// that the drawdown guardrail and monthly-loss breaker apply afterwards are
/// recovered as a ratio from the recorded pair and re-applied, so the replay
/// compares like with like instead of quietly dropping a live reduction.
///
/// Candidates are walked in report order because that is how the manager
/// reserves against the running budget: whether the third BUY fits depends on
/// the two before it.
pub(crate) fn gate_replay_cash_buffer_scenario(runs: &[JsonValue]) -> JsonValue {
    let mut candidate_count = 0usize;
    let mut evaluated_count = 0usize;
    let mut would_block_count = 0usize;
    let mut would_clear_target_gate_only_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut insufficient_evidence_count = 0usize;
    let mut changes = Vec::new();

    for run in runs {
        let budget = run
            .get("manager_json")
            .and_then(|manager| manager.get("hermes_preflight"))
            .and_then(|preflight| preflight.get("capital_budget"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        let waterfall = candidate_scoring_waterfall_from_manager_run(run);
        let candidates = waterfall
            .get("candidates")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        // The scoring projection carries the recorded outcome and gate code,
        // which the change rows need, but not the order's DKK value. That is
        // only on the raw preflight row, so pair the two by strategy key
        // rather than widening a payload the dashboard also renders.
        let values_by_key = gate_replay_candidate_values(run);

        let Some((mut recorded_remaining, mut proposed_remaining)) =
            gate_replay_cash_budgets(&budget)
        else {
            candidate_count += candidates.len();
            insufficient_evidence_count += candidates.len();
            continue;
        };

        for candidate in &candidates {
            candidate_count += 1;
            if json_text(candidate, "action").to_ascii_uppercase() != "BUY" {
                // The buffer reserves cash against new exposure. A SELL frees
                // it, and no SELL is ever tested against this budget.
                not_reached_count += 1;
                continue;
            }
            let Some(value_dkk) = values_by_key
                .get(&json_text(candidate, "strategy_key"))
                .copied()
                .filter(|value: &f64| value.is_finite() && *value > 0.0)
            else {
                insufficient_evidence_count += 1;
                continue;
            };
            evaluated_count += 1;
            let fits_recorded = recorded_remaining >= value_dkk;
            let fits_proposed = proposed_remaining >= value_dkk;
            if fits_recorded {
                recorded_remaining -= value_dkk;
            }
            if fits_proposed {
                proposed_remaining -= value_dkk;
            }
            let effect = match (fits_recorded, fits_proposed) {
                (true, false) => "would_block_target_gate",
                (false, true) => "would_clear_target_gate_only",
                _ => "unchanged_target_gate",
            };
            match effect {
                "would_block_target_gate" => would_block_count += 1,
                "would_clear_target_gate_only" => would_clear_target_gate_only_count += 1,
                _ => unchanged_count += 1,
            }
            if effect != "unchanged_target_gate" && changes.len() < GATE_REPLAY_MAX_CHANGE_ROWS {
                changes.push(gate_replay_change_row(
                    run,
                    candidate,
                    effect,
                    json!({
                        "min_cash_buffer_pct": value_f64(&budget, "min_cash_buffer_pct"),
                        "estimated_value_dkk": value_dkk,
                        "budget_remaining_before_dkk": recorded_remaining + if fits_recorded { value_dkk } else { 0.0 },
                    }),
                    json!({
                        "min_cash_buffer_pct": GATE_REPLAY_MIN_CASH_BUFFER_PCT,
                        "estimated_value_dkk": value_dkk,
                        "budget_remaining_before_dkk": proposed_remaining + if fits_proposed { value_dkk } else { 0.0 },
                    }),
                ));
            }
        }
    }

    json!({
        "variable_path": "strategy.capital.min_cash_buffer_pct",
        "proposed_value": GATE_REPLAY_MIN_CASH_BUFFER_PCT,
        "comparison": "BUY budget capacity only, walked in report order; SELLs never consume this budget and downstream cost or advisory gates are not re-run",
        "summary": {
            "candidate_count": candidate_count,
            "evaluated_count": evaluated_count,
            "would_block_target_gate_count": would_block_count,
            "would_clear_target_gate_only_count": would_clear_target_gate_only_count,
            "unchanged_target_gate_count": unchanged_count,
            "not_reached_count": not_reached_count,
            "insufficient_evidence_count": insufficient_evidence_count,
        },
        "reachability": if candidate_count == 0 {
            "no_candidates"
        } else if evaluated_count == 0 {
            "unreachable_in_retained_evidence"
        } else {
            "evaluated"
        },
        "changes": changes,
    })
}

/// Order value in DKK per candidate, keyed by strategy key.
///
/// Read from the raw preflight rather than the scoring projection, which drops
/// it.
fn gate_replay_candidate_values(run: &JsonValue) -> HashMap<String, f64> {
    run.get("manager_json")
        .and_then(|manager| manager.get("hermes_preflight"))
        .and_then(|preflight| preflight.get("candidate_waterfall"))
        .and_then(JsonValue::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let key = json_text(row, "strategy_key");
                    let value = row.get("estimated_value_dkk").and_then(JsonValue::as_f64)?;
                    (!key.is_empty()).then_some((key, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The recorded and counterfactual BUY budgets for one manager run.
///
/// `None` when the run predates the retained capital-budget block or carries a
/// total market value of zero, in which case the buffer is undefined rather
/// than zero -- an unmeasured run must not read as one with no budget.
pub(crate) fn gate_replay_cash_budgets(budget: &JsonValue) -> Option<(f64, f64)> {
    if !budget.is_object() {
        return None;
    }
    let total_market_value_dkk = value_f64(budget, "total_market_value_dkk");
    if !total_market_value_dkk.is_finite() || total_market_value_dkk <= 0.0 {
        return None;
    }
    let recorded_budget = value_f64(budget, "available_buy_budget_dkk");
    let unreduced = value_f64(budget, "unreduced_available_buy_budget_dkk");
    if !recorded_budget.is_finite() || !unreduced.is_finite() {
        return None;
    }
    let cash_balance_dkk = value_f64(budget, "cash_balance_dkk");
    let reservations = value_f64(budget, "persisted_buy_reservation_value_dkk").max(0.0);
    let deployment_capacity = value_f64(budget, "remaining_deployment_capacity_dkk").max(0.0);
    let proposed_cash_above_buffer = (cash_balance_dkk
        - total_market_value_dkk * GATE_REPLAY_MIN_CASH_BUFFER_PCT
        - reservations)
        .max(0.0);
    let proposed_unreduced = proposed_cash_above_buffer.min(deployment_capacity);
    // Recover the soft multiplier the guardrails applied rather than assuming
    // none was active. Dropping it would compare a reduced recorded budget
    // against an unreduced counterfactual and read as a windfall.
    let multiplier = if unreduced > 0.0 {
        (recorded_budget / unreduced).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Some((recorded_budget, proposed_unreduced * multiplier))
}

pub(crate) fn gate_replay_from_manager_runs(runs: &[JsonValue]) -> JsonValue {
    let generations = gate_replay_evidence_generations(runs);
    let mixed = generations.len() > 1;
    let scenarios = vec![
        gate_replay_markov_scenario(runs),
        gate_replay_technical_scenario(runs),
        gate_replay_cash_buffer_scenario(runs),
    ];
    let coverage = gate_replay_variable_coverage(&scenarios);
    json!({
        "status": if runs.is_empty() { "no_history" } else { "available" },
        "run_count": runs.len(),
        "evidence_generations": generations,
        "evidence_is_mixed": mixed,
        "evidence_caveat": if mixed {
            "The retained runs span more than one Markov configuration or gate threshold, so this comparison pools evidence produced by different models. Treat a target-gate flip as suggestive only, and prefer a proposal measured within a single generation."
        } else {
            "All retained runs share one Markov configuration and gate threshold."
        },
        "scenarios": scenarios,
        "supported_variable_coverage": coverage,
        "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
        "interpretation": "A target-gate clear is not an approval: other recorded gates, capital, holdings, and market conditions remain outside this isolated comparison.",
    })
}

#[derive(Default)]
struct DecisionPulseOutcomeStats {
    attributed_order_count: usize,
    buy_order_count: usize,
    sell_order_count: usize,
    execution_status_counts: BTreeMap<String, usize>,
    hermes_reviewed_order_count: usize,
    hermes_effect_counts: BTreeMap<String, usize>,
    filled_buy_order_count: usize,
    reconciled_sell_order_count: usize,
    one_session_count: usize,
    one_session_return_sum_pct: f64,
    one_session_positive_count: usize,
    five_session_count: usize,
    five_session_return_sum_pct: f64,
    five_session_positive_count: usize,
    realised_sell_gain_dkk: f64,
    realised_sell_commission_dkk: f64,
    realised_sell_tax_dkk: f64,
}

fn normalized_decision_pulse(row: &JsonValue) -> (String, String) {
    let configured_key = json_text(row, "analysis_pulse_key");
    let configured_label = json_text(row, "analysis_pulse_label");
    let strategy_type = json_text(row, "strategy_type");
    let (key, fallback_label) = if strategy_type.eq_ignore_ascii_case("portfolio_sync") {
        ("portfolio_sync", "Portfolio Sync")
    } else if configured_key.starts_with("europe_open_followup") {
        ("europe_open_followup", "EU Open +1h15")
    } else if configured_key.starts_with("us_open_followup") {
        ("us_open_followup", "US Open +1h15")
    } else if configured_key.starts_with("manual_dry_run") {
        ("manual_dry_run", "Manual Dry Run")
    } else if configured_key.starts_with("manual") {
        ("manual", "Manual")
    } else {
        ("other", "Other / legacy")
    };
    let label = if configured_label.trim().is_empty() {
        fallback_label.to_string()
    } else {
        configured_label
    };
    (key.to_string(), label)
}

fn decision_pulse_outcome_summary(stats: &DecisionPulseOutcomeStats) -> JsonValue {
    let directional_summary = |count: usize, return_sum_pct: f64, positive_count: usize| {
        json!({
            "sample_count": count,
            "average_directional_return_pct": average_or_null(return_sum_pct, count),
            "positive_return_rate": fraction_or_null(positive_count, count),
        })
    };
    json!({
        "attributed_order_count": stats.attributed_order_count,
        "buy_order_count": stats.buy_order_count,
        "sell_order_count": stats.sell_order_count,
        "execution_status_counts": stats.execution_status_counts,
        "hermes_reviewed_order_count": stats.hermes_reviewed_order_count,
        "hermes_effect_counts": stats.hermes_effect_counts,
        "filled_buy_order_count": stats.filled_buy_order_count,
        "reconciled_sell_order_count": stats.reconciled_sell_order_count,
        "one_session": directional_summary(
            stats.one_session_count,
            stats.one_session_return_sum_pct,
            stats.one_session_positive_count,
        ),
        "five_session": directional_summary(
            stats.five_session_count,
            stats.five_session_return_sum_pct,
            stats.five_session_positive_count,
        ),
        "realised_sell": {
            "realised_gain_dkk": stats.realised_sell_gain_dkk,
            "commission_dkk": stats.realised_sell_commission_dkk,
            "tax_dkk": stats.realised_sell_tax_dkk,
        },
    })
}

/// Separates observed execution outcomes by their report pulse. BUYs use later
/// stored closes as directional movement while SELLs use reconciled local-ledger
/// DKK gains. The evidence is deliberately observational: Hermes presence is
/// shown as review coverage, not proof that advice caused an outcome.
pub(crate) fn decision_pulse_outcome_evidence_from_observations(
    observations: &[JsonValue],
) -> JsonValue {
    let mut overall = DecisionPulseOutcomeStats::default();
    let mut by_pulse: BTreeMap<String, (String, DecisionPulseOutcomeStats)> = BTreeMap::new();
    for observation in observations {
        let (pulse_key, pulse_label) = normalized_decision_pulse(observation);
        let pulse = by_pulse
            .entry(pulse_key)
            .or_insert_with(|| (pulse_label, DecisionPulseOutcomeStats::default()));
        for stats in [&mut overall, &mut pulse.1] {
            stats.attributed_order_count += 1;
            let execution_status = json_text(observation, "execution_status");
            if !execution_status.is_empty() && execution_status != "not_recorded" {
                *stats
                    .execution_status_counts
                    .entry(execution_status)
                    .or_default() += 1;
            }
            if observation
                .get("hermes_reviewed")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
            {
                stats.hermes_reviewed_order_count += 1;
            }
            let hermes_effect = json_text(observation, "hermes_effect");
            if !hermes_effect.is_empty() && hermes_effect != "not_recorded" {
                *stats.hermes_effect_counts.entry(hermes_effect).or_default() += 1;
            }
            let action = json_text(observation, "action").to_uppercase();
            if action == "BUY" {
                stats.buy_order_count += 1;
                let outcome = observation
                    .get("holding_period_outcome")
                    .unwrap_or(&JsonValue::Null);
                if value_f64(outcome, "filled_quantity") > 0.0 {
                    stats.filled_buy_order_count += 1;
                }
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
            } else if action == "SELL" {
                stats.sell_order_count += 1;
                let outcome = observation
                    .get("ledger_outcome")
                    .unwrap_or(&JsonValue::Null);
                if json_text(outcome, "status") == "reconciled" {
                    stats.reconciled_sell_order_count += 1;
                    stats.realised_sell_gain_dkk += value_f64(outcome, "realised_gain_dkk");
                    stats.realised_sell_commission_dkk += value_f64(outcome, "commission_dkk");
                    stats.realised_sell_tax_dkk += value_f64(outcome, "tax_dkk");
                }
            }
        }
    }
    let status = if overall.attributed_order_count == 0 {
        "no_attributable_orders"
    } else if overall.five_session_count < TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };
    let pulses = by_pulse
        .into_iter()
        .map(|(pulse_key, (pulse_label, stats))| {
            json!({
                "pulse_key": pulse_key,
                "pulse_label": pulse_label,
                "outcome": decision_pulse_outcome_summary(&stats),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": status,
        "overall": decision_pulse_outcome_summary(&overall),
        "pulses": pulses,
        "minimum_complete_observations": TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS,
        "scan_limit": DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT,
        "safety": "read_only_local_execution_orders_fills_ledger_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "BUY rows show equal-weighted forward directional price movement after reconciled fills; they are not unrealised P/L. SELL rows sum local-ledger realised DKK gain, commission, and tax only after reconciliation. Hermes effects come only from the durable Trading Manager advice-delta snapshot matched to the stored execution-order strategy key. They classify the advice applied to an order but do not establish causal performance impact. Portfolio-sync rows are imported-state context, not system-initiated trades."
    })
}

/// Where a proposable variable can actually change an outcome.
///
/// `reachability` alone answers this only for the deterministic gate, and the
/// reflection prompt turned `unreachable_in_retained_evidence` into "a proposal
/// cannot produce a measurable effect no matter which value is chosen". For a
/// variable the advisory layer reads, that conclusion is wrong.
pub(crate) fn experiment_variable_effect_path(
    reachability: Option<&str>,
    advisory_visible: bool,
) -> &'static str {
    match (reachability == Some("evaluated"), advisory_visible) {
        (true, true) => "gate_and_advisory",
        (true, false) => "gate_only",
        (false, true) => "advisory_only",
        (false, false) => "no_observed_effect",
    }
}

fn compact_candidate_market(value: &JsonValue) -> JsonValue {
    let quarantine_active = value
        .get("instrument_quarantine")
        .and_then(|value| value.get("active"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    json!({
        "exchange": json_text(value, "exchange"),
        "exchange_open": value.get("exchange_open").and_then(JsonValue::as_bool).unwrap_or(false),
        "risk_excluded": value.get("risk_excluded").and_then(JsonValue::as_bool).unwrap_or(false),
        "quarantine_active": quarantine_active,
    })
}

fn compact_candidate_advice(value: Option<&JsonValue>) -> JsonValue {
    let Some(value) = value else {
        return json!({"effect": "not_recorded", "requested_quantity": 0.0, "resulting_quantity": 0.0});
    };
    json!({
        "effect": json_text(value, "effect"),
        "requested_quantity": value_f64(value, "requested_quantity"),
        "resulting_quantity": value_f64(value, "resulting_quantity"),
    })
}

/// Reconstructs the deterministic manager gate snapshot for a report without
/// exposing raw Hermes rationale, broker payloads, or raw execution errors.
pub(crate) fn candidate_scoring_waterfall_from_manager_run(run: &JsonValue) -> JsonValue {
    let Some(manager_json) = run.get("manager_json") else {
        return json!({
            "status": "not_processed",
            "candidates": [],
            "summary": {"candidate_count": 0, "approved_count": 0, "skipped_count": 0, "not_reached_count": 0, "gate_counts": {}},
            "safety": "sanitized_local_manager_audit",
        });
    };
    let preflight = manager_json
        .get("hermes_preflight")
        .and_then(|value| value.get("candidate_waterfall"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let advice = manager_json
        .get("hermes_advice_delta")
        .and_then(|value| value.get("candidates"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut advice_by_key = HashMap::new();
    for row in advice {
        advice_by_key.insert(candidate_scoring_key(&row), row);
    }

    let mut outcomes = HashMap::new();
    for (outcome, rows) in [
        ("approved", manager_json.get("approved_orders")),
        ("skipped", manager_json.get("skipped_orders")),
    ] {
        for row in rows.and_then(JsonValue::as_array).into_iter().flatten() {
            outcomes.insert(
                candidate_scoring_key(row),
                json!({
                    "strategy_key": json_text(row, "strategy_key"),
                    "symbol": json_text(row, "symbol"),
                    "action": json_text(row, "action"),
                    "outcome": outcome,
                    "gate_code": candidate_gate_code(row),
                    "final_technical": compact_candidate_final_technical(row),
                    "cost_guard": compact_candidate_cost_guard(row),
                    "final_holding_limit": compact_candidate_holding_limit(row),
                    "concentration": compact_candidate_concentration(row),
                    "final_position_weight": compact_candidate_position_weight(row),
                }),
            );
        }
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for row in preflight {
        let key = candidate_scoring_key(&row);
        seen.insert(key.clone());
        let outcome = outcomes
            .get(&key)
            .cloned()
            .unwrap_or_else(|| json!({"outcome": "not_reached", "gate_code": "other"}));
        candidates.push(json!({
            "strategy_key": json_text(&row, "strategy_key"),
            "symbol": json_text(&row, "symbol"),
            "action": json_text(&row, "action"),
            "order_type": json_text(&row, "order_type"),
            "quantity": value_f64(&row, "quantity"),
            "market": compact_candidate_market(&row),
            "technical": compact_candidate_technical(&row),
            "final_technical": outcome.get("final_technical").cloned().unwrap_or(JsonValue::Null),
            "cost_guard": outcome.get("cost_guard").cloned().unwrap_or(JsonValue::Null),
            "final_holding_limit": outcome.get("final_holding_limit").cloned().unwrap_or(JsonValue::Null),
            "concentration": outcome.get("concentration").cloned().unwrap_or(JsonValue::Null),
            "final_position_weight": outcome.get("final_position_weight").cloned().unwrap_or(JsonValue::Null),
            "markov": compact_candidate_markov(&row),
            "hermes": compact_candidate_advice(advice_by_key.get(&key)),
            "outcome": json_text(&outcome, "outcome"),
            "gate_code": json_text(&outcome, "gate_code"),
        }));
    }
    for (key, outcome) in outcomes {
        if seen.contains(&key) {
            continue;
        }
        let strategy_key = json_text(&outcome, "strategy_key");
        let strategy_key = if strategy_key.trim().is_empty() {
            key.strip_prefix("strategy:")
                .unwrap_or_default()
                .to_string()
        } else {
            strategy_key
        };
        candidates.push(json!({
            "strategy_key": strategy_key,
            "symbol": json_text(&outcome, "symbol"),
            "action": json_text(&outcome, "action"),
            "order_type": "",
            "quantity": 0.0,
            "market": {"exchange": "", "exchange_open": false, "risk_excluded": false, "quarantine_active": false},
            "technical": {"status": "unavailable", "sentiment": "", "trend_bias": "", "confluence_count": 0, "min_confluences": 0},
            "final_technical": outcome.get("final_technical").cloned().unwrap_or(JsonValue::Null),
            "cost_guard": outcome.get("cost_guard").cloned().unwrap_or(JsonValue::Null),
            "final_holding_limit": outcome.get("final_holding_limit").cloned().unwrap_or(JsonValue::Null),
            "concentration": outcome.get("concentration").cloned().unwrap_or(JsonValue::Null),
            "final_position_weight": outcome.get("final_position_weight").cloned().unwrap_or(JsonValue::Null),
            "markov": {"status": "unavailable", "fresh": false, "direction": "", "signed_signal": 0.0, "age_days": 0},
            "hermes": compact_candidate_advice(advice_by_key.get(&key)),
            "outcome": json_text(&outcome, "outcome"),
            "gate_code": json_text(&outcome, "gate_code"),
        }));
    }

    let mut approved_count = 0usize;
    let mut skipped_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut gate_counts: HashMap<String, usize> = HashMap::new();
    for candidate in &candidates {
        match json_text(candidate, "outcome").as_str() {
            "approved" => approved_count += 1,
            "skipped" => skipped_count += 1,
            _ => not_reached_count += 1,
        }
        *gate_counts
            .entry(json_text(candidate, "gate_code"))
            .or_default() += 1;
    }

    json!({
        "status": "available",
        "run_id": value_i64(run, "id"),
        "created_at": json_text(run, "created_at"),
        "manager_status": json_text(run, "status"),
        "candidates": candidates,
        "summary": {
            "candidate_count": approved_count + skipped_count + not_reached_count,
            "approved_count": approved_count,
            "skipped_count": skipped_count,
            "not_reached_count": not_reached_count,
            "gate_counts": gate_counts,
        },
        "safety": "sanitized_local_manager_audit",
    })
}

#[allow(dead_code)]
fn deterministic_selected_assets(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter().take(12) {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        selected.push(json!({
            "symbol": symbol,
            "score": (value_f64(row, "allocation_pct") * 100.0).max(50.0),
            "notes": "Existing portfolio holding included in the manual fallback review.",
            "source": "portfolio"
        }));
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items.iter().take(6) {
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() || !seen.insert(symbol.to_string()) {
                    continue;
                }
                selected.push(json!({
                    "symbol": symbol,
                    "score": value_f64(row, "change_pct").abs().max(50.0),
                    "notes": "Watchlist symbol included in the manual fallback review.",
                    "source": category.get("key").and_then(JsonValue::as_str).unwrap_or("watchlist")
                }));
                if selected.len() >= 20 {
                    return selected;
                }
            }
        }
    }
    selected
}

#[allow(dead_code)]
fn deterministic_symbol_sentiment(
    positions: &[JsonValue],
    selected_assets: &[JsonValue],
) -> Vec<JsonValue> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter() {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let daily = value_f64(row, "daily_pnl_dkk");
        let sentiment = if allocation > 0.15 {
            "UNDERWEIGHT"
        } else if daily < -500.0 {
            "UNDERWEIGHT"
        } else {
            "HOLD"
        };
        rows.push(json!({
            "symbol": symbol,
            "sentiment": sentiment,
            "confidence": 50.0,
            "rationale": "Manual Rust fallback based on current allocation and daily P/L.",
            "risk_notes": ["Review manually before creating orders."]
        }));
    }
    for row in selected_assets {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        rows.push(json!({
            "symbol": symbol,
            "sentiment": "HOLD",
            "confidence": value_f64(row, "score"),
            "rationale": row.get("notes").cloned().unwrap_or_else(|| JsonValue::from("Manual fallback candidate.")),
            "risk_notes": ["No automated broker order was created."]
        }));
    }
    rows
}

#[allow(dead_code)]
fn deterministic_suggested_trades(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut trades = Vec::new();
    for row in positions {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let unrealised = value_f64(row, "unrealised_pnl_dkk");
        if allocation > 0.15 || unrealised < -4000.0 {
            trades.push(json!({
                "symbol": symbol,
                "action": "SELL",
                "priority": "medium",
                "confidence": if allocation > 0.15 { 56.0 } else { 52.0 },
                "quantity_hint": "Reduce toward target allocation",
                "target_weight_pct": 5.56,
                "rationale": "Manual fallback flagged concentration or drawdown for operator review.",
                "risk_notes": ["No automatic order was queued."]
            }));
        }
        if trades.len() >= 6 {
            return trades;
        }
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items {
                if value_f64(row, "change_pct") <= 2.0 {
                    continue;
                }
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() {
                    continue;
                }
                trades.push(json!({
                    "symbol": symbol,
                    "action": "BUY",
                    "priority": "medium",
                    "confidence": value_f64(row, "change_pct").min(75.0),
                    "quantity_hint": "Review for possible starter allocation",
                    "target_weight_pct": 5.56,
                    "rationale": "Manual fallback highlighted positive watchlist momentum.",
                    "risk_notes": ["Confirm thesis, liquidity, and market window before trading."]
                }));
                if trades.len() >= 6 {
                    return trades;
                }
            }
        }
    }
    trades
}

impl AppState {
    /// Latest per-symbol decision view, read from the decision reports
    /// themselves rather than the retired `swing_*` projection tables.
    ///
    /// Those two tables (`swing_sentiment_snapshots`, `swing_position_targets`,
    /// dropped 2026-08-03 once nothing referenced them) were written only by
    /// the retired Python runtime — no `INSERT` for either ever existed in
    /// `src/`. They were frozen at `report_id = 12` (2026-05-08). Because the
    /// previous implementation selected "the most
    /// recent report that has rows in those tables," it always resolved to
    /// report 12 no matter how many newer reports existed, so every position in
    /// that report's fixed ~82-symbol US large-cap universe rendered a decision
    /// chip whose age only ever grew ("Stale · HOLD, 87d old"), and every
    /// position outside it — the Nordic holdings especially — rendered `n/a`
    /// forever. Same failure class as the `audit_log` table dropped 2026-08-03:
    /// a Python-era table nothing in Rust populates, still presented as live.
    ///
    /// `report_json` carries the live equivalents: `symbol_sentiment` (the
    /// sentiment/confidence/rationale half) and `suggested_trades` (the
    /// action/technical half, whose `strategy_metadata.technical` also drives
    /// the trend sparkline via `source.technical`). Scanning newest-first and
    /// keeping the first entry per symbol yields each symbol's most recent
    /// view, with its own report timestamp so the existing staleness horizon
    /// (`strategy.swing.position_decision_stale_after_days`) applies per symbol
    /// instead of uniformly to one frozen report.
    ///
    /// Bounded to the most recent [`SYMBOL_DECISION_REPORT_SCAN`] reports —
    /// roughly four weeks at two weekday pulses. A decision older than that
    /// should read as absent rather than as very stale advice, and the bound
    /// keeps this off the unbounded-scan path since it runs on dashboard load.
    /// JSON is parsed in Rust, not SQL, to stay portable across the SQLite and
    /// PostgreSQL backends `AnyPool` serves.
    pub(crate) async fn latest_symbol_decisions(&self) -> Result<HashMap<String, JsonValue>> {
        let reports = self
            .select_json(&format!(
                "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible, report_json
                 FROM decision_reports
                 WHERE report_json IS NOT NULL
                 ORDER BY id DESC
                 LIMIT {SYMBOL_DECISION_REPORT_SCAN}"
            ))
            .await
            .unwrap_or_default();
        let mut decisions = HashMap::new();
        for report in &reports {
            merge_report_symbol_decisions(report, &mut decisions);
        }
        Ok(decisions)
    }

    pub(crate) async fn decision_pulse_outcome_evidence(&self) -> Result<JsonValue> {
        self.decision_pulse_outcome_evidence_since(None).await
    }

    /// Builds execution-attributed evidence from local orders, fills, ledger,
    /// and persisted daily closes. An optional order-created cutoff lets the
    /// Tuning view use the same explicit observation window as its report and
    /// shadow lanes without changing the Execution tab's bounded recent view.
    pub(crate) async fn decision_pulse_outcome_evidence_since(
        &self,
        created_since: Option<&str>,
    ) -> Result<JsonValue> {
        let created_filter = created_since
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!(" AND eo.created_at >= '{}'", sql_escape(value)))
            .unwrap_or_default();
        let rows = self
            .select_json(&format!(
                "SELECT eo.id, eo.created_at, eo.report_id, eo.symbol, eo.action, eo.quantity,
                        eo.currency, eo.strategy_type, eo.strategy_key, eo.status AS execution_status,
                        dr.analysis_pulse_key, dr.analysis_pulse_label,
                        CASE WHEN EXISTS (
                            SELECT 1
                            FROM hermes_decision_advice h
                            WHERE h.decision_report_id = eo.report_id
                        ) THEN 1 ELSE 0 END AS hermes_reviewed,
                        (
                            SELECT tm.manager_json
                            FROM trading_manager_runs tm
                            WHERE tm.report_id = eo.report_id
                            ORDER BY tm.created_at DESC, tm.id DESC
                            LIMIT 1
                        ) AS manager_json
                 FROM execution_orders eo
                 LEFT JOIN decision_reports dr ON dr.id = eo.report_id
                 WHERE eo.action IN ('BUY', 'SELL')
                   AND (eo.report_id IS NOT NULL OR eo.strategy_type = 'portfolio_sync')
                   {created_filter}
                 ORDER BY eo.created_at DESC, eo.id DESC
                 LIMIT {}",
                DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT
            ))
            .await?;
        let mut observations = Vec::with_capacity(rows.len());
        for row in rows {
            let action = json_text(&row, "action").to_uppercase();
            let raw_manager_json = row.get("manager_json").cloned().unwrap_or(JsonValue::Null);
            let manager_json = if raw_manager_json.is_object() {
                raw_manager_json
            } else {
                raw_manager_json
                    .as_str()
                    .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                    .unwrap_or(JsonValue::Null)
            };
            let hermes_effect = manager_json
                .get("hermes_advice_delta")
                .and_then(|value| value.get("candidates"))
                .and_then(|value| {
                    matching_order_advice(
                        Some(value),
                        &json_text(&row, "strategy_key"),
                        &json_text(&row, "symbol"),
                        &action,
                    )
                })
                .map(|value| json_text(&value, "effect"))
                .unwrap_or_else(|| "not_recorded".to_string());
            let holding_period_outcome = if action == "BUY" {
                self.execution_order_holding_period_outcome(&row).await?
            } else {
                JsonValue::Null
            };
            let ledger_outcome = if action == "SELL" {
                self.execution_order_ledger_outcome(&row).await?
            } else {
                JsonValue::Null
            };
            observations.push(json!({
                "analysis_pulse_key": json_text(&row, "analysis_pulse_key"),
                "analysis_pulse_label": json_text(&row, "analysis_pulse_label"),
                "strategy_type": json_text(&row, "strategy_type"),
                "action": action,
                "execution_status": json_text(&row, "execution_status"),
                "hermes_reviewed": value_i64(&row, "hermes_reviewed") > 0,
                "hermes_effect": hermes_effect,
                "holding_period_outcome": holding_period_outcome,
                "ledger_outcome": ledger_outcome,
            }));
        }
        Ok(decision_pulse_outcome_evidence_from_observations(
            &observations,
        ))
    }

    pub async fn decision_report_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_SUMMARY_COLUMNS} FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_summary(&self, report_id: i64) -> Result<Option<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_SUMMARY_COLUMNS} FROM decision_reports WHERE id = {} LIMIT 1",
            report_id.max(0)
        );
        self.first_json(&sql).await
    }

    pub async fn decision_report_item(&self, report_id: i64) -> Result<Option<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_DETAIL_COLUMNS} FROM decision_reports WHERE id = {} LIMIT 1",
            report_id.max(0)
        );
        self.first_json(&sql).await
    }

    pub(crate) fn sanitized_decision_report_debug_payload(
        report: &JsonValue,
    ) -> DecisionReportDebugPayload {
        let normalized_report = report.get("report_json").unwrap_or(&JsonValue::Null);
        DecisionReportDebugPayload {
            report_id: value_i64(report, "id"),
            created_at: json_text(report, "created_at"),
            status: json_text(report, "status"),
            payloads: DecisionReportDebugPayloads {
                prompt: compact_debug_text(
                    &json_text(report, "prompt_text"),
                    DEBUG_PAYLOAD_MAX_CHARS,
                ),
                request: compact_json_redacted(report.get("request_json"), DEBUG_PAYLOAD_MAX_CHARS),
                provider_response: compact_json_redacted(
                    report.get("response_json"),
                    DEBUG_PAYLOAD_MAX_CHARS,
                ),
                normalized_report: compact_json_redacted(
                    Some(normalized_report),
                    DEBUG_PAYLOAD_MAX_CHARS,
                ),
            },
        }
    }

    pub async fn decision_report_debug_payload(
        &self,
        report_id: i64,
    ) -> Result<Option<DecisionReportDebugPayload>> {
        let Some(report) = self.decision_report_item(report_id).await? else {
            return Ok(None);
        };
        Ok(Some(Self::sanitized_decision_report_debug_payload(&report)))
    }

    pub(crate) async fn attach_decision_candidate_waterfall(
        &self,
        mut report: JsonValue,
    ) -> JsonValue {
        let report_id = value_i64(&report, "id");
        if report_id <= 0 {
            return report;
        }
        match self.decision_candidate_waterfall(report_id).await {
            Ok(waterfall) => {
                if let Some(object) = report.as_object_mut() {
                    object.insert("candidate_scoring_waterfall".to_string(), waterfall);
                }
            }
            Err(err) => {
                warn!(report_id, "candidate scoring waterfall degraded: {err:#}");
            }
        }
        report
    }

    async fn decision_candidate_waterfall(&self, report_id: i64) -> Result<JsonValue> {
        let run = self
            .first_json(&format!(
                "SELECT id, created_at, status, manager_json
                 FROM trading_manager_runs
                 WHERE report_id = {}
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                report_id.max(0)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(candidate_scoring_waterfall_from_manager_run(&run))
    }

    pub async fn decision_gate_replay(&self, limit: i64) -> Result<JsonValue> {
        let sql = format!(
            "SELECT id, report_id, created_at, status, manager_json
             FROM trading_manager_runs
             WHERE manager_json IS NOT NULL
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        let runs = self.select_json(&sql).await?;
        let mut replay = gate_replay_from_manager_runs(&runs);
        let support_risk_evidence = self.support_risk_evidence().await.unwrap_or_else(|err| {
            warn!("support-risk evidence projection degraded: {err:#}");
            json!({
                "status": "unavailable",
                "safety": "read_only_observation_of_stored_daily_indicator_closes",
                "interpretation": "Support-risk evidence could not be loaded. It does not affect gates, Hermes, configuration, or Saxo orders.",
            })
        });
        replay["support_risk_evidence"] = support_risk_evidence;
        Ok(replay)
    }

    pub async fn decision_pulse_statuses(&self) -> Result<Vec<JsonValue>> {
        let pulses = [
            (
                "europe_open_followup",
                "europe_open_followup:",
                "Nordic/EU Open +1h15",
            ),
            ("us_open_followup", "us_open_followup:", "US Open +1h15"),
            (
                "europe_mid_session_shadow",
                "europe_mid_session_shadow:",
                "Nordic/EU 14:15 Shadow",
            ),
            (
                "us_mid_session_shadow",
                "us_mid_session_shadow:",
                "US 14:15 Shadow",
            ),
            ("manual", "manual:", "Manual / Dry Run"),
        ];
        let attempt_cutoff =
            (Utc::now() - Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut statuses = Vec::new();
        for (key, prefix, label) in pulses {
            let latest = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let last_success = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND status IN ('completed', 'xai_fallback')
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let last_failure = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND status IN ('xai_error', 'error', 'failed', 'parse_error')
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let attempts = self
                .first_json(&format!(
                    "SELECT COUNT(*) AS attempts_7d
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND created_at >= '{}'",
                    sql_escape(prefix),
                    sql_escape(&attempt_cutoff)
                ))
                .await?
                .unwrap_or_else(|| json!({"attempts_7d": 0}));
            statuses.push(json!({
                "key": key,
                "prefix": prefix,
                "label": label,
                "enabled": if key == "manual" {
                    true
                } else {
                    crate::xai_decision::scheduled_pulse_is_enabled(self, key)
                },
                "latest": latest,
                "last_success": last_success,
                "last_failure": last_failure,
                "attempts_7d": value_i64(&attempts, "attempts_7d"),
            }));
        }
        Ok(statuses)
    }

    #[allow(dead_code)]
    pub async fn generate_decision_report_fallback(&self) -> Result<JsonValue> {
        // This is a conservative Rust-side generator used by the manual button.
        // It does not call the external xAI service; instead it persists a
        // transparent deterministic report with the same database shape that the
        // Rust UI consumes. This keeps manual operator snapshots auditable when
        // the primary deferred xAI path is unavailable or bypassed.
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let report_date = Utc::now().date_naive().to_string();
        let batch_id = self.latest_batch_id().await?.unwrap_or_default();
        let positions = self.position_items(250).await.unwrap_or_default();
        let watchlists = self
            .watchlists_payload()
            .await
            .unwrap_or_else(|_| json!({}));
        let selected_assets = deterministic_selected_assets(&positions, &watchlists);
        let suggested_trades = deterministic_suggested_trades(&positions, &watchlists);
        let symbol_sentiment = deterministic_symbol_sentiment(&positions, &selected_assets);
        let report_json = json!({
            "report_title": "Manual Rust fallback Decision Report",
            "status": "rust_fallback",
            "created_at": created_at,
            "reasoning_steps": [
                "Manual trigger was requested from the Rust dashboard.",
                "The primary deferred xAI decision path was unavailable or bypassed for this fallback invocation.",
                "This fallback report uses current portfolio, watchlist, cash, and allocation data to create an auditable operator snapshot."
            ],
            "market_view": {
                "bias": "neutral",
                "summary": "Deterministic Rust fallback: review current watchlist and portfolio state before submitting trades."
            },
            "portfolio_summary": {
                "position_count": positions.len(),
                "cash_balance_dkk": self.cash_buffer_value().get("cash_balance_dkk").cloned().unwrap_or(JsonValue::Null)
            },
            "strategy_status": "Rust manual fallback generated. Review suggested trades manually; no broker orders are queued by this action.",
            "strategy_flow": {
                "portfolio": positions.len(),
                "selected": selected_assets.len(),
                "trades": suggested_trades.len()
            },
            "selected_assets": selected_assets,
            "candidate_assets": symbol_sentiment,
            "symbol_sentiment": symbol_sentiment,
            "suggested_trades": suggested_trades,
        });
        let prompt_text = json!({
            "system": "Rust dashboard manual fallback. No external model call was made.",
            "user": "Generate an auditable decision snapshot from current stored portfolio/watchlist data."
        });
        let request_json = json!({
            "source": "rust_dashboard",
            "manual": true,
            "position_count": positions.len()
        });
        let sql = format!(
            "INSERT INTO decision_reports (
                created_at, report_date, batch_id, model, status, analysis_window_active,
                response_id, prompt_text, request_json, response_json, report_json,
                error_text, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible
            ) VALUES (
                '{}', '{}', '{}', 'rust-deterministic-fallback', 'rust_fallback', 0,
                NULL, '{}', '{}', NULL, '{}',
                '{}', '{}', '{}', 'execution_eligible', 1
            )",
            sql_escape(&created_at),
            sql_escape(&report_date),
            sql_escape(&batch_id),
            sql_escape(&serde_json::to_string(&prompt_text)?),
            sql_escape(&serde_json::to_string(&request_json)?),
            sql_escape(&serde_json::to_string(&report_json)?),
            sql_escape(
                "Generated by Rust fallback because external xAI decision generation was unavailable or bypassed."
            ),
            sql_escape(&format!("manual:{report_date}")),
            sql_escape("Manual Decision Report")
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("inserting manual Rust fallback decision report")?;
        let report = self
            .first_json(&format!(
                "SELECT id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible FROM decision_reports WHERE created_at = '{}' ORDER BY id DESC LIMIT 1",
                sql_escape(&created_at)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(report)
    }

    pub(crate) async fn latest_decision_summary(&self) -> Result<JsonValue> {
        let report = self
            .first_json("SELECT id, created_at, status FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT 1")
            .await?;
        Ok(report.unwrap_or_else(|| json!({"id": null, "created_at": null, "status": null})))
    }

    pub async fn strategy_journal_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, metrics_json, learnings_json, source_report_id, diary_json FROM strategy_journal_entries ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn strategy_journal_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, source_report_id FROM strategy_journal_entries ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// Claims the single manual decision-report slot. Returns false when a
    /// fresh claim is already held, so double-clicks and concurrent operators
    /// cannot start overlapping manual pipelines. A claim older than the
    /// stale window is treated as abandoned (crashed task) and taken over.
    pub async fn claim_manual_decision_report(&self) -> Result<bool> {
        const STALE_AFTER_SECONDS: i64 = 900;
        if let Some(existing) = self.runtime_setting("manual_report_claim").await? {
            let fresh = existing
                .get("started_at")
                .and_then(JsonValue::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|started| {
                    Utc::now().signed_duration_since(started.with_timezone(&Utc))
                        < Duration::seconds(STALE_AFTER_SECONDS)
                })
                .unwrap_or(false);
            if fresh {
                return Ok(false);
            }
        }
        let value = json!({
            "started_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        self.save_runtime_setting("manual_report_claim", &value)
            .await?;
        Ok(true)
    }

    pub async fn release_manual_decision_report_claim(&self) -> Result<()> {
        sqlx::query("DELETE FROM runtime_settings WHERE key = 'manual_report_claim'")
            .execute(&self.pool)
            .await
            .context("releasing manual decision report claim")?;
        Ok(())
    }

    /// True while a spawned manual decision-report pipeline holds a fresh
    /// claim — drives the dashboard's pending banner and button state.
    pub async fn manual_decision_report_in_flight(&self) -> bool {
        const STALE_AFTER_SECONDS: i64 = 900;
        match self.runtime_setting("manual_report_claim").await {
            Ok(Some(existing)) => existing
                .get("started_at")
                .and_then(JsonValue::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|started| {
                    Utc::now().signed_duration_since(started.with_timezone(&Utc))
                        < Duration::seconds(STALE_AFTER_SECONDS)
                })
                .unwrap_or(false),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn latest_decision_requires_stable_status_metadata() {
        let latest = dashboard_latest_decision_from_json(json!({
            "id": 312, "created_at": "2026-08-23T12:00:00Z", "status": "completed",
            "model": "openai/gpt-5", "error_text": null,
            "provider_payload": "must-not-reach-the-dashboard"
        }))
        .expect("latest decision fixture has the dashboard contract");
        assert_eq!(latest.id, Some(312));
        assert_eq!(latest.status.as_deref(), Some("completed"));
        assert!(
            !serde_json::to_string(&latest)
                .expect("typed latest decision serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_latest_decision_from_json(JsonValue::Null)
                .expect("absent latest decision is explicit")
                .id
                .is_none()
        );
        assert!(dashboard_latest_decision_from_json(json!({"status": 42})).is_err());
    }

    #[test]
    fn pulse_statuses_keep_only_compact_lifecycle_metadata() {
        let statuses = dashboard_decision_pulse_statuses_from_json(vec![json!({
            "key": "us_mid_session_shadow", "prefix": "us_mid_session_shadow:",
            "label": "US 14:15 Shadow", "enabled": true,
            "latest": {"id": 345, "created_at": "2026-08-23T18:15:00Z", "status": "completed", "provider_payload": "must-not-reach-the-dashboard"},
            "last_success": null, "last_failure": null, "attempts_7d": 5,
            "report_prompt": "must-not-reach-the-dashboard"
        })]).expect("stable decision-pulse status decodes");
        assert_eq!(statuses[0].key, "us_mid_session_shadow");
        assert_eq!(
            statuses[0].latest.as_ref().map(|report| report.id),
            Some(345)
        );
        assert!(
            !serde_json::to_string(&statuses)
                .expect("typed pulse statuses serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_decision_pulse_statuses_from_json(vec![json!({"key": "manual"})]).is_err()
        );
    }

    #[test]
    fn gate_replay_keeps_indicator_documents_outside_the_dashboard() {
        let replay = dashboard_decision_gate_replay_from_json(json!({
            "status": "available", "run_count": 3,
            "scenarios": [{"variable_path": "strategy.swing.markov_gate.min_signed_signal", "proposed_value": 0.2, "comparison": "Historical comparison only.", "summary": {"candidate_count": 3, "evaluated_count": 2, "would_block_target_gate_count": 1, "would_clear_target_gate_only_count": 0, "unchanged_target_gate_count": 1, "not_reached_count": 0, "insufficient_evidence_count": 1}, "changes": []}],
            "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
            "interpretation": "A target-gate clear is not an approval.",
            "support_risk_evidence": {"status": "collecting", "eligible_signal_count": 12, "labels": [{"label": "high", "signal_count": 12, "next_run": {"sample_count": 10, "average_return_pct": -1.2}, "five_run": {"sample_count": 8, "negative_return_rate": 0.625}, "average_confidence": 0.7, "raw_indicator_document": {"must": "stay internal"}}], "raw_support_risk_document": {"must": "stay internal"}}
        })).expect("gate replay fixture has the dashboard contract");
        assert_eq!(replay.run_count, 3);
        assert_eq!(replay.scenarios[0].summary.evaluated_count, 2);
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
    fn report_summaries_keep_detail_documents_outside_responses() {
        let reports = decision_report_summaries_from_json(vec![json!({
            "id": 312, "created_at": "2026-08-26T12:00:00Z", "status": "completed", "model": "openai/gpt-5", "analysis_pulse_key": "us_open_followup:2026-08-26", "analysis_pulse_label": "US Open +1h15",
            "report_json": {"api_key": "must-not-reach-the-dashboard"}, "request_json": {"token": "must-not-reach-the-dashboard"}, "response_json": {"provider": "must-not-reach-the-dashboard"}, "prompt_text": "must-not-reach-the-dashboard", "error_text": "must-not-reach-the-dashboard"
        })]).expect("stable Decision Report summary decodes");
        assert_eq!(reports[0].id, 312);
        assert_eq!(reports[0].analysis_pulse_label, "US Open +1h15");
        assert!(
            !serde_json::to_string(&reports)
                .expect("typed Decision Report summaries serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(decision_report_summaries_from_json(vec![json!({"id": 312})]).is_err());
    }

    #[test]
    fn selected_decision_keeps_manager_detail_outside_the_dashboard_contract() {
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
                "summary": {"candidate_count": 2, "approved_count": 1, "skipped_count": 1, "not_reached_count": 0},
                "candidates": [{"symbol": "ACME:xnas", "action": "BUY", "markov": {"status": "ok", "fresh": true, "signed_signal": 0.42}, "raw_reason": "candidate-raw-detail-must-not-reach-the-dashboard"}],
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
        assert_eq!(decision.candidate_scoring_waterfall.run_id, 91);
        assert!(
            decision.candidate_scoring_waterfall.candidates[0]
                .markov
                .fresh
        );
        let serialized =
            serde_json::to_string(&decision).expect("typed selected Decision Report serializes");
        assert!(!serialized.contains("raw_reason"));
        assert!(!serialized.contains("candidate-raw-detail-must-not-reach-the-dashboard"));
        assert!(
            dashboard_selected_decision_from_json(JsonValue::Null)
                .expect("absent selected Decision Report is explicit")
                .is_none()
        );
        assert!(dashboard_selected_decision_from_json(json!({
            "id": 312, "created_at": "2026-08-26T12:00:00Z", "status": "completed", "queue_eligible": "unknown"
        })).is_err());
    }

    /// Report metadata is assembled from rows that carry nulls for a pulse
    /// that has not run or a report that never reached a model, so a null must
    /// not cost the whole card.
    #[test]
    fn an_explicit_null_never_blanks_a_decision_card() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "id": 312,
                "created_at": "2026-08-31T08:15:00Z",
                "status": "completed",
                "model": "x-ai/grok-4",
                "error_text": null
            }),
            dashboard_latest_decision_from_json,
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "key": "europe_open",
                "prefix": "EU",
                "label": "EU Open",
                "enabled": true,
                "attempts_7d": 5,
                "latest": {
                    "id": 312,
                    "created_at": "2026-08-31T08:15:00Z",
                    "status": "completed"
                },
                "last_success": null,
                "last_failure": null
            }]),
            |value| {
                dashboard_decision_pulse_statuses_from_json(
                    value.as_array().cloned().expect("pulse fixture is a list"),
                )
            },
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "status": "available",
                "run_count": 12,
                "scenarios": [{
                    "variable_path": "strategy.markov.min_signed_signal",
                    "proposed_value": 0.25,
                    "comparison": "target_gate",
                    "summary": {
                        "candidate_count": 12,
                        "evaluated_count": 12,
                        "would_block_target_gate_count": 3,
                        "would_clear_target_gate_only_count": 1,
                        "unchanged_target_gate_count": 8,
                        "not_reached_count": 0,
                        "insufficient_evidence_count": 0
                    },
                    "changes": [{
                        "manager_run_id": 91,
                        "report_id": 312,
                        "created_at": "2026-08-31T08:20:00Z",
                        "symbol": "NOVO-B:xcse",
                        "action": "BUY",
                        "recorded_outcome": "queued",
                        "recorded_gate": "markov_starter",
                        "effect": "would_block",
                        "recorded_value": 0.18,
                        "proposed_value": 0.25
                    }]
                }],
                "safety": "read_only",
                "interpretation": "offline replay",
                "support_risk_evidence": {"status": "available"}
            }),
            dashboard_decision_gate_replay_from_json,
        );
    }
}
