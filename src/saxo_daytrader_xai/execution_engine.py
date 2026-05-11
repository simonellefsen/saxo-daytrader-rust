from __future__ import annotations

import hashlib
import json
import math
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import requests

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import append_audit_log, connect, init_db, release_postgres_advisory_lock, try_postgres_advisory_lock
from saxo_daytrader_xai.fx_service import fetch_ecb_fx_rates, fx_rate_to_dkk
from saxo_daytrader_xai.identifier_lookup import resolve_instrument_identity
from saxo_daytrader_xai.market_data import fetch_live_prices
from saxo_daytrader_xai.market_schedule import get_market_status
from saxo_daytrader_xai.saxo_openapi import (
    SaxoOrderNotFoundError,
    SaxoSessionError,
    build_order_payload,
    cancel_order,
    change_order,
    ensure_access_token,
    get_balance_snapshot,
    get_accounts_snapshot,
    get_chart_samples,
    get_instrument_exposures,
    get_open_order,
    get_order_activity_last,
    get_positions_snapshot,
    lookup_instrument,
    normalize_order_price,
    place_order,
    precheck_order,
)
from saxo_daytrader_xai.market_symbols import parse_exchange_code, saxo_to_yahoo
from saxo_daytrader_xai.portfolio import (
    fetch_cash_summary,
    fetch_latest_batch_id,
    fetch_invalid_trade_ledger_rows,
    fetch_portfolio_positions,
    fetch_portfolio_summary,
)
from saxo_daytrader_xai.strategy_engine import (
    TERMINAL_ORDER_STATUSES,
    strategy_capital_limits,
    strategy_enabled,
)
from saxo_daytrader_xai.tax_engine import calculate_sell_outcome, update_ledger
from saxo_daytrader_xai.xai_decision import fetch_latest_decision_report


def _load_default_config() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]
    return load_config(root / "config.yaml")


MANAGEABLE_LIVE_STATUSES = {
    "submitted_to_broker",
    "broker_working",
    "broker_amended",
    "broker_partially_filled",
    "broker_replace_requested",
    "broker_cancel_requested",
}

SELL_RESERVATION_STATUSES = {
    "pending_execution",
    "pending_approval",
    "waiting_for_market_open",
    "waiting_for_cash_settlement",
    "waiting_for_virtual_cash_budget",
    "submitted_to_broker",
    "broker_working",
    "broker_amended",
    "broker_partially_filled",
    "broker_replace_requested",
    "broker_cancel_requested",
}


def _get_connection_and_config(config: dict[str, Any] | None, connection):
    resolved_config = config or _load_default_config()
    resolved_connection = connection or connect(resolved_config["portfolio"]["database_path"])
    init_db(resolved_connection)
    return resolved_config, resolved_connection, connection is None


def _current_position_map(connection, batch_id: str | None = None) -> dict[str, dict[str, Any]]:
    snapshot_positions = fetch_portfolio_positions(connection, batch_id=batch_id)
    positions = {}
    for row in snapshot_positions:
        positions[row["symbol"]] = {
            **row,
            "quantity_open": row["quantity"],
        }
    return positions


def _active_sell_reservations(connection, *, exclude_order_id: int | None = None) -> dict[str, float]:
    placeholders = ",".join("?" for _ in SELL_RESERVATION_STATUSES)
    params: list[Any] = list(SELL_RESERVATION_STATUSES)
    exclude_clause = ""
    if exclude_order_id is not None:
        exclude_clause = "AND id != ?"
        params.append(int(exclude_order_id))
    rows = connection.execute(
        f"""
        SELECT symbol, COALESCE(SUM(quantity), 0) AS reserved_quantity
        FROM execution_orders
        WHERE action = 'SELL'
          AND status IN ({placeholders})
          {exclude_clause}
        GROUP BY symbol
        """,
        tuple(params),
    ).fetchall()
    return {str(row["symbol"]): float(row["reserved_quantity"] or 0.0) for row in rows}


def _available_sell_quantity(
    connection,
    symbol: str,
    held_quantity: float,
    *,
    exclude_order_id: int | None = None,
) -> float:
    reservations = _active_sell_reservations(connection, exclude_order_id=exclude_order_id)
    reserved_quantity = float(reservations.get(symbol, 0.0))
    return max(float(held_quantity or 0.0) - reserved_quantity, 0.0)


def _broker_snapshot_quantity_map(connection) -> dict[str, float]:
    try:
        rows = connection.execute(
            """
            SELECT symbol, quantity
            FROM broker_position_snapshots
            WHERE quantity > 0
            """
        ).fetchall()
    except Exception:  # noqa: BLE001
        return {}
    return {str(row["symbol"]): float(row["quantity"] or 0.0) for row in rows}


def _get_live_price_map(symbols: list[str], config: dict[str, Any]) -> dict[str, dict[str, Any]]:
    quotes = fetch_live_prices(symbols, timeout_seconds=config["market_data"]["request_timeout_seconds"])
    return {row["symbol"]: row for row in quotes}


def _delayed_limit_order_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("execution", {}).get("delayed_price_limit_orders", {})


def _delayed_limit_price_for_order(
    *,
    symbol: str,
    action: str,
    reference_price: float,
    config: dict[str, Any],
) -> float | None:
    cfg = _delayed_limit_order_cfg(config)
    if not bool(cfg.get("enabled", True)) or reference_price <= 0:
        return None
    if action == "BUY":
        offset_bps = float(cfg.get("buy_limit_offset_bps", 20.0) or 20.0)
        raw_price = reference_price * (1.0 + (offset_bps / 10_000.0))
        side = "buy_limit"
    else:
        offset_bps = float(cfg.get("sell_limit_offset_bps", 20.0) or 20.0)
        raw_price = reference_price * max(0.0, 1.0 - (offset_bps / 10_000.0))
        side = "sell_limit"
    normalized = normalize_order_price(symbol, raw_price, config, side=side, fallback_decimals=2)
    return float(normalized if normalized is not None else round(raw_price, 2))


def _limit_replace_threshold_exceeded(old_price: float, new_price: float, config: dict[str, Any]) -> bool:
    threshold_bps = float(_delayed_limit_order_cfg(config).get("replace_threshold_bps", 10.0) or 10.0)
    if old_price <= 0:
        return True
    return abs(new_price - old_price) / old_price >= (threshold_bps / 10_000.0)


def _symbol_exchange_code(symbol: str) -> str | None:
    if ":" not in symbol:
        return None
    return symbol.split(":", 1)[1].upper()


def _market_status_for_symbol(symbol: str, config: dict[str, Any]) -> dict[str, Any] | None:
    exchange_code = _symbol_exchange_code(symbol)
    if not exchange_code:
        return None
    rows = get_market_status(config)
    return next((row for row in rows if str(row.get("code")) == exchange_code), None)


def _request_payload(order: dict[str, Any]) -> dict[str, Any]:
    payload = order.get("request_json")
    if isinstance(payload, str) and payload:
        try:
            return json.loads(payload)
        except ValueError:
            return {}
    if isinstance(payload, dict):
        return payload
    return {}


def _defer_ladder_entry_bracket(config: dict[str, Any], order: dict[str, Any], request_payload: dict[str, Any]) -> bool:
    ladder_cfg = config.get("strategy", {}).get("ladder", {})
    submit_with_entry = bool(ladder_cfg.get("submit_bracket_with_entry", False))
    return (
        not submit_with_entry
        and str(order.get("action") or "").upper() == "BUY"
        and str(order.get("strategy_type") or request_payload.get("strategy_type") or "") == "ladder"
        and str(order.get("strategy_role") or request_payload.get("strategy_role") or "") == "entry"
        and bool(request_payload.get("related_orders"))
    )


def _strategy_plan(report: dict[str, Any] | None) -> dict[str, Any]:
    if not report:
        return {}
    report_json = report.get("report_json") or {}
    if isinstance(report_json, str):
        try:
            report_json = json.loads(report_json)
        except ValueError:
            return {}
    return dict(report_json.get("strategy_plan") or {})


def _terminal_status(status: str | None) -> bool:
    return str(status or "") in TERMINAL_ORDER_STATUSES


def _strategy_row_signature(order: dict[str, Any]) -> tuple[str | None, str | None, str | None]:
    return (
        str(order.get("strategy_key") or "") or None,
        str(order.get("symbol") or "") or None,
        str(order.get("strategy_role") or "") or None,
    )


def _order_working_price(order: dict[str, Any]) -> float | None:
    for key in ("limit_price_local", "stop_price_local", "price_local"):
        value = _coerce_float(order.get(key))
        if value is not None:
            return value
    return None


def _should_auto_submit_live_orders(config: dict[str, Any]) -> bool:
    return (
        str(config["execution"]["mode"]) == "live"
        and not bool(config["execution"].get("require_approval_live", True))
        and not bool(config["app"].get("dry_run", True))
    )


def _approval_required_for_order(config: dict[str, Any]) -> bool:
    return str(config["execution"]["mode"]) == "live" and bool(config["execution"].get("require_approval_live", True))


def _cash_gate_enabled(config: dict[str, Any]) -> bool:
    return bool(config["execution"].get("require_settled_cash_for_live_buys", True))


def _prefer_broker_state(config: dict[str, Any]) -> bool:
    return (
        str(config.get("execution", {}).get("mode")) == "live"
        and str(config.get("execution", {}).get("adapter")) == "saxo"
    )


def _to_dkk_amount(amount: float | None, currency: str | None, fx_snapshot: dict[str, Any]) -> float:
    if amount is None:
        return 0.0
    return float(amount) * fx_rate_to_dkk(currency or "DKK", fx_snapshot)


def _first_numeric(payload: dict[str, Any], *keys: str) -> float | None:
    for key in keys:
        value = payload.get(key)
        if value in (None, ""):
            continue
        try:
            return float(value)
        except (TypeError, ValueError):
            continue
    return None


def _evaluate_live_buy_cash_gate(order: dict[str, Any], config: dict[str, Any], session: dict[str, Any]) -> dict[str, Any]:
    balance = get_balance_snapshot(config, session)
    fx_snapshot = fetch_ecb_fx_rates()
    balance_currency = str(balance.get("Currency") or "DKK")
    required_dkk = float(order.get("estimated_value_dkk") or 0.0)
    cash_available = _first_numeric(
        balance,
        "CashAvailableForTrading",
        "CashBalance",
        "CollateralAvailable",
        "MarginAvailableForTrading",
    )
    funds_for_settlement = _first_numeric(
        balance,
        "FundsAvailableForSettlement",
        "CashAvailableForTrading",
        "CashBalance",
        "CollateralAvailable",
    )
    transactions_not_booked = _first_numeric(balance, "TransactionsNotBooked", "SettlementValue") or 0.0
    cash_available_dkk = _to_dkk_amount(cash_available, balance_currency, fx_snapshot)
    funds_for_settlement_dkk = _to_dkk_amount(funds_for_settlement, balance_currency, fx_snapshot)
    transactions_not_booked_dkk = _to_dkk_amount(transactions_not_booked, balance_currency, fx_snapshot)

    has_cash = cash_available_dkk >= required_dkk
    settlement_ready = funds_for_settlement_dkk >= required_dkk
    pending_unbooked = abs(transactions_not_booked_dkk) > 1e-9

    allowed = has_cash and (settlement_ready or not pending_unbooked)
    return {
        "allowed": allowed,
        "required_dkk": required_dkk,
        "cash_available_dkk": cash_available_dkk,
        "funds_for_settlement_dkk": funds_for_settlement_dkk,
        "transactions_not_booked_dkk": transactions_not_booked_dkk,
        "balance_currency": balance_currency,
        "raw_balance": balance,
    }


def _evaluate_virtual_buy_budget_gate(order: dict[str, Any], config: dict[str, Any], connection) -> dict[str, Any]:
    batch_id = fetch_latest_batch_id(connection)
    initial_cash_dkk = _initial_cash_dkk(config)
    prefer_broker_cash = _prefer_broker_state(config)
    portfolio_summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    fx_snapshot = fetch_ecb_fx_rates()
    fx_rate = fx_rate_to_dkk(order["currency"], fx_snapshot)
    gross_local = float(order["price_local"] or 0.0) * float(order["quantity"] or 0.0)
    gross_dkk = gross_local * fx_rate
    commission = _calculate_buy_commission(order["symbol"], gross_local, gross_dkk, order["currency"], fx_rate, config)
    required_dkk = gross_dkk + commission["commission_dkk"]
    capital_limits = strategy_capital_limits(
        config=config,
        total_market_value_dkk=float(portfolio_summary["total_market_value_dkk"] or 0.0),
        invested_market_value_dkk=float(portfolio_summary["invested_market_value_dkk"] or 0.0),
        cash_balance_dkk=float(portfolio_summary["cash_balance_dkk"] or 0.0),
    )
    available_dkk = float(capital_limits["spendable_cash_dkk"] or 0.0)
    capital_limit_dkk = _virtual_cash_cap_dkk(config)
    return {
        "allowed": available_dkk + 1e-9 >= required_dkk,
        "required_dkk": required_dkk,
        "available_dkk": available_dkk,
        "capital_limit_dkk": capital_limit_dkk,
        "deployment_headroom_dkk": float(capital_limits["deployment_headroom_dkk"] or 0.0),
        "min_cash_buffer_dkk": float(capital_limits["min_cash_buffer_dkk"] or 0.0),
        "max_deployment_pct": float(capital_limits["max_deployment_pct"] or 0.0),
        "min_cash_buffer_pct": float(capital_limits["min_cash_buffer_pct"] or 0.0),
        "invested_market_value_dkk": float(portfolio_summary["invested_market_value_dkk"] or 0.0),
        "cash_source": portfolio_summary.get("cash_source"),
    }


def _estimate_price_and_fx(
    symbol: str,
    position_map: dict[str, dict[str, Any]],
    live_price_map: dict[str, dict[str, Any]],
    fx_snapshot: dict[str, Any],
) -> tuple[float, str, float]:
    if symbol in position_map:
        position = position_map[symbol]
        price_local = live_price_map.get(symbol, {}).get("current_price") or position["current_price_local"]
        currency = position["currency"]
        market_value_local = position.get("market_value_local")
        market_value_dkk = position.get("market_value_dkk")
        if currency == "DKK":
            fx_rate = 1.0
        elif market_value_local not in (None, 0) and market_value_dkk not in (None, 0):
            fx_rate = float(market_value_dkk) / float(market_value_local)
        else:
            fx_rate = fx_rate_to_dkk(currency, fx_snapshot)
        return float(price_local), currency, fx_rate

    quote = live_price_map.get(symbol)
    if not quote or quote.get("current_price") is None:
        raise ValueError(f"No live price available for {symbol}")
    yahoo_symbol = saxo_to_yahoo(symbol)
    suffix = yahoo_symbol.split(".")[-1] if "." in yahoo_symbol else ""
    currency = {
        "": "USD",
        "CO": "DKK",
        "ST": "SEK",
        "OL": "NOK",
        "HE": "EUR",
        "DE": "EUR",
        "PA": "EUR",
        "AS": "EUR",
        "BR": "EUR",
        "L": "GBP",
        "MI": "EUR",
    }.get(suffix, "USD")
    fx_rate = fx_rate_to_dkk(currency, fx_snapshot)
    return float(quote["current_price"]), currency, fx_rate


def _calculate_buy_commission(symbol: str, gross_local: float, gross_dkk: float, currency: str, fx_rate: float, config: dict[str, Any]) -> dict[str, float]:
    commissions_cfg = config["commissions"]
    trade_commission_local = gross_local * float(commissions_cfg["default_rate"])
    market_code = symbol.split(":", 1)[1].upper() if ":" in symbol else ""
    minimum_cfg = commissions_cfg.get("minimums", {}).get(market_code)
    if minimum_cfg:
        minimum_amount = float(minimum_cfg["amount"])
        minimum_currency = minimum_cfg["currency"]
        if minimum_currency == currency:
            trade_commission_local = max(trade_commission_local, minimum_amount)
        elif minimum_currency == "DKK":
            trade_commission_local = max(trade_commission_local, minimum_amount / max(fx_rate, 1e-9))
    fx_conversion_dkk = 0.0 if currency == "DKK" else gross_dkk * float(commissions_cfg["fx_conversion_rate"])
    trade_commission_dkk = trade_commission_local * fx_rate
    return {
        "commission_local": trade_commission_local,
        "commission_dkk": trade_commission_dkk + fx_conversion_dkk,
        "fx_conversion_dkk": fx_conversion_dkk,
    }


def _remaining_daily_order_capacity(connection, config: dict[str, Any]) -> int:
    limit = int(config["execution"]["max_daily_orders"])
    today = datetime.now(UTC).date().isoformat()
    used = connection.execute(
        """
        SELECT COUNT(*) AS count_orders
        FROM (
            SELECT id AS execution_order_id
            FROM execution_orders
            WHERE substr(created_at, 1, 10) = ?
              AND status = 'executed'
              AND ledger_id IS NOT NULL
            UNION
            SELECT execution_order_id
            FROM execution_fills
            WHERE substr(created_at, 1, 10) = ?
              AND ledger_id IS NOT NULL
        ) successful_orders
        """,
        (today, today),
    ).fetchone()["count_orders"]
    return max(limit - int(used), 0)


def _whole_share_quantity(quantity: float) -> int:
    return max(int(math.floor(float(quantity))), 0)


def _local_open_lot_quantity(connection, symbol: str) -> float:
    row = connection.execute(
        """
        SELECT COALESCE(SUM(quantity_remaining), 0) AS quantity
        FROM (
            SELECT
                pl.lot_id,
                pl.quantity_original - COALESCE(SUM(lr.quantity_sold), 0) AS quantity_remaining
            FROM position_lots pl
            LEFT JOIN lot_realizations lr ON lr.lot_id = pl.lot_id
            WHERE pl.symbol = ?
            GROUP BY pl.lot_id, pl.quantity_original
            HAVING pl.quantity_original - COALESCE(SUM(lr.quantity_sold), 0) > 0
        ) open_lots
        """,
        (symbol,),
    ).fetchone()
    return float(row["quantity"] or 0.0) if row else 0.0


def _initial_cash_dkk(config: dict[str, Any]) -> float:
    return float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0)


def _virtual_cash_cap_dkk(config: dict[str, Any]) -> float:
    portfolio_cfg = config.get("portfolio", {})
    return float(
        portfolio_cfg.get("virtual_cap_dkk")
        or portfolio_cfg.get("live_virtual_cap_dkk")
        or portfolio_cfg.get("initial_cash_dkk", 0.0)
        or 0.0
    )


def _max_affordable_buy_quantity(
    *,
    symbol: str,
    price_local: float,
    currency: str,
    fx_rate: float,
    available_cash_dkk: float,
    config: dict[str, Any],
) -> int:
    if available_cash_dkk <= 0 or price_local <= 0 or fx_rate <= 0:
        return 0
    gross_per_share_dkk = price_local * fx_rate
    quantity = _whole_share_quantity(available_cash_dkk / gross_per_share_dkk)
    while quantity > 0:
        gross_local = price_local * quantity
        gross_dkk = gross_local * fx_rate
        commission = _calculate_buy_commission(symbol, gross_local, gross_dkk, currency, fx_rate, config)
        total_spend_dkk = gross_dkk + commission["commission_dkk"]
        if total_spend_dkk <= available_cash_dkk + 1e-9:
            return quantity
        quantity -= 1
    return 0


def _dispatch_execution_alerts(connection, config: dict[str, Any]) -> dict[str, Any] | None:
    try:
        from saxo_daytrader_xai.notifications import dispatch_broker_alerts_if_due

        return dispatch_broker_alerts_if_due(connection, config, force=False)
    except Exception:  # noqa: BLE001
        return None


def _dispatch_execution_failure_alerts(connection, config: dict[str, Any]) -> None:
    _dispatch_execution_alerts(connection, config)


def _mark_execution_failed(
    connection,
    *,
    order_id: int,
    approved: bool,
    adapter: str,
    error_text: str,
) -> dict[str, Any]:
    connection.execute(
        """
        UPDATE execution_orders
        SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
        WHERE id = ?
        """,
        (
            "execution_failed",
            datetime.now(UTC).isoformat(timespec="seconds") if approved else None,
            error_text,
            json.dumps({"adapter": adapter, "error": error_text}, ensure_ascii=False, sort_keys=True),
            order_id,
        ),
    )
    connection.commit()
    return {"status": "execution_failed", "order_id": order_id, "error": error_text}


def _record_related_orders_after_submission(
    connection,
    *,
    parent_order: dict[str, Any],
    broker_payload: dict[str, Any],
    broker_result: dict[str, Any],
) -> list[int]:
    request_payload = _request_payload(parent_order)
    related_orders = list(request_payload.get("related_orders") or [])
    if not related_orders:
        return []
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    child_results = list(broker_result.get("Orders") or [])
    inserted_ids: list[int] = []
    for index, child in enumerate(related_orders):
        strategy_role = str(child.get("strategy_role") or f"child_{index}")
        strategy_key = None
        if parent_order.get("strategy_key"):
            strategy_key = f"{parent_order['strategy_key']}:{strategy_role}"
        existing = None
        if strategy_key:
            existing = connection.execute(
                """
                SELECT id
                FROM execution_orders
                WHERE strategy_key = ?
                  AND parent_execution_order_id = ?
                LIMIT 1
                """,
                (strategy_key, parent_order["id"]),
            ).fetchone()
        if existing:
            inserted_ids.append(int(existing["id"]))
            continue
        child_broker_result = child_results[index] if index < len(child_results) else {}
        order_type = str(child.get("order_type") or "Limit")
        limit_price = _coerce_float(child.get("limit_price"))
        stop_price = _coerce_float(child.get("stop_price"))
        price_local = limit_price if order_type == "Limit" else stop_price
        execution_result_json = {
            "payload": {
                "AccountKey": broker_payload.get("AccountKey"),
                "Amount": child_quantity if (child_quantity := float(child.get("quantity") or parent_order["quantity"])) else float(parent_order["quantity"]),
                "AssetType": broker_payload.get("AssetType", "Stock"),
                "BuySell": "Buy" if str(child.get("action", "SELL")).upper() == "BUY" else "Sell",
                "OrderDuration": {"DurationType": str(child.get("duration_type") or "GoodTillCancel")},
                "OrderType": order_type,
                "OrderPrice": limit_price if order_type == "Limit" else stop_price,
                "Uic": broker_payload.get("Uic"),
            },
            "broker_result": child_broker_result,
            "parent_broker_order_id": parent_order.get("broker_order_id"),
            "parent_payload": broker_payload,
        }
        cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, approved_at, broker_order_id, parent_execution_order_id,
                strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                created_at,
                parent_order.get("report_id"),
                parent_order["symbol"],
                child.get("action", "SELL"),
                order_type,
                parent_order["mode"],
                "submitted_to_broker",
                parent_order["adapter"],
                parent_order.get("requested_weight_pct"),
                float(child.get("quantity") or parent_order["quantity"]),
                price_local,
                limit_price,
                stop_price,
                parent_order.get("currency"),
                parent_order.get("estimated_value_dkk"),
                0,
                datetime.now(UTC).isoformat(timespec="seconds"),
                str(child_broker_result.get("OrderId", "")) or None,
                parent_order["id"],
                parent_order.get("strategy_type"),
                parent_order.get("strategy_session"),
                strategy_key,
                strategy_role,
                json.dumps(child, ensure_ascii=False, sort_keys=True),
                json.dumps(execution_result_json, ensure_ascii=False, sort_keys=True),
                None,
            ),
        )
        inserted_ids.append(int(cursor.lastrowid))
    connection.commit()
    return inserted_ids


def _create_ladder_protection_orders_after_fill(
    connection,
    *,
    parent_order: dict[str, Any],
    config: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    request_payload = _request_payload(parent_order)
    related_orders = list(request_payload.get("related_orders") or [])
    if not related_orders:
        return []
    ladder_cfg = ((config or {}).get("strategy", {}) or {}).get("ladder", {}) or {}
    submit_stop_after_fill = bool(ladder_cfg.get("submit_stop_loss_after_fill", False))
    submit_take_profit_after_fill = bool(ladder_cfg.get("submit_take_profit_after_fill", False))
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    inserted: list[dict[str, Any]] = []
    for index, child in enumerate(related_orders):
        strategy_role = str(child.get("strategy_role") or f"child_{index}")
        strategy_key = None
        if parent_order.get("strategy_key"):
            strategy_key = f"{parent_order['strategy_key']}:{strategy_role}"
        if strategy_key:
            existing = connection.execute(
                """
                SELECT id, status
                FROM execution_orders
                WHERE strategy_key = ?
                  AND parent_execution_order_id = ?
                LIMIT 1
                """,
                (strategy_key, parent_order["id"]),
            ).fetchone()
            if existing:
                inserted.append({"order_id": int(existing["id"]), "status": str(existing["status"]), "strategy_role": strategy_role})
                continue
        order_type = str(child.get("order_type") or "Limit")
        limit_price = _coerce_float(child.get("limit_price"))
        stop_price = _coerce_float(child.get("stop_price"))
        price_local = limit_price if order_type == "Limit" else stop_price
        if strategy_role == "stop_loss":
            status = "pending_execution" if submit_stop_after_fill else "planned_stop_loss"
        elif strategy_role == "take_profit":
            status = "pending_execution" if submit_take_profit_after_fill else "planned_take_profit"
        else:
            status = "planned_child_order"
        error_text = None
        if status == "planned_take_profit":
            error_text = "Planned take-profit level; not submitted with entry to avoid Saxo rejecting the bracket request."
        elif status == "planned_stop_loss":
            error_text = "Planned stop-loss level; automatic Saxo stop-limit submission is disabled until broker-side validation passes."
        cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, approved_at, broker_order_id, parent_execution_order_id,
                strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                created_at,
                parent_order.get("report_id"),
                parent_order["symbol"],
                child.get("action", "SELL"),
                order_type,
                parent_order["mode"],
                status,
                parent_order["adapter"],
                parent_order.get("requested_weight_pct"),
                float(child.get("quantity") or parent_order["quantity"]),
                price_local,
                limit_price,
                stop_price,
                parent_order.get("currency"),
                parent_order.get("estimated_value_dkk"),
                0,
                None,
                None,
                parent_order["id"],
                parent_order.get("strategy_type"),
                parent_order.get("strategy_session"),
                strategy_key,
                strategy_role,
                json.dumps(child, ensure_ascii=False, sort_keys=True),
                json.dumps(
                    {
                        "parent_execution_order_id": parent_order["id"],
                        "parent_broker_order_id": parent_order.get("broker_order_id"),
                        "created_after_parent_fill": True,
                    },
                    ensure_ascii=False,
                    sort_keys=True,
                ),
                error_text,
            ),
        )
        inserted.append({"order_id": int(cursor.lastrowid), "status": status, "strategy_role": strategy_role})
    connection.commit()
    return inserted


def _is_retryable_execution_failure(error_text: str | None) -> bool:
    text = str(error_text or "").casefold()
    if not text:
        return False
    retryable_markers = (
        "no valid refresh token is available",
        "rate limit exceeded",
        "timed out",
        "timeout",
        "temporarily unavailable",
        "connection aborted",
        "connection reset",
        "order not placed as other order in request was rejected",
        "service unavailable",
        "too many requests",
    )
    return any(marker in text for marker in retryable_markers)


def _current_holdings_map_for_retry(connection, config: dict[str, Any]) -> dict[str, dict[str, Any]]:
    batch_id = fetch_latest_batch_id(connection)
    initial_cash_dkk = _initial_cash_dkk(config)
    prefer_broker_cash = _prefer_broker_state(config)
    holdings = {
        row["symbol"]: dict(row)
        for row in fetch_portfolio_positions(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        )
    }
    if str(config.get("execution", {}).get("adapter") or "").lower() == "saxo":
        broker_quantities = _broker_snapshot_quantity_map(connection)
        if broker_quantities:
            return {
                symbol: {
                    **holdings.get(symbol, {"symbol": symbol}),
                    "symbol": symbol,
                    "quantity": quantity,
                    "quantity_open": quantity,
                }
                for symbol, quantity in broker_quantities.items()
            }
    return holdings


def _retry_block_reason(
    order: dict[str, Any],
    *,
    config: dict[str, Any],
    connection,
) -> str | None:
    if str(order.get("action") or "").upper() != "SELL":
        return None
    holdings = _current_holdings_map_for_retry(connection, config)
    symbol = str(order.get("symbol") or "")
    held_qty = float((holdings.get(symbol) or {}).get("quantity") or 0.0)
    available_qty = _available_sell_quantity(
        connection,
        symbol,
        held_qty,
        exclude_order_id=int(order.get("id") or 0) or None,
    )
    requested_qty = float(order.get("quantity") or 0.0)
    if available_qty + 1e-9 >= requested_qty:
        return None
    return (
        f"Retry blocked: current broker-aligned holdings for {symbol} are {held_qty:g}, "
        f"with {available_qty:g} available after active sell reservations, "
        f"below requested sell quantity {requested_qty:g}."
    )


def retry_execution_order(
    order_id: int,
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    force: bool = False,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        row = resolved_connection.execute("SELECT * FROM execution_orders WHERE id = ?", (order_id,)).fetchone()
        if not row:
            raise ValueError(f"Unknown execution order {order_id}")
        order = dict(row)
        if order["status"] != "execution_failed":
            return {"status": "not_failed", "order_id": order_id, "current_status": order["status"]}
        if not force and not _is_retryable_execution_failure(order.get("error_text")):
            return {"status": "not_retryable", "order_id": order_id, "error": order.get("error_text")}

        blocked_reason = _retry_block_reason(
            order,
            config=resolved_config,
            connection=resolved_connection,
        )
        if blocked_reason:
            previous_error = str(order.get("error_text") or "")
            updated_error = blocked_reason if not previous_error else f"{previous_error} | {blocked_reason}"
            resolved_connection.execute(
                "UPDATE execution_orders SET error_text = ? WHERE id = ?",
                (updated_error, order_id),
            )
            resolved_connection.commit()
            append_audit_log(
                resolved_connection,
                "execution_order_retry_blocked",
                {
                    "order_id": order_id,
                    "reason": blocked_reason,
                    "forced": bool(force),
                },
            )
            return {
                "status": "not_retryable_current_state",
                "order_id": order_id,
                "error": updated_error,
            }

        approval_required = bool(order.get("approval_required")) and bool(
            resolved_config.get("execution", {}).get("require_approval_live", True)
        )
        new_status = "pending_approval" if approval_required else "pending_execution"
        previous_error = str(order.get("error_text") or "")
        payload = _execution_result(order)
        if previous_error:
            payload["retry"] = {
                "previous_error": previous_error,
                "retried_at": datetime.now(UTC).isoformat(timespec="seconds"),
            }
        resolved_connection.execute(
            """
            UPDATE execution_orders
            SET status = ?, error_text = NULL, execution_result_json = ?
            WHERE id = ?
            """,
            (
                new_status,
                json.dumps(payload, ensure_ascii=False, sort_keys=True),
                order_id,
            ),
        )
        resolved_connection.commit()
        append_audit_log(
            resolved_connection,
            "execution_order_requeued",
            {
                "order_id": order_id,
                "from_status": order["status"],
                "to_status": new_status,
                "previous_error": previous_error,
                "forced": bool(force),
            },
        )
        return {
            "status": "requeued",
            "order_id": order_id,
            "new_status": new_status,
            "previous_error": previous_error,
        }
    finally:
        if should_close:
            resolved_connection.close()


def retry_failed_execution_orders(
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    recoverable_only: bool = True,
    limit: int = 100,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        rows = resolved_connection.execute(
            """
            SELECT id
            FROM execution_orders
            WHERE status = 'execution_failed'
            ORDER BY id ASC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        retried: list[dict[str, Any]] = []
        skipped: list[dict[str, Any]] = []
        for row in rows:
            result = retry_execution_order(
                int(row["id"]),
                config=resolved_config,
                connection=resolved_connection,
                force=not recoverable_only,
            )
            if result["status"] == "requeued":
                retried.append(result)
            else:
                skipped.append(result)
        return {
            "status": "ok",
            "retried": retried,
            "skipped": skipped,
        }
    finally:
        if should_close:
            resolved_connection.close()


def _create_or_fetch_orders(
    connection,
    config: dict[str, Any],
    report: dict[str, Any],
    *,
    strategy_orders_override: list[dict[str, Any]] | None = None,
) -> list[dict[str, Any]]:
    existing = connection.execute(
        "SELECT * FROM execution_orders WHERE report_id = ? ORDER BY id",
        (report["id"],),
    ).fetchall()
    if existing and strategy_orders_override is None:
        return [dict(row) for row in existing]

    report_json = report["report_json"] or {}
    strategy_plan = _strategy_plan(report)
    suggestions = report_json.get("suggested_trades", [])
    batch_id = fetch_latest_batch_id(connection)
    initial_cash_dkk = _initial_cash_dkk(config)
    prefer_broker_cash = _prefer_broker_state(config)
    portfolio_summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    position_map = {
        row["symbol"]: {**row, "quantity_open": row["quantity"]}
        for row in fetch_portfolio_positions(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        )
    }
    live_symbols = list({item.get("symbol") for item in suggestions if item.get("symbol")})
    live_symbols.extend(position_map.keys())
    live_price_map = _get_live_price_map([symbol for symbol in live_symbols if symbol], config)
    fx_snapshot = fetch_ecb_fx_rates()
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    new_orders: list[dict[str, Any]] = []
    result_orders: list[dict[str, Any]] = []
    max_position_weight = float(config["risk"]["max_position_weight"])
    min_trade_value_dkk = float(config["execution"]["min_trade_value_dkk"])
    remaining_capacity = _remaining_daily_order_capacity(connection, config)
    remaining_cash_dkk = float(portfolio_summary["cash_balance_dkk"] or 0.0)
    sell_reservations = _active_sell_reservations(connection)
    approval_required = _approval_required_for_order(config)
    desired_strategy_orders: list[dict[str, Any]] = []
    if strategy_orders_override is not None:
        desired_strategy_orders.extend(strategy_orders_override)
    elif strategy_enabled(config):
        desired_strategy_orders.extend(list(strategy_plan.get("swing_orders") or []))
        desired_strategy_orders.extend(list(strategy_plan.get("ladder_orders") or []))
    strategy_by_key: dict[str, dict[str, Any]] = {}
    if desired_strategy_orders:
        desired_keys = [str(item.get("strategy_key") or "") for item in desired_strategy_orders if item.get("strategy_key")]
        if desired_keys:
            placeholders = ",".join("?" for _ in desired_keys)
            rows = connection.execute(
                f"""
                SELECT *
                FROM execution_orders
                WHERE strategy_key IN ({placeholders})
                ORDER BY id ASC
                """,
                tuple(desired_keys),
            ).fetchall()
            strategy_by_key = {
                str(row["strategy_key"]): dict(row)
                for row in rows
                if row["strategy_key"]
            }

    for desired in desired_strategy_orders:
        if remaining_capacity <= 0:
            break
        strategy_key = str(desired.get("strategy_key") or "")
        symbol = str(desired["symbol"])
        existing_strategy_order = strategy_by_key.get(strategy_key) if strategy_key else None
        requested_weight_pct = float(desired.get("requested_weight_pct") or 0.0)
        quantity = float(desired.get("quantity") or 0.0)
        limit_price = _coerce_float(desired.get("limit_price_local"))
        stop_price = _coerce_float(desired.get("stop_price_local"))
        price_local = limit_price or stop_price or _coerce_float(desired.get("price_local"))
        if existing_strategy_order:
            if existing_strategy_order["status"] in {
                "pending_execution",
                "pending_approval",
                "waiting_for_market_open",
                "waiting_for_cash_settlement",
                "waiting_for_virtual_cash_budget",
            }:
                connection.execute(
                    """
                    UPDATE execution_orders
                    SET report_id = ?, quantity = ?, price_local = ?, limit_price_local = ?, stop_price_local = ?,
                        requested_weight_pct = ?, request_json = ?, error_text = NULL
                    WHERE id = ?
                    """,
                    (
                        report["id"],
                        quantity,
                        price_local,
                        limit_price,
                        stop_price,
                        requested_weight_pct,
                        json.dumps(desired, ensure_ascii=False, sort_keys=True),
                        existing_strategy_order["id"],
                    ),
                )
                connection.commit()
                result_orders.append(dict(connection.execute("SELECT * FROM execution_orders WHERE id = ?", (existing_strategy_order["id"],)).fetchone()))
            else:
                result_orders.append(existing_strategy_order)
            continue
        market_row = _market_status_for_symbol(symbol, config)
        if market_row is not None and not bool(market_row.get("is_tradable", market_row.get("is_open"))):
            append_audit_log(
                connection,
                "execution_order_skipped_market_closed",
                {
                    "report_id": report["id"],
                    "symbol": symbol,
                    "action": desired["action"],
                    "status_reason": market_row.get("status_reason"),
                    "next_open": market_row.get("next_open"),
                    "strategy_key": strategy_key,
                },
            )
            continue
        new_orders.append(
            {
                "symbol": symbol,
                "action": desired["action"],
                "order_type": str(desired.get("order_type") or "Market"),
                "mode": config["execution"]["mode"],
                "status": "pending_approval" if approval_required else "pending_execution",
                "adapter": config["execution"]["adapter"],
                "requested_weight_pct": requested_weight_pct,
                "quantity": quantity,
                "price_local": price_local,
                "limit_price_local": limit_price,
                "stop_price_local": stop_price,
                "currency": desired.get("currency"),
                "estimated_value_dkk": float(desired.get("estimated_value_dkk") or 0.0),
                "approval_required": 1 if approval_required else 0,
                "parent_execution_order_id": None,
                "strategy_type": str(desired.get("strategy_type") or "ladder"),
                "strategy_session": desired.get("session_tag"),
                "strategy_key": strategy_key,
                "strategy_role": desired.get("strategy_role"),
                "request_json": json.dumps(desired, ensure_ascii=False, sort_keys=True),
                "execution_result_json": None,
                "error_text": None,
            }
        )
        remaining_capacity -= 1

    for suggestion in suggestions:
        if remaining_capacity <= 0:
            break
        action = suggestion["action"]
        symbol = suggestion["symbol"]
        if strategy_enabled(config) and str(strategy_plan.get("mode") or "").lower() == "swing":
            append_audit_log(
                connection,
                "execution_order_skipped_strategy_swing_fallback",
                {
                    "report_id": report["id"],
                    "symbol": symbol,
                    "action": action,
                    "strategy_status": strategy_plan.get("status"),
                    "reason": "Swing strategy orders execute from strategy_plan.swing_orders; fallback suggestion queueing is disabled.",
                },
            )
            continue
        if action not in {"BUY", "SELL"}:
            continue
        if strategy_enabled(config) and action == "BUY":
            append_audit_log(
                connection,
                "execution_order_skipped_strategy_buy_fallback",
                {
                    "report_id": report["id"],
                    "symbol": symbol,
                    "strategy_status": strategy_plan.get("status"),
                    "reason": "Strategy-managed buys only execute from ladder orders; fallback market buys are disabled.",
                },
            )
            continue
        market_row = _market_status_for_symbol(symbol, config)
        if market_row is not None and not bool(market_row.get("is_tradable", market_row.get("is_open"))):
            append_audit_log(
                connection,
                "execution_order_skipped_market_closed",
                {
                    "report_id": report["id"],
                    "symbol": symbol,
                    "action": action,
                    "status_reason": market_row.get("status_reason"),
                    "next_open": market_row.get("next_open"),
                },
            )
            continue
        requested_weight_pct = float(suggestion["target_weight_pct"])
        if requested_weight_pct > 1.0:
            requested_weight_pct = requested_weight_pct / 100.0
        requested_weight_pct = min(requested_weight_pct, max_position_weight)
        try:
            price_local, currency, fx_rate = _estimate_price_and_fx(symbol, position_map, live_price_map, fx_snapshot)
        except Exception as exc:  # noqa: BLE001
            new_orders.append(
                {
                    "symbol": symbol,
                    "action": action,
                    "order_type": "Market",
                    "mode": config["execution"]["mode"],
                    "status": "error",
                    "adapter": config["execution"]["adapter"],
                    "requested_weight_pct": requested_weight_pct,
                    "quantity": 0.0,
                    "price_local": None,
                    "limit_price_local": None,
                    "stop_price_local": None,
                    "currency": None,
                    "estimated_value_dkk": 0.0,
                    "approval_required": 1 if approval_required else 0,
                    "parent_execution_order_id": None,
                    "strategy_type": None,
                    "strategy_session": None,
                    "strategy_key": None,
                    "strategy_role": None,
                    "request_json": json.dumps(suggestion, ensure_ascii=False, sort_keys=True),
                    "execution_result_json": None,
                    "error_text": str(exc),
                }
            )
            continue

        current_value_dkk = 0.0
        current_quantity = 0.0
        if symbol in position_map:
            current_quantity = float(position_map[symbol]["quantity_open"])
            current_value_dkk = current_quantity * price_local * fx_rate
        available_sell_quantity = current_quantity
        if action == "SELL":
            available_sell_quantity = max(current_quantity - float(sell_reservations.get(symbol, 0.0)), 0.0)
        target_value_dkk = float(portfolio_summary["total_market_value_dkk"]) * requested_weight_pct
        delta_value_dkk = target_value_dkk - current_value_dkk
        if action == "BUY" and delta_value_dkk <= 0:
            continue
        if action == "SELL" and delta_value_dkk >= 0:
            continue
        quantity = abs(delta_value_dkk) / max(price_local * fx_rate, 1e-9)
        if action == "SELL":
            quantity = min(quantity, available_sell_quantity)
            whole_quantity = _whole_share_quantity(quantity)
            estimated_value_dkk = whole_quantity * price_local * fx_rate
        else:
            capped_delta_value_dkk = min(delta_value_dkk, max(remaining_cash_dkk, 0.0))
            target_quantity = capped_delta_value_dkk / max(price_local * fx_rate, 1e-9)
            whole_quantity = min(
                _whole_share_quantity(quantity),
                _max_affordable_buy_quantity(
                    symbol=symbol,
                    price_local=price_local,
                    currency=currency,
                    fx_rate=fx_rate,
                    available_cash_dkk=max(remaining_cash_dkk, 0.0),
                    config=config,
                ),
                _whole_share_quantity(target_quantity),
            )
            gross_local = whole_quantity * price_local
            estimated_value_dkk = gross_local * fx_rate
        if whole_quantity <= 0 or estimated_value_dkk < min_trade_value_dkk:
            continue
        if action == "BUY":
            gross_local = whole_quantity * price_local
            gross_dkk = gross_local * fx_rate
            commission = _calculate_buy_commission(symbol, gross_local, gross_dkk, currency, fx_rate, config)
            remaining_cash_dkk -= gross_dkk + commission["commission_dkk"]
        else:
            try:
                sell_outcome = calculate_sell_outcome(
                    symbol,
                    float(whole_quantity),
                    float(price_local),
                    config=config,
                    connection=connection,
                    batch_id=batch_id,
                    tax_year=datetime.now(UTC).year,
                )
                remaining_cash_dkk += float(sell_outcome["net_DKK"])
            except ValueError:
                pass
            sell_reservations[symbol] = float(sell_reservations.get(symbol, 0.0)) + float(whole_quantity)

        new_orders.append(
            {
                "symbol": symbol,
                "action": action,
                "order_type": "Market",
                "mode": config["execution"]["mode"],
                "status": "pending_approval" if approval_required else "pending_execution",
                "adapter": config["execution"]["adapter"],
                "requested_weight_pct": requested_weight_pct,
                "quantity": float(whole_quantity),
                "price_local": price_local,
                "limit_price_local": None,
                "stop_price_local": None,
                "currency": currency,
                "estimated_value_dkk": estimated_value_dkk,
                "approval_required": 1 if approval_required else 0,
                "parent_execution_order_id": None,
                "strategy_type": None,
                "strategy_session": None,
                "strategy_key": None,
                "strategy_role": None,
                "request_json": json.dumps(suggestion, ensure_ascii=False, sort_keys=True),
                "execution_result_json": None,
                "error_text": None,
            }
        )
        remaining_capacity -= 1

    connection.executemany(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, order_type, mode, status, adapter,
            requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
            approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
            request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        [
            (
                created_at,
                report["id"],
                order["symbol"],
                order["action"],
                order.get("order_type", "Market"),
                order["mode"],
                order["status"],
                order["adapter"],
                order["requested_weight_pct"],
                order["quantity"],
                order["price_local"],
                order.get("limit_price_local"),
                order.get("stop_price_local"),
                order["currency"],
                order["estimated_value_dkk"],
                order["approval_required"],
                order.get("parent_execution_order_id"),
                order.get("strategy_type"),
                order.get("strategy_session"),
                order.get("strategy_key"),
                order.get("strategy_role"),
                order["request_json"],
                order["execution_result_json"],
                order["error_text"],
            )
            for order in new_orders
        ],
    )
    connection.commit()
    created = connection.execute(
        "SELECT * FROM execution_orders WHERE report_id = ? ORDER BY id",
        (report["id"],),
    ).fetchall()
    created_orders = [dict(row) for row in created]
    created_orders.extend(result_orders)
    if desired_strategy_orders:
        created_orders.extend(
            [
                row
                for key, row in strategy_by_key.items()
                if key in {str(item.get("strategy_key") or "") for item in desired_strategy_orders}
                and row["id"] not in {created_row["id"] for created_row in created_orders}
            ]
        )
    deduped: list[dict[str, Any]] = []
    seen_ids: set[int] = set()
    for row in created_orders:
        row_id = int(row["id"])
        if row_id in seen_ids:
            continue
        seen_ids.add(row_id)
        deduped.append(row)
    return deduped


def _flatten_due_for_symbol(symbol: str, config: dict[str, Any]) -> bool:
    if not strategy_enabled(config):
        return False
    market_row = _market_status_for_symbol(symbol, config)
    if not market_row or not bool(market_row.get("is_tradable")):
        return False
    tradable_close_at = market_row.get("tradable_close_at_utc")
    if not tradable_close_at:
        return False
    try:
        close_dt = datetime.fromisoformat(str(tradable_close_at))
    except ValueError:
        return False
    flatten_minutes = int(config.get("strategy", {}).get("ladder", {}).get("flatten_minutes_before_tradable_close", 15) or 15)
    now = datetime.now(UTC)
    return close_dt - timedelta(minutes=flatten_minutes) <= now < close_dt


def enqueue_session_flatten_orders(*, config: dict[str, Any] | None = None, connection=None) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        if not bool(resolved_config.get("strategy", {}).get("ladder", {}).get("session_flatten_enabled", False)):
            return {
                "status": "disabled",
                "created_order_ids": [],
                "message": "Session-close flattening is disabled by strategy configuration.",
            }
        if str(resolved_config.get("execution", {}).get("mode")) != "live":
            return {"status": "skipped", "created_order_ids": []}
        if str(resolved_config.get("execution", {}).get("adapter")) != "saxo":
            return {"status": "skipped", "created_order_ids": []}
        batch_id = fetch_latest_batch_id(resolved_connection)
        initial_cash_dkk = _initial_cash_dkk(resolved_config)
        prefer_broker_cash = _prefer_broker_state(resolved_config)
        positions = fetch_portfolio_positions(
            resolved_connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        )
        broker_quantities = _broker_snapshot_quantity_map(resolved_connection)
        if broker_quantities:
            active_statuses = tuple(SELL_RESERVATION_STATUSES)
            status_placeholders = ",".join("?" for _ in active_statuses)
            symbol_placeholders = ",".join("?" for _ in broker_quantities)
            resolved_connection.execute(
                f"""
                UPDATE execution_orders
                SET status = ?, error_text = ?
                WHERE action = 'SELL'
                  AND strategy_type = 'flatten'
                  AND status IN ({status_placeholders})
                  AND symbol NOT IN ({symbol_placeholders})
                """,
                (
                    "cancelled",
                    "Cancelled because latest Saxo broker snapshot has no held quantity for session flatten.",
                    *active_statuses,
                    *tuple(broker_quantities.keys()),
                ),
            )
        created: list[int] = []
        for position in positions:
            symbol = str(position["symbol"])
            broker_aligned_quantity = _whole_share_quantity(float(position["quantity"] or 0.0))
            if broker_quantities:
                broker_aligned_quantity = _whole_share_quantity(float(broker_quantities.get(symbol, 0.0)))
            local_lot_quantity = _whole_share_quantity(_local_open_lot_quantity(resolved_connection, symbol))
            quantity = min(broker_aligned_quantity, local_lot_quantity)
            if broker_aligned_quantity > local_lot_quantity:
                append_audit_log(
                    resolved_connection,
                    "flatten_quantity_capped_to_local_lots",
                    {
                        "symbol": symbol,
                        "broker_aligned_quantity": broker_aligned_quantity,
                        "local_lot_quantity": local_lot_quantity,
                        "flatten_quantity": quantity,
                    },
                )
            if quantity <= 0 or not _flatten_due_for_symbol(symbol, resolved_config):
                continue
            existing_flatten = resolved_connection.execute(
                """
                SELECT id
                FROM execution_orders
                WHERE symbol = ?
                  AND strategy_type = 'flatten'
                  AND status NOT IN ({})
                LIMIT 1
                """.format(",".join("?" for _ in TERMINAL_ORDER_STATUSES)),
                (symbol, *tuple(TERMINAL_ORDER_STATUSES)),
            ).fetchone()
            if existing_flatten:
                continue
            pending_rows = resolved_connection.execute(
                """
                SELECT *
                FROM execution_orders
                WHERE symbol = ?
                  AND mode = 'live'
                  AND status NOT IN ({})
                  AND (strategy_type IS NULL OR strategy_type != 'flatten')
                ORDER BY id ASC
                """.format(",".join("?" for _ in TERMINAL_ORDER_STATUSES)),
                (symbol, *tuple(TERMINAL_ORDER_STATUSES)),
            ).fetchall()
            for row in pending_rows:
                pending_order = dict(row)
                if pending_order["status"] in MANAGEABLE_LIVE_STATUSES:
                    try:
                        manage_live_order(
                            int(pending_order["id"]),
                            management_action="cancel",
                            config=resolved_config,
                            connection=resolved_connection,
                        )
                    except Exception:  # noqa: BLE001
                        pass
                elif pending_order["status"] in {
                    "pending_execution",
                    "pending_approval",
                    "waiting_for_market_open",
                    "waiting_for_cash_settlement",
                    "waiting_for_virtual_cash_budget",
                }:
                    resolved_connection.execute(
                        "UPDATE execution_orders SET status = ?, error_text = ? WHERE id = ?",
                        ("cancelled", "Cancelled due to session-close flatten window", int(pending_order["id"])),
                    )
            cursor = resolved_connection.execute(
                """
                INSERT INTO execution_orders (
                    created_at, report_id, symbol, action, order_type, mode, status, adapter,
                    requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                    approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                    request_json, execution_result_json, error_text
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    datetime.now(UTC).isoformat(timespec="seconds"),
                    None,
                    symbol,
                    "SELL",
                    "Market",
                    "live",
                    "pending_execution",
                    resolved_config["execution"]["adapter"],
                    0.0,
                    float(quantity),
                    _coerce_float(position.get("current_price_local")),
                    None,
                    None,
                    position.get("currency"),
                    _coerce_float(position.get("market_value_dkk")) or 0.0,
                    0,
                    None,
                    "flatten",
                    parse_exchange_code(symbol),
                    f"flatten:{symbol}:{datetime.now(UTC).date().isoformat()}",
                    "flatten_close",
                    json.dumps(
                        {
                            "symbol": symbol,
                            "action": "SELL",
                            "order_type": "Market",
                            "strategy_type": "flatten",
                            "strategy_role": "flatten_close",
                            "reason": "Session close flatten window",
                        },
                        ensure_ascii=False,
                        sort_keys=True,
                    ),
                    None,
                    None,
                ),
            )
            created.append(int(cursor.lastrowid))
        resolved_connection.commit()
        return {"status": "ok", "created_order_ids": created}
    finally:
        if should_close:
            resolved_connection.close()


def _record_buy_trade(connection, config: dict[str, Any], order: dict[str, Any], batch_id: str) -> dict[str, Any]:
    created_at = datetime.now(UTC).isoformat(timespec="seconds")
    price_local = float(order["price_local"])
    quantity = float(_whole_share_quantity(float(order["quantity"])))
    currency = order["currency"]
    initial_cash_dkk = _initial_cash_dkk(config)
    prefer_broker_cash = _prefer_broker_state(config)
    portfolio_before = {
        "summary": fetch_portfolio_summary(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        ),
        "positions": fetch_portfolio_positions(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        ),
    }
    fx_snapshot = fetch_ecb_fx_rates()
    fx_rate = fx_rate_to_dkk(currency, fx_snapshot)
    gross_local = price_local * quantity
    gross_dkk = gross_local * fx_rate
    commission = _calculate_buy_commission(order["symbol"], gross_local, gross_dkk, currency, fx_rate, config)
    total_spend_dkk = gross_dkk + commission["commission_dkk"]
    identity = resolve_instrument_identity(order["symbol"], currency=currency, config=config)
    available_cash_dkk = float(portfolio_before["summary"]["cash_balance_dkk"])
    if total_spend_dkk > available_cash_dkk + 1e-9:
        raise ValueError(
            f"Insufficient cash to buy {int(quantity)} shares of {order['symbol']}; "
            f"need {total_spend_dkk:.2f} DKK, have {available_cash_dkk:.2f} DKK"
        )
    cursor = connection.execute(
        """
        INSERT INTO trade_ledger (
            created_at, symbol, isin, figi, instrument_name, side, quantity, price_local, currency,
            gross_amount_dkk, commission_dkk, commission_local, fx_conversion_dkk, tax_dkk,
            realised_gain_dkk, cost_basis_sold_dkk, net_amount_dkk, mode, status, notes,
            portfolio_before_json, portfolio_after_json, decision_context_json, tax_year, batch_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            created_at,
            order["symbol"],
            identity.isin,
            identity.figi,
            identity.instrument_name,
            "BUY",
            quantity,
            price_local,
            currency,
            gross_dkk,
            commission["commission_dkk"],
            commission["commission_local"],
            commission["fx_conversion_dkk"],
            0.0,
            0.0,
            0.0,
            -total_spend_dkk,
            order["mode"],
            "executed" if order["mode"] == "simulation" else "approved",
            "Phase 5 buy execution",
            json.dumps(portfolio_before, ensure_ascii=False, sort_keys=True),
            json.dumps({}, ensure_ascii=False, sort_keys=True),
            order["request_json"],
            datetime.now(UTC).year,
            batch_id,
        ),
    )
    ledger_id = int(cursor.lastrowid)
    lot_id = f"buy:{ledger_id}"
    connection.execute(
        """
        INSERT INTO position_lots (
            lot_id, batch_id, created_at, acquired_at, symbol, isin, figi, instrument_name,
            quantity_original, currency, cost_basis_total_local, cost_basis_total_dkk,
            fx_rate_to_dkk, source_type, source_reference, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            lot_id,
            batch_id,
            created_at,
            created_at,
            order["symbol"],
            identity.isin,
            identity.figi,
            identity.instrument_name,
            quantity,
            currency,
            gross_local + commission["commission_local"],
            gross_dkk + commission["commission_dkk"],
            fx_rate,
            f"{order['mode']}_buy",
            f"execution_order:{order['id']}",
            json.dumps(
                {
                    "request": json.loads(order["request_json"]) if order.get("request_json") else {},
                    "identity_source": identity.source,
                    "figi": identity.figi,
                    "isin": identity.isin,
                },
                ensure_ascii=False,
                sort_keys=True,
            ),
        ),
    )
    connection.commit()
    portfolio_after = {
        "summary": fetch_portfolio_summary(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        ),
        "positions": fetch_portfolio_positions(
            connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            prefer_broker_cash=prefer_broker_cash,
        ),
    }
    connection.execute(
        "UPDATE trade_ledger SET portfolio_after_json = ? WHERE id = ?",
        (json.dumps(portfolio_after, ensure_ascii=False, sort_keys=True), ledger_id),
    )
    connection.commit()
    return {"ledger_id": ledger_id, "lot_id": lot_id}


def _synced_fill_quantity(connection, execution_order_id: int) -> float:
    row = connection.execute(
        """
        SELECT COALESCE(SUM(delta_quantity), 0) AS synced_quantity
        FROM execution_fills
        WHERE execution_order_id = ?
        """,
        (execution_order_id,),
    ).fetchone()
    return float(row["synced_quantity"]) if row else 0.0


def _record_execution_fill(
    connection,
    *,
    order: dict[str, Any],
    broker_order_id: str | None,
    fill_status: str,
    cumulative_quantity: float,
    delta_quantity: float,
    average_price_local: float,
    currency: str,
    ledger_id: int | None,
    payload: dict[str, Any],
) -> int:
    cursor = connection.execute(
        """
        INSERT INTO execution_fills (
            created_at, execution_order_id, broker_order_id, symbol, side, fill_status,
            cumulative_quantity, delta_quantity, average_price_local, currency, ledger_id, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            order["id"],
            broker_order_id,
            order["symbol"],
            order["action"],
            fill_status,
            cumulative_quantity,
            delta_quantity,
            average_price_local,
            currency,
            ledger_id,
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
        ),
    )
    return int(cursor.lastrowid)


def _is_portfolio_sync_order(order: dict[str, Any]) -> bool:
    return str(order.get("strategy_type") or "") == "portfolio_sync"


def _coerce_float(value: Any) -> float | None:
    if value in (None, ""):
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _extract_broker_quantity(payload: dict[str, Any]) -> float | None:
    for key in ("Amount", "CurrentAmount", "OrderAmount", "LeavesAmount", "OriginalAmount"):
        value = _coerce_float(payload.get(key))
        if value is not None:
            return value
    return None


def _execution_result(order: dict[str, Any]) -> dict[str, Any]:
    return json.loads(order["execution_result_json"]) if order.get("execution_result_json") else {}


def _broker_payload(order: dict[str, Any]) -> dict[str, Any]:
    return _execution_result(order).get("payload", {})


def _extract_broker_price(payload: dict[str, Any]) -> float | None:
    for key in ("OrderPrice", "Price", "OrderPriceDisplay"):
        value = _coerce_float(payload.get(key))
        if value is not None:
            return value
    return None


def _event_signature(order_id: int, event_type: str, payload: dict[str, Any]) -> str:
    def sanitize(value: Any) -> Any:
        if isinstance(value, dict):
            return {
                key: sanitize(item)
                for key, item in value.items()
                if key not in {"last_sync_at"}
            }
        if isinstance(value, list):
            return [sanitize(item) for item in value]
        return value

    serialized = json.dumps(
        sanitize(
            {
            "order_id": order_id,
            "event_type": event_type,
            "payload": payload,
            }
        ),
        ensure_ascii=False,
        sort_keys=True,
        default=str,
    )
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


def _record_execution_event(
    connection,
    *,
    order: dict[str, Any],
    broker_order_id: str | None,
    event_type: str,
    broker_status: str | None,
    broker_substatus: str | None,
    broker_quantity: float | None,
    broker_price_local: float | None,
    payload: dict[str, Any],
) -> int | None:
    signature = _event_signature(
        int(order["id"]),
        event_type,
        {
            "broker_order_id": broker_order_id,
            "broker_status": broker_status,
            "broker_substatus": broker_substatus,
            "broker_quantity": broker_quantity,
            "broker_price_local": broker_price_local,
        },
    )
    existing = connection.execute(
        "SELECT id FROM execution_order_events WHERE event_signature = ?",
        (signature,),
    ).fetchone()
    if existing:
        return None
    cursor = connection.execute(
        """
        INSERT INTO execution_order_events (
            created_at, execution_order_id, broker_order_id, event_type,
            broker_status, broker_substatus, broker_quantity, broker_price_local,
            event_signature, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            order["id"],
            broker_order_id,
            event_type,
            broker_status,
            broker_substatus,
            broker_quantity,
            broker_price_local,
            signature,
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
        ),
    )
    return int(cursor.lastrowid)


def _sync_incremental_live_fill(
    connection,
    config: dict[str, Any],
    order: dict[str, Any],
    activity: dict[str, Any],
    *,
    broker_order_id: str | None,
    fill_status: str,
) -> dict[str, Any]:
    batch_id = fetch_latest_batch_id(connection)
    filled_quantity = float(activity.get("FilledAmount") or order["quantity"])
    average_price = float(activity.get("AveragePrice") or order["price_local"])
    already_synced = _synced_fill_quantity(connection, int(order["id"]))
    delta_quantity = max(filled_quantity - already_synced, 0.0)
    if delta_quantity <= 1e-9:
        return {
            "ledger_id": None,
            "fill_id": None,
            "delta_quantity": 0.0,
            "cumulative_quantity": filled_quantity,
            "status": "no_new_fill",
        }

    if _is_portfolio_sync_order(order):
        fill_payload = {
            **activity,
            "local_reconciliation": {
                "status": "broker_only_portfolio_sync",
                "note": (
                    "Portfolio-sync orders mirror the existing local ledger into Saxo SIM; "
                    "broker fills must not create additional local tax lots or cash movements."
                ),
            },
        }
        fill_id = _record_execution_fill(
            connection,
            order=order,
            broker_order_id=broker_order_id,
            fill_status=fill_status,
            cumulative_quantity=filled_quantity,
            delta_quantity=delta_quantity,
            average_price_local=average_price,
            currency=str(order["currency"]),
            ledger_id=None,
            payload=fill_payload,
        )
        connection.commit()
        return {
            "ledger_id": None,
            "fill_id": fill_id,
            "delta_quantity": delta_quantity,
            "cumulative_quantity": filled_quantity,
            "status": "portfolio_sync_broker_fill_synced",
            "local_reconciliation": fill_payload["local_reconciliation"],
        }

    synced_order = {**order, "quantity": delta_quantity, "price_local": average_price}

    if order["action"] == "SELL":
        ledger_quantity = delta_quantity
        broker_only_quantity = 0.0
        reconciliation_note = None
        try:
            trade = calculate_sell_outcome(
                order["symbol"],
                ledger_quantity,
                average_price,
                config=config,
                connection=connection,
                batch_id=batch_id,
                tax_year=datetime.now(UTC).year,
            )
        except ValueError:
            local_open_quantity = _local_open_lot_quantity(connection, str(order["symbol"]))
            if local_open_quantity <= 1e-9 or local_open_quantity >= delta_quantity - 1e-9:
                raise
            ledger_quantity = min(delta_quantity, local_open_quantity)
            broker_only_quantity = max(delta_quantity - ledger_quantity, 0.0)
            trade = calculate_sell_outcome(
                order["symbol"],
                ledger_quantity,
                average_price,
                config=config,
                connection=connection,
                batch_id=batch_id,
                tax_year=datetime.now(UTC).year,
            )
            reconciliation_note = (
                f"Broker filled {delta_quantity:g} shares, but local tax lots only covered "
                f"{ledger_quantity:g}; {broker_only_quantity:g} broker-only shares were not booked to tax lots."
            )
        trade["mode"] = order["mode"]
        trade["status"] = "executed"
        trade["notes"] = f"Saxo broker {fill_status.lower()} sync"
        if reconciliation_note:
            trade["notes"] = f"{trade['notes']} | {reconciliation_note}"
        trade["decision_context"] = {
            **activity,
            "local_ledger_quantity": ledger_quantity,
            "broker_filled_quantity": delta_quantity,
            "broker_only_quantity": broker_only_quantity,
            "reconciliation_note": reconciliation_note,
        }
        result = update_ledger(trade, config=config, connection=connection)
        result["ledger_quantity"] = ledger_quantity
        result["broker_only_quantity"] = broker_only_quantity
        if reconciliation_note:
            result["reconciliation_note"] = reconciliation_note
    else:
        result = _record_buy_trade(connection, config, synced_order, batch_id)
        connection.execute(
            """
            UPDATE trade_ledger
            SET notes = ?, decision_context_json = ?
            WHERE id = ?
            """,
            (
                f"Saxo broker {fill_status.lower()} sync",
                json.dumps(activity, ensure_ascii=False, sort_keys=True),
                result["ledger_id"],
            ),
        )
        connection.commit()
    fill_id = _record_execution_fill(
        connection,
        order=order,
        broker_order_id=broker_order_id,
        fill_status=fill_status,
        cumulative_quantity=filled_quantity,
        delta_quantity=delta_quantity,
        average_price_local=average_price,
        currency=str(order["currency"]),
        ledger_id=result["ledger_id"],
        payload=activity,
    )
    connection.commit()
    return {
        **result,
        "fill_id": fill_id,
        "delta_quantity": delta_quantity,
        "cumulative_quantity": filled_quantity,
    }


def _record_unreconciled_broker_fill(
    connection,
    *,
    order: dict[str, Any],
    broker_order_id: str,
    activity_status: str,
    activity_substatus: str,
    broker_quantity: float | None,
    broker_price: float | None,
    payload: dict[str, Any],
    error_text: str,
) -> int | None:
    event_payload = {
        **payload,
        "reconciliation_error": error_text,
    }
    event_id = _record_execution_event(
        connection,
        order=order,
        broker_order_id=broker_order_id,
        event_type="broker_fill_unreconciled",
        broker_status=activity_status,
        broker_substatus=activity_substatus,
        broker_quantity=broker_quantity,
        broker_price_local=broker_price,
        payload=event_payload,
    )
    connection.execute(
        """
        UPDATE execution_orders
        SET status = ?, error_text = ?, execution_result_json = ?
        WHERE id = ?
        """,
        (
            "broker_fill_unreconciled",
            error_text,
            json.dumps(event_payload, ensure_ascii=False, sort_keys=True),
            order["id"],
        ),
    )
    append_audit_log(
        connection,
        "execution_order_fill_unreconciled",
        {
            "order_id": order["id"],
            "broker_order_id": broker_order_id,
            "error": error_text,
            "event_id": event_id,
        },
    )
    return event_id


def _record_broker_sync_error(
    connection,
    *,
    order: dict[str, Any],
    broker_order_id: str | None,
    event_type: str,
    payload: dict[str, Any],
    error_text: str,
) -> int | None:
    event_payload = {
        **payload,
        "sync_error": error_text,
    }
    event_id = _record_execution_event(
        connection,
        order=order,
        broker_order_id=broker_order_id,
        event_type=event_type,
        broker_status=str(order.get("status") or ""),
        broker_substatus="error",
        broker_quantity=_coerce_float(order.get("quantity")),
        broker_price_local=_coerce_float(order.get("price_local")),
        payload=event_payload,
    )
    connection.execute(
        """
        UPDATE execution_orders
        SET error_text = ?, execution_result_json = ?
        WHERE id = ?
        """,
        (
            error_text,
            json.dumps(event_payload, ensure_ascii=False, sort_keys=True),
            order["id"],
        ),
    )
    append_audit_log(
        connection,
        "execution_order_broker_sync_error",
        {
            "order_id": order["id"],
            "broker_order_id": broker_order_id,
            "event_type": event_type,
            "event_id": event_id,
            "error": error_text,
        },
    )
    return event_id


def refresh_broker_position_snapshots(
    connection,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    snapshot_rows = get_positions_snapshot(config, session)
    refreshed_at = datetime.now(UTC).isoformat(timespec="seconds")
    aggregated_rows: dict[str, dict[str, Any]] = {}
    for row in snapshot_rows:
        display = dict(row.get("DisplayAndFormat") or {})
        position_base = dict(row.get("PositionBase") or {})
        position_view = dict(row.get("PositionView") or {})
        symbol = str(display.get("Symbol") or "").strip()
        if not symbol:
            continue
        quantity = float(position_base.get("Amount") or 0.0)
        open_price_local = float(position_base.get("OpenPrice") or 0.0)
        open_price_including_costs_local = float(position_base.get("OpenPriceIncludingCosts") or open_price_local or 0.0)
        existing = aggregated_rows.get(symbol)
        if existing is None:
            aggregated_rows[symbol] = {
                "symbol": symbol,
                "updated_at": refreshed_at,
                "instrument_name": display.get("Description") or display.get("InstrumentType") or symbol,
                "isin": display.get("IsinCode"),
                "uic": position_base.get("Uic"),
                "asset_type": position_base.get("AssetType"),
                "quantity": quantity,
                "currency": display.get("Currency"),
                "open_price_local": open_price_local,
                "open_price_including_costs_local": open_price_including_costs_local,
                "execution_time_open": position_base.get("ExecutionTimeOpen"),
                "value_date": position_base.get("ValueDate"),
                "market_state": position_view.get("MarketState"),
                "can_be_closed": 1 if bool(position_base.get("CanBeClosed")) else 0,
                "raw_payload_json": json.dumps([row], ensure_ascii=False, sort_keys=True),
            }
            continue
        combined_quantity = float(existing["quantity"]) + quantity
        if combined_quantity > 0:
            existing["open_price_local"] = (
                (float(existing["open_price_local"] or 0.0) * float(existing["quantity"]) + open_price_local * quantity)
                / combined_quantity
            )
            existing["open_price_including_costs_local"] = (
                (
                    float(existing["open_price_including_costs_local"] or 0.0) * float(existing["quantity"])
                    + open_price_including_costs_local * quantity
                )
                / combined_quantity
            )
        existing["quantity"] = combined_quantity
        existing["execution_time_open"] = min(
            [
                value
                for value in [existing.get("execution_time_open"), position_base.get("ExecutionTimeOpen")]
                if value
            ],
            default=existing.get("execution_time_open"),
        )
        raw_payload = json.loads(existing["raw_payload_json"])
        raw_payload.append(row)
        existing["raw_payload_json"] = json.dumps(raw_payload, ensure_ascii=False, sort_keys=True)
    resolved_rows = list(aggregated_rows.values())
    connection.execute("DELETE FROM broker_position_snapshots")
    if resolved_rows:
        connection.executemany(
            """
            INSERT INTO broker_position_snapshots (
                symbol, updated_at, instrument_name, isin, uic, asset_type, quantity, currency,
                open_price_local, open_price_including_costs_local, execution_time_open, value_date,
                market_state, can_be_closed, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                (
                    row["symbol"],
                    row["updated_at"],
                    row["instrument_name"],
                    row["isin"],
                    row["uic"],
                    row["asset_type"],
                    row["quantity"],
                    row["currency"],
                    row["open_price_local"],
                    row["open_price_including_costs_local"],
                    row["execution_time_open"],
                    row["value_date"],
                    row["market_state"],
                    row["can_be_closed"],
                    row["raw_payload_json"],
                )
                for row in resolved_rows
            ],
        )
    connection.commit()
    append_audit_log(
        connection,
        "broker_positions_refreshed",
        {"updated_at": refreshed_at, "count": len(resolved_rows)},
    )
    return {
        "status": "ok",
        "updated": len(resolved_rows),
        "updated_at": refreshed_at,
    }


def refresh_broker_balance_snapshot(
    connection,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    payload = get_balance_snapshot(config, session)
    updated_at = datetime.now(UTC).isoformat(timespec="seconds")
    effective_cash_available = _first_numeric(
        payload,
        "CashAvailableForTrading",
        "MarginAvailableForTrading",
        "CashBalance",
        "CollateralAvailable",
    )
    connection.execute(
        """
        INSERT INTO broker_balance_snapshots (
            singleton_key, updated_at, currency, cash_available_for_trading, margin_available_for_trading,
            cash_balance, transactions_not_booked, settlement_value, total_value, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(singleton_key) DO UPDATE SET
            updated_at = excluded.updated_at,
            currency = excluded.currency,
            cash_available_for_trading = excluded.cash_available_for_trading,
            margin_available_for_trading = excluded.margin_available_for_trading,
            cash_balance = excluded.cash_balance,
            transactions_not_booked = excluded.transactions_not_booked,
            settlement_value = excluded.settlement_value,
            total_value = excluded.total_value,
            raw_payload_json = excluded.raw_payload_json
        """,
        (
            "main",
            updated_at,
            payload.get("Currency"),
            effective_cash_available,
            payload.get("MarginAvailableForTrading"),
            payload.get("CashBalance"),
            payload.get("TransactionsNotBooked"),
            payload.get("SettlementValue"),
            payload.get("TotalValue"),
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
        ),
    )
    connection.commit()
    append_audit_log(
        connection,
        "broker_balance_refreshed",
        {"updated_at": updated_at, "currency": payload.get("Currency")},
    )
    return {
        "status": "ok",
        "updated_at": updated_at,
        "currency": payload.get("Currency"),
        "cash_available_for_trading": effective_cash_available,
    }


def refresh_broker_account_snapshot(
    connection,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    accounts = get_accounts_snapshot(config, session)
    account_key = str(config["saxo"].get("account_key") or session.get("account_key") or "")
    selected = next((row for row in accounts if str(row.get("AccountKey") or "") == account_key), accounts[0] if accounts else None)
    if not selected:
        return {"status": "ok", "updated_at": datetime.now(UTC).isoformat(timespec="seconds"), "account": None}
    updated_at = datetime.now(UTC).isoformat(timespec="seconds")
    connection.execute(
        """
        INSERT INTO broker_account_snapshots (
            singleton_key, updated_at, account_key, account_id, account_currency, is_trial_account,
            fractional_order_enabled, fractional_order_enabled_asset_types_json,
            can_use_cash_positions_as_margin_collateral, use_cash_positions_as_margin_collateral,
            legal_asset_types_json, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(singleton_key) DO UPDATE SET
            updated_at = excluded.updated_at,
            account_key = excluded.account_key,
            account_id = excluded.account_id,
            account_currency = excluded.account_currency,
            is_trial_account = excluded.is_trial_account,
            fractional_order_enabled = excluded.fractional_order_enabled,
            fractional_order_enabled_asset_types_json = excluded.fractional_order_enabled_asset_types_json,
            can_use_cash_positions_as_margin_collateral = excluded.can_use_cash_positions_as_margin_collateral,
            use_cash_positions_as_margin_collateral = excluded.use_cash_positions_as_margin_collateral,
            legal_asset_types_json = excluded.legal_asset_types_json,
            raw_payload_json = excluded.raw_payload_json
        """,
        (
            "main",
            updated_at,
            selected.get("AccountKey"),
            selected.get("AccountId"),
            selected.get("Currency"),
            1 if bool(selected.get("IsTrialAccount")) else 0,
            1 if bool(selected.get("FractionalOrderEnabled")) else 0,
            json.dumps(selected.get("FractionalOrderEnabledAssetTypes") or [], ensure_ascii=False, sort_keys=True),
            1 if bool(selected.get("CanUseCashPositionsAsMarginCollateral")) else 0,
            1 if bool(selected.get("UseCashPositionsAsMarginCollateral")) else 0,
            json.dumps(selected.get("LegalAssetTypes") or [], ensure_ascii=False, sort_keys=True),
            json.dumps(selected, ensure_ascii=False, sort_keys=True),
        ),
    )
    connection.commit()
    append_audit_log(connection, "broker_account_refreshed", {"updated_at": updated_at, "account_key": selected.get("AccountKey")})
    return {"status": "ok", "updated_at": updated_at, "account_key": selected.get("AccountKey")}


def refresh_broker_instrument_exposures(
    connection,
    config: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    exposures = get_instrument_exposures(config, session)
    updated_at = datetime.now(UTC).isoformat(timespec="seconds")
    connection.execute("DELETE FROM broker_instrument_exposures")
    if exposures:
        connection.executemany(
            """
            INSERT INTO broker_instrument_exposures (
                symbol, updated_at, uic, asset_type, quantity, average_open_price, profit_loss_on_trade,
                instrument_price_day_percent_change, currency, calculation_reliability, can_be_closed, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                (
                    str((row.get("DisplayAndFormat") or {}).get("Symbol") or ""),
                    updated_at,
                    row.get("Uic"),
                    row.get("AssetType"),
                    row.get("Amount"),
                    row.get("AverageOpenPrice"),
                    row.get("ProfitLossOnTrade"),
                    row.get("InstrumentPriceDayPercentChange"),
                    (row.get("DisplayAndFormat") or {}).get("Currency"),
                    row.get("CalculationReliability"),
                    1 if bool(row.get("CanBeClosed")) else 0,
                    json.dumps(row, ensure_ascii=False, sort_keys=True),
                )
                for row in exposures
                if str((row.get("DisplayAndFormat") or {}).get("Symbol") or "").strip()
            ],
        )
    connection.commit()
    append_audit_log(connection, "broker_exposures_refreshed", {"updated_at": updated_at, "count": len(exposures)})
    return {"status": "ok", "updated_at": updated_at, "updated": len(exposures)}


def sync_broker_order_statuses(*, config: dict[str, Any] | None = None, connection=None, limit: int = 25) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    lock_key = "saxo_daytrader_xai:broker_sync"
    lock_acquired = False
    try:
        lock_acquired = try_postgres_advisory_lock(resolved_connection, lock_key)
        if not lock_acquired:
            return {
                "status": "skipped",
                "reason": "broker_sync_already_running",
                "updated": 0,
                "orders": [],
            }
        rows = resolved_connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE mode = 'live'
              AND status IN (
                  'submitted_to_broker',
                  'broker_working',
                  'broker_partially_filled',
                  'broker_amended',
                  'broker_replace_requested',
                  'broker_cancel_requested',
                  'broker_fill_unreconciled'
              )
            ORDER BY id ASC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
        broker_positions = refresh_broker_position_snapshots(resolved_connection, resolved_config, session)
        broker_balance = refresh_broker_balance_snapshot(resolved_connection, resolved_config, session)
        broker_account = refresh_broker_account_snapshot(resolved_connection, resolved_config, session)
        broker_exposures = refresh_broker_instrument_exposures(resolved_connection, resolved_config, session)
        if not rows:
            return {
                "status": "ok",
                "updated": 0,
                "orders": [],
                "broker_positions": broker_positions,
                "broker_balance": broker_balance,
                "broker_account": broker_account,
                "broker_exposures": broker_exposures,
            }

        updates: list[dict[str, Any]] = []
        sell_fill_symbols: set[str] = set()

        for row in rows:
            order = dict(row)
            execution_result = json.loads(order["execution_result_json"]) if order.get("execution_result_json") else {}
            broker_result = execution_result.get("broker_result", {})
            broker_order_id = broker_result.get("OrderId")
            if not broker_order_id:
                updates.append({"order_id": order["id"], "status": order["status"], "skipped": "missing_order_id"})
                continue

            try:
                try:
                    open_order = get_open_order(str(broker_order_id), resolved_config, session)
                    broker_status = str(open_order.get("Status") or "Working")
                    broker_quantity = _extract_broker_quantity(open_order)
                    broker_price = _extract_broker_price(open_order)
                    quantity_changed = broker_quantity is not None and abs(broker_quantity - float(order["quantity"])) > 1e-9
                    price_changed = (
                        broker_price is not None
                        and order.get("price_local") is not None
                        and abs(broker_price - float(order["price_local"])) > 1e-9
                    )
                    if broker_status.lower() in {"working", "placed"}:
                        new_status = (
                            "broker_amended"
                            if quantity_changed or price_changed or order["status"] == "broker_amended"
                            else "broker_working"
                        )
                    elif broker_status.lower() == "fill":
                        new_status = "broker_partially_filled"
                    else:
                        new_status = order["status"]
                    payload = {
                        **execution_result,
                        "open_order": open_order,
                        "broker_quantity": broker_quantity,
                        "broker_price_local": broker_price,
                        "quantity_changed": quantity_changed,
                        "price_changed": price_changed,
                        "last_sync_at": datetime.now(UTC).isoformat(timespec="seconds"),
                    }
                    event_id = _record_execution_event(
                        resolved_connection,
                        order=order,
                        broker_order_id=str(broker_order_id),
                        event_type=new_status,
                        broker_status=broker_status,
                        broker_substatus=str(open_order.get("SubStatus") or ""),
                        broker_quantity=broker_quantity,
                        broker_price_local=broker_price,
                        payload=payload,
                    )
                    resolved_connection.execute(
                        """
                        UPDATE execution_orders
                        SET status = ?, quantity = COALESCE(?, quantity), price_local = COALESCE(?, price_local), execution_result_json = ?
                        WHERE id = ?
                        """,
                        (
                            new_status,
                            broker_quantity,
                            broker_price,
                            json.dumps(payload, ensure_ascii=False, sort_keys=True),
                            order["id"],
                        ),
                    )
                    if new_status == "broker_amended":
                        append_audit_log(
                            resolved_connection,
                            "execution_order_amended",
                            {
                                "order_id": order["id"],
                                "broker_order_id": broker_order_id,
                                "broker_quantity": broker_quantity,
                                "broker_price_local": broker_price,
                                "event_id": event_id,
                            },
                        )
                    updates.append({"order_id": order["id"], "status": new_status})
                    continue
                except SaxoOrderNotFoundError:
                    activity = get_order_activity_last(str(broker_order_id), resolved_config, session)

                activity_status = str(activity.get("Status") or "")
                activity_substatus = str(activity.get("SubStatus") or "")
                broker_quantity = _extract_broker_quantity(activity)
                broker_price = _extract_broker_price(activity) or _coerce_float(activity.get("AveragePrice"))
                payload = {
                    **execution_result,
                    "last_activity": activity,
                    "broker_quantity": broker_quantity,
                    "broker_price_local": broker_price,
                    "last_sync_at": datetime.now(UTC).isoformat(timespec="seconds"),
                }
                payload.pop("reconciliation_error", None)
                payload.pop("sync_error", None)

                if activity_status == "FinalFill" and activity_substatus == "Confirmed":
                    try:
                        result = _sync_incremental_live_fill(
                            resolved_connection,
                            resolved_config,
                            order,
                            activity,
                            broker_order_id=str(broker_order_id),
                            fill_status="FinalFill",
                        )
                        if float(result.get("broker_only_quantity") or 0.0) > 0:
                            payload["local_reconciliation"] = {
                                "status": "partial_local_lot_reconciled",
                                "ledger_quantity": result.get("ledger_quantity"),
                                "broker_only_quantity": result.get("broker_only_quantity"),
                                "note": result.get("reconciliation_note"),
                            }
                        elif result.get("local_reconciliation"):
                            payload["local_reconciliation"] = result["local_reconciliation"]
                        event_id = _record_execution_event(
                            resolved_connection,
                            order=order,
                            broker_order_id=str(broker_order_id),
                            event_type="broker_final_fill",
                            broker_status=activity_status,
                            broker_substatus=activity_substatus,
                            broker_quantity=broker_quantity,
                            broker_price_local=broker_price,
                            payload=payload,
                        )
                        resolved_connection.execute(
                            """
                            UPDATE execution_orders
                            SET status = ?, ledger_id = COALESCE(?, ledger_id), error_text = NULL, execution_result_json = ?
                            WHERE id = ?
                            """,
                            ("executed", result["ledger_id"], json.dumps(payload, ensure_ascii=False, sort_keys=True), order["id"]),
                        )
                        append_audit_log(
                            resolved_connection,
                            "execution_order_fill_synced",
                            {
                                "order_id": order["id"],
                                "ledger_id": result["ledger_id"],
                                "broker_order_id": broker_order_id,
                                "delta_quantity": result["delta_quantity"],
                                "cumulative_quantity": result["cumulative_quantity"],
                                "event_id": event_id,
                            },
                        )
                        if str(order.get("action") or "").upper() == "SELL" and float(result.get("delta_quantity") or 0.0) > 0:
                            sell_fill_symbols.add(str(order["symbol"]))
                        protection_orders = []
                        protection_results = []
                        if (
                            str(order.get("action") or "").upper() == "BUY"
                            and str(order.get("strategy_type") or "") == "ladder"
                            and str(order.get("strategy_role") or "") == "entry"
                        ):
                            protection_orders = _create_ladder_protection_orders_after_fill(
                                resolved_connection,
                                parent_order={**order, "status": "executed", "broker_order_id": str(broker_order_id)},
                                config=resolved_config,
                            )
                            for child in protection_orders:
                                if child["status"] != "pending_execution":
                                    continue
                                protection_results.append(
                                    execute_order(
                                        int(child["order_id"]),
                                        config=resolved_config,
                                        connection=resolved_connection,
                                        approved=_should_auto_submit_live_orders(resolved_config),
                                    )
                                )
                        updates.append(
                            {
                                "order_id": order["id"],
                                "status": "executed",
                                "ledger_id": result["ledger_id"],
                                "protection_orders": protection_orders,
                                "protection_results": protection_results,
                            }
                        )
                    except ValueError as exc:
                        event_id = _record_unreconciled_broker_fill(
                            resolved_connection,
                            order=order,
                            broker_order_id=str(broker_order_id),
                            activity_status=activity_status,
                            activity_substatus=activity_substatus,
                            broker_quantity=broker_quantity,
                            broker_price=broker_price,
                            payload=payload,
                            error_text=str(exc),
                        )
                        updates.append(
                            {
                                "order_id": order["id"],
                                "status": "broker_fill_unreconciled",
                                "event_id": event_id,
                                "error": str(exc),
                            }
                        )
                elif activity_status == "Fill" and activity_substatus == "Confirmed":
                    try:
                        result = _sync_incremental_live_fill(
                            resolved_connection,
                            resolved_config,
                            order,
                            activity,
                            broker_order_id=str(broker_order_id),
                            fill_status="Fill",
                        )
                        if float(result.get("broker_only_quantity") or 0.0) > 0:
                            payload["local_reconciliation"] = {
                                "status": "partial_local_lot_reconciled",
                                "ledger_quantity": result.get("ledger_quantity"),
                                "broker_only_quantity": result.get("broker_only_quantity"),
                                "note": result.get("reconciliation_note"),
                            }
                        elif result.get("local_reconciliation"):
                            payload["local_reconciliation"] = result["local_reconciliation"]
                        event_id = _record_execution_event(
                            resolved_connection,
                            order=order,
                            broker_order_id=str(broker_order_id),
                            event_type="broker_fill",
                            broker_status=activity_status,
                            broker_substatus=activity_substatus,
                            broker_quantity=broker_quantity,
                            broker_price_local=broker_price,
                            payload=payload,
                        )
                        resolved_connection.execute(
                            """
                            UPDATE execution_orders
                            SET status = ?, ledger_id = COALESCE(?, ledger_id), error_text = NULL, execution_result_json = ?
                            WHERE id = ?
                            """,
                            ("broker_partially_filled", result["ledger_id"], json.dumps(payload, ensure_ascii=False, sort_keys=True), order["id"]),
                        )
                        append_audit_log(
                            resolved_connection,
                            "execution_order_partial_fill_synced",
                            {
                                "order_id": order["id"],
                                "ledger_id": result["ledger_id"],
                                "broker_order_id": broker_order_id,
                                "delta_quantity": result["delta_quantity"],
                                "cumulative_quantity": result["cumulative_quantity"],
                                "event_id": event_id,
                            },
                        )
                        if str(order.get("action") or "").upper() == "SELL" and float(result.get("delta_quantity") or 0.0) > 0:
                            sell_fill_symbols.add(str(order["symbol"]))
                        updates.append(
                            {
                                "order_id": order["id"],
                                "status": "broker_partially_filled",
                                "ledger_id": result["ledger_id"],
                                "delta_quantity": result["delta_quantity"],
                            }
                        )
                    except ValueError as exc:
                        event_id = _record_unreconciled_broker_fill(
                            resolved_connection,
                            order=order,
                            broker_order_id=str(broker_order_id),
                            activity_status=activity_status,
                            activity_substatus=activity_substatus,
                            broker_quantity=broker_quantity,
                            broker_price=broker_price,
                            payload=payload,
                            error_text=str(exc),
                        )
                        updates.append(
                            {
                                "order_id": order["id"],
                                "status": "broker_fill_unreconciled",
                                "event_id": event_id,
                                "error": str(exc),
                            }
                        )
                elif activity_status in {"Changed", "Replaced", "Amended"} and activity_substatus == "Confirmed":
                    event_id = _record_execution_event(
                        resolved_connection,
                        order=order,
                        broker_order_id=str(broker_order_id),
                        event_type="broker_amended",
                        broker_status=activity_status,
                        broker_substatus=activity_substatus,
                        broker_quantity=broker_quantity,
                        broker_price_local=broker_price,
                        payload=payload,
                    )
                    resolved_connection.execute(
                        """
                        UPDATE execution_orders
                        SET status = ?, quantity = COALESCE(?, quantity), price_local = COALESCE(?, price_local), execution_result_json = ?
                        WHERE id = ?
                        """,
                        (
                            "broker_amended",
                            broker_quantity,
                            broker_price,
                            json.dumps(payload, ensure_ascii=False, sort_keys=True),
                            order["id"],
                        ),
                    )
                    append_audit_log(
                        resolved_connection,
                        "execution_order_amended",
                        {
                            "order_id": order["id"],
                            "broker_order_id": broker_order_id,
                            "broker_quantity": broker_quantity,
                            "broker_price_local": broker_price,
                            "event_id": event_id,
                        },
                    )
                    updates.append({"order_id": order["id"], "status": "broker_amended"})
                elif activity_status in {"Cancelled", "Expired"} and activity_substatus == "Confirmed":
                    new_status = "broker_cancelled" if activity_status == "Cancelled" else "broker_expired"
                    event_id = _record_execution_event(
                        resolved_connection,
                        order=order,
                        broker_order_id=str(broker_order_id),
                        event_type=new_status,
                        broker_status=activity_status,
                        broker_substatus=activity_substatus,
                        broker_quantity=broker_quantity,
                        broker_price_local=broker_price,
                        payload=payload,
                    )
                    resolved_connection.execute(
                        """
                        UPDATE execution_orders
                        SET status = ?, execution_result_json = ?
                        WHERE id = ?
                        """,
                        (new_status, json.dumps(payload, ensure_ascii=False, sort_keys=True), order["id"]),
                    )
                    append_audit_log(
                        resolved_connection,
                        "execution_order_closed_without_fill",
                        {
                            "order_id": order["id"],
                            "broker_order_id": broker_order_id,
                            "status": new_status,
                            "event_id": event_id,
                        },
                    )
                    updates.append({"order_id": order["id"], "status": new_status})
                elif activity_status in {"Rejected", "Failed"}:
                    event_id = _record_execution_event(
                        resolved_connection,
                        order=order,
                        broker_order_id=str(broker_order_id),
                        event_type="broker_rejected",
                        broker_status=activity_status,
                        broker_substatus=activity_substatus,
                        broker_quantity=broker_quantity,
                        broker_price_local=broker_price,
                        payload=payload,
                    )
                    resolved_connection.execute(
                        """
                        UPDATE execution_orders
                        SET status = ?, error_text = ?, execution_result_json = ?
                        WHERE id = ?
                        """,
                        (
                            "broker_rejected",
                            json.dumps(activity, ensure_ascii=False, sort_keys=True),
                            json.dumps(payload, ensure_ascii=False, sort_keys=True),
                            order["id"],
                        ),
                    )
                    append_audit_log(
                        resolved_connection,
                        "execution_order_rejected",
                        {
                            "order_id": order["id"],
                            "broker_order_id": broker_order_id,
                            "event_id": event_id,
                        },
                    )
                    updates.append({"order_id": order["id"], "status": "broker_rejected"})
                else:
                    resolved_connection.execute(
                        """
                        UPDATE execution_orders
                        SET execution_result_json = ?
                        WHERE id = ?
                        """,
                        (json.dumps(payload, ensure_ascii=False, sort_keys=True), order["id"]),
                    )
                    updates.append({"order_id": order["id"], "status": order["status"]})
            except Exception as exc:
                resolved_connection.rollback()
                fallback_payload = {
                    **execution_result,
                    "last_sync_at": datetime.now(UTC).isoformat(timespec="seconds"),
                }
                event_id = _record_broker_sync_error(
                    resolved_connection,
                    order=order,
                    broker_order_id=str(broker_order_id),
                    event_type="broker_sync_failed",
                    payload=fallback_payload,
                    error_text=str(exc),
                )
                updates.append(
                    {
                        "order_id": order["id"],
                        "status": order["status"],
                        "event_id": event_id,
                        "error": str(exc),
                    }
                )

        resolved_connection.commit()
        broker_positions_after = broker_positions
        broker_balance_after = broker_balance
        broker_account_after = broker_account
        broker_exposures_after = broker_exposures
        reconciliation_after_sell_fill = None
        if updates:
            broker_positions_after = refresh_broker_position_snapshots(resolved_connection, resolved_config, session)
            broker_balance_after = refresh_broker_balance_snapshot(resolved_connection, resolved_config, session)
            broker_account_after = refresh_broker_account_snapshot(resolved_connection, resolved_config, session)
            broker_exposures_after = refresh_broker_instrument_exposures(resolved_connection, resolved_config, session)
            if sell_fill_symbols:
                reconciliation_after_sell_fill = reconcile_portfolio_to_broker(
                    connection=resolved_connection,
                    config=resolved_config,
                    symbols=sell_fill_symbols,
                )
        return {
            "status": "ok",
            "updated": len(updates),
            "orders": updates,
            "broker_positions": broker_positions_after,
            "broker_balance": broker_balance_after,
            "broker_account": broker_account_after,
            "broker_exposures": broker_exposures_after,
            "reconciliation_after_sell_fill": reconciliation_after_sell_fill,
        }
    finally:
        if lock_acquired:
            try:
                release_postgres_advisory_lock(resolved_connection, lock_key)
            except Exception:
                resolved_connection.rollback()
                release_postgres_advisory_lock(resolved_connection, lock_key)
        if should_close:
            resolved_connection.close()


def _saxo_environment_value(config: dict[str, Any], session: dict[str, Any] | None = None) -> str:
    return str((session or {}).get("environment") or config.get("saxo", {}).get("environment") or "").strip().lower()


def _fail_order(
    connection,
    *,
    order_id: int,
    status: str,
    error_text: str,
    payload: dict[str, Any],
) -> dict[str, Any]:
    connection.execute(
        """
        UPDATE execution_orders
        SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
        WHERE id = ?
        """,
        (
            status,
            datetime.now(UTC).isoformat(timespec="seconds"),
            error_text,
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
            order_id,
        ),
    )
    connection.commit()
    return {"status": status, "order_id": order_id, "error": error_text}


def _market_value_for_quantity(row: dict[str, Any], quantity: float, fx_snapshot: dict[str, float]) -> tuple[float | None, float]:
    price = _coerce_float(row.get("current_price_local")) or _coerce_float(row.get("open_price_local")) or _coerce_float(row.get("open_price_including_costs_local"))
    currency = str(row.get("currency") or "DKK")
    if price is None:
        return None, 0.0
    return price, float(price) * float(quantity) * fx_rate_to_dkk(currency, fx_snapshot)


def adopt_broker_holdings_into_local_ledger(*, config: dict[str, Any] | None = None, connection=None) -> dict[str, Any]:
    """Refresh Saxo broker snapshots and adjust local lots to match broker holdings."""
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        if str(resolved_config.get("execution", {}).get("adapter") or "").lower() != "saxo":
            raise ValueError("Broker adoption requires execution.adapter=saxo.")
        configured_environment = str(resolved_config.get("saxo", {}).get("environment") or "").strip().lower()
        if configured_environment == "sim":
            raise ValueError("Broker adoption is blocked in Saxo SIM. Use portfolio-to-Saxo-SIM reconciliation instead.")

        session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
        session_environment = _saxo_environment_value(resolved_config, session)
        if session_environment == "sim":
            raise ValueError("Broker adoption is blocked because the active Saxo session is SIM.")

        broker_positions = refresh_broker_position_snapshots(resolved_connection, resolved_config, session)
        broker_balance = refresh_broker_balance_snapshot(resolved_connection, resolved_config, session)
        broker_account = refresh_broker_account_snapshot(resolved_connection, resolved_config, session)
        broker_exposures = refresh_broker_instrument_exposures(resolved_connection, resolved_config, session)
        reconciliation = reconcile_portfolio_to_broker(connection=resolved_connection, config=resolved_config)
        append_audit_log(
            resolved_connection,
            "broker_holdings_adopted_into_local_ledger",
            {
                "configured_environment": configured_environment,
                "session_environment": session_environment,
                "broker_positions": broker_positions,
                "broker_balance": broker_balance,
                "broker_account": broker_account,
                "broker_exposures": broker_exposures,
                "reconciliation": reconciliation,
            },
        )
        resolved_connection.commit()
        return {
            **reconciliation,
            "broker_positions": broker_positions,
            "broker_balance": broker_balance,
            "broker_account": broker_account,
            "broker_exposures": broker_exposures,
        }
    finally:
        if should_close:
            resolved_connection.close()


def sync_saxo_sim_account_to_portfolio(*, config: dict[str, Any] | None = None, connection=None) -> dict[str, Any]:
    """Queue/submit SIM-only orders so Saxo SIM holdings match the local portfolio."""
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        if str(resolved_config.get("execution", {}).get("adapter") or "").lower() != "saxo":
            raise ValueError("Saxo SIM portfolio sync requires execution.adapter=saxo.")
        configured_environment = str(resolved_config.get("saxo", {}).get("environment") or "").strip().lower()
        if configured_environment != "sim":
            raise ValueError("Saxo SIM portfolio sync is blocked unless saxo.environment is SIM.")

        session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
        session_environment = _saxo_environment_value(resolved_config, session)
        if session_environment != "sim":
            raise ValueError("Saxo SIM portfolio sync is blocked because the active Saxo session is not SIM.")
        if bool(resolved_config.get("app", {}).get("dry_run", True)):
            return {"status": "blocked_by_dry_run", "created_order_ids": [], "orders": []}

        batch_id = fetch_latest_batch_id(resolved_connection)
        local_rows = fetch_portfolio_positions(
            resolved_connection,
            batch_id=batch_id,
            initial_cash_dkk=_initial_cash_dkk(resolved_config),
            use_broker_positions=False,
        )
        target_by_symbol = {str(row["symbol"]): dict(row) for row in local_rows}

        broker_positions = refresh_broker_position_snapshots(resolved_connection, resolved_config, session)
        broker_balance = refresh_broker_balance_snapshot(resolved_connection, resolved_config, session)
        broker_rows = resolved_connection.execute(
            """
            SELECT symbol, instrument_name, isin, currency, quantity, open_price_local, open_price_including_costs_local
            FROM broker_position_snapshots
            """
        ).fetchall()
        broker_by_symbol = {str(row["symbol"]): dict(row) for row in broker_rows}

        active_rows = resolved_connection.execute(
            """
            SELECT symbol, id, status
            FROM execution_orders
            WHERE strategy_type = 'portfolio_sync'
              AND status NOT IN ({})
            ORDER BY id ASC
            """.format(",".join("?" for _ in TERMINAL_ORDER_STATUSES)),
            tuple(TERMINAL_ORDER_STATUSES),
        ).fetchall()
        active_by_symbol: dict[str, list[dict[str, Any]]] = {}
        for row in active_rows:
            active_by_symbol.setdefault(str(row["symbol"]), []).append(dict(row))

        fx_snapshot = fetch_ecb_fx_rates()
        created_at = datetime.now(UTC).isoformat(timespec="seconds")
        order_specs: list[dict[str, Any]] = []
        skipped: list[dict[str, Any]] = []

        for symbol in sorted(set(target_by_symbol) | set(broker_by_symbol)):
            target = target_by_symbol.get(symbol)
            broker = broker_by_symbol.get(symbol)
            target_quantity = float((target or {}).get("quantity") or 0.0)
            broker_quantity = float((broker or {}).get("quantity") or 0.0)
            delta_quantity = target_quantity - broker_quantity
            whole_delta = _whole_share_quantity(abs(delta_quantity))
            if whole_delta <= 0:
                continue
            if active_by_symbol.get(symbol):
                skipped.append(
                    {
                        "symbol": symbol,
                        "status": "active_sync_order_exists",
                        "active_order_ids": [int(row["id"]) for row in active_by_symbol[symbol]],
                    }
                )
                continue

            action = "BUY" if delta_quantity > 0 else "SELL"
            source_row = target or broker or {"symbol": symbol, "currency": "DKK"}
            price_local, estimated_value_dkk = _market_value_for_quantity(source_row, whole_delta, fx_snapshot)
            currency = str(source_row.get("currency") or "DKK")
            request_payload = {
                "symbol": symbol,
                "action": action,
                "order_type": "Market",
                "strategy_type": "portfolio_sync",
                "strategy_role": "increase_to_target" if action == "BUY" else "reduce_to_target",
                "target_quantity": target_quantity,
                "broker_quantity": broker_quantity,
                "delta_quantity": delta_quantity,
                "saxo_environment": "sim",
                "reason": "Mirror local imported portfolio into Saxo Developer SIM account.",
            }
            order_specs.append(
                {
                    "symbol": symbol,
                    "action": action,
                    "order_type": "Market",
                    "mode": "live",
                    "status": "pending_execution",
                    "adapter": "saxo",
                    "requested_weight_pct": None,
                    "quantity": float(whole_delta),
                    "price_local": price_local,
                    "limit_price_local": None,
                    "stop_price_local": None,
                    "currency": currency,
                    "estimated_value_dkk": estimated_value_dkk,
                    "approval_required": 0,
                    "parent_execution_order_id": None,
                    "strategy_type": "portfolio_sync",
                    "strategy_session": "saxo_sim",
                    "strategy_key": f"portfolio_sync:{symbol}:{created_at}",
                    "strategy_role": request_payload["strategy_role"],
                    "request_json": json.dumps(request_payload, ensure_ascii=False, sort_keys=True),
                    "execution_result_json": None,
                    "error_text": None,
                }
            )

        # Sell reductions first so SIM buying power is freed before increases are attempted.
        order_specs.sort(key=lambda row: 0 if row["action"] == "SELL" else 1)
        created_order_ids: list[int] = []
        resumed_order_ids: list[int] = []
        execution_results: list[dict[str, Any]] = []
        for spec in order_specs:
            cursor = resolved_connection.execute(
                """
                INSERT INTO execution_orders (
                    created_at, report_id, symbol, action, order_type, mode, status, adapter,
                    requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                    approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                    request_json, execution_result_json, error_text
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    created_at,
                    None,
                    spec["symbol"],
                    spec["action"],
                    spec["order_type"],
                    spec["mode"],
                    spec["status"],
                    spec["adapter"],
                    spec["requested_weight_pct"],
                    spec["quantity"],
                    spec["price_local"],
                    spec["limit_price_local"],
                    spec["stop_price_local"],
                    spec["currency"],
                    spec["estimated_value_dkk"],
                    spec["approval_required"],
                    spec["parent_execution_order_id"],
                    spec["strategy_type"],
                    spec["strategy_session"],
                    spec["strategy_key"],
                    spec["strategy_role"],
                    spec["request_json"],
                    spec["execution_result_json"],
                    spec["error_text"],
                ),
            )
            order_id = int(cursor.lastrowid)
            created_order_ids.append(order_id)
            resolved_connection.commit()
            execution_results.append(
                execute_order(
                    order_id,
                    config=resolved_config,
                    connection=resolved_connection,
                    approved=True,
                )
            )
        executable_sync_statuses = {
            "pending_execution",
            "pending_approval",
            "waiting_for_market_open",
            "waiting_for_virtual_cash_budget",
        }
        for row in active_rows:
            if str(row["status"]) not in executable_sync_statuses:
                continue
            resumed_order_ids.append(int(row["id"]))
            execution_results.append(
                execute_order(
                    int(row["id"]),
                    config=resolved_config,
                    connection=resolved_connection,
                    approved=True,
                )
            )

        append_audit_log(
            resolved_connection,
            "saxo_sim_portfolio_sync_requested",
            {
                "created_at": created_at,
                "batch_id": batch_id,
                "created_order_ids": created_order_ids,
                "resumed_order_ids": resumed_order_ids,
                "skipped": skipped,
                "broker_positions": broker_positions,
                "broker_balance": broker_balance,
            },
        )
        resolved_connection.commit()
        return {
            "status": "ok",
            "created": len(created_order_ids),
            "created_order_ids": created_order_ids,
            "resumed": len(resumed_order_ids),
            "resumed_order_ids": resumed_order_ids,
            "orders": execution_results,
            "skipped": skipped,
            "broker_positions": broker_positions,
            "broker_balance": broker_balance,
        }
    finally:
        if should_close:
            resolved_connection.close()


def execute_order(order_id: int, *, config: dict[str, Any] | None = None, connection=None, approved: bool = False) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        order_row = resolved_connection.execute("SELECT * FROM execution_orders WHERE id = ?", (order_id,)).fetchone()
        if not order_row:
            raise ValueError(f"Unknown execution order {order_id}")
        order = dict(order_row)
        normalized_quantity = _whole_share_quantity(float(order["quantity"]))
        if normalized_quantity <= 0:
            resolved_connection.execute(
                """
                UPDATE execution_orders
                SET status = ?, error_text = ?
                WHERE id = ?
                """,
                ("invalid_quantity", "Order quantity must be at least 1 whole share", order_id),
            )
            resolved_connection.commit()
            return {"status": "invalid_quantity", "order_id": order_id}
        if abs(float(order["quantity"]) - normalized_quantity) > 1e-9:
            order["quantity"] = float(normalized_quantity)
            resolved_connection.execute(
                "UPDATE execution_orders SET quantity = ? WHERE id = ?",
                (float(normalized_quantity), order_id),
            )
            resolved_connection.commit()
        if order["status"] not in {"pending_execution", "pending_approval", "waiting_for_market_open", "waiting_for_virtual_cash_budget"}:
            return {"status": order["status"], "order_id": order_id}
        if order["mode"] == "live" and order["approval_required"] and not approved:
            return {"status": "approval_required", "order_id": order_id}
        is_portfolio_sync = str(order.get("strategy_type") or "") == "portfolio_sync"
        if is_portfolio_sync and str(resolved_config.get("saxo", {}).get("environment") or "").strip().lower() != "sim":
            return _fail_order(
                resolved_connection,
                order_id=order_id,
                status="execution_failed",
                error_text="Portfolio sync orders are SIM-only and cannot execute while saxo.environment is LIVE.",
                payload={"strategy_type": "portfolio_sync", "configured_environment": resolved_config.get("saxo", {}).get("environment")},
            )
        market_row = _market_status_for_symbol(str(order["symbol"]), resolved_config)
        if market_row is not None and not bool(market_row.get("is_tradable", market_row.get("is_open"))):
            error_text = f"Exchange closed for {order['symbol']}: {market_row.get('status_reason')}"
            payload = {
                "symbol": order["symbol"],
                "exchange_code": market_row.get("code"),
                "market": market_row.get("market"),
                "status_reason": market_row.get("status_reason"),
                "next_open": market_row.get("next_open"),
                "next_open_at_utc": market_row.get("next_open_at_utc"),
            }
            resolved_connection.execute(
                """
                UPDATE execution_orders
                SET status = ?, error_text = ?, execution_result_json = ?
                WHERE id = ?
                """,
                (
                    "waiting_for_market_open",
                    error_text,
                    json.dumps(payload, ensure_ascii=False, sort_keys=True),
                    order_id,
                ),
            )
            resolved_connection.commit()
            return {
                "status": "waiting_for_market_open",
                "order_id": order_id,
                "error": error_text,
                "market": payload,
            }
        if order["mode"] == "live" and resolved_config["app"]["dry_run"]:
            resolved_connection.execute(
                """
                UPDATE execution_orders
                SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
                WHERE id = ?
                """,
                (
                    "blocked_by_dry_run",
                    datetime.now(UTC).isoformat(timespec="seconds"),
                    "Live execution blocked because app.dry_run is true",
                    json.dumps({"dry_run": True}, ensure_ascii=False, sort_keys=True),
                    order_id,
                ),
            )
            resolved_connection.commit()
            return {"status": "blocked_by_dry_run", "order_id": order_id}
        if order["mode"] == "live":
            if order["adapter"] != "saxo":
                error_text = f"Unsupported live adapter '{order['adapter']}'"
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
                    WHERE id = ?
                    """,
                    (
                        "execution_failed",
                        datetime.now(UTC).isoformat(timespec="seconds") if approved else None,
                        error_text,
                        json.dumps({"adapter": order["adapter"]}, ensure_ascii=False, sort_keys=True),
                        order_id,
                    ),
                )
                resolved_connection.commit()
                _dispatch_execution_failure_alerts(resolved_connection, resolved_config)
                return {"status": "execution_failed", "order_id": order_id, "error": error_text}
            payload: dict[str, Any] | None = None
            order_request: dict[str, Any] = {}
            precheck: dict[str, Any] | None = None
            try:
                session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
                if is_portfolio_sync and _saxo_environment_value(resolved_config, session) != "sim":
                    return _fail_order(
                        resolved_connection,
                        order_id=order_id,
                        status="execution_failed",
                        error_text="Portfolio sync orders are SIM-only and cannot execute with a non-SIM Saxo session.",
                        payload={
                            "strategy_type": "portfolio_sync",
                            "configured_environment": resolved_config.get("saxo", {}).get("environment"),
                            "session_environment": session.get("environment"),
                        },
                    )
                if order["action"] == "SELL":
                    holdings = _current_holdings_map_for_retry(resolved_connection, resolved_config)
                    held_quantity = float((holdings.get(str(order["symbol"])) or {}).get("quantity") or 0.0)
                    available_quantity = _available_sell_quantity(
                        resolved_connection,
                        str(order["symbol"]),
                        held_quantity,
                        exclude_order_id=order_id,
                    )
                    requested_quantity = float(order["quantity"] or 0.0)
                    if available_quantity + 1e-9 < requested_quantity:
                        error_text = (
                            f"Sell blocked before Saxo precheck for {order['symbol']}: "
                            f"requested {requested_quantity:g}, broker-aligned holdings {held_quantity:g}, "
                            f"available after active sell reservations {available_quantity:g}."
                        )
                        resolved_connection.execute(
                            """
                            UPDATE execution_orders
                            SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
                            WHERE id = ?
                            """,
                            (
                                "execution_failed",
                                datetime.now(UTC).isoformat(timespec="seconds") if approved else None,
                                error_text,
                                json.dumps(
                                    {
                                        "sell_guard": {
                                            "requested_quantity": requested_quantity,
                                            "held_quantity": held_quantity,
                                            "available_quantity": available_quantity,
                                        }
                                    },
                                    ensure_ascii=False,
                                    sort_keys=True,
                                ),
                                order_id,
                            ),
                        )
                        resolved_connection.commit()
                        _dispatch_execution_failure_alerts(resolved_connection, resolved_config)
                        return {"status": "execution_failed", "order_id": order_id, "error": error_text}
                if order["action"] == "BUY" and _cash_gate_enabled(resolved_config) and not is_portfolio_sync:
                    virtual_budget_gate = _evaluate_virtual_buy_budget_gate(order, resolved_config, resolved_connection)
                    if not virtual_budget_gate["allowed"]:
                        error_text = (
                            f"Waiting for virtual cash budget before buying {order['symbol']}. "
                            f"Required {virtual_budget_gate['required_dkk']:.2f} DKK, "
                            f"virtual cash available {virtual_budget_gate['available_dkk']:.2f} DKK, "
                            f"deployment headroom {virtual_budget_gate['deployment_headroom_dkk']:.2f} DKK, "
                            f"cash buffer reserved {virtual_budget_gate['min_cash_buffer_dkk']:.2f} DKK, "
                            f"capital limit {virtual_budget_gate['capital_limit_dkk']:.2f} DKK."
                        )
                        resolved_connection.execute(
                            """
                            UPDATE execution_orders
                            SET status = ?, error_text = ?, execution_result_json = ?
                            WHERE id = ?
                            """,
                            (
                                "waiting_for_virtual_cash_budget",
                                error_text,
                                json.dumps({"virtual_budget_gate": virtual_budget_gate}, ensure_ascii=False, sort_keys=True),
                                order_id,
                            ),
                        )
                        resolved_connection.commit()
                        return {
                            "status": "waiting_for_virtual_cash_budget",
                            "order_id": order_id,
                            "error": error_text,
                            "virtual_budget_gate": virtual_budget_gate,
                        }
                    cash_gate = _evaluate_live_buy_cash_gate(order, resolved_config, session)
                    if not cash_gate["allowed"]:
                        error_text = (
                            f"Waiting for settled cash before buying {order['symbol']}. "
                            f"Required {cash_gate['required_dkk']:.2f} DKK, "
                            f"cash available {cash_gate['cash_available_dkk']:.2f} DKK, "
                            f"funds for settlement {cash_gate['funds_for_settlement_dkk']:.2f} DKK, "
                            f"transactions not booked {cash_gate['transactions_not_booked_dkk']:.2f} DKK."
                        )
                        resolved_connection.execute(
                            """
                            UPDATE execution_orders
                            SET status = ?, error_text = ?, execution_result_json = ?
                            WHERE id = ?
                            """,
                            (
                                "waiting_for_cash_settlement",
                                error_text,
                                json.dumps({"cash_gate": cash_gate}, ensure_ascii=False, sort_keys=True),
                                order_id,
                            ),
                        )
                        resolved_connection.commit()
                        return {
                            "status": "waiting_for_cash_settlement",
                            "order_id": order_id,
                            "error": error_text,
                            "cash_gate": cash_gate,
                        }
                order_request = _request_payload(order)
                related_orders = list(order_request.get("related_orders") or [])
                bracket_deferred = _defer_ladder_entry_bracket(resolved_config, order, order_request)
                payload = build_order_payload(
                    symbol=order["symbol"],
                    action=order["action"],
                    quantity=float(_whole_share_quantity(float(order["quantity"]))),
                    external_reference=f"saxo-daytrader:{order_id}",
                    config=resolved_config,
                    session=session,
                    order_type=str(order.get("order_type") or order_request.get("order_type") or "Market"),
                    limit_price=_coerce_float(order.get("limit_price_local")) or _coerce_float(order_request.get("limit_price_local")),
                    stop_price=_coerce_float(order.get("stop_price_local")) or _coerce_float(order_request.get("stop_price_local")),
                    duration_type=str(order_request.get("duration_type") or "DayOrder"),
                    related_orders=[] if bracket_deferred else related_orders,
                )
                precheck = precheck_order(payload, resolved_config, session)
                broker_result = place_order(payload, resolved_config, session)
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET status = ?, approved_at = ?, broker_order_id = ?, execution_result_json = ?
                    WHERE id = ?
                    """,
                    (
                        "submitted_to_broker",
                        datetime.now(UTC).isoformat(timespec="seconds"),
                        str(broker_result.get("OrderId", "")) or None,
                        json.dumps(
                            {
                                "precheck": precheck,
                                "payload": payload,
                                "broker_result": broker_result,
                                "deferred_related_orders": related_orders if bracket_deferred else [],
                            },
                            ensure_ascii=False,
                            sort_keys=True,
                        ),
                        order_id,
                    ),
                )
                resolved_connection.commit()
                child_order_ids = []
                if not bracket_deferred:
                    child_order_ids = _record_related_orders_after_submission(
                        resolved_connection,
                        parent_order={**order, "broker_order_id": str(broker_result.get("OrderId", "")) or None},
                        broker_payload=payload,
                        broker_result=broker_result,
                    )
                append_audit_log(
                    resolved_connection,
                    "execution_order_submitted",
                    {
                        "order_id": order_id,
                        "mode": order["mode"],
                        "adapter": order["adapter"],
                        "payload": payload,
                        "child_order_ids": child_order_ids,
                        "deferred_related_orders": related_orders if bracket_deferred else [],
                    },
                )
                return {
                    "status": "submitted_to_broker",
                    "order_id": order_id,
                    "broker_result": broker_result,
                    "precheck": precheck,
                    "child_order_ids": child_order_ids,
                }
            except (SaxoSessionError, requests.RequestException, ValueError) as exc:  # type: ignore[name-defined]
                error_text = str(exc)
                failure_payload = {"adapter": order["adapter"], "error": error_text}
                if payload is not None:
                    failure_payload["payload"] = payload
                if precheck is not None:
                    failure_payload["precheck"] = precheck
                if order_request:
                    failure_payload["request"] = order_request
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET status = ?, approved_at = ?, error_text = ?, execution_result_json = ?
                    WHERE id = ?
                    """,
                    (
                        "execution_failed",
                        datetime.now(UTC).isoformat(timespec="seconds") if approved else None,
                        error_text,
                        json.dumps(failure_payload, ensure_ascii=False, sort_keys=True),
                        order_id,
                    ),
                )
                resolved_connection.commit()
                append_audit_log(
                    resolved_connection,
                    "execution_order_failed",
                    {"order_id": order_id, "mode": order["mode"], "adapter": order["adapter"], "error": error_text},
                )
                _dispatch_execution_failure_alerts(resolved_connection, resolved_config)
                return {"status": "execution_failed", "order_id": order_id, "error": error_text}

        batch_id = fetch_latest_batch_id(resolved_connection)
        try:
            if order["action"] == "SELL":
                trade = calculate_sell_outcome(
                    order["symbol"],
                    float(order["quantity"]),
                    float(order["price_local"]),
                    config=resolved_config,
                    connection=resolved_connection,
                    batch_id=batch_id,
                    tax_year=datetime.now(UTC).year,
                )
                trade["mode"] = order["mode"]
                trade["status"] = "executed"
                trade["notes"] = "Phase 5 automated execution"
                result = update_ledger(trade, config=resolved_config, connection=resolved_connection)
                ledger_id = result["ledger_id"]
            else:
                result = _record_buy_trade(resolved_connection, resolved_config, order, batch_id)
                ledger_id = result["ledger_id"]
        except ValueError as exc:
            error_text = str(exc)
            failed = _mark_execution_failed(
                resolved_connection,
                order_id=order_id,
                approved=True,
                adapter=order["adapter"],
                error_text=error_text,
            )
            append_audit_log(
                resolved_connection,
                "execution_order_failed",
                {"order_id": order_id, "mode": order["mode"], "adapter": order["adapter"], "error": error_text},
            )
            _dispatch_execution_failure_alerts(resolved_connection, resolved_config)
            return failed

        resolved_connection.execute(
            """
            UPDATE execution_orders
            SET status = ?, approved_at = ?, ledger_id = ?, execution_result_json = ?
            WHERE id = ?
            """,
            (
                "executed",
                datetime.now(UTC).isoformat(timespec="seconds"),
                ledger_id,
                json.dumps(result, ensure_ascii=False, sort_keys=True),
                order_id,
            ),
        )
        resolved_connection.commit()
        append_audit_log(
            resolved_connection,
            "execution_order_executed",
            {"order_id": order_id, "ledger_id": ledger_id, "mode": order["mode"], "action": order["action"]},
        )
        return {"status": "executed", "order_id": order_id, "ledger_id": ledger_id}
    finally:
        if should_close:
            resolved_connection.close()


def manage_live_order(
    order_id: int,
    *,
    management_action: str,
    config: dict[str, Any] | None = None,
    connection=None,
    new_quantity: float | None = None,
    new_price: float | None = None,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        row = resolved_connection.execute("SELECT * FROM execution_orders WHERE id = ?", (order_id,)).fetchone()
        if not row:
            raise ValueError(f"Unknown execution order {order_id}")
        order = dict(row)
        if order["mode"] != "live":
            return {"status": "not_live_order", "order_id": order_id}
        if order["adapter"] != "saxo":
            return {"status": "unsupported_adapter", "order_id": order_id}
        if order["status"] not in MANAGEABLE_LIVE_STATUSES:
            return {"status": "not_manageable", "order_id": order_id, "current_status": order["status"]}
        if resolved_config["app"]["dry_run"]:
            return {"status": "blocked_by_dry_run", "order_id": order_id}

        session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
        execution_result = _execution_result(order)
        broker_result = execution_result.get("broker_result", {})
        broker_order_id = str(order.get("broker_order_id") or broker_result.get("OrderId") or "")
        if not broker_order_id:
            return {"status": "missing_broker_order_id", "order_id": order_id}

        now_iso = datetime.now(UTC).isoformat(timespec="seconds")
        try:
            if management_action == "cancel":
                broker_response = cancel_order(broker_order_id, resolved_config, session)
                new_status = "broker_cancel_requested"
                payload = {
                    **execution_result,
                    "management": {
                        "action": "cancel",
                        "requested_at": now_iso,
                        "broker_response": broker_response,
                    },
                }
                event_type = "broker_cancel_requested"
            elif management_action == "replace":
                original_payload = _broker_payload(order)
                order_type = str(original_payload.get("OrderType") or "Market")
                effective_price = new_price if new_price is not None else _coerce_float(order.get("price_local"))
                normalized_replace_quantity = _whole_share_quantity(
                    new_quantity if new_quantity is not None else float(order["quantity"])
                )
                if normalized_replace_quantity <= 0:
                    return {"status": "invalid_quantity", "order_id": order_id}
                if new_price is not None and order_type == "Market":
                    order_type = "Limit"
                patch_payload: dict[str, Any] = {
                    "AccountKey": original_payload.get("AccountKey") or resolved_config["saxo"]["account_key"] or session.get("account_key"),
                    "OrderId": broker_order_id,
                    "Amount": float(normalized_replace_quantity),
                    "AssetType": original_payload.get("AssetType", "Stock"),
                    "OrderType": order_type,
                }
                if original_payload.get("OrderDuration"):
                    patch_payload["OrderDuration"] = original_payload["OrderDuration"]
                if effective_price is not None and order_type != "Market":
                    patch_payload["OrderPrice"] = effective_price
                broker_response = change_order(patch_payload, resolved_config, session)
                new_status = "broker_replace_requested"
                payload = {
                    **execution_result,
                    "management": {
                        "action": "replace",
                        "requested_at": now_iso,
                        "request_payload": patch_payload,
                        "broker_response": broker_response,
                    },
                }
                event_type = "broker_replace_requested"
            else:
                raise ValueError(f"Unsupported management action '{management_action}'")
        except (SaxoSessionError, SaxoOrderNotFoundError, requests.RequestException, ValueError) as exc:
            error_text = str(exc)
            failure_event_type = "broker_cancel_failed" if management_action == "cancel" else "broker_replace_failed"
            failure_payload = {
                **execution_result,
                "management": {
                    "action": management_action,
                    "requested_at": now_iso,
                    "error": error_text,
                },
            }
            event_id = _record_execution_event(
                resolved_connection,
                order=order,
                broker_order_id=broker_order_id,
                event_type=failure_event_type,
                broker_status=order["status"],
                broker_substatus="failed",
                broker_quantity=_coerce_float(order.get("quantity")),
                broker_price_local=_coerce_float(order.get("price_local")),
                payload=failure_payload,
            )
            resolved_connection.execute(
                """
                UPDATE execution_orders
                SET error_text = ?, execution_result_json = ?
                WHERE id = ?
                """,
                (
                    error_text,
                    json.dumps(failure_payload, ensure_ascii=False, sort_keys=True),
                    order_id,
                ),
            )
            resolved_connection.commit()
            append_audit_log(
                resolved_connection,
                "execution_order_management_failed",
                {
                    "order_id": order_id,
                    "broker_order_id": broker_order_id,
                    "action": management_action,
                    "event_id": event_id,
                    "error": error_text,
                },
            )
            _dispatch_execution_failure_alerts(resolved_connection, resolved_config)
            return {
                "status": "management_failed",
                "order_id": order_id,
                "broker_order_id": broker_order_id,
                "event_id": event_id,
                "error": error_text,
            }

        event_id = _record_execution_event(
            resolved_connection,
            order=order,
            broker_order_id=broker_order_id,
            event_type=event_type,
            broker_status=new_status,
            broker_substatus="requested",
            broker_quantity=float(normalized_replace_quantity) if management_action == "replace" else _coerce_float(order.get("quantity")),
            broker_price_local=new_price if management_action == "replace" else _coerce_float(order.get("price_local")),
            payload=payload,
        )
        resolved_connection.execute(
            """
            UPDATE execution_orders
            SET status = ?, execution_result_json = ?
            WHERE id = ?
            """,
            (new_status, json.dumps(payload, ensure_ascii=False, sort_keys=True), order_id),
        )
        resolved_connection.commit()
        append_audit_log(
            resolved_connection,
            "execution_order_management_requested",
            {
                "order_id": order_id,
                "broker_order_id": broker_order_id,
                "action": management_action,
                "event_id": event_id,
            },
        )
        return {
            "status": new_status,
            "order_id": order_id,
            "broker_order_id": broker_order_id,
            "event_id": event_id,
        }
    finally:
        if should_close:
            resolved_connection.close()


def maintain_swing_limit_orders(*, config: dict[str, Any] | None = None, connection=None, limit: int | None = None) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        cfg = _delayed_limit_order_cfg(resolved_config)
        if not bool(cfg.get("enabled", True)):
            return {"status": "disabled", "updated": 0, "orders": []}
        if str(resolved_config.get("execution", {}).get("mode")) != "live":
            return {"status": "skipped", "updated": 0, "orders": []}
        if str(resolved_config.get("execution", {}).get("adapter")) != "saxo":
            return {"status": "skipped", "updated": 0, "orders": []}
        max_rows = int(limit or cfg.get("max_replacements_per_cycle", 25) or 25)
        rows = resolved_connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE mode = 'live'
              AND strategy_type = 'swing'
              AND order_type = 'Limit'
              AND status IN ('submitted_to_broker', 'broker_working', 'broker_amended', 'broker_replace_requested')
            ORDER BY id ASC
            LIMIT ?
            """,
            (max_rows,),
        ).fetchall()
        if not rows:
            return {"status": "ok", "updated": 0, "orders": []}
        orders = [dict(row) for row in rows]
        price_map = _get_live_price_map([str(order["symbol"]) for order in orders], resolved_config)
        updates: list[dict[str, Any]] = []
        for order in orders:
            quote = price_map.get(str(order["symbol"])) or {}
            reference_price = _coerce_float(quote.get("current_price"))
            if reference_price is None:
                updates.append({"order_id": order["id"], "status": "no_quote", "symbol": order["symbol"]})
                continue
            proposed_limit = _delayed_limit_price_for_order(
                symbol=str(order["symbol"]),
                action=str(order["action"]),
                reference_price=reference_price,
                config=resolved_config,
            )
            current_limit = _coerce_float(order.get("limit_price_local")) or _coerce_float(order.get("price_local"))
            if proposed_limit is None or current_limit is None:
                continue
            if not _limit_replace_threshold_exceeded(current_limit, proposed_limit, resolved_config):
                updates.append(
                    {
                        "order_id": order["id"],
                        "status": "unchanged",
                        "symbol": order["symbol"],
                        "current_limit": current_limit,
                        "proposed_limit": proposed_limit,
                    }
                )
                continue
            result = manage_live_order(
                int(order["id"]),
                management_action="replace",
                config=resolved_config,
                connection=resolved_connection,
                new_quantity=float(order["quantity"]),
                new_price=proposed_limit,
            )
            if result["status"] in {"broker_replace_requested", "broker_amended"}:
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET limit_price_local = ?, price_local = ?
                    WHERE id = ?
                    """,
                    (proposed_limit, proposed_limit, int(order["id"])),
                )
                resolved_connection.commit()
            updates.append(
                {
                    **result,
                    "symbol": order["symbol"],
                    "reference_price": reference_price,
                    "old_limit": current_limit,
                    "new_limit": proposed_limit,
                    "quote_source": quote.get("source"),
                    "assumed_delay_minutes": int(cfg.get("assumed_delay_minutes", 15) or 15),
                }
            )
        return {"status": "ok", "updated": sum(1 for row in updates if row.get("status") in {"broker_replace_requested", "broker_amended"}), "orders": updates}
    finally:
        if should_close:
            resolved_connection.close()


def maintain_ladder_orders(*, config: dict[str, Any] | None = None, connection=None, limit: int = 50) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        if not strategy_enabled(resolved_config):
            return {"status": "disabled", "updated": 0, "orders": []}
        rows = resolved_connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE mode = 'live'
              AND strategy_type = 'ladder'
              AND strategy_role = 'stop_loss'
              AND status IN ('submitted_to_broker', 'broker_working', 'broker_amended', 'broker_replace_requested')
            ORDER BY id ASC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        if not rows:
            return {"status": "ok", "updated": 0, "orders": []}
        session = ensure_access_token(resolved_config, resolved_config["saxo"].get("session_path"))
        updates: list[dict[str, Any]] = []
        for row in rows:
            order = dict(row)
            parent_order = None
            if order.get("parent_execution_order_id"):
                parent_row = resolved_connection.execute(
                    "SELECT * FROM execution_orders WHERE id = ?",
                    (int(order["parent_execution_order_id"]),),
                ).fetchone()
                parent_order = dict(parent_row) if parent_row else None
            if parent_order and parent_order.get("status") not in {"executed", "broker_partially_filled", "submitted_to_broker", "broker_working"}:
                continue
            parent_request = _request_payload(parent_order or {})
            metadata = dict(parent_request.get("strategy_metadata") or {})
            if not metadata:
                continue
            try:
                instrument = lookup_instrument(order["symbol"], resolved_config, session)
                payload = get_chart_samples(
                    uic=instrument.uic,
                    asset_type=instrument.asset_type,
                    config=resolved_config,
                    session=session,
                    horizon_minutes=1,
                    count=20,
                )
            except Exception as exc:  # noqa: BLE001
                updates.append({"order_id": order["id"], "status": "chart_error", "error": str(exc)})
                continue
            bars = payload.get("Data", []) or []
            if not bars:
                continue
            latest_bar = bars[-1]
            current_price = _coerce_float(latest_bar.get("Close"))
            if current_price is None:
                continue
            current_stop = _coerce_float(order.get("stop_price_local")) or _coerce_float(order.get("price_local"))
            if current_stop is None:
                continue
            atr = float(metadata.get("atr_1m") or 0.0)
            trail_multiple = float(metadata.get("trail_stop_atr_multiple") or 1.25)
            decimals = int(metadata.get("decimals") or 2)
            activation_price = _coerce_float(metadata.get("trail_activation_price_local"))
            if activation_price is None or current_price < activation_price:
                continue
            proposed_stop = round(max(current_stop, current_price - (atr * trail_multiple)), max(decimals, 0))
            if proposed_stop <= current_stop:
                continue
            result = manage_live_order(
                int(order["id"]),
                management_action="replace",
                config=resolved_config,
                connection=resolved_connection,
                new_quantity=float(order["quantity"]),
                new_price=proposed_stop,
            )
            if result["status"] in {"broker_replace_requested", "broker_amended"}:
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET stop_price_local = ?, price_local = ?
                    WHERE id = ?
                    """,
                    (proposed_stop, proposed_stop, int(order["id"])),
                )
                resolved_connection.commit()
            updates.append(
                {
                    "order_id": order["id"],
                    "status": result["status"],
                    "current_price": current_price,
                    "new_stop_price": proposed_stop,
                }
            )
        return {"status": "ok", "updated": len(updates), "orders": updates}
    finally:
        if should_close:
            resolved_connection.close()


def queue_and_maybe_execute_latest_report(
    *,
    config: dict[str, Any] | None = None,
    connection=None,
    create_report_orders: bool = True,
    strategy_orders_override: list[dict[str, Any]] | None = None,
    report_override: dict[str, Any] | None = None,
    process_portfolio_sync_orders: bool = False,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        report = report_override or fetch_latest_decision_report(resolved_connection)
        orders = []
        if report and report["status"] == "completed" and create_report_orders:
            orders = _create_or_fetch_orders(
                resolved_connection,
                resolved_config,
                report,
                strategy_orders_override=strategy_orders_override,
            )
        flatten_result = enqueue_session_flatten_orders(config=resolved_config, connection=resolved_connection)
        executed = []
        executable_statuses = {
            "pending_execution",
            "waiting_for_market_open",
            "waiting_for_cash_settlement",
            "waiting_for_virtual_cash_budget",
        }
        auto_execute_queue = (
            (
                resolved_config["execution"]["mode"] == "simulation"
                and resolved_config["execution"]["auto_execute_simulation"]
            )
            or _should_auto_submit_live_orders(resolved_config)
        )
        if auto_execute_queue:
            if _should_auto_submit_live_orders(resolved_config):
                executable_statuses.add("pending_approval")
            queue_rows = resolved_connection.execute(
                f"""
                SELECT *
                FROM execution_orders
                WHERE mode = ?
                  AND status IN ({",".join("?" for _ in executable_statuses)})
                ORDER BY id ASC
                """,
                (str(resolved_config["execution"]["mode"]), *tuple(executable_statuses)),
            ).fetchall()
            for order in queue_rows:
                if str(order["strategy_type"] or "") == "portfolio_sync" and not process_portfolio_sync_orders:
                    continue
                executed.append(
                    execute_order(
                        int(order["id"]),
                        config=resolved_config,
                        connection=resolved_connection,
                        approved=_should_auto_submit_live_orders(resolved_config),
                    )
                )
        broker_sync = sync_broker_order_statuses(config=resolved_config, connection=resolved_connection)
        alert_result = _dispatch_execution_alerts(resolved_connection, resolved_config)
        return {
            "status": "ok" if create_report_orders and report and report["status"] == "completed" else "processed_existing_queue",
            "orders": orders,
            "flatten": flatten_result,
            "executed": executed,
            "broker_sync": broker_sync,
            "alerts": alert_result,
        }
    finally:
        if should_close:
            resolved_connection.close()


def fetch_execution_orders(connection, limit: int = 100) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM execution_orders
        ORDER BY id DESC
        LIMIT ?
        """,
        (limit,),
    ).fetchall()
    return [dict(row) for row in rows]


def fetch_execution_fills(connection, limit: int = 100) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT
            f.*,
            o.action,
            o.strategy_type,
            o.strategy_role,
            o.status AS order_status,
            o.estimated_value_dkk
        FROM execution_fills f
        LEFT JOIN execution_orders o ON o.id = f.execution_order_id
        ORDER BY f.id DESC
        LIMIT ?
        """,
        (limit,),
    ).fetchall()
    return [dict(row) for row in rows]


def fetch_execution_events(connection, limit: int = 100) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM execution_order_events
        ORDER BY id DESC
        LIMIT ?
        """,
        (limit,),
    ).fetchall()
    return [dict(row) for row in rows]


def fetch_invalid_simulation_trades(connection, limit: int = 50) -> list[dict[str, Any]]:
    rows = fetch_invalid_trade_ledger_rows(connection, limit=limit)
    output: list[dict[str, Any]] = []
    for row in rows:
        related_order = connection.execute(
            """
            SELECT id, status, error_text
            FROM execution_orders
            WHERE ledger_id = ?
            LIMIT 1
            """,
            (row["id"],),
        ).fetchone()
        record = dict(row)
        if related_order:
            record["execution_order_id"] = related_order["id"]
            record["execution_order_status"] = related_order["status"]
            record["execution_order_error"] = related_order["error_text"]
        else:
            record["execution_order_id"] = None
            record["execution_order_status"] = None
            record["execution_order_error"] = None
        output.append(record)
    return output


def repair_invalid_simulation_trades(*, connection, config: dict[str, Any] | None = None, limit: int = 50) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        invalid_rows = fetch_invalid_simulation_trades(resolved_connection, limit=limit)
        repaired_ids: list[int] = []
        repaired_order_ids: list[int] = []
        for row in invalid_rows:
            if row["mode"] != "simulation" or row["status"] != "executed":
                continue
            note = str(row.get("notes") or "")
            appended_note = f"{note} | quarantined invalid simulation trade".strip(" |")
            resolved_connection.execute(
                """
                UPDATE trade_ledger
                SET status = ?, notes = ?
                WHERE id = ?
                """,
                ("ignored_invalid_simulation", appended_note, row["id"]),
            )
            repaired_ids.append(int(row["id"]))
            if row.get("execution_order_id") is not None:
                resolved_connection.execute(
                    """
                    UPDATE execution_orders
                    SET status = ?, error_text = ?
                    WHERE id = ?
                    """,
                    (
                        "invalid_repaired",
                        row["validation_note"],
                        int(row["execution_order_id"]),
                    ),
                )
                repaired_order_ids.append(int(row["execution_order_id"]))
            append_audit_log(
                resolved_connection,
                "invalid_simulation_trade_repaired",
                {
                    "ledger_id": int(row["id"]),
                    "execution_order_id": row.get("execution_order_id"),
                    "symbol": row["symbol"],
                    "validation_note": row["validation_note"],
                },
            )
        resolved_connection.commit()
        return {
            "status": "ok",
            "invalid_found": len(invalid_rows),
            "ledger_rows_repaired": repaired_ids,
            "execution_orders_repaired": repaired_order_ids,
        }
    finally:
        if should_close:
            resolved_connection.close()


def reconcile_portfolio_to_broker(
    *,
    connection,
    config: dict[str, Any] | None = None,
    symbols: set[str] | list[str] | tuple[str, ...] | None = None,
) -> dict[str, Any]:
    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        batch_id = fetch_latest_batch_id(resolved_connection)
        initial_cash_dkk = _initial_cash_dkk(resolved_config)
        local_positions = fetch_portfolio_positions(
            resolved_connection,
            batch_id=batch_id,
            initial_cash_dkk=initial_cash_dkk,
            use_broker_positions=False,
        )
        local_by_symbol = {row["symbol"]: row for row in local_positions}
        broker_rows = resolved_connection.execute(
            """
            SELECT symbol, instrument_name, isin, currency, quantity, open_price_including_costs_local, open_price_local
            FROM broker_position_snapshots
            """
        ).fetchall()
        broker_by_symbol = {row["symbol"]: dict(row) for row in broker_rows}
        not_owned_symbols = {
            str(row["symbol"])
            for row in resolved_connection.execute(
                """
                SELECT DISTINCT symbol
                FROM execution_orders
                WHERE status = 'execution_failed'
                  AND error_text LIKE ?
                  AND error_text NOT LIKE ?
                """,
                ("%NotOwned%", "%reconciled to Saxo broker holdings%"),
            ).fetchall()
        }
        active_statuses = tuple(SELL_RESERVATION_STATUSES | {"waiting_for_market_open"})
        active_status_placeholders = ",".join("?" for _ in active_statuses)
        active_portfolio_sync_symbols = {
            str(row["symbol"])
            for row in resolved_connection.execute(
                f"""
                SELECT DISTINCT symbol
                FROM execution_orders
                WHERE strategy_type = 'portfolio_sync'
                  AND status IN ({active_status_placeholders})
                """,
                active_statuses,
            ).fetchall()
        }
        candidate_symbols = set(local_by_symbol) | set(broker_by_symbol) | not_owned_symbols | active_portfolio_sync_symbols
        if symbols is not None:
            requested_symbols = {str(symbol) for symbol in symbols}
            candidate_symbols = (candidate_symbols | requested_symbols) & requested_symbols
        symbols = sorted(candidate_symbols)
        fx_snapshot = fetch_ecb_fx_rates()
        created_at = datetime.now(UTC).isoformat(timespec="seconds")
        adjustments: list[dict[str, Any]] = []

        for symbol in symbols:
            local_row = local_by_symbol.get(symbol)
            broker_row = broker_by_symbol.get(symbol)
            local_quantity = float((local_row or {}).get("quantity") or 0.0)
            broker_quantity = float((broker_row or {}).get("quantity") or 0.0)
            quantity_delta = broker_quantity - local_quantity
            if abs(quantity_delta) <= 1e-9:
                continue

            currency = str((broker_row or local_row or {}).get("currency") or "DKK")
            instrument_name = (broker_row or local_row or {}).get("instrument_name") or symbol
            isin = (broker_row or local_row or {}).get("isin")
            fx_rate = fx_rate_to_dkk(currency, fx_snapshot)

            if quantity_delta > 0:
                paid_price_local = float(
                    (broker_row or {}).get("open_price_including_costs_local")
                    or (broker_row or {}).get("open_price_local")
                    or (local_row or {}).get("paid_price_local")
                    or 0.0
                )
                cost_basis_local_delta = quantity_delta * paid_price_local
                cost_basis_dkk_delta = cost_basis_local_delta * fx_rate
            else:
                local_cost_basis_dkk = float((local_row or {}).get("cost_basis_dkk") or 0.0)
                local_cost_basis_local_total = float((local_row or {}).get("cost_basis_local_total") or 0.0)
                unit_cost_dkk = local_cost_basis_dkk / local_quantity if local_quantity > 0 else 0.0
                unit_cost_local = local_cost_basis_local_total / local_quantity if local_quantity > 0 else 0.0
                cost_basis_dkk_delta = quantity_delta * unit_cost_dkk
                cost_basis_local_delta = quantity_delta * unit_cost_local

            payload = {
                "symbol": symbol,
                "instrument_name": instrument_name,
                "isin": isin,
                "currency": currency,
                "quantity_delta": quantity_delta,
                "cost_basis_local_delta": cost_basis_local_delta,
                "cost_basis_dkk_delta": cost_basis_dkk_delta,
                "local_quantity_before": local_quantity,
                "broker_quantity_target": broker_quantity,
                "note": "Reconciled local portfolio state to Saxo broker holdings.",
            }
            resolved_connection.execute(
                """
                INSERT INTO portfolio_reconciliation_adjustments (
                    created_at, symbol, instrument_name, isin, currency, quantity_delta,
                    cost_basis_local_delta, cost_basis_dkk_delta, local_quantity_before,
                    broker_quantity_target, note, raw_payload_json
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    created_at,
                    symbol,
                    instrument_name,
                    isin,
                    currency,
                    quantity_delta,
                    cost_basis_local_delta,
                    cost_basis_dkk_delta,
                    local_quantity,
                    broker_quantity,
                    payload["note"],
                    json.dumps(payload, ensure_ascii=False, sort_keys=True),
                ),
            )
            adjustments.append(payload)

        affected_symbols = tuple({row["symbol"] for row in adjustments})
        if affected_symbols:
            placeholders = ",".join("?" for _ in affected_symbols)
            resolved_connection.execute(
                f"""
                UPDATE execution_orders
                SET status = CASE
                        WHEN status = 'broker_fill_unreconciled' THEN 'reconciled_to_broker'
                        ELSE status
                    END,
                    error_text = CASE
                        WHEN error_text IS NULL OR error_text = '' THEN 'Resolved by portfolio reconciliation to Saxo broker holdings.'
                        ELSE error_text || ' | reconciled to Saxo broker holdings'
                    END
                WHERE symbol IN ({placeholders})
                  AND (
                      status = 'broker_fill_unreconciled'
                      OR (status = 'execution_failed' AND error_text LIKE ?)
                  )
                """,
                (*affected_symbols, "%NotOwned%"),
            )
            active_statuses = tuple(SELL_RESERVATION_STATUSES | {"waiting_for_market_open"})
            status_placeholders = ",".join("?" for _ in active_statuses)
            resolved_connection.execute(
                f"""
                UPDATE execution_orders
                SET status = ?, error_text = ?
                WHERE symbol IN ({placeholders})
                  AND strategy_type = 'portfolio_sync'
                  AND status IN ({status_placeholders})
                """,
                (
                    "cancelled",
                    "Cancelled because local portfolio was reconciled to Saxo broker holdings.",
                    *affected_symbols,
                    *active_statuses,
                ),
            )
        aligned_symbols = tuple(symbols)
        if aligned_symbols:
            placeholders = ",".join("?" for _ in aligned_symbols)
            resolved_connection.execute(
                f"""
                UPDATE execution_orders
                SET error_text = CASE
                        WHEN error_text IS NULL OR error_text = '' THEN 'Resolved by portfolio reconciliation to Saxo broker holdings.'
                        ELSE error_text || ' | reconciled to Saxo broker holdings'
                    END
                WHERE symbol IN ({placeholders})
                  AND status = 'execution_failed'
                  AND error_text LIKE ?
                  AND error_text NOT LIKE ?
                """,
                (*aligned_symbols, "%NotOwned%", "%reconciled to Saxo broker holdings%"),
            )
            active_status_placeholders = ",".join("?" for _ in active_statuses)
            resolved_connection.execute(
                f"""
                UPDATE execution_orders
                SET status = ?, error_text = ?
                WHERE symbol IN ({placeholders})
                  AND strategy_type = 'portfolio_sync'
                  AND status IN ({active_status_placeholders})
                """,
                (
                    "cancelled",
                    "Cancelled because local portfolio was reconciled to Saxo broker holdings.",
                    *aligned_symbols,
                    *active_statuses,
                ),
            )

        append_audit_log(
            resolved_connection,
            "portfolio_reconciled_to_broker",
            {"created_at": created_at, "adjustments": adjustments},
        )
        resolved_connection.commit()
        return {"status": "ok", "adjustments": adjustments, "reconciled_symbols": [row["symbol"] for row in adjustments]}
    finally:
        if should_close:
            resolved_connection.close()


def export_audit_bundle(output_dir: str, *, config: dict[str, Any] | None = None, connection=None) -> dict[str, Any]:
    import csv

    resolved_config, resolved_connection, should_close = _get_connection_and_config(config, connection)
    try:
        out = Path(output_dir)
        out.mkdir(parents=True, exist_ok=True)
        exports = {
            "trade_ledger": out / "trade_ledger.csv",
            "lot_realizations": out / "lot_realizations.csv",
            "decision_reports": out / "decision_reports.csv",
            "execution_orders": out / "execution_orders.csv",
            "execution_fills": out / "execution_fills.csv",
            "execution_order_events": out / "execution_order_events.csv",
            "notification_deliveries": out / "notification_deliveries.csv",
            "audit_log": out / "audit_log.csv",
        }
        for table_name, path in exports.items():
            rows = resolved_connection.execute(f"SELECT * FROM {table_name}").fetchall()
            if rows:
                fieldnames = list(rows[0].keys())
            else:
                fieldnames = [
                    row["name"]
                    for row in resolved_connection.execute(f"PRAGMA table_info({table_name})").fetchall()
                ]
            with path.open("w", encoding="utf-8", newline="") as handle:
                writer = csv.DictWriter(handle, fieldnames=fieldnames)
                if fieldnames:
                    writer.writeheader()
                    writer.writerows([dict(row) for row in rows])
        return {"status": "ok", "output_dir": str(out), "files": {k: str(v) for k, v in exports.items()}}
    finally:
        if should_close:
            resolved_connection.close()
