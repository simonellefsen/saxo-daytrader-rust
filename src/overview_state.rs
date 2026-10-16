//! Read-only Overview dashboard projections.
//!
//! These helpers narrow persisted market, integrity, and Trading Manager
//! evidence for dashboard rendering. They cannot refresh providers, alter a
//! manager gate, queue an order, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{
    MarketStatusPayload, OverviewIntegrityPayload, OverviewPayload, TradingManagerPayload,
};
use crate::read_model;

/// Decodes the builder's Overview JSON into the public `/api/overview`
/// contract. The builder's JSON itself is unchanged and is still what the
/// prompts and Hermes read.
pub(crate) fn public_overview_from_json(
    overview: JsonValue,
) -> serde_json::Result<OverviewPayload> {
    // The queue-result document stays out, but its status is the one part an
    // operator needs at a glance. Stored text or a decoded object, either way.
    let latest_queue_status = overview
        .pointer("/trading_manager/latest_run/queue_result_json")
        .and_then(|document| match document {
            JsonValue::String(text) => serde_json::from_str::<JsonValue>(text).ok(),
            other => Some(other.clone()),
        })
        .and_then(|document| {
            document
                .get("status")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        });
    let mut payload: OverviewPayload = read_model::decode("public_overview", overview)?;
    payload.trading_manager.latest_queue_status = latest_queue_status;
    Ok(payload)
}

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

    /// The Overview builder's shape, with every dropped document carrying a
    /// marker that must not reach the public payload.
    fn overview_fixture() -> JsonValue {
        let hidden = "must-not-reach-the-public-overview";
        json!({
            "app": {"project_name": "daytrader", "environment": "sim",
                    "config_path": format!("/app/config/{hidden}.yaml"), "runtime": "rust-dioxus"},
            "execution": {"mode": "saxo_sim", "configured_mode": "saxo_sim", "adapter": "saxo",
                          "configured_saxo_environment": "sim", "session_environment": "sim",
                          "require_approval_live": true, "max_daily_orders": 12,
                          "daily_order_capacity": {"max": 12, "used": 3, "remaining": 9},
                          "counts": {"queued": 1, "pending_approval": 0, "broker_live": 2, "failed": 0}},
            "portfolio_summary": {"recorded_at": null, "total_market_value_dkk": 240000.0,
                                  "invested_market_value_dkk": 180000.0, "cash_balance_dkk": 60000.0,
                                  "initial_cash_dkk": 300000.0, "cash_from_trades_dkk": -120000.0,
                                  "total_cost_basis_dkk": 190000.0, "total_unrealised_pnl_dkk": -10000.0,
                                  "total_daily_pnl_dkk": 450.0, "position_count": 14},
            "after_tax_summary": {"status": "estimated", "basis": "average_method", "currency": "DKK",
                                  "tax_year": 2026, "gross_unrealised_pnl_dkk": -10000.0,
                                  "estimated_tax_dkk": 0.0, "unrealised_pnl_after_tax_dkk": -10000.0,
                                  "realised_gain_ytd_dkk": -21298.0},
            "goal_tracking": {"basis": "since_reset", "weekly_target_dkk": 900.0, "monthly_target_dkk": 3600.0,
                              "periods": {
                                  "week": {"status": "behind", "period_start_utc": "2026-10-12T00:00:00Z",
                                           "baseline_value_dkk": 239000.0, "pnl_dkk": 1000.0,
                                           "target_dkk": 900.0, "progress_pct": 111.0},
                                  "month": {"status": "behind", "period_start_utc": "2026-10-01T00:00:00Z",
                                            "baseline_value_dkk": 241000.0, "pnl_dkk": -1000.0,
                                            "target_dkk": 3600.0, "progress_pct": -27.0},
                                  "since_reset": {"status": "behind", "baseline_recorded_at": "2026-07-16T19:00:00Z",
                                                  "baseline_value_dkk": 262000.0, "pnl_dkk": -22000.0,
                                                  "return_pct": -8.4}}},
            "integrity": {"healthy": false, "checked_at": "2026-10-16T08:00:00Z",
                          "acknowledged_issue_count": 0, "adopted_orders_without_ledger": 0,
                          "warnings": [{"code": "stale_snapshot", "severity": "warning", "message": "m",
                                        "issue_key": "k", "watched_positions": 3, "window_minutes": 30}],
                          "mismatches": [], "unreconciled_orders": [], "expiry_pending_orders": [],
                          "acknowledgements": [], "checks": {"positions": "ok"},
                          "config_contract": {"unused": []},
                          "pnl_reconciliation": {"status": "reconciled", "difference_dkk": 0.4},
                          "realised_sell_outcomes": {"closed_sales": 32},
                          "unrealised_pnl_attribution": {"currency_dkk": -6100.0}},
            "analysis_summary": {"analysis_window_active": false, "active_markets": ["XCSE"],
                                 "active_windows": [], "open_active_markets": ["XCSE"],
                                 "close_active_markets": [], "pre_sync_markets": [],
                                 "last_cycle_status": "ok", "last_heartbeat_at": "2026-10-16T08:00:00Z",
                                 "next_pulse_at": null, "next_pulse_label": null,
                                 "price_monitor_status": "fresh",
                                 "price_monitor_updated_at": "2026-10-16T08:00:00Z",
                                 "calendar_refresh": {"status": "ok", "checked_at": "2026-10-16T06:00:00Z",
                                                      "exchange_count": 9, "source": "saxo"}},
            "latest_decision": {"id": 412, "created_at": "2026-10-16T07:30:00Z", "status": "completed"},
            "scheduler_status": {"singleton_key": "scheduler", "scheduler_pid": 1,
                                 "started_at": "2026-10-16T06:07:00Z",
                                 "last_heartbeat_at": "2026-10-16T08:00:00Z",
                                 "last_cycle_started_at": "2026-10-16T07:59:00Z",
                                 "last_cycle_completed_at": "2026-10-16T08:00:00Z",
                                 "last_cycle_status": "ok",
                                 "last_cycle_json": {"detail": hidden}},
            "scheduler_health": {"status": "ok", "message": "Rust scheduler maintains Saxo sessions."},
            "trading_manager": {"status": "available", "latest_run": {
                "id": 77, "created_at": "2026-10-16T07:35:00Z", "manager_key": "eu_open",
                "manager_kind": "scheduled", "manager_label": "EU open", "target_at_utc": "2026-10-16T07:30:00Z",
                "report_id": 412, "status": "completed",
                "manager_json": {"approved_order_count": 1, "skipped_order_count": 2,
                                 "hermes_decision_advice": {"status": "ok", "mode": "conservative"}},
                "queue_result_json": {"status": "queued", "orders": [{"detail": hidden}]},
                "technical_json": {"detail": hidden},
                "open_exchange_codes_json": [hidden],
                "error_text": hidden}},
            "markov_method": {"status": "available", "config": {"detail": hidden},
                              "latest_run": {"id": "88", "created_at": "2026-10-15T21:30:00Z",
                                             "run_date": "2026-10-15", "status": "completed",
                                             "asset_count": 201, "success_count": 201, "error_count": 0,
                                             "config_json": {"detail": hidden},
                                             "summary_json": {"detail": hidden}}},
            "quiver_signals": {"status": "available", "config": {"detail": hidden}, "latest_run": null},
            "saxo_auth": {"configured_environment": "sim", "environment": "sim", "connected": true,
                          "token_valid": true, "refresh_token_valid": true, "refreshing": false,
                          "needs_reauth": false, "status": "connected", "status_text": "Connected",
                          "expires_at": "2026-10-16T08:20:00Z", "expires_in_minutes": 20,
                          "refresh_expires_at": "2026-10-16T09:00:00Z", "refresh_expires_in_minutes": 60,
                          "last_refreshed_at": "2026-10-16T07:40:00Z",
                          "session_path": format!("/tmp/{hidden}.json"), "error": hidden},
            "settings": {"cash_buffer": {"min_cash_buffer_pct": 0.05, "max_deployment_pct": 0.95,
                                         "reinvestment_pressure_threshold_pct": 0.2, "source": "config",
                                         "updated_at": null, "config_default_min_cash_buffer_pct": 0.05},
                         "ai": {"provider": "openrouter", "model": "openai/gpt-5", "config_model": "openai/gpt-5",
                                "source": "config", "updated_at": null, "recent_models": [],
                                "api_key": {"configured": true, "source": "env", "masked": "sk-...abcd",
                                            "updated_at": null}}},
            "refresh": {"price_poll_interval_minutes": 1, "scheduler_poll_interval_minutes": 10,
                        "decision_cadence": "rust_dashboard", "decision_cadence_label": "Rust dashboard",
                        "decision_pulses": [], "next_decision_pulse_at": "2026-10-16T13:45:00Z",
                        "next_decision_pulse_label": "US open"},
        })
    }

    /// The public Overview keeps every field the smoke test and the
    /// diagnostics bundle read, and none of the detail documents, paths or
    /// free-form errors it now leaves to their own endpoints.
    #[test]
    fn the_public_overview_keeps_what_operators_read_and_drops_the_detail() {
        let typed = public_overview_from_json(overview_fixture()).expect("decodes");
        let served = serde_json::to_value(&typed).unwrap();
        let text = served.to_string();
        assert!(
            !text.contains("must-not-reach-the-public-overview"),
            "a dropped document leaked: {text}"
        );
        for gone in [
            "session_path",
            "config_path",
            "last_cycle_json",
            "queue_result_json",
            "technical_json",
            "open_exchange_codes_json",
            "config_json",
            "summary_json",
        ] {
            assert!(!text.contains(gone), "{gone} is not part of the summary");
        }

        // What scripts/post_deploy_smoke.sh matches on.
        assert!(text.contains(r#""last_cycle_status":"ok""#));
        assert!(text.contains(r#""latest_decision""#));
        assert!(text.contains(r#""needs_reauth":false"#));

        // What scripts/diagnostics_bundle.sh reads.
        assert_eq!(
            served["portfolio_summary"]["total_market_value_dkk"],
            240000.0
        );
        assert_eq!(served["portfolio_summary"]["position_count"], 14);
        assert_eq!(
            served["goal_tracking"]["periods"]["week"]["status"],
            "behind"
        );
        assert_eq!(
            served["goal_tracking"]["periods"]["month"]["target_dkk"],
            3600.0
        );
        assert_eq!(served["saxo_auth"]["refresh_expires_in_minutes"], 60);
        assert_eq!(
            served["scheduler_status"]["last_cycle_completed_at"],
            "2026-10-16T08:00:00Z"
        );
        assert_eq!(served["execution"]["counts"]["broker_live"], 2);
        assert_eq!(served["latest_decision"]["id"], 412);
        let run = &served["trading_manager"]["latest_run"];
        assert_eq!(run["report_id"], 412);
        assert_eq!(run["manager_json"]["approved_order_count"], 1);
        assert_eq!(
            run["manager_json"]["hermes_decision_advice"]["mode"],
            "conservative"
        );
        assert_eq!(served["trading_manager"]["latest_queue_status"], "queued");
        assert_eq!(served["markov_method"]["latest_run"]["success_count"], 201);
        assert_eq!(served["quiver_signals"]["latest_run"], JsonValue::Null);
        assert_eq!(
            served["integrity"]["pnl_reconciliation"]["status"], "reconciled",
            "integrity evidence is kept whole"
        );
        assert_eq!(served["integrity"]["warnings"][0]["watched_positions"], 3);
        assert_eq!(served["settings"]["ai"]["api_key"]["masked"], "sk-...abcd");
    }

    #[test]
    fn the_public_overview_tolerates_null_like_absent() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &overview_fixture(),
            public_overview_from_json,
        );
    }

    /// The queue status is read whether the document arrives decoded or as
    /// the text it is stored as.
    #[test]
    fn the_queue_status_is_read_from_stored_text_too() {
        let mut fixture = overview_fixture();
        fixture["trading_manager"]["latest_run"]["queue_result_json"] =
            json!(r#"{"status":"completed_no_orders","orders":[]}"#);
        let typed = public_overview_from_json(fixture).expect("decodes");
        assert_eq!(
            typed.trading_manager.latest_queue_status.as_deref(),
            Some("completed_no_orders")
        );
    }

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
