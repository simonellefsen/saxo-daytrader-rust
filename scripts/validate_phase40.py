from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import (
    _create_or_fetch_orders,
    _record_related_orders_after_submission,
)


def main() -> int:
    config = load_config("config.yaml")
    connection = connect(":memory:")
    init_db(connection)
    connection.execute(
        """
        INSERT INTO import_batches (
            batch_id, imported_at, source_csv, source_position_count,
            imported_position_count, excluded_position_count, notes
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        ("batch-1", "2026-04-23T10:00:00+00:00", "", 0, 0, 0, "phase40"),
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
            "2026-04-23T10:00:00+00:00",
            "2026-04-23",
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

    import saxo_daytrader_xai.execution_engine as execution_engine

    original_market_status = execution_engine._market_status_for_symbol
    execution_engine._market_status_for_symbol = lambda symbol, cfg: {
        "code": "XNAS",
        "market": "Nasdaq US",
        "is_tradable": True,
        "is_open": True,
        "status_reason": "Open",
        "next_open": None,
    }
    try:
        report = {
            "id": 1,
            "report_json": {
                "strategy_plan": {
                    "ladder_orders": [
                        {
                            "symbol": "MU:xnas",
                            "action": "BUY",
                            "order_type": "Limit",
                            "limit_price_local": 100.0,
                            "stop_price_local": None,
                            "quantity": 5.0,
                            "requested_weight_pct": 0.03,
                            "estimated_value_dkk": 3500.0,
                            "currency": "USD",
                            "session_tag": "us_open",
                            "strategy_type": "ladder",
                            "strategy_role": "entry",
                            "strategy_key": "us_open:MU:xnas:entry:0",
                            "related_orders": [
                                {
                                    "action": "SELL",
                                    "order_type": "Limit",
                                    "limit_price": 102.0,
                                    "quantity": 5,
                                    "duration_type": "GoodTillCancel",
                                    "strategy_role": "take_profit",
                                },
                                {
                                    "action": "SELL",
                                    "order_type": "Stop",
                                    "stop_price": 97.0,
                                    "quantity": 5,
                                    "duration_type": "GoodTillCancel",
                                    "strategy_role": "stop_loss",
                                },
                            ],
                            "strategy_metadata": {
                                "atr_1m": 1.2,
                                "rung_spacing_local": 0.3,
                                "trail_activation_price_local": 100.3,
                                "trail_stop_atr_multiple": 1.25,
                                "decimals": 2,
                            },
                        }
                    ]
                },
                "suggested_trades": [],
            },
        }
        orders = _create_or_fetch_orders(connection, config, report)
    finally:
        execution_engine._market_status_for_symbol = original_market_status

    assert len(orders) == 1, orders
    parent_order = orders[0]
    assert parent_order["order_type"] == "Limit"
    assert abs(float(parent_order["limit_price_local"]) - 100.0) < 1e-9
    assert parent_order["strategy_role"] == "entry"

    child_ids = _record_related_orders_after_submission(
        connection,
        parent_order=parent_order,
        broker_payload={"AccountKey": "acc", "AssetType": "Stock", "Uic": 42315},
        broker_result={"OrderId": "parent-1", "Orders": [{"OrderId": "tp-1"}, {"OrderId": "sl-1"}]},
    )
    child_rows = connection.execute(
        """
        SELECT id, order_type, strategy_role, parent_execution_order_id, broker_order_id, execution_result_json
        FROM execution_orders
        WHERE parent_execution_order_id = ?
        ORDER BY id ASC
        """,
        (int(parent_order["id"]),),
    ).fetchall()
    assert len(child_ids) == 2, child_ids
    assert len(child_rows) == 2, child_rows
    assert child_rows[0]["order_type"] == "Limit"
    assert child_rows[1]["order_type"] == "Stop"
    assert child_rows[0]["strategy_role"] == "take_profit"
    assert child_rows[1]["strategy_role"] == "stop_loss"
    payload = json.loads(child_rows[1]["execution_result_json"])
    assert payload["payload"]["OrderType"] == "Stop"
    assert abs(float(payload["payload"]["OrderPrice"]) - 97.0) < 1e-9

    print("Phase 40 validation passed.")
    print(f"Parent strategy order id: {parent_order['id']}")
    print(f"Child order ids: {child_ids}")
    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
