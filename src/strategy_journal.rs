use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::{
    config::{yaml_bool, yaml_string},
    db::{row_to_json, sql_escape, value_f64, value_i64},
    state::AppState,
};

const DEFAULT_DAILY_TIME: &str = "22:30";

pub async fn run_strategy_journal_cycle(state: &AppState) -> Result<JsonValue> {
    if !journal_enabled(state) {
        return Ok(json!({"status": "disabled", "created": []}));
    }

    let tz = journal_timezone(state);
    let daily_time = journal_daily_time(state);
    let now_local = Utc::now().with_timezone(&tz);
    let due_dates = due_daily_journal_dates(now_local, daily_time);
    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for journal_date in due_dates {
        if journal_exists(state, journal_date, "daily").await? {
            skipped.push(json!({
                "journal_date": journal_date.to_string(),
                "cadence": "daily",
                "reason": "already_exists"
            }));
            continue;
        }
        match create_daily_journal(state, tz, journal_date).await {
            Ok(entry) => created.push(entry),
            Err(err) => {
                warn!(
                    journal_date = %journal_date,
                    "daily strategy journal generation failed: {err:#}"
                );
                return Err(err);
            }
        }
    }

    let status = if created.is_empty() {
        "idle"
    } else {
        "created"
    };
    Ok(json!({
        "status": status,
        "timezone": tz.name(),
        "daily_time": daily_time.format("%H:%M").to_string(),
        "created": created,
        "skipped": skipped
    }))
}

fn journal_enabled(state: &AppState) -> bool {
    yaml_bool(&state.config, &["strategy", "swing", "journal", "enabled"]).unwrap_or(false)
}

fn journal_timezone(state: &AppState) -> Tz {
    yaml_string(&state.config, &["strategy", "swing", "journal", "timezone"])
        .or_else(|| yaml_string(&state.config, &["localization", "time_zone"]))
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen)
}

fn journal_daily_time(state: &AppState) -> NaiveTime {
    yaml_string(
        &state.config,
        &["strategy", "swing", "journal", "daily_time"],
    )
    .as_deref()
    .and_then(parse_hh_mm)
    .unwrap_or_else(|| parse_hh_mm(DEFAULT_DAILY_TIME).expect("default daily time is valid"))
}

fn parse_hh_mm(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn due_daily_journal_dates(now_local: DateTime<Tz>, daily_time: NaiveTime) -> Vec<NaiveDate> {
    let today = now_local.date_naive();
    let mut dates = Vec::new();

    // `NaiveDate` is a date without a timezone. We use it for the journal key,
    // then convert the day's local midnight to UTC when querying timestamp rows.
    for days_back in (1..=3).rev() {
        if let Some(date) = today.checked_sub_signed(Duration::days(days_back)) {
            if is_trading_weekday(date) {
                dates.push(date);
            }
        }
    }
    if now_local.time() >= daily_time && is_trading_weekday(today) {
        dates.push(today);
    }
    dates
}

fn is_trading_weekday(date: NaiveDate) -> bool {
    date.weekday().number_from_monday() <= 5
}

async fn journal_exists(state: &AppState, journal_date: NaiveDate, cadence: &str) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM strategy_journal_entries WHERE journal_date = '{}' AND cadence = '{}' LIMIT 1",
        sql_escape(&journal_date.to_string()),
        sql_escape(cadence)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

async fn create_daily_journal(
    state: &AppState,
    tz: Tz,
    journal_date: NaiveDate,
) -> Result<JsonValue> {
    let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (start_utc, end_utc) = local_day_bounds_utc(tz, journal_date);
    let reports = decision_reports_for_day(state, start_utc, end_utc).await?;
    let manager_runs = trading_manager_runs_for_day(state, start_utc, end_utc).await?;
    let portfolio = latest_portfolio_for_day(state, start_utc, end_utc).await?;

    let metrics = journal_metrics(&reports, &manager_runs, portfolio.as_ref());
    let source_report_id = reports
        .first()
        .and_then(|row| row.get("id").and_then(JsonValue::as_i64));
    let summary = journal_summary(journal_date, &metrics);
    let learnings = journal_learnings(&metrics);
    let diary_json = journal_diary_json(journal_date, &created_at, &summary, &metrics, &learnings);
    let source_report_sql = source_report_id
        .map(|id| id.max(0).to_string())
        .unwrap_or_else(|| "NULL".to_string());

    let sql = format!(
        "INSERT INTO strategy_journal_entries (
            created_at, journal_date, cadence, status, summary,
            metrics_json, learnings_json, source_report_id, diary_json
        ) VALUES (
            '{}', '{}', 'daily', 'rust_completed', '{}',
            '{}', '{}', {}, '{}'
        )",
        sql_escape(&created_at),
        sql_escape(&journal_date.to_string()),
        sql_escape(&summary),
        sql_escape(&serde_json::to_string(&metrics)?),
        sql_escape(&serde_json::to_string(&learnings)?),
        source_report_sql,
        sql_escape(&serde_json::to_string(&diary_json)?)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Rust daily strategy journal")?;
    info!(
        journal_date = %journal_date,
        source_report_id,
        "created Rust daily strategy journal"
    );
    Ok(json!({
        "journal_date": journal_date.to_string(),
        "cadence": "daily",
        "status": "rust_completed",
        "source_report_id": source_report_id,
        "summary": summary
    }))
}

fn local_day_bounds_utc(tz: Tz, date: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = local_time_utc(tz, date, NaiveTime::MIN);
    let next_date = date
        .checked_add_signed(Duration::days(1))
        .expect("journal dates remain in chrono range");
    let end = local_time_utc(tz, next_date, NaiveTime::MIN);
    (start, end)
}

fn local_time_utc(tz: Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let naive = date.and_time(time);
    // Time zones can be ambiguous during DST changes. Midnight is normally
    // unambiguous, but `earliest` still gives us a deterministic fallback.
    tz.from_local_datetime(&naive)
        .single()
        .or_else(|| tz.from_local_datetime(&naive).earliest())
        .expect("configured journal timezone can represent local midnight")
        .with_timezone(&Utc)
}

async fn decision_reports_for_day(
    state: &AppState,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> Result<Vec<JsonValue>> {
    let rows = sqlx::query(&format!(
        "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, report_json, error_text
         FROM decision_reports
         WHERE created_at >= '{}' AND created_at < '{}'
         ORDER BY created_at DESC, id DESC",
        sql_escape(&start_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        sql_escape(&end_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading decision reports for strategy journal")?;
    Ok(rows.iter().map(row_to_json).collect())
}

async fn trading_manager_runs_for_day(
    state: &AppState,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> Result<Vec<JsonValue>> {
    let rows = sqlx::query(&format!(
        "SELECT id, created_at, report_id, status, manager_json, queue_result_json, error_text
         FROM trading_manager_runs
         WHERE created_at >= '{}' AND created_at < '{}'
         ORDER BY created_at DESC, id DESC",
        sql_escape(&start_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        sql_escape(&end_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading Trading Manager runs for strategy journal")?;
    Ok(rows.iter().map(row_to_json).collect())
}

async fn latest_portfolio_for_day(
    state: &AppState,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> Result<Option<JsonValue>> {
    let row = sqlx::query(&format!(
        "SELECT recorded_at, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk,
                total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count, source
         FROM portfolio_value_history
         WHERE recorded_at >= '{}' AND recorded_at < '{}'
         ORDER BY recorded_at DESC, id DESC
         LIMIT 1",
        sql_escape(&start_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        sql_escape(&end_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    ))
    .fetch_optional(&state.pool)
    .await
    .context("loading portfolio snapshot for strategy journal")?;
    Ok(row.map(|row| row_to_json(&row)))
}

fn journal_metrics(
    reports: &[JsonValue],
    manager_runs: &[JsonValue],
    portfolio: Option<&JsonValue>,
) -> JsonValue {
    let mut suggested_trades = 0_i64;
    let mut selected_assets = 0_i64;
    let mut xai_errors = Vec::new();
    let mut pulse_labels = Vec::new();

    for report in reports {
        let report_json = parsed_json_field(report, "report_json");
        suggested_trades += strategy_order_count(&report_json);
        selected_assets += selected_asset_count(&report_json);
        if let Some(label) = report
            .get("analysis_pulse_label")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            pulse_labels.push(label.to_string());
        }
        if let Some(error) = report
            .get("error_text")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            xai_errors.push(error.to_string());
        }
    }

    let acted_runs = manager_runs
        .iter()
        .filter(|row| {
            matches!(
                row.get("status").and_then(JsonValue::as_str),
                Some("executed") | Some("queued") | Some("completed")
            )
        })
        .count() as i64;

    json!({
        "report_count": reports.len() as i64,
        "suggested_trade_count": suggested_trades,
        "selected_asset_count": selected_assets,
        "trading_manager_run_count": manager_runs.len() as i64,
        "trading_manager_acted_count": acted_runs,
        "pulse_labels": pulse_labels,
        "xai_errors": xai_errors,
        "portfolio": portfolio.map(|row| json!({
            "recorded_at": row.get("recorded_at").cloned().unwrap_or(JsonValue::Null),
            "total_market_value_dkk": value_f64(row, "total_market_value_dkk"),
            "cash_balance_dkk": value_f64(row, "cash_balance_dkk"),
            "total_unrealised_pnl_dkk": value_f64(row, "total_unrealised_pnl_dkk"),
            "total_daily_pnl_dkk": value_f64(row, "total_daily_pnl_dkk"),
            "position_count": value_i64(row, "position_count"),
            "source": row.get("source").cloned().unwrap_or(JsonValue::Null)
        })).unwrap_or(JsonValue::Null)
    })
}

fn strategy_order_count(report_json: &JsonValue) -> i64 {
    report_json
        .get("strategy_plan")
        .and_then(|value| value.get("swing_orders"))
        .and_then(JsonValue::as_array)
        .or_else(|| {
            report_json
                .get("suggested_trades")
                .and_then(JsonValue::as_array)
        })
        .map(|rows| rows.len() as i64)
        .unwrap_or(0)
}

fn selected_asset_count(report_json: &JsonValue) -> i64 {
    report_json
        .get("selected_assets")
        .and_then(JsonValue::as_array)
        .map(|rows| rows.len() as i64)
        .or_else(|| {
            report_json
                .get("selected_asset_count")
                .and_then(JsonValue::as_i64)
        })
        .unwrap_or(0)
}

fn parsed_json_field(row: &JsonValue, key: &str) -> JsonValue {
    let value = row.get(key).cloned().unwrap_or(JsonValue::Null);
    if let Some(text) = value.as_str() {
        serde_json::from_str(text).unwrap_or(JsonValue::Null)
    } else {
        value
    }
}

fn journal_summary(journal_date: NaiveDate, metrics: &JsonValue) -> String {
    let report_count = value_i64(metrics, "report_count");
    let suggested_trade_count = value_i64(metrics, "suggested_trade_count");
    let manager_run_count = value_i64(metrics, "trading_manager_run_count");
    let daily_pnl = metrics
        .get("portfolio")
        .and_then(|value| value.get("total_daily_pnl_dkk"))
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
        .unwrap_or(0.0);
    format!(
        "Daily strategy journal for {journal_date}: {report_count} decision report(s), {suggested_trade_count} suggested trade(s), {manager_run_count} Trading Manager run(s). Daily P/L {daily_pnl:.2} DKK."
    )
}

fn journal_learnings(metrics: &JsonValue) -> JsonValue {
    let report_count = value_i64(metrics, "report_count");
    let suggested_trade_count = value_i64(metrics, "suggested_trade_count");
    let manager_run_count = value_i64(metrics, "trading_manager_run_count");
    let mut learnings = Vec::new();
    if report_count == 0 {
        learnings.push("No decision report was available for this journal date; verify the scheduled decision report pulses.".to_string());
    } else {
        learnings.push(format!(
            "{report_count} decision report(s) were available for the trading day."
        ));
    }
    if suggested_trade_count > 0 && manager_run_count == 0 {
        learnings.push("Decision reports suggested trades but no Trading Manager run was recorded; investigate execution gating.".to_string());
    }
    if metrics.get("portfolio").is_none_or(|value| value.is_null()) {
        learnings.push(
            "No portfolio valuation snapshot was recorded for this journal date.".to_string(),
        );
    }
    JsonValue::Array(learnings.into_iter().map(JsonValue::from).collect())
}

fn journal_diary_json(
    journal_date: NaiveDate,
    created_at: &str,
    summary: &str,
    metrics: &JsonValue,
    learnings: &JsonValue,
) -> JsonValue {
    json!({
        "status": "rust_completed",
        "journal_date": journal_date.to_string(),
        "generated_at": created_at,
        "runtime": "rust_scheduler",
        "diary": {
            "executive_summary": summary,
            "what_went_well": learnings,
            "what_did_not_work": [],
            "next_session_adjustments": [
                "Review any decision report trades that were not acted on before the next market pulse."
            ],
            "decision_report_memory": learnings,
            "benchmark_readthrough": "Benchmark readthrough is not yet generated by the Rust scheduler."
        },
        "metrics": metrics
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_due_dates_include_recent_weekday_catchup_before_today_due_time() {
        let tz = chrono_tz::Europe::Copenhagen;
        let now = tz.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).single().unwrap();
        let due = due_daily_journal_dates(now, parse_hh_mm("22:30").unwrap());
        assert!(due.contains(&NaiveDate::from_ymd_opt(2026, 5, 11).unwrap()));
        assert!(!due.contains(&NaiveDate::from_ymd_opt(2026, 5, 12).unwrap()));
    }

    #[test]
    fn daily_due_dates_include_today_after_due_time() {
        let tz = chrono_tz::Europe::Copenhagen;
        let now = tz.with_ymd_and_hms(2026, 5, 12, 23, 0, 0).single().unwrap();
        let due = due_daily_journal_dates(now, parse_hh_mm("22:30").unwrap());
        assert!(due.contains(&NaiveDate::from_ymd_opt(2026, 5, 12).unwrap()));
    }

    #[test]
    fn weekend_dates_are_not_due() {
        let tz = chrono_tz::Europe::Copenhagen;
        let now = tz.with_ymd_and_hms(2026, 5, 11, 23, 0, 0).single().unwrap();
        let due = due_daily_journal_dates(now, parse_hh_mm("22:30").unwrap());
        assert!(!due.contains(&NaiveDate::from_ymd_opt(2026, 5, 9).unwrap()));
        assert!(!due.contains(&NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()));
        assert!(due.contains(&NaiveDate::from_ymd_opt(2026, 5, 11).unwrap()));
    }
}
