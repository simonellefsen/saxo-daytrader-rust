//! Live price monitor: keeps portfolio_price_snapshots fresh so market
//! values and the "Daily P/L Since 06:00" figure track real prices.
//!
//! The Python runtime had a price poller writing this table; the Rust port
//! read it but never wrote it, so positions fell back to broker open prices
//! and the static CSV daily P&L (frozen at import time). This module polls
//! Saxo infoprices for all held instruments on a short interval and
//! maintains a per-session baseline that resets at the configured local
//! reset hour (default 06:00).

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64},
    state::AppState,
};

/// Resolved Saxo identifiers for configured extra watch symbols; cached per
/// process so each non-held symbol costs one reference lookup, not one per
/// poll.
static EXTRA_INSTRUMENT_CACHE: OnceLock<RwLock<HashMap<String, (i64, String)>>> = OnceLock::new();

/// Symbols whose resolution failure was already logged. Extra watch symbols
/// may legitimately stay unresolvable for days (e.g. an IPO Saxo has not
/// listed yet); one warning is signal, one per poll is noise.
static RESOLUTION_WARNED: OnceLock<RwLock<std::collections::HashSet<String>>> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub(crate) struct PriceMonitorConfig {
    pub(crate) enabled: bool,
    pub(crate) poll_interval_minutes: u64,
    reset_hour_local: u32,
    timezone: Tz,
}

pub(crate) fn price_monitor_config(state: &AppState) -> PriceMonitorConfig {
    PriceMonitorConfig {
        enabled: yaml_bool(&state.config, &["price_monitor", "enabled"]).unwrap_or(true),
        poll_interval_minutes: yaml_i64(&state.config, &["price_monitor", "poll_interval_minutes"])
            .unwrap_or(1)
            .max(1) as u64,
        reset_hour_local: yaml_i64(&state.config, &["price_monitor", "reset_hour_local"])
            .unwrap_or(6)
            .clamp(0, 23) as u32,
        timezone: yaml_string(&state.config, &["price_monitor", "timezone"])
            .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen),
    }
}

/// The trading session a moment belongs to: before the local reset hour the
/// session is still the previous calendar day's.
pub(crate) fn session_date_for(now_utc: DateTime<Utc>, timezone: Tz, reset_hour: u32) -> NaiveDate {
    let local = now_utc.with_timezone(&timezone);
    if local.hour() < reset_hour {
        local.date_naive() - Duration::days(1)
    } else {
        local.date_naive()
    }
}

/// Best available price from a Saxo infoprice item plus the last close.
pub(crate) fn parse_info_price(item: &JsonValue) -> (Option<f64>, Option<f64>) {
    let details = item.get("PriceInfoDetails").cloned().unwrap_or(json!({}));
    let quote = item.get("Quote").cloned().unwrap_or(json!({}));
    let positive = |value: f64| if value > 0.0 { Some(value) } else { None };
    let last_traded = positive(value_f64(&details, "LastTraded"));
    let mid = positive(value_f64(&quote, "Mid"));
    let bid = positive(value_f64(&quote, "Bid"));
    let ask = positive(value_f64(&quote, "Ask"));
    let bid_ask_mid = match (bid, ask) {
        (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
        (one, other) => one.or(other),
    };
    let price = last_traded.or(mid).or(bid_ask_mid);
    let last_close = positive(value_f64(&details, "LastClose"));
    (price, last_close)
}

/// Background loop spawned by the scheduler process.
pub async fn run_price_monitor_loop(state: AppState) {
    let config = price_monitor_config(&state);
    if !config.enabled {
        info!("price monitor disabled by configuration");
        return;
    }
    info!(
        poll_interval_minutes = config.poll_interval_minutes,
        reset_hour_local = config.reset_hour_local,
        "starting price monitor loop"
    );
    loop {
        match refresh_portfolio_prices(&state).await {
            Ok(summary) => {
                let updated = summary
                    .get("updated")
                    .and_then(JsonValue::as_i64)
                    .unwrap_or(0);
                if updated > 0 {
                    info!(updated, "price monitor refreshed portfolio prices");
                }
            }
            Err(err) => warn!("price monitor refresh failed: {err:#}"),
        }
        sleep(StdDuration::from_secs(config.poll_interval_minutes * 60)).await;
    }
}

pub async fn refresh_portfolio_prices(state: &AppState) -> Result<JsonValue> {
    let config = price_monitor_config(state);
    let session = match state.ensure_saxo_session_json("price_monitor").await {
        Ok(session) => session,
        Err(err) => {
            return Ok(json!({
                "status": "no_session",
                "updated": 0,
                "error": format!("{err:#}")
            }));
        }
    };
    let calendar_refresh = match state.refresh_saxo_exchange_calendars_if_stale().await {
        Ok(value) => value,
        Err(err) => {
            warn!("price monitor using fallback exchange calendar: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let market_rows = state.market_exchange_rows();
    let mut instruments = held_instruments(state).await?;
    append_extra_watch_instruments(state, &session, &market_rows, &mut instruments).await;
    if instruments.is_empty() {
        return Ok(json!({"status": "ok", "updated": 0, "reason": "no_positions"}));
    }
    let total_instruments = instruments.len();
    let (tradable_instruments, skipped_closed) =
        filter_tradable_instruments(&instruments, &market_rows);
    if tradable_instruments.is_empty() {
        return Ok(json!({
            "status": "market_closed",
            "updated": 0,
            "instruments": total_instruments,
            "tradable_instruments": 0,
            "skipped_closed": skipped_closed.len(),
            "skipped_closed_symbols": skipped_closed,
            "calendar_refresh": calendar_refresh,
            "reason": "all_known_exchanges_closed"
        }));
    }
    instruments = tradable_instruments;
    let session_date = session_date_for(Utc::now(), config.timezone, config.reset_hour_local);
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let fx_refresh = match crate::fx::refresh_best_effort_fx_rates(state, &session).await {
        Ok(value) => value,
        Err(err) => {
            warn!("FX rate refresh failed; using cached/static FX fallback: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };

    // One infoprices/list call per asset type covers all held instruments.
    let mut by_asset_type: HashMap<String, Vec<&HeldInstrument>> = HashMap::new();
    for instrument in &instruments {
        by_asset_type
            .entry(instrument.asset_type.clone())
            .or_default()
            .push(instrument);
    }
    let mut updated = 0usize;
    let mut errors = Vec::new();
    for (asset_type, group) in by_asset_type {
        let uics = group
            .iter()
            .map(|instrument| instrument.uic.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let query = vec![
            (
                "AccountKey",
                crate::markov_method::account_key(state, &session)?,
            ),
            ("AssetType", asset_type.clone()),
            ("Uics", uics),
            ("FieldGroups", "Quote,PriceInfoDetails".to_string()),
        ];
        let payload = match crate::markov_method::saxo_get_json(
            state,
            &session,
            "/trade/v1/infoprices/list",
            &query,
        )
        .await
        {
            Ok(payload) => payload,
            Err(err) => {
                errors.push(format!("{asset_type}: {err:#}"));
                continue;
            }
        };
        let by_uic: HashMap<i64, &HeldInstrument> = group
            .iter()
            .map(|instrument| (instrument.uic, *instrument))
            .collect();
        for item in payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let uic = item.get("Uic").and_then(JsonValue::as_i64).unwrap_or(0);
            let Some(instrument) = by_uic.get(&uic) else {
                continue;
            };
            let (price, last_close) = parse_info_price(&item);
            let Some(price) = price else {
                continue;
            };
            match upsert_price_snapshot(state, instrument, price, last_close, session_date, &now)
                .await
            {
                Ok(()) => updated += 1,
                Err(err) => errors.push(format!("{}: {err:#}", instrument.symbol)),
            }
        }
    }
    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "updated": updated,
        "instruments": total_instruments,
        "tradable_instruments": instruments.len(),
        "skipped_closed": skipped_closed.len(),
        "skipped_closed_symbols": skipped_closed,
        "session_date": session_date.to_string(),
        "calendar_refresh": calendar_refresh,
        "fx_refresh": fx_refresh,
        "errors": errors,
    }))
}

#[derive(Clone, Debug)]
struct HeldInstrument {
    symbol: String,
    uic: i64,
    asset_type: String,
    currency: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExtraWatchSymbol {
    pub(crate) symbol: String,
    pub(crate) isin: Option<String>,
}

/// Symbols watched on top of actual holdings, from
/// market_data.watchlists.extra_symbols. Entries are either a plain symbol
/// string or a mapping with `symbol` and optional `isin` (the precise
/// resolution key for instruments Saxo lists under an unexpected symbol or
/// that are still propagating into SIM reference data). They get the same
/// live quotes and daily baselines as held positions and surface on the
/// watchlist tabs.
fn extra_watch_symbols(state: &AppState) -> Vec<ExtraWatchSymbol> {
    parse_extra_watch_symbols(&state.config)
}

pub(crate) fn parse_extra_watch_symbols(config: &serde_yaml::Value) -> Vec<ExtraWatchSymbol> {
    yaml_at(config, &["market_data", "watchlists", "extra_symbols"])
        .and_then(|value| value.as_sequence())
        .map(|sequence| {
            sequence
                .iter()
                .filter_map(|entry| {
                    if let Some(symbol) = entry.as_str() {
                        let symbol = symbol.trim().to_string();
                        if symbol.is_empty() {
                            return None;
                        }
                        return Some(ExtraWatchSymbol { symbol, isin: None });
                    }
                    let symbol = entry.get("symbol")?.as_str()?.trim().to_string();
                    if symbol.is_empty() {
                        return None;
                    }
                    let isin = entry
                        .get("isin")
                        .and_then(|value| value.as_str())
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty());
                    Some(ExtraWatchSymbol { symbol, isin })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve an instrument by ISIN, preferring a candidate on the exchange the
/// configured symbol names. ISIN search is exact, so this finds instruments
/// whose Saxo symbol differs from expectations.
async fn resolve_by_isin(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
    isin: &str,
) -> Option<(i64, String)> {
    let query = vec![
        (
            "AccountKey",
            crate::markov_method::account_key(state, session).ok()?,
        ),
        ("Keywords", isin.to_string()),
        ("AssetTypes", "Stock,Etf,Etn,Etc".to_string()),
        ("IncludeNonTradable", "false".to_string()),
    ];
    let payload =
        crate::markov_method::saxo_get_json(state, session, "/ref/v1/instruments", &query)
            .await
            .ok()?;
    let candidates = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let exchange_suffix = symbol
        .split_once(':')
        .map(|(_, exchange)| format!(":{}", exchange.to_lowercase()))
        .unwrap_or_default();
    let preferred = candidates.iter().find(|candidate| {
        !exchange_suffix.is_empty()
            && candidate
                .get("Symbol")
                .and_then(JsonValue::as_str)
                .map(|value| value.to_lowercase().ends_with(&exchange_suffix))
                .unwrap_or(false)
    });
    let selected = preferred.or(if candidates.len() == 1 {
        candidates.first()
    } else {
        None
    })?;
    let uic = selected.get("Identifier").and_then(JsonValue::as_i64)?;
    let asset_type = selected
        .get("AssetType")
        .and_then(JsonValue::as_str)?
        .to_string();
    Some((uic, asset_type))
}

async fn append_extra_watch_instruments(
    state: &AppState,
    session: &JsonValue,
    market_rows: &[JsonValue],
    instruments: &mut Vec<HeldInstrument>,
) {
    let cache = EXTRA_INSTRUMENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    for watch in extra_watch_symbols(state) {
        let symbol = watch.symbol.clone();
        if symbol_market_tradable(&symbol, market_rows) == Some(false) {
            continue;
        }
        if instruments
            .iter()
            .any(|instrument| instrument.symbol == symbol)
        {
            continue;
        }
        let cached = cache
            .read()
            .ok()
            .and_then(|cache| cache.get(&symbol).cloned());
        let resolved = match cached {
            Some(found) => Some(found),
            None => {
                // ISIN is the exact key when provided; symbol search is the
                // fallback. Cache only successes so failures retry next poll.
                let mut entry = match &watch.isin {
                    Some(isin) => resolve_by_isin(state, session, &symbol, isin).await,
                    None => None,
                };
                if entry.is_none() {
                    entry = crate::markov_method::resolve_instrument(state, session, &symbol)
                        .await
                        .ok()
                        .map(|instrument| (instrument.uic, instrument.asset_type));
                }
                match entry {
                    Some(found) => {
                        if let Ok(mut cache) = cache.write() {
                            cache.insert(symbol.clone(), found.clone());
                        }
                        Some(found)
                    }
                    None => {
                        let warned = RESOLUTION_WARNED
                            .get_or_init(|| RwLock::new(std::collections::HashSet::new()));
                        let first_failure = warned
                            .write()
                            .map(|mut warned| warned.insert(symbol.clone()))
                            .unwrap_or(true);
                        if first_failure {
                            warn!(
                                symbol,
                                "extra watch symbol is not resolvable on Saxo yet; will keep retrying quietly"
                            );
                        }
                        None
                    }
                }
            }
        };
        let Some((uic, asset_type)) = resolved else {
            continue;
        };
        let currency = symbol
            .split_once(':')
            .and_then(|(_, exchange)| crate::saxo_order::currency_for_exchange(exchange))
            .unwrap_or("DKK")
            .to_string();
        instruments.push(HeldInstrument {
            symbol,
            uic,
            asset_type,
            currency,
        });
    }
}

fn exchange_code_for_symbol(symbol: &str) -> Option<String> {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.trim().to_uppercase())
        .filter(|exchange| !exchange.is_empty())
}

fn market_row_for_exchange<'a>(
    market_rows: &'a [JsonValue],
    exchange_code: &str,
) -> Option<&'a JsonValue> {
    market_rows.iter().find(|row| {
        row.get("code")
            .and_then(JsonValue::as_str)
            .map(|code| code.eq_ignore_ascii_case(exchange_code))
            .unwrap_or(false)
    })
}

fn market_row_tradable(row: &JsonValue) -> bool {
    row.get("is_tradable")
        .and_then(JsonValue::as_bool)
        .or_else(|| row.get("is_open").and_then(JsonValue::as_bool))
        .unwrap_or(false)
}

fn symbol_market_tradable(symbol: &str, market_rows: &[JsonValue]) -> Option<bool> {
    let exchange_code = exchange_code_for_symbol(symbol)?;
    market_row_for_exchange(market_rows, &exchange_code).map(market_row_tradable)
}

fn filter_tradable_instruments(
    instruments: &[HeldInstrument],
    market_rows: &[JsonValue],
) -> (Vec<HeldInstrument>, Vec<JsonValue>) {
    let mut tradable = Vec::new();
    let mut skipped = Vec::new();
    for instrument in instruments {
        let Some(exchange_code) = exchange_code_for_symbol(&instrument.symbol) else {
            tradable.push(instrument.clone());
            continue;
        };
        let Some(market) = market_row_for_exchange(market_rows, &exchange_code) else {
            tradable.push(instrument.clone());
            continue;
        };
        if market_row_tradable(market) {
            tradable.push(instrument.clone());
            continue;
        }
        skipped.push(json!({
            "symbol": instrument.symbol,
            "exchange": exchange_code,
            "status_reason": market.get("status_reason").cloned().unwrap_or(JsonValue::Null),
            "next_open_at_utc": market.get("next_open_at_utc").cloned().unwrap_or(JsonValue::Null),
        }));
    }
    (tradable, skipped)
}

async fn held_instruments(state: &AppState) -> Result<Vec<HeldInstrument>> {
    let rows = sqlx::query(
        "SELECT symbol, uic, asset_type, currency
         FROM broker_position_snapshots
         WHERE uic IS NOT NULL AND asset_type IS NOT NULL AND quantity <> 0",
    )
    .fetch_all(&state.pool)
    .await
    .context("loading held instruments for price monitor")?;
    Ok(rows
        .iter()
        .map(row_to_json)
        .filter_map(|row| {
            let symbol = row.get("symbol").and_then(JsonValue::as_str)?.to_string();
            let uic = row
                .get("uic")
                .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))?;
            let currency = row
                .get("currency")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    symbol.split_once(':').and_then(|(_, exchange)| {
                        crate::saxo_order::currency_for_exchange(exchange).map(str::to_string)
                    })
                })
                .unwrap_or_else(|| "DKK".to_string());
            Some(HeldInstrument {
                symbol,
                uic,
                asset_type: row
                    .get("asset_type")
                    .and_then(JsonValue::as_str)?
                    .to_string(),
                currency,
            })
        })
        .collect())
}

async fn upsert_price_snapshot(
    state: &AppState,
    instrument: &HeldInstrument,
    price: f64,
    last_close: Option<f64>,
    session_date: NaiveDate,
    now: &str,
) -> Result<()> {
    let fx_rate =
        crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, &instrument.currency).await;
    let existing = sqlx::query(&format!(
        "SELECT baseline_session_date, baseline_price_local, baseline_fx_rate_to_dkk
         FROM portfolio_price_snapshots WHERE symbol = '{}'",
        sql_escape(&instrument.symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    let existing = existing.as_ref().map(row_to_json);
    let baseline_is_current = existing
        .as_ref()
        .map(|row| {
            row.get("baseline_session_date").and_then(JsonValue::as_str)
                == Some(session_date.to_string().as_str())
                && value_f64(row, "baseline_price_local") > 0.0
        })
        .unwrap_or(false);
    // New session: anchor the daily baseline at the previous close so the
    // first poll of the day starts P/L from yesterday's close, not zero.
    let (baseline_price, baseline_fx) = if baseline_is_current {
        let row = existing.as_ref().expect("existing row present");
        (
            value_f64(row, "baseline_price_local"),
            value_f64(row, "baseline_fx_rate_to_dkk"),
        )
    } else {
        (last_close.unwrap_or(price), fx_rate)
    };
    let change_pct = if baseline_price > 0.0 {
        (price - baseline_price) / baseline_price
    } else {
        0.0
    };
    let previous_close_sql = last_close
        .map(|value| format!("{value}"))
        .unwrap_or_else(|| "NULL".to_string());
    let updated = sqlx::query(&format!(
        "UPDATE portfolio_price_snapshots SET
            updated_at = '{now}',
            baseline_session_date = '{session_date}',
            baseline_at = CASE WHEN baseline_session_date = '{session_date}' THEN baseline_at ELSE '{now}' END,
            current_price_local = {price},
            current_fx_rate_to_dkk = {fx_rate},
            previous_close_local = {previous_close_sql},
            change_pct = {change_pct},
            currency = '{}',
            source = 'saxo_infoprices',
            status = 'ok',
            baseline_price_local = {baseline_price},
            baseline_fx_rate_to_dkk = {baseline_fx}
         WHERE symbol = '{}'",
        sql_escape(&instrument.currency),
        sql_escape(&instrument.symbol),
    ))
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        sqlx::query(&format!(
            "INSERT INTO portfolio_price_snapshots (
                symbol, updated_at, baseline_session_date, baseline_at,
                current_price_local, current_fx_rate_to_dkk, previous_close_local,
                change_pct, currency, source, status,
                baseline_price_local, baseline_fx_rate_to_dkk
            ) VALUES (
                '{}', '{now}', '{session_date}', '{now}',
                {price}, {fx_rate}, {previous_close_sql},
                {change_pct}, '{}', 'saxo_infoprices', 'ok',
                {baseline_price}, {baseline_fx}
            )",
            sql_escape(&instrument.symbol),
            sql_escape(&instrument.currency),
        ))
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn session_rolls_over_at_reset_hour() {
        let tz: Tz = "Europe/Copenhagen".parse().unwrap();
        // 05:30 local on June 11 belongs to the June 10 session.
        let before_reset = tz
            .with_ymd_and_hms(2026, 6, 11, 5, 30, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            session_date_for(before_reset, tz, 6),
            NaiveDate::from_ymd_opt(2026, 6, 10).unwrap()
        );
        // 06:01 local on June 11 starts the June 11 session.
        let after_reset = tz
            .with_ymd_and_hms(2026, 6, 11, 6, 1, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            session_date_for(after_reset, tz, 6),
            NaiveDate::from_ymd_opt(2026, 6, 11).unwrap()
        );
    }

    #[test]
    fn parses_extra_watch_symbols_in_both_forms() {
        let config: serde_yaml::Value = serde_yaml::from_str(
            r#"
market_data:
  watchlists:
    extra_symbols:
      - symbol: SPCX:xnas
        isin: US84615Q1031
      - RKLB:xnas
"#,
        )
        .unwrap();
        let symbols = parse_extra_watch_symbols(&config);
        assert_eq!(
            symbols,
            vec![
                ExtraWatchSymbol {
                    symbol: "SPCX:xnas".to_string(),
                    isin: Some("US84615Q1031".to_string())
                },
                ExtraWatchSymbol {
                    symbol: "RKLB:xnas".to_string(),
                    isin: None
                },
            ]
        );
    }

    #[test]
    fn parses_info_price_with_fallbacks() {
        let full = json!({
            "PriceInfoDetails": {"LastTraded": 101.5, "LastClose": 100.0},
            "Quote": {"Mid": 101.4, "Bid": 101.3, "Ask": 101.5}
        });
        assert_eq!(parse_info_price(&full), (Some(101.5), Some(100.0)));

        let quote_only = json!({"Quote": {"Bid": 10.0, "Ask": 11.0}});
        assert_eq!(parse_info_price(&quote_only), (Some(10.5), None));

        let mid_only = json!({"Quote": {"Mid": 55.5}});
        assert_eq!(parse_info_price(&mid_only), (Some(55.5), None));

        let empty = json!({});
        assert_eq!(parse_info_price(&empty), (None, None));
    }

    #[test]
    fn extracts_exchange_code_from_saxo_symbol() {
        assert_eq!(
            exchange_code_for_symbol("AMD:xnas"),
            Some("XNAS".to_string())
        );
        assert_eq!(
            exchange_code_for_symbol("NOVOb:xcse"),
            Some("XCSE".to_string())
        );
        assert_eq!(exchange_code_for_symbol("CASH"), None);
    }

    #[test]
    fn filter_tradable_instruments_skips_closed_known_exchanges() {
        let instruments = vec![
            HeldInstrument {
                symbol: "AMD:xnas".to_string(),
                uic: 1,
                asset_type: "Stock".to_string(),
                currency: "USD".to_string(),
            },
            HeldInstrument {
                symbol: "NOVOb:xcse".to_string(),
                uic: 2,
                asset_type: "Stock".to_string(),
                currency: "DKK".to_string(),
            },
            HeldInstrument {
                symbol: "UNKNOWN:xabc".to_string(),
                uic: 3,
                asset_type: "Stock".to_string(),
                currency: "DKK".to_string(),
            },
        ];
        let market_rows = vec![
            json!({
                "code": "XNAS",
                "is_tradable": true,
                "status_reason": "Open"
            }),
            json!({
                "code": "XCSE",
                "is_tradable": false,
                "status_reason": "Closed - After hours",
                "next_open_at_utc": "2026-07-10T07:00:00Z"
            }),
        ];

        let (tradable, skipped) = filter_tradable_instruments(&instruments, &market_rows);

        assert_eq!(
            tradable
                .iter()
                .map(|instrument| instrument.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["AMD:xnas", "UNKNOWN:xabc"]
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["symbol"], "NOVOb:xcse");
        assert_eq!(skipped[0]["exchange"], "XCSE");
    }
}
