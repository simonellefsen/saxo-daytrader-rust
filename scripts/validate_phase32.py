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

from saxo_daytrader_xai import notifications, saxo_openapi
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import manage_live_order
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.notifications import fetch_notification_deliveries


class _FakeResponse:
    def __init__(self, status_code: int, payload: dict):
        self.status_code = status_code
        self._payload = payload
        self.text = json.dumps(payload)

    def json(self) -> dict:
        return self._payload

    def raise_for_status(self) -> None:
        return None


class _FakeSlackResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase32_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase32_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["app"]["dry_run"] = False
    config["execution"]["mode"] = "live"
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/T000/B000/XXX"
    config["notifications"]["alerts"]["broker_management_failure_enabled"] = True
    config["saxo"]["session_path"] = str(session_path)

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    session_path.write_text(
        json.dumps(
            {
                "environment": "sim",
                "auth_mode": "pkce",
                "account_key": config["saxo"]["account_key"],
                "access_token": "test-access-token",
                "refresh_token": "test-refresh-token",
                "access_token_expires_at": (datetime.now(UTC) + timedelta(hours=1)).isoformat(timespec="seconds"),
                "refresh_token_expires_at": (datetime.now(UTC) + timedelta(days=30)).isoformat(timespec="seconds"),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    created_at = datetime(2026, 4, 6, 18, 45, tzinfo=UTC).isoformat(timespec="seconds")
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, ledger_id, broker_order_id, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            created_at,
            None,
            "MSTR:xnas",
            "SELL",
            "live",
            "submitted_to_broker",
            "saxo",
            0.0,
            11.0,
            126.98,
            "USD",
            9440.0,
            0,
            created_at,
            None,
            "5037667169",
            json.dumps({"seed": "validate_phase32"}, ensure_ascii=False, sort_keys=True),
            json.dumps({"broker_result": {"OrderId": "5037667169"}, "payload": {"OrderType": "Market"}}, ensure_ascii=False, sort_keys=True),
            None,
        ),
    )
    order_id = int(cursor.lastrowid)
    connection.commit()

    original_delete = saxo_openapi.requests.delete
    original_post = notifications.requests.post
    slack_calls: list[dict] = []

    def fake_delete(url: str, **kwargs):
        return _FakeResponse(
            404,
            {
                "Orders": [
                    {
                        "ErrorInfo": {
                            "ErrorCode": "OrderNotFound",
                            "Message": "Requested order id not found",
                        }
                    }
                ]
            },
        )

    def fake_post(url: str, **kwargs):
        slack_calls.append({"url": url, "json": kwargs.get("json")})
        return _FakeSlackResponse()

    saxo_openapi.requests.delete = fake_delete
    notifications.requests.post = fake_post
    try:
        result_manage = manage_live_order(order_id, management_action="cancel", config=config, connection=connection)
    finally:
        saxo_openapi.requests.delete = original_delete
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=10)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]
    mgmt_rows = [row for row in sent_rows if row["summary_kind"] == "alert_broker_management_failed"]
    stored_order = connection.execute("SELECT status, error_text FROM execution_orders WHERE id = ?", (order_id,)).fetchone()
    event_count = connection.execute(
        "SELECT COUNT(*) AS count_rows FROM execution_order_events WHERE execution_order_id = ? AND event_type = 'broker_cancel_failed'",
        (order_id,),
    ).fetchone()["count_rows"]

    assert result_manage["status"] == "management_failed", result_manage
    assert "OrderNotFound" in result_manage["error"], result_manage
    assert stored_order["status"] == "submitted_to_broker", dict(stored_order)
    assert "OrderNotFound" in str(stored_order["error_text"]), dict(stored_order)
    assert event_count == 1, event_count
    assert len(mgmt_rows) == 1, mgmt_rows
    assert len(slack_calls) == 1, slack_calls

    print("Phase 32 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Management failure alerts sent: {len(mgmt_rows)}")
    print(f"Slack success payloads: {len(slack_calls)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
