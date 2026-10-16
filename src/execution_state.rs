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
use crate::read_model;
use crate::saxo_error::execution_error_taxonomy_for_code;
use crate::{
    db::{clamp_limit, sql_escape, value_f64, value_i64},
    state::{AppState, enrich_execution_order_lifecycle, json_text, matching_order_advice},
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use std::collections::HashSet;
use tracing::{info, warn};

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
    read_model::decode_each("dashboard_execution_fills", fills)
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
            let mut event: DashboardExecutionEventPayload =
                read_model::decode("dashboard_execution_events", event)?;
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
    read_model::decode("dashboard_protective_stop_coverage", coverage)
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

pub(crate) fn matching_manager_preflight_candidate(
    manager_run: &JsonValue,
    strategy_key: &str,
    symbol: &str,
    action: &str,
) -> JsonValue {
    manager_run
        .get("manager_json")
        .and_then(|value| value.get("hermes_preflight"))
        .and_then(|value| value.get("candidate_waterfall"))
        .and_then(|value| matching_order_advice(Some(value), strategy_key, symbol, action))
        .unwrap_or(JsonValue::Null)
}

pub(crate) fn compact_attribution_technical(value: &JsonValue, evidence_source: &str) -> JsonValue {
    if !value.is_object() || json_text(value, "status") != "ok" {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": evidence_source,
        "run_date": json_text(value, "run_date"),
        "sentiment": json_text(value, "sentiment"),
        "trend_bias": json_text(value, "trend_bias"),
        "confluence_count": value_i64(value, "confluence_count"),
        "min_confluences": value_i64(value, "min_confluences"),
        "reward_risk": value_f64(value, "reward_risk"),
    })
}

pub(crate) fn compact_attribution_markov(value: &JsonValue, evidence_source: &str) -> JsonValue {
    if !value.is_object() || json_text(value, "status") != "ok" {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": evidence_source,
        "run_date": json_text(value, "run_date"),
        "current_state": json_text(value, "current_state"),
        "direction": json_text(value, "direction"),
        "signed_signal": value_f64(value, "signed_signal"),
        "conviction": value_f64(value, "conviction"),
        "bull_prob": value_f64(value, "bull_prob"),
        "bear_prob": value_f64(value, "bear_prob"),
    })
}

pub(crate) fn compact_attribution_capital(value: &JsonValue) -> JsonValue {
    if !value.is_object() {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": "manager_run",
        "cash_balance_dkk": value_f64(value, "cash_balance_dkk"),
        "cash_pct": value_f64(value, "cash_pct"),
        "required_cash_buffer_dkk": value_f64(value, "required_cash_buffer_dkk"),
        "available_buy_budget_dkk": value_f64(value, "available_buy_budget_dkk"),
        "remaining_deployment_capacity_dkk": value_f64(value, "remaining_deployment_capacity_dkk"),
        "reinvestment_pressure_active": value
            .get("reinvestment_pressure_active")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) fn compact_execution_ledger_outcome(
    order: &JsonValue,
    summary: &JsonValue,
    evidence_source: &str,
) -> JsonValue {
    let fill_count = value_i64(summary, "fill_count");
    if fill_count <= 0 {
        return JsonValue::Null;
    }
    let ledger_entry_count = value_i64(summary, "ledger_entry_count");
    let filled_quantity = value_f64(summary, "filled_quantity");
    let target_quantity = value_f64(order, "quantity");
    json!({
        "evidence_source": evidence_source,
        "status": if ledger_entry_count >= fill_count {
            "reconciled"
        } else {
            "pending_ledger_reconciliation"
        },
        "side": json_text(order, "action").to_uppercase(),
        "fill_count": fill_count,
        "ledger_entry_count": ledger_entry_count,
        "filled_quantity": filled_quantity,
        "target_quantity": target_quantity,
        "fully_filled": target_quantity > 0.0 && filled_quantity + 1e-9 >= target_quantity,
        "last_fill_at": json_text(summary, "last_fill_at"),
        "commission_dkk": value_f64(summary, "commission_dkk"),
        "tax_dkk": value_f64(summary, "tax_dkk"),
        "realised_gain_dkk": value_f64(summary, "realised_gain_dkk"),
        "cost_basis_sold_dkk": value_f64(summary, "cost_basis_sold_dkk"),
    })
}

/// Post-fill market movement is attribution evidence, not realised P/L. It
/// intentionally uses only persisted fills and daily-indicator closes: no
/// Saxo/quote request occurs while rendering an execution row.
pub(crate) fn compact_holding_period_outcome(
    order: &JsonValue,
    fill_summary: &JsonValue,
    subsequent_closes: &[JsonValue],
) -> JsonValue {
    let side = json_text(order, "action").to_uppercase();
    if !matches!(side.as_str(), "BUY" | "SELL") {
        return JsonValue::Null;
    }
    let filled_quantity = value_f64(fill_summary, "filled_quantity");
    let fill_price_local = value_f64(fill_summary, "average_fill_price_local");
    let first_fill_at = json_text(fill_summary, "first_fill_at");
    if filled_quantity <= 0.0 || fill_price_local <= 0.0 || first_fill_at.is_empty() {
        return JsonValue::Null;
    }

    let directional_multiplier = if side == "BUY" { 1.0 } else { -1.0 };
    let session_outcome = |session: usize, close: Option<&JsonValue>| {
        let Some(close) = close else {
            return JsonValue::Null;
        };
        let close_local = value_f64(close, "close");
        if close_local <= 0.0 {
            return JsonValue::Null;
        }
        let market_return_pct = close_local / fill_price_local - 1.0;
        json!({
            "as_of": json_text(close, "run_date"),
            "session": session,
            "close_local": close_local,
            "market_return_pct": market_return_pct,
            "directional_return_pct": market_return_pct * directional_multiplier,
        })
    };
    let one_session = session_outcome(1, subsequent_closes.first());
    let five_session = session_outcome(5, subsequent_closes.get(4));
    let status = if !five_session.is_null() {
        "complete"
    } else if !one_session.is_null() {
        "partial"
    } else {
        "pending_daily_close"
    };
    json!({
        "status": status,
        "evidence_source": "reconciled_fills_and_daily_indicator_closes",
        "side": side,
        "filled_quantity": filled_quantity,
        "fill_price_local": fill_price_local,
        "currency": json_text(fill_summary, "currency"),
        "first_fill_at": first_fill_at,
        "available_sessions": subsequent_closes.len(),
        "one_session": one_session,
        "five_session": five_session,
        "interpretation": "Directional return is a read-only post-fill price comparison. It excludes FX, commissions, tax, slippage, and later position changes; it is not realised P/L."
    })
}

/// Reconstruct the local, reconciled-fill sequence around one order without
/// treating it as broker position truth. Historical fills can be incomplete
/// (for example after a portfolio import), so a SELL without an observed BUY
/// is explicitly reported as partial rather than inferred as a reduction.
pub(crate) fn compact_execution_position_lifecycle(
    order: &JsonValue,
    observed_fills: &[JsonValue],
) -> JsonValue {
    let order_id = value_i64(order, "id");
    let side = json_text(order, "action").to_uppercase();
    if order_id <= 0 || !matches!(side.as_str(), "BUY" | "SELL") {
        return JsonValue::Null;
    }

    let mut observed_net: f64 = 0.0;
    let mut minimum_observed_net: f64 = 0.0;
    let mut net_before_current = None;
    let mut net_after_current = None;
    let mut current_fill_count = 0_i64;
    let mut observed_order_ids = HashSet::new();
    let mut first_fill_at = String::new();
    let mut latest_fill_at = String::new();
    let mut first_side = String::new();

    for fill in observed_fills {
        let fill_side = json_text(fill, "side").to_uppercase();
        let quantity = value_f64(fill, "delta_quantity");
        if quantity <= 0.0 || !matches!(fill_side.as_str(), "BUY" | "SELL") {
            continue;
        }
        let fill_order_id = value_i64(fill, "execution_order_id");
        if fill_order_id > 0 {
            observed_order_ids.insert(fill_order_id);
        }
        let created_at = json_text(fill, "created_at");
        if first_fill_at.is_empty() {
            first_fill_at = created_at.clone();
            first_side = fill_side.clone();
        }
        latest_fill_at = created_at;

        if fill_order_id == order_id {
            net_before_current.get_or_insert(observed_net);
        }
        let signed_quantity = if fill_side == "BUY" {
            quantity
        } else {
            -quantity
        };
        observed_net += signed_quantity;
        minimum_observed_net = minimum_observed_net.min(observed_net);
        if fill_order_id == order_id {
            current_fill_count += 1;
            net_after_current = Some(observed_net);
        }
    }

    let (Some(net_before), Some(net_after)) = (net_before_current, net_after_current) else {
        return JsonValue::Null;
    };
    let history_status = if first_side == "SELL" || minimum_observed_net < -1e-9 {
        "partial_history"
    } else {
        "observed_local_fills"
    };
    let phase = if history_status == "partial_history" {
        "partial_history"
    } else if side == "BUY" && net_before <= 1e-9 {
        "entry"
    } else if side == "BUY" {
        "add"
    } else if net_before <= 1e-9 || net_after < -1e-9 {
        "partial_history"
    } else if net_after <= 1e-9 {
        "exit"
    } else {
        "reduce"
    };

    json!({
        "evidence_source": "reconciled_execution_fills",
        "history_status": history_status,
        "phase": phase,
        "side": side,
        "observed_net_before": net_before,
        "observed_net_after": net_after,
        "current_order_fill_count": current_fill_count,
        "observed_fill_count": observed_fills.len(),
        "observed_order_count": observed_order_ids.len(),
        "first_observed_fill_at": first_fill_at,
        "latest_observed_fill_at": latest_fill_at,
        "interpretation": "Read-only local reconciled-fill sequence. It excludes outside-ledger inventory and later broker adjustments; it is not broker position truth."
    })
}

pub(crate) fn attribution_delta_label(
    hermes_order: &JsonValue,
    manager_order: &JsonValue,
    order: &JsonValue,
) -> String {
    let hermes_action = json_text(hermes_order, "action").trim().to_lowercase();
    let manager_decision = json_text(manager_order, "manager_decision");
    let status = json_text(order, "status").trim().to_lowercase();
    if hermes_action.is_empty() {
        return if manager_decision == "approved" {
            "manager_only".to_string()
        } else {
            "no_advice".to_string()
        };
    }
    if matches!(hermes_action.as_str(), "stand_down" | "review") && manager_decision == "approved" {
        return "manager_overrode_review".to_string();
    }
    if hermes_action == "reduce" && manager_decision == "approved" {
        return "reduced_or_capped".to_string();
    }
    if hermes_action == "allow" && manager_decision == "approved" {
        return if status == "executed" {
            "allowed_executed".to_string()
        } else {
            "allowed_queued".to_string()
        };
    }
    if manager_decision == "skipped" {
        return format!("{}_skipped", hermes_action);
    }
    hermes_action
}

impl AppState {
    pub async fn execution_orders(&self, limit: i64) -> Result<Vec<JsonValue>> {
        self.execution_orders_page(limit, 0).await
    }

    pub async fn execution_order_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, symbol, action, order_type, mode, status, adapter, quantity,
                    price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                    strategy_type, strategy_role
             FROM execution_orders
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_orders_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_id, symbol, action, order_type, mode, status, adapter, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk, approval_required, approved_at, ledger_id, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role, error_text, broker_order_id, execution_result_json FROM execution_orders ORDER BY created_at DESC, id DESC LIMIT {} OFFSET {}",
            clamp_limit(limit, 1, 500),
            offset.max(0).min(100_000),
        );
        let mut orders = self.select_json(&sql).await.unwrap_or_default();
        let market_rows = self.market_exchange_rows();
        for order in &mut orders {
            enrich_execution_order_lifecycle(order, &market_rows);
            match self.execution_order_attribution(order).await {
                Ok(attribution) => {
                    if let Some(object) = order.as_object_mut() {
                        object.insert("attribution".to_string(), attribution);
                    }
                }
                Err(err) => {
                    warn!("execution attribution degraded: {err:#}");
                }
            }
        }
        Ok(orders)
    }

    pub async fn execution_orders_count(&self) -> Result<i64> {
        let row = self
            .first_json("SELECT COUNT(*) AS count FROM execution_orders")
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count"))
    }

    async fn execution_order_attribution(&self, order: &JsonValue) -> Result<JsonValue> {
        let report_id = value_i64(order, "report_id");
        let symbol = json_text(order, "symbol");
        let action = json_text(order, "action");
        let strategy_key = json_text(order, "strategy_key");

        let report = if report_id > 0 {
            self.first_json(&format!(
                "SELECT id, created_at, status, model, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible
                 FROM decision_reports WHERE id = {} LIMIT 1",
                report_id
            ))
            .await?
            .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };

        let manager_run = if report_id > 0 {
            self.first_json(&format!(
                "SELECT id, created_at, status, manager_key, manager_kind, manager_label, manager_json, queue_result_json
                 FROM trading_manager_runs
                 WHERE report_id = {}
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                report_id
            ))
            .await?
            .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };

        let advice = if report_id > 0 {
            self.hermes_decision_advice_by_report(report_id)
                .await?
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let hermes_order = matching_order_advice(
            advice.get("order_advice_json"),
            &strategy_key,
            &symbol,
            &action,
        )
        .or_else(|| {
            manager_run
                .get("manager_json")
                .and_then(|value| value.get("hermes_decision_advice"))
                .and_then(|value| value.get("raw"))
                .and_then(|value| value.get("order_advice_json"))
                .and_then(|value| {
                    matching_order_advice(Some(value), &strategy_key, &symbol, &action)
                })
        })
        .unwrap_or(JsonValue::Null);

        let manager_order = manager_run
            .get("manager_json")
            .and_then(|value| value.get("approved_orders"))
            .and_then(|value| matching_order_advice(Some(value), &strategy_key, &symbol, &action))
            .map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("manager_decision".to_string(), JsonValue::from("approved"));
                }
                value
            })
            .or_else(|| {
                manager_run
                    .get("manager_json")
                    .and_then(|value| value.get("skipped_orders"))
                    .and_then(|value| {
                        matching_order_advice(Some(value), &strategy_key, &symbol, &action)
                    })
                    .map(|mut value| {
                        if let Some(object) = value.as_object_mut() {
                            object
                                .insert("manager_decision".to_string(), JsonValue::from("skipped"));
                        }
                        value
                    })
            })
            .unwrap_or(JsonValue::Null);

        let preflight_candidate =
            matching_manager_preflight_candidate(&manager_run, &strategy_key, &symbol, &action);
        let technical = compact_attribution_technical(
            manager_order
                .get("final_technical")
                .unwrap_or(&JsonValue::Null),
            "manager_final",
        );
        let technical = if technical.is_null() {
            let preflight = compact_attribution_technical(
                preflight_candidate
                    .get("technical")
                    .unwrap_or(&JsonValue::Null),
                "manager_preflight",
            );
            if preflight.is_null() {
                compact_attribution_technical(
                    &self.latest_indicator_signal_summary(&symbol).await?,
                    "latest_fallback",
                )
            } else {
                preflight
            }
        } else {
            technical
        };
        let markov = compact_attribution_markov(
            preflight_candidate
                .get("markov")
                .unwrap_or(&JsonValue::Null),
            "manager_preflight",
        );
        let markov = if markov.is_null() {
            compact_attribution_markov(
                &crate::markov_method::latest_markov_signal_summary(self, &symbol).await?,
                "latest_fallback",
            )
        } else {
            markov
        };
        let capital = manager_run
            .get("manager_json")
            .and_then(|value| value.get("capital_budget"))
            .map(compact_attribution_capital)
            .unwrap_or(JsonValue::Null);
        let ledger_outcome = match self.execution_order_ledger_outcome(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution ledger attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let holding_period_outcome = match self.execution_order_holding_period_outcome(order).await
        {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution holding-period attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let position_lifecycle = match self.execution_order_position_lifecycle(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution position-lifecycle attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let trade_thesis = match self.execution_order_trade_thesis(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution trade-thesis attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let delta = attribution_delta_label(&hermes_order, &manager_order, order);

        Ok(json!({
            "delta": delta,
            "report": {
                "id": report.get("id").cloned().unwrap_or(JsonValue::Null),
                "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                "model": report.get("model").cloned().unwrap_or(JsonValue::Null),
                "pulse_key": report.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                "pulse_label": report.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
            },
            "trading_manager": {
                "run_id": manager_run.get("id").cloned().unwrap_or(JsonValue::Null),
                "status": manager_run.get("status").cloned().unwrap_or(JsonValue::Null),
                "manager_key": manager_run.get("manager_key").cloned().unwrap_or(JsonValue::Null),
                "decision": manager_order,
            },
            "hermes": {
                "advice_id": advice.get("id").cloned().unwrap_or(JsonValue::Null),
                "status": advice.get("status").cloned().unwrap_or(JsonValue::Null),
                "recommendation": advice.get("overall_recommendation").cloned().unwrap_or(JsonValue::Null),
                "summary": advice.get("summary").cloned().unwrap_or(JsonValue::Null),
                "order_advice": hermes_order,
            },
            "technical": technical,
            "markov": markov,
            "capital_budget": capital,
            "ledger_outcome": ledger_outcome,
            "holding_period_outcome": holding_period_outcome,
            "position_lifecycle": position_lifecycle,
            "trade_thesis": trade_thesis,
        }))
    }

    pub(crate) async fn execution_order_ledger_outcome(
        &self,
        order: &JsonValue,
    ) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let status = json_text(order, "status");
        let ledger_id = value_i64(order, "ledger_id");
        if order_id <= 0 || (ledger_id <= 0 && status != "broker_partially_filled") {
            return Ok(JsonValue::Null);
        }

        let fill_summary = self
            .first_json(&format!(
                "SELECT COUNT(f.id) AS fill_count,
                        COUNT(l.id) AS ledger_entry_count,
                        COALESCE(SUM(f.delta_quantity), 0) AS filled_quantity,
                        MAX(f.created_at) AS last_fill_at,
                        COALESCE(SUM(l.commission_dkk), 0) AS commission_dkk,
                        COALESCE(SUM(l.tax_dkk), 0) AS tax_dkk,
                        COALESCE(SUM(l.realised_gain_dkk), 0) AS realised_gain_dkk,
                        COALESCE(SUM(l.cost_basis_sold_dkk), 0) AS cost_basis_sold_dkk
                 FROM execution_fills f
                 LEFT JOIN trade_ledger l ON l.id = f.ledger_id
                 WHERE f.execution_order_id = {}",
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let outcome = compact_execution_ledger_outcome(order, &fill_summary, "reconciled_fills");
        if !outcome.is_null() || ledger_id <= 0 {
            return Ok(outcome);
        }

        let legacy_ledger = self
            .first_json(&format!(
                "SELECT 1 AS fill_count,
                        1 AS ledger_entry_count,
                        quantity AS filled_quantity,
                        created_at AS last_fill_at,
                        commission_dkk,
                        tax_dkk,
                        realised_gain_dkk,
                        cost_basis_sold_dkk
                 FROM trade_ledger
                 WHERE id = {}
                 LIMIT 1",
                ledger_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(compact_execution_ledger_outcome(
            order,
            &legacy_ledger,
            "legacy_order_ledger",
        ))
    }

    pub(crate) async fn execution_order_holding_period_outcome(
        &self,
        order: &JsonValue,
    ) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        if order_id <= 0 {
            return Ok(JsonValue::Null);
        }
        let fill_summary = self
            .first_json(&format!(
                "SELECT MIN(created_at) AS first_fill_at,
                        COALESCE(SUM(delta_quantity), 0) AS filled_quantity,
                        CASE WHEN COALESCE(SUM(delta_quantity), 0) > 0
                             THEN SUM(delta_quantity * average_price_local) / SUM(delta_quantity)
                             ELSE 0 END AS average_fill_price_local,
                        MAX(currency) AS currency
                 FROM execution_fills
                 WHERE execution_order_id = {} AND delta_quantity > 0",
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let first_fill_at = json_text(&fill_summary, "first_fill_at");
        let fill_date = first_fill_at.chars().take(10).collect::<String>();
        if fill_date.len() != 10 {
            return Ok(JsonValue::Null);
        }
        let symbol = json_text(order, "symbol");
        if symbol.is_empty() {
            return Ok(JsonValue::Null);
        }
        // The next distinct daily closes are trading-session observations. A
        // weekend/holiday does not manufacture a synthetic one-day outcome.
        let subsequent_closes = self
            .select_json(&format!(
                "SELECT run_date, MAX(close) AS close
                 FROM daily_indicator_signals
                     WHERE UPPER(symbol) = UPPER('{}') AND status = 'ok' AND close > 0 AND run_date > '{}'
                 GROUP BY run_date
                 ORDER BY run_date ASC
                 LIMIT 5",
                sql_escape(&symbol),
                sql_escape(&fill_date)
            ))
            .await?;
        Ok(compact_holding_period_outcome(
            order,
            &fill_summary,
            &subsequent_closes,
        ))
    }

    pub(crate) async fn execution_order_position_lifecycle(
        &self,
        order: &JsonValue,
    ) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let symbol = json_text(order, "symbol");
        if order_id <= 0 || symbol.is_empty() {
            return Ok(JsonValue::Null);
        }
        let fills = self
            .select_json(&format!(
                "SELECT id, execution_order_id, created_at, side, delta_quantity
                 FROM execution_fills
                 WHERE symbol = '{}' AND delta_quantity > 0
                 ORDER BY created_at ASC, id ASC",
                sql_escape(&symbol)
            ))
            .await?;
        Ok(compact_execution_position_lifecycle(order, &fills))
    }

    pub(crate) async fn execution_order_trade_thesis(
        &self,
        order: &JsonValue,
    ) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let symbol = json_text(order, "symbol");
        let created_at = json_text(order, "created_at");
        if order_id <= 0 || symbol.is_empty() || created_at.is_empty() {
            return Ok(JsonValue::Null);
        }
        let thesis = self
            .first_json(&format!(
                "SELECT trade_thesis_json
                 FROM execution_orders
                 WHERE symbol = '{}' AND action = 'BUY'
                   AND trade_thesis_json IS NOT NULL
                   AND (created_at < '{}' OR (created_at = '{}' AND id <= {}))
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                sql_escape(&symbol),
                sql_escape(&created_at),
                sql_escape(&created_at),
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let raw = thesis
            .get("trade_thesis_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        if raw.is_object() {
            return Ok(raw);
        }
        let Some(raw) = raw.as_str() else {
            return Ok(JsonValue::Null);
        };
        Ok(serde_json::from_str(raw).unwrap_or(JsonValue::Null))
    }

    pub async fn execution_fills(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_fills ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_fill_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, execution_order_id, broker_order_id, symbol, side, fill_status,
                    cumulative_quantity, delta_quantity, average_price_local, currency, ledger_id
             FROM execution_fills
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// Broker lifecycle events for one order, oldest first.
    ///
    /// The column list is an allowlist, not a `SELECT *` minus a few fields:
    /// `raw_payload_json` holds unredacted Saxo responses and `account_uid`
    /// identifies the account, and neither may reach a browser. Adding a column
    /// to the table must not silently add it to this response.
    ///
    /// Ordered ascending because this renders as a timeline — the interesting
    /// reading is placement to terminal state, not newest-first like the flat
    /// event list on the Execution tab.
    pub async fn execution_order_events(
        &self,
        order_id: i64,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, event_type, broker_status, broker_substatus,
                    broker_quantity, broker_price_local, broker_order_id
             FROM execution_order_events
             WHERE execution_order_id = {}
             ORDER BY created_at ASC, id ASC
             LIMIT {}",
            order_id,
            clamp_limit(limit, 1, 200)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_events(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_order_events ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_event_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, execution_order_id, event_type, broker_status
             FROM execution_order_events
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub(crate) async fn execution_counts(&self) -> Result<JsonValue> {
        let rows = self
            .select_json("SELECT status, COUNT(*) AS count FROM execution_orders GROUP BY status")
            .await?;
        let mut queued = 0;
        let mut pending_approval = 0;
        let mut broker_live = 0;
        let mut failed = 0;
        for row in rows {
            let status = row.get("status").and_then(JsonValue::as_str).unwrap_or("");
            let count = value_i64(&row, "count");
            match status {
                "pending_execution"
                | "waiting_for_market_open"
                | "waiting_for_cash_settlement"
                | "waiting_for_virtual_cash_budget" => queued += count,
                "pending_approval" => pending_approval += count,
                "submitted_to_broker"
                | "submitting_to_broker"
                | "broker_working"
                | "broker_amended"
                | "broker_partially_filled"
                | "broker_replace_requested"
                | "broker_cancel_requested" => broker_live += count,
                "execution_failed" | "expired_local" => failed += count,
                _ => {}
            }
        }
        Ok(
            json!({"queued": queued, "pending_approval": pending_approval, "broker_live": broker_live, "failed": failed}),
        )
    }

    pub(crate) async fn executed_orders_today(&self) -> Result<i64> {
        let today = Utc::now().date_naive().to_string();
        let sql = format!(
            "SELECT COUNT(*) AS count FROM execution_orders WHERE substr(created_at, 1, 10) = '{}' AND status = 'executed'",
            sql_escape(&today)
        );
        let row = self.first_json(&sql).await?.unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count"))
    }

    /// Recompute the price/currency split on ledger rows written while
    /// `fx_gain_dkk` was a hardcoded `0` literal (every sale up to 2026-08-02).
    ///
    /// This is a pure recomputation from columns already stored on each row —
    /// `realised_gain_dkk`, `cost_basis_sold_dkk`, `cost_basis_sold_local`, and
    /// `sale_fx_rate_to_dkk` — so no external rate lookup is involved and the
    /// result cannot drift with today's FX. It is idempotent: the same inputs
    /// produce the same split on every run.
    ///
    /// The cost rate is derived as `cost_basis_sold_dkk / cost_basis_sold_local`
    /// rather than read from `cost_basis_fx_rate_to_dkk`, which is deliberate:
    /// rows written before 2026-07-09 by the legacy Python path stored that
    /// column 100x too small (0.0713 where the rate was 7.0221). The derived
    /// ratio is correct on every row, so the backfill does not inherit that
    /// defect.
    ///
    /// Scoped to every SELL row, not only those still showing the hardcoded
    /// zero. The retired Python path wrote its own `fx_gain_dkk` against the
    /// same corrupt rate column, so rows with a nonzero value are not
    /// necessarily right — filtering on `fx_gain_dkk = 0` would have left the
    /// worst rows untouched and the price/currency identity broken on them.
    /// Recomputation is idempotent, so rewriting a correct row costs nothing.
    ///
    /// Rows the split declines to attribute — no local cost basis, or a derived
    /// cost rate that cannot be a real exchange rate — are reset to price-only
    /// rather than left carrying a stale legacy figure.
    pub(crate) async fn backfill_realised_gain_currency_split(&self) -> Result<()> {
        let rows = sqlx::query(
            "SELECT id, realised_gain_dkk, cost_basis_sold_dkk, cost_basis_sold_local, \
                    sale_fx_rate_to_dkk, fx_gain_dkk, price_gain_dkk \
             FROM trade_ledger \
             WHERE side = 'SELL'",
        )
        .fetch_all(&self.pool)
        .await
        .context("loading trade-ledger rows for currency-split backfill")?;
        let mut updated = 0_u64;
        for row in rows {
            let id: i64 = row.try_get("id").unwrap_or_default();
            let split = crate::fx::split_realised_gain(
                row.try_get("realised_gain_dkk").unwrap_or(0.0),
                row.try_get("cost_basis_sold_dkk").unwrap_or(0.0),
                row.try_get("cost_basis_sold_local").unwrap_or(0.0),
                row.try_get("sale_fx_rate_to_dkk").unwrap_or(0.0),
            );
            let stored_fx: f64 = row.try_get("fx_gain_dkk").unwrap_or(0.0);
            let stored_price: f64 = row.try_get("price_gain_dkk").unwrap_or(0.0);
            if (stored_fx - split.fx_gain_dkk).abs() < 0.005
                && (stored_price - split.price_gain_dkk).abs() < 0.005
            {
                continue;
            }
            sqlx::query(&format!(
                "UPDATE trade_ledger \
                 SET fx_gain_dkk = {}, price_gain_dkk = {}, realised_gain_local = {} \
                 WHERE id = {id}",
                split.fx_gain_dkk, split.price_gain_dkk, split.realised_gain_local
            ))
            .execute(&self.pool)
            .await
            .context("backfilling trade-ledger currency split")?;
            updated += 1;
        }
        if updated > 0 {
            info!(rows = updated, "backfilled realised-gain currency split");
        }
        Ok(())
    }

    /// Backfill `strategy_type` on Trading Manager orders queued before the
    /// runtime started setting it (2026-05-12 to 2026-07-25).
    ///
    /// `report_id IS NOT NULL` is the exact discriminator: on 2026-07-25 all
    /// 101 unset rows carried a report id, and every row with another
    /// `strategy_type` -- `portfolio_sync`, `clean_reconciliation`, `manual` --
    /// carried none, because those come from adoption and manual paths that do
    /// not originate in a decision report. The update is idempotent and cannot
    /// overwrite a value another path already set.
    pub(crate) async fn backfill_trading_manager_strategy_type(&self) -> Result<()> {
        let result = sqlx::query(&format!(
            "UPDATE execution_orders SET strategy_type = '{}' \
             WHERE strategy_type IS NULL AND report_id IS NOT NULL",
            sql_escape(crate::trading_manager::TRADING_MANAGER_STRATEGY_TYPE)
        ))
        .execute(&self.pool)
        .await
        .context("backfilling Trading Manager execution-order strategy type")?;
        if result.rows_affected() > 0 {
            info!(
                rows = result.rows_affected(),
                strategy_type = crate::trading_manager::TRADING_MANAGER_STRATEGY_TYPE,
                "backfilled execution-order strategy type"
            );
        }
        Ok(())
    }

    /// The ENS activity backfill is a read-only daily reconciliation aid. Keep
    /// its scheduler cursor in the database so a rollout does not turn one
    /// intended broker read into a request on every scheduler heartbeat.
    pub(crate) async fn ens_activity_backfill_completed_date(&self) -> Result<Option<String>> {
        Ok(self
            .runtime_setting("ens_activity_backfill")
            .await?
            .and_then(|value| {
                value
                    .get("completed_date")
                    .and_then(JsonValue::as_str)
                    .map(ToString::to_string)
            }))
    }

    pub(crate) async fn record_ens_activity_backfill(
        &self,
        completed_date: &str,
        summary: &JsonValue,
    ) -> Result<()> {
        self.save_runtime_setting(
            "ens_activity_backfill",
            &json!({
                "completed_date": completed_date,
                "completed_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "summary": summary,
            }),
        )
        .await
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

    /// Execution rows are assembled from local order state that is routinely
    /// incomplete mid-lifecycle, so a null must degrade one value rather than
    /// the whole tab.
    #[test]
    fn an_explicit_null_never_blanks_an_execution_panel() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "id": 991,
                "created_at": "2026-08-28T12:10:00Z",
                "execution_order_id": 204,
                "symbol": "NOVO-B:xcse",
                "side": "BUY",
                "fill_status": "final",
                "cumulative_quantity": 12.0,
                "delta_quantity": 12.0,
                "average_price_local": 780.0,
                "currency": "DKK",
                "broker_order_id": null,
                "order_status": null,
                "ledger_id": null
            }]),
            |value| {
                dashboard_execution_fills_from_json(
                    value.as_array().cloned().expect("fill fixture is a list"),
                )
            },
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "created_at": "2026-08-28T12:10:00Z",
                "execution_order_id": 204,
                "event_type": "precheck",
                "raw_payload_json": {"failure_stage": "precheck", "error_taxonomy": null}
            }]),
            |value| {
                dashboard_execution_events_from_json(
                    value.as_array().cloned().expect("event fixture is a list"),
                )
            },
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "status": "available",
                "summary": {"position_count": 3, "covered_count": 2},
                "positions": [{"symbol": "NOVO-B:xcse", "quantity": 12.0}],
                "exceptions": [],
                "recent_prechecks": [],
                "recent_lifecycle_tests": [],
                "safety": "read_only",
                "interpretation": "coverage audit"
            }),
            dashboard_protective_stop_coverage_from_json,
        );
    }
}
