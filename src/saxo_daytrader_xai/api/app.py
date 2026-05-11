from __future__ import annotations

import json
import logging
import os
import secrets
import threading
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Literal
from urllib.parse import quote

from fastapi import FastAPI, HTTPException, Query, Request
from fastapi.responses import HTMLResponse
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

from saxo_daytrader_xai.analysis_pulses import analysis_pulse_status
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, fetch_latest_trading_manager_run, fetch_scheduler_cycles, fetch_scheduler_status, init_db
from saxo_daytrader_xai.execution_engine import (
    adopt_broker_holdings_into_local_ledger,
    fetch_execution_events,
    fetch_execution_fills,
    fetch_execution_orders,
    manage_live_order,
    queue_and_maybe_execute_latest_report,
    retry_failed_execution_orders,
    sync_saxo_sim_account_to_portfolio,
    sync_broker_order_statuses,
)
from saxo_daytrader_xai.market_schedule import get_market_status, summarize_analysis_window
from saxo_daytrader_xai.saxo_openapi import (
    SaxoSessionError,
    build_authorize_url,
    build_pkce_pair,
    build_session_payload,
    ensure_access_token,
    exchange_authorization_code,
    fetch_initial_session_context,
    get_auth_status,
    get_chart_samples,
    lookup_instrument,
    save_session,
)
from saxo_daytrader_xai.portfolio import (
    fetch_goal_tracking,
    fetch_portfolio_integrity_status,
    fetch_portfolio_positions,
    fetch_portfolio_summary,
    fetch_portfolio_value_history,
    fetch_trade_ledger,
    fetch_unrealised_after_tax_summary,
    record_portfolio_value_snapshot,
)
from saxo_daytrader_xai.runtime_settings import (
    apply_runtime_settings,
    fetch_cash_buffer_settings,
    update_cash_buffer_settings,
)
from saxo_daytrader_xai.scheduler_service import assess_scheduler_worker_health, run_manual_scheduler_cycle
from saxo_daytrader_xai.strategy_journal import build_diary_prompt_preview, fetch_strategy_journal_entries
from saxo_daytrader_xai.trading_manager import build_trading_manager_prompt_preview, trading_manager_status
from saxo_daytrader_xai.watchlists import build_watchlists
from saxo_daytrader_xai.xai_decision import (
    build_decision_prompt_preview,
    estimate_next_decision_report,
    fetch_latest_decision_report,
    fetch_latest_symbol_decisions,
    fetch_recent_decision_reports,
    generate_decision_report,
)


class SchedulerCycleRequest(BaseModel):
    mock: bool = False


class LiveOrderActionRequest(BaseModel):
    action: Literal["replace", "cancel"]
    quantity: float | None = None
    price: float | None = None


class CashBufferSettingsRequest(BaseModel):
    min_cash_buffer_pct: float


logger = logging.getLogger(__name__)


def _config_path(config_path: str | None = None) -> str:
    explicit = config_path or os.getenv("DAYTRADER_CONFIG") or "config.yaml"
    return str(Path(explicit).expanduser().resolve())


def _initial_cash_dkk(config: dict[str, Any]) -> float:
    return float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0)


def _prefer_broker_state(config: dict[str, Any]) -> bool:
    execution_cfg = config.get("execution", {})
    saxo_environment = str(config.get("saxo", {}).get("environment") or "").lower()
    return (
        str(execution_cfg.get("mode") or "").lower() == "live"
        and str(execution_cfg.get("adapter") or "").lower() == "saxo"
        and saxo_environment == "live"
    )


def _use_broker_positions(config: dict[str, Any]) -> bool:
    return _prefer_broker_state(config)


def _daily_order_capacity(connection, config: dict[str, Any]) -> dict[str, int]:
    limit = int(config.get("execution", {}).get("max_daily_orders", 0) or 0)
    today = datetime.now(UTC).date().isoformat()
    used = int(
        connection.execute(
            """
            SELECT COUNT(*) AS count_orders
            FROM (
                SELECT id AS execution_order_id
                FROM execution_orders
                WHERE substr(created_at, 1, 10) = ?
                  AND status = 'executed'
                UNION
                SELECT execution_order_id
                FROM execution_fills
                WHERE substr(created_at, 1, 10) = ?
            ) successful_orders
            """,
            (today, today),
        ).fetchone()["count_orders"]
    )
    remaining = max(limit - used, 0)
    return {"max": limit, "used": used, "remaining": remaining}


def _history_start_at(range_key: str, end_at: datetime) -> str | None:
    normalized = range_key.upper()
    if normalized == "1D":
        return (end_at - timedelta(days=1)).isoformat(timespec="seconds")
    if normalized == "1W":
        return (end_at - timedelta(days=7)).isoformat(timespec="seconds")
    if normalized == "1M":
        return (end_at - timedelta(days=31)).isoformat(timespec="seconds")
    if normalized == "3M":
        return (end_at - timedelta(days=93)).isoformat(timespec="seconds")
    if normalized == "1Y":
        return (end_at - timedelta(days=366)).isoformat(timespec="seconds")
    if normalized == "YTD":
        return datetime(end_at.year, 1, 1, tzinfo=UTC).isoformat(timespec="seconds")
    return None


def _parse_json_text(value: Any) -> Any:
    if not value:
        return None
    if isinstance(value, (dict, list)):
        return value
    try:
        return json.loads(value)
    except Exception:  # noqa: BLE001
        return None


def _saxo_auth_mode(config: dict[str, Any]) -> str:
    configured = str(config.get("saxo", {}).get("auth_mode") or "").strip().lower()
    if configured in {"pkce", "secret"}:
        return configured
    environment = str(config.get("saxo", {}).get("environment") or "sim").lower()
    return "secret" if environment == "live" else "pkce"


def _public_base_url(request: Request) -> str:
    forwarded_proto = request.headers.get("x-forwarded-proto")
    forwarded_host = request.headers.get("x-forwarded-host")
    if forwarded_host:
        proto = (forwarded_proto or "https").split(",")[0].strip()
        host = forwarded_host.split(",")[0].strip()
        return f"{proto}://{host}".rstrip("/")
    origin = request.headers.get("origin")
    if origin:
        return origin.rstrip("/")
    referer = request.headers.get("referer")
    if referer:
        return str(referer).split("/api/", 1)[0].rstrip("/")
    return str(request.base_url).rstrip("/")


def _oauth_state_path(config: dict[str, Any], state: str) -> Path:
    session_path = Path(config.get("saxo", {}).get("session_path") or "")
    if not session_path:
        session_path = Path(config["_meta"]["config_dir"]) / ".secrets" / "saxo_session.json"
    if not session_path.is_absolute():
        session_path = Path(config["_meta"]["config_dir"]) / session_path
    return session_path.parent / f"saxo_oauth_state_{state}.json"


def _write_oauth_state(config: dict[str, Any], state: str, payload: dict[str, Any]) -> None:
    path = _oauth_state_path(config, state)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    path.chmod(0o600)


def _pop_oauth_state(config: dict[str, Any], state: str) -> dict[str, Any]:
    path = _oauth_state_path(config, state)
    if not path.exists():
        raise SaxoSessionError("Saxo OAuth state was not found or has expired. Start re-authentication again.")
    payload = json.loads(path.read_text(encoding="utf-8"))
    try:
        path.unlink()
    except OSError:
        pass
    created_at = datetime.fromisoformat(str(payload.get("created_at")).replace("Z", "+00:00"))
    if created_at.tzinfo is None:
        created_at = created_at.replace(tzinfo=UTC)
    if created_at < datetime.now(UTC) - timedelta(minutes=10):
        raise SaxoSessionError("Saxo OAuth state has expired. Start re-authentication again.")
    return payload


def _oauth_callback_html(*, ok: bool, title: str, message: str, return_to: str = "/") -> str:
    color = "#0a7f39" if ok else "#b42318"
    safe_title = title.replace("<", "&lt;").replace(">", "&gt;")
    safe_message = message.replace("<", "&lt;").replace(">", "&gt;")
    safe_return_to = quote(return_to or "/", safe="/:?&=#%")
    return f"""<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="refresh" content="2; url={safe_return_to}" />
    <title>{safe_title}</title>
    <style>
      body {{ font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 4rem; color: #111827; }}
      main {{ max-width: 44rem; padding: 2rem; border: 1px solid #d8e0ea; border-radius: 1rem; box-shadow: 0 18px 60px rgba(15, 23, 42, 0.08); }}
      h1 {{ color: {color}; margin-top: 0; }}
      a {{ color: #2563eb; }}
    </style>
  </head>
  <body>
    <main>
      <h1>{safe_title}</h1>
      <p>{safe_message}</p>
      <p>Returning to the dashboard...</p>
      <p><a href="{safe_return_to}">Continue now</a></p>
    </main>
    <script>window.setTimeout(() => {{ window.location.href = "{safe_return_to}"; }}, 1200);</script>
  </body>
</html>"""


def create_app(config_path: str | None = None) -> FastAPI:
    resolved_config_path = _config_path(config_path)
    app = FastAPI(
        title="saxo-daytrader-xai API",
        version="0.1.0",
        docs_url="/docs",
        openapi_url="/openapi.json",
    )
    app.add_middleware(
        CORSMiddleware,
        allow_origins=[
            "http://127.0.0.1:3000",
            "http://localhost:3000",
        ],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )
    app.state.config_path = resolved_config_path

    @contextmanager
    def runtime():
        config = load_config(app.state.config_path)
        connection = connect(config["portfolio"]["database_path"])
        init_db(connection)
        config = apply_runtime_settings(config, connection)
        try:
            yield config, connection
        finally:
            connection.close()

    def prewarm_watchlists() -> None:
        try:
            with runtime() as (config, _):
                build_watchlists(config, force_refresh=True)
            logger.info("Watchlist cache prewarmed")
        except Exception:  # noqa: BLE001
            logger.exception("Watchlist cache prewarm failed")

    @app.on_event("startup")
    def start_background_tasks() -> None:
        threading.Thread(target=prewarm_watchlists, name="watchlist-cache-prewarm", daemon=True).start()

    def portfolio_kwargs(config: dict[str, Any]) -> dict[str, Any]:
        return {
            "initial_cash_dkk": _initial_cash_dkk(config),
            "prefer_broker_cash": _prefer_broker_state(config),
            "use_broker_positions": _use_broker_positions(config),
        }

    def execution_counts(orders: list[dict[str, Any]]) -> dict[str, int]:
        queued_statuses = {
            "pending_execution",
            "pending_approval",
            "waiting_for_market_open",
            "waiting_for_cash_settlement",
            "waiting_for_virtual_cash_budget",
        }
        return {
            "queued": sum(1 for row in orders if row.get("status") in queued_statuses),
            "pending_approval": sum(1 for row in orders if row.get("status") == "pending_approval"),
            "broker_live": sum(
                1
                for row in orders
                if row.get("status")
                in {
                    "submitted_to_broker",
                    "broker_working",
                    "broker_amended",
                    "broker_partially_filled",
                    "broker_replace_requested",
                    "broker_cancel_requested",
                }
            ),
            "failed": sum(1 for row in orders if row.get("status") == "execution_failed"),
        }

    active_order_statuses = {
        "pending_execution",
        "pending_approval",
        "waiting_for_market_open",
        "waiting_for_cash_settlement",
        "waiting_for_virtual_cash_budget",
        "submitted_to_broker",
        "broker_working",
        "broker_amended",
        "broker_partially_filled",
        "broker_replace_requested",
        "broker_cancel_requested",
    }

    def ladder_status_by_symbol(connection) -> dict[str, dict[str, Any]]:
        rows = connection.execute(
            """
            SELECT symbol, status, strategy_type, strategy_role
            FROM execution_orders
            ORDER BY id ASC
            """
        ).fetchall()
        output: dict[str, dict[str, Any]] = {}
        for row in rows:
            record = output.setdefault(
                str(row["symbol"]),
                {
                    "active_orders": 0,
                    "active_stop_orders": 0,
                    "active_take_profit_orders": 0,
                    "filled_entry_rungs": 0,
                    "total_entry_rungs": 0,
                    "latest_strategy_type": None,
                },
            )
            status = str(row["status"] or "")
            strategy_type = str(row["strategy_type"] or "") or None
            strategy_role = str(row["strategy_role"] or "") or None
            if strategy_type:
                record["latest_strategy_type"] = strategy_type
            if status in active_order_statuses:
                record["active_orders"] += 1
                if strategy_role == "stop_loss":
                    record["active_stop_orders"] += 1
                elif strategy_role == "take_profit":
                    record["active_take_profit_orders"] += 1
            if strategy_role == "entry":
                record["total_entry_rungs"] += 1
            if status == "executed" and strategy_role == "entry":
                record["filled_entry_rungs"] += 1
        for symbol, record in output.items():
            trailing = bool(record["active_stop_orders"])
            filled = int(record["filled_entry_rungs"])
            total = max(int(record["total_entry_rungs"]), filled, 0)
            if record["active_orders"]:
                if total:
                    status_text = f"{filled}/{total} filled"
                else:
                    status_text = f"{record['active_orders']} active"
                if trailing:
                    status_text += " • trailing"
            elif filled:
                status_text = f"{filled}/{total or filled} filled"
            else:
                status_text = "idle"
            record["text"] = status_text
            record["trailing"] = trailing
            record["progress_pct"] = (filled / total) if total else 0.0
        return output

    def _ladder_parameters(order_rows: list[dict[str, Any]], position: dict[str, Any] | None) -> dict[str, Any]:
        metadata: dict[str, Any] = {}
        for row in order_rows:
            request_payload = _parse_json_text(row.get("request_json")) or {}
            strategy_metadata = request_payload.get("strategy_metadata") if isinstance(request_payload, dict) else None
            if isinstance(strategy_metadata, dict):
                metadata = strategy_metadata
                break
        if not metadata:
            return {}
        current_price = float(position.get("current_price_local") or 0.0) if isinstance(position, dict) else 0.0
        ceiling = metadata.get("take_profit_price_local")
        stop = metadata.get("stop_price_local")
        entry = metadata.get("entry_price_local")
        return {
            "atr_1m": metadata.get("atr_1m"),
            "rung_spacing_local": metadata.get("rung_spacing_local"),
            "ladder_rung_id": metadata.get("ladder_rung_id"),
            "max_position_weight_pct": float(metadata.get("max_position_weight_pct") or 0.0),
            "entry_price_local": entry,
            "take_profit_price_local": ceiling,
            "stop_price_local": stop,
            "current_price_local": current_price if current_price else None,
            "ceiling_gap_local": (float(ceiling) - current_price) if ceiling not in (None, "") and current_price else None,
            "stop_gap_local": (current_price - float(stop)) if stop not in (None, "") and current_price else None,
        }

    def _chart_series_for_symbol(config: dict[str, Any], symbol: str, *, range_key: str, first_event_at: datetime | None) -> tuple[list[dict[str, Any]], str | None]:
        execution_cfg = config.get("execution", {})
        if str(execution_cfg.get("adapter") or "").lower() != "saxo":
            return [], "Chart unavailable: non-Saxo adapter."
        try:
            session = ensure_access_token(config, config["saxo"].get("session_path"))
            instrument = lookup_instrument(symbol, config, session)
        except Exception as exc:  # noqa: BLE001
            return [], str(exc)

        now = datetime.now(UTC)
        normalized = str(range_key or "SESSION").upper()
        if normalized == "1H":
            horizon_minutes = 1
            count = 60
        elif normalized == "4H":
            horizon_minutes = 1
            count = 240
        elif normalized == "SESSION":
            age_minutes = 240
            if first_event_at is not None:
                age_minutes = max(int((now - first_event_at).total_seconds() // 60) + 30, 60)
            if age_minutes <= 360:
                horizon_minutes = 1
            elif age_minutes <= 1440:
                horizon_minutes = 5
            elif age_minutes <= 10080:
                horizon_minutes = 30
            else:
                horizon_minutes = 60
            count = min(max(age_minutes // max(horizon_minutes, 1), 60), 1000)
        else:
            horizon_minutes = 5
            count = 288

        try:
            payload = get_chart_samples(
                uic=instrument.uic,
                asset_type=instrument.asset_type,
                config=config,
                session=session,
                horizon_minutes=horizon_minutes,
                count=count,
                mode="UpTo",
            )
        except SaxoSessionError as exc:
            return [], str(exc)
        except Exception as exc:  # noqa: BLE001
            return [], f"Chart fetch failed: {exc}"

        data = payload.get("Data", []) or []
        output: list[dict[str, Any]] = []
        for item in data:
            output.append(
                {
                    "time": item.get("Time"),
                    "open": float(item.get("Open") or 0.0),
                    "high": float(item.get("High") or 0.0),
                    "low": float(item.get("Low") or 0.0),
                    "close": float(item.get("Close") or 0.0),
                    "volume": float(item.get("Volume") or 0.0),
                }
            )
        return output, None

    def asset_ladder_history_payload(config: dict[str, Any], connection, symbol: str, *, range_key: str = "SESSION") -> dict[str, Any]:
        kwargs = portfolio_kwargs(config)
        positions = fetch_portfolio_positions(connection, **kwargs)
        position = next((row for row in positions if str(row.get("symbol")) == symbol), None)
        order_rows = [
            dict(row)
            for row in connection.execute(
                """
                SELECT *
                FROM execution_orders
                WHERE symbol = ?
                ORDER BY id ASC
                """,
                (symbol,),
            ).fetchall()
        ]
        fill_rows = [
            dict(row)
            for row in connection.execute(
                """
                SELECT *
                FROM execution_fills
                WHERE symbol = ?
                ORDER BY id ASC
                """,
                (symbol,),
            ).fetchall()
        ]
        event_rows = [
            {
                **dict(row),
                "request_json": _parse_json_text(row["request_json"]),
            }
            for row in connection.execute(
                """
                SELECT e.*, o.request_json, o.strategy_role, o.strategy_type
                FROM execution_order_events e
                JOIN execution_orders o ON o.id = e.execution_order_id
                WHERE o.symbol = ?
                ORDER BY e.id ASC
                """,
                (symbol,),
            ).fetchall()
        ]

        first_event_at: datetime | None = None
        for row in fill_rows:
            if str(row.get("side") or "").upper() == "BUY" and row.get("created_at"):
                first_event_at = datetime.fromisoformat(str(row["created_at"])).astimezone(UTC)
                break
        if first_event_at is None:
            for row in order_rows:
                if str(row.get("action") or "").upper() == "BUY" and row.get("created_at"):
                    first_event_at = datetime.fromisoformat(str(row["created_at"])).astimezone(UTC)
                    break

        chart_points, chart_error = _chart_series_for_symbol(config, symbol, range_key=range_key, first_event_at=first_event_at)
        chart_source = "saxo" if chart_points else "fallback"

        markers: list[dict[str, Any]] = []
        for row in fill_rows:
            side = str(row.get("side") or "").upper()
            fill_price = float(row.get("average_price_local") or 0.0)
            fill_payload = _parse_json_text(row.get("raw_payload_json")) or {}
            last_activity = fill_payload.get("last_activity") if isinstance(fill_payload, dict) else None
            commission_dkk = None
            if isinstance(last_activity, dict):
                commission_dkk = (last_activity.get("Cost") or {}).get("Commission") or (last_activity.get("CostInAccountCurrency") or {}).get("Commission")
            markers.append(
                {
                    "id": f"fill-{row['id']}",
                    "time": row.get("created_at"),
                    "price": fill_price,
                    "kind": "buy_fill" if side == "BUY" else "sell_fill",
                    "label": f"{side.title()} fill",
                    "quantity": float(row.get("delta_quantity") or 0.0),
                    "commission_dkk": commission_dkk,
                    "strategy_role": None,
                    "ladder_rung_id": None,
                    "amendment_reason": None,
                    "strategy_reason": None,
                    "confidence": None,
                    "details": f"{side.title()} fill at {fill_price:.2f}",
                    "payload": fill_payload,
                }
            )
        for row in order_rows:
            request_payload = _parse_json_text(row.get("request_json")) or {}
            strategy_metadata = request_payload.get("strategy_metadata") if isinstance(request_payload, dict) else None
            if row.get("status") in active_order_statuses or row.get("status") == "executed":
                price = row.get("limit_price_local") or row.get("stop_price_local") or row.get("price_local")
                kind = "order"
                label = f"{str(row.get('action') or '').title()} {str(row.get('order_type') or 'Order')}"
                if str(row.get("strategy_role") or "") == "flatten_close":
                    kind = "flatten"
                    label = "Flatten"
                markers.append(
                    {
                        "id": f"order-{row['id']}",
                        "time": row.get("created_at"),
                        "price": float(price or 0.0),
                        "kind": kind,
                        "label": label,
                        "quantity": float(row.get("quantity") or 0.0),
                        "commission_dkk": None,
                        "strategy_role": row.get("strategy_role"),
                        "ladder_rung_id": strategy_metadata.get("ladder_rung_id") if isinstance(strategy_metadata, dict) else None,
                        "amendment_reason": strategy_metadata.get("amendment_reason") if isinstance(strategy_metadata, dict) else None,
                        "strategy_reason": request_payload.get("rationale") if isinstance(request_payload, dict) else None,
                        "confidence": request_payload.get("confidence") if isinstance(request_payload, dict) else None,
                        "details": f"{label} · {row.get('status')}",
                        "payload": {
                            "request_json": request_payload,
                            "execution_result_json": _parse_json_text(row.get("execution_result_json")),
                            "order": row,
                        },
                    }
                )
        for row in event_rows:
            if str(row.get("event_type") or "").startswith("broker_") and "amend" not in str(row.get("event_type") or ""):
                continue
            payload = _parse_json_text(row.get("raw_payload_json")) or {}
            markers.append(
                {
                    "id": f"event-{row['id']}",
                    "time": row.get("created_at"),
                    "price": float(row.get("broker_price_local") or 0.0),
                    "kind": "amendment",
                    "label": "Order update",
                    "quantity": float(row.get("broker_quantity") or 0.0),
                    "commission_dkk": None,
                    "strategy_role": row.get("strategy_role"),
                    "ladder_rung_id": payload.get("strategy_metadata", {}).get("ladder_rung_id") if isinstance(payload.get("strategy_metadata"), dict) else None,
                    "amendment_reason": payload.get("amendment_reason") if isinstance(payload, dict) else None,
                    "strategy_reason": None,
                    "confidence": None,
                    "details": f"{row.get('event_type')} · {row.get('broker_status') or ''}",
                    "payload": payload,
                }
            )
        markers.sort(key=lambda item: str(item.get("time") or ""))

        if not chart_points:
            fallback_points = []
            for marker in markers:
                marker_time = marker.get("time")
                marker_price = marker.get("price")
                if not marker_time or marker_price in (None, ""):
                    continue
                price_value = float(marker_price)
                fallback_points.append(
                    {
                        "time": marker_time,
                        "open": price_value,
                        "high": price_value,
                        "low": price_value,
                        "close": price_value,
                        "volume": 0.0,
                    }
                )
            if position and position.get("current_price_local") not in (None, ""):
                fallback_points.append(
                    {
                        "time": datetime.now(UTC).isoformat(timespec="seconds"),
                        "open": float(position["current_price_local"]),
                        "high": float(position["current_price_local"]),
                        "low": float(position["current_price_local"]),
                        "close": float(position["current_price_local"]),
                        "volume": 0.0,
                    }
                )
            if fallback_points:
                chart_points = sorted(fallback_points, key=lambda item: str(item["time"]))
                chart_error = chart_error or "Saxo chart samples unavailable; showing execution-price fallback."
                chart_source = "fallback"

        active_lines: list[dict[str, Any]] = []
        ladder_levels: list[dict[str, Any]] = []
        for row in order_rows:
            status = str(row.get("status") or "")
            if status not in active_order_statuses:
                continue
            strategy_role = str(row.get("strategy_role") or "")
            if row.get("stop_price_local") is not None:
                active_lines.append(
                    {
                        "label": "Stop loss",
                        "price": float(row["stop_price_local"]),
                        "color": "#b42318",
                        "kind": strategy_role or "stop",
                        "dashed": True,
                    }
                )
            if row.get("limit_price_local") is not None:
                active_lines.append(
                    {
                        "label": "Take profit" if strategy_role == "take_profit" else "Limit",
                        "price": float(row["limit_price_local"]),
                        "color": "#0f8a4b" if strategy_role == "take_profit" else "#6b7280",
                        "kind": strategy_role or "limit",
                        "dashed": True,
                    }
                )
            if str(row.get("strategy_type") or "") == "ladder":
                request_payload = _parse_json_text(row.get("request_json")) or {}
                metadata = request_payload.get("strategy_metadata") if isinstance(request_payload, dict) else None
                if isinstance(metadata, dict):
                    for key, label, color in (
                        ("entry_price_local", "Entry rung", "#9ca3af"),
                        ("take_profit_price_local", "Take-profit rung", "#0f8a4b"),
                        ("stop_price_local", "Stop rung", "#b42318"),
                    ):
                        value = metadata.get(key)
                        if value not in (None, ""):
                            ladder_levels.append(
                                {
                                    "label": label,
                                    "price": float(value),
                                    "color": color,
                                    "kind": key,
                                }
                            )

        ladder_summary = ladder_status_by_symbol(connection).get(symbol, {"text": "idle", "active_orders": 0, "filled_entry_rungs": 0, "trailing": False, "progress_pct": 0.0})
        ladder_parameters = _ladder_parameters(order_rows, position)
        return {
            "symbol": symbol,
            "range_key": range_key,
            "position": position,
            "ladder_summary": ladder_summary,
            "chart": {
                "points": chart_points,
                "error": chart_error,
                "source": chart_source,
                "has_real_data": chart_source == "saxo" and bool(chart_points),
                "first_event_at": first_event_at.isoformat(timespec="seconds") if first_event_at else None,
            },
            "markers": markers,
            "active_lines": active_lines,
            "ladder_levels": ladder_levels,
            "ladder_parameters": ladder_parameters,
            "legend": [
                {"key": "buy_fill", "label": "Buy fill", "color": "#0f8a4b"},
                {"key": "sell_fill", "label": "Sell fill", "color": "#b42318"},
                {"key": "order", "label": "Order / rung", "color": "#2563eb"},
                {"key": "amendment", "label": "Amendment", "color": "#38bdf8"},
                {"key": "stop_loss", "label": "Stop line", "color": "#b42318"},
                {"key": "take_profit", "label": "Ceiling / take-profit", "color": "#0f8a4b"},
                {"key": "current_price", "label": "Current price", "color": "#2563eb"},
            ],
        }

    @app.get("/api/health")
    def health() -> dict[str, str]:
        return {"status": "ok"}

    @app.get("/api/overview")
    def overview() -> dict[str, Any]:
        with runtime() as (config, connection):
            kwargs = portfolio_kwargs(config)
            summary = fetch_portfolio_summary(connection, **kwargs)
            after_tax = fetch_unrealised_after_tax_summary(
                connection,
                config,
                initial_cash_dkk=kwargs["initial_cash_dkk"],
                use_broker_positions=kwargs["use_broker_positions"],
            )
            integrity = fetch_portfolio_integrity_status(
                connection,
                initial_cash_dkk=kwargs["initial_cash_dkk"],
                use_broker_positions=kwargs["use_broker_positions"],
            )
            market_status = get_market_status(config)
            analysis_summary = summarize_analysis_window(market_status)
            pulse_summary = analysis_pulse_status(config, market_status)
            manager_status = trading_manager_status(config, market_status)
            latest_manager_run = fetch_latest_trading_manager_run(connection)
            latest_decision = fetch_latest_decision_report(connection)
            scheduler_status = fetch_scheduler_status(connection)
            scheduler_health = assess_scheduler_worker_health(
                scheduler_status,
                poll_interval_minutes=int(config.get("scheduler", {}).get("poll_interval_minutes", 10)),
            )
            orders = fetch_execution_orders(connection, limit=250)
            return {
                "app": {
                    "project_name": config.get("app", {}).get("project_name"),
                    "environment": config.get("app", {}).get("environment"),
                    "config_path": app.state.config_path,
                },
                "execution": {
                    "mode": config.get("execution", {}).get("mode"),
                    "adapter": config.get("execution", {}).get("adapter"),
                    "require_approval_live": bool(config.get("execution", {}).get("require_approval_live", True)),
                    "max_daily_orders": int(config.get("execution", {}).get("max_daily_orders", 0)),
                    "daily_order_capacity": _daily_order_capacity(connection, config),
                    "counts": execution_counts(orders),
                },
                "portfolio_summary": summary,
                "after_tax_summary": after_tax,
                "goal_tracking": fetch_goal_tracking(connection, config),
                "integrity": integrity,
                "analysis_summary": analysis_summary,
                "latest_decision": {
                    "id": latest_decision.get("id") if latest_decision else None,
                    "created_at": latest_decision.get("created_at") if latest_decision else None,
                    "status": latest_decision.get("status") if latest_decision else None,
                },
                "scheduler_status": scheduler_status,
                "scheduler_health": scheduler_health,
                "trading_manager": {
                    "status": manager_status,
                    "latest_run": latest_manager_run,
                },
                "saxo_auth": get_auth_status(config, config.get("saxo", {}).get("session_path"), auto_refresh=True),
                "settings": {
                    "cash_buffer": fetch_cash_buffer_settings(config, connection),
                },
                "refresh": {
                    "price_poll_interval_minutes": int(config.get("price_monitor", {}).get("poll_interval_minutes", 1)),
                    "scheduler_poll_interval_minutes": int(config.get("scheduler", {}).get("poll_interval_minutes", 10)),
                    "decision_cadence": "two_daily_open_followups",
                    "decision_cadence_label": "2 daily reports",
                    "decision_pulses": pulse_summary.get("pulses", []),
                    "next_decision_pulse_at": pulse_summary.get("next_pulse_at"),
                    "next_decision_pulse_label": pulse_summary.get("next_pulse_label"),
                },
            }

    @app.get("/api/settings/cash-buffer")
    def cash_buffer_settings() -> dict[str, Any]:
        with runtime() as (config, connection):
            return fetch_cash_buffer_settings(config, connection)

    @app.post("/api/settings/cash-buffer")
    def update_cash_buffer(request: CashBufferSettingsRequest) -> dict[str, Any]:
        with runtime() as (config, connection):
            try:
                return update_cash_buffer_settings(
                    config,
                    connection,
                    min_cash_buffer_pct=float(request.min_cash_buffer_pct),
                )
            except ValueError as exc:
                raise HTTPException(status_code=400, detail=str(exc)) from exc

    @app.get("/api/saxo/auth/status")
    def saxo_auth_status() -> dict[str, Any]:
        with runtime() as (config, _):
            return get_auth_status(config, config.get("saxo", {}).get("session_path"), auto_refresh=True)

    @app.post("/api/saxo/auth/start")
    def saxo_auth_start(request: Request) -> dict[str, Any]:
        with runtime() as (config, _):
            environment = str(config.get("saxo", {}).get("environment") or "sim").lower()
            auth_mode = _saxo_auth_mode(config)
            client_id = str(config.get("saxo", {}).get("client_id") or "")
            if not client_id:
                raise HTTPException(status_code=400, detail="SAXO_CLIENT_ID is missing.")
            code_verifier = None
            code_challenge = None
            if auth_mode == "pkce":
                code_verifier, code_challenge = build_pkce_pair()
            state = secrets.token_urlsafe(32)
            public_base_url = _public_base_url(request)
            redirect_uri = f"{public_base_url}/api/saxo/auth/callback"
            return_to = request.headers.get("referer") or "/"
            authorize_url = build_authorize_url(
                environment=environment,
                client_id=client_id,
                redirect_uri=redirect_uri,
                state=state,
                auth_mode=auth_mode,
                code_challenge=code_challenge,
            )
            _write_oauth_state(
                config,
                state,
                {
                    "state": state,
                    "environment": environment,
                    "auth_mode": auth_mode,
                    "client_id": client_id,
                    "redirect_uri": redirect_uri,
                    "code_verifier": code_verifier,
                    "return_to": return_to,
                    "created_at": datetime.now(UTC).isoformat(timespec="seconds"),
                },
            )
            return {
                "status": "redirect",
                "environment": environment,
                "auth_mode": auth_mode,
                "authorize_url": authorize_url,
                "redirect_uri": redirect_uri,
                "message": "Redirecting to Saxo authorization.",
            }

    @app.get("/api/saxo/auth/callback", response_class=HTMLResponse)
    def saxo_auth_callback(request: Request, code: str | None = None, state: str | None = None, error: str | None = None) -> HTMLResponse:
        with runtime() as (config, _):
            return_to = "/"
            try:
                if error:
                    raise SaxoSessionError(f"Saxo returned an authorization error: {error}")
                if not state or not code:
                    raise SaxoSessionError("Saxo OAuth callback did not include both code and state.")
                oauth_state = _pop_oauth_state(config, state)
                return_to = str(oauth_state.get("return_to") or "/")
                if str(oauth_state.get("state")) != state:
                    raise SaxoSessionError("Saxo OAuth state mismatch.")
                environment = str(oauth_state["environment"]).lower()
                auth_mode = str(oauth_state["auth_mode"]).lower()
                client_id = str(oauth_state["client_id"])
                token_response = exchange_authorization_code(
                    environment=environment,
                    auth_mode=auth_mode,
                    client_id=client_id,
                    client_secret=str(config.get("saxo", {}).get("client_secret") or ""),
                    redirect_uri=str(oauth_state["redirect_uri"]),
                    code=code,
                    code_verifier=oauth_state.get("code_verifier"),
                    timeout_seconds=30,
                )
                session_context = fetch_initial_session_context(
                    environment=environment,
                    access_token=str(token_response["access_token"]),
                    timeout_seconds=30,
                )
                session_payload = build_session_payload(
                    environment=environment,
                    auth_mode=auth_mode,
                    client_id=client_id,
                    redirect_uri=str(oauth_state["redirect_uri"]),
                    code_verifier=oauth_state.get("code_verifier"),
                    token_response=token_response,
                    session_context=session_context,
                )
                session_path = Path(config.get("saxo", {}).get("session_path") or "")
                if not session_path.is_absolute():
                    session_path = Path(config["_meta"]["config_dir"]) / session_path
                save_session(session_path, session_payload)
                html = _oauth_callback_html(
                    ok=True,
                    title="Saxo authorization complete",
                    message="The Saxo session has been renewed and stored for the backend.",
                    return_to=return_to,
                )
                return HTMLResponse(html, status_code=200)
            except Exception as exc:  # noqa: BLE001
                html = _oauth_callback_html(
                    ok=False,
                    title="Saxo authorization failed",
                    message=str(exc),
                    return_to=return_to,
                )
                return HTMLResponse(html, status_code=400)

    @app.get("/api/portfolio/positions")
    def portfolio_positions(limit: int = Query(default=25, ge=1, le=250)) -> dict[str, Any]:
        with runtime() as (config, connection):
            kwargs = portfolio_kwargs(config)
            positions = fetch_portfolio_positions(connection, **kwargs)
            ladder_status_map = ladder_status_by_symbol(connection)
            decision_map = fetch_latest_symbol_decisions(connection)
            items = []
            for row in positions[:limit]:
                enriched = dict(row)
                enriched["decision"] = decision_map.get(str(row.get("symbol") or ""))
                enriched["ladder_status"] = ladder_status_map.get(
                    str(row.get("symbol") or ""),
                    {"text": "idle", "active_orders": 0, "filled_entry_rungs": 0, "trailing": False},
                )
                items.append(enriched)
            return {"items": items, "total": len(positions)}

    @app.get("/api/asset-ladder-history/{symbol}")
    def asset_ladder_history(symbol: str, range_key: str = Query(default="SESSION")) -> dict[str, Any]:
        with runtime() as (config, connection):
            return asset_ladder_history_payload(config, connection, symbol, range_key=range_key)

    @app.get("/api/ladder-chart/{symbol}")
    def ladder_chart(symbol: str, range_key: str = Query(default="SESSION")) -> dict[str, Any]:
        with runtime() as (config, connection):
            return asset_ladder_history_payload(config, connection, symbol, range_key=range_key)

    @app.get("/api/portfolio/trades")
    def portfolio_trades(limit: int = Query(default=50, ge=1, le=250)) -> dict[str, Any]:
        with runtime() as (_, connection):
            return {"items": fetch_trade_ledger(connection, limit=limit)}

    @app.get("/api/performance")
    def performance(
        range_key: str = Query(default="1D"),
        start_at: str | None = Query(default=None),
        end_at: str | None = Query(default=None),
    ) -> dict[str, Any]:
        with runtime() as (config, connection):
            end_dt = datetime.fromisoformat(end_at).astimezone(UTC) if end_at else datetime.now(UTC)
            effective_start_at = start_at or _history_start_at(range_key, end_dt)
            history = fetch_portfolio_value_history(
                connection,
                start_at=effective_start_at,
                end_at=end_dt.isoformat(timespec="seconds"),
                limit=5000,
            )
            if not history:
                record_portfolio_value_snapshot(
                    connection,
                    recorded_at=end_dt.isoformat(timespec="seconds"),
                    snapshot_type="api_current",
                    initial_cash_dkk=_initial_cash_dkk(config),
                    prefer_broker_cash=_prefer_broker_state(config),
                    source="performance_api",
                    extra_payload={"reason": "seed_empty_performance_history"},
                )
                history = fetch_portfolio_value_history(
                    connection,
                    start_at=effective_start_at,
                    end_at=end_dt.isoformat(timespec="seconds"),
                    limit=5000,
                )
            return {
                "range_key": range_key,
                "history": history,
                "goal_tracking": fetch_goal_tracking(connection, config),
            }

    @app.get("/api/market/status")
    def market_status() -> dict[str, Any]:
        with runtime() as (config, _):
            rows = get_market_status(config)
            return {
                "items": rows,
                "summary": summarize_analysis_window(rows),
            }

    @app.get("/api/market/watchlists")
    def market_watchlists() -> dict[str, Any]:
        with runtime() as (config, connection):
            payload = build_watchlists(config)
            decision_map = fetch_latest_symbol_decisions(connection)

            def attach_decisions(rows: list[dict[str, Any]]) -> None:
                for row in rows:
                    row["decision"] = decision_map.get(str(row.get("symbol") or ""))

            for category in payload.get("categories", []) or []:
                attach_decisions(category.get("items", []) or [])
            for key in ("nordic", "uk", "us", "eu", "global"):
                attach_decisions(payload.get(key, []) or [])
            return payload

    @app.get("/api/prompts")
    def ai_prompts() -> dict[str, Any]:
        with runtime() as (config, connection):
            def safe_prompt(kind: str, title: str, builder) -> dict[str, Any]:
                try:
                    item = builder()
                    item.setdefault("kind", kind)
                    item.setdefault("title", title)
                    item.setdefault("status", "ok")
                    return item
                except Exception as exc:  # noqa: BLE001
                    return {
                        "kind": kind,
                        "title": title,
                        "status": "error",
                        "description": "Prompt preview could not be built from current runtime context.",
                        "error": str(exc),
                    }

            latest_decision = fetch_latest_decision_report(connection)
            latest_manager_run = fetch_latest_trading_manager_run(connection)
            items = [
                safe_prompt(
                    "decision_report",
                    "Decision Report",
                    lambda: build_decision_prompt_preview(config, connection),
                ),
                safe_prompt(
                    "trading_manager",
                    "Trading Manager",
                    lambda: build_trading_manager_prompt_preview(config, connection),
                ),
                safe_prompt(
                    "eod_diary",
                    "End-of-Day Diary",
                    lambda: build_diary_prompt_preview(config),
                ),
            ]
            return {
                "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
                "items": items,
                "latest_decision_report": {
                    "id": latest_decision.get("id") if latest_decision else None,
                    "created_at": latest_decision.get("created_at") if latest_decision else None,
                    "status": latest_decision.get("status") if latest_decision else None,
                    "stored_prompt_text": latest_decision.get("prompt_text") if latest_decision else None,
                },
                "latest_trading_manager_run": latest_manager_run,
            }

    @app.get("/api/decision/latest")
    def decision_latest() -> dict[str, Any]:
        with runtime() as (config, connection):
            report = fetch_latest_decision_report(connection)
            next_report = estimate_next_decision_report(connection, config)
            return {"report": report, "next_report": next_report}

    @app.get("/api/decision/reports")
    def decision_reports(limit: int = Query(default=20, ge=1, le=100)) -> dict[str, Any]:
        with runtime() as (_, connection):
            return {"items": fetch_recent_decision_reports(connection, limit=limit)}

    @app.get("/api/strategy-journal")
    def strategy_journal(limit: int = Query(default=20, ge=1, le=100)) -> dict[str, Any]:
        with runtime() as (_, connection):
            return {"items": fetch_strategy_journal_entries(connection, limit=limit)}

    @app.get("/api/execution")
    def execution(limit: int = Query(default=100, ge=1, le=500)) -> dict[str, Any]:
        with runtime() as (_, connection):
            orders = fetch_execution_orders(connection, limit=limit)
            return {
                "orders": orders,
                "fills": fetch_execution_fills(connection, limit=limit),
                "events": fetch_execution_events(connection, limit=limit),
            }

    @app.get("/api/scheduler")
    def scheduler(limit: int = Query(default=20, ge=1, le=100)) -> dict[str, Any]:
        with runtime() as (_, connection):
            return {
                "status": fetch_scheduler_status(connection),
                "cycles": fetch_scheduler_cycles(connection, limit=limit),
            }

    def _run_action(func, *args, **kwargs) -> dict[str, Any]:
        try:
            return func(*args, **kwargs)
        except Exception as exc:  # noqa: BLE001
            raise HTTPException(status_code=400, detail=str(exc)) from exc

    @app.post("/api/actions/decision-report")
    def action_generate_decision_report() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(generate_decision_report, config=config, connection=connection)

    @app.post("/api/actions/queue-process")
    def action_queue_process() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(queue_and_maybe_execute_latest_report, config=config, connection=connection)

    @app.post("/api/actions/sync-broker")
    def action_sync_broker() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(sync_broker_order_statuses, config=config, connection=connection)

    @app.post("/api/actions/retry-failed")
    def action_retry_failed() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(
                retry_failed_execution_orders,
                config=config,
                connection=connection,
                recoverable_only=True,
            )

    @app.post("/api/actions/reconcile-broker")
    def action_reconcile_broker() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(sync_saxo_sim_account_to_portfolio, config=config, connection=connection)

    @app.post("/api/actions/adopt-broker-portfolio")
    def action_adopt_broker_portfolio() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(adopt_broker_holdings_into_local_ledger, config=config, connection=connection)

    @app.post("/api/actions/sync-saxo-sim-portfolio")
    def action_sync_saxo_sim_portfolio() -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(sync_saxo_sim_account_to_portfolio, config=config, connection=connection)

    @app.post("/api/actions/scheduler-cycle")
    def action_scheduler_cycle(request: SchedulerCycleRequest) -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(
                run_manual_scheduler_cycle,
                config=config,
                connection=connection,
                mock=bool(request.mock),
            )

    @app.post("/api/orders/{order_id}/manage")
    def action_manage_order(order_id: int, request: LiveOrderActionRequest) -> dict[str, Any]:
        with runtime() as (config, connection):
            return _run_action(
                manage_live_order,
                order_id,
                management_action=request.action,
                config=config,
                connection=connection,
                new_quantity=request.quantity,
                new_price=request.price,
            )

    return app


app = create_app()
