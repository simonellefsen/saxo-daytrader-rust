use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::{Value as JsonValue, json};
use sqlx::AnyPool;
use tracing::warn;

use crate::{
    db::{sql_escape, value_f64},
    state::AppState,
};

const ECB_DAILY_RATES_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml";
const SAXO_FX_CACHE_TTL_MINUTES: i64 = 30;
const SAXO_FX_SOURCE: &str = "saxo_fx_spot";
const ECB_FX_SOURCE: &str = "ecb_eurofxref_daily";
const COMMON_FX_CURRENCIES_TO_DKK: &[&str] = &["EUR", "USD", "GBP", "NOK", "SEK", "PLN"];

#[derive(Clone, Debug)]
struct SaxoFxPair {
    currency: String,
    pair_symbol: String,
    uic: i64,
    inverted: bool,
}

pub(crate) fn static_fx_rate_to_dkk(currency: &str) -> f64 {
    match currency.trim().to_uppercase().as_str() {
        "DKK" => 1.0,
        "EUR" => 7.4604,
        "USD" => 7.0215,
        "GBP" => 8.70,
        "NOK" => 0.64,
        "SEK" => 0.67,
        "PLN" => 1.75,
        _ => 1.0,
    }
}

pub(crate) async fn cached_or_static_fx_rate_to_dkk(pool: &AnyPool, currency: &str) -> f64 {
    match cached_fx_rate_to_dkk(pool, currency).await {
        Ok(Some(rate)) => rate,
        Ok(None) => static_fx_rate_to_dkk(currency),
        Err(err) => {
            warn!("FX cache lookup failed for {currency}: {err:#}");
            static_fx_rate_to_dkk(currency)
        }
    }
}

pub(crate) async fn cached_fx_rate_to_dkk(pool: &AnyPool, currency: &str) -> Result<Option<f64>> {
    let code = normalize_currency(currency);
    if code == "DKK" {
        return Ok(Some(1.0));
    }
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let row = sqlx::query(&format!(
        "SELECT rate_to_dkk, expires_at
         FROM currency_fx_rates
         WHERE currency_code = '{}' AND base_currency = 'DKK' AND expires_at > '{}'",
        sql_escape(&code),
        sql_escape(&now)
    ))
    .fetch_optional(pool)
    .await
    .context("loading cached FX rate")?;
    Ok(row
        .as_ref()
        .map(crate::db::row_to_json)
        .map(|row| value_f64(&row, "rate_to_dkk"))
        .filter(|value| value.is_finite() && *value > 0.0))
}

pub(crate) async fn refresh_best_effort_fx_rates(
    state: &AppState,
    session: &JsonValue,
) -> Result<serde_json::Value> {
    if let Some(summary) = fresh_cache_summary(&state.pool, SAXO_FX_SOURCE, "USD").await? {
        return Ok(summary);
    }
    match refresh_saxo_fx_rates(state, session).await {
        Ok(summary) => {
            if summary
                .get("upserted")
                .and_then(JsonValue::as_i64)
                .unwrap_or(0)
                > 0
            {
                return Ok(summary);
            }
            warn!("Saxo FX spot refresh produced no usable rates; falling back to ECB");
        }
        Err(err) => warn!("Saxo FX spot refresh failed; falling back to ECB: {err:#}"),
    }
    refresh_ecb_fx_rates(&state.pool).await
}

pub(crate) async fn refresh_ecb_fx_rates(pool: &AnyPool) -> Result<serde_json::Value> {
    if let Some(summary) = fresh_cache_summary(pool, ECB_FX_SOURCE, "EUR").await? {
        return Ok(summary);
    }

    let xml = Client::new()
        .get(ECB_DAILY_RATES_URL)
        .send()
        .await
        .context("fetching ECB FX rates")?
        .error_for_status()
        .context("ECB FX rates returned unsuccessful status")?
        .text()
        .await
        .context("reading ECB FX rates response")?;
    let eur_rates = parse_ecb_daily_rates(&xml)?;
    let dkk_per_eur = *eur_rates
        .get("DKK")
        .filter(|value| value.is_finite() && **value > 0.0)
        .context("ECB FX response did not include a positive DKK rate")?;
    let observed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let expires_at =
        (Utc::now() + Duration::hours(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let raw_payload = serde_json::to_string(&json!({
        "source_url": ECB_DAILY_RATES_URL,
        "rate_count": eur_rates.len(),
    }))?;

    let mut upserted = 0usize;
    for (currency, rate_per_eur) in eur_rates {
        if !rate_per_eur.is_finite() || rate_per_eur <= 0.0 {
            continue;
        }
        let rate_to_dkk = if currency == "EUR" {
            dkk_per_eur
        } else {
            dkk_per_eur / rate_per_eur
        };
        if !rate_to_dkk.is_finite() || rate_to_dkk <= 0.0 {
            continue;
        }
        upsert_fx_rate(
            pool,
            &currency,
            rate_to_dkk,
            ECB_FX_SOURCE,
            &observed_at,
            &expires_at,
            &raw_payload,
        )
        .await?;
        upserted += 1;
    }
    upsert_fx_rate(
        pool,
        "DKK",
        1.0,
        "native",
        &observed_at,
        &expires_at,
        &raw_payload,
    )
    .await?;
    Ok(json!({
        "status": "ok",
        "source": ECB_FX_SOURCE,
        "upserted": upserted + 1,
        "observed_at": observed_at,
        "expires_at": expires_at
    }))
}

async fn refresh_saxo_fx_rates(state: &AppState, session: &JsonValue) -> Result<serde_json::Value> {
    let observed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let expires_at = (Utc::now() + Duration::minutes(SAXO_FX_CACHE_TTL_MINUTES))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut pairs = Vec::new();
    let mut errors = Vec::new();
    for currency in COMMON_FX_CURRENCIES_TO_DKK {
        match resolve_saxo_fx_pair(state, session, currency).await {
            Ok(Some(pair)) => pairs.push(pair),
            Ok(None) => errors.push(format!("{currency}/DKK: no Saxo FX spot instrument")),
            Err(err) => errors.push(format!("{currency}/DKK: {err:#}")),
        }
    }
    if pairs.is_empty() {
        return Ok(json!({
            "status": "empty",
            "source": SAXO_FX_SOURCE,
            "upserted": 0,
            "errors": errors
        }));
    }

    let by_uic = pairs
        .iter()
        .map(|pair| (pair.uic, pair.clone()))
        .collect::<HashMap<_, _>>();
    let uics = pairs
        .iter()
        .map(|pair| pair.uic.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let query = vec![
        (
            "AccountKey",
            crate::markov_method::account_key(state, session)?,
        ),
        ("AssetType", "FxSpot".to_string()),
        ("Uics", uics),
        ("FieldGroups", "Quote,PriceInfoDetails".to_string()),
    ];
    let payload =
        crate::markov_method::saxo_get_json(state, session, "/trade/v1/infoprices/list", &query)
            .await
            .context("fetching Saxo FX spot infoprices")?;
    let raw_payload = serde_json::to_string(&json!({
        "source": SAXO_FX_SOURCE,
        "pair_count": pairs.len(),
    }))?;
    let mut upserted = 0usize;
    for item in payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let uic = item.get("Uic").and_then(JsonValue::as_i64).unwrap_or(0);
        let Some(pair) = by_uic.get(&uic) else {
            continue;
        };
        let Some(rate_to_dkk) = parse_saxo_fx_rate_to_dkk(&item, pair.inverted) else {
            errors.push(format!("{}: no positive quote", pair.pair_symbol));
            continue;
        };
        upsert_fx_rate(
            &state.pool,
            &pair.currency,
            rate_to_dkk,
            SAXO_FX_SOURCE,
            &observed_at,
            &expires_at,
            &raw_payload,
        )
        .await?;
        upserted += 1;
    }
    upsert_fx_rate(
        &state.pool,
        "DKK",
        1.0,
        "native",
        &observed_at,
        &expires_at,
        &raw_payload,
    )
    .await?;
    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "source": SAXO_FX_SOURCE,
        "upserted": upserted + 1,
        "observed_at": observed_at,
        "expires_at": expires_at,
        "errors": errors
    }))
}

async fn fresh_cache_summary(
    pool: &AnyPool,
    source: &str,
    probe_currency: &str,
) -> Result<Option<serde_json::Value>> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let row = sqlx::query(&format!(
        "SELECT observed_at, expires_at
         FROM currency_fx_rates
         WHERE currency_code = '{}'
           AND base_currency = 'DKK'
           AND source = '{}'
           AND expires_at > '{}'
         ORDER BY observed_at DESC
         LIMIT 1",
        sql_escape(probe_currency),
        sql_escape(source),
        sql_escape(&now)
    ))
    .fetch_optional(pool)
    .await
    .context("checking FX cache freshness")?;
    Ok(row.as_ref().map(|row| {
        let row = crate::db::row_to_json(row);
        json!({
            "status": "cached",
            "source": source,
            "observed_at": row.get("observed_at").and_then(|value| value.as_str()).unwrap_or(""),
            "expires_at": row.get("expires_at").and_then(|value| value.as_str()).unwrap_or("")
        })
    }))
}

async fn resolve_saxo_fx_pair(
    state: &AppState,
    session: &JsonValue,
    currency: &str,
) -> Result<Option<SaxoFxPair>> {
    let code = normalize_currency(currency);
    if code == "DKK" {
        return Ok(None);
    }
    for (base, quote, inverted) in [
        (code.clone(), "DKK".to_string(), false),
        ("DKK".to_string(), code.clone(), true),
    ] {
        let keyword = format!("{base}{quote}");
        let query = vec![
            (
                "AccountKey",
                crate::markov_method::account_key(state, session)?,
            ),
            ("Keywords", keyword.clone()),
            ("AssetTypes", "FxSpot".to_string()),
            ("IncludeNonTradable", "false".to_string()),
        ];
        let payload =
            crate::markov_method::saxo_get_json(state, session, "/ref/v1/instruments", &query)
                .await
                .with_context(|| format!("resolving Saxo FX spot pair {keyword}"))?;
        let candidates = payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(candidate) = select_fx_candidate(&candidates, &base, &quote) {
            let Some(uic) = candidate.get("Identifier").and_then(JsonValue::as_i64) else {
                continue;
            };
            return Ok(Some(SaxoFxPair {
                currency: code.clone(),
                pair_symbol: candidate
                    .get("Symbol")
                    .and_then(JsonValue::as_str)
                    .unwrap_or(&keyword)
                    .to_string(),
                uic,
                inverted,
            }));
        }
    }
    Ok(None)
}

fn select_fx_candidate<'a>(
    candidates: &'a [JsonValue],
    base: &str,
    quote: &str,
) -> Option<&'a JsonValue> {
    let exact = format!("{base}{quote}");
    candidates
        .iter()
        .find(|candidate| {
            let symbol = candidate
                .get("Symbol")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            normalize_pair_text(symbol).contains(&exact)
        })
        .or_else(|| {
            if candidates.len() != 1 {
                return None;
            }
            candidates.first().filter(|candidate| {
                candidate
                    .get("AssetType")
                    .and_then(JsonValue::as_str)
                    .map(|value| value.eq_ignore_ascii_case("FxSpot"))
                    .unwrap_or(false)
            })
        })
}

fn parse_saxo_fx_rate_to_dkk(item: &JsonValue, inverted: bool) -> Option<f64> {
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
    let price = last_traded.or(mid).or(bid_ask_mid)?;
    if inverted {
        if price <= 0.0 {
            None
        } else {
            Some(1.0 / price)
        }
    } else {
        Some(price)
    }
}

async fn upsert_fx_rate(
    pool: &AnyPool,
    currency: &str,
    rate_to_dkk: f64,
    source: &str,
    observed_at: &str,
    expires_at: &str,
    raw_payload_json: &str,
) -> Result<()> {
    let updated = sqlx::query(&format!(
        "UPDATE currency_fx_rates
         SET rate_to_dkk = {rate_to_dkk},
             source = '{}',
             observed_at = '{}',
             expires_at = '{}',
             raw_payload_json = '{}'
         WHERE currency_code = '{}' AND base_currency = 'DKK'",
        sql_escape(source),
        sql_escape(observed_at),
        sql_escape(expires_at),
        sql_escape(raw_payload_json),
        sql_escape(currency),
    ))
    .execute(pool)
    .await
    .context("updating FX cache row")?;
    if updated.rows_affected() == 0 {
        sqlx::query(&format!(
            "INSERT INTO currency_fx_rates (
                currency_code, base_currency, rate_to_dkk, source,
                observed_at, expires_at, raw_payload_json
            ) VALUES (
                '{}', 'DKK', {rate_to_dkk}, '{}', '{}', '{}', '{}'
            )",
            sql_escape(currency),
            sql_escape(source),
            sql_escape(observed_at),
            sql_escape(expires_at),
            sql_escape(raw_payload_json),
        ))
        .execute(pool)
        .await
        .context("inserting FX cache row")?;
    }
    Ok(())
}

/// How a realised DKK gain splits between instrument price and currency.
///
/// The two components sum to the realised gain exactly; see
/// [`split_realised_gain`] for the derivation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RealisedGainSplit {
    /// Gain from the instrument's own price move, valued at the sale rate.
    pub(crate) price_gain_dkk: f64,
    /// Gain from the currency moving between purchase and sale.
    pub(crate) fx_gain_dkk: f64,
    /// Gain in the instrument's own currency. This is `net - cost` in local
    /// terms, **not** the DKK gain divided by a rate.
    pub(crate) realised_gain_local: f64,
}

/// Split a realised DKK gain into its price and currency components.
///
/// With `c` the local cost basis, `n` the local net proceeds, `r0` the rate at
/// purchase and `r1` the rate at sale:
///
/// ```text
/// price = (n - c) * r1        currency held constant at the sale rate
/// fx    = c * (r1 - r0)       the cost basis revalued across the move
/// price + fx = n*r1 - c*r0 = realised_gain_dkk
/// ```
///
/// so the decomposition is exact rather than an approximation, and the caller
/// can assert the identity. Attributing the price move at `r1` (rather than
/// `r0`) is the convention that leaves no cross-term stranded.
///
/// This replaces a `fx_gain_dkk` column that was written as a hardcoded `0`
/// literal while `price_gain_dkk` received the whole realised gain, so every
/// sale reported 100% price and 0% currency regardless of what the currency
/// did. See `wiki/urgent-todo.md` U10.
///
/// **Fails to today's behaviour, not to nonsense.** Without a usable cost rate
/// — a missing or zero local cost basis, a non-finite input — there is no basis
/// on which to attribute anything to currency, so the whole gain is reported as
/// price and `fx_gain_dkk` is zero. That is exactly what the column said
/// before, so a degraded row is indistinguishable from the old behaviour rather
/// than being confidently wrong.
pub(crate) fn split_realised_gain(
    realised_gain_dkk: f64,
    cost_basis_sold_dkk: f64,
    cost_basis_sold_local: f64,
    sale_rate_to_dkk: f64,
) -> RealisedGainSplit {
    let degraded = RealisedGainSplit {
        price_gain_dkk: realised_gain_dkk,
        fx_gain_dkk: 0.0,
        realised_gain_local: 0.0,
    };
    if !realised_gain_dkk.is_finite()
        || !cost_basis_sold_dkk.is_finite()
        || !cost_basis_sold_local.is_finite()
        || !sale_rate_to_dkk.is_finite()
        || sale_rate_to_dkk.abs() <= f64::EPSILON
        || cost_basis_sold_local.abs() <= f64::EPSILON
    {
        return degraded;
    }
    let cost_rate = cost_basis_sold_dkk / cost_basis_sold_local;
    if !cost_rate.is_finite() {
        return degraded;
    }
    let net_amount_dkk = realised_gain_dkk + cost_basis_sold_dkk;
    let realised_gain_local = net_amount_dkk / sale_rate_to_dkk - cost_basis_sold_local;
    let price_gain_dkk = realised_gain_local * sale_rate_to_dkk;
    let fx_gain_dkk = cost_basis_sold_local * (sale_rate_to_dkk - cost_rate);
    if !price_gain_dkk.is_finite() || !fx_gain_dkk.is_finite() {
        return degraded;
    }
    RealisedGainSplit {
        price_gain_dkk,
        fx_gain_dkk,
        realised_gain_local,
    }
}

fn normalize_currency(currency: &str) -> String {
    currency.trim().to_uppercase()
}

fn normalize_pair_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

fn parse_ecb_daily_rates(xml: &str) -> Result<HashMap<String, f64>> {
    let mut rates = HashMap::from([("EUR".to_string(), 1.0)]);
    for cube in xml.match_indices("<Cube ") {
        let rest = &xml[cube.0..];
        let Some(end) = rest.find('>') else {
            continue;
        };
        let tag = &rest[..end];
        let Some(currency) = attr_value(tag, "currency") else {
            continue;
        };
        let Some(rate_text) = attr_value(tag, "rate") else {
            continue;
        };
        let rate = rate_text
            .parse::<f64>()
            .with_context(|| format!("parsing ECB FX rate for {currency}"))?;
        rates.insert(currency, rate);
    }
    if rates.len() <= 1 {
        bail!("ECB FX response did not contain currency rates");
    }
    Ok(rates)
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let single = format!("{name}='");
    let double = format!("{name}=\"");
    let (needle, quote) = if tag.contains(&single) {
        (single, '\'')
    } else {
        (double, '"')
    };
    let start = tag.find(&needle)? + needle.len();
    let value = &tag[start..];
    let end = value.find(quote)?;
    Some(value[..end].to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the decomposition: the parts must reconstruct the
    /// total, or the split is a second opinion rather than an attribution.
    fn assert_split_is_exact(realised: f64, cost_dkk: f64, cost_local: f64, rate: f64) {
        let split = split_realised_gain(realised, cost_dkk, cost_local, rate);
        let recombined = split.price_gain_dkk + split.fx_gain_dkk;
        assert!(
            (recombined - realised).abs() < 1e-6,
            "price {} + fx {} = {} but realised is {}",
            split.price_gain_dkk,
            split.fx_gain_dkk,
            recombined,
            realised
        );
    }

    /// Reproduces the live JNJ sale of 2026-07-30, which the ledger recorded as
    /// −543 DKK entirely of price and zero of currency. USD/DKK moved 6.5705 →
    /// 6.4986 between purchase and sale, so a real part of that loss was the
    /// dollar rather than the stock.
    #[test]
    fn a_currency_move_is_attributed_to_currency_rather_than_price() {
        let cost_local = 1_089.75_f64;
        let cost_dkk = cost_local * 6.5705;
        let sale_rate = 6.4986;
        let realised = -543.0;

        let split = split_realised_gain(realised, cost_dkk, cost_local, sale_rate);

        assert!(
            split.fx_gain_dkk < 0.0,
            "a falling dollar must show as a currency loss, got {}",
            split.fx_gain_dkk
        );
        assert!(
            (split.fx_gain_dkk - cost_local * (sale_rate - 6.5705)).abs() < 1e-6,
            "currency component must be the cost basis revalued across the move"
        );
        assert_split_is_exact(realised, cost_dkk, cost_local, sale_rate);
    }

    /// A DKK instrument has no currency exposure, so the entire gain is price.
    /// This is the case the old hardcoded zero happened to get right.
    #[test]
    fn a_home_currency_sale_has_no_currency_component() {
        let split = split_realised_gain(-1_489.0, 10_042.0, 10_042.0, 1.0);
        assert_eq!(split.fx_gain_dkk, 0.0);
        assert!((split.price_gain_dkk - -1_489.0).abs() < 1e-9);
        assert_split_is_exact(-1_489.0, 10_042.0, 10_042.0, 1.0);
    }

    /// A flat instrument in a moving currency is pure FX — the case that is
    /// invisible while the column is a literal zero.
    #[test]
    fn a_flat_instrument_in_a_moving_currency_is_all_currency() {
        let cost_local = 1_000.0;
        let cost_rate = 7.0;
        let sale_rate = 6.5;
        // Sold for exactly what it cost in local terms.
        let realised = cost_local * sale_rate - cost_local * cost_rate;

        let split = split_realised_gain(realised, cost_local * cost_rate, cost_local, sale_rate);

        assert!(
            split.price_gain_dkk.abs() < 1e-6,
            "flat price must contribute nothing, got {}",
            split.price_gain_dkk
        );
        assert!((split.fx_gain_dkk - -500.0).abs() < 1e-6);
        assert!(split.realised_gain_local.abs() < 1e-6);
    }

    /// Without a local cost basis there is nothing to attribute currency
    /// against, so the function must degrade to the previous behaviour rather
    /// than inventing a component.
    #[test]
    fn a_missing_cost_basis_degrades_to_price_only_instead_of_guessing() {
        for (cost_dkk, cost_local, rate) in [
            (0.0, 0.0, 6.5),
            (100.0, 0.0, 6.5),
            (100.0, 15.0, 0.0),
            (f64::NAN, 15.0, 6.5),
            (100.0, f64::INFINITY, 6.5),
        ] {
            let split = split_realised_gain(-250.0, cost_dkk, cost_local, rate);
            assert_eq!(
                split.fx_gain_dkk, 0.0,
                "degraded rows must not claim a currency component"
            );
            assert_eq!(split.price_gain_dkk, -250.0);
        }
    }

    /// A gain and a loss must both decompose exactly, in either FX direction.
    #[test]
    fn the_decomposition_is_exact_across_directions() {
        assert_split_is_exact(4_425.0, 12_507.0, 1_913.42, 6.5531);
        assert_split_is_exact(-4_469.0, 40_206.0, 6_151.0, 6.5662);
        assert_split_is_exact(2_356.0, 3_127.0, 445.31, 7.0215);
        assert_split_is_exact(-891.0, 19_601.0, 2_621.82, 7.4756);
    }

    #[test]
    fn parses_ecb_daily_rates() {
        let xml = r#"
            <Cube time='2026-07-07'>
              <Cube currency='USD' rate='1.1708'/>
              <Cube currency='DKK' rate='7.4604'/>
              <Cube currency='GBP' rate='0.8622'/>
            </Cube>
        "#;
        let rates = parse_ecb_daily_rates(xml).expect("rates parse");
        assert_eq!(rates.get("EUR"), Some(&1.0));
        assert_eq!(rates.get("USD"), Some(&1.1708));
        assert_eq!(rates.get("DKK"), Some(&7.4604));
        assert_eq!(rates.get("GBP"), Some(&0.8622));
    }

    #[test]
    fn static_fallback_covers_common_currencies() {
        assert_eq!(static_fx_rate_to_dkk("dkk"), 1.0);
        assert!(static_fx_rate_to_dkk("USD") > 1.0);
        assert!(static_fx_rate_to_dkk("EUR") > 1.0);
    }

    #[test]
    fn parses_saxo_fx_quote_midpoint_and_inversion() {
        let quote = json!({
            "Quote": {"Bid": 6.8, "Ask": 6.9},
            "PriceInfoDetails": {}
        });
        assert_eq!(parse_saxo_fx_rate_to_dkk(&quote, false), Some(6.85));
        let inverted = parse_saxo_fx_rate_to_dkk(&quote, true).expect("inverse rate");
        assert!((inverted - (1.0 / 6.85)).abs() < 1e-9);
    }

    #[test]
    fn selects_fx_candidate_by_normalized_pair_symbol() {
        let candidates = vec![json!({
            "Identifier": 123,
            "AssetType": "FxSpot",
            "Symbol": "USD/DKK"
        })];
        let selected = select_fx_candidate(&candidates, "USD", "DKK").expect("candidate");
        assert_eq!(
            selected.get("Identifier").and_then(JsonValue::as_i64),
            Some(123)
        );
    }
}
