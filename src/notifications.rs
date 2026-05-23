use anyhow::{Context, Result, anyhow};
use chrono::{Duration, Utc};
use serde_json::{Value as JsonValue, json};

use crate::{
    config::{yaml_bool, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64},
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
    if !yaml_bool(&state.config, &["notifications", "slack", "enabled"]).unwrap_or(false) {
        return Ok(json!({"status": "disabled", "reason": "slack_disabled"}));
    }

    let alerts = pending_execution_alerts(state).await?;
    if alerts.is_empty() {
        return Ok(json!({"status": "ok", "alerts": [], "sent": []}));
    }

    let webhook_url = yaml_string(&state.config, &["notifications", "slack", "webhook_url"])
        .ok_or_else(|| anyhow!("Slack webhook URL is missing"))?;
    let client = reqwest::Client::new();
    let mut sent = Vec::new();
    let mut failed = Vec::new();

    for alert in alerts {
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

use sqlx::Row;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_integer_and_fractional_quantities() {
        assert_eq!(format_quantity(20.0), "20");
        assert_eq!(format_quantity(1.23456), "1.2346");
    }
}
