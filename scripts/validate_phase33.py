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
from saxo_daytrader_xai.execution_engine import execute_order
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions, fetch_portfolio_summary


def _insert_order(connection, *, symbol: str, quantity: float, price_local: float, currency: str) -> int:
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, ledger_id, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            None,
            symbol,
            "BUY",
            "simulation",
            "pending_execution",
            "saxo",
            0.0,
            quantity,
            price_local,
            currency,
            0.0,
            0,
            None,
            None,
            json.dumps({"source": "validate_phase33"}, ensure_ascii=False, sort_keys=True),
            None,
            None,
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase33_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 10_000.0
    config["execution"]["mode"] = "simulation"

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    summary_before = fetch_portfolio_summary(connection, initial_cash_dkk=config["portfolio"]["initial_cash_dkk"])
    positions = fetch_portfolio_positions(connection, initial_cash_dkk=config["portfolio"]["initial_cash_dkk"])
    buy_position = min(
        positions,
        key=lambda row: float(row["current_price_local"] or row["open_price_local"] or 0.0),
    )

    affordable_order_id = _insert_order(
        connection,
        symbol=buy_position["symbol"],
        quantity=1.0,
        price_local=float(buy_position["current_price_local"]),
        currency=str(buy_position["currency"]),
    )
    first_result = execute_order(affordable_order_id, config=config, connection=connection)
    summary_after_buy = fetch_portfolio_summary(connection, initial_cash_dkk=config["portfolio"]["initial_cash_dkk"])

    expensive_order_id = _insert_order(
        connection,
        symbol=buy_position["symbol"],
        quantity=10_000.0,
        price_local=float(buy_position["current_price_local"]),
        currency=str(buy_position["currency"]),
    )
    second_result = execute_order(expensive_order_id, config=config, connection=connection)
    failed_row = connection.execute(
        "SELECT status, error_text FROM execution_orders WHERE id = ?",
        (expensive_order_id,),
    ).fetchone()

    assert first_result["status"] == "executed", first_result
    assert summary_before["cash_balance_dkk"] == 10_000.0, summary_before
    assert summary_after_buy["cash_balance_dkk"] < summary_before["cash_balance_dkk"], (
        summary_before,
        summary_after_buy,
    )
    assert second_result["status"] == "execution_failed", second_result
    assert "Insufficient cash" in str(failed_row["error_text"]), dict(failed_row)

    print("Phase 33 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Starting cash DKK: {summary_before['cash_balance_dkk']:.2f}")
    print(f"Cash after buy DKK: {summary_after_buy['cash_balance_dkk']:.2f}")
    print(f"Insufficient-cash status: {second_result['status']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
