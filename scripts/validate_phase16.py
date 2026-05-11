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
    db_path = Path("/tmp") / f"saxo_daytrader_phase16_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/T000/B000/XXX"
    config["notifications"]["alerts"]["broker_fill_enabled"] = True
    config["notifications"]["alerts"]["broker_reject_enabled"] = True
    config["notifications"]["alerts"]["broker_cancel_enabled"] = True

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    now = datetime(2026, 4, 6, 19, 30, tzinfo=UTC).isoformat(timespec="seconds")
    for order_id, symbol, action in (
        (101, "AMD:xnas", "BUY"),
        (201, "MSFT:xnas", "SELL"),
        (202, "PLTR:xnas", "SELL"),
    ):
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
                action,
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
    connection.execute(
        """
        INSERT INTO execution_fills (
            created_at, execution_order_id, broker_order_id, symbol, side, fill_status,
            cumulative_quantity, delta_quantity, average_price_local, currency, ledger_id, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            now,
            101,
            "SIM-FILL-101",
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
    for event_id, event_type in ((201, "broker_rejected"), (202, "broker_cancelled")):
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
                event_id,
                f"SIM-EVENT-{event_id}",
                event_type,
                event_type,
                "Confirmed",
                5.0,
                99.5,
                f"seed-{event_type}",
                json.dumps({"seed": event_type}, ensure_ascii=False, sort_keys=True),
            ),
        )
    connection.commit()

    sent_payloads: list[dict] = []
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        sent_payloads.append({"url": url, "json": kwargs.get("json")})
        return _FakeResponse()

    notifications.requests.post = fake_post
    try:
        first = dispatch_broker_alerts_if_due(connection, config, reference_time=datetime(2026, 4, 6, 19, 31, tzinfo=UTC))
        second = dispatch_broker_alerts_if_due(connection, config, reference_time=datetime(2026, 4, 6, 19, 32, tzinfo=UTC))
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]

    assert first["status"] == "ok", first
    assert len(first["sent"]) == 3, first
    assert all(row["status"] == "sent" for row in first["sent"]), first
    assert second["status"] == "ok", second
    assert all(row["status"] == "skipped" for row in second["sent"]), second
    assert {row["summary_kind"] for row in sent_rows} == {
        "alert_broker_fill",
        "alert_broker_reject",
        "alert_broker_cancel",
    }, sent_rows
    assert len(sent_rows) == 3, sent_rows
    assert len(sent_payloads) == 3, sent_payloads

    print("Phase 16 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Alert deliveries sent: {len(sent_rows)}")
    print(f"Slack success payloads: {len(sent_payloads)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
