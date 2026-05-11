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
from saxo_daytrader_xai.execution_engine import execute_order
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.notifications import fetch_notification_deliveries


class _FakeSlackResponse:
    status_code = 200

    def raise_for_status(self) -> None:
        return None


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase31_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase31_missing_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["app"]["dry_run"] = False
    config["notifications"]["slack"]["enabled"] = True
    config["notifications"]["slack"]["webhook_url"] = "https://hooks.slack.test/services/T000/B000/XXX"
    config["notifications"]["alerts"]["execution_failure_enabled"] = True
    config["notifications"]["alerts"]["broker_fill_enabled"] = False
    config["notifications"]["alerts"]["broker_reject_enabled"] = False
    config["notifications"]["alerts"]["broker_cancel_enabled"] = False
    config["saxo"]["session_path"] = str(session_path)

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)

    created_at = datetime(2026, 4, 6, 18, 30, tzinfo=UTC).isoformat(timespec="seconds")
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, ledger_id, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            created_at,
            None,
            "SBUX:xnas",
            "BUY",
            "live",
            "pending_approval",
            "saxo",
            0.03,
            12.0,
            93.65,
            "USD",
            7286.12,
            1,
            None,
            None,
            json.dumps({"seed": "validate_phase31"}, ensure_ascii=False, sort_keys=True),
            None,
            None,
        ),
    )
    order_id = int(cursor.lastrowid)
    connection.commit()

    slack_calls: list[dict] = []
    original_post = notifications.requests.post

    def fake_post(url: str, **kwargs):
        slack_calls.append({"url": url, "json": kwargs.get("json")})
        return _FakeSlackResponse()

    notifications.requests.post = fake_post
    try:
        result_execute = execute_order(order_id, config=config, connection=connection, approved=True)
    finally:
        notifications.requests.post = original_post

    deliveries = fetch_notification_deliveries(connection, limit=10)
    sent_rows = [row for row in deliveries if row["status"] == "sent"]
    failure_rows = [row for row in sent_rows if row["summary_kind"] == "alert_execution_failed"]
    stored_order = connection.execute("SELECT status, error_text FROM execution_orders WHERE id = ?", (order_id,)).fetchone()

    assert result_execute["status"] == "execution_failed", result_execute
    assert stored_order["status"] == "execution_failed", dict(stored_order)
    assert "Saxo session file is missing" in str(stored_order["error_text"]), dict(stored_order)
    assert len(failure_rows) == 1, failure_rows
    assert len(slack_calls) == 1, slack_calls

    payload_json = failure_rows[0]["payload_json"]
    assert payload_json["alert_type"] == "execution_failed", payload_json
    assert payload_json["record"]["symbol"] == "SBUX:xnas", payload_json

    print("Phase 31 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Execution failure alerts sent: {len(failure_rows)}")
    print(f"Slack success payloads: {len(slack_calls)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
