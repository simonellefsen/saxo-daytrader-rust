use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use chrono_tz::Tz;
use serde_json::{Value as JsonValue, json};
use sqlx::Row;

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64, value_i64},
    state::AppState,
};

const DEFAULT_ALERT_LOOKBACK_HOURS: i64 = 48;
const DEFAULT_ALERT_LIMIT: i64 = 50;

#[derive(Clone, Debug)]
struct SlackAlert {
    alert_key: String,
    summary_kind: String,
    severity: String,
    scope_key: String,
    subject: String,
    message_text: String,
    payload: JsonValue,
}

pub async fn dispatch_execution_notifications(state: &AppState) -> Result<JsonValue> {
    let alerts = pending_execution_alerts(state).await?;
    dispatch_slack_alerts(state, alerts).await
}

pub async fn dispatch_operational_notifications(state: &AppState) -> Result<JsonValue> {
    if !yaml_bool(
        &state.config,
        &["notifications", "alerts", "operational_alerts_enabled"],
    )
    .unwrap_or(true)
    {
        return Ok(json!({"status": "disabled", "reason": "operational_alerts_disabled"}));
    }

    let alerts = pending_operational_alerts(state).await?;
    dispatch_slack_alerts(state, alerts).await
}

async fn dispatch_slack_alerts(state: &AppState, alerts: Vec<SlackAlert>) -> Result<JsonValue> {
    if !yaml_bool(&state.config, &["notifications", "slack", "enabled"]).unwrap_or(false) {
        return Ok(json!({"status": "disabled", "reason": "slack_disabled"}));
    }

    if alerts.is_empty() {
        return Ok(json!({"status": "ok", "alerts": [], "sent": []}));
    }

    let webhook_url = yaml_string(&state.config, &["notifications", "slack", "webhook_url"])
        .ok_or_else(|| anyhow!("Slack webhook URL is missing"))?;
    let client = reqwest::Client::new();
    let mut sent = Vec::new();
    let mut failed = Vec::new();

    for alert in &alerts {
        match send_alert_to_slack(state, &client, &webhook_url, &alert).await {
            Ok(delivery_id) => sent.push(json!({
                "alert_key": alert.alert_key,
                "summary_kind": alert.summary_kind,
                "channel": "slack",
                "status": "sent",
                "delivery_id": delivery_id
            })),
            Err(err) => {
                let delivery_id = record_notification_delivery(
                    state,
                    &alert,
                    "failed",
                    json!({"alert_type": alert.payload.get("alert_type").cloned().unwrap_or(JsonValue::Null)}),
                    Some(&err.to_string()),
                )
                .await?;
                upsert_channel_state(state, &alert, "failed", Some(&err.to_string())).await?;
                failed.push(json!({
                    "alert_key": alert.alert_key,
                    "summary_kind": alert.summary_kind,
                    "channel": "slack",
                    "status": "failed",
                    "delivery_id": delivery_id,
                    "error": err.to_string()
                }));
            }
        }
    }

    Ok(json!({
        "status": if failed.is_empty() { "ok" } else { "error" },
        "sent": sent,
        "failed": failed
    }))
}

async fn pending_execution_alerts(state: &AppState) -> Result<Vec<SlackAlert>> {
    let limit = yaml_i64(
        &state.config,
        &["notifications", "alerts", "execution_alert_limit"],
    )
    .unwrap_or(DEFAULT_ALERT_LIMIT)
    .clamp(1, 250);
    let lookback_hours = yaml_i64(
        &state.config,
        &["notifications", "alerts", "execution_alert_lookback_hours"],
    )
    .unwrap_or(DEFAULT_ALERT_LOOKBACK_HOURS)
    .max(1);
    let cutoff = (Utc::now() - Duration::hours(lookback_hours))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows = sqlx::query(&format!(
        "SELECT *
         FROM execution_orders
         WHERE created_at >= '{}'
           AND status IN (
             'executed', 'submitted_to_broker', 'execution_failed', 'expired_local',
             'pending_approval', 'blocked_by_dry_run', 'invalid_quantity',
             'waiting_for_market_open', 'waiting_for_cash_settlement',
             'waiting_for_virtual_cash_budget'
           )
         ORDER BY id DESC
         LIMIT {}",
        sql_escape(&cutoff),
        limit
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading execution orders for Slack notifications")?;

    let mut alerts = Vec::new();
    for row in rows.iter().map(row_to_json) {
        let Some(alert) = alert_from_execution_order(state, &row) else {
            continue;
        };
        if alert_already_sent(state, &alert.scope_key).await? {
            continue;
        }
        alerts.push(alert);
    }
    alerts.reverse();
    Ok(alerts)
}

async fn pending_operational_alerts(state: &AppState) -> Result<Vec<SlackAlert>> {
    let mut alerts = Vec::new();

    if yaml_bool(
        &state.config,
        &["notifications", "alerts", "decision_failure_enabled"],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(state, &mut alerts, decision_failure_alert(state).await?).await?;
    }

    if yaml_bool(
        &state.config,
        &["notifications", "alerts", "execution_failure_burst_enabled"],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            execution_failure_burst_alert(state).await?,
        )
        .await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "monthly_loss_circuit_breaker_alert_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            monthly_loss_circuit_breaker_alert(state).await?,
        )
        .await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "drawdown_guardrail_alert_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(state, &mut alerts, drawdown_guardrail_alert(state).await?).await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "instrument_quarantine_alert_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            instrument_quarantine_alert(state).await?,
        )
        .await?;
    }

    if yaml_bool(
        &state.config,
        &["notifications", "alerts", "scheduler_stale_enabled"],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(state, &mut alerts, scheduler_stale_alert(state).await?).await?;
    }

    if yaml_bool(
        &state.config,
        &["notifications", "alerts", "shadow_pulse_missed_enabled"],
    )
    .unwrap_or(true)
    {
        for alert in shadow_pulse_missed_alerts(state).await? {
            maybe_push_unsent(state, &mut alerts, Some(alert)).await?;
        }
    }

    if yaml_bool(
        &state.config,
        &["notifications", "alerts", "integrity_alert_enabled"],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(state, &mut alerts, integrity_alert(state).await?).await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_eod_reflection_missed_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            hermes_eod_reflection_missed_alert(state).await?,
        )
        .await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            hermes_pending_experiment_review_alert(state).await?,
        )
        .await?;
    }

    if yaml_bool(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_digest_enabled",
        ],
    )
    .unwrap_or(true)
    {
        maybe_push_unsent(
            state,
            &mut alerts,
            hermes_pending_experiment_review_digest_alert(state).await?,
        )
        .await?;
    }

    Ok(alerts)
}

async fn maybe_push_unsent(
    state: &AppState,
    alerts: &mut Vec<SlackAlert>,
    alert: Option<SlackAlert>,
) -> Result<()> {
    let Some(alert) = alert else {
        return Ok(());
    };
    if !alert_already_sent(state, &alert.scope_key).await? {
        alerts.push(alert);
    }
    Ok(())
}

async fn decision_failure_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let threshold = yaml_i64(
        &state.config,
        &["notifications", "alerts", "decision_failure_threshold"],
    )
    .unwrap_or(2)
    .max(1);
    let window_hours = yaml_i64(
        &state.config,
        &["notifications", "alerts", "decision_failure_window_hours"],
    )
    .unwrap_or(24)
    .max(1);
    let cutoff = (Utc::now() - Duration::hours(window_hours))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS failure_count, MAX(id) AS latest_id, MAX(created_at) AS latest_created_at
         FROM decision_reports
         WHERE created_at >= '{}'
           AND status IN ('xai_error', 'error', 'failed', 'parse_error')",
        sql_escape(&cutoff)
    ))
    .fetch_optional(&state.pool)
    .await
    .context("checking repeated decision report failures")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let failure_count = row.try_get::<i64, _>("failure_count").unwrap_or(0);
    let latest_id = row.try_get::<i64, _>("latest_id").unwrap_or(0);
    if failure_count < threshold || latest_id <= 0 {
        return Ok(None);
    }
    let latest_created_at = row
        .try_get::<String, _>("latest_created_at")
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(Some(operational_alert(
        "decision_failures",
        format!("ops:decision_failures:latest:{latest_id}"),
        "high",
        "Decision report failures".to_string(),
        vec![
            "Repeated decision report failures detected.".to_string(),
            String::new(),
            format!("Failures: {failure_count} in the last {window_hours}h"),
            format!("Threshold: {threshold}"),
            format!("Latest report ID: {latest_id}"),
            format!("Latest failure time: {latest_created_at}"),
        ],
        json!({
            "failure_count": failure_count,
            "threshold": threshold,
            "window_hours": window_hours,
            "latest_report_id": latest_id,
            "latest_created_at": latest_created_at,
        }),
    )))
}

async fn execution_failure_burst_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let threshold = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "execution_failure_burst_threshold",
        ],
    )
    .unwrap_or(3)
    .max(1);
    let window_hours = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "execution_failure_burst_window_hours",
        ],
    )
    .unwrap_or(24)
    .max(1);
    let cutoff = (Utc::now() - Duration::hours(window_hours))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS failure_count, MAX(id) AS latest_id, MAX(created_at) AS latest_created_at
         FROM execution_orders
         WHERE created_at >= '{}'
           AND status = 'execution_failed'",
        sql_escape(&cutoff)
    ))
    .fetch_optional(&state.pool)
    .await
    .context("checking repeated execution failures")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let failure_count = row.try_get::<i64, _>("failure_count").unwrap_or(0);
    let latest_id = row.try_get::<i64, _>("latest_id").unwrap_or(0);
    if failure_count < threshold || latest_id <= 0 {
        return Ok(None);
    }
    let latest_created_at = row
        .try_get::<String, _>("latest_created_at")
        .unwrap_or_else(|_| "unknown".to_string());
    let latest_failure = sqlx::query(&format!(
        "SELECT execution_result_json
         FROM execution_orders
         WHERE id = {}",
        latest_id
    ))
    .fetch_optional(&state.pool)
    .await
    .context("loading latest execution failure taxonomy")?
    .map(|row| row_to_json(&row))
    .and_then(|row| execution_failure_taxonomy(&row));
    let latest_failure_label = latest_failure
        .as_ref()
        .and_then(|taxonomy| optional_text(taxonomy, "label"))
        .unwrap_or_else(|| "Unclassified execution failure".to_string());
    let latest_failure_code = latest_failure
        .as_ref()
        .and_then(|taxonomy| optional_text(taxonomy, "code"))
        .unwrap_or_else(|| "unknown".to_string());
    let latest_failure_remediation = latest_failure
        .as_ref()
        .and_then(|taxonomy| optional_text(taxonomy, "remediation"));
    let latest_failure_retry_policy = latest_failure
        .as_ref()
        .and_then(|taxonomy| optional_text(taxonomy, "retry_policy"));
    let mut lines = vec![
        "Repeated broker/local execution failures detected.".to_string(),
        String::new(),
        format!("Failures: {failure_count} in the last {window_hours}h"),
        format!("Threshold: {threshold}"),
        format!("Latest execution order ID: {latest_id}"),
        format!("Latest failure time: {latest_created_at}"),
        format!("Latest category: {latest_failure_label} ({latest_failure_code})"),
    ];
    if let Some(remediation) = latest_failure_remediation {
        lines.push(format!("Recommended action: {remediation}"));
    }
    if let Some(retry_policy) = latest_failure_retry_policy {
        lines.push(format!("Retry policy: {retry_policy}"));
    }
    Ok(Some(operational_alert(
        "execution_failure_burst",
        format!("ops:execution_failure_burst:latest:{latest_id}"),
        "high",
        "Execution failure burst".to_string(),
        lines,
        json!({
            "failure_count": failure_count,
            "threshold": threshold,
            "window_hours": window_hours,
            "latest_execution_order_id": latest_id,
            "latest_created_at": latest_created_at,
            "latest_failure_taxonomy": latest_failure,
        }),
    )))
}

async fn monthly_loss_circuit_breaker_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let rows = sqlx::query(
        "SELECT id, created_at, status, manager_json
         FROM trading_manager_runs
         ORDER BY created_at DESC, id DESC
         LIMIT 2",
    )
    .fetch_all(&state.pool)
    .await
    .context("checking latest Trading Manager monthly-loss circuit breaker state")?;
    let runs = rows.iter().map(row_to_json).collect::<Vec<_>>();
    Ok(monthly_loss_circuit_breaker_alert_from_runs(&runs))
}

fn monthly_loss_circuit_breaker_alert_from_runs(runs: &[JsonValue]) -> Option<SlackAlert> {
    let latest = runs.first()?;
    let latest_breaker = latest
        .get("manager_json")
        .and_then(|value| value.get("monthly_loss_circuit_breaker"))?;
    let latest_active = latest_breaker
        .get("active")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let previous_active = runs
        .get(1)
        .and_then(|run| run.get("manager_json"))
        .and_then(|value| value.get("monthly_loss_circuit_breaker"))
        .and_then(|breaker| breaker.get("active"))
        .and_then(JsonValue::as_bool);

    let transition = match (latest_active, previous_active) {
        (true, Some(true)) | (false, Some(false)) | (false, None) => return None,
        (true, _) => "activated",
        (false, Some(true)) => "cleared",
    };

    let latest_run_id = value_i64(latest, "id");
    let previous_run_id = runs.get(1).map(|run| value_i64(run, "id")).unwrap_or(0);
    let latest_created_at = fallback_text(latest, "created_at", "unknown");
    let month_pnl_dkk = value_f64(latest_breaker, "month_pnl_dkk");
    let threshold_dkk = value_f64(latest_breaker, "threshold_dkk");
    let status_line = if latest_active {
        "New BUYs are suspended while the breaker is active; SELLs remain allowed."
    } else {
        "The BUY suspension is no longer active under the latest Trading Manager run."
    };
    let subject = if latest_active {
        "Monthly-loss circuit breaker active"
    } else {
        "Monthly-loss circuit breaker cleared"
    };
    let severity = if latest_active { "high" } else { "medium" };
    Some(operational_alert(
        "monthly_loss_circuit_breaker",
        format!(
            "ops:monthly_loss_circuit_breaker:{transition}:run:{latest_run_id}:prev:{previous_run_id}"
        ),
        severity,
        subject.to_string(),
        vec![
            format!("Monthly-loss circuit breaker {transition}."),
            String::new(),
            status_line.to_string(),
            format!("Month P/L DKK: {month_pnl_dkk:.2}"),
            format!("Threshold DKK: {threshold_dkk:.2}"),
            format!("Latest manager run: #{latest_run_id} at {latest_created_at}"),
            format!("Previous manager run: #{previous_run_id}"),
        ],
        json!({
            "transition": transition,
            "active": latest_active,
            "month_pnl_dkk": month_pnl_dkk,
            "threshold_dkk": threshold_dkk,
            "latest_manager_run_id": latest_run_id,
            "latest_manager_run_created_at": latest_created_at,
            "previous_manager_run_id": previous_run_id,
        }),
    ))
}

async fn drawdown_guardrail_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let rows = sqlx::query(
        "SELECT id, created_at, status, manager_json
         FROM trading_manager_runs
         ORDER BY created_at DESC, id DESC
         LIMIT 2",
    )
    .fetch_all(&state.pool)
    .await
    .context("checking latest Trading Manager drawdown guardrail state")?;
    let runs = rows.iter().map(row_to_json).collect::<Vec<_>>();
    Ok(drawdown_guardrail_alert_from_runs(&runs))
}

/// Alert on the edges of the BUY suspension only.
///
/// A halt that nobody is told about looks exactly like a quiet market: no
/// orders, no errors, nothing on the dashboard demanding attention. That is the
/// specific failure this exists to prevent. Steady state stays silent so the
/// alert keeps meaning something.
fn drawdown_guardrail_alert_from_runs(runs: &[JsonValue]) -> Option<SlackAlert> {
    let latest = runs.first()?;
    let latest_guard = latest
        .get("manager_json")
        .and_then(|value| value.get("drawdown_guardrail"))?;
    let latest_active = latest_guard
        .get("active")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let previous_active = runs
        .get(1)
        .and_then(|run| run.get("manager_json"))
        .and_then(|value| value.get("drawdown_guardrail"))
        .and_then(|guard| guard.get("active"))
        .and_then(JsonValue::as_bool);

    let transition = match (latest_active, previous_active) {
        (true, Some(true)) | (false, Some(false)) | (false, None) => return None,
        (true, _) => "activated",
        (false, Some(true)) => "cleared",
    };

    let latest_run_id = value_i64(latest, "id");
    let previous_run_id = runs.get(1).map(|run| value_i64(run, "id")).unwrap_or(0);
    let latest_created_at = fallback_text(latest, "created_at", "unknown");
    let drawdown_pct = value_f64(latest_guard, "drawdown_pct") * 100.0;
    let halt_pct = value_f64(latest_guard, "halt_pct") * 100.0;
    let peak_value_dkk = value_f64(latest_guard, "peak_value_dkk");
    let current_value_dkk = value_f64(latest_guard, "current_value_dkk");
    let peak_at = fallback_text(latest_guard, "peak_at", "unknown");
    let status_line = if latest_active {
        "New BUYs are suspended while the guardrail is active; SELLs remain allowed."
    } else {
        "The BUY suspension is no longer active under the latest Trading Manager run."
    };
    let subject = if latest_active {
        "Portfolio drawdown guardrail active"
    } else {
        "Portfolio drawdown guardrail cleared"
    };
    let severity = if latest_active { "high" } else { "medium" };
    Some(operational_alert(
        "drawdown_guardrail",
        format!("ops:drawdown_guardrail:{transition}:run:{latest_run_id}:prev:{previous_run_id}"),
        severity,
        subject.to_string(),
        vec![
            format!("Portfolio drawdown guardrail {transition}."),
            String::new(),
            status_line.to_string(),
            format!("Drawdown: {drawdown_pct:.2}% (floor {halt_pct:.2}%)"),
            format!("Peak DKK: {peak_value_dkk:.2} at {peak_at}"),
            format!("Current DKK: {current_value_dkk:.2}"),
            format!("Latest manager run: #{latest_run_id} at {latest_created_at}"),
            format!("Previous manager run: #{previous_run_id}"),
        ],
        json!({
            "transition": transition,
            "active": latest_active,
            "drawdown_pct": value_f64(latest_guard, "drawdown_pct"),
            "halt_pct": value_f64(latest_guard, "halt_pct"),
            "peak_value_dkk": peak_value_dkk,
            "current_value_dkk": current_value_dkk,
            "latest_manager_run_id": latest_run_id,
            "latest_manager_run_created_at": latest_created_at,
            "previous_manager_run_id": previous_run_id,
        }),
    ))
}

async fn instrument_quarantine_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let row = sqlx::query(
        "SELECT id, created_at, status, manager_json
         FROM trading_manager_runs
         ORDER BY created_at DESC, id DESC
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await
    .context("checking latest Trading Manager quarantine state")?;
    Ok(row
        .as_ref()
        .map(row_to_json)
        .and_then(|run| instrument_quarantine_alert_from_run(&run)))
}

fn instrument_quarantine_alert_from_run(run: &JsonValue) -> Option<SlackAlert> {
    let quarantine = run
        .get("manager_json")
        .and_then(|value| value.get("instrument_quarantine"))?;
    if !quarantine
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let active = json_array(quarantine, "active");
    let active_count = value_i64(quarantine, "active_count").max(active.len() as i64);
    if active_count <= 0 || active.is_empty() {
        return None;
    }

    let mut active_keys = active
        .iter()
        .map(|row| {
            format!(
                "{}:{}:{}",
                fallback_text(row, "symbol", "unknown"),
                fallback_text(row, "action", "n/a"),
                fallback_text(row, "signature", "unknown")
            )
        })
        .collect::<Vec<_>>();
    active_keys.sort();
    active_keys.dedup();
    let active_scope = active_keys.join("|");
    let run_id = value_i64(run, "id");
    let run_created_at = fallback_text(run, "created_at", "unknown");
    let lookback_days = value_i64(quarantine, "lookback_days");
    let min_failures = value_i64(quarantine, "min_failures");
    let active_days = value_i64(quarantine, "active_days");
    let row_lines = active
        .iter()
        .take(8)
        .map(|row| instrument_quarantine_summary_line(row))
        .collect::<Vec<_>>();
    let mut lines = vec![
        "Derived instrument quarantine is active.".to_string(),
        String::new(),
        format!("Active quarantines: {active_count}"),
        format!("Lookback: {lookback_days}d"),
        format!("Minimum matching failures: {min_failures}"),
        format!("Active window: {active_days}d"),
        format!("Latest manager run: #{run_id} at {run_created_at}"),
    ];
    if !row_lines.is_empty() {
        lines.push(String::new());
        lines.push("Blocked symbol/action signatures:".to_string());
        lines.extend(row_lines);
    }

    Some(operational_alert(
        "instrument_quarantine_active",
        format!("ops:instrument_quarantine_active:active:{active_scope}:count:{active_count}"),
        "medium",
        "Instrument quarantine active".to_string(),
        lines,
        json!({
            "active_count": active_count,
            "active_keys": active_keys,
            "lookback_days": lookback_days,
            "min_failures": min_failures,
            "active_days": active_days,
            "latest_manager_run_id": run_id,
            "latest_manager_run_created_at": run_created_at,
        }),
    ))
}

fn instrument_quarantine_summary_line(row: &JsonValue) -> String {
    let symbol = fallback_text(row, "symbol", "unknown");
    let action = fallback_text(row, "action", "n/a");
    let signature = fallback_text(row, "signature", "unknown");
    let failure_count = value_i64(row, "failure_count");
    let latest_failure_at = fallback_text(row, "latest_failure_at", "unknown");
    let expires_at = fallback_text(row, "expires_at", "unknown");
    format!(
        "- {symbol} {action}: {signature}, failures {failure_count}, latest {latest_failure_at}, expires {expires_at}"
    )
}

async fn scheduler_stale_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let stale_minutes = yaml_i64(
        &state.config,
        &["notifications", "alerts", "scheduler_stale_minutes"],
    )
    .unwrap_or(30)
    .max(1);
    let row = sqlx::query(
        "SELECT last_heartbeat_at, last_cycle_completed_at, last_cycle_status
         FROM scheduler_status
         WHERE singleton_key = 'main'
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await
    .context("checking scheduler freshness")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let last_cycle_completed_at = row.try_get::<String, _>("last_cycle_completed_at").ok();
    let last_heartbeat_at = row.try_get::<String, _>("last_heartbeat_at").ok();
    let reference = last_cycle_completed_at
        .as_deref()
        .and_then(parse_utc_time)
        .or_else(|| last_heartbeat_at.as_deref().and_then(parse_utc_time));
    let Some(reference) = reference else {
        return Ok(None);
    };
    let age_minutes = (Utc::now() - reference).num_minutes();
    if age_minutes < stale_minutes {
        return Ok(None);
    }
    let last_cycle_status = row
        .try_get::<String, _>("last_cycle_status")
        .unwrap_or_else(|_| "unknown".to_string());
    let reference_text = reference.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Ok(Some(operational_alert(
        "scheduler_stale",
        format!("ops:scheduler_stale:{reference_text}"),
        "high",
        "Scheduler stale".to_string(),
        vec![
            "The latest recorded scheduler completion is stale.".to_string(),
            String::new(),
            format!("Last observed: {reference_text}"),
            format!("Age: {age_minutes} minutes"),
            format!("Threshold: {stale_minutes} minutes"),
            format!("Last cycle status: {last_cycle_status}"),
        ],
        json!({
            "last_observed_at": reference_text,
            "age_minutes": age_minutes,
            "threshold_minutes": stale_minutes,
            "last_cycle_status": last_cycle_status,
        }),
    )))
}

async fn shadow_pulse_missed_alerts(state: &AppState) -> Result<Vec<SlackAlert>> {
    Ok(
        crate::xai_decision::missed_shadow_pulse_alert_candidates(state)
            .await?
            .iter()
            .filter_map(shadow_pulse_missed_alert_from_candidate)
            .collect(),
    )
}

fn shadow_pulse_missed_alert_from_candidate(candidate: &JsonValue) -> Option<SlackAlert> {
    let pulse = candidate.get("pulse")?;
    let key = optional_text(pulse, "key")?;
    let label = fallback_text(pulse, "label", "Shadow Decision Report");
    let due_at_utc = fallback_text(pulse, "target_at_utc", "unknown");
    let due_at_local = fallback_text(pulse, "target_at_local", "unknown");
    let schedule_time_zone = fallback_text(pulse, "schedule_time_zone", "unknown");
    let market_scope = fallback_text(pulse, "market_scope_status", "unknown");
    Some(operational_alert(
        "shadow_pulse_missed",
        format!("ops:shadow_pulse_missed:{key}"),
        "medium",
        format!("Missed {label}"),
        vec![
            "An eligible shadow Decision Report passed its due window without a persisted report."
                .to_string(),
            String::new(),
            format!("Pulse: {key}"),
            format!("Due UTC: {due_at_utc}"),
            format!("Due local: {due_at_local} ({schedule_time_zone})"),
            format!("Market scope at check: {market_scope}"),
            "This is observation-only: no provider retry, Trading Manager queue, or Saxo action was attempted."
                .to_string(),
        ],
        json!({
            "pulse_key": key,
            "pulse_label": label,
            "target_at_utc": due_at_utc,
            "target_at_local": due_at_local,
            "schedule_time_zone": schedule_time_zone,
            "market_scope_status": market_scope,
            "scheduler_status": candidate.get("scheduler_status").cloned().unwrap_or(JsonValue::Null),
            "safety": candidate.get("safety").cloned().unwrap_or(JsonValue::Null),
        }),
    ))
}

async fn integrity_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let overview = state
        .overview_payload()
        .await
        .context("building overview payload for integrity alert")?;
    Ok(overview
        .get("integrity")
        .and_then(integrity_alert_from_payload))
}

fn integrity_alert_from_payload(integrity: &JsonValue) -> Option<SlackAlert> {
    let mismatches = json_array(integrity, "mismatches");
    let warnings = json_array(integrity, "warnings");
    let expiry_pending_orders = json_array(integrity, "expiry_pending_orders");
    if mismatches.is_empty() && warnings.is_empty() && expiry_pending_orders.is_empty() {
        return None;
    }

    let mismatch_count = mismatches.len();
    let warning_count = warnings.len();
    let expiry_pending_count = expiry_pending_orders.len();
    let issue_codes = integrity_issue_codes(&mismatches, &warnings);
    let issue_code_text = if issue_codes.is_empty() {
        "none".to_string()
    } else {
        issue_codes.join(", ")
    };
    let expiry_order_ids = expiry_pending_orders
        .iter()
        .map(|order| value_i64(order, "id"))
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let expiry_scope = if expiry_order_ids.is_empty() {
        "none".to_string()
    } else {
        expiry_order_ids.join(",")
    };
    let severity = if mismatch_count > 0 { "high" } else { "medium" };
    let subject = if mismatch_count > 0 {
        "Overview integrity mismatch"
    } else if expiry_pending_count > 0 {
        "DayOrder expiry sync pending"
    } else {
        "Overview integrity warning"
    };
    let checked_at = fallback_text(integrity, "checked_at", "unknown");
    let order_lines = expiry_pending_orders
        .iter()
        .take(5)
        .map(|order| integrity_order_summary(order))
        .collect::<Vec<_>>();
    let mut lines = vec![
        "Overview integrity checks reported issues.".to_string(),
        String::new(),
        format!("Severity: {severity}"),
        format!("Mismatches: {mismatch_count}"),
        format!("Warnings: {warning_count}"),
        format!("Expiry-pending DayOrders: {expiry_pending_count}"),
        format!("Issue codes: {issue_code_text}"),
        format!("Checked at: {checked_at}"),
    ];
    if !order_lines.is_empty() {
        lines.push(String::new());
        lines.push("Orders needing broker sync confirmation:".to_string());
        lines.extend(order_lines);
    }

    let scope_codes = if issue_codes.is_empty() {
        "no-code".to_string()
    } else {
        issue_codes.join(",")
    };
    Some(operational_alert(
        "overview_integrity",
        format!(
            "ops:overview_integrity:{severity}:codes:{scope_codes}:expiry:{expiry_scope}:counts:{mismatch_count}:{warning_count}:{expiry_pending_count}"
        ),
        severity,
        subject.to_string(),
        lines,
        json!({
            "mismatch_count": mismatch_count,
            "warning_count": warning_count,
            "expiry_pending_count": expiry_pending_count,
            "issue_codes": issue_codes,
            "expiry_pending_order_ids": expiry_order_ids,
            "checked_at": checked_at,
        }),
    ))
}

fn json_array<'a>(value: &'a JsonValue, key: &str) -> Vec<&'a JsonValue> {
    value
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

fn integrity_issue_codes(mismatches: &[&JsonValue], warnings: &[&JsonValue]) -> Vec<String> {
    let mut codes = mismatches
        .iter()
        .chain(warnings.iter())
        .filter_map(|issue| optional_text(issue, "code"))
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn integrity_order_summary(order: &JsonValue) -> String {
    let id = value_i64(order, "id");
    let symbol = fallback_text(order, "symbol", "unknown");
    let status = fallback_text(order, "status", "unknown");
    let action = fallback_text(order, "action", "n/a");
    let expiry = fallback_text(order, "expected_expiry_at_utc", "unknown");
    if id > 0 {
        format!("- #{id} {symbol} {action} {status}, expected expiry {expiry}")
    } else {
        format!("- {symbol} {action} {status}, expected expiry {expiry}")
    }
}

async fn hermes_eod_reflection_missed_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let due_hour_utc = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_eod_reflection_due_hour_utc",
        ],
    )
    .unwrap_or(22)
    .clamp(0, 23);
    let now = Utc::now();
    if i64::from(now.hour()) < due_hour_utc {
        return Ok(None);
    }
    let day = now.date_naive();
    let day_start = format!("{day}T00:00:00Z");
    let row = sqlx::query(&format!(
        "SELECT COUNT(*) AS reflection_count, MAX(created_at) AS latest_created_at
         FROM hermes_reflections
         WHERE created_at >= '{}'",
        sql_escape(&day_start)
    ))
    .fetch_optional(&state.pool)
    .await
    .context("checking Hermes EOD reflection freshness")?;
    let reflection_count = row
        .as_ref()
        .and_then(|row| row.try_get::<i64, _>("reflection_count").ok())
        .unwrap_or(0);
    if reflection_count > 0 {
        return Ok(None);
    }
    Ok(Some(operational_alert(
        "hermes_eod_reflection_missed",
        format!("ops:hermes_eod_reflection_missed:{day}"),
        "medium",
        "Hermes EOD reflection missing".to_string(),
        vec![
            "No Hermes reflection has been recorded for the current UTC day after the expected EOD deadline.".to_string(),
            String::new(),
            format!("Date: {day}"),
            format!("Due hour UTC: {due_hour_utc:02}:00"),
        ],
        json!({
            "date": day.to_string(),
            "due_hour_utc": due_hour_utc,
        }),
    )))
}

async fn hermes_pending_experiment_review_alert(state: &AppState) -> Result<Option<SlackAlert>> {
    let stale_days = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_stale_days",
        ],
    )
    .unwrap_or(14)
    .max(1);
    let limit = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_limit",
        ],
    )
    .unwrap_or(10)
    .clamp(1, 50);
    let cutoff = (Utc::now() - Duration::days(stale_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows = sqlx::query(&format!(
        "SELECT id, created_at, status, changed_variable_path, hypothesis, source_session_id
         FROM strategy_experiments
         WHERE status = 'pending_review'
           AND created_at <= '{}'
         ORDER BY created_at ASC, id ASC
         LIMIT {}",
        sql_escape(&cutoff),
        limit
    ))
    .fetch_all(&state.pool)
    .await
    .context("checking stale Hermes experiment proposals")?;
    let rows = rows.iter().map(row_to_json).collect::<Vec<_>>();
    Ok(hermes_pending_experiment_review_alert_from_rows(
        &rows,
        stale_days,
        Utc::now(),
    ))
}

fn hermes_pending_experiment_review_alert_from_rows(
    rows: &[JsonValue],
    stale_days: i64,
    now: DateTime<Utc>,
) -> Option<SlackAlert> {
    let pending = rows
        .iter()
        .filter(|row| fallback_text(row, "status", "") == "pending_review")
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return None;
    }

    let mut ids = pending
        .iter()
        .map(|row| fallback_text(row, "id", "unknown"))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    let scope = ids.join(",");
    let oldest_created_at = pending
        .iter()
        .filter_map(|row| optional_text(row, "created_at"))
        .min()
        .unwrap_or_else(|| "unknown".to_string());
    let oldest_age_days = parse_utc_time(&oldest_created_at)
        .map(|created| (now - created).num_days().max(0))
        .unwrap_or(0);
    let row_lines = pending
        .iter()
        .take(8)
        .map(|row| hermes_experiment_review_summary_line(row, now))
        .collect::<Vec<_>>();
    let variable_paths = pending
        .iter()
        .filter_map(|row| optional_text(row, "changed_variable_path"))
        .collect::<Vec<_>>();

    let mut lines = vec![
        "Hermes has stale one-variable experiment proposals waiting for operator review."
            .to_string(),
        String::new(),
        format!("Pending stale proposals: {}", pending.len()),
        format!("Stale threshold: {stale_days}d"),
        format!("Oldest proposal: {oldest_created_at} ({oldest_age_days}d old)"),
        String::new(),
        "Review, reject, or merge duplicates so the self-improvement loop can close.".to_string(),
    ];
    if !row_lines.is_empty() {
        lines.push(String::new());
        lines.push("Oldest pending proposals:".to_string());
        lines.extend(row_lines);
    }

    Some(operational_alert(
        "hermes_pending_experiment_review",
        format!("ops:hermes_pending_experiment_review:stale_days:{stale_days}:ids:{scope}"),
        "medium",
        "Hermes experiment review overdue".to_string(),
        lines,
        json!({
            "pending_count": pending.len(),
            "stale_days": stale_days,
            "oldest_created_at": oldest_created_at,
            "oldest_age_days": oldest_age_days,
            "experiment_ids": ids,
            "changed_variable_paths": variable_paths,
        }),
    ))
}

fn hermes_experiment_review_summary_line(row: &JsonValue, now: DateTime<Utc>) -> String {
    let id = fallback_text(row, "id", "unknown");
    let variable = fallback_text(row, "changed_variable_path", "unknown_variable");
    let created_at = fallback_text(row, "created_at", "unknown");
    let age_days = parse_utc_time(&created_at)
        .map(|created| (now - created).num_days().max(0))
        .unwrap_or(0);
    let source = fallback_text(row, "source_session_id", "unknown_source");
    format!("- {id}: {variable}, created {created_at} ({age_days}d), source {source}")
}

async fn hermes_pending_experiment_review_digest_alert(
    state: &AppState,
) -> Result<Option<SlackAlert>> {
    let timezone = yaml_string(&state.config, &["notifications", "timezone"])
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let weekday_local = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_digest_weekday_local",
        ],
    )
    .unwrap_or(1)
    .clamp(1, 7) as u32;
    let hour_local = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_digest_hour_local",
        ],
    )
    .unwrap_or(9)
    .clamp(0, 23) as u32;
    let review_stale_days = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_stale_days",
        ],
    )
    .unwrap_or(14)
    .max(1);
    let expiry_days = yaml_i64(
        &state.config,
        &["hermes", "experiments", "auto_expire_pending_review_days"],
    )
    .unwrap_or(30)
    .max(1);
    let limit = yaml_i64(
        &state.config,
        &[
            "notifications",
            "alerts",
            "hermes_pending_experiment_review_digest_limit",
        ],
    )
    .unwrap_or(10)
    .clamp(1, 50);
    let now = Utc::now();
    if !hermes_experiment_review_digest_is_due(now, timezone, weekday_local, hour_local) {
        return Ok(None);
    }
    let cutoff = (now - Duration::days(review_stale_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let summary = sqlx::query(&format!(
        "SELECT COUNT(*) AS pending_count,
                SUM(CASE WHEN created_at <= '{}' THEN 1 ELSE 0 END) AS overdue_count
         FROM strategy_experiments
         WHERE status = 'pending_review'",
        sql_escape(&cutoff),
    ))
    .fetch_one(&state.pool)
    .await
    .context("counting Hermes experiment review digest")?;
    let pending_count = summary.try_get::<i64, _>("pending_count").unwrap_or(0);
    if pending_count <= 0 {
        return Ok(None);
    }
    let overdue_count = summary
        .try_get::<i64, _>("overdue_count")
        .unwrap_or(0)
        .max(0);

    let rows = sqlx::query(&format!(
        "SELECT id, created_at, status, changed_variable_path, source_session_id
         FROM strategy_experiments
         WHERE status = 'pending_review'
         ORDER BY created_at ASC, id ASC
         LIMIT {}",
        limit
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading Hermes experiment review digest")?;
    let rows = rows.iter().map(row_to_json).collect::<Vec<_>>();
    Ok(
        hermes_pending_experiment_review_digest_alert_from_rows_with_counts(
            &rows,
            pending_count as usize,
            overdue_count as usize,
            review_stale_days,
            expiry_days,
            now,
            timezone,
        ),
    )
}

fn hermes_experiment_review_digest_is_due(
    now: DateTime<Utc>,
    timezone: Tz,
    weekday_local: u32,
    hour_local: u32,
) -> bool {
    let local = now.with_timezone(&timezone);
    local.weekday().number_from_monday() == weekday_local && local.hour() >= hour_local
}

fn hermes_pending_experiment_review_digest_alert_from_rows_with_counts(
    rows: &[JsonValue],
    pending_count: usize,
    overdue_count: usize,
    review_stale_days: i64,
    expiry_days: i64,
    now: DateTime<Utc>,
    timezone: Tz,
) -> Option<SlackAlert> {
    let pending = rows
        .iter()
        .filter(|row| fallback_text(row, "status", "") == "pending_review")
        .collect::<Vec<_>>();
    if pending.is_empty() || pending_count == 0 {
        return None;
    }
    let mut ids = pending
        .iter()
        .map(|row| fallback_text(row, "id", "unknown"))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    let variable_paths = pending
        .iter()
        .filter_map(|row| optional_text(row, "changed_variable_path"))
        .collect::<Vec<_>>();
    let local = now.with_timezone(&timezone);
    let iso_week = local.iso_week();
    let week_label = format!("{}-W{:02}", iso_week.year(), iso_week.week());
    let row_lines = pending
        .iter()
        .take(8)
        .map(|row| hermes_experiment_review_summary_line(row, now))
        .collect::<Vec<_>>();

    let mut lines = vec![
        "Weekly Hermes experiment review digest. These are proposal artifacts only; this digest cannot alter a strategy, baseline, or broker state.".to_string(),
        String::new(),
        format!("Pending proposals: {pending_count}"),
        format!("Overdue for review (>= {review_stale_days}d): {overdue_count}"),
        format!("Automatic closure: {expiry_days}d as expired_stale"),
        String::new(),
        "Review, reject, or merge duplicates before the configured closure window.".to_string(),
    ];
    if !row_lines.is_empty() {
        lines.push(String::new());
        lines.push("Oldest pending proposals:".to_string());
        lines.extend(row_lines);
    }

    Some(operational_alert(
        "hermes_pending_experiment_review_digest",
        format!("ops:hermes_pending_experiment_review_digest:{week_label}"),
        if overdue_count > 0 { "medium" } else { "low" },
        "Hermes experiment review digest".to_string(),
        lines,
        json!({
            "week": week_label,
            "pending_count": pending_count,
            "overdue_count": overdue_count,
            "review_stale_days": review_stale_days,
            "expiry_days": expiry_days,
            "experiment_ids": ids,
            "changed_variable_paths": variable_paths,
        }),
    ))
}

fn operational_alert(
    kind: &str,
    scope_key: String,
    severity: &str,
    subject: String,
    lines: Vec<String>,
    details: JsonValue,
) -> SlackAlert {
    SlackAlert {
        alert_key: scope_key.clone(),
        summary_kind: "alert_operational_issue".to_string(),
        severity: severity.to_string(),
        scope_key,
        subject,
        message_text: lines.join("\n"),
        payload: json!({
            "alert_type": "operational_issue",
            "kind": kind,
            "details": details,
        }),
    }
}

fn alert_from_execution_order(state: &AppState, row: &JsonValue) -> Option<SlackAlert> {
    let status = text(row, "status");
    let source = execution_source_label(row);
    let (summary_kind, severity, subject_prefix) = match status.as_str() {
        "executed" | "submitted_to_broker"
            if yaml_bool(
                &state.config,
                &["notifications", "alerts", "execution_success_enabled"],
            )
            .unwrap_or(false) =>
        {
            (
                "alert_execution_success",
                "medium",
                if status == "executed" {
                    execution_success_prefix(&source)
                } else {
                    execution_submitted_prefix(&source)
                },
            )
        }
        "execution_failed" | "expired_local"
            if yaml_bool(
                &state.config,
                &["notifications", "alerts", "execution_failure_enabled"],
            )
            .unwrap_or(false) =>
        {
            ("alert_execution_failed", "high", "Execution failed")
        }
        "pending_approval"
        | "blocked_by_dry_run"
        | "invalid_quantity"
        | "waiting_for_market_open"
        | "waiting_for_cash_settlement"
        | "waiting_for_virtual_cash_budget"
            if yaml_bool(
                &state.config,
                &["notifications", "alerts", "execution_warning_enabled"],
            )
            .unwrap_or(false) =>
        {
            ("alert_execution_warning", "low", "Execution warning")
        }
        _ => return None,
    };

    let id = row.get("id").and_then(JsonValue::as_i64).unwrap_or(0);
    if id <= 0 {
        return None;
    }
    let symbol = text(row, "symbol");
    let action = text(row, "action");
    let quantity = value_f64(row, "quantity");
    let estimated_value = value_f64(row, "estimated_value_dkk");
    let broker_order_id = fallback_text(row, "broker_order_id", "n/a");
    let subject = format!("{subject_prefix} for {symbol}");
    let mut lines = vec![
        subject.clone(),
        String::new(),
        format!("Execution order ID: {id}"),
        format!("Mode: {}", fallback_text(row, "mode", "n/a")),
        format!("Action: {action}"),
        format!("Source: {source}"),
        format!("Quantity: {}", format_quantity(quantity)),
        format!("Status: {status}"),
        format!("Estimated value DKK: {estimated_value:.2}"),
        format!("Broker Order ID: {broker_order_id}"),
    ];
    if matches!(status.as_str(), "execution_failed" | "expired_local") {
        let taxonomy = execution_failure_taxonomy(row);
        let label = taxonomy
            .as_ref()
            .and_then(|taxonomy| optional_text(taxonomy, "label"))
            .unwrap_or_else(|| "Unclassified execution failure".to_string());
        let code = taxonomy
            .as_ref()
            .and_then(|taxonomy| optional_text(taxonomy, "code"))
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("Failure category: {label} ({code})"));
        if let Some(remediation) = taxonomy
            .as_ref()
            .and_then(|taxonomy| optional_text(taxonomy, "remediation"))
        {
            lines.push(format!("Recommended action: {remediation}"));
        }
        if let Some(retry_policy) = taxonomy
            .as_ref()
            .and_then(|taxonomy| optional_text(taxonomy, "retry_policy"))
        {
            lines.push(format!("Retry policy: {retry_policy}"));
        }
    }
    let alert_key = if matches!(status.as_str(), "execution_failed" | "expired_local") {
        format!("execution_failed:{id}:{status}")
    } else if summary_kind == "alert_execution_warning" {
        format!("execution_warning:{id}:{status}")
    } else {
        format!("execution_success:{id}:{status}")
    };
    Some(SlackAlert {
        alert_key,
        summary_kind: summary_kind.to_string(),
        severity: severity.to_string(),
        scope_key: format!("{summary_kind}:order:{id}"),
        subject,
        message_text: lines.join("\n"),
        payload: json!({
            "alert_type": summary_kind.trim_start_matches("alert_"),
            "record": {
                "id": id,
                "created_at": row.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "symbol": symbol,
                "action": action,
                "status": status,
                "quantity": quantity,
                "estimated_value_dkk": estimated_value,
                "broker_order_id": row.get("broker_order_id").cloned().unwrap_or(JsonValue::Null),
                "ledger_id": row.get("ledger_id").cloned().unwrap_or(JsonValue::Null),
                "failure_taxonomy": if status == "execution_failed" {
                    execution_failure_taxonomy(row)
                } else {
                    None
                },
            }
        }),
    })
}

async fn send_alert_to_slack(
    state: &AppState,
    client: &reqwest::Client,
    webhook_url: &str,
    alert: &SlackAlert,
) -> Result<i64> {
    let response = client
        .post(webhook_url)
        .json(&json!({"text": format!("*{}*\n{}", alert.subject, alert.message_text)}))
        .send()
        .await
        .context("sending Slack notification")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Slack webhook returned {status}: {body}"));
    }

    let delivery_id = record_notification_delivery(
        state,
        alert,
        "sent",
        json!({
            "alert_type": alert.payload.get("alert_type").cloned().unwrap_or(JsonValue::Null),
            "delivery_meta": {"status_code": status.as_u16()}
        }),
        None,
    )
    .await?;
    upsert_channel_state(state, alert, "sent", None).await?;
    upsert_alert_state(state, alert, delivery_id).await?;
    Ok(delivery_id)
}

async fn alert_already_sent(state: &AppState, scope_key: &str) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT scope_key FROM notification_alert_state WHERE scope_key = '{}' LIMIT 1",
        sql_escape(scope_key)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

async fn record_notification_delivery(
    state: &AppState,
    alert: &SlackAlert,
    status: &str,
    delivery_payload: JsonValue,
    error_text: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let payload = json!({
        "alert": alert.payload,
        "delivery": delivery_payload
    });
    sqlx::query(&format!(
        "INSERT INTO notification_deliveries (
            created_at, summary_date, channel, status, subject, message_text,
            payload_json, error_text, summary_kind
         ) VALUES (
            '{}', '{}', 'slack', '{}', '{}', '{}', '{}', {}, '{}'
         )",
        sql_escape(&now),
        sql_escape(&alert.alert_key),
        sql_escape(status),
        sql_escape(&alert.subject),
        sql_escape(&alert.message_text),
        sql_escape(&serde_json::to_string(&payload)?),
        sql_opt_text(error_text),
        sql_escape(&alert.summary_kind)
    ))
    .execute(&state.pool)
    .await
    .context("recording Slack notification delivery")?;
    let row = sqlx::query(
        "SELECT id FROM notification_deliveries WHERE channel = 'slack' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<i64, _>("id").ok())
        .unwrap_or(0))
}

async fn upsert_channel_state(
    state: &AppState,
    alert: &SlackAlert,
    status: &str,
    error_text: Option<&str>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    sqlx::query(&format!(
        "INSERT INTO notification_channel_state (
            channel, summary_date, last_attempt_at, next_attempt_after,
            attempt_count, last_status, last_error_text
         ) VALUES (
            'slack', '{}', '{}', NULL, 1, '{}', {}
         )
         ON CONFLICT(channel) DO UPDATE SET
            summary_date = excluded.summary_date,
            last_attempt_at = excluded.last_attempt_at,
            next_attempt_after = excluded.next_attempt_after,
            attempt_count = notification_channel_state.attempt_count + 1,
            last_status = excluded.last_status,
            last_error_text = excluded.last_error_text",
        sql_escape(&alert.alert_key),
        sql_escape(&now),
        sql_escape(status),
        sql_opt_text(error_text)
    ))
    .execute(&state.pool)
    .await
    .context("upserting Slack notification channel state")?;
    Ok(())
}

async fn upsert_alert_state(state: &AppState, alert: &SlackAlert, delivery_id: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    sqlx::query(&format!(
        "INSERT INTO notification_alert_state (
            scope_key, severity, last_sent_at, last_alert_key, last_summary_kind, last_delivery_id
         ) VALUES (
            '{}', '{}', '{}', '{}', '{}', {}
         )
         ON CONFLICT(scope_key) DO UPDATE SET
            severity = excluded.severity,
            last_sent_at = excluded.last_sent_at,
            last_alert_key = excluded.last_alert_key,
            last_summary_kind = excluded.last_summary_kind,
            last_delivery_id = excluded.last_delivery_id",
        sql_escape(&alert.scope_key),
        sql_escape(&alert.severity),
        sql_escape(&now),
        sql_escape(&alert.alert_key),
        sql_escape(&alert.summary_kind),
        delivery_id
    ))
    .execute(&state.pool)
    .await
    .context("upserting Slack notification alert state")?;
    Ok(())
}

fn execution_source_label(row: &JsonValue) -> String {
    match text(row, "strategy_type").as_str() {
        "portfolio_sync" => "SIM portfolio sync".to_string(),
        // A stop fill is an exit nobody decided in the moment, so the alert has
        // to name it as such. "Execution" would read like a routine order.
        "protective_stop" => "Protective stop".to_string(),
        "swing" | "ladder" => "Trading Manager".to_string(),
        _ => "Execution".to_string(),
    }
}

/// Extracts only the allow-listed fields from the persisted Saxo error
/// taxonomy. Raw broker diagnostics and local error text must never enter a
/// Slack alert payload.
fn execution_failure_taxonomy(row: &JsonValue) -> Option<JsonValue> {
    let taxonomy = row
        .get("execution_result_json")
        .and_then(|payload| payload.get("error_taxonomy"))?;
    let code = optional_text(taxonomy, "code")?;
    let label = optional_text(taxonomy, "label")?;
    let remediation = optional_text(taxonomy, "remediation")?;
    let retry_policy = optional_text(taxonomy, "retry_policy")?;
    Some(json!({
        "code": code,
        "label": label,
        "remediation": remediation,
        "retry_policy": retry_policy,
    }))
}

fn execution_success_prefix(source: &str) -> &str {
    if source == "Execution" {
        "Trade executed"
    } else {
        "Trading Manager executed"
    }
}

fn execution_submitted_prefix(source: &str) -> &str {
    if source == "Execution" {
        "Trade submitted to broker"
    } else {
        "Trading Manager submitted to broker"
    }
}

fn format_quantity(quantity: f64) -> String {
    if (quantity.fract()).abs() < 0.0001 {
        format!("{quantity:.0}")
    } else {
        format!("{quantity:.4}")
    }
}

fn optional_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn text(value: &JsonValue, key: &str) -> String {
    optional_text(value, key).unwrap_or_default()
}

fn fallback_text(value: &JsonValue, key: &str, fallback: &str) -> String {
    optional_text(value, key).unwrap_or_else(|| fallback.to_string())
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn parse_utc_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_integer_and_fractional_quantities() {
        assert_eq!(format_quantity(20.0), "20");
        assert_eq!(format_quantity(1.23456), "1.2346");
    }

    #[test]
    fn execution_failure_taxonomy_keeps_slack_payloads_safe() {
        let row = json!({
            "error_text": "Authorization: Bearer super-secret-token; raw broker diagnostics",
            "execution_result_json": {
                "error_taxonomy": {
                    "code": "tick_size",
                    "label": "Invalid tick size",
                    "remediation": "Recalculate the limit or stop price using Saxo's instrument tick scheme.",
                    "retry_policy": "review_and_resubmit",
                    "raw_broker_body": "must not leave the execution record"
                }
            }
        });

        let taxonomy = execution_failure_taxonomy(&row).expect("safe taxonomy");
        assert_eq!(taxonomy["code"], "tick_size");
        assert_eq!(taxonomy["retry_policy"], "review_and_resubmit");
        assert!(!taxonomy.to_string().contains("super-secret-token"));
        assert!(!taxonomy.to_string().contains("raw_broker_body"));
    }

    #[test]
    fn parses_utc_scheduler_timestamps() {
        let parsed = parse_utc_time("2026-07-04T12:00:00Z").expect("valid timestamp");
        assert_eq!(
            parsed.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "2026-07-04T12:00:00Z"
        );
        assert!(parse_utc_time("not-a-timestamp").is_none());
    }

    #[test]
    fn skips_clear_integrity_payloads() {
        let alert = integrity_alert_from_payload(&json!({
            "warnings": [],
            "mismatches": [],
            "expiry_pending_orders": [],
            "checked_at": "2026-07-09T16:00:00Z"
        }));
        assert!(alert.is_none());
    }

    #[test]
    fn builds_day_order_expiry_integrity_alert() {
        let alert = integrity_alert_from_payload(&json!({
            "warnings": [{
                "code": "day_order_expiry_sync_pending",
                "severity": "warning",
                "message": "One or more Saxo DayOrders passed expected exchange-calendar expiry."
            }],
            "mismatches": [],
            "expiry_pending_orders": [{
                "id": 204,
                "symbol": "BAC:xnys",
                "action": "BUY",
                "status": "broker_working",
                "expected_expiry_at_utc": "2026-07-09T20:00:00Z"
            }],
            "checked_at": "2026-07-09T20:10:00Z"
        }))
        .expect("alert");
        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "medium");
        assert_eq!(alert.subject, "DayOrder expiry sync pending");
        assert!(alert.scope_key.contains("day_order_expiry_sync_pending"));
        assert!(alert.scope_key.contains("expiry:204"));
        assert!(
            alert
                .message_text
                .contains("#204 BAC:xnys BUY broker_working")
        );
    }

    #[test]
    fn builds_high_severity_integrity_mismatch_alert() {
        let alert = integrity_alert_from_payload(&json!({
            "warnings": [],
            "mismatches": [{
                "code": "portfolio_identity_mismatch",
                "severity": "error",
                "message": "Portfolio total does not match invested value plus cash."
            }],
            "expiry_pending_orders": [],
            "checked_at": "2026-07-09T16:00:00Z"
        }))
        .expect("alert");
        assert_eq!(alert.severity, "high");
        assert_eq!(alert.subject, "Overview integrity mismatch");
        assert!(alert.message_text.contains("Mismatches: 1"));
        assert!(alert.scope_key.contains("portfolio_identity_mismatch"));
    }

    #[test]
    fn builds_monthly_loss_activation_alert() {
        let runs = vec![
            json!({
                "id": 151,
                "created_at": "2026-07-09T16:55:00Z",
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": true,
                        "month_pnl_dkk": -28277.40,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
            json!({
                "id": 150,
                "created_at": "2026-07-09T15:55:00Z",
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": false,
                        "month_pnl_dkk": -9777.40,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
        ];
        let alert = monthly_loss_circuit_breaker_alert_from_runs(&runs).expect("alert");
        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "high");
        assert_eq!(alert.subject, "Monthly-loss circuit breaker active");
        assert!(
            alert
                .message_text
                .contains("New BUYs are suspended while the breaker is active")
        );
        assert!(alert.scope_key.contains("activated:run:151:prev:150"));
    }

    fn drawdown_run(id: i64, created_at: &str, active: bool) -> JsonValue {
        json!({
            "id": id,
            "created_at": created_at,
            "manager_json": {
                "drawdown_guardrail": {
                    "status": if active { "halt" } else { "clear" },
                    "active": active,
                    "drawdown_pct": if active { 0.223 } else { 0.041 },
                    "halt_pct": 0.20,
                    "peak_value_dkk": 318_400.0,
                    "current_value_dkk": if active { 247_400.0 } else { 305_350.0 },
                    "peak_at": "2026-06-14T21:00:00Z"
                }
            }
        })
    }

    #[test]
    fn builds_drawdown_guardrail_activation_alert() {
        let runs = vec![
            drawdown_run(212, "2026-07-26T16:55:00Z", true),
            drawdown_run(211, "2026-07-26T15:55:00Z", false),
        ];
        let alert = drawdown_guardrail_alert_from_runs(&runs).expect("alert");
        assert_eq!(alert.severity, "high");
        assert_eq!(alert.subject, "Portfolio drawdown guardrail active");
        assert!(
            alert
                .message_text
                .contains("New BUYs are suspended while the guardrail is active")
        );
        assert!(alert.message_text.contains("22.30% (floor 20.00%)"));
        assert!(alert.scope_key.contains("activated:run:212:prev:211"));
    }

    #[test]
    fn drawdown_guardrail_alerts_only_on_the_edges_of_the_suspension() {
        // A halt that stays on must not re-alert every cycle, or the alert
        // stops carrying information and gets muted -- which is how a real
        // suspension ends up unnoticed.
        assert!(
            drawdown_guardrail_alert_from_runs(&[
                drawdown_run(212, "2026-07-26T16:55:00Z", true),
                drawdown_run(211, "2026-07-26T15:55:00Z", true),
            ])
            .is_none()
        );
        assert!(
            drawdown_guardrail_alert_from_runs(&[
                drawdown_run(212, "2026-07-26T16:55:00Z", false),
                drawdown_run(211, "2026-07-26T15:55:00Z", false),
            ])
            .is_none()
        );

        let cleared = drawdown_guardrail_alert_from_runs(&[
            drawdown_run(212, "2026-07-26T16:55:00Z", false),
            drawdown_run(211, "2026-07-26T15:55:00Z", true),
        ])
        .expect("alert");
        assert_eq!(cleared.subject, "Portfolio drawdown guardrail cleared");
        assert_eq!(cleared.severity, "medium");
    }

    #[test]
    fn a_run_without_the_guardrail_block_produces_no_alert() {
        // Runs recorded before the guardrail shipped have no block at all;
        // reading that absence as "cleared" would fire a spurious alert on the
        // first cycle after deploy.
        assert!(
            drawdown_guardrail_alert_from_runs(&[json!({
                "id": 210,
                "created_at": "2026-07-25T16:55:00Z",
                "manager_json": {}
            })])
            .is_none()
        );
    }

    #[test]
    fn skips_repeated_monthly_loss_active_state() {
        let runs = vec![
            json!({
                "id": 152,
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": true,
                        "month_pnl_dkk": -28277.40,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
            json!({
                "id": 151,
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": true,
                        "month_pnl_dkk": -28100.00,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
        ];
        assert!(monthly_loss_circuit_breaker_alert_from_runs(&runs).is_none());
    }

    #[test]
    fn builds_monthly_loss_clear_alert() {
        let runs = vec![
            json!({
                "id": 161,
                "created_at": "2026-08-01T09:15:00Z",
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": false,
                        "month_pnl_dkk": 0.0,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
            json!({
                "id": 160,
                "created_at": "2026-07-31T20:45:00Z",
                "manager_json": {
                    "monthly_loss_circuit_breaker": {
                        "active": true,
                        "month_pnl_dkk": -28277.40,
                        "threshold_dkk": -10000.0
                    }
                }
            }),
        ];
        let alert = monthly_loss_circuit_breaker_alert_from_runs(&runs).expect("alert");
        assert_eq!(alert.severity, "medium");
        assert_eq!(alert.subject, "Monthly-loss circuit breaker cleared");
        assert!(
            alert
                .message_text
                .contains("BUY suspension is no longer active")
        );
        assert!(alert.scope_key.contains("cleared:run:161:prev:160"));
    }

    #[test]
    fn skips_clear_instrument_quarantine_runs() {
        let alert = instrument_quarantine_alert_from_run(&json!({
            "id": 88,
            "created_at": "2026-07-09T16:00:00Z",
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": true,
                    "active_count": 0,
                    "active": []
                }
            }
        }));
        assert!(alert.is_none());
    }

    #[test]
    fn skips_disabled_instrument_quarantine_runs() {
        let alert = instrument_quarantine_alert_from_run(&json!({
            "id": 88,
            "created_at": "2026-07-09T16:00:00Z",
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": false,
                    "active_count": 1,
                    "active": [{
                        "symbol": "BAC:xnys",
                        "action": "BUY",
                        "signature": "broker_rejected"
                    }]
                }
            }
        }));
        assert!(alert.is_none());
    }

    #[test]
    fn builds_instrument_quarantine_alert_without_raw_error_text() {
        let alert = instrument_quarantine_alert_from_run(&json!({
            "id": 148,
            "created_at": "2026-07-09T16:48:00Z",
            "manager_json": {
                "instrument_quarantine": {
                    "enabled": true,
                    "lookback_days": 14,
                    "min_failures": 3,
                    "active_days": 14,
                    "active_count": 1,
                    "active": [{
                        "symbol": "BAC:xnys",
                        "action": "BUY",
                        "signature": "broker_rejected",
                        "failure_count": 3,
                        "latest_failure_at": "2026-07-09T16:48:00Z",
                        "expires_at": "2026-07-23T16:48:00Z",
                        "sample_error": "raw broker body should stay out of Slack"
                    }]
                }
            }
        }))
        .expect("alert");
        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "medium");
        assert_eq!(alert.subject, "Instrument quarantine active");
        assert!(alert.scope_key.contains("BAC:xnys:BUY:broker_rejected"));
        assert!(alert.message_text.contains("BAC:xnys BUY: broker_rejected"));
        assert!(!alert.message_text.contains("raw broker body"));
        assert!(!alert.payload.to_string().contains("sample_error"));
    }

    #[test]
    fn skips_clear_hermes_pending_experiment_review_alert() {
        let alert = hermes_pending_experiment_review_alert_from_rows(
            &[json!({
                "id": "strategy-experiment-new",
                "created_at": "2026-07-09T18:00:00Z",
                "status": "approved_paper",
                "changed_variable_path": "strategy.swing.daily_indicators.min_confluences"
            })],
            14,
            DateTime::parse_from_rfc3339("2026-07-09T20:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert!(alert.is_none());
    }

    #[test]
    fn builds_hermes_pending_experiment_review_alert() {
        let now = DateTime::parse_from_rfc3339("2026-07-09T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let alert = hermes_pending_experiment_review_alert_from_rows(
            &[
                json!({
                    "id": "strategy-experiment-1",
                    "created_at": "2026-06-16T12:00:00Z",
                    "status": "pending_review",
                    "changed_variable_path": "strategy.swing.daily_indicators.min_confluences",
                    "hypothesis": "raise confluence threshold",
                    "source_session_id": "weekly-reflection-2026-06-16",
                    "raw_payload_json": {"must_not": "surface"}
                }),
                json!({
                    "id": "strategy-experiment-2",
                    "created_at": "2026-06-20T12:00:00Z",
                    "status": "pending_review",
                    "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                    "source_session_id": "daily-eod-reflection-2026-06-20"
                }),
            ],
            14,
            now,
        )
        .expect("alert");

        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "medium");
        assert_eq!(alert.subject, "Hermes experiment review overdue");
        assert!(alert.scope_key.contains("strategy-experiment-1"));
        assert!(alert.scope_key.contains("strategy-experiment-2"));
        assert!(alert.message_text.contains("Pending stale proposals: 2"));
        assert!(
            alert
                .message_text
                .contains("strategy.swing.daily_indicators.min_confluences")
        );
        assert!(!alert.message_text.contains("must_not"));
        assert!(!alert.payload.to_string().contains("raw_payload"));
    }

    #[test]
    fn sends_hermes_review_digest_on_configured_local_weekday_after_due_hour() {
        let timezone = chrono_tz::Europe::Copenhagen;
        let monday_after_due = DateTime::parse_from_rfc3339("2026-07-20T07:15:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let monday_before_due = DateTime::parse_from_rfc3339("2026-07-20T06:59:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let tuesday_after_due = DateTime::parse_from_rfc3339("2026-07-21T07:15:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert!(hermes_experiment_review_digest_is_due(
            monday_after_due,
            timezone,
            1,
            9
        ));
        assert!(!hermes_experiment_review_digest_is_due(
            monday_before_due,
            timezone,
            1,
            9
        ));
        assert!(!hermes_experiment_review_digest_is_due(
            tuesday_after_due,
            timezone,
            1,
            9
        ));
    }

    #[test]
    fn builds_sanitized_hermes_pending_review_digest() {
        let now = DateTime::parse_from_rfc3339("2026-07-20T07:15:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let alert = hermes_pending_experiment_review_digest_alert_from_rows_with_counts(
            &[
                json!({
                    "id": "strategy-experiment-old",
                    "created_at": "2026-07-01T12:00:00Z",
                    "status": "pending_review",
                    "changed_variable_path": "strategy.swing.daily_indicators.min_confluences",
                    "source_session_id": "weekly-reflection-2026-06-27",
                    "raw_payload_json": {"must_not": "surface"}
                }),
                json!({
                    "id": "strategy-experiment-fresh",
                    "created_at": "2026-07-18T12:00:00Z",
                    "status": "pending_review",
                    "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                    "source_session_id": "daily-eod-reflection-2026-07-18"
                }),
                json!({
                    "id": "strategy-experiment-closed",
                    "created_at": "2026-06-01T12:00:00Z",
                    "status": "expired_stale",
                    "changed_variable_path": "execution.min_trade_value_dkk"
                }),
            ],
            7,
            3,
            14,
            30,
            now,
            chrono_tz::Europe::Copenhagen,
        )
        .expect("alert");

        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "medium");
        assert_eq!(alert.subject, "Hermes experiment review digest");
        assert!(alert.scope_key.ends_with("2026-W30"));
        assert!(alert.message_text.contains("Pending proposals: 7"));
        assert!(
            alert
                .message_text
                .contains("Overdue for review (>= 14d): 3")
        );
        assert!(
            alert
                .message_text
                .contains("Automatic closure: 30d as expired_stale")
        );
        assert!(
            alert
                .message_text
                .contains("strategy.swing.daily_indicators.min_confluences")
        );
        assert!(!alert.message_text.contains("must_not"));
        assert!(!alert.payload.to_string().contains("raw_payload"));
        assert!(
            !alert
                .payload
                .to_string()
                .contains("strategy-experiment-closed")
        );
    }

    #[test]
    fn builds_one_observation_only_alert_for_a_missed_shadow_pulse() {
        let alert = shadow_pulse_missed_alert_from_candidate(&json!({
            "pulse": {
                "key": "us_mid_session_shadow:2026-08-19",
                "label": "US 14:15 Shadow Decision Report",
                "target_at_utc": "2026-08-19T18:15:00Z",
                "target_at_local": "2026-08-19T14:15:00-04:00",
                "schedule_time_zone": "America/New_York",
                "market_scope_status": "regular_tradable"
            },
            "scheduler_status": "missed_due_window",
            "safety": "scheduler_observability_only_no_provider_retry_queue_or_saxo_authority"
        }))
        .expect("alert");

        assert_eq!(alert.summary_kind, "alert_operational_issue");
        assert_eq!(alert.severity, "medium");
        assert_eq!(
            alert.scope_key,
            "ops:shadow_pulse_missed:us_mid_session_shadow:2026-08-19"
        );
        assert!(alert.message_text.contains("no provider retry"));
        assert!(alert.message_text.contains("or Saxo action"));
        assert_eq!(
            alert.payload["details"]["scheduler_status"],
            "missed_due_window"
        );
    }
}
