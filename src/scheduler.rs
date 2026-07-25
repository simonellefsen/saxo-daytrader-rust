use std::{
    env,
    time::{Duration, Instant},
};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    daily_indicators::run_daily_indicators_cycle,
    editorial_research::run_editorial_research_cycle,
    markov_method::run_markov_method_cycle,
    notifications::{dispatch_execution_notifications, dispatch_operational_notifications},
    quiver::run_quiver_signal_cycle,
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
    let fast_interval_minutes = env::var("SCHEDULER_FAST_INTERVAL_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1)
        .max(1);
    let state = AppState::load().await?;
    info!(
        interval_minutes,
        fast_interval_minutes, "starting Rust scheduler"
    );
    tokio::spawn(crate::price_monitor::run_price_monitor_loop(state.clone()));
    run_cycle(&state).await?;
    loop {
        let sleep_minutes =
            next_interval_minutes(&state, interval_minutes, fast_interval_minutes).await;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("scheduler shutdown requested");
                return Ok(());
            }
            _ = sleep(Duration::from_secs(sleep_minutes * 60)) => {
                run_cycle(&state).await?;
            }
        }
    }
}

/// Fast poll while orders are queued or awaiting broker sync so fills land
/// within ~a minute instead of waiting out the full idle interval.
async fn next_interval_minutes(state: &AppState, normal: u64, fast: u64) -> u64 {
    match crate::saxo_order::outstanding_order_count(state).await {
        Ok(outstanding) if outstanding > 0 => {
            info!(
                outstanding,
                fast_interval_minutes = fast,
                "outstanding orders detected; scheduler switching to fast polling"
            );
            fast.min(normal)
        }
        Ok(_) => normal,
        Err(err) => {
            warn!("outstanding order check failed; using normal interval: {err:#}");
            normal
        }
    }
}

async fn run_cycle(state: &AppState) -> Result<()> {
    let started_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let cycle_started = Instant::now();
    let mut step_durations = JsonMap::new();
    info!("scheduler cycle started");
    if let Err(err) = state.update_scheduler_heartbeat().await {
        warn!("scheduler heartbeat persistence failed: {err:#}");
    }
    let step_started = Instant::now();
    let hermes_experiment_expiry = match state.expire_stale_hermes_experiments().await {
        Ok(value) => {
            let expired_count = value
                .get("expired_count")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            if expired_count > 0 {
                info!(expired_count, "expired stale Hermes experiment proposals");
            }
            value
        }
        Err(err) => {
            warn!("Hermes experiment expiry failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "hermes_experiment_expiry",
        step_started,
    );
    let step_started = Instant::now();
    let saxo = maintain_saxo_session(state).await;
    record_step_duration(&mut step_durations, "saxo_session", step_started);
    let step_started = Instant::now();
    let broker_read_model = match refresh_broker_snapshots(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker read model refresh failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "broker_read_model", step_started);
    let step_started = Instant::now();
    let broker_order_sync = match sync_saxo_broker_orders(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker order sync failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "broker_order_sync", step_started);
    let step_started = Instant::now();
    let decision_reports = match run_xai_decision_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("xAI decision report cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "decision_reports", step_started);
    let step_started = Instant::now();
    let trading_manager = match run_trading_manager_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Trading Manager cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "trading_manager", step_started);
    let step_started = Instant::now();
    let markov_method = match run_markov_method_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Markov method cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "markov_method", step_started);
    let step_started = Instant::now();
    let quiver_signals = match run_quiver_signal_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Quiver signal cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "quiver_signals", step_started);
    let step_started = Instant::now();
    let editorial_research = match run_editorial_research_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("editorial research cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "editorial_research", step_started);
    let step_started = Instant::now();
    let daily_indicators = match run_daily_indicators_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("daily indicators cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "daily_indicators", step_started);
    let step_started = Instant::now();
    let execution_queue = match run_saxo_execution_queue(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo execution queue failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "execution_queue", step_started);
    let step_started = Instant::now();
    let broker_order_sync_after_execution = match sync_saxo_broker_orders(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("Saxo broker order sync after execution failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "broker_order_sync_after_execution",
        step_started,
    );
    let step_started = Instant::now();
    let portfolio_value_snapshot = match state
        .record_portfolio_value_snapshot(
            "scheduler_cycle",
            None,
            "rust_scheduler",
            json!({"reason": "scheduler_cycle"}),
        )
        .await
    {
        Ok(value) => value,
        Err(err) => {
            warn!("portfolio value snapshot failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "portfolio_value_snapshot",
        step_started,
    );
    let step_started = Instant::now();
    let notifications = match dispatch_execution_notifications(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("execution notification dispatch failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "notifications", step_started);
    let step_started = Instant::now();
    let operational_notifications = match dispatch_operational_notifications(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("operational notification dispatch failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "operational_notifications",
        step_started,
    );
    let step_started = Instant::now();
    let journal = match run_strategy_journal_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!("strategy journal cycle failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "journal", step_started);
    let status = if trading_manager.get("status").and_then(JsonValue::as_str) == Some("error")
        || execution_queue.get("status").and_then(JsonValue::as_str) == Some("error")
        || broker_order_sync.get("status").and_then(JsonValue::as_str) == Some("error")
        || broker_order_sync_after_execution
            .get("status")
            .and_then(JsonValue::as_str)
            == Some("error")
        || notifications.get("status").and_then(JsonValue::as_str) == Some("error")
        || operational_notifications
            .get("status")
            .and_then(JsonValue::as_str)
            == Some("error")
        || portfolio_value_snapshot
            .get("status")
            .and_then(JsonValue::as_str)
            == Some("error")
        || journal.get("status").and_then(JsonValue::as_str) == Some("error")
    {
        "error"
    } else {
        "ok"
    };
    let step_started = Instant::now();
    let market = state
        .market_status_payload()
        .await
        .unwrap_or_else(|err| {
            warn!("scheduler market status snapshot failed: {err:#}");
            json!({"summary": {"analysis_window_active": false}})
        })
        .get("summary")
        .cloned()
        .unwrap_or_else(|| json!({"analysis_window_active": false}));
    record_step_duration(&mut step_durations, "market", step_started);
    let completed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let duration_ms = elapsed_ms(cycle_started.elapsed());
    let cycle_json = json!({
        "runtime": "rust",
        "duration_ms": duration_ms,
        "step_durations": step_durations,
        "hermes_experiment_expiry": hermes_experiment_expiry,
        "saxo_session": saxo,
        "broker_read_model": broker_read_model,
        "broker_order_sync": broker_order_sync,
        "decision_reports": decision_reports,
        "trading_manager": trading_manager,
        "markov_method": markov_method,
        "quiver_signals": quiver_signals,
        "editorial_research": editorial_research,
        "daily_indicators": daily_indicators,
        "execution_queue": execution_queue,
        "broker_order_sync_after_execution": broker_order_sync_after_execution,
        "portfolio_value_snapshot": portfolio_value_snapshot,
        "notifications": notifications,
        "operational_notifications": operational_notifications,
        "journal": journal,
        "market": market
    });
    state
        .record_scheduler_cycle(&started_at, &completed_at, status, &cycle_json)
        .await?;
    match state.prune_scheduler_cycles(Utc::now()).await {
        Ok(0) => {}
        Ok(deleted_rows) => info!(deleted_rows, "pruned scheduler cycle history"),
        Err(err) => warn!("scheduler cycle history prune failed: {err:#}"),
    }
    info!(status, "scheduler cycle completed");
    Ok(())
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn record_step_duration(steps: &mut JsonMap<String, JsonValue>, key: &str, started: Instant) {
    steps.insert(
        key.to_string(),
        json!({
            "duration_ms": elapsed_ms(started.elapsed())
        }),
    );
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
