#!/usr/bin/env python3
"""Create an auditable Saxo SIM reconciliation target from the Positioner import.

The script intentionally creates review-only execution orders (`pending_approval`)
instead of executable orders. The Rust scheduler only auto-submits
`pending_execution` orders, so the generated deltas can be inspected before any
broker mutation is allowed.
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlencode
from urllib.request import Request, urlopen


TRADABLE_ASSET_TYPES = "Stock,Etf,Etn,Etc"
INITIAL_CASH_DKK = 9770.17

EXCHANGE_ALIASES: dict[str, list[str]] = {
    "xnas": ["XNAS", "NAS"],
    "xnys": ["XNYS", "NYS"],
    "xcse": ["XCSE", "CSE"],
    "xetr": ["XETR", "FSE"],
    "xfra": ["XFRA", "FSE"],
    "xlon": ["XLON", "LSE"],
    "xwar": ["XWAR", "WSE"],
    "xsto": ["XSTO", "STO"],
    "xosl": ["XOSL", "OSE"],
    "xhel": ["XHEL", "HEL"],
    "xmil": ["XMIL", "MIL"],
    "xbru": ["XBRU", "BRU"],
}

FX_TO_DKK = {
    "DKK": 1.0,
    "EUR": 7.4604,
    "USD": 6.353711419097016,
    "GBP": 8.70,
    "NOK": 0.64,
    "SEK": 0.67,
    "PLN": 1.75,
}


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


def sql_literal(value: Any) -> str:
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    if isinstance(value, (int, float)):
        if isinstance(value, float) and not math.isfinite(value):
            return "NULL"
        return str(value)
    return "'" + str(value).replace("'", "''") + "'"


def sql_json(value: Any) -> str:
    return sql_literal(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


def fetch_json(sql: str) -> Any:
    out = psql(sql)
    if not out:
        return None
    return json.loads(out.splitlines()[-1])


def symbol_parts(symbol: str) -> tuple[str, str]:
    if ":" in symbol:
        base, exchange = symbol.split(":", 1)
    else:
        base, exchange = symbol, ""
    return base.strip().upper(), exchange.strip().lower()


def candidate_matches_requested(candidate: dict[str, Any], requested_symbol: str) -> bool:
    base, exchange = symbol_parts(requested_symbol)
    candidate_symbol = str(candidate.get("Symbol") or "").upper()
    candidate_base = candidate_symbol.split(":", 1)[0]
    if candidate_symbol == requested_symbol.upper():
        return True
    if candidate_base != base:
        return False
    if not exchange:
        return True
    candidate_exchange = str(candidate.get("ExchangeId") or "").upper()
    aliases = EXCHANGE_ALIASES.get(exchange, [])
    return candidate_exchange in aliases or candidate_symbol.endswith(f":{exchange.upper()}")


def candidate_score(candidate: dict[str, Any], requested_symbol: str) -> tuple[int, int, int]:
    base, exchange = symbol_parts(requested_symbol)
    candidate_symbol = str(candidate.get("Symbol") or "").upper()
    candidate_exchange = str(candidate.get("ExchangeId") or "").upper()
    exact_symbol = int(candidate_symbol == requested_symbol.upper())
    exchange_match = int(
        candidate_exchange in EXCHANGE_ALIASES.get(exchange, [])
        or candidate_symbol.endswith(f":{exchange.upper()}")
    )
    exact_base = int(candidate_symbol.split(":", 1)[0] == base)
    stock_preferred = int("Stock" in (candidate.get("TradableAs") or []))
    return exact_symbol, exchange_match, exact_base + stock_preferred


def openapi_base_url(session: dict[str, Any]) -> str:
    environment = str(session.get("environment") or session.get("configured_environment") or "sim").lower()
    if environment == "live":
        return "https://gateway.saxobank.com/openapi"
    return "https://gateway.saxobank.com/sim/openapi"


def saxo_lookup(session: dict[str, Any], symbol: str, isin: str | None = None) -> dict[str, Any]:
    access_token = session.get("access_token")
    account_key = session.get("account_key") or session.get("default_account_key")
    if not access_token or not account_key:
        return {"status": "invalid", "error": "Saxo session is missing access token or account key"}
    base, exchange = symbol_parts(symbol)
    attempts: list[tuple[str, dict[str, str], bool]] = [
        ("symbol", {"Keywords": symbol}, True),
        ("base_exchange", {"Keywords": base, "ExchangeId": exchange.upper()}, True),
    ]
    if isin:
        # Saxo platform exports can use one venue-specific display symbol while
        # OpenAPI exposes the same ISIN under another Saxo symbol. The ISIN query
        # is therefore allowed to return a symbol mismatch, but only after the
        # stricter symbol attempts have failed.
        attempts.append(("isin", {"Keywords": isin}, False))
    attempts.append(("base", {"Keywords": base}, True))

    last_candidates: list[dict[str, Any]] = []
    selected: dict[str, Any] | None = None
    selected_method = ""
    for method, extra_params, require_symbol_match in attempts:
        params = urlencode(
            {
                "$top": "50",
                "AccountKey": account_key,
                "AssetTypes": TRADABLE_ASSET_TYPES,
                "IncludeNonTradable": "false",
                **extra_params,
            }
        )
        request = Request(
            f"{openapi_base_url(session)}/ref/v1/instruments?{params}",
            headers={"Authorization": f"Bearer {access_token}", "Accept": "application/json"},
        )
        try:
            with urlopen(request, timeout=30) as response:
                payload = json.loads(response.read().decode("utf-8") or "{}")
        except Exception as exc:  # noqa: BLE001 - emit a data error, not a traceback.
            return {"status": "invalid", "error": f"Saxo lookup failed: {exc}"}
        candidates = payload.get("Data") or []
        last_candidates = candidates
        matches = (
            [candidate for candidate in candidates if candidate_matches_requested(candidate, symbol)]
            if require_symbol_match
            else candidates
        )
        if matches:
            selected = sorted(matches, key=lambda candidate: candidate_score(candidate, symbol), reverse=True)[0]
            selected_method = method
            break
    if selected is None:
        return {
            "status": "invalid",
            "error": "No strict Saxo symbol or ISIN match",
            "candidate_count": len(last_candidates),
            "candidate_symbols": [candidate.get("Symbol") for candidate in last_candidates[:5]],
        }
    return {
        "status": "valid",
        "uic": selected.get("Identifier"),
        "asset_type": selected.get("AssetType") or "Stock",
        "saxo_symbol": selected.get("Symbol"),
        "exchange_id": selected.get("ExchangeId"),
        "description": selected.get("Description"),
        "match_method": selected_method,
        "requested_symbol": symbol,
        "requested_isin": isin,
    }


def load_inputs() -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, dict[str, Any]], dict[str, Any]]:
    payload = fetch_json(
        """
        WITH latest AS (
            SELECT batch_id, source_csv
            FROM import_batches
            WHERE source_csv LIKE '%Positioner_05-maj-2026_05_41_51.csv'
            ORDER BY imported_at DESC, batch_id DESC
            LIMIT 1
        )
        SELECT json_build_object(
            'batch', (SELECT row_to_json(latest) FROM latest),
            'target_rows', (
                SELECT COALESCE(json_agg(row_to_json(p) ORDER BY p.symbol), '[]'::json)
                FROM position_snapshots p
                WHERE p.batch_id = (SELECT batch_id FROM latest)
                  AND COALESCE(p.excluded, 0) = 0
                  AND upper(split_part(p.symbol, ':', 1)) NOT IN ('NOVO', 'NOVOB', 'TSLA')
            ),
            'broker_rows', (
                SELECT COALESCE(json_agg(row_to_json(b) ORDER BY b.symbol), '[]'::json)
                FROM broker_position_snapshots b
            ),
            'session', (
                SELECT session_json::json
                FROM saxo_sessions
                WHERE singleton_key IN ('current', 'default')
                ORDER BY CASE WHEN singleton_key = 'current' THEN 0 ELSE 1 END
                LIMIT 1
            )
        )::text;
        """
    )
    if not payload or not payload.get("batch"):
        raise SystemExit("No Positioner_05-maj-2026_05_41_51.csv import batch found")
    target_rows = payload["target_rows"]
    broker_by_symbol = {str(row["symbol"]): row for row in payload["broker_rows"]}
    return payload["batch"], target_rows, broker_by_symbol, payload["session"]


def ensure_tables() -> None:
    psql(
        """
        CREATE TABLE IF NOT EXISTS reconciliation_runs (
            run_id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            source_batch_id TEXT NOT NULL,
            source_csv TEXT,
            status TEXT NOT NULL,
            cash_reset_amount_dkk REAL NOT NULL DEFAULT 0,
            raw_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS reconciliation_target_items (
            run_id TEXT NOT NULL,
            symbol TEXT NOT NULL,
            instrument_name TEXT,
            target_quantity REAL NOT NULL,
            broker_quantity REAL NOT NULL,
            delta_quantity REAL NOT NULL,
            action TEXT,
            order_quantity REAL NOT NULL,
            currency TEXT,
            price_local REAL,
            estimated_value_dkk REAL,
            validation_status TEXT NOT NULL,
            validation_error TEXT,
            saxo_uic INTEGER,
            saxo_asset_type TEXT,
            saxo_symbol TEXT,
            saxo_exchange TEXT,
            raw_json TEXT NOT NULL,
            PRIMARY KEY (run_id, symbol)
        );
        """
    )


def reset_cash(run_id: str, created_at: str) -> float:
    cash_from_trades = float(
        fetch_json(
            """
            SELECT json_build_object(
                'cash_from_trades', COALESCE(SUM(net_amount_dkk), 0)
            )::text
            FROM trade_ledger
            WHERE status IN ('executed', 'approved');
            """
        )["cash_from_trades"]
    )
    if abs(cash_from_trades) < 0.005:
        return 0.0
    reset_amount = -cash_from_trades
    psql(
        f"""
        INSERT INTO trade_ledger (
            created_at, symbol, isin, figi, instrument_name, side, quantity,
            price_local, currency, gross_amount_dkk, commission_dkk, tax_dkk,
            net_amount_dkk, mode, status, notes, portfolio_before_json,
            portfolio_after_json, decision_context_json, commission_local,
            fx_conversion_dkk, realised_gain_dkk, cost_basis_sold_dkk,
            cost_basis_sold_local, realised_gain_local, fx_gain_dkk,
            price_gain_dkk, sale_fx_rate_to_dkk, cost_basis_fx_rate_to_dkk,
            tax_year, batch_id, environment_id, account_uid
        )
        SELECT
            {sql_literal(created_at)}, 'CASH:DKK', NULL, NULL, 'Strategy cash reset',
            'ADJUSTMENT', 0, 0, 'DKK', 0, 0, 0, {reset_amount},
            'live', 'executed',
            {sql_literal(f'Clean reconciliation cash reset to {INITIAL_CASH_DKK:.2f} DKK; run {run_id}')},
            '{{}}', '{{}}', {sql_json({'run_id': run_id, 'reason': 'reset strategy cash to Positioner baseline'})},
            0, 1, 0, 0, 0, 0, 0, 0, 1, 1, 2026, NULL, 'SIM', 'SIM_DEFAULT'
        WHERE NOT EXISTS (
            SELECT 1 FROM trade_ledger
            WHERE notes = {sql_literal(f'Clean reconciliation cash reset to {INITIAL_CASH_DKK:.2f} DKK; run {run_id}')}
        );
        """
    )
    return reset_amount


def main() -> int:
    ensure_tables()
    batch, target_rows, broker_by_symbol, session = load_inputs()
    run_id = "clean_reconcile_" + datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    psql(
        f"""
        UPDATE execution_orders
        SET status = 'cancelled',
            error_text = {sql_literal(f'Superseded by reconciliation run {run_id}')}
        WHERE strategy_type = 'clean_reconciliation'
          AND status IN ('pending_approval', 'pending_execution', 'waiting_for_market_open', 'waiting_for_virtual_cash_budget');
        """
    )
    cash_reset_amount = reset_cash(run_id, created_at)

    target_by_symbol = {str(row["symbol"]): row for row in target_rows}
    all_symbols = sorted(set(target_by_symbol) | set(broker_by_symbol))
    items: list[dict[str, Any]] = []
    order_values: list[str] = []
    for symbol in all_symbols:
        target = target_by_symbol.get(symbol)
        broker = broker_by_symbol.get(symbol)
        target_qty = float((target or {}).get("quantity") or 0.0)
        broker_qty = float((broker or {}).get("quantity") or 0.0)
        delta = target_qty - broker_qty
        order_qty = math.floor(abs(delta) + 1e-9)
        action = "BUY" if delta > 0 else "SELL" if delta < 0 else None
        source = target or broker or {}
        currency = str(source.get("currency") or "DKK")
        if target and target_qty > 0:
            price_local = float(target.get("current_price_local") or 0.0)
            estimated_value_dkk = float(target.get("market_value_dkk") or 0.0) * (order_qty / target_qty)
        else:
            price_local = float((broker or {}).get("open_price_local") or 0.0)
            estimated_value_dkk = price_local * order_qty * FX_TO_DKK.get(currency.upper(), 1.0)
        validation = saxo_lookup(session, symbol, source.get("isin")) if order_qty > 0 else {"status": "aligned"}
        item = {
            "run_id": run_id,
            "symbol": symbol,
            "instrument_name": source.get("instrument_name"),
            "target_quantity": target_qty,
            "broker_quantity": broker_qty,
            "delta_quantity": delta,
            "action": action,
            "order_quantity": order_qty,
            "currency": currency,
            "price_local": price_local,
            "estimated_value_dkk": estimated_value_dkk,
            "validation": validation,
        }
        items.append(item)
        order_values.append(
            "("
            + ",".join(
                [
                    sql_literal(run_id),
                    sql_literal(symbol),
                    sql_literal(item["instrument_name"]),
                    sql_literal(target_qty),
                    sql_literal(broker_qty),
                    sql_literal(delta),
                    sql_literal(action),
                    sql_literal(order_qty),
                    sql_literal(currency),
                    sql_literal(price_local),
                    sql_literal(estimated_value_dkk),
                    sql_literal(validation["status"]),
                    sql_literal(validation.get("error")),
                    sql_literal(validation.get("uic")),
                    sql_literal(validation.get("asset_type")),
                    sql_literal(validation.get("saxo_symbol")),
                    sql_literal(validation.get("exchange_id")),
                    sql_json(item),
                ]
            )
            + ")"
        )

    raw = {
        "source_batch_id": batch["batch_id"],
        "source_csv": batch["source_csv"],
        "excluded": ["NOVO*", "TSLA*"],
        "cash_reset_amount_dkk": cash_reset_amount,
    }
    psql(
        f"""
        INSERT INTO reconciliation_runs (
            run_id, created_at, source_batch_id, source_csv, status, cash_reset_amount_dkk, raw_json
        ) VALUES (
            {sql_literal(run_id)}, {sql_literal(created_at)}, {sql_literal(batch['batch_id'])},
            {sql_literal(batch['source_csv'])}, 'generated_review_only',
            {sql_literal(cash_reset_amount)}, {sql_json(raw)}
        );
        INSERT INTO reconciliation_target_items (
            run_id, symbol, instrument_name, target_quantity, broker_quantity,
            delta_quantity, action, order_quantity, currency, price_local,
            estimated_value_dkk, validation_status, validation_error, saxo_uic,
            saxo_asset_type, saxo_symbol, saxo_exchange, raw_json
        ) VALUES
        {",".join(order_values)};
        """
    )

    executable_items = [
        item
        for item in items
        if item["order_quantity"] > 0 and item["validation"]["status"] == "valid"
    ]
    executable_items.sort(key=lambda item: 0 if item["action"] == "SELL" else 1)
    order_rows: list[str] = []
    for item in executable_items:
        strategy_key = f"clean_reconciliation:{run_id}:{item['symbol']}"
        request_payload = {
            "run_id": run_id,
            "source_csv": batch["source_csv"],
            "symbol": item["symbol"],
            "action": item["action"],
            "target_quantity": item["target_quantity"],
            "broker_quantity": item["broker_quantity"],
            "delta_quantity": item["delta_quantity"],
            "validation": item["validation"],
            "review_required": True,
        }
        order_rows.append(
            "("
            + ",".join(
                [
                    sql_literal(created_at),
                    "NULL",
                    sql_literal(item["symbol"]),
                    sql_literal(item["action"]),
                    "'Market'",
                    "'live'",
                    "'pending_approval'",
                    "'saxo'",
                    "NULL",
                    sql_literal(item["order_quantity"]),
                    sql_literal(item["price_local"]),
                    "NULL",
                    "NULL",
                    sql_literal(item["currency"]),
                    sql_literal(item["estimated_value_dkk"]),
                    "1",
                    "NULL",
                    "'clean_reconciliation'",
                    "'saxo_sim_review'",
                    sql_literal(strategy_key),
                    "'increase_to_target'" if item["action"] == "BUY" else "'reduce_to_target'",
                    sql_json(request_payload),
                    "NULL",
                    "NULL",
                ]
            )
            + ")"
        )
    if order_rows:
        psql(
            f"""
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local,
                currency, estimated_value_dkk, approval_required, parent_execution_order_id,
                strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES
            {",".join(order_rows)};
            """
        )

    invalid = [item for item in items if item["order_quantity"] > 0 and item["validation"]["status"] != "valid"]
    result = {
        "run_id": run_id,
        "source_csv": batch["source_csv"],
        "target_count": len(target_rows),
        "delta_count": sum(1 for item in items if item["order_quantity"] > 0),
        "review_order_count": len(executable_items),
        "invalid_delta_count": len(invalid),
        "cash_reset_amount_dkk": cash_reset_amount,
        "invalid_symbols": [
            {
                "symbol": item["symbol"],
                "action": item["action"],
                "quantity": item["order_quantity"],
                "error": item["validation"].get("error"),
                "candidates": item["validation"].get("candidate_symbols"),
            }
            for item in invalid
        ],
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
