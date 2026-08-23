use std::{
    env,
    future::Future,
    time::{Duration, Instant},
};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

use crate::{
    daily_indicators::run_daily_indicators_cycle,
    editorial_research::run_editorial_research_cycle,
    fx::run_fx_rate_refresh_cycle,
    markov_method::run_markov_method_cycle,
    notifications::{dispatch_execution_notifications, dispatch_operational_notifications},
    performance_benchmarks::run_performance_benchmark_cycle,
    protective_stops::run_automatic_protective_stop_sweep,
    quiver::run_quiver_signal_cycle,
    saxo_order::{backfill_saxo_ens_activities, run_saxo_execution_queue, sync_saxo_broker_orders},
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
    // Runs every cycle regardless of market hours or day of week -- unlike the
    // price monitor, which only reaches FX refresh while a watched exchange is
    // open. Every DKK conversion downstream in this same cycle depends on this
    // being current, so it runs before broker/ledger reads rather than after.
    let fx_rate_refresh = match run_fx_rate_refresh_cycle(state).await {
        Ok(value) => value,
        Err(err) => {
            warn!(
                "FX rate refresh failed; downstream conversions will use cached/static FX: {err:#}"
            );
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(&mut step_durations, "fx_rate_refresh", step_started);
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
    // A daily, bounded read of Saxo's broker-authored activity feed closes the
    // recovery gap between order polling and the later ENS streaming phase.
    // It is intentionally read-only and does not change orders, fills, or the
    // ledger; a partial page is surfaced in scheduler history for follow-up.
    let ens_activity_backfill = bounded_enrichment_step(
        "ens_activity_backfill",
        enrichment_step_timeout("ENS_ACTIVITY_BACKFILL", 45),
        backfill_saxo_ens_activities(state),
    )
    .await;
    record_step_duration(&mut step_durations, "ens_activity_backfill", step_started);
    let step_started = Instant::now();
    // Read-only confirmation of already-placed protective stops. It asks Saxo
    // what state each stop is in and records the answer; it cannot place,
    // amend, or cancel anything. Without it a stop stays at
    // `placement_submitted`, the coverage audit keeps reporting its position as
    // unprotected, and a later batch retries an order the broker already holds.
    crate::api::confirm_unconfirmed_protective_stops(state).await;
    record_step_duration(
        &mut step_durations,
        "protective_stop_confirmation",
        step_started,
    );
    let step_started = Instant::now();
    // Adoption runs after confirmation so a stop that was just promoted to
    // `broker_working` is picked up in the same cycle. It writes only local
    // rows -- the broker order already exists -- and from here on
    // `sync_saxo_broker_orders` above owns the stop, so a fill produces a
    // ledger row, a position update, and an execution notification like any
    // other order.
    let protective_stop_adoption = match state.adopt_protective_stops_into_execution_orders().await
    {
        Ok(adopted) => {
            if !adopted.is_empty() {
                info!(
                    adopted = adopted.len(),
                    "adopted broker-confirmed protective stops into the execution order table"
                );
            }
            json!({"status": "ok", "adopted": adopted.len(), "orders": adopted})
        }
        Err(err) => {
            warn!("Protective stop adoption failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "protective_stop_adoption",
        step_started,
    );
    let step_started = Instant::now();
    // Brings every held position's stop to the right size and level. Unlike the
    // two steps above this one can place and cancel broker orders, so it is off
    // unless `strategy.ladder.submit_stop_loss_after_fill` is set, SIM-only,
    // bounded per cycle, and halts on the first failure.
    let protective_stop_sweep = run_automatic_protective_stop_sweep(state).await;
    record_step_duration(&mut step_durations, "protective_stop_sweep", step_started);
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
    let markov_method = bounded_enrichment_step(
        "markov_method",
        enrichment_step_timeout("MARKOV", 240),
        run_markov_method_cycle(state),
    )
    .await;
    record_step_duration(&mut step_durations, "markov_method", step_started);
    let step_started = Instant::now();
    let quiver_signals = bounded_enrichment_step(
        "quiver_signals",
        enrichment_step_timeout("QUIVER", 45),
        run_quiver_signal_cycle(state),
    )
    .await;
    record_step_duration(&mut step_durations, "quiver_signals", step_started);
    let step_started = Instant::now();
    let editorial_research = bounded_enrichment_step(
        "editorial_research",
        enrichment_step_timeout("EDITORIAL", 45),
        run_editorial_research_cycle(state),
    )
    .await;
    record_step_duration(&mut step_durations, "editorial_research", step_started);
    let step_started = Instant::now();
    let daily_indicators = bounded_enrichment_step(
        "daily_indicators",
        enrichment_step_timeout("DAILY_INDICATORS", 240),
        run_daily_indicators_cycle(state),
    )
    .await;
    record_step_duration(&mut step_durations, "daily_indicators", step_started);
    let step_started = Instant::now();
    let performance_benchmarks = bounded_enrichment_step(
        "performance_benchmarks",
        enrichment_step_timeout("PERFORMANCE_BENCHMARKS", 75),
        run_performance_benchmark_cycle(state),
    )
    .await;
    record_step_duration(&mut step_durations, "performance_benchmarks", step_started);
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
    let portfolio_position_snapshot_prune = match state
        .prune_portfolio_position_snapshots(Utc::now())
        .await
    {
        Ok(deleted_rows) => json!({
            "status": "ok",
            "deleted_rows": deleted_rows,
            "retention_days": 90,
            "safety": "local_portfolio_position_snapshot_retention_no_provider_or_broker_authority",
        }),
        Err(err) => {
            warn!("portfolio position snapshot prune failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
    };
    record_step_duration(
        &mut step_durations,
        "portfolio_position_snapshot_prune",
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
        "fx_rate_refresh": fx_rate_refresh,
        "broker_read_model": broker_read_model,
        "broker_order_sync": broker_order_sync,
        "ens_activity_backfill": ens_activity_backfill,
        "protective_stop_adoption": protective_stop_adoption,
        "protective_stop_sweep": protective_stop_sweep,
        "decision_reports": decision_reports,
        "trading_manager": trading_manager,
        "markov_method": markov_method,
        "quiver_signals": quiver_signals,
        "editorial_research": editorial_research,
        "daily_indicators": daily_indicators,
        "performance_benchmarks": performance_benchmarks,
        "execution_queue": execution_queue,
        "broker_order_sync_after_execution": broker_order_sync_after_execution,
        "portfolio_value_snapshot": portfolio_value_snapshot,
        "portfolio_position_snapshot_prune": portfolio_position_snapshot_prune,
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

/// Bound only read-only/enrichment work. Trading and broker lifecycle paths
/// intentionally do not use this helper: timing out a mutation after it may
/// have reached Saxo would hide an ambiguous order state instead of making the
/// scheduler safer.
async fn bounded_enrichment_step<F>(step: &str, budget: Duration, future: F) -> JsonValue
where
    F: Future<Output = Result<JsonValue>>,
{
    match timeout(budget, future).await {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            warn!(step, "scheduler enrichment step failed: {err:#}");
            json!({"status": "error", "error": err.to_string()})
        }
        Err(_) => {
            warn!(
                step,
                timeout_ms = elapsed_ms(budget),
                "scheduler enrichment step timed out"
            );
            json!({
                "status": "timeout",
                "timeout_ms": elapsed_ms(budget),
                "retry": "next_scheduler_cycle",
                "safety_boundary": "read_only_enrichment_only",
            })
        }
    }
}

fn enrichment_step_timeout(name: &str, default_seconds: u64) -> Duration {
    let key = format!("SCHEDULER_{name}_TIMEOUT_SECONDS");
    let seconds = bounded_timeout_seconds(env::var(&key).ok().as_deref(), default_seconds);
    Duration::from_secs(seconds)
}

fn bounded_timeout_seconds(value: Option<&str>, default_seconds: u64) -> u64 {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=900).contains(seconds))
        .unwrap_or(default_seconds)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_timeout_seconds_accepts_only_safe_timeout_values() {
        assert_eq!(bounded_timeout_seconds(None, 45), 45);
        assert_eq!(bounded_timeout_seconds(Some("invalid"), 45), 45);
        assert_eq!(bounded_timeout_seconds(Some("0"), 45), 45);
        assert_eq!(bounded_timeout_seconds(Some("901"), 45), 45);
        assert_eq!(bounded_timeout_seconds(Some("1"), 45), 1);
        assert_eq!(bounded_timeout_seconds(Some("240"), 45), 240);
        assert_eq!(bounded_timeout_seconds(Some("900"), 45), 900);
    }

    #[tokio::test]
    async fn bounded_enrichment_step_preserves_successful_result() {
        let result = bounded_enrichment_step("test", Duration::from_millis(10), async {
            Ok::<JsonValue, anyhow::Error>(json!({"status": "ok", "count": 1}))
        })
        .await;

        assert_eq!(result["status"], "ok");
        assert_eq!(result["count"], 1);
    }

    #[tokio::test]
    async fn bounded_enrichment_step_records_safe_timeout_result() {
        let result = bounded_enrichment_step("test", Duration::from_millis(1), async {
            sleep(Duration::from_millis(25)).await;
            Ok::<JsonValue, anyhow::Error>(json!({"status": "ok"}))
        })
        .await;

        assert_eq!(result["status"], "timeout");
        assert_eq!(result["timeout_ms"], 1);
        assert_eq!(result["retry"], "next_scheduler_cycle");
        assert_eq!(result["safety_boundary"], "read_only_enrichment_only");
    }

    #[tokio::test]
    async fn bounded_enrichment_step_records_errors() {
        let result = bounded_enrichment_step("test", Duration::from_millis(10), async {
            Err::<JsonValue, _>(anyhow::anyhow!("provider unavailable"))
        })
        .await;

        assert_eq!(result["status"], "error");
        assert_eq!(result["error"], "provider unavailable");
    }
}
