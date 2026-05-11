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

from saxo_daytrader_xai import notifications
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.notifications import dispatch_daily_summary_if_due, fetch_notification_deliveries
from saxo_daytrader_xai.scheduler_service import run_scheduler_cycle


class _FakeResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase10_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["daily_summary_enabled"] = True
    config["notifications"]["dispatch_hour_local"] = 0
    config["notifications"]["dispatch_minute_local"] = 0
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/T000/B000/XXX"
    config["execution"]["mode"] = "simulation"

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    sent_payloads: list[dict] = []
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        sent_payloads.append({"url": url, "json": kwargs.get("json")})
        return _FakeResponse()

    notifications.requests.post = fake_post
    try:
        first = dispatch_daily_summary_if_due(
            connection,
            config,
            reference_time=datetime(2026, 4, 6, 18, 30, tzinfo=UTC),
        )
        second = dispatch_daily_summary_if_due(
            connection,
            config,
            reference_time=datetime(2026, 4, 6, 19, 0, tzinfo=UTC),
        )
        scheduler_result = run_scheduler_cycle(
            config=config,
            connection=connection,
            force_mock=True,
            force_decision=True,
        )
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]

    assert first["status"] == "ok", first
    assert len(first["sent"]) == 1, first
    assert second["status"] == "ok", second
    assert second["sent"] == [], second
    assert len(sent_payloads) == 1, sent_payloads
    assert len(sent_rows) == 1, sent_rows
    assert scheduler_result["notifications"]["status"] in {"ok", "not_due"}, scheduler_result["notifications"]

    print("Phase 10 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Notifications sent: {len(sent_rows)}")
    print(f"Slack calls captured: {len(sent_payloads)}")
    print(f"Scheduler notification status: {scheduler_result['notifications']['status']}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
