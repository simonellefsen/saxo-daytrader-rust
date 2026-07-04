use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{clamp_limit, row_to_json, sql_escape},
    markov_method::markov_assets,
    state::AppState,
};

const DEFAULT_BASE_URL: &str = "https://api.quiverquant.com";
const DEFAULT_DAILY_TIME: &str = "23:10";
const REQUEST_DELAY_MS: u64 = 350;
const MAX_ATTEMPTS: usize = 3;

#[derive(Clone, Debug)]
struct QuiverConfig {
    enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    run_weekdays_only: bool,
    base_url: String,
    api_key: Option<String>,
    max_symbols: usize,
    lookback_days: i64,
}

#[derive(Clone, Debug)]
struct QuiverAsset {
    symbol: String,
    ticker: String,
    instrument_name: String,
    source: String,
}

#[derive(Clone, Debug)]
struct CongressTrade {
    representative: String,
    report_date: Option<NaiveDate>,
    transaction_date: Option<NaiveDate>,
    transaction: String,
    range: String,
    house: String,
    party: String,
    amount: f64,
    excess_return: Option<f64>,
    price_change: Option<f64>,
    spy_change: Option<f64>,
}

#[derive(Clone, Debug)]
struct QuiverAnalysis {
    status: String,
    signal: f64,
    direction: String,
    confidence: f64,
    event_count: usize,
    congress_purchase_count: usize,
    congress_sale_count: usize,
    net_congress_amount: f64,
    latest_event_date: Option<NaiveDate>,
    top_events: Vec<JsonValue>,
    source_status: JsonValue,
    error_text: Option<String>,
}

pub async fn run_quiver_signal_cycle(state: &AppState) -> Result<JsonValue> {
    let config = quiver_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    if config.api_key.as_deref().unwrap_or("").is_empty() {
        return Ok(json!({
            "status": "disabled",
            "reason": "missing_api_key",
            "api_key_env": "QUIVERQUANT_API_KEY"
        }));
    }

    let now_local = Utc::now().with_timezone(&config.timezone);
    let run_date = now_local.date_naive();
    if config.run_weekdays_only && run_date.weekday().number_from_monday() > 5 {
        return Ok(json!({
            "status": "idle",
            "reason": "weekend",
            "run_date": run_date.to_string(),
            "timezone": config.timezone.name()
        }));
    }
    if now_local.time() < config.daily_time {
        return Ok(json!({
            "status": "idle",
            "reason": "not_due",
            "run_date": run_date.to_string(),
            "due_time": config.daily_time.format("%H:%M").to_string(),
            "timezone": config.timezone.name()
        }));
    }
    if quiver_run_exists(state, run_date).await? {
        return Ok(json!({
            "status": "skipped",
            "reason": "already_ran",
            "run_date": run_date.to_string()
        }));
    }

    run_quiver_signals_for_date(state, &config, run_date).await
}

pub async fn run_quiver_signals_now(state: &AppState) -> Result<JsonValue> {
    let config = quiver_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    if config.api_key.as_deref().unwrap_or("").is_empty() {
        return Ok(json!({
            "status": "disabled",
            "reason": "missing_api_key",
            "api_key_env": "QUIVERQUANT_API_KEY"
        }));
    }
    let run_date = Utc::now().with_timezone(&config.timezone).date_naive();
    run_quiver_signals_for_date(state, &config, run_date).await
}

async fn run_quiver_signals_for_date(
    state: &AppState,
    config: &QuiverConfig,
    run_date: NaiveDate,
) -> Result<JsonValue> {
    let assets = quiver_assets(state, config.max_symbols).await?;
    let run_id = format!("quiver-{}", Utc::now().timestamp_micros());
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut rows = Vec::new();
    let mut success_count = 0usize;
    let mut error_count = 0usize;
    let mut terminal_status: Option<String> = None;

    for asset in &assets {
        let row = match analyze_asset(config, run_date, asset).await {
            Ok(analysis) => {
                if analysis.status == "ok" {
                    success_count += 1;
                } else {
                    error_count += 1;
                }
                signal_row_json(&run_id, &created_at, run_date, config, asset, &analysis)
            }
            Err(err) => {
                error_count += 1;
                let terminal_error = classify_terminal_quiver_error(&err);
                let status = terminal_error.unwrap_or("error");
                warn!(symbol = %asset.symbol, "Quiver asset analysis failed: {err:#}");
                let analysis = QuiverAnalysis {
                    status: status.to_string(),
                    signal: 0.0,
                    direction: "neutral".to_string(),
                    confidence: 0.0,
                    event_count: 0,
                    congress_purchase_count: 0,
                    congress_sale_count: 0,
                    net_congress_amount: 0.0,
                    latest_event_date: None,
                    top_events: Vec::new(),
                    source_status: json!({"congress_trading": status}),
                    error_text: Some(format!("{err:#}")),
                };
                let row = signal_row_json(&run_id, &created_at, run_date, config, asset, &analysis);
                if terminal_error.is_some() {
                    terminal_status = Some(status.to_string());
                }
                row
            }
        };
        insert_quiver_signal(state, &row).await?;
        rows.push(row);
        if terminal_status.is_some() {
            break;
        }
        sleep(StdDuration::from_millis(REQUEST_DELAY_MS)).await;
    }

    let status = if let Some(status) = terminal_status.as_deref() {
        status
    } else if success_count > 0 {
        "completed"
    } else if assets.is_empty() {
        "empty"
    } else {
        "error"
    };
    let ok_rows = rows
        .iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok"))
        .cloned()
        .collect::<Vec<_>>();
    let mut ranked_bullish = ok_rows.clone();
    ranked_bullish.sort_by(|left, right| {
        value_f64(right, "signal")
            .partial_cmp(&value_f64(left, "signal"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ranked_bearish = ok_rows.clone();
    ranked_bearish.sort_by(|left, right| {
        value_f64(left, "signal")
            .partial_cmp(&value_f64(right, "signal"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let summary = json!({
        "status": status,
        "run_id": run_id,
        "run_date": run_date.to_string(),
        "asset_count": rows.len(),
        "requested_asset_count": assets.len(),
        "success_count": success_count,
        "error_count": error_count,
        "config": quiver_config_json(config),
        "signals": ranked_bullish.iter().take(20).map(compact_signal_row).collect::<Vec<_>>(),
        "top_bullish": ranked_bullish.iter().filter(|row| value_f64(row, "signal") > 0.15).take(8).map(compact_signal_row).collect::<Vec<_>>(),
        "top_bearish": ranked_bearish.iter().filter(|row| value_f64(row, "signal") < -0.15).take(8).map(compact_signal_row).collect::<Vec<_>>(),
    });
    insert_quiver_run(
        state,
        &run_id,
        &created_at,
        run_date,
        status,
        rows.len(),
        success_count,
        error_count,
        config,
        &summary,
    )
    .await?;
    info!(
        run_id,
        success_count, error_count, "Quiver signal cycle completed"
    );
    Ok(summary)
}

async fn analyze_asset(
    config: &QuiverConfig,
    run_date: NaiveDate,
    asset: &QuiverAsset,
) -> Result<QuiverAnalysis> {
    let trades = fetch_congress_trades(config, &asset.ticker).await?;
    Ok(analyze_congress_trades(
        run_date,
        config.lookback_days,
        &trades,
    ))
}

async fn fetch_congress_trades(config: &QuiverConfig, ticker: &str) -> Result<Vec<CongressTrade>> {
    let path = format!("/beta/historical/congresstrading/{}", ticker);
    let payload = quiver_get_json(config, &path).await?;
    let rows = payload
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|row| CongressTrade {
            representative: text_any(&row, &["Representative", "Senator", "Name"]),
            report_date: parse_date(&text_any(&row, &["ReportDate", "Reportdate", "Filed"])),
            transaction_date: parse_date(&text_any(&row, &["TransactionDate", "Date", "Traded"])),
            transaction: text_any(&row, &["Transaction", "TransactionType"]),
            range: text_any(&row, &["Range", "AmountRange"]),
            house: text_any(&row, &["House", "Chamber"]),
            party: text_any(&row, &["Party"]),
            amount: number_any(&row, &["Amount"])
                .unwrap_or_else(|| range_amount(&text_any(&row, &["Range"]))),
            excess_return: number_any(&row, &["ExcessReturn"]),
            price_change: number_any(&row, &["PriceChange"]),
            spy_change: number_any(&row, &["SPYChange", "SpyChange"]),
        })
        .collect::<Vec<_>>();
    Ok(rows)
}

async fn quiver_get_json(config: &QuiverConfig, path: &str) -> Result<JsonValue> {
    let api_key = config
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("QUIVERQUANT_API_KEY is missing"))?;
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(30))
        .build()?;
    let url = format!("{}{}", config.base_url.trim_end_matches('/'), path);
    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let response = client
            .get(&url)
            .bearer_auth(api_key)
            .header(header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str::<JsonValue>(&body)
                .with_context(|| format!("parsing Quiver response for {path}"));
        }
        let snippet = body.chars().take(280).collect::<String>();
        last_error = format!("Quiver GET failed: HTTP {}: {}", status.as_u16(), snippet);
        if (status.as_u16() == 429 || status.is_server_error()) && attempt < MAX_ATTEMPTS {
            let delay_secs = retry_after.unwrap_or_else(|| 2_u64.pow(attempt as u32).min(30));
            warn!(path, attempt, delay_secs, "Quiver GET retrying after error");
            sleep(StdDuration::from_secs(delay_secs)).await;
            continue;
        }
        bail!("{last_error}");
    }
    bail!("{last_error}")
}

fn classify_terminal_quiver_error(err: &anyhow::Error) -> Option<&'static str> {
    let message = format!("{err:#}").to_lowercase();
    if message.contains("http 401") {
        Some("auth_error")
    } else if message.contains("http 403") || message.contains("upgrade your subscription plan") {
        Some("subscription_required")
    } else {
        None
    }
}

fn analyze_congress_trades(
    run_date: NaiveDate,
    lookback_days: i64,
    trades: &[CongressTrade],
) -> QuiverAnalysis {
    let mut total = 0.0;
    let mut event_count = 0usize;
    let mut purchase_count = 0usize;
    let mut sale_count = 0usize;
    let mut net_amount = 0.0;
    let mut latest_event_date = None;
    let mut event_rows = Vec::new();

    for trade in trades {
        let Some(event_date) = trade.transaction_date.or(trade.report_date) else {
            continue;
        };
        let age_days = (run_date - event_date).num_days();
        if age_days < 0 || age_days > lookback_days {
            continue;
        }
        let direction = transaction_direction(&trade.transaction);
        if direction == 0.0 {
            continue;
        }
        event_count += 1;
        if direction > 0.0 {
            purchase_count += 1;
        } else {
            sale_count += 1;
        }
        let amount = trade.amount.max(0.0);
        net_amount += direction * amount;
        let recency_weight =
            (1.0 - (age_days as f64 / lookback_days.max(1) as f64)).clamp(0.15, 1.0);
        let amount_weight = ((amount + 1.0).ln() / 15_001.0_f64.ln()).clamp(0.2, 1.5);
        total += direction * recency_weight * amount_weight;
        latest_event_date = Some(match latest_event_date {
            Some(current) if current > event_date => current,
            _ => event_date,
        });
        event_rows.push(json!({
            "representative": trade.representative,
            "report_date": trade.report_date.map(|value| value.to_string()),
            "transaction_date": trade.transaction_date.map(|value| value.to_string()),
            "transaction": trade.transaction,
            "range": trade.range,
            "amount": amount,
            "house": trade.house,
            "party": trade.party,
            "excess_return": trade.excess_return,
            "price_change": trade.price_change,
            "spy_change": trade.spy_change,
            "age_days": age_days
        }));
    }

    event_rows.sort_by(|left, right| {
        let left_date = left
            .get("transaction_date")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let right_date = right
            .get("transaction_date")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        right_date.cmp(left_date)
    });
    let signal = (total / 4.0).tanh().clamp(-1.0, 1.0);
    let direction = if signal > 0.15 {
        "bullish"
    } else if signal < -0.15 {
        "bearish"
    } else {
        "neutral"
    }
    .to_string();
    let confidence =
        ((event_count as f64 / 12.0).min(0.55) + signal.abs() * 0.35 + 0.10).clamp(0.0, 1.0);

    QuiverAnalysis {
        status: "ok".to_string(),
        signal,
        direction,
        confidence: if event_count == 0 { 0.0 } else { confidence },
        event_count,
        congress_purchase_count: purchase_count,
        congress_sale_count: sale_count,
        net_congress_amount: net_amount,
        latest_event_date,
        top_events: event_rows.into_iter().take(8).collect(),
        source_status: json!({
            "congress_trading": {
                "status": "ok",
                "lookback_days": lookback_days,
                "matched_events": event_count
            }
        }),
        error_text: None,
    }
}

fn signal_row_json(
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    config: &QuiverConfig,
    asset: &QuiverAsset,
    analysis: &QuiverAnalysis,
) -> JsonValue {
    json!({
        "id": format!("{}-{}", run_id, asset.symbol.replace(':', "-").to_lowercase()),
        "run_id": run_id,
        "created_at": created_at,
        "run_date": run_date.to_string(),
        "status": analysis.status,
        "symbol": asset.symbol,
        "ticker": asset.ticker,
        "instrument_name": asset.instrument_name,
        "source": asset.source,
        "lookback_days": config.lookback_days,
        "signal": analysis.signal,
        "direction": analysis.direction,
        "confidence": analysis.confidence,
        "event_count": analysis.event_count,
        "congress_purchase_count": analysis.congress_purchase_count,
        "congress_sale_count": analysis.congress_sale_count,
        "net_congress_amount": analysis.net_congress_amount,
        "latest_event_date": analysis.latest_event_date.map(|value| value.to_string()),
        "source_status_json": analysis.source_status,
        "top_events_json": analysis.top_events,
        "error_text": analysis.error_text
    })
}

fn compact_signal_row(row: &JsonValue) -> JsonValue {
    json!({
        "symbol": text(row, "symbol"),
        "ticker": text(row, "ticker"),
        "status": text(row, "status"),
        "signal": value_f64(row, "signal"),
        "direction": text(row, "direction"),
        "confidence": value_f64(row, "confidence"),
        "event_count": row.get("event_count").and_then(JsonValue::as_u64).unwrap_or(0),
        "congress_purchase_count": row.get("congress_purchase_count").and_then(JsonValue::as_u64).unwrap_or(0),
        "congress_sale_count": row.get("congress_sale_count").and_then(JsonValue::as_u64).unwrap_or(0),
        "net_congress_amount": value_f64(row, "net_congress_amount"),
        "latest_event_date": row.get("latest_event_date").cloned().unwrap_or(JsonValue::Null)
    })
}

async fn quiver_assets(state: &AppState, max_symbols: usize) -> Result<Vec<QuiverAsset>> {
    let assets = markov_assets(state, max_symbols.saturating_mul(2).max(max_symbols)).await?;
    let mut rows = Vec::new();
    for asset in assets {
        let Some(ticker) = quiver_ticker(&asset.symbol) else {
            continue;
        };
        rows.push(QuiverAsset {
            symbol: asset.symbol,
            ticker,
            instrument_name: asset.instrument_name,
            source: asset.source,
        });
        if max_symbols > 0 && rows.len() >= max_symbols {
            break;
        }
    }
    Ok(rows)
}

fn quiver_ticker(symbol: &str) -> Option<String> {
    let mut parts = symbol.splitn(2, ':');
    let base = parts.next()?.trim().to_uppercase();
    let exchange = parts.next().unwrap_or("").trim().to_lowercase();
    if base.is_empty() {
        return None;
    }
    match exchange.as_str() {
        "xnas" | "xnys" | "arcx" | "bats" => Some(base.replace('.', "-")),
        _ => None,
    }
}

async fn insert_quiver_signal(state: &AppState, row: &JsonValue) -> Result<()> {
    let sql = format!(
        "INSERT INTO quiver_asset_signals (
            id, run_id, created_at, run_date, status, symbol, ticker,
            instrument_name, source, lookback_days, signal, direction,
            confidence, event_count, congress_purchase_count,
            congress_sale_count, net_congress_amount, latest_event_date,
            source_status_json, top_events_json, error_text
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', '{}', '{}',
            '{}', '{}', {}, {}, '{}',
            {}, {}, {}, {}, {}, {}, '{}', '{}', {}
        )",
        sql_escape(&text(row, "id")),
        sql_escape(&text(row, "run_id")),
        sql_escape(&text(row, "created_at")),
        sql_escape(&text(row, "run_date")),
        sql_escape(&text(row, "status")),
        sql_escape(&text(row, "symbol")),
        sql_escape(&text(row, "ticker")),
        sql_escape(&text(row, "instrument_name")),
        sql_escape(&text(row, "source")),
        row.get("lookback_days")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        row.get("signal").and_then(JsonValue::as_f64).unwrap_or(0.0),
        sql_escape(&text(row, "direction")),
        row.get("confidence")
            .and_then(JsonValue::as_f64)
            .unwrap_or(0.0),
        row.get("event_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("congress_purchase_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("congress_sale_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("net_congress_amount")
            .and_then(JsonValue::as_f64)
            .unwrap_or(0.0),
        optional_text_sql(row.get("latest_event_date").and_then(JsonValue::as_str)),
        sql_escape(&serde_json::to_string(
            row.get("source_status_json").unwrap_or(&JsonValue::Null)
        )?),
        sql_escape(&serde_json::to_string(
            row.get("top_events_json").unwrap_or(&JsonValue::Null)
        )?),
        optional_text_sql(row.get("error_text").and_then(JsonValue::as_str)),
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Quiver asset signal")?;
    Ok(())
}

async fn insert_quiver_run(
    state: &AppState,
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    status: &str,
    asset_count: usize,
    success_count: usize,
    error_count: usize,
    config: &QuiverConfig,
    summary: &JsonValue,
) -> Result<()> {
    let sql = format!(
        "INSERT INTO quiver_signal_runs (
            id, created_at, run_date, status, asset_count, success_count,
            error_count, config_json, summary_json
        ) VALUES (
            '{}', '{}', '{}', '{}', {}, {}, {}, '{}', '{}'
        )",
        sql_escape(run_id),
        sql_escape(created_at),
        sql_escape(&run_date.to_string()),
        sql_escape(status),
        asset_count,
        success_count,
        error_count,
        sql_escape(&serde_json::to_string(&quiver_config_json(config))?),
        sql_escape(&serde_json::to_string(summary)?)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Quiver signal run")?;
    Ok(())
}

async fn quiver_run_exists(state: &AppState, run_date: NaiveDate) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM quiver_signal_runs WHERE run_date = '{}' LIMIT 1",
        sql_escape(&run_date.to_string())
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

pub async fn latest_quiver_signals(state: &AppState, limit: i64) -> Result<Vec<JsonValue>> {
    let sql = format!(
        "SELECT id, run_id, created_at, run_date, status, symbol, ticker,
                instrument_name, source, lookback_days, signal, direction,
                confidence, event_count, congress_purchase_count,
                congress_sale_count, net_congress_amount, latest_event_date,
                source_status_json, top_events_json, error_text
         FROM quiver_asset_signals
         WHERE run_id = (
            SELECT id
            FROM quiver_signal_runs
            ORDER BY run_date DESC, created_at DESC
            LIMIT 1
         )
         ORDER BY signal DESC, confidence DESC, symbol ASC
         LIMIT {}",
        clamp_limit(limit, 1, 500)
    );
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.iter().map(row_to_json).collect())
}

pub async fn latest_quiver_run(state: &AppState) -> Result<JsonValue> {
    let row = sqlx::query(
        "SELECT id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json
         FROM quiver_signal_runs
         ORDER BY run_date DESC, created_at DESC
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.as_ref().map(row_to_json).unwrap_or(JsonValue::Null))
}

pub async fn compact_quiver_context(state: &AppState, limit: i64) -> Result<JsonValue> {
    let signals = latest_quiver_signals(state, limit)
        .await?
        .into_iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok"))
        .map(|row| {
            json!({
                "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
                "ticker": row.get("ticker").cloned().unwrap_or(JsonValue::Null),
                "run_date": row.get("run_date").cloned().unwrap_or(JsonValue::Null),
                "signal": row.get("signal").cloned().unwrap_or(JsonValue::Null),
                "direction": row.get("direction").cloned().unwrap_or(JsonValue::Null),
                "confidence": row.get("confidence").cloned().unwrap_or(JsonValue::Null),
                "event_count": row.get("event_count").cloned().unwrap_or(JsonValue::Null),
                "congress_purchase_count": row.get("congress_purchase_count").cloned().unwrap_or(JsonValue::Null),
                "congress_sale_count": row.get("congress_sale_count").cloned().unwrap_or(JsonValue::Null),
                "net_congress_amount": row.get("net_congress_amount").cloned().unwrap_or(JsonValue::Null),
                "latest_event_date": row.get("latest_event_date").cloned().unwrap_or(JsonValue::Null),
                "top_events": parse_json_field(row.get("top_events_json")).unwrap_or(JsonValue::Null)
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "latest_run": latest_quiver_run(state).await.unwrap_or(JsonValue::Null),
        "signals": signals
    }))
}

pub fn quiver_config_json_for_state(state: &AppState) -> JsonValue {
    quiver_config_json(&quiver_config(state))
}

fn quiver_config(state: &AppState) -> QuiverConfig {
    let timezone = yaml_string(&state.config, &["strategy", "quiver", "timezone"])
        .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let daily_time = yaml_string(&state.config, &["strategy", "quiver", "daily_time"])
        .as_deref()
        .and_then(parse_hh_mm)
        .unwrap_or_else(|| parse_hh_mm(DEFAULT_DAILY_TIME).expect("default Quiver time is valid"));
    let api_key = yaml_string(&state.config, &["quiver", "api_key"])
        .or_else(|| std::env::var("QUIVERQUANT_API_KEY").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    QuiverConfig {
        enabled: yaml_bool(&state.config, &["strategy", "quiver", "enabled"]).unwrap_or(true),
        timezone,
        daily_time,
        run_weekdays_only: yaml_bool(&state.config, &["strategy", "quiver", "run_weekdays_only"])
            .unwrap_or(true),
        base_url: yaml_string(&state.config, &["quiver", "base_url"])
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        api_key,
        max_symbols: yaml_i64(&state.config, &["strategy", "quiver", "max_symbols"])
            .unwrap_or(60)
            .max(0) as usize,
        lookback_days: yaml_i64(&state.config, &["strategy", "quiver", "lookback_days"])
            .unwrap_or(120)
            .max(1),
    }
}

fn quiver_config_json(config: &QuiverConfig) -> JsonValue {
    json!({
        "enabled": config.enabled,
        "timezone": config.timezone.name(),
        "daily_time": config.daily_time.format("%H:%M").to_string(),
        "run_weekdays_only": config.run_weekdays_only,
        "base_url": config.base_url,
        "api_key_configured": config.api_key.as_ref().is_some_and(|value| !value.is_empty()),
        "max_symbols": config.max_symbols,
        "lookback_days": config.lookback_days,
        "sources": ["congress_trading"]
    })
}

pub fn create_schema_sql() -> Vec<&'static str> {
    vec![
        "CREATE TABLE IF NOT EXISTS quiver_signal_runs (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            run_date TEXT NOT NULL,
            status TEXT NOT NULL,
            asset_count INTEGER NOT NULL,
            success_count INTEGER NOT NULL,
            error_count INTEGER NOT NULL,
            config_json TEXT NOT NULL,
            summary_json TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_quiver_signal_runs_date
         ON quiver_signal_runs(run_date DESC, created_at DESC)",
        "CREATE TABLE IF NOT EXISTS quiver_asset_signals (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            run_date TEXT NOT NULL,
            status TEXT NOT NULL,
            symbol TEXT NOT NULL,
            ticker TEXT NOT NULL,
            instrument_name TEXT NOT NULL,
            source TEXT NOT NULL,
            lookback_days INTEGER NOT NULL,
            signal REAL NOT NULL,
            direction TEXT NOT NULL,
            confidence REAL NOT NULL,
            event_count INTEGER NOT NULL,
            congress_purchase_count INTEGER NOT NULL,
            congress_sale_count INTEGER NOT NULL,
            net_congress_amount REAL NOT NULL,
            latest_event_date TEXT,
            source_status_json TEXT NOT NULL,
            top_events_json TEXT NOT NULL,
            error_text TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_quiver_asset_signals_run_symbol
         ON quiver_asset_signals(run_id, symbol)",
        "CREATE INDEX IF NOT EXISTS idx_quiver_asset_signals_run_signal
         ON quiver_asset_signals(run_id, signal DESC, confidence DESC)",
    ]
}

fn parse_hh_mm(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn transaction_direction(value: &str) -> f64 {
    let lower = value.to_lowercase();
    if lower.contains("purchase") || lower.contains("buy") {
        1.0
    } else if lower.contains("sale") || lower.contains("sell") {
        -1.0
    } else {
        0.0
    }
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(trimmed, "%Y%m%d"))
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(trimmed).map(|value| value.date_naive()))
        .ok()
}

fn number_any(row: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        row.get(*key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|number| number as f64))
                .or_else(|| value.as_str().and_then(parse_number))
        })
    })
}

fn parse_number(value: &str) -> Option<f64> {
    let cleaned = value
        .trim()
        .trim_end_matches('%')
        .replace(['$', ',', '+'], "");
    cleaned.parse::<f64>().ok()
}

fn range_amount(value: &str) -> f64 {
    value
        .split(['-', '–'])
        .next()
        .and_then(parse_number)
        .unwrap_or(0.0)
}

fn text(row: &JsonValue, key: &str) -> String {
    row.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn value_f64(row: &JsonValue, key: &str) -> f64 {
    row.get(key).and_then(JsonValue::as_f64).unwrap_or(0.0)
}

fn text_any(row: &JsonValue, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            row.get(*key)
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_default()
}

fn parse_json_field(value: Option<&JsonValue>) -> Option<JsonValue> {
    match value {
        Some(JsonValue::String(text)) => serde_json::from_str(text).ok(),
        Some(value) => Some(value.clone()),
        None => None,
    }
}

fn optional_text_sql(value: Option<&str>) -> String {
    value
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_us_ticker_from_saxo_symbol() {
        assert_eq!(quiver_ticker("NVDA:xnas").as_deref(), Some("NVDA"));
        assert_eq!(quiver_ticker("BRK.B:xnys").as_deref(), Some("BRK-B"));
        assert_eq!(quiver_ticker("ADS:xetr"), None);
    }

    #[test]
    fn classifies_subscription_denial_as_terminal() {
        let err = anyhow!(
            "Quiver GET failed: HTTP 403: {{\"detail\":\"Upgrade your subscription plan\"}}"
        );
        assert_eq!(
            classify_terminal_quiver_error(&err),
            Some("subscription_required")
        );
    }

    #[test]
    fn scores_recent_congress_purchases_positive() {
        let trades = vec![CongressTrade {
            representative: "Example".to_string(),
            report_date: Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
            transaction_date: Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()),
            transaction: "Purchase".to_string(),
            range: "$1,001 - $15,000".to_string(),
            house: "House".to_string(),
            party: "Independent".to_string(),
            amount: 1001.0,
            excess_return: None,
            price_change: None,
            spy_change: None,
        }];
        let analysis =
            analyze_congress_trades(NaiveDate::from_ymd_opt(2026, 7, 3).unwrap(), 120, &trades);
        assert_eq!(analysis.direction, "bullish");
        assert!(analysis.signal > 0.0);
    }

    #[test]
    fn scores_recent_congress_sales_negative() {
        let trades = vec![CongressTrade {
            representative: "Example".to_string(),
            report_date: Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
            transaction_date: Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()),
            transaction: "Sale".to_string(),
            range: "$1,001 - $15,000".to_string(),
            house: "Senate".to_string(),
            party: "Independent".to_string(),
            amount: 1001.0,
            excess_return: None,
            price_change: None,
            spy_change: None,
        }];
        let analysis =
            analyze_congress_trades(NaiveDate::from_ymd_opt(2026, 7, 3).unwrap(), 120, &trades);
        assert_eq!(analysis.direction, "bearish");
        assert!(analysis.signal < 0.0);
    }
}
