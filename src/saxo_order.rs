use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use reqwest::{StatusCode, header};
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    auth,
    config::{yaml_bool, yaml_string},
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

pub async fn run_saxo_execution_queue(state: &AppState) -> Result<JsonValue> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let adapter = yaml_string(&state.config, &["execution", "adapter"])
        .unwrap_or_else(|| "simulation".into());
    let dry_run = yaml_bool(&state.config, &["app", "dry_run"]).unwrap_or(true);
    let require_approval =
        yaml_bool(&state.config, &["execution", "require_approval_live"]).unwrap_or(true);

    if !execution_mode.eq_ignore_ascii_case("live") || !adapter.eq_ignore_ascii_case("saxo") {
        return Ok(json!({
            "status": "disabled",
            "reason": "Saxo execution queue only runs when execution.mode=live and execution.adapter=saxo.",
            "execution_mode": execution_mode,
            "adapter": adapter
        }));
    }
    if dry_run {
        return Ok(json!({"status": "disabled", "reason": "app.dry_run is true"}));
    }
    if require_approval {
        return Ok(
            json!({"status": "disabled", "reason": "execution.require_approval_live is true"}),
        );
    }

    let rows = pending_live_saxo_orders(state).await?;
    if rows.is_empty() {
        return Ok(json!({"status": "ok", "processed": [], "submitted": 0, "failed": 0}));
    }

    // Refresh is intentionally service-level. It does not depend on a logged-in UI user,
    // which lets the scheduler survive rollouts and dashboard logout.
    state
        .refresh_saxo_session()
        .await
        .context("refreshing Saxo session before executing queued orders")?;
    let session = auth::ensure_session_json(&state.config, &state.config_path).await?;

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
    broker_positions: &HashMap<String, f64>,
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
        let held_quantity = broker_positions.get(&symbol).copied().unwrap_or(0.0);
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
                        "available_quantity": available
                    }
                }),
            )
            .await;
        }
    }

    let request_payload = build_order_payload(state, session, order, quantity).await?;
    let precheck = precheck_order(state, session, &request_payload).await?;
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
) -> Result<JsonValue> {
    let symbol = order_text(order, "symbol");
    let action = order_text(order, "action").to_uppercase();
    let order_type = normalize_order_type(&order_text(order, "order_type"));
    let instrument = lookup_instrument(state, session, &symbol).await?;
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
            payload["OrderPrice"] = json!(normalize_order_price(&symbol, price, &action, "limit"));
        } else {
            let price = stop_price
                .ok_or_else(|| anyhow!("{order_type} orders require stop_price_local"))?;
            payload["OrderPrice"] = json!(normalize_order_price(&symbol, price, &action, "stop"));
        }
        if order_type == "StopLimit" {
            let price =
                limit_price.ok_or_else(|| anyhow!("StopLimit orders require limit_price_local"))?;
            payload["StopLimitPrice"] =
                json!(normalize_order_price(&symbol, price, &action, "stop_limit"));
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
    let requested_symbol = if parts.exchange.is_empty() {
        parts.base.clone()
    } else {
        format!("{}:{}", parts.base, parts.exchange)
    };
    let mut attempts = vec![
        (
            "symbol".to_string(),
            vec![("Keywords", requested_symbol.clone())],
            true,
        ),
        (
            "exchange".to_string(),
            vec![
                ("Keywords", parts.base.clone()),
                (
                    "ExchangeId",
                    exchange_id_for_suffix(&parts.exchange).to_string(),
                ),
            ],
            true,
        ),
    ];
    if let Some(isin) = latest_position_isin(state, symbol).await? {
        attempts.push(("isin".to_string(), vec![("Keywords", isin)], false));
    }
    attempts.push((
        "base".to_string(),
        vec![("Keywords", parts.base.clone())],
        true,
    ));

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

async fn broker_position_quantities(
    state: &AppState,
    session: &JsonValue,
) -> Result<HashMap<String, f64>> {
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

fn parse_position_quantities(payload: &JsonValue) -> HashMap<String, f64> {
    payload
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
            Some((symbol, amount))
        })
        .collect()
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
    let exact_symbol = i32::from(candidate_symbol == requested_symbol.to_uppercase());
    let exchange_match = i32::from(
        exchange_aliases(&parts.exchange)
            .iter()
            .any(|alias| candidate_exchange == *alias)
            || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase())),
    );
    let exact_base = i32::from(candidate_symbol.split(':').next().unwrap_or("") == parts.base);
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
    if candidate_symbol == requested_symbol.to_uppercase() {
        return true;
    }
    if candidate_base != parts.base {
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

fn normalize_order_price(symbol: &str, price: f64, action: &str, role: &str) -> f64 {
    let tick = default_tick(symbol);
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

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    "DisplayAndFormat": {"Symbol": ""},
                    "PositionBase": {"Amount": 99}
                }
            ]
        });

        let quantities = parse_position_quantities(&payload);

        assert_eq!(quantities.get("AJG:xnys"), Some(&2.0));
        assert_eq!(quantities.get("MSTR:xnas"), Some(&48.0));
        assert_eq!(quantities.len(), 2);
    }
}
