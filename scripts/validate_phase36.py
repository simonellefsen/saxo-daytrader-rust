from __future__ import annotations

import sys
import uuid
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import price_monitor
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions, fetch_portfolio_value_history


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase36_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 25_000.0
    config["price_monitor"]["history_max_rows"] = 0
    config["price_monitor"]["history_retention_days"] = 0

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    initial_history = fetch_portfolio_value_history(connection, limit=20)
    assert len(initial_history) == 1, initial_history
    assert initial_history[0]["snapshot_type"] == "import", initial_history[0]

    positions = fetch_portfolio_positions(connection, initial_cash_dkk=config["portfolio"]["initial_cash_dkk"])
    base_prices = {row["symbol"]: float(row["current_price_local"]) for row in positions}
    phase = {"step": 0}
    original_fetch_live_prices = price_monitor.fetch_live_prices
    original_fetch_ecb_fx_rates = price_monitor.fetch_ecb_fx_rates

    def fake_fetch_live_prices(symbols, timeout_seconds=10, symbol_to_yahoo=None):
        rows = []
        for symbol in symbols:
            current = base_prices.get(symbol, 100.0)
            if symbol == "MSTR:xnas":
                if phase["step"] == 1:
                    current = current + 10.0
                elif phase["step"] == 2:
                    current = current + 15.0
            rows.append(
                {
                    "symbol": symbol,
                    "yahoo_symbol": symbol,
                    "current_price": current,
                    "previous_close": current - 1.0,
                    "change_pct": 0.01,
                    "source": "test",
                    "status": "ok",
                }
            )
        return rows

    def fake_fetch_ecb_fx_rates():
        return {
            "base": "EUR",
            "as_of": datetime.now(UTC).isoformat(timespec="seconds"),
            "rates": {"EUR": 1.0, "DKK": 7.4604, "USD": 7.0},
            "source": "test",
        }

    price_monitor.fetch_live_prices = fake_fetch_live_prices
    price_monitor.fetch_ecb_fx_rates = fake_fetch_ecb_fx_rates
    try:
        first = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 6, 4, 5, tzinfo=UTC),
        )
        phase["step"] = 1
        second = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 6, 4, 15, tzinfo=UTC),
        )
        phase["step"] = 2
        third = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 7, 4, 5, tzinfo=UTC),
        )
    finally:
        price_monitor.fetch_live_prices = original_fetch_live_prices
        price_monitor.fetch_ecb_fx_rates = original_fetch_ecb_fx_rates

    full_history = fetch_portfolio_value_history(connection, limit=20)
    day_one_history = fetch_portfolio_value_history(
        connection,
        start_at="2026-04-06T00:00:00+00:00",
        end_at="2026-04-06T23:59:59+00:00",
        limit=20,
    )

    assert first["status"] == "ok", first
    assert second["status"] == "ok", second
    assert third["status"] == "ok", third
    assert len(full_history) == 4, full_history
    assert len(day_one_history) == 3, day_one_history
    assert any(row["snapshot_type"] == "import" for row in full_history), full_history
    assert full_history[-1]["snapshot_type"] in {"import", "price_monitor"}, full_history[-1]
    price_monitor_rows = [row for row in full_history if row["snapshot_type"] == "price_monitor"]
    assert len(price_monitor_rows) == 3, price_monitor_rows
    assert float(price_monitor_rows[1]["total_market_value_dkk"]) > float(price_monitor_rows[0]["total_market_value_dkk"]), price_monitor_rows
    assert float(price_monitor_rows[-1]["total_daily_pnl_dkk"]) <= 1e-9, price_monitor_rows[-1]

    print("Phase 36 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Initial history rows: {len(initial_history)}")
    print(f"Full history rows: {len(full_history)}")
    print(f"Day-one rows: {len(day_one_history)}")
    print(f"Latest snapshot type: {full_history[-1]['snapshot_type']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
