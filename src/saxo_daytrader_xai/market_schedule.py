from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, date, datetime, time, timedelta
from typing import Any

import pytz

try:
    import exchange_calendars as xcals
except ImportError:  # pragma: no cover - validated through runtime dependency
    xcals = None


@dataclass(frozen=True)
class ExchangeSchedule:
    code: str
    name: str
    timezone: str
    open_time: time
    close_time: time
    tradable_close_offset_minutes: int = 0
    use_configured_session_times: bool = False


@dataclass
class CalendarCacheEntry:
    fetched_at: datetime
    start_date: date
    end_date: date
    sessions: dict[date, tuple[datetime, datetime]]
    source: str


DEFAULT_EXCHANGES: list[ExchangeSchedule] = [
    ExchangeSchedule("XCSE", "Copenhagen", "Europe/Copenhagen", time(9, 0), time(17, 0)),
    ExchangeSchedule("XLON", "London", "Europe/London", time(8, 0), time(16, 30)),
    ExchangeSchedule("XETR", "Frankfurt / Xetra", "Europe/Berlin", time(9, 0), time(17, 30)),
    ExchangeSchedule("XAMS", "Amsterdam / Euronext", "Europe/Amsterdam", time(9, 0), time(17, 30)),
    ExchangeSchedule("XNAS", "Nasdaq US", "America/New_York", time(9, 30), time(16, 0)),
    ExchangeSchedule("XNYS", "NYSE", "America/New_York", time(9, 30), time(16, 0)),
    ExchangeSchedule("XSTO", "Stockholm", "Europe/Stockholm", time(9, 0), time(17, 30)),
    ExchangeSchedule(
        "XOSL",
        "Oslo",
        "Europe/Oslo",
        time(9, 0),
        time(16, 30),
        tradable_close_offset_minutes=5,
        use_configured_session_times=True,
    ),
    ExchangeSchedule("XHEL", "Helsinki", "Europe/Helsinki", time(10, 0), time(18, 30)),
    ExchangeSchedule("XMIL", "Milan", "Europe/Rome", time(9, 0), time(17, 30)),
]

HOLIDAY_LABEL_OVERRIDES: dict[str, dict[date, str]] = {
    "XCSE": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 2): "Maundy Thursday",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 14): "Ascension Day",
        date(2026, 5, 15): "Day after Ascension Day",
        date(2026, 5, 25): "Whit Monday",
        date(2026, 6, 5): "Constitution Day",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
    "XLON": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 4): "Early May bank holiday",
        date(2026, 5, 25): "Spring bank holiday",
        date(2026, 8, 31): "Summer bank holiday",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 28): "Boxing Day (substitute day)",
    },
    "XETR": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
    "XAMS": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 1): "Labour Day",
        date(2026, 12, 25): "Christmas Day",
    },
    "XNAS": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 1, 19): "Martin Luther King Jr. Day",
        date(2026, 2, 16): "Presidents Day",
        date(2026, 4, 3): "Good Friday",
        date(2026, 5, 25): "Memorial Day",
        date(2026, 6, 19): "Juneteenth",
        date(2026, 7, 3): "Independence Day (observed)",
        date(2026, 9, 7): "Labor Day",
        date(2026, 11, 26): "Thanksgiving Day",
        date(2026, 12, 25): "Christmas Day",
    },
    "XNYS": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 1, 19): "Martin Luther King Jr. Day",
        date(2026, 2, 16): "Washington's Birthday",
        date(2026, 4, 3): "Good Friday",
        date(2026, 5, 25): "Memorial Day",
        date(2026, 6, 19): "Juneteenth",
        date(2026, 7, 3): "Independence Day (observed)",
        date(2026, 9, 7): "Labor Day",
        date(2026, 11, 26): "Thanksgiving Day",
        date(2026, 12, 25): "Christmas Day",
    },
    "XSTO": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 1, 6): "Epiphany",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 1): "Labour Day",
        date(2026, 5, 14): "Ascension Day",
        date(2026, 6, 19): "Midsummer Eve",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
    "XOSL": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 2): "Maundy Thursday",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 1): "Labour Day",
        date(2026, 5, 14): "Ascension Day",
        date(2026, 5, 25): "Whit Monday",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
    "XHEL": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 1, 6): "Epiphany",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 1): "Labour Day",
        date(2026, 5, 14): "Ascension Day",
        date(2026, 6, 19): "Midsummer Eve",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
    "XMIL": {
        date(2026, 1, 1): "New Year's Day",
        date(2026, 4, 3): "Good Friday",
        date(2026, 4, 6): "Easter Monday",
        date(2026, 5, 1): "Labour Day",
        date(2026, 12, 24): "Christmas Eve",
        date(2026, 12, 25): "Christmas Day",
        date(2026, 12, 31): "New Year's Eve",
    },
}

_CALENDAR_OBJECTS: dict[str, Any] = {}
_CALENDAR_CACHE: dict[str, CalendarCacheEntry] = {}


def _holiday_name(exchange_code: str, local_date: date) -> str | None:
    return HOLIDAY_LABEL_OVERRIDES.get(exchange_code, {}).get(local_date)


def _is_trading_day(exchange: ExchangeSchedule, local_date: date) -> bool:
    return local_date.weekday() < 5 and _holiday_name(exchange.code, local_date) is None


def _localized_session_time(
    exchange: ExchangeSchedule,
    timezone: pytz.BaseTzInfo,
    session_date: date,
    session_time: time,
) -> datetime:
    return timezone.localize(
        datetime.combine(session_date, session_time),
        is_dst=None,
    )


def _config_int(config: dict[str, Any], key: str, default: int) -> int:
    return int(config.get("analysis_windows", {}).get(key, default))


def _calendar_window(config: dict[str, Any], reference_date: date) -> tuple[date, date]:
    lookback_days = _config_int(config, "calendar_lookback_days", 7)
    lookahead_days = _config_int(config, "calendar_lookahead_days", 45)
    return (
        reference_date - timedelta(days=lookback_days),
        reference_date + timedelta(days=lookahead_days),
    )


def _get_exchange_calendar(exchange_code: str) -> Any:
    if xcals is None:
        return None
    if exchange_code not in _CALENDAR_OBJECTS:
        _CALENDAR_OBJECTS[exchange_code] = xcals.get_calendar(exchange_code)
    return _CALENDAR_OBJECTS[exchange_code]


def _build_schedule_from_exchange_calendar(
    exchange: ExchangeSchedule,
    start_date: date,
    end_date: date,
) -> tuple[dict[date, tuple[datetime, datetime]], str]:
    calendar = _get_exchange_calendar(exchange.code)
    schedule_df = calendar.schedule.loc[str(start_date) : str(end_date), ["open", "close"]]
    sessions: dict[date, tuple[datetime, datetime]] = {}
    for session_label, row in schedule_df.iterrows():
        session_date = session_label.date()
        open_dt = row["open"].to_pydatetime()
        close_dt = row["close"].to_pydatetime()
        sessions[session_date] = (open_dt, close_dt)
    return sessions, "exchange_calendars"


def _build_fallback_schedule(
    exchange: ExchangeSchedule,
    start_date: date,
    end_date: date,
) -> tuple[dict[date, tuple[datetime, datetime]], str]:
    timezone = pytz.timezone(exchange.timezone)
    sessions: dict[date, tuple[datetime, datetime]] = {}
    current_date = start_date
    while current_date <= end_date:
        if _is_trading_day(exchange, current_date):
            sessions[current_date] = (
                _localized_session_time(exchange, timezone, current_date, exchange.open_time).astimezone(UTC),
                _localized_session_time(exchange, timezone, current_date, exchange.close_time).astimezone(UTC),
            )
        current_date += timedelta(days=1)
    return sessions, "fallback"


def _refresh_calendar_entry(
    exchange: ExchangeSchedule,
    config: dict[str, Any],
    reference_date: date,
) -> CalendarCacheEntry:
    start_date, end_date = _calendar_window(config, reference_date)
    fetched_at = datetime.now(UTC)
    try:
        sessions, source = _build_schedule_from_exchange_calendar(exchange, start_date, end_date)
    except Exception:  # noqa: BLE001
        sessions, source = _build_fallback_schedule(exchange, start_date, end_date)
    entry = CalendarCacheEntry(
        fetched_at=fetched_at,
        start_date=start_date,
        end_date=end_date,
        sessions=sessions,
        source=source,
    )
    _CALENDAR_CACHE[exchange.code] = entry
    return entry


def _ensure_calendar_entry(
    exchange: ExchangeSchedule,
    config: dict[str, Any],
    reference_date: date,
    *,
    force_refresh: bool = False,
) -> CalendarCacheEntry:
    entry = _CALENDAR_CACHE.get(exchange.code)
    refresh_minutes = _config_int(config, "calendar_refresh_interval_minutes", 360)
    stale_before = datetime.now(UTC) - timedelta(minutes=refresh_minutes)
    required_start, required_end = _calendar_window(config, reference_date)

    needs_refresh = (
        force_refresh
        or entry is None
        or entry.fetched_at < stale_before
        or required_start < entry.start_date
        or required_end > entry.end_date
    )
    if needs_refresh:
        return _refresh_calendar_entry(exchange, config, reference_date)
    return entry


def refresh_market_calendars(
    config: dict[str, Any],
    reference_time: datetime | None = None,
    *,
    force_refresh: bool = False,
) -> dict[str, Any]:
    now_utc = (reference_time or datetime.now(UTC)).astimezone(UTC)
    reference_date = now_utc.date()
    refreshed_codes: list[str] = []
    sources: dict[str, str] = {}

    for exchange in DEFAULT_EXCHANGES:
        cached_before = _CALENDAR_CACHE.get(exchange.code)
        entry = _ensure_calendar_entry(exchange, config, reference_date, force_refresh=force_refresh)
        sources[exchange.code] = entry.source
        if cached_before is not entry:
            refreshed_codes.append(exchange.code)

    return {
        "checked_at": now_utc.isoformat(timespec="seconds"),
        "refreshed_count": len(refreshed_codes),
        "refreshed_codes": refreshed_codes,
        "sources": sources,
    }


def _next_open_from_entry(
    entry: CalendarCacheEntry,
    exchange: ExchangeSchedule,
    config: dict[str, Any],
    now_utc: datetime,
    reference_date: date,
) -> datetime:
    future_opens = [open_dt for open_dt, _ in entry.sessions.values() if open_dt > now_utc]
    if future_opens:
        return min(future_opens)

    extended_reference = reference_date + timedelta(days=_config_int(config, "calendar_lookahead_days", 45))
    extended_entry = _refresh_calendar_entry(exchange, config, extended_reference)
    future_opens = [open_dt for open_dt, _ in extended_entry.sessions.values() if open_dt > now_utc]
    if future_opens:
        return min(future_opens)

    timezone = pytz.timezone(exchange.timezone)
    return _localized_session_time(exchange, timezone, reference_date + timedelta(days=1), exchange.open_time).astimezone(UTC)


def get_market_status(config: dict[str, Any], reference_time: datetime | None = None) -> list[dict[str, Any]]:
    now_utc = (reference_time or datetime.now(UTC)).astimezone(UTC)
    analysis_cfg = config["analysis_windows"]
    offset_minutes = int(analysis_cfg["offset_minutes_after_open"])
    pre_sync_minutes = int(analysis_cfg.get("pre_sync_minutes_before_analysis", 5))
    end_buffer_minutes = int(analysis_cfg.get("end_buffer_minutes_before_close", 15))
    refresh_market_calendars(config, reference_time=now_utc)
    rows: list[dict[str, Any]] = []

    for exchange in DEFAULT_EXCHANGES:
        tz = pytz.timezone(exchange.timezone)
        local_now = now_utc.astimezone(tz)
        local_date = local_now.date()
        entry = _ensure_calendar_entry(exchange, config, local_date)
        session = entry.sessions.get(local_date)
        holiday_name = None
        if session is None and local_date.weekday() < 5:
            holiday_name = _holiday_name(exchange.code, local_date) or "Exchange holiday"

        if session is not None:
            open_utc, close_utc = session
            open_dt = open_utc.astimezone(tz)
            close_dt = close_utc.astimezone(tz)
            if exchange.use_configured_session_times:
                open_dt = _localized_session_time(exchange, tz, local_date, exchange.open_time)
                close_dt = _localized_session_time(exchange, tz, local_date, exchange.close_time)
                open_utc = open_dt.astimezone(UTC)
                close_utc = close_dt.astimezone(UTC)
            tradable_close_dt = close_dt - timedelta(minutes=exchange.tradable_close_offset_minutes)
            tradable_close_utc = tradable_close_dt.astimezone(UTC)
            is_open = open_utc <= now_utc <= close_utc
            is_tradable = open_utc <= now_utc < tradable_close_utc
            open_analysis_start = open_dt + timedelta(minutes=offset_minutes)
            open_analysis_end = max(open_analysis_start, tradable_close_dt - timedelta(minutes=end_buffer_minutes))
            pre_sync_start = max(open_dt, open_analysis_start - timedelta(minutes=pre_sync_minutes))
            close_analysis_start = None
            close_analysis_end = None
            open_analysis_window_active = open_analysis_start <= local_now <= open_analysis_end
            close_analysis_window_active = False
            pre_analysis_sync_active = pre_sync_start <= local_now < open_analysis_start
            analysis_window_active = open_analysis_window_active
            analysis_window_kind = "open" if open_analysis_window_active else None
        else:
            open_dt = None
            close_dt = None
            tradable_close_dt = None
            tradable_close_utc = None
            is_open = False
            is_tradable = False
            open_analysis_start = None
            open_analysis_end = None
            close_analysis_start = None
            close_analysis_end = None
            pre_sync_start = None
            pre_analysis_sync_active = False
            open_analysis_window_active = False
            close_analysis_window_active = False
            analysis_window_active = False
            analysis_window_kind = None

        next_open = _next_open_from_entry(entry, exchange, config, now_utc, local_date).astimezone(tz)
        if holiday_name is not None:
            status_reason = f"Closed - {holiday_name}"
        elif local_now.weekday() >= 5:
            status_reason = "Closed - Weekend"
        elif open_dt is not None and local_now < open_dt:
            status_reason = "Pre-open"
        elif tradable_close_dt is not None and local_now >= tradable_close_dt and local_now <= close_dt:
            status_reason = "Closed - Closing auction / post-trade"
        elif close_dt is not None and local_now > close_dt:
            status_reason = "Closed - After hours"
        else:
            status_reason = "Open"
        rows.append(
            {
                "code": exchange.code,
                "market": exchange.name,
                "timezone": exchange.timezone,
                "local_time": local_now.strftime("%Y-%m-%d %H:%M"),
                "status_reason": status_reason,
                "holiday_name": holiday_name,
                "session_open_local": open_dt.strftime("%Y-%m-%d %H:%M") if open_dt is not None else "n/a",
                "session_close_local": close_dt.strftime("%Y-%m-%d %H:%M") if close_dt is not None else "n/a",
                "session_open_utc": open_dt.astimezone(UTC).strftime("%Y-%m-%d %H:%M") if open_dt is not None else "n/a",
                "session_close_utc": close_dt.astimezone(UTC).strftime("%Y-%m-%d %H:%M") if close_dt is not None else "n/a",
                "tradable_close_local": tradable_close_dt.strftime("%Y-%m-%d %H:%M") if tradable_close_dt is not None else "n/a",
                "session_open_at_utc": open_dt.astimezone(UTC).isoformat(timespec="seconds") if open_dt is not None else None,
                "session_close_at_utc": close_dt.astimezone(UTC).isoformat(timespec="seconds") if close_dt is not None else None,
                "tradable_close_at_utc": tradable_close_utc.isoformat(timespec="seconds") if tradable_close_utc is not None else None,
                "calendar_source": entry.source,
                "calendar_last_checked": entry.fetched_at.strftime("%Y-%m-%d %H:%M UTC"),
                "is_open": is_open,
                "is_tradable": is_tradable,
                "pre_analysis_sync_active": pre_analysis_sync_active,
                "open_analysis_window_active": open_analysis_window_active,
                "close_analysis_window_active": close_analysis_window_active,
                "analysis_window_active": analysis_window_active,
                "analysis_window_kind": analysis_window_kind,
                "pre_analysis_sync_start": pre_sync_start.strftime("%Y-%m-%d %H:%M") if pre_sync_start is not None else "n/a",
                "pre_analysis_sync_start_at_utc": pre_sync_start.astimezone(UTC).isoformat(timespec="seconds") if pre_sync_start is not None else None,
                "open_analysis_window_start": open_analysis_start.strftime("%Y-%m-%d %H:%M") if open_analysis_start is not None else "n/a",
                "open_analysis_window_end": open_analysis_end.strftime("%Y-%m-%d %H:%M") if open_analysis_end is not None else "n/a",
                "open_analysis_window_start_at_utc": open_analysis_start.astimezone(UTC).isoformat(timespec="seconds") if open_analysis_start is not None else None,
                "open_analysis_window_end_at_utc": open_analysis_end.astimezone(UTC).isoformat(timespec="seconds") if open_analysis_end is not None else None,
                "close_analysis_window_start": close_analysis_start.strftime("%Y-%m-%d %H:%M") if close_analysis_start is not None else "n/a",
                "close_analysis_window_end": close_analysis_end.strftime("%Y-%m-%d %H:%M") if close_analysis_end is not None else "n/a",
                "close_analysis_window_start_at_utc": close_analysis_start.astimezone(UTC).isoformat(timespec="seconds") if close_analysis_start is not None else None,
                "close_analysis_window_end_at_utc": close_analysis_end.astimezone(UTC).isoformat(timespec="seconds") if close_analysis_end is not None else None,
                "analysis_window_start": open_analysis_start.strftime("%Y-%m-%d %H:%M") if open_analysis_start is not None else "n/a",
                "analysis_window_end": open_analysis_end.strftime("%Y-%m-%d %H:%M") if open_analysis_end is not None else "n/a",
                "next_open": next_open.strftime("%Y-%m-%d %H:%M"),
                "next_open_at_utc": next_open.astimezone(UTC).isoformat(timespec="seconds"),
            }
        )
    return rows


def summarize_analysis_window(status_rows: list[dict[str, Any]]) -> dict[str, Any]:
    open_markets = [row["market"] for row in status_rows if row["open_analysis_window_active"]]
    close_markets = [row["market"] for row in status_rows if row["close_analysis_window_active"]]
    pre_sync_markets = [row["market"] for row in status_rows if row["pre_analysis_sync_active"]]
    active_markets = open_markets + [market for market in close_markets if market not in open_markets]
    active_windows: list[str] = []
    if open_markets:
        active_windows.extend([f"{market} (open window)" for market in open_markets])
    if close_markets:
        active_windows.extend([f"{market} (close window)" for market in close_markets])
    return {
        "analysis_window_active": bool(active_markets),
        "active_markets": active_markets,
        "active_windows": active_windows,
        "open_active_markets": open_markets,
        "close_active_markets": close_markets,
        "pre_sync_markets": pre_sync_markets,
    }
