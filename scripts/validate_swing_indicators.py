from __future__ import annotations

import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pandas as pd

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.swing_indicators import evaluate_daily_swing_frame


def _synthetic_bullish_pullback_frame() -> pd.DataFrame:
    rows = []
    price = 100.0
    start = datetime(2025, 1, 1, tzinfo=UTC)
    for index in range(260):
        if index < 225:
            price += 0.35
        elif index < 252:
            price -= 0.75
        else:
            price += 1.5
        open_price = price - 0.8 if index >= 252 else price + 0.2
        close_price = price
        rows.append(
            {
                "Time": start + timedelta(days=index),
                "Open": open_price,
                "High": max(open_price, close_price) + 1.2,
                "Low": min(open_price, close_price) - 1.8,
                "Close": close_price,
                "Volume": 1_500_000 if index >= 252 else 1_000_000,
            }
        )
    return pd.DataFrame(rows).set_index("Time")


def main() -> int:
    result = evaluate_daily_swing_frame(_synthetic_bullish_pullback_frame())
    assert result["status"] == "ok", result
    assert result["trend_bias"] == "bullish", result
    assert result["sentiment"] in {"BUY", "OVERWEIGHT"}, result
    assert result["confluence_count"] >= 3, result
    assert result["reward_risk"] >= 2.0, result
    assert result["stop_loss"] < result["entry_price"] < result["take_profit"], result
    print("Swing indicator validation passed.")
    print(f"Sentiment: {result['sentiment']}")
    print(f"Confluences: {result['confluence_count']}")
    print(f"Reward/risk: {result['reward_risk']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
