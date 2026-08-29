//! Read-only Decision Report dashboard projections.
//!
//! These typed summaries narrow retained Decision Report metadata for dashboard
//! status cards. They cannot invoke a provider, generate a report, change a
//! manager gate, queue an order, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{DecisionPulseStatusPayload, LatestDecisionStatusPayload};

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

/// Decodes the compact lifecycle state shown in the shared report-pulse cards
/// and operations banner. Detailed report payloads remain on their lazy-loaded
/// views, so unexpected fields cannot cross this SSR boundary.
pub(crate) fn dashboard_decision_pulse_statuses_from_json(
    statuses: Vec<JsonValue>,
) -> serde_json::Result<Vec<DecisionPulseStatusPayload>> {
    statuses.into_iter().map(serde_json::from_value).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn latest_decision_requires_stable_status_metadata() {
        let latest = dashboard_latest_decision_from_json(json!({
            "id": 312,
            "created_at": "2026-08-23T12:00:00Z",
            "status": "completed",
            "model": "openai/gpt-5",
            "error_text": null,
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
            "key": "us_mid_session_shadow",
            "prefix": "us_mid_session_shadow:",
            "label": "US 14:15 Shadow",
            "enabled": true,
            "latest": {
                "id": 345,
                "created_at": "2026-08-23T18:15:00Z",
                "status": "completed",
                "provider_payload": "must-not-reach-the-dashboard"
            },
            "last_success": null,
            "last_failure": null,
            "attempts_7d": 5,
            "report_prompt": "must-not-reach-the-dashboard"
        })])
        .expect("stable decision-pulse status decodes");

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
            dashboard_decision_pulse_statuses_from_json(vec![json!({
                "key": "manual"
            })])
            .is_err()
        );
    }
}
