//! What every Alpha Vantage store shares: the key, one daily allowance and
//! the way Alpha Vantage refuses.
//!
//! A free key allows 25 calls a day across every function, so the report-date
//! store and the congress-trade store draw on one allowance. Each keeps one
//! fetch row per stock and asks a stock at most once a day, so the rows dated
//! today across `FETCH_TABLES` are the calls made today.
//!
//! Alpha Vantage takes its key only in the query string; callers strip the URL
//! from every request error and store a failure as a category, never as text.

use std::time::Duration as StdDuration;

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value as JsonValue;

use crate::{
    config::{yaml_i64, yaml_string},
    state::AppState,
};

const DEFAULT_BASE_URL: &str = "https://www.alphavantage.co";
const DEFAULT_DAILY_CALL_BUDGET: i64 = 24;
/// Alpha Vantage refuses a free key's second call inside a second.
pub(crate) const REQUEST_SPACING_MS: u64 = 1_300;
/// One row per stock per store, dated by its last attempt.
const FETCH_TABLES: &[&str] = &["earnings_date_fetches", "congress_trade_fetches"];

#[derive(Clone, Debug)]
pub(crate) struct AlphaVantage {
    pub api_key: Option<String>,
    pub base_url: String,
    /// Calls a day across every store, under the free key's 25.
    pub daily_call_budget: usize,
}

pub(crate) fn alpha_vantage(state: &AppState) -> AlphaVantage {
    AlphaVantage {
        api_key: yaml_string(&state.config, &["alpha_vantage", "api_key"])
            .or_else(|| std::env::var("ALPHAVANTAGE_API_KEY").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        base_url: yaml_string(&state.config, &["alpha_vantage", "base_url"])
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        daily_call_budget: yaml_i64(&state.config, &["alpha_vantage", "daily_call_budget"])
            .unwrap_or(DEFAULT_DAILY_CALL_BUDGET)
            .clamp(0, 500) as usize,
    }
}

/// Alpha Vantage answers a refusal with HTTP 200 and a JSON note
/// (`Information`, `Note` or `Error Message`), so the body decides, not the
/// status.
pub(crate) fn refusal(payload: &JsonValue) -> Option<&'static str> {
    if payload.get("Error Message").is_some() {
        return Some("invalid_symbol");
    }
    payload
        .get("Information")
        .or_else(|| payload.get("Note"))
        .and_then(JsonValue::as_str)
        .map(classify_provider_note)
}

/// Alpha Vantage words its refusals in prose. A daily cap stops every store
/// until tomorrow; a burst limit only until the next call.
fn classify_provider_note(note: &str) -> &'static str {
    let note = note.to_lowercase();
    if note.contains("per day") || note.contains("daily") {
        "quota_exhausted"
    } else if note.contains("per second") || note.contains("spreading out") {
        "rate_limited"
    } else if note.contains("premium") {
        "premium_required"
    } else {
        "provider_notice"
    }
}

/// One `function` for one `symbol`: the body, or a failure category
/// (`http_<status>`, `timeout`, `request_failed`).
pub(crate) async fn get(
    client: &reqwest::Client,
    provider: &AlphaVantage,
    api_key: &str,
    function: &str,
    symbol: &str,
) -> Result<String, &'static str> {
    let url = format!("{}/query", provider.base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .query(&[
            ("function", function),
            ("symbol", symbol),
            ("apikey", api_key),
        ])
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
        return Err(crate::earnings_dates::http_category(status.as_u16()));
    }
    Ok(response.text().await.unwrap_or_default())
}

pub(crate) fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()?)
}

/// Calls made on `day` across every store. A store whose table is missing
/// counts none; the provider's own daily refusal is the backstop.
pub(crate) async fn calls_made_on(state: &AppState, day: NaiveDate) -> usize {
    let mut calls = 0;
    for table in FETCH_TABLES {
        calls += state
            .select_json(&format!(
                "SELECT COUNT(*) AS calls FROM {table} WHERE last_attempt_date = '{day}'"
            ))
            .await
            .ok()
            .and_then(|rows| rows.first()?.get("calls")?.as_i64())
            .unwrap_or(0)
            .max(0) as usize;
    }
    calls
}

/// Whether any store was refused for the day's allowance on `day`.
pub(crate) async fn quota_exhausted_on(state: &AppState, day: NaiveDate) -> bool {
    for table in FETCH_TABLES {
        let exhausted = state
            .select_json(&format!(
                "SELECT last_status FROM {table}
                 WHERE last_attempt_date = '{day}' AND last_status = 'quota_exhausted' LIMIT 1"
            ))
            .await
            .is_ok_and(|rows| !rows.is_empty());
        if exhausted {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn refusals_are_read_from_the_body() {
        let note = |text: &str| refusal(&json!({"Information": text}));
        assert_eq!(
            note(
                "Our standard API rate limit is 25 requests per day. Please subscribe to premium."
            ),
            Some("quota_exhausted")
        );
        assert_eq!(
            note(
                "Please consider spreading out your free API requests more sparingly (1 request per second)."
            ),
            Some("rate_limited")
        );
        assert_eq!(
            note("This is a premium endpoint."),
            Some("premium_required")
        );
        assert_eq!(
            refusal(&json!({"Error Message": "Invalid API call."})),
            Some("invalid_symbol")
        );
        assert_eq!(refusal(&json!({"trades": []})), None);
    }
}
