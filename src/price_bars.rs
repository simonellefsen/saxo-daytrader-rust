//! Daily OHLC bars for every symbol the indicator run analyses.
//!
//! The daily indicator run has always fetched five years of daily bars per
//! symbol from Saxo and kept only the newest close and ATR14. A stop or a
//! take-profit that rests at the broker is touched by a session's low or high,
//! not by its close, so the exit evaluation could replay those rules on closes
//! only. This keeps the bars the run already fetched: no extra Saxo call.
//!
//! Storage never decides anything. Nothing here can fail the indicator run,
//! which the technical gate depends on; a write that fails is counted in the
//! run summary and the gate's evidence is untouched.

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use serde_json::Value as JsonValue;

use crate::{daily_indicators::OhlcBar, db::sql_escape, state::AppState};

/// A first run for a symbol stores about one trading year, enough to replay
/// every entry in the current book; later runs store only what is new.
pub(crate) const BACKFILL_SESSIONS: usize = 260;

/// Bars this many days before the newest stored one are written again, so a
/// bar Saxo corrected after the fact is replaced rather than kept stale.
pub(crate) const REVISE_DAYS: i64 = 7;

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        // `symbol` is the traded symbol, as in `daily_indicator_signals`;
        // `analysis_symbol` is the instrument the bars came from when the
        // run analyses a proxy.
        "CREATE TABLE IF NOT EXISTS daily_price_bars (
            symbol TEXT NOT NULL,
            bar_date TEXT NOT NULL,
            analysis_symbol TEXT NOT NULL,
            bar_time TEXT NOT NULL,
            open DOUBLE PRECISION,
            high DOUBLE PRECISION,
            low DOUBLE PRECISION,
            close DOUBLE PRECISION NOT NULL,
            run_id TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (symbol, bar_date)
        )",
    ]
}

/// One daily bar as Saxo returned it.
///
/// A missing open, high or low stays missing. The indicator math falls back
/// to the close, which is right for a moving average, but a stored range that
/// is really a close would read as a session that never moved, and a stop
/// replayed against it would never be touched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChartBar {
    pub time: String,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: f64,
}

impl ChartBar {
    /// The session date, from the first ten characters of Saxo's `Time`: the
    /// same reading the benchmark store and the entry evaluation use.
    pub(crate) fn bar_date(&self) -> &str {
        self.time.get(..10).unwrap_or_default()
    }

    pub(crate) fn ohlc(&self) -> OhlcBar {
        OhlcBar {
            high: self.high.unwrap_or(self.close),
            low: self.low.unwrap_or(self.close),
            close: self.close,
        }
    }
}

fn number(value: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| {
            value.get(key).and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_i64().map(|n| n as f64))
                    .or_else(|| v.as_str()?.parse().ok())
            })
        })
        .filter(|value: &f64| value.is_finite() && *value > 0.0)
}

/// The bars of a `/chart/v3/charts` payload, oldest first, one per `Time`.
///
/// A bar without a positive close or without a time is dropped: it cannot be
/// placed on a calendar or valued.
pub(crate) fn chart_bars(payload: &JsonValue) -> Vec<ChartBar> {
    let mut bars = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            let close = number(item, &["Close", "ClosePrice", "close", "LastTraded"])?;
            let time = item
                .get("Time")
                .or_else(|| item.get("time"))
                .and_then(JsonValue::as_str)
                .filter(|time| time.len() >= 10)?
                .to_string();
            Some(ChartBar {
                time,
                open: number(item, &["Open", "open"]),
                high: number(item, &["High", "high"]),
                low: number(item, &["Low", "low"]),
                close,
            })
        })
        .collect::<Vec<_>>();
    bars.sort_by(|left, right| left.time.cmp(&right.time));
    bars.dedup_by(|left, right| left.time == right.time);
    bars
}

/// Which fetched bars to write.
///
/// The last `BACKFILL_SESSIONS` when nothing is stored for the symbol yet,
/// otherwise everything from `REVISE_DAYS` before the newest stored bar.
pub(crate) fn bars_to_store<'a>(
    bars: &'a [ChartBar],
    newest_stored: Option<&str>,
) -> &'a [ChartBar] {
    let from = newest_stored
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .map(|date| (date - chrono::Duration::days(REVISE_DAYS)).to_string());
    match from {
        Some(from) => {
            let start = bars.partition_point(|bar| bar.bar_date() < from.as_str());
            &bars[start..]
        }
        None => &bars[bars.len().saturating_sub(BACKFILL_SESSIONS)..],
    }
}

fn sql_number(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map_or_else(|| "NULL".to_string(), |value| value.to_string())
}

async fn newest_stored_bar_date(state: &AppState, symbol: &str) -> Result<Option<String>> {
    let rows = state
        .select_json(&format!(
            "SELECT MAX(bar_date) AS newest FROM daily_price_bars WHERE symbol = '{}'",
            sql_escape(symbol)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("newest"))
        .and_then(JsonValue::as_str)
        .filter(|date| !date.is_empty())
        .map(str::to_string))
}

/// Upserts the bars `bars_to_store` selects and returns how many it wrote.
pub(crate) async fn store_bars(
    state: &AppState,
    symbol: &str,
    analysis_symbol: &str,
    run_id: &str,
    bars: &[ChartBar],
) -> Result<usize> {
    let newest = newest_stored_bar_date(state, symbol).await?;
    let selected = bars_to_store(bars, newest.as_deref());
    let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // Batched: a first run writes a year for each of ~200 symbols.
    for chunk in selected.chunks(100) {
        let values = chunk
            .iter()
            .map(|bar| {
                format!(
                    "('{}', '{}', '{}', '{}', {}, {}, {}, {}, '{}', '{}')",
                    sql_escape(symbol),
                    sql_escape(bar.bar_date()),
                    sql_escape(analysis_symbol),
                    sql_escape(&bar.time),
                    sql_number(bar.open),
                    sql_number(bar.high),
                    sql_number(bar.low),
                    bar.close,
                    sql_escape(run_id),
                    updated_at,
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        sqlx::query(&format!(
            "INSERT INTO daily_price_bars (
                symbol, bar_date, analysis_symbol, bar_time, open, high, low, close, run_id,
                updated_at
             ) VALUES {values}
             ON CONFLICT(symbol, bar_date) DO UPDATE SET
                analysis_symbol = excluded.analysis_symbol,
                bar_time = excluded.bar_time,
                open = excluded.open,
                high = excluded.high,
                low = excluded.low,
                close = excluded.close,
                run_id = excluded.run_id,
                updated_at = excluded.updated_at"
        ))
        .execute(&state.pool)
        .await
        .with_context(|| format!("storing daily price bars for {symbol}"))?;
    }
    Ok(selected.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bar(date: &str, close: f64) -> ChartBar {
        ChartBar {
            time: format!("{date}T00:00:00.000000Z"),
            open: Some(close),
            high: Some(close + 1.0),
            low: Some(close - 1.0),
            close,
        }
    }

    /// Saxo's order is not trusted, a duplicate time is kept once, and a bar
    /// without a range keeps its range missing rather than borrowing the close.
    #[test]
    fn chart_bars_are_ordered_deduplicated_and_keep_a_missing_range_missing() {
        let payload = json!({"Data": [
            {"Time": "2026-10-16T00:00:00.000000Z", "Open": 10.0, "High": 12.0, "Low": 9.0, "Close": 11.0},
            {"Time": "2026-10-15T00:00:00.000000Z", "Close": 10.5},
            {"Time": "2026-10-16T00:00:00.000000Z", "Open": 10.0, "High": 12.0, "Low": 9.0, "Close": 11.0},
            {"Time": "2026-10-14T00:00:00.000000Z", "Close": 0.0},
            {"Close": 9.0},
        ]});
        let bars = chart_bars(&payload);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].bar_date(), "2026-10-15");
        assert_eq!((bars[0].high, bars[0].low), (None, None));
        assert_eq!(
            bars[0].ohlc(),
            OhlcBar {
                high: 10.5,
                low: 10.5,
                close: 10.5
            },
            "the indicator math still gets a usable bar"
        );
        assert_eq!(
            (bars[1].open, bars[1].high, bars[1].low),
            (Some(10.0), Some(12.0), Some(9.0))
        );
    }

    #[test]
    fn a_first_run_backfills_a_year_and_later_runs_revise_the_last_week() {
        let start = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let bars: Vec<ChartBar> = (0..400)
            .map(|day| bar(&(start + chrono::Duration::days(day)).to_string(), 100.0))
            .collect();

        let first = bars_to_store(&bars, None);
        assert_eq!(first.len(), BACKFILL_SESSIONS);
        assert_eq!(first.last(), bars.last());

        let newest = bars[390].bar_date().to_string();
        let later = bars_to_store(&bars, Some(&newest));
        assert_eq!(later.first().unwrap().bar_date(), bars[383].bar_date());
        assert_eq!(later.len(), 400 - 383);

        assert_eq!(bars_to_store(&[], None), &[] as &[ChartBar]);
    }

    async fn bars_test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory bars database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create bars table");
        }
        AppState {
            config_path: std::path::PathBuf::from("bars-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// Writing twice keeps one row per session, a corrected bar replaces the
    /// stored one, and a missing range is stored as NULL.
    #[tokio::test]
    async fn stored_bars_are_upserted_per_session() {
        let state = bars_test_state().await;
        let mut bars = vec![bar("2026-10-15", 100.0), bar("2026-10-16", 101.0)];
        bars[0].high = None;
        assert_eq!(
            store_bars(&state, "AAPL:xnas", "AAPL:xnas", "run-1", &bars)
                .await
                .unwrap(),
            2
        );

        bars[1].low = Some(95.5);
        bars.push(bar("2026-10-19", 102.0));
        // Everything from a week before 10-16 is rewritten, so all three.
        assert_eq!(
            store_bars(&state, "AAPL:xnas", "AAPL:xnas", "run-2", &bars)
                .await
                .unwrap(),
            3
        );

        let rows = state
            .select_json(
                "SELECT bar_date, high, low, run_id FROM daily_price_bars ORDER BY bar_date",
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0]["high"].is_null());
        assert_eq!(rows[1]["low"].as_f64(), Some(95.5));
        assert_eq!(rows[1]["run_id"], "run-2");
    }
}
