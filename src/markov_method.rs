use std::collections::{HashMap, HashSet};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{clamp_limit, row_to_json, sql_escape},
    state::AppState,
};

const STATES: [Regime; 3] = [Regime::Bull, Regime::Sideways, Regime::Bear];
const DEFAULT_DAILY_TIME: &str = "23:30";
const TRADABLE_ASSET_TYPES: &str = "Stock,Etf,Etn,Etc";
const SAXO_MARKOV_REQUEST_DELAY_MS: u64 = 500;
const SAXO_MARKOV_MAX_ATTEMPTS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Regime {
    Bull = 0,
    Sideways = 1,
    Bear = 2,
}

impl Regime {
    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Bull => "Bull",
            Self::Sideways => "Sideways",
            Self::Bear => "Bear",
        }
    }
}

#[derive(Clone, Debug)]
struct MarkovConfig {
    enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    run_weekdays_only: bool,
    window_days: usize,
    threshold: f64,
    horizon_minutes: i64,
    sample_count: usize,
    min_labeled_days: usize,
    signal_horizon_days: usize,
    forecast_steps: Vec<usize>,
    max_symbols: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MarkovAsset {
    pub(crate) symbol: String,
    pub(crate) instrument_name: String,
    pub(crate) source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SaxoInstrument {
    pub(crate) uic: i64,
    pub(crate) asset_type: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug)]
struct ChartBar {
    time: String,
    close: f64,
}

#[derive(Clone, Debug)]
struct LabelPoint {
    time: String,
    close: f64,
    rolling_return: f64,
    regime: Regime,
}

#[derive(Clone, Debug)]
struct MarkovAnalysis {
    current_state: Regime,
    current_close: f64,
    rolling_return: f64,
    labels: Vec<LabelPoint>,
    counts: [[usize; 3]; 3],
    matrix: [[f64; 3]; 3],
    forecasts: Vec<(usize, [f64; 3])>,
    signal_distribution: [f64; 3],
    stationary: [f64; 3],
    signed_signal: f64,
    direction: String,
    conviction: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct SymbolParts {
    base: String,
    exchange: String,
}

pub async fn run_markov_method_cycle(state: &AppState) -> Result<JsonValue> {
    let config = markov_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
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
    if markov_run_exists(state, run_date).await? {
        return Ok(json!({
            "status": "skipped",
            "reason": "already_ran",
            "run_date": run_date.to_string()
        }));
    }

    run_markov_method_for_date(state, &config, run_date).await
}

async fn run_markov_method_for_date(
    state: &AppState,
    config: &MarkovConfig,
    run_date: NaiveDate,
) -> Result<JsonValue> {
    let session = state
        .ensure_saxo_session_json("markov_method")
        .await
        .context("loading Saxo session for Markov method run")?;
    let assets = markov_assets(state, config.max_symbols).await?;
    let run_id = format!("markov-{}", Utc::now().timestamp_micros());
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut rows = Vec::new();
    let mut success_count = 0usize;
    let mut error_count = 0usize;

    for asset in &assets {
        let result = analyze_asset(state, &session, config, asset).await;
        let row = match result {
            Ok((instrument, bars, analysis)) => {
                success_count += 1;
                signal_row_json(
                    &run_id,
                    &created_at,
                    run_date,
                    config,
                    asset,
                    Some(&instrument),
                    Some(&bars),
                    Some(&analysis),
                    None,
                )
            }
            Err(err) => {
                error_count += 1;
                warn!(
                    symbol = %asset.symbol,
                    "Markov method asset analysis failed: {err:#}"
                );
                signal_row_json(
                    &run_id,
                    &created_at,
                    run_date,
                    config,
                    asset,
                    None,
                    None,
                    None,
                    Some(&format!("{err:#}")),
                )
            }
        };
        insert_markov_signal(state, &row).await?;
        rows.push(row);
    }

    let status = if success_count > 0 {
        "completed"
    } else if assets.is_empty() {
        "empty"
    } else {
        "error"
    };
    let summary = json!({
        "status": status,
        "run_id": run_id,
        "run_date": run_date.to_string(),
        "asset_count": assets.len(),
        "success_count": success_count,
        "error_count": error_count,
        "config": markov_config_json(config),
        "signals": rows.iter().filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok")).take(20).cloned().collect::<Vec<_>>(),
    });
    insert_markov_run(
        state,
        &run_id,
        &created_at,
        run_date,
        status,
        assets.len(),
        success_count,
        error_count,
        config,
        &summary,
    )
    .await?;
    info!(
        run_id,
        run_date = %run_date,
        success_count,
        error_count,
        "Markov method daily run completed"
    );
    Ok(summary)
}

async fn analyze_asset(
    state: &AppState,
    session: &JsonValue,
    config: &MarkovConfig,
    asset: &MarkovAsset,
) -> Result<(SaxoInstrument, Vec<ChartBar>, MarkovAnalysis)> {
    let instrument = resolve_instrument(state, session, &asset.symbol)
        .await
        .with_context(|| format!("resolving Saxo instrument for {}", asset.symbol))?;
    let bars = fetch_chart_bars(state, session, &instrument, config)
        .await
        .with_context(|| format!("fetching Saxo chart history for {}", asset.symbol))?;
    let analysis = analyze_bars(&bars, config)
        .with_context(|| format!("running Markov model for {}", asset.symbol))?;
    Ok((instrument, bars, analysis))
}

fn analyze_bars(bars: &[ChartBar], config: &MarkovConfig) -> Result<MarkovAnalysis> {
    let labels = label_regimes(bars, config.window_days, config.threshold);
    if labels.len() < config.min_labeled_days {
        bail!(
            "not enough labeled daily bars: {} available, {} required",
            labels.len(),
            config.min_labeled_days
        );
    }
    let counts = transition_counts(&labels);
    let matrix = transition_matrix(counts);
    let current = labels
        .last()
        .ok_or_else(|| anyhow!("no current regime label was produced"))?;
    let stationary = stationary_distribution(matrix);
    let forecasts = config
        .forecast_steps
        .iter()
        .copied()
        .map(|step| (step, forecast_distribution(matrix, current.regime, step)))
        .collect::<Vec<_>>();
    let signal_distribution =
        forecast_distribution(matrix, current.regime, config.signal_horizon_days);
    let signed_signal =
        signal_distribution[Regime::Bull.index()] - signal_distribution[Regime::Bear.index()];
    let conviction = signed_signal.abs();
    let direction = if signed_signal > 1e-9 {
        "long"
    } else if signed_signal < -1e-9 {
        "short"
    } else {
        "flat"
    }
    .to_string();

    Ok(MarkovAnalysis {
        current_state: current.regime,
        current_close: current.close,
        rolling_return: current.rolling_return,
        labels,
        counts,
        matrix,
        forecasts,
        signal_distribution,
        stationary,
        signed_signal,
        direction,
        conviction,
    })
}

fn label_regimes(bars: &[ChartBar], window_days: usize, threshold: f64) -> Vec<LabelPoint> {
    if window_days == 0 || bars.len() <= window_days {
        return Vec::new();
    }
    let mut labels = Vec::new();
    for index in window_days..bars.len() {
        let current = &bars[index];
        let prior = &bars[index - window_days];
        if current.close <= 0.0 || prior.close <= 0.0 {
            continue;
        }
        let rolling_return = current.close / prior.close - 1.0;
        let regime = if rolling_return >= threshold {
            Regime::Bull
        } else if rolling_return <= -threshold {
            Regime::Bear
        } else {
            Regime::Sideways
        };
        labels.push(LabelPoint {
            time: current.time.clone(),
            close: current.close,
            rolling_return,
            regime,
        });
    }
    labels
}

fn transition_counts(labels: &[LabelPoint]) -> [[usize; 3]; 3] {
    let mut counts = [[0usize; 3]; 3];
    for pair in labels.windows(2) {
        let from = pair[0].regime.index();
        let to = pair[1].regime.index();
        counts[from][to] += 1;
    }
    counts
}

fn transition_matrix(counts: [[usize; 3]; 3]) -> [[f64; 3]; 3] {
    let mut matrix = [[0.0; 3]; 3];
    for row in 0..3 {
        let total = counts[row].iter().sum::<usize>();
        if total == 0 {
            matrix[row][row] = 1.0;
            continue;
        }
        for col in 0..3 {
            matrix[row][col] = counts[row][col] as f64 / total as f64;
        }
    }
    matrix
}

fn forecast_distribution(matrix: [[f64; 3]; 3], current_state: Regime, steps: usize) -> [f64; 3] {
    let powered = matrix_power(matrix, steps.max(1));
    powered[current_state.index()]
}

fn matrix_power(mut matrix: [[f64; 3]; 3], mut exponent: usize) -> [[f64; 3]; 3] {
    let mut result = identity_matrix();
    while exponent > 0 {
        if exponent % 2 == 1 {
            result = matrix_multiply(result, matrix);
        }
        matrix = matrix_multiply(matrix, matrix);
        exponent /= 2;
    }
    result
}

fn matrix_multiply(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = (0..3).map(|k| left[row][k] * right[k][col]).sum();
        }
    }
    out
}

fn identity_matrix() -> [[f64; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn stationary_distribution(matrix: [[f64; 3]; 3]) -> [f64; 3] {
    let mut distribution = [1.0 / 3.0; 3];
    for _ in 0..2000 {
        let next = [
            distribution[0] * matrix[0][0]
                + distribution[1] * matrix[1][0]
                + distribution[2] * matrix[2][0],
            distribution[0] * matrix[0][1]
                + distribution[1] * matrix[1][1]
                + distribution[2] * matrix[2][1],
            distribution[0] * matrix[0][2]
                + distribution[1] * matrix[1][2]
                + distribution[2] * matrix[2][2],
        ];
        let delta = (0..3)
            .map(|idx| (next[idx] - distribution[idx]).abs())
            .fold(0.0, f64::max);
        distribution = next;
        if delta < 1e-12 {
            break;
        }
    }
    normalize_distribution(distribution)
}

fn normalize_distribution(mut distribution: [f64; 3]) -> [f64; 3] {
    for value in &mut distribution {
        if !value.is_finite() || *value < 0.0 {
            *value = 0.0;
        }
    }
    let total = distribution.iter().sum::<f64>();
    if total <= 0.0 {
        [1.0 / 3.0; 3]
    } else {
        [
            distribution[0] / total,
            distribution[1] / total,
            distribution[2] / total,
        ]
    }
}

pub(crate) async fn markov_assets(
    state: &AppState,
    max_symbols: usize,
) -> Result<Vec<MarkovAsset>> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();
    for row in state.position_items(250).await.unwrap_or_default() {
        push_asset(&mut assets, &mut seen, &row, "portfolio");
    }
    let watchlists = state.watchlists_payload().await.unwrap_or_else(|err| {
        warn!("Markov watchlist universe degraded: {err:#}");
        json!({"categories": []})
    });
    for category in watchlists
        .get("categories")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
    {
        if category.get("key").and_then(JsonValue::as_str) != Some("all") {
            continue;
        }
        for row in category
            .get("items")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
        {
            push_asset(&mut assets, &mut seen, &row, "watchlist");
        }
    }
    if max_symbols > 0 && assets.len() > max_symbols {
        assets.truncate(max_symbols);
    }
    Ok(assets)
}

fn push_asset(
    assets: &mut Vec<MarkovAsset>,
    seen: &mut HashSet<String>,
    row: &JsonValue,
    source: &str,
) {
    let symbol = text(row, "symbol").trim().to_string();
    if symbol.is_empty() || !seen.insert(symbol.clone()) {
        return;
    }
    let instrument_name = text(row, "instrument_name")
        .if_empty_then(|| Some(symbol.clone()))
        .unwrap_or_else(|| symbol.clone());
    assets.push(MarkovAsset {
        symbol,
        instrument_name,
        source: source.to_string(),
    });
}

pub(crate) async fn resolve_instrument(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
) -> Result<SaxoInstrument> {
    if let Some(instrument) = stored_instrument(state, symbol).await? {
        return Ok(instrument);
    }
    lookup_instrument(state, session, symbol).await
}

async fn stored_instrument(state: &AppState, symbol: &str) -> Result<Option<SaxoInstrument>> {
    let escaped = sql_escape(symbol);
    let row = sqlx::query(&format!(
        "SELECT symbol, instrument_name, uic, asset_type
         FROM broker_position_snapshots
         WHERE symbol = '{}' AND uic IS NOT NULL AND asset_type IS NOT NULL
         LIMIT 1",
        escaped
    ))
    .fetch_optional(&state.pool)
    .await?;
    if let Some(row) = row {
        let value = row_to_json(&row);
        if let Some(instrument) = instrument_from_row(&value) {
            return Ok(Some(instrument));
        }
    }
    let row = sqlx::query(&format!(
        "SELECT symbol, uic, asset_type
         FROM broker_instrument_exposures
         WHERE symbol = '{}' AND uic IS NOT NULL AND asset_type IS NOT NULL
         LIMIT 1",
        escaped
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .as_ref()
        .map(row_to_json)
        .and_then(|value| instrument_from_row(&value)))
}

fn instrument_from_row(row: &JsonValue) -> Option<SaxoInstrument> {
    let uic = row
        .get("uic")
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))?;
    let asset_type = text(row, "asset_type");
    if asset_type.is_empty() {
        return None;
    }
    let symbol = text(row, "symbol");
    Some(SaxoInstrument {
        uic,
        asset_type,
        description: text(row, "instrument_name")
            .if_empty_then(|| Some(symbol))
            .unwrap_or_default(),
    })
}

async fn lookup_instrument(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
) -> Result<SaxoInstrument> {
    let parts = symbol_parts(symbol);
    let requested_symbol = if parts.exchange.is_empty() {
        parts.base.clone()
    } else {
        format!("{}:{}", parts.base, parts.exchange)
    };
    let mut attempts = vec![
        (
            "symbol".to_string(),
            vec![("Keywords", requested_symbol.clone())],
            true,
        ),
        (
            "exchange".to_string(),
            vec![
                ("Keywords", parts.base.clone()),
                (
                    "ExchangeId",
                    exchange_id_for_suffix(&parts.exchange).to_string(),
                ),
            ],
            true,
        ),
        (
            "base".to_string(),
            vec![("Keywords", parts.base.clone())],
            true,
        ),
    ];
    if let Some(isin) = latest_position_isin(state, symbol).await? {
        attempts.insert(2, ("isin".to_string(), vec![("Keywords", isin)], false));
    }

    for (method, params, require_symbol_match) in attempts {
        let mut query = vec![
            ("$top", "50".to_string()),
            ("AccountKey", account_key(state, session)?),
            ("AssetTypes", TRADABLE_ASSET_TYPES.to_string()),
            ("IncludeNonTradable", "false".to_string()),
        ];
        query.extend(params);
        let payload = saxo_get_json(state, session, "/ref/v1/instruments", &query).await?;
        let candidates = payload
            .get("Data")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let selected = if require_symbol_match {
            candidates
                .iter()
                .filter(|candidate| {
                    candidate_matches_requested(candidate, &requested_symbol, &parts)
                })
                .max_by_key(|candidate| candidate_score(candidate, &requested_symbol, &parts))
        } else {
            candidates
                .iter()
                .max_by_key(|candidate| candidate_score(candidate, &requested_symbol, &parts))
        };
        if let Some(selected) = selected {
            let instrument = SaxoInstrument {
                uic: selected
                    .get("Identifier")
                    .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                    .ok_or_else(|| {
                        anyhow!("Saxo instrument match for {symbol} had no Identifier")
                    })?,
                asset_type: text(selected, "AssetType")
                    .if_empty_then(|| Some("Stock".to_string()))
                    .unwrap(),
                description: text(selected, "Description")
                    .if_empty_then(|| Some(symbol.to_string()))
                    .unwrap_or_else(|| symbol.to_string()),
            };
            info!(
                symbol,
                method,
                uic = instrument.uic,
                "Saxo instrument resolved for Markov method"
            );
            return Ok(instrument);
        }
    }
    Err(anyhow!(
        "No tradable Saxo instrument match found for {symbol}"
    ))
}

async fn latest_position_isin(state: &AppState, symbol: &str) -> Result<Option<String>> {
    let row = sqlx::query(&format!(
        "SELECT isin
         FROM position_snapshots
         WHERE symbol = '{}' AND excluded = 0
         ORDER BY imported_at DESC, id DESC
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<String, _>("isin").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

async fn fetch_chart_bars(
    state: &AppState,
    session: &JsonValue,
    instrument: &SaxoInstrument,
    config: &MarkovConfig,
) -> Result<Vec<ChartBar>> {
    let query = vec![
        ("AccountKey", account_key(state, session)?),
        ("AssetType", instrument.asset_type.clone()),
        ("Uic", instrument.uic.to_string()),
        ("Horizon", config.horizon_minutes.to_string()),
        ("Count", config.sample_count.to_string()),
        ("FieldGroups", "ChartInfo,Data,DisplayAndFormat".to_string()),
    ];
    let payload = saxo_get_json(state, session, "/chart/v3/charts", &query).await?;
    let mut bars = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let close = number_from_keys(
                &item,
                &[
                    "Close",
                    "ClosePrice",
                    "close",
                    "closePrice",
                    "LastTraded",
                    "Price",
                ],
            )?;
            if close <= 0.0 {
                return None;
            }
            let time = text(&item, "Time")
                .if_empty_then(|| Some(text(&item, "time")))
                .unwrap_or_default();
            Some(ChartBar { time, close })
        })
        .collect::<Vec<_>>();
    bars.sort_by(|left, right| left.time.cmp(&right.time));
    bars.dedup_by(|left, right| left.time == right.time);
    Ok(bars)
}

pub(crate) async fn saxo_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = session_text(session, "access_token")
        .ok_or_else(|| anyhow!("Saxo access token is missing from session"))?;
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(30))
        .build()?;
    let url = format!("{}{}", openapi_base_url(state, session)?, path);
    let mut last_error = String::new();
    for attempt in 1..=SAXO_MARKOV_MAX_ATTEMPTS {
        sleep(StdDuration::from_millis(SAXO_MARKOV_REQUEST_DELAY_MS)).await;
        let response = client
            .get(&url)
            .bearer_auth(&access_token)
            .header(header::ACCEPT, "application/json")
            .query(query)
            .send()
            .await?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();
        let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
        if status.is_success() {
            return Ok(payload);
        }
        let snippet = body.chars().take(300).collect::<String>();
        last_error = format!("Saxo GET failed: HTTP {}: {}", status.as_u16(), snippet);
        if status.as_u16() == 429 && attempt < SAXO_MARKOV_MAX_ATTEMPTS {
            let delay_secs = retry_after.unwrap_or_else(|| 2_u64.pow(attempt as u32).min(30));
            warn!(
                path,
                attempt, delay_secs, "Saxo Markov GET rate-limited; backing off before retry"
            );
            sleep(StdDuration::from_secs(delay_secs)).await;
            continue;
        }
        bail!("{last_error}");
    }
    bail!("{last_error}")
}

fn signal_row_json(
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    config: &MarkovConfig,
    asset: &MarkovAsset,
    instrument: Option<&SaxoInstrument>,
    bars: Option<&[ChartBar]>,
    analysis: Option<&MarkovAnalysis>,
    error_text: Option<&str>,
) -> JsonValue {
    let exchange = symbol_parts(&asset.symbol).exchange.to_uppercase();
    let status = if error_text.is_some() { "error" } else { "ok" };
    let raw_payload_json = match analysis {
        Some(analysis) => json!({
            "recent_labels": recent_labels_json(&analysis.labels, 60),
            "label_counts": label_counts_json(&analysis.labels),
            "source": "saxo_chart_v3",
            "method": "observable_markov_rolling_return"
        }),
        None => json!({}),
    };
    json!({
        "id": format!("{}-{}", run_id, asset.symbol.replace(':', "-").to_lowercase()),
        "run_id": run_id,
        "created_at": created_at,
        "run_date": run_date.to_string(),
        "status": status,
        "symbol": asset.symbol,
        "instrument_name": instrument.map(|value| value.description.as_str()).filter(|value| !value.is_empty()).unwrap_or(&asset.instrument_name),
        "exchange": exchange,
        "source": asset.source,
        "uic": instrument.map(|value| value.uic),
        "asset_type": instrument.map(|value| value.asset_type.clone()),
        "window_days": config.window_days,
        "threshold": config.threshold,
        "horizon_minutes": config.horizon_minutes,
        "sample_count": bars.map(|items| items.len()).unwrap_or(0),
        "min_labeled_days": config.min_labeled_days,
        "signal_horizon_days": config.signal_horizon_days,
        "current_state": analysis.map(|value| value.current_state.label()),
        "current_close": analysis.map(|value| value.current_close),
        "rolling_return": analysis.map(|value| value.rolling_return),
        "transition_counts_json": analysis.map(|value| matrix_usize_json(value.counts)).unwrap_or(JsonValue::Null),
        "transition_matrix_json": analysis.map(|value| matrix_f64_json(value.matrix)).unwrap_or(JsonValue::Null),
        "forecasts_json": analysis.map(|value| forecasts_json(&value.forecasts)).unwrap_or(JsonValue::Null),
        "stationary_json": analysis.map(|value| distribution_json(value.stationary)).unwrap_or(JsonValue::Null),
        "bull_prob": analysis.map(|value| value.signal_distribution[Regime::Bull.index()]),
        "sideways_prob": analysis.map(|value| value.signal_distribution[Regime::Sideways.index()]),
        "bear_prob": analysis.map(|value| value.signal_distribution[Regime::Bear.index()]),
        "signed_signal": analysis.map(|value| value.signed_signal),
        "direction": analysis.map(|value| value.direction.as_str()),
        "conviction": analysis.map(|value| value.conviction),
        "error_text": error_text,
        "raw_payload_json": raw_payload_json
    })
}

async fn insert_markov_signal(state: &AppState, row: &JsonValue) -> Result<()> {
    let sql = format!(
        "INSERT INTO markov_asset_signals (
            id, run_id, created_at, run_date, status, symbol, instrument_name,
            exchange, source, uic, asset_type, window_days, threshold,
            horizon_minutes, sample_count, min_labeled_days, signal_horizon_days,
            current_state, current_close, rolling_return, transition_counts_json,
            transition_matrix_json, forecasts_json, stationary_json, bull_prob,
            sideways_prob, bear_prob, signed_signal, direction, conviction,
            error_text, raw_payload_json
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', '{}', '{}',
            '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {}, {}
        )",
        sql_escape(&text(row, "id")),
        sql_escape(&text(row, "run_id")),
        sql_escape(&text(row, "created_at")),
        sql_escape(&text(row, "run_date")),
        sql_escape(&text(row, "status")),
        sql_escape(&text(row, "symbol")),
        sql_escape(&text(row, "instrument_name")),
        sql_escape(&text(row, "exchange")),
        sql_escape(&text(row, "source")),
        optional_i64_sql(row.get("uic").and_then(JsonValue::as_i64)),
        optional_text_sql(row.get("asset_type").and_then(JsonValue::as_str)),
        row.get("window_days")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("threshold")
            .and_then(JsonValue::as_f64)
            .unwrap_or(0.0),
        row.get("horizon_minutes")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        row.get("sample_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("min_labeled_days")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        row.get("signal_horizon_days")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        optional_text_sql(row.get("current_state").and_then(JsonValue::as_str)),
        optional_f64_sql(row.get("current_close").and_then(JsonValue::as_f64)),
        optional_f64_sql(row.get("rolling_return").and_then(JsonValue::as_f64)),
        optional_json_sql(row.get("transition_counts_json")),
        optional_json_sql(row.get("transition_matrix_json")),
        optional_json_sql(row.get("forecasts_json")),
        optional_json_sql(row.get("stationary_json")),
        optional_f64_sql(row.get("bull_prob").and_then(JsonValue::as_f64)),
        optional_f64_sql(row.get("sideways_prob").and_then(JsonValue::as_f64)),
        optional_f64_sql(row.get("bear_prob").and_then(JsonValue::as_f64)),
        optional_f64_sql(row.get("signed_signal").and_then(JsonValue::as_f64)),
        optional_text_sql(row.get("direction").and_then(JsonValue::as_str)),
        optional_f64_sql(row.get("conviction").and_then(JsonValue::as_f64)),
        optional_text_sql(row.get("error_text").and_then(JsonValue::as_str)),
        optional_json_sql(row.get("raw_payload_json"))
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Markov asset signal")?;
    Ok(())
}

async fn insert_markov_run(
    state: &AppState,
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    status: &str,
    asset_count: usize,
    success_count: usize,
    error_count: usize,
    config: &MarkovConfig,
    summary: &JsonValue,
) -> Result<()> {
    let sql = format!(
        "INSERT INTO markov_signal_runs (
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
        sql_escape(&serde_json::to_string(&markov_config_json(config))?),
        sql_escape(&serde_json::to_string(summary)?)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Markov signal run")?;
    Ok(())
}

async fn markov_run_exists(state: &AppState, run_date: NaiveDate) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM markov_signal_runs WHERE run_date = '{}' LIMIT 1",
        sql_escape(&run_date.to_string())
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

pub async fn latest_markov_signals(state: &AppState, limit: i64) -> Result<Vec<JsonValue>> {
    let sql = format!(
        "SELECT id, run_id, created_at, run_date, status, symbol, instrument_name,
                exchange, source, uic, asset_type, window_days, threshold,
                horizon_minutes, sample_count, min_labeled_days, signal_horizon_days,
                current_state, current_close, rolling_return, transition_counts_json,
                transition_matrix_json, forecasts_json, stationary_json, bull_prob,
                sideways_prob, bear_prob, signed_signal, direction, conviction,
                error_text, raw_payload_json
         FROM markov_asset_signals
         WHERE run_id = (
            SELECT id
            FROM markov_signal_runs
            ORDER BY run_date DESC, created_at DESC
            LIMIT 1
         )
         ORDER BY run_date DESC, created_at DESC, symbol ASC
         LIMIT {}",
        clamp_limit(limit, 1, 500)
    );
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.iter().map(row_to_json).collect())
}

pub async fn latest_markov_run(state: &AppState) -> Result<JsonValue> {
    let row = sqlx::query(
        "SELECT id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json
         FROM markov_signal_runs
         ORDER BY run_date DESC, created_at DESC
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.as_ref().map(row_to_json).unwrap_or(JsonValue::Null))
}

pub async fn compact_markov_context(state: &AppState, limit: i64) -> Result<JsonValue> {
    let rows = latest_markov_signals(state, limit)
        .await?
        .into_iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok"))
        .collect::<Vec<_>>();
    let mut signals = Vec::new();
    for row in rows {
        let symbol_text = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        let currency = symbol_text
            .split_once(':')
            .and_then(|(_, exchange)| crate::saxo_order::currency_for_exchange(exchange));
        let close_dkk = match (
            currency,
            row.get("current_close").and_then(JsonValue::as_f64),
        ) {
            (Some(currency), Some(close)) => {
                let fx_rate =
                    crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
                Some(close * fx_rate)
            }
            _ => None,
        };
        signals.push(json!({
            "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
            "run_date": row.get("run_date").cloned().unwrap_or(JsonValue::Null),
            "state": row.get("current_state").cloned().unwrap_or(JsonValue::Null),
            "close": row.get("current_close").cloned().unwrap_or(JsonValue::Null),
            "currency": currency,
            "close_dkk": close_dkk,
            "horizon_days": row.get("signal_horizon_days").cloned().unwrap_or(JsonValue::Null),
            "bull_prob": row.get("bull_prob").cloned().unwrap_or(JsonValue::Null),
            "bear_prob": row.get("bear_prob").cloned().unwrap_or(JsonValue::Null),
            "sideways_prob": row.get("sideways_prob").cloned().unwrap_or(JsonValue::Null),
            "signed_signal": row.get("signed_signal").cloned().unwrap_or(JsonValue::Null),
            "direction": row.get("direction").cloned().unwrap_or(JsonValue::Null),
            "conviction": row.get("conviction").cloned().unwrap_or(JsonValue::Null)
        }));
    }
    Ok(json!({
        "latest_run": latest_markov_run(state).await.unwrap_or(JsonValue::Null),
        "signals": signals
    }))
}

pub fn markov_config_json_for_state(state: &AppState) -> JsonValue {
    markov_config_json(&markov_config(state))
}

fn markov_config(state: &AppState) -> MarkovConfig {
    let timezone = yaml_string(&state.config, &["strategy", "markov", "timezone"])
        .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let daily_time = yaml_string(&state.config, &["strategy", "markov", "daily_time"])
        .as_deref()
        .and_then(parse_hh_mm)
        .unwrap_or_else(|| parse_hh_mm(DEFAULT_DAILY_TIME).expect("default Markov time is valid"));
    let forecast_steps = yaml_at(&state.config, &["strategy", "markov", "forecast_steps"])
        .and_then(forecast_steps_from_yaml)
        .unwrap_or_else(|| vec![1, 2, 3, 5, 10]);
    let window_days = yaml_i64(&state.config, &["strategy", "markov", "window_days"])
        .unwrap_or(20)
        .max(1) as usize;
    let min_labeled_days = yaml_i64(&state.config, &["strategy", "markov", "min_labeled_days"])
        .unwrap_or(60)
        .max(5) as usize;
    MarkovConfig {
        enabled: yaml_bool(&state.config, &["strategy", "markov", "enabled"]).unwrap_or(true),
        timezone,
        daily_time,
        run_weekdays_only: yaml_bool(&state.config, &["strategy", "markov", "run_weekdays_only"])
            .unwrap_or(true),
        window_days,
        threshold: yaml_f64(&state.config, &["strategy", "markov", "threshold"])
            .unwrap_or(0.05)
            .abs()
            .max(0.0001),
        horizon_minutes: yaml_i64(&state.config, &["strategy", "markov", "horizon_minutes"])
            .unwrap_or(1440)
            .max(1),
        sample_count: yaml_i64(&state.config, &["strategy", "markov", "sample_count"])
            .unwrap_or(520)
            .max((window_days + min_labeled_days + 2) as i64) as usize,
        min_labeled_days,
        signal_horizon_days: yaml_i64(
            &state.config,
            &["strategy", "markov", "signal_horizon_days"],
        )
        .unwrap_or(5)
        .max(1) as usize,
        forecast_steps,
        max_symbols: yaml_i64(&state.config, &["strategy", "markov", "max_symbols"])
            .unwrap_or(0)
            .max(0) as usize,
    }
}

fn forecast_steps_from_yaml(value: &serde_yaml::Value) -> Option<Vec<usize>> {
    let values = value
        .as_sequence()?
        .iter()
        .filter_map(|item| item.as_i64())
        .filter(|value| *value > 0)
        .map(|value| value as usize)
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn markov_config_json(config: &MarkovConfig) -> JsonValue {
    json!({
        "enabled": config.enabled,
        "timezone": config.timezone.name(),
        "daily_time": config.daily_time.format("%H:%M").to_string(),
        "run_weekdays_only": config.run_weekdays_only,
        "window_days": config.window_days,
        "threshold": config.threshold,
        "horizon_minutes": config.horizon_minutes,
        "sample_count": config.sample_count,
        "min_labeled_days": config.min_labeled_days,
        "signal_horizon_days": config.signal_horizon_days,
        "forecast_steps": config.forecast_steps,
        "max_symbols": config.max_symbols
    })
}

fn parse_hh_mm(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = session_text(session, "environment")
        .or_else(|| yaml_string(&state.config, &["saxo", "environment"]))
        .unwrap_or_else(|| "sim".to_string())
        .to_lowercase();
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

pub(crate) fn account_key(state: &AppState, session: &JsonValue) -> Result<String> {
    yaml_string(&state.config, &["saxo", "account_key"])
        .or_else(|| session_text(session, "account_key"))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Saxo AccountKey is missing"))
}

fn session_text(session: &JsonValue, key: &str) -> Option<String> {
    session
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn symbol_parts(symbol: &str) -> SymbolParts {
    let mut split = symbol.trim().splitn(2, ':');
    let base = split.next().unwrap_or("").trim().to_uppercase();
    let exchange = split.next().unwrap_or("").trim().to_lowercase();
    SymbolParts { base, exchange }
}

fn exchange_id_for_suffix(exchange: &str) -> &'static str {
    match exchange.to_lowercase().as_str() {
        "xnas" => "XNAS",
        "xnys" => "XNYS",
        "xcse" => "XCSE",
        "xsto" => "XSTO",
        "xosl" => "XOSL",
        "xhel" => "XHEL",
        "xlon" => "XLON",
        "xetr" => "XETR",
        "xfra" => "XFRA",
        "xmil" => "XMIL",
        "xpar" => "XPAR",
        "xams" => "XAMS",
        "xbru" => "XBRU",
        "xlse" => "XLIS",
        _ => "",
    }
}

fn exchange_aliases(exchange: &str) -> Vec<&'static str> {
    match exchange.to_lowercase().as_str() {
        "xnas" => vec!["XNAS", "NASDAQ"],
        "xnys" => vec!["XNYS", "NYSE"],
        "xcse" => vec!["XCSE", "CSE", "COP"],
        "xsto" => vec!["XSTO", "STO", "STK"],
        "xosl" => vec!["XOSL", "OSL", "OSE"],
        "xhel" => vec!["XHEL", "HEL", "HEX"],
        "xlon" => vec!["XLON", "LSE", "LON"],
        "xetr" => vec!["XETR", "XTRA", "ETR"],
        "xfra" => vec!["XFRA", "FSE", "FRA"],
        "xmil" => vec!["XMIL", "MIL"],
        "xpar" => vec!["XPAR", "PAR"],
        "xams" => vec!["XAMS", "AMS"],
        "xbru" => vec!["XBRU", "BRU"],
        "xlse" => vec!["XLIS", "LIS"],
        _ => vec![],
    }
}

fn candidate_score(
    candidate: &JsonValue,
    requested_symbol: &str,
    parts: &SymbolParts,
) -> (i32, i32, i32) {
    let candidate_symbol = text(candidate, "Symbol").to_uppercase();
    let candidate_exchange = text(candidate, "ExchangeId").to_uppercase();
    let exact_symbol = i32::from(candidate_symbol == requested_symbol.to_uppercase());
    let exchange_match = i32::from(
        exchange_aliases(&parts.exchange)
            .iter()
            .any(|alias| candidate_exchange == *alias)
            || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase())),
    );
    let exact_base = i32::from(candidate_symbol.split(':').next().unwrap_or("") == parts.base);
    let stock_preferred = i32::from(
        candidate
            .get("TradableAs")
            .and_then(JsonValue::as_array)
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some("Stock"))),
    );
    (exact_symbol, exchange_match, exact_base + stock_preferred)
}

fn candidate_matches_requested(
    candidate: &JsonValue,
    requested_symbol: &str,
    parts: &SymbolParts,
) -> bool {
    let candidate_symbol = text(candidate, "Symbol").to_uppercase();
    let candidate_base = candidate_symbol.split(':').next().unwrap_or("");
    if candidate_symbol == requested_symbol.to_uppercase() {
        return true;
    }
    if candidate_base != parts.base {
        return false;
    }
    if parts.exchange.is_empty() {
        return true;
    }
    let candidate_exchange = text(candidate, "ExchangeId").to_uppercase();
    exchange_aliases(&parts.exchange)
        .iter()
        .any(|alias| candidate_exchange == *alias)
        || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase()))
}

fn text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn number_from_keys(value: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
            .filter(|value| value.is_finite())
    })
}

trait EmptyStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>;
}

impl EmptyStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        if self.trim().is_empty() {
            fallback()
        } else {
            Some(self)
        }
    }
}

fn optional_text_sql(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn optional_json_sql(value: Option<&JsonValue>) -> String {
    value
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::to_string(value).ok())
        .map(|value| format!("'{}'", sql_escape(&value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn optional_i64_sql(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

fn optional_f64_sql(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

fn matrix_usize_json(matrix: [[usize; 3]; 3]) -> JsonValue {
    JsonValue::Array(
        matrix
            .iter()
            .enumerate()
            .map(|(row_idx, row)| {
                json!({
                    "from": STATES[row_idx].label(),
                    "to": row.iter().enumerate().map(|(col_idx, count)| json!({
                        "state": STATES[col_idx].label(),
                        "count": count
                    })).collect::<Vec<_>>()
                })
            })
            .collect(),
    )
}

fn matrix_f64_json(matrix: [[f64; 3]; 3]) -> JsonValue {
    JsonValue::Array(
        matrix
            .iter()
            .enumerate()
            .map(|(row_idx, row)| {
                json!({
                    "from": STATES[row_idx].label(),
                    "to": row.iter().enumerate().map(|(col_idx, probability)| json!({
                        "state": STATES[col_idx].label(),
                        "probability": probability
                    })).collect::<Vec<_>>()
                })
            })
            .collect(),
    )
}

fn distribution_json(distribution: [f64; 3]) -> JsonValue {
    json!({
        "Bull": distribution[Regime::Bull.index()],
        "Sideways": distribution[Regime::Sideways.index()],
        "Bear": distribution[Regime::Bear.index()]
    })
}

fn forecasts_json(forecasts: &[(usize, [f64; 3])]) -> JsonValue {
    JsonValue::Array(
        forecasts
            .iter()
            .map(|(step, distribution)| {
                json!({
                    "days": step,
                    "distribution": distribution_json(*distribution),
                    "signed_signal": distribution[Regime::Bull.index()] - distribution[Regime::Bear.index()]
                })
            })
            .collect(),
    )
}

fn recent_labels_json(labels: &[LabelPoint], limit: usize) -> JsonValue {
    JsonValue::Array(
        labels
            .iter()
            .rev()
            .take(limit)
            .rev()
            .map(|label| {
                json!({
                    "time": label.time,
                    "close": label.close,
                    "rolling_return": label.rolling_return,
                    "regime": label.regime.label()
                })
            })
            .collect(),
    )
}

fn label_counts_json(labels: &[LabelPoint]) -> JsonValue {
    let mut counts = HashMap::from([("Bull", 0usize), ("Sideways", 0usize), ("Bear", 0usize)]);
    for label in labels {
        if let Some(count) = counts.get_mut(label.regime.label()) {
            *count += 1;
        }
    }
    json!(counts)
}

pub fn create_schema_sql() -> Vec<&'static str> {
    vec![
        "CREATE TABLE IF NOT EXISTS markov_signal_runs (
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
        "CREATE TABLE IF NOT EXISTS markov_asset_signals (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            run_date TEXT NOT NULL,
            status TEXT NOT NULL,
            symbol TEXT NOT NULL,
            instrument_name TEXT,
            exchange TEXT,
            source TEXT,
            uic INTEGER,
            asset_type TEXT,
            window_days INTEGER NOT NULL,
            threshold REAL NOT NULL,
            horizon_minutes INTEGER NOT NULL,
            sample_count INTEGER NOT NULL,
            min_labeled_days INTEGER NOT NULL,
            signal_horizon_days INTEGER NOT NULL,
            current_state TEXT,
            current_close REAL,
            rolling_return REAL,
            transition_counts_json TEXT,
            transition_matrix_json TEXT,
            forecasts_json TEXT,
            stationary_json TEXT,
            bull_prob REAL,
            sideways_prob REAL,
            bear_prob REAL,
            signed_signal REAL,
            direction TEXT,
            conviction REAL,
            error_text TEXT,
            raw_payload_json TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_markov_signal_runs_date ON markov_signal_runs(run_date DESC)",
        "CREATE INDEX IF NOT EXISTS idx_markov_asset_signals_date_symbol ON markov_asset_signals(run_date DESC, symbol)",
        "CREATE INDEX IF NOT EXISTS idx_markov_asset_signals_signal ON markov_asset_signals(run_date DESC, signed_signal DESC)",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars_from_closes(closes: &[f64]) -> Vec<ChartBar> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| ChartBar {
                time: format!("2026-01-{index:02}"),
                close: *close,
            })
            .collect()
    }

    fn test_config() -> MarkovConfig {
        MarkovConfig {
            enabled: true,
            timezone: chrono_tz::Europe::Copenhagen,
            daily_time: NaiveTime::from_hms_opt(23, 30, 0).unwrap(),
            run_weekdays_only: true,
            window_days: 2,
            threshold: 0.05,
            horizon_minutes: 1440,
            sample_count: 100,
            min_labeled_days: 3,
            signal_horizon_days: 2,
            forecast_steps: vec![1, 2],
            max_symbols: 0,
        }
    }

    #[test]
    fn labels_regimes_from_rolling_return_threshold() {
        let bars = bars_from_closes(&[100.0, 100.0, 106.0, 103.0, 94.0, 100.0]);
        let labels = label_regimes(&bars, 2, 0.05);
        let regimes = labels
            .iter()
            .map(|label| label.regime.label())
            .collect::<Vec<_>>();
        assert_eq!(regimes, vec!["Bull", "Sideways", "Bear", "Sideways"]);
    }

    #[test]
    fn builds_transition_matrix_by_mle_counts() {
        let bars = bars_from_closes(&[100.0, 100.0, 106.0, 103.0, 94.0, 100.0]);
        let labels = label_regimes(&bars, 2, 0.05);
        let counts = transition_counts(&labels);
        assert_eq!(counts[Regime::Bull.index()][Regime::Sideways.index()], 1);
        assert_eq!(counts[Regime::Sideways.index()][Regime::Bear.index()], 1);
        assert_eq!(counts[Regime::Bear.index()][Regime::Sideways.index()], 1);
        let matrix = transition_matrix(counts);
        assert_eq!(matrix[Regime::Bull.index()][Regime::Sideways.index()], 1.0);
        assert_eq!(matrix[Regime::Bear.index()][Regime::Sideways.index()], 1.0);
    }

    #[test]
    fn forecasts_by_matrix_power_and_signal_is_bull_minus_bear() {
        let matrix = [[0.8, 0.15, 0.05], [0.2, 0.5, 0.3], [0.05, 0.2, 0.75]];
        let forecast = forecast_distribution(matrix, Regime::Bull, 2);
        assert!((forecast[Regime::Bull.index()] - 0.6725).abs() < 1e-9);
        assert!((forecast[Regime::Bear.index()] - 0.1225).abs() < 1e-9);
        let signal = forecast[Regime::Bull.index()] - forecast[Regime::Bear.index()];
        assert!((signal - 0.55).abs() < 1e-9);
    }

    #[test]
    fn analyzes_bars_into_current_signal() {
        let bars = bars_from_closes(&[
            100.0, 101.0, 102.0, 104.0, 106.0, 108.0, 111.0, 113.0, 120.0, 126.0,
        ]);
        let analysis = analyze_bars(&bars, &test_config()).unwrap();
        assert_eq!(analysis.current_state, Regime::Bull);
        assert_eq!(analysis.direction, "long");
        assert!(analysis.conviction >= 0.0);
    }
}
