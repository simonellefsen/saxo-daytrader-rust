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
from saxo_daytrader_xai.execution_engine import fetch_execution_events, manage_live_order, sync_broker_order_statuses
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
    payload = {
        "AccountKey": "ACCOUNT-KEY-1",
        "Amount": quantity,
        "AssetType": "Stock",
        "OrderDuration": {"DurationType": "DayOrder"},
        "OrderId": broker_order_id,
        "OrderPrice": price_local,
        "OrderType": "Limit",
    }
    execution_result = {
        "payload": payload,
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
            "broker_working",
            "saxo",
            0.05,
            quantity,
            price_local,
            currency,
            price_local * quantity,
            1,
            datetime.now(UTC).isoformat(timespec="seconds"),
            broker_order_id,
            json.dumps({"source": "validate_phase12"}, ensure_ascii=False, sort_keys=True),
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
    db_path = Path("/tmp") / f"saxo_daytrader_phase12_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase12_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["execution"]["adapter"] = "saxo"
    config["saxo"]["session_path"] = str(session_path)
    config["app"]["dry_run"] = False

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    position = next(row for row in fetch_portfolio_positions(connection) if row["current_price_local"] not in (None, 0))

    replace_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=2.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-REPLACE-12",
    )
    cancel_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=1.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-CANCEL-12",
    )

    session_path.write_text(
        json.dumps(
            {
                "environment": "sim",
                "auth_mode": "pkce",
                "client_key": config["saxo"].get("client_key") or "CLIENT-KEY-1",
                "account_key": config["saxo"].get("account_key") or "ACCOUNT-KEY-1",
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

    original_patch = saxo_openapi.requests.patch
    original_delete = saxo_openapi.requests.delete
    original_get = saxo_openapi.requests.get

    def fake_patch(url: str, **kwargs):
        assert url.endswith("/trade/v2/orders"), url
        return _FakeResponse({"OrderId": "SIM-REPLACE-12", "Orders": [{"OrderId": "SIM-REPLACE-12"}]})

    def fake_delete(url: str, **kwargs):
        assert url.endswith("/trade/v2/orders/SIM-CANCEL-12"), url
        return _FakeResponse({"Orders": [{"OrderId": "SIM-CANCEL-12"}]})

    def fake_get(url: str, **kwargs):
        if "/port/v1/orders/" in url and url.endswith("/SIM-REPLACE-12"):
            return _FakeResponse(
                {
                    "Data": [
                        {
                            "OrderId": "SIM-REPLACE-12",
                            "Status": "Working",
                            "Amount": 3.0,
                            "OrderPrice": float(position["current_price_local"]) + 2.5,
                        }
                    ]
                }
            )
        if "/port/v1/orders/" in url and url.endswith("/SIM-CANCEL-12"):
            return _FakeResponse(status_code=404)
        if url.endswith("/cs/v1/audit/orderactivities"):
            broker_order_id = str(kwargs.get("params", {}).get("OrderId"))
            if broker_order_id == "SIM-CANCEL-12":
                return _FakeResponse(
                    {
                        "Data": [
                            {
                                "OrderId": "SIM-CANCEL-12",
                                "Status": "Cancelled",
                                "SubStatus": "Confirmed",
                                "Amount": 1.0,
                                "OrderPrice": float(position["current_price_local"]),
                            }
                        ]
                    }
                )
        raise AssertionError(f"Unexpected GET {url} {kwargs}")

    saxo_openapi.requests.patch = fake_patch
    saxo_openapi.requests.delete = fake_delete
    saxo_openapi.requests.get = fake_get
    try:
        replace_result = manage_live_order(
            replace_order_id,
            management_action="replace",
            config=config,
            connection=connection,
            new_quantity=3.0,
            new_price=float(position["current_price_local"]) + 2.5,
        )
        cancel_result = manage_live_order(
            cancel_order_id,
            management_action="cancel",
            config=config,
            connection=connection,
        )
        sync_result = sync_broker_order_statuses(config=config, connection=connection, limit=10)
    finally:
        saxo_openapi.requests.patch = original_patch
        saxo_openapi.requests.delete = original_delete
        saxo_openapi.requests.get = original_get

    replace_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (replace_order_id,)).fetchone()
    cancel_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (cancel_order_id,)).fetchone()
    events = fetch_execution_events(connection, limit=20)

    assert replace_result["status"] == "broker_replace_requested", replace_result
    assert cancel_result["status"] == "broker_cancel_requested", cancel_result
    assert sync_result["status"] == "ok", sync_result
    assert replace_row["status"] == "broker_amended", dict(replace_row)
    assert abs(float(replace_row["quantity"]) - 3.0) < 1e-9, dict(replace_row)
    assert cancel_row["status"] == "broker_cancelled", dict(cancel_row)
    assert any(row["event_type"] == "broker_replace_requested" for row in events), events
    assert any(row["event_type"] == "broker_cancel_requested" for row in events), events
    assert any(row["event_type"] == "broker_amended" for row in events), events
    assert any(row["event_type"] == "broker_cancelled" for row in events), events

    print("Phase 12 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Replace request status: {replace_result['status']}")
    print(f"Cancel request status: {cancel_result['status']}")
    print(f"Replace sync status: {replace_row['status']}")
    print(f"Cancel sync status: {cancel_row['status']}")
    print(f"Broker event rows: {len(events)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
