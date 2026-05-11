from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import append_audit_log, connect, init_db
from saxo_daytrader_xai.market_symbols import parse_exchange_code
from saxo_daytrader_xai.portfolio import fetch_latest_batch_id, fetch_portfolio_positions, fetch_portfolio_summary


def _load_default_config() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]
    return load_config(root / "config.yaml")


def _get_connection_and_config(
    config: dict[str, Any] | None,
    connection,
):
    resolved_config = config or _load_default_config()
    resolved_connection = connection or connect(resolved_config["portfolio"]["database_path"])
    init_db(resolved_connection)
    return resolved_config, resolved_connection, connection is None


def _tax_due_for_share_income(income_dkk: float, brackets: list[dict[str, Any]]) -> float:
    taxable_income = max(income_dkk, 0.0)
    tax_due = 0.0
    lower_bound = 0.0
    for bracket in brackets:
        upper_raw = bracket.get("up_to_dkk")
        upper_bound = float(upper_raw) if upper_raw not in (None, "") else None
        rate = float(bracket["rate"])
        if upper_bound is None:
            tax_due += max(taxable_income - lower_bound, 0.0) * rate
            break
        taxable_slice = max(min(taxable_income, upper_bound) - lower_bound, 0.0)
        tax_due += taxable_slice * rate
        lower_bound = upper_bound
        if taxable_income <= lower_bound:
            break
    return tax_due


def _fetch_open_lots(connection, symbol: str) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            pl.lot_id,
            pl.batch_id,
            pl.created_at,
            pl.acquired_at,
            pl.symbol,
            pl.isin,
            pl.instrument_name,
            pl.quantity_original,
            pl.currency,
            pl.cost_basis_total_local,
            pl.cost_basis_total_dkk,
            pl.fx_rate_to_dkk,
            pl.source_type,
            pl.source_reference,
            COALESCE(SUM(lr.quantity_sold), 0) AS quantity_sold
        FROM position_lots pl
        LEFT JOIN lot_realizations lr ON lr.lot_id = pl.lot_id
        WHERE pl.symbol = ?
        GROUP BY
            pl.lot_id,
            pl.batch_id,
            pl.created_at,
            pl.acquired_at,
            pl.symbol,
            pl.isin,
            pl.instrument_name,
            pl.quantity_original,
            pl.currency,
            pl.cost_basis_total_local,
            pl.cost_basis_total_dkk,
            pl.fx_rate_to_dkk,
            pl.source_type,
            pl.source_reference
        HAVING quantity_original - COALESCE(SUM(lr.quantity_sold), 0) > 0
        ORDER BY COALESCE(pl.acquired_at, pl.created_at), pl.created_at, pl.lot_id
        """
        ,
        (symbol,),
    ).fetchall()
    lots = []
    for row in rows:
        record = dict(row)
        record["quantity_remaining"] = float(record["quantity_original"]) - float(record["quantity_sold"])
        lots.append(record)
    return lots


def _fetch_position_snapshot(connection, symbol: str, batch_id: str | None = None) -> dict[str, Any] | None:
    effective_batch = batch_id or fetch_latest_batch_id(connection)
    if not effective_batch:
        return None
    row = connection.execute(
        """
        SELECT *
        FROM position_snapshots
        WHERE batch_id = ? AND symbol = ? AND excluded = 0
        LIMIT 1
        """,
        (effective_batch, symbol),
    ).fetchone()
    return dict(row) if row else None


def _fetch_effective_position(connection, symbol: str, batch_id: str | None = None) -> dict[str, Any] | None:
    positions = fetch_portfolio_positions(connection, batch_id=batch_id)
    for row in positions:
        if row["symbol"] == symbol:
            return row
    return None


def _calculate_commission_components(
    symbol: str,
    gross_local: float,
    gross_dkk: float,
    currency: str,
    fx_rate_to_dkk: float,
    config: dict[str, Any],
) -> dict[str, float]:
    exchange_code = parse_exchange_code(symbol).upper()
    commissions_cfg = config["commissions"]
    trade_rate = float(commissions_cfg["default_rate"])
    trade_commission_local = gross_local * trade_rate
    minimum_cfg = commissions_cfg.get("minimums", {}).get(exchange_code)
    if minimum_cfg:
        minimum_amount = float(minimum_cfg["amount"])
        minimum_currency = minimum_cfg["currency"]
        if minimum_currency == currency:
            trade_commission_local = max(trade_commission_local, minimum_amount)
        elif minimum_currency == "DKK":
            trade_commission_local = max(trade_commission_local, minimum_amount / max(fx_rate_to_dkk, 1e-9))
    fx_conversion_dkk = 0.0
    if currency != config["portfolio"]["base_currency"]:
        fx_conversion_dkk = gross_dkk * float(commissions_cfg["fx_conversion_rate"])
    trade_commission_dkk = trade_commission_local * fx_rate_to_dkk
    return {
        "trade_commission_local": trade_commission_local,
        "trade_commission_dkk": trade_commission_dkk,
        "fx_conversion_dkk": fx_conversion_dkk,
        "commission_dkk_total": trade_commission_dkk + fx_conversion_dkk,
    }


def _allocate_sell_to_lots(open_lots: list[dict[str, Any]], qty_to_sell: float, gross_dkk: float) -> dict[str, Any]:
    remaining = qty_to_sell
    allocations: list[dict[str, Any]] = []
    cost_basis_sold_dkk = 0.0
    cost_basis_sold_local = 0.0

    for lot in open_lots:
        if remaining <= 1e-9:
            break
        qty_from_lot = min(remaining, float(lot["quantity_remaining"]))
        qty_fraction = qty_from_lot / float(lot["quantity_original"])
        allocated_cost_dkk = float(lot["cost_basis_total_dkk"]) * qty_fraction
        allocated_cost_local = (float(lot["cost_basis_total_local"] or 0.0)) * qty_fraction
        allocations.append(
            {
                "lot_id": lot["lot_id"],
                "quantity_sold": qty_from_lot,
                "cost_basis_allocated_dkk": allocated_cost_dkk,
                "cost_basis_allocated_local": allocated_cost_local,
                "isin": lot["isin"],
            }
        )
        cost_basis_sold_dkk += allocated_cost_dkk
        cost_basis_sold_local += allocated_cost_local
        remaining -= qty_from_lot

    if remaining > 1e-9:
        raise ValueError(f"Not enough quantity available to sell {qty_to_sell} shares of {open_lots[0]['symbol'] if open_lots else 'unknown'}")

    for allocation in allocations:
        allocation["proceeds_allocated_dkk"] = gross_dkk * (allocation["quantity_sold"] / qty_to_sell)
        allocation["realised_gain_dkk"] = allocation["proceeds_allocated_dkk"] - allocation["cost_basis_allocated_dkk"]

    return {
        "allocations": allocations,
        "cost_basis_sold_dkk": cost_basis_sold_dkk,
        "cost_basis_sold_local": cost_basis_sold_local,
    }


def _fetch_realised_share_income_ytd(connection, tax_year: int) -> float:
    row = connection.execute(
        """
        SELECT COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_ytd
        FROM trade_ledger
        WHERE side = 'SELL' AND tax_year = ?
        """,
        (tax_year,),
    ).fetchone()
    return float(row["realised_gain_ytd"]) if row else 0.0


def calculate_sell_outcome(
    symbol: str,
    qty_to_sell: float,
    current_price: float,
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    batch_id: str | None = None,
    tax_year: int | None = None,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        if qty_to_sell <= 0:
            raise ValueError("qty_to_sell must be greater than 0")
        if current_price <= 0:
            raise ValueError("current_price must be greater than 0")

        open_lots = _fetch_open_lots(resolved_connection, symbol)
        if not open_lots:
            raise ValueError(f"No open lots available for symbol {symbol}")
        open_lot_quantity = sum(float(row["quantity_remaining"] or 0.0) for row in open_lots)
        if qty_to_sell > open_lot_quantity + 1e-9:
            raise ValueError(
                f"Cannot sell {qty_to_sell}; only {open_lot_quantity} open lot quantity available for {symbol}"
            )

        snapshot = _fetch_position_snapshot(resolved_connection, symbol, batch_id=batch_id)
        effective_position = _fetch_effective_position(resolved_connection, symbol, batch_id=batch_id)

        currency = (
            (snapshot or {}).get("currency")
            or (effective_position or {}).get("currency")
            or open_lots[0]["currency"]
        )
        current_fx_rate = 1.0
        if currency != resolved_config["portfolio"]["base_currency"]:
            market_value_local = (effective_position or {}).get("market_value_local")
            market_value_dkk = (effective_position or {}).get("market_value_dkk")
            if market_value_local not in (None, 0) and market_value_dkk not in (None, 0):
                current_fx_rate = float(market_value_dkk) / float(market_value_local)
            else:
                current_fx_rate = float(open_lots[0]["fx_rate_to_dkk"])

        gross_local = qty_to_sell * current_price
        gross_dkk = gross_local * current_fx_rate
        allocation_result = _allocate_sell_to_lots(open_lots, qty_to_sell, gross_dkk)
        commission_result = _calculate_commission_components(
            symbol=symbol,
            gross_local=gross_local,
            gross_dkk=gross_dkk,
            currency=currency,
            fx_rate_to_dkk=current_fx_rate,
            config=resolved_config,
        )
        realised_gain_dkk = (
            gross_dkk
            - allocation_result["cost_basis_sold_dkk"]
            - commission_result["commission_dkk_total"]
        )
        cost_basis_sold_local = float(allocation_result["cost_basis_sold_local"])
        realised_gain_local = gross_local - cost_basis_sold_local
        cost_basis_fx_rate_to_dkk = (
            float(allocation_result["cost_basis_sold_dkk"]) / max(cost_basis_sold_local, 1e-9)
            if cost_basis_sold_local > 0
            else current_fx_rate
        )
        price_gain_dkk = realised_gain_local * current_fx_rate
        fx_gain_dkk = cost_basis_sold_local * (current_fx_rate - cost_basis_fx_rate_to_dkk)
        effective_tax_year = int(tax_year or datetime.now(UTC).year)
        realised_share_income_before = _fetch_realised_share_income_ytd(resolved_connection, effective_tax_year)
        brackets = resolved_config["taxation"]["share_income"]["brackets"]
        tax_before = _tax_due_for_share_income(realised_share_income_before, brackets)
        tax_after = _tax_due_for_share_income(realised_share_income_before + realised_gain_dkk, brackets)
        tax_dkk = tax_after - tax_before
        net_dkk = gross_dkk - commission_result["commission_dkk_total"] - tax_dkk

        return {
            "symbol": symbol,
            "isin": (snapshot or {}).get("isin") or open_lots[0].get("isin"),
            "instrument_name": (snapshot or {}).get("instrument_name") or open_lots[0].get("instrument_name") or symbol,
            "currency": currency,
            "qty_to_sell": qty_to_sell,
            "current_price": current_price,
            "gross_local": gross_local,
            "gross_DKK": gross_dkk,
            "commission_local": commission_result["trade_commission_local"],
            "commission_DKK": commission_result["commission_dkk_total"],
            "commission_breakdown": {
                "trade_commission_DKK": commission_result["trade_commission_dkk"],
                "fx_conversion_DKK": commission_result["fx_conversion_dkk"],
            },
            "cost_basis_sold_DKK": allocation_result["cost_basis_sold_dkk"],
            "cost_basis_sold_local": cost_basis_sold_local,
            "cost_basis_fx_rate_to_dkk": cost_basis_fx_rate_to_dkk,
            "sale_fx_rate_to_dkk": current_fx_rate,
            "realised_gain_local": realised_gain_local,
            "price_gain_dkk": price_gain_dkk,
            "fx_gain_dkk": fx_gain_dkk,
            "realised_gain_DKK": realised_gain_dkk,
            "tax_DKK": tax_dkk,
            "net_DKK": net_dkk,
            "fx_rate_to_dkk": current_fx_rate,
            "tax_year": effective_tax_year,
            "realised_share_income_before_trade_DKK": realised_share_income_before,
            "realised_share_income_after_trade_DKK": realised_share_income_before + realised_gain_dkk,
            "lot_allocations": allocation_result["allocations"],
            "batch_id": (snapshot or {}).get("batch_id") or open_lots[0].get("batch_id") or batch_id or fetch_latest_batch_id(resolved_connection),
        }
    finally:
        if should_close:
            resolved_connection.close()


def update_ledger(
    trade_dict: dict[str, Any],
    *,
    config: dict[str, Any] | None = None,
    connection=None,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        created_at = datetime.now(UTC).isoformat(timespec="seconds")
        batch_id = trade_dict.get("batch_id") or fetch_latest_batch_id(resolved_connection)
        initial_cash_dkk = float(resolved_config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0)
        prefer_broker_cash = (
            str(resolved_config.get("execution", {}).get("mode")) == "live"
            and str(resolved_config.get("execution", {}).get("adapter")) == "saxo"
        )
        portfolio_before = {
            "summary": fetch_portfolio_summary(
                resolved_connection,
                batch_id=batch_id,
                initial_cash_dkk=initial_cash_dkk,
                prefer_broker_cash=prefer_broker_cash,
            ),
            "positions": fetch_portfolio_positions(
                resolved_connection,
                batch_id=batch_id,
                initial_cash_dkk=initial_cash_dkk,
                prefer_broker_cash=prefer_broker_cash,
            ),
        }

        cursor = resolved_connection.execute(
            """
            INSERT INTO trade_ledger (
                created_at,
                symbol,
                isin,
                instrument_name,
                side,
                quantity,
                price_local,
                currency,
                gross_amount_dkk,
                commission_dkk,
                commission_local,
                fx_conversion_dkk,
                tax_dkk,
                realised_gain_dkk,
                realised_gain_local,
                price_gain_dkk,
                fx_gain_dkk,
                cost_basis_sold_dkk,
                cost_basis_sold_local,
                sale_fx_rate_to_dkk,
                cost_basis_fx_rate_to_dkk,
                net_amount_dkk,
                mode,
                status,
                notes,
                portfolio_before_json,
                portfolio_after_json,
                decision_context_json,
                tax_year,
                batch_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                created_at,
                trade_dict["symbol"],
                trade_dict.get("isin"),
                trade_dict.get("instrument_name"),
                "SELL",
                trade_dict["qty_to_sell"],
                trade_dict["current_price"],
                trade_dict["currency"],
                trade_dict["gross_DKK"],
                trade_dict["commission_DKK"],
                trade_dict["commission_local"],
                trade_dict["commission_breakdown"]["fx_conversion_DKK"],
                trade_dict["tax_DKK"],
                trade_dict["realised_gain_DKK"],
                trade_dict.get("realised_gain_local", 0.0),
                trade_dict.get("price_gain_dkk", 0.0),
                trade_dict.get("fx_gain_dkk", 0.0),
                trade_dict["cost_basis_sold_DKK"],
                trade_dict.get("cost_basis_sold_local", 0.0),
                trade_dict.get("sale_fx_rate_to_dkk"),
                trade_dict.get("cost_basis_fx_rate_to_dkk"),
                trade_dict["net_DKK"],
                trade_dict.get("mode", "simulation"),
                trade_dict.get("status", "recorded"),
                trade_dict.get("notes", ""),
                json.dumps(portfolio_before, ensure_ascii=False, sort_keys=True),
                json.dumps({}, ensure_ascii=False, sort_keys=True),
                json.dumps(trade_dict.get("decision_context", {}), ensure_ascii=False, sort_keys=True),
                trade_dict["tax_year"],
                batch_id,
            ),
        )
        ledger_id = int(cursor.lastrowid)

        resolved_connection.executemany(
            """
            INSERT INTO lot_realizations (
                created_at,
                ledger_id,
                lot_id,
                symbol,
                quantity_sold,
                cost_basis_allocated_local,
                cost_basis_allocated_dkk,
                proceeds_allocated_dkk,
                realised_gain_dkk,
                raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                (
                    created_at,
                    ledger_id,
                    allocation["lot_id"],
                    trade_dict["symbol"],
                    allocation["quantity_sold"],
                    allocation["cost_basis_allocated_local"],
                    allocation["cost_basis_allocated_dkk"],
                    allocation["proceeds_allocated_dkk"],
                    allocation["realised_gain_dkk"],
                    json.dumps(allocation, ensure_ascii=False, sort_keys=True),
                )
                for allocation in trade_dict["lot_allocations"]
            ],
        )
        resolved_connection.commit()
        portfolio_after = {
            "summary": fetch_portfolio_summary(
                resolved_connection,
                batch_id=batch_id,
                initial_cash_dkk=initial_cash_dkk,
                prefer_broker_cash=prefer_broker_cash,
            ),
            "positions": fetch_portfolio_positions(
                resolved_connection,
                batch_id=batch_id,
                initial_cash_dkk=initial_cash_dkk,
                prefer_broker_cash=prefer_broker_cash,
            ),
        }
        resolved_connection.execute(
            "UPDATE trade_ledger SET portfolio_after_json = ? WHERE id = ?",
            (json.dumps(portfolio_after, ensure_ascii=False, sort_keys=True), ledger_id),
        )
        resolved_connection.commit()

        append_audit_log(
            resolved_connection,
            "trade_recorded",
            {
                "ledger_id": ledger_id,
                "symbol": trade_dict["symbol"],
                "qty_to_sell": trade_dict["qty_to_sell"],
                "mode": trade_dict.get("mode", "simulation"),
                "realised_gain_dkk": trade_dict["realised_gain_DKK"],
                "tax_dkk": trade_dict["tax_DKK"],
            },
        )
        return {
            "ledger_id": ledger_id,
            "lot_realizations_count": len(trade_dict["lot_allocations"]),
            "status": "recorded",
        }
    finally:
        if should_close:
            resolved_connection.close()
