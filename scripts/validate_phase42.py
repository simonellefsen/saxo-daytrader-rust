from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import _create_or_fetch_orders, _evaluate_virtual_buy_budget_gate
from saxo_daytrader_xai.strategy_engine import CandidateMetrics, build_strategy_plan


def _seed_report_db():
    connection = connect(":memory:")
    init_db(connection)
    connection.execute(
        """
        INSERT INTO import_batches (
            batch_id, imported_at, source_csv, source_position_count,
            imported_position_count, excluded_position_count, notes
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        ("batch-1", "2026-04-24T10:00:00+00:00", "", 0, 0, 0, "phase42"),
    )
    connection.execute(
        """
        INSERT INTO decision_reports (
            id, created_at, report_date, batch_id, model, status, analysis_window_active,
            response_id, prompt_text, request_json, response_json, report_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            1,
            "2026-04-24T10:00:00+00:00",
            "2026-04-24",
            "batch-1",
            "mock",
            "completed",
            1,
            None,
            "{}",
            "{}",
            "{}",
            "{}",
            None,
        ),
    )
    connection.commit()
    return connection


def main() -> int:
    config = load_config("config.yaml")
    # Phase 42 validates the legacy ladder/budget guardrail path. The app now
    # defaults to swing mode, so force ladder mode for this regression check.
    config["strategy"]["mode"] = "ladder"

    import saxo_daytrader_xai.execution_engine as execution_engine
    import saxo_daytrader_xai.strategy_engine as strategy_engine

    original_market_status = execution_engine._market_status_for_symbol
    original_access_token = strategy_engine.ensure_access_token
    original_evaluate_candidate = strategy_engine._evaluate_candidate

    execution_engine._market_status_for_symbol = lambda symbol, cfg: {
        "code": "XNYS",
        "market": "NYSE",
        "is_tradable": True,
        "is_open": True,
        "status_reason": "Open",
        "next_open": None,
    }
    strategy_engine.ensure_access_token = lambda *_args, **_kwargs: {"access_token": "mock"}
    strategy_engine._evaluate_candidate = lambda **_kwargs: CandidateMetrics(
        symbol="PG:xnys",
        session_tag="us_open",
        currency="USD",
        current_price_local=150.0,
        atr_1m=1.5,
        rung_spacing_local=0.5,
        technical_score=82.0,
        volume_score=78.0,
        rvol_15m=1.6,
        vwap_local=149.4,
        decimals=2,
        notes=["mock candidate"],
    )
    try:
        connection = _seed_report_db()
        report = {
            "id": 1,
            "report_json": {
                "strategy_plan": {
                    "status": "saxo_session_error",
                    "ladder_orders": [],
                },
                "suggested_trades": [
                    {
                        "symbol": "PG:xnys",
                        "action": "BUY",
                        "target_weight_pct": 3.0,
                        "confidence": 0.8,
                        "priority": "high",
                        "quantity_hint": "Buy 10 shares",
                        "rationale": "mock",
                        "risk_notes": [],
                    }
                ],
            },
        }
        orders = _create_or_fetch_orders(connection, config, report)
        assert orders == [], orders

        connection.execute(
            """
            INSERT INTO import_batches (
                batch_id, imported_at, source_csv, source_position_count,
                imported_position_count, excluded_position_count, notes
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("batch-2", "2026-04-24T10:05:00+00:00", "", 0, 0, 0, "phase42-gate"),
        )
        connection.commit()
        gate = _evaluate_virtual_buy_budget_gate(
            {
                "symbol": "PG:xnys",
                "currency": "USD",
                "price_local": 150.0,
                "quantity": 300.0,
            },
            config,
            connection,
        )
        assert not gate["allowed"], gate
        assert gate["min_cash_buffer_dkk"] > 0, gate
        assert gate["deployment_headroom_dkk"] > 0, gate

        plan = build_strategy_plan(
            report_json={
                "candidate_assets": [
                    {
                        "symbol": "PG:xnys",
                        "direction": "BUY",
                        "xai_score": 88,
                        "sector": "consumer staples",
                    }
                ],
                "suggested_trades": [],
            },
            context={
                "portfolio_positions": [],
                "portfolio_summary": {
                    "total_market_value_dkk": 250000.0,
                    "invested_market_value_dkk": 187500.0,
                    "cash_balance_dkk": 62500.0,
                },
            },
            config=config,
        )
        assert plan["capital_limits"]["min_cash_buffer_pct"] == 0.1, plan
        assert plan["capital_limits"]["spendable_cash_dkk"] > 0.0, plan
        assert plan["ladder_orders"], plan

        print("Phase 42 validation passed.")
        print(f"Fallback buy orders created: {len(orders)}")
        print(f"Virtual buy gate allowed: {gate['allowed']}")
        print(f"Spendable strategy cash DKK: {plan['capital_limits']['spendable_cash_dkk']:.2f}")
    finally:
        execution_engine._market_status_for_symbol = original_market_status
        strategy_engine.ensure_access_token = original_access_token
        strategy_engine._evaluate_candidate = original_evaluate_candidate
        connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
