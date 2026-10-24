//! Past quarterly report dates for US stocks, from Alpha Vantage.
//!
//! The 2026-10-24 month review asked whether an earnings blackout would have
//! protected the book. Answering that needs the dates of past reports, and
//! the free calendars do not keep them: Finnhub's reaches back about a month
//! and Financial Modeling Prep's only looks forward. Alpha Vantage's
//! `EARNINGS` function returns every past quarter of one stock with its report
//! date and whether the report came before or after the session.
//!
//! A free Alpha Vantage key allows 25 calls a day, shared with the congress
//! trade store (`alpha_vantage`), so the store fills over several days within
//! `daily_call_budget`: held stocks first, then anything the book has traded,
//! then the rest of the US universe. Afterwards each stock is read again every
//! `refresh_days`, which picks up each new quarter.
//!
//! Read-only reference data. Nothing here is read by a gate, a prompt, sizing
//! or the broker path; `earnings_evaluation` measures from it. Alpha Vantage
//! takes its key only in the query string, so every request strips its URL
//! from errors, and a failure is stored as a category, never as text.

use std::{collections::HashSet, time::Duration as StdDuration};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    alpha_vantage::{self, AlphaVantage, REQUEST_SPACING_MS},
    config::{yaml_bool, yaml_i64},
    db::sql_escape,
    state::{AppState, json_text},
};

const DEFAULT_DAILY_CALL_BUDGET: i64 = 20;
const DEFAULT_MAX_CALLS_PER_CYCLE: i64 = 8;
const DEFAULT_REFRESH_DAYS: i64 = 30;

#[derive(Clone, Debug)]
struct EarningsDatesConfig {
    enabled: bool,
    provider: AlphaVantage,
    daily_call_budget: usize,
    max_calls_per_cycle: usize,
    refresh_days: i64,
}

/// A US stock the store covers, under the system symbol and the ticker the
/// data providers know it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReferenceSymbol {
    pub symbol: String,
    pub ticker: String,
}

/// One past quarter as Alpha Vantage reported it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReportedQuarter {
    pub fiscal_date_ending: String,
    pub reported_date: String,
    /// `pre-market`, `post-market`, or empty when Alpha Vantage does not say.
    pub report_time: String,
    pub reported_eps: Option<f64>,
    pub estimated_eps: Option<f64>,
    pub surprise_pct: Option<f64>,
}

/// What one call came back with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FetchOutcome {
    Quarters(Vec<ReportedQuarter>),
    /// A category: `no_data`, `quota_exhausted`, `rate_limited`,
    /// `premium_required`, `invalid_symbol`, `provider_notice`,
    /// `parse_error`, `http_<status>`, `timeout` or `request_failed`.
    Refused(&'static str),
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS earnings_report_dates (
            symbol TEXT NOT NULL,
            fiscal_date_ending TEXT NOT NULL,
            reported_date TEXT NOT NULL,
            report_time TEXT,
            reported_eps DOUBLE PRECISION,
            estimated_eps DOUBLE PRECISION,
            surprise_pct DOUBLE PRECISION,
            source TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (symbol, fiscal_date_ending)
        )",
        "CREATE INDEX IF NOT EXISTS idx_earnings_report_dates_date
         ON earnings_report_dates(reported_date)",
        // One row per stock: when it was last asked for and what came back.
        // A stock is asked at most once a day, so the rows dated today are
        // the calls made today.
        "CREATE TABLE IF NOT EXISTS earnings_date_fetches (
            symbol TEXT PRIMARY KEY,
            ticker TEXT NOT NULL,
            last_attempt_at TEXT NOT NULL,
            last_attempt_date TEXT NOT NULL,
            last_status TEXT NOT NULL,
            last_success_at TEXT,
            quarter_count INTEGER NOT NULL DEFAULT 0
        )",
    ]
}

fn earnings_dates_config(state: &AppState) -> EarningsDatesConfig {
    let key = |suffix: &'static str| ["strategy", "earnings_dates", suffix];
    EarningsDatesConfig {
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
            .clamp(1, 365),
    }
}

/// The ticker Alpha Vantage and Financial Modeling Prep use for a US Saxo
/// symbol, or `None` outside the US.
///
/// Saxo appends `_NEW` to a relisted symbol (`SHOP_NEW:xnas`) and writes
/// share classes with a dot (`BRK.B`); the providers use the bare ticker and
/// a hyphen.
pub(crate) fn us_ticker(symbol: &str) -> Option<String> {
    let (base, exchange) = symbol.split_once(':')?;
    let base = base.trim().to_uppercase();
    let base = base.strip_suffix("_NEW").unwrap_or(&base);
    if base.is_empty() {
        return None;
    }
    match exchange.trim().to_lowercase().as_str() {
        "xnas" | "xnys" | "arcx" | "bats" | "xase" => Some(base.replace('.', "-")),
        _ => None,
    }
}

/// The US stocks reference data is kept for, most useful first: held now,
/// then anything the book has traded, then the rest of the universe.
pub(crate) async fn us_reference_symbols(state: &AppState) -> Result<Vec<ReferenceSymbol>> {
    let mut ordered = Vec::new();
    for row in state.position_items(250).await.unwrap_or_default() {
        ordered.push(json_text(&row, "symbol"));
    }
    for row in state
        .select_json(
            "SELECT symbol, MAX(created_at) AS last_traded FROM trade_ledger
             WHERE status = 'executed' GROUP BY symbol ORDER BY last_traded DESC",
        )
        .await
        .unwrap_or_default()
    {
        ordered.push(json_text(&row, "symbol"));
    }
    for asset in crate::markov_method::markov_assets(state, 0)
        .await
        .unwrap_or_default()
    {
        ordered.push(asset.symbol);
    }
    let mut seen = HashSet::new();
    Ok(ordered
        .into_iter()
        .filter_map(|symbol| {
            let ticker = us_ticker(&symbol)?;
            seen.insert(ticker.clone()).then_some(ReferenceSymbol {
                symbol: symbol.trim().to_string(),
                ticker,
            })
        })
        .collect())
}

/// Reads Alpha Vantage's answer to `function=EARNINGS`.
///
/// Alpha Vantage answers a refusal with HTTP 200 and a JSON note
/// (`Information`, `Note` or `Error Message`), so the body decides, not the
/// status.
pub(crate) fn parse_earnings_body(body: &str) -> FetchOutcome {
    let Ok(payload) = serde_json::from_str::<JsonValue>(body) else {
        return FetchOutcome::Refused("parse_error");
    };
    if let Some(category) = alpha_vantage::refusal(&payload) {
        return FetchOutcome::Refused(category);
    }
    let quarters = payload
        .get("quarterlyEarnings")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            let fiscal_date_ending = valid_date(&json_text(row, "fiscalDateEnding"))?;
            let reported_date = valid_date(&json_text(row, "reportedDate"))?;
            Some(ReportedQuarter {
                fiscal_date_ending,
                reported_date,
                report_time: json_text(row, "reportTime").trim().to_lowercase(),
                reported_eps: number(row, "reportedEPS"),
                estimated_eps: number(row, "estimatedEPS"),
                surprise_pct: number(row, "surprisePercentage"),
            })
        })
        .collect::<Vec<_>>();
    if quarters.is_empty() {
        FetchOutcome::Refused("no_data")
    } else {
        FetchOutcome::Quarters(quarters)
    }
}

fn valid_date(value: &str) -> Option<String> {
    let value = value.trim();
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|_| value.to_string())
}

fn number(row: &JsonValue, key: &str) -> Option<f64> {
    match row.get(key)? {
        JsonValue::Number(value) => value.as_f64(),
        JsonValue::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

/// When a stock is next due, given its last attempt.
///
/// A stock never asked is due at once. A stock that answered is due again
/// after `refresh_days`; so is one Alpha Vantage has nothing for, so a bad
/// ticker does not spend a call every day. Anything transient is due the next
/// day.
pub(crate) fn is_due(
    last_status: Option<&str>,
    last_attempt_date: Option<NaiveDate>,
    today: NaiveDate,
    refresh_days: i64,
) -> bool {
    let (Some(status), Some(attempted)) = (last_status, last_attempt_date) else {
        return true;
    };
    if attempted >= today {
        return false;
    }
    match status {
        "ok" | "no_data" | "invalid_symbol" | "premium_required" => {
            today - attempted >= Duration::days(refresh_days)
        }
        _ => true,
    }
}

pub async fn run_earnings_dates_cycle(state: &AppState) -> Result<JsonValue> {
    let config = earnings_dates_config(state);
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
    if alpha_vantage::quota_exhausted_on(state, today).await {
        return Ok(json!({
            "status": "idle",
            "reason": "quota_exhausted_today",
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
    let symbols = us_reference_symbols(state).await?;
    let attempts = fetch_attempts(state).await?;
    let due = symbols
        .into_iter()
        .filter(|candidate| {
            let last = attempts
                .iter()
                .find(|row| json_text(row, "symbol") == candidate.symbol);
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
        let mut outcome = fetch_quarters(&client, &config, &api_key, &candidate.ticker).await;
        if outcome == FetchOutcome::Refused("rate_limited") {
            sleep(StdDuration::from_millis(REQUEST_SPACING_MS * 2)).await;
            outcome = fetch_quarters(&client, &config, &api_key, &candidate.ticker).await;
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
        stored, "earnings report date refresh completed"
    );
    Ok(json!({
        "status": "completed",
        "attempted": results.len(),
        "stored": stored,
        "calls_today": calls_today + results.len(),
        "daily_call_budget": config.daily_call_budget,
        "results": results,
        "scope": "read_only_reference_data",
    }))
}

async fn fetch_quarters(
    client: &reqwest::Client,
    config: &EarningsDatesConfig,
    api_key: &str,
    ticker: &str,
) -> FetchOutcome {
    match alpha_vantage::get(client, &config.provider, api_key, "EARNINGS", ticker).await {
        Ok(body) => parse_earnings_body(&body),
        Err(category) => FetchOutcome::Refused(category),
    }
}

pub(crate) fn http_category(status: u16) -> &'static str {
    match status {
        401 => "http_401",
        402 => "http_402",
        403 => "http_403",
        404 => "http_404",
        429 => "http_429",
        500..=599 => "http_5xx",
        _ => "http_other",
    }
}

async fn record_outcome(
    state: &AppState,
    candidate: &ReferenceSymbol,
    outcome: &FetchOutcome,
    now: DateTime<Utc>,
) -> Result<&'static str> {
    let fetched_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (status, quarter_count) = match outcome {
        FetchOutcome::Quarters(quarters) => {
            store_quarters(state, &candidate.symbol, quarters, &fetched_at).await?;
            ("ok", quarters.len())
        }
        FetchOutcome::Refused(category) => {
            if *category != "rate_limited" {
                warn!(symbol = %candidate.symbol, category, "earnings report dates not stored");
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
        "INSERT INTO earnings_date_fetches (
            symbol, ticker, last_attempt_at, last_attempt_date, last_status, last_success_at,
            quarter_count
         ) VALUES ('{}', '{}', '{}', '{}', '{}', {success}, {quarter_count})
         ON CONFLICT(symbol) DO UPDATE SET
            ticker = excluded.ticker,
            last_attempt_at = excluded.last_attempt_at,
            last_attempt_date = excluded.last_attempt_date,
            last_status = excluded.last_status,
            last_success_at = COALESCE(excluded.last_success_at, earnings_date_fetches.last_success_at),
            quarter_count = CASE WHEN excluded.last_status = 'ok'
                THEN excluded.quarter_count ELSE earnings_date_fetches.quarter_count END",
        sql_escape(&candidate.symbol),
        sql_escape(&candidate.ticker),
        fetched_at,
        now.date_naive(),
        status,
    ))
    .execute(&state.pool)
    .await
    .context("recording an earnings report date fetch")?;
    Ok(status)
}

async fn store_quarters(
    state: &AppState,
    symbol: &str,
    quarters: &[ReportedQuarter],
    fetched_at: &str,
) -> Result<()> {
    let sql_number =
        |value: Option<f64>| value.map_or_else(|| "NULL".to_string(), |v| v.to_string());
    for chunk in quarters.chunks(100) {
        let values = chunk
            .iter()
            .map(|quarter| {
                format!(
                    "('{}', '{}', '{}', '{}', {}, {}, {}, 'alpha_vantage', '{}')",
                    sql_escape(symbol),
                    sql_escape(&quarter.fiscal_date_ending),
                    sql_escape(&quarter.reported_date),
                    sql_escape(&quarter.report_time),
                    sql_number(quarter.reported_eps),
                    sql_number(quarter.estimated_eps),
                    sql_number(quarter.surprise_pct),
                    fetched_at,
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO earnings_report_dates (
                symbol, fiscal_date_ending, reported_date, report_time, reported_eps,
                estimated_eps, surprise_pct, source, fetched_at
             ) VALUES {values}
             ON CONFLICT(symbol, fiscal_date_ending) DO UPDATE SET
                reported_date = excluded.reported_date,
                report_time = excluded.report_time,
                reported_eps = excluded.reported_eps,
                estimated_eps = excluded.estimated_eps,
                surprise_pct = excluded.surprise_pct,
                source = excluded.source,
                fetched_at = excluded.fetched_at"
        ))
        .execute(&state.pool)
        .await
        .with_context(|| format!("storing earnings report dates for {symbol}"))?;
    }
    Ok(())
}

async fn fetch_attempts(state: &AppState) -> Result<Vec<JsonValue>> {
    state
        .select_json(
            "SELECT symbol, last_attempt_date, last_status, last_success_at, quarter_count
             FROM earnings_date_fetches",
        )
        .await
}

async fn calls_made_on(state: &AppState, day: NaiveDate) -> Result<usize> {
    let rows = state
        .select_json(&format!(
            "SELECT COUNT(*) AS calls FROM earnings_date_fetches WHERE last_attempt_date = '{day}'"
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("calls"))
        .and_then(JsonValue::as_i64)
        .unwrap_or(0)
        .max(0) as usize)
}

/// How far the store has got, for the evaluation's coverage line.
pub(crate) async fn coverage(state: &AppState) -> Result<JsonValue> {
    let symbols = us_reference_symbols(state).await.unwrap_or_default();
    let attempts = fetch_attempts(state).await.unwrap_or_default();
    let mut by_status = std::collections::BTreeMap::<String, i64>::new();
    let mut covered = 0usize;
    for symbol in &symbols {
        match attempts
            .iter()
            .find(|row| json_text(row, "symbol") == symbol.symbol)
        {
            Some(row) => {
                if !json_text(row, "last_success_at").is_empty() {
                    covered += 1;
                }
                *by_status.entry(json_text(row, "last_status")).or_default() += 1;
            }
            None => *by_status.entry("not_yet_asked".to_string()).or_default() += 1,
        }
    }
    Ok(json!({
        "us_symbols": symbols.len(),
        "with_report_dates": covered,
        "by_last_status": by_status,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_tickers_drop_the_relisting_suffix_and_hyphenate_share_classes() {
        assert_eq!(us_ticker("MDB:xnas").as_deref(), Some("MDB"));
        assert_eq!(us_ticker("SHOP_NEW:xnas").as_deref(), Some("SHOP"));
        assert_eq!(us_ticker("BRK.B:xnys").as_deref(), Some("BRK-B"));
        assert_eq!(us_ticker("brk-b:XNYS").as_deref(), Some("BRK-B"));
        assert_eq!(us_ticker("NOVO-B:xcse"), None);
        assert_eq!(us_ticker("SAPG:xetr"), None);
        assert_eq!(us_ticker("NVDA"), None);
    }

    #[test]
    fn quarters_are_read_with_their_timing_and_bad_rows_dropped() {
        let body = r#"{"symbol":"MDB","quarterlyEarnings":[
            {"fiscalDateEnding":"2026-07-31","reportedDate":"2026-09-01","reportedEPS":"1.9",
             "estimatedEPS":"1.6135","surprise":"0.2865","surprisePercentage":"17.7564",
             "reportTime":"post-market"},
            {"fiscalDateEnding":"2026-04-30","reportedDate":"2026-05-28","reportedEPS":"None",
             "estimatedEPS":"1.1993","surprisePercentage":"None","reportTime":"Pre-Market"},
            {"fiscalDateEnding":"bad","reportedDate":"2026-03-02"}
        ]}"#;
        let FetchOutcome::Quarters(quarters) = parse_earnings_body(body) else {
            panic!("quarters parse");
        };
        assert_eq!(quarters.len(), 2);
        assert_eq!(quarters[0].reported_date, "2026-09-01");
        assert_eq!(quarters[0].report_time, "post-market");
        assert_eq!(quarters[0].surprise_pct, Some(17.7564));
        assert_eq!(quarters[1].report_time, "pre-market");
        assert_eq!(quarters[1].reported_eps, None);
    }

    #[test]
    fn refusals_are_read_from_the_body_not_the_status() {
        let note = |text: &str| parse_earnings_body(&json!({"Information": text}).to_string());
        assert_eq!(
            note(
                "Our standard API rate limit is 25 requests per day. Please subscribe to premium."
            ),
            FetchOutcome::Refused("quota_exhausted")
        );
        assert_eq!(
            note(
                "Please consider spreading out your free API requests more sparingly (1 request per second)."
            ),
            FetchOutcome::Refused("rate_limited")
        );
        assert_eq!(
            note("This is a premium endpoint."),
            FetchOutcome::Refused("premium_required")
        );
        assert_eq!(
            parse_earnings_body(r#"{"Error Message":"Invalid API call."}"#),
            FetchOutcome::Refused("invalid_symbol")
        );
        assert_eq!(parse_earnings_body("{}"), FetchOutcome::Refused("no_data"));
        assert_eq!(
            parse_earnings_body("symbol,name\nI,n"),
            FetchOutcome::Refused("parse_error")
        );
    }

    #[test]
    fn a_stock_is_asked_once_a_day_and_an_answered_one_waits_for_its_refresh() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        let day = |offset: i64| Some(today - Duration::days(offset));
        assert!(is_due(None, None, today, 30));
        assert!(!is_due(Some("http_5xx"), day(0), today, 30), "once a day");
        assert!(is_due(Some("http_5xx"), day(1), today, 30));
        assert!(is_due(Some("quota_exhausted"), day(1), today, 30));
        assert!(!is_due(Some("ok"), day(29), today, 30));
        assert!(is_due(Some("ok"), day(30), today, 30));
        assert!(!is_due(Some("invalid_symbol"), day(3), today, 30));
    }

    async fn test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory earnings database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create earnings tables");
        }
        AppState {
            config_path: std::path::PathBuf::from("earnings-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// A second answer replaces the stored quarter, a refusal keeps the last
    /// success, and both count as today's calls.
    #[tokio::test]
    async fn outcomes_are_recorded_and_counted_against_the_day() {
        let state = test_state().await;
        let mdb = ReferenceSymbol {
            symbol: "MDB:xnas".to_string(),
            ticker: "MDB".to_string(),
        };
        let quarter = |eps: f64| ReportedQuarter {
            fiscal_date_ending: "2026-07-31".to_string(),
            reported_date: "2026-09-01".to_string(),
            report_time: "post-market".to_string(),
            reported_eps: Some(eps),
            estimated_eps: Some(1.6),
            surprise_pct: None,
        };
        let now = DateTime::parse_from_rfc3339("2026-10-24T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        record_outcome(
            &state,
            &mdb,
            &FetchOutcome::Quarters(vec![quarter(1.8)]),
            now,
        )
        .await
        .unwrap();
        record_outcome(
            &state,
            &mdb,
            &FetchOutcome::Quarters(vec![quarter(1.9)]),
            now,
        )
        .await
        .unwrap();
        let rows = state
            .select_json("SELECT reported_eps, report_time FROM earnings_report_dates")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["reported_eps"].as_f64(), Some(1.9));

        let tomorrow = now + Duration::days(1);
        record_outcome(&state, &mdb, &FetchOutcome::Refused("http_5xx"), tomorrow)
            .await
            .unwrap();
        let attempts = fetch_attempts(&state).await.unwrap();
        assert_eq!(attempts[0]["last_status"], "http_5xx");
        assert_eq!(attempts[0]["last_success_at"], "2026-10-24T09:00:00Z");
        assert_eq!(attempts[0]["quarter_count"].as_i64(), Some(1));
        assert_eq!(
            calls_made_on(&state, tomorrow.date_naive()).await.unwrap(),
            1
        );
        assert_eq!(calls_made_on(&state, now.date_naive()).await.unwrap(), 0);
        assert_eq!(
            alpha_vantage::calls_made_on(&state, tomorrow.date_naive()).await,
            1
        );
        assert!(!alpha_vantage::quota_exhausted_on(&state, tomorrow.date_naive()).await);

        record_outcome(
            &state,
            &mdb,
            &FetchOutcome::Refused("quota_exhausted"),
            tomorrow,
        )
        .await
        .unwrap();
        assert!(alpha_vantage::quota_exhausted_on(&state, tomorrow.date_naive()).await);
    }
}
