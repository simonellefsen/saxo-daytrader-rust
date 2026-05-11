from __future__ import annotations

import sys
import uuid
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from saxo_daytrader_xai import notifications
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.notifications import (
    build_summary,
    dispatch_daily_summary_if_due,
    dispatch_summary_if_due,
    fetch_notification_deliveries,
)


class _FakeResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase14_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["daily_summary_enabled"] = True
    config["notifications"]["weekly_summary_enabled"] = True
    config["notifications"]["monthly_summary_enabled"] = True
    config["notifications"]["weekly_dispatch_weekday_local"] = 0
    config["notifications"]["monthly_dispatch_day_local"] = 1
    config["notifications"]["dispatch_hour_local"] = 0
    config["notifications"]["dispatch_minute_local"] = 0
    config["notifications"]["retry_backoff_minutes"] = 30
    config["notifications"]["max_attempts_per_day"] = 3
    config["notifications"]["channel_cooldown_minutes"] = 240
    config["notifications"]["summary_style"] = "structured"
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/T000/B000/XXX"

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    sent_payloads: list[dict] = []
    post_attempts = {"count": 0}
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        post_attempts["count"] += 1
        if post_attempts["count"] == 1:
            raise RuntimeError("temporary slack outage")
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
            reference_time=datetime(2026, 4, 6, 18, 40, tzinfo=UTC),
        )
        third = dispatch_daily_summary_if_due(
            connection,
            config,
            reference_time=datetime(2026, 4, 6, 19, 5, tzinfo=UTC),
        )
        fourth = dispatch_daily_summary_if_due(
            connection,
            config,
            reference_time=datetime(2026, 4, 6, 19, 15, tzinfo=UTC),
        )
        weekly = dispatch_summary_if_due(
            connection,
            config,
            summary_kind="weekly",
            reference_time=datetime(2026, 4, 13, 19, 15, tzinfo=UTC),
            force=True,
        )
        monthly = dispatch_summary_if_due(
            connection,
            config,
            summary_kind="monthly",
            reference_time=datetime(2026, 5, 1, 19, 15, tzinfo=UTC),
            force=True,
        )
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    failed_rows = [row for row in deliveries if row["status"] == "failed"]
    sent_rows = [row for row in deliveries if row["status"] == "sent"]
    summary_preview = build_summary(connection, config, summary_kind="weekly", reference_time=datetime(2026, 4, 13, 19, 15, tzinfo=UTC))

    assert first["sent"][0]["status"] == "failed", first
    assert second["sent"][0]["status"] == "skipped", second
    assert second["sent"][0]["reason"] == "backoff_active", second
    assert third["sent"][0]["status"] == "sent", third
    assert fourth["sent"][0]["status"] == "skipped", fourth
    assert fourth["sent"][0]["reason"] in {"already_sent", "cooldown_active"}, fourth
    assert weekly["summary_kind"] == "weekly", weekly
    assert monthly["summary_kind"] == "monthly", monthly
    assert len(failed_rows) == 1, failed_rows
    assert len(sent_rows) == 3, sent_rows
    assert "Period:" in summary_preview["message_text"], summary_preview["message_text"]

    print("Phase 14 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Failed deliveries: {len(failed_rows)}")
    print(f"Sent deliveries: {len(sent_rows)}")
    print(f"Slack success payloads: {len(sent_payloads)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
