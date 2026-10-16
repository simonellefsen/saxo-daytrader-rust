//! Entry evaluation on `AppState`: the forward evaluation of every BUY fill,
//! its typed read model, and the weekly snapshot.
//!
//! Read-only measurement. It proposes nothing, gates nothing, and reaches no
//! broker. The pure evaluation lives in `entry_evaluation`; this module reads
//! the fills, prices and calendars it needs.

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use tracing::warn;

use crate::{
    config::yaml_string,
    db::{clamp_limit, sql_escape, value_f64, value_i64},
    state::{AppState, exchange_code_for, json_text},
};

impl AppState {
    /// The entry evaluation as a typed read model. The builder below stays
    /// JSON because the weekly snapshot stores exactly what it emits.
    pub async fn entry_evaluation_payload(
        &self,
        limit: i64,
    ) -> Result<crate::models::EntryEvaluationPayload> {
        Ok(crate::read_model::decode(
            "entry evaluation",
            self.entry_evaluation(limit).await?,
        )?)
    }

    /// Forward evaluation of every BUY fill at the declared horizons.
    ///
    /// Reads fills rather than sales so entries still open are included, which
    /// is the survivorship fix the ad-hoc analyses lacked. The price series is
    /// the stored daily indicator close -- the same series the gates read, so
    /// the evaluation cannot see a price the strategy could not.
    pub async fn entry_evaluation(&self, limit: i64) -> Result<JsonValue> {
        let fills = self
            .select_json(&format!(
                "SELECT f.created_at, f.symbol, f.delta_quantity, f.average_price_local,
                        f.currency, d.model
                 FROM execution_fills AS f
                 LEFT JOIN execution_orders AS o ON o.id = f.execution_order_id
                 LEFT JOIN decision_reports AS d ON d.id = o.report_id
                 WHERE f.side = 'BUY' AND f.delta_quantity > 0 AND f.average_price_local > 0
                 ORDER BY f.created_at DESC
                 LIMIT {}",
                clamp_limit(limit, 1, 2_000)
            ))
            .await?;

        let horizon_span = crate::entry_evaluation::HORIZONS
            .iter()
            .copied()
            .max()
            .unwrap_or(20);
        let mut evaluated = Vec::new();
        for fill in &fills {
            let symbol = json_text(fill, "symbol");
            let filled_at = json_text(fill, "created_at");
            if symbol.is_empty() || filled_at.is_empty() {
                continue;
            }
            let fill_day = filled_at.get(..10).unwrap_or_default().to_string();
            let exchange = exchange_code_for(&symbol);
            // The indicator series stores a row per calendar weekday, carrying
            // the previous close through market holidays. Those rows are not
            // sessions, and counting them put horizon 1 after a 2026-09-04 US
            // fill on Labor Day.
            let holidays = self.non_session_dates_for_exchange(&exchange).await;
            let closes = crate::entry_evaluation::trading_sessions(
                &self
                    .forward_closes(&symbol, &fill_day, horizon_span * 2)
                    .await,
                &holidays,
            );
            let sessions_elapsed = self.sessions_since(&fill_day, &holidays).await;
            let (min_commission_local, _) =
                crate::saxo_order::min_commission_local_for_exchange(&exchange);
            let benchmark_key = crate::entry_evaluation::benchmark_key_for_exchange(&exchange);
            let benchmark = self
                .benchmark_closes(benchmark_key, &fill_day, horizon_span)
                .await;
            evaluated.push(crate::entry_evaluation::evaluate_entry(
                &crate::entry_evaluation::Entry {
                    symbol: &symbol,
                    filled_at: &filled_at,
                    fill_price_local: value_f64(fill, "average_price_local"),
                    quantity: value_f64(fill, "delta_quantity"),
                    model: &json_text(fill, "model"),
                    forward_closes: &closes,
                    benchmark_closes: &benchmark,
                    min_commission_local,
                    rate_per_side: crate::entry_evaluation::COMMISSION_RATE_PER_SIDE,
                },
                sessions_elapsed,
            ));
        }

        let mut summary = crate::entry_evaluation::summarize(&evaluated);
        if let Some(object) = summary.as_object_mut() {
            object.insert(
                "generated_at".to_string(),
                JsonValue::from(Utc::now().to_rfc3339()),
            );
            object.insert("entries".to_string(), JsonValue::from(evaluated));
        }
        Ok(summary)
    }

    /// Records this ISO week's evaluation snapshot if it is due.
    ///
    /// Idempotent through the week primary key rather than a cursor, so a
    /// scheduler restart cannot double-record and a crash cannot lose the week.
    /// Returns what it decided so the cycle payload says why nothing happened
    /// -- "not due" and "failed" must not look alike in the record.
    pub(crate) async fn record_weekly_entry_evaluation(&self) -> JsonValue {
        let timezone = yaml_string(&self.config, &["localization", "time_zone"])
            .and_then(|value| value.parse::<chrono_tz::Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        let weekday = yaml_string(&self.config, &["evaluation", "weekly_snapshot_weekday"])
            .and_then(|value| value.parse::<chrono::Weekday>().ok())
            .unwrap_or(chrono::Weekday::Sun);
        let at_or_after = yaml_string(&self.config, &["evaluation", "weekly_snapshot_local_time"])
            .and_then(|value| chrono::NaiveTime::parse_from_str(&value, "%H:%M").ok())
            .unwrap_or_else(|| {
                chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("static snapshot time")
            });
        let now = Utc::now();
        let existing = self
            .select_json("SELECT iso_week FROM entry_evaluation_snapshots")
            .await
            .unwrap_or_default()
            .iter()
            .map(|row| json_text(row, "iso_week"))
            .collect::<Vec<_>>();

        // Bounded recovery: a missed week is still owed, but only for a few
        // weeks. Backfilling an old week would compute it from today's prices
        // and silently claim to be what that week knew.
        let Some(week_key) = crate::entry_evaluation::due_week_key(
            now.with_timezone(&timezone).naive_local(),
            weekday,
            at_or_after,
            &existing,
            crate::entry_evaluation::SNAPSHOT_RECOVERY_WEEKS,
        ) else {
            return json!({
                "status": "not_due",
                "current_iso_week": crate::entry_evaluation::iso_week_key(now),
            });
        };

        let summary = match self.entry_evaluation(2_000).await {
            Ok(summary) => summary,
            Err(err) => {
                warn!("weekly entry evaluation failed: {err:#}");
                return json!({"status": "error", "iso_week": week_key, "error": err.to_string()});
            }
        };
        // The per-entry rows stay out of the stored snapshot: they are
        // reproducible from the endpoint at any time, and a row per entry per
        // week would grow without bound for evidence nobody reads in aggregate.
        let mut stored = summary.clone();
        if let Some(object) = stored.as_object_mut() {
            object.remove("entries");
        }
        let entry_count = summary
            .get("entry_count")
            .and_then(JsonValue::as_i64)
            .unwrap_or_default();
        let sql = format!(
            "INSERT INTO entry_evaluation_snapshots (iso_week, created_at, entry_count, summary_json)
             VALUES ('{}', '{}', {}, '{}')
             ON CONFLICT(iso_week) DO UPDATE SET
                created_at = excluded.created_at,
                entry_count = excluded.entry_count,
                summary_json = excluded.summary_json",
            sql_escape(&week_key),
            sql_escape(&now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            entry_count,
            sql_escape(&serde_json::to_string(&stored).unwrap_or_else(|_| "{}".to_string()))
        );
        match sqlx::query(&sql).execute(&self.pool).await {
            Ok(_) => json!({
                "status": "recorded",
                "iso_week": week_key,
                "entry_count": entry_count,
            }),
            Err(err) => {
                warn!("persisting weekly entry evaluation failed: {err:#}");
                json!({"status": "error", "iso_week": week_key, "error": err.to_string()})
            }
        }
    }

    /// Dated daily closes for `symbol` strictly after `from_day`, oldest first.
    async fn forward_closes(&self, symbol: &str, from_day: &str, limit: i64) -> Vec<(String, f64)> {
        self.select_json(&format!(
            "SELECT run_date, close FROM daily_indicator_signals
             WHERE symbol = '{}' AND run_date > '{}' AND close IS NOT NULL
             ORDER BY run_date ASC LIMIT {}",
            sql_escape(symbol),
            sql_escape(from_day),
            clamp_limit(limit, 1, 400)
        ))
        .await
        .unwrap_or_default()
        .iter()
        .map(|row| (json_text(row, "run_date"), value_f64(row, "close")))
        .collect()
    }

    /// Dated benchmark closes spanning the entry's whole horizon window.
    ///
    /// Starts a fortnight before the fill so the base can be resolved even when
    /// the benchmark has no row on the fill date itself -- a venue holiday on
    /// one side of the comparison must not delete the comparison.
    async fn benchmark_closes(
        &self,
        reference_key: &str,
        from_day: &str,
        limit: i64,
    ) -> Vec<(String, f64)> {
        let lookback_start = chrono::NaiveDate::parse_from_str(from_day, "%Y-%m-%d")
            .map(|day| (day - chrono::Duration::days(14)).to_string())
            .unwrap_or_else(|_| from_day.to_string());
        self.select_json(&format!(
            "SELECT substr(observed_at, 1, 10) AS observed_day, close
             FROM performance_benchmark_prices
             WHERE reference_key = '{}' AND substr(observed_at, 1, 10) >= '{}'
               AND close IS NOT NULL
             ORDER BY observed_at ASC LIMIT {}",
            sql_escape(reference_key),
            sql_escape(&lookback_start),
            clamp_limit(limit + 20, 1, 400)
        ))
        .await
        .unwrap_or_default()
        .iter()
        .map(|row| (json_text(row, "observed_day"), value_f64(row, "close")))
        .collect()
    }

    /// Trading sessions recorded strictly after `from_day`, holidays excluded.
    async fn sessions_since(
        &self,
        from_day: &str,
        holidays: &std::collections::HashSet<String>,
    ) -> i64 {
        self.select_json(&format!(
            "SELECT DISTINCT run_date FROM daily_indicator_signals WHERE run_date > '{}'",
            sql_escape(from_day)
        ))
        .await
        .unwrap_or_default()
        .iter()
        .map(|row| json_text(row, "run_date"))
        .filter(|date| !holidays.contains(date))
        .count() as i64
    }

    /// Dates an exchange did not trade, inferred from whether its symbols moved.
    ///
    /// Derived from the stored series rather than from a calendar table,
    /// because no historical exchange calendar is persisted. A real session
    /// moves prices somewhere; a carried-forward holiday row moves nothing.
    async fn non_session_dates_for_exchange(
        &self,
        exchange: &str,
    ) -> std::collections::HashSet<String> {
        let rows = self
            .select_json(&format!(
                "SELECT run_date,
                        COUNT(*) AS symbols,
                        COUNT(*) FILTER (WHERE previous_close IS NOT NULL
                                           AND close <> previous_close) AS changed
                 FROM (
                    SELECT run_date, symbol, close,
                           LAG(close) OVER (PARTITION BY symbol ORDER BY run_date) AS previous_close
                    FROM daily_indicator_signals
                    WHERE close IS NOT NULL AND symbol LIKE '%:{}'
                 ) AS moves
                 GROUP BY run_date",
                sql_escape(exchange)
            ))
            .await
            .unwrap_or_default()
            .iter()
            .map(|row| {
                (
                    json_text(row, "run_date"),
                    value_i64(row, "symbols"),
                    value_i64(row, "changed"),
                )
            })
            .collect::<Vec<_>>();
        crate::entry_evaluation::non_session_dates(&rows, 5, 0.2)
    }
}
