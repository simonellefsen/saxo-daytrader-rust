//! Persisted protective-stop lifecycle records on `AppState`: coverage,
//! prechecks, preparations, manual SIM lifecycle tests and their
//! reconciliation, and adoption of broker-confirmed stops into
//! `execution_orders`.
//!
//! These methods read and write the local audit tables only. Nothing here
//! calls Saxo: placement, cancellation and the automatic sweep stay in
//! `saxo_order` and `protective_stops`.

use crate::state::{AppState, PROTECTIVE_STOP_STRATEGY_TYPE, json_text, text_value};
use crate::{
    config::{yaml_f64, yaml_string},
    db::{sql_escape, value_f64, value_i64},
    execution_state::{protective_stop_lifecycle_test_payloads, protective_stop_precheck_payloads},
};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use serde_json::{Value as JsonValue, json};
use std::collections::{HashMap, HashSet};
use tracing::info;

const DEFAULT_STOP_LOSS_ATR_MULTIPLE: f64 = 2.0;

/// Lifecycle statuses that can still stand for a live broker order. A broker
/// final fill settles any of them to `filled`, the status the manual reconcile
/// already records for a FinalFill. `filled` is deliberately absent from
/// `symbols_with_active_protective_stops`, the
/// `idx_protective_stop_lifecycle_tests_active_source` guard, and adoption.
const FILL_SETTLEABLE_LIFECYCLE_STATUSES: &str = "'placement_submitted', 'broker_working', \
     'broker_state_unknown', 'broker_amended', 'cancellation_submitted', 'reconciliation_pending'";

/// A protective stop level derived from stored daily indicators.
///
/// This is arithmetic on data the nightly indicator run already persisted: it
/// makes no Saxo call and places nothing. The price is deliberately *not*
/// tick-normalized here, because normalization needs Saxo instrument details;
/// the precheck and placement paths normalize before any order is built.
fn proposed_protective_stop(
    indicator: Option<&JsonValue>,
    quantity: f64,
    atr_multiple: f64,
) -> Option<JsonValue> {
    let indicator = indicator?;
    let close = value_f64(indicator, "close");
    let atr14 = value_f64(indicator, "atr14");
    if !close.is_finite() || close <= 0.0 || !atr14.is_finite() || atr14 <= 0.0 {
        return None;
    }
    if !atr_multiple.is_finite() || atr_multiple <= 0.0 {
        return None;
    }
    let distance = atr14 * atr_multiple;
    let stop_price = close - distance;
    // A stop at or below zero is not a protective level; report no proposal
    // rather than a nonsensical one.
    if !stop_price.is_finite() || stop_price <= 0.0 {
        return None;
    }
    Some(json!({
        "stop_price_local": stop_price,
        "quantity": quantity,
        "reference_close": close,
        "atr14": atr14,
        "atr_multiple": atr_multiple,
        "distance_local": distance,
        "distance_pct": (distance / close) * 100.0,
        "indicator_run_date": json_text(indicator, "run_date"),
        "tick_normalized": false,
        "basis": "close_minus_atr14_times_multiple",
        "safety": "computed_from_stored_indicators_no_saxo_call_and_places_nothing",
    }))
}

pub(crate) fn protective_stop_coverage_from_rows(
    position_rows: &[JsonValue],
    execution_order_rows: &[JsonValue],
    lifecycle_test_rows: &[JsonValue],
    indicator_rows: &[JsonValue],
    atr_multiple: f64,
) -> JsonValue {
    let mut indicators_by_symbol: HashMap<String, &JsonValue> = HashMap::new();
    for row in indicator_rows {
        let symbol = json_text(row, "symbol");
        if !symbol.trim().is_empty() {
            // Rows arrive newest-first, so the first entry per symbol wins.
            indicators_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_insert(row);
        }
    }
    let mut stops_by_symbol: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for order in execution_order_rows {
        if !json_text(order, "action").eq_ignore_ascii_case("SELL") {
            continue;
        }
        let order_type = json_text(order, "order_type").to_ascii_lowercase();
        if order_type != "stop" && order_type != "stoplimit" {
            continue;
        }
        let symbol = json_text(order, "symbol");
        if !symbol.trim().is_empty() {
            stops_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_default()
                .push(order);
        }
    }

    // Once a stop is adopted into `execution_orders` it is the same broker
    // order as its lifecycle-test row. Counting both would report two stops
    // covering one position and inflate the active stop count.
    let adopted_broker_order_ids = execution_order_rows
        .iter()
        .filter_map(|order| {
            let broker_order_id = json_text(order, "broker_order_id").trim().to_string();
            (!broker_order_id.is_empty()).then_some(broker_order_id)
        })
        .collect::<HashSet<String>>();

    // A lifecycle test is distinct from the normal execution queue. Count it
    // only after reconciliation has confirmed a broker-working SIM Stop with
    // an order identifier. Submitted, cancelled, failed, and ambiguous tests
    // deliberately provide no coverage evidence.
    let mut lifecycle_stops_by_symbol: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for test in lifecycle_test_rows {
        if !json_text(test, "environment").eq_ignore_ascii_case("sim")
            || json_text(test, "status") != "broker_working"
            || json_text(test, "broker_order_id").trim().is_empty()
            || adopted_broker_order_ids.contains(json_text(test, "broker_order_id").trim())
        {
            continue;
        }
        let symbol = json_text(test, "symbol");
        if !symbol.trim().is_empty() {
            lifecycle_stops_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_default()
                .push(test);
        }
    }

    let mut positions = Vec::new();
    let mut exceptions = Vec::new();
    let mut protected_count = 0usize;
    let mut partial_count = 0usize;
    let mut planned_count = 0usize;
    let mut unprotected_count = 0usize;
    let mut total_quantity = 0.0;
    let mut confirmed_covered_quantity = 0.0;

    for position in position_rows {
        let quantity = value_f64(position, "quantity");
        if !quantity.is_finite() || quantity <= 0.0 {
            continue;
        }
        let symbol = json_text(position, "symbol");
        if symbol.trim().is_empty() {
            continue;
        }
        total_quantity += quantity;
        let orders = stops_by_symbol
            .get(&symbol.trim().to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        let lifecycle_tests = lifecycle_stops_by_symbol
            .get(&symbol.trim().to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        let active_orders = orders
            .iter()
            .copied()
            .filter(|order| {
                matches!(
                    json_text(order, "status").as_str(),
                    "submitted_to_broker"
                        | "broker_working"
                        | "broker_amended"
                        | "broker_partially_filled"
                        | "broker_replace_requested"
                )
            })
            .collect::<Vec<_>>();
        let planned_orders = orders
            .iter()
            .copied()
            .filter(|order| {
                matches!(
                    json_text(order, "status").as_str(),
                    "pending_execution"
                        | "pending_approval"
                        | "submitting_to_broker"
                        | "planned_stop_loss"
                        | "planned_child_order"
                        | "waiting_for_market_open"
                        | "waiting_for_cash_settlement"
                        | "waiting_for_virtual_cash_budget"
                )
            })
            .collect::<Vec<_>>();
        let execution_order_covered_quantity = active_orders
            .iter()
            .map(|order| value_f64(order, "quantity").max(0.0))
            .sum::<f64>();
        let lifecycle_test_covered_quantity = lifecycle_tests
            .iter()
            .map(|test| value_f64(test, "quantity").max(0.0))
            .sum::<f64>();
        let confirmed_quantity =
            (execution_order_covered_quantity + lifecycle_test_covered_quantity).min(quantity);
        let active_stop_price = active_orders
            .iter()
            .filter_map(|order| {
                let price = value_f64(order, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            })
            .chain(lifecycle_tests.iter().filter_map(|test| {
                let price = value_f64(test, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            }))
            .max_by(f64::total_cmp);
        let planned_stop_price = planned_orders
            .iter()
            .filter_map(|order| {
                let price = value_f64(order, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            })
            .max_by(f64::total_cmp);
        let active_stop_count = active_orders.len() + lifecycle_tests.len();
        let protection_status = if active_stop_count > 0 && confirmed_quantity + 1e-6 >= quantity {
            protected_count += 1;
            "protected"
        } else if active_stop_count > 0 {
            partial_count += 1;
            "partial_protection"
        } else if !planned_orders.is_empty() {
            planned_count += 1;
            "planned"
        } else {
            unprotected_count += 1;
            "unprotected"
        };
        confirmed_covered_quantity += confirmed_quantity;
        let unprotected_quantity = (quantity - confirmed_quantity).max(0.0);
        // Only the uncovered share needs a stop, so the proposal is sized to it.
        let proposed_stop = (unprotected_quantity > 0.0)
            .then(|| {
                proposed_protective_stop(
                    indicators_by_symbol
                        .get(&symbol.trim().to_ascii_uppercase())
                        .copied(),
                    unprotected_quantity,
                    atr_multiple,
                )
            })
            .flatten();
        if protection_status != "protected" {
            let (kind, reason) = match protection_status {
                "partial_protection" => (
                    "partial_protective_stop_coverage",
                    "Only part of the persisted broker position has broker-confirmed protective-stop coverage.",
                ),
                "planned" => (
                    "planned_stop_not_broker_confirmed",
                    "A local stop plan exists, but Saxo has not confirmed a working protective stop.",
                ),
                _ => (
                    "unprotected_broker_position",
                    "No broker-confirmed protective stop is recorded for this persisted broker position.",
                ),
            };
            exceptions.push(json!({
                "kind": kind,
                "severity": "warning",
                "symbol": symbol,
                "broker_quantity": quantity,
                "confirmed_covered_quantity": confirmed_quantity,
                "unprotected_quantity": unprotected_quantity,
                "reason": reason,
                "proposed_stop": proposed_stop.clone(),
                "operator_action": "Review the persisted broker position and stop evidence. The SIM lifecycle test is manual and does not place, change, or cancel any order without its separate confirmation.",
            }));
        }
        positions.push(json!({
            "symbol": symbol,
            "quantity": quantity,
            "currency": json_text(position, "currency"),
            "snapshot_updated_at": json_text(position, "updated_at"),
            "protection_status": protection_status,
            "confirmed_covered_quantity": confirmed_quantity,
            "coverage_ratio": (confirmed_quantity / quantity).clamp(0.0, 1.0),
            "active_stop_count": active_stop_count,
            "execution_order_stop_count": active_orders.len(),
            "lifecycle_test_stop_count": lifecycle_tests.len(),
            "coverage_evidence": {
                "execution_orders": active_orders.len(),
                "manual_sim_lifecycle_tests": lifecycle_tests.len(),
            },
            "planned_stop_count": planned_orders.len(),
            "active_stop_price_local": active_stop_price,
            "planned_stop_price_local": planned_stop_price,
            "proposed_stop": proposed_stop,
        }));
    }

    let status = if positions.is_empty() {
        "no_positive_broker_positions_recorded"
    } else if unprotected_count > 0 || partial_count > 0 {
        "attention_required"
    } else if planned_count > 0 {
        "planned_only"
    } else {
        "covered"
    };
    json!({
        "status": status,
        "summary": {
            "position_count": positions.len(),
            "protected_count": protected_count,
            "partial_count": partial_count,
            "planned_count": planned_count,
            "unprotected_count": unprotected_count,
            "total_quantity": total_quantity,
            "confirmed_covered_quantity": confirmed_covered_quantity,
            "exception_count": exceptions.len(),
        },
        "positions": positions,
        "exceptions": exceptions,
        "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
        "interpretation": "Coverage is inferred from the latest persisted broker-position snapshot, local SELL Stop or StopLimit records, and reconciled manual SIM lifecycle-test records. Only broker-confirmed stop states and lifecycle tests reconciled as broker-working count as protection; queued, unresolved, stale, cancelled, or failed records do not. A broker-hosted stop can still fill away from its stop price during a market gap.",
    })
}

impl AppState {
    pub(crate) async fn protective_stop_coverage(&self) -> Result<JsonValue> {
        let positions = self
            .select_json(
                "SELECT symbol, updated_at, quantity, currency
                 FROM broker_position_snapshots
                 WHERE quantity > 0
                 ORDER BY symbol ASC",
            )
            .await?;
        let orders = self
            .select_json(
                "SELECT symbol, action, order_type, status, quantity, stop_price_local, broker_order_id
                 FROM execution_orders
                 WHERE action = 'SELL'
                 ORDER BY created_at DESC, id DESC",
            )
            .await?;
        let prechecks = self
            .select_json(
                "SELECT id, created_at, environment, symbol, quantity, stop_price_local, status, result_json
                 FROM protective_stop_prechecks
                 ORDER BY created_at DESC, id DESC
                 LIMIT 10",
            )
            .await?;
        let lifecycle_tests = self
            .select_json(
                "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                        stop_price_local, status, broker_order_id, external_reference, request_id,
                        placement_result_json, cancellation_result_json, reconciliation_json
                 FROM protective_stop_lifecycle_tests
                 ORDER BY created_at DESC, id DESC
                 LIMIT 10",
            )
            .await?;
        let active_lifecycle_tests = self
            .select_json(
                "SELECT id, updated_at, environment, symbol, quantity, stop_price_local, status, broker_order_id
                 FROM protective_stop_lifecycle_tests
                 WHERE environment = 'sim'
                   AND status = 'broker_working'
                   AND broker_order_id IS NOT NULL
                   AND broker_order_id <> ''
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 100",
            )
            .await?;
        // Latest stored close and ATR14 per symbol, so an unprotected position
        // can be shown with the concrete stop level it should carry. Bounded and
        // newest-first; `proposed_protective_stop` takes the first row per
        // symbol. No Saxo call is made and nothing is placed.
        let indicators = self
            .select_json(
                "SELECT symbol, run_date, close, atr14
                 FROM daily_indicator_signals
                 WHERE close IS NOT NULL AND atr14 IS NOT NULL
                 ORDER BY run_date DESC, id DESC
                 LIMIT 600",
            )
            .await
            .unwrap_or_default();
        let atr_multiple = yaml_f64(
            &self.config,
            &["strategy", "ladder", "stop_loss_atr_multiple"],
        )
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(DEFAULT_STOP_LOSS_ATR_MULTIPLE);
        let mut coverage = protective_stop_coverage_from_rows(
            &positions,
            &orders,
            &active_lifecycle_tests,
            &indicators,
            atr_multiple,
        );
        if let Some(object) = coverage.as_object_mut() {
            object.insert(
                "recent_prechecks".to_string(),
                serde_json::to_value(protective_stop_precheck_payloads(prechecks))?,
            );
            object.insert(
                "recent_lifecycle_tests".to_string(),
                serde_json::to_value(protective_stop_lifecycle_test_payloads(lifecycle_tests))?,
            );
        }
        Ok(coverage)
    }

    pub async fn record_protective_stop_precheck(
        &self,
        symbol: &str,
        quantity: f64,
        stop_price_local: f64,
        status: &str,
        result: &JsonValue,
    ) -> Result<i64> {
        let environment = yaml_string(&self.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string())
            .to_ascii_lowercase();
        sqlx::query(
            "INSERT INTO protective_stop_prechecks (
                created_at, environment, symbol, quantity, stop_price_local, status, result_json
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(environment)
        .bind(symbol.trim())
        .bind(quantity)
        .bind(stop_price_local)
        .bind(status)
        .bind(result.to_string())
        .execute(&self.pool)
        .await
        .context("recording sanitized protective-stop precheck")?;
        // AnyPool does not expose last-insert-id portably, so read the row back
        // by its exact identity instead of trusting a driver-specific handle.
        let id = self
            .first_json(&format!(
                "SELECT id FROM protective_stop_prechecks
                 WHERE symbol = '{}' AND status = '{}'
                 ORDER BY id DESC LIMIT 1",
                sql_escape(symbol.trim()),
                sql_escape(status)
            ))
            .await?
            .map(|row| value_i64(&row, "id"))
            .unwrap_or_default();
        Ok(id)
    }

    /// Lifecycle tests left in `placement_preparing` with no broker order id.
    ///
    /// Axum drops a handler future when the client disconnects, so a
    /// double-clicked placement can commit the prepared row and never reach the
    /// broker call. The orphan then counts as active forever and blocks every
    /// retry for that precheck. Observed 2026-07-25 on lifecycle test 1.
    ///
    /// These are *not* safe to expire on a timer: the future could equally have
    /// been dropped after a successful placement. The caller must reconcile each
    /// one against Saxo and only abandon those the broker does not know about.
    pub async fn stale_protective_stop_preparations(
        &self,
        older_than_seconds: i64,
    ) -> Result<Vec<JsonValue>> {
        let cutoff = (Utc::now() - Duration::seconds(older_than_seconds.max(0)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.select_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id
             FROM protective_stop_lifecycle_tests
             WHERE status = 'placement_preparing'
               AND (broker_order_id IS NULL OR broker_order_id = '')
               AND updated_at < '{}'
             ORDER BY id ASC
             LIMIT 20",
            sql_escape(&cutoff)
        ))
        .await
    }

    /// Placed stops still awaiting broker confirmation.
    ///
    /// `placement_submitted` is not coverage: the audit counts a stop only once
    /// Saxo reports it working. Left unconfirmed, the position keeps appearing
    /// as an exception and a later batch retries it -- which Saxo rejects with
    /// `SellOrdersAlreadyExistForOwnedContracts`, because the stop it does not
    /// know about is already resting. Observed 2026-07-25 across nine symbols.
    pub async fn unconfirmed_protective_stop_placements(
        &self,
        older_than_seconds: i64,
    ) -> Result<Vec<JsonValue>> {
        let cutoff = (Utc::now() - Duration::seconds(older_than_seconds.max(0)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.select_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id
             FROM protective_stop_lifecycle_tests
             WHERE status = 'placement_submitted'
               AND broker_order_id IS NOT NULL
               AND broker_order_id <> ''
               AND updated_at < '{}'
             ORDER BY id ASC
             LIMIT 25",
            sql_escape(&cutoff)
        ))
        .await
    }

    /// Symbols with a protective-stop lifecycle test that is not in a terminal
    /// state. Saxo permits one resting sell per owned holding, so a batch must
    /// not attempt a second one even while local coverage still lags.
    pub async fn symbols_with_active_protective_stops(&self) -> Result<Vec<String>> {
        Ok(self
            .select_json(
                "SELECT DISTINCT symbol FROM protective_stop_lifecycle_tests
                 WHERE status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                                  'broker_state_unknown', 'broker_amended', 'reconciliation_pending')",
            )
            .await?
            .iter()
            .map(|row| text_value(row, "symbol").trim().to_ascii_uppercase())
            .filter(|symbol| !symbol.is_empty())
            .collect())
    }

    /// Marks the lifecycle-test row behind a protective stop as released once
    /// the stop has been cancelled at Saxo to make room for a decided sell.
    /// Keeping the two tables agreed matters: `symbols_with_active_protective_stops`
    /// reads this table, and a stale `broker_working` row there would stop the
    /// position from ever being re-protected after the sell.
    pub async fn release_protective_stop_lifecycle_test(
        &self,
        broker_order_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = 'broker_cancelled', cancellation_result_json = $2
             WHERE broker_order_id = $3 AND status <> 'broker_cancelled'",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(
            json!({
                "cancelled_by": "automatic_release_before_a_decided_sell",
                "verified": "broker_confirmed_not_working_before_the_sell_was_built",
                "safety": "cancellation_is_scoped_to_the_symbol_being_sold"
            })
            .to_string(),
        )
        .bind(broker_order_id)
        .execute(&self.pool)
        .await
        .context("releasing protective-stop lifecycle test after cancellation")?;
        Ok(())
    }

    /// Settles the lifecycle-test row behind a protective stop that Saxo
    /// reported finally filled, once broker sync has marked its execution order
    /// `executed`.
    ///
    /// The fill counterpart of `release_protective_stop_lifecycle_test`. Until
    /// 2026-10-23 nothing wrote it, so every stop that ever filled stayed
    /// `broker_working` here -- 99 rows live -- and a symbol stopped out once
    /// and later re-bought was skipped by the operator batch forever, because
    /// `symbols_with_active_protective_stops` reads this table.
    ///
    /// Bookkeeping only, with no broker call. Keyed on `broker_order_id`, so a
    /// fill that is not a protective stop matches nothing. Each update is
    /// conditional on the status it read, so a concurrent writer is never
    /// overwritten, and a replay matches no row. Returns the rows settled.
    pub async fn record_protective_stop_lifecycle_fill(
        &self,
        broker_order_id: &str,
        execution_order_id: i64,
        source: &str,
    ) -> Result<u64> {
        let broker_order_id = broker_order_id.trim();
        if broker_order_id.is_empty() {
            return Ok(0);
        }
        let rows = self
            .select_json(&format!(
                "SELECT id, status, reconciliation_json
                 FROM protective_stop_lifecycle_tests
                 WHERE broker_order_id = '{}'
                   AND status IN ({FILL_SETTLEABLE_LIFECYCLE_STATUSES})
                 ORDER BY id ASC",
                sql_escape(broker_order_id)
            ))
            .await?;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut settled = 0;
        for row in &rows {
            let previous_status = text_value(row, "status");
            let reconciliation = json!({
                "status": "filled",
                "source": source,
                "settled_at": now,
                "broker_order_id": broker_order_id,
                "execution_order_id": execution_order_id,
                "previous_status": previous_status,
                "previous_reconciliation": row.get("reconciliation_json").cloned().unwrap_or(JsonValue::Null),
                "safety": "local_bookkeeping_after_broker_sync_recorded_the_final_fill_no_broker_call"
            });
            let updated = sqlx::query(
                "UPDATE protective_stop_lifecycle_tests
                 SET updated_at = $1, status = 'filled', reconciliation_json = $2
                 WHERE id = $3 AND status = $4",
            )
            .bind(&now)
            .bind(reconciliation.to_string())
            .bind(value_i64(row, "id"))
            .bind(&previous_status)
            .execute(&self.pool)
            .await
            .context("settling filled protective-stop lifecycle test")?;
            settled += updated.rows_affected();
        }
        Ok(settled)
    }

    /// Startup backfill for `record_protective_stop_lifecycle_fill`: settles
    /// every lifecycle row whose execution order broker sync already marked
    /// `executed`. Idempotent, so every pod can run it on every start.
    ///
    /// A `placement_preparing` row with no broker order id is deliberately left
    /// alone. Only a Saxo lookup can prove the broker never saw it, and
    /// `abandon_protective_stop_preparation` records that such a lookup found
    /// nothing; this backfill makes no broker call, so it cannot make that
    /// claim. `resolve_stale_protective_stop_preparations` does the lookup.
    pub(crate) async fn backfill_filled_protective_stop_lifecycle_tests(&self) -> Result<u64> {
        let rows = self
            .select_json(&format!(
                "SELECT t.broker_order_id, o.id AS execution_order_id
                 FROM protective_stop_lifecycle_tests t
                 JOIN execution_orders o ON o.broker_order_id = t.broker_order_id
                 WHERE t.status IN ({FILL_SETTLEABLE_LIFECYCLE_STATUSES})
                   AND t.broker_order_id IS NOT NULL
                   AND t.broker_order_id <> ''
                   AND o.status = 'executed'
                 ORDER BY t.id ASC"
            ))
            .await
            .context("loading filled protective stops whose lifecycle tests are unsettled")?;
        let mut settled = 0;
        for row in &rows {
            settled += self
                .record_protective_stop_lifecycle_fill(
                    &text_value(row, "broker_order_id"),
                    value_i64(row, "execution_order_id"),
                    "startup_backfill_of_fills_recorded_before_lifecycle_settlement",
                )
                .await?;
        }
        if settled > 0 {
            info!(
                rows = settled,
                "settled lifecycle tests of protective stops that had already filled"
            );
        }
        Ok(settled)
    }

    /// Adopts every broker-confirmed protective stop into `execution_orders`.
    ///
    /// This is the load-bearing half of U1 slice 3. `sync_saxo_broker_orders`
    /// reads `execution_orders` and nothing else, so a stop living only in
    /// `protective_stop_lifecycle_tests` can fill at Saxo without producing a
    /// ledger row, a position update, or any Trading Manager awareness. Giving
    /// each stop an execution-order row inherits broker sync, fill
    /// reconciliation, the trade ledger, and execution notifications with no
    /// new plumbing.
    ///
    /// Adoption is idempotent on `broker_order_id` and creates no broker
    /// traffic: it records an order Saxo already holds. The lifecycle-test row
    /// stays as the placement audit trail.
    pub async fn adopt_protective_stops_into_execution_orders(&self) -> Result<Vec<JsonValue>> {
        let candidates = self
            .select_json(
                "SELECT t.id, t.created_at, t.symbol, t.quantity, t.stop_price_local,
                        t.broker_order_id, t.external_reference, t.request_id
                 FROM protective_stop_lifecycle_tests t
                 WHERE t.status = 'broker_working'
                   AND t.broker_order_id IS NOT NULL
                   AND t.broker_order_id <> ''
                   AND NOT EXISTS (
                         SELECT 1 FROM execution_orders o
                         WHERE o.broker_order_id = t.broker_order_id
                       )
                 ORDER BY t.id ASC
                 LIMIT 50",
            )
            .await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mode = yaml_string(&self.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string());
        let adapter = yaml_string(&self.config, &["execution", "adapter"])
            .unwrap_or_else(|| "saxo".to_string());
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut adopted = Vec::new();
        for candidate in &candidates {
            let test_id = value_i64(candidate, "id");
            let symbol = text_value(candidate, "symbol").trim().to_string();
            let broker_order_id = text_value(candidate, "broker_order_id").trim().to_string();
            let quantity = value_f64(candidate, "quantity");
            let stop_price_local = value_f64(candidate, "stop_price_local");
            if symbol.is_empty() || broker_order_id.is_empty() || quantity <= 0.0 {
                continue;
            }
            let request_json = json!({
                "source": "protective_stop_adoption",
                "adopted_at": now,
                "lifecycle_test_id": test_id,
                "external_reference": text_value(candidate, "external_reference"),
                "request_id": text_value(candidate, "request_id"),
                "placed_at": text_value(candidate, "created_at"),
                "note": "Broker-confirmed protective stop adopted so broker sync, fill reconciliation, and the trade ledger cover it. No broker call was made by the adoption itself."
            });
            // `NOT EXISTS` above is checked outside a transaction, so a
            // concurrent scheduler pod could reach here with the same row. The
            // unique strategy key is the second guard: the insert is skipped
            // rather than duplicated.
            let strategy_key = format!("protective_stop:{test_id}");
            if self
                .select_json(&format!(
                    "SELECT id FROM execution_orders WHERE strategy_key = '{}' LIMIT 1",
                    sql_escape(&strategy_key)
                ))
                .await?
                .first()
                .is_some()
            {
                continue;
            }
            let inserted = sqlx::query(
                "INSERT INTO execution_orders (
                    created_at, symbol, action, order_type, mode, status, adapter,
                    quantity, stop_price_local, approval_required, approved_at,
                    strategy_type, strategy_key, strategy_role, broker_order_id, request_json
                 ) VALUES ($1, $2, 'SELL', 'stop', $3, 'broker_working', $4,
                           $5, $6, 0, $7, $8, $9, 'protective_stop', $10, $11)",
            )
            .bind(&now)
            .bind(&symbol)
            .bind(&mode)
            .bind(&adapter)
            .bind(quantity)
            .bind(stop_price_local)
            .bind(&now)
            .bind(PROTECTIVE_STOP_STRATEGY_TYPE)
            .bind(&strategy_key)
            .bind(&broker_order_id)
            .bind(serde_json::to_string(&request_json)?)
            .execute(&self.pool)
            .await
            .context("adopting protective stop into execution orders")?;
            if inserted.rows_affected() == 1 {
                adopted.push(json!({
                    "lifecycle_test_id": test_id,
                    "symbol": symbol,
                    "quantity": quantity,
                    "stop_price_local": stop_price_local,
                    "broker_order_id": broker_order_id,
                    "strategy_key": strategy_key
                }));
            }
        }
        Ok(adopted)
    }

    /// Marks a prepared lifecycle test abandoned after a reconcile confirmed the
    /// broker never saw it. `placement_abandoned` is deliberately absent from
    /// the active-status list, so the precheck becomes reusable.
    pub async fn abandon_protective_stop_preparation(&self, test_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = 'placement_abandoned', placement_result_json = $2
             WHERE id = $3 AND status = 'placement_preparing'
               AND (broker_order_id IS NULL OR broker_order_id = '')",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(
            json!({
                "placement": "not_sent",
                "abandoned_reason": "prepared_request_did_not_reach_the_broker",
                "verified_by": "saxo_open_order_and_audit_activity_lookup_found_nothing",
                "safety": "verified_absent_at_broker_before_abandoning_never_expired_on_a_timer"
            })
            .to_string(),
        )
        .bind(test_id)
        .execute(&self.pool)
        .await
        .context("abandoning unreached protective-stop preparation")?;
        Ok(())
    }

    pub async fn prepare_protective_stop_lifecycle_test(
        &self,
        source_precheck_id: i64,
    ) -> Result<JsonValue> {
        if source_precheck_id <= 0 {
            bail!("A successful SIM protective-stop precheck is required");
        }
        let source = self
            .first_json(&format!(
                "SELECT id, environment, symbol, quantity, stop_price_local, status
                 FROM protective_stop_prechecks WHERE id = {} LIMIT 1",
                source_precheck_id
            ))
            .await?
            .ok_or_else(|| {
                anyhow!("Protective-stop precheck {source_precheck_id} was not found")
            })?;
        if json_text(&source, "status") != "precheck_ok"
            || !json_text(&source, "environment").eq_ignore_ascii_case("sim")
        {
            bail!("Protective-stop lifecycle tests require a successful SIM precheck");
        }
        let active = self
            .first_json(&format!(
                "SELECT id FROM protective_stop_lifecycle_tests
                 WHERE source_precheck_id = {}
                   AND status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                                  'broker_state_unknown', 'cancellation_submitted', 'reconciliation_pending')
                 ORDER BY id DESC LIMIT 1",
                source_precheck_id
            ))
            .await?;
        if let Some(active) = active {
            bail!(
                "Protective-stop precheck {source_precheck_id} already has active lifecycle test {}",
                value_i64(&active, "id")
            );
        }
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let nonce = Utc::now().timestamp_micros();
        let request_id = format!("saxo-stop-test-{source_precheck_id}-{nonce}");
        let external_reference = format!("stop-test:{source_precheck_id}:{nonce}");
        let result = json!({
            "safety": "manual_sim_only_single_position_precheck_before_place_no_scheduler_or_hermes",
            "source_precheck_id": source_precheck_id,
            "placement": "not_sent"
        });
        sqlx::query(
            "INSERT INTO protective_stop_lifecycle_tests (
                created_at, updated_at, source_precheck_id, environment, symbol, quantity, stop_price_local,
                status, broker_order_id, external_reference, request_id, placement_result_json,
                cancellation_result_json, reconciliation_json
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10, $11, $12, $13)",
        )
        .bind(&now)
        .bind(&now)
        .bind(source_precheck_id)
        .bind("sim")
        .bind(json_text(&source, "symbol"))
        .bind(value_f64(&source, "quantity"))
        .bind(value_f64(&source, "stop_price_local"))
        .bind("placement_preparing")
        .bind(&external_reference)
        .bind(&request_id)
        .bind(result.to_string())
        .bind(json!({"status": "not_requested"}).to_string())
        .bind(json!({"status": "not_requested"}).to_string())
        .execute(&self.pool)
        .await
        .context("creating SIM protective-stop lifecycle test")?;
        self.protective_stop_lifecycle_test_by_request_id(&request_id)
            .await?
            .ok_or_else(|| anyhow!("Could not reload prepared protective-stop lifecycle test"))
    }

    pub async fn protective_stop_lifecycle_test(&self, test_id: i64) -> Result<Option<JsonValue>> {
        if test_id <= 0 {
            return Ok(None);
        }
        self.first_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id,
                    placement_result_json, cancellation_result_json, reconciliation_json
             FROM protective_stop_lifecycle_tests WHERE id = {} LIMIT 1",
            test_id
        ))
        .await
    }

    async fn protective_stop_lifecycle_test_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<JsonValue>> {
        self.first_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id,
                    placement_result_json, cancellation_result_json, reconciliation_json
             FROM protective_stop_lifecycle_tests
             WHERE request_id = '{}' LIMIT 1",
            sql_escape(request_id)
        ))
        .await
    }

    pub async fn record_protective_stop_lifecycle_placement(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(
            test_id,
            status,
            broker_order_id,
            Some(result),
            None,
            None,
        )
        .await
    }

    pub async fn record_protective_stop_lifecycle_cancellation(
        &self,
        test_id: i64,
        status: &str,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(test_id, status, None, None, Some(result), None)
            .await
    }

    pub async fn record_protective_stop_lifecycle_reconciliation(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(
            test_id,
            status,
            broker_order_id,
            None,
            None,
            Some(result),
        )
        .await
    }

    async fn update_protective_stop_lifecycle_test(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        placement: Option<&JsonValue>,
        cancellation: Option<&JsonValue>,
        reconciliation: Option<&JsonValue>,
    ) -> Result<()> {
        let current = self
            .protective_stop_lifecycle_test(test_id)
            .await?
            .ok_or_else(|| anyhow!("Protective-stop lifecycle test {test_id} was not found"))?;
        let broker_order_id = broker_order_id.map(ToString::to_string).or_else(|| {
            let value = json_text(&current, "broker_order_id");
            (!value.is_empty()).then_some(value)
        });
        let placement_text = placement.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "placement_result_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let cancellation_text = cancellation.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "cancellation_result_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let reconciliation_text = reconciliation.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "reconciliation_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let updated = sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = $2, broker_order_id = $3, placement_result_json = $4,
                 cancellation_result_json = $5, reconciliation_json = $6
             WHERE id = $7",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(status)
        .bind(broker_order_id)
        .bind(placement_text)
        .bind(cancellation_text)
        .bind(reconciliation_text)
        .bind(test_id)
        .execute(&self.pool)
        .await
        .context("recording SIM protective-stop lifecycle test state")?;
        if updated.rows_affected() != 1 {
            bail!("Protective-stop lifecycle test {test_id} changed while updating");
        }
        Ok(())
    }
}
