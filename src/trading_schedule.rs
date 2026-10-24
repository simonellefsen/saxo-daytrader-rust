//! Saxo's trading schedule for each held instrument and each exchange traded.
//!
//! Market-open decisions derive from `/ref/v1/exchanges`, which describes an
//! exchange's next sessions and cannot express a half day or a holiday ahead
//! of time. `/ref/v1/instruments/tradingschedule/{Uic}/{AssetType}` gives the
//! sessions of one instrument over the coming days. The roadmap has listed it
//! as open since July.
//!
//! Once a day this reads the schedule of every held instrument plus one
//! instrument from each other exchange in the universe, and shows per
//! exchange: closed weekdays, early closes, and instruments whose sessions
//! differ from their exchange's usual schedule.
//!
//! Read-only. Market-open decisions, the protective-stop sweep and the
//! Trading Manager still use the exchange-level view; whether they should
//! switch is a later decision, made with this evidence. A failure is stored
//! as a category, never as Saxo's error text.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::sql_escape,
    markov_method::{resolve_instrument, saxo_get_json},
    models::{ExchangeTradingSchedulePayload, ScheduleEarlyClosePayload, TradingSchedulePayload},
    state::{AppState, json_text},
};

const DEFAULT_DAILY_TIME: &str = "06:30";
const DEFAULT_MAX_INSTRUMENTS: i64 = 60;
/// Saxo's name for a regular continuous session.
const REGULAR_STATE: &str = "AutomatedTrading";
/// A regular session this much shorter than the exchange's usual length
/// counts as an early close.
const EARLY_CLOSE_MINUTES: i64 = 30;

#[derive(Clone, Debug)]
struct TradingScheduleConfig {
    enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    run_weekdays_only: bool,
    max_instruments: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Session {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub state: String,
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &["CREATE TABLE IF NOT EXISTS instrument_trading_schedules (
        symbol TEXT PRIMARY KEY,
        exchange TEXT NOT NULL,
        uic BIGINT,
        asset_type TEXT,
        status TEXT NOT NULL,
        sessions_json TEXT,
        time_zone_abbreviation TEXT,
        fetched_at TEXT NOT NULL,
        run_date TEXT NOT NULL
    )"]
}

fn trading_schedule_config(state: &AppState) -> TradingScheduleConfig {
    let key = |suffix: &'static str| ["strategy", "trading_schedules", suffix];
    TradingScheduleConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(false),
        timezone: yaml_string(&state.config, &key("timezone"))
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen),
        daily_time: yaml_string(&state.config, &key("daily_time"))
            .and_then(|value| NaiveTime::parse_from_str(&value, "%H:%M").ok())
            .unwrap_or_else(|| {
                NaiveTime::parse_from_str(DEFAULT_DAILY_TIME, "%H:%M")
                    .expect("default schedule time is valid")
            }),
        run_weekdays_only: yaml_bool(&state.config, &key("run_weekdays_only")).unwrap_or(true),
        max_instruments: yaml_i64(&state.config, &key("max_instruments"))
            .unwrap_or(DEFAULT_MAX_INSTRUMENTS)
            .clamp(1, 300) as usize,
    }
}

fn exchange_of(symbol: &str) -> String {
    symbol
        .rsplit_once(':')
        .map(|(_, exchange)| exchange.trim().to_uppercase())
        .unwrap_or_default()
}

/// Every held instrument, then the first instrument of each exchange not yet
/// covered, in universe order.
pub(crate) fn schedule_targets(
    held: &[String],
    universe: &[String],
    max_instruments: usize,
) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    let mut exchanges = HashSet::new();
    for symbol in held {
        if !symbol.is_empty() && seen.insert(symbol.clone()) {
            exchanges.insert(exchange_of(symbol));
            targets.push(symbol.clone());
        }
    }
    for symbol in universe {
        let exchange = exchange_of(symbol);
        if !exchange.is_empty() && exchanges.insert(exchange) && seen.insert(symbol.clone()) {
            targets.push(symbol.clone());
        }
    }
    targets.truncate(max_instruments);
    targets
}

/// The sessions in a trading-schedule answer, oldest first.
pub(crate) fn parse_sessions(payload: &JsonValue) -> Vec<Session> {
    let items = ["Sessions", "TradingSessions", "Data"]
        .iter()
        .find_map(|key| payload.get(*key).and_then(JsonValue::as_array))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let time = |item: &JsonValue, key: &str| {
        item.get(key)
            .and_then(JsonValue::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    };
    let mut sessions = items
        .iter()
        .filter_map(|item| {
            Some(Session {
                start: time(item, "StartTime")?,
                end: time(item, "EndTime")?,
                state: item.get("State").and_then(JsonValue::as_str)?.to_string(),
            })
        })
        .filter(|session| session.end > session.start)
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.start);
    sessions
}

fn sessions_json(sessions: &[Session]) -> String {
    JsonValue::Array(
        sessions
            .iter()
            .map(|session| {
                json!({
                    "start": session.start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "end": session.end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "state": session.state,
                })
            })
            .collect(),
    )
    .to_string()
}

/// The stored sessions of one row.
///
/// `row_to_json` decodes a `_json` column into JSON already, so this takes
/// the decoded array; text is still accepted in case decoding was skipped.
fn sessions_from_stored(value: Option<&JsonValue>) -> Vec<Session> {
    let decoded;
    let payload = match value {
        Some(JsonValue::String(text)) => {
            decoded = serde_json::from_str::<JsonValue>(text).unwrap_or(JsonValue::Null);
            &decoded
        }
        Some(value) => value,
        None => return Vec::new(),
    };
    let time = |item: &JsonValue, key: &str| {
        item.get(key)
            .and_then(JsonValue::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    };
    payload
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            Some(Session {
                start: time(item, "start")?,
                end: time(item, "end")?,
                state: item.get("state").and_then(JsonValue::as_str)?.to_string(),
            })
        })
        .collect()
}

/// A Saxo error as a category: its HTTP status when the message carries one.
fn error_category(err: &anyhow::Error) -> String {
    let message = format!("{err:#}");
    message
        .find("HTTP ")
        .and_then(|index| message.get(index + 5..index + 8))
        .filter(|code| code.chars().all(|ch| ch.is_ascii_digit()))
        .map(|code| format!("http_{code}"))
        .unwrap_or_else(|| "request_failed".to_string())
}

pub async fn run_trading_schedule_cycle(state: &AppState) -> Result<JsonValue> {
    let config = trading_schedule_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let now_local = Utc::now().with_timezone(&config.timezone);
    let run_date = now_local.date_naive();
    if config.run_weekdays_only && run_date.weekday().number_from_monday() > 5 {
        return Ok(
            json!({"status": "idle", "reason": "weekend", "run_date": run_date.to_string()}),
        );
    }
    if now_local.time() < config.daily_time {
        return Ok(json!({
            "status": "idle",
            "reason": "not_due",
            "run_date": run_date.to_string(),
            "due_time": config.daily_time.format("%H:%M").to_string(),
        }));
    }
    let ran = state
        .select_json(&format!(
            "SELECT symbol FROM instrument_trading_schedules WHERE run_date = '{run_date}' LIMIT 1"
        ))
        .await?;
    if !ran.is_empty() {
        return Ok(
            json!({"status": "skipped", "reason": "already_ran", "run_date": run_date.to_string()}),
        );
    }
    let session = state
        .ensure_saxo_session_json("trading_schedules")
        .await
        .context("loading Saxo session for trading schedules")?;
    let held = state
        .position_items(250)
        .await
        .unwrap_or_default()
        .iter()
        .map(|row| json_text(row, "symbol"))
        .collect::<Vec<_>>();
    let universe = crate::markov_method::markov_assets(state, 0)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|asset| asset.symbol)
        .collect::<Vec<_>>();
    let targets = schedule_targets(&held, &universe, config.max_instruments);
    let fetched_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut by_status = BTreeMap::<String, usize>::new();
    for symbol in &targets {
        let (status, instrument, sessions, zone) =
            match resolve_instrument(state, &session, symbol).await {
                Err(_) => ("unresolved".to_string(), None, Vec::new(), String::new()),
                Ok(instrument) => {
                    let path = format!(
                        "/ref/v1/instruments/tradingschedule/{}/{}",
                        instrument.uic, instrument.asset_type
                    );
                    match saxo_get_json(state, &session, &path, &[]).await {
                        Ok(payload) => {
                            let sessions = parse_sessions(&payload);
                            let zone = json_text(&payload, "TimeZoneAbbreviation");
                            let status = if sessions.is_empty() {
                                "no_sessions"
                            } else {
                                "ok"
                            };
                            (status.to_string(), Some(instrument), sessions, zone)
                        }
                        Err(err) => {
                            let category = error_category(&err);
                            warn!(symbol, category, "Saxo trading schedule not stored");
                            (category, Some(instrument), Vec::new(), String::new())
                        }
                    }
                }
            };
        *by_status.entry(status.clone()).or_default() += 1;
        let stored = StoredSchedule {
            symbol,
            uic: instrument.as_ref().map(|instrument| instrument.uic),
            asset_type: instrument
                .as_ref()
                .map(|instrument| instrument.asset_type.as_str()),
            status: &status,
            sessions: &sessions,
            zone: &zone,
        };
        store_schedule(state, &stored, &fetched_at, run_date).await?;
    }
    info!(
        instruments = targets.len(),
        ?by_status,
        "Saxo trading schedule refresh completed"
    );
    Ok(json!({
        "status": "completed",
        "run_date": run_date.to_string(),
        "instruments": targets.len(),
        "by_status": by_status,
        "scope": "read_only_reference_data",
    }))
}

/// One instrument's read, as stored.
pub(crate) struct StoredSchedule<'a> {
    pub symbol: &'a str,
    pub uic: Option<i64>,
    pub asset_type: Option<&'a str>,
    pub status: &'a str,
    pub sessions: &'a [Session],
    pub zone: &'a str,
}

/// Upserts one instrument's read. A failed read keeps the sessions stored
/// before it, under the failure's status, so the view never shows them as
/// current.
pub(crate) async fn store_schedule(
    state: &AppState,
    stored: &StoredSchedule<'_>,
    fetched_at: &str,
    run_date: NaiveDate,
) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO instrument_trading_schedules (
            symbol, exchange, uic, asset_type, status, sessions_json, time_zone_abbreviation,
            fetched_at, run_date
         ) VALUES ('{}', '{}', {}, {}, '{}', {}, '{}', '{}', '{}')
         ON CONFLICT(symbol) DO UPDATE SET
            exchange = excluded.exchange,
            uic = excluded.uic,
            asset_type = excluded.asset_type,
            status = excluded.status,
            sessions_json = COALESCE(excluded.sessions_json, instrument_trading_schedules.sessions_json),
            time_zone_abbreviation = excluded.time_zone_abbreviation,
            fetched_at = excluded.fetched_at,
            run_date = excluded.run_date",
        sql_escape(stored.symbol),
        sql_escape(&exchange_of(stored.symbol)),
        stored.uic.map_or_else(|| "NULL".to_string(), |uic| uic.to_string()),
        stored
            .asset_type
            .map_or_else(|| "NULL".to_string(), |asset_type| format!("'{}'", sql_escape(asset_type))),
        sql_escape(stored.status),
        if stored.sessions.is_empty() {
            "NULL".to_string()
        } else {
            format!("'{}'", sql_escape(&sessions_json(stored.sessions)))
        },
        sql_escape(stored.zone),
        sql_escape(fetched_at),
        run_date,
    ))
    .execute(&state.pool)
    .await
    .context("storing a Saxo trading schedule")?;
    Ok(())
}

fn regular(sessions: &[Session]) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    sessions
        .iter()
        .filter(|session| session.state == REGULAR_STATE)
        .map(|session| (session.start, session.end))
        .collect()
}

/// One exchange's schedule from the instruments read on it.
pub(crate) fn exchange_schedule(
    exchange: &str,
    instruments: &[(String, Vec<Session>)],
    now: DateTime<Utc>,
) -> ExchangeTradingSchedulePayload {
    let mut counts = HashMap::<Vec<(DateTime<Utc>, DateTime<Utc>)>, usize>::new();
    for (_, sessions) in instruments {
        *counts.entry(regular(sessions)).or_default() += 1;
    }
    let usual = counts
        .iter()
        .max_by(|left, right| {
            left.1
                .cmp(right.1)
                .then(right.0.len().cmp(&left.0.len()).reverse())
        })
        .map(|(sessions, _)| sessions.clone())
        .unwrap_or_default();
    let mut differing = instruments
        .iter()
        .filter(|(_, sessions)| regular(sessions) != usual)
        .map(|(symbol, _)| symbol.clone())
        .collect::<Vec<_>>();
    differing.sort();
    let typical = usual
        .iter()
        .map(|(start, end)| (*end - *start).num_minutes())
        .max()
        .unwrap_or(0);
    let early_closes = usual
        .iter()
        .filter(|(start, end)| typical - (*end - *start).num_minutes() >= EARLY_CLOSE_MINUTES)
        .map(|(start, end)| ScheduleEarlyClosePayload {
            date: start.date_naive().to_string(),
            closes_at: end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            minutes: (*end - *start).num_minutes(),
            typical_minutes: typical,
        })
        .collect();
    let trading_days = usual
        .iter()
        .map(|(start, _)| start.date_naive())
        .collect::<HashSet<NaiveDate>>();
    let mut closed_weekdays = Vec::new();
    if let (Some(first), Some(last)) = (
        trading_days.iter().min().copied(),
        trading_days.iter().max().copied(),
    ) {
        let mut day = first;
        while day <= last {
            if day.weekday().number_from_monday() <= 5 && !trading_days.contains(&day) {
                closed_weekdays.push(day.to_string());
            }
            day += Duration::days(1);
        }
    }
    let all = instruments
        .iter()
        .flat_map(|(_, sessions)| sessions.iter())
        .collect::<Vec<_>>();
    let stamp = |time: DateTime<Utc>| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    ExchangeTradingSchedulePayload {
        exchange: exchange.to_string(),
        instruments: instruments.len() as i64,
        covered_from: all.iter().map(|session| session.start).min().map(stamp),
        covered_through: all.iter().map(|session| session.end).max().map(stamp),
        next_regular_open: usual
            .iter()
            .map(|(start, _)| *start)
            .find(|start| *start > now)
            .map(stamp),
        next_regular_close: usual
            .iter()
            .map(|(_, end)| *end)
            .find(|end| *end > now)
            .map(stamp),
        closed_weekdays,
        early_closes,
        instruments_differing: differing,
    }
}

pub(crate) async fn trading_schedule_payload(state: &AppState) -> Result<TradingSchedulePayload> {
    let rows = state
        .select_json(
            "SELECT symbol, exchange, status, sessions_json, run_date
             FROM instrument_trading_schedules ORDER BY exchange, symbol",
        )
        .await?;
    let mut by_status = BTreeMap::<String, i64>::new();
    let mut by_exchange = BTreeMap::<String, Vec<(String, Vec<Session>)>>::new();
    for row in &rows {
        *by_status.entry(json_text(row, "status")).or_default() += 1;
        let sessions = sessions_from_stored(row.get("sessions_json"));
        if json_text(row, "status") == "ok" && !sessions.is_empty() {
            by_exchange
                .entry(json_text(row, "exchange"))
                .or_default()
                .push((json_text(row, "symbol"), sessions));
        }
    }
    let now = Utc::now();
    Ok(TradingSchedulePayload {
        generated_at: now.to_rfc3339(),
        latest_run_date: rows.iter().map(|row| json_text(row, "run_date")).max(),
        instruments: rows.len() as i64,
        by_status,
        exchanges: by_exchange
            .iter()
            .map(|(exchange, instruments)| exchange_schedule(exchange, instruments, now))
            .collect(),
        caveat: "Read-only. Market-open decisions, the Trading Manager and the protective-stop sweep still use Saxo's exchange-level sessions.".to_string(),
        status: None,
        unavailable_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn session(start: &str, end: &str, state: &str) -> Session {
        Session {
            start: at(start),
            end: at(end),
            state: state.to_string(),
        }
    }

    #[test]
    fn held_instruments_come_first_then_one_per_exchange() {
        let held = vec!["NOVO-B:xcse".to_string(), "MDB:xnas".to_string()];
        let universe = ["AAPL:xnas", "DSV:xcse", "SAPG:xetr", "BAS:xetr", "XOM:xnys"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(
            schedule_targets(&held, &universe, 60),
            ["NOVO-B:xcse", "MDB:xnas", "SAPG:xetr", "XOM:xnys"]
        );
        assert_eq!(schedule_targets(&held, &universe, 3).len(), 3);
    }

    #[test]
    fn sessions_are_read_in_order_and_malformed_ones_dropped() {
        let payload = json!({"Sessions": [
            {"StartTime": "2026-11-27T14:30:00.000000Z", "EndTime": "2026-11-27T18:00:00.000000Z", "State": "AutomatedTrading"},
            {"StartTime": "2026-11-26T14:30:00.000000Z", "EndTime": "2026-11-26T21:00:00.000000Z", "State": "Closed"},
            {"StartTime": "bad", "EndTime": "2026-11-26T21:00:00.000000Z", "State": "Closed"},
        ], "TimeZoneAbbreviation": "EST"});
        let sessions = parse_sessions(&payload);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].state, "Closed");
        assert_eq!(
            sessions_from_stored(Some(&JsonValue::String(sessions_json(&sessions)))),
            sessions
        );
    }

    /// Thanksgiving week on NYSE: closed Thursday, a 13:00 close on Friday,
    /// and an instrument that keeps a different schedule is named.
    #[test]
    fn an_exchange_week_shows_its_holiday_early_close_and_odd_instrument() {
        let week = vec![
            session(
                "2026-11-24T14:30:00Z",
                "2026-11-24T21:00:00Z",
                REGULAR_STATE,
            ),
            session(
                "2026-11-25T14:30:00Z",
                "2026-11-25T21:00:00Z",
                REGULAR_STATE,
            ),
            session("2026-11-25T21:00:00Z", "2026-11-27T14:30:00Z", "Closed"),
            session(
                "2026-11-27T14:30:00Z",
                "2026-11-27T18:00:00Z",
                REGULAR_STATE,
            ),
        ];
        let odd = vec![session(
            "2026-11-24T14:30:00Z",
            "2026-11-24T21:00:00Z",
            REGULAR_STATE,
        )];
        let instruments = vec![
            ("XOM:xnys".to_string(), week.clone()),
            ("DIS:xnys".to_string(), week),
            ("SPCX:xnys".to_string(), odd),
        ];
        let schedule = exchange_schedule("XNYS", &instruments, at("2026-11-25T22:00:00Z"));
        assert_eq!(schedule.instruments, 3);
        assert_eq!(schedule.closed_weekdays, ["2026-11-26"]);
        assert_eq!(schedule.early_closes.len(), 1);
        assert_eq!(schedule.early_closes[0].date, "2026-11-27");
        assert_eq!(schedule.early_closes[0].minutes, 210);
        assert_eq!(schedule.early_closes[0].typical_minutes, 390);
        assert_eq!(
            schedule.next_regular_open.as_deref(),
            Some("2026-11-27T14:30:00Z")
        );
        assert_eq!(schedule.instruments_differing, ["SPCX:xnys"]);
    }

    /// The view reads what the refresh stored, through the same row decoding
    /// the dashboard uses, and a later failed read hides the older sessions.
    #[tokio::test]
    async fn stored_schedules_reach_the_view_and_a_failed_read_hides_them() {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory schedule database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create schedule table");
        }
        let state = AppState {
            config_path: std::path::PathBuf::from("schedule-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        };
        let sessions = vec![
            session(
                "2099-11-24T14:30:00Z",
                "2099-11-24T21:00:00Z",
                REGULAR_STATE,
            ),
            session(
                "2099-11-25T14:30:00Z",
                "2099-11-25T21:00:00Z",
                REGULAR_STATE,
            ),
        ];
        let read = |status, sessions| StoredSchedule {
            symbol: "XOM:xnys",
            uic: Some(1),
            asset_type: Some("Stock"),
            status,
            sessions,
            zone: "EST",
        };
        let day = NaiveDate::from_ymd_opt(2099, 11, 24).unwrap();
        store_schedule(&state, &read("ok", &sessions), "2099-11-24T05:30:00Z", day)
            .await
            .unwrap();
        let view = trading_schedule_payload(&state).await.unwrap();
        assert_eq!(view.exchanges.len(), 1);
        assert_eq!(view.exchanges[0].exchange, "XNYS");
        assert_eq!(
            view.exchanges[0].next_regular_open.as_deref(),
            Some("2099-11-24T14:30:00Z")
        );

        store_schedule(&state, &read("http_503", &[]), "2099-11-25T05:30:00Z", day)
            .await
            .unwrap();
        let view = trading_schedule_payload(&state).await.unwrap();
        assert!(view.exchanges.is_empty());
        assert_eq!(view.by_status.get("http_503"), Some(&1));
    }

    #[test]
    fn a_saxo_error_is_kept_as_its_status_only() {
        let err = anyhow::anyhow!("Saxo GET failed: HTTP 404: {{\"Message\":\"not found\"}}");
        assert_eq!(error_category(&err), "http_404");
        assert_eq!(
            error_category(&anyhow::anyhow!("connection reset")),
            "request_failed"
        );
    }
}
