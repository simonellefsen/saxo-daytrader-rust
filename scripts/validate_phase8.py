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
            json.dumps({"source": "validate_phase8"}, ensure_ascii=False, sort_keys=True),
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
    db_path = Path("/tmp") / f"saxo_daytrader_phase8_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase8_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["execution"]["adapter"] = "saxo"
    config["saxo"]["session_path"] = str(session_path)
    config["app"]["dry_run"] = False

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    position = next(row for row in fetch_portfolio_positions(connection) if row["current_price_local"] not in (None, 0))

    order_id = _insert_submitted_order(
        connection,
        symbol=position["symbol"],
        quantity=2.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
        broker_order_id="SIM-PARTIAL-8",
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
    phase = {"value": 1}

    def fake_get(url: str, **kwargs):
        if "/port/v1/orders/" in url and url.endswith("/SIM-PARTIAL-8"):
            return _FakeResponse(status_code=404)
        if url.endswith("/cs/v1/audit/orderactivities"):
            broker_order_id = str(kwargs.get("params", {}).get("OrderId"))
            if broker_order_id == "SIM-PARTIAL-8":
                if phase["value"] == 1:
                    return _FakeResponse(
                        {
                            "Data": [
                                {
                                    "OrderId": "SIM-PARTIAL-8",
                                    "Status": "Fill",
                                    "SubStatus": "Confirmed",
                                    "AveragePrice": float(position["current_price_local"]),
                                    "FilledAmount": 1.0,
                                }
                            ]
                        }
                    )
                return _FakeResponse(
                    {
                        "Data": [
                            {
                                "OrderId": "SIM-PARTIAL-8",
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
        first_sync = sync_broker_order_statuses(config=config, connection=connection, limit=10)
        phase["value"] = 2
        second_sync = sync_broker_order_statuses(config=config, connection=connection, limit=10)
    finally:
        saxo_openapi.requests.get = original_get

    order_row = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (order_id,)).fetchone()
    fills = connection.execute(
        "SELECT * FROM execution_fills WHERE execution_order_id = ? ORDER BY id",
        (order_id,),
    ).fetchall()
    trade_rows = connection.execute("SELECT * FROM trade_ledger ORDER BY id").fetchall()

    assert first_sync["status"] == "ok", first_sync
    assert second_sync["status"] == "ok", second_sync
    assert order_row["status"] == "executed", dict(order_row)
    assert len(fills) == 2, [dict(row) for row in fills]
    assert abs(float(fills[0]["delta_quantity"]) - 1.0) < 1e-9, dict(fills[0])
    assert abs(float(fills[1]["delta_quantity"]) - 1.0) < 1e-9, dict(fills[1])
    assert abs(float(fills[1]["cumulative_quantity"]) - 2.0) < 1e-9, dict(fills[1])
    assert len(trade_rows) == 2, [dict(row) for row in trade_rows]

    print("Phase 8 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"First sync status: {first_sync['orders'][0]['status']}")
    print(f"Second sync status: {second_sync['orders'][0]['status']}")
    print(f"Recorded fill rows: {len(fills)}")
    print(f"Trade ledger rows: {len(trade_rows)}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
