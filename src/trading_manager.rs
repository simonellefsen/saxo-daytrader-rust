use std::{collections::HashMap, env, time::Duration as StdDuration};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tokio::time::sleep;
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
    "strategy.swing.markov_gate.min_signed_signal",
    "strategy.swing.markov_gate.max_position_pct",
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
pub(crate) struct MarkovGateConfig {
    pub(crate) enabled: bool,
    pub(crate) min_signed_signal: f64,
    pub(crate) max_position_pct: f64,
    pub(crate) max_signal_age_days: i64,
}

pub(crate) fn markov_gate_config(state: &AppState) -> MarkovGateConfig {
    MarkovGateConfig {
        enabled: yaml_bool(
            &state.config,
            &["strategy", "swing", "markov_gate", "enabled"],
        )
        .unwrap_or(true),
        min_signed_signal: yaml_f64(
            &state.config,
            &["strategy", "swing", "markov_gate", "min_signed_signal"],
        )
        .unwrap_or(0.15)
        .max(0.0),
        max_position_pct: yaml_f64(
            &state.config,
            &["strategy", "swing", "markov_gate", "max_position_pct"],
        )
        .unwrap_or(0.05)
        .clamp(0.0, 1.0),
        max_signal_age_days: yaml_i64(
            &state.config,
            &["strategy", "swing", "markov_gate", "max_signal_age_days"],
        )
        .unwrap_or(5)
        .max(1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CapitalBudget {
    cash_balance_dkk: f64,
    total_market_value_dkk: f64,
    invested_market_value_dkk: f64,
    cash_pct: f64,
    min_cash_buffer_pct: f64,
    max_deployment_pct: f64,
    reinvestment_pressure_threshold_pct: f64,
    required_cash_buffer_dkk: f64,
    available_cash_above_buffer_dkk: f64,
    available_buy_budget_dkk: f64,
    remaining_deployment_capacity_dkk: f64,
    excess_cash_pct: f64,
    reinvestment_pressure_active: bool,
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

#[derive(Clone, Debug, PartialEq)]
struct HermesDecisionAdvice {
    status: String,
    mode: String,
    source_session_id: String,
    overall_recommendation: String,
    summary: String,
    raw: JsonValue,
    order_advice: HashMap<String, HermesOrderAdvice>,
}

#[derive(Clone, Debug, PartialEq)]
struct HermesOrderAdvice {
    action: String,
    reason: String,
    max_quantity: Option<f64>,
    raw: JsonValue,
}

impl HermesDecisionAdvice {
    fn fallback(status: &str, mode: String, summary: String, report_id: i64) -> Self {
        let overall_recommendation = if mode == "conservative"
            && matches!(
                status,
                "error" | "timeout" | "not_configured" | "submit_failed"
            ) {
            "review"
        } else {
            "proceed"
        };
        let raw = json!({
            "status": status,
            "decision_report_id": report_id,
            "overall_recommendation": overall_recommendation,
            "summary": summary,
            "order_advice_json": [],
            "learning_notes_json": []
        });
        Self {
            status: status.to_string(),
            mode,
            source_session_id: String::new(),
            overall_recommendation: overall_recommendation.to_string(),
            summary,
            raw,
            order_advice: HashMap::new(),
        }
    }

    fn from_row(row: JsonValue, mode: String, source_session_id: String) -> Self {
        let order_advice_json = row
            .get("order_advice_json")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let mut order_advice = HashMap::new();
        if let Some(items) = order_advice_json.as_array() {
            for item in items {
                let advice = HermesOrderAdvice {
                    action: text(item, "action").trim().to_lowercase(),
                    reason: text(item, "reason"),
                    max_quantity: item.get("max_quantity").and_then(JsonValue::as_f64),
                    raw: item.clone(),
                };
                if !matches!(
                    advice.action.as_str(),
                    "allow" | "reduce" | "stand_down" | "review"
                ) {
                    continue;
                }
                let strategy_key = text(item, "strategy_key");
                if !strategy_key.trim().is_empty() {
                    order_advice.insert(format!("strategy:{strategy_key}"), advice.clone());
                }
                let symbol = text(item, "symbol");
                let side = text(item, "side").to_uppercase();
                if !symbol.trim().is_empty() && !side.trim().is_empty() {
                    order_advice.insert(format!("symbol_side:{symbol}:{side}"), advice.clone());
                }
                if !symbol.trim().is_empty() {
                    order_advice.insert(format!("symbol:{symbol}"), advice);
                }
            }
        }

        Self {
            status: text(&row, "status"),
            mode,
            source_session_id,
            overall_recommendation: text(&row, "overall_recommendation").trim().to_lowercase(),
            summary: text(&row, "summary"),
            raw: row,
            order_advice,
        }
    }

    fn for_order(&self, order: &CandidateOrder) -> Option<&HermesOrderAdvice> {
        self.order_advice
            .get(&format!("strategy:{}", order.strategy_key))
            .or_else(|| {
                self.order_advice
                    .get(&format!("symbol_side:{}:{}", order.symbol, order.action))
            })
            .or_else(|| self.order_advice.get(&format!("symbol:{}", order.symbol)))
    }

    fn to_json(&self) -> JsonValue {
        json!({
            "status": self.status,
            "mode": self.mode,
            "source_session_id": self.source_session_id,
            "overall_recommendation": self.overall_recommendation,
            "summary": self.summary,
            "raw": self.raw
        })
    }
}

fn attach_hermes_advice(
    order: &mut CandidateOrder,
    advice: &HermesOrderAdvice,
    decision_advice: &HermesDecisionAdvice,
) {
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "hermes_advice".to_string(),
            json!({
                "mode": decision_advice.mode,
                "status": decision_advice.status,
                "source_session_id": decision_advice.source_session_id,
                "overall_recommendation": decision_advice.overall_recommendation,
                "order_advice": advice.raw,
            }),
        );
    }
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

    // One budget for the whole cycle: every report's approved BUYs reserve
    // from the same pool, so near-simultaneous reports cannot double-spend
    // the same cash snapshot.
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
    let overlay_min_cash_buffer_pct = overlay.as_ref().and_then(|overlay| {
        overlay
            .f64_value("strategy.capital.min_cash_buffer_pct")
            .or_else(|| overlay.f64_value("strategy.swing.cash_buffer_pct"))
    });
    let mut capital_budget = capital_budget_from_overview(&overview, overlay_min_cash_buffer_pct);

    let mut runs = Vec::new();
    for report in reports {
        match run_for_report(
            state,
            &report,
            &open_codes,
            overlay.as_ref(),
            &mut capital_budget,
        )
        .await
        {
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
    overlay: Option<&StrategyExperimentOverlay>,
    capital_budget: &mut CapitalBudget,
) -> Result<JsonValue> {
    let candidates = candidate_orders_from_report(&report.report_json);
    let candidate_order_count = candidates.len();
    let buy_candidate_count = candidates
        .iter()
        .filter(|order| order.action == "BUY")
        .count();
    let sell_candidate_count = candidates
        .iter()
        .filter(|order| order.action == "SELL")
        .count();
    let excluded = excluded_symbols(state);
    let overlay_json = overlay
        .map(|overlay| overlay.clone().to_json())
        .unwrap_or(JsonValue::Null);
    let initial_capital_budget = *capital_budget;
    let hermes_advice = request_hermes_decision_advice(
        state,
        report,
        &candidates,
        open_codes,
        &initial_capital_budget,
        &overlay_json,
    )
    .await
    .unwrap_or_else(|err| {
        warn!(
            report_id = report.id,
            "Hermes decision advice degraded: {err:#}"
        );
        HermesDecisionAdvice::fallback(
            "error",
            hermes_advisory_mode(),
            format!("Hermes decision advice failed: {err:#}"),
            report.id,
        )
    });
    let hermes_conservative = hermes_advice.mode == "conservative";
    let hermes_global_block =
        hermes_conservative && hermes_advice.overall_recommendation == "stand_down";
    let hermes_global_review =
        hermes_conservative && hermes_advice.overall_recommendation == "review";

    let min_trade_value_dkk = overlay
        .and_then(|overlay| overlay.f64_value("execution.min_trade_value_dkk"))
        .unwrap_or_else(|| {
            yaml_f64(&state.config, &["execution", "min_trade_value_dkk"]).unwrap_or(500.0)
        });
    let overlay_min_confluences = overlay
        .and_then(|overlay| overlay.i64_value("strategy.swing.daily_indicators.min_confluences"));
    let mut markov_cfg = markov_gate_config(state);
    if let Some(value) = overlay
        .and_then(|overlay| overlay.f64_value("strategy.swing.markov_gate.min_signed_signal"))
    {
        markov_cfg.min_signed_signal = value.max(0.0);
    }
    if let Some(value) =
        overlay.and_then(|overlay| overlay.f64_value("strategy.swing.markov_gate.max_position_pct"))
    {
        markov_cfg.max_position_pct = value.clamp(0.0, 1.0);
    }
    let require_approval = yaml_bool(&state.config, &["execution", "require_approval_live"])
        .unwrap_or(true)
        && yaml_string(&state.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string())
            .eq_ignore_ascii_case("live");

    let mut approved = Vec::new();
    let mut skipped = Vec::new();
    for mut order in candidates {
        let mut has_order_specific_hermes_allow = false;
        if let Some(advice) = hermes_advice.for_order(&order) {
            attach_hermes_advice(&mut order, advice, &hermes_advice);
            if hermes_conservative && matches!(advice.action.as_str(), "stand_down" | "review") {
                skipped.push(skip_order(
                    &order,
                    &format!("Hermes advisory {}: {}", advice.action, advice.reason),
                ));
                continue;
            }
            if hermes_conservative && advice.action == "reduce" {
                has_order_specific_hermes_allow = true;
                match advice.max_quantity {
                    Some(max_quantity) if max_quantity >= 1.0 && max_quantity < order.quantity => {
                        let original_quantity = order.quantity;
                        let new_quantity = max_quantity.floor();
                        let factor = new_quantity / original_quantity;
                        order.quantity = new_quantity;
                        if let Some(value) = order.estimated_value_dkk {
                            order.estimated_value_dkk = Some(value * factor);
                        }
                    }
                    Some(max_quantity) if max_quantity < 1.0 => {
                        skipped.push(skip_order(
                            &order,
                            &format!("Hermes advisory reduce below one share: {}", advice.reason),
                        ));
                        continue;
                    }
                    _ => {}
                }
            } else if hermes_conservative && advice.action == "allow" {
                has_order_specific_hermes_allow = true;
            }
        }
        if hermes_global_block {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Hermes advisory {} for report: {}",
                    hermes_advice.overall_recommendation, hermes_advice.summary
                ),
            ));
            continue;
        }
        if hermes_global_review && !has_order_specific_hermes_allow {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Hermes advisory review for report requires explicit order allow/reduce: {}",
                    hermes_advice.summary
                ),
            ));
            continue;
        }
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
        let shape = order_shape_gate(&mut order);
        if !shape.approved {
            skipped.push(skip_order(&order, &shape.reason));
            continue;
        }
        let mut value_verified = false;
        if order.action == "BUY" {
            value_verified = verify_buy_value(state, &mut order).await;
            let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
            if estimated_value_dkk > capital_budget.available_buy_budget_dkk + 0.01 {
                // With a verified per-share price the order can be downsized
                // to fit the budget instead of being rejected outright.
                let per_share_dkk = if value_verified && order.quantity >= 1.0 {
                    estimated_value_dkk / order.quantity
                } else {
                    0.0
                };
                let affordable_quantity = if per_share_dkk > f64::EPSILON {
                    (capital_budget.available_buy_budget_dkk / per_share_dkk).floor()
                } else {
                    0.0
                };
                if affordable_quantity >= 1.0 {
                    let original_quantity = order.quantity;
                    order.quantity = affordable_quantity;
                    order.estimated_value_dkk = Some(per_share_dkk * affordable_quantity);
                    if let Some(metadata) = order
                        .raw
                        .as_object_mut()
                        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
                        .and_then(JsonValue::as_object_mut)
                    {
                        metadata.insert(
                            "budget_downsize".to_string(),
                            json!({
                                "original_quantity": original_quantity,
                                "downsized_quantity": affordable_quantity,
                                "per_share_dkk": per_share_dkk,
                                "available_buy_budget_dkk": capital_budget.available_buy_budget_dkk,
                            }),
                        );
                    }
                } else {
                    skipped.push(skip_order(
                        &order,
                        &format!(
                            "BUY would exceed available cash budget after buffer: requested {:.2} DKK ({}), available {:.2} DKK{}.",
                            estimated_value_dkk,
                            if value_verified { "database-verified value" } else { "model-claimed value, no verified price available" },
                            capital_budget.available_buy_budget_dkk,
                            if value_verified { "; even one share does not fit" } else { "; cannot downsize without a verified price" }
                        ),
                    ));
                    continue;
                }
            }
        }
        if order.estimated_value_dkk.unwrap_or(0.0) < min_trade_value_dkk {
            skipped.push(skip_order(
                &order,
                "Estimated trade value is below the configured minimum.",
            ));
            continue;
        }
        if order.action == "SELL" {
            let available = latest_sellable_position_quantity(state, &order.symbol)
                .await
                .unwrap_or(0.0);
            if available <= 0.0 {
                skipped.push(skip_order(
                    &order,
                    "No broker-authoritative sellable holding is available for this SELL.",
                ));
                continue;
            }
            order.quantity = order.quantity.min(available);
        }
        let verified = apply_verified_technical(state, &mut order, Utc::now().date_naive()).await;
        let mut gate = technical_gate(&order, overlay_min_confluences);
        if verified {
            gate.reason = format!("{} (database-verified daily indicators)", gate.reason);
        }
        // The Markov starter path is fallback evidence for symbols without
        // indicator coverage; it never overrides verified technicals.
        if !verified && !gate.approved && order.action == "BUY" && markov_cfg.enabled {
            let held_quantity = latest_position_quantity(state, &order.symbol)
                .await
                .unwrap_or(0.0);
            let starter_block = if !value_verified {
                Some("no database-verified price is available for starter sizing".to_string())
            } else if held_quantity > 0.0 {
                Some(format!(
                    "symbol is already held ({held_quantity}); starters only initiate new positions"
                ))
            } else if has_buy_order_today(state, &order.symbol)
                .await
                .unwrap_or(true)
            {
                Some("a BUY order for this symbol was already created today".to_string())
            } else {
                None
            };
            match starter_block {
                Some(reason) => {
                    gate.reason = format!("{} Markov starter blocked: {reason}.", gate.reason);
                }
                None => {
                    let evidence = match latest_markov_signal(state, &order.symbol).await {
                        Ok(evidence) => evidence,
                        Err(err) => {
                            warn!("Markov gate lookup failed for {}: {err:#}", order.symbol);
                            None
                        }
                    };
                    let markov_gate = markov_buy_gate(
                        &mut order,
                        evidence.as_ref(),
                        markov_cfg,
                        initial_capital_budget.total_market_value_dkk,
                        Utc::now().date_naive(),
                    );
                    if markov_gate.approved {
                        gate = markov_gate;
                    } else {
                        gate.reason =
                            format!("{} Markov fallback: {}", gate.reason, markov_gate.reason);
                    }
                }
            }
        }
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
        "reinvestment_diagnostics": reinvestment_diagnostics(
            &initial_capital_budget,
            candidate_order_count,
            buy_candidate_count,
            sell_candidate_count,
            approved.iter().filter(|(order, _)| order.action == "BUY").count(),
            approved.iter().filter(|(order, _)| order.action == "SELL").count(),
            skipped.iter().filter(|order| order.get("action").and_then(JsonValue::as_str) == Some("BUY")).count(),
            skipped.iter().filter(|order| order.get("action").and_then(JsonValue::as_str) == Some("SELL")).count(),
        ),
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
        "hermes_decision_advice": hermes_advice.to_json(),
        "execution_notes": [
            "Approved Hermes experiment overlays are loaded only in paper/simulation mode or Saxo SIM.",
            "Hermes decision advice is audited for every fresh report when configured; by default it is record-only. In conservative mode it can only block, reduce, or require review.",
            "Orders are deduplicated by strategy_key before insertion.",
            "BUY orders are capped by cash available after the configured buffer and deployment cap.",
            "BUY orders without technical confluence can pass as starter positions when a fresh database-verified Markov long signal supports them; starter size is capped by markov_gate.max_position_pct.",
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

async fn request_hermes_decision_advice(
    state: &AppState,
    report: &DecisionReport,
    candidates: &[CandidateOrder],
    open_codes: &[String],
    capital_budget: &CapitalBudget,
    overlay_json: &JsonValue,
) -> Result<HermesDecisionAdvice> {
    let mode = hermes_advisory_mode();
    let source_session_id = format!("decision-advice-{}", report.id);
    if !hermes_advisory_enabled() {
        return Ok(HermesDecisionAdvice::fallback(
            "disabled",
            mode,
            "Hermes Trading Manager advisory is disabled.".to_string(),
            report.id,
        ));
    }
    if let Some(existing) = state
        .hermes_decision_advice_by_session(&source_session_id)
        .await?
    {
        return Ok(HermesDecisionAdvice::from_row(
            existing,
            mode,
            source_session_id,
        ));
    }
    if let Some(existing) = state.hermes_decision_advice_by_report(report.id).await? {
        let source = text(&existing, "source_session_id");
        return Ok(HermesDecisionAdvice::from_row(
            existing,
            mode,
            if source.trim().is_empty() {
                source_session_id
            } else {
                source
            },
        ));
    }

    let Some(api_key) = hermes_gateway_api_key() else {
        return Ok(HermesDecisionAdvice::fallback(
            "not_configured",
            mode,
            "Hermes gateway API key is not configured for Trading Manager advisory.".to_string(),
            report.id,
        ));
    };

    let gateway_url = env::var("HERMES_GATEWAY_URL")
        .or_else(|_| env::var("HERMES_API_BASE_URL"))
        .unwrap_or_else(|_| "http://hermes-gateway.saxo:8642".to_string());
    let run_url = format!("{}/v1/runs", gateway_url.trim_end_matches('/'));
    let wait_seconds = env::var("HERMES_TRADING_MANAGER_ADVISORY_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(90)
        .min(180);
    let http_timeout_seconds = env::var("HERMES_TRADING_MANAGER_ADVISORY_HTTP_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .min(60);

    let candidate_payload = candidates
        .iter()
        .map(|order| {
            json!({
                "strategy_key": &order.strategy_key,
                "symbol": &order.symbol,
                "action": &order.action,
                "quantity": order.quantity,
                "order_type": &order.order_type,
                "estimated_value_dkk": order.estimated_value_dkk,
                "strategy_role": &order.strategy_role,
                "strategy_metadata": order.raw.get("strategy_metadata").cloned().unwrap_or(JsonValue::Null),
            })
        })
        .collect::<Vec<_>>();
    let input = format!(
        "Review decision report {} before the Rust Trading Manager queues orders. Use the configured daytrader MCP tools, especially get_decision_reports, get_markov_signals, get_end_of_day_reports, list_reflections, list_experiments, and create_decision_advice. Pull the latest decision report, Markov signals, EOD reports, and Hermes learnings. Then call create_decision_advice exactly once with decision_report_id {}, source_session_id {}, overall_recommendation proceed|stand_down|review, a concise summary, and per-order advice items using action allow|reduce|stand_down|review. You may only make the system more conservative: do not add trades, increase size, approve live orders, place orders, access Saxo sessions, or request secrets.",
        report.id, report.id, source_session_id
    );
    let payload = json!({
        "session_id": "saxo-daytrader-trading-manager-advice",
        "input": input,
        "instructions": "You are Hermes Agent acting as an advisory risk and learning reviewer for one saxo-rust decision report. You must produce an audited advisory record through the daytrader MCP create_decision_advice tool. Your advice is not an order and cannot approve or execute trades. Be specific, use current Markov and learning context, and only recommend proceed, stand_down, review, allow, reduce, or stand_down/review per candidate.",
        "metadata": {
            "source": "rust_trading_manager",
            "decision_report_id": report.id,
            "decision_pulse_key": report.pulse_key,
            "source_session_id": source_session_id,
            "open_exchange_codes": open_codes,
            "advisory_mode": mode,
            "capital_budget": capital_budget.to_json(),
            "strategy_experiment_overlay": overlay_json,
            "candidate_orders": candidate_payload,
        }
    });

    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(http_timeout_seconds))
        .build()
        .context("building Hermes advisory HTTP client")?;
    let submit = client
        .post(&run_url)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await;
    match submit {
        Ok(response) if response.status().is_success() => {
            info!(
                report_id = report.id,
                source_session_id, "Hermes decision advice run submitted"
            );
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(
                report_id = report.id,
                %status,
                "Hermes decision advice submit failed: {body}"
            );
            return Ok(HermesDecisionAdvice::fallback(
                "submit_failed",
                mode,
                format!("Hermes advisory submit failed with {status}: {body}"),
                report.id,
            ));
        }
        Err(err) => {
            warn!(
                report_id = report.id,
                "Hermes decision advice submit error: {err:#}"
            );
            return Ok(HermesDecisionAdvice::fallback(
                "submit_failed",
                mode,
                format!("Hermes advisory submit error: {err:#}"),
                report.id,
            ));
        }
    }

    let deadline = Utc::now() + Duration::seconds(wait_seconds as i64);
    while Utc::now() < deadline {
        if let Some(row) = state
            .hermes_decision_advice_by_session(&source_session_id)
            .await?
        {
            return Ok(HermesDecisionAdvice::from_row(row, mode, source_session_id));
        }
        if let Some(row) = state.hermes_decision_advice_by_report(report.id).await? {
            let source = text(&row, "source_session_id");
            return Ok(HermesDecisionAdvice::from_row(
                row,
                mode,
                if source.trim().is_empty() {
                    source_session_id.clone()
                } else {
                    source
                },
            ));
        }
        sleep(StdDuration::from_secs(3)).await;
    }

    // One last read after the deadline catches advice written during the final
    // sleep interval or through the report-id fallback when the session id is
    // malformed by a model/tool call.
    if let Some(row) = state
        .hermes_decision_advice_by_session(&source_session_id)
        .await?
        .or(state.hermes_decision_advice_by_report(report.id).await?)
    {
        let source = text(&row, "source_session_id");
        return Ok(HermesDecisionAdvice::from_row(
            row,
            mode,
            if source.trim().is_empty() {
                source_session_id
            } else {
                source
            },
        ));
    }

    Ok(HermesDecisionAdvice::fallback(
        "timeout",
        mode,
        format!("Hermes did not record decision advice within {wait_seconds}s."),
        report.id,
    ))
}

fn hermes_advisory_enabled() -> bool {
    env::var("HERMES_TRADING_MANAGER_ADVISORY_ENABLED")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn hermes_advisory_mode() -> String {
    match env::var("HERMES_TRADING_MANAGER_ADVISORY_MODE")
        .unwrap_or_else(|_| "record_only".to_string())
        .trim()
        .to_lowercase()
        .as_str()
    {
        "conservative" => "conservative".to_string(),
        _ => "record_only".to_string(),
    }
}

fn hermes_gateway_api_key() -> Option<String> {
    env::var("HERMES_API_SERVER_KEY")
        .or_else(|_| env::var("API_SERVER_KEY"))
        .ok()
        .or_else(|| env::var("HERMES_DAYTRADER_API_KEY").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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

fn order_shape_gate(order: &mut CandidateOrder) -> GateDecision {
    let order_type = order.order_type.trim();
    let canonical = if order_type.eq_ignore_ascii_case("market") || order_type.is_empty() {
        "Market"
    } else if order_type.eq_ignore_ascii_case("limit") {
        "Limit"
    } else if order_type.eq_ignore_ascii_case("stop") {
        "Stop"
    } else if order_type.eq_ignore_ascii_case("stoplimit")
        || order_type.eq_ignore_ascii_case("stop_limit")
        || order_type.eq_ignore_ascii_case("stop-limit")
    {
        "StopLimit"
    } else {
        return GateDecision {
            approved: false,
            reason: format!("Unsupported order_type {order_type}."),
        };
    };
    order.order_type = canonical.to_string();

    match canonical {
        "Market" => GateDecision {
            approved: true,
            reason: "Order shape is broker-compatible.".to_string(),
        },
        "Limit" => {
            if order.limit_price_local.is_none() {
                order.limit_price_local = order.price_local.filter(|price| *price > 0.0);
            }
            if order.limit_price_local.is_some_and(|price| price > 0.0) {
                GateDecision {
                    approved: true,
                    reason: "Limit order has a usable limit price.".to_string(),
                }
            } else {
                GateDecision {
                    approved: false,
                    reason:
                        "Limit orders require limit_price_local or a positive price_local fallback."
                            .to_string(),
                }
            }
        }
        "Stop" => {
            if order.stop_price_local.is_some_and(|price| price > 0.0) {
                GateDecision {
                    approved: true,
                    reason: "Stop order has a usable stop price.".to_string(),
                }
            } else {
                GateDecision {
                    approved: false,
                    reason: "Stop orders require stop_price_local.".to_string(),
                }
            }
        }
        "StopLimit" => {
            if order.limit_price_local.is_none() {
                order.limit_price_local = order.price_local.filter(|price| *price > 0.0);
            }
            if order.stop_price_local.is_none() || order.limit_price_local.is_none() {
                GateDecision {
                    approved: false,
                    reason: "StopLimit orders require stop_price_local and limit_price_local."
                        .to_string(),
                }
            } else {
                GateDecision {
                    approved: true,
                    reason: "StopLimit order has usable stop and limit prices.".to_string(),
                }
            }
        }
        _ => GateDecision {
            approved: false,
            reason: format!("Unsupported order_type {canonical}."),
        },
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

const INDICATOR_MAX_AGE_DAYS: i64 = 5;

/// Server-verified per-share price in DKK for a symbol: close from our own
/// daily indicator run (preferred) or Markov signal, converted via the
/// exchange-implied currency. None when this process has no own price data.
async fn verified_close_dkk(state: &AppState, symbol: &str) -> Option<f64> {
    let signal_close = |signal: Option<JsonValue>, key: &str| -> Option<f64> {
        let signal = signal?;
        if signal.get("status").and_then(JsonValue::as_str) != Some("ok") {
            return None;
        }
        Some(value_f64(&signal, key)).filter(|close| *close > 0.0)
    };
    let indicator = crate::daily_indicators::latest_indicator_signal(state, symbol)
        .await
        .ok()
        .flatten();
    let close = match signal_close(indicator, "close") {
        Some(close) => close,
        None => {
            let markov = latest_markov_signal(state, symbol).await.ok().flatten();
            signal_close(markov, "current_close")?
        }
    };
    let currency = crate::saxo_order::currency_for_exchange(&exchange_code(symbol).to_lowercase())?;
    let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
    Some(close * fx_rate)
}

/// Overwrite the model-claimed estimated_value_dkk on a BUY with a value
/// computed from our own price data. Model-supplied estimates have already
/// produced orders ~6x over budget by quoting USD prices as DKK.
async fn verify_buy_value(state: &AppState, order: &mut CandidateOrder) -> bool {
    let Some(per_share_dkk) = verified_close_dkk(state, &order.symbol).await else {
        return false;
    };
    let verified_value = per_share_dkk * order.quantity.max(0.0);
    let claimed = order.estimated_value_dkk;
    order.estimated_value_dkk = Some(verified_value);
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "value_verification".to_string(),
            json!({
                "verified_from_db": true,
                "per_share_dkk": per_share_dkk,
                "verified_value_dkk": verified_value,
                "model_claimed_value_dkk": claimed,
            }),
        );
    }
    true
}

/// True when a non-terminal BUY execution order for this symbol already exists
/// today (UTC); guards against duplicate starters from near-simultaneous
/// reports while still allowing retries after pre-broker validation failures.
async fn has_buy_order_today(state: &AppState, symbol: &str) -> Result<bool> {
    let today = Utc::now().format("%Y-%m-%d");
    let row = sqlx::query(&format!(
        "SELECT id FROM execution_orders
         WHERE symbol = '{}' AND action = 'BUY' AND created_at >= '{today}T00:00:00Z'
           AND COALESCE(status, '') NOT IN (
               'execution_failed',
               'broker_cancelled',
               'cancelled',
               'rejected'
           )
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

/// Replace model-supplied technical metadata with database-verified daily
/// indicator values when a fresh signal exists for the symbol. Returns true
/// when verified data was applied; the technical gate then runs on numbers
/// this process computed itself instead of model-self-reported claims.
async fn apply_verified_technical(
    state: &AppState,
    order: &mut CandidateOrder,
    today: chrono::NaiveDate,
) -> bool {
    let signal = match crate::daily_indicators::latest_indicator_signal(state, &order.symbol).await
    {
        Ok(Some(signal)) => signal,
        Ok(None) => return false,
        Err(err) => {
            warn!(
                "verified indicator lookup failed for {}: {err:#}",
                order.symbol
            );
            return false;
        }
    };
    if signal.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return false;
    }
    let Some(run_date) = signal
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
    else {
        return false;
    };
    if (today - run_date).num_days() > INDICATOR_MAX_AGE_DAYS {
        return false;
    }
    let technical = json!({
        "status": "ok",
        "source": "daily_indicators_db",
        "verified_from_db": true,
        "run_date": run_date.to_string(),
        "sentiment": signal.get("sentiment").cloned().unwrap_or(JsonValue::Null),
        "trend_bias": signal.get("trend_bias").cloned().unwrap_or(JsonValue::Null),
        "confluence_count": signal.get("confluence_count").cloned().unwrap_or(JsonValue::Null),
        "min_confluences": signal.get("min_confluences").cloned().unwrap_or(JsonValue::Null),
        "rsi14": signal.get("rsi14").cloned().unwrap_or(JsonValue::Null),
        "reward_risk": signal.get("reward_risk").cloned().unwrap_or(JsonValue::Null),
        "confluences": signal.get("confluences_json")
            .and_then(JsonValue::as_str)
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .unwrap_or(JsonValue::Null),
    });
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert("technical".to_string(), technical);
        return true;
    }
    false
}

/// Latest Markov regime signal for one symbol from the most recent run.
/// Queried server-side so the gate never trusts model-reported signal values.
async fn latest_markov_signal(state: &AppState, symbol: &str) -> Result<Option<JsonValue>> {
    let sql = format!(
        "SELECT run_date, status, current_state, current_close, signed_signal, direction, conviction
         FROM markov_asset_signals
         WHERE symbol = '{}' AND run_id = (
            SELECT id FROM markov_signal_runs ORDER BY run_date DESC, created_at DESC LIMIT 1
         )
         LIMIT 1",
        sql_escape(symbol)
    );
    let row = sqlx::query(&sql).fetch_optional(&state.pool).await?;
    Ok(row.as_ref().map(row_to_json))
}

/// Fallback BUY gate: a fresh database-verified Markov long signal admits a
/// size-capped starter position when daily technical confluence is missing.
fn markov_buy_gate(
    order: &mut CandidateOrder,
    evidence: Option<&JsonValue>,
    config: MarkovGateConfig,
    total_market_value_dkk: f64,
    today: chrono::NaiveDate,
) -> GateDecision {
    let Some(evidence) = evidence else {
        return GateDecision {
            approved: false,
            reason: "No Markov regime signal is available for this symbol.".to_string(),
        };
    };
    if evidence.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return GateDecision {
            approved: false,
            reason: "Latest Markov regime signal for this symbol did not complete.".to_string(),
        };
    }
    let run_date = evidence
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    let Some(run_date) = run_date else {
        return GateDecision {
            approved: false,
            reason: "Markov regime signal has no parsable run date.".to_string(),
        };
    };
    let age_days = (today - run_date).num_days();
    if age_days > config.max_signal_age_days {
        return GateDecision {
            approved: false,
            reason: format!(
                "Markov regime signal from {run_date} is {age_days} days old (max {}).",
                config.max_signal_age_days
            ),
        };
    }
    let direction = evidence
        .get("direction")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let signed_signal = value_f64(evidence, "signed_signal");
    if direction != "long" || signed_signal < config.min_signed_signal {
        return GateDecision {
            approved: false,
            reason: format!(
                "Markov signal {signed_signal:.2} ({direction}) does not meet the long threshold {:.2}.",
                config.min_signed_signal
            ),
        };
    }

    // Cap the starter position to a small share of the portfolio.
    let max_value_dkk = total_market_value_dkk * config.max_position_pct;
    let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    let mut scaled = false;
    if max_value_dkk > 0.0 && estimated_value_dkk > max_value_dkk {
        let factor = max_value_dkk / estimated_value_dkk;
        let scaled_quantity = (order.quantity * factor).floor();
        if scaled_quantity < 1.0 {
            return GateDecision {
                approved: false,
                reason: format!(
                    "Starter cap {max_value_dkk:.0} DKK is below the value of a single share."
                ),
            };
        }
        let quantity_ratio = scaled_quantity / order.quantity;
        order.quantity = scaled_quantity;
        order.estimated_value_dkk = Some(estimated_value_dkk * quantity_ratio);
        scaled = true;
    }
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "markov_gate".to_string(),
            json!({
                "verified_from_db": true,
                "run_date": run_date.to_string(),
                "signed_signal": signed_signal,
                "direction": direction,
                "state": evidence.get("current_state").cloned().unwrap_or(JsonValue::Null),
                "min_signed_signal": config.min_signed_signal,
                "max_position_pct": config.max_position_pct,
                "size_capped": scaled,
            }),
        );
    }
    GateDecision {
        approved: true,
        reason: format!(
            "Starter BUY approved by database-verified Markov long signal {signed_signal:.2} (state {}); position capped at {:.0}% of portfolio{}.",
            evidence
                .get("current_state")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown"),
            config.max_position_pct * 100.0,
            if scaled {
                ", quantity scaled down to fit"
            } else {
                ""
            }
        ),
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
    if broker_position_snapshots_available(state).await? {
        let row = sqlx::query(&format!(
            "SELECT COALESCE(SUM(quantity), 0) AS quantity
             FROM broker_position_snapshots
             WHERE symbol = '{}'",
            sql_escape(symbol)
        ))
        .fetch_optional(&state.pool)
        .await?;
        return Ok(row
            .and_then(|row| row.try_get::<f64, _>("quantity").ok())
            .unwrap_or(0.0));
    }

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

async fn latest_sellable_position_quantity(state: &AppState, symbol: &str) -> Result<f64> {
    if broker_position_snapshots_available(state).await? {
        let row = sqlx::query(&format!(
            "SELECT COALESCE(SUM(quantity), 0) AS quantity
             FROM broker_position_snapshots
             WHERE symbol = '{}'
               AND COALESCE(can_be_closed, 1) <> 0",
            sql_escape(symbol)
        ))
        .fetch_optional(&state.pool)
        .await?;
        return Ok(row
            .and_then(|row| row.try_get::<f64, _>("quantity").ok())
            .unwrap_or(0.0));
    }
    latest_position_quantity(state, symbol).await
}

async fn broker_position_snapshots_available(state: &AppState) -> Result<bool> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM broker_position_snapshots")
        .fetch_optional(&state.pool)
        .await?;
    Ok(row
        .and_then(|row| row.try_get::<i64, _>("count").ok())
        .unwrap_or(0)
        > 0)
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
    let reinvestment_pressure_threshold_pct = cash_policy
        .get("reinvestment_pressure_threshold_pct")
        .map(|_| value_f64(&cash_policy, "reinvestment_pressure_threshold_pct"))
        .unwrap_or(0.05)
        .max(0.0);
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
    let cash_pct = if total_market_value_dkk > 0.0 {
        cash_balance_dkk / total_market_value_dkk
    } else {
        0.0
    };
    let excess_cash_pct = (cash_pct - min_cash_buffer_pct).max(0.0);
    let reinvestment_pressure_active =
        excess_cash_pct >= reinvestment_pressure_threshold_pct && available_buy_budget_dkk > 0.0;
    CapitalBudget {
        cash_balance_dkk,
        total_market_value_dkk,
        invested_market_value_dkk,
        cash_pct,
        min_cash_buffer_pct,
        max_deployment_pct,
        reinvestment_pressure_threshold_pct,
        required_cash_buffer_dkk,
        available_cash_above_buffer_dkk,
        available_buy_budget_dkk,
        remaining_deployment_capacity_dkk,
        excess_cash_pct,
        reinvestment_pressure_active,
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
            "cash_pct": self.cash_pct,
            "min_cash_buffer_pct": self.min_cash_buffer_pct,
            "max_deployment_pct": self.max_deployment_pct,
            "reinvestment_pressure_threshold_pct": self.reinvestment_pressure_threshold_pct,
            "required_cash_buffer_dkk": self.required_cash_buffer_dkk,
            "available_cash_above_buffer_dkk": self.available_cash_above_buffer_dkk,
            "available_buy_budget_dkk": self.available_buy_budget_dkk,
            "remaining_deployment_capacity_dkk": self.remaining_deployment_capacity_dkk,
            "excess_cash_pct": self.excess_cash_pct,
            "reinvestment_pressure_active": self.reinvestment_pressure_active,
        })
    }
}

fn reinvestment_diagnostics(
    budget: &CapitalBudget,
    candidate_order_count: usize,
    buy_candidate_count: usize,
    sell_candidate_count: usize,
    approved_buy_count: usize,
    approved_sell_count: usize,
    skipped_buy_count: usize,
    skipped_sell_count: usize,
) -> JsonValue {
    let status = if !budget.reinvestment_pressure_active {
        "within_policy"
    } else if buy_candidate_count == 0 {
        "excess_cash_without_buy_candidates"
    } else if approved_buy_count == 0 {
        "excess_cash_with_blocked_buy_candidates"
    } else {
        "reinvestment_candidates_approved"
    };
    json!({
        "status": status,
        "active": budget.reinvestment_pressure_active,
        "cash_balance_dkk": budget.cash_balance_dkk,
        "cash_pct": budget.cash_pct,
        "min_cash_buffer_pct": budget.min_cash_buffer_pct,
        "excess_cash_pct": budget.excess_cash_pct,
        "available_buy_budget_dkk": budget.available_buy_budget_dkk,
        "threshold_pct": budget.reinvestment_pressure_threshold_pct,
        "candidate_order_count": candidate_order_count,
        "buy_candidate_count": buy_candidate_count,
        "sell_candidate_count": sell_candidate_count,
        "approved_buy_count": approved_buy_count,
        "approved_sell_count": approved_sell_count,
        "skipped_buy_count": skipped_buy_count,
        "skipped_sell_count": skipped_sell_count,
        "message": match status {
            "excess_cash_without_buy_candidates" => "Cash is above policy, but the decision report supplied no BUY candidates.",
            "excess_cash_with_blocked_buy_candidates" => "Cash is above policy, but BUY candidates were blocked by exchange, budget, risk, minimum value, or technical gates.",
            "reinvestment_candidates_approved" => "Cash is above policy and at least one BUY candidate was approved for queueing.",
            _ => "Cash is inside the configured policy band or no deployment capacity is available.",
        }
    })
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
    fn hermes_decision_advice_matches_strategy_key_before_symbol_side() {
        let row = json!({
            "status": "received",
            "overall_recommendation": "proceed",
            "summary": "Proceed with one reduction.",
            "order_advice_json": [
                {
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "stand_down",
                    "reason": "Symbol fallback"
                },
                {
                    "strategy_key": "test:BUY",
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "reduce",
                    "max_quantity": 2,
                    "reason": "Strategy-specific reduction"
                }
            ],
            "learning_notes_json": []
        });
        let advice = HermesDecisionAdvice::from_row(
            row,
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );
        let matched = advice
            .for_order(&order("BUY", "BUY", "bullish", 4))
            .unwrap();

        assert_eq!(matched.action, "reduce");
        assert_eq!(matched.max_quantity, Some(2.0));
        assert_eq!(matched.reason, "Strategy-specific reduction");
    }

    #[test]
    fn hermes_conservative_timeout_requires_review() {
        let advice = HermesDecisionAdvice::fallback(
            "timeout",
            "conservative".to_string(),
            "Hermes timed out.".to_string(),
            42,
        );

        assert_eq!(advice.overall_recommendation, "review");
        assert_eq!(
            advice
                .raw
                .get("overall_recommendation")
                .and_then(JsonValue::as_str),
            Some("review")
        );
    }

    #[test]
    fn hermes_record_only_timeout_proceeds_for_audit_only() {
        let advice = HermesDecisionAdvice::fallback(
            "timeout",
            "record_only".to_string(),
            "Hermes timed out.".to_string(),
            42,
        );

        assert_eq!(advice.overall_recommendation, "proceed");
    }

    #[test]
    fn hermes_review_allows_explicit_order_advice_to_drive_decision() {
        let row = json!({
            "status": "received",
            "overall_recommendation": "review",
            "summary": "Review report, but allow NVDA.",
            "order_advice_json": [
                {
                    "strategy_key": "test:BUY",
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "allow",
                    "reason": "Covered by Hermes."
                }
            ],
            "learning_notes_json": []
        });
        let advice = HermesDecisionAdvice::from_row(
            row,
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        assert_eq!(advice.overall_recommendation, "review");
        assert_eq!(
            advice
                .for_order(&order("BUY", "BUY", "bullish", 4))
                .unwrap()
                .action,
            "allow"
        );
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
    fn rejects_limit_order_without_limit_price() {
        let mut order = CandidateOrder::from_json(json!({
            "symbol": "GE:xnys",
            "action": "BUY",
            "quantity": 1,
            "order_type": "Limit",
            "estimated_value_dkk": 2300,
            "strategy_key": "test:limit-missing"
        }))
        .unwrap();

        let gate = order_shape_gate(&mut order);

        assert!(!gate.approved);
        assert!(gate.reason.contains("Limit orders require"));
    }

    #[test]
    fn uses_price_local_as_limit_price_fallback() {
        let mut order = CandidateOrder::from_json(json!({
            "symbol": "GE:xnys",
            "action": "BUY",
            "quantity": 1,
            "order_type": "limit",
            "price_local": 325.25,
            "estimated_value_dkk": 2300,
            "strategy_key": "test:limit-price-fallback"
        }))
        .unwrap();

        let gate = order_shape_gate(&mut order);

        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.order_type, "Limit");
        assert_eq!(order.limit_price_local, Some(325.25));
    }

    #[test]
    fn experiment_overlay_can_adjust_min_confluences() {
        assert!(!technical_gate(&order("BUY", "BUY", "bullish", 2), None).approved);
        assert!(technical_gate(&order("BUY", "BUY", "bullish", 2), Some(2)).approved);
    }

    fn markov_test_config() -> MarkovGateConfig {
        MarkovGateConfig {
            enabled: true,
            min_signed_signal: 0.15,
            max_position_pct: 0.05,
            max_signal_age_days: 5,
        }
    }

    fn buy_order(quantity: f64, estimated_value_dkk: f64) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "AMD:xnas",
            "action": "BUY",
            "quantity": quantity,
            "order_type": "Market",
            "estimated_value_dkk": estimated_value_dkk,
            "strategy_key": "test:starter",
            "strategy_role": "starter"
        }))
        .unwrap()
    }

    fn markov_evidence(signed_signal: f64, direction: &str, run_date: &str) -> JsonValue {
        json!({
            "status": "ok",
            "run_date": run_date,
            "current_state": "Bull",
            "signed_signal": signed_signal,
            "direction": direction,
            "conviction": signed_signal
        })
    }

    #[test]
    fn markov_gate_approves_fresh_long_signal() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let mut order = buy_order(5.0, 10_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(gate.approved, "{}", gate.reason);
        // 10k is below the 13.3k cap: quantity untouched.
        assert_eq!(order.quantity, 5.0);
        let recorded = order.raw["strategy_metadata"]["markov_gate"].clone();
        assert_eq!(recorded["verified_from_db"], json!(true));
        assert_eq!(recorded["size_capped"], json!(false));
    }

    #[test]
    fn markov_gate_rejects_weak_or_short_signals() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let config = markov_test_config();
        let weak = markov_evidence(0.10, "long", "2026-06-08");
        let short = markov_evidence(0.40, "short", "2026-06-08");
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                Some(&weak),
                config,
                266_000.0,
                today
            )
            .approved
        );
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                Some(&short),
                config,
                266_000.0,
                today
            )
            .approved
        );
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                None,
                config,
                266_000.0,
                today
            )
            .approved
        );
    }

    #[test]
    fn markov_gate_rejects_stale_signal() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let stale = markov_evidence(0.60, "long", "2026-06-01");
        let gate = markov_buy_gate(
            &mut buy_order(5.0, 10_000.0),
            Some(&stale),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(!gate.approved);
        assert!(gate.reason.contains("days old"), "{}", gate.reason);
    }

    #[test]
    fn markov_gate_scales_oversized_orders_to_cap() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        // 40k requested vs 13.3k cap (5% of 266k): expect quantity scaled from 20 to 6.
        let mut order = buy_order(20.0, 40_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.quantity, 6.0);
        let estimated = order.estimated_value_dkk.unwrap();
        assert!((estimated - 12_000.0).abs() < 1.0, "estimated {estimated}");
        assert_eq!(
            order.raw["strategy_metadata"]["markov_gate"]["size_capped"],
            json!(true)
        );
    }

    #[test]
    fn markov_gate_rejects_when_one_share_exceeds_cap() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        // Single share worth 20k against a 13.3k cap cannot be scaled down.
        let mut order = buy_order(1.0, 20_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(!gate.approved);
        assert!(gate.reason.contains("single share"), "{}", gate.reason);
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
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let mut budget = capital_budget_from_overview(&overview, None);
        assert_eq!(budget.required_cash_buffer_dkk, 30000.0);
        assert_eq!(budget.available_buy_budget_dkk, 20000.0);
        assert!(budget.reinvestment_pressure_active);
        budget.reserve_buy(7500.0);
        assert_eq!(budget.available_buy_budget_dkk, 12500.0);
    }

    #[test]
    fn reinvestment_diagnostics_flags_missing_buy_candidates() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 220000.0,
                "cash_balance_dkk": 80000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let budget = capital_budget_from_overview(&overview, None);
        let diagnostics = reinvestment_diagnostics(&budget, 1, 0, 1, 0, 1, 0, 0);
        assert_eq!(
            diagnostics["status"],
            JsonValue::from("excess_cash_without_buy_candidates")
        );
        assert_eq!(diagnostics["active"], JsonValue::from(true));
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
