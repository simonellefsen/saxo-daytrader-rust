from __future__ import annotations

import time
import os
import signal
from datetime import UTC, datetime, timedelta
from pathlib import Path
from types import FrameType
from typing import Any

from apscheduler.schedulers.blocking import BlockingScheduler

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import append_audit_log, connect, init_db, prune_scheduler_cycles, record_scheduler_cycle, update_scheduler_status
from saxo_daytrader_xai.execution_engine import enqueue_session_flatten_orders, maintain_ladder_orders, maintain_swing_limit_orders, queue_and_maybe_execute_latest_report, sync_broker_order_statuses
from saxo_daytrader_xai.market_schedule import get_market_status, refresh_market_calendars, summarize_analysis_window
from saxo_daytrader_xai.notifications import dispatch_broker_alerts_if_due, dispatch_summaries_if_due
from saxo_daytrader_xai.price_monitor import refresh_portfolio_price_state
from saxo_daytrader_xai.runtime_settings import apply_runtime_settings
from saxo_daytrader_xai.saxo_openapi import SaxoSessionError, ensure_access_token
from saxo_daytrader_xai.strategy_journal import generate_due_strategy_journals
from saxo_daytrader_xai.trading_manager import run_trading_manager_cycle
from saxo_daytrader_xai.xai_decision import generate_decision_report, should_auto_run_decision_report


def _scheduler_shutdown_signal_handler(signum: int, _frame: FrameType | None) -> None:
    raise SystemExit(f"Received signal {signal.Signals(signum).name}")


def _install_scheduler_signal_handlers() -> dict[int, signal.Handlers]:
    previous: dict[int, signal.Handlers] = {}
    for sig in (signal.SIGINT, signal.SIGTERM):
        previous[sig] = signal.getsignal(sig)
        signal.signal(sig, _scheduler_shutdown_signal_handler)
    return previous


def _restore_scheduler_signal_handlers(previous: dict[int, signal.Handlers]) -> None:
    for sig, handler in previous.items():
        signal.signal(sig, handler)


def _resolve_config(config: dict[str, Any] | None, config_path: str | Path) -> dict[str, Any]:
    if config is not None:
        return config
    return load_config(config_path)


def _pid_is_alive(pid: int | None) -> bool | None:
    if pid in (None, 0):
        return None
    try:
        os.kill(int(pid), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def assess_scheduler_worker_health(
    status: dict[str, Any] | None,
    *,
    poll_interval_minutes: int,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    if not status or not status.get("last_heartbeat_at"):
        return {
            "status": "unknown",
            "message": "No scheduler heartbeat recorded yet.",
            "heartbeat_age_minutes": None,
            "pid_alive": None,
            "restart_recommended": False,
        }

    now = (reference_time or datetime.now(UTC)).astimezone(UTC)
    last_heartbeat = datetime.fromisoformat(str(status["last_heartbeat_at"]))
    age_minutes = (now - last_heartbeat).total_seconds() / 60.0
    healthy_window = max(poll_interval_minutes * 2, 5)
    pid_alive = _pid_is_alive(status.get("scheduler_pid"))

    if pid_alive is False:
        return {
            "status": "dead",
            "message": f"Scheduler PID {status.get('scheduler_pid')} is no longer running.",
            "heartbeat_age_minutes": age_minutes,
            "pid_alive": False,
            "restart_recommended": True,
        }
    if age_minutes > healthy_window:
        return {
            "status": "stale",
            "message": f"Last heartbeat {age_minutes:.1f} minutes ago.",
            "heartbeat_age_minutes": age_minutes,
            "pid_alive": pid_alive,
            "restart_recommended": True,
        }
    return {
        "status": "healthy",
        "message": f"Last heartbeat {age_minutes:.1f} minutes ago.",
        "heartbeat_age_minutes": age_minutes,
        "pid_alive": pid_alive,
        "restart_recommended": False,
    }


def _scheduler_history_policy(config: dict[str, Any]) -> dict[str, int]:
    scheduler_cfg = config.get("scheduler", {})
    return {
        "history_max_rows": int(scheduler_cfg.get("history_max_rows", 250)),
        "history_retention_days": int(scheduler_cfg.get("history_retention_days", 30)),
    }


def _prune_scheduler_history(connection, config: dict[str, Any]) -> int:
    policy = _scheduler_history_policy(config)
    keep_since_started_at = None
    if policy["history_retention_days"] > 0:
        keep_since_started_at = (
            datetime.now(UTC) - timedelta(days=policy["history_retention_days"])
        ).isoformat(timespec="seconds")
    return prune_scheduler_cycles(
        connection,
        keep_max_rows=policy["history_max_rows"],
        keep_since_started_at=keep_since_started_at,
    )


def run_scheduler_cycle(
    *,
    config: dict[str, Any] | None = None,
    config_path: str | Path = "config.yaml",
    connection=None,
    force_mock: bool = False,
    force_decision: bool = False,
) -> dict[str, Any]:
    resolved_config = _resolve_config(config, config_path)
    resolved_connection = connection or connect(resolved_config["portfolio"]["database_path"])
    init_db(resolved_connection)
    resolved_config = apply_runtime_settings(resolved_config, resolved_connection)
    should_close = connection is None

    try:
        cycle_started_at = datetime.now(UTC).isoformat(timespec="seconds")
        update_scheduler_status(
            resolved_connection,
            last_heartbeat_at=cycle_started_at,
            last_cycle_started_at=cycle_started_at,
            scheduler_pid=os.getpid(),
        )
        calendar_refresh = refresh_market_calendars(resolved_config)
        session_keepalive = None
        if (
            str(resolved_config["execution"].get("mode")) == "live"
            and str(resolved_config["execution"].get("adapter")) == "saxo"
        ):
            try:
                session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
                session_keepalive = {
                    "status": "ok",
                    "access_token_expires_at": session.get("access_token_expires_at"),
                    "refresh_token_expires_at": session.get("refresh_token_expires_at"),
                    "last_refreshed_at": session.get("last_refreshed_at"),
                }
            except SaxoSessionError as exc:
                session_keepalive = {"status": "error", "error": str(exc)}
                append_audit_log(
                    resolved_connection,
                    "saxo_session_keepalive_failed",
                    {
                        "status": "error",
                        "error": str(exc),
                        "environment": resolved_config.get("saxo", {}).get("environment"),
                        "occurred_at": datetime.now(UTC).isoformat(timespec="seconds"),
                    },
                )
        market_status = get_market_status(resolved_config)
        analysis_summary = summarize_analysis_window(market_status)
        should_generate = force_decision or should_auto_run_decision_report(
            resolved_connection,
            resolved_config,
            analysis_summary["analysis_window_active"],
        )

        decision_result = None
        if should_generate:
            decision_result = generate_decision_report(
                config=resolved_config,
                connection=resolved_connection,
                force_mock=force_mock,
            )

        trading_manager_result = run_trading_manager_cycle(
            config=resolved_config,
            connection=resolved_connection,
            market_status_rows=market_status,
        )
        manager_runs = trading_manager_result.get("runs") if isinstance(trading_manager_result, dict) else None
        if manager_runs:
            queue_result = manager_runs[-1].get("queue") or {"status": "manager_completed_no_queue"}
        else:
            queue_result = queue_and_maybe_execute_latest_report(
                config=resolved_config,
                connection=resolved_connection,
                create_report_orders=False,
            )
        notification_result = dispatch_summaries_if_due(
            resolved_connection,
            resolved_config,
        )
        broker_alert_result = queue_result.get("alerts") if isinstance(queue_result, dict) else None
        if broker_alert_result is None:
            broker_alert_result = dispatch_broker_alerts_if_due(
                resolved_connection,
                resolved_config,
            )
        journal_result = generate_due_strategy_journals(
            resolved_connection,
            resolved_config,
        )

        outcome = {
            "status": "ok",
            "timestamp": datetime.now(UTC).isoformat(timespec="seconds"),
            "calendar_refresh": calendar_refresh,
            "saxo_session_keepalive": session_keepalive,
            "analysis_window_active": analysis_summary["analysis_window_active"],
            "active_markets": analysis_summary["active_markets"],
            "generated_decision": decision_result is not None,
            "decision": decision_result,
            "trading_manager": trading_manager_result,
            "queue": queue_result,
            "notifications": notification_result,
            "broker_alerts": broker_alert_result,
            "journal": journal_result,
        }
        update_scheduler_status(
            resolved_connection,
            last_heartbeat_at=outcome["timestamp"],
            last_cycle_completed_at=outcome["timestamp"],
            last_cycle_status="ok",
            last_cycle_json=outcome,
            scheduler_pid=os.getpid(),
        )
        record_scheduler_cycle(
            resolved_connection,
            started_at=cycle_started_at,
            completed_at=outcome["timestamp"],
            status="ok",
            analysis_window_active=bool(analysis_summary["analysis_window_active"]),
            generated_decision=decision_result is not None,
            queue_status=queue_result.get("status"),
            notifications_status=notification_result.get("status"),
            broker_alerts_status=broker_alert_result.get("status"),
            cycle_json=outcome,
        )
        pruned_rows = _prune_scheduler_history(resolved_connection, resolved_config)
        if pruned_rows:
            append_audit_log(
                resolved_connection,
                "scheduler_cycle_history_pruned",
                {
                    "deleted_rows": pruned_rows,
                    **_scheduler_history_policy(resolved_config),
                },
            )
        append_audit_log(resolved_connection, "scheduler_cycle_completed", outcome)
        return outcome
    except Exception as exc:  # noqa: BLE001
        payload = {
            "status": "failed",
            "timestamp": datetime.now(UTC).isoformat(timespec="seconds"),
            "error": str(exc),
        }
        update_scheduler_status(
            resolved_connection,
            last_heartbeat_at=payload["timestamp"],
            last_cycle_completed_at=payload["timestamp"],
            last_cycle_status="failed",
            last_cycle_json=payload,
            scheduler_pid=os.getpid(),
        )
        record_scheduler_cycle(
            resolved_connection,
            started_at=cycle_started_at if "cycle_started_at" in locals() else payload["timestamp"],
            completed_at=payload["timestamp"],
            status="failed",
            analysis_window_active=False,
            generated_decision=False,
            queue_status=None,
            notifications_status=None,
            broker_alerts_status=None,
            cycle_json=payload,
        )
        pruned_rows = _prune_scheduler_history(resolved_connection, resolved_config)
        if pruned_rows:
            append_audit_log(
                resolved_connection,
                "scheduler_cycle_history_pruned",
                {
                    "deleted_rows": pruned_rows,
                    **_scheduler_history_policy(resolved_config),
                },
            )
        append_audit_log(resolved_connection, "scheduler_cycle_failed", payload)
        return payload
    finally:
        if should_close:
            resolved_connection.close()


def run_manual_scheduler_cycle(
    *,
    config: dict[str, Any] | None = None,
    config_path: str | Path = "config.yaml",
    connection=None,
    mock: bool = False,
) -> dict[str, Any]:
    return run_scheduler_cycle(
        config=config,
        config_path=config_path,
        connection=connection,
        force_mock=mock,
        force_decision=False,
    )


def run_price_monitor_cycle(
    *,
    config_path: str | Path = "config.yaml",
) -> dict[str, Any]:
    resolved_config = load_config(config_path)
    decision_result = None
    queue_result = None
    trading_manager_result = None
    with connect(resolved_config["portfolio"]["database_path"]) as connection:
        init_db(connection)
        resolved_config = apply_runtime_settings(resolved_config, connection)
        market_status = get_market_status(resolved_config)
        analysis_summary = summarize_analysis_window(market_status)
        if should_auto_run_decision_report(connection, resolved_config, analysis_summary["analysis_window_active"]):
            decision_result = generate_decision_report(
                config=resolved_config,
                connection=connection,
                force_mock=False,
            )
        trading_manager_result = run_trading_manager_cycle(
            config=resolved_config,
            connection=connection,
            market_status_rows=market_status,
        )
        manager_runs = trading_manager_result.get("runs") if isinstance(trading_manager_result, dict) else None
        if manager_runs:
            queue_result = manager_runs[-1].get("queue")
        else:
            queue_result = queue_and_maybe_execute_latest_report(
                config=resolved_config,
                connection=connection,
                create_report_orders=False,
            )
    broker_sync = None
    if (
        str(resolved_config["execution"].get("mode")) == "live"
        and str(resolved_config["execution"].get("adapter")) == "saxo"
    ):
        try:
            broker_sync = sync_broker_order_statuses(config=resolved_config)
        except SaxoSessionError as exc:
            broker_sync = {"status": "error", "error": str(exc)}
    price_result = refresh_portfolio_price_state(config_path=config_path)
    flatten_result = enqueue_session_flatten_orders(config=resolved_config)
    ladder_maintenance = None
    swing_maintenance = None
    if (
        str(resolved_config["execution"].get("mode")) == "live"
        and str(resolved_config["execution"].get("adapter")) == "saxo"
    ):
        try:
            ladder_maintenance = maintain_ladder_orders(config=resolved_config)
        except SaxoSessionError as exc:
            ladder_maintenance = {"status": "error", "error": str(exc)}
        try:
            swing_maintenance = maintain_swing_limit_orders(config=resolved_config)
        except SaxoSessionError as exc:
            swing_maintenance = {"status": "error", "error": str(exc)}
    return {
        "status": "ok",
        "analysis_window_active": analysis_summary["analysis_window_active"] if "analysis_summary" in locals() else False,
        "generated_decision": decision_result is not None,
        "decision": decision_result,
        "trading_manager": trading_manager_result,
        "queue": queue_result,
        "price_monitor": price_result,
        "broker_sync": broker_sync,
        "flatten": flatten_result,
        "ladder_maintenance": ladder_maintenance,
        "swing_maintenance": swing_maintenance,
    }


def run_scheduler_forever(
    *,
    config_path: str | Path = "config.yaml",
    force_mock: bool = False,
) -> None:
    previous_handlers = _install_scheduler_signal_handlers()
    resolved_config = load_config(config_path)
    connection = connect(resolved_config["portfolio"]["database_path"])
    init_db(connection)
    started_at = datetime.now(UTC).isoformat(timespec="seconds")
    update_scheduler_status(
        connection,
        started_at=started_at,
        last_heartbeat_at=started_at,
        scheduler_pid=os.getpid(),
    )
    connection.close()
    interval_minutes = int(resolved_config["scheduler"]["poll_interval_minutes"])
    price_interval_minutes = int(resolved_config.get("price_monitor", {}).get("poll_interval_minutes", 5))
    scheduler = BlockingScheduler(timezone="Europe/Copenhagen")

    scheduler.add_job(
        run_scheduler_cycle,
        "interval",
        minutes=interval_minutes,
        max_instances=1,
        coalesce=True,
        kwargs={
            "config_path": str(config_path),
            "force_mock": force_mock,
        },
    )
    if bool(resolved_config.get("price_monitor", {}).get("enabled", True)):
        scheduler.add_job(
            run_price_monitor_cycle,
            "interval",
            minutes=price_interval_minutes,
            max_instances=1,
            coalesce=True,
            kwargs={
                "config_path": str(config_path),
            },
        )

    if resolved_config["scheduler"].get("startup_run", True):
        if bool(resolved_config.get("price_monitor", {}).get("enabled", True)):
            price_result = run_price_monitor_cycle(config_path=config_path)
            print(price_result)
        result = run_scheduler_cycle(config_path=config_path, force_mock=force_mock)
        print(result)

    print(
        f"Scheduler started. Poll interval={interval_minutes} minutes, "
        f"price poll interval={price_interval_minutes} minutes, "
        f"force_mock={force_mock}. Press Ctrl+C to stop."
    )
    try:
        scheduler.start()
    except (KeyboardInterrupt, SystemExit):
        print("Scheduler stopped.")
    finally:
        if scheduler.running:
            scheduler.shutdown(wait=False)
        time.sleep(0.1)
        _restore_scheduler_signal_handlers(previous_handlers)
