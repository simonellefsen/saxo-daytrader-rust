from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import pandas as pd

from saxo_daytrader_xai.fx_service import fetch_ecb_fx_rates, fx_rate_to_dkk
from saxo_daytrader_xai.market_schedule import get_market_status
from saxo_daytrader_xai.market_symbols import parse_exchange_code
from saxo_daytrader_xai.saxo_openapi import (
    SaxoSessionError,
    ensure_access_token,
    get_chart_samples,
    lookup_instrument,
    normalize_order_price,
    price_tick_size_for_symbol,
)
from saxo_daytrader_xai.swing_strategy import build_swing_strategy_plan, swing_strategy_enabled


EU_EXCHANGES = {"xcse", "xsto", "xosl", "xhel", "xlon", "xetr", "xfra", "xmil", "xpar", "xams", "xbru", "xlse"}
US_EXCHANGES = {"xnas", "xnys"}
TERMINAL_ORDER_STATUSES = {
    "executed",
    "execution_failed",
    "broker_rejected",
    "broker_cancelled",
    "broker_expired",
    "cancelled",
    "error",
    "invalid_quantity",
}

SESSION_ERROR_MARKERS = (
    "refresh token",
    "access token",
    "oauth",
    "authorization",
    "unauthorized",
    "forbidden",
    "session file",
    "client key",
    "account key",
)


@dataclass(frozen=True)
class CandidateMetrics:
    symbol: str
    session_tag: str
    currency: str
    current_price_local: float
    atr_1m: float
    rung_spacing_local: float
    technical_score: float
    volume_score: float
    rvol_15m: float
    vwap_local: float
    decimals: int
    notes: list[str]


def _strategy_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("strategy", {})


def _normalized_pct(value: Any, default: float) -> float:
    raw = _safe_float(value, default)
    if raw > 1.0:
        raw = raw / 100.0
    return max(0.0, min(raw, 1.0))


def _capital_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return _strategy_cfg(config).get("capital", {})


def strategy_enabled(config: dict[str, Any]) -> bool:
    return bool(_strategy_cfg(config).get("enabled", True))


def strategy_max_deployment_pct(config: dict[str, Any]) -> float:
    return _normalized_pct(_capital_cfg(config).get("max_deployment_pct", 0.75), 0.75)


def strategy_min_cash_buffer_pct(config: dict[str, Any]) -> float:
    capital_cfg = _capital_cfg(config)
    default_buffer = max(0.0, 1.0 - strategy_max_deployment_pct(config))
    return _normalized_pct(capital_cfg.get("min_cash_buffer_pct", default_buffer), default_buffer)


def strategy_capital_limits(
    *,
    config: dict[str, Any],
    total_market_value_dkk: float,
    invested_market_value_dkk: float,
    cash_balance_dkk: float,
) -> dict[str, float]:
    total_value = max(_safe_float(total_market_value_dkk), 0.0)
    invested_value = max(_safe_float(invested_market_value_dkk), 0.0)
    cash_value = max(_safe_float(cash_balance_dkk), 0.0)
    max_deployment_pct = strategy_max_deployment_pct(config)
    min_cash_buffer_pct = strategy_min_cash_buffer_pct(config)
    max_deployment_dkk = total_value * max_deployment_pct
    min_cash_buffer_dkk = total_value * min_cash_buffer_pct
    deployment_headroom_dkk = max(max_deployment_dkk - invested_value, 0.0)
    cash_after_buffer_dkk = max(cash_value - min_cash_buffer_dkk, 0.0)
    spendable_cash_dkk = min(cash_value, deployment_headroom_dkk, cash_after_buffer_dkk)
    return {
        "max_deployment_pct": max_deployment_pct,
        "min_cash_buffer_pct": min_cash_buffer_pct,
        "max_deployment_dkk": max_deployment_dkk,
        "min_cash_buffer_dkk": min_cash_buffer_dkk,
        "deployment_headroom_dkk": deployment_headroom_dkk,
        "cash_after_buffer_dkk": cash_after_buffer_dkk,
        "spendable_cash_dkk": max(spendable_cash_dkk, 0.0),
    }


def _safe_float(value: Any, default: float = 0.0) -> float:
    try:
        if value is None:
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


def _is_session_error(exc: Exception) -> bool:
    if not isinstance(exc, SaxoSessionError):
        return False
    text = str(exc).lower()
    return any(marker in text for marker in SESSION_ERROR_MARKERS)


def _market_status_by_code(config: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {str(row.get("code", "")).lower(): row for row in get_market_status(config)}


def _session_tag_for_symbol(symbol: str, market_status_by_code: dict[str, dict[str, Any]]) -> str | None:
    exchange_code = parse_exchange_code(symbol)
    if not exchange_code:
        return None
    row = market_status_by_code.get(exchange_code)
    if not row or not bool(row.get("analysis_window_active")):
        return None
    region = "us" if exchange_code in US_EXCHANGES else "eu"
    kind = str(row.get("analysis_window_kind") or "open")
    return f"{region}_{kind}"


def _xai_score_from_candidate(candidate: dict[str, Any]) -> float:
    raw = candidate.get("xai_score")
    if raw is None:
        raw = candidate.get("confidence")
    score = _safe_float(raw, 0.0)
    if 0.0 <= score <= 1.0:
        score *= 100.0
    return max(0.0, min(score, 100.0))


def _extract_candidate_pool(report_json: dict[str, Any]) -> list[dict[str, Any]]:
    output: list[dict[str, Any]] = []
    seen: set[str] = set()
    for item in report_json.get("candidate_assets", []) or []:
        symbol = str(item.get("symbol") or "").strip()
        if not symbol or symbol in seen:
            continue
        seen.add(symbol)
        output.append(
            {
                "symbol": symbol,
                "direction": str(item.get("direction") or "BUY").upper(),
                "xai_score": _xai_score_from_candidate(item),
                "sector": str(item.get("sector") or "").strip() or None,
                "thesis": str(item.get("thesis") or ""),
                "catalysts": list(item.get("catalysts") or []),
                "risks": list(item.get("risks") or []),
            }
        )
    for item in report_json.get("suggested_trades", []) or []:
        symbol = str(item.get("symbol") or "").strip()
        action = str(item.get("action") or "").upper()
        if not symbol or symbol in seen or action not in {"BUY", "SELL", "HOLD"}:
            continue
        seen.add(symbol)
        output.append(
            {
                "symbol": symbol,
                "direction": "BUY" if action == "HOLD" else action,
                "xai_score": _xai_score_from_candidate(item),
                "sector": None,
                "thesis": str(item.get("rationale") or ""),
                "catalysts": [],
                "risks": list(item.get("risk_notes") or []),
            }
        )
    for item in report_json.get("watchlist_focus", []) or []:
        symbol = str(item.get("symbol") or "").strip()
        if not symbol or symbol in seen:
            continue
        seen.add(symbol)
        output.append(
            {
                "symbol": symbol,
                "direction": "BUY",
                "xai_score": 55.0,
                "sector": None,
                "thesis": str(item.get("thesis") or ""),
                "catalysts": list(item.get("catalysts") or []),
                "risks": list(item.get("risks") or []),
            }
        )
    return output


def _bars_to_frame(payload: dict[str, Any]) -> tuple[pd.DataFrame, int]:
    rows = payload.get("Data", []) or []
    if not rows:
        return pd.DataFrame(), 2
    frame = pd.DataFrame(rows)
    if "Time" not in frame:
        return pd.DataFrame(), 2
    frame["Time"] = pd.to_datetime(frame["Time"], utc=True)
    for column in ("Open", "High", "Low", "Close", "Volume"):
        if column in frame:
            frame[column] = pd.to_numeric(frame[column], errors="coerce")
    frame = frame.dropna(subset=["Time", "Close"]).sort_values("Time").set_index("Time")
    decimals = int(((payload.get("DisplayAndFormat") or {}).get("Decimals")) or 2)
    return frame, decimals


def _resample_bars(frame: pd.DataFrame, rule: str) -> pd.DataFrame:
    if frame.empty:
        return frame
    mapping = {
        "Open": "first",
        "High": "max",
        "Low": "min",
        "Close": "last",
        "Volume": "sum",
    }
    columns = {key: value for key, value in mapping.items() if key in frame.columns}
    return frame.resample(rule, label="right", closed="right").agg(columns).dropna(subset=["Close"])


def _ema(series: pd.Series, span: int) -> pd.Series:
    return series.ewm(span=span, adjust=False).mean()


def _atr(frame: pd.DataFrame, length: int = 14) -> pd.Series:
    high = frame["High"]
    low = frame["Low"]
    close = frame["Close"]
    prev_close = close.shift(1)
    true_range = pd.concat(
        [
            (high - low).abs(),
            (high - prev_close).abs(),
            (low - prev_close).abs(),
        ],
        axis=1,
    ).max(axis=1)
    return true_range.rolling(length, min_periods=2).mean()


def _vwap(frame: pd.DataFrame) -> pd.Series:
    if "Volume" not in frame or frame["Volume"].fillna(0).sum() <= 0:
        return frame["Close"]
    typical = (frame["High"] + frame["Low"] + frame["Close"]) / 3.0
    cumulative_volume = frame["Volume"].cumsum()
    return (typical * frame["Volume"]).cumsum() / cumulative_volume.where(cumulative_volume != 0)


def _score_timeframe(frame: pd.DataFrame) -> float:
    if frame.empty or len(frame) < 30:
        return 25.0
    close = frame["Close"]
    ema9 = _ema(close, 9)
    ema21 = _ema(close, 21)
    ema50 = _ema(close, 50)
    ema200 = _ema(close, 200)
    latest_close = _safe_float(close.iloc[-1])
    latest_ema9 = _safe_float(ema9.iloc[-1])
    latest_ema21 = _safe_float(ema21.iloc[-1])
    latest_ema50 = _safe_float(ema50.iloc[-1])
    latest_ema200 = _safe_float(ema200.iloc[-1], latest_ema50)
    score = 0.0
    if latest_close > latest_ema9:
        score += 18.0
    if latest_ema9 > latest_ema21:
        score += 18.0
    if latest_ema21 > latest_ema50:
        score += 18.0
    if latest_ema50 > latest_ema200:
        score += 18.0
    if latest_close > latest_ema200:
        score += 12.0
    slope = latest_ema9 - _safe_float(ema9.iloc[max(len(ema9) - 5, 0)])
    if slope > 0:
        score += 16.0
    return min(score, 100.0)


def _score_volume(frame_1m: pd.DataFrame) -> tuple[float, float]:
    frame_15m = _resample_bars(frame_1m, "15min")
    if frame_15m.empty or "Volume" not in frame_15m or len(frame_15m) < 6:
        return 25.0, 1.0
    latest_volume = _safe_float(frame_15m["Volume"].iloc[-1], 0.0)
    baseline = _safe_float(frame_15m["Volume"].iloc[:-1].tail(20).mean(), 0.0)
    if baseline <= 0:
        return 25.0, 1.0
    rvol = latest_volume / baseline
    if rvol >= 1.8:
        score = 100.0
    elif rvol >= 1.5:
        score = 80.0
    elif rvol >= 1.2:
        score = 60.0
    elif rvol >= 1.0:
        score = 40.0
    else:
        score = 20.0
    return score, rvol


def _estimate_round_trip_cost_dkk(
    *,
    symbol: str,
    quantity: int,
    entry_price_local: float,
    currency: str,
    fx_rate: float,
    config: dict[str, Any],
) -> float:
    gross_local = float(quantity) * entry_price_local
    gross_dkk = gross_local * fx_rate
    commissions_cfg = config.get("commissions", {})
    rate = float(commissions_cfg.get("default_rate", 0.0) or 0.0)
    exchange_code = parse_exchange_code(symbol).upper()
    minimum = (commissions_cfg.get("minimums", {}) or {}).get(exchange_code, {})
    minimum_amount = float(minimum.get("amount", 0.0) or 0.0)
    commission_per_side_dkk = max(gross_dkk * rate, minimum_amount * fx_rate if minimum_amount else 0.0)
    fx_conversion_rate = float(commissions_cfg.get("fx_conversion_rate", 0.0) or 0.0)
    fx_cost_dkk = 0.0 if currency == "DKK" else gross_dkk * fx_conversion_rate
    return (commission_per_side_dkk + fx_cost_dkk) * 2.0


def _round_price(price: float, decimals: int) -> float:
    return round(float(price), max(int(decimals), 0))


def _round_order_price(symbol: str, price: float, config: dict[str, Any], *, side: str, decimals: int) -> float:
    normalized = normalize_order_price(symbol, price, config, side=side, fallback_decimals=decimals)
    return float(normalized if normalized is not None else _round_price(price, decimals))


def _evaluate_candidate(
    *,
    candidate: dict[str, Any],
    config: dict[str, Any],
    session: dict[str, Any],
    market_status_by_code: dict[str, dict[str, Any]],
) -> CandidateMetrics | None:
    symbol = candidate["symbol"]
    session_tag = _session_tag_for_symbol(symbol, market_status_by_code)
    if session_tag is None:
        return None
    instrument = lookup_instrument(symbol, config, session)
    payload = get_chart_samples(
        uic=instrument.uic,
        asset_type=instrument.asset_type,
        config=config,
        session=session,
        horizon_minutes=1,
        count=260,
    )
    frame_1m, decimals = _bars_to_frame(payload)
    if frame_1m.empty or len(frame_1m) < 60:
        return None
    frame_5m = _resample_bars(frame_1m, "5min")
    frame_15m = _resample_bars(frame_1m, "15min")
    current_price = _safe_float(frame_1m["Close"].iloc[-1])
    atr_1m = max(_safe_float(_atr(frame_1m).iloc[-1], 0.0), max(current_price * 0.0025, 0.01))
    spacing_cfg = config.get("strategy", {}).get("ladder", {})
    spacing_factor = float(spacing_cfg.get("atr_spacing_factor", 0.25) or 0.25)
    spacing_factor = min(
        float(spacing_cfg.get("atr_spacing_max", 0.4) or 0.4),
        max(float(spacing_cfg.get("atr_spacing_min", 0.15) or 0.15), spacing_factor),
    )
    rung_spacing = max(atr_1m * spacing_factor, 10 ** (-decimals))
    timeframe_scores = [
        _score_timeframe(frame_1m),
        _score_timeframe(frame_5m),
        _score_timeframe(frame_15m),
    ]
    technical_score = sum(timeframe_scores) / len(timeframe_scores)
    vwap_local = _safe_float(_vwap(frame_1m).iloc[-1], current_price)
    if current_price > vwap_local:
        technical_score = min(100.0, technical_score + 5.0)
    volume_score, rvol = _score_volume(frame_1m)
    notes = [
        f"ATR(1m) {atr_1m:.4f}",
        f"VWAP {vwap_local:.4f}",
        f"RVOL(15m) {rvol:.2f}",
    ]
    return CandidateMetrics(
        symbol=symbol,
        session_tag=session_tag,
        currency=str(instrument.currency_code or "DKK"),
        current_price_local=current_price,
        atr_1m=atr_1m,
        rung_spacing_local=rung_spacing,
        technical_score=technical_score,
        volume_score=volume_score,
        rvol_15m=rvol,
        vwap_local=vwap_local,
        decimals=decimals,
        notes=notes,
    )


def _current_position_map(context: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {str(row["symbol"]): row for row in context.get("portfolio_positions", [])}


def _distribute_quantity(total_quantity: int, rung_count: int) -> list[int]:
    if total_quantity <= 0 or rung_count <= 0:
        return []
    base = total_quantity // rung_count
    remainder = total_quantity % rung_count
    quantities = [base + (1 if index < remainder else 0) for index in range(rung_count)]
    return [value for value in quantities if value > 0]


def _build_entry_ladder_orders(
    *,
    symbol: str,
    session_tag: str,
    target_weight_pct: float,
    metrics: CandidateMetrics,
    config: dict[str, Any],
    portfolio_value_dkk: float,
    remaining_cash_dkk: float,
    remaining_capacity: int,
) -> tuple[list[dict[str, Any]], float, int, list[str]]:
    notes: list[str] = []
    if remaining_capacity <= 0 or remaining_cash_dkk <= 0:
        return [], remaining_cash_dkk, remaining_capacity, notes
    ladder_cfg = config.get("strategy", {}).get("ladder", {})
    configured_rung_count = max(1, int(ladder_cfg.get("rung_count", 5) or 5))
    fx_rate = fx_rate_to_dkk(metrics.currency, fetch_ecb_fx_rates())
    target_capital_dkk = min(target_weight_pct * max(float(portfolio_value_dkk or 0.0), remaining_cash_dkk), remaining_cash_dkk)
    total_quantity = int(target_capital_dkk / max(metrics.current_price_local * fx_rate, 1e-9))
    default_min_rung_value_dkk = max(float(config.get("execution", {}).get("min_trade_value_dkk", 500) or 500), 5000.0)
    min_rung_value_dkk = max(float(ladder_cfg.get("min_rung_value_dkk", default_min_rung_value_dkk) or default_min_rung_value_dkk), 1.0)
    affordable_rung_count = max(1, int(target_capital_dkk // min_rung_value_dkk))
    rung_count = max(1, min(configured_rung_count, affordable_rung_count, remaining_capacity))
    if rung_count < configured_rung_count:
        notes.append(
            f"{symbol}: reduced entry ladder from {configured_rung_count} to {rung_count} rung(s) "
            f"because target capital {target_capital_dkk:.2f} DKK is too small for {configured_rung_count} cost-efficient rungs."
        )
    rung_quantities = _distribute_quantity(total_quantity, rung_count)
    if not rung_quantities:
        notes.append(
            f"{symbol}: skipped entry ladder because target capital {target_capital_dkk:.2f} DKK "
            f"cannot buy one whole share at {metrics.current_price_local:.4f} {metrics.currency}."
        )
        return [], remaining_cash_dkk, remaining_capacity, notes
    stop_multiple = float(ladder_cfg.get("stop_loss_atr_multiple", 2.0) or 2.0)
    profit_multiple = float(ladder_cfg.get("take_profit_rung_multiple", 2.0) or 2.0)
    max_profit_atr_multiple = float(ladder_cfg.get("max_take_profit_atr_multiple", 8.0) or 8.0)
    min_profit_multiple = float(config.get("strategy", {}).get("cost_guard_multiple", 1.5) or 1.5)
    slippage_bps = float(config.get("strategy", {}).get("estimated_slippage_bps", 8.0) or 8.0)
    orders: list[dict[str, Any]] = []
    for rung_index, quantity in enumerate(rung_quantities):
        if remaining_capacity <= 0:
            break
        entry_price = _round_order_price(
            symbol,
            metrics.current_price_local - metrics.rung_spacing_local * float(rung_index + 1),
            config,
            side="buy_limit",
            decimals=metrics.decimals,
        )
        take_profit_price = _round_order_price(
            symbol,
            entry_price + metrics.rung_spacing_local * max(profit_multiple, float(rung_index + 1)),
            config,
            side="sell_limit",
            decimals=metrics.decimals,
        )
        stop_price = _round_order_price(
            symbol,
            max(entry_price - metrics.atr_1m * stop_multiple, entry_price - metrics.rung_spacing_local * 2.0, 10 ** (-metrics.decimals)),
            config,
            side="sell_stop",
            decimals=metrics.decimals,
        )
        price_tick = price_tick_size_for_symbol(symbol, config, fallback_decimals=metrics.decimals)
        stop_limit_price = _round_order_price(
            symbol,
            max(stop_price - price_tick, 10 ** (-metrics.decimals)),
            config,
            side="sell_stop_limit",
            decimals=metrics.decimals,
        )
        gross_dkk = float(quantity) * entry_price * fx_rate
        round_trip_cost_dkk = _estimate_round_trip_cost_dkk(
            symbol=symbol,
            quantity=quantity,
            entry_price_local=entry_price,
            currency=metrics.currency,
            fx_rate=fx_rate,
            config=config,
        )
        expected_profit_dkk = max(take_profit_price - entry_price, 0.0) * float(quantity) * fx_rate
        slippage_dkk = gross_dkk * (slippage_bps / 10_000.0)
        if expected_profit_dkk <= 0:
            notes.append(f"{symbol} rung {rung_index + 1}: skipped because take-profit was not above entry.")
            continue
        required_profit_dkk = (round_trip_cost_dkk * min_profit_multiple) + slippage_dkk
        if expected_profit_dkk <= required_profit_dkk:
            required_delta_local = required_profit_dkk / max(float(quantity) * fx_rate, 1e-9)
            max_delta_local = max(metrics.atr_1m * max_profit_atr_multiple, metrics.rung_spacing_local * profit_multiple)
            if required_delta_local <= max_delta_local:
                take_profit_price = _round_order_price(
                    symbol,
                    entry_price + required_delta_local + price_tick,
                    config,
                    side="sell_limit",
                    decimals=metrics.decimals,
                )
                expected_profit_dkk = max(take_profit_price - entry_price, 0.0) * float(quantity) * fx_rate
                notes.append(
                    f"{symbol} rung {rung_index + 1}: widened take-profit to cover estimated round-trip "
                    f"cost/slippage ({expected_profit_dkk:.2f} DKK expected vs {required_profit_dkk:.2f} DKK required)."
                )
            else:
                notes.append(
                    f"{symbol} rung {rung_index + 1}: skipped because a cost-efficient take-profit would need "
                    f"{required_delta_local:.4f} {metrics.currency}, above the configured {max_profit_atr_multiple:.1f} ATR cap."
                )
                continue
        total_cash_lock_dkk = gross_dkk + (round_trip_cost_dkk / 2.0)
        if total_cash_lock_dkk > remaining_cash_dkk + 1e-9:
            notes.append(
                f"{symbol} rung {rung_index + 1}: skipped because it needs {total_cash_lock_dkk:.2f} DKK "
                f"but only {remaining_cash_dkk:.2f} DKK remains after cash guardrails."
            )
            continue
        strategy_key = f"{session_tag}:{symbol}:entry:{rung_index}"
        orders.append(
            {
                "symbol": symbol,
                "action": "BUY",
                "order_type": "Limit",
                "limit_price_local": entry_price,
                "stop_price_local": None,
                "quantity": float(quantity),
                "requested_weight_pct": target_weight_pct,
                "estimated_value_dkk": gross_dkk,
                "currency": metrics.currency,
                "session_tag": session_tag,
                "strategy_type": "ladder",
                "strategy_role": "entry",
                "strategy_key": strategy_key,
                "related_orders": [
                    {
                        "action": "SELL",
                        "order_type": "Limit",
                        "limit_price": take_profit_price,
                        "quantity": quantity,
                        "duration_type": "GoodTillCancel",
                        "strategy_role": "take_profit",
                    },
                    {
                        "action": "SELL",
                        "order_type": "StopLimit",
                        "limit_price": stop_limit_price,
                        "stop_price": stop_price,
                        "quantity": quantity,
                        "duration_type": "GoodTillCancel",
                        "strategy_role": "stop_loss",
                    },
                ],
                "strategy_metadata": {
                    "atr_1m": metrics.atr_1m,
                    "rung_spacing_local": metrics.rung_spacing_local,
                    "entry_price_local": entry_price,
                    "take_profit_price_local": take_profit_price,
                    "stop_price_local": stop_price,
                    "decimals": metrics.decimals,
                    "stop_limit_price_local": stop_limit_price,
                    "price_tick_local": price_tick,
                    "trail_activation_price_local": _round_order_price(
                        symbol,
                        entry_price + metrics.rung_spacing_local,
                        config,
                        side="sell_limit",
                        decimals=metrics.decimals,
                    ),
                    "trail_stop_atr_multiple": float(ladder_cfg.get("trail_stop_atr_multiple", 1.25) or 1.25),
                },
            }
        )
        remaining_cash_dkk -= total_cash_lock_dkk
        remaining_capacity -= 1
    return orders, remaining_cash_dkk, remaining_capacity, notes


def build_strategy_plan(
    *,
    report_json: dict[str, Any],
    context: dict[str, Any],
    config: dict[str, Any],
) -> dict[str, Any]:
    if not strategy_enabled(config):
        return {"status": "disabled", "selected_assets": [], "ladder_orders": [], "notes": ["Strategy engine disabled."]}
    if swing_strategy_enabled(config):
        return build_swing_strategy_plan(report_json=report_json, context=context, config=config)
    market_status_by_code = _market_status_by_code(config)
    candidates = _extract_candidate_pool(report_json)
    if not candidates:
        return {"status": "no_candidates", "selected_assets": [], "ladder_orders": [], "notes": ["xAI returned no candidate symbols."]}

    try:
        session = ensure_access_token(config, config["saxo"].get("session_path"))
    except SaxoSessionError as exc:
        return {
            "status": "saxo_session_error",
            "selected_assets": [],
            "ladder_orders": [],
            "notes": [str(exc)],
        }

    scored: list[dict[str, Any]] = []
    for candidate in candidates[: int(_strategy_cfg(config).get("max_candidates", 20) or 20)]:
        try:
            metrics = _evaluate_candidate(
                candidate=candidate,
                config=config,
                session=session,
                market_status_by_code=market_status_by_code,
            )
        except SaxoSessionError as exc:
            if _is_session_error(exc):
                return {
                    "status": "saxo_session_error",
                    "selected_assets": [],
                    "ladder_orders": [],
                    "notes": [str(exc)],
                }
            return {
                "status": "strategy_data_error",
                "selected_assets": [],
                "ladder_orders": [],
                "notes": [str(exc)],
            }
        except Exception as exc:  # noqa: BLE001
            metrics = None
            candidate = {**candidate, "error": str(exc)}
        if metrics is None:
            continue
        xai_score = _xai_score_from_candidate(candidate)
        combined_score = round((xai_score * 0.30) + (metrics.technical_score * 0.40) + (metrics.volume_score * 0.30), 2)
        scored.append(
            {
                **candidate,
                "session_tag": metrics.session_tag,
                "technical_score": round(metrics.technical_score, 2),
                "volume_score": round(metrics.volume_score, 2),
                "combined_score": combined_score,
                "metrics": metrics,
            }
        )
    if not scored:
        return {"status": "no_scored_candidates", "selected_assets": [], "ladder_orders": [], "notes": ["No tradable candidates passed technical scoring."]}

    scored.sort(key=lambda item: item["combined_score"], reverse=True)
    max_selected = int(_strategy_cfg(config).get("max_selected_assets", 8) or 8)
    selected: list[dict[str, Any]] = []
    for item in scored:
        selected.append(item)
        if len(selected) >= max_selected:
            break

    position_map = _current_position_map(context)
    portfolio_summary = dict(context.get("portfolio_summary", {}) or {})
    portfolio_value_dkk = _safe_float(portfolio_summary.get("total_market_value_dkk"), 0.0)
    capital_limits = strategy_capital_limits(
        config=config,
        total_market_value_dkk=portfolio_value_dkk,
        invested_market_value_dkk=_safe_float(portfolio_summary.get("invested_market_value_dkk"), 0.0),
        cash_balance_dkk=_safe_float(portfolio_summary.get("cash_balance_dkk"), 0.0),
    )
    remaining_cash_dkk = capital_limits["spendable_cash_dkk"]
    remaining_capacity = int(config.get("execution", {}).get("max_daily_orders", 6) or 6)
    min_weight = 0.0
    max_weight = float(config.get("strategy", {}).get("ladder", {}).get("max_position_weight", 0.04) or 0.04)
    orders: list[dict[str, Any]] = []
    selected_rows: list[dict[str, Any]] = []
    top_score = max(float(item["combined_score"]) for item in selected) if selected else 100.0
    for item in selected:
        metrics: CandidateMetrics = item["metrics"]
        normalized = (float(item["combined_score"]) / top_score) if top_score > 0 else 0.5
        target_weight_pct = min(max_weight, max(min_weight, min_weight + (max_weight - min_weight) * normalized))
        selected_rows.append(
            {
                "symbol": item["symbol"],
                "direction": item["direction"],
                "session_tag": metrics.session_tag,
                "sector": item.get("sector"),
                "xai_score": item["xai_score"],
                "technical_score": item["technical_score"],
                "volume_score": item["volume_score"],
                "combined_score": item["combined_score"],
                "target_weight_pct": round(target_weight_pct * 100.0, 2),
                "current_price_local": metrics.current_price_local,
                "currency": metrics.currency,
                "atr_1m": metrics.atr_1m,
                "rung_spacing_local": metrics.rung_spacing_local,
                "rvol_15m": metrics.rvol_15m,
                "vwap_local": metrics.vwap_local,
                "notes": metrics.notes,
            }
        )
        current_position = position_map.get(item["symbol"])
        if str(item["direction"]).upper() != "BUY":
            continue
        if current_position is not None and _safe_float(current_position.get("allocation_pct"), 0.0) >= target_weight_pct * 100.0:
            continue
        built_orders, remaining_cash_dkk, remaining_capacity, order_notes = _build_entry_ladder_orders(
            symbol=item["symbol"],
            session_tag=metrics.session_tag,
            target_weight_pct=target_weight_pct,
            metrics=metrics,
            config=config,
            portfolio_value_dkk=portfolio_value_dkk,
            remaining_cash_dkk=remaining_cash_dkk,
            remaining_capacity=remaining_capacity,
        )
        metrics.notes.extend(order_notes)
        orders.extend(built_orders)

    status = "ok" if orders else "selected_without_orders"
    notes = [
        "Level-1 Saxo chart data drives EMA, ATR, VWAP, and RVOL scoring.",
        "Depth of Market is not yet integrated; ladder spacing currently uses ATR and minute bars only.",
        "Each ladder entry is submitted as a limit order with related take-profit and stop child orders.",
        (
            f"Capital guardrails reserve {capital_limits['min_cash_buffer_pct'] * 100:.0f}% cash and cap "
            f"deployment at {capital_limits['max_deployment_pct'] * 100:.0f}% of equity."
        ),
    ]
    if portfolio_value_dkk <= 0:
        notes.append("Portfolio value was non-positive; sizing fell back to available cash only.")
    if capital_limits["spendable_cash_dkk"] <= 0:
        notes.append("No new ladder cash was available after applying the deployment cap and next-session cash buffer.")
    if selected and not orders and capital_limits["spendable_cash_dkk"] > 0:
        notes.append("Selected BUY candidates produced no ladder orders after whole-share sizing and cost-guard checks.")
    return {
        "status": status,
        "selected_assets": selected_rows,
        "ladder_orders": orders,
        "candidate_count": len(candidates),
        "scored_count": len(scored),
        "capital_limits": capital_limits,
        "notes": notes,
    }
