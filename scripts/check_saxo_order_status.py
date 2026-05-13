#!/usr/bin/env python3
"""Read-only Saxo order status diagnostic for a local execution order.

The script reads the refreshable Saxo session from Postgres, queries the open
order endpoint and order-activity endpoint, and prints only broker status data.
It intentionally redacts account and client keys from output.
"""

from __future__ import annotations

import argparse
import gzip
import json
import subprocess
import sys
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen


def psql(sql: str) -> str:
    proc = subprocess.run(
        [
            "rtk",
            "kubectl",
            "--context",
            "docker-desktop",
            "-n",
            "saxo",
            "exec",
            "-i",
            "daytrader-postgres-1",
            "--",
            "psql",
            "-U",
            "postgres",
            "-d",
            "daytrader",
            "-v",
            "ON_ERROR_STOP=1",
            "-t",
            "-A",
        ],
        input=sql,
        text=True,
        capture_output=True,
        check=True,
    )
    return proc.stdout.strip()


def fetch_json(sql: str) -> Any:
    out = psql(sql)
    if not out:
        return None
    return json.loads(out.splitlines()[-1])


def openapi_base_url(session: dict[str, Any]) -> str:
    environment = str(session.get("environment") or session.get("configured_environment") or "sim").lower()
    if environment == "live":
        return "https://gateway.saxobank.com/openapi"
    return "https://gateway.saxobank.com/sim/openapi"


def saxo_get(session: dict[str, Any], path: str, params: dict[str, str] | None = None) -> dict[str, Any]:
    token = session.get("access_token")
    if not token:
        return {"http_status": None, "error": "missing access token"}
    url = f"{openapi_base_url(session)}{path}"
    if params:
        url = f"{url}?{urlencode(params)}"
    request = Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
            "Accept-Encoding": "identity",
        },
    )
    try:
        with urlopen(request, timeout=30) as response:
            return {
                "http_status": response.status,
                "payload": json.loads(response.read().decode("utf-8") or "{}"),
            }
    except HTTPError as exc:
        body = exc.read(1600)
        if body.startswith(b"\x1f\x8b"):
            body = gzip.decompress(body)
        return {"http_status": exc.code, "error": body.decode("utf-8", errors="replace")}
    except URLError as exc:
        return {"http_status": None, "error": str(exc.reason)}


def sanitize_open_order(payload: dict[str, Any]) -> dict[str, Any]:
    data = payload.get("payload") if isinstance(payload.get("payload"), dict) else payload.get("payload")
    if not isinstance(data, dict):
        return payload
    keys = [
        "OrderId",
        "Status",
        "SubStatus",
        "BuySell",
        "Amount",
        "FilledAmount",
        "OpenAmount",
        "OrderType",
        "Duration",
        "Uic",
        "AssetType",
        "DisplayAndFormat",
    ]
    return {"http_status": payload.get("http_status"), "payload": {key: data.get(key) for key in keys if key in data}}


def sanitize_activity(payload: dict[str, Any]) -> dict[str, Any]:
    data = payload.get("payload")
    if not isinstance(data, dict):
        return payload
    rows = data.get("Data") or []
    compact = []
    for row in rows[:10]:
        if not isinstance(row, dict):
            continue
        compact.append(
            {
                "OrderId": row.get("OrderId"),
                "Status": row.get("Status"),
                "SubStatus": row.get("SubStatus"),
                "ActivityType": row.get("ActivityType"),
                "BuySell": row.get("BuySell"),
                "Amount": row.get("Amount"),
                "FilledAmount": row.get("FilledAmount"),
                "Price": row.get("Price"),
                "ExecutionPrice": row.get("ExecutionPrice"),
                "ActivityTime": row.get("ActivityTime"),
            }
        )
    return {"http_status": payload.get("http_status"), "payload": {"Data": compact}}


def main() -> int:
    parser = argparse.ArgumentParser(description="Check Saxo broker status for a local execution order.")
    parser.add_argument("execution_order_id", type=int)
    args = parser.parse_args()

    order = fetch_json(
        f"""
        SELECT row_to_json(o)
        FROM execution_orders o
        WHERE id = {args.execution_order_id}
        LIMIT 1;
        """
    )
    if not order:
        raise SystemExit(f"execution order {args.execution_order_id} not found")
    broker_order_id = order.get("broker_order_id")
    if not broker_order_id:
        raise SystemExit(f"execution order {args.execution_order_id} has no broker_order_id")

    session = fetch_json(
        """
        SELECT session_json::json
        FROM saxo_sessions
        WHERE singleton_key IN ('current', 'default')
        ORDER BY CASE WHEN singleton_key = 'current' THEN 0 ELSE 1 END
        LIMIT 1;
        """
    )
    if not session:
        raise SystemExit("No Saxo session found")
    client_key = session.get("client_key") or session.get("ClientKey")
    account_key = session.get("account_key") or session.get("default_account_key")
    if not client_key or not account_key:
        raise SystemExit("Saxo session is missing client/account keys")

    open_order = saxo_get(
        session,
        f"/port/v1/orders/{quote(str(client_key), safe='')}/{quote(str(broker_order_id), safe='')}",
        {"FieldGroups": "DisplayAndFormat"},
    )
    activity = saxo_get(
        session,
        "/cs/v1/audit/orderactivities",
        {
            "AccountKey": str(account_key),
            "ClientKey": str(client_key),
            "EntryType": "Last",
            "OrderId": str(broker_order_id),
        },
    )
    print(
        json.dumps(
            {
                "local_order": {
                    "id": order.get("id"),
                    "symbol": order.get("symbol"),
                    "action": order.get("action"),
                    "status": order.get("status"),
                    "quantity": order.get("quantity"),
                    "broker_order_id": broker_order_id,
                },
                "open_order": sanitize_open_order(open_order),
                "activity": sanitize_activity(activity),
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
