from __future__ import annotations

import json
import sqlite3
from datetime import UTC, datetime, time, timedelta
from typing import Any

import pytz

from saxo_daytrader_xai.fx_service import fetch_ecb_fx_rates, fx_rate_to_dkk


ACTIVE_LEDGER_STATUSES = {"executed", "approved", "recorded"}


def _normalize_asset_class(value: str | None) -> str:
    text = (value or "").strip()
    normalized = text.casefold()
    mapping = {
        "aktie": "Equity",
        "aktier": "Equity",
        "equity": "Equity",
        "stock": "Equity",
        "stocks": "Equity",
    }
    return mapping.get(normalized, text)


def fetch_latest_batch_id(connection: sqlite3.Connection) -> str | None:
    row = connection.execute(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1"
    ).fetchone()
    return row["batch_id"] if row else None


def _daily_pnl_reset_start_utc() -> str:
    timezone = pytz.timezone("Europe/Copenhagen")
    now_local = datetime.now(timezone)
    reset_local = now_local.replace(hour=6, minute=0, second=0, microsecond=0)
    if now_local < reset_local:
        reset_local -= timedelta(days=1)
    return reset_local.astimezone(pytz.UTC).isoformat(timespec="seconds")


def fetch_realised_daily_pnl_summary(connection: sqlite3.Connection, *, since_utc: str | None = None) -> dict[str, float]:
    reset_start = since_utc or _daily_pnl_reset_start_utc()
    row = connection.execute(
        """
        SELECT
            COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_dkk,
            COALESCE(SUM(commission_dkk), 0) AS commission_dkk,
            COUNT(*) AS trade_count
        FROM trade_ledger
        WHERE created_at >= ?
          AND status IN ({})
        """.format(",".join("?" for _ in ACTIVE_LEDGER_STATUSES)),
        (reset_start, *tuple(ACTIVE_LEDGER_STATUSES)),
    ).fetchone()
    realised_gain_dkk = float(row["realised_gain_dkk"] or 0.0) if row else 0.0
    commission_dkk = float(row["commission_dkk"] or 0.0) if row else 0.0
    return {
        "realised_gain_dkk": realised_gain_dkk,
        "commission_dkk": commission_dkk,
        "realised_pnl_after_commission_dkk": realised_gain_dkk - commission_dkk,
        "trade_count": float(row["trade_count"] or 0.0) if row else 0.0,
    }


def _has_overlay_positions_without_batch(connection: sqlite3.Connection, *, use_broker_positions: bool) -> bool:
    if use_broker_positions:
        broker_row = connection.execute(
            """
            SELECT 1
            FROM broker_position_snapshots
            WHERE quantity > 0
            LIMIT 1
            """
        ).fetchone()
        if broker_row:
            return True
    trade_row = connection.execute(
        """
        SELECT 1
        FROM trade_ledger
        WHERE status IN ('executed', 'approved', 'recorded')
        LIMIT 1
        """
    ).fetchone()
    if trade_row:
        return True
    adjustment_row = connection.execute(
        """
        SELECT 1
        FROM portfolio_reconciliation_adjustments
        LIMIT 1
        """
    ).fetchone()
    return bool(adjustment_row)


def _base_snapshot_rows(connection: sqlite3.Connection, batch_id: str) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
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
            asset_class,
            market_status,
            value_date
        FROM position_snapshots
        WHERE batch_id = ? AND excluded = 0
        ORDER BY market_value_dkk DESC, symbol ASC
        """,
        (batch_id,),
    ).fetchall()
    return [dict(row) for row in rows]


def _trade_rows(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            created_at,
            symbol,
            instrument_name,
            isin,
            figi,
            side,
            quantity,
            price_local,
            currency,
            gross_amount_dkk,
            commission_dkk,
            cost_basis_sold_dkk,
            cost_basis_sold_local
        FROM trade_ledger
        WHERE status IN ('executed', 'approved', 'recorded')
        ORDER BY created_at, id
        """
    ).fetchall()
    return [dict(row) for row in rows]


def _cash_effect_rows(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            id,
            created_at,
            symbol,
            side,
            net_amount_dkk,
            status
        FROM trade_ledger
        ORDER BY created_at, id
        """
    ).fetchall()
    return [dict(row) for row in rows]


def _latest_price_state_by_symbol(connection: sqlite3.Connection) -> dict[str, dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM portfolio_price_snapshots
        """
    ).fetchall()
    return {row["symbol"]: dict(row) for row in rows}


def _broker_position_rows(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            symbol,
            updated_at,
            instrument_name,
            isin,
            uic,
            asset_type,
            quantity,
            currency,
            open_price_local,
            open_price_including_costs_local,
            execution_time_open,
            value_date,
            market_state,
            can_be_closed
        FROM broker_position_snapshots
        ORDER BY updated_at DESC, symbol ASC
        """
    ).fetchall()
    return [dict(row) for row in rows]


def _broker_balance_row(connection: sqlite3.Connection) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT
            updated_at,
            currency,
            cash_available_for_trading,
            margin_available_for_trading,
            cash_balance,
            transactions_not_booked,
            settlement_value,
            total_value
        FROM broker_balance_snapshots
        WHERE singleton_key = 'main'
        """
    ).fetchone()
    return dict(row) if row else None


def _broker_account_row(connection: sqlite3.Connection) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT
            updated_at,
            account_key,
            account_id,
            account_currency,
            is_trial_account,
            fractional_order_enabled,
            fractional_order_enabled_asset_types_json,
            can_use_cash_positions_as_margin_collateral,
            use_cash_positions_as_margin_collateral,
            legal_asset_types_json
        FROM broker_account_snapshots
        WHERE singleton_key = 'main'
        """
    ).fetchone()
    if not row:
        return None
    data = dict(row)
    for key in ("fractional_order_enabled_asset_types_json", "legal_asset_types_json"):
        data[key] = json.loads(data[key]) if data.get(key) else []
    return data


def fetch_broker_account_summary(connection: sqlite3.Connection) -> dict[str, Any] | None:
    return _broker_account_row(connection)


def _broker_exposure_rows(connection: sqlite3.Connection) -> dict[str, dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            symbol,
            updated_at,
            uic,
            asset_type,
            quantity,
            average_open_price,
            profit_loss_on_trade,
            instrument_price_day_percent_change,
            currency,
            calculation_reliability,
            can_be_closed
        FROM broker_instrument_exposures
        """
    ).fetchall()
    return {row["symbol"]: dict(row) for row in rows}


def _reconciliation_adjustment_rows(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            created_at,
            symbol,
            instrument_name,
            isin,
            currency,
            quantity_delta,
            cost_basis_local_delta,
            cost_basis_dkk_delta,
            local_quantity_before,
            broker_quantity_target,
            note
        FROM portfolio_reconciliation_adjustments
        ORDER BY created_at, id
        """
    ).fetchall()
    return [dict(row) for row in rows]


def fetch_cash_summary(
    connection: sqlite3.Connection,
    *,
    initial_cash_dkk: float = 0.0,
    prefer_broker_cash: bool = False,
    broker_cash_cap_dkk: float | None = None,
    invested_market_value_dkk: float = 0.0,
) -> dict[str, Any]:
    cash_from_trades = 0.0
    invalid_trade_ids: list[int] = []
    invalid_rows = {
        int(row["id"]): row
        for row in fetch_invalid_trade_ledger_rows(connection, limit=10_000)
    }
    for row in _cash_effect_rows(connection):
        if row["status"] not in ACTIVE_LEDGER_STATUSES:
            continue
        if int(row["id"]) in invalid_rows:
            invalid_trade_ids.append(int(row["id"]))
            continue
        cash_from_trades += float(row["net_amount_dkk"] or 0.0)
    broker_balance = _broker_balance_row(connection) if prefer_broker_cash else None
    if broker_balance:
        fx_snapshot = fetch_ecb_fx_rates()
        broker_currency = str(broker_balance.get("currency") or "DKK")
        broker_cash_available = float(broker_balance.get("cash_available_for_trading") or 0.0)
        broker_cash_balance_dkk = broker_cash_available * fx_rate_to_dkk(broker_currency, fx_snapshot)
        effective_cash_balance_dkk = broker_cash_balance_dkk
        cash_source = "broker_balance_snapshot"
        if broker_cash_cap_dkk not in (None, 0):
            virtual_cash_overlay_dkk = float(broker_cash_cap_dkk or 0.0) + cash_from_trades
            effective_cash_balance_dkk = min(
                broker_cash_balance_dkk,
                max(virtual_cash_overlay_dkk, 0.0),
            )
            cash_source = "broker_balance_snapshot_virtual_cap"
        return {
            "initial_cash_dkk": float(initial_cash_dkk or 0.0),
            "cash_from_trades_dkk": cash_from_trades,
            "cash_balance_dkk": effective_cash_balance_dkk,
            "ignored_invalid_trade_ids": invalid_trade_ids,
            "cash_source": cash_source,
            "broker_cash_available": broker_cash_available,
            "broker_cash_balance_dkk": broker_cash_balance_dkk,
            "broker_cash_currency": broker_currency,
            "broker_cash_updated_at": broker_balance.get("updated_at"),
            "broker_cash_cap_dkk": float(broker_cash_cap_dkk or 0.0),
        }
    return {
        "initial_cash_dkk": float(initial_cash_dkk or 0.0),
        "cash_from_trades_dkk": cash_from_trades,
        "cash_balance_dkk": float(initial_cash_dkk or 0.0) + cash_from_trades,
        "ignored_invalid_trade_ids": invalid_trade_ids,
        "cash_source": "local_ledger_overlay",
        "broker_cash_available": None,
        "broker_cash_balance_dkk": None,
        "broker_cash_currency": None,
        "broker_cash_updated_at": None,
        "broker_cash_cap_dkk": float(broker_cash_cap_dkk or 0.0),
    }


def _effective_positions(
    connection: sqlite3.Connection,
    batch_id: str,
    *,
    initial_cash_dkk: float = 0.0,
    prefer_broker_cash: bool = False,
    use_broker_positions: bool = True,
) -> list[dict[str, Any]]:
    base_rows = _base_snapshot_rows(connection, batch_id)
    latest_price_state = _latest_price_state_by_symbol(connection)
    broker_rows = _broker_position_rows(connection) if use_broker_positions else []
    broker_exposure_rows = _broker_exposure_rows(connection) if use_broker_positions else {}
    reconciliation_adjustments = _reconciliation_adjustment_rows(connection)
    broker_account_summary = _broker_account_row(connection) if use_broker_positions else None
    account_currency = str((broker_account_summary or {}).get("account_currency") or "DKK")
    account_fx_snapshot = fetch_ecb_fx_rates() if broker_account_summary else None
    broker_symbols = {str(row["symbol"]) for row in broker_rows}
    states: dict[str, dict[str, Any]] = {}
    for row in base_rows:
        base_quantity = float(row["quantity"] or 0.0)
        current_price_local = float(row["current_price_local"] or row["open_price_local"] or 0.0)
        cost_basis_local_total = base_quantity * float(row["cost_basis_local"] or 0.0)
        fx_rate = (
            float(row["market_value_dkk"] or 0.0) / max(float(row["market_value_local"] or (base_quantity * current_price_local or 0.0)), 1e-9)
            if current_price_local > 0
            else 1.0
        )
        states[row["symbol"]] = {
            **row,
            "asset_class": _normalize_asset_class(row.get("asset_class")),
            "quantity": base_quantity,
            "cost_basis_dkk": float(row["cost_basis_dkk"] or 0.0),
            "cost_basis_local_total": cost_basis_local_total,
            "current_price_local": current_price_local,
            "latest_fx_rate": fx_rate,
            "base_quantity": base_quantity,
            "base_daily_pnl_dkk": float(row["daily_pnl_dkk"] or 0.0),
        }

    for trade in _trade_rows(connection):
        symbol = trade["symbol"]
        quantity = float(trade["quantity"] or 0.0)
        gross_amount_dkk = float(trade["gross_amount_dkk"] or 0.0)
        price_local = float(trade["price_local"] or 0.0)
        fx_rate = gross_amount_dkk / max(quantity * price_local, 1e-9) if quantity > 0 and price_local > 0 else 1.0
        state = states.setdefault(
            symbol,
            {
                "instrument_name": trade.get("instrument_name") or symbol,
                "symbol": symbol,
                "isin": trade.get("isin"),
                "figi": trade.get("figi"),
                "quantity": 0.0,
                "currency": trade["currency"],
                "open_price_local": price_local,
                "current_price_local": price_local,
                "cost_basis_local": None,
                "cost_basis_dkk": 0.0,
                "cost_basis_local_total": 0.0,
                "market_value_local": 0.0,
                "market_value_dkk": 0.0,
                "unrealised_pnl_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "asset_class": "Equity",
                "market_status": "Local overlay",
                "value_date": None,
                "latest_fx_rate": fx_rate,
                "base_quantity": 0.0,
                "base_daily_pnl_dkk": 0.0,
            },
        )
        state["current_price_local"] = price_local or state["current_price_local"]
        state["latest_fx_rate"] = fx_rate or state["latest_fx_rate"]
        state["currency"] = trade["currency"] or state["currency"]
        state["asset_class"] = _normalize_asset_class(state.get("asset_class") or "Equity")
        if trade.get("instrument_name"):
            state["instrument_name"] = trade["instrument_name"]
        if trade.get("isin"):
            state["isin"] = trade["isin"]
        if trade.get("figi"):
            state["figi"] = trade["figi"]

        if trade["side"] == "BUY":
            state["quantity"] = float(state["quantity"]) + quantity
            state["cost_basis_dkk"] = float(state["cost_basis_dkk"]) + gross_amount_dkk + float(trade["commission_dkk"] or 0.0)
            state["cost_basis_local_total"] = float(state.get("cost_basis_local_total") or 0.0) + (quantity * price_local)
        else:
            available_quantity = float(state["quantity"] or 0.0)
            if quantity > available_quantity + 1e-9:
                continue
            state["quantity"] = max(available_quantity - quantity, 0.0)
            state["cost_basis_dkk"] = max(
                float(state["cost_basis_dkk"]) - float(trade["cost_basis_sold_dkk"] or 0.0),
                0.0,
            )
            state["cost_basis_local_total"] = max(
                float(state.get("cost_basis_local_total") or 0.0) - float(trade.get("cost_basis_sold_local") or 0.0),
                0.0,
            )

    for adjustment in reconciliation_adjustments:
        symbol = str(adjustment["symbol"])
        quantity_delta = float(adjustment["quantity_delta"] or 0.0)
        if abs(quantity_delta) <= 1e-9:
            continue
        state = states.setdefault(
            symbol,
            {
                "instrument_name": adjustment.get("instrument_name") or symbol,
                "symbol": symbol,
                "isin": adjustment.get("isin"),
                "figi": None,
                "quantity": 0.0,
                "currency": adjustment.get("currency"),
                "open_price_local": None,
                "current_price_local": None,
                "cost_basis_local": None,
                "cost_basis_dkk": 0.0,
                "cost_basis_local_total": 0.0,
                "market_value_local": 0.0,
                "market_value_dkk": 0.0,
                "unrealised_pnl_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "asset_class": "Equity",
                "market_status": "Portfolio reconciliation",
                "value_date": None,
                "latest_fx_rate": 1.0,
                "base_quantity": 0.0,
                "base_daily_pnl_dkk": 0.0,
            },
        )
        state["quantity"] = max(float(state.get("quantity") or 0.0) + quantity_delta, 0.0)
        state["cost_basis_dkk"] = max(
            float(state.get("cost_basis_dkk") or 0.0) + float(adjustment.get("cost_basis_dkk_delta") or 0.0),
            0.0,
        )
        state["cost_basis_local_total"] = max(
            float(state.get("cost_basis_local_total") or 0.0) + float(adjustment.get("cost_basis_local_delta") or 0.0),
            0.0,
        )
        state["market_status"] = "Portfolio reconciliation"
        if adjustment.get("instrument_name"):
            state["instrument_name"] = adjustment["instrument_name"]
        if adjustment.get("isin"):
            state["isin"] = adjustment["isin"]
        if adjustment.get("currency"):
            state["currency"] = adjustment["currency"]

    for broker in broker_rows:
        symbol = str(broker["symbol"])
        broker_quantity = float(broker["quantity"] or 0.0)
        if broker_quantity <= 1e-9:
            states.pop(symbol, None)
            continue
        broker_currency = broker.get("currency")
        broker_open_price = float(
            broker.get("open_price_including_costs_local")
            or broker.get("open_price_local")
            or 0.0
        )
        state = states.get(symbol)
        if state is None:
            state = {
                "instrument_name": broker.get("instrument_name") or symbol,
                "symbol": symbol,
                "isin": broker.get("isin"),
                "uic": broker.get("uic"),
                "asset_type": broker.get("asset_type"),
                "quantity": broker_quantity,
                "currency": broker_currency,
                "open_price_local": broker_open_price,
                "current_price_local": broker_open_price,
                "cost_basis_local": broker_open_price,
                "cost_basis_dkk": 0.0,
                "cost_basis_local_total": broker_quantity * broker_open_price,
                "market_value_local": 0.0,
                "market_value_dkk": 0.0,
                "unrealised_pnl_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "asset_class": "Equity",
                "market_status": "Saxo broker snapshot",
                "value_date": broker.get("value_date"),
                "latest_fx_rate": 1.0,
                "base_quantity": broker_quantity,
                "base_daily_pnl_dkk": 0.0,
            }
            states[symbol] = state
        else:
            previous_quantity = float(state.get("quantity") or 0.0)
            if previous_quantity > 0 and abs(previous_quantity - broker_quantity) > 1e-9:
                unit_cost_dkk = float(state.get("cost_basis_dkk") or 0.0) / previous_quantity
                unit_cost_local = float(state.get("cost_basis_local_total") or 0.0) / previous_quantity
                state["cost_basis_dkk"] = unit_cost_dkk * broker_quantity
                state["cost_basis_local_total"] = unit_cost_local * broker_quantity
            state["quantity"] = broker_quantity
            state["base_quantity"] = broker_quantity
            state["market_status"] = "Saxo broker snapshot"
            state["value_date"] = broker.get("value_date") or state.get("value_date")
        if broker_open_price > 0:
            state["open_price_local"] = broker_open_price
            state["current_price_local"] = broker_open_price if not latest_price_state.get(symbol) else state.get("current_price_local")
        if broker_currency:
            state["currency"] = broker_currency
        if broker.get("instrument_name"):
            state["instrument_name"] = broker["instrument_name"]
        if broker.get("isin"):
            state["isin"] = broker["isin"]
        state["asset_class"] = _normalize_asset_class(state.get("asset_class") or broker.get("asset_type") or "Equity")
        if broker_open_price > 0 and broker_quantity > 0:
            state["cost_basis_local_total"] = broker_quantity * broker_open_price if float(state.get("cost_basis_local_total") or 0.0) <= 0 else float(state.get("cost_basis_local_total") or 0.0)

    positions: list[dict[str, Any]] = []
    for state in states.values():
        if broker_symbols and state["symbol"] not in broker_symbols and float(state.get("base_quantity") or 0.0) <= 1e-9:
            continue
        effective_quantity = float(state["quantity"] or 0.0)
        if effective_quantity <= 1e-9:
            continue
        price_state = latest_price_state.get(state["symbol"], {})
        current_price_local = float(
            price_state.get("current_price_local")
            or state["current_price_local"]
            or state["open_price_local"]
            or 0.0
        )
        fx_rate = float(price_state.get("current_fx_rate_to_dkk") or state["latest_fx_rate"] or 1.0)
        exposure = broker_exposure_rows.get(state["symbol"], {})
        baseline_price_local = price_state.get("baseline_price_local")
        baseline_fx_rate = price_state.get("baseline_fx_rate_to_dkk")
        if baseline_price_local not in (None, "") and baseline_fx_rate not in (None, ""):
            daily_pnl_dkk = effective_quantity * (
                current_price_local * fx_rate
                - float(baseline_price_local) * float(baseline_fx_rate)
            )
        else:
            base_quantity = float(state.get("base_quantity") or 0.0)
            quantity_scale = effective_quantity / base_quantity if base_quantity > 0 else 0.0
            daily_pnl_dkk = float(state.get("base_daily_pnl_dkk") or 0.0) * quantity_scale
        effective_market_value_dkk = effective_quantity * current_price_local * fx_rate
        cost_basis_local_total = float(state.get("cost_basis_local_total") or 0.0)
        paid_price_local = cost_basis_local_total / effective_quantity if effective_quantity > 0 and cost_basis_local_total > 0 else None
        cost_basis_fx_rate_to_dkk = (
            float(state["cost_basis_dkk"] or 0.0) / max(cost_basis_local_total, 1e-9)
            if cost_basis_local_total > 0
            else 1.0
        )
        fx_unrealised_pnl_dkk = 0.0
        if str(state.get("currency") or "").upper() not in {"DKK", "EUR"} and cost_basis_local_total > 0:
            fx_unrealised_pnl_dkk = cost_basis_local_total * (fx_rate - cost_basis_fx_rate_to_dkk)
        unrealised_pnl_dkk = effective_market_value_dkk - float(state["cost_basis_dkk"] or 0.0)
        exposure_profit = exposure.get("profit_loss_on_trade")
        if exposure_profit not in (None, ""):
            unrealised_pnl_dkk = float(exposure_profit) * fx_rate_to_dkk(account_currency, account_fx_snapshot or {})
        positions.append(
            {
                **state,
                "quantity": effective_quantity,
                "paid_price_local": paid_price_local,
                "current_price_local": current_price_local,
                "market_value_local": effective_quantity * current_price_local,
                "market_value_dkk": effective_market_value_dkk,
                "unrealised_pnl_dkk": unrealised_pnl_dkk,
                "fx_unrealised_pnl_dkk": fx_unrealised_pnl_dkk,
                "daily_pnl_dkk": daily_pnl_dkk,
                "cost_basis_fx_rate_to_dkk": cost_basis_fx_rate_to_dkk,
                "day_baseline_price_local": baseline_price_local,
                "day_baseline_fx_rate_to_dkk": baseline_fx_rate,
                "current_fx_rate_to_dkk": fx_rate,
                "latest_quote_updated_at": price_state.get("updated_at"),
                "baseline_session_date": price_state.get("baseline_session_date"),
                "quote_status": price_state.get("status"),
                "broker_profit_loss_on_trade": exposure_profit,
                "broker_calculation_reliability": exposure.get("calculation_reliability"),
            }
        )

    invested_market_value_dkk = sum(float(row["market_value_dkk"] or 0.0) for row in positions)
    broker_cash_cap_dkk = float(initial_cash_dkk or 0.0) if prefer_broker_cash else None
    total_portfolio_value_dkk = invested_market_value_dkk + float(
        fetch_cash_summary(
            connection,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
            broker_cash_cap_dkk=broker_cash_cap_dkk,
            invested_market_value_dkk=invested_market_value_dkk,
        )["cash_balance_dkk"]
    )
    for row in positions:
        row["allocation_pct"] = (
            float(row["market_value_dkk"] or 0.0) / total_portfolio_value_dkk
            if total_portfolio_value_dkk > 0
            else 0.0
        )
    positions.sort(key=lambda row: (-(float(row["market_value_dkk"] or 0.0)), row["symbol"]))
    return positions


def fetch_portfolio_positions(
    connection: sqlite3.Connection,
    batch_id: str | None = None,
    *,
    initial_cash_dkk: float = 0.0,
    prefer_broker_cash: bool = False,
    use_broker_positions: bool = True,
) -> list[dict[str, Any]]:
    batch_id = batch_id or fetch_latest_batch_id(connection)
    if not batch_id:
        if _has_overlay_positions_without_batch(connection, use_broker_positions=use_broker_positions):
            batch_id = "__broker_overlay__"
        else:
            return []
    return _effective_positions(
        connection,
        batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
        use_broker_positions=use_broker_positions,
    )


def _empty_portfolio_summary(initial_cash_dkk: float) -> dict[str, Any]:
    return {
        "batch_id": None,
        "position_count": 0,
        "total_market_value_dkk": 0.0,
        "invested_market_value_dkk": 0.0,
        "cash_balance_dkk": float(initial_cash_dkk or 0.0),
        "initial_cash_dkk": float(initial_cash_dkk or 0.0),
        "cash_from_trades_dkk": 0.0,
        "total_cost_basis_dkk": 0.0,
        "total_unrealised_pnl_dkk": 0.0,
        "total_daily_pnl_dkk": 0.0,
        "total_open_daily_pnl_dkk": 0.0,
        "total_realised_daily_pnl_dkk": 0.0,
        "total_realised_daily_gain_dkk": 0.0,
        "total_daily_commission_dkk": 0.0,
    }


def _summary_batch_id(connection: sqlite3.Connection, batch_id: str | None, *, use_broker_positions: bool) -> str | None:
    resolved_batch_id = batch_id or fetch_latest_batch_id(connection)
    if resolved_batch_id:
        return resolved_batch_id
    if _has_overlay_positions_without_batch(connection, use_broker_positions=use_broker_positions):
        return "__broker_overlay__"
    return None


def fetch_portfolio_summary(
    connection: sqlite3.Connection,
    batch_id: str | None = None,
    *,
    initial_cash_dkk: float = 0.0,
    prefer_broker_cash: bool = False,
    use_broker_positions: bool = True,
) -> dict[str, Any]:
    batch_id = _summary_batch_id(connection, batch_id, use_broker_positions=use_broker_positions)
    if not batch_id:
        return _empty_portfolio_summary(initial_cash_dkk)
    positions = _effective_positions(
        connection,
        batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
        use_broker_positions=use_broker_positions,
    )
    invested_market_value_dkk = sum(float(row["market_value_dkk"] or 0.0) for row in positions)
    open_daily_pnl_dkk = sum(float(row["daily_pnl_dkk"] or 0.0) for row in positions)
    realised_daily = fetch_realised_daily_pnl_summary(connection)
    realised_daily_pnl_dkk = float(realised_daily["realised_pnl_after_commission_dkk"])
    cash_summary = fetch_cash_summary(
        connection,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
        broker_cash_cap_dkk=float(initial_cash_dkk or 0.0) if prefer_broker_cash else None,
        invested_market_value_dkk=invested_market_value_dkk,
    )
    return {
        "batch_id": batch_id,
        "position_count": len(positions),
        "total_market_value_dkk": invested_market_value_dkk + float(cash_summary["cash_balance_dkk"]),
        "invested_market_value_dkk": invested_market_value_dkk,
        "cash_balance_dkk": float(cash_summary["cash_balance_dkk"]),
        "initial_cash_dkk": float(cash_summary["initial_cash_dkk"]),
        "cash_from_trades_dkk": float(cash_summary["cash_from_trades_dkk"]),
        "cash_source": cash_summary.get("cash_source"),
        "broker_cash_available": cash_summary.get("broker_cash_available"),
        "broker_cash_currency": cash_summary.get("broker_cash_currency"),
        "broker_cash_updated_at": cash_summary.get("broker_cash_updated_at"),
        "total_cost_basis_dkk": sum(float(row["cost_basis_dkk"] or 0.0) for row in positions),
        "total_unrealised_pnl_dkk": sum(float(row["unrealised_pnl_dkk"] or 0.0) for row in positions),
        "total_daily_pnl_dkk": open_daily_pnl_dkk + realised_daily_pnl_dkk,
        "total_open_daily_pnl_dkk": open_daily_pnl_dkk,
        "total_realised_daily_pnl_dkk": realised_daily_pnl_dkk,
        "total_realised_daily_gain_dkk": float(realised_daily["realised_gain_dkk"]),
        "total_daily_commission_dkk": float(realised_daily["commission_dkk"]),
    }


def fetch_portfolio_integrity_status(
    connection: sqlite3.Connection,
    *,
    batch_id: str | None = None,
    initial_cash_dkk: float = 0.0,
    use_broker_positions: bool = True,
) -> dict[str, Any]:
    batch_id = batch_id or fetch_latest_batch_id(connection)
    if not batch_id:
        return {"healthy": True, "warnings": [], "mismatches": [], "unreconciled_orders": []}

    local_positions = fetch_portfolio_positions(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        use_broker_positions=False,
    )
    local_qty = {row["symbol"]: float(row["quantity"] or 0.0) for row in local_positions}
    mismatches: list[dict[str, Any]] = []
    if use_broker_positions:
        broker_positions = _broker_position_rows(connection)
        broker_qty = {
            row["symbol"]: float(row["quantity"] or 0.0)
            for row in broker_positions
            if float(row["quantity"] or 0.0) > 1e-9
        }
        for symbol in sorted(set(local_qty) | set(broker_qty)):
            local_value = float(local_qty.get(symbol, 0.0))
            broker_value = float(broker_qty.get(symbol, 0.0))
            if abs(local_value - broker_value) > 1e-9:
                mismatches.append(
                    {
                        "symbol": symbol,
                        "local_quantity": local_value,
                        "broker_quantity": broker_value,
                    }
                )
    mismatch_symbols = {row["symbol"] for row in mismatches}

    candidate_rows = connection.execute(
        """
        SELECT id, symbol, status, error_text, broker_order_id
        FROM execution_orders
        WHERE status IN ('broker_fill_unreconciled', 'execution_failed')
          AND (
              status = 'broker_fill_unreconciled'
              OR (error_text LIKE ? AND error_text NOT LIKE ?)
          )
        ORDER BY id DESC
        LIMIT 20
        """,
        ("%NotOwned%", "%reconciled to Saxo broker holdings%"),
    ).fetchall()
    unreconciled_orders: list[dict[str, Any]] = []
    for row in candidate_rows:
        item = dict(row)
        symbol = str(item.get("symbol") or "")
        if item["status"] == "execution_failed" and symbol not in mismatch_symbols:
            # A stale NotOwned precheck is only an integrity issue while the symbol
            # still differs from Saxo. Once broker/local holdings agree, keep it in
            # the audit trail but stop surfacing it as an actionable warning.
            continue
        unreconciled_orders.append(item)

    warnings: list[str] = []
    if mismatches:
        sample = ", ".join(
            f"{row['symbol']} (local ledger {row['local_quantity']:.0f} vs broker {row['broker_quantity']:.0f})"
            for row in mismatches[:3]
        )
        warnings.append(
            f"Broker holdings differ from local ledger/tax lots for {len(mismatches)} symbol(s): {sample}. "
            "The portfolio table is using Saxo LIVE broker holdings because broker snapshots are authoritative."
        )
    if unreconciled_orders:
        sample = ", ".join(
            f"{row['symbol']} [{row['status']}]"
            for row in unreconciled_orders[:3]
        )
        warnings.append(f"There are {len(unreconciled_orders)} unreconciled/ownership-related live order(s): {sample}.")

    return {
        "healthy": not warnings,
        "warnings": warnings,
        "mismatches": mismatches,
        "unreconciled_orders": unreconciled_orders,
    }


def _annotated_trade_ledger_rows(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    batch_id = fetch_latest_batch_id(connection)
    available_by_symbol = {
        row["symbol"]: float(row["quantity"] or 0.0)
        for row in (_base_snapshot_rows(connection, batch_id) if batch_id else [])
    }
    rows = connection.execute(
        """
        SELECT
            id,
            created_at,
            symbol,
            side,
            quantity,
            price_local,
            currency,
            gross_amount_dkk,
            commission_dkk,
            tax_dkk,
            realised_gain_local,
            realised_gain_dkk,
            price_gain_dkk,
            fx_gain_dkk,
            cost_basis_sold_dkk,
            cost_basis_sold_local,
            sale_fx_rate_to_dkk,
            cost_basis_fx_rate_to_dkk,
            net_amount_dkk,
            mode,
            status,
            notes
        FROM trade_ledger
        ORDER BY id ASC
        """,
        (),
    ).fetchall()
    annotated: list[dict[str, Any]] = []
    for row in rows:
        record = dict(row)
        symbol = record["symbol"]
        quantity = float(record["quantity"] or 0.0)
        validation_note = ""
        if record["side"] == "BUY":
            available_by_symbol[symbol] = available_by_symbol.get(symbol, 0.0) + quantity
        else:
            available = available_by_symbol.get(symbol, 0.0)
            if quantity > available + 1e-9:
                validation_note = f"Ignored by effective portfolio overlay: sell exceeds available quantity ({available:.4f})."
            else:
                available_by_symbol[symbol] = max(available - quantity, 0.0)
        record["validation_note"] = validation_note
        annotated.append(record)
    annotated.reverse()
    return annotated


def fetch_trade_ledger(connection: sqlite3.Connection, limit: int = 50) -> list[dict[str, Any]]:
    annotated = _annotated_trade_ledger_rows(connection)
    return annotated[:limit]


def fetch_invalid_trade_ledger_rows(connection: sqlite3.Connection, limit: int = 50) -> list[dict[str, Any]]:
    invalid_rows = [
        row
        for row in _annotated_trade_ledger_rows(connection)
        if row.get("validation_note") and row.get("status") in {"executed", "approved", "recorded"}
    ]
    return invalid_rows[:limit]


def fetch_portfolio_symbols(connection: sqlite3.Connection, batch_id: str | None = None) -> list[str]:
    batch_id = batch_id or fetch_latest_batch_id(connection)
    if not batch_id:
        return []
    rows = connection.execute(
        """
        SELECT symbol
        FROM position_snapshots
        WHERE batch_id = ? AND excluded = 0
        ORDER BY market_value_dkk DESC, symbol ASC
        """,
        (batch_id,),
    ).fetchall()
    return [row["symbol"] for row in rows]


def fetch_realised_tax_summary(connection: sqlite3.Connection, tax_year: int) -> dict[str, Any]:
    row = connection.execute(
        """
        SELECT
            COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_dkk,
            COALESCE(SUM(tax_dkk), 0) AS tax_dkk,
            COALESCE(SUM(commission_dkk), 0) AS commission_dkk,
            COUNT(*) AS trade_count
        FROM trade_ledger
        WHERE side = 'SELL' AND tax_year = ?
        """,
        (tax_year,),
    ).fetchone()
    return dict(row) if row else {"realised_gain_dkk": 0.0, "tax_dkk": 0.0, "commission_dkk": 0.0, "trade_count": 0}


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


def fetch_unrealised_after_tax_summary(
    connection: sqlite3.Connection,
    config: dict[str, Any],
    batch_id: str | None = None,
    *,
    initial_cash_dkk: float = 0.0,
    use_broker_positions: bool = True,
    tax_year: int | None = None,
) -> dict[str, Any]:
    effective_tax_year = int(tax_year or datetime.now(UTC).year)
    summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        use_broker_positions=use_broker_positions,
    )
    total_unrealised_pnl_dkk = float(summary["total_unrealised_pnl_dkk"] or 0.0)
    realised_summary = fetch_realised_tax_summary(connection, tax_year=effective_tax_year)
    realised_gain_ytd = float(realised_summary["realised_gain_dkk"] or 0.0)
    brackets = config["taxation"]["share_income"]["brackets"]
    tax_before = _tax_due_for_share_income(realised_gain_ytd, brackets)
    tax_after = _tax_due_for_share_income(realised_gain_ytd + total_unrealised_pnl_dkk, brackets)
    estimated_tax_dkk = tax_after - tax_before
    return {
        "tax_year": effective_tax_year,
        "gross_unrealised_pnl_dkk": total_unrealised_pnl_dkk,
        "estimated_tax_dkk": estimated_tax_dkk,
        "after_tax_unrealised_pnl_dkk": total_unrealised_pnl_dkk - estimated_tax_dkk,
        "realised_gain_ytd_dkk": realised_gain_ytd,
    }


def _price_monitor_timezone(config: dict[str, Any]):
    return pytz.timezone(str(config.get("price_monitor", {}).get("timezone", "Europe/Copenhagen")))


def _reset_hour_local(config: dict[str, Any]) -> int:
    return int(config.get("price_monitor", {}).get("reset_hour_local", 6))


def _session_start_local(config: dict[str, Any], local_date) -> datetime:
    timezone = _price_monitor_timezone(config)
    return timezone.localize(datetime.combine(local_date, time(hour=_reset_hour_local(config))))


def _session_date_for_local_dt(config: dict[str, Any], local_dt: datetime):
    session_date = local_dt.date()
    if local_dt.hour < _reset_hour_local(config):
        session_date = session_date - timedelta(days=1)
    return session_date


def _history_with_local_timestamps(connection: sqlite3.Connection, config: dict[str, Any]) -> list[dict[str, Any]]:
    timezone = _price_monitor_timezone(config)
    rows = fetch_portfolio_value_history(connection, limit=200_000)
    output: list[dict[str, Any]] = []
    for row in rows:
        recorded_at_utc = datetime.fromisoformat(str(row["recorded_at"]))
        local_dt = recorded_at_utc.astimezone(timezone)
        output.append(
            {
                **row,
                "recorded_at_dt": recorded_at_utc,
                "recorded_at_local": local_dt,
                "session_date": row.get("baseline_session_date") or _session_date_for_local_dt(config, local_dt).isoformat(),
            }
        )
    return output


def _prefer_broker_state_for_goal_tracking(config: dict[str, Any]) -> bool:
    execution_cfg = config.get("execution", {})
    saxo_environment = str(config.get("saxo", {}).get("environment") or "").lower()
    return (
        str(execution_cfg.get("mode") or "").lower() == "live"
        and str(execution_cfg.get("adapter") or "").lower() == "saxo"
        and saxo_environment == "live"
    )


def _reconcile_latest_history_row_with_current_summary(
    connection: sqlite3.Connection,
    config: dict[str, Any],
    history_rows: list[dict[str, Any]],
) -> dict[str, Any] | None:
    if not history_rows:
        return None
    prefer_broker_state = _prefer_broker_state_for_goal_tracking(config)
    current_summary = fetch_portfolio_summary(
        connection,
        initial_cash_dkk=float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0),
        prefer_broker_cash=prefer_broker_state,
        use_broker_positions=prefer_broker_state,
    )
    current_value = float(current_summary.get("total_market_value_dkk") or 0.0)
    latest_row = history_rows[-1]
    latest_value = float(latest_row.get("total_market_value_dkk") or 0.0)
    if current_value <= 0 or abs(current_value - latest_value) < 1.0:
        return None
    history_rows[-1] = {
        **latest_row,
        "total_market_value_dkk": current_value,
        "invested_market_value_dkk": float(current_summary.get("invested_market_value_dkk") or 0.0),
        "cash_balance_dkk": float(current_summary.get("cash_balance_dkk") or 0.0),
        "total_cost_basis_dkk": float(current_summary.get("total_cost_basis_dkk") or 0.0),
        "total_unrealised_pnl_dkk": float(current_summary.get("total_unrealised_pnl_dkk") or 0.0),
        "total_daily_pnl_dkk": float(current_summary.get("total_daily_pnl_dkk") or 0.0),
        "position_count": int(current_summary.get("position_count") or 0),
        "source": "current_summary_reconciled",
    }
    return {
        "applied": True,
        "recorded_at": latest_row["recorded_at_local"].isoformat(timespec="seconds"),
        "stored_value_dkk": latest_value,
        "current_value_dkk": current_value,
        "difference_dkk": current_value - latest_value,
        "stored_position_count": int(latest_row.get("position_count") or 0),
        "current_position_count": int(current_summary.get("position_count") or 0),
    }


def _period_stats(history_rows: list[dict[str, Any]], *, start_local: datetime | None, end_local: datetime) -> dict[str, Any]:
    eligible = [row for row in history_rows if row["recorded_at_local"] <= end_local]
    if not eligible:
        return {
            "available": False,
            "start_at": None,
            "end_at": end_local.isoformat(timespec="seconds"),
            "current_valuation_at": None,
            "current_value_dkk": 0.0,
            "anchor_value_dkk": 0.0,
            "pnl_dkk": 0.0,
            "observed_session_days": 0,
        }
    current_row = eligible[-1]
    if start_local is None:
        anchor_row = eligible[0]
        in_period = eligible
    else:
        in_period = [row for row in eligible if row["recorded_at_local"] >= start_local]
        anchor_row = in_period[0] if in_period else None
        if anchor_row is None:
            return {
                "available": False,
                "start_at": start_local.isoformat(timespec="seconds"),
                "end_at": end_local.isoformat(timespec="seconds"),
                "current_valuation_at": current_row["recorded_at_local"].isoformat(timespec="seconds"),
                "current_value_dkk": float(current_row["total_market_value_dkk"]),
                "anchor_value_dkk": 0.0,
                "pnl_dkk": 0.0,
                "observed_session_days": 0,
            }
    observed_days = len({row["session_date"] for row in in_period})
    pnl_dkk = float(current_row["total_market_value_dkk"]) - float(anchor_row["total_market_value_dkk"])
    return {
        "available": True,
        "start_at": anchor_row["recorded_at_local"].isoformat(timespec="seconds"),
        "end_at": current_row["recorded_at_local"].isoformat(timespec="seconds"),
        "current_valuation_at": current_row["recorded_at_local"].isoformat(timespec="seconds"),
        "current_value_dkk": float(current_row["total_market_value_dkk"]),
        "anchor_value_dkk": float(anchor_row["total_market_value_dkk"]),
        "pnl_dkk": pnl_dkk,
        "observed_session_days": observed_days,
    }


def _goal_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("xai", {}).get("performance_goals", {})


def _goal_float(config: dict[str, Any], key: str, default: float) -> float:
    try:
        return float(_goal_cfg(config).get(key, default) or default)
    except (TypeError, ValueError):
        return default


def _goal_weekdays(config: dict[str, Any]) -> list[int]:
    cfg = _goal_cfg(config)
    start = int(cfg.get("week_start_weekday", 0) or 0)
    end = int(cfg.get("week_end_weekday", 4) or 4)
    if start <= end:
        return list(range(start, end + 1))
    return list(range(start, 7)) + list(range(0, end + 1))


def _count_goal_weekdays(start_date, end_date, weekdays: list[int]) -> int:
    cursor = start_date
    count = 0
    while cursor <= end_date:
        if cursor.weekday() in weekdays:
            count += 1
        cursor += timedelta(days=1)
    return count


def fetch_goal_tracking(
    connection: sqlite3.Connection,
    config: dict[str, Any],
    *,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    history_rows = _history_with_local_timestamps(connection, config)
    current_summary_reconciliation = _reconcile_latest_history_row_with_current_summary(connection, config, history_rows)
    timezone = _price_monitor_timezone(config)
    now_local = (reference_time or datetime.now(UTC)).astimezone(timezone)
    goal_text = str(config.get("xai", {}).get("goal", ""))
    baseline_day_start = _session_start_local(config, _session_date_for_local_dt(config, now_local))
    goal_weekdays = _goal_weekdays(config)
    first_weekday = goal_weekdays[0] if goal_weekdays else 0
    days_since_week_start = (baseline_day_start.date().weekday() - first_weekday) % 7
    baseline_week_start = _session_start_local(config, baseline_day_start.date() - timedelta(days=days_since_week_start))
    baseline_month_start = _session_start_local(config, baseline_day_start.replace(day=1).date())
    baseline_year_start = _session_start_local(config, baseline_day_start.replace(month=1, day=1).date())

    weekly_target_dkk = _goal_float(config, "weekly_target_dkk", 5000.0)
    monthly_target_dkk = _goal_float(config, "monthly_target_dkk", 20000.0)
    week_trading_days = max(len(goal_weekdays), 1)
    daily_target_dkk = _goal_float(config, "daily_target_dkk", weekly_target_dkk / week_trading_days)
    stretch_weekly_target_dkk = _goal_float(config, "stretch_weekly_target_dkk", weekly_target_dkk * 1.5)
    stretch_daily_target_dkk = _goal_float(config, "stretch_daily_target_dkk", stretch_weekly_target_dkk / week_trading_days)

    periods = {
        "day": _period_stats(history_rows, start_local=baseline_day_start, end_local=now_local),
        "week": _period_stats(history_rows, start_local=baseline_week_start, end_local=now_local),
        "month": _period_stats(history_rows, start_local=baseline_month_start, end_local=now_local),
        "year": _period_stats(history_rows, start_local=baseline_year_start, end_local=now_local),
        "all_time": _period_stats(history_rows, start_local=None, end_local=now_local),
    }
    current_month_expected_days = _count_goal_weekdays(
        baseline_month_start.date(),
        (baseline_month_start.replace(day=1) + timedelta(days=32)).replace(day=1).date() - timedelta(days=1),
        goal_weekdays,
    )

    for name, period in periods.items():
        if name == "week":
            target_dkk = min(weekly_target_dkk, daily_target_dkk * period["observed_session_days"])
            full_period_target_dkk = weekly_target_dkk
        elif name == "month":
            month_progress = period["observed_session_days"] / max(current_month_expected_days, 1)
            target_dkk = monthly_target_dkk * max(0.0, min(month_progress, 1.0))
            full_period_target_dkk = monthly_target_dkk
        else:
            target_dkk = daily_target_dkk * period["observed_session_days"]
            full_period_target_dkk = target_dkk
        period["target_dkk"] = target_dkk
        period["full_period_target_dkk"] = full_period_target_dkk
        period["stretch_target_dkk"] = stretch_daily_target_dkk * period["observed_session_days"]
        period["gap_dkk"] = period["pnl_dkk"] - target_dkk
        period["pct_of_target"] = (period["pnl_dkk"] / target_dkk * 100.0) if abs(target_dkk) > 1e-9 else 0.0

    all_time = periods["all_time"]
    average_per_observed_day = (
        all_time["pnl_dkk"] / all_time["observed_session_days"]
        if all_time["observed_session_days"] > 0
        else 0.0
    )
    projected_weekly_from_average = average_per_observed_day * 5.0

    return {
        "goal_text": goal_text,
        "as_of": now_local.isoformat(timespec="seconds"),
        "timezone": str(timezone),
        "reset_hour_local": _reset_hour_local(config),
        "daily_target_dkk": daily_target_dkk,
        "stretch_daily_target_dkk": stretch_daily_target_dkk,
        "weekly_target_dkk": weekly_target_dkk,
        "monthly_target_dkk": monthly_target_dkk,
        "week_start_weekday": first_weekday,
        "week_end_weekday": goal_weekdays[-1] if goal_weekdays else 4,
        "week_trading_days": week_trading_days,
        "current_month_expected_session_days": current_month_expected_days,
        "average_dkk_per_observed_day": average_per_observed_day,
        "projected_weekly_dkk_from_average": projected_weekly_from_average,
        "current_summary_reconciliation": current_summary_reconciliation,
        "periods": periods,
    }


def record_portfolio_value_snapshot(
    connection: sqlite3.Connection,
    *,
    recorded_at: str,
    snapshot_type: str,
    initial_cash_dkk: float = 0.0,
    prefer_broker_cash: bool = False,
    batch_id: str | None = None,
    baseline_session_date: str | None = None,
    source: str | None = None,
    extra_payload: dict[str, Any] | None = None,
) -> int:
    summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    payload = {
        "summary": summary,
        "snapshot_type": snapshot_type,
        "baseline_session_date": baseline_session_date,
        "source": source,
    }
    if extra_payload:
        payload["extra"] = extra_payload
    cursor = connection.execute(
        """
        INSERT INTO portfolio_value_history (
            recorded_at,
            snapshot_type,
            baseline_session_date,
            batch_id,
            total_market_value_dkk,
            invested_market_value_dkk,
            cash_balance_dkk,
            total_cost_basis_dkk,
            total_unrealised_pnl_dkk,
            total_daily_pnl_dkk,
            position_count,
            source,
            raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            recorded_at,
            snapshot_type,
            baseline_session_date,
            summary.get("batch_id"),
            float(summary["total_market_value_dkk"]),
            float(summary["invested_market_value_dkk"]),
            float(summary["cash_balance_dkk"]),
            float(summary["total_cost_basis_dkk"]),
            float(summary["total_unrealised_pnl_dkk"]),
            float(summary["total_daily_pnl_dkk"]),
            int(summary["position_count"]),
            source,
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


def fetch_portfolio_value_history(
    connection: sqlite3.Connection,
    *,
    start_at: str | None = None,
    end_at: str | None = None,
    limit: int = 20_000,
) -> list[dict[str, Any]]:
    conditions: list[str] = []
    params: list[Any] = []
    if start_at:
        conditions.append("recorded_at >= ?")
        params.append(start_at)
    if end_at:
        conditions.append("recorded_at <= ?")
        params.append(end_at)
    where_clause = f"WHERE {' AND '.join(conditions)}" if conditions else ""
    rows = connection.execute(
        f"""
        SELECT *
        FROM (
            SELECT *
            FROM portfolio_value_history
            {where_clause}
            ORDER BY recorded_at DESC, id DESC
            LIMIT ?
        )
        ORDER BY recorded_at ASC, id ASC
        """,
        (*params, int(limit)),
    ).fetchall()
    output: list[dict[str, Any]] = []
    for row in rows:
        record = dict(row)
        record["raw_payload_json"] = json.loads(record["raw_payload_json"]) if record.get("raw_payload_json") else None
        output.append(record)
    return output


def prune_portfolio_value_history(
    connection: sqlite3.Connection,
    *,
    keep_max_rows: int | None = None,
    keep_since_recorded_at: str | None = None,
) -> int:
    deleted_rows = 0
    if keep_since_recorded_at:
        cursor = connection.execute(
            """
            DELETE FROM portfolio_value_history
            WHERE recorded_at < ?
            """,
            (keep_since_recorded_at,),
        )
        deleted_rows += int(cursor.rowcount or 0)
    if keep_max_rows is not None and keep_max_rows > 0:
        cursor = connection.execute(
            """
            DELETE FROM portfolio_value_history
            WHERE id NOT IN (
                SELECT id
                FROM portfolio_value_history
                ORDER BY recorded_at DESC, id DESC
                LIMIT ?
            )
            """,
            (keep_max_rows,),
        )
        deleted_rows += int(cursor.rowcount or 0)
    connection.commit()
    return deleted_rows


def fetch_open_lot_summary(connection: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            pl.symbol,
            pl.instrument_name,
            pl.currency,
            SUM(pl.quantity_original) AS quantity_original,
            COALESCE(SUM(lr.quantity_sold), 0) AS quantity_sold,
            SUM(pl.cost_basis_total_dkk) AS cost_basis_total_dkk
        FROM position_lots pl
        LEFT JOIN lot_realizations lr ON lr.lot_id = pl.lot_id
        GROUP BY pl.symbol, pl.instrument_name, pl.currency
        ORDER BY pl.symbol
        """
    ).fetchall()
    output = []
    for row in rows:
        record = dict(row)
        record["quantity_open"] = float(record["quantity_original"]) - float(record["quantity_sold"])
        output.append(record)
    return output
