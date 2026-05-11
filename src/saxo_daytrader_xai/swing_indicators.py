from __future__ import annotations

from typing import Any

import pandas as pd

from saxo_daytrader_xai.saxo_openapi import SaxoRateLimitError, SaxoSessionError, ensure_access_token, get_chart_samples, lookup_instrument


def _safe_float(value: Any, default: float = 0.0) -> float:
    try:
        if value is None or pd.isna(value):
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


def _bars_to_frame(payload: dict[str, Any]) -> pd.DataFrame:
    rows = payload.get("Data", []) or []
    if not rows:
        return pd.DataFrame()
    frame = pd.DataFrame(rows)
    if "Time" not in frame:
        return pd.DataFrame()
    frame["Time"] = pd.to_datetime(frame["Time"], utc=True)
    for column in ("Open", "High", "Low", "Close", "Volume"):
        if column in frame:
            frame[column] = pd.to_numeric(frame[column], errors="coerce")
    required = [column for column in ("Open", "High", "Low", "Close") if column in frame]
    return frame.dropna(subset=["Time", *required]).sort_values("Time").set_index("Time")


def _ema(series: pd.Series, span: int) -> pd.Series:
    return series.ewm(span=span, adjust=False).mean()


def _rsi(close: pd.Series, length: int = 14) -> pd.Series:
    delta = close.diff()
    gain = delta.clip(lower=0.0)
    loss = -delta.clip(upper=0.0)
    avg_gain = gain.ewm(alpha=1 / length, min_periods=length, adjust=False).mean()
    avg_loss = loss.ewm(alpha=1 / length, min_periods=length, adjust=False).mean()
    rs = avg_gain / avg_loss.replace(0.0, pd.NA)
    return 100.0 - (100.0 / (1.0 + rs))


def _macd(close: pd.Series) -> tuple[pd.Series, pd.Series, pd.Series]:
    line = _ema(close, 12) - _ema(close, 26)
    signal = _ema(line, 9)
    return line, signal, line - signal


def _bollinger(close: pd.Series, length: int = 20) -> tuple[pd.Series, pd.Series, pd.Series]:
    middle = close.rolling(length, min_periods=length).mean()
    std = close.rolling(length, min_periods=length).std()
    return middle - (2.0 * std), middle, middle + (2.0 * std)


def _stochastic(frame: pd.DataFrame, length: int = 14) -> tuple[pd.Series, pd.Series]:
    low_min = frame["Low"].rolling(length, min_periods=length).min()
    high_max = frame["High"].rolling(length, min_periods=length).max()
    k = ((frame["Close"] - low_min) / (high_max - low_min).replace(0.0, pd.NA)) * 100.0
    d = k.rolling(3, min_periods=3).mean()
    return k, d


def _obv(frame: pd.DataFrame) -> pd.Series:
    if "Volume" not in frame:
        return pd.Series([0.0] * len(frame), index=frame.index)
    direction = frame["Close"].diff().fillna(0.0).apply(lambda value: 1.0 if value > 0 else -1.0 if value < 0 else 0.0)
    return (direction * frame["Volume"].fillna(0.0)).cumsum()


def _atr(frame: pd.DataFrame, length: int = 14) -> pd.Series:
    high = frame["High"]
    low = frame["Low"]
    prev_close = frame["Close"].shift(1)
    true_range = pd.concat(
        [
            (high - low).abs(),
            (high - prev_close).abs(),
            (low - prev_close).abs(),
        ],
        axis=1,
    ).max(axis=1)
    return true_range.rolling(length, min_periods=length).mean()


def _bullish_candle(frame: pd.DataFrame) -> bool:
    if len(frame) < 2:
        return False
    latest = frame.iloc[-1]
    previous = frame.iloc[-2]
    body = abs(_safe_float(latest["Close"]) - _safe_float(latest["Open"]))
    candle_range = max(_safe_float(latest["High"]) - _safe_float(latest["Low"]), 1e-9)
    lower_wick = min(_safe_float(latest["Open"]), _safe_float(latest["Close"])) - _safe_float(latest["Low"])
    engulfing = (
        _safe_float(latest["Close"]) > _safe_float(latest["Open"])
        and _safe_float(previous["Close"]) < _safe_float(previous["Open"])
        and _safe_float(latest["Close"]) >= _safe_float(previous["Open"])
        and _safe_float(latest["Open"]) <= _safe_float(previous["Close"])
    )
    hammer = _safe_float(latest["Close"]) > _safe_float(latest["Open"]) and lower_wick >= body * 1.5 and body / candle_range <= 0.5
    return bool(engulfing or hammer)


def evaluate_daily_swing_frame(
    frame: pd.DataFrame,
    *,
    min_confluences: int = 3,
    min_reward_risk: float = 2.0,
) -> dict[str, Any]:
    if frame.empty or len(frame) < 60:
        return {
            "status": "insufficient_data",
            "sentiment": "HOLD",
            "technical_score": 0.0,
            "confluence_count": 0,
            "confluences": [],
            "notes": ["At least 60 daily bars are required for swing indicator scoring."],
        }

    close = frame["Close"]
    ema50 = _ema(close, 50)
    ema200 = _ema(close, 200)
    macd_line, macd_signal, macd_hist = _macd(close)
    rsi = _rsi(close)
    lower_band, middle_band, upper_band = _bollinger(close)
    stoch_k, stoch_d = _stochastic(frame)
    obv = _obv(frame)
    atr = _atr(frame)

    latest_close = _safe_float(close.iloc[-1])
    latest_open = _safe_float(frame["Open"].iloc[-1])
    latest_ema50 = _safe_float(ema50.iloc[-1])
    latest_ema200 = _safe_float(ema200.iloc[-1], latest_ema50)
    ema50_rising = latest_ema50 > _safe_float(ema50.iloc[max(len(ema50) - 6, 0)], latest_ema50)
    ema200_rising = latest_ema200 >= _safe_float(ema200.iloc[max(len(ema200) - 11, 0)], latest_ema200)
    latest_macd = _safe_float(macd_line.iloc[-1])
    latest_signal = _safe_float(macd_signal.iloc[-1])
    latest_hist = _safe_float(macd_hist.iloc[-1])
    previous_hist = _safe_float(macd_hist.iloc[-2], latest_hist)

    bullish_trend = (
        latest_close > latest_ema50
        and latest_close > latest_ema200
        and ema50_rising
        and ema200_rising
        and latest_macd >= latest_signal
    )
    bearish_trend = (
        latest_close < latest_ema50
        and latest_close < latest_ema200
        and not ema50_rising
        and latest_macd < latest_signal
    )

    confluences: list[str] = []
    latest_rsi = _safe_float(rsi.iloc[-1], 50.0)
    previous_rsi = _safe_float(rsi.iloc[-2], latest_rsi)
    if 30.0 <= latest_rsi <= 55.0 and latest_rsi >= previous_rsi:
        confluences.append("RSI(14) pullback is turning up from the 30-55 swing zone.")
    if latest_macd >= latest_signal and latest_hist > previous_hist:
        confluences.append("MACD is bullish or improving versus the signal line.")
    latest_lower = _safe_float(lower_band.iloc[-1], latest_close)
    latest_middle = _safe_float(middle_band.iloc[-1], latest_close)
    latest_upper = _safe_float(upper_band.iloc[-1], latest_close)
    previous_close = _safe_float(close.iloc[-2], latest_close)
    if previous_close <= latest_lower * 1.02 or latest_close >= latest_middle:
        confluences.append("Bollinger structure supports a pullback/reclaim setup.")
    latest_k = _safe_float(stoch_k.iloc[-1], 50.0)
    latest_d = _safe_float(stoch_d.iloc[-1], 50.0)
    previous_k = _safe_float(stoch_k.iloc[-2], latest_k)
    previous_d = _safe_float(stoch_d.iloc[-2], latest_d)
    if latest_k >= latest_d and previous_k <= previous_d and latest_k <= 55.0:
        confluences.append("Stochastic crossed up from a constructive zone.")
    latest_volume = _safe_float(frame.get("Volume", pd.Series(dtype=float)).iloc[-1] if "Volume" in frame else None)
    avg_volume = _safe_float(frame["Volume"].tail(21).iloc[:-1].mean() if "Volume" in frame and len(frame) > 21 else None)
    if avg_volume > 0 and latest_volume >= avg_volume * 1.15 and _safe_float(obv.iloc[-1]) >= _safe_float(obv.iloc[-6], _safe_float(obv.iloc[-1])):
        confluences.append("Volume/OBV confirms improving demand.")
    if _bullish_candle(frame):
        confluences.append("Latest daily candle confirms bullish reversal price action.")

    confluence_count = len(confluences)
    latest_atr = max(_safe_float(atr.iloc[-1], latest_close * 0.03), latest_close * 0.005)
    stop_loss = max(latest_close - latest_atr * 1.8, 0.01)
    risk_per_share = max(latest_close - stop_loss, 1e-9)
    take_profit = latest_close + risk_per_share * max(min_reward_risk, 2.0)
    reward_risk = (take_profit - latest_close) / risk_per_share

    if bullish_trend and confluence_count >= min_confluences and reward_risk >= min_reward_risk:
        sentiment = "BUY" if confluence_count >= min_confluences + 1 else "OVERWEIGHT"
    elif bearish_trend:
        sentiment = "SELL"
    elif latest_close < latest_ema50 or latest_close >= latest_upper:
        sentiment = "UNDERWEIGHT"
    else:
        sentiment = "HOLD"

    trend_score = 35.0 if bullish_trend else 0.0 if bearish_trend else 15.0
    confluence_score = min(confluence_count, 5) * 12.0
    momentum_score = max(0.0, min(20.0, (latest_rsi - 30.0) / 40.0 * 20.0))
    technical_score = max(0.0, min(100.0, trend_score + confluence_score + momentum_score))

    return {
        "status": "ok",
        "sentiment": sentiment,
        "technical_score": round(technical_score, 2),
        "trend_bias": "bullish" if bullish_trend else "bearish" if bearish_trend else "neutral",
        "confluence_count": confluence_count,
        "min_confluences": min_confluences,
        "confluences": confluences,
        "entry_price": round(latest_close, 4),
        "stop_loss": round(stop_loss, 4),
        "take_profit": round(take_profit, 4),
        "reward_risk": round(reward_risk, 2),
        "rsi14": round(latest_rsi, 2),
        "macd_histogram": round(latest_hist, 4),
        "ema50": round(latest_ema50, 4),
        "ema200": round(latest_ema200, 4),
        "atr14": round(latest_atr, 4),
        "latest_open": round(latest_open, 4),
        "latest_close": round(latest_close, 4),
        "bollinger_upper": round(latest_upper, 4),
        "bollinger_middle": round(latest_middle, 4),
        "bollinger_lower": round(latest_lower, 4),
    }


def evaluate_daily_swing_payload(
    payload: dict[str, Any],
    *,
    min_confluences: int = 3,
    min_reward_risk: float = 2.0,
) -> dict[str, Any]:
    return evaluate_daily_swing_frame(
        _bars_to_frame(payload),
        min_confluences=min_confluences,
        min_reward_risk=min_reward_risk,
    )


def fetch_daily_swing_indicators(symbols: list[str], config: dict[str, Any]) -> dict[str, dict[str, Any]]:
    cfg = config.get("strategy", {}).get("swing", {}).get("daily_indicators", {})
    if not bool(cfg.get("enabled", True)) or not symbols:
        return {}
    max_symbols = int(cfg.get("max_symbols", 20) or 20)
    horizon_minutes = int(cfg.get("horizon_minutes", 1440) or 1440)
    sample_count = int(cfg.get("sample_count", 260) or 260)
    min_confluences = int(cfg.get("min_confluences", 3) or 3)
    min_reward_risk = float(cfg.get("min_reward_risk", 2.0) or 2.0)
    try:
        session = ensure_access_token(config, config.get("saxo", {}).get("session_path"))
    except (KeyError, SaxoSessionError) as exc:
        return {
            symbol: {
                "status": "saxo_session_error",
                "sentiment": "HOLD",
                "technical_score": 0.0,
                "confluence_count": 0,
                "confluences": [],
                "notes": [str(exc)],
            }
            for symbol in symbols[:max_symbols]
        }

    output: dict[str, dict[str, Any]] = {}
    for symbol in symbols[:max_symbols]:
        try:
            instrument = lookup_instrument(symbol, config, session)
            payload = get_chart_samples(
                uic=instrument.uic,
                asset_type=instrument.asset_type,
                config=config,
                session=session,
                horizon_minutes=horizon_minutes,
                count=sample_count,
            )
            output[symbol] = {
                **evaluate_daily_swing_payload(
                    payload,
                    min_confluences=min_confluences,
                    min_reward_risk=min_reward_risk,
                ),
                "uic": instrument.uic,
                "asset_type": instrument.asset_type,
                "currency": instrument.currency_code,
            }
        except SaxoRateLimitError:
            raise
        except Exception as exc:  # noqa: BLE001
            output[symbol] = {
                "status": "error",
                "sentiment": "HOLD",
                "technical_score": 0.0,
                "confluence_count": 0,
                "confluences": [],
                "notes": [str(exc)],
            }
    return output
