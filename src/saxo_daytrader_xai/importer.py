from __future__ import annotations

import csv
import json
import uuid
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from saxo_daytrader_xai.db import append_audit_log, connect, init_db
from saxo_daytrader_xai.portfolio import record_portfolio_value_snapshot


@dataclass(frozen=True)
class ImportResult:
    batch_id: str
    source_csv: str
    source_positions: int
    imported_positions: int
    excluded_positions: int


def _parse_float(raw: str | None, decimal_style: str = "auto") -> float | None:
    if raw is None:
        return None
    text = raw.strip().replace("\xa0", " ")
    if not text:
        return None
    text = text.replace("%", "").strip()
    if decimal_style == "dk":
        text = text.replace(".", "").replace(",", ".")
    elif decimal_style == "en":
        text = text.replace(",", "")
    else:
        if "," in text and "." in text:
            if text.rfind(",") > text.rfind("."):
                text = text.replace(".", "").replace(",", ".")
            else:
                text = text.replace(",", "")
        elif "," in text:
            text = text.replace(".", "").replace(",", ".")
    try:
        return float(text)
    except ValueError:
        return None


def _parse_money_with_currency(raw: str | None) -> tuple[float | None, str | None]:
    if raw is None:
        return None, None
    parts = raw.strip().split()
    if not parts:
        return None, None
    amount = _parse_float(parts[0], decimal_style="dk")
    currency = parts[1] if len(parts) > 1 else None
    return amount, currency


def _normalise_text(raw: str | None) -> str:
    return (raw or "").strip()


def _normalise_asset_class(raw: str | None) -> str:
    text = _normalise_text(raw)
    normalized = text.casefold()
    mapping = {
        "aktie": "Equity",
        "aktier": "Equity",
        "equity": "Equity",
        "stock": "Equity",
        "stocks": "Equity",
    }
    return mapping.get(normalized, text)


def _load_detail_rows(source_csv: Path) -> list[dict[str, str]]:
    with source_csv.open("r", encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        return [row for row in reader if row.get("L/K") == "Lang" and row.get("Status") == "Åben"]


def _build_snapshot_row(row: dict[str, str], source_csv: str, excluded_symbols: set[str]) -> dict[str, Any]:
    symbol = _normalise_text(row.get("Symbol"))
    market_value_local, _ = _parse_money_with_currency(row.get("Markedsværdi"))
    return {
        "instrument_name": _normalise_text(row.get("Instrument")),
        "symbol": symbol,
        "isin": _normalise_text(row.get("ISIN")),
        "quantity": _parse_float(row.get("Antal"), decimal_style="en") or 0.0,
        "currency": _normalise_text(row.get("Valuta")),
        "open_price_local": _parse_float(row.get("Åbningskurs"), decimal_style="en"),
        "current_price_local": _parse_float(row.get("Aktuel kurs"), decimal_style="en"),
        "cost_basis_local": _parse_float(row.get("Kostpris"), decimal_style="en"),
        "cost_basis_dkk": _parse_float(row.get("Oprindelig værdi (DKK)"), decimal_style="en") or 0.0,
        "market_value_local": market_value_local,
        "market_value_dkk": _parse_float(row.get("Markedsværdi (DKK)"), decimal_style="en") or 0.0,
        "unrealised_pnl_dkk": _parse_float(row.get("Gevinst/Tab i alt (DKK)"), decimal_style="en") or 0.0,
        "daily_pnl_dkk": _parse_float(row.get("1-dags gevinst/tab (DKK)"), decimal_style="en"),
        "allocation_pct": _parse_float(row.get("% af portefølje"), decimal_style="en"),
        "status": _normalise_text(row.get("Status")),
        "account_name": _normalise_text(row.get("Konto")),
        "asset_class": _normalise_asset_class(row.get("Aktivtype") or row.get("Aktivklasse")),
        "market_status": _normalise_text(row.get("Markedsstatus")),
        "value_date": _normalise_text(row.get("Valørdato")),
        "source_csv": source_csv,
        "excluded": 1 if symbol in excluded_symbols else 0,
        "exclusion_reason": "Configured excluded symbol" if symbol in excluded_symbols else None,
        "raw_payload_json": json.dumps(row, ensure_ascii=False, sort_keys=True),
    }


def _infer_fx_rate_to_dkk(snapshot_row: dict[str, Any]) -> float:
    currency = snapshot_row["currency"]
    if currency == "DKK":
        return 1.0
    market_value_local = snapshot_row.get("market_value_local")
    market_value_dkk = snapshot_row.get("market_value_dkk")
    if market_value_local not in (None, 0) and market_value_dkk not in (None, 0):
        return float(market_value_dkk) / float(market_value_local)
    quantity = snapshot_row.get("quantity")
    cost_basis_local = snapshot_row.get("cost_basis_local")
    cost_basis_dkk = snapshot_row.get("cost_basis_dkk")
    local_total = (quantity or 0.0) * (cost_basis_local or 0.0)
    if local_total not in (None, 0) and cost_basis_dkk not in (None, 0):
        return float(cost_basis_dkk) / float(local_total)
    return 1.0


def _build_lot_row(batch_id: str, imported_at: str, snapshot_row: dict[str, Any]) -> dict[str, Any]:
    return {
        "lot_id": f"{batch_id}:{snapshot_row['symbol']}",
        "batch_id": batch_id,
        "created_at": imported_at,
        "acquired_at": snapshot_row.get("value_date") or imported_at,
        "symbol": snapshot_row["symbol"],
        "isin": snapshot_row["isin"],
        "instrument_name": snapshot_row["instrument_name"],
        "quantity_original": snapshot_row["quantity"],
        "currency": snapshot_row["currency"],
        "cost_basis_total_local": (snapshot_row["quantity"] or 0.0) * (snapshot_row["cost_basis_local"] or 0.0),
        "cost_basis_total_dkk": snapshot_row["cost_basis_dkk"],
        "fx_rate_to_dkk": _infer_fx_rate_to_dkk(snapshot_row),
        "source_type": "csv_import",
        "source_reference": snapshot_row["source_csv"],
        "raw_payload_json": snapshot_row["raw_payload_json"],
    }


def sync_portfolio(config: dict[str, Any]) -> ImportResult:
    portfolio_cfg = config["portfolio"]
    source_csv_value = str(portfolio_cfg.get("source_csv", "") or "").strip()
    source_csv = Path(source_csv_value).resolve() if source_csv_value else None
    database_path_value = str(portfolio_cfg["database_path"])
    database_path: str | Path = (
        database_path_value
        if database_path_value.startswith(("postgresql://", "postgres://"))
        else Path(database_path_value).resolve()
    )
    excluded_symbols = set(config.get("risk", {}).get("excluded_symbols", []))
    detail_rows = _load_detail_rows(source_csv) if source_csv is not None else []
    batch_id = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    imported_at = datetime.now(UTC).isoformat(timespec="seconds")
    source_reference = str(source_csv) if source_csv is not None else ""
    snapshot_rows = [_build_snapshot_row(row, source_reference, excluded_symbols) for row in detail_rows]
    excluded_positions = sum(row["excluded"] for row in snapshot_rows)
    imported_positions = len(snapshot_rows) - excluded_positions
    active_lot_rows = [_build_lot_row(batch_id, imported_at, row) for row in snapshot_rows if not row["excluded"]]

    connection = connect(database_path)
    init_db(connection)
    connection.execute(
        """
        INSERT INTO import_batches (
            batch_id,
            imported_at,
            source_csv,
            source_position_count,
            imported_position_count,
            excluded_position_count,
            notes
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (
            batch_id,
            imported_at,
            source_reference,
            len(detail_rows),
            imported_positions,
            excluded_positions,
            "Empty baseline import" if source_csv is None else "Phase 1 CSV import",
        ),
    )
    connection.executemany(
        """
        INSERT INTO position_snapshots (
            batch_id,
            imported_at,
            instrument_name,
            symbol,
            isin,
            quantity,
            currency,
            open_price_local,
            current_price_local,
            cost_basis_local,
            cost_basis_dkk,
            market_value_local,
            market_value_dkk,
            unrealised_pnl_dkk,
            daily_pnl_dkk,
            allocation_pct,
            status,
            account_name,
            asset_class,
            market_status,
            value_date,
            source_csv,
            excluded,
            exclusion_reason,
            raw_payload_json
        ) VALUES (
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
        )
        """,
        [
            (
                batch_id,
                imported_at,
                row["instrument_name"],
                row["symbol"],
                row["isin"],
                row["quantity"],
                row["currency"],
                row["open_price_local"],
                row["current_price_local"],
                row["cost_basis_local"],
                row["cost_basis_dkk"],
                row["market_value_local"],
                row["market_value_dkk"],
                row["unrealised_pnl_dkk"],
                row["daily_pnl_dkk"],
                row["allocation_pct"],
                row["status"],
                row["account_name"],
                row["asset_class"],
                row["market_status"],
                row["value_date"],
                row["source_csv"],
                row["excluded"],
                row["exclusion_reason"],
                row["raw_payload_json"],
            )
            for row in snapshot_rows
        ],
    )
    connection.executemany(
        """
        INSERT INTO position_lots (
            lot_id,
            batch_id,
            created_at,
            acquired_at,
            symbol,
            isin,
            instrument_name,
            quantity_original,
            currency,
            cost_basis_total_local,
            cost_basis_total_dkk,
            fx_rate_to_dkk,
            source_type,
            source_reference,
            raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                row["lot_id"],
                row["batch_id"],
                row["created_at"],
                row["acquired_at"],
                row["symbol"],
                row["isin"],
                row["instrument_name"],
                row["quantity_original"],
                row["currency"],
                row["cost_basis_total_local"],
                row["cost_basis_total_dkk"],
                row["fx_rate_to_dkk"],
                row["source_type"],
                row["source_reference"],
                row["raw_payload_json"],
            )
            for row in active_lot_rows
        ],
    )
    connection.commit()

    record_portfolio_value_snapshot(
        connection,
        recorded_at=imported_at,
        snapshot_type="import",
        initial_cash_dkk=float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0),
        batch_id=batch_id,
        source="csv_import",
        extra_payload={
            "source_positions": len(detail_rows),
            "imported_positions": imported_positions,
            "excluded_positions": excluded_positions,
        },
    )

    append_audit_log(
        connection,
        "portfolio_import",
        {
            "batch_id": batch_id,
            "source_csv": source_reference,
            "source_positions": len(detail_rows),
            "imported_positions": imported_positions,
            "excluded_positions": excluded_positions,
            "excluded_symbols": sorted(excluded_symbols),
            "created_lots": len(active_lot_rows),
        },
    )
    connection.close()
    return ImportResult(
        batch_id=batch_id,
        source_csv=source_reference,
        source_positions=len(detail_rows),
        imported_positions=imported_positions,
        excluded_positions=excluded_positions,
    )
