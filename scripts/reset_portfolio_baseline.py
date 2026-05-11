from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_summary
from saxo_daytrader_xai.execution_engine import sync_saxo_sim_account_to_portfolio


def _reset_database(config: dict[str, Any]) -> None:
    database_path = str(config["portfolio"]["database_path"])
    if database_path.startswith(("postgresql://", "postgres://")):
        connection = connect(database_path)
        try:
            connection.execute("DROP SCHEMA IF EXISTS public CASCADE")
            connection.execute("CREATE SCHEMA public")
            connection.commit()
            init_db(connection)
            connection.commit()
        finally:
            connection.close()
        return

    path = Path(database_path)
    if path.exists():
        path.unlink()
    connection = connect(path)
    try:
        init_db(connection)
        connection.commit()
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="Reset the database, import a baseline portfolio CSV, and optionally mirror it to Saxo SIM.")
    parser.add_argument("--config", default="config.yaml", help="Path to config YAML.")
    parser.add_argument("--source-csv", required=True, help="Saxo positions CSV to import.")
    parser.add_argument("--initial-cash-dkk", type=float, required=True, help="Local cash balance after reset.")
    parser.add_argument("--sync-saxo-sim", action="store_true", help="Queue/submit SIM-only orders to mirror the imported portfolio.")
    args = parser.parse_args()

    config = load_config(args.config)
    source_csv = Path(args.source_csv).expanduser()
    if not source_csv.is_absolute():
        source_csv = Path.cwd() / source_csv
    config["portfolio"]["source_csv"] = str(source_csv.resolve())
    config["portfolio"]["initial_cash_dkk"] = float(args.initial_cash_dkk)

    _reset_database(config)
    result = sync_portfolio(config)

    connection = connect(config["portfolio"]["database_path"])
    try:
        init_db(connection)
        summary = fetch_portfolio_summary(
            connection,
            batch_id=result.batch_id,
            initial_cash_dkk=float(args.initial_cash_dkk),
            use_broker_positions=False,
        )
        sync_result = None
        if args.sync_saxo_sim:
            sync_result = sync_saxo_sim_account_to_portfolio(config=config, connection=connection)
        print(
            {
                "status": "ok",
                "batch_id": result.batch_id,
                "source_positions": result.source_positions,
                "imported_positions": result.imported_positions,
                "excluded_positions": result.excluded_positions,
                "cash_balance_dkk": summary["cash_balance_dkk"],
                "position_count": summary["position_count"],
                "saxo_sim_sync": sync_result,
            }
        )
    finally:
        connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
