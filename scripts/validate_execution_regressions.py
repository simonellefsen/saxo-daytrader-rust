from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from saxo_daytrader_xai.config import load_config
from saxo_daytrader_xai.db import connect, init_db
import saxo_daytrader_xai.execution_engine as execution_engine
from saxo_daytrader_xai.execution_engine import (
    _active_sell_reservations,
    _available_sell_quantity,
    _create_ladder_protection_orders_after_fill,
    _defer_ladder_entry_bracket,
    _is_retryable_execution_failure,
    _sync_incremental_live_fill,
    adopt_broker_holdings_into_local_ledger,
    enqueue_session_flatten_orders,
    execute_order,
    queue_and_maybe_execute_latest_report,
    reconcile_portfolio_to_broker,
    sync_saxo_sim_account_to_portfolio,
)
from saxo_daytrader_xai.portfolio import (
    fetch_portfolio_integrity_status,
    fetch_portfolio_positions,
    fetch_realised_daily_pnl_summary,
)
from saxo_daytrader_xai.saxo_openapi import SaxoInstrument, build_order_payload, normalize_order_price
from saxo_daytrader_xai.strategy_engine import CandidateMetrics, _build_entry_ladder_orders


def _config() -> dict:
    config = load_config("config.yaml")
    config["saxo"]["account_key"] = "account-key"
    config["saxo"]["environment"] = "sim"
    config["execution"]["mode"] = "live"
    config["execution"]["price_tick_overrides"] = {
        "xetr": 0.05,
        "xosl": 0.10,
        "xhel": 0.01,
        "xnys": 0.01,
        "xnas": 0.01,
    }
    config["strategy"]["ladder"]["submit_bracket_with_entry"] = False
    config["strategy"]["ladder"]["submit_stop_loss_after_fill"] = False
    config["strategy"]["ladder"]["submit_take_profit_after_fill"] = False
    config["strategy"]["ladder"]["session_flatten_enabled"] = True
    return config


def _fake_session() -> dict:
    return {
        "environment": "sim",
        "access_token": "mock-token",
        "account_key": "account-key",
    }


def _fake_instrument(symbol: str) -> SaxoInstrument:
    return SaxoInstrument(
        symbol=symbol,
        uic=12345,
        asset_type="Stock",
        exchange_id=symbol.split(":", 1)[1].upper(),
        description=symbol,
        tradable_as=["Stock"],
        currency_code="USD",
        isin_code=None,
    )


def _seed_batch_and_local_lot(connection, *, symbol: str = "CEG:xnas", quantity: float = 3.0) -> None:
    connection.execute(
        """
        INSERT INTO import_batches (
            batch_id, imported_at, source_csv, source_position_count,
            imported_position_count, excluded_position_count, notes
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        ("regression-batch", "2026-05-01T06:00:00+00:00", "", 0, 0, 0, "execution-regression"),
    )
    connection.execute(
        """
        INSERT INTO trade_ledger (
            id, created_at, symbol, instrument_name, side, quantity, price_local, currency,
            gross_amount_dkk, commission_dkk, tax_dkk, net_amount_dkk, mode, status,
            notes, decision_context_json, tax_year, batch_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            1,
            "2026-05-01T00:01:00+00:00",
            symbol,
            symbol,
            "BUY",
            quantity,
            310.0,
            "USD",
            5940.0,
            30.0,
            0.0,
            -5970.0,
            "live",
            "approved",
            "regression buy",
            "{}",
            2026,
            "regression-batch",
        ),
    )
    connection.execute(
        """
        INSERT INTO position_lots (
            lot_id, batch_id, created_at, acquired_at, symbol, instrument_name,
            quantity_original, currency, cost_basis_total_local, cost_basis_total_dkk,
            fx_rate_to_dkk, source_type, source_reference, raw_payload_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            "buy:1",
            "regression-batch",
            "2026-05-01T00:01:00+00:00",
            "2026-05-01T00:01:00+00:00",
            symbol,
            symbol,
            quantity,
            "USD",
            quantity * 310.0,
            5970.0,
            6.4,
            "live_buy",
            "execution_order:1",
            "{}",
        ),
    )
    connection.commit()


def _assert_sim_integrity_ignores_non_authoritative_broker_snapshot() -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO import_batches (
                batch_id, imported_at, source_csv, source_position_count,
                imported_position_count, excluded_position_count, notes
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("sim-baseline", "2026-05-03T08:30:00+00:00", "", 1, 1, 0, "sim baseline"),
        )
        connection.execute(
            """
            INSERT INTO position_snapshots (
                batch_id, imported_at, instrument_name, symbol, quantity, currency,
                open_price_local, current_price_local, cost_basis_local, cost_basis_dkk,
                market_value_local, market_value_dkk, unrealised_pnl_dkk, source_csv, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "sim-baseline",
                "2026-05-03T08:30:00+00:00",
                "Analog Devices Inc",
                "ADI:xnas",
                9,
                "USD",
                210.0,
                210.0,
                1890.0,
                12096.0,
                1890.0,
                12096.0,
                0.0,
                "",
                "{}",
            ),
        )
        connection.commit()

        sim_integrity = fetch_portfolio_integrity_status(
            connection,
            initial_cash_dkk=9770.17,
            use_broker_positions=False,
        )
        assert sim_integrity["healthy"], sim_integrity
        assert sim_integrity["mismatches"] == [], sim_integrity

        live_integrity = fetch_portfolio_integrity_status(
            connection,
            initial_cash_dkk=9770.17,
            use_broker_positions=True,
        )
        assert not live_integrity["healthy"], live_integrity
        assert live_integrity["mismatches"][0]["symbol"] == "ADI:xnas", live_integrity
    finally:
        connection.close()


def _assert_price_normalization(config: dict) -> None:
    assert normalize_order_price("ADS:xetr", 147.66, config, side="buy_limit") == 147.65
    assert normalize_order_price("ADS:xetr", 147.66, config, side="sell_limit") == 147.70
    assert normalize_order_price("IFX:xetr", 54.876, config, side="buy_limit") == 54.85
    assert normalize_order_price("TOM:xosl", 93.89, config, side="buy_limit") == 93.80
    assert normalize_order_price("NOKIA:xhel", 10.211, config, side="buy_limit") == 10.21


def _assert_broker_payload_normalizes_all_prices(config: dict) -> None:
    import saxo_daytrader_xai.saxo_openapi as saxo_openapi

    original_lookup = saxo_openapi.lookup_instrument
    saxo_openapi.lookup_instrument = lambda symbol, _config, _session: _fake_instrument(symbol)
    try:
        payload = build_order_payload(
            symbol="ADS:xetr",
            action="BUY",
            quantity=3,
            external_reference="test-ads",
            config=config,
            session=_fake_session(),
            order_type="Limit",
            limit_price=147.66,
            related_orders=[
                {
                    "action": "SELL",
                    "order_type": "Limit",
                    "quantity": 3,
                    "limit_price": 149.03,
                },
                {
                    "action": "SELL",
                    "order_type": "StopLimit",
                    "quantity": 3,
                    "stop_price": 144.87,
                    "limit_price": 144.83,
                },
            ],
        )
    finally:
        saxo_openapi.lookup_instrument = original_lookup

    assert payload["OrderPrice"] == 147.65, payload
    assert payload["Orders"][0]["OrderPrice"] == 149.05, payload
    assert payload["Orders"][1]["OrderPrice"] == 144.85, payload
    assert payload["Orders"][1]["StopLimitPrice"] == 144.80, payload


def _assert_broker_payload_uses_saxo_tick_scheme(config: dict) -> None:
    import saxo_daytrader_xai.saxo_openapi as saxo_openapi

    original_lookup = saxo_openapi.lookup_instrument
    original_details = saxo_openapi.get_instrument_details
    saxo_openapi.lookup_instrument = lambda symbol, _config, _session: _fake_instrument(symbol)
    saxo_openapi.get_instrument_details = lambda _instrument, _config, _session: {
        "TickSizeScheme": {
            "Elements": [
                {"HighPrice": 1000.0, "TickSize": 0.5},
            ],
            "DefaultTickSize": 1.0,
        }
    }
    try:
        payload = build_order_payload(
            symbol="NKT:xcse",
            action="BUY",
            quantity=8,
            external_reference="test-nkt",
            config={**config, "execution": {**config["execution"], "price_tick_overrides": {}}},
            session=_fake_session(),
            order_type="Limit",
            limit_price=932.9,
            related_orders=[
                {
                    "action": "SELL",
                    "order_type": "Limit",
                    "quantity": 8,
                    "limit_price": 941.1,
                }
            ],
        )
    finally:
        saxo_openapi.lookup_instrument = original_lookup
        saxo_openapi.get_instrument_details = original_details

    assert payload["OrderPrice"] == 932.5, payload
    assert payload["Orders"][0]["OrderPrice"] == 941.5, payload


def _assert_tick_size_failures_do_not_retry() -> None:
    assert not _is_retryable_execution_failure(
        "Order precheck failed: PriceNotInTickSizeIncrements: The order price is not in tick size increments."
    )
    assert _is_retryable_execution_failure("Order precheck failed: Rate limit exceeded!")


def _assert_strategy_prices_are_tick_aligned(config: dict) -> None:
    metrics = CandidateMetrics(
        symbol="ADS:xetr",
        session_tag="eu_open",
        currency="EUR",
        current_price_local=147.71,
        atr_1m=0.62,
        rung_spacing_local=0.17,
        technical_score=88.0,
        volume_score=80.0,
        rvol_15m=1.5,
        vwap_local=147.6,
        decimals=2,
        notes=[],
    )
    orders, _cash, _capacity, notes = _build_entry_ladder_orders(
        symbol="ADS:xetr",
        session_tag="eu_open",
        target_weight_pct=0.10,
        metrics=metrics,
        config=config,
        portfolio_value_dkk=250000.0,
        remaining_cash_dkk=60000.0,
        remaining_capacity=5,
    )
    assert orders, notes
    for order in orders:
        assert order["limit_price_local"] == normalize_order_price(
            order["symbol"], order["limit_price_local"], config, side="buy_limit"
        )
        related = order["related_orders"]
        assert related[0]["limit_price"] == normalize_order_price(
            order["symbol"], related[0]["limit_price"], config, side="sell_limit"
        )
        assert related[1]["order_type"] == "StopLimit"
        assert related[1]["stop_price"] == normalize_order_price(
            order["symbol"], related[1]["stop_price"], config, side="sell_stop"
        )
        assert related[1]["limit_price"] == normalize_order_price(
            order["symbol"], related[1]["limit_price"], config, side="sell_stop_limit"
        )


def _assert_ladder_bracket_is_deferred(config: dict) -> None:
    order = {"action": "BUY", "strategy_type": "ladder", "strategy_role": "entry"}
    request_payload = {"related_orders": [{"strategy_role": "stop_loss"}]}
    assert _defer_ladder_entry_bracket(config, order, request_payload)


def _assert_protection_orders_are_planned_by_default(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO import_batches (
                batch_id, imported_at, source_csv, source_position_count,
                imported_position_count, excluded_position_count, notes
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("regression-batch", "2026-04-29T09:00:00+00:00", "", 0, 0, 0, "execution-regression"),
        )
        connection.execute(
            """
            INSERT INTO decision_reports (
                id, created_at, report_date, batch_id, model, status, analysis_window_active,
                response_id, prompt_text, request_json, response_json, report_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                1,
                "2026-04-29T09:00:00+00:00",
                "2026-04-29",
                "regression-batch",
                "test",
                "completed",
                1,
                None,
                "{}",
                "{}",
                "{}",
                "{}",
                None,
            ),
        )
        request_json = {
            "related_orders": [
                {
                    "action": "SELL",
                    "order_type": "Limit",
                    "quantity": 4,
                    "limit_price": 270.15,
                    "strategy_role": "take_profit",
                },
                {
                    "action": "SELL",
                    "order_type": "StopLimit",
                    "quantity": 4,
                    "stop_price": 252.31,
                    "limit_price": 252.28,
                    "strategy_role": "stop_loss",
                },
            ]
        }
        connection.execute(
            """
            INSERT INTO execution_orders (
                id, created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local,
                currency, estimated_value_dkk, approval_required, approved_at, broker_order_id,
                parent_execution_order_id, strategy_type, strategy_session, strategy_key,
                strategy_role, request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                10,
                "2026-04-29T09:01:00+00:00",
                1,
                "AMZN:xnas",
                "BUY",
                "Limit",
                "live",
                "executed",
                "saxo",
                0.04,
                4,
                260.0,
                260.0,
                None,
                "USD",
                7000.0,
                0,
                "2026-04-29T09:01:01+00:00",
                "broker-1",
                None,
                "ladder",
                "us_open",
                "us_open:AMZN:xnas:entry:0",
                "entry",
                json.dumps(request_json),
                "{}",
                None,
            ),
        )
        connection.commit()
        parent = {
            "id": 10,
            "report_id": 1,
            "symbol": "AMZN:xnas",
            "action": "BUY",
            "order_type": "Limit",
            "mode": "live",
            "adapter": "saxo",
            "requested_weight_pct": 0.04,
            "quantity": 4,
            "currency": "USD",
            "estimated_value_dkk": 7000.0,
            "broker_order_id": "broker-1",
            "strategy_type": "ladder",
            "strategy_session": "us_open",
            "strategy_key": "us_open:AMZN:xnas:entry:0",
            "request_json": json.dumps(request_json),
        }
        created = _create_ladder_protection_orders_after_fill(connection, parent_order=parent, config=config)
        statuses = {row["strategy_role"]: row["status"] for row in created}
        assert statuses == {
            "take_profit": "planned_take_profit",
            "stop_loss": "planned_stop_loss",
        }, statuses
    finally:
        connection.close()


def _assert_active_sell_reservations_reduce_available_quantity() -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO execution_orders (
                id, created_at, symbol, action, order_type, mode, status, adapter,
                quantity, price_local, currency, estimated_value_dkk, request_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                20,
                "2026-04-30T13:00:00+00:00",
                "V:xnys",
                "SELL",
                "Market",
                "live",
                "broker_working",
                "saxo",
                18,
                330.0,
                "USD",
                39000.0,
                "{}",
            ),
        )
        connection.commit()
        assert _active_sell_reservations(connection)["V:xnys"] == 18
        assert _available_sell_quantity(connection, "V:xnys", 18) == 0
        assert _available_sell_quantity(connection, "V:xnys", 20) == 2
    finally:
        connection.close()


def _assert_realised_daily_pnl_includes_commission() -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO trade_ledger (
                created_at, symbol, side, quantity, price_local, currency,
                gross_amount_dkk, commission_dkk, tax_dkk, net_amount_dkk,
                mode, status, realised_gain_dkk
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-04-30T10:00:00+00:00",
                "AMZN:xnas",
                "SELL",
                1,
                260.0,
                "USD",
                1700.0,
                25.0,
                0.0,
                1675.0,
                "live",
                "executed",
                100.0,
            ),
        )
        connection.commit()
        summary = fetch_realised_daily_pnl_summary(connection, since_utc="2026-04-30T00:00:00+00:00")
        assert summary["realised_gain_dkk"] == 100.0, summary
        assert summary["commission_dkk"] == 25.0, summary
        assert summary["realised_pnl_after_commission_dkk"] == 75.0, summary
    finally:
        connection.close()


def _assert_oversized_broker_sell_fill_closes_local_lots(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        _seed_batch_and_local_lot(connection, symbol="CEG:xnas", quantity=3.0)
        connection.execute(
            """
            INSERT INTO execution_orders (
                id, created_at, symbol, action, order_type, mode, status, adapter,
                quantity, price_local, currency, estimated_value_dkk, request_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                99,
                "2026-05-01T06:05:00+00:00",
                "CEG:xnas",
                "SELL",
                "Market",
                "live",
                "submitted_to_broker",
                "saxo",
                6,
                313.08,
                "USD",
                12000.0,
                "{}",
            ),
        )
        connection.commit()
        order = dict(connection.execute("SELECT * FROM execution_orders WHERE id = 99").fetchone())
        result = _sync_incremental_live_fill(
            connection,
            config,
            order,
            {
                "Status": "FinalFill",
                "SubStatus": "Confirmed",
                "FilledAmount": 6,
                "AveragePrice": 313.08,
            },
            broker_order_id="broker-ceg-sell",
            fill_status="FinalFill",
        )
        assert result["ledger_quantity"] == 3.0, result
        assert result["broker_only_quantity"] == 3.0, result
        sell_row = connection.execute(
            "SELECT quantity, notes FROM trade_ledger WHERE symbol = ? AND side = 'SELL'",
            ("CEG:xnas",),
        ).fetchone()
        assert sell_row is not None
        assert float(sell_row["quantity"]) == 3.0, dict(sell_row)
        assert "Broker filled 6 shares" in str(sell_row["notes"])
        fill_row = connection.execute(
            "SELECT cumulative_quantity, delta_quantity, ledger_id FROM execution_fills WHERE execution_order_id = ?",
            (99,),
        ).fetchone()
        assert float(fill_row["cumulative_quantity"]) == 6.0, dict(fill_row)
        assert float(fill_row["delta_quantity"]) == 6.0, dict(fill_row)
        local_positions = fetch_portfolio_positions(connection, use_broker_positions=False)
        assert not [row for row in local_positions if row["symbol"] == "CEG:xnas"], local_positions
    finally:
        connection.close()


def _assert_flatten_orders_are_capped_to_local_lots(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    original_flatten_due = execution_engine._flatten_due_for_symbol
    execution_engine._flatten_due_for_symbol = lambda _symbol, _config: True
    try:
        _seed_batch_and_local_lot(connection, symbol="CEG:xnas", quantity=3.0)
        connection.execute(
            """
            INSERT INTO broker_position_snapshots (
                symbol, updated_at, instrument_name, quantity, currency,
                open_price_local, open_price_including_costs_local, can_be_closed, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "CEG:xnas",
                "2026-05-01T06:05:00+00:00",
                "Constellation Energy Corp",
                6,
                "USD",
                310.0,
                310.0,
                1,
                "{}",
            ),
        )
        connection.commit()
        result = enqueue_session_flatten_orders(config=config, connection=connection)
        assert result["created_order_ids"], result
        flatten_row = connection.execute(
            "SELECT quantity FROM execution_orders WHERE id = ?",
            (result["created_order_ids"][0],),
        ).fetchone()
        assert float(flatten_row["quantity"]) == 3.0, dict(flatten_row)
    finally:
        execution_engine._flatten_due_for_symbol = original_flatten_due
        connection.close()


def _assert_flatten_ignores_local_only_positions_when_broker_snapshot_exists(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    original_flatten_due = execution_engine._flatten_due_for_symbol
    execution_engine._flatten_due_for_symbol = lambda _symbol, _config: True
    try:
        _seed_batch_and_local_lot(connection, symbol="ARKK:xmil", quantity=160.0)
        connection.execute(
            """
            INSERT INTO broker_position_snapshots (
                symbol, updated_at, instrument_name, quantity, currency,
                open_price_local, open_price_including_costs_local, can_be_closed, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "ADI:xnas",
                "2026-05-04T14:48:00+00:00",
                "Analog Devices Inc",
                9,
                "USD",
                398.64,
                398.64,
                1,
                "{}",
            ),
        )
        stale_cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-04T14:45:00+00:00",
                None,
                "ARKK:xmil",
                "SELL",
                "Market",
                "live",
                "pending_execution",
                "saxo",
                0.0,
                160,
                6.90,
                None,
                None,
                "EUR",
                8280.0,
                0,
                None,
                "flatten",
                "session_close",
                "flatten:ARKK:xmil:test",
                "flatten_close",
                "{}",
                None,
                None,
            ),
        )
        connection.commit()
        result = enqueue_session_flatten_orders(config=config, connection=connection)
        assert result["created_order_ids"] == [], result
        stale = connection.execute("SELECT status, error_text FROM execution_orders WHERE id = ?", (int(stale_cursor.lastrowid),)).fetchone()
        assert stale["status"] == "cancelled", dict(stale)
        assert "no held quantity" in stale["error_text"], dict(stale)
    finally:
        execution_engine._flatten_due_for_symbol = original_flatten_due
        connection.close()


def _assert_scoped_reconciliation_restores_residual_broker_position(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO trade_ledger (
                created_at, symbol, instrument_name, side, quantity, price_local, currency,
                gross_amount_dkk, commission_dkk, tax_dkk, cost_basis_sold_dkk, cost_basis_sold_local,
                net_amount_dkk, mode, status, notes, decision_context_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-01T10:00:00+00:00",
                "GN:xcse",
                "GN Store Nord A/S",
                "BUY",
                174,
                98.4,
                "DKK",
                17121.6,
                29.0,
                0.0,
                0.0,
                0.0,
                -17150.6,
                "live",
                "approved",
                "regression buy",
                "{}",
            ),
        )
        connection.execute(
            """
            INSERT INTO trade_ledger (
                created_at, symbol, instrument_name, side, quantity, price_local, currency,
                gross_amount_dkk, commission_dkk, tax_dkk, cost_basis_sold_dkk, cost_basis_sold_local,
                net_amount_dkk, mode, status, notes, decision_context_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-01T14:47:02+00:00",
                "GN:xcse",
                "GN Store Nord A/S",
                "SELL",
                174,
                99.4,
                "DKK",
                17295.6,
                29.0,
                0.0,
                17150.6,
                17121.6,
                17266.6,
                "live",
                "executed",
                "regression sell",
                "{}",
            ),
        )
        connection.execute(
            """
            INSERT INTO broker_position_snapshots (
                symbol, updated_at, instrument_name, quantity, currency,
                open_price_local, open_price_including_costs_local, can_be_closed, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "GN:xcse",
                "2026-05-01T14:48:00+00:00",
                "GN Store Nord A/S",
                87,
                "DKK",
                98.4,
                98.4,
                1,
                "{}",
            ),
        )
        stale_sync_cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-01T14:47:30+00:00",
                None,
                "GN:xcse",
                "BUY",
                "Market",
                "live",
                "waiting_for_market_open",
                "saxo",
                None,
                87,
                98.4,
                None,
                None,
                "DKK",
                8560.8,
                0,
                None,
                "portfolio_sync",
                "saxo_sim",
                "portfolio_sync:GN:xcse:test",
                "increase_to_target",
                "{}",
                None,
                "Exchange closed",
            ),
        )
        connection.commit()

        before = fetch_portfolio_positions(connection, use_broker_positions=False)
        assert not [row for row in before if row["symbol"] == "GN:xcse"], before

        result = reconcile_portfolio_to_broker(connection=connection, config=config, symbols={"GN:xcse"})
        assert result["reconciled_symbols"] == ["GN:xcse"], result
        assert result["adjustments"][0]["quantity_delta"] == 87.0, result

        after = fetch_portfolio_positions(connection, use_broker_positions=False)
        gn_rows = [row for row in after if row["symbol"] == "GN:xcse"]
        assert len(gn_rows) == 1, after
        assert float(gn_rows[0]["quantity"]) == 87.0, gn_rows[0]
        stale_sync = connection.execute("SELECT status, error_text FROM execution_orders WHERE id = ?", (int(stale_sync_cursor.lastrowid),)).fetchone()
        assert stale_sync["status"] == "cancelled", dict(stale_sync)
        assert "reconciled to Saxo broker holdings" in stale_sync["error_text"], dict(stale_sync)
    finally:
        connection.close()


def _assert_portfolio_sync_is_sim_only(config: dict) -> None:
    live_config = json.loads(json.dumps(config))
    live_config["saxo"]["environment"] = "live"

    connection = connect(":memory:")
    init_db(connection)
    try:
        try:
            sync_saxo_sim_account_to_portfolio(config=live_config, connection=connection)
        except ValueError as exc:
            assert "SIM" in str(exc), exc
        else:
            raise AssertionError("Expected Saxo LIVE portfolio sync to be blocked")

        cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-01T10:00:00+00:00",
                None,
                "AMD:xnas",
                "BUY",
                "Market",
                "live",
                "pending_execution",
                "saxo",
                None,
                1,
                100.0,
                None,
                None,
                "USD",
                650.0,
                0,
                None,
                "portfolio_sync",
                "saxo_sim",
                "portfolio_sync:AMD:xnas:test",
                "increase_to_target",
                "{}",
                None,
                None,
            ),
        )
        connection.commit()
        result = execute_order(int(cursor.lastrowid), config=live_config, connection=connection, approved=True)
        assert result["status"] == "execution_failed", result
        assert "SIM-only" in result["error"], result
    finally:
        connection.close()


def _assert_portfolio_sync_fills_do_not_mutate_local_ledger(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    try:
        connection.execute(
            """
            INSERT INTO import_batches (
                batch_id, imported_at, source_csv, source_position_count,
                imported_position_count, excluded_position_count, notes
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            ("sync-baseline", "2026-05-03T08:30:00+00:00", "", 1, 1, 0, "sync baseline"),
        )
        connection.execute(
            """
            INSERT INTO position_snapshots (
                batch_id, imported_at, instrument_name, symbol, quantity, currency,
                open_price_local, current_price_local, cost_basis_local, cost_basis_dkk,
                market_value_local, market_value_dkk, unrealised_pnl_dkk, source_csv, raw_payload_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "sync-baseline",
                "2026-05-03T08:30:00+00:00",
                "Analog Devices Inc",
                "ADI:xnas",
                9,
                "USD",
                210.0,
                210.0,
                1890.0,
                12096.0,
                1890.0,
                12096.0,
                0.0,
                "",
                "{}",
            ),
        )
        cursor = connection.execute(
            """
            INSERT INTO execution_orders (
                created_at, report_id, symbol, action, order_type, mode, status, adapter,
                requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
                approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
                request_json, execution_result_json, error_text
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                "2026-05-03T08:50:16+00:00",
                None,
                "ADI:xnas",
                "BUY",
                "Market",
                "live",
                "broker_fill_unreconciled",
                "saxo",
                None,
                9,
                398.64,
                None,
                None,
                "USD",
                22914.0,
                0,
                None,
                "portfolio_sync",
                "saxo_sim",
                "portfolio_sync:ADI:xnas:test",
                "increase_to_target",
                json.dumps({"strategy_type": "portfolio_sync"}),
                None,
                "Previous code failed here with insufficient local cash.",
            ),
        )
        connection.commit()

        order = dict(connection.execute("SELECT * FROM execution_orders WHERE id = ?", (int(cursor.lastrowid),)).fetchone())
        before_positions = fetch_portfolio_positions(connection, use_broker_positions=False)
        before_quantity = [row for row in before_positions if row["symbol"] == "ADI:xnas"][0]["quantity"]

        result = _sync_incremental_live_fill(
            connection,
            config,
            order,
            {"Status": "FinalFill", "SubStatus": "Confirmed", "FilledAmount": 9, "AveragePrice": 398.64},
            broker_order_id="5038088867",
            fill_status="FinalFill",
        )
        assert result["status"] == "portfolio_sync_broker_fill_synced", result
        assert result["ledger_id"] is None, result
        assert result["delta_quantity"] == 9.0, result

        trade_count = connection.execute("SELECT COUNT(*) AS count FROM trade_ledger").fetchone()["count"]
        assert trade_count == 0, trade_count
        fill_count = connection.execute("SELECT COUNT(*) AS count FROM execution_fills").fetchone()["count"]
        assert fill_count == 1, fill_count
        after_positions = fetch_portfolio_positions(connection, use_broker_positions=False)
        after_quantity = [row for row in after_positions if row["symbol"] == "ADI:xnas"][0]["quantity"]
        assert float(after_quantity) == float(before_quantity) == 9.0, after_positions
    finally:
        connection.close()


def _assert_broker_adoption_is_blocked_in_sim(config: dict) -> None:
    sim_config = json.loads(json.dumps(config))
    sim_config["saxo"]["environment"] = "sim"

    connection = connect(":memory:")
    init_db(connection)
    try:
        try:
            adopt_broker_holdings_into_local_ledger(config=sim_config, connection=connection)
        except ValueError as exc:
            assert "SIM" in str(exc), exc
        else:
            raise AssertionError("Expected Saxo SIM broker adoption to be blocked")
    finally:
        connection.close()


def _insert_queue_order(connection, *, symbol: str, strategy_type: str | None, status: str = "waiting_for_market_open") -> int:
    cursor = connection.execute(
        """
        INSERT INTO execution_orders (
            created_at, report_id, symbol, action, order_type, mode, status, adapter,
            requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk,
            approval_required, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role,
            request_json, execution_result_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            "2026-05-05T07:20:09+00:00",
            None,
            symbol,
            "BUY",
            "Market",
            "live",
            status,
            "saxo",
            None,
            1,
            100.0,
            None,
            None,
            "USD",
            650.0,
            0,
            None,
            strategy_type,
            "test",
            f"{strategy_type or 'manual'}:{symbol}:test",
            "entry",
            json.dumps({"symbol": symbol, "strategy_type": strategy_type}),
            None,
            "Exchange closed for regression setup.",
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


def _assert_scheduler_queue_skips_portfolio_sync_orders(config: dict) -> None:
    connection = connect(":memory:")
    init_db(connection)
    original_execute_order = execution_engine.execute_order
    original_sync = execution_engine.sync_broker_order_statuses
    original_alerts = execution_engine._dispatch_execution_alerts
    executed_ids: list[int] = []

    def fake_execute_order(order_id: int, *, config=None, connection=None, approved: bool = False):  # noqa: ANN001
        executed_ids.append(order_id)
        connection.execute(
            "UPDATE execution_orders SET status = ?, error_text = NULL WHERE id = ?",
            ("executed", order_id),
        )
        connection.commit()
        return {"status": "executed", "order_id": order_id}

    try:
        portfolio_sync_id = _insert_queue_order(connection, symbol="ADI:xnas", strategy_type="portfolio_sync")
        manager_order_id = _insert_queue_order(connection, symbol="NVDA:xnas", strategy_type="swing")
        execution_engine.execute_order = fake_execute_order
        execution_engine.sync_broker_order_statuses = lambda **_kwargs: {"status": "mocked"}
        execution_engine._dispatch_execution_alerts = lambda *_args, **_kwargs: {"status": "mocked"}

        result = queue_and_maybe_execute_latest_report(
            config=config,
            connection=connection,
            create_report_orders=False,
        )
        assert result["status"] == "processed_existing_queue", result
        assert executed_ids == [manager_order_id], executed_ids
        portfolio_sync_status = connection.execute(
            "SELECT status FROM execution_orders WHERE id = ?",
            (portfolio_sync_id,),
        ).fetchone()["status"]
        manager_status = connection.execute(
            "SELECT status FROM execution_orders WHERE id = ?",
            (manager_order_id,),
        ).fetchone()["status"]
        assert portfolio_sync_status == "waiting_for_market_open", portfolio_sync_status
        assert manager_status == "executed", manager_status
    finally:
        execution_engine.execute_order = original_execute_order
        execution_engine.sync_broker_order_statuses = original_sync
        execution_engine._dispatch_execution_alerts = original_alerts
        connection.close()


def main() -> int:
    config = _config()
    _assert_sim_integrity_ignores_non_authoritative_broker_snapshot()
    _assert_price_normalization(config)
    _assert_broker_payload_normalizes_all_prices(config)
    _assert_broker_payload_uses_saxo_tick_scheme(config)
    _assert_tick_size_failures_do_not_retry()
    _assert_strategy_prices_are_tick_aligned(config)
    _assert_ladder_bracket_is_deferred(config)
    _assert_protection_orders_are_planned_by_default(config)
    _assert_active_sell_reservations_reduce_available_quantity()
    _assert_realised_daily_pnl_includes_commission()
    _assert_oversized_broker_sell_fill_closes_local_lots(config)
    _assert_flatten_orders_are_capped_to_local_lots(config)
    _assert_flatten_ignores_local_only_positions_when_broker_snapshot_exists(config)
    _assert_scoped_reconciliation_restores_residual_broker_position(config)
    _assert_portfolio_sync_is_sim_only(config)
    _assert_portfolio_sync_fills_do_not_mutate_local_ledger(config)
    _assert_broker_adoption_is_blocked_in_sim(config)
    _assert_scheduler_queue_skips_portfolio_sync_orders(config)
    print("Execution regression validation passed.")
    print("Covered: Saxo tick-size rounding, sell reservations, realised daily P/L, deferred brackets, planned protection-order defaults, broker/local fill reconciliation, residual broker-position reconciliation, broker-authoritative flatten guards, SIM integrity warning suppression, SIM-only portfolio sync guards, portfolio-sync fill ledger isolation, SIM broker-adoption blocking, and scheduler queue isolation for portfolio-sync orders.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
