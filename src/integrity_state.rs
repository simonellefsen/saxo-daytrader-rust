//! Overview integrity on `AppState`: the cross-checks between the local
//! ledger, position snapshots and the broker's own figures, the realised and
//! unrealised P/L reconciliations, and the operator acknowledgements of
//! known issues.
//!
//! Read-only evidence apart from the acknowledgement it saves. It flags a
//! mismatch; it never corrects one, gates an order, or reaches Saxo.

use crate::state::{
    ADOPTED_ORDERS_WITHOUT_LEDGER_SQL, AppState, BlankStringExt,
    INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK, RealisedSellBaseline, average_or_null,
    canonical_symbol_key, enrich_execution_order_lifecycle, fraction_or_null, json_text,
    text_value,
};
use crate::{
    config::{yaml_bool, yaml_i64},
    db::{sql_escape, value_f64, value_i64},
};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tracing::warn;

const INTEGRITY_MONEY_ABS_TOLERANCE_DKK: f64 = 50.0;

const INTEGRITY_MONEY_REL_TOLERANCE: f64 = 0.002;

const INTEGRITY_BROKER_CASH_ABS_TOLERANCE_DKK: f64 = 500.0;

const INTEGRITY_BROKER_CASH_REL_TOLERANCE: f64 = 0.05;

const INTEGRITY_BROKER_EXPOSURE_ABS_TOLERANCE_DKK: f64 = 1_000.0;

const INTEGRITY_BROKER_EXPOSURE_REL_TOLERANCE: f64 = 0.10;

const INTEGRITY_BROKER_QUANTITY_ABS_TOLERANCE: f64 = 1e-6;

const PORTFOLIO_POSITION_SNAPSHOT_INTEGRITY_LIMIT: i64 = 30;

const PORTFOLIO_POSITION_SNAPSHOT_MONEY_ABS_TOLERANCE_DKK: f64 = 0.05;

const PORTFOLIO_POSITION_SNAPSHOT_MONEY_REL_TOLERANCE: f64 = 1e-8;

const PERFORMANCE_EXPOSURE_ATTRIBUTION_LIMIT: usize = 20;

const PERFORMANCE_REALISED_SELL_OUTCOME_LIMIT: i64 = 250;

const PERFORMANCE_REALISED_SELL_OUTCOME_RECENT_LIMIT: usize = 12;

const PERFORMANCE_REALISED_SELL_OUTCOME_ATTRIBUTION_LIMIT: usize = 12;

const PERFORMANCE_REALISED_SELL_OUTCOME_MIN_SAMPLE_SIZE: usize = 20;

pub(crate) fn money_mismatch_exceeds_tolerance(
    left: f64,
    right: f64,
    abs_tolerance: f64,
    rel_tolerance: f64,
) -> bool {
    if !left.is_finite() || !right.is_finite() {
        return true;
    }
    let diff = (left - right).abs();
    let scale = left.abs().max(right.abs()).max(1.0);
    diff > abs_tolerance && diff / scale > rel_tolerance
}

pub(crate) fn portfolio_position_snapshot_integrity_from_rows(rows: &[JsonValue]) -> JsonValue {
    let mut structural_mismatches = Vec::new();
    let mut broker_unrealised_differences = Vec::new();
    for row in rows {
        let snapshot_id = value_i64(row, "snapshot_id");
        let recorded_at = json_text(row, "recorded_at");
        let aggregate_position_count = value_i64(row, "aggregate_position_count");
        let detail_position_count = value_i64(row, "detail_position_count");
        let aggregate_market_value_dkk = value_f64(row, "aggregate_market_value_dkk");
        let detail_market_value_dkk = value_f64(row, "detail_market_value_dkk");
        let aggregate_cost_basis_dkk = value_f64(row, "aggregate_cost_basis_dkk");
        let detail_cost_basis_dkk = value_f64(row, "detail_cost_basis_dkk");
        let aggregate_unrealised_pnl_dkk = value_f64(row, "aggregate_unrealised_pnl_dkk");
        let detail_unrealised_pnl_dkk = value_f64(row, "detail_unrealised_pnl_dkk");
        let position_count_mismatch = aggregate_position_count != detail_position_count;
        let market_value_mismatch = money_mismatch_exceeds_tolerance(
            aggregate_market_value_dkk,
            detail_market_value_dkk,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_ABS_TOLERANCE_DKK,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_REL_TOLERANCE,
        );
        let cost_basis_mismatch = money_mismatch_exceeds_tolerance(
            aggregate_cost_basis_dkk,
            detail_cost_basis_dkk,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_ABS_TOLERANCE_DKK,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_REL_TOLERANCE,
        );
        if position_count_mismatch || market_value_mismatch || cost_basis_mismatch {
            structural_mismatches.push(json!({
                "snapshot_id": snapshot_id,
                "recorded_at": recorded_at,
                "position_count": {
                    "aggregate": aggregate_position_count,
                    "detail": detail_position_count,
                    "mismatch": position_count_mismatch,
                },
                "market_value_difference_dkk": aggregate_market_value_dkk - detail_market_value_dkk,
                "market_value_mismatch": market_value_mismatch,
                "cost_basis_difference_dkk": aggregate_cost_basis_dkk - detail_cost_basis_dkk,
                "cost_basis_mismatch": cost_basis_mismatch,
            }));
        }
        if money_mismatch_exceeds_tolerance(
            aggregate_unrealised_pnl_dkk,
            detail_unrealised_pnl_dkk,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_ABS_TOLERANCE_DKK,
            PORTFOLIO_POSITION_SNAPSHOT_MONEY_REL_TOLERANCE,
        ) {
            broker_unrealised_differences.push(json!({
                "snapshot_id": snapshot_id,
                "recorded_at": recorded_at,
                "difference_dkk": aggregate_unrealised_pnl_dkk - detail_unrealised_pnl_dkk,
                "aggregate_unrealised_pnl_dkk": aggregate_unrealised_pnl_dkk,
                "recomputed_unrealised_pnl_dkk": detail_unrealised_pnl_dkk,
                "interpretation": "aggregate_uses_broker_derived_unrealised_pnl",
            }));
        }
    }
    let status = if rows.is_empty() {
        "collecting"
    } else if !structural_mismatches.is_empty() {
        "attention_required"
    } else if !broker_unrealised_differences.is_empty() {
        "broker_derived_unrealised_difference"
    } else {
        "aligned"
    };
    json!({
        "status": status,
        "checked_snapshot_count": rows.len(),
        "structural_mismatch_count": structural_mismatches.len(),
        "structural_mismatches": structural_mismatches,
        "broker_derived_unrealised_difference_count": broker_unrealised_differences.len(),
        "broker_derived_unrealised_differences": broker_unrealised_differences,
        "tolerance": {
            "absolute_dkk": PORTFOLIO_POSITION_SNAPSHOT_MONEY_ABS_TOLERANCE_DKK,
            "relative": PORTFOLIO_POSITION_SNAPSHOT_MONEY_REL_TOLERANCE,
        },
        "safety": "local_aggregate_and_position_snapshot_comparison_no_provider_hermes_gate_or_order_authority",
    })
}

fn latest_snapshot_timestamp(rows: &[JsonValue], field: &str) -> Option<String> {
    rows.iter()
        .filter_map(|row| row.get(field).and_then(JsonValue::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .max_by_key(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|timestamp| timestamp.timestamp())
                .unwrap_or(i64::MIN)
        })
}

/// Read-only source comparison for Performance. A broker exposure snapshot is
/// not asserted to be a realtime quote, so the UI renders its own timestamp
/// and FX conversion basis instead of calling it current.
pub(crate) fn unrealised_pnl_reconciliation(
    aggregate: &JsonValue,
    latest_history: &JsonValue,
    broker_exposure_snapshot: &JsonValue,
) -> JsonValue {
    let dashboard_pnl = aggregate
        .get("total_unrealised_pnl_dkk")
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite());
    let history_pnl = latest_history
        .get("total_unrealised_pnl_dkk")
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite());
    let broker_pnl = broker_exposure_snapshot
        .get("unrealised_pnl_dkk")
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite());
    let broker_difference_dkk = dashboard_pnl
        .zip(broker_pnl)
        .map(|(dashboard, broker)| dashboard - broker);
    let broker_status = match (dashboard_pnl, broker_pnl) {
        (Some(dashboard), Some(broker))
            if money_mismatch_exceeds_tolerance(
                dashboard,
                broker,
                INTEGRITY_BROKER_EXPOSURE_ABS_TOLERANCE_DKK,
                INTEGRITY_BROKER_EXPOSURE_REL_TOLERANCE,
            ) =>
        {
            "drift"
        }
        (Some(_), Some(_)) => "aligned",
        _ => "unavailable",
    };
    json!({
        "scope": "read_only_unrealised_pnl_sources",
        "dashboard": {
            "status": if dashboard_pnl.is_some() { "available" } else { "unavailable" },
            "unrealised_pnl_dkk": dashboard_pnl,
            "source": text_value(aggregate, "source"),
            "snapshot_type": "runtime_current",
        },
        "latest_history": {
            "status": if history_pnl.is_some() { "available" } else { "unavailable" },
            "unrealised_pnl_dkk": history_pnl,
            "source": text_value(latest_history, "source"),
            "snapshot_type": text_value(latest_history, "snapshot_type"),
            "recorded_at": latest_history.get("recorded_at").cloned().unwrap_or(JsonValue::Null),
            "difference_from_dashboard_dkk": dashboard_pnl.zip(history_pnl).map(|(dashboard, history)| dashboard - history),
        },
        "broker_exposure": {
            "status": broker_status,
            "unrealised_pnl_dkk": broker_pnl,
            "difference_from_dashboard_dkk": broker_difference_dkk,
            "account_currency": broker_exposure_snapshot.get("account_currency").cloned().unwrap_or(JsonValue::Null),
            "fx_basis": broker_exposure_snapshot.get("fx_basis").cloned().unwrap_or(JsonValue::Null),
            "instrument_fx_rates_to_dkk": broker_exposure_snapshot.get("instrument_fx_rates_to_dkk").cloned().unwrap_or(JsonValue::Null),
            "exposure_count": broker_exposure_snapshot.get("exposure_count").cloned().unwrap_or(JsonValue::Null),
            "updated_at": broker_exposure_snapshot.get("updated_at").cloned().unwrap_or(JsonValue::Null),
        },
    })
}

/// Read-only current-exposure attribution. Saxo reports `ProfitLossOnTrade`
/// in the instrument currency, so every row is converted using that
/// instrument's current FX rate rather than the broker account currency.
pub(crate) fn broker_exposure_pnl_attribution(
    broker_exposures: &[JsonValue],
    broker_exposure_snapshot: &JsonValue,
    instrument_fx_rates_to_dkk: &BTreeMap<String, f64>,
) -> JsonValue {
    if broker_exposures.is_empty() {
        return json!({
            "status": "unavailable",
            "scope": "read_only_stored_saxo_exposure_unrealised_pnl",
            "rows": [],
            "currencies": [],
            "exposure_count": 0,
        });
    }

    let account_currency = text_value(broker_exposure_snapshot, "account_currency")
        .if_empty_then(|| Some("DKK".to_string()))
        .unwrap_or_else(|| "DKK".to_string());
    let mut rows = broker_exposures
        .iter()
        .filter_map(|row| {
            let symbol = text_value(row, "symbol");
            if symbol.is_empty() {
                return None;
            }
            let instrument_currency = text_value(row, "currency")
                .if_empty_then(|| Some("DKK".to_string()))
                .unwrap_or_else(|| "DKK".to_string());
            let profit_loss_instrument_currency = row
                .get("profit_loss_on_trade")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite())?;
            let fx_rate_to_dkk = instrument_fx_rates_to_dkk
                .get(&instrument_currency)
                .copied()
                .filter(|value| value.is_finite() && *value > 0.0)?;
            Some(json!({
                "symbol": symbol,
                "instrument_currency": instrument_currency,
                "quantity": row.get("quantity").cloned().unwrap_or(JsonValue::Null),
                "unrealised_pnl_dkk": profit_loss_instrument_currency * fx_rate_to_dkk,
                "profit_loss_instrument_currency": profit_loss_instrument_currency,
                "fx_rate_to_dkk": fx_rate_to_dkk,
                "calculation_reliability": row.get("calculation_reliability").cloned().unwrap_or(JsonValue::Null),
                "updated_at": row.get("updated_at").cloned().unwrap_or(JsonValue::Null),
            }))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        value_f64(right, "unrealised_pnl_dkk")
            .abs()
            .partial_cmp(&value_f64(left, "unrealised_pnl_dkk").abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
    });
    let total_rows = rows.len();
    let total_absolute_pnl_dkk = rows
        .iter()
        .map(|row| value_f64(row, "unrealised_pnl_dkk").abs())
        .sum::<f64>();
    let mut currency_totals = BTreeMap::<String, (usize, f64)>::new();
    for row in &rows {
        let currency = text_value(row, "instrument_currency");
        let entry = currency_totals.entry(currency).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += value_f64(row, "unrealised_pnl_dkk");
    }
    let mut currencies = currency_totals
        .into_iter()
        .map(|(currency, (symbol_count, unrealised_pnl_dkk))| {
            let absolute_contribution_pct = (total_absolute_pnl_dkk > 0.0)
                .then_some(unrealised_pnl_dkk.abs() / total_absolute_pnl_dkk * 100.0);
            json!({
                "instrument_currency": currency,
                "symbol_count": symbol_count,
                "unrealised_pnl_dkk": unrealised_pnl_dkk,
                "absolute_contribution_pct": absolute_contribution_pct,
            })
        })
        .collect::<Vec<_>>();
    currencies.sort_by(|left, right| {
        value_f64(right, "unrealised_pnl_dkk")
            .abs()
            .partial_cmp(&value_f64(left, "unrealised_pnl_dkk").abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                text_value(left, "instrument_currency")
                    .cmp(&text_value(right, "instrument_currency"))
            })
    });
    rows.truncate(PERFORMANCE_EXPOSURE_ATTRIBUTION_LIMIT);

    json!({
        "status": "available",
        "scope": "read_only_stored_saxo_exposure_unrealised_pnl",
        "account_currency": account_currency,
        "fx_basis": "instrument_currency",
        "instrument_fx_rates_to_dkk": instrument_fx_rates_to_dkk,
        "updated_at": broker_exposure_snapshot.get("updated_at").cloned().unwrap_or(JsonValue::Null),
        "exposure_count": total_rows,
        "shown_row_count": rows.len(),
        "total_unrealised_pnl_dkk": broker_exposure_snapshot.get("unrealised_pnl_dkk").cloned().unwrap_or(JsonValue::Null),
        "rows": rows,
        "currencies": currencies,
    })
}

/// Group an exit by what it did rather than by how the model phrased it.
///
/// `strategy_role` is model-authored free text. Across the SELL orders it holds
/// **22 distinct values** for about three concepts, including a 150-character
/// sentence where a label belongs ("FLATTEN risk-reduction: close the full
/// 6-share DSV position because ..."). Grouping on the raw string split one
/// concept across four rows, so the panel showed four one-off flattens instead
/// of the 7,073 DKK they lost together — on the table that exists to say which
/// kind of exit is costing money.
///
/// The flatten test is deliberately the same substring the Trading Manager's
/// own `is_flatten_role` uses, so the attribution and the gate cannot disagree
/// about what counts as a flatten.
pub(crate) fn exit_route_family(strategy_role: &str) -> &'static str {
    let role = strategy_role.trim().to_ascii_lowercase();
    if role.is_empty() {
        return "unlabelled role";
    }
    if role.contains("protective_stop") {
        "protective stop"
    } else if role.contains("flatten") {
        "flatten"
    } else if role.contains("rotation") {
        "rotation exit"
    } else if role.contains("risk") {
        "risk reduction"
    } else {
        "other role"
    }
}

/// Realised-sale evidence for the **current** book.
///
/// `baseline` is the active import batch: sales booked before it belong to a
/// superseded book whose cost bases the bootstrap explicitly replaced
/// ("supersedes stale 2026-05-18 import rows that no longer match the broker
/// book"). Blending the two produced a headline of +18,762 DKK on 2026-09-03
/// while the live book stood at -21,298 -- a 40,000 DKK swing in the flattering
/// direction, on the one panel that exists to answer whether this works. The
/// superseded total is still reported, separately and labelled, because hiding
/// it would be its own distortion.
pub(crate) fn realised_sell_outcome_evidence(
    rows: &[JsonValue],
    holding_period: JsonValue,
    baseline: Option<&RealisedSellBaseline>,
) -> JsonValue {
    let (rows, superseded): (Vec<JsonValue>, Vec<JsonValue>) = match baseline {
        Some(baseline) => rows.iter().cloned().partition(|row| {
            text_value(row, "created_at").as_str() >= baseline.started_at.as_str()
        }),
        None => (rows.to_vec(), Vec::new()),
    };
    let superseded_realised_gain_dkk: f64 = superseded
        .iter()
        .filter_map(|row| row.get("realised_gain_dkk").and_then(JsonValue::as_f64))
        .filter(|value| value.is_finite())
        .sum();
    let baseline_json = match baseline {
        Some(baseline) => json!({
            "batch_id": baseline.batch_id,
            "started_at": baseline.started_at,
            "scope": "sales booked before this batch belong to a superseded book and are excluded from every figure above",
            "superseded_sale_count": superseded.len(),
            "superseded_realised_gain_dkk": superseded_realised_gain_dkk,
        }),
        None => json!({
            "batch_id": JsonValue::Null,
            "status": "no_import_batch_recorded",
            "scope": "every retained sale is counted, because no baseline boundary is known",
        }),
    };
    let rows = rows.as_slice();
    let mut realised_rows = rows
        .iter()
        .filter_map(|row| {
            let realised_gain_dkk = row
                .get("realised_gain_dkk")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite())?;
            let cost_basis_sold_dkk = row
                .get("cost_basis_sold_dkk")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)?;
            Some(json!({
                "created_at": row.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "symbol": text_value(row, "symbol"),
                "instrument_name": row.get("instrument_name").cloned().unwrap_or(JsonValue::Null),
                "quantity": row.get("quantity").cloned().unwrap_or(JsonValue::Null),
                "currency": row.get("currency").cloned().unwrap_or(JsonValue::Null),
                "realised_gain_dkk": realised_gain_dkk,
                "commission_dkk": value_f64(row, "commission_dkk"),
                "tax_dkk": value_f64(row, "tax_dkk"),
                "cost_basis_sold_dkk": cost_basis_sold_dkk,
                "mode": row.get("mode").cloned().unwrap_or(JsonValue::Null),
                "status": row.get("status").cloned().unwrap_or(JsonValue::Null),
                "execution_order_id": row.get("execution_order_id").cloned().unwrap_or(JsonValue::Null),
                "linked_order_count": row.get("linked_order_count").cloned().unwrap_or(JsonValue::Null),
                "exit_strategy_type": row.get("exit_strategy_type").cloned().unwrap_or(JsonValue::Null),
                "exit_strategy_role": row.get("exit_strategy_role").cloned().unwrap_or(JsonValue::Null),
            }))
        })
        .collect::<Vec<_>>();
    if realised_rows.is_empty() {
        return json!({
            "status": "unavailable",
            "scope": "read_only_reconciled_sell_ledger_outcomes",
            "counting_unit": "closed sale ledger row",
            "sample_requirement": PERFORMANCE_REALISED_SELL_OUTCOME_MIN_SAMPLE_SIZE,
            "closed_sale_count": 0,
            "recent_rows": [],
            "baseline": baseline_json,
            // Holding time is matched from the whole ledger, so it survives an
            // empty outcome list rather than disappearing with it.
            "holding_time_status": json_text(&holding_period, "status"),
            "holding_period": holding_period,
            "slippage_status": "unavailable_no_quote_at_submission",
        });
    }

    let mut win_count = 0usize;
    let mut loss_count = 0usize;
    let mut breakeven_count = 0usize;
    let mut total_realised_gain_dkk = 0.0;
    let mut total_commission_dkk = 0.0;
    let mut total_tax_dkk = 0.0;
    let mut total_cost_basis_sold_dkk = 0.0;
    let mut total_win_dkk = 0.0;
    let mut total_loss_dkk = 0.0;
    let mut symbol_totals = BTreeMap::<String, (String, usize, f64, f64, f64)>::new();
    let mut currency_totals = BTreeMap::<String, (usize, f64, f64, f64)>::new();
    let mut exit_route_totals = BTreeMap::<String, (String, usize, f64, f64, f64)>::new();
    let mut linked_exit_route_count = 0usize;
    let mut unlinked_ledger_count = 0usize;
    let mut ambiguous_exit_link_count = 0usize;
    for row in &realised_rows {
        let realised_gain_dkk = value_f64(row, "realised_gain_dkk");
        let symbol = text_value(row, "symbol");
        let currency = text_value(row, "currency")
            .if_empty_then(|| Some("Unknown".to_string()))
            .unwrap_or_else(|| "Unknown".to_string());
        total_realised_gain_dkk += realised_gain_dkk;
        total_commission_dkk += value_f64(row, "commission_dkk");
        total_tax_dkk += value_f64(row, "tax_dkk");
        total_cost_basis_sold_dkk += value_f64(row, "cost_basis_sold_dkk");
        let symbol_total =
            symbol_totals
                .entry(symbol)
                .or_insert((currency.clone(), 0, 0.0, 0.0, 0.0));
        symbol_total.1 += 1;
        symbol_total.2 += realised_gain_dkk;
        symbol_total.3 += value_f64(row, "commission_dkk");
        symbol_total.4 += value_f64(row, "tax_dkk");
        let currency_total = currency_totals
            .entry(currency)
            .or_insert((0, 0.0, 0.0, 0.0));
        currency_total.0 += 1;
        currency_total.1 += realised_gain_dkk;
        currency_total.2 += value_f64(row, "commission_dkk");
        currency_total.3 += value_f64(row, "tax_dkk");
        let linked_order_count = value_i64(row, "linked_order_count");
        let (exit_link_status, exit_route) = if linked_order_count > 1 {
            ambiguous_exit_link_count += 1;
            (
                "ambiguous_reconciled_link".to_string(),
                "ambiguous reconciled link".to_string(),
            )
        } else if linked_order_count == 1 && value_i64(row, "execution_order_id") > 0 {
            linked_exit_route_count += 1;
            let strategy_type = text_value(row, "exit_strategy_type");
            let strategy_role = text_value(row, "exit_strategy_role");
            let strategy_type = if strategy_type.is_empty() {
                "unspecified type".to_string()
            } else {
                strategy_type
            };
            (
                "linked_execution_order".to_string(),
                format!("{strategy_type} / {}", exit_route_family(&strategy_role)),
            )
        } else {
            unlinked_ledger_count += 1;
            (
                "unlinked_local_ledger".to_string(),
                "unlinked local ledger".to_string(),
            )
        };
        let exit_route_total =
            exit_route_totals
                .entry(exit_route)
                .or_insert((exit_link_status, 0, 0.0, 0.0, 0.0));
        exit_route_total.1 += 1;
        exit_route_total.2 += realised_gain_dkk;
        exit_route_total.3 += value_f64(row, "commission_dkk");
        exit_route_total.4 += value_f64(row, "tax_dkk");
        if realised_gain_dkk > 1e-9 {
            win_count += 1;
            total_win_dkk += realised_gain_dkk;
        } else if realised_gain_dkk < -1e-9 {
            loss_count += 1;
            total_loss_dkk += realised_gain_dkk;
        } else {
            breakeven_count += 1;
        }
    }
    let decisive_sale_count = win_count + loss_count;
    let average_win_dkk = average_or_null(total_win_dkk, win_count);
    let average_loss_dkk = average_or_null(total_loss_dkk, loss_count);
    let payoff_ratio = match (average_win_dkk.as_f64(), average_loss_dkk.as_f64()) {
        (Some(average_win), Some(average_loss)) if average_loss < -1e-9 => {
            json!(average_win / average_loss.abs())
        }
        _ => JsonValue::Null,
    };
    let status = if realised_rows.len() < PERFORMANCE_REALISED_SELL_OUTCOME_MIN_SAMPLE_SIZE {
        "collecting"
    } else {
        "preliminary"
    };
    let mut symbol_attribution = symbol_totals
        .into_iter()
        .map(
            |(
                symbol,
                (
                    instrument_currency,
                    closed_sale_count,
                    realised_gain_dkk,
                    commission_dkk,
                    tax_dkk,
                ),
            )| {
                json!({
                    "symbol": symbol,
                    "instrument_currency": instrument_currency,
                    "closed_sale_count": closed_sale_count,
                    "realised_gain_dkk": realised_gain_dkk,
                    "commission_dkk": commission_dkk,
                    "tax_dkk": tax_dkk,
                })
            },
        )
        .collect::<Vec<_>>();
    symbol_attribution.sort_by(|left, right| {
        value_f64(right, "realised_gain_dkk")
            .abs()
            .partial_cmp(&value_f64(left, "realised_gain_dkk").abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
    });
    let attributed_symbol_count = symbol_attribution.len();
    symbol_attribution.truncate(PERFORMANCE_REALISED_SELL_OUTCOME_ATTRIBUTION_LIMIT);
    let mut currency_attribution = currency_totals
        .into_iter()
        .map(
            |(
                instrument_currency,
                (closed_sale_count, realised_gain_dkk, commission_dkk, tax_dkk),
            )| {
                json!({
                    "instrument_currency": instrument_currency,
                    "closed_sale_count": closed_sale_count,
                    "realised_gain_dkk": realised_gain_dkk,
                    "commission_dkk": commission_dkk,
                    "tax_dkk": tax_dkk,
                })
            },
        )
        .collect::<Vec<_>>();
    currency_attribution.sort_by(|left, right| {
        value_f64(right, "realised_gain_dkk")
            .abs()
            .partial_cmp(&value_f64(left, "realised_gain_dkk").abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                text_value(left, "instrument_currency")
                    .cmp(&text_value(right, "instrument_currency"))
            })
    });
    let mut exit_route_attribution = exit_route_totals
        .into_iter()
        .map(
            |(
                exit_route,
                (link_status, closed_sale_count, realised_gain_dkk, commission_dkk, tax_dkk),
            )| {
                json!({
                    "exit_route": exit_route,
                    "link_status": link_status,
                    "closed_sale_count": closed_sale_count,
                    "realised_gain_dkk": realised_gain_dkk,
                    "commission_dkk": commission_dkk,
                    "tax_dkk": tax_dkk,
                })
            },
        )
        .collect::<Vec<_>>();
    exit_route_attribution.sort_by(|left, right| {
        value_f64(right, "realised_gain_dkk")
            .abs()
            .partial_cmp(&value_f64(left, "realised_gain_dkk").abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text_value(left, "exit_route").cmp(&text_value(right, "exit_route")))
    });
    realised_rows.truncate(PERFORMANCE_REALISED_SELL_OUTCOME_RECENT_LIMIT);
    json!({
        "status": status,
        "scope": "read_only_reconciled_sell_ledger_outcomes",
        "counting_unit": "closed sale ledger row",
        "sample_requirement": PERFORMANCE_REALISED_SELL_OUTCOME_MIN_SAMPLE_SIZE,
        "scan_limit": PERFORMANCE_REALISED_SELL_OUTCOME_LIMIT,
        "baseline": baseline_json,
        "closed_sale_count": win_count + loss_count + breakeven_count,
        "decisive_sale_count": decisive_sale_count,
        "win_count": win_count,
        "loss_count": loss_count,
        "breakeven_count": breakeven_count,
        "win_rate": fraction_or_null(win_count, decisive_sale_count),
        "average_win_dkk": average_win_dkk,
        "average_loss_dkk": average_loss_dkk,
        "payoff_ratio": payoff_ratio,
        "total_realised_gain_dkk": total_realised_gain_dkk,
        "total_commission_dkk": total_commission_dkk,
        "total_tax_dkk": total_tax_dkk,
        "total_cost_basis_sold_dkk": total_cost_basis_sold_dkk,
        "attributed_symbol_count": attributed_symbol_count,
        "shown_symbol_attribution_count": symbol_attribution.len(),
        "symbol_attribution": symbol_attribution,
        "currency_attribution": currency_attribution,
        "linked_exit_route_count": linked_exit_route_count,
        "unlinked_ledger_count": unlinked_ledger_count,
        "ambiguous_exit_link_count": ambiguous_exit_link_count,
        "exit_route_attribution": exit_route_attribution,
        "recent_rows": realised_rows,
        "holding_time_status": json_text(&holding_period, "status"),
        "holding_period": holding_period,
        "slippage_status": "unavailable_no_quote_at_submission",
    })
}

pub(crate) fn annotate_overview_integrity_acknowledgements(
    mismatches: &mut [JsonValue],
    warnings: &mut [JsonValue],
    acknowledgements: &JsonValue,
) -> usize {
    let active_acknowledgements = acknowledgements
        .get("acknowledgements")
        .and_then(JsonValue::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    if !row
                        .get("enabled")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    Some((json_text(row, "issue_key"), row.clone()))
                })
                .filter(|(key, _)| !key.is_empty())
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut acknowledged_count = 0;
    for issue in mismatches.iter_mut().chain(warnings.iter_mut()) {
        let issue_key = overview_integrity_issue_key(issue);
        if let Some(object) = issue.as_object_mut() {
            object.insert("issue_key".to_string(), JsonValue::from(issue_key.clone()));
            if let Some(acknowledgement) = active_acknowledgements.get(&issue_key) {
                acknowledged_count += 1;
                object.insert("acknowledged".to_string(), JsonValue::from(true));
                object.insert("acknowledgement".to_string(), acknowledgement.clone());
            } else {
                object.insert("acknowledged".to_string(), JsonValue::from(false));
            }
        }
    }
    acknowledged_count
}

/// Adopted broker positions are bookkeeping for holdings that already existed
/// at the broker when this system took over the book. No trade happened under
/// this system, so they can never acquire a trade-ledger row.
///
/// Counting them as unreconciled held overview `healthy` false continuously
/// from the 2026-05-05 adoption onward, which trains an operator to read this
/// warning as background noise -- exactly how a genuine ledger-less fill would
/// go unnoticed. They are reported as a separate count instead.
const ADOPTED_ORDER_EXCLUSION: &str = "COALESCE(strategy_type, '') <> 'portfolio_sync'";

/// Execution orders that are genuinely stuck or missing accounting. Shared by
/// `overview_integrity` and its regression test so the two cannot drift.
pub(crate) fn unreconciled_orders_sql(stale_cutoff: &str, fill_cutoff: &str) -> String {
    format!(
        "SELECT id, created_at, symbol, action, status, quantity, currency, \
                limit_price_local, ledger_id, broker_order_id, error_text \
         FROM execution_orders \
         WHERE status = 'broker_state_unknown' \
            OR (status IN ('broker_working', 'submitted_to_broker', \
                           'broker_partially_filled', 'broker_replace_requested', \
                           'broker_cancel_requested', 'pending_execution', \
                           'waiting_for_market_open', \
                           'waiting_for_cash_settlement', \
                           'waiting_for_virtual_cash_budget', \
                           'waiting_for_technical_gate') \
                AND {} \
                AND created_at < '{}') \
            OR (status = 'executed' \
                AND ledger_id IS NULL \
                AND {} \
                AND created_at < '{}') \
         ORDER BY created_at ASC, id ASC \
         LIMIT 20",
        RESTING_PROTECTIVE_STOP_EXCLUSION,
        sql_escape(stale_cutoff),
        ADOPTED_ORDER_EXCLUSION,
        sql_escape(fill_cutoff)
    )
}

/// A protective stop is GoodTillCancel and is *supposed* to rest at
/// `broker_working` for as long as the position is held, so age alone says
/// nothing about it. Without this exclusion every adopted stop becomes a
/// permanent integrity warning 24 hours after placement, which is the same
/// false positive that adopted positions produced before 2026-07-25 -- and a
/// panel that is always warning is a panel nobody reads. The
/// `broker_state_unknown` and executed-without-a-ledger-row branches
/// deliberately still apply: those are real faults for a stop too.
const RESTING_PROTECTIVE_STOP_EXCLUSION: &str = "COALESCE(strategy_type, '') <> 'protective_stop'";

pub(crate) fn overview_integrity_issue_key(issue: &JsonValue) -> String {
    let code = json_text(issue, "code");
    let severity = json_text(issue, "severity");
    let scope = match code.as_str() {
        "portfolio_identity_mismatch" => "portfolio".to_string(),
        "ledger_history_cash_drift" => {
            let recorded_at = json_text(issue, "history_recorded_at");
            if recorded_at.is_empty() {
                "latest-history".to_string()
            } else {
                recorded_at
            }
        }
        "broker_cash_drift" => {
            let currency = json_text(issue, "broker_currency");
            if currency.is_empty() {
                "broker-cash".to_string()
            } else {
                format!("broker-cash-{currency}")
            }
        }
        "broker_exposure_pnl_drift" => {
            let currency = json_text(issue, "broker_account_currency");
            if currency.is_empty() {
                "broker-exposure-pnl".to_string()
            } else {
                format!("broker-exposure-pnl-{currency}")
            }
        }
        "broker_exposure_quantity_drift" => issue
            .get("symbols")
            .and_then(JsonValue::as_array)
            .map(|symbols| {
                symbols
                    .iter()
                    .map(|row| json_text(row, "symbol"))
                    .filter(|symbol| !symbol.is_empty())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| "broker-exposure-quantities".to_string()),
        "implausible_position_lot_cost_basis" => issue
            .get("lots")
            .and_then(JsonValue::as_array)
            .map(|lots| {
                lots.iter()
                    .map(|lot| json_text(lot, "lot_id"))
                    .filter(|lot_id| !lot_id.is_empty())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| "position-lots".to_string()),
        "stale_or_unreconciled_execution_orders" => "execution-orders".to_string(),
        "day_order_expiry_sync_pending" => "day-orders".to_string(),
        // Scope by the exact key set so acknowledging today's drift cannot
        // suppress tomorrow's newly uncontracted key.
        "config_contract_drift" => issue
            .get("keys")
            .and_then(JsonValue::as_array)
            .map(|keys| {
                let mut keys = keys
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                keys.sort();
                keys.join(",")
            })
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| "config-contract".to_string()),
        _ => "general".to_string(),
    };
    format!(
        "{}:{}:{}",
        integrity_key_part(&code),
        integrity_key_part(&severity),
        integrity_key_part(&scope)
    )
}

pub(crate) fn broker_cash_reconciliation_enabled(config: &YamlValue) -> bool {
    yaml_bool(config, &["portfolio", "broker_cash_reconciliation_enabled"]).unwrap_or(false)
}

fn integrity_key_part(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

pub(crate) fn broker_exposure_quantity_mismatches(
    exposures: &[JsonValue],
    positions: &HashMap<String, JsonValue>,
) -> Vec<JsonValue> {
    let positions_by_symbol = positions
        .iter()
        .map(|(symbol, row)| (canonical_symbol_key(symbol), row))
        .collect::<HashMap<_, _>>();
    exposures
        .iter()
        .filter_map(|exposure| {
            let symbol = text_value(exposure, "symbol");
            if symbol.is_empty() {
                return None;
            }
            let exposure_quantity = value_f64(exposure, "quantity");
            let position = positions_by_symbol
                .get(&canonical_symbol_key(&symbol));
            let position_quantity = position
                .map(|row| value_f64(row, "quantity"))
                .unwrap_or(0.0);
            let difference = exposure_quantity - position_quantity;
            if difference.abs() <= INTEGRITY_BROKER_QUANTITY_ABS_TOLERANCE {
                return None;
            }
            Some(json!({
                "symbol": symbol,
                "exposure_quantity": exposure_quantity,
                "position_quantity": position_quantity,
                "difference": difference,
                "exposure_updated_at": exposure.get("updated_at").cloned().unwrap_or(JsonValue::Null),
                "position_updated_at": position
                    .and_then(|row| row.get("updated_at"))
                    .cloned()
                    .unwrap_or(JsonValue::Null)
            }))
        })
        .take(20)
        .collect()
}

impl AppState {
    pub(crate) async fn overview_integrity(
        &self,
        aggregate: &JsonValue,
        latest_history: &JsonValue,
        cash_summary: &JsonValue,
    ) -> Result<JsonValue> {
        let mut warnings = Vec::new();
        let mut mismatches = Vec::new();
        let mut checks = serde_json::Map::new();

        let total_value = value_f64(aggregate, "total_market_value_dkk");
        let invested_value = value_f64(aggregate, "invested_market_value_dkk");
        let aggregate_cash = value_f64(aggregate, "cash_balance_dkk");
        let expected_total = invested_value + aggregate_cash;
        if total_value.abs() > 1e-9 || expected_total.abs() > 1e-9 {
            if money_mismatch_exceeds_tolerance(
                total_value,
                expected_total,
                INTEGRITY_MONEY_ABS_TOLERANCE_DKK,
                INTEGRITY_MONEY_REL_TOLERANCE,
            ) {
                mismatches.push(json!({
                    "code": "portfolio_identity_mismatch",
                    "severity": "error",
                    "message": "Portfolio total does not match invested value plus cash.",
                    "total_market_value_dkk": total_value,
                    "invested_market_value_dkk": invested_value,
                    "cash_balance_dkk": aggregate_cash,
                    "expected_total_market_value_dkk": expected_total,
                    "difference_dkk": total_value - expected_total
                }));
                checks.insert("portfolio_identity".to_string(), json!("mismatch"));
            } else {
                checks.insert("portfolio_identity".to_string(), json!("ok"));
            }
        } else {
            checks.insert("portfolio_identity".to_string(), json!("skipped_no_value"));
        }

        let ledger_cash = value_f64(cash_summary, "cash_balance_dkk");
        if latest_history
            .as_object()
            .is_some_and(|history| !history.is_empty())
        {
            let history_cash = value_f64(latest_history, "cash_balance_dkk");
            if money_mismatch_exceeds_tolerance(
                ledger_cash,
                history_cash,
                INTEGRITY_MONEY_ABS_TOLERANCE_DKK,
                INTEGRITY_MONEY_REL_TOLERANCE,
            ) {
                mismatches.push(json!({
                    "code": "ledger_history_cash_drift",
                    "severity": "error",
                    "message": "Ledger-derived cash differs from the latest portfolio value snapshot.",
                    "ledger_cash_balance_dkk": ledger_cash,
                    "history_cash_balance_dkk": history_cash,
                    "difference_dkk": ledger_cash - history_cash,
                    "history_recorded_at": latest_history.get("recorded_at").cloned().unwrap_or(JsonValue::Null)
                }));
                checks.insert("ledger_history_cash".to_string(), json!("mismatch"));
            } else {
                checks.insert("ledger_history_cash".to_string(), json!("ok"));
            }
        } else {
            checks.insert(
                "ledger_history_cash".to_string(),
                json!("skipped_no_history"),
            );
        }

        if let Ok(Some(broker_cash)) = self
            .first_json(
                "SELECT updated_at, currency, cash_available_for_trading, cash_balance \
                 FROM broker_balance_snapshots WHERE singleton_key = 'main' LIMIT 1",
            )
            .await
        {
            if !broker_cash_reconciliation_enabled(&self.config) {
                // Saxo SIM is often a large broker account while the application tracks a
                // deliberately bounded DKK strategy book. Comparing their absolute cash
                // values produces a false integrity warning. Keep the broker snapshot for
                // execution/audit, but require an explicit opt-in before reconciling it.
                checks.insert(
                    "broker_cash".to_string(),
                    json!("skipped_independent_strategy_ledger"),
                );
            } else {
                let broker_currency = text_value(&broker_cash, "currency")
                    .if_empty_then(|| Some("DKK".to_string()))
                    .unwrap_or_else(|| "DKK".to_string());
                let broker_cash_local = value_f64(&broker_cash, "cash_available_for_trading")
                    .max(value_f64(&broker_cash, "cash_balance"));
                let broker_fx =
                    crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &broker_currency).await;
                let broker_cash_dkk = broker_cash_local * broker_fx;
                if broker_cash_local.abs() > 1e-9
                    && money_mismatch_exceeds_tolerance(
                        ledger_cash,
                        broker_cash_dkk,
                        INTEGRITY_BROKER_CASH_ABS_TOLERANCE_DKK,
                        INTEGRITY_BROKER_CASH_REL_TOLERANCE,
                    )
                {
                    warnings.push(json!({
                        "code": "broker_cash_drift",
                        "severity": "warning",
                        "message": "Ledger-derived cash differs from the latest Saxo broker cash snapshot; settlement timing can explain some drift.",
                        "ledger_cash_balance_dkk": ledger_cash,
                        "broker_cash_balance_dkk": broker_cash_dkk,
                        "broker_cash_local": broker_cash_local,
                        "broker_currency": broker_currency,
                        "difference_dkk": ledger_cash - broker_cash_dkk,
                        "broker_updated_at": broker_cash.get("updated_at").cloned().unwrap_or(JsonValue::Null)
                    }));
                    checks.insert("broker_cash".to_string(), json!("warning"));
                } else {
                    checks.insert("broker_cash".to_string(), json!("ok"));
                }
            }
        } else {
            checks.insert("broker_cash".to_string(), json!("skipped_no_snapshot"));
        }

        let broker_exposures = self
            .select_json(
                "SELECT symbol, updated_at, quantity, profit_loss_on_trade, currency, calculation_reliability \
                 FROM broker_instrument_exposures ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default();
        let mut broker_exposure_fx_rates = BTreeMap::new();
        for currency in broker_exposures
            .iter()
            .map(|row| text_value(row, "currency"))
            .map(|currency| {
                if currency.is_empty() {
                    "DKK".to_string()
                } else {
                    currency
                }
            })
            .collect::<BTreeSet<_>>()
        {
            let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &currency).await;
            broker_exposure_fx_rates.insert(currency, fx_rate);
        }
        let broker_exposure_snapshot = if broker_exposures.is_empty() {
            JsonValue::Null
        } else {
            let broker_account_currency = self
                .first_json(
                    "SELECT account_currency FROM broker_account_snapshots WHERE singleton_key = 'main' LIMIT 1",
                )
                .await
                .ok()
                .flatten()
                .and_then(|row| row.get("account_currency").cloned())
                .and_then(|value| value.as_str().map(ToString::to_string))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "DKK".to_string());
            json!({
                "status": "available",
                "unrealised_pnl_dkk": broker_exposures
                    .iter()
                    .map(|row| {
                        let currency = text_value(row, "currency");
                        let currency = if currency.is_empty() { "DKK" } else { &currency };
                        value_f64(row, "profit_loss_on_trade")
                            * broker_exposure_fx_rates.get(currency).copied().unwrap_or(1.0)
                    })
                    .sum::<f64>(),
                "account_currency": broker_account_currency,
                "fx_basis": "instrument_currency",
                "instrument_fx_rates_to_dkk": broker_exposure_fx_rates,
                "exposure_count": broker_exposures.len(),
                "updated_at": latest_snapshot_timestamp(&broker_exposures, "updated_at"),
            })
        };
        let pnl_reconciliation =
            unrealised_pnl_reconciliation(aggregate, latest_history, &broker_exposure_snapshot);
        let unrealised_pnl_attribution = broker_exposure_pnl_attribution(
            &broker_exposures,
            &broker_exposure_snapshot,
            &broker_exposure_fx_rates,
        );
        let realised_sell_rows = self
            .select_json(&format!(
                "SELECT l.id, l.created_at, l.symbol, l.instrument_name, l.quantity, l.currency, \
                        l.realised_gain_dkk, l.commission_dkk, l.tax_dkk, l.cost_basis_sold_dkk, l.mode, l.status, \
                        fill_link.execution_order_id, fill_link.linked_order_count, \
                        eo.strategy_type AS exit_strategy_type, eo.strategy_role AS exit_strategy_role \
                 FROM trade_ledger l \
                 LEFT JOIN ( \
                    SELECT ledger_id, \
                           CASE WHEN COUNT(DISTINCT execution_order_id) = 1 \
                                THEN MIN(execution_order_id) END AS execution_order_id, \
                           COUNT(DISTINCT execution_order_id) AS linked_order_count \
                    FROM execution_fills \
                    WHERE ledger_id IS NOT NULL \
                    GROUP BY ledger_id \
                 ) fill_link ON fill_link.ledger_id = l.id \
                 LEFT JOIN execution_orders eo ON eo.id = fill_link.execution_order_id \
                 WHERE UPPER(l.side) = 'SELL' \
                   AND l.status IN ('executed', 'approved') \
                   AND COALESCE(l.cost_basis_sold_dkk, 0) > 0 \
                 ORDER BY l.created_at DESC, l.id DESC \
                 LIMIT {}",
                PERFORMANCE_REALISED_SELL_OUTCOME_LIMIT
            ))
            .await
            // Swallowing this error made a broken query and an empty ledger
            // render identically: the panel said "unavailable" either way, and
            // nobody could tell which. Whatever the cause, it must be legible.
            .unwrap_or_else(|err| {
                warn!("realised SELL outcome query failed: {err:#}");
                Vec::new()
            });
        // Holding time needs the BUY side of the ledger and the acquisitions
        // that predate it, neither of which the sell-outcome query carries.
        let holding_ledger_rows = self
            .select_json(
                "SELECT symbol, side, quantity, created_at, realised_gain_dkk \
                 FROM trade_ledger \
                 WHERE status IN ('executed', 'approved') \
                 ORDER BY created_at ASC, id ASC",
            )
            .await
            .unwrap_or_else(|err| {
                warn!("holding-period ledger query failed: {err:#}");
                Vec::new()
            });
        // `buy_fill` lots duplicate a BUY ledger row and would double-count the
        // acquisition; only the import and bootstrap lots add anything.
        let holding_seed_lots = self
            .select_json(
                "SELECT symbol, quantity_original, \
                        COALESCE(acquired_at, created_at) AS acquired_at \
                 FROM position_lots \
                 WHERE source_type <> 'buy_fill' \
                 ORDER BY COALESCE(acquired_at, created_at) ASC",
            )
            .await
            .unwrap_or_else(|err| {
                warn!("holding-period seed-lot query failed: {err:#}");
                Vec::new()
            });
        let holding_period = crate::holding_period::holding_period_evidence_json(
            &crate::holding_period::fifo_holding_periods(&holding_ledger_rows, &holding_seed_lots),
        );
        // The active import batch marks where the current book begins. Sales
        // before it were booked against cost bases the bootstrap replaced.
        let realised_sell_baseline = self
            .first_json(
                "SELECT batch_id, imported_at FROM import_batches                  ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
            )
            .await
            .unwrap_or_else(|err| {
                warn!("realised SELL baseline lookup failed: {err:#}");
                None
            })
            .map(|row| RealisedSellBaseline {
                batch_id: text_value(&row, "batch_id"),
                started_at: text_value(&row, "imported_at"),
            })
            .filter(|baseline| !baseline.started_at.is_empty());
        let realised_sell_outcomes = realised_sell_outcome_evidence(
            &realised_sell_rows,
            holding_period,
            realised_sell_baseline.as_ref(),
        );
        if broker_exposure_snapshot.is_null() {
            checks.insert(
                "broker_exposure_aggregate".to_string(),
                json!("skipped_no_snapshot"),
            );
        } else {
            let broker_account_currency = text_value(&broker_exposure_snapshot, "account_currency");
            let exposure_unrealised_pnl_dkk =
                value_f64(&broker_exposure_snapshot, "unrealised_pnl_dkk");
            let aggregate_unrealised_pnl_dkk = value_f64(aggregate, "total_unrealised_pnl_dkk");
            if money_mismatch_exceeds_tolerance(
                aggregate_unrealised_pnl_dkk,
                exposure_unrealised_pnl_dkk,
                INTEGRITY_BROKER_EXPOSURE_ABS_TOLERANCE_DKK,
                INTEGRITY_BROKER_EXPOSURE_REL_TOLERANCE,
            ) {
                warnings.push(json!({
                    "code": "broker_exposure_pnl_drift",
                    "severity": "warning",
                    "message": "Dashboard unrealised P/L differs from the latest Saxo instrument exposure aggregate.",
                    "dashboard_unrealised_pnl_dkk": aggregate_unrealised_pnl_dkk,
                    "broker_exposure_unrealised_pnl_dkk": exposure_unrealised_pnl_dkk,
                    "broker_account_currency": broker_account_currency,
                    "broker_exposure_fx_basis": "instrument_currency",
                    "difference_dkk": aggregate_unrealised_pnl_dkk - exposure_unrealised_pnl_dkk,
                    "exposure_count": broker_exposures.len()
                }));
                checks.insert("broker_exposure_aggregate".to_string(), json!("warning"));
            } else {
                checks.insert("broker_exposure_aggregate".to_string(), json!("ok"));
            }

            let broker_positions = self
                .select_json(
                    "SELECT symbol, quantity, updated_at FROM broker_position_snapshots ORDER BY symbol ASC",
                )
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|row| (canonical_symbol_key(&text_value(&row, "symbol")), row))
                .collect::<HashMap<_, _>>();
            let quantity_mismatches =
                broker_exposure_quantity_mismatches(&broker_exposures, &broker_positions);
            if quantity_mismatches.is_empty() {
                checks.insert("broker_exposure_quantities".to_string(), json!("ok"));
            } else {
                warnings.push(json!({
                    "code": "broker_exposure_quantity_drift",
                    "severity": "warning",
                    "message": "One or more Saxo instrument exposure quantities differ from broker position quantities.",
                    "count": quantity_mismatches.len(),
                    "symbols": quantity_mismatches
                }));
                checks.insert("broker_exposure_quantities".to_string(), json!("warning"));
            }
        }

        let suspicious_lots = self
            .select_json(&format!(
                "SELECT lot_id, symbol, quantity_original, cost_basis_total_dkk, \
                 cost_basis_total_dkk / NULLIF(quantity_original, 0) AS unit_cost_dkk \
                 FROM position_lots \
                 WHERE quantity_original > 0 \
                   AND cost_basis_total_dkk > 0 \
                   AND cost_basis_total_dkk / NULLIF(quantity_original, 0) > {} \
                 ORDER BY unit_cost_dkk DESC \
                 LIMIT 10",
                INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK
            ))
            .await
            .unwrap_or_default();
        if suspicious_lots.is_empty() {
            checks.insert("position_lot_plausibility".to_string(), json!("ok"));
        } else {
            mismatches.push(json!({
                "code": "implausible_position_lot_cost_basis",
                "severity": "error",
                "message": "One or more position lots have an implausibly high per-share DKK cost basis.",
                "threshold_unit_cost_dkk": INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK,
                "lots": suspicious_lots
            }));
            checks.insert("position_lot_plausibility".to_string(), json!("mismatch"));
        }

        let stale_cutoff =
            (Utc::now() - Duration::hours(24)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let fill_cutoff =
            (Utc::now() - Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let unreconciled_orders = self
            .select_json(&unreconciled_orders_sql(&stale_cutoff, &fill_cutoff))
            .await
            .unwrap_or_default();
        let adopted_orders_without_ledger = self
            .select_json(ADOPTED_ORDERS_WITHOUT_LEDGER_SQL)
            .await
            .ok()
            .and_then(|rows| rows.first().map(|row| value_f64(row, "count") as i64))
            .unwrap_or(0);
        if unreconciled_orders.is_empty() {
            checks.insert("unreconciled_orders".to_string(), json!("ok"));
        } else {
            checks.insert("unreconciled_orders".to_string(), json!("warning"));
            warnings.push(json!({
                "code": "stale_or_unreconciled_execution_orders",
                "severity": "warning",
                "message": "Some execution orders are still pending, have an unresolved broker placement outcome, or executed without a linked ledger row.",
                "count": unreconciled_orders.len()
            }));
        }

        let mut active_broker_orders = self
            .select_json(
                "SELECT id, created_at, symbol, action, order_type, mode, status, adapter, \
                        quantity, price_local, limit_price_local, stop_price_local, currency, \
                        ledger_id, broker_order_id, execution_result_json \
                 FROM execution_orders \
                 WHERE mode = 'live' \
                   AND adapter = 'saxo' \
                   AND broker_order_id IS NOT NULL \
                   AND broker_order_id <> '' \
                   AND status IN ('submitted_to_broker', 'broker_working', \
                                  'broker_amended', 'broker_partially_filled', \
                                  'broker_replace_requested', 'broker_cancel_requested') \
                 ORDER BY created_at ASC, id ASC \
                 LIMIT 50",
            )
            .await
            .unwrap_or_default();
        let market_rows = self.market_exchange_rows();
        for order in &mut active_broker_orders {
            enrich_execution_order_lifecycle(order, &market_rows);
        }
        let expiry_pending_orders = active_broker_orders
            .into_iter()
            .filter(|order| text_value(order, "lifecycle_state") == "expiry_pending_broker_sync")
            .collect::<Vec<_>>();
        if expiry_pending_orders.is_empty() {
            checks.insert("day_order_expiry_sync".to_string(), json!("ok"));
        } else {
            checks.insert("day_order_expiry_sync".to_string(), json!("warning"));
            warnings.push(json!({
                "code": "day_order_expiry_sync_pending",
                "severity": "warning",
                "message": "One or more Saxo DayOrders passed expected exchange-calendar expiry but still need broker sync confirmation.",
                "count": expiry_pending_orders.len()
            }));
        }

        // Quote freshness. Asks whether prices moved at all on an open
        // exchange, because every other check here asks whether an action
        // failed and a frozen feed never fails -- it simply stops, and the
        // 2026-09-19 session was lost with `integrity.healthy` true throughout.
        let stale_after_minutes =
            yaml_i64(&self.config, &["execution", "stale_quote_alert_minutes"])
                .filter(|value| *value > 0)
                .unwrap_or(crate::quote_freshness::DEFAULT_STALE_QUOTE_MINUTES);
        let open_exchange_codes = self
            .market_exchange_rows()
            .iter()
            .filter(|row| {
                row.get("is_open")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
            })
            .filter_map(|row| {
                row.get("code")
                    .and_then(JsonValue::as_str)
                    .map(|code| code.to_ascii_lowercase())
            })
            .collect::<std::collections::HashSet<_>>();
        // A failed read must not become an empty-and-therefore-fine result. The
        // verdict distinguishes "no rows" from "nothing held here", so the
        // error is carried rather than flattened into a default. The fetch is
        // wider than the threshold so the trailing window is always covered;
        // the verdict trims to the threshold itself.
        let quote_read = self
            .select_json(&format!(
                "SELECT recorded_at, symbol, price_local
                 FROM portfolio_position_snapshots
                 WHERE recorded_at >= '{}'
                 ORDER BY recorded_at ASC",
                sql_escape(
                    &(Utc::now() - chrono::Duration::minutes(stale_after_minutes * 3))
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                )
            ))
            .await;
        let quote_read_failed = quote_read.is_err();
        if let Err(err) = &quote_read {
            warn!("quote freshness snapshot read degraded: {err:#}");
        }
        let quote_freshness = crate::quote_freshness::quote_freshness_verdict(
            &quote_read.unwrap_or_default(),
            &open_exchange_codes,
            stale_after_minutes,
            &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        let quote_status = quote_freshness
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown");
        // `unknown` means the question could not be answered -- a failed read or
        // no snapshot in the window. Reporting that as `ok` is exactly the
        // silent pass this check was added to remove.
        let quote_check = match quote_status {
            "error" => "error",
            "unknown" => "warning",
            _ => "ok",
        };
        checks.insert("quote_freshness".to_string(), JsonValue::from(quote_check));
        if quote_check != "ok" {
            warnings.push(json!({
                "code": if quote_status == "error" {
                    "quote_feed_frozen"
                } else {
                    "quote_freshness_unmeasured"
                },
                "severity": quote_check,
                "message": quote_freshness
                    .get("message")
                    .cloned()
                    .unwrap_or(JsonValue::Null),
                "watched_positions": quote_freshness.get("watched_positions").cloned(),
                "window_minutes": quote_freshness.get("window_minutes").cloned(),
                "snapshot_read_failed": quote_read_failed,
            }));
        }

        // Config contract. Known unwired keys are reported as visible context
        // rather than as a warning: they are already documented in
        // wiki/urgent-todo.md and would otherwise hold the whole overview
        // permanently unhealthy, which trains the operator to ignore this panel.
        // A warning is reserved for genuine new drift -- a key added to config
        // without a contract entry, or an enforced key config stopped supplying
        // -- both of which are actionable and self-clearing.
        let (contract_summary, contract_findings) =
            crate::config_contract::audit_config(&self.config);
        let unused_risk_keys = contract_findings
            .iter()
            .filter(|finding| {
                finding.kind == crate::config_contract::FindingKind::UnusedKeyPresent
                    && finding.risk_surface
            })
            .map(|finding| json!({"key": finding.path, "note": finding.note}))
            .collect::<Vec<_>>();
        let drift_keys = contract_findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.kind,
                    crate::config_contract::FindingKind::UncontractedKey
                ) || (finding.kind == crate::config_contract::FindingKind::ContractedKeyMissing
                    && finding.risk_surface)
            })
            .map(|finding| {
                json!({
                    "key": finding.path,
                    "kind": finding.kind.as_str(),
                    "note": finding.note
                })
            })
            .collect::<Vec<_>>();
        let config_contract = json!({
            "enforced": contract_summary.enforced,
            "advisory": contract_summary.advisory,
            "unused": contract_summary.unused,
            "unused_risk_surface": contract_summary.unused_risk_surface,
            "uncontracted": contract_summary.uncontracted,
            "missing": contract_summary.missing,
            "unused_risk_keys": unused_risk_keys,
            "drift_keys": drift_keys
        });
        if drift_keys.is_empty() {
            checks.insert(
                "config_contract".to_string(),
                if contract_summary.unused_risk_surface > 0 {
                    json!("unused_risk_keys")
                } else {
                    json!("ok")
                },
            );
        } else {
            checks.insert("config_contract".to_string(), json!("warning"));
            let keys = drift_keys
                .iter()
                .map(|row| text_value(row, "key"))
                .collect::<Vec<_>>();
            warnings.push(json!({
                "code": "config_contract_drift",
                "severity": "warning",
                "message": "Configuration keys are not described by the config contract, or an enforced key is missing from configuration. Update CONTRACT in src/config_contract.rs so the key's real effect is recorded.",
                "count": drift_keys.len(),
                "keys": keys,
                "details": drift_keys.clone()
            }));
        }

        let acknowledgements = self
            .overview_integrity_acknowledgements_value()
            .await
            .unwrap_or_else(|err| {
                warn!("overview integrity acknowledgement state degraded: {err:#}");
                json!({"acknowledgements": [], "updated_at": null})
            });
        let acknowledged_issue_count = annotate_overview_integrity_acknowledgements(
            &mut mismatches,
            &mut warnings,
            &acknowledgements,
        );

        Ok(json!({
            "healthy": warnings.is_empty() && mismatches.is_empty() && unreconciled_orders.is_empty(),
            "warnings": warnings,
            "mismatches": mismatches,
            "unreconciled_orders": unreconciled_orders,
            "adopted_orders_without_ledger": adopted_orders_without_ledger,
            "expiry_pending_orders": expiry_pending_orders,
            "acknowledgements": acknowledgements
                .get("acknowledgements")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "acknowledged_issue_count": acknowledged_issue_count,
            "config_contract": config_contract,
            "pnl_reconciliation": pnl_reconciliation,
            "unrealised_pnl_attribution": unrealised_pnl_attribution,
            "realised_sell_outcomes": realised_sell_outcomes,
            "checks": checks,
            "checked_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }))
    }

    /// Compares recent aggregate snapshots with their immutable position
    /// evidence. Broker-derived unrealised P/L is reported separately because
    /// it can legitimately differ from `market value - local cost basis`.
    pub async fn portfolio_position_snapshot_integrity(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT h.id AS snapshot_id, h.recorded_at,
                        h.position_count AS aggregate_position_count,
                        h.invested_market_value_dkk AS aggregate_market_value_dkk,
                        h.total_cost_basis_dkk AS aggregate_cost_basis_dkk,
                        h.total_unrealised_pnl_dkk AS aggregate_unrealised_pnl_dkk,
                        COUNT(p.snapshot_id) AS detail_position_count,
                        COALESCE(SUM(p.market_value_dkk), 0) AS detail_market_value_dkk,
                        COALESCE(SUM(p.cost_basis_dkk), 0) AS detail_cost_basis_dkk,
                        COALESCE(SUM(p.unrealised_pnl_dkk), 0) AS detail_unrealised_pnl_dkk
                 FROM portfolio_value_history h
                 INNER JOIN portfolio_position_snapshots p ON p.snapshot_id = h.id
                 GROUP BY h.id, h.recorded_at, h.position_count,
                          h.invested_market_value_dkk, h.total_cost_basis_dkk,
                          h.total_unrealised_pnl_dkk
                 ORDER BY h.id DESC
                 LIMIT {PORTFOLIO_POSITION_SNAPSHOT_INTEGRITY_LIMIT}"
            ))
            .await?;
        Ok(portfolio_position_snapshot_integrity_from_rows(&rows))
    }

    pub async fn overview_integrity_acknowledgements_value(&self) -> Result<JsonValue> {
        let saved = self
            .runtime_setting("overview_integrity_acknowledgements")
            .await?;
        let acknowledgements = saved
            .as_ref()
            .and_then(|value| value.get("acknowledgements"))
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                item.get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                    && item
                        .get("issue_key")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "acknowledgements": acknowledgements,
            "updated_at": saved
                .as_ref()
                .and_then(|value| value.get("updated_at"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        }))
    }

    pub async fn save_overview_integrity_acknowledgement(
        &self,
        issue_key: &str,
        code: &str,
        severity: &str,
        enabled: bool,
        notes: &str,
    ) -> Result<JsonValue> {
        let issue_key = issue_key.trim();
        let code = code.trim();
        let severity = severity.trim();
        if issue_key.is_empty() || code.is_empty() || severity.is_empty() {
            anyhow::bail!("Overview integrity acknowledgement requires issue, code, and severity");
        }
        if issue_key.len() > 240 || code.len() > 120 || severity.len() > 40 {
            anyhow::bail!("Overview integrity acknowledgement identifier is too long");
        }
        if notes.len() > 500 {
            anyhow::bail!("Overview integrity acknowledgement notes are too long");
        }
        let existing = self.overview_integrity_acknowledgements_value().await?;
        let mut acknowledgements = existing
            .get("acknowledgements")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.get("issue_key").and_then(JsonValue::as_str) != Some(issue_key))
            .collect::<Vec<_>>();
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        if enabled {
            acknowledgements.push(json!({
                "issue_key": issue_key,
                "code": code,
                "severity": severity,
                "enabled": true,
                "notes": notes,
                "updated_at": updated_at
            }));
        }
        let value = json!({
            "acknowledgements": acknowledgements,
            "updated_at": updated_at
        });
        self.save_runtime_setting("overview_integrity_acknowledgements", &value)
            .await?;
        self.overview_integrity_acknowledgements_value().await
    }
}
