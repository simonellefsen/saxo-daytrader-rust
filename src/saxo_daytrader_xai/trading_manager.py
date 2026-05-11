from __future__ import annotations

import copy
import json
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import requests

from saxo_daytrader_xai.analysis_pulses import analysis_pulse_status
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import (
    append_audit_log,
    connect,
    has_trading_manager_run,
    init_db,
    record_trading_manager_run,
)
from saxo_daytrader_xai.execution_engine import queue_and_maybe_execute_latest_report
from saxo_daytrader_xai.market_schedule import get_market_status
from saxo_daytrader_xai.market_symbols import parse_exchange_code
from saxo_daytrader_xai.portfolio import fetch_goal_tracking, fetch_latest_batch_id, fetch_portfolio_positions
from saxo_daytrader_xai.saxo_openapi import SaxoRateLimitError
from saxo_daytrader_xai.swing_indicators import fetch_daily_swing_indicators
from saxo_daytrader_xai.watchlists import build_watchlists
from saxo_daytrader_xai.xai_decision import fetch_latest_decision_report
from saxo_daytrader_xai.xai_request import apply_response_options, timeout_seconds


TRADING_MANAGER_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "summary": {"type": "string"},
        "approved_orders": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "strategy_key": {"type": "string"},
                    "symbol": {"type": "string"},
                    "approve": {"type": "boolean"},
                    "confidence": {"type": "number"},
                    "rationale": {"type": "string"},
                    "risk_notes": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["strategy_key", "symbol", "approve", "confidence", "rationale", "risk_notes"],
                "additionalProperties": False,
            },
        },
        "execution_notes": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["summary", "approved_orders", "execution_notes"],
    "additionalProperties": False,
}

TRADING_MANAGER_SYSTEM_PROMPT = "You are the Trading Manager execution gate. Return strict JSON only."

TRADING_MANAGER_INSTRUCTION = (
    "Your job is to curate a long-only swing portfolio, not to flatten the book at the end of each session. "
    "Prefer stocks the Decision Report and daily technicals support for a daily, weekly, and monthly horizon. "
    "Approve only trades that satisfy the swing rules: open exchange, watchlist-only, long-only, "
    "MACD/RSI/Bollinger/Stochastic/OBV confluence, 1:2 reward-risk, and no blacklist symbols. "
    "Account for progress versus the 5,000 DKK weekly and 20,000 DKK monthly pre-tax goals, "
    "but do not approve low-confluence trades just to chase the target. "
    "Reject marginal BUYs. SELL or FLATTEN is allowed only when the position thesis is invalidated, "
    "technicals show clear deterioration, cash/risk limits require de-risking, or a clearly superior open-market opportunity "
    "requires capital rotation. Do not sell merely because the trading day is ending."
)


def _load_default_config() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]
    return load_config(root / "config.yaml")


def _manager_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("strategy", {}).get("swing", {}).get("trading_manager", {})


def _backoff_cfg(config: dict[str, Any]) -> dict[str, int]:
    cfg = _manager_cfg(config)
    return {
        "initial_seconds": int(cfg.get("rate_limit_initial_backoff_seconds", 60) or 60),
        "max_seconds": int(cfg.get("rate_limit_max_backoff_seconds", 900) or 900),
    }


def _enabled(config: dict[str, Any]) -> bool:
    return bool(_manager_cfg(config).get("enabled", True))


def _latest_deferred_attempt_count(connection, manager_key: str) -> int:
    row = connection.execute(
        """
        SELECT manager_json
        FROM trading_manager_runs
        WHERE manager_key = ?
          AND status = 'deferred_rate_limited'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        """,
        (manager_key,),
    ).fetchone()
    if not row:
        return 0
    try:
        payload = json.loads(row["manager_json"]) if row["manager_json"] else {}
    except ValueError:
        return 0
    return int((payload.get("backoff") or {}).get("attempt_count") or 0)


def _rate_limit_backoff_payload(
    *,
    connection,
    config: dict[str, Any],
    pulse: dict[str, Any],
    now: datetime,
    retry_after_seconds: float | None,
) -> dict[str, Any]:
    manager_key = str(pulse.get("key") or "")
    attempt_count = _latest_deferred_attempt_count(connection, manager_key) + 1
    cfg = _backoff_cfg(config)
    exponential_delay = min(
        float(cfg["max_seconds"]),
        float(cfg["initial_seconds"]) * (2 ** max(attempt_count - 1, 0)),
    )
    delay_seconds = max(exponential_delay, float(retry_after_seconds or 0.0))
    next_attempt_at = now + timedelta(seconds=delay_seconds)
    return {
        "attempt_count": attempt_count,
        "delay_seconds": round(delay_seconds, 3),
        "initial_seconds": cfg["initial_seconds"],
        "max_seconds": cfg["max_seconds"],
        "retry_after_seconds": retry_after_seconds,
        "next_attempt_at": next_attempt_at.astimezone(UTC).isoformat(timespec="seconds"),
    }


def trading_manager_status(
    config: dict[str, Any],
    market_status_rows: list[dict[str, Any]] | None = None,
    *,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    now = (reference_time or datetime.now(UTC)).astimezone(UTC)
    if not _enabled(config):
        return {
            "generated_at": now.isoformat(timespec="seconds"),
            "enabled": False,
            "due": False,
            "active_pulses": [],
            "pulses": [],
            "next_pulse_at": None,
            "next_pulse_label": None,
        }
    rows = market_status_rows or get_market_status(config, reference_time=now)
    decision_pulse_summary = analysis_pulse_status(config, rows, reference_time=now)
    pulses = [
        {
            **pulse,
            "label": str(pulse.get("label") or "Decision Report").replace("Decision Report", "Trading Manager"),
            "decision_pulse_key": pulse.get("key"),
        }
        for pulse in decision_pulse_summary.get("pulses", [])
    ]
    pulses = sorted(pulses, key=lambda pulse: str(pulse["target_at_utc"]))
    active_pulses = [pulse for pulse in pulses if pulse["due"]]
    future_pulses = [
        pulse
        for pulse in pulses
        if datetime.fromisoformat(str(pulse["target_at_utc"])).astimezone(UTC) > now
    ]
    next_pulse = future_pulses[0] if future_pulses else None
    return {
        "generated_at": now.isoformat(timespec="seconds"),
        "enabled": True,
        "due": bool(active_pulses),
        "active_pulses": active_pulses,
        "pulses": pulses,
        "next_pulse_at": next_pulse["target_at_utc"] if next_pulse else None,
        "next_pulse_label": next_pulse["label"] if next_pulse else None,
    }


def should_auto_run_trading_manager(
    connection,
    config: dict[str, Any],
    market_status_rows: list[dict[str, Any]] | None = None,
    *,
    reference_time: datetime | None = None,
) -> bool:
    status = trading_manager_status(config, market_status_rows, reference_time=reference_time)
    return any(not has_trading_manager_run(connection, str(pulse["key"])) for pulse in status["active_pulses"])


def _decode_decision_report(row: dict[str, Any] | None) -> dict[str, Any] | None:
    if not row:
        return None
    item = dict(row)
    item["request_json"] = json.loads(item["request_json"]) if item.get("request_json") else None
    item["response_json"] = json.loads(item["response_json"]) if item.get("response_json") else None
    item["report_json"] = json.loads(item["report_json"]) if item.get("report_json") else None
    return item


def _completed_decision_report_for_pulse(connection, pulse_key: str) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT *
        FROM decision_reports
        WHERE analysis_pulse_key = ?
          AND status = 'completed'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        """,
        (pulse_key,),
    ).fetchone()
    return _decode_decision_report(dict(row) if row else None)


def _manager_pulse_from_report(report: dict[str, Any]) -> dict[str, Any] | None:
    report_json = report.get("report_json") or {}
    pulse = report_json.get("analysis_pulse") or {}
    if not pulse.get("key"):
        return None
    return {
        **pulse,
        "label": str(pulse.get("label") or "Decision Report").replace("Decision Report", "Trading Manager"),
        "decision_pulse_key": pulse.get("key"),
        "due": True,
    }


def _extract_output_text(response_json: dict[str, Any]) -> str:
    for item in response_json.get("output", []):
        if item.get("type") != "message":
            continue
        for content in item.get("content", []):
            if content.get("type") == "output_text":
                return content.get("text", "")
    return ""


def build_trading_manager_prompt_payload(
    *,
    manager_pulse: dict[str, Any],
    report: dict[str, Any],
    candidate_orders: list[dict[str, Any]],
    technical_by_symbol: dict[str, dict[str, Any]],
    goal_tracking: dict[str, Any],
) -> dict[str, Any]:
    return {
        "manager_pulse": manager_pulse,
        "decision_report": {
            "id": report.get("id"),
            "created_at": report.get("created_at"),
            "status": report.get("status"),
            "market_regime": (report.get("report_json") or {}).get("market_regime"),
            "portfolio_assessment": (report.get("report_json") or {}).get("portfolio_assessment"),
            "execution_notes": (report.get("report_json") or {}).get("execution_notes"),
        },
        "candidate_orders": candidate_orders,
        "daily_technicals": technical_by_symbol,
        "goal_tracking": goal_tracking,
        "instruction": TRADING_MANAGER_INSTRUCTION,
    }


def build_trading_manager_request_json(config: dict[str, Any], prompt_payload: dict[str, Any]) -> dict[str, Any]:
    request_json = {
        "model": config["xai"]["model"],
        "input": [
            {
                "role": "system",
                "content": TRADING_MANAGER_SYSTEM_PROMPT,
            },
            {"role": "user", "content": json.dumps(prompt_payload, ensure_ascii=False, indent=2)},
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "trading_manager_decision",
                "schema": TRADING_MANAGER_SCHEMA,
                "strict": True,
            }
        },
    }
    return apply_response_options(request_json, config)


def build_trading_manager_prompt_preview(config: dict[str, Any], connection) -> dict[str, Any]:
    market_rows = get_market_status(config)
    status = trading_manager_status(config, market_rows)
    latest_report = fetch_latest_decision_report(connection)
    if latest_report and str(latest_report.get("status") or "") != "completed":
        latest_report = None
    pulse = _manager_pulse_from_report(latest_report) if latest_report else None
    if pulse is None:
        active_pulses = status.get("active_pulses") or []
        all_pulses = status.get("pulses") or []
        pulse = (active_pulses or all_pulses or [{}])[0]
    report = latest_report or {"id": None, "created_at": None, "status": "preview", "report_json": {}}
    candidate_orders = _candidate_orders_for_pulse(report, pulse) if latest_report and pulse else []
    exchange_codes = {str(code).upper() for code in pulse.get("exchange_codes", [])}
    open_codes = {
        str(row.get("code") or "").upper()
        for row in market_rows
        if str(row.get("code") or "").upper() in exchange_codes and bool(row.get("is_tradable"))
    }
    preview_codes = open_codes or exchange_codes
    candidate_symbols = [str(order["symbol"]) for order in candidate_orders if order.get("symbol")]
    if preview_codes:
        technical_symbols = _ordered_unique_symbols(
            candidate_symbols,
            _portfolio_symbols_for_exchanges(connection, config, preview_codes),
            _watchlist_symbols_for_exchanges(config, preview_codes),
        )
    else:
        technical_symbols = _ordered_unique_symbols(candidate_symbols)
    technical_symbols = technical_symbols[: int(_manager_cfg(config).get("max_symbols", 30) or 30)]
    technical_preview = {
        symbol: {"status": "preview_not_fetched", "note": "Live Trading Manager runs fetch full daily indicators before calling xAI."}
        for symbol in technical_symbols
    }
    prompt_payload = build_trading_manager_prompt_payload(
        manager_pulse=pulse,
        report=report,
        candidate_orders=candidate_orders,
        technical_by_symbol=technical_preview,
        goal_tracking=fetch_goal_tracking(connection, config),
    )
    return {
        "kind": "trading_manager",
        "title": "Trading Manager",
        "description": "Execution-gate prompt. Preview uses the latest completed Decision Report and current pulse context; live runs fetch full technical indicators.",
        "system_prompt": TRADING_MANAGER_SYSTEM_PROMPT,
        "instruction": TRADING_MANAGER_INSTRUCTION,
        "user_prompt": json.dumps(prompt_payload, ensure_ascii=False, indent=2, default=str),
        "schema": TRADING_MANAGER_SCHEMA,
        "latest_report_id": latest_report.get("id") if latest_report else None,
        "manager_status": status,
    }


def _request_ai_manager(
    *,
    config: dict[str, Any],
    manager_pulse: dict[str, Any],
    report: dict[str, Any],
    candidate_orders: list[dict[str, Any]],
    technical_by_symbol: dict[str, dict[str, Any]],
    goal_tracking: dict[str, Any],
) -> dict[str, Any]:
    api_key = config.get("xai", {}).get("api_key")
    if not api_key:
        raise ValueError("XAI_API_KEY is missing")
    prompt = build_trading_manager_prompt_payload(
        manager_pulse=manager_pulse,
        report=report,
        candidate_orders=candidate_orders,
        technical_by_symbol=technical_by_symbol,
        goal_tracking=goal_tracking,
    )
    request_json = build_trading_manager_request_json(config, prompt)
    response = requests.post(
        f"{config['xai']['base_url'].rstrip('/')}/responses",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        json=request_json,
        timeout=timeout_seconds(config),
    )
    response.raise_for_status()
    response_json = response.json()
    output_text = _extract_output_text(response_json)
    if not output_text:
        raise ValueError("Trading Manager AI response did not contain structured output text")
    parsed = json.loads(output_text)
    return {"status": "ok", "request_json": request_json, "response_json": response_json, "parsed": parsed}


def _technical_gate(order: dict[str, Any], technical: dict[str, Any] | None) -> tuple[bool, str]:
    if not technical or technical.get("status") != "ok":
        return False, "No usable daily technical indicator result."
    action = str(order.get("action") or "").upper()
    strategy_role = str(order.get("strategy_role") or action).upper()
    sentiment = str(technical.get("sentiment") or "HOLD").upper()
    trend_bias = str(technical.get("trend_bias") or "neutral").lower()
    confluences = int(technical.get("confluence_count") or 0)
    minimum = int(technical.get("min_confluences") or 3)
    if action == "BUY":
        if sentiment not in {"BUY", "OVERWEIGHT"}:
            return False, f"Technical sentiment is {sentiment}, not BUY/OVERWEIGHT."
        if trend_bias != "bullish":
            return False, f"Trend bias is {trend_bias}, not bullish."
        if confluences < minimum:
            return False, f"Only {confluences}/{minimum} indicator confluences."
        return True, "BUY approved by bullish technical confluence."
    if action == "SELL":
        if strategy_role == "FLATTEN" or sentiment in {"SELL", "UNDERWEIGHT"} or trend_bias == "bearish":
            return True, "SELL/FLATTEN approved by deteriorating technicals or explicit flatten role."
        return False, f"SELL not approved; technical sentiment is {sentiment} with {trend_bias} trend."
    return False, f"Unsupported manager action {action}."


def _filter_candidate_orders(
    *,
    candidate_orders: list[dict[str, Any]],
    technical_by_symbol: dict[str, dict[str, Any]],
    ai_result: dict[str, Any] | None,
) -> dict[str, Any]:
    ai_by_key = {
        str(item.get("strategy_key") or ""): item
        for item in ((ai_result or {}).get("approved_orders") or [])
        if item.get("strategy_key")
    }
    approved: list[dict[str, Any]] = []
    skipped: list[dict[str, Any]] = []
    for order in candidate_orders:
        key = str(order.get("strategy_key") or "")
        symbol = str(order.get("symbol") or "")
        technical_ok, technical_reason = _technical_gate(order, technical_by_symbol.get(symbol))
        ai_item = ai_by_key.get(key)
        ai_approved = True if ai_result is None else bool(ai_item and ai_item.get("approve"))
        action = str(order.get("action") or "").upper()
        allow_ai_risk_reduction_without_technicals = (
            ai_result is not None
            and ai_approved
            and action == "SELL"
            and technical_reason == "No usable daily technical indicator result."
        )
        if (technical_ok or allow_ai_risk_reduction_without_technicals) and ai_approved:
            approval_reason = technical_reason
            if allow_ai_risk_reduction_without_technicals:
                approval_reason = f"{technical_reason} AI-approved risk reduction allowed despite missing indicators."
            enriched = dict(order)
            metadata = dict(enriched.get("strategy_metadata") or {})
            metadata["trading_manager"] = {
                "technical_gate": approval_reason,
                "ai_rationale": ai_item.get("rationale") if ai_item else None,
                "ai_confidence": ai_item.get("confidence") if ai_item else None,
            }
            enriched["strategy_metadata"] = metadata
            approved.append(enriched)
            continue
        skipped.append(
            {
                "strategy_key": key,
                "symbol": symbol,
                "action": order.get("action"),
                "technical_gate": technical_reason,
                "ai_approved": ai_approved,
                "ai_rationale": ai_item.get("rationale") if ai_item else None,
            }
        )
    return {"approved_orders": approved, "skipped_orders": skipped}


def _watchlist_symbols_for_exchanges(config: dict[str, Any], exchange_codes: set[str]) -> list[str]:
    symbols: list[str] = []
    watchlists = build_watchlists(config)
    for category in watchlists.get("categories", []) or []:
        for row in category.get("items", []) or []:
            symbol = str(row.get("symbol") or "")
            if parse_exchange_code(symbol).upper() in exchange_codes:
                symbols.append(symbol)
    return symbols


def _ordered_unique_symbols(*symbol_groups: list[str]) -> list[str]:
    symbols: list[str] = []
    seen: set[str] = set()
    for group in symbol_groups:
        for symbol in group:
            normalized = str(symbol or "")
            if not normalized or normalized in seen:
                continue
            seen.add(normalized)
            symbols.append(normalized)
    return symbols


def _portfolio_symbols_for_exchanges(connection, config: dict[str, Any], exchange_codes: set[str]) -> list[str]:
    batch_id = fetch_latest_batch_id(connection)
    symbols: list[str] = []
    for row in fetch_portfolio_positions(connection, batch_id=batch_id, initial_cash_dkk=float(config["portfolio"]["initial_cash_dkk"])):
        symbol = str(row.get("symbol") or "")
        if parse_exchange_code(symbol).upper() in exchange_codes:
            symbols.append(symbol)
    return symbols


def _candidate_orders_for_pulse(report: dict[str, Any], manager_pulse: dict[str, Any]) -> list[dict[str, Any]]:
    exchange_codes = {str(code).upper() for code in manager_pulse.get("exchange_codes", [])}
    strategy_plan = (report.get("report_json") or {}).get("strategy_plan") or {}
    orders = []
    for order in strategy_plan.get("swing_orders", []) or []:
        symbol = str(order.get("symbol") or "")
        if parse_exchange_code(symbol).upper() in exchange_codes:
            orders.append(dict(order))
    return orders


def run_trading_manager_cycle(
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    market_status_rows: list[dict[str, Any]] | None = None,
    reference_time: datetime | None = None,
    force: bool = False,
) -> dict[str, Any]:
    resolved_config = config or _load_default_config()
    resolved_connection = connection or connect(resolved_config["portfolio"]["database_path"])
    init_db(resolved_connection)
    should_close = connection is None
    try:
        now = (reference_time or datetime.now(UTC)).astimezone(UTC)
        market_rows = market_status_rows or get_market_status(resolved_config, reference_time=now)
        status = trading_manager_status(resolved_config, market_rows, reference_time=now)
        latest_report = fetch_latest_decision_report(resolved_connection)
        if latest_report and str(latest_report.get("status") or "") != "completed":
            latest_report = None
        report_pulse = _manager_pulse_from_report(latest_report) if latest_report else None
        due_pulses = list(status["active_pulses"])
        if report_pulse and all(str(pulse.get("key")) != str(report_pulse.get("key")) for pulse in due_pulses):
            due_pulses.append(report_pulse)
        if force and not due_pulses:
            due_pulses = [report_pulse] if report_pulse else status["pulses"][:1]
        runnable = [pulse for pulse in due_pulses if pulse and (force or not has_trading_manager_run(resolved_connection, str(pulse["key"])))]
        if not runnable:
            return {"status": "not_due", "manager_status": status}

        results: list[dict[str, Any]] = []
        for pulse in runnable:
            decision_pulse_key = str(pulse.get("decision_pulse_key") or pulse.get("key") or "")
            report = _completed_decision_report_for_pulse(resolved_connection, decision_pulse_key) if decision_pulse_key else None
            if not report:
                results.append({"status": "skipped_no_completed_report", "pulse": pulse})
                continue
            exchange_codes = {str(code).upper() for code in pulse.get("exchange_codes", [])}
            open_codes = {
                str(row.get("code") or "").upper()
                for row in market_rows
                if str(row.get("code") or "").upper() in exchange_codes and bool(row.get("is_tradable"))
            }
            if not open_codes:
                manager_payload = {"summary": "No source exchanges are currently tradable.", "approved_orders": [], "execution_notes": []}
                run_id = record_trading_manager_run(
                    resolved_connection,
                    manager_pulse=pulse,
                    report_id=int(report["id"]),
                    status="skipped_market_closed",
                    open_exchange_codes=[],
                    technical={},
                    manager=manager_payload,
                )
                results.append({"status": "skipped_market_closed", "id": run_id, "pulse": pulse})
                continue

            candidate_orders = [
                order
                for order in _candidate_orders_for_pulse(report, pulse)
                if parse_exchange_code(str(order.get("symbol") or "")).upper() in open_codes
            ]
            max_symbols = int(_manager_cfg(resolved_config).get("max_symbols", 30) or 30)
            technical_symbols = _ordered_unique_symbols(
                [str(order["symbol"]) for order in candidate_orders if order.get("symbol")],
                _portfolio_symbols_for_exchanges(resolved_connection, resolved_config, open_codes),
                _watchlist_symbols_for_exchanges(resolved_config, open_codes),
            )[:max_symbols]
            indicator_config = copy.deepcopy(resolved_config)
            indicator_config.setdefault("strategy", {}).setdefault("swing", {}).setdefault("daily_indicators", {})["max_symbols"] = max_symbols
            try:
                technical_by_symbol = fetch_daily_swing_indicators(technical_symbols, indicator_config)
            except SaxoRateLimitError as exc:
                backoff = _rate_limit_backoff_payload(
                    connection=resolved_connection,
                    config=resolved_config,
                    pulse=pulse,
                    now=now,
                    retry_after_seconds=exc.retry_after_seconds,
                )
                manager_payload = {
                    "summary": "Trading Manager deferred because Saxo rate-limited daily indicator data.",
                    "approved_order_count": 0,
                    "skipped_order_count": len(candidate_orders),
                    "approved_orders": [],
                    "skipped_orders": [
                        {
                            "strategy_key": order.get("strategy_key"),
                            "symbol": order.get("symbol"),
                            "action": order.get("action"),
                            "technical_gate": "Deferred until Saxo rate-limit backoff expires.",
                            "ai_approved": None,
                            "ai_rationale": None,
                        }
                        for order in candidate_orders
                    ],
                    "execution_notes": [
                        "No orders were created. The same Trading Manager pulse will be retried after the backoff window."
                    ],
                    "backoff": backoff,
                }
                run_id = record_trading_manager_run(
                    resolved_connection,
                    manager_pulse=pulse,
                    report_id=int(report["id"]),
                    status="deferred_rate_limited",
                    open_exchange_codes=sorted(open_codes),
                    technical={},
                    manager=manager_payload,
                    queue_result={"status": "deferred_rate_limited", "orders": []},
                    error_text=str(exc),
                )
                append_audit_log(
                    resolved_connection,
                    "trading_manager_deferred_rate_limited",
                    {
                        "manager_key": pulse["key"],
                        "report_id": report["id"],
                        "error": str(exc),
                        "backoff": backoff,
                    },
                )
                results.append(
                    {
                        "status": "deferred_rate_limited",
                        "id": run_id,
                        "pulse": pulse,
                        "open_exchange_codes": sorted(open_codes),
                        "backoff": backoff,
                        "queue": {"status": "deferred_rate_limited", "orders": []},
                    }
                )
                continue

            ai_payload = None
            ai_error = None
            if bool(_manager_cfg(resolved_config).get("use_ai", True)) and candidate_orders:
                try:
                    ai_response = _request_ai_manager(
                        config=resolved_config,
                        manager_pulse=pulse,
                        report=report,
                        candidate_orders=candidate_orders,
                        technical_by_symbol=technical_by_symbol,
                        goal_tracking=fetch_goal_tracking(resolved_connection, resolved_config),
                    )
                    ai_payload = ai_response["parsed"]
                except Exception as exc:  # noqa: BLE001
                    ai_error = str(exc)
                    append_audit_log(
                        resolved_connection,
                        "trading_manager_ai_failed",
                        {"manager_key": pulse["key"], "report_id": report["id"], "error": ai_error},
                    )

            manager_decision = _filter_candidate_orders(
                candidate_orders=candidate_orders,
                technical_by_symbol=technical_by_symbol,
                ai_result=ai_payload,
            )
            queue_result = queue_and_maybe_execute_latest_report(
                config=resolved_config,
                connection=resolved_connection,
                create_report_orders=True,
                strategy_orders_override=manager_decision["approved_orders"],
                report_override=report,
            )
            manager_payload = {
                "summary": (ai_payload or {}).get("summary") or "Trading Manager used deterministic technical execution gates.",
                "ai_error": ai_error,
                "approved_order_count": len(manager_decision["approved_orders"]),
                "skipped_order_count": len(manager_decision["skipped_orders"]),
                "approved_orders": [
                    {"strategy_key": order.get("strategy_key"), "symbol": order.get("symbol"), "action": order.get("action")}
                    for order in manager_decision["approved_orders"]
                ],
                "skipped_orders": manager_decision["skipped_orders"],
                "execution_notes": (ai_payload or {}).get("execution_notes") or [],
            }
            run_status = "completed" if manager_decision["approved_orders"] else "completed_no_orders"
            run_id = record_trading_manager_run(
                resolved_connection,
                manager_pulse=pulse,
                report_id=int(report["id"]),
                status=run_status,
                open_exchange_codes=sorted(open_codes),
                technical=technical_by_symbol,
                manager=manager_payload,
                queue_result=queue_result,
            )
            results.append(
                {
                    "status": run_status,
                    "id": run_id,
                    "pulse": pulse,
                    "open_exchange_codes": sorted(open_codes),
                    "approved_orders": manager_decision["approved_orders"],
                    "skipped_orders": manager_decision["skipped_orders"],
                    "queue": queue_result,
                }
            )
        if results and all(str(result.get("status")) == "skipped_no_completed_report" for result in results):
            return {
                "status": "skipped_no_completed_report",
                "manager_status": status,
                "skipped_pulses": [result["pulse"] for result in results],
            }
        return {"status": "ok", "manager_status": status, "runs": results}
    finally:
        if should_close:
            resolved_connection.close()
