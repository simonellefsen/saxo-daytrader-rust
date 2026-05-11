from __future__ import annotations

import contextlib
import io
from datetime import UTC, datetime, timedelta
from typing import Any

import feedparser
import yfinance as yf

from saxo_daytrader_xai.market_symbols import saxo_to_yahoo


def _parse_datetime(value: Any) -> datetime | None:
    if value is None:
        return None
    if isinstance(value, datetime):
        return value.astimezone(UTC) if value.tzinfo else value.replace(tzinfo=UTC)
    if hasattr(value, "to_pydatetime"):
        dt = value.to_pydatetime()
        return dt.astimezone(UTC) if dt.tzinfo else dt.replace(tzinfo=UTC)
    return None


def fetch_rss_items(feeds: list[dict[str, str]], limit: int = 20) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for feed in feeds:
        try:
            parsed = feedparser.parse(feed["url"])
        except Exception as exc:  # noqa: BLE001
            items.append(
                {
                    "source": feed["name"],
                    "title": f"Feed unavailable: {exc}",
                    "url": "",
                    "published_at": None,
                    "category": "error",
                }
            )
            continue

        for entry in parsed.entries[:limit]:
            published = None
            if getattr(entry, "published_parsed", None):
                published = datetime(*entry.published_parsed[:6], tzinfo=UTC)
            items.append(
                {
                    "source": feed["name"],
                    "title": getattr(entry, "title", "").strip(),
                    "url": getattr(entry, "link", "").strip(),
                    "published_at": published.isoformat(timespec="seconds") if published else None,
                    "category": "rss",
                }
            )
    items.sort(key=lambda item: item["published_at"] or "", reverse=True)
    return items[:limit]


def fetch_earnings_calendar(symbols: list[str], days_ahead: int = 14) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    lower_bound = datetime.now(UTC) - timedelta(days=1)
    upper_bound = datetime.now(UTC) + timedelta(days=days_ahead)
    for symbol in symbols:
        yahoo_symbol = saxo_to_yahoo(symbol)
        try:
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
                earnings = yf.Ticker(yahoo_symbol).get_earnings_dates(limit=6)
        except Exception:  # noqa: BLE001
            continue
        if earnings is None or getattr(earnings, "empty", True):
            continue
        for idx, row in earnings.iterrows():
            event_dt = _parse_datetime(idx)
            if event_dt is None or not (lower_bound <= event_dt <= upper_bound):
                continue
            events.append(
                {
                    "symbol": symbol,
                    "yahoo_symbol": yahoo_symbol,
                    "earnings_at": event_dt.isoformat(timespec="seconds"),
                    "eps_estimate": None if "EPS Estimate" not in earnings.columns else row.get("EPS Estimate"),
                    "reported_eps": None if "Reported EPS" not in earnings.columns else row.get("Reported EPS"),
                    "surprise_pct": None if "Surprise(%)" not in earnings.columns else row.get("Surprise(%)"),
                }
            )
    events.sort(key=lambda row: row["earnings_at"])
    return events


def fetch_market_intelligence(
    config: dict[str, Any],
    portfolio_symbols: list[str],
    watchlist_symbols: list[str],
) -> dict[str, Any]:
    market_feeds = config["market_data"]["rss"]["market_feeds"]
    macro_feeds = config["market_data"]["rss"]["macro_feeds"]
    crypto_feeds = config["market_data"]["rss"].get("crypto_feeds", [])
    focus_symbols = list(dict.fromkeys((portfolio_symbols + watchlist_symbols)[:10]))
    return {
        "market_news": fetch_rss_items(market_feeds, limit=18),
        "macro_events": fetch_rss_items(macro_feeds, limit=12),
        "crypto_news": fetch_rss_items(crypto_feeds, limit=8) if crypto_feeds else [],
        "earnings_calendar": fetch_earnings_calendar(focus_symbols, days_ahead=14),
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
    }
