use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Timelike, Utc};
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
             'executed', 'submitted_to_broker', 'execution_failed',
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
    Ok(Some(operational_alert(
        "execution_failure_burst",
        format!("ops:execution_failure_burst:latest:{latest_id}"),
        "high",
        "Execution failure burst".to_string(),
        vec![
            "Repeated broker/local execution failures detected.".to_string(),
            String::new(),
            format!("Failures: {failure_count} in the last {window_hours}h"),
            format!("Threshold: {threshold}"),
            format!("Latest execution order ID: {latest_id}"),
            format!("Latest failure time: {latest_created_at}"),
        ],
        json!({
            "failure_count": failure_count,
            "threshold": threshold,
            "window_hours": window_hours,
            "latest_execution_order_id": latest_id,
            "latest_created_at": latest_created_at,
        }),
    )))
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
        "execution_failed"
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
    let error_text = fallback_text(row, "error_text", "n/a");
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
    if status == "execution_failed" {
        lines.push(format!("Error: {error_text}"));
    }
    let alert_key = if status == "execution_failed" {
        format!("execution_failed:{id}")
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
        "swing" | "ladder" => "Trading Manager".to_string(),
        _ => "Execution".to_string(),
    }
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
}
