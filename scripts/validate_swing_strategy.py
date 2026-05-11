from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

import saxo_daytrader_xai.swing_strategy as swing_strategy
from saxo_daytrader_xai.config import load_config


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    original_fetch_fx = swing_strategy.fetch_ecb_fx_rates
    original_fx_rate = swing_strategy.fx_rate_to_dkk
    original_indicators = swing_strategy.fetch_daily_swing_indicators
    swing_strategy.fetch_ecb_fx_rates = lambda: {}
    swing_strategy.fx_rate_to_dkk = lambda _currency, _rates: 1.0
    swing_strategy.fetch_daily_swing_indicators = lambda symbols, _config: {
        symbol: {
            "status": "ok",
            "sentiment": "BUY",
            "technical_score": 82.0,
            "trend_bias": "bullish",
            "confluence_count": 3,
            "min_confluences": 3,
            "confluences": ["mock confluence"],
            "reward_risk": 2.0,
        }
        for symbol in symbols
    }
    try:
        context = {
            "portfolio_summary": {
                "total_market_value_dkk": 250000.0,
                "cash_balance_dkk": 125000.0,
            },
            "portfolio_positions": [
                {
                    "symbol": "AAA:xnas",
                    "instrument_name": "AAA",
                    "quantity": 125.0,
                    "allocation_pct": 5.0,
                    "current_price_local": 100.0,
                    "currency": "DKK",
                },
                {
                    "symbol": "TSLA:xnas",
                    "instrument_name": "Tesla",
                    "quantity": 1.0,
                    "allocation_pct": 1.0,
                    "current_price_local": 250.0,
                    "currency": "DKK",
                },
            ],
            "watchlists": {
                "categories": [
                    {
                        "key": "us",
                        "items": [
                            {"symbol": "AAA:xnas", "current_price": 100.0, "currency": "DKK", "region": "US"},
                            {"symbol": "BBB:xnas", "current_price": 125.0, "currency": "DKK", "region": "US"},
                            {"symbol": "CCC:xnas", "current_price": 90.0, "currency": "DKK", "region": "US"},
                        ],
                    }
                ],
                "global": [],
                "nordic": [],
                "uk": [],
                "us": [],
                "eu": [],
            },
            "market_news": {"market_news": [{"title": "mock catalyst"}]},
            "analysis_pulse": {"kind": "manual", "key": "manual"},
        }
        report_json = {
            "market_regime": {"bias": "bullish"},
            "symbol_sentiment": [
                {
                    "symbol": "BBB:xnas",
                    "sentiment": "BUY",
                    "confidence": 86.0,
                    "rationale": "Liquid catalyst name with strong macro fit.",
                    "catalysts": ["mock catalyst"],
                    "risk_notes": ["mock risk"],
                },
                {
                    "symbol": "TSLA:xnas",
                    "sentiment": "BUY",
                    "confidence": 99.0,
                    "rationale": "Must be blocked by hard blacklist.",
                    "catalysts": [],
                    "risk_notes": [],
                },
                {
                    "symbol": "OUTSIDE:xnas",
                    "sentiment": "BUY",
                    "confidence": 95.0,
                    "rationale": "Must be blocked because it is not in the Watchlist.",
                    "catalysts": [],
                    "risk_notes": [],
                },
            ],
            "suggested_trades": [],
        }
        plan = swing_strategy.build_swing_strategy_plan(
            report_json=report_json,
            context=context,
            config=config,
        )
        order_symbols = {order["symbol"] for order in plan["swing_orders"]}
        assert "BBB:xnas" in order_symbols, plan
        assert "TSLA:xnas" not in order_symbols, plan
        assert "OUTSIDE:xnas" not in order_symbols, plan
        assert all(order["strategy_type"] == "swing" for order in plan["swing_orders"]), plan
        assert all(order["order_type"] == "Limit" for order in plan["swing_orders"]), plan
        assert all(order.get("limit_price_local") for order in plan["swing_orders"]), plan
        assert all(0.05 <= float(target["target_weight_pct"]) / 100.0 <= 0.25 for target in plan["position_targets"]), plan
        assert plan["constraints"]["cash_buffer_pct"] == 10.0, plan
        print("Swing strategy validation passed.")
        print(f"Swing orders: {len(plan['swing_orders'])}")
        print(f"Suggested trades: {len(plan['suggested_trades'])}")
        print(f"Status: {plan['status']}")
    finally:
        swing_strategy.fetch_ecb_fx_rates = original_fetch_fx
        swing_strategy.fx_rate_to_dkk = original_fx_rate
        swing_strategy.fetch_daily_swing_indicators = original_indicators
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
