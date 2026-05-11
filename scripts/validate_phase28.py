from __future__ import annotations

import json
import sys
import uuid
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import fetch_invalid_simulation_trades, repair_invalid_simulation_trades
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase28_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    result = sync_portfolio(config)

    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    now = datetime.now(UTC).isoformat(timespec="seconds")
    ledger_cursor = connection.execute(
        """
        INSERT INTO trade_ledger (
            created_at, symbol, isin, side, quantity, price_local, currency,
            gross_amount_dkk, commission_dkk, tax_dkk, net_amount_dkk, mode, status,
            notes, portfolio_before_json, portfolio_after_json, decision_context_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            now,
            "MSTR:xnas",
            None,
            "SELL",
            100.0,
            126.98,
            "USD",
            82000.0,
            200.0,
            0.0,
            81800.0,
            "simulation",
            "executed",
            "Synthetic invalid simulation trade",
            json.dumps({}, ensure_ascii=False, sort_keys=True),
            json.dumps({}, ensure_ascii=False, sort_keys=True),
            json.dumps({}, ensure_ascii=False, sort_keys=True),
        ),
    )
    ledger_id = int(ledger_cursor.lastrowid)
    order_cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, ledger_id, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            now,
            None,
            "MSTR:xnas",
            "SELL",
            "simulation",
            "executed",
            "saxo",
            0.03,
            100.0,
            126.98,
            "USD",
            82000.0,
            0,
            now,
            ledger_id,
            json.dumps({"seed": "phase28"}, ensure_ascii=False, sort_keys=True),
            json.dumps({"seed": "phase28"}, ensure_ascii=False, sort_keys=True),
            None,
        ),
    )
    order_id = int(order_cursor.lastrowid)
    connection.commit()

    invalid_before = fetch_invalid_simulation_trades(connection, limit=10)
    positions_before = fetch_portfolio_positions(connection)
    repair_result = repair_invalid_simulation_trades(config=config, connection=connection)
    invalid_after = fetch_invalid_simulation_trades(connection, limit=10)
    positions_after = fetch_portfolio_positions(connection)
    repaired_ledger = connection.execute("SELECT status, notes FROM trade_ledger WHERE id = ?", (ledger_id,)).fetchone()
    repaired_order = connection.execute("SELECT status, error_text FROM execution_orders WHERE id = ?", (order_id,)).fetchone()

    assert len(invalid_before) == 1, invalid_before
    assert invalid_before[0]["id"] == ledger_id, invalid_before
    assert repair_result["invalid_found"] == 1, repair_result
    assert repair_result["ledger_rows_repaired"] == [ledger_id], repair_result
    assert repair_result["execution_orders_repaired"] == [order_id], repair_result
    assert invalid_after == [], invalid_after
    assert repaired_ledger["status"] == "ignored_invalid_simulation", repaired_ledger
    assert "quarantined invalid simulation trade" in str(repaired_ledger["notes"]), repaired_ledger
    assert repaired_order["status"] == "invalid_repaired", repaired_order
    assert len(positions_before) == len(positions_after), (positions_before, positions_after)

    print("Phase 28 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Invalid rows detected: {len(invalid_before)}")
    print(f"Ledger rows repaired: {len(repair_result['ledger_rows_repaired'])}")
    print(f"Execution orders repaired: {len(repair_result['execution_orders_repaired'])}")
    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
