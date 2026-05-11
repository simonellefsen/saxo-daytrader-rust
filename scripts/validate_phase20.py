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
    db_path = Path("/tmp") / f"saxo_daytrader_phase20_{uuid.uuid4().hex}.db"
    config["portfolio"]["database_path"] = str(db_path)
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/default"
    config["notifications"]["alerts"]["broker_fill_enabled"] = True
    config["notifications"]["alerts"]["broker_cancel_enabled"] = True
    config["notifications"]["alert_grouping"]["enabled"] = True
    config["notifications"]["alert_grouping"]["max_items_per_group"] = 5

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    now = datetime(2026, 4, 6, 19, 30, tzinfo=UTC).isoformat(timespec="seconds")
    for order_id, symbol in ((701, "AMD:xnas"), (702, "MSFT:xnas")):
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
            701,
            "SIM-FILL-701",
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
    connection.execute(
        """
        INSERT INTO execution_order_events (
            id, created_at, execution_order_id, broker_order_id, event_type,
            broker_status, broker_substatus, broker_quantity, broker_price_local,
            event_signature, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            801,
            now,
            701,
            "SIM-EVENT-801",
            "broker_cancelled",
            "broker_cancelled",
            "Confirmed",
            5.0,
            99.5,
            "seed-broker_cancelled-801",
            json.dumps({"seed": "broker_cancelled"}, ensure_ascii=False, sort_keys=True),
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
            802,
            now,
            702,
            "SIM-EVENT-802",
            "broker_cancelled",
            "broker_cancelled",
            "Confirmed",
            5.0,
            99.5,
            "seed-broker_cancelled-802",
            json.dumps({"seed": "broker_cancelled-2"}, ensure_ascii=False, sort_keys=True),
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
        dispatch = dispatch_broker_alerts_if_due(connection, config, reference_time=datetime(2026, 4, 6, 19, 31, tzinfo=UTC))
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=20)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]
    grouped_rows = [row for row in sent_rows if row["summary_kind"] == "alert_broker_grouped"]

    assert dispatch["status"] == "ok", dispatch
    assert len([row for row in dispatch["sent"] if row["status"] == "sent"]) == 2, dispatch
    assert len(grouped_rows) == 1, grouped_rows
    assert len(sent_rows) == 2, sent_rows
    assert len(calls) == 2, calls

    print("Phase 20 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Grouped deliveries sent: {len(sent_rows)}")
    print(f"Grouped alert kind rows: {len(grouped_rows)}")
    print(f"Slack success payloads: {len(calls)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
