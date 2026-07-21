use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, Utc};
use reqwest::{StatusCode, header};
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_string},
    db::{row_to_json, sql_escape, value_f64, value_i64},
    saxo_portfolio::refresh_broker_snapshots,
    state::AppState,
};

const TRADABLE_ASSET_TYPES: &str = "Stock,Etf,Etn,Etc";
const ACTIVE_SELL_STATUSES: &[&str] = &[
    "pending_execution",
    "pending_approval",
    "submitting_to_broker",
    "submitted_to_broker",
    "broker_working",
    "broker_amended",
    "broker_partially_filled",
    "broker_replace_requested",
    "waiting_for_market_open",
    "waiting_for_cash_settlement",
    "waiting_for_virtual_cash_budget",
];
const BROKER_SYNC_STATUSES: &[&str] = &[
    "submitted_to_broker",
    "broker_working",
    "broker_amended",
    "broker_partially_filled",
    "broker_replace_requested",
    "broker_cancel_requested",
    "broker_fill_unreconciled",
];

#[derive(Clone, Debug, PartialEq)]
struct SymbolParts {
    base: String,
    exchange: String,
}

#[derive(Clone, Debug, PartialEq)]
struct SaxoInstrument {
    uic: i64,
    asset_type: String,
    exchange_id: String,
    description: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BrokerPosition {
    quantity: f64,
    can_be_closed: bool,
    instrument: Option<SaxoInstrument>,
}

#[derive(Clone, Debug, Default)]
struct PositionCostBasis {
    quantity: f64,
    cost_basis_dkk: f64,
    cost_basis_local: f64,
    isin: Option<String>,
    instrument_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionQueueGate {
    NotLiveSaxo,
    DryRun,
    ApprovalRequired,
}

impl ExecutionQueueGate {
    fn reason(self) -> &'static str {
        match self {
            Self::NotLiveSaxo => {
                "Saxo execution queue only runs when execution.mode=live and execution.adapter=saxo."
            }
            Self::DryRun => "app.dry_run is true",
            Self::ApprovalRequired => "execution.require_approval_live is true",
        }
    }
}

/// Returns the first safety gate that keeps the queue away from Saxo.
/// The order is deliberate: a non-live/non-Saxo configuration cannot be
/// described as an approval or dry-run issue because it must never enter the
/// live execution path in the first place.
fn execution_queue_gate(
    execution_mode: &str,
    adapter: &str,
    dry_run: bool,
    require_approval: bool,
) -> Option<ExecutionQueueGate> {
    if !execution_mode.eq_ignore_ascii_case("live") || !adapter.eq_ignore_ascii_case("saxo") {
        Some(ExecutionQueueGate::NotLiveSaxo)
    } else if dry_run {
        Some(ExecutionQueueGate::DryRun)
    } else if require_approval {
        Some(ExecutionQueueGate::ApprovalRequired)
    } else {
        None
    }
}

pub async fn run_saxo_execution_queue(state: &AppState) -> Result<JsonValue> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let adapter = yaml_string(&state.config, &["execution", "adapter"])
        .unwrap_or_else(|| "simulation".into());
    let dry_run = yaml_bool(&state.config, &["app", "dry_run"]).unwrap_or(true);
    let require_approval =
        yaml_bool(&state.config, &["execution", "require_approval_live"]).unwrap_or(true);

    if let Some(gate) = execution_queue_gate(&execution_mode, &adapter, dry_run, require_approval) {
        return Ok(match gate {
            ExecutionQueueGate::NotLiveSaxo => json!({
                "status": "disabled",
                "reason": gate.reason(),
                "execution_mode": execution_mode,
                "adapter": adapter
            }),
            ExecutionQueueGate::DryRun | ExecutionQueueGate::ApprovalRequired => json!({
                "status": "disabled",
                "reason": gate.reason(),
            }),
        });
    }

    let rows = pending_live_saxo_orders(state).await?;
    if rows.is_empty() {
        return Ok(json!({"status": "ok", "processed": [], "submitted": 0, "failed": 0}));
    }

    // Refresh is intentionally service-level. It does not depend on a logged-in UI user,
    // which lets the scheduler survive rollouts and dashboard logout.
    let session = state
        .ensure_saxo_session_json("execution_queue")
        .await
        .context("loading Saxo session before executing queued orders")?;

    if let Err(err) = state.refresh_saxo_exchange_calendars_if_stale().await {
        warn!("Saxo execution queue using fallback exchange calendar: {err:#}");
    }
    let market_rows = state.market_exchange_rows();
    let broker_positions = broker_position_quantities(state, &session)
        .await
        .context("fetching live Saxo positions before executing queued orders")?;
    let mut processed = Vec::new();
    for order in rows {
        let order_id = value_i64(&order, "id");
        let symbol = order_text(&order, "symbol");
        if !claim_order_for_submission(state, order_id).await? {
            processed.push(json!({
                "status": "skipped",
                "order_id": order_id,
                "reason": "Order was already claimed, submitted, or no longer pending."
            }));
            continue;
        }
        match execute_order(state, &session, &market_rows, &broker_positions, &order).await {
            Ok(result) => processed.push(result),
            Err(err) => {
                warn!(order_id, symbol, "Saxo order execution failed: {err:#}");
                let result = fail_order(
                    state,
                    order_id,
                    "execution_failed",
                    &err.to_string(),
                    &json!({"adapter": "saxo", "error": err.to_string()}),
                )
                .await?;
                processed.push(result);
            }
        }
    }
    let submitted = processed
        .iter()
        .filter(|value| {
            value.get("status").and_then(JsonValue::as_str) == Some("submitted_to_broker")
        })
        .count();
    let failed = processed
        .iter()
        .filter(|value| value.get("status").and_then(JsonValue::as_str) == Some("execution_failed"))
        .count();
    let broker_read_model = refresh_after_execution(state).await;
    Ok(json!({
        "status": "ok",
        "processed": processed,
        "submitted": submitted,
        "failed": failed,
        "broker_read_model": broker_read_model
    }))
}

pub async fn sync_saxo_broker_orders(state: &AppState) -> Result<JsonValue> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let adapter = yaml_string(&state.config, &["execution", "adapter"])
        .unwrap_or_else(|| "simulation".into());
    if !execution_mode.eq_ignore_ascii_case("live") || !adapter.eq_ignore_ascii_case("saxo") {
        return Ok(json!({
            "status": "disabled",
            "reason": "Saxo broker order sync only runs when execution.mode=live and execution.adapter=saxo.",
            "execution_mode": execution_mode,
            "adapter": adapter
        }));
    }

    let rows = broker_sync_orders(state).await?;
    if rows.is_empty() {
        return Ok(
            json!({"status": "ok", "checked": 0, "updated": 0, "fills": 0, "processed": []}),
        );
    }

    let session = state
        .ensure_saxo_session_json("broker_order_sync")
        .await
        .context("loading Saxo session before broker order sync")?;
    let client_key = client_key(state, &session)?;
    let mut processed = Vec::new();
    let mut updated = 0;
    let mut fills = 0;
    for order in rows {
        match sync_one_broker_order(state, &session, &client_key, &order).await {
            Ok(result) => {
                if result
                    .get("updated")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                {
                    updated += 1;
                }
                fills += result.get("fills").and_then(JsonValue::as_i64).unwrap_or(0);
                processed.push(result);
            }
            Err(err) => {
                let order_id = value_i64(&order, "id");
                let symbol = order_text(&order, "symbol");
                warn!(order_id, symbol, "Saxo broker order sync failed: {err:#}");
                processed.push(json!({
                    "status": "error",
                    "order_id": order_id,
                    "symbol": symbol,
                    "error": err.to_string()
                }));
            }
        }
    }
    let broker_read_model = if updated > 0 {
        match refresh_broker_snapshots(state).await {
            Ok(value) => value,
            Err(err) => {
                warn!("Saxo broker read model refresh after broker order sync failed: {err:#}");
                json!({"status": "error", "error": err.to_string()})
            }
        }
    } else {
        json!({"status": "skipped", "reason": "no broker order state changes"})
    };
    Ok(json!({
        "status": "ok",
        "checked": processed.len(),
        "updated": updated,
        "fills": fills,
        "processed": processed,
        "broker_read_model": broker_read_model
    }))
}

async fn refresh_after_execution(state: &AppState) -> JsonValue {
    // Market orders can fill at the broker before the next scheduler tick.
    // Refreshing the broker read model here keeps the Rust UI aligned with Saxo
    // instead of showing the previous local portfolio snapshot.
    sleep(Duration::from_secs(2)).await;
    match refresh_broker_snapshots(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker read model refresh after execution failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    }
}

/// Count of orders that still need scheduler attention: queued locally or
/// awaiting broker fill/state sync. Used to switch the scheduler to a fast
/// poll interval while work is in flight. Statuses that idle by design for
/// long stretches (market closed, human approval) do not count.
pub(crate) async fn outstanding_order_count(state: &AppState) -> Result<i64> {
    let statuses = ACTIVE_SELL_STATUSES
        .iter()
        .chain(BROKER_SYNC_STATUSES)
        .filter(|status| !matches!(**status, "waiting_for_market_open" | "pending_approval"))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ");
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS outstanding
         FROM execution_orders
         WHERE mode = 'live' AND adapter = 'saxo' AND status IN ({statuses})"
    ))
    .fetch_one(&state.pool)
    .await?;
    Ok(row.try_get::<i64, _>("outstanding").unwrap_or(0))
}

async fn pending_live_saxo_orders(state: &AppState) -> Result<Vec<JsonValue>> {
    let rows = sqlx::query(
        "SELECT * FROM execution_orders
         WHERE mode = 'live'
           AND adapter = 'saxo'
           AND status IN ('pending_execution', 'waiting_for_market_open', 'waiting_for_virtual_cash_budget')
           AND (approval_required = 0 OR approved_at IS NOT NULL)
         ORDER BY created_at ASC, id ASC
         LIMIT 25",
    )
    .fetch_all(&state.pool)
    .await
    .context("fetching pending Saxo execution orders")?;
    Ok(rows.iter().map(row_to_json).collect())
}

async fn broker_sync_orders(state: &AppState) -> Result<Vec<JsonValue>> {
    let statuses = BROKER_SYNC_STATUSES
        .iter()
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = sqlx::query(&format!(
        "SELECT *
         FROM execution_orders
         WHERE mode = 'live'
           AND adapter = 'saxo'
           AND status IN ({})
           AND broker_order_id IS NOT NULL
           AND broker_order_id <> ''
         ORDER BY created_at ASC, id ASC
         LIMIT 50",
        statuses
    ))
    .fetch_all(&state.pool)
    .await
    .context("fetching Saxo broker orders pending sync")?;
    Ok(rows.iter().map(row_to_json).collect())
}

async fn sync_one_broker_order(
    state: &AppState,
    session: &JsonValue,
    client_key: &str,
    order: &JsonValue,
) -> Result<JsonValue> {
    let order_id = value_i64(order, "id");
    let symbol = order_text(order, "symbol");
    let broker_order_id = resolve_broker_order_id(order)
        .ok_or_else(|| anyhow!("execution order {order_id} has no Saxo broker_order_id"))?;
    let Some(broker_state) =
        fetch_broker_order_state(state, session, client_key, &broker_order_id).await?
    else {
        let missing_state = broker_order_lookup_miss_state(&broker_order_id);
        record_broker_order_event(
            state,
            order,
            &broker_order_id,
            "broker_sync_not_found",
            Some("NotFound"),
            None,
            None,
            None,
            &missing_state,
        )
        .await?;
        let current_status = order_text(order, "status");
        update_order_broker_status(state, order, &current_status, &missing_state, None).await?;
        return Ok(json!({
            "status": "not_found",
            "updated": true,
            "fills": 0,
            "order_id": order_id,
            "symbol": symbol,
            "broker_order_id": broker_order_id,
            "broker_visibility": "not_found",
            "quantity_changed": false,
            "price_changed": false,
        }));
    };
    let broker_payload = broker_payload(&broker_state);
    let broker_status = broker_status_text(broker_payload);
    let broker_substatus = json_text(broker_payload, "SubStatus");
    let broker_quantity = extract_broker_quantity(broker_payload).unwrap_or_else(|| {
        if is_final_fill_status(&broker_status, broker_substatus.as_deref()) {
            value_f64(order, "quantity")
        } else {
            0.0
        }
    });
    let broker_price = extract_broker_price(broker_payload)
        .or_else(|| optional_f64(order, "price_local"))
        .unwrap_or(0.0);

    // Amendment detection (matches Python execution_engine.py behavior)
    // We compare the broker-reported values against the last known local values.
    let order_quantity = value_f64(order, "quantity");
    let order_price_local = optional_f64(order, "price_local");
    let quantity_changed = (broker_quantity - order_quantity).abs() > 1e-9;
    let price_changed = order_price_local
        .map(|old_price| (broker_price - old_price).abs() > 1e-9)
        .unwrap_or(false);
    let previous_was_amended = order_text(order, "status") == "broker_amended";

    // Enrich the broker state we persist so the UI and audit trail contain the same
    // metadata the legacy Python path produced (quantity_changed, price_changed, last_sync_at).
    let mut enriched_state = broker_state.clone();
    if let Some(obj) = enriched_state.as_object_mut() {
        obj.insert("quantity_changed".to_string(), json!(quantity_changed));
        obj.insert("price_changed".to_string(), json!(price_changed));
        obj.insert("last_sync_at".to_string(), json!(now_iso()));
        if quantity_changed {
            obj.insert("broker_quantity".to_string(), json!(broker_quantity));
        }
        if price_changed {
            obj.insert("broker_price_local".to_string(), json!(broker_price));
        }
    }

    record_broker_order_event(
        state,
        order,
        &broker_order_id,
        if is_final_fill_status(&broker_status, broker_substatus.as_deref()) {
            "broker_final_fill"
        } else {
            "broker_status_sync"
        },
        broker_status.as_deref(),
        broker_substatus.as_deref(),
        Some(broker_quantity),
        Some(broker_price),
        &enriched_state,
    )
    .await?;

    if is_final_fill_status(&broker_status, broker_substatus.as_deref()) {
        return sync_final_fill(
            state,
            order,
            &broker_order_id,
            broker_quantity,
            broker_price,
            &enriched_state,
        )
        .await;
    }

    if is_terminal_failure_status(&broker_status) {
        let status = local_terminal_status(&broker_status);
        update_order_broker_status(state, order, status, &enriched_state, None).await?;
        return Ok(json!({
            "status": status,
            "updated": true,
            "fills": 0,
            "order_id": order_id,
            "symbol": symbol,
            "broker_order_id": broker_order_id,
            "broker_status": broker_status,
            "broker_substatus": broker_substatus,
            "quantity_changed": quantity_changed,
            "price_changed": price_changed,
        }));
    }

    let local_status = if broker_status
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("fill"))
    {
        "broker_partially_filled"
    } else if quantity_changed || price_changed || previous_was_amended {
        // Explicitly surface amendments (quantity or price changed since last known local state).
        // This mirrors the legacy Python logic in execution_engine.py:2424-2429.
        "broker_amended"
    } else {
        "broker_working"
    };
    update_order_broker_status(state, order, local_status, &enriched_state, None).await?;
    Ok(json!({
        "status": local_status,
        "updated": true,
        "fills": 0,
        "order_id": order_id,
        "symbol": symbol,
        "broker_order_id": broker_order_id,
        "broker_status": broker_status,
        "broker_substatus": broker_substatus,
        "quantity_changed": quantity_changed,
        "price_changed": price_changed,
    }))
}

async fn fetch_broker_order_state(
    state: &AppState,
    session: &JsonValue,
    client_key: &str,
    broker_order_id: &str,
) -> Result<Option<JsonValue>> {
    let order_path = format!(
        "/port/v1/orders/{}/{}",
        percent_encode_path_segment(client_key),
        percent_encode_path_segment(broker_order_id)
    );
    if let Some(open_order) = saxo_get_json_optional(
        state,
        session,
        &order_path,
        &[("FieldGroups", "DisplayAndFormat".to_string())],
        "Saxo open order lookup",
    )
    .await?
    .and_then(|payload| meaningful_order_payload(&payload))
    {
        return Ok(Some(json!({
            "source": "port/v1/orders",
            "broker_visibility": "open_order",
            "open_order_lookup": "found",
            "activity_lookup": "skipped",
            "broker_payload": open_order,
            "last_sync_at": now_iso()
        })));
    }

    let activity = saxo_get_json_optional(
        state,
        session,
        "/cs/v1/audit/orderactivities",
        &[
            ("AccountKey", account_key(state, session)?),
            ("ClientKey", client_key.to_string()),
            ("EntryType", "Last".to_string()),
            ("OrderId", broker_order_id.to_string()),
        ],
        "Saxo order activity lookup",
    )
    .await?
    .and_then(|payload| {
        payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .and_then(|items| items.first())
            .cloned()
    });
    Ok(activity.map(|broker_payload| {
        json!({
            "source": "cs/v1/audit/orderactivities",
            "broker_visibility": "activity_only",
            "open_order_lookup": "not_found",
            "activity_lookup": "found",
            "broker_visibility_note": "Saxo open-order lookup returned no active order; using latest audit activity as broker status fallback.",
            "broker_payload": broker_payload,
            "last_sync_at": now_iso()
        })
    }))
}

fn broker_order_lookup_miss_state(broker_order_id: &str) -> JsonValue {
    json!({
        "source": "broker_sync_lookup",
        "broker_visibility": "not_found",
        "open_order_lookup": "not_found",
        "activity_lookup": "not_found",
        "broker_visibility_note": "Saxo returned no active open order and no latest audit activity for this broker order id; local status is left unchanged pending later reconciliation.",
        "broker_order_id": broker_order_id,
        "broker_payload": {"Status": "NotFound"},
        "last_sync_at": now_iso()
    })
}

async fn claim_order_for_submission(state: &AppState, order_id: i64) -> Result<bool> {
    // This is the important idempotency guard. Kubernetes can briefly run an old and a
    // new scheduler pod during rollout, and an operator can also click a manual queue
    // action while the scheduler is awake. The conditional UPDATE lets exactly one
    // process claim a queued order before any Saxo mutation happens.
    let sql = format!(
        "UPDATE execution_orders
         SET status = 'submitting_to_broker', error_text = NULL
         WHERE id = {}
           AND status IN ('pending_execution', 'waiting_for_market_open', 'waiting_for_virtual_cash_budget')
           AND (broker_order_id IS NULL OR broker_order_id = '')",
        order_id
    );
    let result = sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("claiming execution order before Saxo submission")?;
    Ok(result.rows_affected() == 1)
}

async fn execute_order(
    state: &AppState,
    session: &JsonValue,
    market_rows: &[JsonValue],
    broker_positions: &HashMap<String, BrokerPosition>,
    order: &JsonValue,
) -> Result<JsonValue> {
    let order_id = value_i64(order, "id");
    let symbol = order_text(order, "symbol");
    let action = order_text(order, "action").to_uppercase();
    let quantity = value_f64(order, "quantity").floor();

    if quantity < 1.0 {
        return fail_order(
            state,
            order_id,
            "invalid_quantity",
            "Order quantity must be at least 1 whole share",
            &json!({"quantity": value_f64(order, "quantity")}),
        )
        .await;
    }
    normalize_order_quantity(state, order_id, quantity).await?;

    if let Some(market) = market_for_symbol(&symbol, market_rows) {
        let tradable = market
            .get("is_tradable")
            .and_then(JsonValue::as_bool)
            .unwrap_or_else(|| {
                market
                    .get("is_open")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
            });
        if !tradable {
            let error_text = format!(
                "Exchange closed for {symbol}: {}",
                market
                    .get("status_reason")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("closed")
            );
            return mark_waiting_for_market(state, order_id, &error_text, &market).await;
        }
    }

    if action == "SELL" {
        // Local snapshots are useful for the dashboard, but Saxo is the source of truth
        // for whether a sell can be placed. This matters after rollouts or stale imports:
        // Python/Node systems often trust their latest cached row, while broker APIs can
        // already have a later fill that changed the holding.
        let local_snapshot_quantity = latest_position_quantity(state, &symbol).await?;
        let broker_position = broker_positions.get(&symbol);
        let held_quantity = broker_position
            .map(|position| position.quantity.max(0.0))
            .unwrap_or(0.0);
        let reserved_quantity = active_sell_reservations(state, &symbol, order_id).await?;
        let available = (held_quantity - reserved_quantity).max(0.0);
        if available + 1e-9 < quantity {
            let error_text = format!(
                "Sell blocked before Saxo precheck for {symbol}: requested {}, Saxo holdings {}, local snapshot holdings {}, available after active sell reservations {}.",
                compact_number(quantity),
                compact_number(held_quantity),
                compact_number(local_snapshot_quantity),
                compact_number(available)
            );
            return fail_order(
                state,
                order_id,
                "execution_failed",
                &error_text,
                &json!({
                    "sell_guard": {
                        "requested_quantity": quantity,
                        "held_quantity": held_quantity,
                        "local_snapshot_quantity": local_snapshot_quantity,
                        "reserved_quantity": reserved_quantity,
                        "available_quantity": available,
                        "broker_position": broker_position.map(sanitized_broker_position)
                    }
                }),
            )
            .await;
        }
    }

    let closing_position = if action == "SELL" {
        broker_positions.get(&symbol)
    } else {
        None
    };
    let request_payload =
        build_order_payload(state, session, order, quantity, closing_position).await?;
    let precheck = match precheck_order(state, session, &request_payload).await {
        Ok(precheck) => precheck,
        Err(err) => {
            return fail_order(
                state,
                order_id,
                "execution_failed",
                &err.to_string(),
                &json!({
                    "adapter": "saxo",
                    "error": err.to_string(),
                    "payload": sanitized_order_payload(&request_payload),
                    "broker_position": closing_position.map(sanitized_broker_position)
                }),
            )
            .await;
        }
    };
    let broker_result = place_order(state, session, order_id, &request_payload).await?;
    let broker_order_id = broker_order_id(&broker_result);
    let execution_result = json!({
        "precheck": precheck,
        "payload": request_payload,
        "broker_result": broker_result
    });
    mark_submitted(
        state,
        order_id,
        broker_order_id.as_deref(),
        &execution_result,
    )
    .await?;
    info!(
        order_id,
        symbol, broker_order_id, "Saxo order submitted to broker"
    );
    Ok(json!({
        "status": "submitted_to_broker",
        "order_id": order_id,
        "broker_order_id": broker_order_id,
        "broker_result": broker_result,
        "precheck": precheck
    }))
}

async fn build_order_payload(
    state: &AppState,
    session: &JsonValue,
    order: &JsonValue,
    quantity: f64,
    closing_position: Option<&BrokerPosition>,
) -> Result<JsonValue> {
    let symbol = order_text(order, "symbol");
    let action = order_text(order, "action").to_uppercase();
    let order_type = normalize_order_type(&order_text(order, "order_type"));
    let instrument = if action == "SELL" {
        closing_position
            .and_then(|position| position.instrument.clone())
            .or_else(|| instrument_from_reviewed_order(order))
            .ok_or_else(|| {
                anyhow!(
                    "Sell blocked before Saxo precheck for {symbol}: no broker-held instrument metadata was available."
                )
            })?
    } else if let Some(instrument) = instrument_from_reviewed_order(order) {
        instrument
    } else {
        lookup_instrument(state, session, &symbol).await?
    };
    let mut payload = json!({
        "AccountKey": account_key(state, session)?,
        "Amount": quantity as i64,
        "AssetType": instrument.asset_type,
        "BuySell": if action == "BUY" { "Buy" } else { "Sell" },
        "ExternalReference": external_reference(value_i64(order, "id")),
        "ManualOrder": true,
        "OrderDuration": {"DurationType": "DayOrder"},
        "OrderType": order_type,
        "Uic": instrument.uic
    });

    if order_type == "Limit" || order_type == "Stop" || order_type == "StopLimit" {
        let limit_price = optional_f64(order, "limit_price_local");
        let stop_price = optional_f64(order, "stop_price_local");
        if order_type == "Limit" {
            let price =
                limit_price.ok_or_else(|| anyhow!("Limit orders require limit_price_local"))?;
            payload["OrderPrice"] = json!(
                normalize_broker_order_price(
                    state,
                    session,
                    &symbol,
                    &instrument,
                    price,
                    &action,
                    "limit",
                )
                .await?
            );
        } else {
            let price = stop_price
                .ok_or_else(|| anyhow!("{order_type} orders require stop_price_local"))?;
            payload["OrderPrice"] = json!(
                normalize_broker_order_price(
                    state,
                    session,
                    &symbol,
                    &instrument,
                    price,
                    &action,
                    "stop",
                )
                .await?
            );
        }
        if order_type == "StopLimit" {
            let price =
                limit_price.ok_or_else(|| anyhow!("StopLimit orders require limit_price_local"))?;
            payload["StopLimitPrice"] = json!(
                normalize_broker_order_price(
                    state,
                    session,
                    &symbol,
                    &instrument,
                    price,
                    &action,
                    "stop_limit",
                )
                .await?
            );
        }
    }
    Ok(payload)
}

async fn lookup_instrument(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
) -> Result<SaxoInstrument> {
    let parts = symbol_parts(symbol);
    let requested_symbol = requested_symbol(&parts);
    let mut attempts = Vec::new();
    let mut seen_attempts = HashSet::new();
    for variant in symbol_lookup_variants(&parts) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant.eq_ignore_ascii_case(&requested_symbol) {
                "symbol"
            } else {
                "symbol_variant"
            },
            vec![("Keywords", variant)],
            true,
        );
    }
    for variant in base_lookup_variants(&parts.base) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant == parts.base {
                "exchange"
            } else {
                "exchange_variant"
            },
            vec![
                ("Keywords", variant),
                (
                    "ExchangeId",
                    exchange_id_for_suffix(&parts.exchange).to_string(),
                ),
            ],
            true,
        );
    }
    if let Some(isin) = latest_position_isin(state, symbol).await? {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            "isin",
            vec![("Keywords", isin)],
            false,
        );
    }
    for variant in base_lookup_variants(&parts.base) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant == parts.base {
                "base"
            } else {
                "base_variant"
            },
            vec![("Keywords", variant)],
            true,
        );
    }

    for (method, params, require_symbol_match) in attempts {
        let mut query = vec![
            ("$top", "50".to_string()),
            ("AccountKey", account_key(state, session)?),
            ("AssetTypes", TRADABLE_ASSET_TYPES.to_string()),
            ("IncludeNonTradable", "false".to_string()),
        ];
        query.extend(params);
        let payload = saxo_get_json(state, session, "/ref/v1/instruments", &query)
            .await
            .with_context(|| format!("looking up Saxo instrument for {symbol}"))?;
        let candidates = payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let selected = if require_symbol_match {
            candidates
                .iter()
                .filter(|candidate| {
                    candidate_matches_requested(candidate, &requested_symbol, &parts)
                })
                .max_by_key(|candidate| candidate_score(candidate, &requested_symbol, &parts))
        } else {
            candidates
                .iter()
                .max_by_key(|candidate| candidate_score(candidate, &requested_symbol, &parts))
        };
        if let Some(selected) = selected {
            let instrument = SaxoInstrument {
                uic: selected
                    .get("Identifier")
                    .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                    .ok_or_else(|| {
                        anyhow!("Saxo instrument match for {symbol} had no Identifier")
                    })?,
                asset_type: json_text(selected, "AssetType").unwrap_or_else(|| "Stock".to_string()),
                exchange_id: json_text(selected, "ExchangeId")
                    .unwrap_or_else(|| exchange_id_for_suffix(&parts.exchange).to_string()),
                description: json_text(selected, "Description")
                    .unwrap_or_else(|| symbol.to_string()),
            };
            info!(
                symbol,
                saxo_symbol = json_text(selected, "Symbol").unwrap_or_default(),
                method,
                uic = instrument.uic,
                "Saxo instrument resolved"
            );
            return Ok(instrument);
        }
    }
    Err(anyhow!(
        "No tradable Saxo instrument match found for {symbol}"
    ))
}

fn instrument_from_reviewed_order(order: &JsonValue) -> Option<SaxoInstrument> {
    let validation = order.get("request_json")?.get("validation")?;
    if validation.get("status").and_then(JsonValue::as_str) != Some("valid") {
        return None;
    }
    let uic = validation
        .get("uic")
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))?;
    let asset_type = json_text(validation, "asset_type")?;
    Some(SaxoInstrument {
        uic,
        asset_type,
        exchange_id: json_text(validation, "exchange_id").unwrap_or_default(),
        description: json_text(validation, "description")
            .unwrap_or_else(|| order_text(order, "symbol")),
    })
}

async fn broker_position_quantities(
    state: &AppState,
    session: &JsonValue,
) -> Result<HashMap<String, BrokerPosition>> {
    let payload = saxo_get_json(
        state,
        session,
        "/port/v1/positions/me",
        &[(
            "FieldGroups",
            "DisplayAndFormat,PositionBase,PositionView".to_string(),
        )],
    )
    .await?;
    Ok(parse_position_quantities(&payload))
}

fn parse_position_quantities(payload: &JsonValue) -> HashMap<String, BrokerPosition> {
    let mut positions = HashMap::new();
    for (symbol, amount, can_be_closed, instrument) in payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|position| {
            let symbol = position
                .get("DisplayAndFormat")
                .and_then(|value| value.get("Symbol"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let amount = position
                .get("PositionBase")
                .and_then(|value| value.get("Amount"))
                .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))?;
            let can_be_closed = position
                .get("PositionView")
                .and_then(|value| value.get("CanBeClosed"))
                .and_then(JsonValue::as_bool)
                .unwrap_or(true);
            let instrument = broker_position_instrument(position);
            Some((symbol, amount, can_be_closed, instrument))
        })
    {
        let amount_available_to_sell = if can_be_closed { amount.max(0.0) } else { 0.0 };
        let entry = positions
            .entry(symbol)
            .or_insert_with(BrokerPosition::default);
        let previous_quantity = entry.quantity;
        let missing_instrument = entry.instrument.is_none();
        entry.quantity += amount_available_to_sell;
        entry.can_be_closed |= can_be_closed;
        if missing_instrument || amount_available_to_sell > previous_quantity {
            entry.instrument = instrument;
        }
    }
    positions
}

fn broker_position_instrument(position: &JsonValue) -> Option<SaxoInstrument> {
    let base = position.get("PositionBase")?;
    let display = position.get("DisplayAndFormat").unwrap_or(&JsonValue::Null);
    let uic = base
        .get("Uic")
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))?;
    let asset_type = json_text(base, "AssetType")?;
    Some(SaxoInstrument {
        uic,
        asset_type,
        exchange_id: json_text(display, "ExchangeId")
            .or_else(|| json_text(display, "Exchange"))
            .unwrap_or_default(),
        description: json_text(display, "Description")
            .or_else(|| json_text(display, "InstrumentDescription"))
            .unwrap_or_default(),
    })
}

async fn precheck_order(
    state: &AppState,
    session: &JsonValue,
    payload: &JsonValue,
) -> Result<JsonValue> {
    let mut precheck_payload = payload.clone();
    precheck_payload["FieldGroups"] = json!(["Costs", "MarginImpactBuySell"]);
    saxo_post_json(
        state,
        session,
        "/trade/v2/orders/precheck",
        None,
        &precheck_payload,
        "Order precheck",
    )
    .await
}

async fn place_order(
    state: &AppState,
    session: &JsonValue,
    order_id: i64,
    payload: &JsonValue,
) -> Result<JsonValue> {
    let request_id = format!("saxo-rust-{order_id}-{}", Utc::now().format("%Y%m%d%H%M%S"));
    saxo_post_json(
        state,
        session,
        "/trade/v2/orders",
        Some(&request_id),
        payload,
        "Order placement",
    )
    .await
}

async fn saxo_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = session_text(session, "access_token")
        .ok_or_else(|| anyhow!("Saxo access token is missing from session"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!("{}{}", openapi_base_url(state, session)?, path))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await?;
    saxo_response_json(response, "Saxo GET").await
}

async fn saxo_get_json_optional(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
    action: &str,
) -> Result<Option<JsonValue>> {
    let access_token = session_text(session, "access_token")
        .ok_or_else(|| anyhow!("Saxo access token is missing from session"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!("{}{}", openapi_base_url(state, session)?, path))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        if let Some(error_text) = extract_saxo_error(&payload) {
            bail!("{action} failed: {error_text}");
        }
        let snippet: String = body.chars().take(300).collect();
        bail!("{action} failed: HTTP {}: {}", status.as_u16(), snippet);
    }
    if let Some(error_text) = extract_saxo_error(&payload) {
        let lower = error_text.to_ascii_lowercase();
        if lower.contains("not found") || lower.contains("does not exist") {
            return Ok(None);
        }
        bail!("{action} failed: {error_text}");
    }
    Ok(Some(payload))
}

async fn saxo_post_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    request_id: Option<&str>,
    body: &JsonValue,
    action: &str,
) -> Result<JsonValue> {
    let access_token = session_text(session, "access_token")
        .ok_or_else(|| anyhow!("Saxo access token is missing from session"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut last_error = None;
    for attempt in 0..=3 {
        let mut request = client
            .post(format!("{}{}", openapi_base_url(state, session)?, path))
            .bearer_auth(&access_token)
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(body);
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
        }
        let response = request.send().await?;
        if response.status() != StatusCode::TOO_MANY_REQUESTS {
            return saxo_response_json(response, action).await;
        }
        let wait_seconds = retry_after_seconds(&response).unwrap_or(1.0) + 0.25;
        last_error = Some(format!("{action} rate limited by Saxo"));
        warn!(attempt, wait_seconds, "Saxo order request rate limited");
        if attempt < 3 {
            sleep(Duration::from_secs_f64(wait_seconds)).await;
        }
    }
    bail!(last_error.unwrap_or_else(|| format!("{action} rate limited by Saxo")));
}

async fn saxo_response_json(response: reqwest::Response, action: &str) -> Result<JsonValue> {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
    let error_text = extract_saxo_error(&payload);
    if !status.is_success() {
        if let Some(error_text) = error_text {
            bail!("{action} failed: {error_text}");
        }
        let snippet: String = body.chars().take(300).collect();
        bail!("{action} failed: HTTP {}: {}", status.as_u16(), snippet);
    }
    if let Some(error_text) = error_text {
        bail!("{action} failed: {error_text}");
    }
    Ok(payload)
}

fn extract_saxo_error(payload: &JsonValue) -> Option<String> {
    if let Some(error_info) = payload.get("ErrorInfo").and_then(JsonValue::as_object) {
        let code = error_info.get("ErrorCode").and_then(JsonValue::as_str);
        let message = error_info.get("Message").and_then(JsonValue::as_str);
        return match (code, message) {
            (Some(code), Some(message)) => Some(format!("{code}: {message}")),
            (Some(code), None) => Some(code.to_string()),
            (None, Some(message)) => Some(message.to_string()),
            (None, None) => None,
        };
    }
    if let Some(message) = payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("error_description"))
        .and_then(JsonValue::as_str)
    {
        return Some(message.to_string());
    }
    payload
        .get("Orders")
        .and_then(JsonValue::as_array)
        .and_then(|orders| orders.iter().find_map(extract_saxo_error))
}

fn retry_after_seconds(response: &reqwest::Response) -> Option<f64> {
    [
        "X-RateLimit-SessionOrders-Reset",
        "X-RateLimit-Session-Reset",
        "Retry-After",
    ]
    .iter()
    .find_map(|name| {
        response
            .headers()
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
    })
}

async fn normalize_order_quantity(state: &AppState, order_id: i64, quantity: f64) -> Result<()> {
    let sql = format!(
        "UPDATE execution_orders SET quantity = {} WHERE id = {} AND ABS(COALESCE(quantity, 0) - {}) > 0.000000001",
        quantity, order_id, quantity
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    Ok(())
}

async fn mark_waiting_for_market(
    state: &AppState,
    order_id: i64,
    error_text: &str,
    market: &JsonValue,
) -> Result<JsonValue> {
    let payload_text = serde_json::to_string(market)?;
    let sql = format!(
        "UPDATE execution_orders
         SET status = 'waiting_for_market_open', error_text = '{}', execution_result_json = '{}'
         WHERE id = {}",
        sql_escape(error_text),
        sql_escape(&payload_text),
        order_id
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    insert_order_event(
        state,
        order_id,
        None,
        "waiting_for_market_open",
        &json!({"error": error_text, "market": market}),
    )
    .await?;
    Ok(
        json!({"status": "waiting_for_market_open", "order_id": order_id, "error": error_text, "market": market}),
    )
}

async fn mark_submitted(
    state: &AppState,
    order_id: i64,
    broker_order_id: Option<&str>,
    payload: &JsonValue,
) -> Result<()> {
    let now = now_iso();
    let payload_text = serde_json::to_string(payload)?;
    let broker_sql = broker_order_id
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string());
    let sql = format!(
        "UPDATE execution_orders
         SET status = 'submitted_to_broker',
             approved_at = '{}',
             broker_order_id = {},
             execution_result_json = '{}',
             error_text = NULL
         WHERE id = {}",
        sql_escape(&now),
        broker_sql,
        sql_escape(&payload_text),
        order_id
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("marking Saxo order as submitted")?;
    insert_order_event(
        state,
        order_id,
        broker_order_id,
        "submitted_to_broker",
        payload,
    )
    .await
}

async fn fail_order(
    state: &AppState,
    order_id: i64,
    status: &str,
    error_text: &str,
    payload: &JsonValue,
) -> Result<JsonValue> {
    let now = now_iso();
    let payload_text = serde_json::to_string(payload)?;
    let sql = format!(
        "UPDATE execution_orders
         SET status = '{}', approved_at = '{}', error_text = '{}', execution_result_json = '{}'
         WHERE id = {}",
        sql_escape(status),
        sql_escape(&now),
        sql_escape(error_text),
        sql_escape(&payload_text),
        order_id
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    insert_order_event(
        state,
        order_id,
        None,
        if status == "invalid_quantity" {
            "invalid_quantity"
        } else {
            "execution_order_failed"
        },
        payload,
    )
    .await?;
    Ok(json!({"status": status, "order_id": order_id, "error": error_text}))
}

async fn insert_order_event(
    state: &AppState,
    order_id: i64,
    broker_order_id: Option<&str>,
    event_type: &str,
    payload: &JsonValue,
) -> Result<()> {
    let now = now_iso();
    let payload_text = serde_json::to_string(payload)?;
    let broker_sql = broker_order_id
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string());
    let signature = format!("{event_type}:{order_id}:{now}");
    let sql = format!(
        "INSERT INTO execution_order_events (
            created_at, execution_order_id, broker_order_id, event_type, broker_status,
            broker_substatus, broker_quantity, broker_price_local, event_signature,
            raw_payload_json
        ) VALUES (
            '{}', {}, {}, '{}', NULL, NULL, NULL, NULL, '{}', '{}'
        )
        ON CONFLICT(event_signature) DO NOTHING",
        sql_escape(&now),
        order_id,
        broker_sql,
        sql_escape(event_type),
        sql_escape(&signature),
        sql_escape(&payload_text)
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    Ok(())
}

async fn sync_final_fill(
    state: &AppState,
    order: &JsonValue,
    broker_order_id: &str,
    cumulative_quantity: f64,
    average_price_local: f64,
    broker_state: &JsonValue,
) -> Result<JsonValue> {
    let order_id = value_i64(order, "id");
    let symbol = order_text(order, "symbol");
    let side = order_text(order, "action").to_uppercase();
    let synced_quantity = synced_fill_quantity(state, order_id).await?;
    let delta_quantity = (cumulative_quantity - synced_quantity).max(0.0);
    let mut ledger_id = latest_fill_ledger_id(state, order_id).await?;
    let mut inserted_fill = 0;
    if delta_quantity > 1e-9 {
        let currency = resolve_order_currency(state, order, broker_payload(broker_state)).await?;
        let new_ledger_id = insert_trade_ledger_for_fill(
            state,
            order,
            &side,
            delta_quantity,
            average_price_local,
            &currency,
            broker_state,
        )
        .await?;
        insert_execution_fill(
            state,
            order,
            broker_order_id,
            "FinalFill",
            cumulative_quantity,
            delta_quantity,
            average_price_local,
            &currency,
            Some(new_ledger_id),
            broker_state,
        )
        .await?;
        apply_fill_to_local_book(
            state,
            order,
            &side,
            delta_quantity,
            average_price_local,
            &currency,
            new_ledger_id,
        )
        .await?;
        ledger_id = Some(new_ledger_id);
        inserted_fill = 1;
        // Backfill the order row so the UI shows the fill price for market
        // orders that were submitted without a known price.
        sqlx::query(&format!(
            "UPDATE execution_orders
             SET price_local = {average_price_local},
                 currency = COALESCE(currency, '{}')
             WHERE id = {order_id} AND price_local IS NULL",
            sql_escape(&currency)
        ))
        .execute(&state.pool)
        .await?;
    }
    update_order_broker_status(state, order, "executed", broker_state, ledger_id).await?;
    Ok(json!({
        "status": "executed",
        "updated": true,
        "fills": inserted_fill,
        "order_id": order_id,
        "symbol": symbol,
        "broker_order_id": broker_order_id,
        "cumulative_quantity": cumulative_quantity,
        "synced_before": synced_quantity,
        "delta_quantity": delta_quantity,
        "average_price_local": average_price_local,
        "ledger_id": ledger_id
    }))
}

async fn synced_fill_quantity(state: &AppState, order_id: i64) -> Result<f64> {
    let row = sqlx::query(&format!(
        "SELECT COALESCE(SUM(delta_quantity), 0) AS synced_quantity
         FROM execution_fills
         WHERE execution_order_id = {}",
        order_id
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<f64, _>("synced_quantity").ok())
        .unwrap_or(0.0))
}

async fn latest_fill_ledger_id(state: &AppState, order_id: i64) -> Result<Option<i64>> {
    let row = sqlx::query(&format!(
        "SELECT ledger_id
         FROM execution_fills
         WHERE execution_order_id = {} AND ledger_id IS NOT NULL
         ORDER BY id DESC
         LIMIT 1",
        order_id
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(|row| row.try_get::<i64, _>("ledger_id").ok()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_execution_fill(
    state: &AppState,
    order: &JsonValue,
    broker_order_id: &str,
    fill_status: &str,
    cumulative_quantity: f64,
    delta_quantity: f64,
    average_price_local: f64,
    currency: &str,
    ledger_id: Option<i64>,
    payload: &JsonValue,
) -> Result<()> {
    let now = now_iso();
    let payload_text = serde_json::to_string(payload)?;
    let ledger_sql = ledger_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string());
    let sql = format!(
        "INSERT INTO execution_fills (
            created_at, execution_order_id, broker_order_id, symbol, side, fill_status,
            cumulative_quantity, delta_quantity, average_price_local, currency, ledger_id,
            raw_payload_json
        ) VALUES (
            '{}', {}, '{}', '{}', '{}', '{}', {}, {}, {}, '{}', {}, '{}'
        )",
        sql_escape(&now),
        value_i64(order, "id"),
        sql_escape(broker_order_id),
        sql_escape(&order_text(order, "symbol")),
        sql_escape(&order_text(order, "action").to_uppercase()),
        sql_escape(fill_status),
        cumulative_quantity,
        delta_quantity,
        average_price_local,
        sql_escape(currency),
        ledger_sql,
        sql_escape(&payload_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording Saxo execution fill")?;
    Ok(())
}

async fn insert_trade_ledger_for_fill(
    state: &AppState,
    order: &JsonValue,
    side: &str,
    quantity: f64,
    price_local: f64,
    currency: &str,
    broker_state: &JsonValue,
) -> Result<i64> {
    let now = now_iso();
    let symbol = order_text(order, "symbol");
    let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
    let gross_local = price_local * quantity;
    let gross_amount_dkk = gross_local * fx_rate;
    let commission_dkk = commission_dkk_for_fill(order, gross_amount_dkk, currency);
    let commission_local = if fx_rate.abs() > f64::EPSILON {
        commission_dkk / fx_rate
    } else {
        0.0
    };
    let cost_basis = if side == "SELL" {
        latest_position_cost_basis(state, &symbol).await?
    } else {
        PositionCostBasis::default()
    };
    let cost_basis_sold_dkk = prorated(cost_basis.cost_basis_dkk, quantity, cost_basis.quantity);
    let cost_basis_sold_local =
        prorated(cost_basis.cost_basis_local, quantity, cost_basis.quantity);
    let tax_dkk = 0.0;
    let net_amount_dkk = if side == "BUY" {
        -(gross_amount_dkk + commission_dkk + tax_dkk)
    } else {
        gross_amount_dkk - commission_dkk - tax_dkk
    };
    let realised_gain_dkk = if side == "SELL" {
        net_amount_dkk - cost_basis_sold_dkk
    } else {
        0.0
    };
    let realised_gain_local = if side == "SELL" && fx_rate.abs() > f64::EPSILON {
        realised_gain_dkk / fx_rate
    } else {
        0.0
    };
    let notes = format!(
        "Saxo broker fill sync from execution_order:{}",
        value_i64(order, "id")
    );
    let decision_context = json!({
        "execution_order": order,
        "broker_sync": broker_state
    });
    let decision_context_text = serde_json::to_string(&decision_context)?;
    let portfolio_placeholder = serde_json::to_string(&json!({}))?;
    let instrument_name = cost_basis.instrument_name.unwrap_or_else(|| symbol.clone());
    let isin_sql = sql_opt_text(cost_basis.isin.as_deref());
    let tax_year = Utc::now().year();
    let batch_id_sql = latest_batch_id(state)
        .await?
        .as_deref()
        .map(|value| sql_opt_text(Some(value)))
        .unwrap_or_else(|| "NULL".to_string());
    let sql = format!(
        "INSERT INTO trade_ledger (
            created_at, symbol, isin, figi, instrument_name, side, quantity, price_local,
            currency, gross_amount_dkk, commission_dkk, commission_local, fx_conversion_dkk,
            tax_dkk, realised_gain_dkk, cost_basis_sold_dkk, cost_basis_sold_local,
            realised_gain_local, fx_gain_dkk, price_gain_dkk, sale_fx_rate_to_dkk,
            cost_basis_fx_rate_to_dkk, net_amount_dkk, mode, status, notes,
            portfolio_before_json, portfolio_after_json, decision_context_json, tax_year, batch_id
        ) VALUES (
            '{}', '{}', {}, NULL, '{}', '{}', {}, {}, '{}', {}, {}, {}, 0, {}, {}, {}, {},
            {}, 0, {}, {}, {}, {}, '{}', 'executed', '{}', '{}', '{}', '{}', {}, {}
        )",
        sql_escape(&now),
        sql_escape(&symbol),
        isin_sql,
        sql_escape(&instrument_name),
        sql_escape(side),
        quantity,
        price_local,
        sql_escape(currency),
        gross_amount_dkk,
        commission_dkk,
        commission_local,
        tax_dkk,
        realised_gain_dkk,
        cost_basis_sold_dkk,
        cost_basis_sold_local,
        realised_gain_local,
        realised_gain_dkk,
        fx_rate,
        if cost_basis_sold_local.abs() > f64::EPSILON {
            cost_basis_sold_dkk / cost_basis_sold_local
        } else {
            fx_rate
        },
        net_amount_dkk,
        sql_escape(&order_text(order, "mode")),
        sql_escape(&notes),
        sql_escape(&portfolio_placeholder),
        sql_escape(&portfolio_placeholder),
        sql_escape(&decision_context_text),
        tax_year,
        batch_id_sql
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording Saxo broker fill in trade ledger")?;
    let row = sqlx::query(&format!(
        "SELECT id
         FROM trade_ledger
         WHERE notes = '{}' AND symbol = '{}' AND side = '{}'
         ORDER BY id DESC
         LIMIT 1",
        sql_escape(&notes),
        sql_escape(&symbol),
        sql_escape(side)
    ))
    .fetch_one(&state.pool)
    .await
    .context("reading inserted trade ledger id")?;
    Ok(row.try_get::<i64, _>("id").unwrap_or(0))
}

async fn update_order_broker_status(
    state: &AppState,
    order: &JsonValue,
    status: &str,
    broker_state: &JsonValue,
    ledger_id: Option<i64>,
) -> Result<()> {
    let order_id = value_i64(order, "id");
    let mut result = order
        .get("execution_result_json")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !result.is_object() {
        result = json!({});
    }
    result["broker_sync"] = broker_state.clone();
    let payload_text = serde_json::to_string(&result)?;
    let ledger_sql = ledger_id
        .map(|value| format!(", ledger_id = {value}"))
        .unwrap_or_default();
    let error_sql = if matches!(
        status,
        "execution_failed" | "broker_cancelled" | "broker_expired" | "broker_done_for_day"
    ) {
        let broker_payload = broker_payload(broker_state);
        let status_text = broker_status_text(broker_payload).unwrap_or_else(|| status.to_string());
        format!(", error_text = '{}'", sql_escape(&status_text))
    } else {
        ", error_text = NULL".to_string()
    };
    let currency_sql = match resolve_order_currency(state, order, broker_payload(broker_state))
        .await
    {
        Ok(currency) if !currency.trim().is_empty() => format!(
            ", currency = CASE WHEN currency IS NULL OR TRIM(currency) = '' THEN '{}' ELSE currency END",
            sql_escape(currency.trim())
        ),
        _ => String::new(),
    };
    let sql = format!(
        "UPDATE execution_orders
         SET status = '{}',
             execution_result_json = '{}'
             {}{}{}
         WHERE id = {}",
        sql_escape(status),
        sql_escape(&payload_text),
        ledger_sql,
        error_sql,
        currency_sql,
        order_id
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("updating Saxo execution order broker status")?;
    Ok(())
}

async fn record_broker_order_event(
    state: &AppState,
    order: &JsonValue,
    broker_order_id: &str,
    event_type: &str,
    broker_status: Option<&str>,
    broker_substatus: Option<&str>,
    broker_quantity: Option<f64>,
    broker_price_local: Option<f64>,
    payload: &JsonValue,
) -> Result<()> {
    let now = now_iso();
    let payload_text = serde_json::to_string(payload)?;
    let quantity_sql = broker_quantity
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string());
    let price_sql = broker_price_local
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string());
    let signature = broker_event_signature(
        value_i64(order, "id"),
        event_type,
        broker_order_id,
        broker_status,
        broker_substatus,
        broker_quantity,
        broker_price_local,
    );
    let sql = format!(
        "INSERT INTO execution_order_events (
            created_at, execution_order_id, broker_order_id, event_type,
            broker_status, broker_substatus, broker_quantity, broker_price_local,
            event_signature, raw_payload_json
        ) VALUES (
            '{}', {}, '{}', '{}', {}, {}, {}, {}, '{}', '{}'
        )
        ON CONFLICT(event_signature) DO NOTHING",
        sql_escape(&now),
        value_i64(order, "id"),
        sql_escape(broker_order_id),
        sql_escape(event_type),
        sql_opt_text(broker_status),
        sql_opt_text(broker_substatus),
        quantity_sql,
        price_sql,
        sql_escape(&signature),
        sql_escape(&payload_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording Saxo broker order event")?;
    Ok(())
}

async fn latest_position_quantity(state: &AppState, symbol: &str) -> Result<f64> {
    let latest_batch = sqlx::query(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?
    .and_then(|row| row.try_get::<String, _>("batch_id").ok());
    let where_batch = latest_batch
        .as_ref()
        .map(|batch| format!(" AND batch_id = '{}'", sql_escape(batch)))
        .unwrap_or_default();
    let row = sqlx::query(&format!(
        "SELECT quantity FROM position_snapshots WHERE symbol = '{}' AND excluded = 0{} ORDER BY id DESC LIMIT 1",
        sql_escape(symbol),
        where_batch
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<f64, _>("quantity").ok())
        .unwrap_or(0.0))
}

async fn latest_position_isin(state: &AppState, symbol: &str) -> Result<Option<String>> {
    let latest_batch = sqlx::query(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?
    .and_then(|row| row.try_get::<String, _>("batch_id").ok());
    let where_batch = latest_batch
        .as_ref()
        .map(|batch| format!(" AND batch_id = '{}'", sql_escape(batch)))
        .unwrap_or_default();
    let row = sqlx::query(&format!(
        "SELECT isin FROM position_snapshots WHERE symbol = '{}' AND excluded = 0{} ORDER BY id DESC LIMIT 1",
        sql_escape(symbol),
        where_batch
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<String, _>("isin").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

async fn latest_position_cost_basis(state: &AppState, symbol: &str) -> Result<PositionCostBasis> {
    let row = sqlx::query(&format!(
        "SELECT quantity, cost_basis_dkk, cost_basis_local, isin, instrument_name
         FROM position_snapshots
         WHERE symbol = '{}' AND excluded = 0
         ORDER BY imported_at DESC, id DESC
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    let local = row.as_ref().map(row_to_json).map(|row| PositionCostBasis {
        quantity: value_f64(&row, "quantity"),
        cost_basis_dkk: value_f64(&row, "cost_basis_dkk"),
        cost_basis_local: value_f64(&row, "cost_basis_local"),
        isin: json_text(&row, "isin"),
        instrument_name: json_text(&row, "instrument_name"),
    });
    if let Some(local) = local {
        if local.quantity > 0.0 && local.cost_basis_dkk > 0.0 {
            return Ok(local);
        }
    }
    // Positions acquired outside the snapshot imports (broker-side buys or
    // ledger buys that never wrote a snapshot) have no local basis; booking a
    // zero basis would record the full sale proceeds as realised gain. Fall
    // back to the broker-authoritative open price for the live position.
    if let Some(broker) = broker_position_cost_basis(state, symbol).await? {
        tracing::warn!(
            "No usable local cost basis for {symbol}; using broker-authoritative open price as basis"
        );
        return Ok(broker);
    }
    tracing::warn!(
        "No local or broker cost basis for {symbol}; SELL accounting will book a zero basis"
    );
    Ok(PositionCostBasis::default())
}

/// Broker-authoritative fallback basis: the open price (including costs when
/// available) of the live broker position, converted to DKK via the cached FX
/// rate for the position currency.
async fn broker_position_cost_basis(
    state: &AppState,
    symbol: &str,
) -> Result<Option<PositionCostBasis>> {
    let row = sqlx::query(&format!(
        "SELECT quantity, open_price_local, open_price_including_costs_local,
                currency, isin, instrument_name
         FROM broker_position_snapshots
         WHERE symbol = '{}' AND quantity > 0
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row.as_ref().map(row_to_json) else {
        return Ok(None);
    };
    let quantity = value_f64(&row, "quantity");
    let open_price = Some(value_f64(&row, "open_price_including_costs_local"))
        .filter(|price| *price > 0.0)
        .unwrap_or_else(|| value_f64(&row, "open_price_local"));
    if quantity <= 0.0 || open_price <= 0.0 {
        return Ok(None);
    }
    let currency = json_text(&row, "currency").unwrap_or_else(|| "DKK".to_string());
    let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, &currency).await;
    let cost_basis_local = open_price * quantity;
    Ok(Some(PositionCostBasis {
        quantity,
        cost_basis_dkk: cost_basis_local * fx_rate,
        cost_basis_local,
        isin: json_text(&row, "isin"),
        instrument_name: json_text(&row, "instrument_name"),
    }))
}

/// Applies a broker fill to the local position book (position_snapshots +
/// position_lots) so positions this system opens carry a real local cost
/// basis instead of depending on the broker fallback at SELL time. BUY fills
/// add quantity and commission-inclusive basis (creating the snapshot and a
/// lot when the position is new); SELL fills remove quantity and prorated
/// basis so subsequent proration stays correct across re-buys.
async fn apply_fill_to_local_book(
    state: &AppState,
    order: &JsonValue,
    side: &str,
    delta_quantity: f64,
    average_price_local: f64,
    currency: &str,
    ledger_id: i64,
) -> Result<()> {
    if delta_quantity <= 0.0 || average_price_local <= 0.0 {
        return Ok(());
    }
    let symbol = order_text(order, "symbol");
    let order_id = value_i64(order, "id");
    let snapshot = sqlx::query(&format!(
        "SELECT id, quantity, cost_basis_local, cost_basis_dkk
         FROM position_snapshots
         WHERE symbol = '{}' AND excluded = 0
         ORDER BY imported_at DESC, id DESC
         LIMIT 1",
        sql_escape(&symbol)
    ))
    .fetch_optional(&state.pool)
    .await?
    .as_ref()
    .map(row_to_json);

    if side == "SELL" {
        let Some(snapshot) = snapshot else {
            tracing::warn!(
                "SELL fill for {symbol} has no local snapshot to decrement; the trade ledger basis came from the broker fallback"
            );
            return Ok(());
        };
        let held = value_f64(&snapshot, "quantity");
        if held <= 0.0 {
            return Ok(());
        }
        let remaining = (held - delta_quantity).max(0.0);
        let fraction = remaining / held;
        sqlx::query(&format!(
            "UPDATE position_snapshots
             SET quantity = {remaining},
                 cost_basis_local = {},
                 cost_basis_dkk = {}
             WHERE id = {}",
            value_f64(&snapshot, "cost_basis_local") * fraction,
            value_f64(&snapshot, "cost_basis_dkk") * fraction,
            value_i64(&snapshot, "id")
        ))
        .execute(&state.pool)
        .await
        .context("decrementing local position snapshot for SELL fill")?;
        return Ok(());
    }

    let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
    let gross_local = average_price_local * delta_quantity;
    let gross_dkk = gross_local * fx_rate;
    let commission_dkk = commission_dkk_for_fill(order, gross_dkk, currency);
    let commission_local = if fx_rate.abs() > f64::EPSILON {
        commission_dkk / fx_rate
    } else {
        0.0
    };
    let basis_local = gross_local + commission_local;
    let basis_dkk = gross_dkk + commission_dkk;
    let now = now_iso();
    let batch_id = fill_sync_batch_id(state).await?;
    let raw_payload = serde_json::to_string(&json!({
        "source": "fill_sync",
        "execution_order_id": order_id,
        "ledger_id": ledger_id,
        "delta_quantity": delta_quantity,
        "average_price_local": average_price_local,
        "commission_dkk": commission_dkk,
        "currency": currency,
        "fx_rate_to_dkk": fx_rate
    }))?;

    match snapshot {
        Some(snapshot) => {
            let new_quantity = value_f64(&snapshot, "quantity").max(0.0) + delta_quantity;
            let new_basis_local = value_f64(&snapshot, "cost_basis_local") + basis_local;
            let new_basis_dkk = value_f64(&snapshot, "cost_basis_dkk") + basis_dkk;
            sqlx::query(&format!(
                "UPDATE position_snapshots
                 SET quantity = {new_quantity},
                     cost_basis_local = {new_basis_local},
                     cost_basis_dkk = {new_basis_dkk},
                     open_price_local = {}
                 WHERE id = {}",
                new_basis_local / new_quantity,
                value_i64(&snapshot, "id")
            ))
            .execute(&state.pool)
            .await
            .context("adding BUY fill to local position snapshot")?;
        }
        None => {
            let instrument_name =
                json_text(order, "instrument_name").unwrap_or_else(|| symbol.clone());
            let isin_sql = sql_opt_text(json_text(order, "isin").as_deref());
            sqlx::query(&format!(
                "INSERT INTO position_snapshots (
                    batch_id, imported_at, instrument_name, symbol, isin, quantity, currency,
                    open_price_local, current_price_local, cost_basis_local, cost_basis_dkk,
                    market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk,
                    allocation_pct, status, account_name, asset_class, market_status,
                    value_date, source_csv, excluded, raw_payload_json
                ) VALUES (
                    '{}', '{}', '{}', '{}', {}, {}, '{}', {}, {}, {}, {}, {}, {}, 0, 0, 0,
                    'Open', 'Fill-Sync', 'Stock', 'Open', '{}', 'fill_sync', 0, '{}'
                )",
                sql_escape(&batch_id),
                sql_escape(&now),
                sql_escape(&instrument_name),
                sql_escape(&symbol),
                isin_sql,
                delta_quantity,
                sql_escape(currency),
                basis_local / delta_quantity,
                average_price_local,
                basis_local,
                basis_dkk,
                gross_local,
                gross_dkk,
                sql_escape(&now),
                sql_escape(&raw_payload)
            ))
            .execute(&state.pool)
            .await
            .context("creating local position snapshot for BUY fill")?;
        }
    }

    let instrument_name = json_text(order, "instrument_name").unwrap_or_else(|| symbol.clone());
    sqlx::query(&format!(
        "INSERT INTO position_lots (
            lot_id, batch_id, created_at, acquired_at, symbol, isin, instrument_name,
            quantity_original, currency, cost_basis_total_local, cost_basis_total_dkk,
            fx_rate_to_dkk, source_type, source_reference, raw_payload_json
        ) VALUES (
            'buy-fill:{order_id}:{ledger_id}', '{}', '{}', '{}', '{}', {}, '{}', {}, '{}',
            {}, {}, {}, 'buy_fill', 'execution_order:{order_id}', '{}'
        )
        ON CONFLICT (lot_id) DO NOTHING",
        sql_escape(&batch_id),
        sql_escape(&now),
        sql_escape(&now),
        sql_escape(&symbol),
        sql_opt_text(json_text(order, "isin").as_deref()),
        sql_escape(&instrument_name),
        delta_quantity,
        sql_escape(currency),
        basis_local,
        basis_dkk,
        fx_rate,
        sql_escape(&raw_payload)
    ))
    .execute(&state.pool)
    .await
    .context("recording position lot for BUY fill")?;
    Ok(())
}

/// Batch id used for fill-sync snapshot/lot rows. Reuses the latest import
/// batch so latest-batch queries keep seeing the whole book; creates a
/// dedicated batch only when the database has none yet.
async fn fill_sync_batch_id(state: &AppState) -> Result<String> {
    if let Some(batch) = latest_batch_id(state).await? {
        return Ok(batch);
    }
    let now = now_iso();
    let batch_id = format!("fill-sync-{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
    sqlx::query(&format!(
        "INSERT INTO import_batches (
            batch_id, imported_at, source_csv, source_position_count,
            imported_position_count, excluded_position_count, notes
        ) VALUES (
            '{}', '{}', 'fill_sync', 0, 0, 0,
            'Batch created by broker fill sync for positions opened outside any import.'
        )",
        sql_escape(&batch_id),
        sql_escape(&now)
    ))
    .execute(&state.pool)
    .await
    .context("creating fill-sync import batch")?;
    Ok(batch_id)
}

async fn latest_batch_id(state: &AppState) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(|row| row.try_get::<String, _>("batch_id").ok()))
}

async fn resolve_order_currency(
    state: &AppState,
    order: &JsonValue,
    broker_payload: &JsonValue,
) -> Result<String> {
    if let Some(currency) = json_text(order, "currency")
        .or_else(|| json_text(broker_payload, "Currency"))
        .or_else(|| nested_json_text(broker_payload, &["DisplayAndFormat", "Currency"]))
    {
        return Ok(currency);
    }
    let symbol = order_text(order, "symbol");
    let row = sqlx::query(&format!(
        "SELECT currency
         FROM position_snapshots
         WHERE symbol = '{}' AND excluded = 0
         ORDER BY imported_at DESC, id DESC
         LIMIT 1",
        sql_escape(&symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    if let Some(currency) = row
        .as_ref()
        .map(row_to_json)
        .and_then(|row| json_text(&row, "currency"))
    {
        return Ok(currency);
    }
    // Exchange suffix is a reliable currency signal for symbols that were
    // never held locally; a blind DKK default silently corrupts FX math.
    Ok(currency_for_exchange(&symbol_parts(&symbol).exchange)
        .unwrap_or("DKK")
        .to_string())
}

async fn active_sell_reservations(
    state: &AppState,
    symbol: &str,
    exclude_order_id: i64,
) -> Result<f64> {
    let statuses = ACTIVE_SELL_STATUSES
        .iter()
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ");
    let row = sqlx::query(&format!(
        "SELECT COALESCE(SUM(quantity), 0) AS quantity
         FROM execution_orders
         WHERE symbol = '{}'
           AND action = 'SELL'
           AND id <> {}
           AND status IN ({})",
        sql_escape(symbol),
        exclude_order_id,
        statuses
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<f64, _>("quantity").ok())
        .unwrap_or(0.0))
}

fn openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = session_text(session, "environment")
        .or_else(|| yaml_string(&state.config, &["saxo", "environment"]))
        .unwrap_or_else(|| "sim".to_string())
        .to_lowercase();
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn account_key(state: &AppState, session: &JsonValue) -> Result<String> {
    yaml_string(&state.config, &["saxo", "account_key"])
        .or_else(|| session_text(session, "account_key"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Saxo AccountKey is missing"))
}

fn client_key(state: &AppState, session: &JsonValue) -> Result<String> {
    yaml_string(&state.config, &["saxo", "client_key"])
        .or_else(|| session_text(session, "client_key"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Saxo ClientKey is missing"))
}

fn meaningful_order_payload(payload: &JsonValue) -> Option<JsonValue> {
    if payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .and_then(|items| items.first())
        .is_some()
    {
        return payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .and_then(|items| items.first())
            .cloned();
    }
    if payload.as_object().is_some_and(|object| !object.is_empty())
        && (payload.get("OrderId").is_some()
            || payload.get("Status").is_some()
            || payload.get("Amount").is_some())
    {
        return Some(payload.clone());
    }
    None
}

fn broker_payload(value: &JsonValue) -> &JsonValue {
    value.get("broker_payload").unwrap_or(value)
}

fn broker_status_text(value: &JsonValue) -> Option<String> {
    json_text(value, "Status").or_else(|| json_text(value, "OrderStatus"))
}

fn is_final_fill_status(status: &Option<String>, substatus: Option<&str>) -> bool {
    status.as_deref().is_some_and(|value| {
        value.eq_ignore_ascii_case("FinalFill")
            && substatus
                .map(|substatus| {
                    substatus.is_empty() || substatus.eq_ignore_ascii_case("Confirmed")
                })
                .unwrap_or(true)
    })
}

fn is_terminal_failure_status(status: &Option<String>) -> bool {
    status.as_deref().is_some_and(|value| {
        ["Rejected", "Cancelled", "Expired", "DoneForDay"]
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
    })
}

fn local_terminal_status(status: &Option<String>) -> &'static str {
    match status
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "cancelled" => "broker_cancelled",
        "expired" => "broker_expired",
        "doneforday" => "broker_done_for_day",
        _ => "execution_failed",
    }
}

fn extract_broker_quantity(payload: &JsonValue) -> Option<f64> {
    [
        "FilledAmount",
        "Amount",
        "CurrentAmount",
        "OrderAmount",
        "LeavesAmount",
        "OriginalAmount",
    ]
    .iter()
    .find_map(|key| json_number(payload, key))
}

fn extract_broker_price(payload: &JsonValue) -> Option<f64> {
    [
        "ExecutionPrice",
        "AveragePrice",
        "OrderPrice",
        "Price",
        "OrderPriceDisplay",
    ]
    .iter()
    .find_map(|key| json_number(payload, key))
}

fn resolve_broker_order_id(order: &JsonValue) -> Option<String> {
    json_text(order, "broker_order_id").or_else(|| {
        order
            .get("execution_result_json")
            .and_then(|value| value.get("broker_result"))
            .and_then(broker_order_id)
    })
}

fn market_for_symbol(symbol: &str, rows: &[JsonValue]) -> Option<JsonValue> {
    let exchange = symbol_parts(symbol).exchange;
    let exchange_id = exchange_id_for_suffix(&exchange);
    rows.iter()
        .find(|row| {
            row.get("code")
                .and_then(JsonValue::as_str)
                .is_some_and(|code| code.eq_ignore_ascii_case(exchange_id))
        })
        .cloned()
}

fn symbol_parts(symbol: &str) -> SymbolParts {
    let mut split = symbol.trim().splitn(2, ':');
    let base = split.next().unwrap_or("").trim().to_uppercase();
    let exchange = split.next().unwrap_or("").trim().to_lowercase();
    SymbolParts { base, exchange }
}

fn requested_symbol(parts: &SymbolParts) -> String {
    if parts.exchange.is_empty() {
        parts.base.clone()
    } else {
        format!("{}:{}", parts.base, parts.exchange)
    }
}

fn base_lookup_variants(base: &str) -> Vec<String> {
    let mut variants = Vec::new();
    push_unique_string(&mut variants, base.trim().to_uppercase());
    let Some((prefix, share_class)) = base.trim().split_once('-') else {
        return variants;
    };
    let share_class = share_class.trim();
    if prefix.trim().is_empty() || share_class.len() != 1 || !share_class.is_ascii() {
        return variants;
    }
    let mut chars = share_class.chars();
    let class = chars.next().unwrap().to_ascii_lowercase();
    push_unique_string(
        &mut variants,
        format!("{}{}", prefix.trim().to_uppercase(), class),
    );
    variants
}

fn symbol_lookup_variants(parts: &SymbolParts) -> Vec<String> {
    base_lookup_variants(&parts.base)
        .into_iter()
        .map(|base| {
            if parts.exchange.is_empty() {
                base
            } else {
                format!("{base}:{}", parts.exchange)
            }
        })
        .collect()
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn push_lookup_attempt(
    attempts: &mut Vec<(String, Vec<(&'static str, String)>, bool)>,
    seen: &mut HashSet<String>,
    method: &str,
    params: Vec<(&'static str, String)>,
    require_symbol_match: bool,
) {
    let signature = format!("{require_symbol_match}:{params:?}");
    if seen.insert(signature) {
        attempts.push((method.to_string(), params, require_symbol_match));
    }
}

fn exchange_id_for_suffix(exchange: &str) -> &'static str {
    match exchange.to_lowercase().as_str() {
        "xnas" => "XNAS",
        "xnys" => "XNYS",
        "xcse" => "XCSE",
        "xsto" => "XSTO",
        "xosl" => "XOSL",
        "xhel" => "XHEL",
        "xlon" => "XLON",
        "xetr" => "XETR",
        "xfra" => "XFRA",
        "xmil" => "XMIL",
        "xpar" => "XPAR",
        "xams" => "XAMS",
        "xbru" => "XBRU",
        "xlse" => "XLIS",
        _ => "",
    }
}

fn exchange_aliases(exchange: &str) -> Vec<&'static str> {
    match exchange.to_lowercase().as_str() {
        "xnas" => vec!["XNAS", "NASDAQ"],
        "xnys" => vec!["XNYS", "NYSE"],
        "xcse" => vec!["XCSE", "CSE", "COP"],
        "xsto" => vec!["XSTO", "STO", "STK"],
        "xosl" => vec!["XOSL", "OSL", "OSE"],
        "xhel" => vec!["XHEL", "HEL", "HEX"],
        "xlon" => vec!["XLON", "LSE", "LON"],
        "xetr" => vec!["XETR", "XTRA", "ETR"],
        "xfra" => vec!["XFRA", "FSE", "FRA"],
        "xmil" => vec!["XMIL", "MIL"],
        "xpar" => vec!["XPAR", "PAR"],
        "xams" => vec!["XAMS", "AMS"],
        "xbru" => vec!["XBRU", "BRU"],
        "xlse" => vec!["XLIS", "LIS"],
        _ => vec![],
    }
}

fn candidate_score(
    candidate: &JsonValue,
    requested_symbol: &str,
    parts: &SymbolParts,
) -> (i32, i32, i32) {
    let candidate_symbol = json_text(candidate, "Symbol")
        .unwrap_or_default()
        .to_uppercase();
    let candidate_exchange = json_text(candidate, "ExchangeId")
        .unwrap_or_default()
        .to_uppercase();
    let requested_variants = symbol_lookup_variants(parts)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    let base_variants = base_lookup_variants(&parts.base)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    let exact_symbol = i32::from(
        candidate_symbol == requested_symbol.to_uppercase()
            || requested_variants.contains(&candidate_symbol),
    );
    let exchange_match = i32::from(
        exchange_aliases(&parts.exchange)
            .iter()
            .any(|alias| candidate_exchange == *alias)
            || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase())),
    );
    let exact_base = i32::from(
        candidate_symbol
            .split(':')
            .next()
            .is_some_and(|base| base == parts.base || base_variants.contains(base)),
    );
    let stock_preferred = i32::from(
        candidate
            .get("TradableAs")
            .and_then(JsonValue::as_array)
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some("Stock"))),
    );
    (exact_symbol, exchange_match, exact_base + stock_preferred)
}

fn candidate_matches_requested(
    candidate: &JsonValue,
    requested_symbol: &str,
    parts: &SymbolParts,
) -> bool {
    let candidate_symbol = json_text(candidate, "Symbol")
        .unwrap_or_default()
        .to_uppercase();
    let candidate_base = candidate_symbol.split(':').next().unwrap_or("");
    let requested_variants = symbol_lookup_variants(parts)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    if candidate_symbol == requested_symbol.to_uppercase()
        || requested_variants.contains(&candidate_symbol)
    {
        return true;
    }
    let base_variants = base_lookup_variants(&parts.base)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    if candidate_base != parts.base && !base_variants.contains(candidate_base) {
        return false;
    }
    if parts.exchange.is_empty() {
        return true;
    }
    let candidate_exchange = json_text(candidate, "ExchangeId")
        .unwrap_or_default()
        .to_uppercase();
    exchange_aliases(&parts.exchange)
        .iter()
        .any(|alias| candidate_exchange == *alias)
        || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase()))
}

fn normalize_order_type(value: &str) -> &'static str {
    match value.to_lowercase().as_str() {
        "limit" => "Limit",
        "stop" => "Stop",
        "stoplimit" | "stop_limit" | "stop-limit" => "StopLimit",
        _ => "Market",
    }
}

#[cfg(test)]
fn normalize_order_price(symbol: &str, price: f64, action: &str, role: &str) -> f64 {
    let tick = default_tick(symbol);
    normalize_order_price_with_tick(price, tick, action, role)
}

async fn normalize_broker_order_price(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
    instrument: &SaxoInstrument,
    price: f64,
    action: &str,
    role: &str,
) -> Result<f64> {
    let tick = if let Some(tick) = configured_price_tick_override(&state.config, symbol) {
        tick
    } else {
        match instrument_tick_size(state, session, instrument, price).await {
            Ok(Some(tick)) => tick,
            Ok(None) => default_tick(symbol),
            Err(err) => {
                warn!(
                    symbol,
                    uic = instrument.uic,
                    asset_type = %instrument.asset_type,
                    "Falling back to configured/default tick after Saxo instrument details lookup failed: {err:#}"
                );
                default_tick(symbol)
            }
        }
    };
    Ok(normalize_order_price_with_tick(price, tick, action, role))
}

fn normalize_order_price_with_tick(price: f64, tick: f64, action: &str, role: &str) -> f64 {
    if tick <= 0.0 {
        return price;
    }
    let units = price / tick;
    let rounded = match (action, role) {
        ("BUY", "limit") | ("SELL", "stop") | ("SELL", "stop_limit") => units.floor(),
        ("SELL", "limit") | ("BUY", "stop") => units.ceil(),
        _ => units.round(),
    };
    let decimals = tick_decimals(tick);
    round_to_decimals(rounded * tick, decimals)
}

async fn instrument_tick_size(
    state: &AppState,
    session: &JsonValue,
    instrument: &SaxoInstrument,
    price: f64,
) -> Result<Option<f64>> {
    let details_path = format!(
        "/ref/v1/instruments/details/{}/{}",
        instrument.uic,
        percent_encode_path_segment(&instrument.asset_type)
    );
    let details = saxo_get_json_optional(
        state,
        session,
        &details_path,
        &[("AccountKey", account_key(state, session)?)],
        "Saxo instrument details lookup",
    )
    .await?;
    Ok(details
        .as_ref()
        .and_then(|details| tick_from_instrument_details(price, details)))
}

fn configured_price_tick_override(config: &serde_yaml::Value, symbol: &str) -> Option<f64> {
    let parts = symbol_parts(symbol);
    let overrides = yaml_at(config, &["execution", "price_tick_overrides"])?;
    let symbol_keys = [
        symbol.to_string(),
        symbol.to_uppercase(),
        symbol.to_lowercase(),
    ];
    for key in symbol_keys {
        if let Some(tick) = yaml_number_at_key(overrides, &key) {
            return Some(tick);
        }
    }
    let exchange_id = exchange_id_for_suffix(&parts.exchange);
    let exchange_keys = [
        parts.exchange.clone(),
        parts.exchange.to_uppercase(),
        exchange_id.to_string(),
    ];
    for key in exchange_keys {
        if let Some(tick) = yaml_number_at_key(overrides, &key) {
            return Some(tick);
        }
    }
    None
}

fn yaml_number_at_key(value: &serde_yaml::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
    })
}

fn tick_from_instrument_details(price: f64, details: &JsonValue) -> Option<f64> {
    numeric_from_keys(
        details,
        &["TickSize", "tickSize", "PriceTickSize", "priceTickSize"],
    )
    .or_else(|| tick_from_scheme_keys(price, details))
    .or_else(|| {
        details.get("DisplayAndFormat").and_then(|display| {
            numeric_from_keys(
                display,
                &["TickSize", "tickSize", "PriceTickSize", "priceTickSize"],
            )
            .or_else(|| tick_from_scheme_keys(price, display))
        })
    })
}

fn tick_from_scheme_keys(price: f64, value: &JsonValue) -> Option<f64> {
    [
        "TickSizeScheme",
        "tickSizeScheme",
        "PriceTickSizeScheme",
        "priceTickSizeScheme",
    ]
    .iter()
    .find_map(|key| {
        value
            .get(*key)
            .and_then(|scheme| tick_from_scheme(price, scheme))
    })
}

fn tick_from_scheme(price: f64, scheme: &JsonValue) -> Option<f64> {
    let mut elements = scheme
        .get("Elements")
        .or_else(|| scheme.get("elements"))
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let tick = numeric_from_keys(item, &["TickSize", "tickSize", "Size", "size"])?;
                    let high = numeric_from_keys(
                        item,
                        &[
                            "HighPrice",
                            "highPrice",
                            "UpperBound",
                            "upperBound",
                            "Price",
                            "price",
                        ],
                    )?;
                    Some((high, tick))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    elements.sort_by(|left, right| left.0.total_cmp(&right.0));
    for (high, tick) in elements {
        if price <= high + 1e-9 {
            return Some(tick);
        }
    }
    numeric_from_keys(
        scheme,
        &["DefaultTickSize", "defaultTickSize", "TickSize", "tickSize"],
    )
}

fn numeric_from_keys(value: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| json_number(value, key).filter(|tick| tick.is_finite() && *tick > 0.0))
}

fn default_tick(symbol: &str) -> f64 {
    match symbol_parts(symbol).exchange.as_str() {
        "xnas" | "xnys" | "xhel" | "xmil" | "xpar" | "xams" | "xbru" | "xlse" => 0.01,
        "xcse" | "xetr" | "xfra" => 0.05,
        "xsto" | "xosl" => 0.10,
        _ => 0.01,
    }
}

fn tick_decimals(tick: f64) -> i32 {
    let text = format!("{tick:.10}");
    text.trim_end_matches('0')
        .split('.')
        .nth(1)
        .map(|value| value.len() as i32)
        .unwrap_or(0)
}

fn round_to_decimals(value: f64, decimals: i32) -> f64 {
    let factor = 10_f64.powi(decimals);
    (value * factor).round() / factor
}

fn external_reference(order_id: i64) -> String {
    format!("saxo-daytrader:{order_id}")
        .chars()
        .take(50)
        .collect()
}

fn broker_order_id(payload: &JsonValue) -> Option<String> {
    ["OrderId", "OrderID", "order_id"].iter().find_map(|key| {
        payload.get(*key).and_then(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .or_else(|| value.as_i64().map(|value| value.to_string()))
        })
    })
}

fn optional_f64(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
}

fn order_text(value: &JsonValue, key: &str) -> String {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn session_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn nested_json_text(value: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_number(value: &JsonValue, key: &str) -> Option<f64> {
    value.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
    })
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn broker_event_signature(
    order_id: i64,
    event_type: &str,
    broker_order_id: &str,
    broker_status: Option<&str>,
    broker_substatus: Option<&str>,
    broker_quantity: Option<f64>,
    broker_price_local: Option<f64>,
) -> String {
    format!(
        "{}:{}:{}:{}:{}:{:.8}:{:.8}",
        event_type,
        order_id,
        broker_order_id,
        broker_status.unwrap_or(""),
        broker_substatus.unwrap_or(""),
        broker_quantity.unwrap_or(0.0),
        broker_price_local.unwrap_or(0.0)
    )
}

fn commission_dkk_for_fill(order: &JsonValue, gross_amount_dkk: f64, currency: &str) -> f64 {
    if let Some(commission) = order
        .get("execution_result_json")
        .and_then(|value| value.get("precheck"))
        .and_then(|value| value.get("Cost"))
        .and_then(|value| value.get("Commission"))
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|value| value as f64))
        })
        .filter(|value| *value >= 0.0)
    {
        if currency.eq_ignore_ascii_case("DKK") {
            return commission;
        }
    }
    let rate_commission = gross_amount_dkk.abs() * 0.0008;
    rate_commission.max(default_min_commission_dkk(&order_text(order, "symbol")))
}

fn default_min_commission_dkk(symbol: &str) -> f64 {
    min_commission_dkk_for_exchange(&symbol_parts(symbol).exchange)
}

/// Minimum broker commission in DKK for one order on an exchange. Used both
/// for fill cost booking and for the commission-efficiency floor on BUYs.
pub(crate) fn min_commission_dkk_for_exchange(exchange: &str) -> f64 {
    match exchange.trim().to_lowercase().as_str() {
        "xnas" | "xnys" => 3.0 * fx_rate_to_dkk("USD"),
        "xlon" => 8.0 * fx_rate_to_dkk("GBP"),
        "xsto" => 69.0 * fx_rate_to_dkk("SEK"),
        "xosl" => 39.0 * fx_rate_to_dkk("NOK"),
        "xhel" | "xetr" | "xfra" | "xmil" | "xpar" | "xams" | "xbru" | "xlse" => {
            3.0 * fx_rate_to_dkk("EUR")
        }
        _ => 14.0,
    }
}

pub(crate) fn fx_rate_to_dkk(currency: &str) -> f64 {
    crate::fx::static_fx_rate_to_dkk(currency)
}

/// Trading currency implied by the exchange suffix of a symbol like
/// "AMD:xnas". Deterministic for the exchanges this system trades; used to
/// verify order values and as the fallback when no broker currency is known.
pub(crate) fn currency_for_exchange(exchange: &str) -> Option<&'static str> {
    match exchange.trim().to_lowercase().as_str() {
        "xnas" | "xnys" => Some("USD"),
        "xcse" => Some("DKK"),
        "xetr" | "xfra" | "xmil" | "xams" | "xpar" | "xbru" | "xhel" => Some("EUR"),
        "xlon" | "xlse" => Some("GBP"),
        "xsto" => Some("SEK"),
        "xosl" => Some("NOK"),
        "xwar" => Some("PLN"),
        _ => None,
    }
}

fn prorated(total: f64, quantity: f64, total_quantity: f64) -> f64 {
    if total_quantity.abs() < f64::EPSILON {
        0.0
    } else {
        total * (quantity / total_quantity)
    }
}

fn percent_encode_path_segment(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn compact_number(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 1e-9 {
        format!("{}", rounded as i64)
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn sanitized_order_payload(payload: &JsonValue) -> JsonValue {
    let mut sanitized = payload.clone();
    if let Some(object) = sanitized.as_object_mut() {
        object.remove("AccountKey");
    }
    sanitized
}

fn sanitized_broker_position(position: &BrokerPosition) -> JsonValue {
    json!({
        "quantity": position.quantity,
        "can_be_closed": position.can_be_closed,
        "instrument": position.instrument.as_ref().map(|instrument| json!({
            "uic": instrument.uic,
            "asset_type": instrument.asset_type,
            "exchange_id": instrument.exchange_id,
            "description": instrument.description
        }))
    })
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Row, any::AnyPoolOptions};
    use std::{path::PathBuf, sync::Once};

    async fn execution_order_test_state() -> AppState {
        static INSTALL_DRIVERS: Once = Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);

        // A single connection keeps SQLite's in-memory database stable while the
        // concurrent callers still exercise the conditional claim transition.
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory execution-order test database");
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                status TEXT NOT NULL,
                error_text TEXT,
                broker_order_id TEXT,
                execution_result_json TEXT,
                currency TEXT,
                action TEXT,
                symbol TEXT,
                mode TEXT,
                quantity REAL,
                price_local REAL,
                ledger_id INTEGER
            )",
        )
        .execute(&pool)
        .await
        .expect("create execution-order test table");
        sqlx::query(
            "CREATE TABLE execution_order_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                execution_order_id INTEGER NOT NULL,
                broker_order_id TEXT,
                event_type TEXT NOT NULL,
                broker_status TEXT,
                broker_substatus TEXT,
                broker_quantity REAL,
                broker_price_local REAL,
                event_signature TEXT NOT NULL UNIQUE,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create execution-order event test table");
        sqlx::query(
            "CREATE TABLE execution_fills (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                execution_order_id INTEGER NOT NULL,
                broker_order_id TEXT,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                fill_status TEXT NOT NULL,
                cumulative_quantity REAL NOT NULL,
                delta_quantity REAL NOT NULL,
                average_price_local REAL NOT NULL,
                currency TEXT NOT NULL,
                ledger_id INTEGER,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create execution-fill test table");
        sqlx::query(
            "CREATE TABLE trade_ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                isin TEXT,
                figi TEXT,
                instrument_name TEXT,
                side TEXT NOT NULL,
                quantity REAL NOT NULL,
                price_local REAL NOT NULL,
                currency TEXT NOT NULL,
                gross_amount_dkk REAL NOT NULL,
                commission_dkk REAL NOT NULL,
                commission_local REAL NOT NULL,
                fx_conversion_dkk REAL NOT NULL,
                tax_dkk REAL NOT NULL,
                realised_gain_dkk REAL NOT NULL,
                cost_basis_sold_dkk REAL NOT NULL,
                cost_basis_sold_local REAL NOT NULL,
                realised_gain_local REAL NOT NULL,
                fx_gain_dkk REAL NOT NULL,
                price_gain_dkk REAL NOT NULL,
                sale_fx_rate_to_dkk REAL NOT NULL,
                cost_basis_fx_rate_to_dkk REAL NOT NULL,
                net_amount_dkk REAL NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                notes TEXT,
                portfolio_before_json TEXT,
                portfolio_after_json TEXT,
                decision_context_json TEXT,
                tax_year INTEGER,
                batch_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create trade-ledger test table");
        sqlx::query(
            "CREATE TABLE position_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                batch_id TEXT,
                imported_at TEXT NOT NULL,
                instrument_name TEXT NOT NULL,
                symbol TEXT NOT NULL,
                isin TEXT,
                quantity REAL NOT NULL,
                currency TEXT NOT NULL,
                open_price_local REAL,
                current_price_local REAL,
                cost_basis_local REAL,
                cost_basis_dkk REAL NOT NULL,
                market_value_local REAL,
                market_value_dkk REAL,
                unrealised_pnl_dkk REAL,
                daily_pnl_dkk REAL,
                allocation_pct REAL,
                status TEXT,
                account_name TEXT,
                asset_class TEXT,
                market_status TEXT,
                value_date TEXT,
                source_csv TEXT,
                excluded INTEGER NOT NULL DEFAULT 0,
                exclusion_reason TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create position-snapshot test table");
        sqlx::query(
            "CREATE TABLE position_lots (
                lot_id TEXT PRIMARY KEY,
                batch_id TEXT,
                created_at TEXT NOT NULL,
                acquired_at TEXT,
                symbol TEXT NOT NULL,
                isin TEXT,
                figi TEXT,
                instrument_name TEXT,
                quantity_original REAL NOT NULL,
                currency TEXT NOT NULL,
                cost_basis_total_local REAL,
                cost_basis_total_dkk REAL NOT NULL,
                fx_rate_to_dkk REAL NOT NULL,
                source_type TEXT NOT NULL,
                source_reference TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create position-lot test table");
        sqlx::query(
            "CREATE TABLE import_batches (
                batch_id TEXT PRIMARY KEY,
                imported_at TEXT NOT NULL,
                source_csv TEXT,
                source_position_count INTEGER,
                imported_position_count INTEGER,
                excluded_position_count INTEGER,
                notes TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create import-batch test table");
        sqlx::query(
            "CREATE TABLE currency_fx_rates (
                currency_code TEXT NOT NULL,
                base_currency TEXT NOT NULL,
                rate_to_dkk REAL NOT NULL,
                expires_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create FX-rate test table");
        sqlx::query(
            "CREATE TABLE broker_position_snapshots (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                quantity REAL NOT NULL,
                open_price_local REAL,
                open_price_including_costs_local REAL,
                currency TEXT,
                isin TEXT,
                instrument_name TEXT,
                can_be_closed INTEGER
            )",
        )
        .execute(&pool)
        .await
        .expect("create broker-position test table");

        AppState {
            config_path: PathBuf::from("saxo-order-claim-test.yaml"),
            config: serde_yaml::from_str("execution:\n  mode: live\n  adapter: saxo\n")
                .expect("parse claim test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[test]
    fn execution_queue_gate_fails_closed_until_live_saxo_is_explicitly_ungated() {
        assert_eq!(
            execution_queue_gate("simulation", "saxo", false, false),
            Some(ExecutionQueueGate::NotLiveSaxo)
        );
        assert_eq!(
            execution_queue_gate("live", "simulation", false, false),
            Some(ExecutionQueueGate::NotLiveSaxo)
        );
        assert_eq!(
            execution_queue_gate("live", "saxo", true, false),
            Some(ExecutionQueueGate::DryRun)
        );
        assert_eq!(
            execution_queue_gate("live", "saxo", false, true),
            Some(ExecutionQueueGate::ApprovalRequired)
        );
        assert_eq!(execution_queue_gate("live", "saxo", false, false), None);
    }

    #[tokio::test]
    async fn concurrent_order_claims_have_exactly_one_database_winner() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (status, error_text, broker_order_id)
             VALUES ('pending_execution', 'stale local error', NULL)",
        )
        .execute(&state.pool)
        .await
        .expect("seed claimable execution order");
        let order_id = sqlx::query("SELECT id FROM execution_orders LIMIT 1")
            .fetch_one(&state.pool)
            .await
            .expect("read seeded execution-order id")
            .try_get::<i64, _>("id")
            .expect("execution-order id");

        let (first, second) = tokio::join!(
            claim_order_for_submission(&state, order_id),
            claim_order_for_submission(&state, order_id),
        );
        let claims = [
            first.expect("first claim query"),
            second.expect("second claim query"),
        ];

        assert_eq!(claims.iter().filter(|claimed| **claimed).count(), 1);
        let stored = sqlx::query(
            "SELECT status, error_text, broker_order_id FROM execution_orders WHERE id = ?",
        )
        .bind(order_id)
        .fetch_one(&state.pool)
        .await
        .expect("read claimed execution order");
        assert_eq!(
            stored.try_get::<String, _>("status").unwrap(),
            "submitting_to_broker"
        );
        assert!(
            stored
                .try_get::<Option<String>, _>("error_text")
                .unwrap()
                .is_none()
        );
        assert!(
            stored
                .try_get::<Option<String>, _>("broker_order_id")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn terminal_broker_sync_persists_expiry_status_and_event_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, error_text, broker_order_id, execution_result_json, currency
            ) VALUES (1, 'broker_working', NULL, '5039132483', '{\"precheck\":true}', 'EUR')",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working execution order");
        let order = json!({
            "id": 1,
            "symbol": "ADS:xetr",
            "status": "broker_working",
            "currency": "EUR",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "Expired",
                "SubStatus": "DoneForDay"
            }
        });

        // These are the two local persistence calls made after a broker response
        // has already been received. The fixture deliberately does not fetch Saxo.
        record_broker_order_event(
            &state,
            &order,
            "5039132483",
            "broker_status_sync",
            Some("Expired"),
            Some("DoneForDay"),
            Some(3.0),
            Some(245.5),
            &broker_state,
        )
        .await
        .expect("record terminal broker event");
        update_order_broker_status(
            &state,
            &order,
            local_terminal_status(&Some("Expired".to_string())),
            &broker_state,
            None,
        )
        .await
        .expect("persist terminal broker status");

        let stored = sqlx::query(
            "SELECT status, error_text, execution_result_json
             FROM execution_orders WHERE id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read terminal execution order");
        assert_eq!(
            stored.try_get::<String, _>("status").unwrap(),
            "broker_expired"
        );
        assert_eq!(
            stored.try_get::<Option<String>, _>("error_text").unwrap(),
            Some("Expired".to_string())
        );
        let result: JsonValue = serde_json::from_str(
            &stored
                .try_get::<String, _>("execution_result_json")
                .expect("broker-sync result"),
        )
        .expect("parse stored broker-sync result");
        assert_eq!(result["precheck"], json!(true));
        assert_eq!(
            result["broker_sync"]["broker_payload"]["Status"],
            json!("Expired")
        );

        let event = sqlx::query(
            "SELECT event_type, broker_order_id, broker_status, broker_substatus,
                    broker_quantity, broker_price_local
             FROM execution_order_events WHERE execution_order_id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read terminal broker event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "broker_status_sync"
        );
        assert_eq!(
            event.try_get::<String, _>("broker_order_id").unwrap(),
            "5039132483"
        );
        assert_eq!(
            event.try_get::<String, _>("broker_status").unwrap(),
            "Expired"
        );
        assert_eq!(
            event.try_get::<String, _>("broker_substatus").unwrap(),
            "DoneForDay"
        );
        assert_eq!(event.try_get::<f64, _>("broker_quantity").unwrap(), 3.0);
        assert_eq!(
            event.try_get::<f64, _>("broker_price_local").unwrap(),
            245.5
        );
    }

    #[tokio::test]
    async fn broker_lookup_miss_preserves_local_status_and_audits_not_found_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, error_text, broker_order_id, execution_result_json, currency
            ) VALUES (1, 'broker_working', NULL, '5039132483', '{\"precheck\":true}', 'EUR')",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working execution order");
        let order = json!({
            "id": 1,
            "symbol": "ADS:xetr",
            "status": "broker_working",
            "currency": "EUR",
            "execution_result_json": {"precheck": true}
        });
        let missing_state = broker_order_lookup_miss_state("5039132483");

        // A missing open-order and activity lookup is not proof of expiry or fill.
        // Preserve the local status and retain the broker-visibility evidence.
        record_broker_order_event(
            &state,
            &order,
            "5039132483",
            "broker_sync_not_found",
            Some("NotFound"),
            None,
            None,
            None,
            &missing_state,
        )
        .await
        .expect("record missing broker lookup event");
        update_order_broker_status(&state, &order, "broker_working", &missing_state, None)
            .await
            .expect("persist missing broker lookup state");

        let stored = sqlx::query(
            "SELECT status, error_text, execution_result_json
             FROM execution_orders WHERE id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read unchanged execution order");
        assert_eq!(
            stored.try_get::<String, _>("status").unwrap(),
            "broker_working"
        );
        assert!(
            stored
                .try_get::<Option<String>, _>("error_text")
                .unwrap()
                .is_none()
        );
        let result: JsonValue = serde_json::from_str(
            &stored
                .try_get::<String, _>("execution_result_json")
                .expect("broker-sync result"),
        )
        .expect("parse stored broker-sync result");
        assert_eq!(result["precheck"], json!(true));
        assert_eq!(
            result["broker_sync"]["broker_visibility"],
            json!("not_found")
        );
        assert_eq!(result["broker_sync"]["activity_lookup"], json!("not_found"));

        let event = sqlx::query(
            "SELECT event_type, broker_order_id, broker_status, broker_substatus,
                    broker_quantity, broker_price_local, raw_payload_json
             FROM execution_order_events WHERE execution_order_id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read missing broker lookup event");
        assert_eq!(
            event.try_get::<String, _>("event_type").unwrap(),
            "broker_sync_not_found"
        );
        assert_eq!(
            event.try_get::<String, _>("broker_order_id").unwrap(),
            "5039132483"
        );
        assert_eq!(
            event.try_get::<String, _>("broker_status").unwrap(),
            "NotFound"
        );
        assert!(
            event
                .try_get::<Option<String>, _>("broker_substatus")
                .unwrap()
                .is_none()
        );
        assert!(
            event
                .try_get::<Option<f64>, _>("broker_quantity")
                .unwrap()
                .is_none()
        );
        assert!(
            event
                .try_get::<Option<f64>, _>("broker_price_local")
                .unwrap()
                .is_none()
        );
        let event_payload: JsonValue = serde_json::from_str(
            &event
                .try_get::<String, _>("raw_payload_json")
                .expect("missing lookup payload"),
        )
        .expect("parse missing lookup payload");
        assert_eq!(event_payload["broker_visibility"], json!("not_found"));
    }

    #[tokio::test]
    async fn final_fill_reconciliation_persists_one_ledger_and_fill_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132483', '{\"precheck\":true}', 'USD',
                'BUY', 'AMD:xnas', 'live', 2, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working execution order");
        let order = json!({
            "id": 1,
            "symbol": "AMD:xnas",
            "action": "BUY",
            "mode": "live",
            "status": "broker_working",
            "quantity": 2.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 2.0,
                "ExecutionPrice": 101.25,
                "Currency": "USD"
            }
        });

        // This begins after a final Saxo fill payload has been received. It
        // exercises only the local reconciliation and never loads a session.
        record_broker_order_event(
            &state,
            &order,
            "5039132483",
            "broker_final_fill",
            Some("FinalFill"),
            Some("Confirmed"),
            Some(2.0),
            Some(101.25),
            &broker_state,
        )
        .await
        .expect("record final-fill broker event");
        let first = sync_final_fill(&state, &order, "5039132483", 2.0, 101.25, &broker_state)
            .await
            .expect("reconcile final fill");
        let ledger_id = first["ledger_id"].as_i64().expect("ledger id");
        assert_eq!(first["status"], json!("executed"));
        assert_eq!(first["fills"], json!(1));
        assert_eq!(first["delta_quantity"], json!(2.0));

        let stored_order = sqlx::query(
            "SELECT status, error_text, price_local, currency, ledger_id, execution_result_json
             FROM execution_orders WHERE id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read reconciled execution order");
        assert_eq!(
            stored_order.try_get::<String, _>("status").unwrap(),
            "executed"
        );
        assert!(
            stored_order
                .try_get::<Option<String>, _>("error_text")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            stored_order.try_get::<f64, _>("price_local").unwrap(),
            101.25
        );
        assert_eq!(
            stored_order.try_get::<String, _>("currency").unwrap(),
            "USD"
        );
        assert_eq!(
            stored_order.try_get::<i64, _>("ledger_id").unwrap(),
            ledger_id
        );
        let result: JsonValue = serde_json::from_str(
            &stored_order
                .try_get::<String, _>("execution_result_json")
                .expect("broker-sync result"),
        )
        .expect("parse stored broker-sync result");
        assert_eq!(result["precheck"], json!(true));
        assert_eq!(
            result["broker_sync"]["broker_payload"]["Status"],
            json!("FinalFill")
        );

        let fill = sqlx::query(
            "SELECT broker_order_id, symbol, side, fill_status, cumulative_quantity,
                    delta_quantity, average_price_local, currency, ledger_id
             FROM execution_fills WHERE execution_order_id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read final-fill record");
        assert_eq!(
            fill.try_get::<String, _>("broker_order_id").unwrap(),
            "5039132483"
        );
        assert_eq!(fill.try_get::<String, _>("symbol").unwrap(), "AMD:xnas");
        assert_eq!(fill.try_get::<String, _>("side").unwrap(), "BUY");
        assert_eq!(
            fill.try_get::<String, _>("fill_status").unwrap(),
            "FinalFill"
        );
        assert_eq!(fill.try_get::<f64, _>("cumulative_quantity").unwrap(), 2.0);
        assert_eq!(fill.try_get::<f64, _>("delta_quantity").unwrap(), 2.0);
        assert_eq!(
            fill.try_get::<f64, _>("average_price_local").unwrap(),
            101.25
        );
        assert_eq!(fill.try_get::<String, _>("currency").unwrap(), "USD");
        assert_eq!(fill.try_get::<i64, _>("ledger_id").unwrap(), ledger_id);

        let ledger = sqlx::query(
            "SELECT symbol, side, quantity, price_local, currency, mode, status,
                    decision_context_json
             FROM trade_ledger WHERE id = ?",
        )
        .bind(ledger_id)
        .fetch_one(&state.pool)
        .await
        .expect("read reconciled ledger row");
        assert_eq!(ledger.try_get::<String, _>("symbol").unwrap(), "AMD:xnas");
        assert_eq!(ledger.try_get::<String, _>("side").unwrap(), "BUY");
        assert_eq!(ledger.try_get::<f64, _>("quantity").unwrap(), 2.0);
        assert_eq!(ledger.try_get::<f64, _>("price_local").unwrap(), 101.25);
        assert_eq!(ledger.try_get::<String, _>("currency").unwrap(), "USD");
        assert_eq!(ledger.try_get::<String, _>("mode").unwrap(), "live");
        assert_eq!(ledger.try_get::<String, _>("status").unwrap(), "executed");
        let ledger_context: JsonValue = serde_json::from_str(
            &ledger
                .try_get::<String, _>("decision_context_json")
                .expect("ledger context"),
        )
        .expect("parse ledger context");
        assert_eq!(
            ledger_context["broker_sync"]["broker_payload"]["Status"],
            json!("FinalFill")
        );

        // Replaying the same cumulative broker fill must not duplicate money
        // movement, the execution-fill row, or the trade-ledger row.
        let replay = sync_final_fill(&state, &order, "5039132483", 2.0, 101.25, &broker_state)
            .await
            .expect("reconcile duplicate final fill");
        assert_eq!(replay["fills"], json!(0));
        assert_eq!(replay["ledger_id"], json!(ledger_id));
        let fill_count = sqlx::query("SELECT COUNT(*) AS count FROM execution_fills")
            .fetch_one(&state.pool)
            .await
            .expect("count execution fills")
            .try_get::<i64, _>("count")
            .expect("execution fill count");
        let ledger_count = sqlx::query("SELECT COUNT(*) AS count FROM trade_ledger")
            .fetch_one(&state.pool)
            .await
            .expect("count ledger rows")
            .try_get::<i64, _>("count")
            .expect("trade ledger count");
        assert_eq!(fill_count, 1);
        assert_eq!(ledger_count, 1);
    }

    #[tokio::test]
    async fn partial_fill_delta_is_reconciled_once_when_later_final_fill_arrives_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132483', '{\"precheck\":true}', 'USD',
                'BUY', 'AMD:xnas', 'live', 4, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed partially-filled execution order");
        let order = json!({
            "id": 1,
            "symbol": "AMD:xnas",
            "action": "BUY",
            "mode": "live",
            "status": "broker_working",
            "quantity": 4.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let partial_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "PartiallyFilled",
                "FilledAmount": 1.0,
                "ExecutionPrice": 100.0,
                "Currency": "USD"
            }
        });

        // Model a one-share fill already persisted by an earlier reconciliation
        // cycle. This test starts after broker responses exist and makes no HTTP.
        record_broker_order_event(
            &state,
            &order,
            "5039132483",
            "broker_status_sync",
            Some("PartiallyFilled"),
            None,
            Some(1.0),
            Some(100.0),
            &partial_state,
        )
        .await
        .expect("record partial-fill broker event");
        update_order_broker_status(
            &state,
            &order,
            "broker_partially_filled",
            &partial_state,
            None,
        )
        .await
        .expect("persist partial-fill broker status");
        let partial_ledger_id =
            insert_trade_ledger_for_fill(&state, &order, "BUY", 1.0, 100.0, "USD", &partial_state)
                .await
                .expect("record prior partial-fill ledger row");
        insert_execution_fill(
            &state,
            &order,
            "5039132483",
            "PartialFill",
            1.0,
            1.0,
            100.0,
            "USD",
            Some(partial_ledger_id),
            &partial_state,
        )
        .await
        .expect("record prior partial-fill row");

        let final_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 4.0,
                "ExecutionPrice": 101.25,
                "Currency": "USD"
            }
        });
        record_broker_order_event(
            &state,
            &order,
            "5039132483",
            "broker_final_fill",
            Some("FinalFill"),
            Some("Confirmed"),
            Some(4.0),
            Some(101.25),
            &final_state,
        )
        .await
        .expect("record final-fill broker event");
        let final_fill = sync_final_fill(&state, &order, "5039132483", 4.0, 101.25, &final_state)
            .await
            .expect("reconcile remaining final-fill quantity");
        let final_ledger_id = final_fill["ledger_id"].as_i64().expect("final ledger id");
        assert_ne!(final_ledger_id, partial_ledger_id);
        assert_eq!(final_fill["status"], json!("executed"));
        assert_eq!(final_fill["fills"], json!(1));
        assert_eq!(final_fill["synced_before"], json!(1.0));
        assert_eq!(final_fill["delta_quantity"], json!(3.0));

        let stored_order = sqlx::query(
            "SELECT status, price_local, ledger_id, execution_result_json
             FROM execution_orders WHERE id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read final execution order");
        assert_eq!(
            stored_order.try_get::<String, _>("status").unwrap(),
            "executed"
        );
        assert_eq!(
            stored_order.try_get::<f64, _>("price_local").unwrap(),
            101.25
        );
        assert_eq!(
            stored_order.try_get::<i64, _>("ledger_id").unwrap(),
            final_ledger_id
        );
        let result: JsonValue = serde_json::from_str(
            &stored_order
                .try_get::<String, _>("execution_result_json")
                .expect("broker-sync result"),
        )
        .expect("parse stored broker-sync result");
        assert_eq!(result["precheck"], json!(true));
        assert_eq!(
            result["broker_sync"]["broker_payload"]["Status"],
            json!("FinalFill")
        );

        let fills = sqlx::query(
            "SELECT fill_status, cumulative_quantity, delta_quantity, average_price_local,
                    ledger_id
             FROM execution_fills WHERE execution_order_id = 1 ORDER BY id ASC",
        )
        .fetch_all(&state.pool)
        .await
        .expect("read partial and final fill rows");
        assert_eq!(fills.len(), 2);
        assert_eq!(
            fills[0].try_get::<String, _>("fill_status").unwrap(),
            "PartialFill"
        );
        assert_eq!(
            fills[0].try_get::<f64, _>("cumulative_quantity").unwrap(),
            1.0
        );
        assert_eq!(fills[0].try_get::<f64, _>("delta_quantity").unwrap(), 1.0);
        assert_eq!(
            fills[0].try_get::<i64, _>("ledger_id").unwrap(),
            partial_ledger_id
        );
        assert_eq!(
            fills[1].try_get::<String, _>("fill_status").unwrap(),
            "FinalFill"
        );
        assert_eq!(
            fills[1].try_get::<f64, _>("cumulative_quantity").unwrap(),
            4.0
        );
        assert_eq!(fills[1].try_get::<f64, _>("delta_quantity").unwrap(), 3.0);
        assert_eq!(
            fills[1].try_get::<f64, _>("average_price_local").unwrap(),
            101.25
        );
        assert_eq!(
            fills[1].try_get::<i64, _>("ledger_id").unwrap(),
            final_ledger_id
        );

        let ledger_rows = sqlx::query(
            "SELECT quantity, price_local FROM trade_ledger WHERE symbol = 'AMD:xnas'
             ORDER BY id ASC",
        )
        .fetch_all(&state.pool)
        .await
        .expect("read partial and final ledger rows");
        assert_eq!(ledger_rows.len(), 2);
        assert_eq!(ledger_rows[0].try_get::<f64, _>("quantity").unwrap(), 1.0);
        assert_eq!(
            ledger_rows[0].try_get::<f64, _>("price_local").unwrap(),
            100.0
        );
        assert_eq!(ledger_rows[1].try_get::<f64, _>("quantity").unwrap(), 3.0);
        assert_eq!(
            ledger_rows[1].try_get::<f64, _>("price_local").unwrap(),
            101.25
        );

        // A replay of the final cumulative quantity cannot produce a third
        // accounting record after the partial and final rows are present.
        let replay = sync_final_fill(&state, &order, "5039132483", 4.0, 101.25, &final_state)
            .await
            .expect("reconcile duplicate final fill");
        assert_eq!(replay["fills"], json!(0));
        assert_eq!(replay["delta_quantity"], json!(0.0));
        assert_eq!(replay["ledger_id"], json!(final_ledger_id));
        let fill_count = sqlx::query("SELECT COUNT(*) AS count FROM execution_fills")
            .fetch_one(&state.pool)
            .await
            .expect("count execution fills")
            .try_get::<i64, _>("count")
            .expect("execution fill count");
        let ledger_count = sqlx::query("SELECT COUNT(*) AS count FROM trade_ledger")
            .fetch_one(&state.pool)
            .await
            .expect("count ledger rows")
            .try_get::<i64, _>("count")
            .expect("trade ledger count");
        assert_eq!(fill_count, 2);
        assert_eq!(ledger_count, 2);
    }

    #[tokio::test]
    async fn sell_final_fill_uses_latest_position_basis_and_is_idempotent_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO position_snapshots (
                imported_at, instrument_name, symbol, isin, quantity, currency,
                cost_basis_local, cost_basis_dkk, excluded
            ) VALUES (
                '2026-07-15T00:00:00Z', 'AMD Inc.', 'AMD:xnas', 'US0079031078',
                10, 'USD', 800, 5617.2, 0
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed current position cost basis");
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132484', '{\"precheck\":true}', 'USD',
                'SELL', 'AMD:xnas', 'live', 4, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working sell order");
        let order = json!({
            "id": 1,
            "symbol": "AMD:xnas",
            "action": "SELL",
            "mode": "live",
            "status": "broker_working",
            "quantity": 4.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 4.0,
                "ExecutionPrice": 150.0,
                "Currency": "USD"
            }
        });

        // This starts after Saxo has returned the final fill. It exercises the
        // local accounting boundary only and never creates a Saxo HTTP request.
        record_broker_order_event(
            &state,
            &order,
            "5039132484",
            "broker_final_fill",
            Some("FinalFill"),
            Some("Confirmed"),
            Some(4.0),
            Some(150.0),
            &broker_state,
        )
        .await
        .expect("record final sell-fill broker event");
        let first = sync_final_fill(&state, &order, "5039132484", 4.0, 150.0, &broker_state)
            .await
            .expect("reconcile final sell fill");
        let ledger_id = first["ledger_id"].as_i64().expect("sell ledger id");
        assert_eq!(first["status"], json!("executed"));
        assert_eq!(first["fills"], json!(1));
        assert_eq!(first["delta_quantity"], json!(4.0));

        let ledger = sqlx::query(
            "SELECT symbol, isin, instrument_name, side, quantity, price_local, currency,
                    gross_amount_dkk, commission_dkk, commission_local, net_amount_dkk,
                    cost_basis_sold_dkk, cost_basis_sold_local, realised_gain_dkk,
                    realised_gain_local, mode, status
             FROM trade_ledger WHERE id = ?",
        )
        .bind(ledger_id)
        .fetch_one(&state.pool)
        .await
        .expect("read reconciled sell ledger row");
        assert_eq!(ledger.try_get::<String, _>("symbol").unwrap(), "AMD:xnas");
        assert_eq!(ledger.try_get::<String, _>("isin").unwrap(), "US0079031078");
        assert_eq!(
            ledger.try_get::<String, _>("instrument_name").unwrap(),
            "AMD Inc."
        );
        assert_eq!(ledger.try_get::<String, _>("side").unwrap(), "SELL");
        assert_eq!(ledger.try_get::<f64, _>("quantity").unwrap(), 4.0);
        assert_eq!(ledger.try_get::<f64, _>("price_local").unwrap(), 150.0);
        assert_eq!(ledger.try_get::<String, _>("currency").unwrap(), "USD");
        assert!((ledger.try_get::<f64, _>("gross_amount_dkk").unwrap() - 4212.9).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("commission_dkk").unwrap() - 21.0645).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("commission_local").unwrap() - 3.0).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("net_amount_dkk").unwrap() - 4191.8355).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("cost_basis_sold_dkk").unwrap() - 2246.88).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("cost_basis_sold_local").unwrap() - 320.0).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("realised_gain_dkk").unwrap() - 1944.9555).abs() < 1e-6);
        assert!((ledger.try_get::<f64, _>("realised_gain_local").unwrap() - 277.0).abs() < 1e-6);
        assert_eq!(ledger.try_get::<String, _>("mode").unwrap(), "live");
        assert_eq!(ledger.try_get::<String, _>("status").unwrap(), "executed");

        let fill = sqlx::query(
            "SELECT fill_status, cumulative_quantity, delta_quantity, average_price_local,
                    ledger_id
             FROM execution_fills WHERE execution_order_id = 1",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read final sell-fill record");
        assert_eq!(
            fill.try_get::<String, _>("fill_status").unwrap(),
            "FinalFill"
        );
        assert_eq!(fill.try_get::<f64, _>("cumulative_quantity").unwrap(), 4.0);
        assert_eq!(fill.try_get::<f64, _>("delta_quantity").unwrap(), 4.0);
        assert_eq!(
            fill.try_get::<f64, _>("average_price_local").unwrap(),
            150.0
        );
        assert_eq!(fill.try_get::<i64, _>("ledger_id").unwrap(), ledger_id);

        // The local book must shrink with the sale: 10 - 4 = 6 shares keep
        // 6/10 of the original basis.
        let snapshot = sqlx::query(
            "SELECT quantity, cost_basis_local, cost_basis_dkk
             FROM position_snapshots WHERE symbol = 'AMD:xnas'",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read decremented position snapshot");
        assert!((snapshot.try_get::<f64, _>("quantity").unwrap() - 6.0).abs() < 1e-9);
        assert!((snapshot.try_get::<f64, _>("cost_basis_local").unwrap() - 480.0).abs() < 1e-6);
        assert!((snapshot.try_get::<f64, _>("cost_basis_dkk").unwrap() - 3370.32).abs() < 1e-6);

        let replay = sync_final_fill(&state, &order, "5039132484", 4.0, 150.0, &broker_state)
            .await
            .expect("reconcile duplicate final sell fill");
        assert_eq!(replay["fills"], json!(0));
        assert_eq!(replay["delta_quantity"], json!(0.0));
        assert_eq!(replay["ledger_id"], json!(ledger_id));
        let replayed_snapshot =
            sqlx::query("SELECT quantity FROM position_snapshots WHERE symbol = 'AMD:xnas'")
                .fetch_one(&state.pool)
                .await
                .expect("read snapshot after replay");
        assert!((replayed_snapshot.try_get::<f64, _>("quantity").unwrap() - 6.0).abs() < 1e-9);
        let fill_count = sqlx::query("SELECT COUNT(*) AS count FROM execution_fills")
            .fetch_one(&state.pool)
            .await
            .expect("count execution fills")
            .try_get::<i64, _>("count")
            .expect("execution fill count");
        let ledger_count = sqlx::query("SELECT COUNT(*) AS count FROM trade_ledger")
            .fetch_one(&state.pool)
            .await
            .expect("count trade ledger rows")
            .try_get::<i64, _>("count")
            .expect("trade ledger count");
        assert_eq!(fill_count, 1);
        assert_eq!(ledger_count, 1);
    }

    #[tokio::test]
    async fn buy_final_fill_writes_local_snapshot_and_lot_without_http() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132485', '{\"precheck\":true}', 'USD',
                'BUY', 'AMD:xnas', 'live', 2, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working buy order");
        let order = json!({
            "id": 1,
            "symbol": "AMD:xnas",
            "action": "BUY",
            "mode": "live",
            "status": "broker_working",
            "quantity": 2.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 2.0,
                "ExecutionPrice": 101.25,
                "Currency": "USD"
            }
        });

        let first = sync_final_fill(&state, &order, "5039132485", 2.0, 101.25, &broker_state)
            .await
            .expect("reconcile final buy fill");
        assert_eq!(first["fills"], json!(1));
        let ledger_id = first["ledger_id"].as_i64().expect("buy ledger id");

        // The buy must open a local position: 2 x 101.25 USD plus the minimum
        // Nasdaq commission (3 USD = 21.0645 DKK at the static rate).
        let snapshot = sqlx::query(
            "SELECT quantity, currency, cost_basis_local, cost_basis_dkk, open_price_local
             FROM position_snapshots WHERE symbol = 'AMD:xnas' AND excluded = 0",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read created position snapshot");
        assert!((snapshot.try_get::<f64, _>("quantity").unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(snapshot.try_get::<String, _>("currency").unwrap(), "USD");
        assert!((snapshot.try_get::<f64, _>("cost_basis_local").unwrap() - 205.5).abs() < 1e-6);
        assert!((snapshot.try_get::<f64, _>("cost_basis_dkk").unwrap() - 1442.91825).abs() < 1e-6);
        assert!((snapshot.try_get::<f64, _>("open_price_local").unwrap() - 102.75).abs() < 1e-6);

        let lot = sqlx::query(
            "SELECT lot_id, quantity_original, cost_basis_total_local, cost_basis_total_dkk,
                    source_type, source_reference
             FROM position_lots WHERE symbol = 'AMD:xnas'",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read created position lot");
        assert_eq!(
            lot.try_get::<String, _>("lot_id").unwrap(),
            format!("buy-fill:1:{ledger_id}")
        );
        assert!((lot.try_get::<f64, _>("quantity_original").unwrap() - 2.0).abs() < 1e-9);
        assert!((lot.try_get::<f64, _>("cost_basis_total_local").unwrap() - 205.5).abs() < 1e-6);
        assert!((lot.try_get::<f64, _>("cost_basis_total_dkk").unwrap() - 1442.91825).abs() < 1e-6);
        assert_eq!(lot.try_get::<String, _>("source_type").unwrap(), "buy_fill");
        assert_eq!(
            lot.try_get::<String, _>("source_reference").unwrap(),
            "execution_order:1"
        );

        // A later SELL now finds a usable local basis, so realised gains are
        // measured against real cost instead of zero.
        let basis = latest_position_cost_basis(&state, "AMD:xnas")
            .await
            .expect("read basis after buy fill");
        assert!((basis.quantity - 2.0).abs() < 1e-9);
        assert!((basis.cost_basis_local - 205.5).abs() < 1e-6);

        // Replaying the same cumulative fill must not grow the book.
        let replay = sync_final_fill(&state, &order, "5039132485", 2.0, 101.25, &broker_state)
            .await
            .expect("reconcile duplicate buy fill");
        assert_eq!(replay["fills"], json!(0));
        let snapshot_count =
            sqlx::query("SELECT COUNT(*) AS count FROM position_snapshots WHERE excluded = 0")
                .fetch_one(&state.pool)
                .await
                .expect("count snapshots")
                .try_get::<i64, _>("count")
                .expect("snapshot count");
        let lot_count = sqlx::query("SELECT COUNT(*) AS count FROM position_lots")
            .fetch_one(&state.pool)
            .await
            .expect("count lots")
            .try_get::<i64, _>("count")
            .expect("lot count");
        assert_eq!(snapshot_count, 1);
        assert_eq!(lot_count, 1);
        let replayed = sqlx::query(
            "SELECT quantity FROM position_snapshots WHERE symbol = 'AMD:xnas' AND excluded = 0",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read snapshot after buy replay");
        assert!((replayed.try_get::<f64, _>("quantity").unwrap() - 2.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn buy_final_fill_tops_up_existing_snapshot_in_place() {
        let state = execution_order_test_state().await;
        sqlx::query(
            "INSERT INTO position_snapshots (
                imported_at, instrument_name, symbol, isin, quantity, currency,
                cost_basis_local, cost_basis_dkk, excluded
            ) VALUES (
                '2026-07-15T00:00:00Z', 'AMD Inc.', 'AMD:xnas', 'US0079031078',
                10, 'USD', 800, 5617.2, 0
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed existing position snapshot");
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132486', '{\"precheck\":true}', 'USD',
                'BUY', 'AMD:xnas', 'live', 2, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working top-up buy order");
        let order = json!({
            "id": 1,
            "symbol": "AMD:xnas",
            "action": "BUY",
            "mode": "live",
            "status": "broker_working",
            "quantity": 2.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 2.0,
                "ExecutionPrice": 101.25,
                "Currency": "USD"
            }
        });

        sync_final_fill(&state, &order, "5039132486", 2.0, 101.25, &broker_state)
            .await
            .expect("reconcile top-up buy fill");

        // The existing snapshot row grows in place instead of forking a
        // second row: 10 + 2 shares, 800 + 205.5 USD basis.
        let snapshot_count =
            sqlx::query("SELECT COUNT(*) AS count FROM position_snapshots WHERE excluded = 0")
                .fetch_one(&state.pool)
                .await
                .expect("count snapshots")
                .try_get::<i64, _>("count")
                .expect("snapshot count");
        assert_eq!(snapshot_count, 1);
        let snapshot = sqlx::query(
            "SELECT quantity, cost_basis_local, cost_basis_dkk, open_price_local
             FROM position_snapshots WHERE symbol = 'AMD:xnas' AND excluded = 0",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read topped-up position snapshot");
        assert!((snapshot.try_get::<f64, _>("quantity").unwrap() - 12.0).abs() < 1e-9);
        assert!((snapshot.try_get::<f64, _>("cost_basis_local").unwrap() - 1005.5).abs() < 1e-6);
        assert!((snapshot.try_get::<f64, _>("cost_basis_dkk").unwrap() - 7060.11825).abs() < 1e-6);
        assert!(
            (snapshot.try_get::<f64, _>("open_price_local").unwrap() - 1005.5 / 12.0).abs() < 1e-6
        );
    }

    #[tokio::test]
    async fn sell_final_fill_falls_back_to_broker_basis_when_local_snapshot_is_missing() {
        let state = execution_order_test_state().await;
        // No position_snapshots row exists for this symbol: the position was
        // acquired outside the snapshot imports. Only the broker holds it.
        sqlx::query(
            "INSERT INTO broker_position_snapshots (
                symbol, updated_at, quantity, open_price_local,
                open_price_including_costs_local, currency, isin, instrument_name, can_be_closed
            ) VALUES (
                'ARM:xnas', '2026-07-16T18:31:00Z', 5, 370.684, 371.0,
                'USD', 'US0420682058', 'ARM Holdings plc ADR', 1
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-authoritative position");
        sqlx::query(
            "INSERT INTO execution_orders (
                id, status, broker_order_id, execution_result_json, currency,
                action, symbol, mode, quantity, price_local
            ) VALUES (
                1, 'broker_working', '5039132555', '{\"precheck\":true}', 'USD',
                'SELL', 'ARM:xnas', 'live', 5, NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-working sell order");
        let order = json!({
            "id": 1,
            "symbol": "ARM:xnas",
            "action": "SELL",
            "mode": "live",
            "status": "broker_working",
            "quantity": 5.0,
            "currency": "USD",
            "execution_result_json": {"precheck": true}
        });
        let broker_state = json!({
            "source": "test_fixture",
            "broker_payload": {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 5.0,
                "ExecutionPrice": 277.01,
                "Currency": "USD"
            }
        });

        let result = sync_final_fill(&state, &order, "5039132555", 5.0, 277.01, &broker_state)
            .await
            .expect("reconcile final sell fill without local snapshot");
        let ledger_id = result["ledger_id"].as_i64().expect("sell ledger id");

        let ledger = sqlx::query(
            "SELECT isin, instrument_name, cost_basis_sold_dkk, cost_basis_sold_local,
                    realised_gain_dkk, net_amount_dkk
             FROM trade_ledger WHERE id = ?",
        )
        .bind(ledger_id)
        .fetch_one(&state.pool)
        .await
        .expect("read reconciled sell ledger row");
        // Basis comes from the broker open price including costs: 371.0 * 5
        // shares in USD, converted with the static 7.0215 fallback rate.
        let expected_basis_local = 371.0 * 5.0;
        let expected_basis_dkk = expected_basis_local * 7.0215;
        assert!(
            (ledger.try_get::<f64, _>("cost_basis_sold_local").unwrap() - expected_basis_local)
                .abs()
                < 1e-6
        );
        assert!(
            (ledger.try_get::<f64, _>("cost_basis_sold_dkk").unwrap() - expected_basis_dkk).abs()
                < 1e-6
        );
        // The realised loss must reflect the real basis instead of booking the
        // full sale proceeds as gain against a zero basis.
        let net = ledger.try_get::<f64, _>("net_amount_dkk").unwrap();
        let realised = ledger.try_get::<f64, _>("realised_gain_dkk").unwrap();
        assert!((realised - (net - expected_basis_dkk)).abs() < 1e-6);
        assert!(realised < 0.0, "position sold under water must book a loss");
        assert_eq!(ledger.try_get::<String, _>("isin").unwrap(), "US0420682058");
        assert_eq!(
            ledger.try_get::<String, _>("instrument_name").unwrap(),
            "ARM Holdings plc ADR"
        );
    }

    #[tokio::test]
    async fn stale_zero_basis_snapshot_defers_to_broker_position() {
        let state = execution_order_test_state().await;
        // A stale snapshot row exists but carries no usable basis; the live
        // broker position must win over it.
        sqlx::query(
            "INSERT INTO position_snapshots (
                imported_at, instrument_name, symbol, isin, quantity, currency,
                cost_basis_local, cost_basis_dkk, excluded
            ) VALUES (
                '2026-05-18T00:00:00Z', 'ARM Holdings plc ADR', 'ARM:xnas', NULL,
                0, 'USD', 0, 0, 0
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed stale zero-quantity snapshot");
        sqlx::query(
            "INSERT INTO broker_position_snapshots (
                symbol, updated_at, quantity, open_price_local,
                open_price_including_costs_local, currency, isin, instrument_name, can_be_closed
            ) VALUES (
                'ARM:xnas', '2026-07-16T18:31:00Z', 5, 370.684, 371.0,
                'USD', 'US0420682058', 'ARM Holdings plc ADR', 1
            )",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker-authoritative position");

        let basis = latest_position_cost_basis(&state, "ARM:xnas")
            .await
            .expect("resolve cost basis");
        assert_eq!(basis.quantity, 5.0);
        assert!((basis.cost_basis_local - 1855.0).abs() < 1e-6);
        assert!((basis.cost_basis_dkk - 1855.0 * 7.0215).abs() < 1e-6);
    }

    #[test]
    fn splits_symbols_into_saxo_lookup_parts() {
        assert_eq!(
            symbol_parts("MSTR:xnas"),
            SymbolParts {
                base: "MSTR".to_string(),
                exchange: "xnas".to_string()
            }
        );
    }

    #[test]
    fn ranks_exact_exchange_match_above_same_base_elsewhere() {
        let requested = SymbolParts {
            base: "MSTR".to_string(),
            exchange: "xnas".to_string(),
        };
        let nasdaq = json!({"Symbol": "MSTR:xnas", "ExchangeId": "XNAS", "TradableAs": ["Stock"]});
        let other = json!({"Symbol": "MSTR:xnys", "ExchangeId": "XNYS", "TradableAs": ["Stock"]});

        assert!(
            candidate_score(&nasdaq, "MSTR:xnas", &requested)
                > candidate_score(&other, "MSTR:xnas", &requested)
        );
    }

    #[test]
    fn rejects_unrelated_saxo_keyword_hits() {
        let requested = SymbolParts {
            base: "PLTR".to_string(),
            exchange: "xnas".to_string(),
        };
        let unrelated = json!({
            "Symbol": "NEU:xwar",
            "ExchangeId": "XWAR",
            "TradableAs": ["Stock"]
        });
        let exact = json!({
            "Symbol": "PLTR:xnas",
            "ExchangeId": "XNAS",
            "TradableAs": ["Stock"]
        });

        assert!(!candidate_matches_requested(
            &unrelated,
            "PLTR:xnas",
            &requested
        ));
        assert!(candidate_matches_requested(&exact, "PLTR:xnas", &requested));
    }

    #[test]
    fn share_class_symbol_variants_match_saxo_compact_symbols() {
        let requested = symbol_parts("BRK-B:xnys");
        assert_eq!(
            symbol_lookup_variants(&requested),
            vec!["BRK-B:xnys".to_string(), "BRKb:xnys".to_string()]
        );
        let compact = json!({
            "Symbol": "BRKb:xnys",
            "ExchangeId": "XNYS",
            "TradableAs": ["Stock"]
        });
        let wrong_exchange = json!({
            "Symbol": "BRKb:xnas",
            "ExchangeId": "XNAS",
            "TradableAs": ["Stock"]
        });

        assert!(candidate_matches_requested(
            &compact,
            "BRK-B:xnys",
            &requested
        ));
        assert!(!candidate_matches_requested(
            &wrong_exchange,
            "BRK-B:xnys",
            &requested
        ));
    }

    #[test]
    fn uses_reviewed_reconciliation_instrument_when_present() {
        let order = json!({
            "symbol": "PLTR:xnas",
            "request_json": {
                "validation": {
                    "status": "valid",
                    "uic": 46019839,
                    "asset_type": "CfdOnStock",
                    "exchange_id": "NASDAQ",
                    "description": "Palantir Technologies Inc."
                }
            }
        });

        let instrument = instrument_from_reviewed_order(&order).expect("reviewed instrument");

        assert_eq!(instrument.uic, 46019839);
        assert_eq!(instrument.asset_type, "CfdOnStock");
        assert_eq!(instrument.description, "Palantir Technologies Inc.");
    }

    #[test]
    fn builds_external_reference_with_saxo_limit() {
        assert_eq!(external_reference(42), "saxo-daytrader:42");
        assert!(external_reference(123456789).len() <= 50);
    }

    #[test]
    fn normalizes_limit_prices_in_broker_safe_direction() {
        assert_eq!(
            normalize_order_price("MSTR:xnas", 187.596, "BUY", "limit"),
            187.59
        );
        assert_eq!(
            normalize_order_price("MSTR:xnas", 187.591, "SELL", "limit"),
            187.6
        );
    }

    #[test]
    fn normalizes_demant_like_limit_with_broker_tick_scheme() {
        let details = json!({
            "TickSizeScheme": {
                "Elements": [
                    {"HighPrice": 200.0, "TickSize": 0.05},
                    {"HighPrice": 500.0, "TickSize": 0.10}
                ],
                "DefaultTickSize": 0.01
            }
        });
        let tick = tick_from_instrument_details(261.3999938964844, &details);

        assert_eq!(tick, Some(0.10));
        assert_eq!(
            normalize_order_price_with_tick(261.3999938964844, tick.unwrap(), "BUY", "limit"),
            261.3
        );
    }

    #[test]
    fn configured_price_tick_override_accepts_symbol_and_exchange_keys() {
        let config: serde_yaml::Value = serde_yaml::from_str(
            r#"
execution:
  price_tick_overrides:
    DEMANT:xcse: 0.1
    XETR: 0.01
"#,
        )
        .unwrap();

        assert_eq!(
            configured_price_tick_override(&config, "DEMANT:xcse"),
            Some(0.1)
        );
        assert_eq!(
            configured_price_tick_override(&config, "ADS:xetr"),
            Some(0.01)
        );
    }

    #[test]
    fn maps_broker_expired_to_explicit_terminal_status() {
        assert_eq!(
            local_terminal_status(&Some("Expired".to_string())),
            "broker_expired"
        );
        assert_eq!(
            local_terminal_status(&Some("DoneForDay".to_string())),
            "broker_done_for_day"
        );
        assert_eq!(
            local_terminal_status(&Some("Rejected".to_string())),
            "execution_failed"
        );
    }

    #[test]
    fn broker_order_lookup_miss_state_preserves_local_reconciliation_context() {
        let state = broker_order_lookup_miss_state("5039132483");

        assert_eq!(
            state
                .get("broker_visibility")
                .and_then(JsonValue::as_str)
                .unwrap_or(""),
            "not_found"
        );
        assert_eq!(
            state
                .get("open_order_lookup")
                .and_then(JsonValue::as_str)
                .unwrap_or(""),
            "not_found"
        );
        assert_eq!(
            state
                .get("activity_lookup")
                .and_then(JsonValue::as_str)
                .unwrap_or(""),
            "not_found"
        );
        assert_eq!(
            broker_status_text(broker_payload(&state)).as_deref(),
            Some("NotFound")
        );
    }

    #[test]
    fn parses_live_saxo_position_quantities_by_symbol() {
        let payload = json!({
            "Data": [
                {
                    "DisplayAndFormat": {"Symbol": "AJG:xnys"},
                    "PositionBase": {"Amount": 2.0}
                },
                {
                    "DisplayAndFormat": {"Symbol": "MSTR:xnas"},
                    "PositionBase": {"Amount": 48}
                },
                {
                    "DisplayAndFormat": {"Symbol": "MSTR:xnas"},
                    "PositionBase": {"Amount": 3}
                },
                {
                    "DisplayAndFormat": {"Symbol": ""},
                    "PositionBase": {"Amount": 99}
                }
            ]
        });

        let quantities = parse_position_quantities(&payload);

        assert_eq!(
            quantities.get("AJG:xnys").map(|value| value.quantity),
            Some(2.0)
        );
        assert_eq!(
            quantities.get("MSTR:xnas").map(|value| value.quantity),
            Some(51.0)
        );
        assert_eq!(quantities.len(), 2);
    }

    #[test]
    fn parses_broker_held_instrument_for_sells() {
        let payload = json!({
            "Data": [
                {
                    "DisplayAndFormat": {
                        "Symbol": "PLTR:xnas",
                        "ExchangeId": "NASDAQ",
                        "Description": "Palantir Technologies Inc."
                    },
                    "PositionBase": {
                        "Amount": 31,
                        "AssetType": "CfdOnStock",
                        "Uic": 46019839
                    },
                    "PositionView": {"CanBeClosed": true}
                }
            ]
        });

        let positions = parse_position_quantities(&payload);
        let position = positions.get("PLTR:xnas").expect("PLTR broker position");
        let instrument = position.instrument.as_ref().expect("held Saxo instrument");

        assert_eq!(position.quantity, 31.0);
        assert!(position.can_be_closed);
        assert_eq!(instrument.uic, 46019839);
        assert_eq!(instrument.asset_type, "CfdOnStock");
    }

    #[test]
    fn detects_confirmed_final_fill_from_saxo_activity() {
        let activity = json!({
            "Status": "FinalFill",
            "SubStatus": "Confirmed",
            "FilledAmount": 100,
            "ExecutionPrice": 40.335
        });
        let status = broker_status_text(&activity);
        let substatus = json_text(&activity, "SubStatus");

        assert!(is_final_fill_status(&status, substatus.as_deref()));
        assert_eq!(extract_broker_quantity(&activity), Some(100.0));
        assert_eq!(extract_broker_price(&activity), Some(40.335));
    }

    #[test]
    fn broker_event_signature_is_stable_for_duplicate_status_payloads() {
        let first = broker_event_signature(
            44,
            "broker_final_fill",
            "5038240033",
            Some("FinalFill"),
            Some("Confirmed"),
            Some(100.0),
            Some(40.335),
        );
        let duplicate = broker_event_signature(
            44,
            "broker_final_fill",
            "5038240033",
            Some("FinalFill"),
            Some("Confirmed"),
            Some(100.0),
            Some(40.335),
        );

        assert_eq!(first, duplicate);
    }

    #[test]
    fn percent_encodes_saxo_client_key_for_path_segments() {
        assert_eq!(
            percent_encode_path_segment("ldJR0mfLg0buaAtllBotfQ=="),
            "ldJR0mfLg0buaAtllBotfQ%3D%3D"
        );
    }
}
