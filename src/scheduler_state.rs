//! Read-only Scheduler dashboard projections.
//!
//! This module bounds the persisted scheduler-cycle history shown on the
//! Execution dashboard. It does not affect scheduler cadence, work execution,
//! retention, or any broker-facing behavior.

use serde_json::Value as JsonValue;

use crate::models::{DashboardSchedulerCyclePayload, SchedulerStatusSummaryPayload};

pub(crate) const SCHEDULER_CYCLES_PAGE_SIZE: i64 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerCyclePage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn scheduler_cycle_page(requested_page: i64, total_cycles: i64) -> SchedulerCyclePage {
    let total_pages = ((total_cycles.max(0) + SCHEDULER_CYCLES_PAGE_SIZE - 1)
        / SCHEDULER_CYCLES_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    SchedulerCyclePage {
        page,
        offset: (page - 1) * SCHEDULER_CYCLES_PAGE_SIZE,
    }
}

/// Flattens only stable scheduler-cycle fields for the dashboard and public
/// API. Retained `cycle_json` can contain detailed provider and operational
/// diagnostics, so it is parsed locally and never becomes part of either
/// response. The parser accepts both the legacy JSON-string form and the
/// database adapter's parsed-object form.
pub(crate) fn scheduler_cycle_summaries_from_json(
    cycles: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardSchedulerCyclePayload>> {
    cycles
        .into_iter()
        .map(dashboard_scheduler_cycle_from_json)
        .collect()
}

/// Decodes the stable scheduler-status metadata used by the public API.
///
/// A missing status row is a normal startup condition and remains `None`.
/// A malformed stored row fails closed so callers can show an unavailable
/// status without exposing the retained cycle document or local process data.
pub(crate) fn scheduler_status_summary_from_json(
    status: JsonValue,
) -> serde_json::Result<Option<SchedulerStatusSummaryPayload>> {
    if status.is_null() {
        return Ok(None);
    }
    Ok(Some(SchedulerStatusSummaryPayload {
        started_at: required_string(&status, "started_at")?,
        last_heartbeat_at: required_string(&status, "last_heartbeat_at")?,
        last_cycle_started_at: optional_string(&status, "last_cycle_started_at")?,
        last_cycle_completed_at: optional_string(&status, "last_cycle_completed_at")?,
        last_cycle_status: required_string(&status, "last_cycle_status")?,
    }))
}

fn dashboard_scheduler_cycle_from_json(
    cycle: JsonValue,
) -> serde_json::Result<DashboardSchedulerCyclePayload> {
    let cycle_json = embedded_json(&cycle, "cycle_json").unwrap_or(JsonValue::Null);
    Ok(DashboardSchedulerCyclePayload {
        started_at: required_string(&cycle, "started_at")?,
        status: required_string(&cycle, "status")?,
        generated_decision: required_boolish(&cycle, "generated_decision")?,
        queue_status: required_string(&cycle, "queue_status")?,
        notifications_status: optional_string(&cycle, "notifications_status")?,
        duration_ms: cycle_duration_ms(&cycle_json),
        operational_notifications_status: cycle_nested_status(
            &cycle_json,
            "operational_notifications",
        ),
        portfolio_position_snapshot_integrity_status: cycle_nested_status(
            &cycle_json,
            "portfolio_position_snapshot_integrity",
        ),
    })
}

fn embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_string(row: &JsonValue, key: &str) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_boolish(row: &JsonValue, key: &str) -> serde_json::Result<bool> {
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

fn cycle_duration_ms(cycle_json: &JsonValue) -> Option<u64> {
    cycle_json.get("duration_ms").and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_f64()
                .filter(|duration| duration.is_finite() && *duration >= 0.0)
                .map(|duration| duration.round() as u64)
        })
    })
}

fn cycle_nested_status(cycle_json: &JsonValue, key: &str) -> Option<String> {
    cycle_json
        .get(key)
        .and_then(|item| item.get("status"))
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn clamps_page_and_calculates_offset() {
        assert_eq!(
            scheduler_cycle_page(2, 25),
            SchedulerCyclePage {
                page: 2,
                offset: SCHEDULER_CYCLES_PAGE_SIZE,
            }
        );
        assert_eq!(
            scheduler_cycle_page(9, 13),
            SchedulerCyclePage {
                page: 2,
                offset: SCHEDULER_CYCLES_PAGE_SIZE,
            }
        );
        assert_eq!(
            scheduler_cycle_page(0, 0),
            SchedulerCyclePage { page: 1, offset: 0 }
        );
    }

    #[test]
    fn cycle_summaries_keep_raw_cycle_documents_outside_responses() {
        let cycles = scheduler_cycle_summaries_from_json(vec![json!({
            "started_at": "2026-08-24T08:30:00Z",
            "status": "ok",
            "generated_decision": 1,
            "queue_status": "queued",
            "notifications_status": "ok",
            "cycle_json": {
                "duration_ms": 65_123,
                "operational_notifications": {"status": "ok"},
                "portfolio_position_snapshot_integrity": {"status": "warning"},
                "provider_payload": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable scheduler-cycle evidence decodes");

        assert!(cycles[0].generated_decision);
        assert_eq!(cycles[0].duration_ms, Some(65_123));
        assert_eq!(
            cycles[0]
                .portfolio_position_snapshot_integrity_status
                .as_deref(),
            Some("warning")
        );
        assert!(
            !serde_json::to_string(&cycles)
                .expect("typed scheduler-cycle evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            scheduler_cycle_summaries_from_json(vec![json!({
                "started_at": "2026-08-24T08:30:00Z"
            })])
            .is_err()
        );
    }

    #[test]
    fn status_summary_keeps_cycle_document_and_process_data_outside_api() {
        let status = scheduler_status_summary_from_json(json!({
            "started_at": "2026-08-27T06:00:00Z",
            "last_heartbeat_at": "2026-08-27T08:30:00Z",
            "last_cycle_started_at": "2026-08-27T08:29:00Z",
            "last_cycle_completed_at": "2026-08-27T08:30:00Z",
            "last_cycle_status": "ok",
            "last_cycle_json": {
                "provider_payload": "must-not-reach-the-public-api"
            },
            "scheduler_pid": 42
        }))
        .expect("stable scheduler status decodes")
        .expect("scheduler status exists");

        assert_eq!(status.last_cycle_status, "ok");
        let serialized = serde_json::to_string(&status).expect("scheduler status serializes");
        assert!(!serialized.contains("must-not-reach-the-public-api"));
        assert!(!serialized.contains("scheduler_pid"));
        assert!(
            scheduler_status_summary_from_json(JsonValue::Null)
                .expect("missing scheduler status is valid")
                .is_none()
        );
        assert!(
            scheduler_status_summary_from_json(json!({"started_at": "2026-08-27T06:00:00Z"}))
                .is_err()
        );
    }
}
