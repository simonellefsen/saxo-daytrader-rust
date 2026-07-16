-- Broker bootstrap 2026-07-16: give every live broker position a local
-- cost-basis snapshot + lot, and retire the stale 2026-05-18 import rows.
-- Basis = broker open price (including costs when available) x quantity,
-- converted to DKK with the current cached FX rate.
BEGIN;

INSERT INTO import_batches (
    batch_id, imported_at, source_csv, source_position_count,
    imported_position_count, excluded_position_count, notes
)
SELECT
    'broker-bootstrap-20260716T190000Z',
    '2026-07-16T19:00:00Z',
    'broker_position_snapshots',
    COUNT(*), COUNT(*), 0,
    'Bootstrap local cost basis from broker-authoritative open positions; supersedes stale 2026-05-18 import rows that no longer match the broker book.'
FROM broker_position_snapshots
WHERE quantity > 0;

INSERT INTO position_snapshots (
    batch_id, imported_at, instrument_name, symbol, isin, quantity, currency,
    open_price_local, current_price_local, cost_basis_local, cost_basis_dkk,
    market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk,
    allocation_pct, status, account_name, asset_class, market_status,
    value_date, source_csv, excluded, raw_payload_json,
    environment_id, account_uid
)
SELECT
    'broker-bootstrap-20260716T190000Z',
    '2026-07-16T19:00:00Z',
    COALESCE(b.instrument_name, b.symbol),
    b.symbol,
    b.isin,
    b.quantity,
    COALESCE(b.currency, 'DKK'),
    b.open_price_local,
    b.open_price_local,
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity,
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity
        * COALESCE(fx.rate_to_dkk, 1.0),
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity,
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity
        * COALESCE(fx.rate_to_dkk, 1.0),
    0, 0, 0,
    'Open', 'Broker-Bootstrap', COALESCE(b.asset_type, 'Stock'), 'Open',
    '2026-07-16T19:00:00Z',
    'broker_position_snapshots',
    0,
    json_build_object(
        'source', 'broker_bootstrap',
        'symbol', b.symbol,
        'quantity', b.quantity,
        'open_price_local', b.open_price_local,
        'open_price_including_costs_local', b.open_price_including_costs_local,
        'currency', b.currency,
        'fx_rate_to_dkk', COALESCE(fx.rate_to_dkk, 1.0),
        'broker_updated_at', b.updated_at
    )::text,
    b.environment_id,
    b.account_uid
FROM broker_position_snapshots b
LEFT JOIN currency_fx_rates fx
    ON fx.currency_code = COALESCE(b.currency, 'DKK') AND fx.base_currency = 'DKK'
WHERE b.quantity > 0;

INSERT INTO position_lots (
    lot_id, batch_id, created_at, acquired_at, symbol, isin, instrument_name,
    quantity_original, currency, cost_basis_total_local, cost_basis_total_dkk,
    fx_rate_to_dkk, source_type, source_reference, raw_payload_json,
    environment_id, account_uid
)
SELECT
    'broker-bootstrap-20260716T190000Z:' || b.symbol,
    'broker-bootstrap-20260716T190000Z',
    '2026-07-16T19:00:00Z',
    COALESCE(b.execution_time_open, '2026-07-16T19:00:00Z'),
    b.symbol,
    b.isin,
    COALESCE(b.instrument_name, b.symbol),
    b.quantity,
    COALESCE(b.currency, 'DKK'),
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity,
    COALESCE(NULLIF(b.open_price_including_costs_local, 0), b.open_price_local) * b.quantity
        * COALESCE(fx.rate_to_dkk, 1.0),
    COALESCE(fx.rate_to_dkk, 1.0),
    'broker_bootstrap',
    'broker_position_snapshots@' || b.updated_at,
    json_build_object(
        'source', 'broker_bootstrap',
        'symbol', b.symbol,
        'quantity', b.quantity,
        'open_price_local', b.open_price_local,
        'open_price_including_costs_local', b.open_price_including_costs_local,
        'currency', b.currency,
        'fx_rate_to_dkk', COALESCE(fx.rate_to_dkk, 1.0)
    )::text,
    b.environment_id,
    b.account_uid
FROM broker_position_snapshots b
LEFT JOIN currency_fx_rates fx
    ON fx.currency_code = COALESCE(b.currency, 'DKK') AND fx.base_currency = 'DKK'
WHERE b.quantity > 0;

UPDATE position_snapshots
SET excluded = 1,
    exclusion_reason = 'Superseded by broker bootstrap 2026-07-16: stale 2026-05-18 import no longer matches the broker book.'
WHERE excluded = 0
  AND batch_id <> 'broker-bootstrap-20260716T190000Z';

COMMIT;
