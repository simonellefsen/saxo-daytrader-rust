from __future__ import annotations

import contextlib
import io
from typing import Any

import pandas as pd
import yfinance as yf

from saxo_daytrader_xai.market_symbols import saxo_to_yahoo


def _coerce_float(value: Any) -> float | None:
    if value is None:
        return None
    try:
        if pd.isna(value):
            return None
    except TypeError:
        pass
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _empty_quote_row(symbol: str, yahoo_symbol: str, reason: str) -> dict[str, Any]:
    return {
        "symbol": symbol,
        "yahoo_symbol": yahoo_symbol,
        "current_price": None,
        "previous_close": None,
        "change_pct": None,
        "source": "unavailable",
        "status": reason,
    }


def fetch_live_prices(
    symbols: list[str],
    timeout_seconds: int = 10,
    symbol_to_yahoo: dict[str, str] | None = None,
) -> list[dict[str, Any]]:
    symbol_to_yahoo = symbol_to_yahoo or {}
    requested_symbols = [symbol for symbol in symbols if symbol]
    yahoo_by_symbol = {
        symbol: symbol_to_yahoo.get(symbol, saxo_to_yahoo(symbol))
        for symbol in requested_symbols
    }
    unique_yahoo_symbols = sorted({value for value in yahoo_by_symbol.values() if value})
    if not unique_yahoo_symbols:
        return []

    results = {
        symbol: _empty_quote_row(symbol, yahoo_symbol, "No market data requested")
        for symbol, yahoo_symbol in yahoo_by_symbol.items()
    }

    try:
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            data = yf.download(
                tickers=unique_yahoo_symbols,
                period="5d",
                interval="1d",
                auto_adjust=False,
                progress=False,
                threads=True,
                timeout=timeout_seconds,
                group_by="ticker",
            )
    except Exception as exc:  # noqa: BLE001
        return [
            _empty_quote_row(symbol, yahoo_symbol, f"Quote fetch failed: {exc}")
            for symbol, yahoo_symbol in yahoo_by_symbol.items()
        ]

    if data is None or getattr(data, "empty", True):
        return [
            _empty_quote_row(symbol, yahoo_symbol, "No quote data returned")
            for symbol, yahoo_symbol in yahoo_by_symbol.items()
        ]

    for symbol, yahoo_symbol in yahoo_by_symbol.items():
        try:
            history = data[yahoo_symbol] if isinstance(data.columns, pd.MultiIndex) else data
        except KeyError:
            results[symbol] = _empty_quote_row(symbol, yahoo_symbol, "Ticker missing from response")
            continue

        if history is None or history.empty or "Close" not in history:
            results[symbol] = _empty_quote_row(symbol, yahoo_symbol, "Empty price history")
            continue

        closes = history["Close"].dropna()
        if closes.empty:
            results[symbol] = _empty_quote_row(symbol, yahoo_symbol, "No closing prices available")
            continue

        current_price = _coerce_float(closes.iloc[-1])
        previous_close = _coerce_float(closes.iloc[-2]) if len(closes) > 1 else None
        change_pct = None
        if current_price is not None and previous_close not in (None, 0):
            change_pct = (current_price / previous_close) - 1

        results[symbol] = {
            "symbol": symbol,
            "yahoo_symbol": yahoo_symbol,
            "current_price": current_price,
            "previous_close": previous_close,
            "change_pct": change_pct,
            "source": "yfinance",
            "status": "ok",
        }

    return [results[symbol] for symbol in requested_symbols]
