use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64},
    state::AppState,
};

const DEFAULT_MAX_REPORT_AGE_HOURS: i64 = 6;
const EXPERIMENT_STATUS_ALLOWLIST: &[&str] = &[
    "approved_sim",
    "active_sim",
    "approved_paper",
    "active_paper",
];
const EXPERIMENT_VARIABLE_ALLOWLIST: &[&str] = &[
    "execution.min_trade_value_dkk",
    "strategy.capital.min_cash_buffer_pct",
    "strategy.swing.cash_buffer_pct",
    "strategy.swing.daily_indicators.min_confluences",
];

#[derive(Clone, Debug)]
struct DecisionReport {
    id: i64,
    created_at: String,
    status: String,
    pulse_key: String,
    pulse_label: String,
    report_json: JsonValue,
}

#[derive(Clone, Debug, PartialEq)]
struct CandidateOrder {
    symbol: String,
    action: String,
    order_type: String,
    currency: Option<String>,
    quantity: f64,
    price_local: Option<f64>,
    limit_price_local: Option<f64>,
    stop_price_local: Option<f64>,
    requested_weight_pct: Option<f64>,
    estimated_value_dkk: Option<f64>,
    strategy_type: Option<String>,
    strategy_session: Option<String>,
    strategy_key: String,
    strategy_role: Option<String>,
    raw: JsonValue,
}

#[derive(Debug, PartialEq)]
struct GateDecision {
    approved: bool,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CapitalBudget {
    cash_balance_dkk: f64,
    total_market_value_dkk: f64,
    invested_market_value_dkk: f64,
    min_cash_buffer_pct: f64,
    max_deployment_pct: f64,
    required_cash_buffer_dkk: f64,
    available_buy_budget_dkk: f64,
    remaining_deployment_capacity_dkk: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct StrategyExperimentOverlay {
    id: String,
    status: String,
    goal_version: i64,
    changed_variable_path: String,
    old_value: JsonValue,
    new_value: JsonValue,
    hypothesis: String,
}

pub async fn run_trading_manager_cycle(state: &AppState) -> Result<JsonValue> {
    let reports = fresh_unmanaged_reports(state).await?;
    if reports.is_empty() {
        info!("Trading Manager found no fresh scheduled decision reports to process");
        return Ok(json!({"status": "not_due", "runs": []}));
    }

    if let Err(err) = state.refresh_saxo_exchange_calendars_if_stale().await {
        warn!("Trading Manager using fallback exchange calendar: {err:#}");
    }
    let market_rows = state.market_exchange_rows();
    let open_codes = market_rows
        .iter()
        .filter(|row| {
            row.get("is_tradable")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|row| row.get("code").and_then(JsonValue::as_str))
        .map(|code| code.to_uppercase())
        .collect::<Vec<_>>();

    let mut runs = Vec::new();
    for report in reports {
        match run_for_report(state, &report, &open_codes).await {
            Ok(run) => runs.push(run),
            Err(err) => {
                warn!(
                    report_id = report.id,
                    "Trading Manager report processing failed: {err:#}"
                );
                runs.push(json!({
                    "status": "error",
                    "report_id": report.id,
                    "manager_key": report.pulse_key,
                    "error": err.to_string()
                }));
            }
        }
    }

    Ok(json!({
        "status": "ok",
        "open_exchange_codes": open_codes,
        "runs": runs
    }))
}

async fn run_for_report(
    state: &AppState,
    report: &DecisionReport,
    open_codes: &[String],
) -> Result<JsonValue> {
    let candidates = candidate_orders_from_report(&report.report_json);
    let excluded = excluded_symbols(state);
    let overview = state.overview_payload().await.unwrap_or_else(|err| {
        warn!("Trading Manager capital context degraded: {err:#}");
        json!({})
    });
    let overlay = approved_strategy_experiment_overlay(state)
        .await
        .unwrap_or_else(|err| {
            warn!("Trading Manager experiment overlay disabled: {err:#}");
            None
        });
    let overlay_json = overlay
        .as_ref()
        .map(StrategyExperimentOverlay::to_json)
        .unwrap_or(JsonValue::Null);
    let overlay_min_cash_buffer_pct = overlay.as_ref().and_then(|overlay| {
        overlay
            .f64_value("strategy.capital.min_cash_buffer_pct")
            .or_else(|| overlay.f64_value("strategy.swing.cash_buffer_pct"))
    });
    let initial_capital_budget =
        capital_budget_from_overview(&overview, overlay_min_cash_buffer_pct);
    let mut capital_budget = initial_capital_budget;

    let min_trade_value_dkk = overlay
        .as_ref()
        .and_then(|overlay| overlay.f64_value("execution.min_trade_value_dkk"))
        .unwrap_or_else(|| {
            yaml_f64(&state.config, &["execution", "min_trade_value_dkk"]).unwrap_or(500.0)
        });
    let overlay_min_confluences = overlay
        .as_ref()
        .and_then(|overlay| overlay.i64_value("strategy.swing.daily_indicators.min_confluences"));
    let require_approval = yaml_bool(&state.config, &["execution", "require_approval_live"])
        .unwrap_or(true)
        && yaml_string(&state.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string())
            .eq_ignore_ascii_case("live");

    let mut approved = Vec::new();
    let mut skipped = Vec::new();
    for mut order in candidates {
        let exchange = exchange_code(&order.symbol);
        if !open_codes.iter().any(|code| code == &exchange) {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Exchange {exchange} is not currently tradable for this scheduler cycle. Open exchanges: {}.",
                    open_codes.join(", ")
                ),
            ));
            continue;
        }
        if excluded.iter().any(|symbol| symbol == &order.symbol) {
            skipped.push(skip_order(
                &order,
                "Symbol is excluded by risk configuration.",
            ));
            continue;
        }
        if order.quantity <= 0.0 {
            skipped.push(skip_order(&order, "Order quantity is zero or negative."));
            continue;
        }
        if order.estimated_value_dkk.unwrap_or(0.0) < min_trade_value_dkk {
            skipped.push(skip_order(
                &order,
                "Estimated trade value is below the configured minimum.",
            ));
            continue;
        }
        if order.action == "BUY" {
            let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
            if estimated_value_dkk > capital_budget.available_buy_budget_dkk + 0.01 {
                skipped.push(skip_order(
                    &order,
                    &format!(
                        "BUY would exceed available cash budget after buffer: requested {:.2} DKK, available {:.2} DKK.",
                        estimated_value_dkk, capital_budget.available_buy_budget_dkk
                    ),
                ));
                continue;
            }
        }
        if order.action == "SELL" {
            let available = latest_position_quantity(state, &order.symbol)
                .await
                .unwrap_or(0.0);
            if available <= 0.0 {
                skipped.push(skip_order(
                    &order,
                    "No local holding is available for this SELL.",
                ));
                continue;
            }
            order.quantity = order.quantity.min(available);
        }
        let gate = technical_gate(&order, overlay_min_confluences);
        if gate.approved {
            if order.action == "BUY" {
                capital_budget.reserve_buy(order.estimated_value_dkk.unwrap_or(0.0));
            }
            approved.push((order, gate.reason));
        } else {
            skipped.push(skip_order(&order, &gate.reason));
        }
    }

    let mut queued_orders = Vec::new();
    for (order, approval_reason) in &approved {
        queued_orders.push(
            insert_execution_order(
                state,
                report,
                order,
                approval_reason,
                require_approval,
                &overlay_json,
            )
            .await?,
        );
    }

    let queue_result = json!({
        "status": if queued_orders.is_empty() { "completed_no_orders" } else { "queued" },
        "orders": queued_orders
    });
    let manager_json = json!({
        "summary": "Rust Trading Manager approved scheduled report orders using embedded daily technical gates.",
        "capital_budget": initial_capital_budget.to_json(),
        "remaining_buy_budget_dkk": capital_budget.available_buy_budget_dkk,
        "approved_order_count": approved.len(),
        "skipped_order_count": skipped.len(),
        "approved_orders": approved.iter().map(|(order, reason)| json!({
            "strategy_key": order.strategy_key,
            "symbol": order.symbol,
            "action": order.action,
            "technical_gate": reason,
        })).collect::<Vec<_>>(),
        "skipped_orders": skipped,
        "strategy_experiment_overlay": overlay_json,
        "execution_notes": [
            "Approved Hermes experiment overlays are loaded only in paper/simulation mode or Saxo SIM.",
            "Orders are deduplicated by strategy_key before insertion.",
            "BUY orders are capped by cash available after the configured buffer and deployment cap.",
            "SELL quantities are capped to the latest local holding quantity."
        ]
    });
    let run_status = if approved.is_empty() {
        "completed_no_orders"
    } else {
        "completed"
    };
    let run_id = insert_trading_manager_run(
        state,
        report,
        run_status,
        open_codes,
        &manager_json,
        &queue_result,
        None,
    )
    .await?;

    info!(
        report_id = report.id,
        run_id,
        approved_orders = approved.len(),
        skipped_orders = manager_json
            .get("skipped_orders")
            .and_then(JsonValue::as_array)
            .map_or(0, Vec::len),
        "Trading Manager processed scheduled decision report"
    );

    Ok(json!({
        "id": run_id,
        "status": run_status,
        "report_id": report.id,
        "manager_key": report.pulse_key,
        "approved_orders": approved.len(),
        "queued_orders": queue_result.get("orders").cloned().unwrap_or_else(|| json!([])),
    }))
}

async fn fresh_unmanaged_reports(state: &AppState) -> Result<Vec<DecisionReport>> {
    let max_age_hours = yaml_i64(
        &state.config,
        &[
            "strategy",
            "swing",
            "trading_manager",
            "max_report_age_hours",
        ],
    )
    .unwrap_or(DEFAULT_MAX_REPORT_AGE_HOURS);
    let cutoff = Utc::now() - Duration::hours(max_age_hours.max(1));
    let rows = sqlx::query(
        "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, report_json
         FROM decision_reports
         WHERE report_json IS NOT NULL
           AND COALESCE(analysis_pulse_key, '') <> ''
         ORDER BY id DESC
         LIMIT 30",
    )
    .fetch_all(&state.pool)
    .await
    .context("loading recent decision reports for Trading Manager")?;

    let mut reports = Vec::new();
    for row in rows.iter().map(row_to_json) {
        let report = decode_report(&row)?;
        if !matches!(report.status.as_str(), "completed" | "xai_fallback") {
            continue;
        }
        if parse_report_time(&report.created_at).is_some_and(|created| created < cutoff) {
            continue;
        }
        if has_manager_run_for_report(state, report.id).await? {
            continue;
        }
        reports.push(report);
    }
    reports.sort_by_key(|report| report.id);
    Ok(reports)
}

fn decode_report(row: &JsonValue) -> Result<DecisionReport> {
    let report_json = row.get("report_json").cloned().unwrap_or(JsonValue::Null);
    let report_json = if let Some(text) = report_json.as_str() {
        serde_json::from_str(text).context("parsing decision report_json")?
    } else {
        report_json
    };
    Ok(DecisionReport {
        id: row.get("id").and_then(JsonValue::as_i64).unwrap_or(0),
        created_at: text(row, "created_at"),
        status: text(row, "status"),
        pulse_key: text(row, "analysis_pulse_key"),
        pulse_label: text(row, "analysis_pulse_label"),
        report_json,
    })
}

async fn has_manager_run_for_report(state: &AppState, report_id: i64) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM trading_manager_runs WHERE report_id = {} LIMIT 1",
        report_id.max(0)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

fn candidate_orders_from_report(report_json: &JsonValue) -> Vec<CandidateOrder> {
    let orders = report_json
        .get("strategy_plan")
        .and_then(|value| value.get("swing_orders"))
        .and_then(JsonValue::as_array)
        .or_else(|| {
            report_json
                .get("suggested_trades")
                .and_then(JsonValue::as_array)
        })
        .cloned()
        .unwrap_or_default();

    orders
        .into_iter()
        .filter_map(|raw| CandidateOrder::from_json(raw).ok())
        .collect()
}

impl CandidateOrder {
    fn from_json(raw: JsonValue) -> Result<Self> {
        let symbol = text(&raw, "symbol");
        let action = text(&raw, "action").to_uppercase();
        let order_type = fallback_text(&raw, "order_type", "Market");
        let strategy_key = fallback_text(
            &raw,
            "strategy_key",
            &format!(
                "rust-manager:{}:{}:{}",
                text(&raw, "session_tag"),
                symbol,
                action
            ),
        );
        let strategy_key = unique_strategy_key(strategy_key, &symbol, &action);
        Ok(Self {
            symbol,
            action,
            order_type,
            currency: optional_text(&raw, "currency"),
            quantity: value_f64(&raw, "quantity"),
            price_local: optional_f64(&raw, "price_local"),
            limit_price_local: optional_f64(&raw, "limit_price_local"),
            stop_price_local: optional_f64(&raw, "stop_price_local"),
            requested_weight_pct: optional_f64(&raw, "requested_weight_pct"),
            estimated_value_dkk: optional_f64(&raw, "estimated_value_dkk"),
            strategy_type: optional_text(&raw, "strategy_type"),
            strategy_session: optional_text(&raw, "session_tag"),
            strategy_key,
            strategy_role: optional_text(&raw, "strategy_role"),
            raw,
        })
    }
}

fn technical_gate(order: &CandidateOrder, overlay_min_confluences: Option<i64>) -> GateDecision {
    let technical = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("technical"));
    let Some(technical) = technical else {
        return GateDecision {
            approved: false,
            reason: "No usable daily technical indicator result.".to_string(),
        };
    };
    if technical.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return GateDecision {
            approved: false,
            reason: "No usable daily technical indicator result.".to_string(),
        };
    }
    let sentiment = fallback_text(technical, "sentiment", "HOLD").to_uppercase();
    let trend_bias = fallback_text(technical, "trend_bias", "neutral").to_lowercase();
    let confluences = value_f64(technical, "confluence_count") as i64;
    let minimum = overlay_min_confluences
        .unwrap_or_else(|| value_f64(technical, "min_confluences").max(3.0) as i64)
        .max(1);
    let strategy_role = order
        .strategy_role
        .as_deref()
        .unwrap_or(&order.action)
        .to_uppercase();

    match order.action.as_str() {
        "BUY" => {
            if !matches!(sentiment.as_str(), "BUY" | "OVERWEIGHT") {
                return GateDecision {
                    approved: false,
                    reason: format!("Technical sentiment is {sentiment}, not BUY/OVERWEIGHT."),
                };
            }
            if trend_bias != "bullish" {
                return GateDecision {
                    approved: false,
                    reason: format!("Trend bias is {trend_bias}, not bullish."),
                };
            }
            if confluences < minimum {
                return GateDecision {
                    approved: false,
                    reason: format!("Only {confluences}/{minimum} indicator confluences."),
                };
            }
            GateDecision {
                approved: true,
                reason: "BUY approved by bullish technical confluence.".to_string(),
            }
        }
        "SELL" => {
            if strategy_role == "FLATTEN"
                || matches!(sentiment.as_str(), "SELL" | "UNDERWEIGHT")
                || trend_bias == "bearish"
            {
                return GateDecision {
                    approved: true,
                    reason:
                        "SELL/FLATTEN approved by deteriorating technicals or explicit flatten role."
                            .to_string(),
                };
            }
            GateDecision {
                approved: false,
                reason: format!(
                    "SELL not approved; technical sentiment is {sentiment} with {trend_bias} trend."
                ),
            }
        }
        other => GateDecision {
            approved: false,
            reason: format!("Unsupported manager action {other}."),
        },
    }
}

async fn insert_execution_order(
    state: &AppState,
    report: &DecisionReport,
    order: &CandidateOrder,
    approval_reason: &str,
    require_approval: bool,
    overlay_json: &JsonValue,
) -> Result<JsonValue> {
    if let Some(existing) = existing_order_by_strategy_key(state, &order.strategy_key).await? {
        return Ok(json!({
            "id": existing,
            "strategy_key": order.strategy_key,
            "symbol": order.symbol,
            "status": "already_exists"
        }));
    }
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let status = if require_approval {
        "pending_approval"
    } else {
        "pending_execution"
    };
    let approved_at = if require_approval {
        "NULL".to_string()
    } else {
        format!("'{}'", sql_escape(&now))
    };
    let request_json = json!({
        "source": "rust_trading_manager",
        "approval_reason": approval_reason,
        "decision_report_id": report.id,
        "decision_pulse_key": report.pulse_key,
        "strategy_experiment_overlay": overlay_json,
        "order": order.raw,
    });
    let sql = format!(
        "INSERT INTO execution_orders (
            created_at, report_id, symbol, action, order_type, mode, status, adapter,
            requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local,
            currency, estimated_value_dkk, approval_required, approved_at, strategy_type,
            strategy_session, strategy_key, strategy_role, request_json, execution_result_json,
            error_text
        ) VALUES (
            '{}', {}, '{}', '{}', '{}', '{}', '{}', '{}',
            {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {}, '{}', {}, '{}', NULL, NULL
        )",
        sql_escape(&now),
        report.id,
        sql_escape(&order.symbol),
        sql_escape(&order.action),
        sql_escape(&order.order_type),
        sql_escape(
            &yaml_string(&state.config, &["execution", "mode"])
                .unwrap_or_else(|| "simulation".to_string())
        ),
        status,
        sql_escape(
            &yaml_string(&state.config, &["execution", "adapter"])
                .unwrap_or_else(|| "saxo".to_string())
        ),
        sql_num(order.requested_weight_pct),
        order.quantity,
        sql_num(order.price_local),
        sql_num(order.limit_price_local),
        sql_num(order.stop_price_local),
        sql_opt_text(order.currency.as_deref()),
        sql_num(order.estimated_value_dkk),
        if require_approval { 1 } else { 0 },
        approved_at,
        sql_opt_text(order.strategy_type.as_deref()),
        sql_opt_text(order.strategy_session.as_deref()),
        sql_escape(&order.strategy_key),
        sql_opt_text(order.strategy_role.as_deref()),
        sql_escape(&serde_json::to_string(&request_json)?)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Trading Manager execution order")?;
    let id = existing_order_by_strategy_key(state, &order.strategy_key)
        .await?
        .unwrap_or(0);
    if id > 0 {
        insert_order_event(state, id, "queued_by_trading_manager", &request_json).await?;
    }
    Ok(json!({
        "id": id,
        "strategy_key": order.strategy_key,
        "symbol": order.symbol,
        "action": order.action,
        "status": status
    }))
}

async fn insert_trading_manager_run(
    state: &AppState,
    report: &DecisionReport,
    status: &str,
    open_codes: &[String],
    manager_json: &JsonValue,
    queue_result: &JsonValue,
    error_text: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let manager_kind = report
        .pulse_key
        .split(':')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("scheduled_report");
    let target_at_utc = report.created_at.clone();
    let sql = format!(
        "INSERT INTO trading_manager_runs (
            created_at, manager_key, manager_kind, manager_label, target_at_utc, report_id,
            status, open_exchange_codes_json, technical_json, manager_json, queue_result_json,
            error_text
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', {}, '{}', '{}', '{{}}', '{}', '{}', {}
        )",
        sql_escape(&now),
        sql_escape(&report.pulse_key),
        sql_escape(manager_kind),
        sql_escape(&report.pulse_label),
        sql_escape(&target_at_utc),
        report.id,
        sql_escape(status),
        sql_escape(&serde_json::to_string(open_codes)?),
        sql_escape(&serde_json::to_string(manager_json)?),
        sql_escape(&serde_json::to_string(queue_result)?),
        sql_opt_text(error_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording Trading Manager run")?;
    let row = sqlx::query(&format!(
        "SELECT id FROM trading_manager_runs WHERE report_id = {} ORDER BY id DESC LIMIT 1",
        report.id
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<i64, _>("id").ok())
        .unwrap_or(0))
}

async fn existing_order_by_strategy_key(
    state: &AppState,
    strategy_key: &str,
) -> Result<Option<i64>> {
    let row = sqlx::query(&format!(
        "SELECT id FROM execution_orders WHERE strategy_key = '{}' ORDER BY id DESC LIMIT 1",
        sql_escape(strategy_key)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(|row| row.try_get::<i64, _>("id").ok()))
}

async fn insert_order_event(
    state: &AppState,
    order_id: i64,
    event_type: &str,
    payload: &JsonValue,
) -> Result<()> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let payload_text = serde_json::to_string(payload)?;
    let signature = format!("{event_type}:{order_id}");
    let sql = format!(
        "INSERT INTO execution_order_events (
            created_at, execution_order_id, broker_order_id, event_type, broker_status,
            broker_substatus, broker_quantity, broker_price_local, event_signature,
            raw_payload_json
        ) VALUES (
            '{}', {}, NULL, '{}', NULL, NULL, NULL, NULL, '{}', '{}'
        )
        ON CONFLICT(event_signature) DO NOTHING",
        sql_escape(&now),
        order_id,
        sql_escape(event_type),
        sql_escape(&signature),
        sql_escape(&payload_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording execution order event")?;
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

fn skip_order(order: &CandidateOrder, reason: &str) -> JsonValue {
    json!({
        "strategy_key": order.strategy_key,
        "symbol": order.symbol,
        "action": order.action,
        "technical_gate": reason,
    })
}

fn unique_strategy_key(strategy_key: String, symbol: &str, action: &str) -> String {
    if strategy_key.contains(symbol) {
        strategy_key
    } else {
        format!("{strategy_key}:{symbol}:{action}")
    }
}

async fn approved_strategy_experiment_overlay(
    state: &AppState,
) -> Result<Option<StrategyExperimentOverlay>> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let saxo_environment =
        yaml_string(&state.config, &["saxo", "environment"]).unwrap_or_else(|| "SIM".into());
    if !experiment_overlays_allowed(&execution_mode, &saxo_environment) {
        return Ok(None);
    }

    let statuses = EXPERIMENT_STATUS_ALLOWLIST
        .iter()
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = sqlx::query(&format!(
        "SELECT id, created_at, status, baseline_id, goal_version, hypothesis,
            changed_variable_path, old_value_json, new_value_json, expected_effect,
            risk_notes, evidence_json, approval_json, metrics_json, source_session_id,
            raw_payload_json
         FROM strategy_experiments
         WHERE status IN ({statuses})
         ORDER BY created_at DESC, id DESC
         LIMIT 10"
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading approved Hermes strategy experiment overlay")?;

    for row in rows.iter().map(row_to_json) {
        if let Some(overlay) = StrategyExperimentOverlay::from_row(&row) {
            return Ok(Some(overlay));
        }
        warn!(
            experiment_id = text(&row, "id"),
            variable = text(&row, "changed_variable_path"),
            "Ignoring unsupported Hermes strategy experiment overlay"
        );
    }
    Ok(None)
}

fn experiment_overlays_allowed(execution_mode: &str, saxo_environment: &str) -> bool {
    !execution_mode.eq_ignore_ascii_case("live") || saxo_environment.eq_ignore_ascii_case("SIM")
}

impl StrategyExperimentOverlay {
    fn from_row(row: &JsonValue) -> Option<Self> {
        let changed_variable_path = text(row, "changed_variable_path");
        if !EXPERIMENT_VARIABLE_ALLOWLIST
            .iter()
            .any(|path| *path == changed_variable_path)
        {
            return None;
        }
        let new_value = row
            .get("new_value_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        json_f64_value(&new_value)?;
        Some(Self {
            id: text(row, "id"),
            status: text(row, "status"),
            goal_version: row
                .get("goal_version")
                .and_then(JsonValue::as_i64)
                .unwrap_or(1),
            changed_variable_path,
            old_value: row
                .get("old_value_json")
                .cloned()
                .unwrap_or(JsonValue::Null),
            new_value,
            hypothesis: text(row, "hypothesis"),
        })
    }

    fn f64_value(&self, variable_path: &str) -> Option<f64> {
        (self.changed_variable_path == variable_path)
            .then(|| json_f64_value(&self.new_value))
            .flatten()
    }

    fn i64_value(&self, variable_path: &str) -> Option<i64> {
        self.f64_value(variable_path)
            .map(|value| value.round() as i64)
    }

    fn to_json(&self) -> JsonValue {
        json!({
            "id": self.id,
            "status": self.status,
            "goal_version": self.goal_version,
            "changed_variable_path": self.changed_variable_path,
            "old_value": self.old_value,
            "new_value": self.new_value,
            "hypothesis": self.hypothesis,
            "scope": "paper_or_saxo_sim_only"
        })
    }
}

fn json_f64_value(value: &JsonValue) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
}

fn capital_budget_from_overview(
    overview: &JsonValue,
    overlay_min_cash_buffer_pct: Option<f64>,
) -> CapitalBudget {
    let summary = overview
        .get("portfolio_summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let cash_policy = overview
        .get("settings")
        .and_then(|value| value.get("cash_buffer"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let total_market_value_dkk = value_f64(&summary, "total_market_value_dkk");
    let invested_market_value_dkk = value_f64(&summary, "invested_market_value_dkk");
    let cash_balance_dkk = value_f64(&summary, "cash_balance_dkk");
    let min_cash_buffer_pct = overlay_min_cash_buffer_pct
        .unwrap_or_else(|| value_f64(&cash_policy, "min_cash_buffer_pct"))
        .clamp(0.0, 1.0);
    let max_deployment_pct = value_f64(&cash_policy, "max_deployment_pct").clamp(0.0, 1.0);
    let required_cash_buffer_dkk = (total_market_value_dkk * min_cash_buffer_pct).max(0.0);
    let deployment_cap_dkk = if max_deployment_pct > 0.0 {
        total_market_value_dkk * max_deployment_pct
    } else {
        total_market_value_dkk
    };
    let available_cash_above_buffer_dkk = (cash_balance_dkk - required_cash_buffer_dkk).max(0.0);
    let remaining_deployment_capacity_dkk =
        (deployment_cap_dkk - invested_market_value_dkk).max(0.0);
    let available_buy_budget_dkk =
        available_cash_above_buffer_dkk.min(remaining_deployment_capacity_dkk);
    CapitalBudget {
        cash_balance_dkk,
        total_market_value_dkk,
        invested_market_value_dkk,
        min_cash_buffer_pct,
        max_deployment_pct,
        required_cash_buffer_dkk,
        available_buy_budget_dkk,
        remaining_deployment_capacity_dkk,
    }
}

impl CapitalBudget {
    fn reserve_buy(&mut self, estimated_value_dkk: f64) {
        self.available_buy_budget_dkk =
            (self.available_buy_budget_dkk - estimated_value_dkk.max(0.0)).max(0.0);
    }

    fn to_json(self) -> JsonValue {
        json!({
            "cash_balance_dkk": self.cash_balance_dkk,
            "total_market_value_dkk": self.total_market_value_dkk,
            "invested_market_value_dkk": self.invested_market_value_dkk,
            "min_cash_buffer_pct": self.min_cash_buffer_pct,
            "max_deployment_pct": self.max_deployment_pct,
            "required_cash_buffer_dkk": self.required_cash_buffer_dkk,
            "available_buy_budget_dkk": self.available_buy_budget_dkk,
            "remaining_deployment_capacity_dkk": self.remaining_deployment_capacity_dkk,
        })
    }
}

fn excluded_symbols(state: &AppState) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(items) = state
        .config
        .get("risk")
        .and_then(|value| value.get("excluded_symbols"))
        .and_then(serde_yaml::Value::as_sequence)
    {
        values.extend(
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string),
        );
    }
    if let Some(items) = state
        .config
        .get("strategy")
        .and_then(|value| value.get("swing"))
        .and_then(|value| value.get("never_trade_symbols"))
        .and_then(serde_yaml::Value::as_sequence)
    {
        values.extend(
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string),
        );
    }
    values
}

fn exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange)
        .unwrap_or("")
        .to_uppercase()
}

fn parse_report_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn optional_f64(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
}

fn optional_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn text(value: &JsonValue, key: &str) -> String {
    optional_text(value, key).unwrap_or_default()
}

fn fallback_text(value: &JsonValue, key: &str, fallback: &str) -> String {
    optional_text(value, key).unwrap_or_else(|| fallback.to_string())
}

fn sql_num(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn order(
        action: &str,
        sentiment: &str,
        trend_bias: &str,
        confluence_count: i64,
    ) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "NVDA:xnas",
            "action": action,
            "quantity": 4,
            "order_type": "Limit",
            "price_local": 215.45,
            "limit_price_local": 215.45,
            "estimated_value_dkk": 5400,
            "strategy_key": format!("test:{action}"),
            "strategy_role": action.to_lowercase(),
            "strategy_metadata": {
                "technical": {
                    "status": "ok",
                    "sentiment": sentiment,
                    "trend_bias": trend_bias,
                    "confluence_count": confluence_count,
                    "min_confluences": 3
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn extracts_strategy_plan_swing_orders() {
        let report = json!({
            "strategy_plan": {
                "swing_orders": [
                    {
                        "symbol": "NVDA:xnas",
                        "action": "BUY",
                        "quantity": 4,
                        "strategy_key": "swing:test",
                        "estimated_value_dkk": 5400
                    }
                ]
            }
        });
        let orders = candidate_orders_from_report(&report);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].strategy_key, "swing:test:NVDA:xnas:BUY");
    }

    #[test]
    fn makes_model_strategy_keys_symbol_specific() {
        assert_eq!(
            unique_strategy_key("rebalance_overweight".to_string(), "MSTR:xnas", "SELL"),
            "rebalance_overweight:MSTR:xnas:SELL"
        );
        assert_eq!(
            unique_strategy_key("pulse:MSTR:xnas:SELL".to_string(), "MSTR:xnas", "SELL"),
            "pulse:MSTR:xnas:SELL"
        );
    }

    #[test]
    fn approves_only_bullish_buy_setups() {
        assert!(technical_gate(&order("BUY", "BUY", "bullish", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "HOLD", "bullish", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "BUY", "neutral", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "BUY", "bullish", 2), None).approved);
    }

    #[test]
    fn approves_risk_reducing_sell_setups() {
        assert!(technical_gate(&order("SELL", "SELL", "neutral", 1), None).approved);
        assert!(technical_gate(&order("SELL", "HOLD", "bearish", 1), None).approved);
        assert!(!technical_gate(&order("SELL", "HOLD", "bullish", 3), None).approved);
    }

    #[test]
    fn experiment_overlay_can_adjust_min_confluences() {
        assert!(!technical_gate(&order("BUY", "BUY", "bullish", 2), None).approved);
        assert!(technical_gate(&order("BUY", "BUY", "bullish", 2), Some(2)).approved);
    }

    #[test]
    fn experiment_overlays_are_not_allowed_for_live_saxo_live() {
        assert!(experiment_overlays_allowed("simulation", "LIVE"));
        assert!(experiment_overlays_allowed("live", "SIM"));
        assert!(!experiment_overlays_allowed("live", "LIVE"));
    }

    #[test]
    fn parses_supported_strategy_experiment_overlay() {
        let row = json!({
            "id": "strategy-experiment-test",
            "status": "approved_sim",
            "goal_version": 1,
            "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
            "old_value_json": 0.10,
            "new_value_json": "0.15",
            "hypothesis": "More cash buffer reduces drawdown."
        });
        let overlay = StrategyExperimentOverlay::from_row(&row).unwrap();
        assert_eq!(
            overlay.f64_value("strategy.capital.min_cash_buffer_pct"),
            Some(0.15)
        );
        assert!(
            StrategyExperimentOverlay::from_row(&json!({
                "id": "unsupported",
                "status": "approved_sim",
                "changed_variable_path": "execution.adapter",
                "new_value_json": "saxo"
            }))
            .is_none()
        );
    }

    #[test]
    fn derives_available_buy_budget_after_cash_buffer() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 250000.0,
                "cash_balance_dkk": 50000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90
                }
            }
        });
        let mut budget = capital_budget_from_overview(&overview, None);
        assert_eq!(budget.required_cash_buffer_dkk, 30000.0);
        assert_eq!(budget.available_buy_budget_dkk, 20000.0);
        budget.reserve_buy(7500.0);
        assert_eq!(budget.available_buy_budget_dkk, 12500.0);
    }

    #[test]
    fn experiment_overlay_can_adjust_cash_buffer() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 250000.0,
                "cash_balance_dkk": 50000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90
                }
            }
        });
        let budget = capital_budget_from_overview(&overview, Some(0.15));
        assert_eq!(budget.required_cash_buffer_dkk, 45000.0);
        assert_eq!(budget.available_buy_budget_dkk, 5000.0);
    }
}
