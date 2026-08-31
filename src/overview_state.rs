//! Read-only Overview dashboard projections.
//!
//! These helpers narrow persisted market, integrity, and Trading Manager
//! evidence for dashboard rendering. They cannot refresh providers, alter a
//! manager gate, queue an order, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{MarketStatusPayload, OverviewIntegrityPayload, TradingManagerPayload};
use crate::read_model;

/// Decodes the stable, read-only Market Status envelope used by Overview.
pub(crate) fn dashboard_market_status_from_json(
    market_status: JsonValue,
) -> serde_json::Result<MarketStatusPayload> {
    read_model::decode("dashboard_market_status", market_status)
}

/// Decodes the stable Integrity status used by the dashboard. Individual
/// issue rows remain typed and allowlisted because their fields are check-specific.
pub(crate) fn dashboard_integrity_from_json(
    integrity: JsonValue,
) -> serde_json::Result<OverviewIntegrityPayload> {
    read_model::decode("dashboard_integrity", integrity)
}

/// Decodes the stable Trading Manager boundary used by Overview panels. Its
/// lifecycle metadata is allowlisted while gate diagnostics remain staged.
pub(crate) fn dashboard_trading_manager_from_json(
    trading_manager: JsonValue,
) -> serde_json::Result<TradingManagerPayload> {
    read_model::decode("dashboard_trading_manager", trading_manager)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn market_status_keeps_provider_documents_outside_dashboard() {
        let market_status = dashboard_market_status_from_json(json!({
            "items": [],
            "summary": {
                "analysis_window_active": false,
                "active_markets": [],
                "active_windows": [],
                "open_active_markets": [],
                "close_active_markets": [],
                "pre_sync_markets": [],
                "last_cycle_status": null,
                "last_heartbeat_at": null,
                "next_pulse_at": null,
                "next_pulse_label": null,
                "price_monitor_status": null,
                "price_monitor_updated_at": null,
                "calendar_refresh": {
                    "status": "not_run",
                    "source": null,
                    "checked_at": null,
                    "exchange_count": null
                }
            },
            "scheduler": null,
            "price_monitor": null,
            "raw_provider_document": {"AccountKey": "must-not-reach-the-dashboard"}
        }))
        .expect("stable market status decodes");

        assert!(market_status.items.is_empty());
        assert_eq!(market_status.summary.calendar_refresh.status, "not_run");
        assert!(
            !serde_json::to_string(&market_status)
                .expect("typed market status serializes")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(dashboard_market_status_from_json(json!({"items": []})).is_err());
    }

    #[test]
    fn integrity_requires_the_stable_outer_contract() {
        let integrity = dashboard_integrity_from_json(json!({
            "healthy": false,
            "warnings": [{
                "code": "broker_cash_drift",
                "raw_broker_document": {"account": "must not reach dashboard"}
            }],
            "mismatches": [],
            "expiry_pending_orders": [{
                "id": 204,
                "symbol": "BAC:xnys",
                "raw_execution_document": {"broker_order_id": "must not reach dashboard"}
            }],
            "acknowledged_issue_count": 1,
            "checked_at": "2026-08-23T12:00:00Z"
        }))
        .expect("integrity fixture has the dashboard contract");

        assert!(!integrity.healthy);
        assert_eq!(integrity.warnings.len(), 1);
        assert_eq!(integrity.expiry_pending_orders[0].id, 204);
        assert_eq!(integrity.acknowledged_issue_count, 1);
        let serialized = serde_json::to_value(&integrity)
            .expect("typed integrity payload serializes")
            .to_string();
        assert!(!serialized.contains("raw_broker_document"));
        assert!(!serialized.contains("raw_execution_document"));
        assert!(dashboard_integrity_from_json(json!({"healthy": true})).is_err());
    }

    #[test]
    fn trading_manager_requires_the_stable_outer_contract() {
        let trading_manager = dashboard_trading_manager_from_json(json!({
            "status": "available",
            "latest_run": {
                "id": 52,
                "status": "completed",
                "created_at": "2026-08-28T08:00:00Z",
                "manager_json": {"status": "completed"},
                "error_text": "must-not-reach-dashboard",
                "technical_json": {"provider_error": "must-not-reach-dashboard"},
                "queue_result_json": {"raw": "must-not-reach-dashboard"}
            }
        }))
        .expect("Trading Manager fixture has the dashboard contract");

        assert_eq!(trading_manager.status, "available");
        assert_eq!(
            trading_manager.latest_run.as_ref().map(|run| run.id),
            Some(52)
        );
        let serialized = serde_json::to_value(trading_manager)
            .expect("typed Trading Manager payload serializes");
        assert!(serialized["latest_run"].get("error_text").is_none());
        assert!(serialized["latest_run"].get("technical_json").is_none());
        assert!(serialized["latest_run"].get("queue_result_json").is_none());
        assert!(
            dashboard_trading_manager_from_json(json!({"status": "available"}))
                .expect("a missing optional run degrades to no run")
                .latest_run
                .is_none()
        );
        assert!(dashboard_trading_manager_from_json(json!({"latest_run": null})).is_err());
    }

    #[test]
    fn an_explicit_null_never_blanks_an_overview_panel() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "items": [{
                    "code": "xnas",
                    "market": "US",
                    "timezone": "America/New_York",
                    "local_time": "2026-08-31T10:15:00",
                    "status_reason": "regular_session",
                    "session_open_local": "09:30",
                    "session_close_local": "16:00",
                    "tradable_close_local": "16:00",
                    "is_open": true,
                    "is_tradable": true,
                    "pre_analysis_sync_active": false,
                    "open_analysis_window_active": true,
                    "close_analysis_window_active": false,
                    "analysis_window_active": true,
                    "next_open_at_utc": "2026-09-01T13:30:00Z",
                    "next_open": "2026-09-01 09:30",
                    "calendar_source": "saxo_exchanges",
                    "calendar_last_checked": "2026-08-31T06:00:00Z",
                    "saxo_session_state": "Open",
                    "holiday_name": null
                }],
                "summary": {
                    "analysis_window_active": false,
                    "active_markets": [],
                    "active_windows": [],
                    "open_active_markets": [],
                    "close_active_markets": [],
                    "pre_sync_markets": [],
                    "last_cycle_status": "completed",
                    "calendar_refresh": {"status": "not_run"}
                },
                "scheduler": null,
                "price_monitor": null
            }),
            dashboard_market_status_from_json,
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "healthy": false,
                "warnings": [{
                    "code": "broker_cash_drift",
                    "severity": "warning",
                    "message": "cash drifted"
                }],
                "mismatches": [],
                "expiry_pending_orders": [{"id": 204, "symbol": "BAC:xnys"}],
                "acknowledged_issue_count": 1,
                "checked_at": "2026-08-23T12:00:00Z"
            }),
            dashboard_integrity_from_json,
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "status": "available",
                "latest_run": {
                    "id": 52,
                    "status": "completed",
                    "created_at": "2026-08-28T08:00:00Z",
                    "manager_json": {"status": "completed", "gate": null}
                }
            }),
            dashboard_trading_manager_from_json,
        );
    }
}
