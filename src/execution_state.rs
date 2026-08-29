//! Read-only Execution dashboard projections.
//!
//! These bounds only select which persisted local execution-order rows the
//! dashboard reads. They deliberately do not change broker synchronization,
//! order lifecycle, reconciliation, or Saxo mutation behavior.

use serde_json::Value as JsonValue;

use crate::models::{DashboardExecutionEventPayload, DashboardExecutionFillPayload};

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
/// Failure-stage labels originate in local order processing, so the decoder
/// retains only the fixed vocabulary shown in the dashboard.
pub(crate) fn dashboard_execution_events_from_json(
    events: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardExecutionEventPayload>> {
    events
        .into_iter()
        .map(|event| {
            let failure_stage = dashboard_execution_event_failure_stage(&event);
            let mut event: DashboardExecutionEventPayload = serde_json::from_value(event)?;
            event.failure_stage = failure_stage;
            Ok(event)
        })
        .collect()
}

fn dashboard_execution_event_failure_stage(event: &JsonValue) -> Option<String> {
    let payload = match event.get("raw_payload_json")? {
        JsonValue::String(value) => serde_json::from_str(value).ok()?,
        value => value.clone(),
    };
    match payload.get("failure_stage").and_then(JsonValue::as_str) {
        Some(
            stage @ ("local_validation" | "precheck_guard" | "request_build" | "precheck"
            | "placement" | "execution"),
        ) => Some(stage.to_string()),
        _ => None,
    }
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
                "AccountKey": "must-not-reach-the-dashboard",
                "Message": "must-not-reach-the-dashboard"
            }
        })])
        .expect("stable execution-event evidence decodes");

        assert_eq!(events[0].execution_order_id, 345);
        assert_eq!(events[0].failure_stage.as_deref(), Some("precheck"));
        let serialized = serde_json::to_string(&events).expect("typed event evidence serializes");
        assert!(!serialized.contains("must-not-reach-the-dashboard"));
        assert!(
            dashboard_execution_events_from_json(vec![json!({
                "event_type": "execution_failed"
            })])
            .is_err()
        );
    }
}
