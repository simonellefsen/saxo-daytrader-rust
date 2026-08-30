//! Read-only Execution dashboard projections.
//!
//! These bounds only select which persisted local execution-order rows the
//! dashboard reads. They deliberately do not change broker synchronization,
//! order lifecycle, reconciliation, or Saxo mutation behavior.

use serde_json::Value as JsonValue;

use crate::models::{
    DashboardExecutionEventPayload, DashboardExecutionFillPayload, ProtectiveStopCoveragePayload,
    ProtectiveStopCoverageSummaryPayload, ProtectiveStopLifecycleTestPayload,
    ProtectiveStopPrecheckPayload,
};
use crate::saxo_error::execution_error_taxonomy_for_code;

pub(crate) const EXECUTION_ORDERS_PAGE_SIZE: i64 = 25;
pub(crate) const OVERVIEW_EXECUTION_ORDERS_LIMIT: i64 = 12;
pub(crate) const SHARED_EXECUTION_ORDERS_LIMIT: i64 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionOrderWindow {
    pub(crate) page: i64,
    pub(crate) page_size: i64,
    pub(crate) offset: i64,
}

pub(crate) fn execution_order_window(
    active_view: &str,
    requested_page: i64,
    total_orders: i64,
) -> ExecutionOrderWindow {
    if active_view != "execution" {
        let page_size = if active_view == "overview" {
            OVERVIEW_EXECUTION_ORDERS_LIMIT
        } else {
            SHARED_EXECUTION_ORDERS_LIMIT
        };
        return ExecutionOrderWindow {
            page: 1,
            page_size,
            offset: 0,
        };
    }

    let total_pages = ((total_orders.max(0) + EXECUTION_ORDERS_PAGE_SIZE - 1)
        / EXECUTION_ORDERS_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    ExecutionOrderWindow {
        page,
        page_size: EXECUTION_ORDERS_PAGE_SIZE,
        offset: (page - 1) * EXECUTION_ORDERS_PAGE_SIZE,
    }
}

/// Decodes the compact fill facts rendered on the Execution tab. Raw Saxo
/// fill payloads stay outside the dashboard SSR model and cannot become an
/// accidental browser-facing transport path.
pub(crate) fn dashboard_execution_fills_from_json(
    fills: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardExecutionFillPayload>> {
    fills.into_iter().map(serde_json::from_value).collect()
}

/// Decodes only the stable lifecycle facts needed by the flat Execution-tab
/// event list. The persisted raw Saxo response never enters this SSR model.
/// Failure-stage and taxonomy labels originate in local order processing, so
/// the decoder retains only the fixed vocabulary shown in the dashboard.
pub(crate) fn dashboard_execution_events_from_json(
    events: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardExecutionEventPayload>> {
    events
        .into_iter()
        .map(|event| {
            let failure_stage = dashboard_execution_event_failure_stage(&event);
            let (failure_category, failure_remediation, failure_retry_policy) =
                dashboard_execution_event_error_taxonomy(&event);
            let mut event: DashboardExecutionEventPayload = serde_json::from_value(event)?;
            event.failure_stage = failure_stage;
            event.failure_category = failure_category;
            event.failure_remediation = failure_remediation;
            event.failure_retry_policy = failure_retry_policy;
            Ok(event)
        })
        .collect()
}

fn dashboard_execution_event_failure_stage(event: &JsonValue) -> Option<String> {
    let payload = dashboard_execution_event_payload(event)?;
    match payload.get("failure_stage").and_then(JsonValue::as_str) {
        Some(
            stage @ ("local_validation" | "precheck_guard" | "request_build" | "precheck"
            | "placement" | "execution" | "queue_expiry"),
        ) => Some(stage.to_string()),
        _ => None,
    }
}

/// Projects only the locally-created, versioned Saxo error taxonomy attached
/// to an execution event. Broker messages and arbitrary raw payload fields are
/// deliberately ignored. Unknown taxonomy versions or codes stay hidden until
/// their dashboard vocabulary has been reviewed.
fn dashboard_execution_event_error_taxonomy(
    event: &JsonValue,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(payload) = dashboard_execution_event_payload(event) else {
        return (None, None, None);
    };
    let Some(taxonomy) = payload.get("error_taxonomy") else {
        return (None, None, None);
    };
    if taxonomy.get("version").and_then(JsonValue::as_i64) != Some(1) {
        return (None, None, None);
    }
    let Some(code) = taxonomy.get("code").and_then(JsonValue::as_str) else {
        return (None, None, None);
    };
    let Some(taxonomy) = execution_error_taxonomy_for_code(code) else {
        return (None, None, None);
    };
    let label = taxonomy.get("label").and_then(JsonValue::as_str);
    let remediation = taxonomy.get("remediation").and_then(JsonValue::as_str);
    let retry_policy = taxonomy.get("retry_policy").and_then(JsonValue::as_str);
    match (label, remediation, retry_policy) {
        (Some(label), Some(remediation), Some(retry_policy)) => (
            Some(code.to_string()),
            Some(label.to_string() + ": " + remediation),
            Some(retry_policy.to_string()),
        ),
        _ => (None, None, None),
    }
}

fn dashboard_execution_event_payload(event: &JsonValue) -> Option<JsonValue> {
    match event.get("raw_payload_json")? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

/// Decodes the stable protective-stop coverage boundary used by the Execution
/// tab. Detailed broker and lifecycle evidence stays staged JSON.
pub(crate) fn dashboard_protective_stop_coverage_from_json(
    coverage: JsonValue,
) -> serde_json::Result<ProtectiveStopCoveragePayload> {
    serde_json::from_value(coverage)
}

/// Supplies the explicit, read-only state for dashboard views that do not load
/// protective-stop coverage. It does not alter any placement or lifecycle path.
pub(crate) fn dashboard_protective_stop_coverage_not_loaded() -> ProtectiveStopCoveragePayload {
    ProtectiveStopCoveragePayload {
        status: "not_loaded".to_string(),
        summary: ProtectiveStopCoverageSummaryPayload::default(),
        positions: Vec::new(),
        exceptions: Vec::new(),
        recent_prechecks: Vec::new(),
        recent_lifecycle_tests: Vec::new(),
        safety: "not_loaded_outside_execution_tab".to_string(),
        interpretation: String::new(),
    }
}

/// Projects persisted SIM prechecks into the small dashboard/form contract.
///
/// This never grants placement authority: the only form identifier is the
/// local precheck id, and the existing handler reloads it, validates SIM and
/// accepted status, and requires a separate confirmation before reaching Saxo.
pub(crate) fn protective_stop_precheck_payloads(
    rows: Vec<JsonValue>,
) -> Vec<ProtectiveStopPrecheckPayload> {
    rows.into_iter()
        .map(|row| {
            let status = execution_json_text(&row, "status");
            let result = execution_embedded_json(&row, "result_json").unwrap_or(JsonValue::Null);
            let result_label = result
                .get("error")
                .and_then(|error| error.get("label"))
                .and_then(JsonValue::as_str)
                .unwrap_or_else(|| {
                    if status == "precheck_ok" {
                        "Accepted"
                    } else {
                        "Review required"
                    }
                })
                .to_string();
            let safety = result
                .get("safety")
                .and_then(JsonValue::as_str)
                .unwrap_or("no Saxo order placement")
                .to_string();
            ProtectiveStopPrecheckPayload {
                id: execution_i64(&row, "id"),
                created_at: execution_json_text(&row, "created_at"),
                symbol: execution_json_text(&row, "symbol"),
                quantity: execution_f64(&row, "quantity"),
                stop_price_local: execution_f64(&row, "stop_price_local"),
                status,
                result_label,
                safety,
            }
        })
        .collect()
}

/// Projects lifecycle records into their dashboard/action-link contract.
///
/// The output carries no broker response document. Its local id remains only a
/// pointer for existing handlers, which reload the record before reconciling
/// or cancelling anything at Saxo.
pub(crate) fn protective_stop_lifecycle_test_payloads(
    rows: Vec<JsonValue>,
) -> Vec<ProtectiveStopLifecycleTestPayload> {
    rows.into_iter()
        .map(|row| ProtectiveStopLifecycleTestPayload {
            id: execution_i64(&row, "id"),
            created_at: execution_json_text(&row, "created_at"),
            symbol: execution_json_text(&row, "symbol"),
            quantity: execution_f64(&row, "quantity"),
            stop_price_local: execution_f64(&row, "stop_price_local"),
            status: execution_json_text(&row, "status"),
            broker_order_id: execution_json_text(&row, "broker_order_id"),
        })
        .collect()
}

fn execution_embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn execution_json_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn execution_f64(value: &JsonValue, key: &str) -> f64 {
    value
        .get(key)
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|value| value as f64))
                .or_else(|| value.as_str()?.parse().ok())
        })
        .unwrap_or(0.0)
}

fn execution_i64(value: &JsonValue, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|value| value as i64))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pages_execution_and_bounds_other_tabs() {
        assert_eq!(
            execution_order_window("execution", 2, 56),
            ExecutionOrderWindow {
                page: 2,
                page_size: EXECUTION_ORDERS_PAGE_SIZE,
                offset: EXECUTION_ORDERS_PAGE_SIZE,
            }
        );
        assert_eq!(
            execution_order_window("execution", 99, 26),
            ExecutionOrderWindow {
                page: 2,
                page_size: EXECUTION_ORDERS_PAGE_SIZE,
                offset: EXECUTION_ORDERS_PAGE_SIZE,
            }
        );
        assert_eq!(
            execution_order_window("overview", 5, 500),
            ExecutionOrderWindow {
                page: 1,
                page_size: OVERVIEW_EXECUTION_ORDERS_LIMIT,
                offset: 0,
            }
        );
        assert_eq!(
            execution_order_window("markov", 5, 500),
            ExecutionOrderWindow {
                page: 1,
                page_size: SHARED_EXECUTION_ORDERS_LIMIT,
                offset: 0,
            }
        );
    }

    #[test]
    fn dashboard_fills_keep_raw_broker_payloads_outside_ssr() {
        let fills = dashboard_execution_fills_from_json(vec![json!({
            "id": 91,
            "created_at": "2026-08-23T18:15:59Z",
            "execution_order_id": 345,
            "broker_order_id": "SAXO-123",
            "symbol": "AMD:xnas",
            "side": "BUY",
            "fill_status": "FinalFill",
            "order_status": "broker_final_fill",
            "cumulative_quantity": 4.0,
            "delta_quantity": 4.0,
            "average_price_local": 193.12,
            "currency": "USD",
            "ledger_id": 811,
            "raw_payload_json": {"AccountKey": "must-not-reach-the-dashboard"}
        })])
        .expect("stable fill evidence decodes");

        assert_eq!(fills[0].execution_order_id, 345);
        assert_eq!(fills[0].ledger_id, Some(811));
        assert!(
            !serde_json::to_string(&fills)
                .expect("typed fill evidence serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_execution_fills_from_json(vec![json!({
                "id": 91,
                "symbol": "AMD:xnas"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_events_keep_raw_broker_payloads_outside_ssr() {
        let events = dashboard_execution_events_from_json(vec![json!({
            "id": 188,
            "created_at": "2026-08-24T08:30:00Z",
            "execution_order_id": 345,
            "event_type": "execution_failed",
            "broker_status": "execution_failed",
            "raw_payload_json": {
                "failure_stage": "precheck",
                "error_taxonomy": {
                    "version": 1,
                    "code": "tick_size",
                    "label": "must-not-reach-the-dashboard",
                    "remediation": "must-not-reach-the-dashboard",
                    "retry_policy": "must-not-reach-the-dashboard"
                },
                "AccountKey": "must-not-reach-the-dashboard",
                "Message": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable execution-event evidence decodes");

        assert_eq!(events[0].execution_order_id, 345);
        assert_eq!(events[0].failure_stage.as_deref(), Some("precheck"));
        assert_eq!(events[0].failure_category.as_deref(), Some("tick_size"));
        assert_eq!(
            events[0].failure_retry_policy.as_deref(),
            Some("review_and_resubmit")
        );
        let serialized = serde_json::to_string(&events).expect("typed event evidence serializes");
        assert!(!serialized.contains("must-not-reach-the-dashboard"));
        assert!(serialized.contains("Invalid tick size"));
        assert!(
            dashboard_execution_events_from_json(vec![json!({
                "event_type": "execution_failed"
            })])
            .is_err()
        );
    }

    #[test]
    fn dashboard_events_hide_unknown_or_incomplete_taxonomy() {
        let events = dashboard_execution_events_from_json(vec![json!({
            "created_at": "2026-08-24T08:30:00Z",
            "execution_order_id": 345,
            "event_type": "execution_failed",
            "raw_payload_json": {
                "failure_stage": "queue_expiry",
                "error_taxonomy": {
                    "version": 1,
                    "code": "unreviewed_code",
                    "label": "must-not-reach-the-dashboard",
                    "remediation": "must-not-reach-the-dashboard",
                    "retry_policy": "must-not-reach-the-dashboard"
                }
            }
        })])
        .expect("stable event with unknown taxonomy decodes");

        assert_eq!(events[0].failure_stage.as_deref(), Some("queue_expiry"));
        assert_eq!(events[0].failure_category, None);
        assert_eq!(events[0].failure_remediation, None);
        assert_eq!(events[0].failure_retry_policy, None);
    }

    #[test]
    fn protective_stop_coverage_keeps_broker_and_indicator_documents_staged() {
        let coverage = dashboard_protective_stop_coverage_from_json(json!({
            "status": "attention_required",
            "summary": {
                "protected_count": 4,
                "unprotected_count": 1,
                "raw_broker_document": {"account": "must not reach dashboard"}
            },
            "positions": [{
                "symbol": "NOVO-B:xcse",
                "quantity": 12,
                "currency": "DKK",
                "confirmed_covered_quantity": 12,
                "active_stop_price_local": 780.0,
                "raw_broker_document": {"account": "must not reach dashboard"}
            }],
            "exceptions": [{
                "symbol": "NOVO-B:xcse",
                "unprotected_quantity": 12,
                "reason": "missing_stop",
                "proposed_stop": {
                    "stop_price_local": 780.0,
                    "reference_close": 800.0,
                    "atr14": 10.0,
                    "atr_multiple": 2.0,
                    "distance_pct": 2.5,
                    "raw_indicator_document": {"must": "stay staged"}
                },
                "raw_broker_document": {"account": "must not reach dashboard"}
            }],
            "recent_prechecks": [],
            "recent_lifecycle_tests": [],
            "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
            "interpretation": "Coverage is a local audit."
        }))
        .expect("protective-stop fixture has the dashboard contract");

        assert_eq!(coverage.status, "attention_required");
        assert_eq!(coverage.summary.protected_count, 4);
        assert_eq!(coverage.summary.unprotected_count, 1);
        assert_eq!(coverage.positions[0].symbol, "NOVO-B:xcse");
        assert_eq!(coverage.positions[0].active_stop_price_local, Some(780.0));
        assert_eq!(coverage.exceptions.len(), 1);
        assert_eq!(coverage.exceptions[0].unprotected_quantity, 12.0);
        assert_eq!(
            coverage.exceptions[0]
                .proposed_stop
                .as_ref()
                .map(|proposal| proposal.stop_price_local),
            Some(780.0)
        );
        let serialized = serde_json::to_value(&coverage)
            .expect("typed protective-stop coverage serializes")
            .to_string();
        assert!(!serialized.contains("raw_broker_document"));
        assert!(!serialized.contains("raw_indicator_document"));
        assert_eq!(
            dashboard_protective_stop_coverage_not_loaded().status,
            "not_loaded"
        );
        assert!(
            dashboard_protective_stop_coverage_from_json(json!({"status": "covered"})).is_err()
        );
    }

    #[test]
    fn protective_stop_precheck_payloads_allowlist_result_and_safety_display() {
        let payloads = protective_stop_precheck_payloads(vec![json!({
            "id": 42,
            "created_at": "2026-08-28T12:00:00Z",
            "environment": "sim",
            "symbol": "NOVO-B:xcse",
            "quantity": 12,
            "stop_price_local": 780.0,
            "status": "precheck_ok",
            "result_json": "{\"error\":{\"label\":\"Accepted by simulated broker\"},\"safety\":\"precheck_only_no_order_placement\",\"raw_saxo_response\":{\"account\":\"must stay internal\"}}",
            "raw_broker_document": {"account": "must stay internal"}
        })]);

        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].id, 42);
        assert_eq!(payloads[0].result_label, "Accepted by simulated broker");
        assert_eq!(payloads[0].safety, "precheck_only_no_order_placement");
        let serialized = serde_json::to_value(&payloads)
            .expect("typed precheck payload serializes")
            .to_string();
        assert!(!serialized.contains("raw_saxo_response"));
        assert!(!serialized.contains("raw_broker_document"));
        assert!(!serialized.contains("result_json"));
    }

    #[test]
    fn protective_stop_lifecycle_payloads_allowlist_action_link_fields() {
        let payloads = protective_stop_lifecycle_test_payloads(vec![json!({
            "id": 43,
            "created_at": "2026-08-28T12:10:00Z",
            "updated_at": "2026-08-28T12:12:00Z",
            "source_precheck_id": 42,
            "environment": "sim",
            "symbol": "NOVO-B:xcse",
            "quantity": 12,
            "stop_price_local": 780.0,
            "status": "broker_working",
            "broker_order_id": "SIM-123",
            "external_reference": "must stay internal",
            "request_id": "must stay internal",
            "placement_result_json": {"raw_saxo_response": {"account": "must stay internal"}},
            "cancellation_result_json": {"raw_saxo_response": {"account": "must stay internal"}},
            "reconciliation_json": {"raw_saxo_response": {"account": "must stay internal"}}
        })]);

        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].id, 43);
        assert_eq!(payloads[0].status, "broker_working");
        assert_eq!(payloads[0].broker_order_id, "SIM-123");
        let serialized = serde_json::to_value(&payloads)
            .expect("typed lifecycle payload serializes")
            .to_string();
        assert!(!serialized.contains("source_precheck_id"));
        assert!(!serialized.contains("external_reference"));
        assert!(!serialized.contains("request_id"));
        assert!(!serialized.contains("raw_saxo_response"));
    }
}
