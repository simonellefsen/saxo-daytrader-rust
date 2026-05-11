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
from saxo_daytrader_xai.notifications import dispatch_broker_alerts_if_due, fetch_notification_deliveries


class _FakeResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase18_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/default"
    config["notifications"]["alerts"]["broker_reject_enabled"] = True
    config["notifications"]["alerts"]["broker_cancel_enabled"] = True
    config["notifications"]["alert_suppression"]["enabled"] = True
    config["notifications"]["alert_suppression"]["low_cooldown_minutes"] = 240
    config["notifications"]["alert_suppression"]["high_cooldown_minutes"] = 0

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    now = datetime(2026, 4, 6, 19, 30, tzinfo=UTC).isoformat(timespec="seconds")
    for order_id, symbol in ((401, "MSFT:xnas"), (402, "PLTR:xnas")):
        connection.execute(
            """
            INSERT INTO execution_orders (
                id, created_at, report_id, symbol, action, mode, status, adapter,
                requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
                approval_required, approved_at, ledger_id, request_json, execution_result_json, error_text, broker_order_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                order_id,
                now,
                None,
                symbol,
                "SELL",
                "live",
                "submitted_to_broker",
                "saxo",
                None,
                5.0,
                99.5,
                "USD",
                3500.0,
                0,
                None,
                None,
                json.dumps({"seed": "order"}, ensure_ascii=False, sort_keys=True),
                json.dumps({"seed": "broker_result"}, ensure_ascii=False, sort_keys=True),
                None,
                f"SIM-ORDER-{order_id}",
            ),
        )
    for event_id, order_id, event_type in (
        (501, 401, "broker_cancelled"),
        (502, 402, "broker_rejected"),
    ):
        connection.execute(
            """
            INSERT INTO execution_order_events (
                id, created_at, execution_order_id, broker_order_id, event_type,
                broker_status, broker_substatus, broker_quantity, broker_price_local,
                event_signature, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                event_id,
                now,
                order_id,
                f"SIM-EVENT-{event_id}",
                event_type,
                event_type,
                "Confirmed",
                5.0,
                99.5,
                f"seed-{event_type}-{event_id}",
                json.dumps({"seed": event_type}, ensure_ascii=False, sort_keys=True),
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
        first = dispatch_broker_alerts_if_due(connection, config, reference_time=datetime(2026, 4, 6, 19, 31, tzinfo=UTC))

        connection.execute(
            """
            INSERT INTO execution_order_events (
                id, created_at, execution_order_id, broker_order_id, event_type,
                broker_status, broker_substatus, broker_quantity, broker_price_local,
                event_signature, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                503,
                datetime(2026, 4, 6, 19, 32, tzinfo=UTC).isoformat(timespec="seconds"),
                401,
                "SIM-EVENT-503",
                "broker_cancelled",
                "broker_cancelled",
                "Confirmed",
                5.0,
                99.5,
                "seed-broker_cancelled-503",
                json.dumps({"seed": "broker_cancelled-repeat"}, ensure_ascii=False, sort_keys=True),
            ),
        )
        connection.execute(
            """
            INSERT INTO execution_order_events (
                id, created_at, execution_order_id, broker_order_id, event_type,
                broker_status, broker_substatus, broker_quantity, broker_price_local,
                event_signature, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                504,
                datetime(2026, 4, 6, 19, 32, tzinfo=UTC).isoformat(timespec="seconds"),
                402,
                "SIM-EVENT-504",
                "broker_rejected",
                "broker_rejected",
                "Confirmed",
                5.0,
                99.5,
                "seed-broker_rejected-504",
                json.dumps({"seed": "broker_rejected-repeat"}, ensure_ascii=False, sort_keys=True),
            ),
        )
        connection.commit()

        second = dispatch_broker_alerts_if_due(connection, config, reference_time=datetime(2026, 4, 6, 19, 33, tzinfo=UTC))
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]

    assert len([row for row in first["sent"] if row["status"] == "sent"]) == 2, first
    second_sent = [row for row in second["sent"] if row["status"] == "sent"]
    second_skipped = [row for row in second["sent"] if row["status"] == "skipped"]
    assert len(second_sent) == 1, second
    assert len(second_skipped) == 1, second
    assert second_skipped[0]["reason"] == "suppressed_low", second_skipped
    assert second_sent[0]["summary_kind"] == "alert_broker_reject", second_sent
    assert len(sent_rows) == 3, sent_rows
    assert len(calls) == 3, calls

    print("Phase 18 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"First pass sent: {len([row for row in first['sent'] if row['status'] == 'sent'])}")
    print(f"Second pass suppressed/skipped: {len(second_skipped)}")
    print(f"Slack success payloads: {len(calls)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
