//! Shadow evidence on `AppState`: the missed-trade shadow book and the
//! shadow-report outcome ledger.
//!
//! Record-only. A shadow candidate was not traded and a shadow report queues
//! nothing; these methods record what each would have done, refresh its
//! prices, and measure the outcome at the declared horizons. They cannot
//! create, size or block an order, or reach Saxo.

use crate::state::{
    AppState, MISSED_TRADE_SHADOW_LIMIT, average_or_null, canonical_symbol_key, exchange_code_for,
    fraction_or_null, hermes_counterfactual_quote_metrics, json_text,
    shadow_report_approved_policy_source, shadow_report_outcome_report_is_eligible,
    sql_optional_text,
};
use crate::{
    config::yaml_f64,
    db::{clamp_limit, sql_escape, value_f64, value_i64},
};
use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, HashSet};

const MISSED_TRADE_SHADOW_EVIDENCE_LIMIT: i64 = 200;

const MISSED_TRADE_SHADOW_MIN_COMPLETE_OBSERVATIONS: usize = 20;

/// Gates whose blocks are useful to observe as an unfilled, quote-to-quote
/// shadow. This intentionally excludes technical, Markov, risk exclusions,
/// and instrument quarantine: those are validity failures, not candidates the
/// runtime elected not to deploy because of capital, timing, or capacity.
pub(crate) fn missed_trade_shadow_gate_is_eligible(gate_code: &str) -> bool {
    matches!(
        gate_code,
        "candidate_limit"
            | "market_open"
            | "monthly_loss_breaker"
            | "drawdown_guardrail"
            | "cash_budget"
            | "risk_per_trade"
            | "position_weight"
            | "max_holdings"
            | "concentration"
            | "concentration_exchange"
            | "concentration_currency"
            | "max_selected_assets"
            | "cost_guard"
            | "commission_floor"
            | "minimum_trade_value"
    )
}

#[derive(Default)]
struct MissedTradeShadowOutcomeStats {
    recorded_shadow_count: usize,
    observed_shadow_count: usize,
    directional_return_sum_pct: f64,
    positive_return_count: usize,
}

/// Summarizes quote-to-quote observations for the selected deterministic
/// manager blocks. Each shadow gets equal weight so local-currency P/L is not
/// combined across instruments. This remains an observational diagnostic, not
/// evidence that a gate was wrong or a trading-rule backtest.
pub(crate) fn missed_trade_shadow_outcome_evidence_from_rows(rows: &[JsonValue]) -> JsonValue {
    let mut overall = MissedTradeShadowOutcomeStats {
        recorded_shadow_count: rows.len(),
        ..MissedTradeShadowOutcomeStats::default()
    };
    let mut by_gate: BTreeMap<String, MissedTradeShadowOutcomeStats> = BTreeMap::new();

    for row in rows {
        let Some(directional_return_pct) = row
            .get("estimated_return_pct")
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite())
        else {
            continue;
        };
        let gate = json_text(row, "source_gate");
        let gate = if gate.trim().is_empty() {
            "unknown".to_string()
        } else {
            gate
        };
        let gate_stats = by_gate.entry(gate).or_default();
        for stats in [&mut overall, gate_stats] {
            stats.observed_shadow_count += 1;
            stats.directional_return_sum_pct += directional_return_pct;
            if directional_return_pct > 0.0 {
                stats.positive_return_count += 1;
            }
        }
    }

    let summary = |stats: &MissedTradeShadowOutcomeStats| {
        json!({
            "sample_count": stats.observed_shadow_count,
            "average_directional_return_pct": average_or_null(
                stats.directional_return_sum_pct,
                stats.observed_shadow_count,
            ),
            "positive_return_rate": fraction_or_null(
                stats.positive_return_count,
                stats.observed_shadow_count,
            ),
        })
    };
    let status = if overall.recorded_shadow_count == 0 {
        "no_recorded_shadows"
    } else if overall.observed_shadow_count < MISSED_TRADE_SHADOW_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };
    let by_gate = by_gate
        .into_iter()
        .map(|(source_gate, stats)| {
            json!({
                "source_gate": source_gate,
                "recorded_shadow_count": stats.observed_shadow_count,
                "outcome": summary(&stats),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": status,
        "recorded_shadow_count": overall.recorded_shadow_count,
        "observed_shadow_count": overall.observed_shadow_count,
        "overall": summary(&overall),
        "by_gate": by_gate,
        "minimum_complete_observations": MISSED_TRADE_SHADOW_MIN_COMPLETE_OBSERVATIONS,
        "scan_limit": MISSED_TRADE_SHADOW_EVIDENCE_LIMIT,
        "safety": "read_only_local_quote_to_quote_observations_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "Each observed manager-gate shadow has equal weight. Directional returns are quote-to-quote estimates for the blocked side only; they exclude fees, FX, slippage, tax, broker execution, later position changes, and any claim that a gate should have allowed the trade."
    })
}

/// Forward shadow movement is deliberately measured only from the durable
/// read-only Saxo reference quote to later persisted daily-indicator closes.
/// It is not fill evidence, realised P/L, an execution simulation, or an
/// assertion that the suggestion should have been traded.
pub(crate) fn compact_shadow_report_forward_outcome(
    action: &str,
    reference_price_local: f64,
    reference_at: &str,
    subsequent_closes: &[JsonValue],
) -> JsonValue {
    let action = action.trim().to_ascii_uppercase();
    if !matches!(action.as_str(), "BUY" | "SELL")
        || !reference_price_local.is_finite()
        || reference_price_local <= 0.0
        || reference_at.trim().is_empty()
    {
        return JsonValue::Null;
    }
    let directional_multiplier = if action == "BUY" { 1.0 } else { -1.0 };
    let session_outcome = |session: usize, close: Option<&JsonValue>| {
        let Some(close) = close else {
            return JsonValue::Null;
        };
        let close_local = value_f64(close, "close");
        if !close_local.is_finite() || close_local <= 0.0 {
            return JsonValue::Null;
        }
        let market_return_pct = close_local / reference_price_local - 1.0;
        json!({
            "as_of": json_text(close, "run_date"),
            "session": session,
            "close_local": close_local,
            "market_return_pct": market_return_pct,
            "directional_return_pct": market_return_pct * directional_multiplier,
        })
    };
    let one_session = session_outcome(1, subsequent_closes.first());
    let five_session = session_outcome(5, subsequent_closes.get(4));
    let twenty_session = session_outcome(20, subsequent_closes.get(19));
    let maturity_status = if !twenty_session.is_null() {
        "mature"
    } else if !five_session.is_null() {
        "preliminary"
    } else if !one_session.is_null() {
        "collecting"
    } else {
        "awaiting_daily_close"
    };
    json!({
        "status": maturity_status,
        "evidence_source": "saxo_infoprice_reference_and_daily_indicator_closes",
        "action": action,
        "reference_price_local": reference_price_local,
        "reference_at": reference_at,
        "available_sessions": subsequent_closes.len(),
        "one_session": one_session,
        "five_session": five_session,
        "twenty_session": twenty_session,
        "interpretation": "Directional return compares an observed read-only Saxo reference quote with later persisted daily closes. It excludes FX, commissions, tax, slippage, fills, position changes, and any causal or realised-P/L claim."
    })
}

/// Attach a fixed-reference, estimated round-trip cost to one shadow forward
/// observation. This is intentionally a learning metric, not a fill model:
/// it uses the FX basis and cost schedule captured when the read-only quote
/// arrived and never substitutes current FX, broker fees, or execution data.
fn compact_shadow_report_after_cost_outcome(
    forward_outcome: &JsonValue,
    estimated_cost: &JsonValue,
) -> JsonValue {
    if forward_outcome.is_null() {
        return JsonValue::Null;
    }
    if json_text(estimated_cost, "status") != "available" {
        return json!({
            "status": "unavailable",
            "reason": json_text(estimated_cost, "reason"),
            "interpretation": "No after-cost estimate is produced without a fresh FX basis captured with the shadow reference quote. This is not realised P/L."
        });
    }
    let directional_return_pct = value_f64(forward_outcome, "directional_return_pct");
    let reference_notional_dkk = value_f64(estimated_cost, "reference_notional_dkk");
    let estimated_round_trip_cost_dkk = value_f64(estimated_cost, "estimated_round_trip_cost_dkk");
    if !directional_return_pct.is_finite()
        || !reference_notional_dkk.is_finite()
        || reference_notional_dkk <= 0.0
        || !estimated_round_trip_cost_dkk.is_finite()
        || estimated_round_trip_cost_dkk < 0.0
    {
        return json!({
            "status": "unavailable",
            "reason": "non_finite_reference_cost_basis",
            "interpretation": "No after-cost estimate is produced when the captured cost basis is not finite. This is not realised P/L."
        });
    }
    let gross_directional_pnl_dkk = directional_return_pct * reference_notional_dkk;
    let estimated_after_cost_pnl_dkk = gross_directional_pnl_dkk - estimated_round_trip_cost_dkk;
    json!({
        "status": "estimated",
        "as_of": json_text(forward_outcome, "as_of"),
        "session": value_i64(forward_outcome, "session"),
        "directional_return_pct": directional_return_pct,
        "reference_notional_dkk": reference_notional_dkk,
        "gross_directional_pnl_dkk": gross_directional_pnl_dkk,
        "estimated_round_trip_cost_dkk": estimated_round_trip_cost_dkk,
        "estimated_after_cost_pnl_dkk": estimated_after_cost_pnl_dkk,
        "estimated_after_cost_return_pct": estimated_after_cost_pnl_dkk / reference_notional_dkk,
        "cost_basis": estimated_cost,
        "interpretation": "Estimated post-cost directional outcome using the fixed FX basis, exchange minimum commission schedule, and configured slippage captured with the read-only Saxo reference quote. It excludes actual fills, actual commissions, tax, currency movement after reference, and position changes; it is not realised P/L or an execution simulation."
    })
}

/// Observed intraday excursion over the shadow-specific quote trail. This is
/// intentionally not an exchange high/low: only sampled, persisted Saxo
/// infoprices count, so gaps and monitor downtime remain visible in coverage.
fn compact_shadow_report_observed_excursions(
    action: &str,
    reference_price_local: f64,
    quotes: &[JsonValue],
) -> JsonValue {
    let action = action.trim().to_ascii_uppercase();
    if !matches!(action.as_str(), "BUY" | "SELL")
        || !reference_price_local.is_finite()
        || reference_price_local <= 0.0
    {
        return json!({
            "status": "unavailable",
            "reason": "invalid_action_or_reference_price",
        });
    }
    let mut observed = quotes
        .iter()
        .filter_map(|quote| {
            let price_local = value_f64(quote, "price_local");
            (price_local.is_finite() && price_local > 0.0).then(|| {
                let market_return_pct = price_local / reference_price_local - 1.0;
                let directional_return_pct = if action == "BUY" {
                    market_return_pct
                } else {
                    -market_return_pct
                };
                json!({
                    "observed_at": json_text(quote, "observed_at"),
                    "price_local": price_local,
                    "directional_return_pct": directional_return_pct,
                })
            })
        })
        .collect::<Vec<_>>();
    if observed.is_empty() {
        return json!({
            "status": "unavailable",
            "reason": "no_persisted_intraday_quotes",
            "interpretation": "No excursion is inferred without persisted quote samples.",
        });
    }
    observed.sort_by(|left, right| {
        json_text(left, "observed_at").cmp(&json_text(right, "observed_at"))
    });
    let favourable = observed
        .iter()
        .max_by(|left, right| {
            value_f64(left, "directional_return_pct")
                .partial_cmp(&value_f64(right, "directional_return_pct"))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
        .unwrap_or(JsonValue::Null);
    let adverse = observed
        .iter()
        .min_by(|left, right| {
            value_f64(left, "directional_return_pct")
                .partial_cmp(&value_f64(right, "directional_return_pct"))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
        .unwrap_or(JsonValue::Null);
    json!({
        "status": if observed.len() >= 2 { "observed" } else { "collecting" },
        "evidence_source": "persisted_shadow_saxo_infoprice_samples",
        "action": action,
        "reference_price_local": reference_price_local,
        "sample_count": observed.len(),
        "first_observed_at": json_text(observed.first().unwrap_or(&JsonValue::Null), "observed_at"),
        "last_observed_at": json_text(observed.last().unwrap_or(&JsonValue::Null), "observed_at"),
        "maximum_favourable_observed": favourable,
        "maximum_adverse_observed": adverse,
        "coverage": "sampled_quotes_only_not_continuous_intraday_high_low_coverage",
        "interpretation": "Maximum favourable/adverse excursion is calculated only from the retained read-only Saxo quote samples after reference. It does not claim the market's true intraday high/low, a fill path, realised P/L, or execution quality."
    })
}

fn compact_shadow_report_reference_cost_estimate(
    symbol: &str,
    currency: &str,
    proposed_quantity: f64,
    reference_price_local: f64,
    fx_basis: &JsonValue,
    config: &YamlValue,
) -> JsonValue {
    let currency = currency.trim().to_ascii_uppercase();
    if json_text(fx_basis, "status") != "available" {
        return json!({
            "status": "unavailable",
            "reason": json_text(fx_basis, "reason"),
            "currency": currency,
            "fx_basis": fx_basis,
        });
    }
    let rate_to_dkk = value_f64(fx_basis, "rate_to_dkk");
    if currency.is_empty()
        || !proposed_quantity.is_finite()
        || proposed_quantity <= 0.0
        || !reference_price_local.is_finite()
        || reference_price_local <= 0.0
        || !rate_to_dkk.is_finite()
        || rate_to_dkk <= 0.0
    {
        return json!({
            "status": "unavailable",
            "reason": "invalid_reference_notional_or_fx_basis",
            "currency": currency,
            "fx_basis": fx_basis,
        });
    }
    let exchange = exchange_code_for(symbol);
    let (minimum_commission_local, commission_currency) =
        crate::saxo_order::min_commission_local_for_exchange(&exchange);
    if !commission_currency.eq_ignore_ascii_case(&currency) {
        return json!({
            "status": "unavailable",
            "reason": "commission_currency_differs_from_reference_currency",
            "currency": currency,
            "commission_currency": commission_currency,
            "fx_basis": fx_basis,
        });
    }
    let reference_notional_dkk = proposed_quantity * reference_price_local * rate_to_dkk;
    let one_way_commission_dkk = minimum_commission_local * rate_to_dkk;
    let estimated_slippage_bps = yaml_f64(config, &["strategy", "estimated_slippage_bps"])
        .unwrap_or(8.0)
        .max(0.0);
    // Same rule as the manager's cost guard, deliberately not the same
    // round trip: see `trading_manager::one_way_slippage_dkk`.
    let one_way_slippage_dkk = crate::trading_manager::one_way_slippage_dkk(
        reference_notional_dkk,
        estimated_slippage_bps,
    );
    let estimated_round_trip_cost_dkk = 2.0 * (one_way_commission_dkk + one_way_slippage_dkk);
    if !reference_notional_dkk.is_finite()
        || !one_way_commission_dkk.is_finite()
        || !one_way_slippage_dkk.is_finite()
        || !estimated_round_trip_cost_dkk.is_finite()
    {
        return json!({
            "status": "unavailable",
            "reason": "non_finite_cost_estimate",
            "currency": currency,
            "fx_basis": fx_basis,
        });
    }
    json!({
        "status": "available",
        "method": "fixed_reference_fx_exchange_minimum_commission_and_configured_round_trip_slippage",
        "currency": currency,
        "exchange": exchange,
        "proposed_quantity": proposed_quantity,
        "reference_price_local": reference_price_local,
        "reference_notional_dkk": reference_notional_dkk,
        "fx_basis": fx_basis,
        "minimum_commission_local_per_side": minimum_commission_local,
        "commission_currency": commission_currency,
        "one_way_minimum_commission_dkk": one_way_commission_dkk,
        "estimated_slippage_bps_per_side": estimated_slippage_bps,
        "one_way_slippage_dkk": one_way_slippage_dkk,
        "estimated_round_trip_cost_dkk": estimated_round_trip_cost_dkk,
        "limitations": "Minimum commission and configured slippage are estimates only. No broker precheck, order, fill, fee, tax, or later FX conversion is used."
    })
}

fn shadow_opening_pulse_prefix(pulse_key: &str) -> Option<&'static str> {
    if pulse_key.starts_with("europe_mid_session_shadow:") {
        Some("europe_open_followup:")
    } else if pulse_key.starts_with("us_mid_session_shadow:") {
        Some("us_open_followup:")
    } else {
        None
    }
}

fn shadow_report_suggested_symbol_keys(report: &JsonValue) -> HashSet<String> {
    report
        .get("suggested_trades")
        .or_else(|| {
            report
                .get("strategy_plan")
                .and_then(|plan| plan.get("suggested_trades"))
        })
        .and_then(JsonValue::as_array)
        .map(|trades| {
            trades
                .iter()
                .map(|trade| json_text(trade, "symbol"))
                .filter(|symbol| !symbol.is_empty())
                .map(|symbol| canonical_symbol_key(&symbol))
                .collect()
        })
        .unwrap_or_default()
}

/// Decode the exact persisted decision-time prompt, but expose only a later
/// allowlisted projection. The raw prompt can contain lengthy provider-facing
/// context and must never become a general-purpose shadow-ledger payload.
fn shadow_report_prompt_user_context(report: &JsonValue) -> JsonValue {
    let prompt = report
        .get("prompt_text")
        .and_then(JsonValue::as_str)
        .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok());
    prompt
        .and_then(|prompt| prompt.get("user").cloned())
        .filter(JsonValue::is_object)
        .unwrap_or_else(|| {
            json!({
                "status": "unavailable",
                "reason": "persisted_decision_time_prompt_unavailable",
            })
        })
}

fn shadow_prompt_matching_symbol_row(
    context: &JsonValue,
    section: &str,
    symbol: &str,
) -> JsonValue {
    let Some(rows) = context
        .get(section)
        .and_then(|section| section.get("signals"))
        .and_then(JsonValue::as_array)
    else {
        return JsonValue::Null;
    };
    rows.iter()
        .find(|row| canonical_symbol_key(&json_text(row, "symbol")) == canonical_symbol_key(symbol))
        .cloned()
        .unwrap_or(JsonValue::Null)
}

fn compact_shadow_prompt_indicator_snapshot(context: &JsonValue, symbol: &str) -> JsonValue {
    let row = shadow_prompt_matching_symbol_row(context, "daily_indicators", symbol);
    if row.is_null() {
        return json!({"status": "not_available_at_report_time"});
    }
    json!({
        "status": "available",
        "source_run": context.get("daily_indicators").and_then(|value| value.get("latest_run")).cloned().unwrap_or(JsonValue::Null),
        "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
        "close": row.get("close").cloned().unwrap_or(JsonValue::Null),
        "currency": row.get("currency").cloned().unwrap_or(JsonValue::Null),
        "trend_bias": row.get("trend_bias").cloned().unwrap_or(JsonValue::Null),
        "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
        "confluence_count": row.get("confluence_count").cloned().unwrap_or(JsonValue::Null),
        "min_confluences": row.get("min_confluences").cloned().unwrap_or(JsonValue::Null),
        "support_risk": row.get("support").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_shadow_prompt_markov_snapshot(context: &JsonValue, symbol: &str) -> JsonValue {
    let row = shadow_prompt_matching_symbol_row(context, "markov_method", symbol);
    if row.is_null() {
        return json!({"status": "not_available_at_report_time"});
    }
    json!({
        "status": "available",
        "source_run": context.get("markov_method").and_then(|value| value.get("latest_run")).cloned().unwrap_or(JsonValue::Null),
        "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
        "run_date": row.get("run_date").cloned().unwrap_or(JsonValue::Null),
        "state": row.get("state").cloned().unwrap_or(JsonValue::Null),
        "signed_signal": row.get("signed_signal").cloned().unwrap_or(JsonValue::Null),
        "direction": row.get("direction").cloned().unwrap_or(JsonValue::Null),
        "conviction": row.get("conviction").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_shadow_prompt_quiver_snapshot(context: &JsonValue, symbol: &str) -> JsonValue {
    let source = context
        .get("quiver_signals")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let row = shadow_prompt_matching_symbol_row(context, "quiver_signals", symbol);
    if row.is_null() {
        return json!({
            "status": "not_available_at_report_time",
            "freshness": source.get("freshness").cloned().unwrap_or(JsonValue::Null),
        });
    }
    json!({
        "status": "available",
        "freshness": source.get("freshness").cloned().unwrap_or(JsonValue::Null),
        "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
        "run_date": row.get("run_date").cloned().unwrap_or(JsonValue::Null),
        "signal": row.get("signal").cloned().unwrap_or(JsonValue::Null),
        "direction": row.get("direction").cloned().unwrap_or(JsonValue::Null),
        "confidence": row.get("confidence").cloned().unwrap_or(JsonValue::Null),
        "event_count": row.get("event_count").cloned().unwrap_or(JsonValue::Null),
        "latest_event_date": row.get("latest_event_date").cloned().unwrap_or(JsonValue::Null),
        "safety": "advisory_prompt_snapshot_only_not_a_gate_or_execution_input",
    })
}

fn compact_shadow_prompt_concentration_snapshot(
    context: &JsonValue,
    symbol: &str,
    currency: Option<&str>,
) -> JsonValue {
    let target_symbol = canonical_symbol_key(symbol);
    let target_exchange = exchange_code_for(symbol);
    let target_currency = currency
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_ascii_uppercase())
        .or_else(|| crate::saxo_order::currency_for_exchange(&target_exchange).map(str::to_string));
    let positions = context
        .get("positions")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let held = positions
        .iter()
        .filter(|position| value_f64(position, "quantity") > 0.0)
        .collect::<Vec<_>>();
    let same_symbol = held
        .iter()
        .filter(|position| canonical_symbol_key(&json_text(position, "symbol")) == target_symbol)
        .count();
    let same_exchange = held
        .iter()
        .filter(|position| exchange_code_for(&json_text(position, "symbol")) == target_exchange)
        .count();
    let same_currency = target_currency.as_ref().map(|currency| {
        held.iter()
            .filter(|position| {
                let position_currency = json_text(position, "currency");
                let fallback_currency = crate::saxo_order::currency_for_exchange(
                    &exchange_code_for(&json_text(position, "symbol")),
                );
                position_currency.eq_ignore_ascii_case(currency)
                    || fallback_currency.is_some_and(|value| value.eq_ignore_ascii_case(currency))
            })
            .count()
    });
    json!({
        "status": if context.get("positions").is_some() { "available" } else { "unavailable" },
        "scope": "decision_prompt_market_scope_positions_not_manager_concentration_gate",
        "held_position_count": held.len(),
        "matching_symbol_position_count": same_symbol,
        "same_exchange_held_position_count": same_exchange,
        "same_currency_held_position_count": same_currency,
        "target_exchange": target_exchange,
        "target_currency": target_currency,
    })
}

fn compact_shadow_prompt_thesis_snapshot(context: &JsonValue, report: &JsonValue) -> JsonValue {
    let baseline = context
        .get("active_strategy_baseline")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(JsonValue::Null);
    json!({
        "status": if baseline.is_null() { "not_available_at_report_time" } else { "available" },
        "strategy_baseline_id": report.get("strategy_baseline_id").cloned().unwrap_or(JsonValue::Null),
        "active_baseline": if baseline.is_null() { JsonValue::Null } else { json!({
            "id": baseline.get("id").cloned().unwrap_or(JsonValue::Null),
            "status": baseline.get("status").cloned().unwrap_or(JsonValue::Null),
            "goal_version": baseline.get("goal_version").cloned().unwrap_or(JsonValue::Null),
            "activated_at": baseline.get("activated_at").cloned().unwrap_or(JsonValue::Null),
        }) },
        "candidate_entry_thesis": "not_available_pre_trade",
        "interpretation": "The report-time approved strategy baseline is preserved where available. A candidate has no execution-entry thesis before a manager-approved order, so none is inferred.",
    })
}

/// A deliberately bounded, decision-time-only signal-gate projection for a
/// shadow candidate. It shares the technical/Markov rule shape with the
/// manager, but cannot honestly recreate its current market, cash, position,
/// risk, concentration, cost, sellability, or Hermes gates from a saved
/// prompt. Those omitted checks remain explicit so a signal clear is never
/// mistaken for a queue approval.
pub(crate) fn compact_shadow_report_decision_time_signal_gate(
    action: &str,
    report_date: &str,
    context: &JsonValue,
) -> JsonValue {
    let action = action.trim().to_ascii_uppercase();
    let technical = context
        .get("technical_snapshot")
        .unwrap_or(&JsonValue::Null);
    let technical_available = json_text(technical, "status") == "available";
    let signal_result = |status: &str, gate_code: &str, stage: &str, reason: &str| {
        json!({
            "status": status,
            "gate_code": gate_code,
            "stage": stage,
            "reason": reason,
            "evidence_source": "persisted_decision_time_prompt_projection",
            "coverage": "technical_and_markov_signal_rules_only",
            "omitted_manager_gates": [
                "market_open", "risk_exclusion", "instrument_quarantine",
                "order_shape", "monthly_loss_breaker", "drawdown_guardrail",
                "cash_budget", "minimum_trade_value", "sellable_quantity",
                "risk_per_trade", "max_holdings", "concentration",
                "position_weight", "cost_guard", "max_selected_assets",
                "hermes_advice"
            ],
            "interpretation": "This is a bounded replay over server-generated decision-time technical and Markov prompt snapshots. It is not a full Trading Manager evaluation, queue approval, broker precheck, execution simulation, Hermes effect, or Saxo action."
        })
    };
    if !matches!(action.as_str(), "BUY" | "SELL") {
        return signal_result(
            "insufficient_decision_time_evidence",
            "not_evaluated_shadow",
            "not_evaluated",
            "The candidate action is not a supported BUY or SELL.",
        );
    }

    if technical_available {
        let sentiment = json_text(technical, "sentiment").to_ascii_uppercase();
        let trend_bias = json_text(technical, "trend_bias").to_ascii_lowercase();
        let confluences = value_i64(technical, "confluence_count");
        let minimum = value_i64(technical, "min_confluences").max(1);
        let technical_clear = match action.as_str() {
            "BUY" => {
                matches!(sentiment.as_str(), "BUY" | "OVERWEIGHT")
                    && trend_bias == "bullish"
                    && confluences >= minimum
            }
            "SELL" => {
                matches!(sentiment.as_str(), "SELL" | "UNDERWEIGHT") || trend_bias == "bearish"
            }
            _ => false,
        };
        if technical_clear {
            return signal_result(
                "clear_signal_gate_only",
                "technical",
                "technical",
                "The saved daily-indicator snapshot supports the candidate direction.",
            );
        }
        if action == "SELL" {
            return signal_result(
                "blocked_signal_gate",
                "technical",
                "technical",
                "The saved daily-indicator snapshot does not support the SELL direction; flatten risk-off fallback evidence is not reconstructed from the prompt.",
            );
        }
    }

    if action == "BUY" {
        let policy = context
            .get("signal_gate_policy")
            .and_then(|value| value.get("markov_starter"))
            .unwrap_or(&JsonValue::Null);
        let markov = context.get("markov_snapshot").unwrap_or(&JsonValue::Null);
        if policy.get("enabled").and_then(JsonValue::as_bool) != Some(true)
            || json_text(markov, "status") != "available"
        {
            return signal_result(
                "insufficient_decision_time_evidence",
                "not_evaluated_shadow",
                "markov",
                "The saved prompt does not contain an enabled Markov starter-policy snapshot and an available signal needed to evaluate the technical fallback.",
            );
        }
        let Some(report_date) = NaiveDate::parse_from_str(report_date, "%Y-%m-%d").ok() else {
            return signal_result(
                "insufficient_decision_time_evidence",
                "not_evaluated_shadow",
                "markov",
                "The shadow report has no parsable local report date for Markov freshness.",
            );
        };
        let Some(run_date) = json_text(markov, "run_date")
            .split('T')
            .next()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        else {
            return signal_result(
                "insufficient_decision_time_evidence",
                "not_evaluated_shadow",
                "markov",
                "The saved Markov signal has no parsable run date.",
            );
        };
        let max_age_days = value_i64(policy, "max_signal_age_days").max(1);
        let age_days = (report_date - run_date).num_days();
        let min_signed_signal = value_f64(policy, "min_signed_signal");
        let signed_signal = value_f64(markov, "signed_signal");
        let direction = json_text(markov, "direction");
        if age_days >= 0
            && age_days <= max_age_days
            && direction == "long"
            && signed_signal >= min_signed_signal
        {
            return signal_result(
                "clear_signal_gate_only",
                "markov",
                "markov_starter_fallback",
                "The saved fresh Markov long signal clears the technical-fallback signal rule; current manager starter, value, position, and risk checks remain omitted.",
            );
        }
        return signal_result(
            "blocked_signal_gate",
            "markov",
            "markov_starter_fallback",
            "The saved Markov signal does not satisfy the decision-time starter fallback rule.",
        );
    }

    signal_result(
        "insufficient_decision_time_evidence",
        "not_evaluated_shadow",
        "technical",
        "No saved daily-indicator snapshot is available to evaluate this SELL candidate.",
    )
}

/// Compact, persisted-safe shadow candidates. All context comes from the
/// report's saved decision-time prompt or normalized report, never from a
/// later live query; this prevents hindsight from leaking into learning data.
pub(crate) fn shadow_report_outcome_candidates(
    report: &JsonValue,
    earlier_symbols: &HashSet<String>,
    prompt_context: &JsonValue,
    report_date: &str,
) -> Vec<JsonValue> {
    let suggested_trades = report
        .get("suggested_trades")
        .or_else(|| {
            report
                .get("strategy_plan")
                .and_then(|plan| plan.get("suggested_trades"))
        })
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let capital_plan = report
        .get("capital_plan")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let change_status = report
        .get("shadow_change_assessment")
        .and_then(|assessment| assessment.get("status"))
        .cloned()
        .unwrap_or(JsonValue::Null);
    suggested_trades
        .into_iter()
        .enumerate()
        .filter_map(|(index, trade)| {
            let symbol = json_text(&trade, "symbol");
            let action = json_text(&trade, "action").to_ascii_uppercase();
            let proposed_quantity = value_f64(&trade, "quantity");
            if symbol.is_empty()
                || !matches!(action.as_str(), "BUY" | "SELL")
                || !proposed_quantity.is_finite()
                || proposed_quantity <= 0.0
            {
                return None;
            }
            let currency = trade
                .get("currency")
                .and_then(JsonValue::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .or_else(|| {
                    symbol
                        .split_once(':')
                        .and_then(|(_, exchange)| crate::saxo_order::currency_for_exchange(exchange))
                        .map(str::to_string)
                });
            let reported_reference_price_local = trade
                .get("limit_price_local")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0);
            let strategy_metadata = trade
                .get("strategy_metadata")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let currency_text = currency.as_deref().unwrap_or_default();
            let report_time_context = json!({
                "capture_version": "phase3_decision_time_prompt_projection_v2",
                "strategy_key": trade.get("strategy_key").cloned().unwrap_or(JsonValue::Null),
                "strategy_role": trade.get("strategy_role").cloned().unwrap_or(JsonValue::Null),
                "estimated_value_dkk": trade.get("estimated_value_dkk").cloned().unwrap_or(JsonValue::Null),
                "technical_reported": strategy_metadata.get("technical").cloned().unwrap_or(JsonValue::Null),
                "technical_snapshot": compact_shadow_prompt_indicator_snapshot(prompt_context, &symbol),
                "markov_reported": strategy_metadata.get("markov").cloned().unwrap_or(JsonValue::Null),
                "markov_snapshot": compact_shadow_prompt_markov_snapshot(prompt_context, &symbol),
                "signal_gate_policy": prompt_context.get("decision_time_gate_policy").cloned().unwrap_or(JsonValue::Null),
                "quiver_snapshot": compact_shadow_prompt_quiver_snapshot(prompt_context, &symbol),
                "support_risk_snapshot": compact_shadow_prompt_indicator_snapshot(prompt_context, &symbol).get("support_risk").cloned().unwrap_or(JsonValue::Null),
                "cash_snapshot": {
                    "reported_cash_balance_dkk": capital_plan.get("cash_balance_dkk").cloned().unwrap_or(JsonValue::Null),
                    "reported_available_buy_budget_dkk": capital_plan.get("available_buy_budget_dkk").cloned().unwrap_or(JsonValue::Null),
                    "decision_time_capital_plan": prompt_context.get("capital_plan").cloned().unwrap_or(JsonValue::Null),
                },
                "concentration_snapshot": compact_shadow_prompt_concentration_snapshot(prompt_context, &symbol, Some(currency_text)),
                "thesis_snapshot": compact_shadow_prompt_thesis_snapshot(prompt_context, report),
                "shadow_change_status": change_status,
                "prompt_context_status": prompt_context.get("status").cloned().unwrap_or(JsonValue::String("available".to_string())),
            });
            let deterministic_gate = compact_shadow_report_decision_time_signal_gate(
                &action,
                report_date,
                &report_time_context,
            );
            Some(json!({
                "candidate_rank": index as i64 + 1,
                "symbol": symbol,
                "action": action,
                "proposed_quantity": proposed_quantity,
                "currency": currency,
                "reported_reference_price_local": reported_reference_price_local,
                "appeared_in_earlier_pulse": earlier_symbols.contains(&canonical_symbol_key(&symbol)),
                "report_time_context": report_time_context,
                "deterministic_gate": deterministic_gate,
            }))
        })
        .collect()
}

impl AppState {
    /// Records selected deterministic manager blocks as quote-to-quote
    /// observations. It never re-opens a gate, makes a provider/Saxo call, or
    /// creates an order. The candidate is an observed missed opportunity, not
    /// evidence that the skipped trade should have been placed.
    pub async fn record_missed_trade_shadows(
        &self,
        report_id: i64,
        manager_run_id: i64,
        skipped_candidates: &[JsonValue],
    ) -> Result<JsonValue> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut created = 0usize;
        let mut awaiting_reference = 0usize;
        let mut skipped = 0usize;

        for (index, candidate) in skipped_candidates.iter().enumerate() {
            let gate_code = json_text(candidate, "gate_code");
            if !missed_trade_shadow_gate_is_eligible(&gate_code) {
                skipped += 1;
                continue;
            }
            let strategy_key = json_text(candidate, "strategy_key");
            let symbol = json_text(candidate, "symbol");
            let action = json_text(candidate, "action").to_uppercase();
            let shadow_quantity = value_f64(candidate, "quantity");
            if strategy_key.trim().is_empty()
                || symbol.trim().is_empty()
                || !matches!(action.as_str(), "BUY" | "SELL")
                || !shadow_quantity.is_finite()
                || shadow_quantity <= 0.0
            {
                skipped += 1;
                continue;
            }
            let reported_reference_price_local = candidate
                .get("reference_price_local")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0);
            awaiting_reference += 1;
            let reported_reference_sql = reported_reference_price_local
                .map(|value| value.to_string())
                .unwrap_or_else(|| "NULL".to_string());
            let currency = json_text(candidate, "currency");
            let id = format!("missed-trade-shadow-{manager_run_id}-{index}");
            let result = sqlx::query(&format!(
                "INSERT INTO missed_trade_shadows (
                    id, created_at, updated_at, report_id, manager_run_id,
                    strategy_key, symbol, action, source_gate, shadow_quantity,
                    reference_price_local, reference_price_at, reference_price_source,
                    reported_reference_price_local, currency, status, observation_count
                ) VALUES (
                    '{}', '{}', '{}', {}, {}, '{}', '{}', '{}', '{}', {}, NULL, NULL, 'awaiting_saxo_infoprice', {}, {}, 'awaiting_reference', 0
                ) ON CONFLICT (manager_run_id, strategy_key) DO NOTHING",
                sql_escape(&id),
                sql_escape(&now),
                sql_escape(&now),
                report_id,
                manager_run_id,
                sql_escape(&strategy_key),
                sql_escape(&symbol),
                sql_escape(&action),
                sql_escape(&gate_code),
                shadow_quantity,
                reported_reference_sql,
                sql_optional_text(Some(&currency)),
            ))
            .execute(&self.pool)
            .await
            .context("recording missed-trade shadow")?;
            created += result.rows_affected() as usize;
        }

        Ok(json!({
            "status": "ok",
            "created": created,
            "awaiting_reference": awaiting_reference,
            "skipped": skipped,
            "safety": "quote_to_quote_observation_only_no_gate_or_order_mutation",
        }))
    }

    pub async fn missed_trade_shadows(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, updated_at, report_id, manager_run_id, strategy_key,
                    symbol, action, source_gate, shadow_quantity, reference_price_local,
                    reference_price_at, reference_price_source, reported_reference_price_local,
                    currency, status, latest_price_local, latest_price_at,
                    estimated_return_pct, estimated_pnl_local, observation_count
             FROM missed_trade_shadows
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, MISSED_TRADE_SHADOW_LIMIT)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub(crate) async fn missed_trade_shadow_outcome_evidence(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT source_gate, estimated_return_pct
                 FROM missed_trade_shadows
                 WHERE reference_price_source = 'saxo_infoprices'
                 ORDER BY created_at DESC, id DESC
                 LIMIT {}",
                MISSED_TRADE_SHADOW_EVIDENCE_LIMIT
            ))
            .await?;
        Ok(missed_trade_shadow_outcome_evidence_from_rows(&rows))
    }

    pub async fn active_missed_trade_shadow_symbols(&self) -> Result<Vec<String>> {
        let rows = self
            .select_json(
                "SELECT DISTINCT symbol
                 FROM missed_trade_shadows
                 WHERE status IN ('tracking', 'awaiting_reference')
                 ORDER BY symbol",
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| json_text(row, "symbol"))
            .filter(|symbol| !symbol.trim().is_empty())
            .collect())
    }

    pub async fn refresh_missed_trade_shadow_price(
        &self,
        symbol: &str,
        latest_price_local: f64,
        observed_at: &str,
    ) -> Result<usize> {
        if symbol.trim().is_empty() || !latest_price_local.is_finite() || latest_price_local <= 0.0
        {
            return Ok(0);
        }
        let rows = self
            .select_json(&format!(
                "SELECT id, action, shadow_quantity, reference_price_local, status
                 FROM missed_trade_shadows
                 WHERE symbol = '{}' AND status IN ('tracking', 'awaiting_reference')",
                sql_escape(symbol)
            ))
            .await?;
        let mut updated = 0usize;
        for row in rows {
            let id = json_text(&row, "id");
            if json_text(&row, "status") == "awaiting_reference" {
                let result = sqlx::query(&format!(
                    "UPDATE missed_trade_shadows
                     SET updated_at = '{}', reference_price_local = {},
                         reference_price_at = '{}', reference_price_source = 'saxo_infoprices',
                         status = 'tracking'
                     WHERE id = '{}' AND status = 'awaiting_reference'",
                    sql_escape(observed_at),
                    latest_price_local,
                    sql_escape(observed_at),
                    sql_escape(&id),
                ))
                .execute(&self.pool)
                .await
                .context("capturing missed-trade shadow reference quote")?;
                updated += result.rows_affected() as usize;
                continue;
            }
            let Some((estimated_return_pct, estimated_pnl_local)) =
                hermes_counterfactual_quote_metrics(
                    &json_text(&row, "action"),
                    value_f64(&row, "shadow_quantity"),
                    value_f64(&row, "reference_price_local"),
                    latest_price_local,
                )
            else {
                continue;
            };
            let result = sqlx::query(&format!(
                "UPDATE missed_trade_shadows
                 SET updated_at = '{}', latest_price_local = {}, latest_price_at = '{}',
                     estimated_return_pct = {}, estimated_pnl_local = {},
                     observation_count = observation_count + 1
                 WHERE id = '{}'",
                sql_escape(observed_at),
                latest_price_local,
                sql_escape(observed_at),
                estimated_return_pct,
                estimated_pnl_local,
                sql_escape(&id)
            ))
            .execute(&self.pool)
            .await
            .context("updating missed-trade shadow quote")?;
            updated += result.rows_affected() as usize;
        }
        Ok(updated)
    }

    /// Persist the provider-selected candidates from a server-owned shadow
    /// pulse as observations. This is intentionally narrower than the normal
    /// Trading Manager path: it only records report context and waits for a
    /// read-only Saxo quote; it neither evaluates a gate nor creates a queue
    /// row, Hermes request, Saxo order precheck, or Saxo order mutation.
    pub async fn record_shadow_report_outcomes(
        &self,
        report_id: i64,
        report_json: &JsonValue,
    ) -> Result<JsonValue> {
        let report = self
            .first_json(&format!(
                "SELECT id, report_date, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible, prompt_text\n                 FROM decision_reports\n                 WHERE id = {}\n                 LIMIT 1",
                report_id.max(0)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        if !shadow_report_outcome_report_is_eligible(&report) {
            return Ok(json!({
                "status": "not_shadow_report",
                "created": 0,
                "safety": "no_gate_hermes_queue_or_saxo_order_authority",
            }));
        }

        let opening_prefix = shadow_opening_pulse_prefix(&json_text(&report, "analysis_pulse_key"));
        let earlier_report = match opening_prefix {
            Some(prefix) => self
                .first_json(&format!(
                    "SELECT id, report_json\n                     FROM decision_reports\n                     WHERE report_date = '{}'\n                       AND analysis_pulse_key LIKE '{}%'\n                       AND status IN ('completed', 'xai_fallback')\n                     ORDER BY created_at DESC, id DESC\n                     LIMIT 1",
                    sql_escape(&json_text(&report, "report_date")),
                    sql_escape(prefix),
                ))
                .await?
                .unwrap_or(JsonValue::Null),
            None => JsonValue::Null,
        };
        let earlier_symbols = shadow_report_suggested_symbol_keys(
            earlier_report
                .get("report_json")
                .unwrap_or(&JsonValue::Null),
        );
        let earlier_report_id = (!earlier_report.is_null())
            .then(|| value_i64(&earlier_report, "id"))
            .filter(|id| *id > 0);
        let prompt_context = shadow_report_prompt_user_context(&report);
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut created = 0usize;
        let mut skipped = 0usize;

        for candidate in shadow_report_outcome_candidates(
            report_json,
            &earlier_symbols,
            &prompt_context,
            &json_text(&report, "report_date"),
        ) {
            let candidate_rank = value_i64(&candidate, "candidate_rank");
            let symbol = json_text(&candidate, "symbol");
            let action = json_text(&candidate, "action");
            let proposed_quantity = value_f64(&candidate, "proposed_quantity");
            if candidate_rank <= 0
                || symbol.is_empty()
                || !matches!(action.as_str(), "BUY" | "SELL")
                || !proposed_quantity.is_finite()
                || proposed_quantity <= 0.0
            {
                skipped += 1;
                continue;
            }
            let reported_reference_sql = candidate
                .get("reported_reference_price_local")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "NULL".to_string());
            let context_json = candidate
                .get("report_time_context")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let deterministic_gate =
                candidate
                    .get("deterministic_gate")
                    .cloned()
                    .unwrap_or_else(|| {
                        compact_shadow_report_decision_time_signal_gate(
                            &action,
                            &json_text(&report, "report_date"),
                            &context_json,
                        )
                    });
            let deterministic_gate_code = json_text(&deterministic_gate, "gate_code");
            let approved_policy_source = shadow_report_approved_policy_source(&context_json);
            let id = format!("shadow-report-outcome-{report_id}-{candidate_rank}");
            let result = sqlx::query(&format!(
                "INSERT INTO shadow_report_outcomes (\n                    id, created_at, updated_at, report_id, analysis_pulse_key, analysis_pulse_label,\n                    candidate_rank, symbol, action, proposed_quantity, currency,\n                    earlier_pulse_report_id, appeared_in_earlier_pulse,\n                    deterministic_gate_code, deterministic_gate_json, hermes_effect, hermes_advice_snapshot_json, approved_policy_source,\n                    report_time_context_json, reported_reference_price_local,\n                    reference_price_local, reference_price_at, reference_price_source, reference_dkk_basis,\n                    status, observation_count\n                ) VALUES (\n                    '{}', '{}', '{}', {}, '{}', '{}',\n                    {}, '{}', '{}', {}, {},\n                    {}, {},\n                    '{}', '{}', 'not_requested_shadow', NULL, {},\n                    '{}', {},\n                    NULL, NULL, 'awaiting_saxo_infoprice', 'not_captured_phase3_initial',\n                    'awaiting_reference', 0\n                ) ON CONFLICT (report_id, candidate_rank) DO NOTHING",
                sql_escape(&id),
                sql_escape(&now),
                sql_escape(&now),
                report_id,
                sql_escape(&json_text(&report, "analysis_pulse_key")),
                sql_escape(&json_text(&report, "analysis_pulse_label")),
                candidate_rank,
                sql_escape(&symbol),
                sql_escape(&action),
                proposed_quantity,
                sql_optional_text(candidate.get("currency").and_then(JsonValue::as_str)),
                earlier_report_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "NULL".to_string()),
                if candidate
                    .get("appeared_in_earlier_pulse")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                {
                    1
                } else {
                    0
                },
                sql_escape(if deterministic_gate_code.is_empty() {
                    "not_evaluated_shadow"
                } else {
                    &deterministic_gate_code
                }),
                sql_escape(&serde_json::to_string(&deterministic_gate)?),
                sql_optional_text(approved_policy_source.as_deref()),
                sql_escape(&serde_json::to_string(&context_json)?),
                reported_reference_sql,
            ))
            .execute(&self.pool)
            .await
            .context("recording shadow report outcome")?;
            created += result.rows_affected() as usize;
        }

        Ok(json!({
            "status": "ok",
            "created": created,
            "skipped": skipped,
            "earlier_pulse_report_id": earlier_report_id,
            "safety": "observation_only_no_gate_hermes_queue_or_saxo_order_authority",
        }))
    }

    /// A report that completed before the observational ledger was available
    /// can still retain its candidates, but its historical infoprice cannot be
    /// recreated honestly. Keep those rows out of the live quote loop instead
    /// of treating a later quote as their report-time baseline.
    pub async fn mark_shadow_report_outcomes_retroactive_reference_unavailable(
        &self,
        report_id: i64,
    ) -> Result<JsonValue> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let result = sqlx::query(&format!(
            "UPDATE shadow_report_outcomes
             SET updated_at = '{}',
                 reference_price_source = 'not_captured_retroactively',
                 status = 'reference_unavailable_retroactive',
                 outcome_maturity_status = 'reference_unavailable_retroactive'
             WHERE report_id = {}
               AND status = 'awaiting_reference'",
            sql_escape(&now),
            report_id.max(0),
        ))
        .execute(&self.pool)
        .await
        .context("marking retroactive shadow report reference unavailable")?;
        Ok(json!({
            "status": "reference_not_captured_retroactively",
            "updated": result.rows_affected(),
            "safety": "historical_shadow_baseline_not_inferred_from_later_saxo_quote",
        }))
    }

    pub async fn active_shadow_report_outcome_symbols(&self) -> Result<Vec<String>> {
        let rows = self
            .select_json(
                "SELECT DISTINCT symbol\n                 FROM shadow_report_outcomes\n                 WHERE status IN ('tracking', 'awaiting_reference')\n                   AND (outcome_maturity_status IS NULL OR outcome_maturity_status <> 'mature')\n                 ORDER BY symbol",
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| json_text(row, "symbol"))
            .filter(|symbol| !symbol.trim().is_empty())
            .collect())
    }

    /// The price monitor calls this only after its ordinary read-only Saxo
    /// info-price refresh. Recording or updating a shadow outcome cannot reach
    /// the execution queue or any Saxo trade endpoint.
    pub async fn refresh_shadow_report_outcome_price(
        &self,
        symbol: &str,
        latest_price_local: f64,
        observed_at: &str,
    ) -> Result<usize> {
        if symbol.trim().is_empty() || !latest_price_local.is_finite() || latest_price_local <= 0.0
        {
            return Ok(0);
        }
        let rows = self
            .select_json(&format!(
                "SELECT id, status, currency, proposed_quantity, action, reference_price_local\n                 FROM shadow_report_outcomes\n                 WHERE symbol = '{}' AND status IN ('tracking', 'awaiting_reference')\n                   AND (outcome_maturity_status IS NULL OR outcome_maturity_status <> 'mature')",
                sql_escape(symbol)
            ))
            .await?;
        let mut updated = 0usize;
        for row in rows {
            let id = json_text(&row, "id");
            let sql = if json_text(&row, "status") == "awaiting_reference" {
                let currency = json_text(&row, "currency");
                let fx_basis = crate::fx::cached_fx_conversion_basis_to_dkk(&self.pool, &currency)
                    .await
                    .unwrap_or_else(|_| {
                        json!({
                            "status": "unavailable",
                            "reason": "fx_cache_lookup_failed",
                            "currency": currency,
                        })
                    });
                let estimated_cost = compact_shadow_report_reference_cost_estimate(
                    symbol,
                    &currency,
                    value_f64(&row, "proposed_quantity"),
                    latest_price_local,
                    &fx_basis,
                    &self.config,
                );
                let dkk_basis = if json_text(&estimated_cost, "status") == "available" {
                    "captured_fresh_fx"
                } else {
                    "unavailable_at_reference"
                };
                format!(
                    "UPDATE shadow_report_outcomes\n                     SET updated_at = '{}', reference_price_local = {},\n                         reference_price_at = '{}', reference_price_source = 'saxo_infoprices',\n                         reference_dkk_basis = '{}', reference_fx_json = '{}', estimated_cost_json = '{}',\n                         status = 'tracking'\n                     WHERE id = '{}' AND status = 'awaiting_reference'",
                    sql_escape(observed_at),
                    latest_price_local,
                    sql_escape(observed_at),
                    dkk_basis,
                    sql_escape(&serde_json::to_string(&fx_basis)?),
                    sql_escape(&serde_json::to_string(&estimated_cost)?),
                    sql_escape(&id),
                )
            } else {
                format!(
                    "UPDATE shadow_report_outcomes\n                     SET updated_at = '{}', latest_price_local = {}, latest_price_at = '{}',\n                         observation_count = observation_count + 1\n                     WHERE id = '{}' AND status = 'tracking'",
                    sql_escape(observed_at),
                    latest_price_local,
                    sql_escape(observed_at),
                    sql_escape(&id),
                )
            };
            let result = sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .context("updating shadow report outcome quote")?;
            if result.rows_affected() > 0 {
                sqlx::query(&format!(
                    "INSERT INTO shadow_report_outcome_quotes (outcome_id, observed_at, price_local, source)\n                     VALUES ('{}', '{}', {}, 'saxo_infoprices')\n                     ON CONFLICT(outcome_id, observed_at) DO NOTHING",
                    sql_escape(&id),
                    sql_escape(observed_at),
                    latest_price_local,
                ))
                .execute(&self.pool)
                .await
                .context("persisting shadow report outcome quote sample")?;
                self.refresh_shadow_report_outcome_observed_excursions(&id)
                    .await?;
                updated += 1;
            }
        }
        Ok(updated)
    }

    /// Recomputes a shadow candidate's observed-price excursion entirely from
    /// the small local quote trail retained for that candidate. No broker call
    /// is possible here, and missing samples are exposed as limited coverage.
    async fn refresh_shadow_report_outcome_observed_excursions(&self, id: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Ok(());
        }
        let outcome = self
            .first_json(&format!(
                "SELECT action, reference_price_local\n                 FROM shadow_report_outcomes\n                 WHERE id = '{}'\n                 LIMIT 1",
                sql_escape(id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let reference_price_local = value_f64(&outcome, "reference_price_local");
        if outcome.is_null() || reference_price_local <= 0.0 {
            return Ok(());
        }
        let quotes = self
            .select_json(&format!(
                "SELECT observed_at, price_local\n                 FROM shadow_report_outcome_quotes\n                 WHERE outcome_id = '{}'\n                 ORDER BY observed_at ASC",
                sql_escape(id)
            ))
            .await?;
        let excursion = compact_shadow_report_observed_excursions(
            &json_text(&outcome, "action"),
            reference_price_local,
            &quotes,
        );
        sqlx::query(&format!(
            "UPDATE shadow_report_outcomes\n             SET intraday_excursion_json = '{}'\n             WHERE id = '{}'",
            sql_escape(&serde_json::to_string(&excursion)?),
            sql_escape(id),
        ))
        .execute(&self.pool)
        .await
        .context("persisting shadow report observed intraday excursion")?;
        Ok(())
    }

    /// Matures shadow-report observations from already persisted daily closes.
    /// This is invoked after a daily-indicator run and performs database reads
    /// and writes only; it deliberately cannot refresh a Saxo quote, request a
    /// provider response, run Hermes, evaluate a manager gate, or mutate an
    /// order.
    pub async fn refresh_shadow_report_outcome_daily_outcomes(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(
                "SELECT id, symbol, action, reference_price_local, reference_price_at, estimated_cost_json\n                 FROM shadow_report_outcomes\n                 WHERE reference_price_source = 'saxo_infoprices'\n                   AND reference_price_local > 0\n                   AND reference_price_at IS NOT NULL\n                   AND (outcome_maturity_status IS NULL OR outcome_maturity_status <> 'mature')\n                 ORDER BY reference_price_at ASC, id ASC\n                 LIMIT 500",
            )
            .await?;
        let mut refreshed = 0usize;
        let mut awaiting_daily_close = 0usize;
        let mut mature = 0usize;
        let mut skipped = 0usize;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        for row in rows {
            let id = json_text(&row, "id");
            let reference_at = json_text(&row, "reference_price_at");
            let reference_date = reference_at.chars().take(10).collect::<String>();
            if id.is_empty() || reference_date.len() != 10 {
                skipped += 1;
                continue;
            }
            // Distinct run dates are trading-session observations. Weekends,
            // holidays, and missing instrument coverage never manufacture a
            // forward session simply because calendar time elapsed.
            let subsequent_closes = self
                .select_json(&format!(
                    "SELECT run_date, MAX(close) AS close\n                     FROM daily_indicator_signals\n                     WHERE symbol = '{}' AND status = 'ok' AND close > 0 AND run_date > '{}'\n                     GROUP BY run_date\n                     ORDER BY run_date ASC\n                     LIMIT 20",
                    sql_escape(&json_text(&row, "symbol")),
                    sql_escape(&reference_date),
                ))
                .await?;
            let outcome = compact_shadow_report_forward_outcome(
                &json_text(&row, "action"),
                value_f64(&row, "reference_price_local"),
                &reference_at,
                &subsequent_closes,
            );
            if outcome.is_null() {
                skipped += 1;
                continue;
            }
            let estimated_cost = row
                .get("estimated_cost_json")
                .cloned()
                .filter(JsonValue::is_object)
                .or_else(|| {
                    row.get("estimated_cost_json")
                        .and_then(JsonValue::as_str)
                        .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
                })
                .unwrap_or_else(|| {
                    json!({
                        "status": "unavailable",
                        "reason": "cost_basis_not_captured_at_reference",
                    })
                });
            let one_session_after_cost = compact_shadow_report_after_cost_outcome(
                outcome.get("one_session").unwrap_or(&JsonValue::Null),
                &estimated_cost,
            );
            let five_session_after_cost = compact_shadow_report_after_cost_outcome(
                outcome.get("five_session").unwrap_or(&JsonValue::Null),
                &estimated_cost,
            );
            let twenty_session_after_cost = compact_shadow_report_after_cost_outcome(
                outcome.get("twenty_session").unwrap_or(&JsonValue::Null),
                &estimated_cost,
            );
            let maturity_status = json_text(&outcome, "status");
            let result = sqlx::query(&format!(
                "UPDATE shadow_report_outcomes\n                 SET updated_at = '{}',\n                     one_session_outcome_json = '{}',\n                     five_session_outcome_json = '{}',\n                     twenty_session_outcome_json = '{}',\n                     one_session_after_cost_outcome_json = '{}',\n                     five_session_after_cost_outcome_json = '{}',\n                     twenty_session_after_cost_outcome_json = '{}',\n                     outcome_maturity_status = '{}'\n                 WHERE id = '{}'",
                sql_escape(&now),
                sql_escape(&serde_json::to_string(
                    outcome.get("one_session").unwrap_or(&JsonValue::Null)
                )?),
                sql_escape(&serde_json::to_string(
                    outcome.get("five_session").unwrap_or(&JsonValue::Null)
                )?),
                sql_escape(&serde_json::to_string(
                    outcome.get("twenty_session").unwrap_or(&JsonValue::Null)
                )?),
                sql_escape(&serde_json::to_string(&one_session_after_cost)?),
                sql_escape(&serde_json::to_string(&five_session_after_cost)?),
                sql_escape(&serde_json::to_string(&twenty_session_after_cost)?),
                sql_escape(&maturity_status),
                sql_escape(&id),
            ))
            .execute(&self.pool)
            .await
            .context("persisting shadow report daily outcome")?;
            refreshed += result.rows_affected() as usize;
            if maturity_status == "awaiting_daily_close" {
                awaiting_daily_close += 1;
            } else if maturity_status == "mature" {
                mature += 1;
            }
        }
        Ok(json!({
            "status": "ok",
            "refreshed": refreshed,
            "mature": mature,
            "awaiting_daily_close": awaiting_daily_close,
            "skipped": skipped,
            "safety": "read_only_persisted_daily_indicator_close_observations_no_saxo_provider_hermes_gate_or_order_authority",
        }))
    }
}
