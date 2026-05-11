from __future__ import annotations

import json
import sys
import uuid
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import scheduler_service
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, fetch_scheduler_cycles, init_db, record_scheduler_cycle
from saxo_daytrader_xai.importer import sync_portfolio


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase27_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["scheduler"]["history_max_rows"] = 3
    config["scheduler"]["history_retention_days"] = 1
    sync_portfolio(config)

    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    old_started_at = (datetime.now(UTC) - timedelta(days=5)).isoformat(timespec="seconds")
    record_scheduler_cycle(
        connection,
        started_at=old_started_at,
        completed_at=old_started_at,
        status="ok",
        analysis_window_active=False,
        generated_decision=False,
        queue_status="ok",
        notifications_status="ok",
        broker_alerts_status="ok",
        cycle_json={"status": "ok", "seed": "old"},
    )

    scheduler_service.refresh_market_calendars = lambda _config: {"status": "ok"}
    scheduler_service.get_market_status = lambda _config: []
    scheduler_service.summarize_analysis_window = lambda _rows: {"analysis_window_active": True, "active_markets": ["XCSE"]}
    scheduler_service.should_auto_run_decision_report = lambda *_args, **_kwargs: True
    scheduler_service.generate_decision_report = lambda **_kwargs: {"status": "completed", "id": 1}
    scheduler_service.queue_and_maybe_execute_latest_report = lambda **_kwargs: {"status": "ok", "orders": []}
    scheduler_service.dispatch_summaries_if_due = lambda *_args, **_kwargs: {"status": "ok", "results": []}
    scheduler_service.dispatch_broker_alerts_if_due = lambda *_args, **_kwargs: {"status": "ok", "alerts": [], "sent": []}

    for _ in range(4):
        scheduler_service.run_scheduler_cycle(config=config, connection=connection, force_mock=True, force_decision=True)

    cycles = fetch_scheduler_cycles(connection, limit=10)
    audit_rows = connection.execute(
        """
        SELECT event_json
        FROM audit_log
        WHERE event_type = 'scheduler_cycle_history_pruned'
        ORDER BY id DESC
        """
    ).fetchall()
    parsed_audit = [json.loads(row["event_json"]) for row in audit_rows]

    assert len(cycles) == 3, cycles
    assert all(cycle["cycle_json"].get("seed") != "old" for cycle in cycles), cycles
    assert parsed_audit, parsed_audit
    assert any(int(row["deleted_rows"]) >= 1 for row in parsed_audit), parsed_audit

    print("Phase 27 validation passed.")
    print(f"Retained cycle rows: {len(cycles)}")
    print(f"Old cycle removed: {all(cycle['cycle_json'].get('seed') != 'old' for cycle in cycles)}")
    print(f"Prune audit entries: {len(parsed_audit)}")
    print(f"Configured max rows: {config['scheduler']['history_max_rows']}")
    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
