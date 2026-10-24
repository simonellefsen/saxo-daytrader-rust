//! The daily congress-trading signal for US portfolio and watchlist assets.
//!
//! Named for QuiverQuant, its source until 2026-10-24. The trades now come
//! from Alpha Vantage through `congress_trades`, which fetches within the
//! shared daily allowance; this cycle only scores what that store holds. The
//! `quiver_*` names and tables stay because decision reports, Hermes and the
//! Jev reviews read them.

use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tracing::info;

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{clamp_limit, row_to_json, sql_escape},
    markov_method::markov_assets,
    state::AppState,
};

const DEFAULT_US_OPEN_FOLLOWUP_MINUTES: i64 = 45;
const DEFAULT_US_EXCHANGE_CODES: &[&str] = &["XNAS", "XNYS"];
const STRONG_BEARISH_CONFLICT_SIGNAL: f64 = -0.35;
/// A stock `congress_trades` has not answered for yet.
const NOT_YET_FETCHED: &str = "not_yet_fetched";

#[derive(Clone, Debug)]
struct QuiverConfig {
    enabled: bool,
    timezone: Tz,
    run_weekdays_only: bool,
    minutes_after_us_open: i64,
    exchange_codes: Vec<String>,
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

    let now = Utc::now();
    let now_local = now.with_timezone(&config.timezone);
    let run_date = now_local.date_naive();
    if config.run_weekdays_only && run_date.weekday().number_from_monday() > 5 {
        return Ok(json!({
            "status": "idle",
            "reason": "weekend",
            "run_date": run_date.to_string(),
            "timezone": config.timezone.name()
        }));
    }
    let target = current_us_open_followup_target(state, &config, now, run_date);
    let Some(target) = target else {
        return Ok(json!({
            "status": "idle",
            "reason": "no_us_session",
            "run_date": run_date.to_string(),
            "timezone": config.timezone.name(),
            "schedule_kind": "us_open_followup",
            "minutes_after_open": config.minutes_after_us_open,
            "exchange_codes": config.exchange_codes,
        }));
    };
    if now < target.target_at_utc {
        return Ok(json!({
            "status": "idle",
            "reason": "not_due",
            "run_date": run_date.to_string(),
            "scheduled_for": target.target_at_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "timezone": config.timezone.name(),
            "schedule_kind": "us_open_followup",
            "minutes_after_open": config.minutes_after_us_open,
            "exchange_codes": target.exchange_codes,
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
    let mut pending_count = 0usize;

    for asset in &assets {
        let analysis = analyze_asset(state, config, run_date, asset).await?;
        match analysis.status.as_str() {
            "ok" => success_count += 1,
            NOT_YET_FETCHED => pending_count += 1,
            _ => error_count += 1,
        }
        let row = signal_row_json(&run_id, &created_at, run_date, config, asset, &analysis);
        insert_quiver_signal(state, &row).await?;
        rows.push(row);
    }

    let status = if success_count > 0 {
        "completed"
    } else if assets.is_empty() {
        "empty"
    } else if pending_count == assets.len() {
        "awaiting_congress_trades"
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
        "not_yet_fetched_count": pending_count,
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
    state: &AppState,
    config: &QuiverConfig,
    run_date: NaiveDate,
    asset: &QuiverAsset,
) -> Result<QuiverAnalysis> {
    let Some(stored) = crate::congress_trades::stored_trades(state, &asset.ticker).await? else {
        return Ok(not_yet_fetched_analysis());
    };
    let trades = stored.iter().map(scored_trade).collect::<Vec<_>>();
    Ok(analyze_congress_trades(
        run_date,
        config.lookback_days,
        &trades,
    ))
}

fn not_yet_fetched_analysis() -> QuiverAnalysis {
    QuiverAnalysis {
        status: NOT_YET_FETCHED.to_string(),
        signal: 0.0,
        direction: "neutral".to_string(),
        confidence: 0.0,
        event_count: 0,
        congress_purchase_count: 0,
        congress_sale_count: 0,
        net_congress_amount: 0.0,
        latest_event_date: None,
        top_events: Vec::new(),
        source_status: json!({"congress_trading": {"status": NOT_YET_FETCHED, "source": "alpha_vantage"}}),
        error_text: None,
    }
}

/// An Alpha Vantage disclosure in the shape the score was written for. The
/// amount is the band's lower bound, as Quiver's range was read; Alpha
/// Vantage reports no excess return or price change.
fn scored_trade(trade: &crate::congress_trades::CongressTrade) -> CongressTrade {
    let band = |value: Option<f64>| value.map(|v| format!("{v:.0}")).unwrap_or_default();
    CongressTrade {
        representative: trade.politician.clone(),
        report_date: trade.filed_date,
        transaction_date: trade.transaction_date,
        transaction: trade.transaction_type.clone(),
        range: format!("{}-{}", band(trade.amount_min), band(trade.amount_max)),
        house: trade.chamber.clone(),
        party: trade.party.clone(),
        amount: trade.amount_min.unwrap_or(0.0),
        excess_return: None,
        price_change: None,
        spy_change: None,
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
                "matched_events": event_count,
                "source": "alpha_vantage"
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
        let Some(ticker) = crate::earnings_dates::us_ticker(&asset.symbol) else {
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
    latest_quiver_signals_page(state, limit, 0).await
}

pub async fn latest_quiver_signals_page(
    state: &AppState,
    limit: i64,
    offset: i64,
) -> Result<Vec<JsonValue>> {
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
         LIMIT {} OFFSET {}",
        clamp_limit(limit, 1, 500),
        offset.max(0).min(100_000)
    );
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.iter().map(row_to_json).collect())
}

pub async fn latest_quiver_signal_count(state: &AppState) -> Result<i64> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count
         FROM quiver_asset_signals
         WHERE run_id = (
            SELECT id
            FROM quiver_signal_runs
            ORDER BY run_date DESC, created_at DESC
            LIMIT 1
         )",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .as_ref()
        .and_then(|row| row_to_json(row).get("count").and_then(JsonValue::as_i64))
        .unwrap_or(0))
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
    let schedule = quiver_config_json_for_state(state);
    let latest_run = latest_quiver_run(state).await.unwrap_or(JsonValue::Null);
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
        "freshness": quiver_context_freshness(&latest_run, &schedule),
        "latest_run": latest_run,
        "signals": signals
    }))
}

/// States whether the latest Quiver run can be interpreted as current for the
/// calendar-derived US-open follow-up. It is descriptive context only: callers
/// must still treat Quiver data as advisory rather than a trading gate.
fn quiver_context_freshness(latest_run: &JsonValue, schedule: &JsonValue) -> JsonValue {
    let schedule_status = text(schedule, "schedule_status");
    let expected_run_date = text(schedule, "scheduled_run_date");
    let scheduled_for = text(schedule, "scheduled_for");
    let run_date = text(latest_run, "run_date");
    let run_status = text(latest_run, "status");
    let success_count = value_f64(latest_run, "success_count");
    let error_count = value_f64(latest_run, "error_count");
    let asset_count = value_f64(latest_run, "asset_count");

    let (status, instruction) = match schedule_status.as_str() {
        "no_us_session" => (
            "no_us_session",
            "No eligible US session is scheduled today. Any stored Quiver data is historical context only.",
        ),
        "waiting" => (
            "not_due",
            "Today's US-open Quiver cycle is not due yet. Any stored Quiver data is historical context only.",
        ),
        "due" if latest_run.is_null() || run_date != expected_run_date => (
            if latest_run.is_null() {
                "missing"
            } else {
                "stale"
            },
            "Today's Quiver cycle is due but no matching completed run is available. Do not treat Quiver signals as current.",
        ),
        "due" if run_status != "completed" || success_count <= 0.0 => (
            "failed",
            "Today's Quiver cycle did not produce usable signals. Do not treat Quiver signals as current.",
        ),
        "due" if error_count > 0.0 || success_count < asset_count => (
            "partial",
            "Today's Quiver cycle completed with partial coverage. Use only listed successful signals; omitted assets have no current Quiver evidence.",
        ),
        "due" => (
            "fresh",
            "Today's Quiver cycle completed with full recorded coverage. Signals remain corroborating advisory context only.",
        ),
        _ => (
            "unknown",
            "The Quiver schedule state is unavailable. Treat stored Quiver data as historical context only.",
        ),
    };

    json!({
        "status": status,
        "schedule_status": schedule_status,
        "expected_run_date": expected_run_date,
        "latest_run_date": run_date,
        "latest_run_status": run_status,
        "scheduled_for": scheduled_for,
        "asset_count": asset_count,
        "success_count": success_count,
        "error_count": error_count,
        "instruction": instruction,
        "safety": "advisory_freshness_only_no_gate_hermes_or_broker_mutation",
    })
}

/// Surfaces strong negative Quiver signals against currently held symbols.
/// Quiver remains advisory: this projection does not approve, reject, size,
/// or otherwise change an order.
pub fn held_position_conflicts(positions: &[JsonValue], context: &JsonValue) -> JsonValue {
    let held_symbols = positions
        .iter()
        .filter(|position| value_f64(position, "quantity") > 0.0)
        .map(|position| text(position, "symbol").to_uppercase())
        .filter(|symbol| !symbol.is_empty())
        .collect::<HashSet<_>>();
    let mut conflicts = context
        .get("signals")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|signal| {
            let symbol = text(signal, "symbol").to_uppercase();
            let score = value_f64(signal, "signal");
            (held_symbols.contains(&symbol) && score <= STRONG_BEARISH_CONFLICT_SIGNAL).then(|| {
                json!({
                    "symbol": symbol,
                    "signal": score,
                    "direction": text(signal, "direction"),
                    "confidence": value_f64(signal, "confidence"),
                    "event_count": signal.get("event_count").cloned().unwrap_or(JsonValue::Null),
                    "latest_event_date": signal.get("latest_event_date").cloned().unwrap_or(JsonValue::Null),
                })
            })
        })
        .collect::<Vec<_>>();
    conflicts.sort_by(|left, right| {
        value_f64(left, "signal")
            .partial_cmp(&value_f64(right, "signal"))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text(left, "symbol").cmp(&text(right, "symbol")))
    });
    let status = if held_symbols.is_empty() {
        "no_positions"
    } else if conflicts.is_empty() {
        "clear"
    } else {
        "conflicts_detected"
    };
    json!({
        "status": status,
        "held_symbol_count": held_symbols.len(),
        "strong_bearish_signal_lte": STRONG_BEARISH_CONFLICT_SIGNAL,
        "conflicts": conflicts,
        "safety": "advisory_quiver_context_only_no_gate_hermes_or_broker_mutation",
        "interpretation": "A strong bearish Congress-trading signal conflicts with a current holding and deserves explicit review. It is not an exit instruction, a causality claim, or a replacement for technical, Markov, capital, or broker evidence.",
    })
}

pub fn quiver_config_json_for_state(state: &AppState) -> JsonValue {
    let config = quiver_config(state);
    let now = Utc::now();
    let run_date = now.with_timezone(&config.timezone).date_naive();
    let mut value = quiver_config_json(&config);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if let Some(target) = current_us_open_followup_target(state, &config, now, run_date) {
        object.insert(
            "scheduled_for".to_string(),
            JsonValue::from(
                target
                    .target_at_utc
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ),
        );
        object.insert(
            "scheduled_run_date".to_string(),
            JsonValue::from(run_date.to_string()),
        );
        object.insert(
            "schedule_status".to_string(),
            JsonValue::from(if now < target.target_at_utc {
                "waiting"
            } else {
                "due"
            }),
        );
    } else {
        object.insert(
            "schedule_status".to_string(),
            JsonValue::from("no_us_session"),
        );
    }
    value
}

fn quiver_config(state: &AppState) -> QuiverConfig {
    let timezone = yaml_string(&state.config, &["strategy", "quiver", "timezone"])
        .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    QuiverConfig {
        enabled: yaml_bool(&state.config, &["strategy", "quiver", "enabled"]).unwrap_or(true),
        timezone,
        run_weekdays_only: yaml_bool(&state.config, &["strategy", "quiver", "run_weekdays_only"])
            .unwrap_or(true),
        minutes_after_us_open: yaml_i64(
            &state.config,
            &[
                "strategy",
                "quiver",
                "us_open_followup",
                "minutes_after_open",
            ],
        )
        .unwrap_or(DEFAULT_US_OPEN_FOLLOWUP_MINUTES)
        .max(0),
        exchange_codes: quiver_exchange_codes(state),
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
        "schedule_kind": "us_open_followup",
        "minutes_after_open": config.minutes_after_us_open,
        "exchange_codes": config.exchange_codes,
        "run_weekdays_only": config.run_weekdays_only,
        "max_symbols": config.max_symbols,
        "lookback_days": config.lookback_days,
        "sources": ["congress_trading"],
        "provider": "alpha_vantage"
    })
}

fn quiver_exchange_codes(state: &AppState) -> Vec<String> {
    crate::config::yaml_at(
        &state.config,
        &["strategy", "quiver", "us_open_followup", "exchange_codes"],
    )
    .and_then(|value| value.as_sequence())
    .map(|values| {
        values
            .iter()
            .filter_map(|value| value.as_str())
            .map(|code| code.trim().to_uppercase())
            .filter(|code| !code.is_empty())
            .collect::<Vec<_>>()
    })
    .filter(|codes| !codes.is_empty())
    .unwrap_or_else(|| {
        DEFAULT_US_EXCHANGE_CODES
            .iter()
            .map(|code| (*code).to_string())
            .collect()
    })
}

fn current_us_open_followup_target(
    state: &AppState,
    config: &QuiverConfig,
    now: chrono::DateTime<Utc>,
    run_date: NaiveDate,
) -> Option<crate::xai_decision::MarketOpenFollowupTarget> {
    crate::xai_decision::market_open_followup_targets(
        state,
        &config.exchange_codes,
        config.minutes_after_us_open,
    )
    .into_iter()
    .filter(|target| {
        target
            .target_at_utc
            .with_timezone(&config.timezone)
            .date_naive()
            == run_date
    })
    .min_by_key(|target| (target.target_at_utc < now, target.target_at_utc))
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

    /// Alpha Vantage's `BUY`/`SELL` score the way Quiver's `Purchase`/`Sale`
    /// did, on the band's lower bound.
    #[test]
    fn alpha_vantage_disclosures_score_like_quiver_ones() {
        let disclosure = |kind: &str| crate::congress_trades::CongressTrade {
            politician: "Example".to_string(),
            bioguide_id: "X1".to_string(),
            chamber: "HOUSE".to_string(),
            party: "R".to_string(),
            transaction_type: kind.to_string(),
            transaction_date: NaiveDate::from_ymd_opt(2026, 6, 30),
            filed_date: NaiveDate::from_ymd_opt(2026, 7, 1),
            amount_min: Some(1001.0),
            amount_max: Some(15000.0),
            owner_code: "SELF".to_string(),
        };
        let run_date = NaiveDate::from_ymd_opt(2026, 7, 3).unwrap();
        let bought = analyze_congress_trades(run_date, 120, &[scored_trade(&disclosure("BUY"))]);
        let sold = analyze_congress_trades(run_date, 120, &[scored_trade(&disclosure("SELL"))]);
        let other = analyze_congress_trades(run_date, 120, &[scored_trade(&disclosure("OTHER"))]);
        assert_eq!(bought.direction, "bullish");
        assert_eq!(sold.direction, "bearish");
        assert_eq!(other.event_count, 0);
        assert_eq!(bought.top_events[0]["amount"], json!(1001.0));
        assert_eq!(bought.top_events[0]["range"], json!("1001-15000"));
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

    #[test]
    fn surfaces_only_strong_bearish_signals_for_held_symbols() {
        let conflicts = held_position_conflicts(
            &[json!({"symbol": "NVDA:xnas", "quantity": 2.0})],
            &json!({
                "signals": [
                    {"symbol": "NVDA:xnas", "signal": -0.48, "direction": "bearish", "confidence": 0.8, "event_count": 6},
                    {"symbol": "AMZN:xnas", "signal": -0.72, "direction": "bearish", "confidence": 0.9, "event_count": 4},
                    {"symbol": "NVDA:xnas", "signal": -0.10, "direction": "bearish", "confidence": 0.3, "event_count": 1}
                ]
            }),
        );
        assert_eq!(conflicts["status"], "conflicts_detected");
        assert_eq!(conflicts["conflicts"].as_array().unwrap().len(), 1);
        assert_eq!(conflicts["conflicts"][0]["symbol"], "NVDA:XNAS");
    }

    #[test]
    fn marks_matching_completed_quiver_run_as_fresh() {
        let freshness = quiver_context_freshness(
            &json!({
                "run_date": "2026-07-30",
                "status": "completed",
                "asset_count": 60,
                "success_count": 60,
                "error_count": 0,
            }),
            &json!({
                "schedule_status": "due",
                "scheduled_run_date": "2026-07-30",
                "scheduled_for": "2026-07-30T14:15:00Z",
            }),
        );
        assert_eq!(freshness["status"], "fresh");
    }

    #[test]
    fn does_not_treat_yesterday_quiver_run_as_current_after_due_time() {
        let freshness = quiver_context_freshness(
            &json!({
                "run_date": "2026-07-29",
                "status": "completed",
                "asset_count": 60,
                "success_count": 60,
                "error_count": 0,
            }),
            &json!({
                "schedule_status": "due",
                "scheduled_run_date": "2026-07-30",
                "scheduled_for": "2026-07-30T14:15:00Z",
            }),
        );
        assert_eq!(freshness["status"], "stale");
    }

    #[test]
    fn marks_partial_quiver_coverage_explicitly() {
        let freshness = quiver_context_freshness(
            &json!({
                "run_date": "2026-07-30",
                "status": "completed",
                "asset_count": 60,
                "success_count": 53,
                "error_count": 7,
            }),
            &json!({
                "schedule_status": "due",
                "scheduled_run_date": "2026-07-30",
                "scheduled_for": "2026-07-30T14:15:00Z",
            }),
        );
        assert_eq!(freshness["status"], "partial");
    }
}
