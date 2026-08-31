//! Daily technical indicators computed in Rust from Saxo chart history.
//!
//! This is the data source the BUY/SELL technical gates were always meant to
//! verify against: SMA trend structure, RSI, MACD, and ATR-based reward/risk
//! are computed here once per day for portfolio and top-watchlist symbols,
//! stored in the database, injected into the xAI decision prompt, and read
//! back by the Trading Manager so order approval never depends on
//! model-self-reported confluence counts.

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape},
    markov_method::{
        MarkovAsset, SaxoInstrument, account_key, markov_assets, resolve_instrument, saxo_get_json,
    },
    state::AppState,
};

const DEFAULT_DAILY_TIME: &str = "23:45";
const RSI_PERIOD: usize = 14;
const ATR_PERIOD: usize = 14;
const MACD_FAST: usize = 12;
const MACD_SLOW: usize = 26;
const MACD_SIGNAL: usize = 9;
const RESISTANCE_LOOKBACK: usize = 60;
const SUPPORT_SHORT_WINDOW: usize = 252;
const SUPPORT_LONG_WINDOW: usize = 1_260;
const SUPPORT_PIVOT_RADIUS: usize = 2;

/// The only horizon these indicators are defined for.
pub(crate) const DAILY_HORIZON_MINUTES: i64 = 1440;

/// Force the chart horizon back to daily, warning when configuration asked for
/// anything else.
///
/// Every window in this module is a hardcoded **bar count** named for a daily
/// meaning: `RSI_PERIOD` 14, `ATR_PERIOD` 14, the MACD 12/26/9,
/// `RESISTANCE_LOOKBACK` 60, `SUPPORT_SHORT_WINDOW` 252 ("one trading year")
/// and `SUPPORT_LONG_WINDOW` 1260 ("five trading years"). `horizon_minutes` is
/// configurable and contracted, so an intraday value would silently reinterpret
/// a 14-day RSI as a 14-hour one and a one-year support window as about 28
/// days, with no error and a plausible-looking output. `markov_method` scales
/// its tunings instead, because a Markov regime window has no canonical
/// duration; these indicators do, so the right move is to refuse the horizon
/// rather than rescale a definition.
///
/// Clamping rather than failing: an empty indicator run leaves the technical
/// gate with no evidence, which blocks every BUY. Daily is what the constants
/// already mean, so it is also the correct reading of the intent.
fn daily_horizon_minutes(configured: i64) -> i64 {
    if configured != DAILY_HORIZON_MINUTES {
        warn!(
            configured,
            using = DAILY_HORIZON_MINUTES,
            "daily indicator windows are fixed bar counts defined on daily bars; \
             ignoring the configured horizon"
        );
    }
    DAILY_HORIZON_MINUTES
}

#[derive(Clone, Debug)]
pub(crate) struct IndicatorConfig {
    pub(crate) enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    run_weekdays_only: bool,
    horizon_minutes: i64,
    sample_count: usize,
    max_symbols: usize,
    pub(crate) min_confluences: i64,
    min_reward_risk: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OhlcBar {
    pub(crate) high: f64,
    pub(crate) low: f64,
    pub(crate) close: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndicatorAnalysis {
    pub(crate) close: f64,
    pub(crate) sma20: f64,
    pub(crate) sma50: f64,
    pub(crate) sma200: Option<f64>,
    pub(crate) rsi14: f64,
    pub(crate) macd: f64,
    pub(crate) macd_signal: f64,
    pub(crate) macd_histogram: f64,
    pub(crate) atr14: f64,
    pub(crate) resistance: f64,
    pub(crate) reward_risk: Option<f64>,
    pub(crate) nearest_support: Option<f64>,
    pub(crate) next_support: Option<f64>,
    pub(crate) downside_to_support_pct: Option<f64>,
    pub(crate) downside_after_break_pct: Option<f64>,
    pub(crate) support_break_risk: f64,
    pub(crate) support_break_risk_label: &'static str,
    pub(crate) support_confidence: f64,
    pub(crate) support_history_coverage: f64,
    pub(crate) support_touch_count: i64,
    pub(crate) trend_bias: &'static str,
    pub(crate) sentiment: &'static str,
    pub(crate) bullish_confluences: Vec<&'static str>,
    pub(crate) bearish_confluences: Vec<&'static str>,
}

pub(crate) fn indicator_config(state: &AppState) -> IndicatorConfig {
    let base = &["strategy", "swing", "daily_indicators"];
    let key = |suffix: &'static str| -> [&str; 4] { [base[0], base[1], base[2], suffix] };
    let timezone = yaml_string(&state.config, &key("timezone"))
        .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let daily_time = yaml_string(&state.config, &key("daily_time"))
        .as_deref()
        .and_then(parse_hh_mm)
        .unwrap_or_else(|| parse_hh_mm(DEFAULT_DAILY_TIME).expect("default time is valid"));
    IndicatorConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(true),
        timezone,
        daily_time,
        run_weekdays_only: yaml_bool(&state.config, &key("run_weekdays_only")).unwrap_or(true),
        horizon_minutes: daily_horizon_minutes(
            yaml_i64(&state.config, &key("horizon_minutes")).unwrap_or(DAILY_HORIZON_MINUTES),
        ),
        sample_count: yaml_i64(&state.config, &key("sample_count"))
            .unwrap_or(260)
            .max(60) as usize,
        max_symbols: yaml_i64(&state.config, &key("max_symbols"))
            .unwrap_or(20)
            .max(0) as usize,
        min_confluences: yaml_i64(&state.config, &key("min_confluences"))
            .unwrap_or(3)
            .max(1),
        min_reward_risk: yaml_f64(&state.config, &key("min_reward_risk"))
            .unwrap_or(2.0)
            .max(0.0),
    }
}

pub(crate) fn indicator_config_json_for_state(state: &AppState) -> JsonValue {
    indicator_config_json(&indicator_config(state))
}

fn indicator_config_json(config: &IndicatorConfig) -> JsonValue {
    json!({
        "enabled": config.enabled,
        "sample_count": config.sample_count,
        "horizon_minutes": config.horizon_minutes,
        "max_symbols": config.max_symbols,
        "min_confluences": config.min_confluences,
        "min_reward_risk": config.min_reward_risk,
        "daily_time": config.daily_time.format("%H:%M").to_string(),
        "timezone": config.timezone.name(),
        "run_weekdays_only": config.run_weekdays_only,
    })
}

fn parse_hh_mm(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M").ok()
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS daily_indicator_runs (
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
        "CREATE TABLE IF NOT EXISTS daily_indicator_signals (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            run_date TEXT NOT NULL,
            status TEXT NOT NULL,
            symbol TEXT NOT NULL,
            instrument_name TEXT,
            source TEXT,
            uic BIGINT,
            asset_type TEXT,
            sample_count INTEGER,
            close DOUBLE PRECISION,
            sma20 DOUBLE PRECISION,
            sma50 DOUBLE PRECISION,
            sma200 DOUBLE PRECISION,
            rsi14 DOUBLE PRECISION,
            macd DOUBLE PRECISION,
            macd_signal DOUBLE PRECISION,
            macd_histogram DOUBLE PRECISION,
            atr14 DOUBLE PRECISION,
            resistance DOUBLE PRECISION,
            reward_risk DOUBLE PRECISION,
            nearest_support DOUBLE PRECISION,
            next_support DOUBLE PRECISION,
            downside_to_support_pct DOUBLE PRECISION,
            downside_after_break_pct DOUBLE PRECISION,
            support_break_risk DOUBLE PRECISION,
            support_break_risk_label TEXT,
            support_confidence DOUBLE PRECISION,
            support_history_coverage DOUBLE PRECISION,
            support_touch_count INTEGER,
            trend_bias TEXT,
            sentiment TEXT,
            confluence_count INTEGER,
            min_confluences INTEGER,
            confluences_json TEXT,
            bearish_confluences_json TEXT,
            error_text TEXT
        )",
        "CREATE INDEX IF NOT EXISTS idx_daily_indicator_signals_run ON daily_indicator_signals(run_id, symbol)",
        "CREATE INDEX IF NOT EXISTS idx_daily_indicator_signals_date ON daily_indicator_signals(run_date DESC, symbol)",
    ]
}

/// Operator-triggered run: bypasses the due-time and already-ran gates so
/// indicators can be refreshed on demand. Lookups always use the newest run.
pub async fn run_daily_indicators_now(state: &AppState) -> Result<JsonValue> {
    let config = indicator_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let run_date = Utc::now().with_timezone(&config.timezone).date_naive();
    run_for_date(state, &config, run_date).await
}

pub async fn run_daily_indicators_cycle(state: &AppState) -> Result<JsonValue> {
    let config = indicator_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let now_local = Utc::now().with_timezone(&config.timezone);
    let run_date = now_local.date_naive();
    if config.run_weekdays_only && run_date.weekday().number_from_monday() > 5 {
        return Ok(json!({
            "status": "idle",
            "reason": "weekend",
            "run_date": run_date.to_string()
        }));
    }
    if now_local.time() < config.daily_time {
        return Ok(json!({
            "status": "idle",
            "reason": "not_due",
            "run_date": run_date.to_string(),
            "due_time": config.daily_time.format("%H:%M").to_string()
        }));
    }
    if run_exists(state, run_date).await? {
        return Ok(json!({
            "status": "skipped",
            "reason": "already_ran",
            "run_date": run_date.to_string()
        }));
    }
    run_for_date(state, &config, run_date).await
}

async fn run_for_date(
    state: &AppState,
    config: &IndicatorConfig,
    run_date: NaiveDate,
) -> Result<JsonValue> {
    let session = state
        .ensure_saxo_session_json("daily_indicators")
        .await
        .context("loading Saxo session for daily indicators run")?;
    let assets = markov_assets(state, config.max_symbols).await?;
    let run_id = format!("indicators-{}", Utc::now().timestamp_micros());
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut success_count = 0usize;
    let mut error_count = 0usize;
    let mut ok_rows = Vec::new();

    for asset in &assets {
        let row = match analyze_asset(state, &session, config, asset).await {
            Ok((instrument, sample_count, analysis)) => {
                success_count += 1;
                signal_row_json(
                    &run_id,
                    &created_at,
                    run_date,
                    config,
                    asset,
                    Some(&instrument),
                    sample_count,
                    Some(&analysis),
                    None,
                )
            }
            Err(err) => {
                error_count += 1;
                warn!(symbol = %asset.symbol, "daily indicator analysis failed: {err:#}");
                signal_row_json(
                    &run_id,
                    &created_at,
                    run_date,
                    config,
                    asset,
                    None,
                    0,
                    None,
                    Some(&format!("{err:#}")),
                )
            }
        };
        insert_signal(state, &row).await?;
        if row.get("status").and_then(JsonValue::as_str) == Some("ok") {
            ok_rows.push(row);
        }
    }

    let status = if success_count > 0 {
        "completed"
    } else if assets.is_empty() {
        "empty"
    } else {
        "error"
    };
    let mut summary = json!({
        "status": status,
        "run_id": run_id,
        "run_date": run_date.to_string(),
        "asset_count": assets.len(),
        "success_count": success_count,
        "error_count": error_count,
        "signals": ok_rows.iter().take(20).cloned().collect::<Vec<_>>(),
    });
    // Mature shadow observations only after the locally persisted indicator
    // closes are available. This performs no additional Saxo/provider call and
    // cannot evaluate, queue, or place an order.
    let shadow_outcome_maturity = match state.refresh_shadow_report_outcome_daily_outcomes().await {
        Ok(value) => value,
        Err(err) => {
            warn!("shadow report daily-outcome maturation degraded: {err:#}");
            json!({
                "status": "error",
                "error": "local_shadow_outcome_maturity_unavailable",
            })
        }
    };
    if let Some(summary_object) = summary.as_object_mut() {
        summary_object.insert(
            "shadow_outcome_maturity".to_string(),
            shadow_outcome_maturity,
        );
    }
    insert_run(
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
    info!(run_id, run_date = %run_date, success_count, error_count, "daily indicators run completed");
    Ok(summary)
}

async fn analyze_asset(
    state: &AppState,
    session: &JsonValue,
    config: &IndicatorConfig,
    asset: &MarkovAsset,
) -> Result<(SaxoInstrument, usize, IndicatorAnalysis)> {
    let instrument = resolve_instrument(state, session, &asset.analysis_symbol)
        .await
        .with_context(|| {
            format!(
                "resolving Saxo instrument for {}",
                asset_analysis_label(asset)
            )
        })?;
    let bars = fetch_ohlc_bars(state, session, &instrument, config)
        .await
        .with_context(|| {
            format!(
                "fetching Saxo chart history for {}",
                asset_analysis_label(asset)
            )
        })?;
    let analysis = analyze_bars(&bars, config.min_reward_risk, config.min_confluences)
        .with_context(|| {
            format!(
                "computing daily indicators for {}",
                asset_analysis_label(asset)
            )
        })?;
    Ok((instrument, bars.len(), analysis))
}

fn asset_analysis_label(asset: &MarkovAsset) -> String {
    if asset.analysis_symbol == asset.symbol {
        asset.symbol.clone()
    } else {
        format!("{} via {}", asset.symbol, asset.analysis_symbol)
    }
}

async fn fetch_ohlc_bars(
    state: &AppState,
    session: &JsonValue,
    instrument: &SaxoInstrument,
    config: &IndicatorConfig,
) -> Result<Vec<OhlcBar>> {
    let query = vec![
        ("AccountKey", account_key(state, session)?),
        ("AssetType", instrument.asset_type.clone()),
        ("Uic", instrument.uic.to_string()),
        ("Horizon", config.horizon_minutes.to_string()),
        ("Count", config.sample_count.to_string()),
        ("FieldGroups", "ChartInfo,Data,DisplayAndFormat".to_string()),
    ];
    let payload = saxo_get_json(state, session, "/chart/v3/charts", &query).await?;
    let mut rows = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let close = number_from_keys(&item, &["Close", "ClosePrice", "close", "LastTraded"])?;
            if close <= 0.0 {
                return None;
            }
            let high = number_from_keys(&item, &["High", "high"]).unwrap_or(close);
            let low = number_from_keys(&item, &["Low", "low"]).unwrap_or(close);
            let time = item
                .get("Time")
                .or_else(|| item.get("time"))
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            Some((time, OhlcBar { high, low, close }))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows.dedup_by(|left, right| left.0 == right.0);
    Ok(rows.into_iter().map(|(_, bar)| bar).collect())
}

// ---------------------------------------------------------------------------
// Pure indicator math (unit-tested, no I/O)
// ---------------------------------------------------------------------------

pub(crate) fn sma(closes: &[f64], period: usize) -> Option<f64> {
    if period == 0 || closes.len() < period {
        return None;
    }
    Some(closes[closes.len() - period..].iter().sum::<f64>() / period as f64)
}

fn ema_series(closes: &[f64], period: usize) -> Vec<f64> {
    if period == 0 || closes.is_empty() {
        return Vec::new();
    }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut values = Vec::with_capacity(closes.len());
    let mut ema = closes[0];
    values.push(ema);
    for close in &closes[1..] {
        ema = alpha * close + (1.0 - alpha) * ema;
        values.push(ema);
    }
    values
}

/// Wilder-smoothed RSI.
pub(crate) fn rsi(closes: &[f64], period: usize) -> Option<f64> {
    if period == 0 || closes.len() <= period {
        return None;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for window in closes[..=period].windows(2) {
        let delta = window[1] - window[0];
        if delta >= 0.0 {
            gains += delta;
        } else {
            losses -= delta;
        }
    }
    let mut avg_gain = gains / period as f64;
    let mut avg_loss = losses / period as f64;
    for window in closes[period..].windows(2) {
        let delta = window[1] - window[0];
        let (gain, loss) = if delta >= 0.0 {
            (delta, 0.0)
        } else {
            (0.0, -delta)
        };
        avg_gain = (avg_gain * (period as f64 - 1.0) + gain) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + loss) / period as f64;
    }
    if avg_loss <= f64::EPSILON {
        return Some(100.0);
    }
    let rs = avg_gain / avg_loss;
    Some(100.0 - 100.0 / (1.0 + rs))
}

/// MACD(12,26,9): returns (macd_line, signal_line, histogram).
pub(crate) fn macd(closes: &[f64]) -> Option<(f64, f64, f64)> {
    if closes.len() < MACD_SLOW + MACD_SIGNAL {
        return None;
    }
    let fast = ema_series(closes, MACD_FAST);
    let slow = ema_series(closes, MACD_SLOW);
    let macd_line: Vec<f64> = fast.iter().zip(&slow).map(|(f, s)| f - s).collect();
    let signal = ema_series(&macd_line, MACD_SIGNAL);
    let macd_value = *macd_line.last()?;
    let signal_value = *signal.last()?;
    Some((macd_value, signal_value, macd_value - signal_value))
}

/// Wilder-smoothed Average True Range.
pub(crate) fn atr(bars: &[OhlcBar], period: usize) -> Option<f64> {
    if period == 0 || bars.len() <= period {
        return None;
    }
    let true_ranges: Vec<f64> = bars
        .windows(2)
        .map(|pair| {
            let (prev, current) = (&pair[0], &pair[1]);
            (current.high - current.low)
                .max((current.high - prev.close).abs())
                .max((current.low - prev.close).abs())
        })
        .collect();
    let mut atr = true_ranges[..period].iter().sum::<f64>() / period as f64;
    for tr in &true_ranges[period..] {
        atr = (atr * (period as f64 - 1.0) + tr) / period as f64;
    }
    Some(atr)
}

#[derive(Clone, Debug, PartialEq)]
struct SupportAnalysis {
    nearest_support: Option<f64>,
    next_support: Option<f64>,
    downside_to_support_pct: Option<f64>,
    downside_after_break_pct: Option<f64>,
    break_risk: f64,
    break_risk_label: &'static str,
    confidence: f64,
    history_coverage: f64,
    touch_count: i64,
}

/// Find clustered daily pivot-low zones below the current close. This is
/// intentionally an observational model: its output is persisted for the
/// dashboard, decision context, and Hermes, but never changes a trade gate.
fn support_analysis(
    bars: &[OhlcBar],
    close: f64,
    atr14: f64,
    trend_bias: &str,
    rsi14: f64,
    macd_histogram: f64,
) -> SupportAnalysis {
    let history_coverage = (bars.len() as f64 / SUPPORT_LONG_WINDOW as f64).clamp(0.0, 1.0);
    let short_history_coverage = (bars.len() as f64 / SUPPORT_SHORT_WINDOW as f64).clamp(0.0, 1.0);
    if bars.len() < SUPPORT_PIVOT_RADIUS * 2 + 1 || close <= f64::EPSILON {
        return SupportAnalysis {
            nearest_support: None,
            next_support: None,
            downside_to_support_pct: None,
            downside_after_break_pct: None,
            break_risk: 0.0,
            break_risk_label: "unavailable",
            confidence: 0.0,
            history_coverage,
            touch_count: 0,
        };
    }

    let window_start = bars.len().saturating_sub(SUPPORT_LONG_WINDOW);
    let candidate_lows = (window_start + SUPPORT_PIVOT_RADIUS..bars.len() - SUPPORT_PIVOT_RADIUS)
        .filter_map(|index| {
            let low = bars[index].low;
            let neighborhood = &bars[index - SUPPORT_PIVOT_RADIUS..=index + SUPPORT_PIVOT_RADIUS];
            (low > 0.0 && neighborhood.iter().all(|bar| low <= bar.low)).then_some(low)
        })
        .filter(|low| *low <= close * 1.005)
        .collect::<Vec<_>>();

    let zone_width = (atr14 * 0.75).max(close * 0.01).max(f64::EPSILON);
    let mut zones = Vec::<(f64, i64)>::new();
    for low in candidate_lows {
        if let Some((level, touches)) = zones
            .iter_mut()
            .find(|(level, _)| (low - *level).abs() <= zone_width)
        {
            *level = (*level * *touches as f64 + low) / (*touches as f64 + 1.0);
            *touches += 1;
        } else {
            zones.push((low, 1));
        }
    }
    zones.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let Some((nearest_support, touch_count)) = zones.first().copied() else {
        return SupportAnalysis {
            nearest_support: None,
            next_support: None,
            downside_to_support_pct: None,
            downside_after_break_pct: None,
            break_risk: 0.0,
            break_risk_label: "unavailable",
            confidence: 0.0,
            history_coverage,
            touch_count: 0,
        };
    };
    let next_support = zones.get(1).map(|(level, _)| *level);
    let downside_to_support_pct = Some(((close - nearest_support) / close * 100.0).max(0.0));
    let downside_after_break_pct = next_support.map(|lower_support| {
        ((nearest_support - lower_support) / nearest_support * 100.0).max(0.0)
    });

    let proximity = (1.0 - ((close - nearest_support) / (atr14 * 4.0).max(zone_width)).min(1.0))
        .clamp(0.0, 1.0);
    let momentum_risk = [
        (trend_bias == "bearish", 0.35),
        (rsi14 < 45.0, 0.20),
        (macd_histogram < 0.0, 0.20),
    ]
    .into_iter()
    .filter_map(|(active, weight)| active.then_some(weight))
    .sum::<f64>();
    let break_risk = (momentum_risk + proximity * 0.25).clamp(0.0, 1.0);
    let break_risk_label = if break_risk >= 0.65 {
        "high"
    } else if break_risk >= 0.35 {
        "moderate"
    } else {
        "low"
    };
    let touch_confidence = (touch_count as f64 / 3.0).min(1.0);
    let confidence =
        (short_history_coverage * 0.35 + history_coverage * 0.30 + touch_confidence * 0.35)
            .clamp(0.0, 1.0);
    SupportAnalysis {
        nearest_support: Some(nearest_support),
        next_support,
        downside_to_support_pct,
        downside_after_break_pct,
        break_risk,
        break_risk_label,
        confidence,
        history_coverage,
        touch_count,
    }
}

pub(crate) fn analyze_bars(
    bars: &[OhlcBar],
    min_reward_risk: f64,
    min_confluences: i64,
) -> Result<IndicatorAnalysis> {
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close).collect();
    let close = *closes
        .last()
        .ok_or_else(|| anyhow::anyhow!("no chart bars returned"))?;
    let sma20 = sma(&closes, 20)
        .ok_or_else(|| anyhow::anyhow!("not enough bars for SMA20: {}", closes.len()))?;
    let sma50 = sma(&closes, 50)
        .ok_or_else(|| anyhow::anyhow!("not enough bars for SMA50: {}", closes.len()))?;
    let sma200 = sma(&closes, 200);
    let rsi14 = rsi(&closes, RSI_PERIOD)
        .ok_or_else(|| anyhow::anyhow!("not enough bars for RSI: {}", closes.len()))?;
    let (macd_line, macd_signal, macd_histogram) =
        macd(&closes).ok_or_else(|| anyhow::anyhow!("not enough bars for MACD"))?;
    let atr14 = atr(bars, ATR_PERIOD)
        .ok_or_else(|| anyhow::anyhow!("not enough bars for ATR: {}", bars.len()))?;
    let resistance = bars[bars.len().saturating_sub(RESISTANCE_LOOKBACK)..]
        .iter()
        .map(|bar| bar.high)
        .fold(f64::MIN, f64::max);

    // Reward/risk: target is the recent swing high; risk is a 2*ATR stop.
    // At a fresh high, assume an ATR-paced continuation instead.
    let risk = 2.0 * atr14;
    let reward_risk = if risk > f64::EPSILON {
        let reward = if resistance > close * 1.005 {
            resistance - close
        } else {
            2.0 * atr14
        };
        Some(reward / risk)
    } else {
        None
    };

    let trend_bias = if close > sma50 && sma20 > sma50 {
        "bullish"
    } else if close < sma50 && sma20 < sma50 {
        "bearish"
    } else {
        "neutral"
    };
    let support = support_analysis(bars, close, atr14, trend_bias, rsi14, macd_histogram);

    let mut bullish_confluences = Vec::new();
    if close > sma20 {
        bullish_confluences.push("price_above_sma20");
    }
    if sma20 > sma50 {
        bullish_confluences.push("sma20_above_sma50");
    }
    if sma200.map(|sma200| close > sma200).unwrap_or(false) {
        bullish_confluences.push("price_above_sma200");
    }
    if (50.0..=70.0).contains(&rsi14) {
        bullish_confluences.push("rsi_bullish_zone");
    }
    if macd_line > macd_signal {
        bullish_confluences.push("macd_above_signal");
    }
    if reward_risk.map(|rr| rr >= min_reward_risk).unwrap_or(false) {
        bullish_confluences.push("reward_risk_ok");
    }

    let mut bearish_confluences = Vec::new();
    if close < sma20 {
        bearish_confluences.push("price_below_sma20");
    }
    if sma20 < sma50 {
        bearish_confluences.push("sma20_below_sma50");
    }
    if sma200.map(|sma200| close < sma200).unwrap_or(false) {
        bearish_confluences.push("price_below_sma200");
    }
    if rsi14 < 45.0 {
        bearish_confluences.push("rsi_weak");
    }
    if macd_line < macd_signal {
        bearish_confluences.push("macd_below_signal");
    }

    let min = min_confluences as usize;
    let sentiment = if trend_bias == "bullish" && bullish_confluences.len() >= min {
        if bullish_confluences.len() >= min + 2 {
            "BUY"
        } else {
            "OVERWEIGHT"
        }
    } else if trend_bias == "bearish" && bearish_confluences.len() >= min {
        if bearish_confluences.len() >= min + 2 {
            "SELL"
        } else {
            "UNDERWEIGHT"
        }
    } else {
        "HOLD"
    };

    Ok(IndicatorAnalysis {
        close,
        sma20,
        sma50,
        sma200,
        rsi14,
        macd: macd_line,
        macd_signal,
        macd_histogram,
        atr14,
        resistance,
        reward_risk,
        nearest_support: support.nearest_support,
        next_support: support.next_support,
        downside_to_support_pct: support.downside_to_support_pct,
        downside_after_break_pct: support.downside_after_break_pct,
        support_break_risk: support.break_risk,
        support_break_risk_label: support.break_risk_label,
        support_confidence: support.confidence,
        support_history_coverage: support.history_coverage,
        support_touch_count: support.touch_count,
        trend_bias,
        sentiment,
        bullish_confluences,
        bearish_confluences,
    })
}

// ---------------------------------------------------------------------------
// Persistence and lookups
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn signal_row_json(
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    config: &IndicatorConfig,
    asset: &MarkovAsset,
    instrument: Option<&SaxoInstrument>,
    sample_count: usize,
    analysis: Option<&IndicatorAnalysis>,
    error_text: Option<&str>,
) -> JsonValue {
    let confluence_count = analysis
        .map(|analysis| match analysis.trend_bias {
            "bearish" => analysis.bearish_confluences.len() as i64,
            _ => analysis.bullish_confluences.len() as i64,
        })
        .unwrap_or(0);
    json!({
        "id": format!("{run_id}-{}", asset.symbol.to_lowercase().replace(':', "-")),
        "run_id": run_id,
        "created_at": created_at,
        "run_date": run_date.to_string(),
        "status": if analysis.is_some() { "ok" } else { "error" },
        "symbol": asset.symbol,
        "instrument_name": instrument.map(|i| i.description.clone()).unwrap_or_else(|| asset.instrument_name.clone()),
        "source": asset.source,
        "uic": instrument.map(|i| i.uic),
        "asset_type": instrument.map(|i| i.asset_type.clone()),
        "sample_count": sample_count,
        "close": analysis.map(|a| a.close),
        "sma20": analysis.map(|a| a.sma20),
        "sma50": analysis.map(|a| a.sma50),
        "sma200": analysis.and_then(|a| a.sma200),
        "rsi14": analysis.map(|a| a.rsi14),
        "macd": analysis.map(|a| a.macd),
        "macd_signal": analysis.map(|a| a.macd_signal),
        "macd_histogram": analysis.map(|a| a.macd_histogram),
        "atr14": analysis.map(|a| a.atr14),
        "resistance": analysis.map(|a| a.resistance),
        "reward_risk": analysis.and_then(|a| a.reward_risk),
        "nearest_support": analysis.and_then(|a| a.nearest_support),
        "next_support": analysis.and_then(|a| a.next_support),
        "downside_to_support_pct": analysis.and_then(|a| a.downside_to_support_pct),
        "downside_after_break_pct": analysis.and_then(|a| a.downside_after_break_pct),
        "support_break_risk": analysis.map(|a| a.support_break_risk),
        "support_break_risk_label": analysis.map(|a| a.support_break_risk_label),
        "support_confidence": analysis.map(|a| a.support_confidence),
        "support_history_coverage": analysis.map(|a| a.support_history_coverage),
        "support_touch_count": analysis.map(|a| a.support_touch_count),
        "trend_bias": analysis.map(|a| a.trend_bias),
        "sentiment": analysis.map(|a| a.sentiment),
        "confluence_count": confluence_count,
        "min_confluences": config.min_confluences,
        "confluences_json": analysis.map(|a| json!(a.bullish_confluences)),
        "bearish_confluences_json": analysis.map(|a| json!(a.bearish_confluences)),
        "error_text": error_text,
    })
}

async fn insert_signal(state: &AppState, row: &JsonValue) -> Result<()> {
    let text = |key: &str| -> String {
        row.get(key)
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let optional_number = |key: &str| -> String {
        row.get(key)
            .and_then(JsonValue::as_f64)
            .map(|value| format!("{value}"))
            .unwrap_or_else(|| "NULL".to_string())
    };
    let optional_text = |key: &str| -> String {
        match row.get(key) {
            Some(JsonValue::String(value)) => format!("'{}'", sql_escape(value)),
            Some(JsonValue::Array(_)) | Some(JsonValue::Object(_)) => format!(
                "'{}'",
                sql_escape(&row.get(key).map(|v| v.to_string()).unwrap_or_default())
            ),
            _ => "NULL".to_string(),
        }
    };
    let sql = format!(
        "INSERT INTO daily_indicator_signals (
            id, run_id, created_at, run_date, status, symbol, instrument_name, source,
            uic, asset_type, sample_count, close, sma20, sma50, sma200, rsi14,
            macd, macd_signal, macd_histogram, atr14, resistance, reward_risk,
            nearest_support, next_support, downside_to_support_pct, downside_after_break_pct,
            support_break_risk, support_break_risk_label, support_confidence,
            support_history_coverage, support_touch_count,
            trend_bias, sentiment, confluence_count, min_confluences,
            confluences_json, bearish_confluences_json, error_text
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', '{}', {}, {},
            {}, {}, {}, {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {}, {}, {}, {},
            {}, {}, {}, {},
            {}, {}, {}
        )",
        sql_escape(&text("id")),
        sql_escape(&text("run_id")),
        sql_escape(&text("created_at")),
        sql_escape(&text("run_date")),
        sql_escape(&text("status")),
        sql_escape(&text("symbol")),
        optional_text("instrument_name"),
        optional_text("source"),
        optional_number("uic"),
        optional_text("asset_type"),
        row.get("sample_count")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        optional_number("close"),
        optional_number("sma20"),
        optional_number("sma50"),
        optional_number("sma200"),
        optional_number("rsi14"),
        optional_number("macd"),
        optional_number("macd_signal"),
        optional_number("macd_histogram"),
        optional_number("atr14"),
        optional_number("resistance"),
        optional_number("reward_risk"),
        optional_number("nearest_support"),
        optional_number("next_support"),
        optional_number("downside_to_support_pct"),
        optional_number("downside_after_break_pct"),
        optional_number("support_break_risk"),
        optional_text("support_break_risk_label"),
        optional_number("support_confidence"),
        optional_number("support_history_coverage"),
        row.get("support_touch_count")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        optional_text("trend_bias"),
        optional_text("sentiment"),
        row.get("confluence_count")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        row.get("min_confluences")
            .and_then(JsonValue::as_i64)
            .unwrap_or(3),
        optional_text("confluences_json"),
        optional_text("bearish_confluences_json"),
        optional_text("error_text"),
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_run(
    state: &AppState,
    run_id: &str,
    created_at: &str,
    run_date: NaiveDate,
    status: &str,
    asset_count: usize,
    success_count: usize,
    error_count: usize,
    config: &IndicatorConfig,
    summary: &JsonValue,
) -> Result<()> {
    let config_json = indicator_config_json(config);
    let sql = format!(
        "INSERT INTO daily_indicator_runs (id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json)
         VALUES ('{}', '{}', '{}', '{}', {}, {}, {}, '{}', '{}')",
        sql_escape(run_id),
        sql_escape(created_at),
        run_date,
        sql_escape(status),
        asset_count,
        success_count,
        error_count,
        sql_escape(&config_json.to_string()),
        sql_escape(&summary.to_string()),
    );
    sqlx::query(&sql).execute(&state.pool).await?;
    Ok(())
}

async fn run_exists(state: &AppState, run_date: NaiveDate) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM daily_indicator_runs WHERE run_date = '{run_date}' LIMIT 1"
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

/// Latest stored indicator signal for one symbol, if the most recent run has
/// one. Used by the Trading Manager to verify model claims server-side.
pub(crate) async fn latest_indicator_signal(
    state: &AppState,
    symbol: &str,
) -> Result<Option<JsonValue>> {
    let sql = format!(
        "SELECT run_date, status, close, sma20, sma50, sma200, rsi14, macd, macd_signal,
                macd_histogram, atr14, resistance, reward_risk, nearest_support, next_support,
                downside_to_support_pct, downside_after_break_pct, support_break_risk,
                support_break_risk_label, support_confidence, support_history_coverage,
                support_touch_count, trend_bias, sentiment,
                confluence_count, min_confluences, confluences_json
         FROM daily_indicator_signals
         WHERE symbol = '{}' AND run_id = (
            SELECT id FROM daily_indicator_runs ORDER BY run_date DESC, created_at DESC LIMIT 1
         )
         LIMIT 1",
        sql_escape(symbol)
    );
    let row = sqlx::query(&sql).fetch_optional(&state.pool).await?;
    Ok(row.as_ref().map(row_to_json))
}

/// Compact per-symbol indicator rows from the latest run for the xAI prompt.
pub async fn compact_indicator_context(state: &AppState, limit: i64) -> Result<JsonValue> {
    let run = sqlx::query(
        "SELECT id, run_date, status, asset_count, success_count, error_count
         FROM daily_indicator_runs ORDER BY run_date DESC, created_at DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    let Some(run) = run else {
        return Ok(json!({"latest_run": null, "signals": []}));
    };
    let run = row_to_json(&run);
    let run_id = run
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let rows = sqlx::query(&format!(
        "SELECT symbol, uic, asset_type, run_date, close, sma20, sma50, sma200, rsi14, macd_histogram,
                atr14, reward_risk, nearest_support, next_support, downside_to_support_pct,
                downside_after_break_pct, support_break_risk, support_break_risk_label,
                support_confidence, support_history_coverage, support_touch_count,
                trend_bias, sentiment, confluence_count, min_confluences,
                confluences_json
         FROM daily_indicator_signals
         WHERE run_id = '{}' AND status = 'ok'
         ORDER BY confluence_count DESC, symbol ASC
         LIMIT {}",
        sql_escape(run_id),
        limit.clamp(1, 500)
    ))
    .fetch_all(&state.pool)
    .await?;
    let mut signals = Vec::new();
    for row in rows.iter().map(row_to_json) {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        let currency = symbol
            .split_once(':')
            .and_then(|(_, exchange)| crate::saxo_order::currency_for_exchange(exchange));
        let close_dkk = match (currency, row.get("close").and_then(JsonValue::as_f64)) {
            (Some(currency), Some(close)) => {
                let fx_rate =
                    crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
                Some(close * fx_rate)
            }
            _ => None,
        };
        signals.push(json!({
            "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
            "uic": row.get("uic").cloned().unwrap_or(JsonValue::Null),
            "asset_type": row.get("asset_type").cloned().unwrap_or(JsonValue::Null),
            "close": row.get("close").cloned().unwrap_or(JsonValue::Null),
            "currency": currency,
            "close_dkk": close_dkk,
            "sma20": row.get("sma20").cloned().unwrap_or(JsonValue::Null),
            "sma50": row.get("sma50").cloned().unwrap_or(JsonValue::Null),
            "sma200": row.get("sma200").cloned().unwrap_or(JsonValue::Null),
            "rsi14": row.get("rsi14").cloned().unwrap_or(JsonValue::Null),
            "macd_histogram": row.get("macd_histogram").cloned().unwrap_or(JsonValue::Null),
            "atr14": row.get("atr14").cloned().unwrap_or(JsonValue::Null),
            "reward_risk": row.get("reward_risk").cloned().unwrap_or(JsonValue::Null),
            "support": {
                "nearest_support": row.get("nearest_support").cloned().unwrap_or(JsonValue::Null),
                "next_support": row.get("next_support").cloned().unwrap_or(JsonValue::Null),
                "downside_to_support_pct": row.get("downside_to_support_pct").cloned().unwrap_or(JsonValue::Null),
                "downside_after_break_pct": row.get("downside_after_break_pct").cloned().unwrap_or(JsonValue::Null),
                "break_risk": row.get("support_break_risk").cloned().unwrap_or(JsonValue::Null),
                "break_risk_label": row.get("support_break_risk_label").cloned().unwrap_or(JsonValue::Null),
                "confidence": row.get("support_confidence").cloned().unwrap_or(JsonValue::Null),
                "history_coverage": row.get("support_history_coverage").cloned().unwrap_or(JsonValue::Null),
                "touch_count": row.get("support_touch_count").cloned().unwrap_or(JsonValue::Null),
            },
            "trend_bias": row.get("trend_bias").cloned().unwrap_or(JsonValue::Null),
            "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
            "confluence_count": row.get("confluence_count").cloned().unwrap_or(JsonValue::Null),
            "min_confluences": row.get("min_confluences").cloned().unwrap_or(JsonValue::Null),
            "confluences": row.get("confluences_json")
                .and_then(JsonValue::as_str)
                .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
                .unwrap_or(JsonValue::Null),
        }));
    }
    Ok(json!({"latest_run": run, "signals": signals}))
}

fn number_from_keys(value: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(key).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_i64().map(|n| n as f64))
                .or_else(|| v.as_str()?.parse().ok())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_bars(count: usize, close: f64) -> Vec<OhlcBar> {
        (0..count)
            .map(|_| OhlcBar {
                high: close * 1.01,
                low: close * 0.99,
                close,
            })
            .collect()
    }

    /// Steady uptrend: each bar gains ~0.4%.
    fn uptrend_bars(count: usize) -> Vec<OhlcBar> {
        let mut close = 100.0;
        (0..count)
            .map(|_| {
                close *= 1.004;
                OhlcBar {
                    high: close * 1.012,
                    low: close * 0.992,
                    close,
                }
            })
            .collect()
    }

    fn downtrend_bars(count: usize) -> Vec<OhlcBar> {
        let mut close = 100.0;
        (0..count)
            .map(|_| {
                close *= 0.996;
                OhlcBar {
                    high: close * 1.008,
                    low: close * 0.988,
                    close,
                }
            })
            .collect()
    }

    #[test]
    fn computes_sma_over_trailing_window() {
        let closes = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sma(&closes, 5), Some(3.0));
        assert_eq!(sma(&closes, 2), Some(4.5));
        assert_eq!(sma(&closes, 6), None);
    }

    #[test]
    fn rsi_saturates_at_extremes_and_centers_on_flat() {
        let rising: Vec<f64> = (1..=40).map(|i| i as f64).collect();
        assert!(rsi(&rising, 14).unwrap() > 95.0);
        let falling: Vec<f64> = (1..=40).rev().map(|i| i as f64).collect();
        assert!(rsi(&falling, 14).unwrap() < 5.0);
    }

    #[test]
    fn macd_positive_in_uptrend_negative_in_downtrend() {
        let up: Vec<f64> = uptrend_bars(120).iter().map(|b| b.close).collect();
        let (line, _, _) = macd(&up).unwrap();
        assert!(line > 0.0);
        let down: Vec<f64> = downtrend_bars(120).iter().map(|b| b.close).collect();
        let (line, _, _) = macd(&down).unwrap();
        assert!(line < 0.0);
    }

    #[test]
    fn atr_reflects_bar_ranges() {
        let bars = flat_bars(40, 100.0);
        let atr_value = atr(&bars, 14).unwrap();
        // Range is high-low = 2.0 on every bar.
        assert!((atr_value - 2.0).abs() < 0.01, "atr {atr_value}");
    }

    #[test]
    fn analyze_bars_flags_uptrend_as_bullish_with_confluences() {
        let analysis = analyze_bars(&uptrend_bars(260), 2.0, 3).unwrap();
        assert_eq!(analysis.trend_bias, "bullish");
        assert!(matches!(analysis.sentiment, "BUY" | "OVERWEIGHT"));
        assert!(
            analysis.bullish_confluences.len() >= 3,
            "{:?}",
            analysis.bullish_confluences
        );
        assert!(analysis.bullish_confluences.contains(&"price_above_sma20"));
        assert!(analysis.bullish_confluences.contains(&"sma20_above_sma50"));
        assert!(analysis.bullish_confluences.contains(&"price_above_sma200"));
    }

    #[test]
    fn analyze_bars_flags_downtrend_as_bearish() {
        let analysis = analyze_bars(&downtrend_bars(260), 2.0, 3).unwrap();
        assert_eq!(analysis.trend_bias, "bearish");
        assert!(matches!(analysis.sentiment, "SELL" | "UNDERWEIGHT"));
        assert!(analysis.bearish_confluences.len() >= 3);
    }

    #[test]
    fn analyze_bars_is_neutral_on_flat_series() {
        let analysis = analyze_bars(&flat_bars(260, 100.0), 2.0, 3).unwrap();
        assert_eq!(analysis.trend_bias, "neutral");
        assert_eq!(analysis.sentiment, "HOLD");
    }

    #[test]
    fn analyze_bars_requires_enough_history() {
        assert!(analyze_bars(&flat_bars(30, 100.0), 2.0, 3).is_err());
    }

    #[test]
    fn sma200_is_optional_with_shorter_history() {
        let analysis = analyze_bars(&uptrend_bars(120), 2.0, 3).unwrap();
        assert!(analysis.sma200.is_none());
        assert!(!analysis.bullish_confluences.contains(&"price_above_sma200"));
    }

    #[test]
    fn clustered_pivot_lows_produce_nearest_and_next_support() {
        let lows = [
            108.0, 104.0, 100.0, 104.0, 108.0, 110.0, 105.0, 100.3, 105.0, 110.0, 108.0, 100.0,
            90.0, 100.0, 108.0, 110.0,
        ];
        let bars = lows
            .into_iter()
            .map(|low| OhlcBar {
                high: low + 4.0,
                low,
                close: 110.0,
            })
            .collect::<Vec<_>>();
        let support = support_analysis(&bars, 110.0, 2.0, "bearish", 40.0, -0.5);

        assert!((support.nearest_support.unwrap() - 100.1).abs() < 0.25);
        assert!((support.next_support.unwrap() - 90.0).abs() < 0.01);
        assert!(support.downside_to_support_pct.unwrap() > 8.0);
        assert!(support.downside_after_break_pct.unwrap() > 9.0);
        assert_eq!(support.break_risk_label, "high");
        assert!(support.confidence > 0.2);
        assert_eq!(support.touch_count, 2);
    }

    #[test]
    fn the_indicator_horizon_stays_daily_whatever_configuration_asks_for() {
        // Every window in this module is a bar count named for a daily meaning.
        // An intraday horizon would make RSI-14 a fourteen-hour RSI and the
        // 252-bar "one trading year" support window about 28 days, silently.
        // This is the trap that bit markov_method the same day, which scales
        // instead -- correct there, because a regime window has no canonical
        // duration, and wrong here, because RSI-14 does.
        assert_eq!(
            daily_horizon_minutes(DAILY_HORIZON_MINUTES),
            DAILY_HORIZON_MINUTES
        );
        for configured in [1, 15, 30, 60, 240, 720, 10080] {
            assert_eq!(
                daily_horizon_minutes(configured),
                DAILY_HORIZON_MINUTES,
                "configured horizon {configured} must not reach the indicator windows"
            );
        }
    }

    #[test]
    fn the_support_windows_still_mean_what_their_names_say() {
        // Guards the constants the clamp exists to protect: if one is ever
        // retuned, the "one year"/"five years" reading in the docs and in the
        // Support/Risk projection has to be revisited with it.
        assert_eq!(SUPPORT_SHORT_WINDOW, 252, "one trading year in daily bars");
        assert_eq!(
            SUPPORT_LONG_WINDOW, 1_260,
            "five trading years in daily bars"
        );
        assert_eq!(RSI_PERIOD, 14);
        assert_eq!(ATR_PERIOD, 14);
        assert_eq!((MACD_FAST, MACD_SLOW, MACD_SIGNAL), (12, 26, 9));
    }
}
