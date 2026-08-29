//! Read-only Decision Report dashboard projections.
//!
//! These typed summaries narrow retained report metadata and offline gate-replay
//! evidence. They cannot invoke a provider, change a manager gate, queue an
//! order, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{
    DashboardDecisionReportSummaryPayload, DecisionGateReplayPayload, DecisionPulseStatusPayload,
    LatestDecisionStatusPayload, SupportRiskEvidencePayload,
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
}
