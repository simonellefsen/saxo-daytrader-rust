//! Sector, industry and beta for US stocks, from Financial Modeling Prep.
//!
//! The runtime had no sector data at all. The old sector cap
//! (`strategy.max_assets_per_sector`) is retired and reported as uncontracted,
//! and every BUY's approval evidence says "Concentration policy is explicitly
//! unlimited". In the 2026-10-24 month review ten new US tech and high-beta
//! stocks lost 6,564 DKK of the month's 8,796, and nothing could see that they
//! were one bet.
//!
//! This keeps one profile per US stock and shows the book grouped by sector,
//! with each sector's value-weighted beta and the last 30 days of buying. The
//! free plan answers for US stocks only (European and Danish profiles return
//! HTTP 402), so non-US holdings are shown as unprofiled rather than guessed.
//!
//! Read-only. No gate, prompt, sizing rule or broker path reads it; whether a
//! sector cap should exist is the operator's decision. The key travels in the
//! query string, so every request strips its URL from errors and a failure is
//! stored as a category, never as text.

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration as StdDuration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{sql_escape, value_f64},
    earnings_dates::{ReferenceSymbol, http_category, is_due, us_reference_symbols, us_ticker},
    models::{HighBetaHoldingPayload, SectorConcentrationPayload, SectorExposureRowPayload},
    state::{AppState, json_text},
};

const DEFAULT_BASE_URL: &str = "https://financialmodelingprep.com";
const DEFAULT_DAILY_CALL_BUDGET: i64 = 100;
const DEFAULT_MAX_CALLS_PER_CYCLE: i64 = 25;
const DEFAULT_REFRESH_DAYS: i64 = 30;
const REQUEST_SPACING_MS: u64 = 300;
/// A holding at or above this beta is listed by name.
const HIGH_BETA: f64 = 1.5;
const UNPROFILED: &str = "Unprofiled";

#[derive(Clone, Debug)]
struct CompanyProfilesConfig {
    enabled: bool,
    api_key: Option<String>,
    base_url: String,
    daily_call_budget: usize,
    max_calls_per_cycle: usize,
    refresh_days: i64,
}

/// One profile as Financial Modeling Prep returned it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CompanyProfile {
    pub company_name: Option<String>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub beta: Option<f64>,
    pub country: Option<String>,
    pub exchange: Option<String>,
    pub currency: Option<String>,
    pub market_cap: Option<f64>,
    pub is_etf: Option<bool>,
    pub is_adr: Option<bool>,
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &["CREATE TABLE IF NOT EXISTS company_profiles (
        symbol TEXT PRIMARY KEY,
        ticker TEXT NOT NULL,
        status TEXT NOT NULL,
        company_name TEXT,
        sector TEXT,
        industry TEXT,
        beta DOUBLE PRECISION,
        country TEXT,
        exchange TEXT,
        currency TEXT,
        market_cap DOUBLE PRECISION,
        is_etf INTEGER,
        is_adr INTEGER,
        source TEXT NOT NULL,
        last_attempt_at TEXT NOT NULL,
        last_attempt_date TEXT NOT NULL,
        last_success_at TEXT
    )"]
}

fn company_profiles_config(state: &AppState) -> CompanyProfilesConfig {
    let key = |suffix: &'static str| ["strategy", "company_profiles", suffix];
    CompanyProfilesConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(false),
        api_key: yaml_string(&state.config, &["financial_modeling_prep", "api_key"])
            .or_else(|| std::env::var("FINANCIALMODELINGPREP_API_KEY").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        base_url: yaml_string(&state.config, &["financial_modeling_prep", "base_url"])
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        daily_call_budget: yaml_i64(&state.config, &key("daily_call_budget"))
            .unwrap_or(DEFAULT_DAILY_CALL_BUDGET)
            .clamp(0, 2_000) as usize,
        max_calls_per_cycle: yaml_i64(&state.config, &key("max_calls_per_cycle"))
            .unwrap_or(DEFAULT_MAX_CALLS_PER_CYCLE)
            .clamp(1, 100) as usize,
        refresh_days: yaml_i64(&state.config, &key("refresh_days"))
            .unwrap_or(DEFAULT_REFRESH_DAYS)
            .clamp(1, 365),
    }
}

/// Reads the answer to `/stable/profile`: an array with one object, an empty
/// array for an unknown ticker, or an object carrying `Error Message`.
pub(crate) fn parse_profile_body(body: &str) -> Result<CompanyProfile, &'static str> {
    let payload = serde_json::from_str::<JsonValue>(body).map_err(|_| "parse_error")?;
    if payload.get("Error Message").is_some() {
        return Err("provider_notice");
    }
    let row = payload
        .as_array()
        .and_then(|rows| rows.first())
        .ok_or("no_data")?;
    let text = |key: &str| {
        row.get(key)
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let number = |key: &str| {
        row.get(key)
            .and_then(JsonValue::as_f64)
            .filter(|v| v.is_finite())
    };
    Ok(CompanyProfile {
        company_name: text("companyName"),
        sector: text("sector"),
        industry: text("industry"),
        beta: number("beta"),
        country: text("country"),
        exchange: text("exchange"),
        currency: text("currency"),
        market_cap: number("marketCap"),
        is_etf: row.get("isEtf").and_then(JsonValue::as_bool),
        is_adr: row.get("isAdr").and_then(JsonValue::as_bool),
    })
}

pub async fn run_company_profiles_cycle(state: &AppState) -> Result<JsonValue> {
    let config = company_profiles_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let Some(api_key) = config.api_key.clone() else {
        return Ok(json!({
            "status": "disabled",
            "reason": "missing_api_key",
            "api_key_env": "FINANCIALMODELINGPREP_API_KEY",
        }));
    };
    let now = Utc::now();
    let today = now.date_naive();
    let attempts = state
        .select_json("SELECT symbol, status, last_attempt_date FROM company_profiles")
        .await?;
    let calls_today = attempts
        .iter()
        .filter(|row| json_text(row, "last_attempt_date") == today.to_string())
        .count();
    if attempts.iter().any(|row| {
        json_text(row, "last_attempt_date") == today.to_string()
            && json_text(row, "status") == "http_429"
    }) {
        return Ok(
            json!({"status": "idle", "reason": "rate_limited_today", "calls_today": calls_today}),
        );
    }
    let remaining = config.daily_call_budget.saturating_sub(calls_today);
    if remaining == 0 {
        return Ok(json!({
            "status": "idle",
            "reason": "daily_budget_spent",
            "calls_today": calls_today,
            "daily_call_budget": config.daily_call_budget,
        }));
    }
    let due = us_reference_symbols(state)
        .await?
        .into_iter()
        .filter(|candidate| {
            let last = attempts
                .iter()
                .find(|row| json_text(row, "symbol") == candidate.symbol);
            is_due(
                last.map(|row| profile_due_status(row["status"].as_str().unwrap_or_default())),
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
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let mut results = Vec::new();
    for (index, candidate) in due.iter().enumerate() {
        if index > 0 {
            sleep(StdDuration::from_millis(REQUEST_SPACING_MS)).await;
        }
        let outcome = fetch_profile(&client, &config, &api_key, &candidate.ticker).await;
        let status = record_profile(state, candidate, &outcome, now).await?;
        results.push(json!({"symbol": candidate.symbol, "status": status}));
        if status == "http_429" {
            break;
        }
    }
    let stored = results.iter().filter(|row| row["status"] == "ok").count();
    info!(
        attempted = results.len(),
        stored, "company profile refresh completed"
    );
    Ok(json!({
        "status": "completed",
        "attempted": results.len(),
        "stored": stored,
        "calls_today": calls_today + results.len(),
        "results": results,
        "scope": "read_only_reference_data",
    }))
}

/// `is_due` waits a full refresh for answers that will not change by
/// tomorrow; a stock outside the plan is one of them.
fn profile_due_status(status: &str) -> &str {
    match status {
        "http_402" => "premium_required",
        other => other,
    }
}

async fn fetch_profile(
    client: &reqwest::Client,
    config: &CompanyProfilesConfig,
    api_key: &str,
    ticker: &str,
) -> Result<CompanyProfile, &'static str> {
    let url = format!("{}/stable/profile", config.base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .query(&[("symbol", ticker), ("apikey", api_key)])
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
    parse_profile_body(&response.text().await.unwrap_or_default())
}

async fn record_profile(
    state: &AppState,
    candidate: &ReferenceSymbol,
    outcome: &Result<CompanyProfile, &'static str>,
    now: DateTime<Utc>,
) -> Result<&'static str> {
    let at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let text = |value: &Option<String>| {
        value
            .as_deref()
            .map_or_else(|| "NULL".to_string(), |v| format!("'{}'", sql_escape(v)))
    };
    let number = |value: Option<f64>| value.map_or_else(|| "NULL".to_string(), |v| v.to_string());
    let flag =
        |value: Option<bool>| value.map_or_else(|| "NULL".to_string(), |v| (v as i64).to_string());
    let empty = CompanyProfile::default();
    let (status, profile, success) = match outcome {
        Ok(profile) => ("ok", profile, format!("'{at}'")),
        Err(category) => {
            warn!(symbol = %candidate.symbol, category, "company profile not stored");
            (*category, &empty, "NULL".to_string())
        }
    };
    sqlx::query(&format!(
        "INSERT INTO company_profiles (
            symbol, ticker, status, company_name, sector, industry, beta, country, exchange,
            currency, market_cap, is_etf, is_adr, source, last_attempt_at, last_attempt_date,
            last_success_at
         ) VALUES ('{}', '{}', '{status}', {}, {}, {}, {}, {}, {}, {}, {}, {}, {},
                   'financial_modeling_prep', '{at}', '{}', {success})
         ON CONFLICT(symbol) DO UPDATE SET
            ticker = excluded.ticker,
            status = excluded.status,
            company_name = COALESCE(excluded.company_name, company_profiles.company_name),
            sector = COALESCE(excluded.sector, company_profiles.sector),
            industry = COALESCE(excluded.industry, company_profiles.industry),
            beta = COALESCE(excluded.beta, company_profiles.beta),
            country = COALESCE(excluded.country, company_profiles.country),
            exchange = COALESCE(excluded.exchange, company_profiles.exchange),
            currency = COALESCE(excluded.currency, company_profiles.currency),
            market_cap = COALESCE(excluded.market_cap, company_profiles.market_cap),
            is_etf = COALESCE(excluded.is_etf, company_profiles.is_etf),
            is_adr = COALESCE(excluded.is_adr, company_profiles.is_adr),
            last_attempt_at = excluded.last_attempt_at,
            last_attempt_date = excluded.last_attempt_date,
            last_success_at = COALESCE(excluded.last_success_at, company_profiles.last_success_at)",
        sql_escape(&candidate.symbol),
        sql_escape(&candidate.ticker),
        text(&profile.company_name),
        text(&profile.sector),
        text(&profile.industry),
        number(profile.beta),
        text(&profile.country),
        text(&profile.exchange),
        text(&profile.currency),
        number(profile.market_cap),
        flag(profile.is_etf),
        flag(profile.is_adr),
        now.date_naive(),
    ))
    .execute(&state.pool)
    .await
    .context("recording a company profile")?;
    Ok(status)
}

/// A held position, valued in DKK.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Holding {
    pub symbol: String,
    pub value_dkk: f64,
}

/// A stored profile's sector and beta, keyed by provider ticker.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProfileFacts {
    pub sector: String,
    pub beta: Option<f64>,
}

/// Groups holdings and recent buying by sector.
///
/// A holding without a profile (outside the US, or not fetched yet) is kept
/// under `Unprofiled`, so the shares still sum to the whole book.
pub(crate) fn concentration(
    holdings: &[Holding],
    recent_buys: &[Holding],
    profiles: &HashMap<String, ProfileFacts>,
) -> (
    Vec<SectorExposureRowPayload>,
    Option<f64>,
    Vec<HighBetaHoldingPayload>,
) {
    let facts = |symbol: &str| us_ticker(symbol).and_then(|ticker| profiles.get(&ticker));
    let sector_of = |symbol: &str| {
        facts(symbol)
            .map(|facts| facts.sector.clone())
            .unwrap_or_else(|| UNPROFILED.to_string())
    };
    let invested = holdings.iter().map(|row| row.value_dkk).sum::<f64>();
    let mut rows = BTreeMap::<String, SectorExposureRowPayload>::new();
    let mut beta_weight = BTreeMap::<String, (f64, f64)>::new();
    let mut high_beta = Vec::new();
    for holding in holdings {
        let sector = sector_of(&holding.symbol);
        let row = rows
            .entry(sector.clone())
            .or_insert_with(|| SectorExposureRowPayload {
                sector: sector.clone(),
                ..Default::default()
            });
        row.holdings += 1;
        row.value_dkk += holding.value_dkk;
        row.symbols.push(holding.symbol.clone());
        if let Some(beta) = facts(&holding.symbol).and_then(|facts| facts.beta) {
            let entry = beta_weight.entry(sector).or_default();
            entry.0 += beta * holding.value_dkk;
            entry.1 += holding.value_dkk;
            if beta >= HIGH_BETA {
                high_beta.push(HighBetaHoldingPayload {
                    symbol: holding.symbol.clone(),
                    beta,
                    value_dkk: holding.value_dkk,
                });
            }
        }
    }
    for buy in recent_buys {
        let sector = sector_of(&buy.symbol);
        let row = rows
            .entry(sector.clone())
            .or_insert_with(|| SectorExposureRowPayload {
                sector,
                ..Default::default()
            });
        row.buys_30d += 1;
        row.buys_30d_dkk += buy.value_dkk;
    }
    let mut rows = rows
        .into_values()
        .map(|mut row| {
            row.share_of_invested = (invested > 0.0).then(|| row.value_dkk / invested);
            row.weighted_beta = beta_weight
                .get(&row.sector)
                .filter(|(_, weight)| *weight > 0.0)
                .map(|(sum, weight)| sum / weight);
            row
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        (left.sector == UNPROFILED)
            .cmp(&(right.sector == UNPROFILED))
            .then(right.value_dkk.total_cmp(&left.value_dkk))
            .then(right.buys_30d_dkk.total_cmp(&left.buys_30d_dkk))
    });
    let (beta_sum, beta_value) = beta_weight.values().fold((0.0, 0.0), |acc, (sum, weight)| {
        (acc.0 + sum, acc.1 + weight)
    });
    let weighted_beta = (beta_value > 0.0).then(|| beta_sum / beta_value);
    high_beta.sort_by(|left, right| right.beta.total_cmp(&left.beta));
    (rows, weighted_beta, high_beta)
}

pub(crate) async fn sector_concentration_payload(
    state: &AppState,
) -> Result<SectorConcentrationPayload> {
    let mut profiles = HashMap::new();
    let mut by_status = BTreeMap::<String, i64>::new();
    for row in state
        .select_json("SELECT ticker, status, sector, beta FROM company_profiles")
        .await?
    {
        *by_status.entry(json_text(&row, "status")).or_default() += 1;
        let sector = json_text(&row, "sector");
        if !sector.is_empty() {
            profiles.insert(
                json_text(&row, "ticker"),
                ProfileFacts {
                    sector,
                    beta: row.get("beta").and_then(JsonValue::as_f64),
                },
            );
        }
    }
    let holdings = state
        .position_items(250)
        .await?
        .iter()
        .map(|row| Holding {
            symbol: json_text(row, "symbol"),
            value_dkk: value_f64(row, "market_value_dkk"),
        })
        .filter(|row| !row.symbol.is_empty())
        .collect::<Vec<_>>();
    let since =
        (Utc::now() - Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let recent_buys = state
        .select_json(&format!(
            "SELECT symbol, gross_amount_dkk FROM trade_ledger
             WHERE status = 'executed' AND side = 'BUY' AND created_at >= '{}'",
            sql_escape(&since)
        ))
        .await?
        .iter()
        .map(|row| Holding {
            symbol: json_text(row, "symbol"),
            value_dkk: value_f64(row, "gross_amount_dkk"),
        })
        .collect::<Vec<_>>();
    let (sectors, weighted_beta, high_beta_holdings) =
        concentration(&holdings, &recent_buys, &profiles);
    let profiled = holdings
        .iter()
        .filter(|row| us_ticker(&row.symbol).is_some_and(|ticker| profiles.contains_key(&ticker)))
        .count();
    Ok(SectorConcentrationPayload {
        generated_at: Utc::now().to_rfc3339(),
        invested_value_dkk: holdings.iter().map(|row| row.value_dkk).sum(),
        holdings: holdings.len() as i64,
        profiled_holdings: profiled as i64,
        weighted_beta_profiled: weighted_beta,
        sectors,
        high_beta_holdings,
        high_beta_threshold: HIGH_BETA,
        profile_status: by_status,
        caveat: "US stocks only: the free Financial Modeling Prep plan has no European or Danish profiles. Read-only; no sector limit is enforced.".to_string(),
        status: None,
        unavailable_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_is_read_and_refusals_are_categories() {
        let body = r#"[{"symbol":"MDB","companyName":"MongoDB, Inc.","sector":"Technology",
            "industry":"Software - Infrastructure","beta":1.585,"country":"US",
            "exchange":"NASDAQ","currency":"USD","marketCap":28000000000,"isEtf":false,
            "isAdr":false}]"#;
        let profile = parse_profile_body(body).unwrap();
        assert_eq!(profile.sector.as_deref(), Some("Technology"));
        assert_eq!(profile.beta, Some(1.585));
        assert_eq!(profile.is_etf, Some(false));
        assert_eq!(parse_profile_body("[]"), Err("no_data"));
        assert_eq!(
            parse_profile_body(r#"{"Error Message":"Invalid API KEY."}"#),
            Err("provider_notice")
        );
        assert_eq!(parse_profile_body("Premium Query"), Err("parse_error"));
    }

    /// Unprofiled holdings stay in the book's total, betas are value-weighted,
    /// and buying is grouped by the same sectors.
    #[test]
    fn holdings_and_buys_are_grouped_by_sector() {
        let profiles = HashMap::from([
            (
                "MDB".to_string(),
                ProfileFacts {
                    sector: "Technology".to_string(),
                    beta: Some(1.6),
                },
            ),
            (
                "SNOW".to_string(),
                ProfileFacts {
                    sector: "Technology".to_string(),
                    beta: Some(1.0),
                },
            ),
            (
                "XOM".to_string(),
                ProfileFacts {
                    sector: "Energy".to_string(),
                    beta: None,
                },
            ),
        ]);
        let holding = |symbol: &str, value_dkk: f64| Holding {
            symbol: symbol.to_string(),
            value_dkk,
        };
        let (rows, beta, high) = concentration(
            &[
                holding("MDB:xnas", 3_000.0),
                holding("SNOW:xnys", 1_000.0),
                holding("XOM:xnys", 2_000.0),
                holding("NOVO-B:xcse", 4_000.0),
            ],
            &[holding("MDB:xnas", 8_500.0), holding("DDOG:xnas", 7_000.0)],
            &profiles,
        );
        assert_eq!(rows[0].sector, "Technology");
        assert_eq!(rows[0].holdings, 2);
        assert_eq!(rows[0].share_of_invested, Some(0.4));
        assert!((rows[0].weighted_beta.unwrap() - 1.45).abs() < 1e-9);
        assert_eq!(rows[0].buys_30d, 1);
        assert_eq!(rows[1].sector, "Energy");
        assert_eq!(rows[1].weighted_beta, None);
        let unprofiled = rows.last().unwrap();
        assert_eq!(unprofiled.sector, UNPROFILED);
        assert_eq!(unprofiled.value_dkk, 4_000.0);
        assert_eq!(unprofiled.buys_30d_dkk, 7_000.0, "a buy not yet profiled");
        assert!((beta.unwrap() - 1.45).abs() < 1e-9);
        assert_eq!(high.len(), 1);
        assert_eq!(high[0].symbol, "MDB:xnas");
    }
}
