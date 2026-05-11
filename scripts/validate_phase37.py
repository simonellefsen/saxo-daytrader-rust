from __future__ import annotations

import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import identifier_lookup
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import _record_buy_trade
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_latest_batch_id, fetch_portfolio_positions


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase37_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 500_000.0

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    batch_id = fetch_latest_batch_id(connection)
    assert batch_id, "Expected imported batch"

    original_saxo_identity = identifier_lookup._saxo_identity
    original_openfigi_identity = identifier_lookup._openfigi_identity

    def fake_saxo_identity(symbol: str, config: dict):
        if symbol == "SBUX:xnas":
            return identifier_lookup.InstrumentIdentity(
                symbol=symbol,
                instrument_name="Starbucks Corporation",
                isin="US8552441094",
                figi=None,
                source="saxo",
            )
        return None

    def fake_openfigi_identity(symbol: str, currency: str | None, config: dict):
        if symbol == "MDB:xnas":
            return identifier_lookup.InstrumentIdentity(
                symbol=symbol,
                instrument_name="MongoDB, Inc.",
                isin=None,
                figi="BBG00M1R0111",
                source="openfigi",
            )
        return None

    identifier_lookup._saxo_identity = fake_saxo_identity
    identifier_lookup._openfigi_identity = fake_openfigi_identity
    try:
        _record_buy_trade(
            connection,
            config,
            {
                "id": 1001,
                "symbol": "SBUX:xnas",
                "mode": "simulation",
                "quantity": 3.0,
                "price_local": 95.0,
                "currency": "USD",
                "request_json": "{}",
            },
            batch_id,
        )
        _record_buy_trade(
            connection,
            config,
            {
                "id": 1002,
                "symbol": "MDB:xnas",
                "mode": "simulation",
                "quantity": 2.0,
                "price_local": 380.0,
                "currency": "USD",
                "request_json": "{}",
            },
            batch_id,
        )
    finally:
        identifier_lookup._saxo_identity = original_saxo_identity
        identifier_lookup._openfigi_identity = original_openfigi_identity

    trade_rows = connection.execute(
        """
        SELECT symbol, isin, figi, instrument_name
        FROM trade_ledger
        WHERE symbol IN ('SBUX:xnas', 'MDB:xnas')
        ORDER BY id ASC
        """
    ).fetchall()
    lot_rows = connection.execute(
        """
        SELECT symbol, isin, figi, instrument_name
        FROM position_lots
        WHERE symbol IN ('SBUX:xnas', 'MDB:xnas')
        ORDER BY created_at ASC
        """
    ).fetchall()
    positions = {row["symbol"]: row for row in fetch_portfolio_positions(connection, batch_id=batch_id, initial_cash_dkk=config["portfolio"]["initial_cash_dkk"])}

    sbux_trade = next(dict(row) for row in trade_rows if row["symbol"] == "SBUX:xnas")
    mdb_trade = next(dict(row) for row in trade_rows if row["symbol"] == "MDB:xnas")
    sbux_lot = next(dict(row) for row in lot_rows if row["symbol"] == "SBUX:xnas")
    mdb_lot = next(dict(row) for row in lot_rows if row["symbol"] == "MDB:xnas")

    assert sbux_trade["isin"] == "US8552441094", sbux_trade
    assert sbux_trade["instrument_name"] == "Starbucks Corporation", sbux_trade
    assert sbux_lot["isin"] == "US8552441094", sbux_lot
    assert mdb_trade["isin"] in (None, ""), mdb_trade
    assert mdb_trade["figi"] == "BBG00M1R0111", mdb_trade
    assert mdb_lot["instrument_name"] == "MongoDB, Inc.", mdb_lot
    assert positions["SBUX:xnas"]["isin"] == "US8552441094", positions["SBUX:xnas"]
    assert positions["SBUX:xnas"]["instrument_name"] == "Starbucks Corporation", positions["SBUX:xnas"]
    assert positions["MDB:xnas"]["instrument_name"] == "MongoDB, Inc.", positions["MDB:xnas"]

    print("Phase 37 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Saxo-enriched ISIN: {sbux_trade['isin']}")
    print(f"OpenFIGI fallback FIGI: {mdb_trade['figi']}")
    print(f"Overlay positions now include: {', '.join(sorted([key for key in positions if key in {'SBUX:xnas', 'MDB:xnas'}]))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
