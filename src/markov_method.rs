use std::collections::{HashMap, HashSet};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, NaiveTime, TimeZone, Utc};
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
/// Length of a regular cash-equity session in minutes. Used only to convert
/// calendar tunings into bar counts at an intraday horizon.
const DEFAULT_SESSION_MINUTES: i64 = 510;
/// Saxo service group the chart sweep spends its quota in.
const SAXO_CHART_SERVICE_GROUP: &str = "chart";
const TRADABLE_ASSET_TYPES: &str = "Stock,Etf,Etn,Etc";
const SAXO_MARKOV_MAX_ATTEMPTS: usize = 4;
const DEFAULT_INSTRUMENT_NEGATIVE_CACHE_RETRY_DAYS: i64 = 7;
const NO_TRADABLE_INSTRUMENT_PREFIX: &str = "No tradable Saxo instrument match found for";

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

/// A named time of day at which a Markov refresh should run, in addition to
/// the nightly `daily_time` pass.
#[derive(Clone, Debug, PartialEq)]
struct MarkovRunSlot {
    name: String,
    local_time: NaiveTime,
    /// Zone `local_time` is read in. A slot that has to stay on one side of a
    /// report anchored to another zone must share that zone, or the two drift
    /// apart for the weeks each year when US and EU daylight saving disagree.
    timezone: Tz,
}

impl MarkovRunSlot {
    /// The instant this slot becomes due on `run_date`, in its own zone.
    fn due_at(&self, run_date: NaiveDate) -> Option<DateTime<Utc>> {
        self.timezone
            .from_local_datetime(&run_date.and_time(self.local_time))
            .earliest()
            .map(|due| due.with_timezone(&Utc))
    }
}

/// How many bars the chart feed emits per trading session at `horizon_minutes`.
///
/// Every Markov tuning below (`window_days`, `min_labeled_days`,
/// `signal_horizon_days`, `forecast_steps`) is applied as an index offset into
/// the bar series, so each one counts *bars*, not calendar days. That is only
/// the same thing while the horizon is daily. Returning 1 for daily-or-coarser
/// horizons keeps those tunings byte-identical to their historical meaning; an
/// intraday horizon scales them up so "20 days" stays 20 days.
/// The tunings scaled into bar counts for one instrument's exchange.
///
/// Per exchange rather than global because a trading session is not the same
/// length everywhere: measured against live SIM, NASDAQ and NYSE return 7
/// hourly bars a day, Copenhagen and Oslo 8, and the rest 9. A single global
/// value made `window_days: 20` mean 20 trading days on some exchanges and 26
/// on others, silently, for roughly 40% of the universe.
///
/// Derived per *exchange* and never per instrument: a thin listing drops bars,
/// so measuring one instrument's own series gives the wrong session length --
/// `ARKI:xlon` reads 7 where every other London name reads 9.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MarkovScaling {
    pub(crate) bars_per_session: usize,
    pub(crate) window_bars: usize,
    pub(crate) min_labeled_bars: usize,
    pub(crate) signal_horizon_bars: usize,
    pub(crate) forecast_step_bars: Vec<usize>,
}

impl MarkovScaling {
    fn for_exchange(config: &MarkovConfig, exchange: &str) -> Self {
        let per_session = bars_per_session(
            config.horizon_minutes,
            session_minutes_for_exchange(config, exchange),
        );
        Self {
            bars_per_session: per_session,
            window_bars: config.window_days.saturating_mul(per_session).max(1),
            min_labeled_bars: config.min_labeled_days.saturating_mul(per_session).max(1),
            signal_horizon_bars: config
                .signal_horizon_days
                .saturating_mul(per_session)
                .max(1),
            forecast_step_bars: config
                .forecast_steps
                .iter()
                .map(|step| step.saturating_mul(per_session).max(1))
                .collect(),
        }
    }
}

/// Session length for one exchange, falling back to the global default.
fn session_minutes_for_exchange(config: &MarkovConfig, exchange: &str) -> i64 {
    config
        .session_minutes_by_exchange
        .get(&exchange.to_ascii_lowercase())
        .copied()
        .unwrap_or(config.session_minutes)
}

fn bars_per_session(horizon_minutes: i64, session_minutes: i64) -> usize {
    if horizon_minutes >= 1440 {
        return 1;
    }
    let horizon = horizon_minutes.max(1);
    let session = session_minutes.max(horizon);
    (((session + horizon - 1) / horizon).max(1)) as usize
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
    /// Minutes in one regular trading session, used to convert the calendar
    /// tunings above into bar counts when `horizon_minutes` is intraday.
    session_minutes: i64,
    /// Per-exchange overrides, keyed by lowercase suffix. A session is not the
    /// same length everywhere, so a single value makes `window_days` mean
    /// different numbers of days on different exchanges.
    session_minutes_by_exchange: HashMap<String, i64>,
    /// Bars the chart feed returns per session at `horizon_minutes`; 1 for
    /// daily-or-coarser horizons.
    /// The three fields below are the scaled *defaults*, reported on the config
    /// surface beside `session_minutes_by_exchange`. What an analysis actually
    /// applies comes from `MarkovScaling::for_exchange`, never from here.
    bars_per_session: usize,
    window_bars: usize,
    min_labeled_bars: usize,
    signal_horizon_bars: usize,
    /// Every refresh slot for one trading day, ascending. Always contains the
    /// nightly `daily_time` pass; intraday slots are prepended by config.
    run_slots: Vec<MarkovRunSlot>,
    max_symbols: usize,
    instrument_negative_cache_retry_days: i64,
    symbol_aliases: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub(crate) struct MarkovAsset {
    pub(crate) symbol: String,
    pub(crate) analysis_symbol: String,
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
    // Pick the most recent slot that is already due today. The nightly pass is
    // just the last slot of the day, so a single-slot config behaves exactly as
    // it did before intraday refreshes existed.
    let Some(slot) = due_markov_slot(&config, run_date, Utc::now()) else {
        let next = config
            .run_slots
            .first()
            .map(|slot| slot.local_time)
            .unwrap_or(config.daily_time);
        return Ok(json!({
            "status": "idle",
            "reason": "not_due",
            "run_date": run_date.to_string(),
            "due_time": next.format("%H:%M").to_string(),
            "timezone": config.timezone.name()
        }));
    };

    // Dedup per slot rather than per date: a slot has run when a run for today
    // was created at or after that slot became due.
    let slot_due_utc = slot.due_at(run_date);
    if markov_run_exists_since(state, run_date, slot_due_utc).await? {
        return Ok(json!({
            "status": "skipped",
            "reason": "already_ran",
            "run_date": run_date.to_string(),
            "slot": slot.name,
            "due_time": slot.local_time.format("%H:%M").to_string()
        }));
    }

    info!(
        run_date = %run_date,
        slot = %slot.name,
        due_time = %slot.local_time.format("%H:%M"),
        horizon_minutes = config.horizon_minutes,
        bars_per_session = config.bars_per_session,
        "starting Markov refresh"
    );
    run_markov_method_for_date(state, &config, run_date).await
}

/// The latest configured slot already due on `run_date`.
///
/// Compared as instants rather than wall-clock times, so slots in different
/// zones still order correctly against each other.
fn due_markov_slot(
    config: &MarkovConfig,
    run_date: NaiveDate,
    now: DateTime<Utc>,
) -> Option<&MarkovRunSlot> {
    config
        .run_slots
        .iter()
        .filter_map(|slot| slot.due_at(run_date).map(|due| (slot, due)))
        .filter(|(_, due)| *due <= now)
        .max_by_key(|(_, due)| *due)
        .map(|(slot, _)| slot)
}

async fn run_markov_method_for_date(
    state: &AppState,
    config: &MarkovConfig,
    run_date: NaiveDate,
) -> Result<JsonValue> {
    let assets = markov_assets(state, config.max_symbols).await?;
    run_markov_over_assets(state, config, run_date, assets, false).await
}

/// Refresh Markov signals for a named set of symbols, outside the slot
/// schedule.
///
/// This exists so Hermes can ask for a stale signal to be recomputed instead of
/// blocking a candidate on its age. It is deliberately narrow: it recomputes
/// the same model over the same inputs for symbols that are already in the
/// configured universe, and can neither widen the universe nor change any
/// tuning.
pub(crate) async fn refresh_markov_signals_for_symbols(
    state: &AppState,
    symbols: &[String],
) -> Result<JsonValue> {
    let config = markov_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let wanted = symbols
        .iter()
        .map(|symbol| normalized_symbol_key(symbol))
        .collect::<HashSet<_>>();
    if wanted.is_empty() {
        return Ok(json!({"status": "skipped", "reason": "no_symbols"}));
    }
    let assets = markov_assets(state, 0)
        .await?
        .into_iter()
        .filter(|asset| wanted.contains(&normalized_symbol_key(&asset.symbol)))
        .collect::<Vec<_>>();
    if assets.is_empty() {
        return Ok(json!({
            "status": "skipped",
            "reason": "no_matching_universe_symbols",
            "requested": symbols,
        }));
    }
    let run_date = Utc::now().with_timezone(&config.timezone).date_naive();
    run_markov_over_assets(state, &config, run_date, assets, true).await
}

/// Shared analyse-and-persist loop for both the scheduled pass and a targeted
/// refresh. `targeted` only changes the recorded run status, so a partial
/// refresh is distinguishable from a full nightly run in run health.
async fn run_markov_over_assets(
    state: &AppState,
    config: &MarkovConfig,
    run_date: NaiveDate,
    assets: Vec<MarkovAsset>,
    targeted: bool,
) -> Result<JsonValue> {
    let session = state
        .ensure_saxo_session_json("markov_method")
        .await
        .context("loading Saxo session for Markov method run")?;
    let run_id = format!("markov-{}", Utc::now().timestamp_micros());
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // Measure only this sweep's share of the chart quota. Wall-clock duration
    // cannot separate "200 assets take this long" from "something else is
    // spending the same budget"; pacer waiting can.
    crate::saxo_rate_limit::reset(SAXO_CHART_SERVICE_GROUP);
    let mut rows = Vec::new();
    let mut success_count = 0usize;
    let mut error_count = 0usize;

    for asset in &assets {
        let scaling = MarkovScaling::for_exchange(
            config,
            asset.symbol.split_once(':').map(|(_, ex)| ex).unwrap_or(""),
        );
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
                    &scaling,
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
                    &scaling,
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
        if targeted {
            "targeted_refresh"
        } else {
            "completed"
        }
    } else if assets.is_empty() {
        "empty"
    } else {
        "error"
    };
    let summary = json!({
        "status": status,
        "run_id": run_id,
        "rate_limiter": crate::saxo_rate_limit::snapshot(SAXO_CHART_SERVICE_GROUP),
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
        targeted,
        "Markov method run completed"
    );
    Ok(summary)
}

async fn analyze_asset(
    state: &AppState,
    session: &JsonValue,
    config: &MarkovConfig,
    asset: &MarkovAsset,
) -> Result<(SaxoInstrument, Vec<ChartBar>, MarkovAnalysis)> {
    let instrument = resolve_instrument(state, session, &asset.analysis_symbol)
        .await
        .with_context(|| {
            format!(
                "resolving Saxo instrument for {}",
                asset_analysis_label(asset)
            )
        })?;
    let bars = fetch_chart_bars(state, session, &instrument, config)
        .await
        .with_context(|| {
            format!(
                "fetching Saxo chart history for {}",
                asset_analysis_label(asset)
            )
        })?;
    // Scaled for this instrument's own exchange, so `window_days` means the
    // same number of trading days everywhere.
    let scaling = MarkovScaling::for_exchange(
        config,
        asset.symbol.split_once(':').map(|(_, ex)| ex).unwrap_or(""),
    );
    let analysis = analyze_bars(&bars, config, &scaling)
        .with_context(|| format!("running Markov model for {}", asset_analysis_label(asset)))?;
    Ok((instrument, bars, analysis))
}

fn asset_analysis_label(asset: &MarkovAsset) -> String {
    if asset.analysis_symbol == asset.symbol {
        asset.symbol.clone()
    } else {
        format!("{} via {}", asset.symbol, asset.analysis_symbol)
    }
}

fn analyze_bars(
    bars: &[ChartBar],
    config: &MarkovConfig,
    scaling: &MarkovScaling,
) -> Result<MarkovAnalysis> {
    let labels = label_regimes(bars, scaling.window_bars, config.threshold);
    if labels.len() < scaling.min_labeled_bars {
        bail!(
            "not enough labeled bars: {} available, {} required",
            labels.len(),
            scaling.min_labeled_bars
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
        .zip(scaling.forecast_step_bars.iter().copied())
        // Keep the persisted key in calendar steps so stored forecasts stay
        // comparable across a horizon change; only the lookup depth scales.
        .map(|(step, step_bars)| {
            (
                step,
                forecast_distribution(matrix, current.regime, step_bars),
            )
        })
        .collect::<Vec<_>>();
    let signal_distribution =
        forecast_distribution(matrix, current.regime, scaling.signal_horizon_bars);
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
    let aliases = analysis_symbol_aliases(&state.config);
    for row in state.position_items(250).await.unwrap_or_default() {
        push_asset(&mut assets, &mut seen, &row, "portfolio", &aliases);
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
            push_asset(&mut assets, &mut seen, &row, "watchlist", &aliases);
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
    aliases: &HashMap<String, String>,
) {
    let symbol = text(row, "symbol").trim().to_string();
    if symbol.is_empty() || !seen.insert(symbol.clone()) {
        return;
    }
    let alias_key = normalized_symbol_key(&symbol);
    let analysis_symbol = aliases
        .get(&alias_key)
        .cloned()
        .unwrap_or_else(|| symbol.clone());
    let instrument_name = text(row, "instrument_name")
        .if_empty_then(|| Some(symbol.clone()))
        .unwrap_or_else(|| symbol.clone());
    assets.push(MarkovAsset {
        symbol,
        analysis_symbol,
        instrument_name,
        source: source.to_string(),
    });
}

fn analysis_symbol_aliases(config: &serde_yaml::Value) -> HashMap<String, String> {
    yaml_at(config, &["strategy", "markov", "symbol_aliases"])
        .and_then(serde_yaml::Value::as_mapping)
        .map(|mapping| {
            mapping
                .iter()
                .filter_map(|(key, value)| {
                    let from = key.as_str()?.trim();
                    let to = value.as_str()?.trim();
                    if from.is_empty() || to.is_empty() {
                        return None;
                    }
                    Some((normalized_symbol_key(from), to.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn normalized_symbol_key(symbol: &str) -> String {
    let parts = symbol_parts(symbol);
    requested_symbol(&parts).to_lowercase()
}

pub(crate) async fn resolve_instrument(
    state: &AppState,
    session: &JsonValue,
    symbol: &str,
) -> Result<SaxoInstrument> {
    if let Some(instrument) = stored_instrument(state, symbol).await? {
        clear_negative_instrument_lookup(state, symbol).await?;
        return Ok(instrument);
    }
    if let Some(cached) = fresh_negative_instrument_lookup(state, symbol).await? {
        bail!(
            "Saxo instrument lookup skipped for {symbol}; cached negative result until {}: {}",
            cached.retry_after,
            cached.error_text
        );
    }
    match lookup_instrument(state, session, symbol).await {
        Ok(instrument) => {
            clear_negative_instrument_lookup(state, symbol).await?;
            Ok(instrument)
        }
        Err(err) => {
            if is_negative_cacheable_lookup_error(&err) {
                record_negative_instrument_lookup(state, symbol, &format!("{err:#}")).await?;
            }
            Err(err)
        }
    }
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
    let requested_symbol = requested_symbol(&parts);
    let mut attempts = Vec::new();
    let mut seen_attempts = HashSet::new();
    for variant in symbol_lookup_variants(&parts) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant.eq_ignore_ascii_case(&requested_symbol) {
                "symbol"
            } else {
                "symbol_variant"
            },
            vec![("Keywords", variant)],
            true,
        );
    }
    for variant in base_lookup_variants(&parts.base) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant == parts.base {
                "exchange"
            } else {
                "exchange_variant"
            },
            vec![
                ("Keywords", variant),
                (
                    "ExchangeId",
                    exchange_id_for_suffix(&parts.exchange).to_string(),
                ),
            ],
            true,
        );
    }
    if let Some(isin) = latest_position_isin(state, symbol).await? {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            "isin",
            vec![("Keywords", isin)],
            false,
        );
    }
    for variant in base_lookup_variants(&parts.base) {
        push_lookup_attempt(
            &mut attempts,
            &mut seen_attempts,
            if variant == parts.base {
                "base"
            } else {
                "base_variant"
            },
            vec![("Keywords", variant)],
            true,
        );
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
    Err(anyhow!("{NO_TRADABLE_INSTRUMENT_PREFIX} {symbol}"))
}

#[derive(Clone, Debug)]
struct NegativeInstrumentLookup {
    retry_after: String,
    error_text: String,
}

async fn fresh_negative_instrument_lookup(
    state: &AppState,
    symbol: &str,
) -> Result<Option<NegativeInstrumentLookup>> {
    let row = sqlx::query(&format!(
        "SELECT retry_after, error_text
         FROM saxo_instrument_negative_cache
         WHERE symbol = '{}'
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let retry_after = row.try_get::<String, _>("retry_after").unwrap_or_default();
    let error_text = row.try_get::<String, _>("error_text").unwrap_or_default();
    let retry_at = DateTime::parse_from_rfc3339(&retry_after)
        .map(|value| value.with_timezone(&Utc))
        .ok();
    if retry_at.is_some_and(|value| value > Utc::now()) {
        Ok(Some(NegativeInstrumentLookup {
            retry_after,
            error_text,
        }))
    } else {
        Ok(None)
    }
}

async fn record_negative_instrument_lookup(
    state: &AppState,
    symbol: &str,
    error_text: &str,
) -> Result<()> {
    let now = Utc::now();
    let retry_days = instrument_negative_cache_retry_days(state);
    let retry_after = now + ChronoDuration::days(retry_days);
    sqlx::query(&format!(
        "INSERT INTO saxo_instrument_negative_cache (
            symbol, created_at, last_error_at, retry_after, error_text, attempt_count
        ) VALUES ('{}', '{}', '{}', '{}', '{}', 1)
        ON CONFLICT(symbol) DO UPDATE SET
            last_error_at = excluded.last_error_at,
            retry_after = excluded.retry_after,
            error_text = excluded.error_text,
            attempt_count = COALESCE(saxo_instrument_negative_cache.attempt_count, 0) + 1",
        sql_escape(symbol),
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        retry_after.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        sql_escape(error_text)
    ))
    .execute(&state.pool)
    .await
    .context("recording Saxo instrument negative lookup cache")?;
    Ok(())
}

async fn clear_negative_instrument_lookup(state: &AppState, symbol: &str) -> Result<()> {
    sqlx::query(&format!(
        "DELETE FROM saxo_instrument_negative_cache WHERE symbol = '{}'",
        sql_escape(symbol)
    ))
    .execute(&state.pool)
    .await
    .context("clearing Saxo instrument negative lookup cache")?;
    Ok(())
}

fn is_negative_cacheable_lookup_error(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains(NO_TRADABLE_INSTRUMENT_PREFIX)
}

fn instrument_negative_cache_retry_days(state: &AppState) -> i64 {
    yaml_i64(
        &state.config,
        &["strategy", "markov", "instrument_negative_cache_retry_days"],
    )
    .unwrap_or(DEFAULT_INSTRUMENT_NEGATIVE_CACHE_RETRY_DAYS)
    .max(1)
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
    let client = crate::saxo_http::client();
    let url = format!("{}{}", openapi_base_url(state, session)?, path);
    let requests_per_minute = crate::saxo_rate_limit::configured_rate(&state.config);
    let mut last_error = String::new();
    for attempt in 1..=SAXO_MARKOV_MAX_ATTEMPTS {
        // Replaces a fixed 500 ms sleep chosen when this job was capped at 20
        // symbols. The bucket is shared with the daily-indicator sweep, which
        // calls through here too, so the two no longer pace independently
        // against one limit.
        crate::saxo_rate_limit::acquire(path, requests_per_minute).await;
        let response = client
            .get(&url)
            .bearer_auth(&access_token)
            .header(header::ACCEPT, "application/json")
            .query(query)
            .send()
            .await?;
        let status = response.status();
        crate::saxo_rate_limit::observe(path, response.headers());
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
    // The scaling actually applied to this instrument, which differs by
    // exchange, rather than the configuration default.
    scaling: &MarkovScaling,
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
            "method": "observable_markov_rolling_return",
            "analysis_symbol": asset.analysis_symbol,
            "symbol_alias_applied": asset.analysis_symbol != asset.symbol
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
        "session_minutes": config.session_minutes,
        "bars_per_session": scaling.bars_per_session,
        "window_bars": scaling.window_bars,
        "min_labeled_bars": scaling.min_labeled_bars,
        "signal_horizon_bars": scaling.signal_horizon_bars,
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
            bars_per_session, window_bars, min_labeled_bars, signal_horizon_bars,
            current_state, current_close, rolling_return, transition_counts_json,
            transition_matrix_json, forecasts_json, stationary_json, bull_prob,
            sideways_prob, bear_prob, signed_signal, direction, conviction,
            error_text, raw_payload_json
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', '{}', '{}',
            '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {},
            {}, {}, {}, {},
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
        optional_i64_sql(row.get("bars_per_session").and_then(JsonValue::as_i64)),
        optional_i64_sql(row.get("window_bars").and_then(JsonValue::as_i64)),
        optional_i64_sql(row.get("min_labeled_bars").and_then(JsonValue::as_i64)),
        optional_i64_sql(row.get("signal_horizon_bars").and_then(JsonValue::as_i64)),
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

/// True when a run for `run_date` was created at or after `since`.
///
/// `created_at` is stored as second-precision RFC3339 in UTC, so the string
/// comparison below is a chronological one.
async fn markov_run_exists_since(
    state: &AppState,
    run_date: NaiveDate,
    since: Option<DateTime<Utc>>,
) -> Result<bool> {
    let Some(since) = since else {
        return markov_run_exists(state, run_date).await;
    };
    let row = sqlx::query(&format!(
        "SELECT id FROM markov_signal_runs WHERE run_date = '{}' AND created_at >= '{}' LIMIT 1",
        sql_escape(&run_date.to_string()),
        sql_escape(&since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
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
    latest_markov_signals_page_filtered(state, limit, 0, "all", 0.0).await
}

/// Canonical Markov signal filters, in the order the UI presents them.
///
/// Deliberately omits the roadmap's suggested "stale signals" filter. Every row
/// in a single run shares that run's `run_date`, so staleness is a property of
/// the run as a whole, never of one signal against its siblings — a per-row
/// stale filter would either match everything or nothing, and dressing that up
/// as a choice would be misleading.
pub(crate) const MARKOV_FILTERS: &[&str] =
    &["all", "portfolio", "watchlist", "conviction", "errors"];

pub(crate) fn normalize_markov_filter(value: Option<&str>) -> String {
    let requested = value.unwrap_or("all").trim().to_ascii_lowercase();
    if MARKOV_FILTERS.contains(&requested.as_str()) {
        requested
    } else {
        "all".to_string()
    }
}

pub(crate) fn markov_filter_label(filter: &str) -> &'static str {
    match filter {
        "portfolio" => "Portfolio",
        "watchlist" => "Watchlist",
        "conviction" => "High conviction",
        "errors" => "Errors",
        _ => "All",
    }
}

/// The SQL fragment a filter appends to the shared `WHERE run_id = (...)` base.
///
/// Both [`latest_markov_signals_page`] and [`latest_markov_signal_count`] build
/// on this one function on purpose: if the page and the count ever applied
/// different predicates, the pagination controls would advertise pages that
/// render empty, which is worse than having no filter at all.
///
/// Portfolio membership is matched case-insensitively because broker and
/// universe spellings genuinely differ for the same instrument — Saxo returns
/// `DB1Gn:xetr` where the configured universe carries `db1gn:xetr`.
pub(crate) fn markov_filter_sql(filter: &str, min_signed_signal: f64) -> String {
    const HELD: &str = "SELECT UPPER(symbol) FROM broker_instrument_exposures WHERE quantity > 0";
    match filter {
        "portfolio" => format!(" AND UPPER(symbol) IN ({HELD})"),
        "watchlist" => format!(" AND UPPER(symbol) NOT IN ({HELD})"),
        // Compares against the same threshold the Trading Manager's Markov gate
        // applies, so "high conviction" means "would clear the gate" rather
        // than an arbitrary display cutoff.
        "conviction" => {
            let threshold = if min_signed_signal.is_finite() && min_signed_signal > 0.0 {
                min_signed_signal
            } else {
                0.0
            };
            format!(" AND status = 'ok' AND ABS(signed_signal) >= {threshold}")
        }
        "errors" => " AND status <> 'ok'".to_string(),
        _ => String::new(),
    }
}

/// One page of the newest signal per symbol.
///
/// Selected per symbol rather than from the newest *run*. A targeted refresh
/// writes a run containing only the symbols it names, so run-scoping collapsed
/// the whole Markov view to that one instrument while the run card beside it --
/// which excludes targeted runs -- still reported the full nightly pass. The
/// two disagreed on screen: "Signals: 200" above a list of one.
///
/// The universe is then pinned to the symbols in the latest *full* run, because
/// "latest per symbol" across all history also resurrects retired tickers: the
/// 27 symbols U11 remapped still hold rows under their old suffixes, which took
/// the live count from 201 to 230. Pinning keeps the list equal to the run card
/// while still preferring a targeted refresh row where one is fresher.
pub async fn latest_markov_signals_page_filtered(
    state: &AppState,
    limit: i64,
    offset: i64,
    filter: &str,
    min_signed_signal: f64,
) -> Result<Vec<JsonValue>> {
    let sql = format!(
        "SELECT id, run_id, created_at, run_date, status, symbol, instrument_name,
                exchange, source, uic, asset_type, window_days, threshold,
                horizon_minutes, sample_count, min_labeled_days, signal_horizon_days,
                current_state, current_close, rolling_return, transition_counts_json,
                transition_matrix_json, forecasts_json, stationary_json, bull_prob,
                sideways_prob, bear_prob, signed_signal, direction, conviction,
                error_text, raw_payload_json
         FROM markov_asset_signals
         WHERE id = (
            SELECT inner_signal.id
            FROM markov_asset_signals AS inner_signal
            WHERE inner_signal.symbol = markov_asset_signals.symbol
            ORDER BY inner_signal.run_date DESC, inner_signal.created_at DESC, inner_signal.id DESC
            LIMIT 1
         )
         AND symbol IN (
            SELECT universe.symbol
            FROM markov_asset_signals AS universe
            WHERE universe.run_id = (
               SELECT id
               FROM markov_signal_runs
               WHERE status <> 'targeted_refresh'
               ORDER BY run_date DESC, created_at DESC
               LIMIT 1
            )
         ){}
         ORDER BY run_date DESC, created_at DESC, symbol ASC
         LIMIT {} OFFSET {}",
        markov_filter_sql(filter, min_signed_signal),
        clamp_limit(limit, 1, 500),
        offset.max(0).min(100_000),
    );
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.iter().map(row_to_json).collect())
}

pub async fn latest_markov_signal_count_filtered(
    state: &AppState,
    filter: &str,
    min_signed_signal: f64,
) -> Result<i64> {
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS count
         FROM markov_asset_signals
         WHERE id = (
            SELECT inner_signal.id
            FROM markov_asset_signals AS inner_signal
            WHERE inner_signal.symbol = markov_asset_signals.symbol
            ORDER BY inner_signal.run_date DESC, inner_signal.created_at DESC, inner_signal.id DESC
            LIMIT 1
         )
         AND symbol IN (
            SELECT universe.symbol
            FROM markov_asset_signals AS universe
            WHERE universe.run_id = (
               SELECT id
               FROM markov_signal_runs
               WHERE status <> 'targeted_refresh'
               ORDER BY run_date DESC, created_at DESC
               LIMIT 1
            )
         ){}",
        markov_filter_sql(filter, min_signed_signal)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .as_ref()
        .and_then(|row| row.try_get::<i64, _>("count").ok())
        .unwrap_or(0))
}

/// Returns the latest persisted signal for one asset. This is read-only
/// attribution context; it does not refresh a signal, call Saxo, or influence
/// Markov scheduling and trading decisions.
/// The freshest signal for one symbol, whichever run produced it.
///
/// Deliberately *not* scoped to the newest run. A targeted refresh writes a run
/// containing only the symbols it was asked for, so pinning to the newest run
/// reports every other symbol as unavailable until the next full pass -- which
/// would blank the Markov column for exactly the candidates a refresh was meant
/// to leave untouched.
pub async fn latest_markov_signal_summary(state: &AppState, symbol: &str) -> Result<JsonValue> {
    let sql = format!(
        "SELECT run_date, status, current_state, current_close, rolling_return,
                bull_prob, sideways_prob, bear_prob, signed_signal, direction, conviction,
                error_text
         FROM markov_asset_signals
         WHERE symbol = '{}'
         ORDER BY run_date DESC, created_at DESC
         LIMIT 1",
        sql_escape(symbol)
    );
    let row = sqlx::query(&sql).fetch_optional(&state.pool).await?;
    Ok(row.as_ref().map(row_to_json).unwrap_or(JsonValue::Null))
}

pub async fn latest_markov_run(state: &AppState) -> Result<JsonValue> {
    // Targeted refreshes cover only a handful of symbols, so treating one as
    // "the latest run" would report a healthy full pass as a tiny degraded one.
    // Run health stays anchored to full passes; per-symbol freshness is read
    // from the signals themselves, which do pick up targeted rows.
    let row = sqlx::query(
        "SELECT id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json
         FROM markov_signal_runs
         WHERE status <> 'targeted_refresh'
         ORDER BY run_date DESC, created_at DESC
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.as_ref().map(row_to_json).unwrap_or(JsonValue::Null))
}

/// Cap on symbols one scoped request may name.
pub(crate) const MARKOV_CONTEXT_MAX_SYMBOLS: usize = 40;

/// Signals for exactly the symbols named, rather than a page of the universe.
///
/// The unscoped page returns `limit` rows ordered by run then symbol, so with
/// ~200 symbols and a 50-row default whether a candidate appears depends on its
/// alphabetical position. On 2026-08-31 that made DE:xnys (55th) visible and
/// PLTR:xnas (141st) invisible to the same advisory round, and Hermes had to
/// spend a data-request round recovering a signal that already existed.
pub async fn compact_markov_context_for_symbols(
    state: &AppState,
    symbols: &[String],
) -> Result<JsonValue> {
    let mut seen = HashSet::new();
    let mut wanted = Vec::new();
    for symbol in symbols {
        let trimmed = symbol.trim();
        if trimmed.is_empty() || !seen.insert(normalized_symbol_key(trimmed)) {
            continue;
        }
        wanted.push(trimmed.to_string());
        if wanted.len() >= MARKOV_CONTEXT_MAX_SYMBOLS {
            break;
        }
    }
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for symbol in &wanted {
        let row = latest_markov_signal_summary(state, symbol).await?;
        if row.is_null() {
            missing.push(symbol.clone());
            continue;
        }
        // `latest_markov_signal_summary` does not echo the symbol back.
        let mut row = row;
        if let Some(object) = row.as_object_mut() {
            object.insert("symbol".to_string(), json!(symbol));
        }
        rows.push(row);
    }
    let mut context = compact_markov_context_from_rows(state, rows).await?;
    if let Some(object) = context.as_object_mut() {
        object.insert("requested_symbols".to_string(), json!(wanted));
        object.insert("missing_symbols".to_string(), json!(missing));
        object.insert("scope".to_string(), json!("requested_symbols"));
    }
    Ok(context)
}

/// The Markov block the decision prompt receives.
///
/// Ordered by conviction, and never without a held position.
///
/// It used to reuse the dashboard's paged reader, whose `ORDER BY ... symbol
/// ASC` is right for a browsable list and wrong here: every row in a run shares
/// its `run_date` and `created_at`, so the effective order was alphabetical and
/// truncating at 80 of ~200 cut the universe off at `GSK`. Everything from H to
/// Z had no Markov evidence in the prompt at all — `TSLA:xnas` at +0.2471 was
/// invisible while a 0.0043 signal beginning with "A" was shown. This is the
/// 2026-08-31 `get_markov_signals` defect in a second place; that fix reached
/// Hermes's retrieval path, and Hermes only reviews candidates where this
/// decides which symbols can become one.
///
/// Held positions are pinned ahead of the conviction ranking because the two
/// jobs differ: an unheld symbol with a weak signal needs no decision, while a
/// held one always does. Conviction ordering alone would still have dropped
/// `ORCL:xnys` at rank 136 — a position already down 7.7% with no regime read
/// in front of the model.
pub async fn compact_markov_context(state: &AppState, limit: i64) -> Result<JsonValue> {
    let rows = latest_markov_signals_by_conviction(state, limit)
        .await?
        .into_iter()
        .filter(|row| row.get("status").and_then(JsonValue::as_str) == Some("ok"))
        .collect::<Vec<_>>();
    compact_markov_context_from_rows(state, rows).await
}

/// Newest signal per symbol, held positions first, then highest conviction.
///
/// `conviction` is the absolute signed signal, so this surfaces the strongest
/// views in both directions — the prompt uses a negative regime for risk
/// reduction just as it uses a positive one for entry.
pub(crate) async fn latest_markov_signals_by_conviction(
    state: &AppState,
    limit: i64,
) -> Result<Vec<JsonValue>> {
    let sql = format!(
        "SELECT s.id, s.run_id, s.created_at, s.run_date, s.status, s.symbol, s.instrument_name,
                s.exchange, s.source, s.uic, s.asset_type, s.window_days, s.threshold,
                s.horizon_minutes, s.sample_count, s.min_labeled_days, s.signal_horizon_days,
                s.current_state, s.current_close, s.rolling_return, s.transition_counts_json,
                s.transition_matrix_json, s.forecasts_json, s.stationary_json, s.bull_prob,
                s.sideways_prob, s.bear_prob, s.signed_signal, s.direction, s.conviction,
                s.error_text, s.raw_payload_json
         FROM markov_asset_signals AS s
         WHERE s.id = (
            SELECT inner_signal.id
            FROM markov_asset_signals AS inner_signal
            WHERE inner_signal.symbol = s.symbol
            ORDER BY inner_signal.run_date DESC, inner_signal.created_at DESC, inner_signal.id DESC
            LIMIT 1
         )
         AND s.symbol IN (
            SELECT universe.symbol
            FROM markov_asset_signals AS universe
            WHERE universe.run_id = (
               SELECT id
               FROM markov_signal_runs
               WHERE status <> 'targeted_refresh'
               ORDER BY run_date DESC, created_at DESC
               LIMIT 1
            )
         )
         ORDER BY
            CASE WHEN s.symbol IN (
               SELECT held.symbol FROM portfolio_position_snapshots AS held
               WHERE held.recorded_at = (
                  SELECT MAX(newest.recorded_at) FROM portfolio_position_snapshots AS newest
               )
            ) THEN 0 ELSE 1 END,
            s.conviction DESC,
            s.symbol ASC
         LIMIT {}",
        clamp_limit(limit, 1, 500),
    );
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.iter().map(row_to_json).collect())
}

async fn compact_markov_context_from_rows(
    state: &AppState,
    rows: Vec<JsonValue>,
) -> Result<JsonValue> {
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
    let latest_run = latest_markov_run(state).await.unwrap_or(JsonValue::Null);
    Ok(json!({
        "latest_run": trim_markov_run_for_prompt(&latest_run),
        "signals": signals
    }))
}

/// Strip the embedded per-signal payload out of a Markov run row before it
/// reaches the decision prompt.
///
/// `markov_signal_runs.summary_json` deliberately embeds up to 20 full signal
/// rows for operational debugging (`run_markov_method_for_date`), and each one
/// carries its `raw_payload_json.recent_labels` -- 60 raw daily observations
/// per symbol. `compact_markov_context` already serializes a properly trimmed
/// `signals` list of its own from `latest_markov_signals`, so embedding the
/// raw run row verbatim as `latest_run` duplicated that data and was the
/// actual source of the bloat the function's name promised not to have: the
/// decision prompt averaged 527 KB by July 2026, and the bulk of it was these
/// 20 embedded `recent_labels` arrays. See `wiki/urgent-todo.md` U12.
///
/// Everything else on the row -- `status`, `run_id`, `asset_count`,
/// `success_count`, `error_count`, `config` -- is run-level metadata the model
/// can use to judge signal freshness and coverage, and stays.
pub(crate) fn trim_markov_run_for_prompt(run: &JsonValue) -> JsonValue {
    let Some(object) = run.as_object() else {
        return run.clone();
    };
    let mut trimmed = object.clone();
    if let Some(summary) = trimmed
        .get_mut("summary_json")
        .and_then(JsonValue::as_object_mut)
    {
        summary.remove("signals");
    }
    JsonValue::Object(trimmed)
}

/// The tunings that define what a Markov signal *means*, as a compact stamp.
///
/// Retained gate evidence is only comparable across runs that share these. The
/// 2026-08-31 move from daily to hourly bars changed the signal distribution --
/// the same 0.15 threshold went from admitting 111 of 200 symbols to 132 -- so
/// a replay pooling runs from either side of it compares two different models
/// under one label. `min_signed_signal` was already recorded per run and shows
/// the threshold change; this records the model the threshold was applied to.
pub(crate) fn markov_model_fingerprint(state: &AppState) -> JsonValue {
    let config = markov_config(state);
    // Sorted so the stamp is stable across runs and two runs of the same model
    // compare equal.
    let mut by_exchange = config
        .session_minutes_by_exchange
        .iter()
        .map(|(exchange, minutes)| (exchange.clone(), JsonValue::from(*minutes)))
        .collect::<Vec<_>>();
    by_exchange.sort_by(|left, right| left.0.cmp(&right.0));
    json!({
        "horizon_minutes": config.horizon_minutes,
        "window_days": config.window_days,
        "threshold": config.threshold,
        "signal_horizon_days": config.signal_horizon_days,
        "bars_per_session": config.bars_per_session,
        "session_minutes_by_exchange": JsonValue::Object(by_exchange.into_iter().collect()),
    })
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
    let horizon_minutes = yaml_i64(&state.config, &["strategy", "markov", "horizon_minutes"])
        .unwrap_or(1440)
        .max(1);
    let session_minutes = yaml_i64(&state.config, &["strategy", "markov", "session_minutes"])
        .unwrap_or(DEFAULT_SESSION_MINUTES)
        .max(1);
    let session_minutes_by_exchange = yaml_at(
        &state.config,
        &["strategy", "markov", "session_minutes_by_exchange"],
    )
    .and_then(|value| value.as_mapping().cloned())
    .map(|mapping| {
        mapping
            .into_iter()
            .filter_map(|(key, value)| {
                let exchange = key.as_str()?.trim().to_ascii_lowercase();
                let minutes = value.as_i64().filter(|minutes| *minutes > 0)?;
                (!exchange.is_empty()).then_some((exchange, minutes))
            })
            .collect::<HashMap<_, _>>()
    })
    .unwrap_or_default();
    let signal_horizon_days = yaml_i64(
        &state.config,
        &["strategy", "markov", "signal_horizon_days"],
    )
    .unwrap_or(5)
    .max(1) as usize;
    // Scale the calendar tunings into bar counts. At a daily horizon this is a
    // multiply by one, so the deployed behavior is unchanged.
    // The sample_count floor sizes the chart request, so it has to satisfy
    // whichever exchange needs the most bars, not merely the default session.
    let most_bars_per_session = std::iter::once(session_minutes)
        .chain(session_minutes_by_exchange.values().copied())
        .map(|minutes| bars_per_session(horizon_minutes, minutes))
        .max()
        .unwrap_or(1);
    let bars_per_session = bars_per_session(horizon_minutes, session_minutes);
    let window_bars = window_days.saturating_mul(bars_per_session).max(1);
    let min_labeled_bars = min_labeled_days.saturating_mul(bars_per_session).max(1);
    let signal_horizon_bars = signal_horizon_days.saturating_mul(bars_per_session).max(1);
    let mut run_slots = markov_intraday_runs(&state.config, timezone);
    if !run_slots
        .iter()
        .any(|slot| slot.local_time == daily_time && slot.timezone == timezone)
    {
        run_slots.push(MarkovRunSlot {
            name: "nightly".to_string(),
            local_time: daily_time,
            timezone,
        });
    }
    run_slots.sort_by_key(|slot| slot.local_time);
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
        horizon_minutes,
        // The floor has to be expressed in bars too, otherwise an intraday
        // horizon would request fewer samples than it needs to label anything.
        sample_count: yaml_i64(&state.config, &["strategy", "markov", "sample_count"])
            .unwrap_or(520)
            .max(((window_days + min_labeled_days) * most_bars_per_session + 2) as i64)
            as usize,
        min_labeled_days,
        signal_horizon_days,
        forecast_steps,
        session_minutes,
        session_minutes_by_exchange,
        bars_per_session,
        window_bars,
        min_labeled_bars,
        signal_horizon_bars,
        run_slots,
        max_symbols: yaml_i64(&state.config, &["strategy", "markov", "max_symbols"])
            .unwrap_or(0)
            .max(0) as usize,
        instrument_negative_cache_retry_days: instrument_negative_cache_retry_days(state),
        symbol_aliases: analysis_symbol_aliases(&state.config),
    }
}

/// Parse `strategy.markov.intraday_runs` into ordered, de-duplicated slots.
///
/// Entries without a parsable `local_time` are dropped rather than defaulted,
/// so a typo cannot silently move a refresh to midnight.
fn markov_intraday_runs(config: &serde_yaml::Value, default_timezone: Tz) -> Vec<MarkovRunSlot> {
    let Some(entries) = yaml_at(config, &["strategy", "markov", "intraday_runs"])
        .and_then(|value| value.as_sequence().cloned())
    else {
        return Vec::new();
    };
    let mut slots: Vec<MarkovRunSlot> = Vec::new();
    for entry in entries {
        if !entry
            .get("enabled")
            .and_then(serde_yaml::Value::as_bool)
            .unwrap_or(true)
        {
            continue;
        }
        let Some(local_time) = entry
            .get("local_time")
            .and_then(serde_yaml::Value::as_str)
            .and_then(parse_hh_mm)
        else {
            continue;
        };
        let name = entry
            .get("name")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("intraday")
            .to_string();
        let timezone = entry
            .get("time_zone")
            .and_then(serde_yaml::Value::as_str)
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(default_timezone);
        if slots
            .iter()
            .any(|slot| slot.local_time == local_time && slot.timezone == timezone)
        {
            continue;
        }
        slots.push(MarkovRunSlot {
            name,
            local_time,
            timezone,
        });
    }
    slots.sort_by_key(|slot| slot.local_time);
    slots
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
        "session_minutes": config.session_minutes,
        "bars_per_session": config.bars_per_session,
        "window_bars": config.window_bars,
        "min_labeled_bars": config.min_labeled_bars,
        "signal_horizon_bars": config.signal_horizon_bars,
        "forecast_steps": config.forecast_steps,
        "run_slots": config
            .run_slots
            .iter()
            .map(|slot| json!({
                "name": slot.name,
                "local_time": slot.local_time.format("%H:%M").to_string(),
                "time_zone": slot.timezone.name()
            }))
            .collect::<Vec<_>>(),
        "max_symbols": config.max_symbols,
        "instrument_negative_cache_retry_days": config.instrument_negative_cache_retry_days,
        "symbol_aliases": config.symbol_aliases
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
    crate::saxo_http::openapi_base_url(&environment)
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

fn requested_symbol(parts: &SymbolParts) -> String {
    if parts.exchange.is_empty() {
        parts.base.clone()
    } else {
        format!("{}:{}", parts.base, parts.exchange)
    }
}

/// Expand a share-class base into the spellings Saxo actually uses.
///
/// Saxo is not internally consistent about Nordic share classes: some are
/// suffixed with a bare lowercase letter (`ERICb`, `ASSAb`, `VOLVb`) and others
/// with an underscore (`ESSITY_B`, `SWED_A`, `NDA_SE`). Both were verified
/// against SIM on 2026-08-02. Emitting both spellings costs one extra lookup
/// attempt on a miss and avoids pinning either convention into configuration,
/// where it would have to be maintained per symbol.
///
/// The separator is also accepted in either form, so a base already written as
/// `ESSITY_B` expands to `ESSITYb` and vice versa.
fn base_lookup_variants(base: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let trimmed = base.trim();
    push_unique_string(&mut variants, trimmed.to_uppercase());
    let Some((prefix, share_class)) = trimmed.split_once('-').or_else(|| trimmed.split_once('_'))
    else {
        return variants;
    };
    let prefix = prefix.trim();
    let share_class = share_class.trim();
    if prefix.is_empty() || share_class.is_empty() || !share_class.is_ascii() {
        return variants;
    }
    if share_class.len() == 1 {
        let class = share_class.chars().next().unwrap().to_ascii_lowercase();
        push_unique_string(&mut variants, format!("{}{}", prefix.to_uppercase(), class));
    }
    push_unique_string(
        &mut variants,
        format!("{}_{}", prefix.to_uppercase(), share_class.to_uppercase()),
    );
    variants
}

fn symbol_lookup_variants(parts: &SymbolParts) -> Vec<String> {
    base_lookup_variants(&parts.base)
        .into_iter()
        .map(|base| {
            if parts.exchange.is_empty() {
                base
            } else {
                format!("{base}:{}", parts.exchange)
            }
        })
        .collect()
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn push_lookup_attempt(
    attempts: &mut Vec<(String, Vec<(&'static str, String)>, bool)>,
    seen: &mut HashSet<String>,
    method: &str,
    params: Vec<(&'static str, String)>,
    require_symbol_match: bool,
) {
    let signature = format!("{require_symbol_match}:{params:?}");
    if seen.insert(signature) {
        attempts.push((method.to_string(), params, require_symbol_match));
    }
}

/// Saxo's `ExchangeId` is a proprietary code, **not** the ISO MIC.
///
/// This mapping previously returned the MIC for every entry, which meant the
/// exchange-scoped fallback in `lookup_instrument` could never match anything —
/// `ExchangeId=XSTO` returns an empty result set where `SSE` returns Stockholm.
/// All fifteen entries were wrong, so the fallback had never resolved a single
/// instrument; the symbols that do resolve succeed on the earlier keyword
/// attempt and never reach here. Values below were read from `/ref/v1/exchanges`
/// against SIM on 2026-08-02 (see `wiki/urgent-todo.md` U11).
///
/// Note Stockholm's MIC in Saxo's own reference data is `XOME`, not `XSTO`, and
/// the canonical symbol suffix follows it (`ERICb:xome`). Both spellings map
/// here so a stray `xsto` from older data still resolves.
fn exchange_id_for_suffix(exchange: &str) -> &'static str {
    match exchange.to_lowercase().as_str() {
        "xnas" => "NASDAQ",
        "xnys" => "NYSE",
        "arcx" => "NYSE_ARCA",
        "xcse" => "CSE",
        "xome" | "xsto" => "SSE",
        "xosl" => "OSE",
        "xhel" => "HSE",
        "xlon" => "LSE_SETS",
        "xetr" => "FSE",
        "xfra" => "FFT",
        "xmil" => "MIL",
        "xpar" => "PAR",
        "xams" => "AMS",
        "xbru" => "BRU",
        "xlse" | "xlis" => "LISB",
        _ => "",
    }
}

fn exchange_aliases(exchange: &str) -> Vec<&'static str> {
    match exchange.to_lowercase().as_str() {
        "xnas" => vec!["XNAS", "NASDAQ"],
        "xnys" => vec!["XNYS", "NYSE"],
        "arcx" => vec!["ARCX", "NYSE ARCA"],
        "xcse" => vec!["XCSE", "CSE", "COP"],
        "xsto" | "xome" => vec!["XSTO", "STO", "STK"],
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
    let requested_variants = symbol_lookup_variants(parts)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    let base_variants = base_lookup_variants(&parts.base)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    let exact_symbol = i32::from(
        candidate_symbol == requested_symbol.to_uppercase()
            || requested_variants.contains(&candidate_symbol),
    );
    let exchange_match = i32::from(
        exchange_aliases(&parts.exchange)
            .iter()
            .any(|alias| candidate_exchange == *alias)
            || candidate_symbol.ends_with(&format!(":{}", parts.exchange.to_uppercase())),
    );
    let exact_base = i32::from(
        candidate_symbol
            .split(':')
            .next()
            .is_some_and(|base| base == parts.base || base_variants.contains(base)),
    );
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
    let requested_variants = symbol_lookup_variants(parts)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    if candidate_symbol == requested_symbol.to_uppercase()
        || requested_variants.contains(&candidate_symbol)
    {
        return true;
    }
    let base_variants = base_lookup_variants(&parts.base)
        .into_iter()
        .map(|value| value.to_uppercase())
        .collect::<HashSet<_>>();
    if candidate_base != parts.base && !base_variants.contains(candidate_base) {
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
            bars_per_session INTEGER,
            window_bars INTEGER,
            min_labeled_bars INTEGER,
            signal_horizon_bars INTEGER,
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
        "CREATE TABLE IF NOT EXISTS saxo_instrument_negative_cache (
            symbol TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            last_error_at TEXT NOT NULL,
            retry_after TEXT NOT NULL,
            error_text TEXT NOT NULL,
            attempt_count INTEGER NOT NULL DEFAULT 1
        )",
        "CREATE INDEX IF NOT EXISTS idx_saxo_instrument_negative_cache_retry ON saxo_instrument_negative_cache(retry_after)",
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
            instrument_negative_cache_retry_days: DEFAULT_INSTRUMENT_NEGATIVE_CACHE_RETRY_DAYS,
            symbol_aliases: HashMap::new(),
            session_minutes: DEFAULT_SESSION_MINUTES,
            session_minutes_by_exchange: HashMap::new(),
            bars_per_session: 1,
            window_bars: 2,
            min_labeled_bars: 3,
            signal_horizon_bars: 2,
            run_slots: vec![MarkovRunSlot {
                name: "nightly".to_string(),
                local_time: NaiveTime::from_hms_opt(23, 30, 0).unwrap(),
                timezone: chrono_tz::Europe::Copenhagen,
            }],
        }
    }

    /// Reproduces the real shape observed in production `markov_signal_runs`:
    /// `summary_json.signals` embeds up to 20 full signal rows, each carrying
    /// `raw_payload_json.recent_labels` -- one entry per trading day in the
    /// window, 62 in production. This was the actual source of the prompt
    /// bloat `wiki/urgent-todo.md` U12 measured (527 KB average by July 2026):
    /// `compact_markov_context`'s own `signals` list was already trimmed, but
    /// embedding this raw run row verbatim as `latest_run` defeated it.
    fn production_shaped_markov_run(
        embedded_signal_count: usize,
        labels_per_signal: usize,
    ) -> JsonValue {
        let recent_label = json!({
            "close": 325.05,
            "regime": "Sideways",
            "rolling_return": 0.0318,
            "time": "2026-05-06T00:00:00Z"
        });
        let embedded_signal = json!({
            "id": "markov-1785533673049411-v-xnys",
            "symbol": "V:xnys",
            "instrument_name": "Visa Inc.",
            "current_state": "Sideways",
            "bull_prob": 0.163,
            "bear_prob": 0.113,
            "conviction": 0.05,
            "forecasts_json": [
                {"days": 1, "distribution": {"Bear": 0.05, "Bull": 0.05, "Sideways": 0.9}, "signed_signal": 0.0},
                {"days": 5, "distribution": {"Bear": 0.11, "Bull": 0.16, "Sideways": 0.72}, "signed_signal": 0.05}
            ],
            "raw_payload_json": {
                "method": "observable_markov_rolling_return",
                "label_counts": {"Bear": 53, "Bull": 136, "Sideways": 311},
                "recent_labels": vec![recent_label; labels_per_signal]
            }
        });
        json!({
            "id": "markov-1785533673049411",
            "created_at": "2026-07-31T21:34:33Z",
            "run_date": "2026-07-31",
            "status": "completed",
            "asset_count": 201,
            "success_count": 173,
            "error_count": 28,
            "config_json": {"window_days": 20, "threshold": 0.05},
            "summary_json": {
                "status": "completed",
                "run_id": "markov-1785533673049411",
                "run_date": "2026-07-31",
                "asset_count": 201,
                "success_count": 173,
                "error_count": 28,
                "config": {"window_days": 20, "threshold": 0.05},
                "signals": vec![embedded_signal; embedded_signal_count]
            }
        })
    }

    #[test]
    fn trimming_the_markov_run_removes_the_duplicated_signal_payload() {
        let run = production_shaped_markov_run(20, 62);
        let untrimmed_bytes = serde_json::to_string(&run).unwrap().len();

        let trimmed = trim_markov_run_for_prompt(&run);
        let trimmed_bytes = serde_json::to_string(&trimmed).unwrap().len();

        assert!(
            trimmed["summary_json"].get("signals").is_none(),
            "the embedded per-signal payload must be gone"
        );
        assert_eq!(
            trimmed["summary_json"]["status"], "completed",
            "run-level metadata inside summary_json must survive"
        );
        assert_eq!(trimmed["summary_json"]["success_count"], 173);
        assert_eq!(
            trimmed["summary_json"]["config"]["window_days"], 20,
            "config, needed to judge signal freshness, must survive"
        );
        assert_eq!(trimmed["status"], "completed");
        assert_eq!(trimmed["asset_count"], 201);
        assert!(
            trimmed_bytes < untrimmed_bytes / 5,
            "expected at least an 80% reduction on the production-shaped fixture, \
             got {untrimmed_bytes} -> {trimmed_bytes}"
        );
    }

    #[test]
    fn trimming_a_run_with_no_embedded_signals_is_a_harmless_no_op() {
        let run = production_shaped_markov_run(0, 0);
        let trimmed = trim_markov_run_for_prompt(&run);
        assert_eq!(trimmed["status"], "completed");
        assert!(
            trimmed["summary_json"].get("signals").is_none(),
            "the key is removed outright, not replaced with an empty array"
        );
    }

    #[test]
    fn trimming_tolerates_missing_or_malformed_run_data() {
        assert_eq!(
            trim_markov_run_for_prompt(&JsonValue::Null),
            JsonValue::Null
        );
        assert_eq!(
            trim_markov_run_for_prompt(&json!("not an object")),
            json!("not an object")
        );
        // No summary_json at all -- must not panic, must pass the rest through.
        let sparse = json!({"status": "empty"});
        assert_eq!(trim_markov_run_for_prompt(&sparse), sparse);
        // summary_json present but not an object.
        let malformed = json!({"status": "ok", "summary_json": "not-an-object"});
        assert_eq!(trim_markov_run_for_prompt(&malformed), malformed);
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
        let config = test_config();
        let scaling = MarkovScaling::for_exchange(&config, "xome");
        let analysis = analyze_bars(&bars, &config, &scaling).unwrap();
        assert_eq!(analysis.current_state, Regime::Bull);
        assert_eq!(analysis.direction, "long");
        assert!(analysis.conviction >= 0.0);
    }

    #[test]
    fn classifies_only_no_tradable_match_as_negative_cacheable() {
        assert!(is_negative_cacheable_lookup_error(&anyhow!(
            "{NO_TRADABLE_INSTRUMENT_PREFIX} ABB:xsto"
        )));
        assert!(!is_negative_cacheable_lookup_error(&anyhow!(
            "HTTP 429 from Saxo reference lookup"
        )));
    }

    #[test]
    fn share_class_symbol_variants_match_saxo_compact_symbols() {
        let requested = symbol_parts("CARL-B:xcse");
        assert_eq!(
            symbol_lookup_variants(&requested),
            vec![
                "CARL-B:xcse".to_string(),
                "CARLb:xcse".to_string(),
                "CARL_B:xcse".to_string()
            ]
        );
        let compact = json!({
            "Symbol": "CARLb:xcse",
            "ExchangeId": "XCSE",
            "TradableAs": ["Stock"]
        });
        let wrong_exchange = json!({
            "Symbol": "CARLb:xsto",
            "ExchangeId": "XSTO",
            "TradableAs": ["Stock"]
        });

        assert!(candidate_matches_requested(
            &compact,
            "CARL-B:xcse",
            &requested
        ));
        assert!(!candidate_matches_requested(
            &wrong_exchange,
            "CARL-B:xcse",
            &requested
        ));
    }

    #[test]
    fn supports_nyse_arca_instrument_resolution() {
        let requested = symbol_parts("SPY:arcx");
        let candidate = json!({
            "Symbol": "SPY:arcx",
            "ExchangeId": "ARCX",
            "TradableAs": ["Etf"]
        });

        assert_eq!(exchange_id_for_suffix("arcx"), "NYSE_ARCA");
        assert!(candidate_matches_requested(
            &candidate, "SPY:arcx", &requested
        ));
    }

    #[test]
    fn an_unknown_or_hostile_filter_falls_back_to_all() {
        assert_eq!(normalize_markov_filter(None), "all");
        assert_eq!(normalize_markov_filter(Some("")), "all");
        assert_eq!(normalize_markov_filter(Some("nonsense")), "all");
        // The value reaches SQL, so anything not on the allowlist must be
        // rejected outright rather than sanitized and passed through.
        assert_eq!(
            normalize_markov_filter(Some("all'; DROP TABLE markov_asset_signals; --")),
            "all"
        );
        assert_eq!(normalize_markov_filter(Some("  PORTFOLIO  ")), "portfolio");
        for key in MARKOV_FILTERS {
            assert_eq!(&normalize_markov_filter(Some(key)), key);
        }
    }

    /// The page query and the count query must apply an identical predicate.
    /// If they diverge, the pagination control advertises pages that render
    /// empty -- worse than shipping no filter at all.
    #[test]
    fn every_filter_produces_one_predicate_shared_by_page_and_count() {
        for key in MARKOV_FILTERS {
            let predicate = markov_filter_sql(key, 0.15);
            if *key == "all" {
                assert!(predicate.is_empty(), "all must not constrain the run");
                continue;
            }
            assert!(
                predicate.starts_with(" AND "),
                "{key} must extend the shared WHERE rather than replace it, got {predicate}"
            );
            assert!(
                !predicate.contains(';'),
                "{key} must not be able to terminate the statement"
            );
        }
    }

    #[test]
    fn portfolio_and_watchlist_are_exact_complements() {
        let held = markov_filter_sql("portfolio", 0.0);
        let not_held = markov_filter_sql("watchlist", 0.0);
        assert!(held.contains(" IN ("), "got {held}");
        assert!(not_held.contains(" NOT IN ("), "got {not_held}");
        // Case-insensitive on both sides: Saxo returns `DB1Gn:xetr` where the
        // configured universe carries `db1gn:xetr`, and an exact match would
        // silently classify a held position as watchlist.
        assert!(held.contains("UPPER(symbol) IN"), "got {held}");
        assert!(held.contains("SELECT UPPER(symbol)"), "got {held}");
        assert_eq!(
            held.replace(" IN (", " NOT IN ("),
            not_held,
            "the two filters must partition the run with no gap or overlap"
        );
    }

    /// "High conviction" is defined as "would clear the Trading Manager's
    /// Markov gate", not an arbitrary display cutoff, so it must carry the
    /// configured threshold through.
    #[test]
    fn the_conviction_filter_uses_the_configured_gate_threshold() {
        let predicate = markov_filter_sql("conviction", 0.15);
        assert!(
            predicate.contains("ABS(signed_signal) >= 0.15"),
            "got {predicate}"
        );
        assert!(
            predicate.contains("status = 'ok'"),
            "an errored row has no usable signal to be convicted about, got {predicate}"
        );
        // A missing or nonsensical threshold must not become a negative bound
        // that admits every row including errors.
        for bad in [0.0, -1.0, f64::NAN] {
            let fallback = markov_filter_sql("conviction", bad);
            assert!(
                fallback.contains("ABS(signed_signal) >= 0"),
                "threshold {bad} must clamp to 0, got {fallback}"
            );
        }
    }

    /// The exchange-scoped fallback in `lookup_instrument` passes this value to
    /// Saxo as `ExchangeId`. Saxo uses proprietary codes, not ISO MICs, so
    /// returning the MIC made the fallback silently match nothing — verified
    /// live: `ExchangeId=XSTO` returns an empty set, `SSE` returns Stockholm.
    /// Values pinned from `/ref/v1/exchanges` on 2026-08-02.
    #[test]
    fn exchange_ids_are_saxo_codes_rather_than_iso_mics() {
        for (suffix, expected) in [
            ("xnas", "NASDAQ"),
            ("xnys", "NYSE"),
            ("xcse", "CSE"),
            ("xome", "SSE"),
            ("xetr", "FSE"),
            ("xosl", "OSE"),
            ("xhel", "HSE"),
            ("xlon", "LSE_SETS"),
        ] {
            let actual = exchange_id_for_suffix(suffix);
            assert_eq!(actual, expected, "wrong ExchangeId for {suffix}");
            assert_ne!(
                actual.to_lowercase(),
                suffix,
                "{suffix} still returns its own MIC, which Saxo never matches"
            );
        }
        assert_eq!(
            exchange_id_for_suffix("xsto"),
            "SSE",
            "legacy xsto spelling must still resolve"
        );
        assert_eq!(exchange_id_for_suffix("unknown"), "");
    }

    /// Saxo spells Nordic share classes two different ways and we cannot tell
    /// which from the symbol alone, so both must be attempted. `ERICb:xome` and
    /// `ESSITY_B:xome` are both real, both live, and differ only in convention.
    #[test]
    fn share_class_variants_cover_both_saxo_spellings() {
        assert_eq!(
            base_lookup_variants("ERIC-B"),
            vec![
                "ERIC-B".to_string(),
                "ERICb".to_string(),
                "ERIC_B".to_string()
            ]
        );
        assert_eq!(
            base_lookup_variants("ESSITY_B"),
            vec!["ESSITY_B".to_string(), "ESSITYb".to_string()]
        );
        // Two-letter classes have no bare-letter form; only the underscore.
        assert_eq!(
            base_lookup_variants("NDA-SE"),
            vec!["NDA-SE".to_string(), "NDA_SE".to_string()]
        );
        // A plain symbol must not sprout variants.
        assert_eq!(base_lookup_variants("TELIA"), vec!["TELIA".to_string()]);
    }

    /// Guards the config correction: these symbols were unresolvable for weeks
    /// because the universe carried Stockholm as `:xsto`, which Saxo does not
    /// use. A candidate returned by Saxo must match the corrected form.
    #[test]
    fn stockholm_symbols_resolve_under_the_xome_suffix() {
        let requested = symbol_parts("ERIC-B:xome");
        let candidate = json!({
            "Symbol": "ERICb:xome",
            "ExchangeId": "SSE",
            "TradableAs": ["Stock"]
        });
        assert!(candidate_matches_requested(
            &candidate,
            "ERIC-B:xome",
            &requested
        ));

        let underscored = symbol_parts("ESSITY-B:xome");
        let underscored_candidate = json!({
            "Symbol": "ESSITY_B:xome",
            "ExchangeId": "SSE",
            "TradableAs": ["Stock"]
        });
        assert!(candidate_matches_requested(
            &underscored_candidate,
            "ESSITY-B:xome",
            &underscored
        ));
    }

    #[test]
    fn analysis_symbol_alias_keeps_original_asset_symbol() {
        let mut aliases = HashMap::new();
        aliases.insert("cost:xnys".to_string(), "COST:xnas".to_string());
        let mut assets = Vec::new();
        let mut seen = HashSet::new();
        push_asset(
            &mut assets,
            &mut seen,
            &json!({
                "symbol": "COST:xnys",
                "instrument_name": "Costco Wholesale Corp."
            }),
            "watchlist",
            &aliases,
        );

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].symbol, "COST:xnys");
        assert_eq!(assets[0].analysis_symbol, "COST:xnas");
        assert_eq!(assets[0].source, "watchlist");
    }

    #[test]
    fn normalized_symbol_key_is_case_insensitive_and_trimmed() {
        assert_eq!(normalized_symbol_key(" cost:XNYS "), "cost:xnys");
    }

    #[test]
    fn a_daily_horizon_leaves_every_tuning_in_calendar_units() {
        // The deployed config is daily. Scaling must be a no-op there, or this
        // refactor would silently change live regime labelling.
        assert_eq!(bars_per_session(1440, 510), 1);
        assert_eq!(bars_per_session(10080, 510), 1);
    }

    #[test]
    fn an_intraday_horizon_scales_a_window_back_up_to_calendar_days() {
        // 510-minute session at hourly bars is 9 bars per day, so a 20-day
        // window has to span 180 bars to still mean twenty days.
        assert_eq!(bars_per_session(60, 510), 9);
        assert_eq!(20 * bars_per_session(60, 510), 180);
        assert_eq!(bars_per_session(30, 510), 17);
        assert_eq!(bars_per_session(240, 510), 3);
    }

    #[test]
    fn an_unscaled_intraday_window_would_collapse_the_regime_signal() {
        // Guards the bug this refactor exists to prevent: at hourly bars an
        // unscaled 20-"day" window covers ~2 sessions, so a 5% threshold never
        // trips and every bar labels Sideways.
        let bars = (0..400)
            .map(|index| ChartBar {
                time: format!("2026-01-01T{:02}:00:00Z", index % 24),
                close: 100.0 * (1.0 + 0.0008 * index as f64),
            })
            .collect::<Vec<_>>();
        let unscaled = label_regimes(&bars, 20, 0.05);
        assert!(
            unscaled
                .iter()
                .all(|point| point.regime == Regime::Sideways),
            "unscaled intraday window should see no trend at all"
        );
        let scaled = label_regimes(&bars, 20 * bars_per_session(60, 510), 0.05);
        assert!(
            scaled.iter().any(|point| point.regime == Regime::Bull),
            "scaled window should recover the underlying uptrend"
        );
    }

    fn slot(name: &str, hour: u32, minute: u32) -> MarkovRunSlot {
        slot_in(name, hour, minute, chrono_tz::Europe::Copenhagen)
    }

    fn slot_in(name: &str, hour: u32, minute: u32, timezone: Tz) -> MarkovRunSlot {
        MarkovRunSlot {
            name: name.to_string(),
            local_time: NaiveTime::from_hms_opt(hour, minute, 0).unwrap(),
            timezone,
        }
    }

    fn copenhagen(date: NaiveDate, hour: u32, minute: u32) -> DateTime<Utc> {
        chrono_tz::Europe::Copenhagen
            .from_local_datetime(&date.and_time(NaiveTime::from_hms_opt(hour, minute, 0).unwrap()))
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn the_due_slot_is_the_latest_one_already_passed() {
        let mut config = test_config();
        config.run_slots = vec![
            slot("europe_open", 10, 15),
            slot("us_open", 16, 30),
            slot("nightly", 23, 30),
        ];
        let date = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();

        assert!(due_markov_slot(&config, date, copenhagen(date, 9, 0)).is_none());
        for (hour, minute, expected) in [
            (10, 15, "europe_open"),
            (16, 29, "europe_open"),
            (16, 30, "us_open"),
            (23, 29, "us_open"),
            (23, 30, "nightly"),
        ] {
            let due = due_markov_slot(&config, date, copenhagen(date, hour, minute))
                .expect("a slot is due");
            assert_eq!(due.name, expected, "at {hour:02}:{minute:02}");
        }
    }

    #[test]
    fn a_slot_anchored_to_new_york_tracks_that_zone_not_copenhagen() {
        // The US decision report is anchored to America/New_York. In mid-March
        // the US has moved to daylight saving and Europe has not, so 10:15 in
        // New York is 15:15 in Copenhagen rather than the usual 16:15. A slot
        // that has to stay ahead of that report must move with it.
        let mut config = test_config();
        config.run_slots = vec![slot_in("us_open", 10, 15, chrono_tz::America::New_York)];
        let march = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();

        assert!(
            due_markov_slot(&config, march, copenhagen(march, 15, 0)).is_none(),
            "not yet due at 15:00 Copenhagen"
        );
        assert!(
            due_markov_slot(&config, march, copenhagen(march, 15, 20)).is_some(),
            "due at 15:20 Copenhagen, which is 10:20 in New York"
        );

        // In August both zones observe DST, so the same slot is an hour later.
        let august = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        assert!(due_markov_slot(&config, august, copenhagen(august, 16, 0)).is_none());
        assert!(due_markov_slot(&config, august, copenhagen(august, 16, 20)).is_some());
    }

    #[test]
    fn intraday_slots_drop_bad_times_and_keep_one_entry_per_time() {
        let config: serde_yaml::Value = serde_yaml::from_str(
            r#"
strategy:
  markov:
    intraday_runs:
      - name: us_open
        local_time: "15:00"
      - name: europe_open
        local_time: "10:15"
      - name: duplicate
        local_time: "10:15"
      - name: typo
        local_time: "not a time"
      - name: disabled
        local_time: "12:00"
        enabled: false
"#,
        )
        .unwrap();
        let slots = markov_intraday_runs(&config, chrono_tz::Europe::Copenhagen);
        assert_eq!(
            slots.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["europe_open", "us_open"],
            "sorted, de-duplicated, typo and disabled entries dropped"
        );
    }

    /// Regression for 2026-09-01: per-exchange scaling was computed, placed in
    /// the row map, and then dropped at insert time because the INSERT names
    /// its columns explicitly and nobody added these four. The sweep ran, the
    /// signals looked fine, and every persisted `bars_per_session` was NULL --
    /// so the claim that a signal records the scaling applied to it was false
    /// for a full day. A round trip is the only check that catches it.
    #[tokio::test]
    async fn a_persisted_signal_carries_the_scaling_it_was_computed_under() {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory markov database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create markov tables");
        }
        let state = AppState {
            config_path: std::path::PathBuf::from("markov-test.yaml"),
            config: serde_yaml::from_str("{}").expect("parse test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        };

        let config = test_config();
        let asset = MarkovAsset {
            symbol: "PLTR:xnas".to_string(),
            analysis_symbol: "PLTR:xnas".to_string(),
            instrument_name: "Palantir".to_string(),
            source: "watchlist".to_string(),
        };
        // Deliberately not the default: a US session is shorter, and the whole
        // point is that the row records what this instrument actually used.
        let scaling = MarkovScaling {
            bars_per_session: 7,
            window_bars: 140,
            min_labeled_bars: 420,
            signal_horizon_bars: 21,
            forecast_step_bars: vec![7, 14, 21, 35],
        };
        let row = signal_row_json(
            "markov-test-run",
            "2026-09-01T21:35:51Z",
            NaiveDate::from_ymd_opt(2026, 9, 1).expect("valid date"),
            &config,
            &asset,
            None,
            &scaling,
            None,
            None,
            Some("no bars"),
        );
        insert_markov_signal(&state, &row)
            .await
            .expect("insert markov signal");

        let stored = sqlx::query(
            "SELECT bars_per_session, window_bars, min_labeled_bars, signal_horizon_bars
             FROM markov_asset_signals WHERE symbol = 'PLTR:xnas'",
        )
        .fetch_one(&state.pool)
        .await
        .expect("read back the signal");

        assert_eq!(stored.get::<i32, _>("bars_per_session"), 7);
        assert_eq!(stored.get::<i32, _>("window_bars"), 140);
        assert_eq!(stored.get::<i32, _>("min_labeled_bars"), 420);
        assert_eq!(stored.get::<i32, _>("signal_horizon_bars"), 21);
    }

    /// The prompt's Markov block was ordered alphabetically and truncated at 80
    /// of ~200, so the universe was cut off at `GSK` and everything from H to Z
    /// had no regime evidence in front of the model. Two properties replace it:
    /// a held position is never dropped, and the remainder ranks by conviction
    /// rather than by ticker.
    #[tokio::test]
    async fn the_prompt_markov_block_pins_holdings_then_ranks_by_conviction() {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create tables");
        }
        sqlx::query(
            "CREATE TABLE portfolio_position_snapshots (recorded_at TEXT NOT NULL, symbol TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create positions");
        sqlx::query(
            "INSERT INTO markov_signal_runs (id, created_at, run_date, status, asset_count, \
             success_count, error_count, config_json, summary_json) \
             VALUES ('r1', '2026-09-03T21:35:00Z', '2026-09-03', 'completed', 4, 4, 0, '{}', '{}')",
        )
        .execute(&pool)
        .await
        .expect("insert run");
        // ORCL sorts late and has the weakest signal; it is the held one.
        for (symbol, conviction) in [
            ("AAPL:xnas", 0.05),
            ("BMW:xetr", 0.60),
            ("ORCL:xnys", 0.01),
            ("TSLA:xnas", 0.40),
        ] {
            sqlx::query(&format!(
                "INSERT INTO markov_asset_signals (id, run_id, created_at, run_date, status, symbol, \
                 window_days, threshold, horizon_minutes, sample_count, min_labeled_days, \
                 signal_horizon_days, signed_signal, conviction, direction) \
                 VALUES ('{symbol}', 'r1', '2026-09-03T21:35:00Z', '2026-09-03', 'ok', '{symbol}', \
                 20, 0.05, 60, 900, 60, 5, {conviction}, {conviction}, 'long')"
            ))
            .execute(&pool)
            .await
            .expect("insert signal");
        }
        sqlx::query(
            "INSERT INTO portfolio_position_snapshots (recorded_at, symbol) \
             VALUES ('2026-09-03T20:00:00Z', 'ORCL:xnys')",
        )
        .execute(&pool)
        .await
        .expect("insert holding");
        let state = AppState {
            config_path: std::path::PathBuf::from("markov-test.yaml"),
            config: serde_yaml::from_str("{}").expect("parse test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        };

        let rows = latest_markov_signals_by_conviction(&state, 3)
            .await
            .expect("read conviction-ordered signals");
        let order: Vec<String> = rows
            .iter()
            .map(|row| {
                row.get("symbol")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect();

        assert_eq!(
            order[0], "ORCL:xnys",
            "a held position comes first however weak its signal: {order:?}"
        );
        assert_eq!(order[1], "BMW:xetr", "then the strongest conviction");
        assert_eq!(order[2], "TSLA:xnas");
        assert!(
            !order.contains(&"AAPL:xnas".to_string()),
            "the weakest unheld signal is the one that drops, not the alphabetically last"
        );
    }

    #[tokio::test]
    async fn a_targeted_refresh_does_not_blank_the_other_symbols() {
        // Regression for 2026-08-31 report 257: a one-symbol targeted refresh
        // became the newest run, and the per-symbol lookup was scoped to that
        // run, so the rebuilt preflight reported DE:xnys Markov as unavailable
        // while a fresh 0.1281 signal existed. The refresh is meant to improve
        // one symbol's evidence, never to remove anyone else's.
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory markov database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create markov tables");
        }
        let state = AppState {
            config_path: std::path::PathBuf::from("markov-test.yaml"),
            config: serde_yaml::from_str("{}").expect("parse test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        };
        let insert = |run_id: &str, created_at: &str, symbol: &str, signal: f64| {
            format!(
                "INSERT INTO markov_asset_signals
                 (id, run_id, created_at, run_date, status, symbol, window_days, threshold,
                  horizon_minutes, sample_count, min_labeled_days, signal_horizon_days,
                  signed_signal, direction)
                 VALUES ('{symbol}-{run_id}', '{run_id}', '{created_at}', '2026-08-31', 'ok',
                         '{symbol}', 20, 0.05, 60, 900, 60, 5, {signal}, 'long')"
            )
        };
        // A full pass covering both symbols, then a targeted refresh of one.
        for sql in [
            insert("full", "2026-08-31T14:38:38Z", "DE:xnys", 0.1281),
            insert("full", "2026-08-31T14:38:38Z", "PLTR:xnas", 0.1289),
            // Distinct value so the assertions can tell which row won.
            insert("targeted", "2026-08-31T14:54:18Z", "PLTR:xnas", 0.3000),
        ] {
            sqlx::query(&sql)
                .execute(&state.pool)
                .await
                .expect("insert signal");
        }

        let refreshed = latest_markov_signal_summary(&state, "PLTR:xnas")
            .await
            .expect("read refreshed symbol");
        assert_eq!(
            refreshed.get("signed_signal").and_then(JsonValue::as_f64),
            Some(0.3),
            "the refreshed symbol should resolve to the targeted row, not the full run"
        );

        let untouched = latest_markov_signal_summary(&state, "DE:xnys")
            .await
            .expect("read untouched symbol");
        assert!(
            !untouched.is_null(),
            "a symbol absent from the targeted refresh must keep its last full-run signal"
        );
        assert_eq!(
            untouched.get("status").and_then(JsonValue::as_str),
            Some("ok")
        );
        assert_eq!(
            untouched.get("signed_signal").and_then(JsonValue::as_f64),
            Some(0.1281),
            "and it should still carry its own full-run value"
        );
    }

    #[test]
    fn a_twenty_day_window_is_twenty_days_on_every_exchange() {
        // With one global session length, window_days: 20 meant 20 trading days
        // on a 9-bar exchange and about 26 on a 7-bar one -- silently, for
        // roughly 40% of the universe. Measured against live SIM hourly bars:
        // NASDAQ/NYSE 7 bars a session, Copenhagen/Oslo 8, the rest 9.
        let mut config = test_config();
        config.horizon_minutes = 60;
        config.window_days = 20;
        config.min_labeled_days = 60;
        config.signal_horizon_days = 5;
        config.session_minutes = 510;
        config.session_minutes_by_exchange = HashMap::from([
            ("xnas".to_string(), 390),
            ("xnys".to_string(), 390),
            ("xcse".to_string(), 480),
            ("xosl".to_string(), 480),
        ]);

        for (exchange, expected_bars) in [
            ("xnas", 7),
            ("xnys", 7),
            ("xcse", 8),
            ("xosl", 8),
            ("xetr", 9),
            ("xome", 9),
        ] {
            let scaling = MarkovScaling::for_exchange(&config, exchange);
            assert_eq!(
                scaling.bars_per_session, expected_bars,
                "{exchange} session length"
            );
            assert_eq!(
                scaling.window_bars,
                20 * expected_bars,
                "{exchange} window must span twenty of its own sessions"
            );
            assert_eq!(scaling.signal_horizon_bars, 5 * expected_bars);
            assert_eq!(scaling.min_labeled_bars, 60 * expected_bars);
        }

        // The bug this replaces: a US window scaled at the global nine bars
        // spans 180 bars, which is roughly 26 of its own seven-bar sessions.
        let us = MarkovScaling::for_exchange(&config, "xnas");
        assert_eq!(us.window_bars, 140);
        assert!(
            180 / us.bars_per_session >= 25,
            "the old global scaling stretched a 20-day window past 25 US sessions"
        );

        // An unknown exchange falls back to the default rather than to nothing.
        assert_eq!(
            MarkovScaling::for_exchange(&config, "xnew").bars_per_session,
            9
        );
        assert_eq!(MarkovScaling::for_exchange(&config, "").bars_per_session, 9);
    }

    #[test]
    fn exchange_session_lengths_are_matched_case_insensitively() {
        // Symbols carry a lowercase suffix, but a config written with an
        // uppercase key must not silently fall back to the default.
        let mut config = test_config();
        config.horizon_minutes = 60;
        config.session_minutes = 510;
        config.session_minutes_by_exchange = HashMap::from([("xnas".to_string(), 390)]);
        assert_eq!(
            MarkovScaling::for_exchange(&config, "XNAS").bars_per_session,
            7
        );
        assert_eq!(
            MarkovScaling::for_exchange(&config, "xnas").bars_per_session,
            7
        );
    }
}
