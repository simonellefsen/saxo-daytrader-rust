from __future__ import annotations

import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, fetch_scheduler_status, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai import scheduler_service


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase22_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    sync_portfolio(config)

    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    scheduler_service.refresh_market_calendars = lambda _config: {"status": "ok"}
    scheduler_service.get_market_status = lambda _config: []
    scheduler_service.summarize_analysis_window = lambda _rows: {"analysis_window_active": True, "active_markets": []}
    scheduler_service.should_auto_run_decision_report = lambda *_args, **_kwargs: True
    scheduler_service.generate_decision_report = lambda **_kwargs: {"status": "completed", "id": 1}
    scheduler_service.queue_and_maybe_execute_latest_report = lambda **_kwargs: {"status": "ok", "orders": []}
    scheduler_service.dispatch_summaries_if_due = lambda *_args, **_kwargs: {"status": "ok", "results": []}
    scheduler_service.dispatch_broker_alerts_if_due = lambda *_args, **_kwargs: {"status": "ok", "alerts": [], "sent": []}

    result = scheduler_service.run_scheduler_cycle(config=config, connection=connection, force_mock=True, force_decision=True)
    status = fetch_scheduler_status(connection)

    assert result["status"] == "ok", result
    assert status is not None, status
    assert status["last_cycle_status"] == "ok", status
    assert status["last_heartbeat_at"], status
    assert status["last_cycle_json"] is not None, status

    print("Phase 22 validation passed.")
    print(f"Scheduler status row present: {status is not None}")
    print(f"Last cycle status: {status['last_cycle_status']}")
    print(f"Heartbeat recorded: {bool(status['last_heartbeat_at'])}")
    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
