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
from saxo_daytrader_xai.execution_engine import sync_broker_order_statuses
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
            approval_required, approved_at, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            json.dumps({"source": "validate_phase7"}, ensure_ascii=False, sort_keys=True),
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
    db_path = Path("/tmp") / f"saxo_daytrader_phase7_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase7_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["execution"]["adapter"] = "saxo"
    config["saxo"]["session_path"] = str(session_path)
    config["app"]["dry_run"] = False

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    position = next(row for row in fetch_portfolio_positions(connection) if row["current_price_local"] not in (None, 0))

    working_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=1.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-WORKING-1",
    )
    fill_order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=2.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-FILL-2",
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
        if "/port/v1/orders/" in url and url.endswith("/SIM-WORKING-1"):
            return _FakeResponse({"Data": [{"OrderId": "SIM-WORKING-1", "Status": "Working"}]})
        if "/port/v1/orders/" in url and url.endswith("/SIM-FILL-2"):
            return _FakeResponse(status_code=404)
        if url.endswith("/cs/v1/audit/orderactivities"):
            broker_order_id = str(kwargs.get("params", {}).get("OrderId"))
            if broker_order_id == "SIM-FILL-2":
                return _FakeResponse(
                    {
                        "Data": [
                            {
                                "OrderId": "SIM-FILL-2",
                                "Status": "FinalFill",
                                "SubStatus": "Confirmed",
                                "AveragePrice": float(position["current_price_local"]),
                                "FilledAmount": 2.0,
                            }
                        ]
                    }
                )
        raise AssertionError(f"Unexpected GET {url} {kwargs}")

    saxo_openapi.requests.get = fake_get
    try:
        sync_result = sync_broker_order_statuses(config=config, connection=connection, limit=10)
    finally:
        saxo_openapi.requests.get = original_get

    working_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (working_order_id,)).fetchone()
    filled_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (fill_order_id,)).fetchone()
    trade_count = connection.execute("SELECT COUNT(*) AS count_rows FROM trade_ledger").fetchone()["count_rows"]
    lot_count = connection.execute("SELECT COUNT(*) AS count_rows FROM position_lots").fetchone()["count_rows"]

    assert sync_result["status"] == "ok", sync_result
    assert working_row["status"] == "broker_working", dict(working_row)
    assert filled_row["status"] == "executed", dict(filled_row)
    assert filled_row["ledger_id"] is not None, dict(filled_row)
    assert trade_count >= 1, "Expected a live fill sync to create a trade_ledger row"
    assert lot_count >= 1, "Expected a live buy fill sync to create a lot"

    print("Phase 7 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Working order status: {working_row['status']}")
    print(f"Filled order status: {filled_row['status']}")
    print(f"Filled ledger id: {filled_row['ledger_id']}")
    print(f"Trade ledger rows: {trade_count}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
