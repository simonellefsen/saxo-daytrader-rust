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
from saxo_daytrader_xai.notifications import dispatch_broker_alerts_if_due, dispatch_summary_if_due, fetch_notification_deliveries


class _FakeResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase24_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/default"
    config["notifications"]["weekly_summary_enabled"] = True
    config["notifications"]["alerts"]["broker_fill_enabled"] = True
    config["notifications"]["route_profiles"] = {
        "ops": {
            "slack_webhook_url": "https://hooks.slack.test/services/ops",
            "subject_prefix": "[OPS]",
            "message_preamble": "Shared profile preamble",
            "summary_style": "compact",
        }
    }
    config["notifications"]["routes"]["weekly"] = {"profile": "ops"}
    config["notifications"]["routes"]["alert_broker_fill"] = {"profile": "ops"}

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
            901,
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
            "SIM-ORDER-901",
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
            901,
            "SIM-FILL-901",
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

    calls: list[dict[str, str]] = []
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        calls.append({"url": url, "text": str(kwargs.get("json", {}).get("text", ""))})
        return _FakeResponse()

    notifications.requests.post = fake_post
    try:
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
    weekly_row = next(row for row in sent_rows if row["summary_kind"] == "weekly")
    alert_row = next(row for row in sent_rows if row["summary_kind"] == "alert_broker_fill")

    assert weekly["sent"][0]["status"] == "sent", weekly
    assert alerts["sent"][0]["status"] == "sent", alerts
    assert len(calls) == 2, calls
    assert all(call["url"] == "https://hooks.slack.test/services/ops" for call in calls), calls
    assert weekly_row["subject"].startswith("[OPS] "), weekly_row
    assert alert_row["subject"].startswith("[OPS] "), alert_row
    assert weekly_row["message_text"].startswith("Shared profile preamble\n\n"), weekly_row
    assert alert_row["message_text"].startswith("Shared profile preamble\n\n"), alert_row
    assert "Portfolio:" not in weekly_row["message_text"], weekly_row["message_text"]
    assert " | Trades " in weekly_row["message_text"], weekly_row["message_text"]

    print("Phase 24 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Formatted deliveries sent: {len(sent_rows)}")
    print(f"Subject prefix applied: {weekly_row['subject'].startswith('[OPS] ') and alert_row['subject'].startswith('[OPS] ')}")
    print(f"Message preamble applied: {weekly_row['message_text'].startswith('Shared profile preamble') and alert_row['message_text'].startswith('Shared profile preamble')}")
    print(f"Profile style applied: {'Portfolio:' not in weekly_row['message_text'] and ' | Trades ' in weekly_row['message_text']}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
