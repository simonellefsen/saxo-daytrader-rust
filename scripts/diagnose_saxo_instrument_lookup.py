#!/usr/bin/env python3
"""Capture Saxo OpenAPI instrument lookup candidates for timing diagnostics.

This script is intentionally read-only against Saxo. It records how the
reference-data lookup behaves for exact symbols, base ticker + exchange,
ISIN-based lookup, and non-tradable variants. That makes it possible to compare
morning, US pre-open, and US-open runs without exposing access tokens or
account keys in output.
"""

from __future__ import annotations

import argparse
import gzip
import json
import sys
import time
from datetime import UTC, datetime
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from create_clean_reconciliation_target import (
    EXCHANGE_ALIASES,
    TRADABLE_ASSET_TYPES,
    fetch_json,
    openapi_base_url,
    psql,
    sql_json,
    sql_literal,
    symbol_parts,
)


# These are the symbols that differed between the Saxo platform export and the
# SIM OpenAPI reference lookup during the clean reconciliation rebuild.
DEFAULT_SYMBOLS = {
    "ARKI:xlon": "IE0003A512E4",
    "FIGR:xnas": "US3493811034",
    "PLTR:xnas": "US69608A1088",
    "QOMP:xetr": "IE000C6ITGC8",
    "ARKK:xmil": "IE000GA3D489",
}


def load_session() -> dict[str, Any]:
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
        raise SystemExit("No Saxo session found in saxo_sessions")
    if not session.get("access_token"):
        raise SystemExit("Saxo session is missing an access token")
    return session


def parse_symbol_specs(specs: list[str]) -> dict[str, str | None]:
    symbols: dict[str, str | None] = {}
    for spec in specs:
        if "=" in spec:
            symbol, isin = spec.split("=", 1)
            symbols[symbol.strip()] = isin.strip() or None
        else:
            symbols[spec.strip()] = None
    return {symbol: isin for symbol, isin in symbols.items() if symbol}


def compact_candidate(candidate: dict[str, Any]) -> dict[str, Any]:
    """Keep enough Saxo metadata for diagnosis without storing raw payloads."""
    return {
        "symbol": candidate.get("Symbol"),
        "description": candidate.get("Description"),
        "identifier": candidate.get("Identifier"),
        "asset_type": candidate.get("AssetType"),
        "exchange_id": candidate.get("ExchangeId"),
        "currency_code": candidate.get("CurrencyCode"),
        "tradable_as": candidate.get("TradableAs"),
        "is_tradable": candidate.get("IsTradable"),
    }


def symbol_matches_exchange(candidate: dict[str, Any], requested_symbol: str) -> bool:
    base, exchange = symbol_parts(requested_symbol)
    candidate_symbol = str(candidate.get("Symbol") or "").upper()
    candidate_base = candidate_symbol.split(":", 1)[0]
    if candidate_symbol == requested_symbol.upper():
        return True
    if candidate_base != base:
        return False
    aliases = EXCHANGE_ALIASES.get(exchange, [])
    candidate_exchange = str(candidate.get("ExchangeId") or "").upper()
    return bool(exchange) and (
        candidate_exchange in aliases or candidate_symbol.endswith(f":{exchange.upper()}")
    )


def query_instruments(
    session: dict[str, Any],
    params: dict[str, str],
) -> tuple[int | None, list[dict[str, Any]], str | None]:
    access_token = str(session["access_token"])
    url = f"{openapi_base_url(session)}/ref/v1/instruments?{urlencode(params)}"
    request = Request(
        url,
        headers={
            "Authorization": f"Bearer {access_token}",
            "Accept": "application/json",
            "Accept-Encoding": "identity",
        },
    )
    for attempt in range(2):
        try:
            with urlopen(request, timeout=30) as response:
                payload = json.loads(response.read().decode("utf-8") or "{}")
                return response.status, payload.get("Data") or [], None
        except HTTPError as exc:
            body = exc.read(1000)
            if body.startswith(b"\x1f\x8b"):
                body = gzip.decompress(body)
            error_text = body.decode("utf-8", errors="replace")
            if exc.code == 429 and attempt == 0:
                retry_after = exc.headers.get("Retry-After")
                time.sleep(float(retry_after or 5))
                continue
            return exc.code, [], error_text
        except URLError as exc:
            return None, [], str(exc.reason)
        except Exception as exc:  # noqa: BLE001 - diagnostic should report failures as data.
            return None, [], str(exc)
    return None, [], "Saxo lookup retry loop exited unexpectedly"


def build_attempts(symbol: str, isin: str | None) -> list[tuple[str, dict[str, str]]]:
    base, exchange = symbol_parts(symbol)
    attempts = [
        ("exact_symbol", {"Keywords": symbol}),
        ("base_exchange", {"Keywords": base, "ExchangeId": exchange.upper()}),
        ("base_only", {"Keywords": base}),
    ]
    if isin:
        attempts.insert(2, ("isin", {"Keywords": isin}))
    return attempts


def diagnose_symbol(
    session: dict[str, Any],
    symbol: str,
    isin: str | None,
    sleep_seconds: float,
    include_unscoped: bool,
) -> dict[str, Any]:
    account_key = session.get("account_key") or session.get("default_account_key")
    attempts: list[dict[str, Any]] = []
    for label, extra in build_attempts(symbol, isin):
        for include_non_tradable in (False, True):
            for account_scoped in ([True, False] if include_unscoped else [True]):
                if attempts:
                    time.sleep(max(sleep_seconds, 0.0))
                params = {
                    "$top": "25",
                    "AssetTypes": TRADABLE_ASSET_TYPES,
                    "IncludeNonTradable": str(include_non_tradable).lower(),
                    **extra,
                }
                if account_scoped and account_key:
                    params["AccountKey"] = str(account_key)
                status, candidates, error = query_instruments(session, params)
                compact = [compact_candidate(candidate) for candidate in candidates[:10]]
                attempts.append(
                    {
                        "attempt": label,
                        "include_non_tradable": include_non_tradable,
                        "account_scoped": account_scoped,
                        "status": status,
                        "candidate_count": len(candidates),
                        "matching_symbols": [
                            candidate.get("Symbol")
                            for candidate in candidates
                            if symbol_matches_exchange(candidate, symbol)
                        ],
                        "candidates": compact,
                        "error": error,
                    }
                )
    return {"symbol": symbol, "isin": isin, "attempts": attempts}


def ensure_table() -> None:
    psql(
        """
        CREATE TABLE IF NOT EXISTS saxo_instrument_lookup_diagnostics (
            id INTEGER GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
            run_id TEXT NOT NULL,
            captured_at TEXT NOT NULL,
            environment TEXT NOT NULL,
            symbols_json TEXT NOT NULL,
            result_json TEXT NOT NULL
        );
        """
    )


def persist_report(report: dict[str, Any]) -> None:
    ensure_table()
    psql(
        f"""
        INSERT INTO saxo_instrument_lookup_diagnostics (
            run_id, captured_at, environment, symbols_json, result_json
        ) VALUES (
            {sql_literal(report["run_id"])},
            {sql_literal(report["captured_at"])},
            {sql_literal(report["environment"])},
            {sql_json(report["symbols"])},
            {sql_json(report)}
        );
        """
    )


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    rows = []
    for result in report["results"]:
        non_empty = [
            attempt
            for attempt in result["attempts"]
            if attempt["candidate_count"] > 0 or attempt.get("error")
        ]
        rows.append(
            {
                "symbol": result["symbol"],
                "isin": result["isin"],
                "non_empty_attempts": [
                    {
                        "attempt": attempt["attempt"],
                        "include_non_tradable": attempt["include_non_tradable"],
                        "account_scoped": attempt["account_scoped"],
                        "candidate_count": attempt["candidate_count"],
                        "matching_symbols": attempt["matching_symbols"],
                        "first_candidates": [
                            candidate.get("symbol") for candidate in attempt["candidates"][:3]
                        ],
                        "error": attempt.get("error"),
                    }
                    for attempt in non_empty
                ],
            }
        )
    return {
        "run_id": report["run_id"],
        "captured_at": report["captured_at"],
        "environment": report["environment"],
        "symbols": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Diagnose Saxo ref/v1/instruments lookup candidates for exported symbols."
    )
    parser.add_argument(
        "symbols",
        nargs="*",
        help="Optional SYMBOL or SYMBOL=ISIN values. Defaults to the current unresolved/remapped symbols.",
    )
    parser.add_argument(
        "--no-db",
        action="store_true",
        help="Print the diagnostic only; do not persist it in Postgres.",
    )
    parser.add_argument(
        "--full",
        action="store_true",
        help="Print the full report instead of the compact summary.",
    )
    parser.add_argument(
        "--sleep-seconds",
        type=float,
        default=0.75,
        help="Delay between Saxo reference-data requests. Defaults to 0.75s to avoid 429s.",
    )
    parser.add_argument(
        "--include-unscoped",
        action="store_true",
        help="Also run lookup attempts without AccountKey. This doubles request volume.",
    )
    args = parser.parse_args()

    symbols = parse_symbol_specs(args.symbols) if args.symbols else dict(DEFAULT_SYMBOLS)
    session = load_session()
    captured_at = datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    report = {
        "run_id": f"saxo_lookup_{captured_at.replace('-', '').replace(':', '').replace('Z', '')}",
        "captured_at": captured_at,
        "environment": str(session.get("environment") or session.get("configured_environment") or "sim"),
        "account_key_present": bool(session.get("account_key") or session.get("default_account_key")),
        "symbols": symbols,
        "results": [
            diagnose_symbol(session, symbol, isin, args.sleep_seconds, args.include_unscoped)
            for symbol, isin in sorted(symbols.items())
            if symbol
        ],
    }
    if not args.no_db:
        persist_report(report)
    print(json.dumps(report if args.full else summarize(report), indent=2, sort_keys=True, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
