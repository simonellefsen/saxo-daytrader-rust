//! Disclosed congressional trades in US stocks, from Alpha Vantage.
//!
//! Replaced QuiverQuant's paid feed on 2026-10-24. Alpha Vantage's
//! `CONGRESS_TRADES` returns every House and Senate disclosure for one stock,
//! including each trade's date, filing date and amount band. One stock costs
//! one call from the free key's daily allowance, which the report-date store
//! shares (`alpha_vantage`), so the store fills over several days: held
//! stocks first, then anything the book has traded, then the rest of the US
//! universe. Disclosures arrive weeks after the trade, so each stock is read
//! again every `refresh_days` rather than daily.
//!
//! The daily congress signal (`quiver.rs`, which keeps its name and tables
//! because decision reports, Hermes and the Jev reviews read them) scores from
//! this store and calls no provider itself. Option trades are left out: the
//! disclosure says buy or sell but not call or put, so their direction is
//! unknown.

use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    alpha_vantage::{self, AlphaVantage, REQUEST_SPACING_MS},
    config::{yaml_bool, yaml_i64},
    db::sql_escape,
    earnings_dates::{ReferenceSymbol, is_due, us_reference_symbols},
    state::{AppState, json_text},
};

const DEFAULT_DAILY_CALL_BUDGET: i64 = 20;
const DEFAULT_MAX_CALLS_PER_CYCLE: i64 = 8;
const DEFAULT_REFRESH_DAYS: i64 = 7;
/// Longer than any signal lookback, so a refresh never shortens what the
/// signal can see.
const RETAIN_DAYS: i64 = 400;

#[derive(Clone, Debug)]
struct CongressTradesConfig {
    enabled: bool,
    provider: AlphaVantage,
    daily_call_budget: usize,
    max_calls_per_cycle: usize,
    refresh_days: i64,
}

/// One disclosed trade as stored.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CongressTrade {
    pub politician: String,
    pub bioguide_id: String,
    pub chamber: String,
    pub party: String,
    /// `BUY`, `SELL` or `OTHER`.
    pub transaction_type: String,
    pub transaction_date: Option<NaiveDate>,
    pub filed_date: Option<NaiveDate>,
    pub amount_min: Option<f64>,
    pub amount_max: Option<f64>,
    pub owner_code: String,
}

#[derive(Clone, Debug, PartialEq)]
enum FetchOutcome {
    /// Possibly empty: most stocks have no recent disclosure.
    Trades(Vec<CongressTrade>),
    /// A category, as in `earnings_dates::FetchOutcome`.
    Refused(&'static str),
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        // A stock's trades are replaced as a whole on each answer, so `seq`
        // only orders rows within one answer; two identical disclosures stay
        // two rows.
        "CREATE TABLE IF NOT EXISTS congress_trades (
            ticker TEXT NOT NULL,
            seq INTEGER NOT NULL,
            politician TEXT NOT NULL,
            bioguide_id TEXT,
            chamber TEXT,
            party TEXT,
            transaction_type TEXT NOT NULL,
            transaction_date TEXT,
            filed_date TEXT,
            amount_min DOUBLE PRECISION,
            amount_max DOUBLE PRECISION,
            owner_code TEXT,
            source TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (ticker, seq)
        )",
        // One row per stock, like `earnings_date_fetches`; the rows dated
        // today are today's calls.
        "CREATE TABLE IF NOT EXISTS congress_trade_fetches (
            ticker TEXT PRIMARY KEY,
            symbol TEXT NOT NULL,
            last_attempt_at TEXT NOT NULL,
            last_attempt_date TEXT NOT NULL,
            last_status TEXT NOT NULL,
            last_success_at TEXT,
            trade_count INTEGER NOT NULL DEFAULT 0
        )",
    ]
}

fn congress_trades_config(state: &AppState) -> CongressTradesConfig {
    let key = |suffix: &'static str| ["strategy", "congress_trades", suffix];
    CongressTradesConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(false),
        provider: alpha_vantage::alpha_vantage(state),
        daily_call_budget: yaml_i64(&state.config, &key("daily_call_budget"))
            .unwrap_or(DEFAULT_DAILY_CALL_BUDGET)
            .clamp(0, 500) as usize,
        max_calls_per_cycle: yaml_i64(&state.config, &key("max_calls_per_cycle"))
            .unwrap_or(DEFAULT_MAX_CALLS_PER_CYCLE)
            .clamp(1, 50) as usize,
        refresh_days: yaml_i64(&state.config, &key("refresh_days"))
            .unwrap_or(DEFAULT_REFRESH_DAYS)
            .clamp(1, 90),
    }
}

/// Reads Alpha Vantage's answer to `function=CONGRESS_TRADES`, keeping stock
/// trades disclosed or made within `RETAIN_DAYS` of `today`.
fn parse_congress_body(body: &str, today: NaiveDate) -> FetchOutcome {
    let Ok(payload) = serde_json::from_str::<JsonValue>(body) else {
        return FetchOutcome::Refused("parse_error");
    };
    if let Some(category) = alpha_vantage::refusal(&payload) {
        return FetchOutcome::Refused(category);
    }
    let Some(rows) = payload.get("trades").and_then(JsonValue::as_array) else {
        return FetchOutcome::Refused("no_data");
    };
    let oldest = today - Duration::days(RETAIN_DAYS);
    let trades = rows
        .iter()
        .filter(|row| !is_option(&json_text(row, "asset_type_code")))
        .filter_map(|row| {
            let transaction_date = date(row, "transaction_date");
            let filed_date = date(row, "filed_date").or_else(|| date(row, "notification_date"));
            let latest = transaction_date.max(filed_date)?;
            (latest >= oldest).then(|| CongressTrade {
                politician: Some(json_text(row, "politician_canonical"))
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| json_text(row, "politician")),
                bioguide_id: json_text(row, "bioguide_id"),
                chamber: json_text(row, "chamber"),
                party: json_text(row, "party"),
                transaction_type: json_text(row, "transaction_type").trim().to_uppercase(),
                transaction_date,
                filed_date,
                amount_min: number(row, "amount_min"),
                amount_max: number(row, "amount_max"),
                owner_code: json_text(row, "owner_code"),
            })
        })
        .collect();
    FetchOutcome::Trades(trades)
}

fn is_option(asset_type_code: &str) -> bool {
    let code = asset_type_code.trim().to_lowercase();
    code == "op" || code.contains("option")
}

fn date(row: &JsonValue, key: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(json_text(row, key).trim(), "%Y-%m-%d").ok()
}

fn number(row: &JsonValue, key: &str) -> Option<f64> {
    match row.get(key)? {
        JsonValue::Number(value) => value.as_f64(),
        JsonValue::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

pub async fn run_congress_trades_cycle(state: &AppState) -> Result<JsonValue> {
    let config = congress_trades_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let Some(api_key) = config.provider.api_key.clone() else {
        return Ok(json!({
            "status": "disabled",
            "reason": "missing_api_key",
            "api_key_env": "ALPHAVANTAGE_API_KEY",
        }));
    };
    let now = Utc::now();
    let today = now.date_naive();
    let calls_today = calls_made_on(state, today).await?;
    let provider_calls_today = alpha_vantage::calls_made_on(state, today).await;
    if alpha_vantage::quota_refused_recently(state, now).await {
        return Ok(json!({
            "status": "idle",
            "reason": "quota_refused_recently",
            "calls_today": calls_today,
        }));
    }
    let remaining = config.daily_call_budget.saturating_sub(calls_today).min(
        config
            .provider
            .daily_call_budget
            .saturating_sub(provider_calls_today),
    );
    if remaining == 0 {
        return Ok(json!({
            "status": "idle",
            "reason": "daily_budget_spent",
            "calls_today": calls_today,
            "provider_calls_today": provider_calls_today,
            "daily_call_budget": config.daily_call_budget,
        }));
    }
    let attempts = fetch_attempts(state).await?;
    let due = us_reference_symbols(state)
        .await?
        .into_iter()
        .filter(|candidate| {
            let last = attempts
                .iter()
                .find(|row| json_text(row, "ticker") == candidate.ticker);
            is_due(
                last.map(|row| row["last_status"].as_str().unwrap_or_default()),
                last.and_then(|row| {
                    NaiveDate::parse_from_str(&json_text(row, "last_attempt_date"), "%Y-%m-%d").ok()
                }),
                today,
                config.refresh_days,
            )
        })
        .take(remaining.min(config.max_calls_per_cycle))
        .collect::<Vec<_>>();
    if due.is_empty() {
        return Ok(json!({"status": "idle", "reason": "nothing_due", "calls_today": calls_today}));
    }
    let client = alpha_vantage::client()?;
    let mut results = Vec::new();
    for (index, candidate) in due.iter().enumerate() {
        if index > 0 {
            sleep(StdDuration::from_millis(REQUEST_SPACING_MS)).await;
        }
        let mut outcome = fetch_trades(&client, &config, &api_key, &candidate.ticker, today).await;
        if outcome == FetchOutcome::Refused("rate_limited") {
            sleep(StdDuration::from_millis(REQUEST_SPACING_MS * 2)).await;
            outcome = fetch_trades(&client, &config, &api_key, &candidate.ticker, today).await;
        }
        let status = record_outcome(state, candidate, &outcome, now).await?;
        results.push(json!({"symbol": candidate.symbol, "status": status}));
        if status == "quota_exhausted" {
            break;
        }
    }
    let stored = results.iter().filter(|row| row["status"] == "ok").count();
    info!(
        attempted = results.len(),
        stored, "congress trade refresh completed"
    );
    Ok(json!({
        "status": "completed",
        "attempted": results.len(),
        "stored": stored,
        "calls_today": calls_today + results.len(),
        "daily_call_budget": config.daily_call_budget,
        "results": results,
        "scope": "advisory_reference_data",
    }))
}

async fn fetch_trades(
    client: &reqwest::Client,
    config: &CongressTradesConfig,
    api_key: &str,
    ticker: &str,
    today: NaiveDate,
) -> FetchOutcome {
    match alpha_vantage::get(client, &config.provider, api_key, "CONGRESS_TRADES", ticker).await {
        Ok(body) => parse_congress_body(&body, today),
        Err(category) => FetchOutcome::Refused(category),
    }
}

async fn record_outcome(
    state: &AppState,
    candidate: &ReferenceSymbol,
    outcome: &FetchOutcome,
    now: DateTime<Utc>,
) -> Result<&'static str> {
    let fetched_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (status, trade_count) = match outcome {
        FetchOutcome::Trades(trades) => {
            store_trades(state, &candidate.ticker, trades, &fetched_at).await?;
            ("ok", trades.len())
        }
        FetchOutcome::Refused(category) => {
            if *category != "rate_limited" {
                warn!(symbol = %candidate.symbol, category, "congress trades not stored");
            }
            (*category, 0)
        }
    };
    let success = if status == "ok" {
        format!("'{fetched_at}'")
    } else {
        "NULL".to_string()
    };
    sqlx::query(&format!(
        "INSERT INTO congress_trade_fetches (
            ticker, symbol, last_attempt_at, last_attempt_date, last_status, last_success_at,
            trade_count
         ) VALUES ('{}', '{}', '{}', '{}', '{}', {success}, {trade_count})
         ON CONFLICT(ticker) DO UPDATE SET
            symbol = excluded.symbol,
            last_attempt_at = excluded.last_attempt_at,
            last_attempt_date = excluded.last_attempt_date,
            last_status = excluded.last_status,
            last_success_at = COALESCE(excluded.last_success_at, congress_trade_fetches.last_success_at),
            trade_count = CASE WHEN excluded.last_status = 'ok'
                THEN excluded.trade_count ELSE congress_trade_fetches.trade_count END",
        sql_escape(&candidate.ticker),
        sql_escape(&candidate.symbol),
        fetched_at,
        now.date_naive(),
        status,
    ))
    .execute(&state.pool)
    .await
    .context("recording a congress trade fetch")?;
    Ok(status)
}

async fn store_trades(
    state: &AppState,
    ticker: &str,
    trades: &[CongressTrade],
    fetched_at: &str,
) -> Result<()> {
    let text = |value: &str| format!("'{}'", sql_escape(value));
    let day = |value: Option<NaiveDate>| {
        value.map_or_else(|| "NULL".to_string(), |d| text(&d.to_string()))
    };
    let amount = |value: Option<f64>| value.map_or_else(|| "NULL".to_string(), |v| v.to_string());
    let mut tx = state.pool.begin().await?;
    sqlx::query(&format!(
        "DELETE FROM congress_trades WHERE ticker = {}",
        text(ticker)
    ))
    .execute(&mut *tx)
    .await
    .with_context(|| format!("clearing congress trades for {ticker}"))?;
    for (chunk_index, chunk) in trades.chunks(100).enumerate() {
        let values = chunk
            .iter()
            .enumerate()
            .map(|(index, trade)| {
                format!(
                    "({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, 'alpha_vantage', '{}')",
                    text(ticker),
                    chunk_index * 100 + index,
                    text(&trade.politician),
                    text(&trade.bioguide_id),
                    text(&trade.chamber),
                    text(&trade.party),
                    text(&trade.transaction_type),
                    day(trade.transaction_date),
                    day(trade.filed_date),
                    amount(trade.amount_min),
                    amount(trade.amount_max),
                    text(&trade.owner_code),
                    fetched_at,
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO congress_trades (
                ticker, seq, politician, bioguide_id, chamber, party, transaction_type,
                transaction_date, filed_date, amount_min, amount_max, owner_code, source,
                fetched_at
             ) VALUES {values}"
        ))
        .execute(&mut *tx)
        .await
        .with_context(|| format!("storing congress trades for {ticker}"))?;
    }
    tx.commit().await?;
    Ok(())
}

/// A stock's stored trades, or `None` until it has answered once.
pub(crate) async fn stored_trades(
    state: &AppState,
    ticker: &str,
) -> Result<Option<Vec<CongressTrade>>> {
    let answered = state
        .select_json(&format!(
            "SELECT ticker FROM congress_trade_fetches
             WHERE ticker = '{}' AND last_success_at IS NOT NULL",
            sql_escape(ticker)
        ))
        .await?;
    if answered.is_empty() {
        return Ok(None);
    }
    let rows = state
        .select_json(&format!(
            "SELECT politician, bioguide_id, chamber, party, transaction_type, transaction_date,
                    filed_date, amount_min, amount_max, owner_code
             FROM congress_trades WHERE ticker = '{}' ORDER BY seq",
            sql_escape(ticker)
        ))
        .await?;
    Ok(Some(
        rows.iter()
            .map(|row| CongressTrade {
                politician: json_text(row, "politician"),
                bioguide_id: json_text(row, "bioguide_id"),
                chamber: json_text(row, "chamber"),
                party: json_text(row, "party"),
                transaction_type: json_text(row, "transaction_type"),
                transaction_date: date(row, "transaction_date"),
                filed_date: date(row, "filed_date"),
                amount_min: number(row, "amount_min"),
                amount_max: number(row, "amount_max"),
                owner_code: json_text(row, "owner_code"),
            })
            .collect(),
    ))
}

async fn fetch_attempts(state: &AppState) -> Result<Vec<JsonValue>> {
    state
        .select_json(
            "SELECT ticker, last_attempt_date, last_status, last_success_at, trade_count
             FROM congress_trade_fetches",
        )
        .await
}

async fn calls_made_on(state: &AppState, day: NaiveDate) -> Result<usize> {
    let rows = state
        .select_json(&format!(
            "SELECT COUNT(*) AS calls FROM congress_trade_fetches WHERE last_attempt_date = '{day}'"
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("calls"))
        .and_then(JsonValue::as_i64)
        .unwrap_or(0)
        .max(0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 24).unwrap()
    }

    #[test]
    fn stock_trades_are_kept_and_options_and_old_rows_dropped() {
        let body = r#"{"symbol":"MSFT","bioguide_id":"","trades":[
            {"chamber":"HOUSE","politician":"Hon. Lisa McClain","politician_canonical":"Lisa C. McClain",
             "bioguide_id":"M001136","party":"R","asset_type_code":"ST","transaction_type":"BUY",
             "transaction_date":"2026-09-30","notification_date":"2026-10-01","amount_min":"1001.00",
             "amount_max":"15000.00","owner_code":"SPOUSE","filed_date":"2026-10-12"},
            {"chamber":"SENATE","politician":"Example","politician_canonical":null,"bioguide_id":"X1",
             "party":"D","asset_type_code":null,"transaction_type":"sell","transaction_date":"2026-08-01",
             "amount_min":"15001.00","amount_max":"50000.00","owner_code":"SELF","filed_date":null,
             "notification_date":"2026-08-20"},
            {"chamber":"HOUSE","politician":"Option","asset_type_code":"OP","transaction_type":"BUY",
             "transaction_date":"2026-09-01","filed_date":"2026-09-10"},
            {"chamber":"HOUSE","politician":"Old","asset_type_code":"Stock","transaction_type":"BUY",
             "transaction_date":"2024-01-02","filed_date":"2024-01-20"},
            {"chamber":"HOUSE","politician":"Undated","asset_type_code":"ST","transaction_type":"BUY"}
        ]}"#;
        let FetchOutcome::Trades(trades) = parse_congress_body(body, today()) else {
            panic!("trades parse");
        };
        assert_eq!(trades.len(), 2);
        assert_eq!(trades[0].politician, "Lisa C. McClain");
        assert_eq!(trades[0].amount_min, Some(1001.0));
        assert_eq!(trades[0].filed_date, NaiveDate::from_ymd_opt(2026, 10, 12));
        assert_eq!(trades[1].politician, "Example");
        assert_eq!(trades[1].transaction_type, "SELL");
        assert_eq!(
            trades[1].filed_date,
            NaiveDate::from_ymd_opt(2026, 8, 20),
            "the notification date stands in for a missing filing date"
        );
    }

    #[test]
    fn an_empty_list_is_an_answer_and_a_missing_one_is_not() {
        assert_eq!(
            parse_congress_body(r#"{"symbol":"ZZZ","trades":[]}"#, today()),
            FetchOutcome::Trades(Vec::new())
        );
        assert_eq!(
            parse_congress_body("{}", today()),
            FetchOutcome::Refused("no_data")
        );
        assert_eq!(
            parse_congress_body(
                r#"{"Information":"Our standard API rate limit is 25 requests per day."}"#,
                today()
            ),
            FetchOutcome::Refused("quota_exhausted")
        );
    }

    async fn test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory congress database");
        for sql in create_schema_sql()
            .iter()
            .chain(crate::earnings_dates::create_schema_sql())
        {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create reference tables");
        }
        AppState {
            config_path: std::path::PathBuf::from("congress-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// An answer replaces the stock's trades, a refusal keeps them, and both
    /// count against the allowance the report-date store shares.
    #[tokio::test]
    async fn answers_replace_trades_and_share_the_daily_allowance() {
        let state = test_state().await;
        let msft = ReferenceSymbol {
            symbol: "MSFT:xnas".to_string(),
            ticker: "MSFT".to_string(),
        };
        let trade = |kind: &str| CongressTrade {
            politician: "O'Example".to_string(),
            bioguide_id: "X1".to_string(),
            chamber: "HOUSE".to_string(),
            party: "R".to_string(),
            transaction_type: kind.to_string(),
            transaction_date: NaiveDate::from_ymd_opt(2026, 9, 30),
            filed_date: NaiveDate::from_ymd_opt(2026, 10, 12),
            amount_min: Some(1001.0),
            amount_max: Some(15000.0),
            owner_code: "SELF".to_string(),
        };
        let now = DateTime::parse_from_rfc3339("2026-10-24T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(stored_trades(&state, "MSFT").await.unwrap(), None);
        let same = FetchOutcome::Trades(vec![trade("BUY"), trade("BUY"), trade("SELL")]);
        record_outcome(&state, &msft, &same, now).await.unwrap();
        record_outcome(
            &state,
            &msft,
            &FetchOutcome::Trades(vec![trade("SELL")]),
            now,
        )
        .await
        .unwrap();
        let stored = stored_trades(&state, "MSFT").await.unwrap().unwrap();
        assert_eq!(stored, vec![trade("SELL")]);

        let tomorrow = now + Duration::days(1);
        record_outcome(&state, &msft, &FetchOutcome::Refused("http_5xx"), tomorrow)
            .await
            .unwrap();
        assert_eq!(
            stored_trades(&state, "MSFT").await.unwrap().unwrap().len(),
            1
        );

        sqlx::query(
            "INSERT INTO earnings_date_fetches
                (symbol, ticker, last_attempt_at, last_attempt_date, last_status)
             VALUES ('AAPL:xnas', 'AAPL', '2026-10-25T08:00:00Z', '2026-10-25', 'ok')",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        assert_eq!(
            calls_made_on(&state, tomorrow.date_naive()).await.unwrap(),
            1
        );
        assert_eq!(
            alpha_vantage::calls_made_on(&state, tomorrow.date_naive()).await,
            2
        );
    }
}
