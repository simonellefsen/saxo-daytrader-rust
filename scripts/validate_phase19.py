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
from saxo_daytrader_xai.notifications import (
    dispatch_broker_alerts_if_due,
    dispatch_summary_if_due,
    fetch_notification_deliveries,
)


class _FakeResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase19_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/default"
    config["notifications"]["weekly_summary_enabled"] = True
    config["notifications"]["alerts"]["broker_fill_enabled"] = True
    config["notifications"]["route_profiles"] = {
        "ops": {
            "slack_webhook_url": "https://hooks.slack.test/services/ops",
        }
    }
    config["notifications"]["routes"]["weekly"] = {
        "profile": "ops",
    }
    config["notifications"]["routes"]["alert_broker_fill"] = {
        "profile": "ops",
    }
    config["notifications"]["routes"]["daily"] = {
        "profile": "ops",
        "slack_webhook_url": "https://hooks.slack.test/services/daily-override",
    }

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    now = datetime(2026, 4, 6, 19, 30, tzinfo=UTC).isoformat(timespec="seconds")
    connection.execute(
        """
        INSERT INTO execution_orders (
            id, created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, ledger_id, request_json, execution_result_json, error_text, broker_order_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            601,
            now,
            None,
            "AMD:xnas",
            "BUY",
            "live",
            "submitted_to_broker",
            "saxo",
            None,
            5.0,
            101.0,
            "USD",
            3500.0,
            0,
            None,
            None,
            json.dumps({"seed": "order"}, ensure_ascii=False, sort_keys=True),
            json.dumps({"seed": "broker_result"}, ensure_ascii=False, sort_keys=True),
            None,
            "SIM-ORDER-601",
        ),
    )
    connection.execute(
        """
        INSERT INTO execution_fills (
            created_at, execution_order_id, broker_order_id, symbol, side, fill_status,
            cumulative_quantity, delta_quantity, average_price_local, currency, ledger_id, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            now,
            601,
            "SIM-FILL-601",
            "AMD:xnas",
            "BUY",
            "FinalFill",
            10.0,
            10.0,
            101.25,
            "USD",
            None,
            json.dumps({"seed": "fill"}, ensure_ascii=False, sort_keys=True),
        ),
    )
    connection.commit()

    calls: list[str] = []
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        calls.append(url)
        return _FakeResponse()

    notifications.requests.post = fake_post
    try:
        daily = dispatch_summary_if_due(
            connection,
            config,
            summary_kind="daily",
            reference_time=datetime(2026, 4, 6, 19, 31, tzinfo=UTC),
            force=True,
        )
        weekly = dispatch_summary_if_due(
            connection,
            config,
            summary_kind="weekly",
            reference_time=datetime(2026, 4, 13, 19, 31, tzinfo=UTC),
            force=True,
        )
        alerts = dispatch_broker_alerts_if_due(
            connection,
            config,
            reference_time=datetime(2026, 4, 6, 19, 32, tzinfo=UTC),
            force=True,
        )
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]

    assert daily["sent"][0]["status"] == "sent", daily
    assert weekly["sent"][0]["status"] == "sent", weekly
    assert alerts["sent"][0]["status"] == "sent", alerts
    assert calls.count("https://hooks.slack.test/services/ops") == 2, calls
    assert calls.count("https://hooks.slack.test/services/daily-override") == 1, calls
    assert len(sent_rows) == 3, sent_rows

    print("Phase 19 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Profile-routed deliveries sent: {len(sent_rows)}")
    print(f"Profile webhook calls: {calls.count('https://hooks.slack.test/services/ops')}")
    print(f"Override webhook calls: {calls.count('https://hooks.slack.test/services/daily-override')}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
