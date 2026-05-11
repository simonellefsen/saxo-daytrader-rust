from __future__ import annotations

import contextlib
import io
from typing import Any

import pandas as pd
import yfinance as yf


DEFAULT_BENCHMARK_INDICES: dict[str, dict[str, str]] = {
    "UK": {"FTSE 100": "^FTSE"},
    "EU": {"Euro Stoxx 50": "^STOXX50E", "DAX": "^GDAXI"},
    "Nordics": {"OMX Copenhagen 25": "^OMXC25", "OMX Stockholm 30": "^OMX", "Oslo OBX": "OBX.OL"},
    "US": {"S&P 500": "^GSPC", "Nasdaq Composite": "^IXIC", "Dow Jones": "^DJI"},
}


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


def _benchmark_config(config: dict[str, Any]) -> dict[str, dict[str, str]]:
    configured = config.get("strategy", {}).get("swing", {}).get("journal", {}).get("benchmark_indices")
    if not isinstance(configured, dict) or not configured:
        return DEFAULT_BENCHMARK_INDICES
    output: dict[str, dict[str, str]] = {}
    for region, entries in configured.items():
        if not isinstance(entries, dict):
            continue
        output[str(region)] = {str(name): str(ticker) for name, ticker in entries.items() if ticker}
    return output or DEFAULT_BENCHMARK_INDICES


def fetch_benchmark_index_snapshot(config: dict[str, Any], *, timeout_seconds: int = 10) -> dict[str, Any]:
    benchmarks = _benchmark_config(config)
    ticker_to_meta = {
        ticker: {"region": region, "name": name, "ticker": ticker}
        for region, entries in benchmarks.items()
        for name, ticker in entries.items()
    }
    tickers = sorted(ticker_to_meta)
    if not tickers:
        return {"status": "empty", "regions": {}, "items": []}
    try:
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            data = yf.download(
                tickers=tickers,
                period="5d",
                interval="1d",
                auto_adjust=False,
                progress=False,
                threads=True,
                timeout=timeout_seconds,
                group_by="ticker",
            )
    except Exception as exc:  # noqa: BLE001
        return {"status": "error", "error": str(exc), "regions": {}, "items": []}
    if data is None or getattr(data, "empty", True):
        return {"status": "empty", "regions": {}, "items": []}

    items: list[dict[str, Any]] = []
    for ticker in tickers:
        meta = ticker_to_meta[ticker]
        try:
            history = data[ticker] if isinstance(data.columns, pd.MultiIndex) else data
        except KeyError:
            items.append({**meta, "status": "missing"})
            continue
        if history is None or history.empty or "Close" not in history:
            items.append({**meta, "status": "empty"})
            continue
        closes = history["Close"].dropna()
        if closes.empty:
            items.append({**meta, "status": "empty"})
            continue
        current = _coerce_float(closes.iloc[-1])
        previous = _coerce_float(closes.iloc[-2]) if len(closes) > 1 else None
        change_pct = ((current / previous) - 1.0) if current is not None and previous not in (None, 0.0) else None
        items.append(
            {
                **meta,
                "status": "ok",
                "current_price": current,
                "previous_close": previous,
                "change_pct": change_pct,
            }
        )

    regions: dict[str, dict[str, Any]] = {}
    for item in items:
        region = str(item["region"])
        regions.setdefault(region, {"items": [], "average_change_pct": None})
        regions[region]["items"].append(item)
    for region_payload in regions.values():
        changes = [float(item["change_pct"]) for item in region_payload["items"] if item.get("change_pct") is not None]
        region_payload["average_change_pct"] = sum(changes) / len(changes) if changes else None
    return {"status": "ok", "regions": regions, "items": items}
