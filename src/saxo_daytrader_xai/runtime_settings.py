from __future__ import annotations

import copy
import json
from datetime import UTC, datetime
from typing import Any


CASH_BUFFER_KEY = "strategy.capital.cash_buffer"


def _safe_float(value: Any, default: float) -> float:
    try:
        if value is None:
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


def _normalized_pct(value: Any, default: float) -> float:
    raw = _safe_float(value, default)
    if raw > 1.0:
        raw = raw / 100.0
    return max(0.0, min(raw, 0.95))


def _base_cash_buffer_pct(config: dict[str, Any]) -> float:
    capital_cfg = config.get("strategy", {}).get("capital", {})
    max_deployment = _normalized_pct(capital_cfg.get("max_deployment_pct", 0.75), 0.75)
    return _normalized_pct(capital_cfg.get("min_cash_buffer_pct", max(0.0, 1.0 - max_deployment)), max(0.0, 1.0 - max_deployment))


def fetch_runtime_setting(connection, key: str) -> dict[str, Any] | None:
    row = connection.execute(
        "SELECT key, value_json, updated_at FROM runtime_settings WHERE key = ?",
        (key,),
    ).fetchone()
    if row is None:
        return None
    try:
        value = json.loads(row["value_json"] or "{}")
    except json.JSONDecodeError:
        value = {}
    return {"key": row["key"], "value": value, "updated_at": row["updated_at"]}


def upsert_runtime_setting(connection, key: str, value: dict[str, Any]) -> dict[str, Any]:
    updated_at = datetime.now(UTC).isoformat(timespec="seconds")
    value_json = json.dumps(value, ensure_ascii=False, sort_keys=True)
    connection.execute(
        """
        INSERT INTO runtime_settings (key, value_json, updated_at)
        VALUES (?, ?, ?)
        ON CONFLICT(key) DO UPDATE SET
            value_json = excluded.value_json,
            updated_at = excluded.updated_at
        """,
        (key, value_json, updated_at),
    )
    connection.commit()
    return {"key": key, "value": value, "updated_at": updated_at}


def apply_runtime_settings(config: dict[str, Any], connection) -> dict[str, Any]:
    setting = fetch_runtime_setting(connection, CASH_BUFFER_KEY)
    if not setting:
        return config

    value = setting.get("value") or {}
    min_cash_buffer_pct = _normalized_pct(value.get("min_cash_buffer_pct"), _base_cash_buffer_pct(config))
    adjusted = copy.deepcopy(config)
    strategy_cfg = adjusted.setdefault("strategy", {})
    capital_cfg = strategy_cfg.setdefault("capital", {})
    capital_cfg["min_cash_buffer_pct"] = min_cash_buffer_pct
    capital_cfg["max_deployment_pct"] = max(0.0, min(1.0 - min_cash_buffer_pct, 1.0))
    return adjusted


def fetch_cash_buffer_settings(config: dict[str, Any], connection) -> dict[str, Any]:
    base_pct = _base_cash_buffer_pct(config)
    setting = fetch_runtime_setting(connection, CASH_BUFFER_KEY)
    effective_pct = base_pct
    updated_at = None
    source = "config"
    if setting:
        effective_pct = _normalized_pct((setting.get("value") or {}).get("min_cash_buffer_pct"), base_pct)
        updated_at = setting.get("updated_at")
        source = "runtime"
    return {
        "min_cash_buffer_pct": effective_pct,
        "max_deployment_pct": max(0.0, min(1.0 - effective_pct, 1.0)),
        "source": source,
        "updated_at": updated_at,
        "config_default_min_cash_buffer_pct": base_pct,
    }


def update_cash_buffer_settings(config: dict[str, Any], connection, *, min_cash_buffer_pct: float) -> dict[str, Any]:
    normalized = _normalized_pct(min_cash_buffer_pct, _base_cash_buffer_pct(config))
    if normalized < 0.0 or normalized > 0.90:
        raise ValueError("Cash buffer must be between 0% and 90%.")
    upsert_runtime_setting(
        connection,
        CASH_BUFFER_KEY,
        {
            "min_cash_buffer_pct": normalized,
            "max_deployment_pct": max(0.0, min(1.0 - normalized, 1.0)),
        },
    )
    return fetch_cash_buffer_settings(config, connection)
