//! A record-only earnings blackout for US BUYs.
//!
//! The 2026-10-24 earnings evaluation showed report reaction sessions gapping
//! a median 5.7% against 0.8% otherwise. Simon chose to record what a
//! blackout would do before letting one block anything: when the Trading
//! Manager queues a BUY of a US stock whose next report reacts within
//! `window_sessions` sessions, the BUY is noted as one a blackout would have
//! stopped. The order goes ahead unchanged, and nothing here is read by a
//! gate, a prompt, sizing or the broker path. `/api/evaluation/earnings`
//! later shows what those reports did to the BUYs that went ahead.
//!
//! Upcoming report dates come from Finnhub's free earnings calendar, which
//! covers US stocks only. One request for a window stops at 1,500 rows and
//! drops the busiest days, so the calendar is read one weekday at a time,
//! `days_ahead` days forward, once a day. The key travels in the
//! `X-Finnhub-Token` header, never in the URL.
//!
//! Sessions are counted as weekdays; an exchange holiday inside the window
//! makes a report look one session nearer than it is.

use std::{collections::HashSet, time::Duration as StdDuration};

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{sql_escape, value_f64},
    earnings_dates::{http_category, us_reference_symbols, us_ticker},
    models::{EarningsBlackoutObservationPayload, EarningsBlackoutPayload},
    state::{AppState, json_text},
};

const DEFAULT_BASE_URL: &str = "https://finnhub.io/api/v1";
const DEFAULT_DAYS_AHEAD: i64 = 21;
const DEFAULT_WINDOW_SESSIONS: i64 = 5;
const DEFAULT_DAILY_TIME: &str = "06:45";
/// Finnhub's free tier allows 60 calls a minute.
const REQUEST_SPACING_MS: u64 = 1_100;
/// Finnhub cuts a calendar answer off at this many rows.
const CALENDAR_ROW_CAP: usize = 1_500;
const MAX_READ_ATTEMPTS_PER_DAY: i64 = 3;

#[derive(Clone, Debug)]
struct BlackoutConfig {
    enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    days_ahead: i64,
    window_sessions: i64,
    api_key: Option<String>,
    base_url: String,
}

/// A queued BUY to note.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlackoutCandidate {
    pub execution_order_id: i64,
    pub symbol: String,
    pub estimated_value_dkk: Option<f64>,
}

/// One upcoming report from the calendar.
#[derive(Clone, Debug, PartialEq)]
struct UpcomingReport {
    ticker: String,
    report_date: NaiveDate,
    /// `pre-market`, `post-market` or empty, as in `earnings_report_dates`.
    report_time: String,
    eps_estimate: Option<f64>,
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS upcoming_earnings (
            ticker TEXT NOT NULL,
            report_date TEXT NOT NULL,
            report_time TEXT,
            eps_estimate DOUBLE PRECISION,
            source TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (ticker, report_date)
        )",
        "CREATE TABLE IF NOT EXISTS earnings_calendar_reads (
            read_date TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            days_read INTEGER NOT NULL DEFAULT 0,
            reports_stored INTEGER NOT NULL DEFAULT 0,
            truncated_days INTEGER NOT NULL DEFAULT 0,
            finished_at TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS earnings_blackout_observations (
            execution_order_id INTEGER PRIMARY KEY,
            report_id INTEGER NOT NULL,
            recorded_at TEXT NOT NULL,
            symbol TEXT NOT NULL,
            ticker TEXT NOT NULL,
            estimated_value_dkk DOUBLE PRECISION,
            next_report_date TEXT,
            report_time TEXT,
            reaction_date TEXT,
            sessions_until_reaction INTEGER,
            would_block INTEGER NOT NULL,
            window_sessions INTEGER NOT NULL,
            calendar_status TEXT NOT NULL
        )",
    ]
}

fn blackout_config(state: &AppState) -> BlackoutConfig {
    let key = |suffix: &'static str| ["strategy", "earnings_blackout", suffix];
    BlackoutConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(false),
        timezone: yaml_string(&state.config, &key("timezone"))
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen),
        daily_time: yaml_string(&state.config, &key("daily_time"))
            .and_then(|value| NaiveTime::parse_from_str(&value, "%H:%M").ok())
            .unwrap_or_else(|| NaiveTime::parse_from_str(DEFAULT_DAILY_TIME, "%H:%M").unwrap()),
        days_ahead: yaml_i64(&state.config, &key("days_ahead"))
            .unwrap_or(DEFAULT_DAYS_AHEAD)
            .clamp(1, 60),
        window_sessions: yaml_i64(&state.config, &key("window_sessions"))
            .unwrap_or(DEFAULT_WINDOW_SESSIONS)
            .clamp(0, 30),
        api_key: yaml_string(&state.config, &["finnhub", "api_key"])
            .or_else(|| std::env::var("FINNHUBIO_API_KEY").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        base_url: yaml_string(&state.config, &["finnhub", "base_url"])
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
    }
}

/// Reads one day of Finnhub's `/calendar/earnings`, keeping `tickers`.
/// Returns the reports and whether the answer hit the row cap.
fn parse_calendar_day(
    body: &str,
    day: NaiveDate,
    tickers: &HashSet<String>,
) -> Result<(Vec<UpcomingReport>, bool), &'static str> {
    let payload = serde_json::from_str::<JsonValue>(body).map_err(|_| "parse_error")?;
    let rows = payload
        .get("earningsCalendar")
        .and_then(JsonValue::as_array)
        .ok_or("no_data")?;
    let reports = rows
        .iter()
        .filter_map(|row| {
            let ticker = json_text(row, "symbol")
                .trim()
                .to_uppercase()
                .replace('.', "-");
            let report_date =
                NaiveDate::parse_from_str(&json_text(row, "date"), "%Y-%m-%d").ok()?;
            (report_date == day && tickers.contains(&ticker)).then(|| UpcomingReport {
                ticker,
                report_date,
                report_time: match json_text(row, "hour").trim().to_lowercase().as_str() {
                    "bmo" => "pre-market",
                    "amc" => "post-market",
                    _ => "",
                }
                .to_string(),
                eps_estimate: row.get("epsEstimate").and_then(JsonValue::as_f64),
            })
        })
        .collect();
    Ok((reports, rows.len() >= CALENDAR_ROW_CAP))
}

fn weekdays_from(start: NaiveDate, days_ahead: i64) -> Vec<NaiveDate> {
    (0..=days_ahead)
        .map(|offset| start + Duration::days(offset))
        .filter(|day| !matches!(day.weekday(), Weekday::Sat | Weekday::Sun))
        .collect()
}

pub async fn run_earnings_calendar_cycle(state: &AppState) -> Result<JsonValue> {
    let config = blackout_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let Some(api_key) = config.api_key.clone() else {
        return Ok(json!({
            "status": "disabled",
            "reason": "missing_api_key",
            "api_key_env": "FINNHUBIO_API_KEY",
        }));
    };
    let now = Utc::now();
    let local = now.with_timezone(&config.timezone);
    let read_date = local.date_naive();
    if local.time() < config.daily_time {
        return Ok(
            json!({"status": "idle", "reason": "not_due", "read_date": read_date.to_string()}),
        );
    }
    let previous = state
        .select_json(&format!(
            "SELECT status, attempts FROM earnings_calendar_reads WHERE read_date = '{read_date}'"
        ))
        .await?;
    if let Some(row) = previous.first() {
        if json_text(row, "status") == "ok" {
            return Ok(
                json!({"status": "idle", "reason": "already_read", "read_date": read_date.to_string()}),
            );
        }
        if row.get("attempts").and_then(JsonValue::as_i64).unwrap_or(0) >= MAX_READ_ATTEMPTS_PER_DAY
        {
            return Ok(
                json!({"status": "idle", "reason": "attempts_spent", "read_date": read_date.to_string()}),
            );
        }
    }
    let tickers = us_reference_symbols(state)
        .await?
        .into_iter()
        .map(|symbol| symbol.ticker)
        .collect::<HashSet<_>>();
    let us_today = now
        .with_timezone(&chrono_tz::America::New_York)
        .date_naive();
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let fetched_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut days_read = 0usize;
    let mut stored = 0usize;
    let mut truncated_days = 0usize;
    let mut failure = None;
    for (index, day) in weekdays_from(us_today, config.days_ahead)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            sleep(StdDuration::from_millis(REQUEST_SPACING_MS)).await;
        }
        let answer = fetch_calendar_day(&client, &config, &api_key, day, &tickers).await;
        let (reports, truncated) = match answer {
            Ok(answer) => answer,
            Err(category) => {
                failure = Some(category);
                break;
            }
        };
        store_calendar_day(state, day, &reports, &fetched_at).await?;
        days_read += 1;
        stored += reports.len();
        truncated_days += usize::from(truncated);
    }
    let status = failure.unwrap_or("ok");
    sqlx::query(&format!(
        "INSERT INTO earnings_calendar_reads (
            read_date, status, attempts, days_read, reports_stored, truncated_days, finished_at
         ) VALUES ('{read_date}', '{status}', 1, {days_read}, {stored}, {truncated_days}, '{fetched_at}')
         ON CONFLICT(read_date) DO UPDATE SET
            status = excluded.status,
            attempts = earnings_calendar_reads.attempts + 1,
            days_read = excluded.days_read,
            reports_stored = excluded.reports_stored,
            truncated_days = excluded.truncated_days,
            finished_at = excluded.finished_at"
    ))
    .execute(&state.pool)
    .await
    .context("recording an earnings calendar read")?;
    if let Some(category) = failure {
        warn!(category, days_read, "earnings calendar read stopped early");
    }
    info!(days_read, stored, "earnings calendar read completed");
    Ok(json!({
        "status": if failure.is_some() { "partial" } else { "completed" },
        "failure": failure,
        "days_read": days_read,
        "reports_stored": stored,
        "truncated_days": truncated_days,
        "scope": "record_only_earnings_blackout",
    }))
}

async fn fetch_calendar_day(
    client: &reqwest::Client,
    config: &BlackoutConfig,
    api_key: &str,
    day: NaiveDate,
    tickers: &HashSet<String>,
) -> Result<(Vec<UpcomingReport>, bool), &'static str> {
    let day_text = day.to_string();
    let response = client
        .get(format!(
            "{}/calendar/earnings",
            config.base_url.trim_end_matches('/')
        ))
        .header("X-Finnhub-Token", api_key)
        .query(&[("from", day_text.as_str()), ("to", day_text.as_str())])
        .send()
        .await
        .map_err(crate::debug_redaction::request_error_without_url);
    let response = match response {
        Ok(response) => response,
        Err(err) if err.is_timeout() => return Err("timeout"),
        Err(_) => return Err("request_failed"),
    };
    let status = response.status();
    if !status.is_success() {
        return Err(http_category(status.as_u16()));
    }
    parse_calendar_day(&response.text().await.unwrap_or_default(), day, tickers)
}

/// Replaces one day: a report that moved is dropped from the day it left
/// when that day is read again in the same pass.
async fn store_calendar_day(
    state: &AppState,
    day: NaiveDate,
    reports: &[UpcomingReport],
    fetched_at: &str,
) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    sqlx::query(&format!(
        "DELETE FROM upcoming_earnings WHERE report_date = '{day}'"
    ))
    .execute(&mut *tx)
    .await
    .context("clearing a calendar day")?;
    if !reports.is_empty() {
        let values = reports
            .iter()
            .map(|report| {
                format!(
                    "('{}', '{}', '{}', {}, 'finnhub', '{fetched_at}')",
                    sql_escape(&report.ticker),
                    report.report_date,
                    report.report_time,
                    report
                        .eps_estimate
                        .filter(|value| value.is_finite())
                        .map_or_else(|| "NULL".to_string(), |value| value.to_string()),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO upcoming_earnings (
                ticker, report_date, report_time, eps_estimate, source, fetched_at
             ) VALUES {values}"
        ))
        .execute(&mut *tx)
        .await
        .context("storing a calendar day")?;
    }
    tx.commit().await?;
    Ok(())
}

/// The first session that can react to a report: the report day for one
/// before the open, the next weekday for one after the close, and the report
/// day when the timing is unknown, as the earliest it could be.
fn reaction_date(report_date: NaiveDate, report_time: &str) -> NaiveDate {
    if report_time == "post-market" {
        next_weekday(report_date)
    } else {
        report_date
    }
}

fn next_weekday(day: NaiveDate) -> NaiveDate {
    let mut next = day + Duration::days(1);
    while matches!(next.weekday(), Weekday::Sat | Weekday::Sun) {
        next += Duration::days(1);
    }
    next
}

/// Weekday sessions after `today` up to and including `day`: 0 when `day` is
/// today, 1 for the next session.
fn sessions_until(today: NaiveDate, day: NaiveDate) -> i64 {
    let mut count = 0;
    let mut cursor = today;
    while cursor < day {
        cursor += Duration::days(1);
        if !matches!(cursor.weekday(), Weekday::Sat | Weekday::Sun) {
            count += 1;
        }
    }
    count
}

/// Notes each queued US BUY a blackout of `window_sessions` would have
/// stopped. It never changes or delays an order, and a failure here is
/// reported in the result rather than raised.
pub(crate) async fn record_buy_observations(
    state: &AppState,
    report_id: i64,
    candidates: &[BlackoutCandidate],
) -> JsonValue {
    let config = blackout_config(state);
    if !config.enabled {
        return json!({"mode": "record_only", "status": "disabled"});
    }
    match record(state, &config, report_id, candidates).await {
        Ok(value) => value,
        Err(err) => {
            warn!("earnings blackout observation failed: {err:#}");
            json!({"mode": "record_only", "status": "unavailable", "reason": err.to_string()})
        }
    }
}

async fn record(
    state: &AppState,
    config: &BlackoutConfig,
    report_id: i64,
    candidates: &[BlackoutCandidate],
) -> Result<JsonValue> {
    let now = Utc::now();
    let recorded_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let today = now
        .with_timezone(&chrono_tz::America::New_York)
        .date_naive();
    let calendar_status = calendar_status(state, config, now).await?;
    let mut observed = 0usize;
    let mut would_block = Vec::new();
    for candidate in candidates {
        let Some(ticker) = us_ticker(&candidate.symbol) else {
            continue;
        };
        let next = state
            .select_json(&format!(
                "SELECT report_date, report_time FROM upcoming_earnings
                 WHERE ticker = '{}' AND report_date >= '{today}'
                 ORDER BY report_date LIMIT 1",
                sql_escape(&ticker)
            ))
            .await?;
        let next = next.first().and_then(|row| {
            let date =
                NaiveDate::parse_from_str(&json_text(row, "report_date"), "%Y-%m-%d").ok()?;
            Some((date, json_text(row, "report_time")))
        });
        let reaction = next.as_ref().map(|(date, time)| reaction_date(*date, time));
        let sessions = reaction.map(|day| sessions_until(today, day));
        let blocks = sessions.is_some_and(|sessions| sessions <= config.window_sessions);
        let text = |value: Option<String>| {
            value.map_or_else(|| "NULL".to_string(), |v| format!("'{}'", sql_escape(&v)))
        };
        sqlx::query(&format!(
            "INSERT INTO earnings_blackout_observations (
                execution_order_id, report_id, recorded_at, symbol, ticker, estimated_value_dkk,
                next_report_date, report_time, reaction_date, sessions_until_reaction,
                would_block, window_sessions, calendar_status
             ) VALUES ({}, {report_id}, '{recorded_at}', '{}', '{}', {}, {}, {}, {}, {}, {}, {}, '{}')
             ON CONFLICT(execution_order_id) DO NOTHING",
            candidate.execution_order_id,
            sql_escape(&candidate.symbol),
            sql_escape(&ticker),
            candidate
                .estimated_value_dkk
                .filter(|value| value.is_finite())
                .map_or_else(|| "NULL".to_string(), |value| value.to_string()),
            text(next.as_ref().map(|(date, _)| date.to_string())),
            text(next.as_ref().map(|(_, time)| time.clone())),
            text(reaction.map(|day| day.to_string())),
            sessions.map_or_else(|| "NULL".to_string(), |value| value.to_string()),
            i64::from(blocks),
            config.window_sessions,
            calendar_status,
        ))
        .execute(&state.pool)
        .await
        .context("recording an earnings blackout observation")?;
        observed += 1;
        if blocks {
            would_block.push(json!({
                "symbol": candidate.symbol,
                "next_report_date": next.as_ref().map(|(date, _)| date.to_string()),
                "report_time": next.as_ref().map(|(_, time)| time.clone()),
                "sessions_until_reaction": sessions,
            }));
        }
    }
    Ok(json!({
        "mode": "record_only",
        "status": "recorded",
        "window_sessions": config.window_sessions,
        "calendar_status": calendar_status,
        "observed_us_buys": observed,
        "would_block": would_block,
    }))
}

/// `fresh` when the calendar was read today or yesterday (local date), else
/// `stale`, or `never_read`.
async fn calendar_status(
    state: &AppState,
    config: &BlackoutConfig,
    now: chrono::DateTime<Utc>,
) -> Result<&'static str> {
    let rows = state
        .select_json(
            "SELECT MAX(read_date) AS last_read FROM earnings_calendar_reads WHERE status = 'ok'",
        )
        .await?;
    let last = rows
        .first()
        .map(|row| json_text(row, "last_read"))
        .and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok());
    let today = now.with_timezone(&config.timezone).date_naive();
    Ok(match last {
        None => "never_read",
        Some(day) if today - day <= Duration::days(1) => "fresh",
        Some(_) => "stale",
    })
}

/// The recorded observations, with what each report did once it has passed.
pub(crate) async fn blackout_payload(state: &AppState) -> Result<EarningsBlackoutPayload> {
    let config = blackout_config(state);
    let last_read = state
        .select_json(
            "SELECT read_date, status, days_read, reports_stored, truncated_days
             FROM earnings_calendar_reads ORDER BY read_date DESC LIMIT 1",
        )
        .await?;
    let upcoming = state
        .select_json("SELECT COUNT(*) AS reports FROM upcoming_earnings")
        .await?;
    let rows = state
        .select_json(
            "SELECT o.execution_order_id, o.recorded_at, o.symbol, o.estimated_value_dkk,
                    o.next_report_date, o.report_time, o.reaction_date, o.sessions_until_reaction,
                    o.would_block, o.calendar_status, e.status AS order_status, e.quantity,
                    e.currency
             FROM earnings_blackout_observations o
             LEFT JOIN execution_orders e ON e.id = o.execution_order_id
             ORDER BY o.recorded_at DESC
             LIMIT 200",
        )
        .await?;
    let today = Utc::now().date_naive().to_string();
    let mut observations = Vec::new();
    for row in &rows {
        let reaction = json_text(row, "reaction_date");
        let gap = if !reaction.is_empty() && reaction <= today {
            reaction_gap(state, &json_text(row, "symbol"), &reaction).await?
        } else {
            None
        };
        let (gap_pct, previous_close) = gap.unzip();
        let executed = json_text(row, "order_status") == "executed";
        let quantity = value_f64(row, "quantity");
        let currency = json_text(row, "currency");
        let gap_pnl_local = match (gap_pct, previous_close) {
            (Some(gap), Some(close)) if executed && quantity > 0.0 => {
                Some(quantity * close * gap / 100.0)
            }
            _ => None,
        };
        let gap_pnl_dkk = match gap_pnl_local {
            Some(pnl) => {
                Some(pnl * crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, &currency).await)
            }
            None => None,
        };
        observations.push(EarningsBlackoutObservationPayload {
            execution_order_id: row
                .get("execution_order_id")
                .and_then(JsonValue::as_i64)
                .unwrap_or_default(),
            recorded_at: json_text(row, "recorded_at"),
            symbol: json_text(row, "symbol"),
            estimated_value_dkk: row.get("estimated_value_dkk").and_then(JsonValue::as_f64),
            next_report_date: Some(json_text(row, "next_report_date")).filter(|v| !v.is_empty()),
            report_time: json_text(row, "report_time"),
            reaction_date: Some(reaction).filter(|v| !v.is_empty()),
            sessions_until_reaction: row
                .get("sessions_until_reaction")
                .and_then(JsonValue::as_i64),
            would_block: row.get("would_block").and_then(JsonValue::as_i64) == Some(1),
            calendar_status: json_text(row, "calendar_status"),
            order_status: json_text(row, "order_status"),
            reaction_gap_pct: gap_pct,
            gap_pnl_dkk,
        });
    }
    let would_block = observations.iter().filter(|row| row.would_block).count();
    let measured_blocked = observations
        .iter()
        .filter(|row| row.would_block && row.reaction_gap_pct.is_some())
        .count();
    let blocked_gap_pnl_dkk = observations
        .iter()
        .filter(|row| row.would_block)
        .filter_map(|row| row.gap_pnl_dkk)
        .reduce(|left, right| left + right);
    let last = last_read.first();
    Ok(EarningsBlackoutPayload {
        mode: "record_only".to_string(),
        enabled: config.enabled,
        window_sessions: config.window_sessions,
        calendar_last_read: last.map(|row| json_text(row, "read_date")),
        calendar_last_status: last.map(|row| json_text(row, "status")),
        calendar_truncated_days: last
            .and_then(|row| row.get("truncated_days"))
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        upcoming_reports_stored: upcoming
            .first()
            .and_then(|row| row.get("reports"))
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        observed_us_buys: observations.len() as i64,
        would_block_buys: would_block as i64,
        measured_blocked_buys: measured_blocked as i64,
        blocked_gap_pnl_dkk,
        observations,
    })
}

/// The gap at the open of `reaction_date` (or the first bar within five days
/// after it), with the previous close it is measured from.
async fn reaction_gap(
    state: &AppState,
    symbol: &str,
    reaction_date: &str,
) -> Result<Option<(f64, f64)>> {
    let bars = state
        .select_json(&format!(
            "SELECT bar_date, open, close FROM daily_price_bars
             WHERE symbol = '{}' AND bar_date >= '{}' ORDER BY bar_date LIMIT 12",
            sql_escape(symbol),
            NaiveDate::parse_from_str(reaction_date, "%Y-%m-%d")
                .map(|day| (day - Duration::days(10)).to_string())
                .unwrap_or_default(),
        ))
        .await?
        .iter()
        .filter_map(|row| {
            let close = value_f64(row, "close");
            (close > 0.0).then(|| crate::earnings_evaluation::Bar {
                date: json_text(row, "bar_date"),
                open: row
                    .get("open")
                    .and_then(JsonValue::as_f64)
                    .filter(|v| *v > 0.0),
                close,
            })
        })
        .collect::<Vec<_>>();
    let report = crate::earnings_evaluation::Report {
        reported_date: reaction_date.to_string(),
        report_time: "pre-market".to_string(),
    };
    Ok(
        crate::earnings_evaluation::reaction_index(&bars, &report).and_then(|index| {
            let gap = crate::earnings_evaluation::gap_pct(&bars, index)?;
            Some((gap, bars[index - 1].close))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn calendar_days_keep_universe_tickers_and_read_the_timing() {
        let tickers = ["MSFT", "BRK-B", "GOOGL"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();
        let body = r#"{"earningsCalendar":[
            {"symbol":"MSFT","date":"2026-10-28","hour":"amc","epsEstimate":3.1},
            {"symbol":"BRK.B","date":"2026-10-28","hour":"bmo","epsEstimate":null},
            {"symbol":"GOOGL","date":"2026-10-28","hour":"","epsEstimate":2.0},
            {"symbol":"ZZZ","date":"2026-10-28","hour":"bmo"},
            {"symbol":"GOOGL","date":"2026-10-29","hour":"bmo"}
        ]}"#;
        let (reports, truncated) = parse_calendar_day(body, day("2026-10-28"), &tickers).unwrap();
        assert!(!truncated);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].report_time, "post-market");
        assert_eq!(reports[1].ticker, "BRK-B");
        assert_eq!(reports[1].report_time, "pre-market");
        assert_eq!(reports[2].report_time, "");
        assert_eq!(
            parse_calendar_day("{}", day("2026-10-28"), &tickers),
            Err("no_data")
        );
    }

    #[test]
    fn a_full_answer_is_flagged_as_cut_off() {
        let rows = (0..CALENDAR_ROW_CAP)
            .map(|index| json!({"symbol": format!("T{index}"), "date": "2026-11-12", "hour": ""}))
            .collect::<Vec<_>>();
        let body = json!({"earningsCalendar": rows}).to_string();
        let (_, truncated) = parse_calendar_day(&body, day("2026-11-12"), &HashSet::new()).unwrap();
        assert!(truncated);
    }

    /// A report after Friday's close reacts on Monday, one session after a
    /// Friday BUY; a report before Monday's open reacts that Monday.
    #[test]
    fn reaction_sessions_follow_the_timing_and_skip_weekends() {
        let friday = day("2026-10-23");
        assert_eq!(reaction_date(friday, "post-market"), day("2026-10-26"));
        assert_eq!(
            reaction_date(day("2026-10-26"), "pre-market"),
            day("2026-10-26")
        );
        assert_eq!(reaction_date(day("2026-10-26"), ""), day("2026-10-26"));
        assert_eq!(sessions_until(friday, friday), 0);
        assert_eq!(sessions_until(friday, day("2026-10-26")), 1);
        assert_eq!(sessions_until(friday, day("2026-10-30")), 5);
        assert_eq!(sessions_until(friday, day("2026-11-02")), 6);
        assert_eq!(weekdays_from(friday, 3), vec![friday, day("2026-10-26")]);
    }

    async fn test_state(enabled: bool) -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory blackout database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create blackout tables");
        }
        let config = serde_yaml::from_str(&format!(
            "strategy:\n  earnings_blackout:\n    enabled: {enabled}\n    window_sessions: 5\n"
        ))
        .unwrap();
        AppState {
            config_path: std::path::PathBuf::from("blackout-test.yaml"),
            config,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// A US BUY whose report reacts inside the window is noted as one a
    /// blackout would stop; one reporting later, and a non-US BUY, are not.
    #[tokio::test]
    async fn queued_buys_are_noted_without_being_touched() {
        let state = test_state(true).await;
        let today = Utc::now()
            .with_timezone(&chrono_tz::America::New_York)
            .date_naive();
        let soon = next_weekday(today);
        let later = today + Duration::days(30);
        store_calendar_day(
            &state,
            soon,
            &[UpcomingReport {
                ticker: "MSFT".to_string(),
                report_date: soon,
                report_time: "pre-market".to_string(),
                eps_estimate: Some(3.1),
            }],
            "2026-10-24T06:45:00Z",
        )
        .await
        .unwrap();
        store_calendar_day(
            &state,
            later,
            &[UpcomingReport {
                ticker: "AAPL".to_string(),
                report_date: later,
                report_time: "post-market".to_string(),
                eps_estimate: None,
            }],
            "2026-10-24T06:45:00Z",
        )
        .await
        .unwrap();
        let candidate = |id: i64, symbol: &str| BlackoutCandidate {
            execution_order_id: id,
            symbol: symbol.to_string(),
            estimated_value_dkk: Some(5_000.0),
        };
        let result = record_buy_observations(
            &state,
            7,
            &[
                candidate(1, "MSFT:xnas"),
                candidate(2, "AAPL:xnas"),
                candidate(3, "NOVO-B:xcse"),
            ],
        )
        .await;
        assert_eq!(result["status"], "recorded");
        assert_eq!(result["observed_us_buys"], 2);
        assert_eq!(result["calendar_status"], "never_read");
        assert_eq!(result["would_block"].as_array().unwrap().len(), 1);
        assert_eq!(result["would_block"][0]["symbol"], "MSFT:xnas");

        let rows = state
            .select_json(
                "SELECT execution_order_id, would_block, sessions_until_reaction
                 FROM earnings_blackout_observations ORDER BY execution_order_id",
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["would_block"].as_i64(), Some(1));
        assert_eq!(rows[0]["sessions_until_reaction"].as_i64(), Some(1));
        assert_eq!(rows[1]["would_block"].as_i64(), Some(0));

        // Recording the same order again changes nothing.
        record_buy_observations(&state, 7, &[candidate(1, "MSFT:xnas")]).await;
        let count = state
            .select_json("SELECT COUNT(*) AS rows FROM earnings_blackout_observations")
            .await
            .unwrap();
        assert_eq!(count[0]["rows"].as_i64(), Some(2));
    }

    #[tokio::test]
    async fn a_disabled_blackout_records_nothing() {
        let state = test_state(false).await;
        let result = record_buy_observations(
            &state,
            7,
            &[BlackoutCandidate {
                execution_order_id: 1,
                symbol: "MSFT:xnas".to_string(),
                estimated_value_dkk: None,
            }],
        )
        .await;
        assert_eq!(result["status"], "disabled");
        let count = state
            .select_json("SELECT COUNT(*) AS rows FROM earnings_blackout_observations")
            .await
            .unwrap();
        assert_eq!(count[0]["rows"].as_i64(), Some(0));
    }
}
