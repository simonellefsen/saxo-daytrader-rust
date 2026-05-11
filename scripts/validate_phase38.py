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
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_goal_tracking


def _insert_history_row(connection, *, recorded_at: str, baseline_session_date: str, total_value: float) -> None:
    connection.execute(
        """
        INSERT INTO portfolio_value_history (
            recorded_at,
            snapshot_type,
            baseline_session_date,
            batch_id,
            total_market_value_dkk,
            invested_market_value_dkk,
            cash_balance_dkk,
            total_cost_basis_dkk,
            total_unrealised_pnl_dkk,
            total_daily_pnl_dkk,
            position_count,
            source,
            raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            recorded_at,
            "price_monitor",
            baseline_session_date,
            "phase38",
            total_value,
            total_value - 25000.0,
            25000.0,
            100000.0,
            total_value - 100000.0,
            0.0,
            18,
            "phase38",
            json.dumps({"total_market_value_dkk": total_value}, ensure_ascii=False, sort_keys=True),
        ),
    )
    connection.commit()


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase38_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["portfolio"]["initial_cash_dkk"] = 25000.0

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    connection.execute("DELETE FROM portfolio_value_history")
    connection.commit()

    _insert_history_row(connection, recorded_at="2026-04-01T04:00:00+00:00", baseline_session_date="2026-04-01", total_value=100000.0)
    _insert_history_row(connection, recorded_at="2026-04-02T04:00:00+00:00", baseline_session_date="2026-04-02", total_value=100400.0)
    _insert_history_row(connection, recorded_at="2026-04-06T04:00:00+00:00", baseline_session_date="2026-04-06", total_value=101000.0)
    _insert_history_row(connection, recorded_at="2026-04-06T10:00:00+00:00", baseline_session_date="2026-04-06", total_value=101800.0)

    tracking = fetch_goal_tracking(
        connection,
        config,
        reference_time=datetime(2026, 4, 6, 10, 0, tzinfo=UTC),
    )

    assert round(float(tracking["periods"]["day"]["pnl_dkk"]), 2) == 800.00, tracking
    assert round(float(tracking["periods"]["day"]["target_dkk"]), 2) == 1000.00, tracking
    assert round(float(tracking["periods"]["week"]["pnl_dkk"]), 2) == 800.00, tracking
    assert round(float(tracking["periods"]["month"]["pnl_dkk"]), 2) == 1800.00, tracking
    assert int(tracking["periods"]["month"]["observed_session_days"]) == 3, tracking
    assert round(float(tracking["periods"]["month"]["target_dkk"]), 2) == 2727.27, tracking
    assert round(float(tracking["average_dkk_per_observed_day"]), 2) == 600.00, tracking
    assert round(float(tracking["projected_weekly_dkk_from_average"]), 2) == 3000.00, tracking

    print("Phase 38 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Day pnl DKK: {tracking['periods']['day']['pnl_dkk']:.2f}")
    print(f"Month pnl DKK: {tracking['periods']['month']['pnl_dkk']:.2f}")
    print(f"Average observed-day pnl DKK: {tracking['average_dkk_per_observed_day']:.2f}")
    print(f"Projected weekly pnl DKK: {tracking['projected_weekly_dkk_from_average']:.2f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
