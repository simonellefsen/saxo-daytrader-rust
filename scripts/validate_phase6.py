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

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
from saxo_daytrader_xai.execution_engine import execute_order
from saxo_daytrader_xai.importer import sync_portfolio
from saxo_daytrader_xai.portfolio import fetch_portfolio_positions
from saxo_daytrader_xai import saxo_openapi


def _insert_live_order(connection, *, symbol: str, quantity: float, price_local: float, currency: str) -> int:
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, mode, status, adapter,
            requested_weight_pct, quantity, price_local, currency, estimated_value_dkk,
            approval_required, request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            None,
            symbol,
            "BUY",
            "live",
            "pending_approval",
            "saxo",
            0.05,
            quantity,
            price_local,
            currency,
            price_local * quantity,
            1,
            json.dumps({"source": "validate_phase6"}, ensure_ascii=False, sort_keys=True),
            None,
            None,
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


class _FakeResponse:
    def __init__(self, payload: dict):
        self._payload = payload

    def raise_for_status(self) -> None:
        return None

    def json(self) -> dict:
        return self._payload


def main() -> int:
    config = load_config(ROOT / "config.yaml")
    db_path = Path("/tmp") / f"saxo_daytrader_phase6_{uuid.uuid4().hex}.db"
    session_path = Path("/tmp") / f"saxo_daytrader_phase6_session_{uuid.uuid4().hex}.json"
    config["portfolio"]["database_path"] = str(db_path)
    config["execution"]["mode"] = "live"
    config["execution"]["adapter"] = "saxo"
    config["execution"]["require_approval_live"] = True
    config["saxo"]["session_path"] = str(session_path)
    config["app"]["dry_run"] = True

    result = sync_portfolio(config)
    connection = connect(config["portfolio"]["database_path"])
    init_db(connection)
    positions = fetch_portfolio_positions(connection)
    position = next(row for row in positions if row["current_price_local"] not in (None, 0))

    approval_order_id = _insert_live_order(
        connection,
        symbol=position["symbol"],
        quantity=1.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
    )
    approval_result = execute_order(approval_order_id, config=config, connection=connection, approved=False)
    assert approval_result["status"] == "approval_required", approval_result

    dry_run_order_id = _insert_live_order(
        connection,
        symbol=position["symbol"],
        quantity=1.0,
        price_local=float(position["current_price_local"]),
        currency=str(position["currency"]),
    )
    dry_run_result = execute_order(dry_run_order_id, config=config, connection=connection, approved=True)
    assert dry_run_result["status"] == "blocked_by_dry_run", dry_run_result

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

    original_get = saxo_openapi.requests.get
    original_post = saxo_openapi.requests.post

    def fake_get(url: str, **kwargs):
        if url.endswith("/ref/v1/instruments"):
            return _FakeResponse(
                {
                    "Data": [
                        {
                            "Identifier": 211,
                            "AssetType": "Stock",
                            "ExchangeId": position["symbol"].split(":", 1)[1].upper(),
                            "Symbol": position["symbol"].split(":", 1)[0].upper(),
                            "Description": position["instrument_name"],
                            "TradableAs": ["Stock"],
                            "CurrencyCode": position["currency"],
                        }
                    ]
                }
            )
        raise AssertionError(f"Unexpected GET {url}")

    def fake_post(url: str, **kwargs):
        if url.endswith("/trade/v2/orders/precheck"):
            return _FakeResponse({"EstimatedTotalCost": 12.34, "OrderId": None})
        if url.endswith("/trade/v2/orders"):
            return _FakeResponse({"OrderId": "SIM-ORDER-123", "Status": "Placed"})
        raise AssertionError(f"Unexpected POST {url}")

    saxo_openapi.requests.get = fake_get
    saxo_openapi.requests.post = fake_post
    try:
        config["app"]["dry_run"] = False
        live_order_id = _insert_live_order(
            connection,
            symbol=position["symbol"],
            quantity=1.0,
            price_local=float(position["current_price_local"]),
            currency=str(position["currency"]),
        )
        live_result = execute_order(live_order_id, config=config, connection=connection, approved=True)
    finally:
        saxo_openapi.requests.get = original_get
        saxo_openapi.requests.post = original_post

    stored_order = connection.execute("SELECT * FROM execution_orders WHERE id = ?", (live_order_id,)).fetchone()
    trade_count = connection.execute("SELECT COUNT(*) AS count_rows FROM trade_ledger").fetchone()["count_rows"]

    assert live_result["status"] == "submitted_to_broker", live_result
    assert stored_order["status"] == "submitted_to_broker", dict(stored_order)
    assert trade_count == 0, "Live submission should not be recorded as an executed fill in trade_ledger"

    print("Phase 6 validation passed.")
    print(f"Imported source positions: {result.source_positions}")
    print(f"Excluded positions: {result.excluded_positions}")
    print(f"Approval status: {approval_result['status']}")
    print(f"Dry-run status: {dry_run_result['status']}")
    print(f"Live submission status: {live_result['status']}")
    print(f"Broker order id: {live_result['broker_result']['OrderId']}")

    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
