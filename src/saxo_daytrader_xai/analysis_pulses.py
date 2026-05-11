from __future__ import annotations

from datetime import UTC, datetime, timedelta
from typing import Any
from zoneinfo import ZoneInfo


DEFAULT_EU_OPEN_CODES = {"XCSE", "XSTO", "XOSL", "XHEL", "XLON", "XETR", "XFRA", "XMIL", "XAMS"}
DEFAULT_US_OPEN_CODES = {"XNAS", "XNYS"}


def _swing_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("strategy", {}).get("swing", {})


def _pulse_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return _swing_cfg(config).get("analysis_pulses", {})


def _timezone(config: dict[str, Any]) -> ZoneInfo:
    timezone_name = str(_pulse_cfg(config).get("timezone") or "Europe/Copenhagen")
    return ZoneInfo(timezone_name)


def _due_window(config: dict[str, Any]) -> timedelta:
    minutes = int(_pulse_cfg(config).get("due_window_minutes", 20) or 20)
    return timedelta(minutes=max(minutes, 1))


def _pulse_key(kind: str, local_date: str) -> str:
    return f"{kind}:{local_date}"


def _pulse_row(
    *,
    kind: str,
    label: str,
    target_at: datetime,
    now: datetime,
    due_window: timedelta,
    source_markets: list[str],
    exchange_codes: list[str],
) -> dict[str, Any]:
    target_utc = target_at.astimezone(UTC)
    window_end = target_utc + due_window
    local_target = target_at.isoformat(timespec="seconds")
    local_date = target_at.date().isoformat()
    return {
        "key": _pulse_key(kind, local_date),
        "kind": kind,
        "label": label,
        "target_at": local_target,
        "target_at_utc": target_utc.isoformat(timespec="seconds"),
        "window_end_at_utc": window_end.isoformat(timespec="seconds"),
        "due": target_utc <= now < window_end,
        "source_markets": source_markets,
        "exchange_codes": exchange_codes,
    }


def _open_followup_pulses(
    config: dict[str, Any],
    *,
    now: datetime,
    market_status_rows: list[dict[str, Any]],
    cfg_key: str,
    kind: str,
    label: str,
    default_codes: set[str],
    default_minutes_after_open: int,
) -> list[dict[str, Any]]:
    cfg = _pulse_cfg(config).get(cfg_key, {})
    if not bool(cfg.get("enabled", True)):
        return []
    codes = {str(code).upper() for code in cfg.get("exchange_codes", sorted(default_codes))}
    minutes_after_open = int(cfg.get("minutes_after_open", default_minutes_after_open) or default_minutes_after_open)
    grouped: dict[datetime, list[dict[str, Any]]] = {}
    for row in market_status_rows:
        code = str(row.get("code") or "").upper()
        if code not in codes or row.get("holiday_name"):
            continue
        if not row.get("session_open_at_utc") or not row.get("tradable_close_at_utc"):
            continue
        try:
            session_open = datetime.fromisoformat(str(row["session_open_at_utc"])).astimezone(UTC)
            tradable_close = datetime.fromisoformat(str(row["tradable_close_at_utc"])).astimezone(UTC)
        except ValueError:
            continue
        target_at = session_open + timedelta(minutes=minutes_after_open)
        if target_at >= tradable_close:
            continue
        # Only build current/future session pulses. The next pulse comes from
        # the next market-status refresh for the next trading session.
        if now < tradable_close + _due_window(config):
            grouped.setdefault(target_at, []).append(row)
    return [
        _pulse_row(
            kind=kind,
            label=label,
            target_at=target_at,
            now=now,
            due_window=_due_window(config),
            source_markets=sorted({str(row.get("market") or row.get("code")) for row in rows}),
            exchange_codes=sorted({str(row.get("code") or "").upper() for row in rows}),
        )
        for target_at, rows in grouped.items()
    ]


def analysis_pulse_status(
    config: dict[str, Any],
    market_status_rows: list[dict[str, Any]],
    *,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    now = (reference_time or datetime.now(UTC)).astimezone(UTC)
    pulses = [
        *_open_followup_pulses(
            config,
            now=now,
            market_status_rows=market_status_rows,
            cfg_key="europe_open_followup",
            kind="europe_open_followup",
            label="Nordic/EU Open +1h15 Decision Report",
            default_codes=DEFAULT_EU_OPEN_CODES,
            default_minutes_after_open=75,
        ),
        *_open_followup_pulses(
            config,
            now=now,
            market_status_rows=market_status_rows,
            cfg_key="us_open_followup",
            kind="us_open_followup",
            label="US Open +1h15 Decision Report",
            default_codes=DEFAULT_US_OPEN_CODES,
            default_minutes_after_open=75,
        ),
    ]
    active_pulses = [pulse for pulse in pulses if pulse and bool(pulse["due"])]
    future_pulses = [
        pulse for pulse in pulses
        if pulse and datetime.fromisoformat(str(pulse["target_at_utc"])) > now
    ]
    next_pulse = min(
        future_pulses,
        key=lambda pulse: datetime.fromisoformat(str(pulse["target_at_utc"])),
        default=None,
    )
    return {
        "generated_at": now.isoformat(timespec="seconds"),
        "timezone": str(_timezone(config).key),
        "due": bool(active_pulses),
        "active_pulses": active_pulses,
        "pulses": [pulse for pulse in pulses if pulse is not None],
        "next_pulse_at": next_pulse["target_at_utc"] if next_pulse else None,
        "next_pulse_label": next_pulse["label"] if next_pulse else None,
    }
