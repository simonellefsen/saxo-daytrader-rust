use std::collections::HashSet;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tracing::{info, warn};

use crate::{
    config::{yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64, value_i64},
    state::AppState,
};

const DEFAULT_DUE_WINDOW_MINUTES: i64 = 20;
const DEFAULT_MINUTES_AFTER_OPEN: i64 = 75;

#[derive(Clone, Debug)]
struct DecisionPulse {
    key: String,
    label: String,
    kind: String,
    target_at_utc: String,
    exchange_codes: Vec<String>,
    source_markets: Vec<String>,
}

#[derive(Clone, Debug)]
struct PendingDeferredReport {
    id: i64,
    request_id: String,
    request_json: JsonValue,
    report_json: JsonValue,
}

/// One scheduler step for AI decision reports.
///
/// xAI is treated as a background job system: submit with `deferred: true`, save
/// the request id in Postgres, and poll on later scheduler cycles. OpenRouter is
/// handled as a synchronous Chat Completions provider and inserted as a completed
/// report immediately.
pub async fn run_xai_decision_cycle(state: &AppState) -> Result<JsonValue> {
    let polled = poll_pending_deferred_reports(state).await?;
    let submitted = submit_due_scheduled_reports(state).await?;
    Ok(json!({
        "status": "ok",
        "polled": polled,
        "submitted": submitted,
    }))
}

pub async fn submit_manual_decision_report(state: &AppState) -> Result<JsonValue> {
    let pulse = DecisionPulse {
        key: format!("manual:{}", Utc::now().format("%Y-%m-%dT%H:%M:%SZ")),
        label: "Manual Decision Report".to_string(),
        kind: "manual".to_string(),
        target_at_utc: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        exchange_codes: Vec::new(),
        source_markets: Vec::new(),
    };
    submit_deferred_report(state, &pulse, true).await
}

async fn submit_due_scheduled_reports(state: &AppState) -> Result<Vec<JsonValue>> {
    if let Err(err) = state.refresh_saxo_exchange_calendars_if_stale().await {
        warn!("xAI decision scheduler using fallback exchange calendar: {err:#}");
    }
    let pulses = active_decision_pulses(state);
    if pulses.is_empty() {
        return Ok(Vec::new());
    }
    let mut submitted = Vec::new();
    for pulse in pulses {
        if has_report_for_pulse(state, &pulse.key).await? {
            submitted.push(json!({
                "status": "already_exists",
                "pulse_key": pulse.key,
                "pulse_label": pulse.label,
            }));
            continue;
        }
        submitted.push(submit_deferred_report(state, &pulse, false).await?);
    }
    Ok(submitted)
}

async fn submit_deferred_report(
    state: &AppState,
    pulse: &DecisionPulse,
    manual: bool,
) -> Result<JsonValue> {
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let prompt = build_decision_prompt(state, pulse, manual).await?;
    let provider = ai_provider(state);
    let model = state.effective_xai_model().await?;
    let request_json = build_chat_request(state, &prompt, &model)?;

    let Some(api_key) = ai_api_key(state) else {
        let report = insert_xai_error_report(
            state,
            &created_at,
            pulse,
            &model,
            &prompt,
            &request_json,
            &format!(
                "{} is missing; decision report was not submitted.",
                ai_api_key_env_name(state)
            ),
        )
        .await?;
        warn!(pulse_key = %pulse.key, provider = %provider, "AI decision report submit skipped because API key is missing");
        return Ok(report);
    };

    let base_url = ai_base_url(state);
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(xai_http_timeout_seconds(state)))
        .build()
        .context("building AI provider HTTP client")?;
    let mut outbound_request = request_json.clone();
    if provider == "xai" {
        if let Some(obj) = outbound_request.as_object_mut() {
            obj.insert("deferred".to_string(), JsonValue::from(true));
        }
    }
    let response = client
        .post(format!("{base_url}/chat/completions"))
        .bearer_auth(api_key)
        .json(&outbound_request)
        .send()
        .await
        .context("submitting AI decision report")?;
    let status = response.status();
    let response_body = response
        .text()
        .await
        .unwrap_or_else(|err| format!("failed to read AI provider response body: {err}"));
    if !status.is_success() {
        let response_excerpt = truncate_error_text(&response_body, 2_000);
        let report = insert_xai_error_report(
            state,
            &created_at,
            pulse,
            &model,
            &prompt,
            &outbound_request,
            &format!("{provider} decision submit failed with HTTP {status}: {response_excerpt}"),
        )
        .await?;
        warn!(
            pulse_key = %pulse.key,
            provider = %provider,
            status = %status,
            "AI decision report submit failed"
        );
        return Ok(report);
    }
    let response_json: JsonValue = match serde_json::from_str(&response_body) {
        Ok(value) => value,
        Err(err) => {
            let response_excerpt = truncate_error_text(&response_body, 2_000);
            let report = insert_xai_error_report(
                state,
                &created_at,
                pulse,
                &model,
                &prompt,
                &outbound_request,
                &format!(
                    "{provider} decision submit returned invalid JSON despite HTTP {status}: {err}; response excerpt: {response_excerpt}"
                ),
            )
            .await?;
            warn!(
                pulse_key = %pulse.key,
                provider = %provider,
                status = %status,
                error = %err,
                "AI decision report submit returned invalid JSON"
            );
            return Ok(report);
        }
    };
    if provider != "xai" {
        let response_id = response_json
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let seed_report = json!({
            "created_at": created_at,
            "analysis_pulse": pulse_to_json(pulse)
        });
        let report_json = completed_report_json_from_parts(
            &outbound_request,
            &seed_report,
            &response_json,
            "openrouter",
            json!({
                "response_id": response_id,
                "completed_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "mode": "chat_completion"
            }),
        )?;
        let row = insert_decision_report(
            state,
            &created_at,
            pulse,
            model.clone(),
            "completed",
            Some(response_id),
            &prompt,
            &outbound_request,
            Some(&response_json),
            &report_json,
            None,
        )
        .await?;
        info!(
            pulse_key = %pulse.key,
            provider = %provider,
            response_id = %response_id,
            "completed AI decision report"
        );
        return Ok(row);
    }
    let request_id = response_json
        .get("request_id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("xAI deferred submit response did not include request_id"))?;

    let report_json = json!({
        "status": "xai_deferred",
        "created_at": created_at,
        "report_title": pulse.label,
        "analysis_pulse": pulse_to_json(pulse),
        "xai_deferred": {
            "request_id": request_id,
            "submitted_at": created_at,
            "poll_url": format!("{base_url}/chat/deferred-completion/{request_id}"),
            "mode": "deferred_chat_completion"
        },
        "strategy_plan": {
            "status": "xai_deferred",
            "selected_assets": [],
            "swing_orders": [],
            "suggested_trades": [],
            "notes": ["Waiting for xAI deferred completion before strategy planning."]
        },
        "suggested_trades": [],
        "execution_notes": [
            "Deferred xAI request submitted. The scheduler will poll for completion.",
            "The Trading Manager will only act after this report becomes completed."
        ]
    });
    let row = insert_decision_report(
        state,
        &created_at,
        pulse,
        model,
        "xai_deferred",
        Some(request_id),
        &prompt,
        &outbound_request,
        Some(&response_json),
        &report_json,
        None,
    )
    .await?;
    info!(
        report_id = row.get("id").and_then(JsonValue::as_i64).unwrap_or(0),
        pulse_key = %pulse.key,
        request_id,
        "submitted deferred xAI decision report"
    );
    Ok(row)
}

async fn poll_pending_deferred_reports(state: &AppState) -> Result<Vec<JsonValue>> {
    if ai_provider(state) != "xai" {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, request_json, response_json, report_json, response_id
         FROM decision_reports
         WHERE status = 'xai_deferred'
         ORDER BY created_at ASC, id ASC
         LIMIT 10",
    )
    .fetch_all(&state.pool)
    .await
    .context("loading pending xAI deferred reports")?;
    let mut output = Vec::new();
    for row in rows.iter().map(row_to_json) {
        let pending = decode_pending_report(&row)?;
        match poll_one_deferred_report(state, &pending).await {
            Ok(value) => output.push(value),
            Err(err) => {
                warn!(report_id = pending.id, "xAI deferred poll failed: {err:#}");
                output.push(json!({
                    "status": "error",
                    "report_id": pending.id,
                    "request_id": pending.request_id,
                    "error": err.to_string()
                }));
            }
        }
    }
    Ok(output)
}

async fn poll_one_deferred_report(
    state: &AppState,
    pending: &PendingDeferredReport,
) -> Result<JsonValue> {
    let Some(api_key) = yaml_string(&state.config, &["xai", "api_key"]) else {
        let key_name = ai_api_key_env_name(state);
        return Ok(json!({
            "status": "pending",
            "report_id": pending.id,
            "request_id": pending.request_id,
            "reason": format!("{key_name} is missing")
        }));
    };
    let base_url = xai_base_url(state);
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(xai_http_timeout_seconds(state)))
        .build()
        .context("building xAI HTTP client")?;
    let response = client
        .get(format!(
            "{base_url}/chat/deferred-completion/{}",
            pending.request_id
        ))
        .bearer_auth(api_key)
        .send()
        .await
        .context("polling xAI deferred decision report")?;
    if response.status() == StatusCode::ACCEPTED {
        return Ok(json!({
            "status": "pending",
            "report_id": pending.id,
            "request_id": pending.request_id
        }));
    }
    let status = response.status();
    let response_body = response
        .text()
        .await
        .unwrap_or_else(|err| format!("failed to read xAI deferred response body: {err}"));
    if !status.is_success() {
        mark_deferred_report_error(
            state,
            pending.id,
            &format!("xAI deferred poll failed with HTTP {status}: {response_body}"),
        )
        .await?;
        return Ok(json!({
            "status": "error",
            "report_id": pending.id,
            "request_id": pending.request_id,
            "http_status": status.as_u16()
        }));
    }
    let response_json: JsonValue =
        serde_json::from_str(&response_body).context("parsing xAI deferred completion response")?;
    let report_json = completed_report_json(pending, &response_json)?;
    update_completed_report(state, pending.id, &response_json, &report_json).await?;
    info!(
        report_id = pending.id,
        request_id = pending.request_id,
        response_id = response_json
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or(""),
        "completed xAI deferred decision report"
    );
    Ok(json!({
        "status": "completed",
        "report_id": pending.id,
        "request_id": pending.request_id,
        "response_id": response_json.get("id").cloned().unwrap_or(JsonValue::Null)
    }))
}

fn completed_report_json(
    pending: &PendingDeferredReport,
    response_json: &JsonValue,
) -> Result<JsonValue> {
    completed_report_json_from_parts(
        &pending.request_json,
        &pending.report_json,
        response_json,
        "xai_deferred",
        json!({
            "request_id": pending.request_id,
            "completed_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }),
    )
}

fn completed_report_json_from_parts(
    request_json: &JsonValue,
    report_json: &JsonValue,
    response_json: &JsonValue,
    provider_key: &str,
    provider_metadata: JsonValue,
) -> Result<JsonValue> {
    let content = response_json
        .get("choices")
        .and_then(JsonValue::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| anyhow!("AI completion did not include message.content"))?;
    let mut parsed = parse_json_content(content).context("parsing xAI decision report JSON")?;
    let requested_capital_plan = request_capital_plan(request_json);
    let created_at = report_json
        .get("created_at")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let pulse = report_json
        .get("analysis_pulse")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let scope_enforcement = enforce_completed_report_scope(&mut parsed, &pulse);
    if let Some(obj) = parsed.as_object_mut() {
        obj.insert("status".to_string(), JsonValue::from("completed"));
        obj.entry("created_at".to_string())
            .or_insert_with(|| JsonValue::from(created_at));
        obj.entry("analysis_pulse".to_string()).or_insert(pulse);
        obj.insert("market_scope_enforcement".to_string(), scope_enforcement);
        obj.insert(provider_key.to_string(), provider_metadata);
        if !obj.contains_key("strategy_plan") {
            let suggested = obj
                .get("suggested_trades")
                .cloned()
                .unwrap_or_else(|| json!([]));
            obj.insert(
                "strategy_plan".to_string(),
                json!({
                    "mode": "swing",
                    "status": "completed",
                    "swing_orders": suggested,
                    "suggested_trades": obj.get("suggested_trades").cloned().unwrap_or_else(|| json!([])),
                    "notes": ["Strategy plan was normalized by the Rust xAI deferred completion poller."]
                }),
            );
        }
        if let Some(capital_plan) = requested_capital_plan {
            obj.entry("capital_plan".to_string())
                .or_insert_with(|| capital_plan.clone());
            if let Some(plan) = obj
                .get_mut("strategy_plan")
                .and_then(JsonValue::as_object_mut)
            {
                plan.entry("capital_plan".to_string())
                    .or_insert(capital_plan);
            }
        }
    }
    Ok(parsed)
}

fn request_capital_plan(request_json: &JsonValue) -> Option<JsonValue> {
    request_json
        .get("messages")
        .and_then(JsonValue::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    message
                        .get("content")
                        .and_then(JsonValue::as_str)
                        .and_then(|content| serde_json::from_str::<JsonValue>(content).ok())
                })
                .find_map(|payload| payload.get("capital_plan").cloned())
        })
        .or_else(|| request_json.get("capital_plan").cloned())
}

fn enforce_completed_report_scope(report: &mut JsonValue, pulse: &JsonValue) -> JsonValue {
    let kind = pulse.get("kind").and_then(JsonValue::as_str).unwrap_or("");
    if kind != "europe_open_followup" {
        return json!({"status": "not_required"});
    }
    let allowed = pulse
        .get("exchange_codes")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(|value| value.to_uppercase()))
        .collect::<HashSet<_>>();
    if allowed.is_empty() {
        return json!({"status": "no_allowed_exchange_codes"});
    }
    let mut filtered = Vec::new();
    filter_report_array(report, "suggested_trades", &allowed, &mut filtered);
    filter_report_array(report, "selected_assets", &allowed, &mut filtered);
    filter_report_array(report, "candidate_assets", &allowed, &mut filtered);
    filter_report_array(report, "symbol_sentiment", &allowed, &mut filtered);
    if let Some(plan) = report.get_mut("strategy_plan") {
        filter_report_array(plan, "swing_orders", &allowed, &mut filtered);
        filter_report_array(plan, "suggested_trades", &allowed, &mut filtered);
    }
    filtered.sort();
    filtered.dedup();
    json!({
        "status": "enforced",
        "allowed_exchange_codes": allowed.into_iter().collect::<Vec<_>>(),
        "filtered_out_symbols": filtered,
    })
}

fn filter_report_array(
    object: &mut JsonValue,
    key: &str,
    allowed: &HashSet<String>,
    filtered: &mut Vec<String>,
) {
    let Some(array) = object.get_mut(key).and_then(JsonValue::as_array_mut) else {
        return;
    };
    array.retain(|row| {
        let symbol = text(row, "symbol");
        let code = symbol_exchange_code(&symbol);
        let keep = code.is_empty() || allowed.contains(&code);
        if !keep {
            filtered.push(symbol);
        }
        keep
    });
}

fn parse_json_content(content: &str) -> Result<JsonValue> {
    let trimmed = content.trim();
    if let Ok(value) = serde_json::from_str::<JsonValue>(trimmed) {
        return Ok(value);
    }
    let without_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    Ok(serde_json::from_str::<JsonValue>(without_fence)?)
}

async fn build_decision_prompt(
    state: &AppState,
    pulse: &DecisionPulse,
    manual: bool,
) -> Result<JsonValue> {
    let market = state
        .market_status_payload()
        .await
        .unwrap_or_else(|_| json!({}));
    let market_items = market
        .get("items")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let scope = market_scope_for_pulse(pulse, &market_items, manual);
    let allowed_codes = scope
        .get("allowed_trade_exchange_codes")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(|value| value.to_uppercase()))
        .collect::<HashSet<_>>();
    let positions = filter_rows_by_exchange(
        state.position_items(250).await.unwrap_or_default(),
        &allowed_codes,
    );
    let watchlists = state
        .watchlists_payload()
        .await
        .unwrap_or_else(|_| json!({}));
    let overview = state.overview_payload().await.unwrap_or_else(|_| json!({}));
    let active_strategy_baseline = state
        .active_strategy_baseline()
        .await
        .unwrap_or(JsonValue::Null);
    let markov_method = crate::markov_method::compact_markov_context(state, 80)
        .await
        .unwrap_or_else(|_| json!({"signals": []}));
    let daily_indicators = crate::daily_indicators::compact_indicator_context(state, 80)
        .await
        .unwrap_or_else(|_| json!({"latest_run": null, "signals": []}));
    let capital_context = capital_planning_context(&overview);
    let markov_gate = crate::trading_manager::markov_gate_config(state);
    let markov_buy_instruction = format!(
        "When daily technical indicator data is unavailable for a BUY candidate, you may still propose a starter BUY backed by the supplied markov_method signals: the symbol must have a fresh signal with direction long and signed_signal at or above {:.2}. Set strategy_role to \"starter\" and reference the signal in strategy_metadata.markov. The manager re-verifies the signal against its own database and caps starter positions at {:.0}% of total portfolio value, so prefer several smaller starters over one large order.",
        markov_gate.min_signed_signal,
        markov_gate.max_position_pct * 100.0
    );
    let system = [
        "You are the portfolio decision engine for a Danish SaxoInvestor swing/day-trading system.",
        "Return strict JSON only. No markdown, no prose outside JSON.",
        "Use the sentiment scale SELL, UNDERWEIGHT, HOLD, OVERWEIGHT, BUY.",
        "Never short. Treat all pnl, commissions, and taxes in DKK where possible.",
        "Always assess available cash before recommending BUY orders. Preserve the configured cash buffer and do not rely on margin.",
        "When reinvestment_pressure.active is true, explicitly decide whether to redeploy excess cash, wait in cash, or rotate risk. If qualifying Markov-backed starter candidates exist, prefer proposing capped starter BUYs over waiting in cash; only wait when no candidate qualifies, and explain the blocker in capital_plan.cash_policy with watched candidates in capital_plan.near_term_opportunities.",
        "Think in two horizons: near-term opportunities for the next 2 weeks, and medium-term opportunities for the next 1-3 months.",
        "Use selected_assets and symbol_sentiment to document forward-looking opportunities even when they are not tradable or actionable today.",
        "Suggested trades must be conservative and include strategy_metadata.technical when available.",
        "Only put a symbol in suggested_trades when its exchange is currently tradable under the supplied market_scope.",
        "Only put BUY trades in suggested_trades when the trade fits inside capital_plan.available_buy_budget_dkk after preserving the cash buffer.",
        "Prices in daily_indicators and markov_method are quoted in each instrument's trading currency; use the supplied close_dkk (close converted to DKK) when sizing orders: estimated_value_dkk must equal quantity times close_dkk. Instruments on XNAS/XNYS trade in USD, not DKK. The manager recomputes every BUY value from its own data and downsizes oversized orders to fit the budget.",
        "If order_type is Limit, include limit_price_local in the instrument's trading currency. Use Market when no explicit limit price is intended.",
        "For BUY trades backed by technical indicator data, strategy_metadata.technical must support the action with BUY or OVERWEIGHT sentiment, bullish trend_bias, and enough confluences.",
        "The supplied daily_indicators section contains technical data (SMA trend, RSI, MACD, ATR reward/risk, confluence counts) computed by the runtime from broker chart history. Base technical claims on this data; the manager re-verifies every order against its own indicator database, so fabricated confluence counts are discarded.",
        markov_buy_instruction.as_str(),
        "For SELL trades, strategy_metadata.technical must support the action with SELL or UNDERWEIGHT sentiment, bearish trend_bias, or an explicit FLATTEN/risk-reduction role justified by portfolio risk.",
        "Markov method regime signals also serve as general directional context: positive bull_prob-minus-bear_prob supports long bias, negative signal supports risk reduction or stand-down.",
        "Each suggested trade must use a unique strategy_key that includes the pulse key, symbol, and action.",
        "When active_strategy_baseline is present, include its id in strategy_baseline_id and explain how the decision stays consistent with or intentionally departs from that baseline.",
    ]
    .join("\n");
    let user_payload = json!({
        "task": if manual { "Generate an operator-triggered decision report." } else { "Generate a scheduled decision report for the active market pulse." },
        "market_scope": scope,
        "required_json_shape": {
            "report_title": "string",
            "market_view": {"bias": "string", "summary": "string"},
            "reasoning_steps": ["string"],
            "capital_plan": {"cash_balance_dkk": "number", "available_buy_budget_dkk": "number", "cash_policy": "string", "reinvestment_decision": "redeploy|wait|risk_reduce", "near_term_opportunities": ["string"], "medium_term_watchlist": ["string"]},
            "selected_assets": [{"symbol": "string", "score": "number", "notes": "string"}],
            "symbol_sentiment": [{"symbol": "string", "sentiment": "SELL|UNDERWEIGHT|HOLD|OVERWEIGHT|BUY", "confidence": "number", "rationale": "string"}],
            "suggested_trades": [{"symbol": "string", "action": "BUY|SELL", "quantity": "number", "order_type": "Market|Limit", "limit_price_local": "number|null; required when order_type is Limit", "estimated_value_dkk": "number", "strategy_key": "string", "strategy_role": "string", "strategy_metadata": {"technical": {"status": "ok|missing", "sentiment": "string", "trend_bias": "bullish|neutral|bearish", "confluence_count": "number", "min_confluences": "number"}, "markov": {"signed_signal": "number", "direction": "long|short", "state": "string", "run_date": "string"}}}],
            "strategy_baseline_id": "string|null",
            "strategy_status": "string",
            "strategy_flow": {"portfolio": "number", "selected": "number", "trades": "number"}
        },
        "pulse": pulse_to_json(pulse),
        "portfolio_summary": overview.get("portfolio_summary").cloned().unwrap_or(JsonValue::Null),
        "goal_tracking": overview.get("goal_tracking").cloned().unwrap_or(JsonValue::Null),
        "cash_buffer": overview.get("settings").and_then(|v| v.get("cash_buffer")).cloned().unwrap_or(JsonValue::Null),
        "capital_plan": capital_context,
        "reinvestment_pressure": capital_context.get("reinvestment_pressure").cloned().unwrap_or(JsonValue::Null),
        "active_strategy_baseline": active_strategy_baseline,
        "opportunity_horizons": {
            "near_term": {
                "label": "next_2_weeks",
                "instruction": "Find high-conviction setups, catalysts, pullbacks, and risk-reducing rotations that could become actionable soon. Only create an immediate order when market_scope and technical gates support it."
            },
            "medium_term": {
                "label": "next_1_to_3_months",
                "instruction": "Identify watchlist or portfolio names worth monitoring for earnings, valuation, macro, momentum, or allocation reasons. Prefer selected_assets or symbol_sentiment notes over immediate orders unless the setup is actionable today."
            }
        },
        "market_summary": market.get("summary").cloned().unwrap_or(JsonValue::Null),
        "positions": positions.into_iter().take(80).collect::<Vec<_>>(),
        "watchlists": compact_watchlists(&watchlists, &allowed_codes),
        "markov_method": markov_method,
        "daily_indicators": daily_indicators,
    });
    Ok(json!({"system": system, "user": user_payload}))
}

fn capital_planning_context(overview: &JsonValue) -> JsonValue {
    let summary = overview
        .get("portfolio_summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let cash_policy = overview
        .get("settings")
        .and_then(|value| value.get("cash_buffer"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let total_value_dkk = value_f64(&summary, "total_market_value_dkk");
    let invested_value_dkk = value_f64(&summary, "invested_market_value_dkk");
    let cash_balance_dkk = value_f64(&summary, "cash_balance_dkk");
    let min_cash_buffer_pct = value_f64(&cash_policy, "min_cash_buffer_pct").max(0.0);
    let max_deployment_pct = value_f64(&cash_policy, "max_deployment_pct").clamp(0.0, 1.0);
    let reinvestment_pressure_threshold_pct = cash_policy
        .get("reinvestment_pressure_threshold_pct")
        .map(|_| value_f64(&cash_policy, "reinvestment_pressure_threshold_pct"))
        .unwrap_or(0.05)
        .max(0.0);
    let required_cash_buffer_dkk = (total_value_dkk * min_cash_buffer_pct).max(0.0);
    let deployment_cap_dkk = if max_deployment_pct > 0.0 {
        total_value_dkk * max_deployment_pct
    } else {
        total_value_dkk
    };
    let available_cash_above_buffer_dkk = (cash_balance_dkk - required_cash_buffer_dkk).max(0.0);
    let remaining_deployment_capacity_dkk = (deployment_cap_dkk - invested_value_dkk).max(0.0);
    let available_buy_budget_dkk =
        available_cash_above_buffer_dkk.min(remaining_deployment_capacity_dkk);
    let cash_pct = if total_value_dkk > 0.0 {
        cash_balance_dkk / total_value_dkk
    } else {
        0.0
    };
    let excess_cash_pct = (cash_pct - min_cash_buffer_pct).max(0.0);
    let reinvestment_pressure_active =
        excess_cash_pct >= reinvestment_pressure_threshold_pct && available_buy_budget_dkk > 0.0;
    json!({
        "cash_balance_dkk": cash_balance_dkk,
        "total_market_value_dkk": total_value_dkk,
        "invested_market_value_dkk": invested_value_dkk,
        "cash_pct": cash_pct,
        "min_cash_buffer_pct": min_cash_buffer_pct,
        "max_deployment_pct": max_deployment_pct,
        "reinvestment_pressure_threshold_pct": reinvestment_pressure_threshold_pct,
        "required_cash_buffer_dkk": required_cash_buffer_dkk,
        "available_cash_above_buffer_dkk": available_cash_above_buffer_dkk,
        "remaining_deployment_capacity_dkk": remaining_deployment_capacity_dkk,
        "available_buy_budget_dkk": available_buy_budget_dkk,
        "excess_cash_pct": excess_cash_pct,
        "reinvestment_pressure": {
            "active": reinvestment_pressure_active,
            "excess_cash_dkk": available_buy_budget_dkk,
            "excess_cash_pct": excess_cash_pct,
            "threshold_pct": reinvestment_pressure_threshold_pct,
            "instruction": "If active, either recommend gated BUY candidates within available_buy_budget_dkk or explicitly justify holding cash."
        },
        "cash_policy": "Preserve the required cash buffer, avoid margin, and size any BUY recommendations within available_buy_budget_dkk.",
    })
}

fn market_scope_for_pulse(
    pulse: &DecisionPulse,
    market_items: &[JsonValue],
    manual: bool,
) -> JsonValue {
    let open_codes = market_items
        .iter()
        .filter(|row| {
            row.get("is_tradable")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|row| row.get("code").and_then(JsonValue::as_str))
        .map(|code| code.to_uppercase())
        .collect::<HashSet<_>>();
    let pulse_codes = pulse
        .exchange_codes
        .iter()
        .map(|code| code.to_uppercase())
        .collect::<HashSet<_>>();
    let allowed_codes = if manual || pulse.kind == "us_open_followup" {
        open_codes.clone()
    } else {
        open_codes
            .intersection(&pulse_codes)
            .cloned()
            .collect::<HashSet<_>>()
    };
    let mut allowed_list = allowed_codes.into_iter().collect::<Vec<_>>();
    allowed_list.sort();
    let mut primary_list = pulse_codes.into_iter().collect::<Vec<_>>();
    primary_list.sort();
    let policy = match pulse.kind.as_str() {
        "europe_open_followup" => {
            "This is the Nordic/EU/UK open follow-up. Suggest trades only for allowed_trade_exchange_codes; do not suggest US symbols before the US session opens."
        }
        "us_open_followup" => {
            "This is the US open follow-up. Prioritize XNAS/XNYS symbols, but rebalancing may include any currently tradable allowed_trade_exchange_codes."
        }
        _ => {
            "Use only currently tradable exchanges unless the operator explicitly requests a broader manual review."
        }
    };
    json!({
        "policy": policy,
        "pulse_exchange_codes": primary_list,
        "allowed_trade_exchange_codes": allowed_list,
        "source_markets": pulse.source_markets,
        "target_at_utc": pulse.target_at_utc,
    })
}

fn filter_rows_by_exchange(
    rows: Vec<JsonValue>,
    allowed_codes: &HashSet<String>,
) -> Vec<JsonValue> {
    if allowed_codes.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter(|row| {
            let symbol = text(row, "symbol");
            let code = symbol_exchange_code(&symbol);
            code.is_empty() || allowed_codes.contains(&code)
        })
        .collect()
}

fn build_chat_request(state: &AppState, prompt: &JsonValue, model: &str) -> Result<JsonValue> {
    let system = prompt
        .get("system")
        .and_then(JsonValue::as_str)
        .unwrap_or("Return strict JSON only.");
    let user = serde_json::to_string(
        prompt
            .get("user")
            .ok_or_else(|| anyhow!("decision prompt missing user payload"))?,
    )?;
    let max_tokens = yaml_i64(&state.config, &["xai", "max_output_tokens"]).unwrap_or(8192);
    let mut request = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "response_format": {"type": "json_object"},
        "max_tokens": max_tokens
    });
    if let Some(reasoning_effort) = yaml_string(&state.config, &["xai", "reasoning_effort"]) {
        if let Some(obj) = request.as_object_mut() {
            obj.insert(
                "reasoning_effort".to_string(),
                JsonValue::from(reasoning_effort),
            );
        }
    }
    Ok(request)
}

fn active_decision_pulses(state: &AppState) -> Vec<DecisionPulse> {
    let due_window = Duration::minutes(
        yaml_i64(
            &state.config,
            &["strategy", "swing", "analysis_pulses", "due_window_minutes"],
        )
        .unwrap_or(DEFAULT_DUE_WINDOW_MINUTES)
        .max(1),
    );
    let now = Utc::now();
    configured_decision_pulses(state)
        .into_iter()
        .filter(|pulse| {
            let Some(target) = parse_rfc3339_text(&pulse.target_at_utc) else {
                return false;
            };
            now >= target && now < target + due_window
        })
        .collect()
}

/// Build UI-facing decision-pulse metadata from the same exchange schedule used
/// by the scheduler. This is deliberately read-only: it helps operators see
/// whether a report is due soon without submitting a report.
pub fn decision_pulse_summary(state: &AppState) -> JsonValue {
    let due_window = Duration::minutes(
        yaml_i64(
            &state.config,
            &["strategy", "swing", "analysis_pulses", "due_window_minutes"],
        )
        .unwrap_or(DEFAULT_DUE_WINDOW_MINUTES)
        .max(1),
    );
    let now = Utc::now();
    let mut active = Vec::new();
    let mut upcoming = Vec::new();

    for pulse in configured_decision_pulses(state) {
        let Some(target) = parse_rfc3339_text(&pulse.target_at_utc) else {
            continue;
        };
        if now >= target && now < target + due_window {
            active.push(pulse);
        } else if target > now {
            upcoming.push(pulse);
        }
    }
    upcoming.sort_by_key(|pulse| pulse.target_at_utc.clone());
    let next = upcoming.first();

    json!({
        "pulses": active.iter().map(pulse_to_json).collect::<Vec<_>>(),
        "next_pulse_at": next.map(|pulse| JsonValue::from(pulse.target_at_utc.clone())).unwrap_or(JsonValue::Null),
        "next_pulse_label": next.map(|pulse| JsonValue::from(pulse.label.clone())).unwrap_or(JsonValue::Null),
    })
}

fn configured_decision_pulses(state: &AppState) -> Vec<DecisionPulse> {
    let rows = state.market_exchange_rows();
    let mut pulses = Vec::new();
    pulses.extend(grouped_open_followup_pulse_candidates(
        &rows,
        &configured_codes(
            state,
            &[
                "strategy",
                "swing",
                "analysis_pulses",
                "europe_open_followup",
                "exchange_codes",
            ],
            &[
                "XCSE", "XSTO", "XOSL", "XHEL", "XLON", "XETR", "XFRA", "XMIL", "XAMS",
            ],
        ),
        "europe_open_followup",
        "Nordic/EU Open +1h15 Decision Report",
        minutes_after_open(state, "europe_open_followup"),
    ));
    pulses.extend(grouped_open_followup_pulse_candidates(
        &rows,
        &configured_codes(
            state,
            &[
                "strategy",
                "swing",
                "analysis_pulses",
                "us_open_followup",
                "exchange_codes",
            ],
            &["XNAS", "XNYS"],
        ),
        "us_open_followup",
        "US Open +1h15 Decision Report",
        minutes_after_open(state, "us_open_followup"),
    ));
    pulses
}

fn grouped_open_followup_pulse_candidates(
    rows: &[JsonValue],
    configured_codes: &HashSet<String>,
    kind: &str,
    label: &str,
    minutes_after_open: i64,
) -> Vec<DecisionPulse> {
    let mut groups: Vec<(DateTime<Utc>, Vec<JsonValue>)> = Vec::new();
    for row in rows {
        let code = text(row, "code").to_uppercase();
        if !configured_codes.contains(&code) {
            continue;
        }
        let Some(session_open) = parse_time(row.get("session_open_at_utc")) else {
            continue;
        };
        let Some(tradable_close) = parse_time(row.get("tradable_close_at_utc")) else {
            continue;
        };
        let target = session_open + Duration::minutes(minutes_after_open);
        if target >= tradable_close {
            continue;
        }
        if let Some((_, values)) = groups.iter_mut().find(|(existing, _)| *existing == target) {
            values.push(row.clone());
        } else {
            groups.push((target, vec![row.clone()]));
        }
    }
    groups
        .into_iter()
        .map(|(target, rows)| {
            let local_date = target.date_naive().to_string();
            let exchange_codes = rows
                .iter()
                .map(|row| text(row, "code").to_uppercase())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let source_markets = rows
                .iter()
                .map(|row| text(row, "market"))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            DecisionPulse {
                key: format!("{kind}:{local_date}"),
                label: label.to_string(),
                kind: kind.to_string(),
                target_at_utc: target.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                exchange_codes,
                source_markets,
            }
        })
        .collect()
}

fn configured_codes(state: &AppState, keys: &[&str], fallback: &[&str]) -> HashSet<String> {
    crate::config::yaml_at(&state.config, keys)
        .and_then(JsonValueFromYaml::as_sequence_strings)
        .unwrap_or_else(|| fallback.iter().map(|value| value.to_string()).collect())
        .into_iter()
        .map(|code| code.to_uppercase())
        .collect()
}

struct JsonValueFromYaml;

impl JsonValueFromYaml {
    fn as_sequence_strings(value: &serde_yaml::Value) -> Option<Vec<String>> {
        Some(
            value
                .as_sequence()?
                .iter()
                .filter_map(|item| item.as_str().map(ToString::to_string))
                .collect(),
        )
    }
}

fn minutes_after_open(state: &AppState, key: &str) -> i64 {
    yaml_i64(
        &state.config,
        &[
            "strategy",
            "swing",
            "analysis_pulses",
            key,
            "minutes_after_open",
        ],
    )
    .unwrap_or(DEFAULT_MINUTES_AFTER_OPEN)
}

async fn has_report_for_pulse(state: &AppState, pulse_key: &str) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id, status FROM decision_reports WHERE analysis_pulse_key = '{}' ORDER BY created_at DESC, id DESC LIMIT 1",
        sql_escape(pulse_key)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

async fn insert_xai_error_report(
    state: &AppState,
    created_at: &str,
    pulse: &DecisionPulse,
    model: &str,
    prompt: &JsonValue,
    request_json: &JsonValue,
    error_text: &str,
) -> Result<JsonValue> {
    let report_json = json!({
        "status": "xai_error",
        "created_at": created_at,
        "report_title": pulse.label,
        "analysis_pulse": pulse_to_json(pulse),
        "strategy_plan": {"status": "xai_error", "swing_orders": [], "suggested_trades": []},
        "suggested_trades": [],
        "execution_notes": [error_text]
    });
    insert_decision_report(
        state,
        created_at,
        pulse,
        model.to_string(),
        "xai_error",
        None,
        prompt,
        request_json,
        None,
        &report_json,
        Some(error_text),
    )
    .await
}

fn truncate_error_text(value: &str, max_chars: usize) -> String {
    let mut text = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        text.push_str("...");
    }
    text
}

async fn insert_decision_report(
    state: &AppState,
    created_at: &str,
    pulse: &DecisionPulse,
    model: String,
    status: &str,
    response_id: Option<&str>,
    prompt: &JsonValue,
    request_json: &JsonValue,
    response_json: Option<&JsonValue>,
    report_json: &JsonValue,
    error_text: Option<&str>,
) -> Result<JsonValue> {
    let report_date = created_at.chars().take(10).collect::<String>();
    let batch_id = latest_batch_id(state).await?.unwrap_or_default();
    let response_id_sql = sql_opt_text(response_id);
    let response_json_sql = sql_opt_json(response_json)?;
    let error_sql = sql_opt_text(error_text);
    let sql = format!(
        "INSERT INTO decision_reports (
            created_at, report_date, batch_id, model, status, analysis_window_active,
            response_id, prompt_text, request_json, response_json, report_json,
            error_text, analysis_pulse_key, analysis_pulse_label
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', 1,
            {}, '{}', '{}', {}, '{}',
            {}, '{}', '{}'
        )
        RETURNING id, created_at, report_date, model, status, analysis_window_active, response_id,
            prompt_text, request_json, response_json, report_json, error_text,
            analysis_pulse_key, analysis_pulse_label",
        sql_escape(created_at),
        sql_escape(&report_date),
        sql_escape(&batch_id),
        sql_escape(&model),
        sql_escape(status),
        response_id_sql,
        sql_escape(&serde_json::to_string(prompt)?),
        sql_escape(&serde_json::to_string(request_json)?),
        response_json_sql,
        sql_escape(&serde_json::to_string(report_json)?),
        error_sql,
        sql_escape(&pulse.key),
        sql_escape(&pulse.label)
    );
    let row = sqlx::query(&sql)
        .fetch_one(&state.pool)
        .await
        .context("inserting xAI decision report row")?;
    Ok(row_to_json(&row))
}

async fn update_completed_report(
    state: &AppState,
    report_id: i64,
    response_json: &JsonValue,
    report_json: &JsonValue,
) -> Result<()> {
    let response_id = response_json
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let sql = format!(
        "UPDATE decision_reports
         SET status = 'completed',
             response_id = '{}',
             response_json = '{}',
             report_json = '{}',
             error_text = NULL
         WHERE id = {}",
        sql_escape(response_id),
        sql_escape(&serde_json::to_string(response_json)?),
        sql_escape(&serde_json::to_string(report_json)?),
        report_id.max(0)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("updating completed xAI decision report")?;
    Ok(())
}

async fn mark_deferred_report_error(
    state: &AppState,
    report_id: i64,
    error_text: &str,
) -> Result<()> {
    let sql = format!(
        "UPDATE decision_reports SET status = 'xai_error', error_text = '{}' WHERE id = {}",
        sql_escape(error_text),
        report_id.max(0)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("marking xAI deferred report failed")?;
    Ok(())
}

fn decode_pending_report(row: &JsonValue) -> Result<PendingDeferredReport> {
    let request_json = decode_json_field(row.get("request_json"));
    let report_json = decode_json_field(row.get("report_json"));
    let request_id = row
        .get("response_id")
        .and_then(JsonValue::as_str)
        .or_else(|| {
            report_json
                .get("xai_deferred")
                .and_then(|value| value.get("request_id"))
                .and_then(JsonValue::as_str)
        })
        .or_else(|| request_json.get("request_id").and_then(JsonValue::as_str))
        .ok_or_else(|| anyhow!("pending xAI report does not include request_id"))?
        .to_string();
    Ok(PendingDeferredReport {
        id: value_i64(row, "id"),
        request_id,
        request_json,
        report_json,
    })
}

fn decode_json_field(value: Option<&JsonValue>) -> JsonValue {
    match value {
        Some(JsonValue::String(text)) => serde_json::from_str(text).unwrap_or(JsonValue::Null),
        Some(value) => value.clone(),
        None => JsonValue::Null,
    }
}

async fn latest_batch_id(state: &AppState) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(|row| row.try_get::<String, _>("batch_id").ok()))
}

fn compact_watchlists(watchlists: &JsonValue, allowed_codes: &HashSet<String>) -> JsonValue {
    let categories = watchlists
        .get("categories")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    JsonValue::Array(
        categories
            .into_iter()
            .map(|category| {
                let items = category
                    .get("items")
                    .and_then(JsonValue::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|row| {
                        if allowed_codes.is_empty() {
                            return true;
                        }
                        let symbol = text(row, "symbol");
                        let code = symbol_exchange_code(&symbol);
                        code.is_empty() || allowed_codes.contains(&code)
                    })
                    .take(80)
                    .collect::<Vec<_>>();
                json!({
                    "key": category.get("key").cloned().unwrap_or(JsonValue::Null),
                    "label": category.get("label").cloned().unwrap_or(JsonValue::Null),
                    "items": items,
                })
            })
            .collect(),
    )
}

fn pulse_to_json(pulse: &DecisionPulse) -> JsonValue {
    json!({
        "key": pulse.key,
        "label": pulse.label,
        "kind": pulse.kind,
        "target_at_utc": pulse.target_at_utc,
        "exchange_codes": pulse.exchange_codes,
        "source_markets": pulse.source_markets,
    })
}

fn xai_base_url(state: &AppState) -> String {
    ai_base_url(state)
}

fn ai_base_url(state: &AppState) -> String {
    yaml_string(&state.config, &["xai", "base_url"])
        .unwrap_or_else(|| {
            if ai_provider(state) == "openrouter" {
                "https://openrouter.ai/api/v1".to_string()
            } else {
                "https://api.x.ai/v1".to_string()
            }
        })
        .trim_end_matches('/')
        .to_string()
}

fn ai_provider(state: &AppState) -> String {
    yaml_string(&state.config, &["xai", "provider"])
        .or_else(|| yaml_string(&state.config, &["xai", "inference_provider"]))
        .unwrap_or_else(|| {
            if yaml_string(&state.config, &["xai", "base_url"])
                .map(|value| value.to_lowercase().contains("openrouter.ai"))
                .unwrap_or(false)
            {
                "openrouter".to_string()
            } else {
                "xai".to_string()
            }
        })
        .trim()
        .to_lowercase()
}

fn ai_api_key(state: &AppState) -> Option<String> {
    yaml_string(&state.config, &["xai", "api_key"])
}

fn ai_api_key_env_name(state: &AppState) -> &'static str {
    if ai_provider(state) == "openrouter" {
        "OPENROUTER_API_KEY"
    } else {
        "XAI_API_KEY"
    }
}

fn xai_http_timeout_seconds(state: &AppState) -> u64 {
    yaml_i64(&state.config, &["xai", "http_timeout_seconds"])
        .or_else(|| yaml_i64(&state.config, &["xai", "deferred_http_timeout_seconds"]))
        .or_else(|| yaml_i64(&state.config, &["xai", "timeout_seconds"]))
        .unwrap_or(30)
        .max(5) as u64
}

fn parse_time(value: Option<&JsonValue>) -> Option<DateTime<Utc>> {
    let text = value?.as_str()?;
    parse_rfc3339_text(text)
}

fn parse_rfc3339_text(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_opt_json(value: Option<&JsonValue>) -> Result<String> {
    Ok(match value {
        Some(value) => format!("'{}'", sql_escape(&serde_json::to_string(value)?)),
        None => "NULL".to_string(),
    })
}

fn text(value: &JsonValue, key: &str) -> String {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}

fn symbol_exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.to_uppercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_fenced_json_content() {
        assert_eq!(
            parse_json_content(r#"{"status":"ok"}"#).unwrap()["status"],
            "ok"
        );
        assert_eq!(
            parse_json_content("```json\n{\"status\":\"ok\"}\n```").unwrap()["status"],
            "ok"
        );
    }

    #[test]
    fn normalizes_completed_report_with_strategy_plan() {
        let pending = PendingDeferredReport {
            id: 1,
            request_id: "req-1".to_string(),
            request_json: json!({}),
            report_json: json!({"created_at": "2026-05-11T08:15:00Z", "analysis_pulse": {"key": "europe_open_followup:2026-05-11"}}),
        };
        let response = json!({
            "id": "chatcmpl-1",
            "choices": [{"message": {"content": "{\"report_title\":\"Daily\",\"suggested_trades\":[]}"}}]
        });
        let report = completed_report_json(&pending, &response).unwrap();
        assert_eq!(report["status"], "completed");
        assert_eq!(report["strategy_plan"]["status"], "completed");
    }

    #[test]
    fn completed_report_preserves_requested_capital_plan() {
        let pending = PendingDeferredReport {
            id: 1,
            request_id: "req-1".to_string(),
            request_json: json!({
                "messages": [
                    {"role": "system", "content": "system"},
                    {"role": "user", "content": "{\"capital_plan\":{\"cash_balance_dkk\":76000.0,\"available_buy_budget_dkk\":47000.0}}"}
                ]
            }),
            report_json: json!({"created_at": "2026-05-11T08:15:00Z", "analysis_pulse": {"key": "us_open_followup:2026-05-11"}}),
        };
        let response = json!({
            "id": "chatcmpl-1",
            "choices": [{"message": {"content": "{\"report_title\":\"Daily\",\"suggested_trades\":[]}"}}]
        });
        let report = completed_report_json(&pending, &response).unwrap();
        assert_eq!(
            report["capital_plan"]["available_buy_budget_dkk"],
            JsonValue::from(47000.0)
        );
        assert_eq!(
            report["strategy_plan"]["capital_plan"]["cash_balance_dkk"],
            JsonValue::from(76000.0)
        );
    }

    #[test]
    fn enforces_europe_pulse_scope_on_completed_report() {
        let pulse = json!({
            "kind": "europe_open_followup",
            "exchange_codes": ["XCSE", "XLON"]
        });
        let mut report = json!({
            "suggested_trades": [
                {"symbol": "MSTR:xnas", "action": "SELL"},
                {"symbol": "ORSTED:xcse", "action": "BUY"}
            ],
            "strategy_plan": {
                "swing_orders": [
                    {"symbol": "NVDA:xnas", "action": "BUY"},
                    {"symbol": "AZN:xlon", "action": "BUY"}
                ]
            }
        });
        let enforcement = enforce_completed_report_scope(&mut report, &pulse);
        assert_eq!(report["suggested_trades"].as_array().unwrap().len(), 1);
        assert_eq!(
            report["suggested_trades"][0]["symbol"],
            JsonValue::from("ORSTED:xcse")
        );
        assert_eq!(
            report["strategy_plan"]["swing_orders"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(enforcement["status"], "enforced");
    }

    #[test]
    fn builds_cash_aware_capital_planning_context() {
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
        let context = capital_planning_context(&overview);
        assert_eq!(
            context["required_cash_buffer_dkk"],
            JsonValue::from(30000.0)
        );
        assert_eq!(
            context["available_buy_budget_dkk"],
            JsonValue::from(20000.0)
        );
        assert_eq!(
            context["reinvestment_pressure"]["active"],
            JsonValue::from(true)
        );
    }
}
