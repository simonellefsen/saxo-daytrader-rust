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


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase39_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 0.0
    config["price_monitor"]["post_close_grace_minutes"] = 15

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    original_fetch_live_prices = price_monitor.fetch_live_prices
    original_fetch_ecb_fx_rates = price_monitor.fetch_ecb_fx_rates
    calls = {"quotes": 0}

    def fake_fetch_live_prices(symbols, timeout_seconds=10, symbol_to_yahoo=None):
        calls["quotes"] += 1
        return [
            {
                "symbol": symbol,
                "yahoo_symbol": symbol,
                "current_price": 100.0,
                "previous_close": 99.0,
                "change_pct": 0.01,
                "source": "test",
                "status": "ok",
            }
            for symbol in symbols
        ]

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
        open_window = price_monitor.price_monitor_window_status(
            config,
            reference_time=datetime(2026, 4, 6, 19, 30, tzinfo=UTC),
        )
        grace_window = price_monitor.price_monitor_window_status(
            config,
            reference_time=datetime(2026, 4, 6, 20, 10, tzinfo=UTC),
        )
        closed_window = price_monitor.price_monitor_window_status(
            config,
            reference_time=datetime(2026, 4, 6, 20, 20, tzinfo=UTC),
        )
        refresh_during_grace = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 6, 20, 10, tzinfo=UTC),
        )
        calls_after_grace = calls["quotes"]
        refresh_after_close = price_monitor.refresh_portfolio_price_state(
            config=config,
            connection=connection,
            reference_time=datetime(2026, 4, 6, 20, 20, tzinfo=UTC),
        )
    finally:
        price_monitor.fetch_live_prices = original_fetch_live_prices
        price_monitor.fetch_ecb_fx_rates = original_fetch_ecb_fx_rates

    assert open_window["polling_active"] is True, open_window
    assert open_window["status"] == "open", open_window

    assert grace_window["polling_active"] is True, grace_window
    assert grace_window["status"] == "post_close_grace", grace_window
    assert grace_window["grace_markets"], grace_window

    assert closed_window["polling_active"] is False, closed_window
    assert closed_window["status"] == "closed", closed_window
    assert closed_window["next_resume_at"], closed_window

    assert refresh_during_grace["status"] == "ok", refresh_during_grace
    assert calls["quotes"] >= 1, calls

    assert refresh_after_close["status"] == "outside_trading_hours", refresh_after_close
    assert refresh_after_close["monitor_window"]["polling_active"] is False, refresh_after_close
    assert calls["quotes"] == calls_after_grace, calls

    print("Phase 39 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Open status: {open_window['status']}")
    print(f"Grace status: {grace_window['status']}")
    print(f"Closed status: {closed_window['status']}")
    print(f"Next resume at: {closed_window['next_resume_at']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
