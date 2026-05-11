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
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions, fetch_portfolio_summary


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase34_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 0.0

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    initial_positions = fetch_portfolio_positions(connection, initial_cash_dkk=0.0)
    target = next(row for row in initial_positions if row["symbol"] == "MSTR:xnas")
    base_prices = {row["symbol"]: float(row["current_price_local"]) for row in initial_positions}

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
        positions_first = fetch_portfolio_positions(connection, initial_cash_dkk=0.0)
        summary_first = fetch_portfolio_summary(connection, initial_cash_dkk=0.0)

        phase["step"] = 1
        second = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 6, 4, 15, tzinfo=UTC),
        )
        positions_second = fetch_portfolio_positions(connection, initial_cash_dkk=0.0)
        summary_second = fetch_portfolio_summary(connection, initial_cash_dkk=0.0)

        phase["step"] = 2
        third = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 7, 4, 5, tzinfo=UTC),
        )
        positions_third = fetch_portfolio_positions(connection, initial_cash_dkk=0.0)
    finally:
        price_monitor.fetch_live_prices = original_fetch_live_prices
        price_monitor.fetch_ecb_fx_rates = original_fetch_ecb_fx_rates

    target_first = next(row for row in positions_first if row["symbol"] == "MSTR:xnas")
    target_second = next(row for row in positions_second if row["symbol"] == "MSTR:xnas")
    target_third = next(row for row in positions_third if row["symbol"] == "MSTR:xnas")

    assert first["status"] == "ok", first
    assert first["baseline_session_date"] == "2026-04-06", first
    assert abs(float(target_first["daily_pnl_dkk"])) <= 1e-9, target_first

    assert second["status"] == "ok", second
    assert float(target_second["daily_pnl_dkk"]) > 0, target_second
    assert float(summary_second["total_daily_pnl_dkk"]) > float(summary_first["total_daily_pnl_dkk"]), (
        summary_first,
        summary_second,
    )

    assert third["status"] == "ok", third
    assert third["baseline_session_date"] == "2026-04-07", third
    assert abs(float(target_third["daily_pnl_dkk"])) <= 1e-9, target_third

    print("Phase 34 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"First baseline date: {first['baseline_session_date']}")
    print(f"MSTR daily pnl after move DKK: {float(target_second['daily_pnl_dkk']):.2f}")
    print(f"Reset baseline date: {third['baseline_session_date']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
