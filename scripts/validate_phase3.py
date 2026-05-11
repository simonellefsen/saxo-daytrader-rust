from __future__ import annotations

import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import (
    fetch_open_lot_summary,
    fetch_portfolio_positions,
    fetch_portfolio_summary,
    fetch_realised_tax_summary,
)
from saxo_daytrader_xai.tax_engine import calculate_sell_outcome, update_ledger


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase3_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    summary_before = fetch_portfolio_summary(connection, batch_id=result.batch_id)
    positions = sorted(fetch_portfolio_positions(connection, batch_id=result.batch_id), key=lambda row: row["symbol"])

    assert len(positions) == 18, f"Expected 18 positions, got {len(positions)}"

    trade_results = []
    for position in positions:
        qty_to_sell = round(float(position["quantity"]) * 0.10, 6)
        trade = calculate_sell_outcome(
            position["symbol"],
            qty_to_sell,
            float(position["current_price_local"]),
            config=config,
            connection=connection,
            batch_id=result.batch_id,
            tax_year=2026,
        )
        trade["mode"] = "simulation"
        trade["status"] = "simulated"
        trade["notes"] = "Phase 3 validation sell of 10% of imported position"
        ledger_result = update_ledger(trade, config=config, connection=connection)
        trade_results.append((trade, ledger_result))

    tax_summary = fetch_realised_tax_summary(connection, tax_year=2026)
    open_lots = fetch_open_lot_summary(connection)
    summary_after = fetch_portfolio_summary(connection, batch_id=result.batch_id)

    assert tax_summary["trade_count"] == 18, f"Expected 18 simulated trades, got {tax_summary['trade_count']}"
    assert all(item[1]["status"] == "recorded" for item in trade_results)
    assert all(item[1]["lot_realizations_count"] == 1 for item in trade_results)

    sold_qty_by_symbol = {trade["symbol"]: trade["qty_to_sell"] for trade, _ in trade_results}
    open_qty_by_symbol = {row["symbol"]: row["quantity_open"] for row in open_lots}
    for position in positions:
        expected_open = float(position["quantity"]) - sold_qty_by_symbol[position["symbol"]]
        assert abs(open_qty_by_symbol[position["symbol"]] - expected_open) < 1e-6

    print("Phase 3 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Active positions in DB: {summary_before['position_count']}")
    print(f"Simulated sells: {tax_summary['trade_count']}")
    print(f"Realised gains DKK: {tax_summary['realised_gain_dkk']:.2f}")
    print(f"Tax impact DKK: {tax_summary['tax_dkk']:.2f}")
    print(f"Commission DKK: {tax_summary['commission_dkk']:.2f}")
    print(f"Reference portfolio market value DKK: {summary_after['total_market_value_dkk']:.2f}")
    print("Per-position 10% sell simulation:")
    for trade, _ in trade_results:
        print(
            f"- {trade['symbol']}: qty={trade['qty_to_sell']:.6f} gross={trade['gross_DKK']:.2f} "
            f"commission={trade['commission_DKK']:.2f} realised_gain={trade['realised_gain_DKK']:.2f} "
            f"tax={trade['tax_DKK']:.2f} net={trade['net_DKK']:.2f}"
        )

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
