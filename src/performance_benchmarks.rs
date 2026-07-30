//! Read-only account-value comparisons against Saxo-resolved ETF proxies.
//!
//! This module intentionally keeps benchmark data out of trading research:
//! it is neither a candidate universe nor prompt context. The account series
//! is DKK portfolio value (including cash), while reference series are native
//! currency price returns. The UI must keep those limits visible.

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64},
    markov_method::{SaxoInstrument, account_key, resolve_instrument, saxo_get_json},
    state::AppState,
};

const DEFAULT_DAILY_TIME: &str = "23:55";
const DEFAULT_SAMPLE_COUNT: usize = 1260;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchmarkReference {
    key: String,
    label: String,
    symbol: String,
}

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    enabled: bool,
    timezone: Tz,
    daily_time: NaiveTime,
    run_weekdays_only: bool,
    sample_count: usize,
    references: Vec<BenchmarkReference>,
}

#[derive(Clone, Debug)]
struct ClosePoint {
    observed_at: String,
    close: f64,
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS performance_benchmark_runs (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            run_date TEXT NOT NULL,
            status TEXT NOT NULL,
            reference_count INTEGER NOT NULL,
            success_count INTEGER NOT NULL,
            error_count INTEGER NOT NULL,
            config_json TEXT NOT NULL,
            summary_json TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS performance_benchmark_prices (
            reference_key TEXT NOT NULL,
            label TEXT NOT NULL,
            symbol TEXT NOT NULL,
            observed_at TEXT NOT NULL,
            close REAL NOT NULL,
            uic INTEGER,
            asset_type TEXT,
            run_id TEXT NOT NULL,
            PRIMARY KEY (reference_key, observed_at)
        )",
        "CREATE INDEX IF NOT EXISTS idx_performance_benchmark_prices_reference_time
         ON performance_benchmark_prices(reference_key, observed_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_performance_benchmark_runs_date
         ON performance_benchmark_runs(run_date DESC, created_at DESC)",
    ]
}

fn benchmark_config(state: &AppState) -> BenchmarkConfig {
    let base = ["strategy", "performance_benchmarks"];
    let key = |suffix: &'static str| -> [&str; 3] { [base[0], base[1], suffix] };
    let timezone = yaml_string(&state.config, &key("timezone"))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let daily_time = yaml_string(&state.config, &key("daily_time"))
        .and_then(|value| NaiveTime::parse_from_str(&value, "%H:%M").ok())
        .unwrap_or_else(|| {
            NaiveTime::parse_from_str(DEFAULT_DAILY_TIME, "%H:%M")
                .expect("default benchmark time is valid")
        });
    let references = yaml_at(
        &state.config,
        &["strategy", "performance_benchmarks", "references"],
    )
    .and_then(serde_yaml::Value::as_sequence)
    .map(|items| {
        items
            .iter()
            .filter_map(|item| {
                let mapping = item.as_mapping()?;
                let field = |name: &str| {
                    mapping
                        .get(serde_yaml::Value::String(name.to_string()))
                        .and_then(serde_yaml::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                };
                Some(BenchmarkReference {
                    key: field("key")?,
                    label: field("label")?,
                    symbol: field("symbol")?,
                })
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    BenchmarkConfig {
        enabled: yaml_bool(&state.config, &key("enabled")).unwrap_or(false),
        timezone,
        daily_time,
        run_weekdays_only: yaml_bool(&state.config, &key("run_weekdays_only")).unwrap_or(true),
        sample_count: yaml_i64(&state.config, &key("sample_count"))
            .unwrap_or(DEFAULT_SAMPLE_COUNT as i64)
            .clamp(20, 5000) as usize,
        references,
    }
}

pub(crate) fn benchmark_config_json_for_state(state: &AppState) -> JsonValue {
    benchmark_config_json(&benchmark_config(state))
}

fn benchmark_config_json(config: &BenchmarkConfig) -> JsonValue {
    json!({
        "enabled": config.enabled,
        "timezone": config.timezone.name(),
        "daily_time": config.daily_time.format("%H:%M").to_string(),
        "run_weekdays_only": config.run_weekdays_only,
        "sample_count": config.sample_count,
        "reference_count": config.references.len(),
        "references": config.references.iter().map(|reference| json!({
            "key": reference.key,
            "label": reference.label,
            "symbol": reference.symbol,
        })).collect::<Vec<_>>(),
        "scope": "read_only_performance_comparison_only",
    })
}

pub async fn run_performance_benchmarks_now(state: &AppState) -> Result<JsonValue> {
    let config = benchmark_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    let run_date = Utc::now().with_timezone(&config.timezone).date_naive();
    run_for_date(state, &config, run_date).await
}

pub async fn run_performance_benchmark_cycle(state: &AppState) -> Result<JsonValue> {
    let config = benchmark_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    if config.references.is_empty() {
        return Ok(json!({"status": "empty", "reason": "no_references_configured"}));
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
    if run_exists(state, run_date).await? {
        return Ok(
            json!({"status": "skipped", "reason": "already_ran", "run_date": run_date.to_string()}),
        );
    }
    run_for_date(state, &config, run_date).await
}

async fn run_for_date(
    state: &AppState,
    config: &BenchmarkConfig,
    run_date: NaiveDate,
) -> Result<JsonValue> {
    let session = state
        .ensure_saxo_session_json("performance_benchmarks")
        .await
        .context("loading Saxo session for performance benchmark refresh")?;
    let run_id = format!("performance-benchmarks-{}", Utc::now().timestamp_micros());
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut successes = Vec::new();
    let mut errors = Vec::new();
    for reference in &config.references {
        match refresh_reference(state, &session, config, &run_id, reference).await {
            Ok(point_count) => successes.push(json!({
                "key": reference.key,
                "label": reference.label,
                "symbol": reference.symbol,
                "point_count": point_count,
            })),
            Err(err) => {
                warn!(key = %reference.key, symbol = %reference.symbol, "performance benchmark refresh failed: {err:#}");
                errors.push(json!({
                    "key": reference.key,
                    "label": reference.label,
                    "symbol": reference.symbol,
                    "error": format!("{err:#}"),
                }));
            }
        }
    }
    let status = if successes.is_empty() {
        "error"
    } else if errors.is_empty() {
        "completed"
    } else {
        "partial"
    };
    let summary = json!({
        "status": status,
        "run_id": run_id,
        "run_date": run_date.to_string(),
        "success_count": successes.len(),
        "error_count": errors.len(),
        "references": successes,
        "errors": errors,
        "scope": "read_only_performance_comparison_only",
    });
    let sql = format!(
        "INSERT INTO performance_benchmark_runs (
            id, created_at, run_date, status, reference_count, success_count, error_count,
            config_json, summary_json
         ) VALUES ('{}', '{}', '{}', '{}', {}, {}, {}, '{}', '{}')",
        sql_escape(&run_id),
        sql_escape(&created_at),
        run_date,
        status,
        config.references.len(),
        summary
            .get("success_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        summary
            .get("error_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0),
        sql_escape(&benchmark_config_json(config).to_string()),
        sql_escape(&summary.to_string()),
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording performance benchmark refresh")?;
    info!(run_id, run_date = %run_date, status, "performance benchmark refresh completed");
    Ok(summary)
}

async fn refresh_reference(
    state: &AppState,
    session: &JsonValue,
    config: &BenchmarkConfig,
    run_id: &str,
    reference: &BenchmarkReference,
) -> Result<usize> {
    let instrument = resolve_instrument(state, session, &reference.symbol)
        .await
        .with_context(|| format!("resolving Saxo benchmark {}", reference.symbol))?;
    let points = fetch_close_points(state, session, &instrument, config)
        .await
        .with_context(|| format!("fetching Saxo benchmark closes for {}", reference.symbol))?;
    if points.is_empty() {
        anyhow::bail!("Saxo chart response contained no usable closes");
    }
    for point in &points {
        upsert_close(state, run_id, reference, &instrument, point).await?;
    }
    Ok(points.len())
}

async fn fetch_close_points(
    state: &AppState,
    session: &JsonValue,
    instrument: &SaxoInstrument,
    config: &BenchmarkConfig,
) -> Result<Vec<ClosePoint>> {
    let query = vec![
        ("AccountKey", account_key(state, session)?),
        ("AssetType", instrument.asset_type.clone()),
        ("Uic", instrument.uic.to_string()),
        ("Horizon", "1440".to_string()),
        ("Count", config.sample_count.to_string()),
        ("FieldGroups", "ChartInfo,Data,DisplayAndFormat".to_string()),
    ];
    let payload = saxo_get_json(state, session, "/chart/v3/charts", &query).await?;
    let mut points = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let observed_at = item
                .get("Time")
                .or_else(|| item.get("time"))
                .and_then(JsonValue::as_str)?
                .trim()
                .to_string();
            let close = close_from_chart_row(&item)?;
            (!observed_at.is_empty() && close > 0.0).then_some(ClosePoint { observed_at, close })
        })
        .collect::<Vec<_>>();
    points.sort_by(|left, right| left.observed_at.cmp(&right.observed_at));
    points.dedup_by(|left, right| left.observed_at == right.observed_at);
    Ok(points)
}

fn close_from_chart_row(row: &JsonValue) -> Option<f64> {
    [
        "Close",
        "ClosePrice",
        "close",
        "closePrice",
        "LastTraded",
        "Price",
    ]
    .iter()
    .find_map(|key| {
        row.get(*key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_i64().map(|number| number as f64))
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        })
    })
}

async fn upsert_close(
    state: &AppState,
    run_id: &str,
    reference: &BenchmarkReference,
    instrument: &SaxoInstrument,
    point: &ClosePoint,
) -> Result<()> {
    let sql = format!(
        "INSERT INTO performance_benchmark_prices (
            reference_key, label, symbol, observed_at, close, uic, asset_type, run_id
         ) VALUES ('{}', '{}', '{}', '{}', {}, {}, '{}', '{}')
         ON CONFLICT(reference_key, observed_at) DO UPDATE SET
            label = excluded.label,
            symbol = excluded.symbol,
            close = excluded.close,
            uic = excluded.uic,
            asset_type = excluded.asset_type,
            run_id = excluded.run_id",
        sql_escape(&reference.key),
        sql_escape(&reference.label),
        sql_escape(&reference.symbol),
        sql_escape(&point.observed_at),
        point.close,
        instrument.uic,
        sql_escape(&instrument.asset_type),
        sql_escape(run_id),
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("upserting performance benchmark close")?;
    Ok(())
}

pub async fn performance_benchmark_payload(
    state: &AppState,
    history: &[JsonValue],
) -> Result<JsonValue> {
    let config = benchmark_config(state);
    let run = latest_run(state).await?;
    let first = history.first().cloned().unwrap_or(JsonValue::Null);
    let latest = history.last().cloned().unwrap_or(JsonValue::Null);
    let start_at = first
        .get("recorded_at")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let end_at = latest
        .get("recorded_at")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let start_value = value_f64(&first, "total_market_value_dkk");
    let end_value = value_f64(&latest, "total_market_value_dkk");
    if !config.enabled {
        return Ok(json!({"status": "disabled", "references": []}));
    }
    if start_at.is_empty() || end_at.is_empty() || start_value <= 0.0 || end_value <= 0.0 {
        return Ok(json!({
            "status": "pending_portfolio_history",
            "latest_run": run,
            "references": [],
            "caveat": caveat(),
        }));
    }
    let portfolio_return_pct = return_pct(start_value, end_value);
    let mut references = Vec::new();
    let mut ready_count = 0usize;
    for reference in &config.references {
        let start = close_at_or_before(state, &reference.key, start_at).await?;
        let end = close_at_or_before(state, &reference.key, end_at).await?;
        let row = match (start, end) {
            (Some(start), Some(end)) if start.close > 0.0 && end.close > 0.0 => {
                ready_count += 1;
                let benchmark_return_pct = return_pct(start.close, end.close);
                json!({
                    "key": reference.key,
                    "label": reference.label,
                    "symbol": reference.symbol,
                    "status": "ready",
                    "portfolio_return_pct": portfolio_return_pct,
                    "benchmark_return_pct": benchmark_return_pct,
                    "excess_return_pct": portfolio_return_pct - benchmark_return_pct,
                    "baseline_close": start.close,
                    "latest_close": end.close,
                    "baseline_at": start.observed_at,
                    "latest_at": end.observed_at,
                })
            }
            _ => json!({
                "key": reference.key,
                "label": reference.label,
                "symbol": reference.symbol,
                "status": "pending_history",
                "portfolio_return_pct": portfolio_return_pct,
            }),
        };
        references.push(row);
    }
    let status = if ready_count == config.references.len() && ready_count > 0 {
        "ready"
    } else if ready_count > 0 {
        "partial"
    } else if run.is_null() {
        "pending_benchmark_sync"
    } else {
        "pending_history"
    };
    Ok(json!({
        "status": status,
        "latest_run": run,
        "portfolio_baseline_at": start_at,
        "portfolio_latest_at": end_at,
        "portfolio_return_pct": portfolio_return_pct,
        "ready_count": ready_count,
        "reference_count": config.references.len(),
        "references": references,
        "caveat": caveat(),
    }))
}

/// Builds the same comparison used by the Performance view for a bounded
/// end-of-day account interval. The result remains read-only context: callers
/// must not use it to alter candidate selection, sizing, or broker actions.
pub async fn performance_benchmark_readthrough(
    state: &AppState,
    baseline: Option<&JsonValue>,
    latest: Option<&JsonValue>,
) -> Result<JsonValue> {
    let history = match (baseline, latest) {
        (Some(baseline), Some(latest)) => vec![baseline.clone(), latest.clone()],
        _ => Vec::new(),
    };
    let mut payload = performance_benchmark_payload(state, &history).await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "scope".to_string(),
            JsonValue::String("read_only_end_of_day_context_only".to_string()),
        );
    }
    Ok(payload)
}

fn return_pct(start: f64, end: f64) -> f64 {
    if start <= 0.0 || !start.is_finite() || !end.is_finite() {
        0.0
    } else {
        ((end / start) - 1.0) * 100.0
    }
}

fn caveat() -> &'static str {
    "Portfolio value in DKK, including cash, compared with native-currency ETF price-return proxies. This is not a time-weighted or total-return comparison; dividends, FX effects, fees, tax, and external cash flows are not normalized. Read-only only."
}

async fn latest_run(state: &AppState) -> Result<JsonValue> {
    let row = sqlx::query(
        "SELECT id, created_at, run_date, status, reference_count, success_count, error_count
         FROM performance_benchmark_runs
         ORDER BY run_date DESC, created_at DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.as_ref().map(row_to_json).unwrap_or(JsonValue::Null))
}

async fn close_at_or_before(
    state: &AppState,
    reference_key: &str,
    boundary: &str,
) -> Result<Option<ClosePoint>> {
    let sql = format!(
        "SELECT observed_at, close FROM performance_benchmark_prices
         WHERE reference_key = '{}' AND observed_at <= '{}'
         ORDER BY observed_at DESC LIMIT 1",
        sql_escape(reference_key),
        sql_escape(boundary),
    );
    let row = sqlx::query(&sql).fetch_optional(&state.pool).await?;
    Ok(row.map(|row| {
        let value = row_to_json(&row);
        ClosePoint {
            observed_at: value
                .get("observed_at")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string(),
            // PostgreSQL REAL is decoded as f32 by sqlx::AnyPool, while
            // SQLite returns f64. The shared row adapter handles both.
            close: value_f64(&value, "close"),
        }
    }))
}

async fn run_exists(state: &AppState, run_date: NaiveDate) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM performance_benchmark_runs WHERE run_date = '{}' LIMIT 1",
        run_date
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::{path::PathBuf, sync::Once};

    fn test_config() -> serde_yaml::Value {
        serde_yaml::from_str(
            "strategy:\n  performance_benchmarks:\n    enabled: true\n    timezone: Europe/Copenhagen\n    daily_time: '23:55'\n    references:\n      - key: us_large_cap\n        label: S&P 500 (SPY ETF proxy)\n        symbol: SPY:arcx\n",
        )
        .expect("test config parses")
    }

    async fn test_state() -> AppState {
        static INSTALL_DRIVERS: Once = Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open benchmark test database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create benchmark tables");
        }
        AppState {
            config_path: PathBuf::from("performance-benchmark-test.yaml"),
            config: test_config(),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[test]
    fn price_return_is_signed_and_defensive() {
        assert!((return_pct(100.0, 110.0) - 10.0).abs() < 1e-10);
        assert!((return_pct(100.0, 90.0) + 10.0).abs() < 1e-10);
        assert_eq!(return_pct(0.0, 90.0), 0.0);
    }

    #[tokio::test]
    async fn comparison_uses_the_latest_close_at_or_before_each_portfolio_boundary() {
        let state = test_state().await;
        for (observed_at, close) in [
            ("2026-07-01T00:00:00Z", 100.0),
            ("2026-07-10T00:00:00Z", 110.0),
            ("2026-07-20T00:00:00Z", 120.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO performance_benchmark_prices (reference_key, label, symbol, observed_at, close, run_id)
                 VALUES ('us_large_cap', 'S&P 500 (SPY ETF proxy)', 'SPY:arcx', '{observed_at}', {close}, 'test')"
            ))
            .execute(&state.pool)
            .await
            .expect("insert benchmark close");
        }
        let history = vec![
            json!({"recorded_at": "2026-07-11T12:00:00Z", "total_market_value_dkk": 200_000.0}),
            json!({"recorded_at": "2026-07-21T12:00:00Z", "total_market_value_dkk": 220_000.0}),
        ];
        let payload = performance_benchmark_payload(&state, &history)
            .await
            .expect("benchmark payload");
        let reference = payload
            .get("references")
            .and_then(JsonValue::as_array)
            .and_then(|references| references.first())
            .expect("reference row");
        assert_eq!(
            reference.get("status").and_then(JsonValue::as_str),
            Some("ready")
        );
        let benchmark_return = reference
            .get("benchmark_return_pct")
            .and_then(JsonValue::as_f64)
            .expect("benchmark return");
        let excess_return = reference
            .get("excess_return_pct")
            .and_then(JsonValue::as_f64)
            .expect("excess return");
        assert!((benchmark_return - 9.090_909_090_9).abs() < 1e-8);
        assert!((excess_return - 0.909_090_909_1).abs() < 1e-8);
    }

    #[tokio::test]
    async fn end_of_day_readthrough_is_scoped_read_only() {
        let state = test_state().await;
        for (observed_at, close) in [
            ("2026-07-01T00:00:00Z", 100.0),
            ("2026-07-02T00:00:00Z", 105.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO performance_benchmark_prices (reference_key, label, symbol, observed_at, close, run_id)
                 VALUES ('us_large_cap', 'S&P 500 (SPY ETF proxy)', 'SPY:arcx', '{observed_at}', {close}, 'test')"
            ))
            .execute(&state.pool)
            .await
            .expect("insert benchmark close");
        }
        let baseline = json!({
            "recorded_at": "2026-07-01T12:00:00Z",
            "total_market_value_dkk": 200_000.0,
        });
        let latest = json!({
            "recorded_at": "2026-07-02T12:00:00Z",
            "total_market_value_dkk": 210_000.0,
        });

        let payload = performance_benchmark_readthrough(&state, Some(&baseline), Some(&latest))
            .await
            .expect("build end-of-day readthrough");

        assert_eq!(
            payload.get("scope").and_then(JsonValue::as_str),
            Some("read_only_end_of_day_context_only")
        );
        assert_eq!(
            payload.get("status").and_then(JsonValue::as_str),
            Some("ready")
        );
    }
}
