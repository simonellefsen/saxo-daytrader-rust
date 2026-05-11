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

from saxo_daytrader_xai import saxo_openapi
from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import fetch_execution_events, sync_broker_order_statuses
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions


def _insert_submitted_order(
    connection,
    *,
    symbol: str,
    quantity: float,
    price_local: float,
    currency: str,
    broker_order_id: str,
) -> int:
    execution_result = {
        "payload": {"ExternalReference": f"saxo-daytrader:{broker_order_id}"},
        "broker_result": {"OrderId": broker_order_id, "Status": "Placed"},
        "precheck": {"EstimatedTotalCost": 10.0},
    }
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, approved_at, broker_order_id, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            None,
            symbol,
            "BUY",
            "live",
            "submitted_to_broker",
            "saxo",
            0.05,
            quantity,
            price_local,
            currency,
            price_local * quantity,
            1,
            datetime.now(UTC).isoformat(timespec="seconds"),
            broker_order_id,
            json.dumps({"source": "validate_phase9"}, ensure_ascii=False, sort_keys=True),
            json.dumps(execution_result, ensure_ascii=False, sort_keys=True),
            None,
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


class _FakeResponse:
    def __init__(self, payload: dict | None = None, status_code: int = 200):
        self._payload = payload or {}
        self.status_code = status_code

    def raise_for_status(self) -> None:
        if self.status_code >= 400:
            raise saxo_openapi.requests.HTTPError(f"HTTP {self.status_code}")

    def json(self) -> dict:
        return self._payload


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase9_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase9_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["execution"]["adapter"] = "saxo"
    config["saxo"]["session_path"] = str(session_path)
    config["app"]["dry_run"] = False

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    position = next(row for row in fetch_portfolio_positions(connection) if row["current_price_local"] not in (None, 0))

    amended_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=2.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-CHANGE-9",
    )
    cancelled_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=1.5,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-CANCEL-9",
    )

    session_path.write_text(
        json.dumps(
            {
                "environment": "sim",
                "auth_mode": "pkce",
                "client_key": config["saxo"].get("client_key") or "CLIENT-KEY-1",
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

    original_get = saxo_openapi.requests.get

    def fake_get(url: str, **kwargs):
        if "/port/v1/orders/" in url and url.endswith("/SIM-CHANGE-9"):
            return _FakeResponse(
                {
                    "Data": [
                        {
                            "OrderId": "SIM-CHANGE-9",
                            "Status": "Working",
                            "Amount": 3.0,
                            "OrderPrice": float(position["current_price_local"]) + 4.25,
                        }
                    ]
                }
            )
        if "/port/v1/orders/" in url and url.endswith("/SIM-CANCEL-9"):
            return _FakeResponse(status_code=404)
        if url.endswith("/cs/v1/audit/orderactivities"):
            broker_order_id = str(kwargs.get("params", {}).get("OrderId"))
            if broker_order_id == "SIM-CANCEL-9":
                return _FakeResponse(
                    {
                        "Data": [
                            {
                                "OrderId": "SIM-CANCEL-9",
                                "Status": "Cancelled",
                                "SubStatus": "Confirmed",
                                "Amount": 1.5,
                                "OrderPrice": float(position["current_price_local"]),
                            }
                        ]
                    }
                )
        raise AssertionError(f"Unexpected GET {url} {kwargs}")

    saxo_openapi.requests.get = fake_get
    try:
        first_sync = sync_broker_order_statuses(config=config, connection=connection, limit=10)
        second_sync = sync_broker_order_statuses(config=config, connection=connection, limit=10)
    finally:
        saxo_openapi.requests.get = original_get

    amended_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (amended_order_id,)).fetchone()
    cancelled_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (cancelled_order_id,)).fetchone()
    events = fetch_execution_events(connection, limit=20)
    amended_events = [row for row in events if row["execution_order_id"] == amended_order_id]
    cancelled_events = [row for row in events if row["execution_order_id"] == cancelled_order_id]

    assert first_sync["status"] == "ok", first_sync
    assert second_sync["status"] == "ok", second_sync
    assert amended_row["status"] == "broker_amended", dict(amended_row)
    assert abs(float(amended_row["quantity"]) - 3.0) < 1e-9, dict(amended_row)
    assert abs(float(amended_row["price_local"]) - (float(position["current_price_local"]) + 4.25)) < 1e-9, dict(amended_row)
    assert cancelled_row["status"] == "broker_cancelled", dict(cancelled_row)
    assert len(amended_events) == 1, amended_events
    assert amended_events[0]["event_type"] == "broker_amended", amended_events[0]
    assert len(cancelled_events) == 1, cancelled_events
    assert cancelled_events[0]["event_type"] == "broker_cancelled", cancelled_events[0]

    print("Phase 9 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Amended order status: {amended_row['status']}")
    print(f"Cancelled order status: {cancelled_row['status']}")
    print(f"Broker event rows: {len(events)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
