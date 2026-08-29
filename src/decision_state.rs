//! Read-only Decision Report dashboard projections.
//!
//! These typed summaries narrow retained report metadata and offline gate-replay
//! evidence. They cannot invoke a provider, change a manager gate, queue an
//! order, or reach Saxo.

use serde_json::Value as JsonValue;

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
};

/// Decodes compact latest-report metadata used outside the detailed Decisions
/// view. Full Decision Report detail remains staged JSON.
pub(crate) fn dashboard_latest_decision_from_json(
    decision: JsonValue,
) -> serde_json::Result<LatestDecisionStatusPayload> {
    if decision.is_null() {
        Ok(LatestDecisionStatusPayload::default())
    } else {
        serde_json::from_value(decision)
    }
}

/// Decodes compact report-pulse lifecycle cards. Detailed report payloads stay
/// on lazy-loaded views, so unexpected fields cannot cross this SSR boundary.
pub(crate) fn dashboard_decision_pulse_statuses_from_json(
    statuses: Vec<JsonValue>,
) -> serde_json::Result<Vec<DecisionPulseStatusPayload>> {
    statuses.into_iter().map(serde_json::from_value).collect()
}

/// Decodes the stable Decision Gate Replay envelope used by the Decisions tab.
/// Scenario and support-risk evidence remain staged historical-analysis JSON.
pub(crate) fn dashboard_decision_gate_replay_from_json(
    replay: JsonValue,
) -> serde_json::Result<DecisionGateReplayPayload> {
    serde_json::from_value(replay)
}

/// Supplies the explicit, offline-only state for views that do not load gate
/// replay evidence. It cannot create a report, change a manager gate, or reach
/// Saxo.
pub(crate) fn dashboard_decision_gate_replay_not_loaded() -> DecisionGateReplayPayload {
    DecisionGateReplayPayload {
        status: "not_loaded".to_string(),
        run_count: 0,
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
}
