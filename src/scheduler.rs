use std::{env, time::Duration};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    notifications::dispatch_execution_notifications,
    saxo_order::{run_saxo_execution_queue, sync_saxo_broker_orders},
    saxo_portfolio::refresh_broker_snapshots,
    state::AppState,
    strategy_journal::run_strategy_journal_cycle,
    trading_manager::run_trading_manager_cycle,
    xai_decision::run_xai_decision_cycle,
};

pub async fn run_scheduler() -> Result<()> {
    let interval_minutes = env::var("SCHEDULER_INTERVAL_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);
    let state = AppState::load().await?;
    info!(interval_minutes, "starting Rust scheduler");
    run_cycle(&state).await?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("scheduler shutdown requested");
                return Ok(());
            }
            _ = sleep(Duration::from_secs(interval_minutes * 60)) => {
                run_cycle(&state).await?;
            }
        }
    }
}

async fn run_cycle(state: &AppState) -> Result<()> {
    let started_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    info!("scheduler cycle started");
    if let Err(err) = state.update_scheduler_heartbeat().await {
        warn!("scheduler heartbeat persistence failed: {err:#}");
    }
    let saxo = maintain_saxo_session(state).await;
    let broker_read_model = match refresh_broker_snapshots(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker read model refresh failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let broker_order_sync = match sync_saxo_broker_orders(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker order sync failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let decision_reports = match run_xai_decision_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("xAI decision report cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let trading_manager = match run_trading_manager_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Trading Manager cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let execution_queue = match run_saxo_execution_queue(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo execution queue failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let broker_order_sync_after_execution = match sync_saxo_broker_orders(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker order sync after execution failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let notifications = match dispatch_execution_notifications(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("execution notification dispatch failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let journal = match run_strategy_journal_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("strategy journal cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    let completed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let status = if trading_manager.get("status").and_then(JsonValue::as_str) == Some("error")
        || execution_queue.get("status").and_then(JsonValue::as_str) == Some("error")
        || broker_order_sync.get("status").and_then(JsonValue::as_str) == Some("error")
        || broker_order_sync_after_execution
            .get("status")
            .and_then(JsonValue::as_str)
            == Some("error")
        || notifications.get("status").and_then(JsonValue::as_str) == Some("error")
        || journal.get("status").and_then(JsonValue::as_str) == Some("error")
    {
        "error"
    } else {
        "ok"
    };
    let cycle_json = json!({
        "runtime": "rust",
        "saxo_session": saxo,
        "broker_read_model": broker_read_model,
        "broker_order_sync": broker_order_sync,
        "decision_reports": decision_reports,
        "trading_manager": trading_manager,
        "execution_queue": execution_queue,
        "broker_order_sync_after_execution": broker_order_sync_after_execution,
        "notifications": notifications,
        "journal": journal,
        "market": state.market_status_payload().await.unwrap_or_else(|err| {
            warn!("scheduler market status snapshot failed: {err:#}");
            json!({"summary": {"analysis_window_active": false}})
        }).get("summary").cloned().unwrap_or_else(|| json!({"analysis_window_active": false}))
    });
    state
        .record_scheduler_cycle(&started_at, &completed_at, status, &cycle_json)
        .await?;
    info!(status, "scheduler cycle completed");
    Ok(())
}

async fn maintain_saxo_session(state: &AppState) -> JsonValue {
    match state.refresh_saxo_session().await {
        Ok(status) => {
            let status_key = status
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let status_text = status
                .get("status_text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Saxo session checked");
            let needs_reauth = status
                .get("needs_reauth")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let connected = status
                .get("connected")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if needs_reauth {
                warn!(
                    status = %status_key,
                    connected,
                    "Saxo session maintenance requires re-authentication: {status_text}"
                );
            } else {
                info!(
                    status = %status_key,
                    connected,
                    "Saxo session maintenance completed: {status_text}"
                );
            }
            status
        }
        Err(err) => {
            warn!("Saxo session maintenance failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    }
}
