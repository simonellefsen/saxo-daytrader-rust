use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    time::Duration as StdDuration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};
use sqlx::Row;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{row_to_json, sql_escape, value_f64},
    drawdown_guard::{DrawdownGuard, DrawdownPolicy, evaluate_drawdown_guard},
    state::{AppState, SUPPORTED_EXPERIMENT_VARIABLES},
};

const DEFAULT_MAX_REPORT_AGE_HOURS: i64 = 6;
const EXPERIMENT_STATUS_ALLOWLIST: &[&str] = &[
    "approved_sim",
    "active_sim",
    "approved_paper",
    "active_paper",
];
const HERMES_CONTEXT_SELF_CHECK_FIELDS: &[&str] = &[
    "latest_report",
    "markov_signals",
    "end_of_day_report",
    "current_positions",
    "active_experiments",
];

#[derive(Clone, Debug)]
struct DecisionReport {
    id: i64,
    created_at: String,
    status: String,
    pulse_key: String,
    pulse_label: String,
    report_json: JsonValue,
}

#[derive(Clone, Debug, PartialEq)]
struct CandidateOrder {
    symbol: String,
    action: String,
    order_type: String,
    currency: Option<String>,
    quantity: f64,
    price_local: Option<f64>,
    limit_price_local: Option<f64>,
    stop_price_local: Option<f64>,
    requested_weight_pct: Option<f64>,
    estimated_value_dkk: Option<f64>,
    strategy_type: Option<String>,
    strategy_session: Option<String>,
    strategy_key: String,
    strategy_role: Option<String>,
    raw: JsonValue,
}

#[derive(Debug, PartialEq)]
struct GateDecision {
    approved: bool,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MarkovGateConfig {
    pub(crate) enabled: bool,
    pub(crate) min_signed_signal: f64,
    pub(crate) max_position_pct: f64,
    pub(crate) max_signal_age_days: i64,
}

/// Deterministic BUY-sizing policy. The loss budget is tied to the same ATR
/// distance the automatic protective-stop sweep will use, so a configured
/// `risk_per_trade_pct` describes a concrete maximum loss rather than a
/// model-supplied weight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RiskPerTradeConfig {
    risk_per_trade_pct: f64,
    stop_loss_atr_multiple: f64,
    protective_stops_enabled: bool,
}

/// Deterministic lower-bound transaction-cost policy for BUYs. The estimate
/// deliberately uses only the exchange minimum commission and configured
/// slippage rather than claiming to know the broker's eventual commission or
/// fill price.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CostGuardConfig {
    estimated_slippage_bps: f64,
    cost_guard_multiple: f64,
}

/// Bounds the number of distinct instruments a single Decision Report may
/// send through the deterministic manager gates. The provider keeps its full
/// report for audit, but only the first distinct symbols in report order can
/// consume Hermes evaluation, capital budget, or broker queue capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CandidateLimitConfig {
    max_symbols: i64,
}

/// Maximum distinct BUY symbols a single Decision Report may approve after all
/// deterministic gates have run. It limits simultaneous new exposure without
/// suppressing SELLs or follow-up actions for a symbol already selected by the
/// same report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectedAssetLimitConfig {
    max_selected_assets: i64,
}

/// Maximum total portfolio allocation permitted for one symbol. This is
/// deliberately a portfolio-exposure cap, not a model target weight: it uses
/// the persisted position value plus any BUYs already admitted in this
/// scheduler cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PositionWeightConfig {
    max_position_weight: f64,
}

/// Maximum number of concurrently held symbols. This cap applies only when a
/// BUY introduces a new symbol; adds to a current position do not consume a
/// second slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HoldingLimitConfig {
    max_holdings: i64,
}

/// Caps distinct positive-quantity symbols within one exchange or trading
/// currency. A zero cap is an explicit unlimited policy, allowing the policy
/// surface to ship and be audited before an operator selects a live limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConcentrationConfig {
    max_assets_per_exchange: i64,
    max_assets_per_currency: i64,
}

/// A bounded local view of current position market values. It is loaded once
/// per manager cycle and then reserved as BUYs are approved, so sequential
/// reports cannot independently spend the same symbol headroom.
#[derive(Clone, Debug, PartialEq)]
struct PositionExposure {
    values_dkk: HashMap<String, f64>,
    invalid_symbols: HashSet<String>,
    held_symbols: HashSet<String>,
    available: bool,
}

fn risk_per_trade_config(state: &AppState) -> RiskPerTradeConfig {
    RiskPerTradeConfig {
        risk_per_trade_pct: yaml_f64(&state.config, &["strategy", "swing", "risk_per_trade_pct"])
            .unwrap_or(0.01),
        stop_loss_atr_multiple: yaml_f64(
            &state.config,
            &["strategy", "ladder", "stop_loss_atr_multiple"],
        )
        .unwrap_or(2.0),
        protective_stops_enabled: yaml_bool(
            &state.config,
            &["strategy", "ladder", "submit_stop_loss_after_fill"],
        )
        .unwrap_or(false),
    }
}

fn cost_guard_config(state: &AppState) -> CostGuardConfig {
    CostGuardConfig {
        estimated_slippage_bps: yaml_f64(&state.config, &["strategy", "estimated_slippage_bps"])
            .unwrap_or(8.0),
        cost_guard_multiple: yaml_f64(&state.config, &["strategy", "cost_guard_multiple"])
            .unwrap_or(1.5),
    }
}

fn candidate_limit_config(state: &AppState) -> CandidateLimitConfig {
    CandidateLimitConfig {
        max_symbols: yaml_i64(
            &state.config,
            &["strategy", "swing", "trading_manager", "max_symbols"],
        )
        .unwrap_or(30),
    }
}

fn selected_asset_limit_config(state: &AppState) -> SelectedAssetLimitConfig {
    SelectedAssetLimitConfig {
        max_selected_assets: yaml_i64(&state.config, &["strategy", "max_selected_assets"])
            .unwrap_or(8),
    }
}

fn position_weight_config(state: &AppState) -> PositionWeightConfig {
    PositionWeightConfig {
        // This is the established ladder allocation ceiling from the legacy
        // strategy engine. The other historical weight keys remain unused
        // until their distinct policies are deliberately reconciled.
        max_position_weight: yaml_f64(
            &state.config,
            &["strategy", "ladder", "max_position_weight"],
        )
        .unwrap_or(0.04),
    }
}

fn holding_limit_config(state: &AppState) -> HoldingLimitConfig {
    HoldingLimitConfig {
        max_holdings: yaml_i64(&state.config, &["strategy", "swing", "max_holdings"]).unwrap_or(25),
    }
}

fn concentration_config(state: &AppState) -> ConcentrationConfig {
    ConcentrationConfig {
        max_assets_per_exchange: yaml_i64(
            &state.config,
            &["strategy", "concentration", "max_assets_per_exchange"],
        )
        .unwrap_or(0),
        max_assets_per_currency: yaml_i64(
            &state.config,
            &["strategy", "concentration", "max_assets_per_currency"],
        )
        .unwrap_or(0),
    }
}

impl PositionWeightConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "max_position_weight": self.max_position_weight,
            "scope": "total_symbol_exposure_after_approved_buy",
            "basis": "persisted_portfolio_position_values",
        })
    }
}

impl HoldingLimitConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "max_holdings": self.max_holdings,
            "scope": "new_symbol_buys_only",
            "basis": "persisted_positive_quantity_positions_plus_same_cycle_approved_buys",
        })
    }
}

impl ConcentrationConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "max_assets_per_exchange": self.max_assets_per_exchange,
            "max_assets_per_currency": self.max_assets_per_currency,
            "exchange_mode": concentration_mode(self.max_assets_per_exchange),
            "currency_mode": concentration_mode(self.max_assets_per_currency),
            "scope": "distinct_positive_quantity_positions_plus_same_cycle_approved_buys",
            "bucket_source": "canonical_symbol_exchange_suffix_and_exchange_currency_mapping",
        })
    }
}

fn concentration_mode(cap: i64) -> &'static str {
    if cap == 0 {
        "unlimited"
    } else if cap > 0 {
        "limited"
    } else {
        "invalid"
    }
}

impl PositionExposure {
    async fn load(state: &AppState) -> Self {
        let rows = match state.position_items(250).await {
            Ok(rows) => rows,
            Err(err) => {
                warn!("position-weight gate could not load persisted position values: {err:#}");
                return Self {
                    values_dkk: HashMap::new(),
                    invalid_symbols: HashSet::new(),
                    held_symbols: HashSet::new(),
                    available: false,
                };
            }
        };

        let mut values_dkk = HashMap::new();
        let mut invalid_symbols = HashSet::new();
        let mut held_symbols = HashSet::new();
        for row in rows {
            let symbol = row
                .get("symbol")
                .and_then(JsonValue::as_str)
                .map(normalize_symbol_key)
                .unwrap_or_default();
            let quantity = row.get("quantity").and_then(json_number).unwrap_or(0.0);
            if symbol.is_empty() || quantity <= 0.0 {
                continue;
            }
            held_symbols.insert(symbol.clone());
            let market_value_dkk = row.get("market_value_dkk").and_then(json_number);
            if let Some(value) = market_value_dkk.filter(|value| value.is_finite() && *value > 0.0)
            {
                values_dkk.insert(symbol, value);
            } else {
                invalid_symbols.insert(symbol);
            }
        }
        Self {
            values_dkk,
            invalid_symbols,
            held_symbols,
            available: true,
        }
    }

    fn value_for(&self, symbol: &str) -> Option<f64> {
        self.values_dkk.get(&normalize_symbol_key(symbol)).copied()
    }

    fn has_invalid_value(&self, symbol: &str) -> bool {
        self.invalid_symbols.contains(&normalize_symbol_key(symbol))
    }

    fn has_position(&self, symbol: &str) -> bool {
        self.held_symbols.contains(&normalize_symbol_key(symbol))
    }

    fn holding_count(&self) -> usize {
        self.held_symbols.len()
    }

    fn exchange_count(&self, exchange: &str) -> usize {
        self.held_symbols
            .iter()
            .filter(|symbol| exchange_code(symbol) == exchange)
            .count()
    }

    fn currency_count(&self, currency: &str) -> usize {
        self.held_symbols
            .iter()
            .filter(|symbol| currency_for_symbol(symbol).as_deref() == Some(currency))
            .count()
    }

    fn unmapped_exchange_symbols(&self) -> Vec<String> {
        self.held_symbols
            .iter()
            .filter(|symbol| exchange_code(symbol).is_empty())
            .cloned()
            .collect()
    }

    fn unmapped_currency_symbols(&self) -> Vec<String> {
        self.held_symbols
            .iter()
            .filter(|symbol| currency_for_symbol(symbol).is_none())
            .cloned()
            .collect()
    }

    fn concentration_for_symbol(&self, symbol: &str) -> JsonValue {
        let exchange = exchange_code(symbol);
        let currency = currency_for_symbol(symbol);
        json!({
            "exchange": exchange,
            "currency": currency,
            "exchange_count_before": if exchange.is_empty() { JsonValue::Null } else { json!(self.exchange_count(&exchange)) },
            "currency_count_before": currency.as_deref().map(|currency| json!(self.currency_count(currency))).unwrap_or(JsonValue::Null),
            "already_held": self.has_position(symbol),
            "position_snapshot_available": self.available,
        })
    }

    fn reserve_buy(&mut self, symbol: &str, value_dkk: f64) {
        if !self.available || !value_dkk.is_finite() || value_dkk <= 0.0 {
            return;
        }
        *self
            .values_dkk
            .entry(normalize_symbol_key(symbol))
            .or_insert(0.0) += value_dkk;
        self.held_symbols.insert(normalize_symbol_key(symbol));
    }

    fn to_json(&self) -> JsonValue {
        let mut exchange_counts = BTreeMap::new();
        let mut currency_counts = BTreeMap::new();
        let mut unmapped_exchange_symbols = self.unmapped_exchange_symbols();
        let mut unmapped_currency_symbols = self.unmapped_currency_symbols();
        unmapped_exchange_symbols.sort();
        unmapped_currency_symbols.sort();
        for symbol in &self.held_symbols {
            let exchange = exchange_code(symbol);
            if !exchange.is_empty() {
                *exchange_counts.entry(exchange).or_insert(0_usize) += 1;
            }
            if let Some(currency) = currency_for_symbol(symbol) {
                *currency_counts.entry(currency).or_insert(0_usize) += 1;
            }
        }
        json!({
            "status": if self.available { "available" } else { "unavailable" },
            "valued_symbol_count": self.values_dkk.len(),
            "invalid_value_symbol_count": self.invalid_symbols.len(),
            "held_symbol_count": self.held_symbols.len(),
            "exchange_counts": exchange_counts,
            "currency_counts": currency_counts,
            "unmapped_exchange_symbols": unmapped_exchange_symbols,
            "unmapped_currency_symbols": unmapped_currency_symbols,
            "scope": "persisted_positions_plus_approved_buys_in_this_cycle",
        })
    }
}

fn currency_for_symbol(symbol: &str) -> Option<String> {
    crate::saxo_order::currency_for_exchange(&exchange_code(symbol)).map(str::to_string)
}

fn normalize_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

fn json_number(value: &JsonValue) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .filter(|value| value.is_finite())
}

impl CandidateLimitConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "max_symbols": self.max_symbols,
            "mode": if self.max_symbols == 0 { "unlimited" } else if self.max_symbols > 0 { "limited" } else { "invalid" },
            "scope": "distinct_symbols_per_report",
        })
    }
}

impl SelectedAssetLimitConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "max_selected_assets": self.max_selected_assets,
            "mode": if self.max_selected_assets == 0 { "unlimited" } else if self.max_selected_assets > 0 { "limited" } else { "invalid" },
            "scope": "distinct_approved_buy_symbols_per_report",
        })
    }
}

fn selected_asset_limit_gate(
    order: &mut CandidateOrder,
    config: SelectedAssetLimitConfig,
    selected_buy_symbols: &mut HashSet<String>,
) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Selection cap applies to BUYs only; SELLs remain eligible.".to_string(),
        };
    }
    if config.max_selected_assets < 0 {
        return GateDecision {
            approved: false,
            reason: "Selection cap configuration is invalid: strategy.max_selected_assets must be non-negative (0 means unlimited).".to_string(),
        };
    }

    let symbol = candidate_symbol_key(order);
    let selected_before = selected_buy_symbols.len();
    let already_selected = selected_buy_symbols.contains(&symbol);
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "selected_asset_limit".to_string(),
            json!({
                "max_selected_assets": config.max_selected_assets,
                "selected_buy_symbol_count_before": selected_before,
                "already_selected": already_selected,
                "scope": "distinct_approved_buy_symbols_per_report",
            }),
        );
    }
    if config.max_selected_assets > 0
        && !already_selected
        && selected_before >= config.max_selected_assets as usize
    {
        return GateDecision {
            approved: false,
            reason: format!(
                "Selection cap is {}; {} distinct BUY symbols are already approved in this Decision Report, so additional {} BUY is blocked.",
                config.max_selected_assets, selected_before, order.symbol
            ),
        };
    }

    selected_buy_symbols.insert(symbol);
    if already_selected {
        GateDecision {
            approved: true,
            reason: format!(
                "Selection cap allows additional {} BUY because this symbol is already selected by this Decision Report.",
                order.symbol
            ),
        }
    } else if config.max_selected_assets == 0 {
        GateDecision {
            approved: true,
            reason: "Selection cap is unlimited for this Decision Report.".to_string(),
        }
    } else {
        GateDecision {
            approved: true,
            reason: format!(
                "Selection cap allows {} BUY ({}/{} distinct BUY symbols selected).",
                order.symbol,
                selected_before + 1,
                config.max_selected_assets
            ),
        }
    }
}

fn candidate_symbol_key(order: &CandidateOrder) -> String {
    order.symbol.trim().to_ascii_uppercase()
}

fn candidate_limit_skip_reason(config: CandidateLimitConfig) -> String {
    if config.max_symbols < 0 {
        "Candidate limit configuration is invalid: strategy.swing.trading_manager.max_symbols must be non-negative (0 means unlimited).".to_string()
    } else {
        format!(
            "Candidate limit reached: only the first {} distinct symbols in this Decision Report are eligible for Trading Manager evaluation.",
            config.max_symbols
        )
    }
}

fn attach_candidate_limit_metadata(
    order: &mut CandidateOrder,
    config: CandidateLimitConfig,
    eligible: bool,
) {
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "candidate_limit".to_string(),
            json!({
                "max_symbols": config.max_symbols,
                "scope": "distinct_symbols_per_report",
                "eligible": eligible,
            }),
        );
    }
}

/// Preserves the provider's report order and keeps repeated actions for a
/// symbol already inside the limit. This is a symbol cap, rather than an order
/// cap, because a report can legitimately contain an adjustment and an exit
/// for the same instrument while still representing one portfolio name.
fn enforce_candidate_symbol_limit(
    candidates: Vec<CandidateOrder>,
    config: CandidateLimitConfig,
) -> (Vec<CandidateOrder>, Vec<CandidateOrder>) {
    if config.max_symbols < 0 {
        let mut rejected = candidates;
        for order in &mut rejected {
            attach_candidate_limit_metadata(order, config, false);
        }
        return (Vec::new(), rejected);
    }
    if config.max_symbols == 0 {
        let mut eligible = candidates;
        for order in &mut eligible {
            attach_candidate_limit_metadata(order, config, true);
        }
        return (eligible, Vec::new());
    }

    let limit = config.max_symbols as usize;
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    let mut included_symbols = HashSet::new();
    for mut order in candidates {
        let key = candidate_symbol_key(&order);
        let within_limit = included_symbols.contains(&key) || included_symbols.len() < limit;
        if within_limit {
            included_symbols.insert(key);
            attach_candidate_limit_metadata(&mut order, config, true);
            eligible.push(order);
        } else {
            attach_candidate_limit_metadata(&mut order, config, false);
            rejected.push(order);
        }
    }
    (eligible, rejected)
}

pub(crate) fn markov_gate_config(state: &AppState) -> MarkovGateConfig {
    MarkovGateConfig {
        enabled: yaml_bool(
            &state.config,
            &["strategy", "swing", "markov_gate", "enabled"],
        )
        .unwrap_or(true),
        min_signed_signal: yaml_f64(
            &state.config,
            &["strategy", "swing", "markov_gate", "min_signed_signal"],
        )
        .unwrap_or(0.15)
        .max(0.0),
        max_position_pct: yaml_f64(
            &state.config,
            &["strategy", "swing", "markov_gate", "max_position_pct"],
        )
        .unwrap_or(0.05)
        .clamp(0.0, 1.0),
        max_signal_age_days: yaml_i64(
            &state.config,
            &["strategy", "swing", "markov_gate", "max_signal_age_days"],
        )
        .unwrap_or(5)
        .max(1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CapitalBudget {
    cash_balance_dkk: f64,
    total_market_value_dkk: f64,
    invested_market_value_dkk: f64,
    cash_pct: f64,
    min_cash_buffer_pct: f64,
    max_deployment_pct: f64,
    reinvestment_pressure_threshold_pct: f64,
    required_cash_buffer_dkk: f64,
    available_cash_above_buffer_dkk: f64,
    unreduced_available_buy_budget_dkk: f64,
    available_buy_budget_dkk: f64,
    remaining_deployment_capacity_dkk: f64,
    excess_cash_pct: f64,
    reinvestment_pressure_active: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct StrategyExperimentOverlay {
    id: String,
    status: String,
    goal_version: i64,
    changed_variable_path: String,
    old_value: JsonValue,
    new_value: JsonValue,
    hypothesis: String,
}

#[derive(Clone, Debug, PartialEq)]
struct HermesDecisionAdvice {
    status: String,
    mode: String,
    source_session_id: String,
    overall_recommendation: String,
    summary: String,
    raw: JsonValue,
    order_advice: HashMap<String, HermesOrderAdvice>,
}

#[derive(Clone, Debug, PartialEq)]
struct HermesOrderAdvice {
    action: String,
    reason: String,
    max_quantity: Option<f64>,
    raw: JsonValue,
}

fn hermes_context_self_check_not_recorded(status: &str) -> JsonValue {
    json!({
        "complete": false,
        "status": status,
        "required": HERMES_CONTEXT_SELF_CHECK_FIELDS,
        "missing": HERMES_CONTEXT_SELF_CHECK_FIELDS,
        "notes": "Hermes did not record a context self-check for this advisory result."
    })
}

fn hermes_context_self_check_from_raw(row: &JsonValue) -> JsonValue {
    let source = row
        .get("raw_payload_json")
        .and_then(|value| value.get("context_self_check"))
        .or_else(|| row.get("context_self_check"));
    let Some(source) = source else {
        return hermes_context_self_check_not_recorded("missing");
    };
    let mut object = source
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    let mut missing = Vec::new();
    for field in HERMES_CONTEXT_SELF_CHECK_FIELDS {
        if object.get(*field).and_then(JsonValue::as_bool) != Some(true) {
            missing.push(JsonValue::String((*field).to_string()));
        }
    }
    object.insert(
        "required".to_string(),
        json!(HERMES_CONTEXT_SELF_CHECK_FIELDS),
    );
    object.insert("missing".to_string(), JsonValue::Array(missing.clone()));
    object.insert("complete".to_string(), JsonValue::Bool(missing.is_empty()));
    JsonValue::Object(object)
}

impl HermesDecisionAdvice {
    fn fallback(status: &str, mode: String, summary: String, report_id: i64) -> Self {
        let overall_recommendation = if mode == "conservative"
            && matches!(
                status,
                "error" | "timeout" | "not_configured" | "submit_failed"
            ) {
            "review"
        } else {
            "proceed"
        };
        let raw = json!({
            "status": status,
            "decision_report_id": report_id,
            "overall_recommendation": overall_recommendation,
            "summary": summary,
            "order_advice_json": [],
            "learning_notes_json": [],
            "context_self_check": hermes_context_self_check_not_recorded(status)
        });
        Self {
            status: status.to_string(),
            mode,
            source_session_id: String::new(),
            overall_recommendation: overall_recommendation.to_string(),
            summary,
            raw,
            order_advice: HashMap::new(),
        }
    }

    fn from_row(row: JsonValue, mode: String, source_session_id: String) -> Self {
        let order_advice_json = row
            .get("order_advice_json")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let mut order_advice = HashMap::new();
        if let Some(items) = order_advice_json.as_array() {
            for item in items {
                let advice = HermesOrderAdvice {
                    action: text(item, "action").trim().to_lowercase(),
                    reason: text(item, "reason"),
                    max_quantity: item.get("max_quantity").and_then(JsonValue::as_f64),
                    raw: item.clone(),
                };
                if !matches!(
                    advice.action.as_str(),
                    "allow" | "reduce" | "stand_down" | "review"
                ) {
                    continue;
                }
                let strategy_key = text(item, "strategy_key");
                if !strategy_key.trim().is_empty() {
                    order_advice.insert(format!("strategy:{strategy_key}"), advice.clone());
                }
                let symbol = text(item, "symbol");
                let side = text(item, "side").to_uppercase();
                if !symbol.trim().is_empty() && !side.trim().is_empty() {
                    order_advice.insert(format!("symbol_side:{symbol}:{side}"), advice.clone());
                }
                if !symbol.trim().is_empty() {
                    order_advice.insert(format!("symbol:{symbol}"), advice);
                }
            }
        }

        Self {
            status: text(&row, "status"),
            mode,
            source_session_id,
            overall_recommendation: text(&row, "overall_recommendation").trim().to_lowercase(),
            summary: text(&row, "summary"),
            raw: row,
            order_advice,
        }
    }

    fn for_order(&self, order: &CandidateOrder) -> Option<&HermesOrderAdvice> {
        self.for_order_with_match_source(order)
            .map(|(advice, _)| advice)
    }

    fn for_order_with_match_source(
        &self,
        order: &CandidateOrder,
    ) -> Option<(&HermesOrderAdvice, &'static str)> {
        self.order_advice
            .get(&format!("strategy:{}", order.strategy_key))
            .map(|advice| (advice, "strategy_key"))
            .or_else(|| {
                self.order_advice
                    .get(&format!("symbol_side:{}:{}", order.symbol, order.action))
                    .map(|advice| (advice, "symbol_side"))
            })
            .or_else(|| {
                self.order_advice
                    .get(&format!("symbol:{}", order.symbol))
                    .map(|advice| (advice, "symbol"))
            })
    }

    fn to_json(&self) -> JsonValue {
        json!({
            "status": self.status,
            "mode": self.mode,
            "source_session_id": self.source_session_id,
            "overall_recommendation": self.overall_recommendation,
            "summary": self.summary,
            "context_self_check": hermes_context_self_check_from_raw(&self.raw),
            "raw": self.raw
        })
    }
}

fn hermes_context_self_check_gate_reason(advice: &HermesDecisionAdvice) -> Option<String> {
    if advice.mode != "conservative" {
        return None;
    }
    let check = hermes_context_self_check_from_raw(&advice.raw);
    if check
        .get("complete")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let missing = check
        .get("missing")
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(JsonValue::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "required context was not recorded".to_string());
    Some(format!(
        "Hermes context self-check is incomplete ({missing}); conservative mode blocks automatic queueing until the advice is re-run with the required context."
    ))
}

fn attach_hermes_context_gate(
    order: &mut CandidateOrder,
    advice: &HermesDecisionAdvice,
    reason: &str,
) {
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "hermes_context_gate".to_string(),
            json!({
                "mode": advice.mode,
                "automatic_queueing_blocked": true,
                "reason": reason,
                "context_self_check": hermes_context_self_check_from_raw(&advice.raw),
            }),
        );
    }
}

fn attach_hermes_advice(
    order: &mut CandidateOrder,
    advice: &HermesOrderAdvice,
    decision_advice: &HermesDecisionAdvice,
) {
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "hermes_advice".to_string(),
            json!({
                "mode": decision_advice.mode,
                "status": decision_advice.status,
                "source_session_id": decision_advice.source_session_id,
                "overall_recommendation": decision_advice.overall_recommendation,
                "order_advice": advice.raw,
            }),
        );
    }
}

/// Persist a deterministic account of the advisory effect before local gates
/// run. This keeps Hermes influence measurable without serializing arbitrary
/// Hermes rationale or broker data into the manager-run record.
fn hermes_advice_delta(
    candidates: &[CandidateOrder],
    advice: &HermesDecisionAdvice,
    context_gate_reason: Option<&str>,
) -> JsonValue {
    let conservative = advice.mode == "conservative";
    let global_stand_down = conservative && advice.overall_recommendation == "stand_down";
    let global_review = conservative && advice.overall_recommendation == "review";
    let mut matched_candidate_count = 0usize;
    let mut effect_counts: HashMap<String, usize> = HashMap::new();
    let candidates = candidates
        .iter()
        .map(|order| {
            let matched = advice.for_order_with_match_source(order);
            if matched.is_some() {
                matched_candidate_count += 1;
            }
            let (advice_action, advice_max_quantity, match_source) = matched
                .map(|(item, source)| {
                    (
                        item.action.clone(),
                        item.max_quantity,
                        Some(source.to_string()),
                    )
                })
                .unwrap_or_else(|| ("none".to_string(), None, None));
            let requested_quantity = order.quantity;
            let (effect, resulting_quantity) = if !conservative {
                ("record_only_no_op", requested_quantity)
            } else if context_gate_reason.is_some() {
                ("context_gate_blocked", 0.0)
            } else if matches!(advice_action.as_str(), "stand_down" | "review") {
                ("blocked_by_order_advice", 0.0)
            } else {
                let (reduced_quantity, reduced) = match advice_max_quantity {
                    Some(max_quantity)
                        if advice_action == "reduce"
                            && max_quantity >= 1.0
                            && max_quantity.floor() < requested_quantity =>
                    {
                        (max_quantity.floor(), true)
                    }
                    _ => (requested_quantity, false),
                };
                let explicit_allow = matches!(advice_action.as_str(), "allow" | "reduce");
                if advice_action == "reduce" && advice_max_quantity.unwrap_or(0.0) < 1.0 {
                    ("blocked_by_reduce_below_one_share", 0.0)
                } else if global_stand_down {
                    ("blocked_by_global_stand_down", 0.0)
                } else if global_review && !explicit_allow {
                    ("review_required_by_global_advice", 0.0)
                } else if reduced {
                    ("reduced", reduced_quantity)
                } else if advice_action == "allow" {
                    ("allowed", requested_quantity)
                } else if advice_action == "reduce" {
                    ("reduce_cap_no_op", requested_quantity)
                } else {
                    ("no_op", requested_quantity)
                }
            };
            *effect_counts.entry(effect.to_string()).or_default() += 1;
            json!({
                "strategy_key": order.strategy_key,
                "symbol": order.symbol,
                "action": order.action,
                "currency": order.currency,
                "reference_price_local": order.limit_price_local.or(order.price_local),
                "match_source": match_source,
                "advice_action": advice_action,
                "advice_max_quantity": advice_max_quantity,
                "requested_quantity": requested_quantity,
                "resulting_quantity": resulting_quantity,
                "effect": effect,
                "manager_outcome": "not_recorded",
            })
        })
        .collect::<Vec<_>>();
    json!({
        "version": 1,
        "mode": advice.mode,
        "overall_recommendation": advice.overall_recommendation,
        "candidate_count": candidates.len(),
        "matched_candidate_count": matched_candidate_count,
        "context_gate_enforced": context_gate_reason.is_some(),
        "effect_counts": effect_counts,
        "candidates": candidates,
        "safety": {
            "hermes_rationale_excluded": true,
            "raw_broker_payloads_excluded": true,
            "raw_execution_errors_excluded": true,
        }
    })
}

fn with_hermes_advice_manager_outcomes(
    mut delta: JsonValue,
    approved: &[(CandidateOrder, String)],
    skipped: &[JsonValue],
) -> JsonValue {
    let Some(entries) = delta
        .get_mut("candidates")
        .and_then(JsonValue::as_array_mut)
    else {
        return delta;
    };
    let mut manager_outcome_counts: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let strategy_key = text(entry, "strategy_key");
        let symbol = text(entry, "symbol");
        let action = text(entry, "action");
        let outcome = if approved.iter().any(|(order, _)| {
            order.strategy_key == strategy_key && order.symbol == symbol && order.action == action
        }) {
            "approved"
        } else if skipped.iter().any(|order| {
            text(order, "strategy_key") == strategy_key
                && text(order, "symbol") == symbol
                && text(order, "action") == action
        }) {
            "skipped"
        } else {
            "not_reached"
        };
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                "manager_outcome".to_string(),
                JsonValue::String(outcome.to_string()),
            );
        }
        *manager_outcome_counts
            .entry(outcome.to_string())
            .or_default() += 1;
    }
    if let Some(object) = delta.as_object_mut() {
        object.insert(
            "manager_outcome_counts".to_string(),
            json!(manager_outcome_counts),
        );
    }
    delta
}

/// Monthly-loss circuit breaker: a soft loss band reduces new BUY capacity,
/// while the hard floor suspends BUYs altogether. SELLs stay allowed in both
/// bands so defensive exits are never blocked by the capital guardrail.
#[derive(Clone, Debug, PartialEq)]
struct MonthlyLossBuyHalt {
    active: bool,
    threshold_breached: bool,
    month_pnl_dkk: f64,
    threshold_dkk: f64,
    soft_threshold_dkk: f64,
    soft_buy_multiplier: f64,
    soft_reduction_active: bool,
    override_active: bool,
    override_value: JsonValue,
}

async fn monthly_loss_buy_halt(state: &AppState, overview: &JsonValue) -> MonthlyLossBuyHalt {
    let threshold_dkk = yaml_f64(
        &state.config,
        &["strategy", "capital", "monthly_loss_halt_dkk"],
    )
    .unwrap_or(-10_000.0);
    let soft_threshold_dkk = yaml_f64(
        &state.config,
        &["strategy", "capital", "monthly_loss_soft_reduce_dkk"],
    )
    .unwrap_or(-25_000.0);
    let soft_buy_multiplier = yaml_f64(
        &state.config,
        &["strategy", "capital", "monthly_loss_soft_buy_multiplier"],
    )
    .unwrap_or(0.5)
    .clamp(0.0, 1.0);
    let month_pnl_dkk = overview
        .get("goal_tracking")
        .and_then(|value| value.get("periods"))
        .and_then(|value| value.get("month"))
        .map(|value| value_f64(value, "pnl_dkk"))
        .unwrap_or(0.0);
    let override_value = state
        .monthly_loss_breaker_override_value()
        .await
        .unwrap_or_else(|err| {
            warn!("monthly-loss breaker override lookup degraded: {err:#}");
            json!({
                "enabled": false,
                "active_for_current_month": false,
                "error": err.to_string()
            })
        });
    let override_active = override_value
        .get("active_for_current_month")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let threshold_breached = monthly_loss_threshold_breached(month_pnl_dkk, threshold_dkk);
    let soft_reduction_active =
        monthly_loss_soft_reduction_active(month_pnl_dkk, soft_threshold_dkk, threshold_dkk);
    MonthlyLossBuyHalt {
        // A non-negative threshold disables the breaker.
        active: threshold_breached && !override_active,
        threshold_breached,
        month_pnl_dkk,
        threshold_dkk,
        soft_threshold_dkk,
        soft_buy_multiplier,
        soft_reduction_active,
        override_active,
        override_value,
    }
}

fn monthly_loss_threshold_breached(month_pnl_dkk: f64, threshold_dkk: f64) -> bool {
    threshold_dkk < 0.0 && month_pnl_dkk <= threshold_dkk
}

fn monthly_loss_soft_reduction_active(
    month_pnl_dkk: f64,
    soft_threshold_dkk: f64,
    hard_threshold_dkk: f64,
) -> bool {
    // The soft band must be a real, less-severe negative loss floor above a
    // configured hard halt. Invalid or disabled values must not silently
    // change deployment capacity.
    hard_threshold_dkk < 0.0
        && soft_threshold_dkk < 0.0
        && soft_threshold_dkk > hard_threshold_dkk
        && month_pnl_dkk <= soft_threshold_dkk
        && month_pnl_dkk > hard_threshold_dkk
}

/// Load the portfolio drawdown guardrail for this cycle.
///
/// A read failure disables the guardrail rather than halting the strategy: see
/// the direction-of-failure note in `drawdown_guard`. It is logged at warn so a
/// blind guardrail is never mistaken for a satisfied one.
async fn portfolio_drawdown_guard(state: &AppState) -> DrawdownGuard {
    let policy = DrawdownPolicy::from_config(&state.config);
    let rows = state
        .portfolio_drawdown_history(policy.lookback_days)
        .await
        .unwrap_or_else(|err| {
            warn!("drawdown guardrail history unavailable: {err:#}");
            Vec::new()
        });
    let saved_override = state
        .drawdown_guard_override_value()
        .await
        .unwrap_or_else(|err| {
            warn!("drawdown guardrail override lookup degraded: {err:#}");
            json!({"enabled": false})
        });
    evaluate_drawdown_guard(policy, &rows, saved_override)
}

/// The BUY-budget multiplier to apply when more than one soft guardrail is in
/// its reduction band.
///
/// The strictest single multiplier wins rather than the product. The bands
/// overlap in practice -- a losing month and a drawdown are usually the same
/// event seen from two angles -- so multiplying them double-counts one decline
/// and lands on a deployed capacity nobody chose. Taking the minimum keeps the
/// reduced budget a number the operator can predict from configuration.
fn combined_soft_buy_multiplier(multipliers: &[f64]) -> Option<f64> {
    multipliers
        .iter()
        .copied()
        .filter(|value| value.is_finite() && (0.0..1.0).contains(value))
        .min_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InstrumentQuarantineConfig {
    enabled: bool,
    lookback_days: i64,
    min_failures: usize,
    active_days: i64,
}

#[derive(Clone, Debug, PartialEq)]
struct InstrumentQuarantine {
    symbol: String,
    action: String,
    signature: String,
    failure_count: usize,
    latest_failure_at: String,
    expires_at: String,
    sample_error: String,
    override_active: bool,
    override_notes: String,
    override_updated_at: String,
}

impl InstrumentQuarantine {
    fn to_json(&self) -> JsonValue {
        json!({
            "symbol": self.symbol,
            "action": self.action,
            "signature": self.signature,
            "failure_count": self.failure_count,
            "latest_failure_at": self.latest_failure_at,
            "expires_at": self.expires_at,
            "sample_error": self.sample_error,
            "override_active": self.override_active,
            "override_notes": self.override_notes,
            "override_updated_at": self.override_updated_at,
        })
    }
}

#[cfg(test)]
mod automation_switch_tests {
    use super::*;

    #[test]
    fn trading_manager_switches_default_open_but_honor_false() {
        let enabled: serde_yaml::Value = serde_yaml::from_str(
            "strategy:\n  enabled: true\n  swing:\n    trading_manager:\n      enabled: true\n",
        )
        .unwrap();
        assert!(trading_manager_automation_enabled(&enabled));

        let strategy_disabled: serde_yaml::Value = serde_yaml::from_str(
            "strategy:\n  enabled: false\n  swing:\n    trading_manager:\n      enabled: true\n",
        )
        .unwrap();
        assert!(!trading_manager_automation_enabled(&strategy_disabled));

        let manager_disabled: serde_yaml::Value = serde_yaml::from_str(
            "strategy:\n  enabled: true\n  swing:\n    trading_manager:\n      enabled: false\n",
        )
        .unwrap();
        assert!(!trading_manager_automation_enabled(&manager_disabled));
    }
}

pub async fn run_trading_manager_cycle(state: &AppState) -> Result<JsonValue> {
    if !trading_manager_automation_enabled(&state.config) {
        return Ok(json!({
            "status": "disabled",
            "reason": "strategy.enabled or strategy.swing.trading_manager.enabled is false",
            "runs": []
        }));
    }
    let reports = fresh_unmanaged_reports(state).await?;
    if reports.is_empty() {
        info!("Trading Manager found no fresh scheduled decision reports to process");
        return Ok(json!({"status": "not_due", "runs": []}));
    }

    if let Err(err) = state.refresh_saxo_exchange_calendars_if_stale().await {
        warn!("Trading Manager using fallback exchange calendar: {err:#}");
    }
    let market_rows = state.market_exchange_rows();
    let open_codes = market_rows
        .iter()
        .filter(|row| {
            row.get("is_tradable")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|row| row.get("code").and_then(JsonValue::as_str))
        .map(|code| code.to_uppercase())
        .collect::<Vec<_>>();

    // One budget for the whole cycle: every report's approved BUYs reserve
    // from the same pool, so near-simultaneous reports cannot double-spend
    // the same cash snapshot.
    let overview = state.overview_payload().await.unwrap_or_else(|err| {
        warn!("Trading Manager capital context degraded: {err:#}");
        json!({})
    });
    let overlay = approved_strategy_experiment_overlay(state)
        .await
        .unwrap_or_else(|err| {
            warn!("Trading Manager experiment overlay disabled: {err:#}");
            None
        });
    let overlay_min_cash_buffer_pct = overlay
        .as_ref()
        .and_then(|overlay| overlay.f64_value("strategy.capital.min_cash_buffer_pct"));
    let mut capital_budget = capital_budget_from_overview(&overview, overlay_min_cash_buffer_pct);
    let mut position_exposure = PositionExposure::load(state).await;
    let buy_halt = monthly_loss_buy_halt(state, &overview).await;
    let drawdown = portfolio_drawdown_guard(state).await;
    let mut soft_multipliers = Vec::new();
    if buy_halt.soft_reduction_active {
        soft_multipliers.push(buy_halt.soft_buy_multiplier);
    }
    if drawdown.reduces_buys() {
        soft_multipliers.push(drawdown.policy.soft_buy_multiplier);
    }
    if let Some(multiplier) = combined_soft_buy_multiplier(&soft_multipliers) {
        capital_budget.apply_buy_multiplier(multiplier);
        warn!(
            month_pnl_dkk = buy_halt.month_pnl_dkk,
            monthly_soft_reduction_active = buy_halt.soft_reduction_active,
            drawdown_pct = drawdown.drawdown_pct(),
            drawdown_soft_reduction_active = drawdown.reduces_buys(),
            buy_multiplier = multiplier,
            available_buy_budget_dkk = capital_budget.available_buy_budget_dkk,
            "soft risk guardrail reduced cycle-wide BUY budget"
        );
    }
    if buy_halt.active {
        warn!(
            month_pnl_dkk = buy_halt.month_pnl_dkk,
            threshold_dkk = buy_halt.threshold_dkk,
            "monthly-loss circuit breaker active; all BUY candidates will be skipped"
        );
    }
    if drawdown.halts_buys() {
        warn!(
            drawdown_pct = drawdown.drawdown_pct(),
            halt_pct = drawdown.policy.halt_pct,
            lookback_days = drawdown.policy.lookback_days,
            "portfolio drawdown guardrail active; all BUY candidates will be skipped"
        );
    }
    if drawdown.status == "insufficient_history" {
        warn!(
            lookback_days = drawdown.policy.lookback_days,
            "portfolio drawdown guardrail has too little history to measure a peak; it is not restricting this cycle"
        );
    }
    if drawdown.override_active {
        warn!(
            drawdown_pct = drawdown.drawdown_pct(),
            "portfolio drawdown guardrail suppressed by an operator override"
        );
    }
    let quarantine_cfg = instrument_quarantine_config(state);
    let active_quarantines = active_instrument_quarantines(state)
        .await
        .unwrap_or_else(|err| {
            warn!("instrument quarantine read degraded: {err:#}");
            Vec::new()
        });
    let quarantine_overrides = state
        .instrument_quarantine_overrides_value()
        .await
        .unwrap_or_else(|err| {
            warn!("instrument quarantine override lookup degraded: {err:#}");
            json!({"overrides": [], "error": err.to_string()})
        });
    let active_quarantines =
        apply_instrument_quarantine_overrides(active_quarantines, &quarantine_overrides);

    let mut runs = Vec::new();
    for report in reports {
        match run_for_report(
            state,
            &report,
            &overview,
            &open_codes,
            overlay.as_ref(),
            &mut capital_budget,
            &mut position_exposure,
            &buy_halt,
            &drawdown,
            quarantine_cfg,
            &active_quarantines,
        )
        .await
        {
            Ok(run) => runs.push(run),
            Err(err) => {
                warn!(
                    report_id = report.id,
                    "Trading Manager report processing failed: {err:#}"
                );
                runs.push(json!({
                    "status": "error",
                    "report_id": report.id,
                    "manager_key": report.pulse_key,
                    "error": err.to_string()
                }));
            }
        }
    }

    Ok(json!({
        "status": "ok",
        "open_exchange_codes": open_codes,
        "runs": runs
    }))
}

fn trading_manager_automation_enabled(config: &serde_yaml::Value) -> bool {
    crate::config::yaml_bool(config, &["strategy", "enabled"]).unwrap_or(true)
        && crate::config::yaml_bool(config, &["strategy", "swing", "trading_manager", "enabled"])
            .unwrap_or(true)
}

async fn run_for_report(
    state: &AppState,
    report: &DecisionReport,
    overview: &JsonValue,
    open_codes: &[String],
    overlay: Option<&StrategyExperimentOverlay>,
    capital_budget: &mut CapitalBudget,
    position_exposure: &mut PositionExposure,
    buy_halt: &MonthlyLossBuyHalt,
    drawdown: &DrawdownGuard,
    quarantine_cfg: InstrumentQuarantineConfig,
    active_quarantines: &[InstrumentQuarantine],
) -> Result<JsonValue> {
    let all_candidates = candidate_orders_from_report(&report.report_json);
    let candidate_order_count = all_candidates.len();
    let buy_candidate_count = all_candidates
        .iter()
        .filter(|order| order.action == "BUY")
        .count();
    let sell_candidate_count = all_candidates
        .iter()
        .filter(|order| order.action == "SELL")
        .count();
    let candidate_limit = candidate_limit_config(state);
    let (candidates, candidate_limit_skipped) =
        enforce_candidate_symbol_limit(all_candidates, candidate_limit);
    let eligible_candidate_order_count = candidates.len();
    let excluded = excluded_symbols(state);
    let overlay_json = overlay
        .map(|overlay| overlay.clone().to_json())
        .unwrap_or(JsonValue::Null);
    let initial_capital_budget = *capital_budget;
    let position_weight = position_weight_config(state);
    let holding_limit = holding_limit_config(state);
    let concentration = concentration_config(state);
    let selected_asset_limit = selected_asset_limit_config(state);
    let hermes_preflight = hermes_decision_preflight_bundle(
        state,
        report,
        overview,
        &candidates,
        open_codes,
        &initial_capital_budget,
        position_exposure,
        position_weight,
        holding_limit,
        concentration,
        selected_asset_limit,
        &overlay_json,
        &excluded,
        buy_halt,
        drawdown,
        active_quarantines,
    )
    .await;
    let hermes_advice = request_hermes_decision_advice(state, report, &hermes_preflight)
        .await
        .unwrap_or_else(|err| {
            warn!(
                report_id = report.id,
                "Hermes decision advice degraded: {err:#}"
            );
            HermesDecisionAdvice::fallback(
                "error",
                hermes_advisory_mode(),
                format!("Hermes decision advice failed: {err:#}"),
                report.id,
            )
        });
    let hermes_conservative = hermes_advice.mode == "conservative";
    let hermes_context_self_check = hermes_context_self_check_from_raw(&hermes_advice.raw);
    let hermes_context_gate_reason = hermes_context_self_check_gate_reason(&hermes_advice);
    let hermes_global_block =
        hermes_conservative && hermes_advice.overall_recommendation == "stand_down";
    let hermes_global_review =
        hermes_conservative && hermes_advice.overall_recommendation == "review";
    let hermes_advice_delta = hermes_advice_delta(
        &candidates,
        &hermes_advice,
        hermes_context_gate_reason.as_deref(),
    );

    let min_trade_value_dkk = overlay
        .and_then(|overlay| overlay.f64_value("execution.min_trade_value_dkk"))
        .unwrap_or_else(|| {
            yaml_f64(&state.config, &["execution", "min_trade_value_dkk"]).unwrap_or(500.0)
        });
    // Commission-efficiency floor: a BUY must be large enough that the
    // exchange minimum commission stays under this share of the clip.
    // 14 days of live fills averaged ~3,500 DKK per clip at 0.67% one-way
    // commission drag, which no swing edge survives round trip.
    let max_commission_pct_per_side =
        yaml_f64(&state.config, &["execution", "max_commission_pct_per_side"])
            .unwrap_or(0.003)
            .max(0.0);
    let buy_value_floor_dkk = |symbol: &str| -> f64 {
        if max_commission_pct_per_side <= f64::EPSILON {
            return min_trade_value_dkk;
        }
        let commission_floor = crate::saxo_order::min_commission_dkk_for_exchange(
            &exchange_code(symbol).to_lowercase(),
        ) / max_commission_pct_per_side;
        min_trade_value_dkk.max(commission_floor)
    };
    let overlay_min_confluences = overlay
        .and_then(|overlay| overlay.i64_value("strategy.swing.daily_indicators.min_confluences"));
    let risk_per_trade = risk_per_trade_config(state);
    let cost_guard = cost_guard_config(state);
    let mut markov_cfg = markov_gate_config(state);
    if let Some(value) = overlay
        .and_then(|overlay| overlay.f64_value("strategy.swing.markov_gate.min_signed_signal"))
    {
        markov_cfg.min_signed_signal = value.max(0.0);
    }
    let require_approval = yaml_bool(&state.config, &["execution", "require_approval_live"])
        .unwrap_or(true)
        && yaml_string(&state.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string())
            .eq_ignore_ascii_case("live");

    let mut approved = Vec::new();
    let mut selected_buy_symbols = HashSet::new();
    let mut skipped = candidate_limit_skipped
        .iter()
        .map(|order| skip_order(order, &candidate_limit_skip_reason(candidate_limit)))
        .collect::<Vec<_>>();
    for mut order in candidates {
        let mut has_order_specific_hermes_allow = false;
        if let Some(advice) = hermes_advice.for_order(&order) {
            attach_hermes_advice(&mut order, advice, &hermes_advice);
        }
        if let Some(reason) = &hermes_context_gate_reason {
            attach_hermes_context_gate(&mut order, &hermes_advice, reason);
            skipped.push(skip_order(&order, reason));
            continue;
        }
        if let Some(advice) = hermes_advice.for_order(&order) {
            if hermes_conservative && matches!(advice.action.as_str(), "stand_down" | "review") {
                skipped.push(skip_order(
                    &order,
                    &format!("Hermes advisory {}: {}", advice.action, advice.reason),
                ));
                continue;
            }
            if hermes_conservative && advice.action == "reduce" {
                has_order_specific_hermes_allow = true;
                match advice.max_quantity {
                    Some(max_quantity) if max_quantity >= 1.0 && max_quantity < order.quantity => {
                        let original_quantity = order.quantity;
                        let new_quantity = max_quantity.floor();
                        let factor = new_quantity / original_quantity;
                        order.quantity = new_quantity;
                        if let Some(value) = order.estimated_value_dkk {
                            order.estimated_value_dkk = Some(value * factor);
                        }
                    }
                    Some(max_quantity) if max_quantity < 1.0 => {
                        skipped.push(skip_order(
                            &order,
                            &format!("Hermes advisory reduce below one share: {}", advice.reason),
                        ));
                        continue;
                    }
                    _ => {}
                }
            } else if hermes_conservative && advice.action == "allow" {
                has_order_specific_hermes_allow = true;
            }
        }
        if hermes_global_block {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Hermes advisory {} for report: {}",
                    hermes_advice.overall_recommendation, hermes_advice.summary
                ),
            ));
            continue;
        }
        if hermes_global_review && !has_order_specific_hermes_allow {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Hermes advisory review for report requires explicit order allow/reduce: {}",
                    hermes_advice.summary
                ),
            ));
            continue;
        }
        let exchange = exchange_code(&order.symbol);
        if !open_codes.iter().any(|code| code == &exchange) {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Exchange {exchange} is not currently tradable for this scheduler cycle. Open exchanges: {}.",
                    open_codes.join(", ")
                ),
            ));
            continue;
        }
        if is_excluded_symbol(&excluded, &order.symbol) {
            skipped.push(skip_order(
                &order,
                "Symbol is excluded by risk configuration.",
            ));
            continue;
        }
        if let Some(quarantine) = matching_instrument_quarantine(active_quarantines, &order) {
            if quarantine.override_active {
                if let Some(metadata) = order
                    .raw
                    .as_object_mut()
                    .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
                    .and_then(JsonValue::as_object_mut)
                {
                    metadata.insert(
                        "instrument_quarantine_override".to_string(),
                        quarantine.to_json(),
                    );
                }
            } else {
                skipped.push(skip_order(
                &order,
                &format!(
                    "Instrument quarantine active for {} {} after {} repeated {} failures; latest failure at {}, quarantine expires at {}. Sample: {}",
                    quarantine.symbol,
                    quarantine.action,
                    quarantine.failure_count,
                    quarantine.signature,
                    quarantine.latest_failure_at,
                    quarantine.expires_at,
                    quarantine.sample_error
                ),
            ));
                continue;
            }
        }
        if order.quantity <= 0.0 {
            skipped.push(skip_order(&order, "Order quantity is zero or negative."));
            continue;
        }
        let shape = order_shape_gate(&mut order);
        if !shape.approved {
            skipped.push(skip_order(&order, &shape.reason));
            continue;
        }
        if order.action == "BUY" && buy_halt.active {
            skipped.push(skip_order(
                &order,
                &format!(
                    "Monthly-loss circuit breaker active: month P/L {:.0} DKK breached the {:.0} DKK floor; new BUYs are suspended (SELLs unaffected).",
                    buy_halt.month_pnl_dkk, buy_halt.threshold_dkk
                ),
            ));
            continue;
        }
        if order.action == "BUY" && drawdown.halts_buys() {
            skipped.push(skip_order(&order, &drawdown.skip_reason()));
            continue;
        }
        let mut value_verified = false;
        if order.action == "BUY" {
            value_verified = verify_buy_value(state, &mut order).await;
            let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
            if estimated_value_dkk > capital_budget.available_buy_budget_dkk + 0.01 {
                // With a verified per-share price the order can be downsized
                // to fit the budget instead of being rejected outright.
                let per_share_dkk = if value_verified && order.quantity >= 1.0 {
                    estimated_value_dkk / order.quantity
                } else {
                    0.0
                };
                let affordable_quantity = if per_share_dkk > f64::EPSILON {
                    (capital_budget.available_buy_budget_dkk / per_share_dkk).floor()
                } else {
                    0.0
                };
                if affordable_quantity >= 1.0 {
                    let original_quantity = order.quantity;
                    order.quantity = affordable_quantity;
                    order.estimated_value_dkk = Some(per_share_dkk * affordable_quantity);
                    if let Some(metadata) = order
                        .raw
                        .as_object_mut()
                        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
                        .and_then(JsonValue::as_object_mut)
                    {
                        metadata.insert(
                            "budget_downsize".to_string(),
                            json!({
                                "original_quantity": original_quantity,
                                "downsized_quantity": affordable_quantity,
                                "per_share_dkk": per_share_dkk,
                                "available_buy_budget_dkk": capital_budget.available_buy_budget_dkk,
                            }),
                        );
                    }
                } else {
                    skipped.push(skip_order(
                        &order,
                        &format!(
                            "BUY would exceed available cash budget after buffer: requested {:.2} DKK ({}), available {:.2} DKK{}.",
                            estimated_value_dkk,
                            if value_verified { "database-verified value" } else { "model-claimed value, no verified price available" },
                            capital_budget.available_buy_budget_dkk,
                            if value_verified { "; even one share does not fit" } else { "; cannot downsize without a verified price" }
                        ),
                    ));
                    continue;
                }
            }
        }
        if order.action == "BUY" {
            let floor = buy_value_floor_dkk(&order.symbol);
            let estimated = order.estimated_value_dkk.unwrap_or(0.0);
            if estimated < floor {
                skipped.push(skip_order(
                    &order,
                    &format!(
                        "BUY of {estimated:.0} DKK is below the commission-efficiency floor of {floor:.0} DKK (minimum commission must stay under {:.2}% per side).",
                        max_commission_pct_per_side * 100.0
                    ),
                ));
                continue;
            }
        } else if order.estimated_value_dkk.unwrap_or(0.0) < min_trade_value_dkk {
            skipped.push(skip_order(
                &order,
                "Estimated trade value is below the configured minimum.",
            ));
            continue;
        }
        if order.action == "SELL" {
            let available = latest_sellable_position_quantity(state, &order.symbol)
                .await
                .unwrap_or(0.0);
            if available <= 0.0 {
                skipped.push(skip_order(
                    &order,
                    "No broker-authoritative sellable holding is available for this SELL.",
                ));
                continue;
            }
            order.quantity = order.quantity.min(available);
        }
        let verified = apply_verified_technical(state, &mut order, Utc::now().date_naive()).await;
        let mut gate = technical_gate(&order, overlay_min_confluences);
        if verified {
            gate.reason = format!("{} (database-verified daily indicators)", gate.reason);
        }
        // The Markov starter path is fallback evidence for symbols without
        // indicator coverage; it never overrides verified technicals.
        if !verified && !gate.approved && order.action == "BUY" && markov_cfg.enabled {
            let held_quantity = latest_position_quantity(state, &order.symbol)
                .await
                .unwrap_or(0.0);
            let starter_block = if !value_verified {
                Some("no database-verified price is available for starter sizing".to_string())
            } else if held_quantity > 0.0 {
                Some(format!(
                    "symbol is already held ({held_quantity}); starters only initiate new positions"
                ))
            } else if has_buy_order_today(state, &order.symbol)
                .await
                .unwrap_or(true)
            {
                Some("a BUY order for this symbol was already created today".to_string())
            } else {
                None
            };
            match starter_block {
                Some(reason) => {
                    gate.reason = format!("{} Markov starter blocked: {reason}.", gate.reason);
                }
                None => {
                    let evidence = match latest_markov_signal(state, &order.symbol).await {
                        Ok(evidence) => evidence,
                        Err(err) => {
                            warn!("Markov gate lookup failed for {}: {err:#}", order.symbol);
                            None
                        }
                    };
                    let markov_gate = markov_buy_gate(
                        &mut order,
                        evidence.as_ref(),
                        markov_cfg,
                        initial_capital_budget.total_market_value_dkk,
                        Utc::now().date_naive(),
                    );
                    if markov_gate.approved {
                        gate = markov_gate;
                    } else {
                        gate.reason =
                            format!("{} Markov fallback: {}", gate.reason, markov_gate.reason);
                    }
                }
            }
        }
        // Flatten-family SELLs are the model's risk-off exits. The role label
        // alone never admits them: this process must independently confirm an
        // under-water broker position or a negative Markov regime before a
        // flatten may bypass neutral technical sentiment.
        if !gate.approved && order.action == "SELL" && is_flatten_role(&order) {
            match verified_risk_off_evidence(state, &order.symbol, Utc::now().date_naive()).await {
                Some(evidence) => {
                    gate = GateDecision {
                        approved: true,
                        reason: format!(
                            "SELL flatten approved: model requested a risk-off exit and {evidence}."
                        ),
                    };
                }
                None => {
                    gate.reason = format!(
                        "{} Flatten fallback: no server-verified risk-off evidence (position is not under water and the Markov regime is not negative).",
                        gate.reason
                    );
                }
            }
        }
        if gate.approved && order.action == "BUY" {
            let risk_gate = risk_per_trade_gate(
                &mut order,
                initial_capital_budget.total_market_value_dkk,
                risk_per_trade,
                value_verified,
            );
            if !risk_gate.approved {
                gate = risk_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, risk_gate.reason);
                // Risk sizing can downsize an otherwise economical clip below
                // the commission floor checked above. Re-check the actual
                // queued value rather than letting the earlier, larger value
                // stand in for it.
                let floor = buy_value_floor_dkk(&order.symbol);
                let estimated = order.estimated_value_dkk.unwrap_or(0.0);
                if estimated < floor {
                    gate = GateDecision {
                        approved: false,
                        reason: format!(
                            "BUY downsized by the risk-per-trade cap to {estimated:.0} DKK, below the commission-efficiency floor of {floor:.0} DKK."
                        ),
                    };
                }
            }
        }
        if gate.approved && order.action == "BUY" {
            let holding_gate = holding_limit_gate(&mut order, holding_limit, position_exposure);
            if !holding_gate.approved {
                gate = holding_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, holding_gate.reason);
            }
        }
        if gate.approved && order.action == "BUY" {
            let concentration_gate =
                concentration_gate(&mut order, concentration, position_exposure);
            if !concentration_gate.approved {
                gate = concentration_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, concentration_gate.reason);
            }
        }
        if gate.approved && order.action == "BUY" {
            let position_weight_gate = position_weight_gate(
                &mut order,
                initial_capital_budget.total_market_value_dkk,
                position_weight,
                position_exposure,
            );
            if !position_weight_gate.approved {
                gate = position_weight_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, position_weight_gate.reason);
                // A concentration cap can downsize a clip after the earlier
                // budget/risk checks. Keep the commission floor true for the
                // final queued size rather than its original proposal.
                let floor = buy_value_floor_dkk(&order.symbol);
                let estimated = order.estimated_value_dkk.unwrap_or(0.0);
                if estimated < floor {
                    gate = GateDecision {
                        approved: false,
                        reason: format!(
                            "BUY downsized by the position-weight cap to {estimated:.0} DKK, below the commission-efficiency floor of {floor:.0} DKK."
                        ),
                    };
                }
            }
        }
        if gate.approved && order.action == "BUY" {
            let cost_gate = cost_guard_gate(&mut order, cost_guard);
            if !cost_gate.approved {
                gate = cost_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, cost_gate.reason);
            }
        }
        if gate.approved {
            let selection_gate = selected_asset_limit_gate(
                &mut order,
                selected_asset_limit,
                &mut selected_buy_symbols,
            );
            if !selection_gate.approved {
                gate = selection_gate;
            } else {
                gate.reason = format!("{} {}", gate.reason, selection_gate.reason);
            }
        }
        if gate.approved {
            if order.action == "BUY" {
                capital_budget.reserve_buy(order.estimated_value_dkk.unwrap_or(0.0));
                position_exposure
                    .reserve_buy(&order.symbol, order.estimated_value_dkk.unwrap_or(0.0));
            }
            approved.push((order, gate.reason));
        } else {
            skipped.push(skip_order(&order, &gate.reason));
        }
    }

    let mut queued_orders = Vec::new();
    for (order, approval_reason) in &approved {
        queued_orders.push(
            insert_execution_order(
                state,
                report,
                order,
                approval_reason,
                require_approval,
                &overlay_json,
            )
            .await?,
        );
    }

    let queue_result = json!({
        "status": if queued_orders.is_empty() { "completed_no_orders" } else { "queued" },
        "orders": queued_orders
    });
    // Preserve only the compact, post-gate candidate snapshot. The separate
    // shadow ledger is read-only and deliberately records no raw model or
    // broker detail.
    let missed_trade_shadow_candidates = skipped.clone();
    let hermes_advice_delta =
        with_hermes_advice_manager_outcomes(hermes_advice_delta, &approved, &skipped);
    let manager_json = json!({
        "summary": "Rust Trading Manager approved scheduled report orders using embedded daily technical gates.",
        "capital_budget": initial_capital_budget.to_json(),
        "reinvestment_diagnostics": reinvestment_diagnostics(
            &initial_capital_budget,
            candidate_order_count,
            buy_candidate_count,
            sell_candidate_count,
            approved.iter().filter(|(order, _)| order.action == "BUY").count(),
            approved.iter().filter(|(order, _)| order.action == "SELL").count(),
            &skipped,
        ),
        "remaining_buy_budget_dkk": capital_budget.available_buy_budget_dkk,
        "monthly_loss_circuit_breaker": {
            "active": buy_halt.active,
            "threshold_breached": buy_halt.threshold_breached,
            "month_pnl_dkk": buy_halt.month_pnl_dkk,
            "threshold_dkk": buy_halt.threshold_dkk,
            "soft_threshold_dkk": buy_halt.soft_threshold_dkk,
            "soft_buy_multiplier": buy_halt.soft_buy_multiplier,
            "soft_reduction_active": buy_halt.soft_reduction_active,
            "override_active": buy_halt.override_active,
            "override": buy_halt.override_value,
        },
        "drawdown_guardrail": drawdown.to_json(),
        "instrument_quarantine": {
            "enabled": quarantine_cfg.enabled,
            "lookback_days": quarantine_cfg.lookback_days,
            "min_failures": quarantine_cfg.min_failures,
            "active_days": quarantine_cfg.active_days,
            "active_count": active_quarantines.len(),
            "blocked_count": active_quarantines.iter().filter(|item| !item.override_active).count(),
            "override_count": active_quarantines.iter().filter(|item| item.override_active).count(),
            "active": active_quarantines.iter().map(InstrumentQuarantine::to_json).collect::<Vec<_>>(),
        },
        "max_commission_pct_per_side": max_commission_pct_per_side,
        "cost_guard": cost_guard.to_json(),
        "holding_limit_policy": holding_limit.to_json(),
        "concentration_policy": concentration.to_json(),
        "selected_asset_limit_policy": selected_asset_limit.to_json(),
        "position_weight_policy": position_weight.to_json(),
        "position_exposure": position_exposure.to_json(),
        "candidate_order_count": candidate_order_count,
        "eligible_candidate_order_count": eligible_candidate_order_count,
        "candidate_limit_skipped_count": candidate_limit_skipped.len(),
        "candidate_limit": candidate_limit.to_json(),
        "approved_order_count": approved.len(),
        "skipped_order_count": skipped.len(),
        "approved_orders": approved.iter().map(|(order, reason)| json!({
            "strategy_key": order.strategy_key,
            "symbol": order.symbol,
            "action": order.action,
            "gate_code": "approved",
            "final_technical": compact_hermes_preflight_technical(order),
            "final_cost_guard": compact_cost_guard(order),
            "final_holding_limit": compact_holding_limit(order),
            "final_concentration": compact_concentration(order),
            "final_position_weight": compact_position_weight(order),
            "technical_gate": reason,
        })).collect::<Vec<_>>(),
        "skipped_orders": skipped,
        "strategy_experiment_overlay": overlay_json,
        "hermes_preflight": hermes_preflight,
        "hermes_decision_advice": hermes_advice.to_json(),
        "hermes_advice_delta": hermes_advice_delta,
        "hermes_context_self_check_gate": {
            "enforced": hermes_context_gate_reason.is_some(),
            "mode": hermes_advice.mode,
            "reason": hermes_context_gate_reason,
            "context_self_check": hermes_context_self_check,
        },
        "execution_notes": [
            "Approved Hermes experiment overlays are loaded only in paper/simulation mode or Saxo SIM.",
            "Hermes decision advice is audited for every fresh report when configured; by default it is record-only. In conservative mode it can only block, reduce, or require review.",
            "In conservative mode, incomplete Hermes context self-checks block all automatic queueing even when per-order advice says allow or reduce.",
            "Orders are deduplicated by strategy_key before insertion.",
            "BUY orders are capped by cash available after the configured buffer and deployment cap.",
            "BUY orders below the commission-efficiency floor (exchange minimum commission / max_commission_pct_per_side) are rejected so fixed commissions stay a bounded share of each clip.",
            "BUY orders must also have a database-verified indicator reward that exceeds the configured lower-bound round-trip commission/slippage hurdle; this cost guard does not claim to predict realised broker costs or fill prices.",
            "BUY orders are capped to strategy.ladder.max_position_weight using persisted position values plus BUYs approved earlier in the same scheduler cycle; unavailable or invalid position-value evidence blocks the BUY rather than assuming zero exposure.",
            "Exchange and currency concentration caps use canonical symbol exchange suffixes plus the local exchange-to-currency mapping. Zero is explicit unlimited policy; a nonzero cap fails closed if the position snapshot or bucket mapping is unavailable.",
            "Distinct approved BUY symbols are capped by strategy.max_selected_assets per Decision Report after all deterministic gates; SELLs and repeated actions for a previously selected symbol remain eligible.",
            "The monthly-loss guardrail halves (by configuration) the cycle-wide BUY budget in its soft-loss band and suspends BUYs at the hard floor; SELLs are never blocked. An operator override can resume BUYs for the current month after the hard floor and remains visible in manager JSON.",
            "Instruments with repeated identical hard execution failures are quarantined per symbol/action before queueing new orders unless an operator override is active for the exact symbol/action/signature.",
            "BUY orders without technical confluence can pass as starter positions when a fresh database-verified Markov long signal supports them; starter size is capped by markov_gate.max_position_pct.",
            "SELL orders with a flatten-family strategy role (e.g. risk_reduction_flatten) can pass neutral technicals only when this process independently verifies risk-off evidence: an under-water broker position against a fresh verified close, or a fresh negative Markov regime signal. The role label alone never approves an order.",
            "SELL quantities are capped to the latest local holding quantity."
        ]
    });
    let run_status = if approved.is_empty() {
        "completed_no_orders"
    } else {
        "completed"
    };
    let run_id = insert_trading_manager_run(
        state,
        report,
        run_status,
        open_codes,
        &manager_json,
        &queue_result,
        None,
    )
    .await?;
    if run_id > 0 {
        if let Err(err) = state
            .record_hermes_counterfactuals(report.id, run_id, &hermes_advice_delta)
            .await
        {
            warn!(
                report_id = report.id,
                run_id, "Hermes counterfactual audit persistence degraded: {err:#}"
            );
        }
        if let Err(err) = state
            .record_missed_trade_shadows(report.id, run_id, &missed_trade_shadow_candidates)
            .await
        {
            warn!(
                report_id = report.id,
                run_id, "missed-trade shadow persistence degraded: {err:#}"
            );
        }
    } else {
        warn!(
            report_id = report.id,
            "Trading Manager run id missing; skipped Hermes counterfactual audit persistence"
        );
    }

    info!(
        report_id = report.id,
        run_id,
        approved_orders = approved.len(),
        skipped_orders = manager_json
            .get("skipped_orders")
            .and_then(JsonValue::as_array)
            .map_or(0, Vec::len),
        "Trading Manager processed scheduled decision report"
    );

    Ok(json!({
        "id": run_id,
        "status": run_status,
        "report_id": report.id,
        "manager_key": report.pulse_key,
        "approved_orders": approved.len(),
        "queued_orders": queue_result.get("orders").cloned().unwrap_or_else(|| json!([])),
    }))
}

async fn hermes_decision_preflight_bundle(
    state: &AppState,
    report: &DecisionReport,
    overview: &JsonValue,
    candidates: &[CandidateOrder],
    open_codes: &[String],
    capital_budget: &CapitalBudget,
    position_exposure: &PositionExposure,
    position_weight: PositionWeightConfig,
    holding_limit: HoldingLimitConfig,
    concentration: ConcentrationConfig,
    selected_asset_limit: SelectedAssetLimitConfig,
    overlay_json: &JsonValue,
    excluded_symbols: &[String],
    buy_halt: &MonthlyLossBuyHalt,
    drawdown: &DrawdownGuard,
    active_quarantines: &[InstrumentQuarantine],
) -> JsonValue {
    let today = Utc::now().date_naive();
    let markov_cfg = markov_gate_config(state);
    let cost_guard = cost_guard_config(state);
    let candidate_limit = candidate_limit_config(state);
    let latest_markov_run = match state.latest_markov_run().await {
        Ok(run) if !run.is_null() => compact_hermes_preflight_markov_run(&run),
        _ => json!({"status": "unavailable"}),
    };
    let latest_markov_run_status = text(&latest_markov_run, "status");
    let (experiments, experiment_context_status) = match state.hermes_experiments(20).await {
        Ok(rows) => (compact_hermes_preflight_experiments(&rows), "available"),
        Err(_) => (Vec::new(), "unavailable"),
    };
    let (recent_failures, failure_context_status) = match state.hermes_execution_failures(12).await
    {
        Ok(rows) => (compact_hermes_preflight_failures(&rows), "available"),
        Err(_) => (Vec::new(), "unavailable"),
    };

    let mut candidate_waterfall = Vec::with_capacity(candidates.len());
    for order in candidates {
        let (position_quantity, position_context_status) =
            match latest_position_quantity(state, &order.symbol).await {
                Ok(quantity) => (quantity, "available"),
                Err(_) => (0.0, "unavailable"),
            };
        let (sellable_quantity, sellable_context_status) = if order.action == "SELL" {
            match latest_sellable_position_quantity(state, &order.symbol).await {
                Ok(quantity) => (Some(quantity), "available"),
                Err(_) => (None, "unavailable"),
            }
        } else {
            (None, "not_applicable")
        };
        let markov_signal = match latest_markov_signal(state, &order.symbol).await {
            Ok(signal) => compact_hermes_preflight_markov_signal(
                signal.as_ref(),
                today,
                markov_cfg.max_signal_age_days,
            ),
            Err(_) => json!({"status": "unavailable"}),
        };
        let quarantine = matching_instrument_quarantine(active_quarantines, order).map(|item| {
            json!({
                "active": !item.override_active,
                "override_active": item.override_active,
                "signature": item.signature,
                "failure_count": item.failure_count,
                "expires_at": item.expires_at,
            })
        });
        candidate_waterfall.push(json!({
            "strategy_key": &order.strategy_key,
            "symbol": &order.symbol,
            "action": &order.action,
            "order_type": &order.order_type,
            "quantity": order.quantity,
            "currency": &order.currency,
            "estimated_value_dkk": order.estimated_value_dkk,
            "strategy_role": &order.strategy_role,
            "exchange": exchange_code(&order.symbol),
            "exchange_open": open_codes.iter().any(|code| code == &exchange_code(&order.symbol)),
            "risk_excluded": is_excluded_symbol(excluded_symbols, &order.symbol),
            "instrument_quarantine": quarantine,
            "current_position_quantity": position_quantity,
            "position_context_status": position_context_status,
            "current_position_value_dkk": position_exposure.value_for(&order.symbol),
            "current_holding_count": position_exposure.holding_count(),
            "already_held": position_exposure.has_position(&order.symbol),
            "concentration": position_exposure.concentration_for_symbol(&order.symbol),
            "position_value_context_status": if !position_exposure.available {
                "unavailable"
            } else if position_exposure.has_invalid_value(&order.symbol) {
                "invalid"
            } else {
                "available"
            },
            "sellable_quantity": sellable_quantity,
            "sellable_context_status": sellable_context_status,
            "technical": compact_hermes_preflight_technical(order),
            "markov": markov_signal,
        }));
    }

    json!({
        "version": 1,
        "generated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "report": {
            "id": report.id,
            "created_at": &report.created_at,
            "status": &report.status,
            "pulse_key": &report.pulse_key,
            "pulse_label": &report.pulse_label,
        },
        "portfolio": overview.get("portfolio_summary").cloned().unwrap_or(JsonValue::Null),
        "execution_capacity": overview.get("execution").and_then(|value| value.get("daily_order_capacity")).cloned().unwrap_or(JsonValue::Null),
        "capital_budget": capital_budget.to_json(),
        "cost_guard": cost_guard.to_json(),
        "position_weight_policy": position_weight.to_json(),
        "holding_limit_policy": holding_limit.to_json(),
        "concentration_policy": concentration.to_json(),
        "selected_asset_limit_policy": selected_asset_limit.to_json(),
        "position_exposure": position_exposure.to_json(),
        "candidate_limit": candidate_limit.to_json(),
        "monthly_loss_circuit_breaker": {
            "active": buy_halt.active,
            "threshold_breached": buy_halt.threshold_breached,
            "month_pnl_dkk": buy_halt.month_pnl_dkk,
            "threshold_dkk": buy_halt.threshold_dkk,
            "soft_threshold_dkk": buy_halt.soft_threshold_dkk,
            "soft_buy_multiplier": buy_halt.soft_buy_multiplier,
            "soft_reduction_active": buy_halt.soft_reduction_active,
            "override_active": buy_halt.override_active,
        },
        "drawdown_guardrail": drawdown.to_json(),
        "open_exchange_codes": open_codes,
        "strategy_experiment_overlay": overlay_json,
        "markov": {
            "max_signal_age_days": markov_cfg.max_signal_age_days,
            "min_signed_signal": markov_cfg.min_signed_signal,
            "latest_run": latest_markov_run,
        },
        "candidate_waterfall": candidate_waterfall,
        "active_experiments": experiments,
        "recent_execution_failures": recent_failures,
        "data_availability": {
            "portfolio_snapshot": overview.get("portfolio_summary").is_some(),
            "latest_markov_run": latest_markov_run_status,
            "active_experiments": experiment_context_status,
            "recent_execution_failures": failure_context_status,
        },
        "safety": {
            "saxo_sessions_excluded": true,
            "broker_mutation_endpoints_excluded": true,
            "raw_broker_payloads_excluded": true,
            "raw_execution_errors_excluded": true,
        }
    })
}

fn compact_hermes_preflight_technical(order: &CandidateOrder) -> JsonValue {
    let technical = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("technical"));
    let Some(technical) = technical else {
        return json!({"status": "unavailable"});
    };
    json!({
        "status": technical.get("status").cloned().unwrap_or(JsonValue::Null),
        "source": technical.get("source").cloned().unwrap_or(JsonValue::Null),
        "run_date": technical.get("run_date").cloned().unwrap_or(JsonValue::Null),
        "sentiment": technical.get("sentiment").cloned().unwrap_or(JsonValue::Null),
        "trend_bias": technical.get("trend_bias").cloned().unwrap_or(JsonValue::Null),
        "confluence_count": technical.get("confluence_count").cloned().unwrap_or(JsonValue::Null),
        "min_confluences": technical.get("min_confluences").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_hermes_preflight_markov_run(run: &JsonValue) -> JsonValue {
    json!({
        "status": run.get("status").cloned().unwrap_or(JsonValue::Null),
        "run_date": run.get("run_date").cloned().unwrap_or(JsonValue::Null),
        "created_at": run.get("created_at").cloned().unwrap_or(JsonValue::Null),
        "asset_count": run.get("asset_count").cloned().unwrap_or(JsonValue::Null),
        "success_count": run.get("success_count").cloned().unwrap_or(JsonValue::Null),
        "error_count": run.get("error_count").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_hermes_preflight_markov_signal(
    signal: Option<&JsonValue>,
    today: chrono::NaiveDate,
    max_age_days: i64,
) -> JsonValue {
    let Some(signal) = signal else {
        return json!({"status": "unavailable"});
    };
    let run_date = signal
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    let age_days = run_date.map(|date| (today - date).num_days());
    json!({
        "status": signal.get("status").cloned().unwrap_or(JsonValue::Null),
        "run_date": run_date.map(|date| date.to_string()),
        "age_days": age_days,
        "fresh": age_days.is_some_and(|age| age >= 0 && age <= max_age_days),
        "current_state": signal.get("current_state").cloned().unwrap_or(JsonValue::Null),
        "direction": signal.get("direction").cloned().unwrap_or(JsonValue::Null),
        "signed_signal": signal.get("signed_signal").cloned().unwrap_or(JsonValue::Null),
        "conviction": signal.get("conviction").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_hermes_preflight_experiments(rows: &[JsonValue]) -> Vec<JsonValue> {
    rows.iter()
        .filter(|row| {
            matches!(
                text(row, "status").as_str(),
                "pending_review"
                    | "approved_paper"
                    | "active_paper"
                    | "approved_sim"
                    | "active_sim"
                    | "ready_for_promotion"
            )
        })
        .map(|row| {
            json!({
                "id": row.get("id").cloned().unwrap_or(JsonValue::Null),
                "created_at": row.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "status": row.get("status").cloned().unwrap_or(JsonValue::Null),
                "changed_variable_path": row.get("changed_variable_path").cloned().unwrap_or(JsonValue::Null),
                "expected_effect": row.get("expected_effect").cloned().unwrap_or(JsonValue::Null),
            })
        })
        .collect()
}

fn compact_hermes_preflight_failures(rows: &[JsonValue]) -> Vec<JsonValue> {
    rows.iter()
        .map(|row| {
            json!({
                "created_at": row.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "symbol": row.get("symbol").cloned().unwrap_or(JsonValue::Null),
                "action": row.get("action").cloned().unwrap_or(JsonValue::Null),
                "order_type": row.get("order_type").cloned().unwrap_or(JsonValue::Null),
                "status": row.get("status").cloned().unwrap_or(JsonValue::Null),
                "failure_signature": persisted_execution_failure_signature(row)
                    .or_else(|| classify_execution_failure_signature(row))
                    .unwrap_or_else(|| "unclassified_execution_failure".to_string()),
            })
        })
        .collect()
}

async fn request_hermes_decision_advice(
    state: &AppState,
    report: &DecisionReport,
    preflight: &JsonValue,
) -> Result<HermesDecisionAdvice> {
    let mode = hermes_advisory_mode();
    let source_session_id = format!("decision-advice-{}", report.id);
    if !hermes_advisory_enabled() {
        return Ok(HermesDecisionAdvice::fallback(
            "disabled",
            mode,
            "Hermes Trading Manager advisory is disabled.".to_string(),
            report.id,
        ));
    }
    if let Some(existing) = state
        .hermes_decision_advice_by_session(&source_session_id)
        .await?
    {
        return Ok(HermesDecisionAdvice::from_row(
            existing,
            mode,
            source_session_id,
        ));
    }
    if let Some(existing) = state.hermes_decision_advice_by_report(report.id).await? {
        let source = text(&existing, "source_session_id");
        return Ok(HermesDecisionAdvice::from_row(
            existing,
            mode,
            if source.trim().is_empty() {
                source_session_id
            } else {
                source
            },
        ));
    }

    let Some(api_key) = hermes_gateway_api_key() else {
        return Ok(HermesDecisionAdvice::fallback(
            "not_configured",
            mode,
            "Hermes gateway API key is not configured for Trading Manager advisory.".to_string(),
            report.id,
        ));
    };

    let gateway_url = env::var("HERMES_GATEWAY_URL")
        .or_else(|_| env::var("HERMES_API_BASE_URL"))
        .unwrap_or_else(|_| "http://hermes-gateway.saxo:8642".to_string());
    let run_url = format!("{}/v1/runs", gateway_url.trim_end_matches('/'));
    let wait_seconds = env::var("HERMES_TRADING_MANAGER_ADVISORY_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(90)
        .min(180);
    let http_timeout_seconds = env::var("HERMES_TRADING_MANAGER_ADVISORY_HTTP_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .min(60);

    let input = format!(
        "Review decision report {} before the Rust Trading Manager queues orders. The metadata contains a sanitized, deterministic preflight bundle built from this exact manager cycle: report/candidate waterfall, portfolio and candidate exposure, Markov freshness, experiment state, and classified execution failures. Treat the bundle as supplied context, but use the configured daytrader MCP tools to independently retrieve the latest decision report, Markov signals, EOD reports, positions or overview exposure, and Hermes learnings before declaring each source reviewed. Before giving advice, complete a context_self_check with booleans for latest_report, markov_signals, end_of_day_report, current_positions, and active_experiments; set any missing source to false and explain it in notes. Then call create_decision_advice exactly once with decision_report_id {}, source_session_id {}, overall_recommendation proceed|stand_down|review, context_self_check, a concise summary, and per-order advice items using action allow|reduce|stand_down|review. You may only make the system more conservative: do not add trades, increase size, approve live orders, place orders, access Saxo sessions, or request secrets.",
        report.id, report.id, source_session_id
    );
    let payload = json!({
        "session_id": "saxo-daytrader-trading-manager-advice",
        "input": input,
        "instructions": "You are Hermes Agent acting as an advisory risk and learning reviewer for one saxo-rust decision report. You must produce an audited advisory record through the daytrader MCP create_decision_advice tool. Your advice is not an order and cannot approve or execute trades. Be specific, use current Markov and learning context, and only recommend proceed, stand_down, review, allow, reduce, or stand_down/review per candidate. Always include context_self_check so operators can audit whether you saw the latest report, Markov signals, EOD report, positions, and active experiments.",
        "metadata": {
            "source": "rust_trading_manager",
            "decision_report_id": report.id,
            "decision_pulse_key": report.pulse_key,
            "source_session_id": source_session_id,
            "advisory_mode": mode,
            "preflight": preflight,
            "required_context_self_check": {
                "fields": HERMES_CONTEXT_SELF_CHECK_FIELDS,
                "expected_sources": [
                    "get_decision_reports",
                    "get_markov_signals",
                    "get_end_of_day_reports",
                    "get_context current positions and overview exposure",
                    "list_reflections",
                    "list_experiments"
                ],
                "note": "Set booleans false when a source is unavailable; do not imply a source was reviewed unless it was actually fetched."
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(http_timeout_seconds))
        .build()
        .context("building Hermes advisory HTTP client")?;
    let submit = client
        .post(&run_url)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await;
    match submit {
        Ok(response) if response.status().is_success() => {
            info!(
                report_id = report.id,
                source_session_id, "Hermes decision advice run submitted"
            );
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(
                report_id = report.id,
                %status,
                "Hermes decision advice submit failed: {body}"
            );
            return Ok(HermesDecisionAdvice::fallback(
                "submit_failed",
                mode,
                format!("Hermes advisory submit failed with {status}: {body}"),
                report.id,
            ));
        }
        Err(err) => {
            warn!(
                report_id = report.id,
                "Hermes decision advice submit error: {err:#}"
            );
            return Ok(HermesDecisionAdvice::fallback(
                "submit_failed",
                mode,
                format!("Hermes advisory submit error: {err:#}"),
                report.id,
            ));
        }
    }

    let deadline = Utc::now() + Duration::seconds(wait_seconds as i64);
    while Utc::now() < deadline {
        if let Some(row) = state
            .hermes_decision_advice_by_session(&source_session_id)
            .await?
        {
            return Ok(HermesDecisionAdvice::from_row(row, mode, source_session_id));
        }
        if let Some(row) = state.hermes_decision_advice_by_report(report.id).await? {
            let source = text(&row, "source_session_id");
            return Ok(HermesDecisionAdvice::from_row(
                row,
                mode,
                if source.trim().is_empty() {
                    source_session_id.clone()
                } else {
                    source
                },
            ));
        }
        sleep(StdDuration::from_secs(3)).await;
    }

    // One last read after the deadline catches advice written during the final
    // sleep interval or through the report-id fallback when the session id is
    // malformed by a model/tool call.
    if let Some(row) = state
        .hermes_decision_advice_by_session(&source_session_id)
        .await?
        .or(state.hermes_decision_advice_by_report(report.id).await?)
    {
        let source = text(&row, "source_session_id");
        return Ok(HermesDecisionAdvice::from_row(
            row,
            mode,
            if source.trim().is_empty() {
                source_session_id
            } else {
                source
            },
        ));
    }

    Ok(HermesDecisionAdvice::fallback(
        "timeout",
        mode,
        format!("Hermes did not record decision advice within {wait_seconds}s."),
        report.id,
    ))
}

fn hermes_advisory_enabled() -> bool {
    env::var("HERMES_TRADING_MANAGER_ADVISORY_ENABLED")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn hermes_advisory_mode() -> String {
    match env::var("HERMES_TRADING_MANAGER_ADVISORY_MODE")
        .unwrap_or_else(|_| "record_only".to_string())
        .trim()
        .to_lowercase()
        .as_str()
    {
        "conservative" => "conservative".to_string(),
        _ => "record_only".to_string(),
    }
}

fn hermes_gateway_api_key() -> Option<String> {
    env::var("HERMES_API_SERVER_KEY")
        .or_else(|_| env::var("API_SERVER_KEY"))
        .ok()
        .or_else(|| env::var("HERMES_DAYTRADER_API_KEY").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn fresh_unmanaged_reports(state: &AppState) -> Result<Vec<DecisionReport>> {
    let max_age_hours = yaml_i64(
        &state.config,
        &[
            "strategy",
            "swing",
            "trading_manager",
            "max_report_age_hours",
        ],
    )
    .unwrap_or(DEFAULT_MAX_REPORT_AGE_HOURS);
    let cutoff = Utc::now() - Duration::hours(max_age_hours.max(1));
    let rows = sqlx::query(
        "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, report_json
         FROM decision_reports
         WHERE report_json IS NOT NULL
           AND COALESCE(analysis_pulse_key, '') <> ''
         ORDER BY id DESC
         LIMIT 30",
    )
    .fetch_all(&state.pool)
    .await
    .context("loading recent decision reports for Trading Manager")?;

    let mut reports = Vec::new();
    for row in rows.iter().map(row_to_json) {
        let report = decode_report(&row)?;
        if !is_fresh_scheduled_report(&report, cutoff) {
            continue;
        }
        if has_manager_run_for_report(state, report.id).await? {
            continue;
        }
        reports.push(report);
    }
    reports.sort_by_key(|report| report.id);
    Ok(reports)
}

/// A report is eligible for automatic queueing only when it is a completed
/// scheduled pulse with a timestamp we can verify is inside the manager's
/// freshness window. Missing or malformed timestamps fail closed: a report
/// without an auditable age must not create broker work.
fn is_fresh_scheduled_report(report: &DecisionReport, cutoff: DateTime<Utc>) -> bool {
    report.id > 0
        && matches!(report.status.as_str(), "completed" | "xai_fallback")
        && !report.pulse_key.trim().is_empty()
        && parse_report_time(&report.created_at).is_some_and(|created| created >= cutoff)
}

fn decode_report(row: &JsonValue) -> Result<DecisionReport> {
    let report_json = row.get("report_json").cloned().unwrap_or(JsonValue::Null);
    let report_json = if let Some(text) = report_json.as_str() {
        serde_json::from_str(text).context("parsing decision report_json")?
    } else {
        report_json
    };
    Ok(DecisionReport {
        id: row.get("id").and_then(JsonValue::as_i64).unwrap_or(0),
        created_at: text(row, "created_at"),
        status: text(row, "status"),
        pulse_key: text(row, "analysis_pulse_key"),
        pulse_label: text(row, "analysis_pulse_label"),
        report_json,
    })
}

async fn has_manager_run_for_report(state: &AppState, report_id: i64) -> Result<bool> {
    let row = sqlx::query(&format!(
        "SELECT id FROM trading_manager_runs WHERE report_id = {} LIMIT 1",
        report_id.max(0)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

fn candidate_orders_from_report(report_json: &JsonValue) -> Vec<CandidateOrder> {
    let orders = report_json
        .get("strategy_plan")
        .and_then(|value| value.get("swing_orders"))
        .and_then(JsonValue::as_array)
        .or_else(|| {
            report_json
                .get("suggested_trades")
                .and_then(JsonValue::as_array)
        })
        .cloned()
        .unwrap_or_default();

    orders
        .into_iter()
        .filter_map(|raw| CandidateOrder::from_json(raw).ok())
        .collect()
}

/// Builds the durable thesis snapshot attached to an approved BUY. The source
/// report stays immutable; this compact record preserves only the report-time
/// evidence needed to interpret later attribution. It must not become a new
/// approval path or an automated exit rule.
fn compact_trade_thesis(
    report: &DecisionReport,
    order: &CandidateOrder,
    approval_reason: &str,
) -> JsonValue {
    if order.action != "BUY" {
        return JsonValue::Null;
    }
    let symbol_key = normalize_symbol(&order.symbol);
    let symbol_sentiment = report
        .report_json
        .get("symbol_sentiment")
        .and_then(JsonValue::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| normalize_symbol(&text(item, "symbol")) == symbol_key)
        })
        .cloned()
        .unwrap_or(JsonValue::Null);
    let selected_asset = report
        .report_json
        .get("selected_assets")
        .and_then(JsonValue::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| normalize_symbol(&text(item, "symbol")) == symbol_key)
        })
        .cloned()
        .unwrap_or(JsonValue::Null);
    let technical = compact_hermes_preflight_technical(order);
    let markov = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("markov"))
        .map(|value| {
            json!({
                "run_date": value.get("run_date").cloned().unwrap_or(JsonValue::Null),
                "state": value.get("state").cloned().unwrap_or(JsonValue::Null),
                "direction": value.get("direction").cloned().unwrap_or(JsonValue::Null),
                "signed_signal": value.get("signed_signal").cloned().unwrap_or(JsonValue::Null),
            })
        })
        .unwrap_or(JsonValue::Null);
    json!({
        "status": "recorded",
        "evidence_source": "decision_report_and_manager_gate",
        "report_id": report.id,
        "report_created_at": report.created_at,
        "pulse_key": report.pulse_key,
        "pulse_label": report.pulse_label,
        "symbol": order.symbol,
        "strategy_key": order.strategy_key,
        "strategy_role": order.strategy_role,
        "intended_holding_window": "next_2_weeks",
        "entry_rationale": compact_trade_thesis_text(&text(&symbol_sentiment, "rationale"), 360),
        "catalyst_or_monitor": compact_trade_thesis_text(&text(&selected_asset, "notes"), 360),
        "approval_evidence": compact_trade_thesis_text(approval_reason, 420),
        "technical": technical,
        "markov": markov,
        "invalidation": "Re-evaluate on a fresh decision pulse if verified technical evidence or the Markov regime no longer supports the long setup. This records a review condition only; it is not an automatic exit rule.",
        "safety": "Read-only provenance captured before queueing. It cannot approve, size, place, amend, cancel, or retain a broker order."
    })
}

fn compact_trade_thesis_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// Every order this module queues comes from a decision report, so it carries
/// one strategy type. The pulse (scheduled EU/US or manual) is separate and
/// lives in `strategy_session` and `strategy_key`.
///
/// The value matches what the legacy Python runtime wrote through 2026-05-07
/// and what `execution_source_label` already maps to "Trading Manager", so
/// backfilled and new rows read identically.
pub(crate) const TRADING_MANAGER_STRATEGY_TYPE: &str = "swing";

impl CandidateOrder {
    fn from_json(raw: JsonValue) -> Result<Self> {
        let symbol = text(&raw, "symbol");
        let action = text(&raw, "action").to_uppercase();
        let order_type = fallback_text(&raw, "order_type", "Market");
        let strategy_key = fallback_text(
            &raw,
            "strategy_key",
            &format!(
                "rust-manager:{}:{}:{}",
                text(&raw, "session_tag"),
                symbol,
                action
            ),
        );
        let strategy_key = unique_strategy_key(strategy_key, &symbol, &action);
        Ok(Self {
            symbol,
            action,
            order_type,
            currency: optional_text(&raw, "currency"),
            quantity: value_f64(&raw, "quantity"),
            price_local: optional_f64(&raw, "price_local"),
            limit_price_local: optional_f64(&raw, "limit_price_local"),
            stop_price_local: optional_f64(&raw, "stop_price_local"),
            requested_weight_pct: optional_f64(&raw, "requested_weight_pct"),
            estimated_value_dkk: optional_f64(&raw, "estimated_value_dkk"),
            // Provenance is recorded by the component that knows it. This was
            // previously read from the model's suggested-trade JSON, but the
            // decision-report schema has no such field, so every Rust-queued
            // order carried NULL and rendered to the operator as "manual".
            strategy_type: Some(TRADING_MANAGER_STRATEGY_TYPE.to_string()),
            strategy_session: optional_text(&raw, "session_tag"),
            strategy_key,
            strategy_role: optional_text(&raw, "strategy_role"),
            raw,
        })
    }
}

fn order_shape_gate(order: &mut CandidateOrder) -> GateDecision {
    let order_type = order.order_type.trim();
    let canonical = if order_type.eq_ignore_ascii_case("market") || order_type.is_empty() {
        "Market"
    } else if order_type.eq_ignore_ascii_case("limit") {
        "Limit"
    } else if order_type.eq_ignore_ascii_case("stop") {
        "Stop"
    } else if order_type.eq_ignore_ascii_case("stoplimit")
        || order_type.eq_ignore_ascii_case("stop_limit")
        || order_type.eq_ignore_ascii_case("stop-limit")
    {
        "StopLimit"
    } else {
        return GateDecision {
            approved: false,
            reason: format!("Unsupported order_type {order_type}."),
        };
    };
    order.order_type = canonical.to_string();

    match canonical {
        "Market" => GateDecision {
            approved: true,
            reason: "Order shape is broker-compatible.".to_string(),
        },
        "Limit" => {
            if order.limit_price_local.is_none() {
                order.limit_price_local = order.price_local.filter(|price| *price > 0.0);
            }
            if order.limit_price_local.is_some_and(|price| price > 0.0) {
                GateDecision {
                    approved: true,
                    reason: "Limit order has a usable limit price.".to_string(),
                }
            } else {
                GateDecision {
                    approved: false,
                    reason:
                        "Limit orders require limit_price_local or a positive price_local fallback."
                            .to_string(),
                }
            }
        }
        "Stop" => {
            if order.stop_price_local.is_some_and(|price| price > 0.0) {
                GateDecision {
                    approved: true,
                    reason: "Stop order has a usable stop price.".to_string(),
                }
            } else {
                GateDecision {
                    approved: false,
                    reason: "Stop orders require stop_price_local.".to_string(),
                }
            }
        }
        "StopLimit" => {
            if order.limit_price_local.is_none() {
                order.limit_price_local = order.price_local.filter(|price| *price > 0.0);
            }
            if order.stop_price_local.is_none() || order.limit_price_local.is_none() {
                GateDecision {
                    approved: false,
                    reason: "StopLimit orders require stop_price_local and limit_price_local."
                        .to_string(),
                }
            } else {
                GateDecision {
                    approved: true,
                    reason: "StopLimit order has usable stop and limit prices.".to_string(),
                }
            }
        }
        _ => GateDecision {
            approved: false,
            reason: format!("Unsupported order_type {canonical}."),
        },
    }
}

fn technical_gate(order: &CandidateOrder, overlay_min_confluences: Option<i64>) -> GateDecision {
    let technical = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("technical"));
    let Some(technical) = technical else {
        return GateDecision {
            approved: false,
            reason: "No usable daily technical indicator result.".to_string(),
        };
    };
    if technical.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return GateDecision {
            approved: false,
            reason: "No usable daily technical indicator result.".to_string(),
        };
    }
    let sentiment = fallback_text(technical, "sentiment", "HOLD").to_uppercase();
    let trend_bias = fallback_text(technical, "trend_bias", "neutral").to_lowercase();
    let confluences = value_f64(technical, "confluence_count") as i64;
    let minimum = overlay_min_confluences
        .unwrap_or_else(|| value_f64(technical, "min_confluences").max(3.0) as i64)
        .max(1);

    match order.action.as_str() {
        "BUY" => {
            if !matches!(sentiment.as_str(), "BUY" | "OVERWEIGHT") {
                return GateDecision {
                    approved: false,
                    reason: format!("Technical sentiment is {sentiment}, not BUY/OVERWEIGHT."),
                };
            }
            if trend_bias != "bullish" {
                return GateDecision {
                    approved: false,
                    reason: format!("Trend bias is {trend_bias}, not bullish."),
                };
            }
            if confluences < minimum {
                return GateDecision {
                    approved: false,
                    reason: format!("Only {confluences}/{minimum} indicator confluences."),
                };
            }
            GateDecision {
                approved: true,
                reason: "BUY approved by bullish technical confluence.".to_string(),
            }
        }
        "SELL" => {
            // A model-claimed flatten role never approves a SELL by itself;
            // flatten-family exits go through the server-verified risk-off
            // fallback at the call site instead.
            if matches!(sentiment.as_str(), "SELL" | "UNDERWEIGHT") || trend_bias == "bearish" {
                return GateDecision {
                    approved: true,
                    reason: "SELL approved by deteriorating technicals.".to_string(),
                };
            }
            GateDecision {
                approved: false,
                reason: format!(
                    "SELL not approved; technical sentiment is {sentiment} with {trend_bias} trend."
                ),
            }
        }
        other => GateDecision {
            approved: false,
            reason: format!("Unsupported manager action {other}."),
        },
    }
}

/// Enforce the configured maximum loss of a BUY using only runtime-verified
/// inputs. `estimated_value_dkk` is safe here only after `verify_buy_value`;
/// ATR14 and close must come from `apply_verified_technical`, not the model.
///
/// The trade's initial loss distance is the configured automatic-stop
/// distance. If automatic stop placement is disabled or the required daily
/// data is unavailable, the BUY cannot honestly claim a bounded risk and is
/// rejected. SELLs are intentionally outside this gate.
fn risk_per_trade_gate(
    order: &mut CandidateOrder,
    total_market_value_dkk: f64,
    config: RiskPerTradeConfig,
    value_verified: bool,
) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Risk-per-trade sizing applies to BUYs only.".to_string(),
        };
    }
    if !config.protective_stops_enabled {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing requires automatic protective stops, but strategy.ladder.submit_stop_loss_after_fill is disabled.".to_string(),
        };
    }
    if !config.risk_per_trade_pct.is_finite()
        || !(0.0..=1.0).contains(&config.risk_per_trade_pct)
        || config.risk_per_trade_pct <= 0.0
    {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.swing.risk_per_trade_pct must be greater than zero and at most one.".to_string(),
        };
    }
    if !config.stop_loss_atr_multiple.is_finite() || config.stop_loss_atr_multiple <= 0.0 {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.ladder.stop_loss_atr_multiple must be positive for risk-per-trade sizing.".to_string(),
        };
    }
    if !total_market_value_dkk.is_finite() || total_market_value_dkk <= 0.0 {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing requires a positive portfolio value.".to_string(),
        };
    }
    if !value_verified || order.quantity < 1.0 {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing requires a database-verified BUY value and at least one share.".to_string(),
        };
    }
    let Some(technical) = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("technical"))
    else {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing requires database-verified daily close and ATR14."
                .to_string(),
        };
    };
    if technical
        .get("verified_from_db")
        .and_then(JsonValue::as_bool)
        != Some(true)
    {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing will not use model-supplied daily indicators."
                .to_string(),
        };
    }
    let close = value_f64(technical, "close");
    let atr14 = value_f64(technical, "atr14");
    let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    if !close.is_finite()
        || close <= 0.0
        || !atr14.is_finite()
        || atr14 <= 0.0
        || !estimated_value_dkk.is_finite()
        || estimated_value_dkk <= 0.0
    {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing requires positive database-verified close, ATR14, and DKK value.".to_string(),
        };
    }

    let per_share_dkk = estimated_value_dkk / order.quantity;
    let stop_distance_local = atr14 * config.stop_loss_atr_multiple;
    let risk_per_share_dkk = per_share_dkk * (stop_distance_local / close);
    let max_loss_dkk = total_market_value_dkk * config.risk_per_trade_pct;
    if !risk_per_share_dkk.is_finite() || risk_per_share_dkk <= 0.0 {
        return GateDecision {
            approved: false,
            reason: "Risk-per-trade sizing could not derive a positive DKK loss per share."
                .to_string(),
        };
    }
    let max_quantity = (max_loss_dkk / risk_per_share_dkk).floor();
    if max_quantity < 1.0 {
        return GateDecision {
            approved: false,
            reason: format!(
                "Risk-per-trade cap is {max_loss_dkk:.0} DKK ({:.2}% of portfolio), below one share's estimated stop loss of {risk_per_share_dkk:.0} DKK.",
                config.risk_per_trade_pct * 100.0,
            ),
        };
    }

    let original_quantity = order.quantity;
    let mut downsized = false;
    if max_quantity < original_quantity {
        order.quantity = max_quantity;
        order.estimated_value_dkk = Some(per_share_dkk * max_quantity);
        downsized = true;
    }
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "risk_per_trade".to_string(),
            json!({
                "verified_from_db": true,
                "risk_per_trade_pct": config.risk_per_trade_pct,
                "portfolio_value_dkk": total_market_value_dkk,
                "max_loss_dkk": max_loss_dkk,
                "reference_close_local": close,
                "atr14": atr14,
                "stop_loss_atr_multiple": config.stop_loss_atr_multiple,
                "stop_distance_local": stop_distance_local,
                "risk_per_share_dkk": risk_per_share_dkk,
                "original_quantity": original_quantity,
                "approved_quantity": order.quantity,
                "downsized": downsized,
                "basis": "automatic_protective_stop_atr_distance",
            }),
        );
    }
    GateDecision {
        approved: true,
        reason: if downsized {
            format!(
                "BUY downsized from {original_quantity:.0} to {:.0} shares by the {:.2}% risk-per-trade cap ({max_loss_dkk:.0} DKK at {:.2} ATR).",
                order.quantity,
                config.risk_per_trade_pct * 100.0,
                config.stop_loss_atr_multiple,
            )
        } else {
            format!(
                "BUY fits the {:.2}% risk-per-trade cap ({max_loss_dkk:.0} DKK at {:.2} ATR).",
                config.risk_per_trade_pct * 100.0,
                config.stop_loss_atr_multiple,
            )
        },
    }
}

/// Enforce the total per-symbol portfolio allocation ceiling for BUYs. The
/// incoming value has already been re-priced from database-backed market data;
/// existing exposure comes from the persisted broker/local position view, and
/// `PositionExposure` also carries BUYs approved earlier in this cycle.
fn holding_limit_gate(
    order: &mut CandidateOrder,
    config: HoldingLimitConfig,
    exposure: &PositionExposure,
) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Holding cap applies to BUYs only.".to_string(),
        };
    }
    if config.max_holdings <= 0 {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.swing.max_holdings must be a positive whole-number cap."
                .to_string(),
        };
    }
    if !exposure.available {
        return GateDecision {
            approved: false,
            reason: "Holding cap requires a persisted position snapshot, but the local position snapshot is unavailable."
                .to_string(),
        };
    }
    let holding_count = exposure.holding_count();
    let already_held = exposure.has_position(&order.symbol);
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "holding_limit".to_string(),
            json!({
                "verified_from_state": true,
                "max_holdings": config.max_holdings,
                "holding_count_before": holding_count,
                "already_held": already_held,
                "basis": "persisted_positive_quantity_positions_plus_same_cycle_approved_buys",
            }),
        );
    }
    if already_held {
        return GateDecision {
            approved: true,
            reason: format!(
                "BUY adds to an existing {} holding and does not consume a new holding slot.",
                order.symbol
            ),
        };
    }
    if holding_count >= config.max_holdings as usize {
        return GateDecision {
            approved: false,
            reason: format!(
                "Holding cap is {}; {} persisted/planned symbols already occupy every slot, so new {} BUY is blocked.",
                config.max_holdings, holding_count, order.symbol
            ),
        };
    }
    GateDecision {
        approved: true,
        reason: format!(
            "Holding cap allows a new {} position ({}/{} occupied slots before this BUY).",
            order.symbol, holding_count, config.max_holdings
        ),
    }
}

/// Enforce optional exchange and currency diversification caps. The buckets
/// are derived solely from canonical symbol suffixes and the local exchange
/// mapping, never from an untrusted provider/model currency field. When an
/// operator enables a cap, missing portfolio or bucket evidence blocks BUYs
/// rather than treating unknown exposure as zero.
fn concentration_gate(
    order: &mut CandidateOrder,
    config: ConcentrationConfig,
    exposure: &PositionExposure,
) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Concentration caps apply to BUYs only.".to_string(),
        };
    }

    let exchange = exchange_code(&order.symbol);
    let currency = currency_for_symbol(&order.symbol);
    let already_held = exposure.has_position(&order.symbol);
    let exchange_count = (!exchange.is_empty()).then(|| exposure.exchange_count(&exchange));
    let currency_count = currency
        .as_deref()
        .map(|currency| exposure.currency_count(currency));
    let exchange_unmapped = exposure.unmapped_exchange_symbols();
    let currency_unmapped = exposure.unmapped_currency_symbols();
    let caps_enabled = config.max_assets_per_exchange > 0 || config.max_assets_per_currency > 0;

    let record = |order: &mut CandidateOrder, status: &str| {
        if let Some(metadata) = order
            .raw
            .as_object_mut()
            .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
            .and_then(JsonValue::as_object_mut)
        {
            metadata.insert(
                "concentration".to_string(),
                json!({
                    "status": status,
                    "verified_from_state": exposure.available,
                    "max_assets_per_exchange": config.max_assets_per_exchange,
                    "max_assets_per_currency": config.max_assets_per_currency,
                    "exchange": exchange,
                    "currency": currency,
                    "exchange_count_before": exchange_count,
                    "currency_count_before": currency_count,
                    "already_held": already_held,
                    "unmapped_exchange_symbol_count": exchange_unmapped.len(),
                    "unmapped_currency_symbol_count": currency_unmapped.len(),
                    "basis": "persisted_positive_quantity_positions_plus_same_cycle_approved_buys",
                    "bucket_source": "canonical_symbol_exchange_suffix_and_exchange_currency_mapping",
                }),
            );
        }
    };

    if config.max_assets_per_exchange < 0 || config.max_assets_per_currency < 0 {
        record(order, "invalid_config");
        return GateDecision {
            approved: false,
            reason: "Concentration cap configuration is invalid: strategy.concentration limits must be non-negative whole numbers (0 means unlimited).".to_string(),
        };
    }
    if !caps_enabled {
        record(order, "unlimited");
        return GateDecision {
            approved: true,
            reason: "Concentration policy is explicitly unlimited.".to_string(),
        };
    }
    if !exposure.available {
        record(order, "position_snapshot_unavailable");
        return GateDecision {
            approved: false,
            reason: "Concentration cap requires a persisted position snapshot, but the local position snapshot is unavailable.".to_string(),
        };
    }
    if config.max_assets_per_exchange > 0 && exchange.is_empty() {
        record(order, "candidate_exchange_unmapped");
        return GateDecision {
            approved: false,
            reason: "Exchange concentration cap cannot classify this BUY because its symbol has no exchange suffix.".to_string(),
        };
    }
    if config.max_assets_per_exchange > 0 && !exchange_unmapped.is_empty() {
        record(order, "held_exchange_unmapped");
        return GateDecision {
            approved: false,
            reason: "Exchange concentration cap cannot be evaluated because one or more held symbols have no exchange suffix.".to_string(),
        };
    }
    if config.max_assets_per_currency > 0 && currency.is_none() {
        record(order, "candidate_currency_unmapped");
        return GateDecision {
            approved: false,
            reason: "Currency concentration cap cannot classify this BUY because its exchange has no canonical currency mapping.".to_string(),
        };
    }
    if config.max_assets_per_currency > 0 && !currency_unmapped.is_empty() {
        record(order, "held_currency_unmapped");
        return GateDecision {
            approved: false,
            reason: "Currency concentration cap cannot be evaluated because one or more held symbols have no canonical currency mapping.".to_string(),
        };
    }
    if !already_held
        && config.max_assets_per_exchange > 0
        && exchange_count.unwrap_or(usize::MAX) >= config.max_assets_per_exchange as usize
    {
        record(order, "exchange_cap_reached");
        return GateDecision {
            approved: false,
            reason: format!(
                "Exchange concentration cap is {}; {} distinct held/planned symbols already occupy the {} bucket, so new {} BUY is blocked.",
                config.max_assets_per_exchange,
                exchange_count.unwrap_or_default(),
                exchange,
                order.symbol,
            ),
        };
    }
    if !already_held
        && config.max_assets_per_currency > 0
        && currency_count.unwrap_or(usize::MAX) >= config.max_assets_per_currency as usize
    {
        record(order, "currency_cap_reached");
        return GateDecision {
            approved: false,
            reason: format!(
                "Currency concentration cap is {}; {} distinct held/planned symbols already occupy the {} bucket, so new {} BUY is blocked.",
                config.max_assets_per_currency,
                currency_count.unwrap_or_default(),
                currency.as_deref().unwrap_or("unknown"),
                order.symbol,
            ),
        };
    }

    record(
        order,
        if already_held {
            "existing_symbol"
        } else {
            "allowed"
        },
    );
    GateDecision {
        approved: true,
        reason: if already_held {
            format!(
                "Concentration caps allow an add to existing {} without consuming another bucket slot.",
                order.symbol
            )
        } else {
            format!(
                "Concentration caps allow new {} exposure ({} exchange, {} currency symbols before this BUY).",
                order.symbol,
                exchange_count.unwrap_or_default(),
                currency_count.unwrap_or_default(),
            )
        },
    }
}

fn position_weight_gate(
    order: &mut CandidateOrder,
    total_market_value_dkk: f64,
    config: PositionWeightConfig,
    exposure: &PositionExposure,
) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Position-weight cap applies to BUYs only.".to_string(),
        };
    }
    if !config.max_position_weight.is_finite()
        || !(0.0..=1.0).contains(&config.max_position_weight)
        || config.max_position_weight <= 0.0
    {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.ladder.max_position_weight must be greater than zero and at most one."
                .to_string(),
        };
    }
    if !total_market_value_dkk.is_finite() || total_market_value_dkk <= 0.0 {
        return GateDecision {
            approved: false,
            reason: "Position-weight cap requires a positive portfolio value.".to_string(),
        };
    }
    if !exposure.available {
        return GateDecision {
            approved: false,
            reason: "Position-weight cap requires persisted position values, but the local position snapshot is unavailable."
                .to_string(),
        };
    }
    if exposure.has_invalid_value(&order.symbol) {
        return GateDecision {
            approved: false,
            reason: format!(
                "Position-weight cap requires a positive DKK market value for the existing {} holding.",
                order.symbol
            ),
        };
    }
    let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    if order.quantity < 1.0 || !estimated_value_dkk.is_finite() || estimated_value_dkk <= 0.0 {
        return GateDecision {
            approved: false,
            reason: "Position-weight cap requires a positive database-verified BUY quantity and DKK value."
                .to_string(),
        };
    }

    let current_position_value_dkk = exposure.value_for(&order.symbol).unwrap_or(0.0);
    let max_position_value_dkk = total_market_value_dkk * config.max_position_weight;
    let remaining_headroom_dkk = (max_position_value_dkk - current_position_value_dkk).max(0.0);
    let original_quantity = order.quantity;
    let original_value_dkk = estimated_value_dkk;
    let mut downsized = false;

    if estimated_value_dkk > remaining_headroom_dkk + 0.01 {
        let per_share_dkk = estimated_value_dkk / order.quantity;
        let max_quantity = if per_share_dkk.is_finite() && per_share_dkk > 0.0 {
            (remaining_headroom_dkk / per_share_dkk).floor()
        } else {
            0.0
        };
        if max_quantity < 1.0 {
            return GateDecision {
                approved: false,
                reason: format!(
                    "Position-weight cap is {:.2}% ({max_position_value_dkk:.0} DKK); {} already has {:.0} DKK of persisted/planned exposure, leaving less than one share of headroom.",
                    config.max_position_weight * 100.0,
                    order.symbol,
                    current_position_value_dkk,
                ),
            };
        }
        order.quantity = max_quantity;
        order.estimated_value_dkk = Some(per_share_dkk * max_quantity);
        downsized = true;
    }

    let approved_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    let resulting_position_value_dkk = current_position_value_dkk + approved_value_dkk;
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "position_weight".to_string(),
            json!({
                "verified_from_state": true,
                "max_position_weight": config.max_position_weight,
                "portfolio_value_dkk": total_market_value_dkk,
                "max_position_value_dkk": max_position_value_dkk,
                "current_position_value_dkk": current_position_value_dkk,
                "remaining_headroom_dkk": remaining_headroom_dkk,
                "original_quantity": original_quantity,
                "original_value_dkk": original_value_dkk,
                "approved_quantity": order.quantity,
                "approved_value_dkk": approved_value_dkk,
                "resulting_position_value_dkk": resulting_position_value_dkk,
                "downsized": downsized,
                "basis": "persisted_position_value_plus_same_cycle_approved_buys",
            }),
        );
    }
    GateDecision {
        approved: true,
        reason: if downsized {
            format!(
                "BUY downsized from {original_quantity:.0} to {:.0} shares by the {:.2}% position-weight cap ({current_position_value_dkk:.0} DKK current exposure, {max_position_value_dkk:.0} DKK ceiling).",
                order.quantity,
                config.max_position_weight * 100.0,
            )
        } else {
            format!(
                "BUY fits the {:.2}% position-weight cap ({resulting_position_value_dkk:.0} DKK after order vs {max_position_value_dkk:.0} DKK ceiling).",
                config.max_position_weight * 100.0,
            )
        },
    }
}

impl CostGuardConfig {
    fn to_json(self) -> JsonValue {
        json!({
            "estimated_slippage_bps": self.estimated_slippage_bps,
            "cost_guard_multiple": self.cost_guard_multiple,
            "model": "exchange_minimum_commission_plus_one_way_slippage",
            "scope": "BUY_only",
        })
    }
}

/// Require an indicator-implied target to clear a deterministic lower-bound
/// cost hurdle. `reward_risk` is computed from the stored daily close,
/// resistance, and a 2x ATR risk distance, so neither the provider nor a
/// model-provided price can make a marginal trade appear economical.
///
/// This is intentionally not a P/L forecast. The actual commission, FX cost,
/// spread, and fill price remain broker-dependent; using the exchange minimum
/// makes the stored result a transparent floor, not an optimistic estimate.
fn cost_guard_gate(order: &mut CandidateOrder, config: CostGuardConfig) -> GateDecision {
    if order.action != "BUY" {
        return GateDecision {
            approved: true,
            reason: "Cost guard applies to BUYs only.".to_string(),
        };
    }
    if !config.estimated_slippage_bps.is_finite() || config.estimated_slippage_bps < 0.0 {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.estimated_slippage_bps must be finite and non-negative."
                .to_string(),
        };
    }
    if !config.cost_guard_multiple.is_finite() || config.cost_guard_multiple < 0.0 {
        return GateDecision {
            approved: false,
            reason: "Configured strategy.cost_guard_multiple must be finite and non-negative."
                .to_string(),
        };
    }
    let Some(technical) = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("technical"))
    else {
        return GateDecision {
            approved: false,
            reason: "Cost guard requires database-verified daily close, ATR14, and reward/risk."
                .to_string(),
        };
    };
    if technical
        .get("verified_from_db")
        .and_then(JsonValue::as_bool)
        != Some(true)
    {
        return GateDecision {
            approved: false,
            reason: "Cost guard will not use model-supplied daily indicators.".to_string(),
        };
    }
    let close = value_f64(technical, "close");
    let atr14 = value_f64(technical, "atr14");
    let reward_risk = value_f64(technical, "reward_risk");
    let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    if order.quantity < 1.0
        || !close.is_finite()
        || close <= 0.0
        || !atr14.is_finite()
        || atr14 <= 0.0
        || !reward_risk.is_finite()
        || reward_risk <= 0.0
        || !estimated_value_dkk.is_finite()
        || estimated_value_dkk <= 0.0
    {
        return GateDecision {
            approved: false,
            reason: "Cost guard requires positive database-verified close, ATR14, reward/risk, quantity, and DKK value."
                .to_string(),
        };
    }

    let per_share_dkk = estimated_value_dkk / order.quantity;
    let expected_reward_per_share_local = reward_risk * 2.0 * atr14;
    let expected_reward_dkk =
        expected_reward_per_share_local * order.quantity * (per_share_dkk / close);
    let one_way_commission_dkk = crate::saxo_order::min_commission_dkk_for_exchange(
        &exchange_code(&order.symbol).to_lowercase(),
    );
    let round_trip_commission_dkk = one_way_commission_dkk * 2.0;
    let one_way_slippage_dkk = estimated_value_dkk * (config.estimated_slippage_bps / 10_000.0);
    let required_reward_dkk =
        (round_trip_commission_dkk * config.cost_guard_multiple) + one_way_slippage_dkk;
    if !expected_reward_dkk.is_finite()
        || !one_way_commission_dkk.is_finite()
        || !required_reward_dkk.is_finite()
    {
        return GateDecision {
            approved: false,
            reason: "Cost guard could not derive finite DKK reward and cost estimates.".to_string(),
        };
    }
    let passes = expected_reward_dkk > required_reward_dkk;
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "cost_guard".to_string(),
            json!({
                "verified_from_db": true,
                "estimated_slippage_bps": config.estimated_slippage_bps,
                "cost_guard_multiple": config.cost_guard_multiple,
                "reference_close_local": close,
                "atr14": atr14,
                "reward_risk": reward_risk,
                "expected_reward_dkk": expected_reward_dkk,
                "one_way_commission_dkk": one_way_commission_dkk,
                "round_trip_commission_dkk": round_trip_commission_dkk,
                "one_way_slippage_dkk": one_way_slippage_dkk,
                "required_reward_dkk": required_reward_dkk,
                "passes": passes,
                "basis": "exchange_minimum_commission_plus_one_way_slippage",
            }),
        );
    }
    if !passes {
        return GateDecision {
            approved: false,
            reason: format!(
                "Cost guard rejected BUY: expected reward {expected_reward_dkk:.0} DKK does not exceed the {required_reward_dkk:.0} DKK lower-bound commission/slippage hurdle ({:.1}x commission plus {:.1} bps one-way slippage).",
                config.cost_guard_multiple, config.estimated_slippage_bps,
            ),
        };
    }
    GateDecision {
        approved: true,
        reason: format!(
            "Cost guard passed: expected reward {expected_reward_dkk:.0} DKK exceeds the {required_reward_dkk:.0} DKK lower-bound commission/slippage hurdle.",
        ),
    }
}

fn compact_cost_guard(order: &CandidateOrder) -> JsonValue {
    let guard = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("cost_guard"));
    let Some(guard) = guard else {
        return JsonValue::Null;
    };
    json!({
        "verified_from_db": guard.get("verified_from_db").cloned().unwrap_or(JsonValue::Null),
        "estimated_slippage_bps": guard.get("estimated_slippage_bps").cloned().unwrap_or(JsonValue::Null),
        "cost_guard_multiple": guard.get("cost_guard_multiple").cloned().unwrap_or(JsonValue::Null),
        "expected_reward_dkk": guard.get("expected_reward_dkk").cloned().unwrap_or(JsonValue::Null),
        "round_trip_commission_dkk": guard.get("round_trip_commission_dkk").cloned().unwrap_or(JsonValue::Null),
        "one_way_slippage_dkk": guard.get("one_way_slippage_dkk").cloned().unwrap_or(JsonValue::Null),
        "required_reward_dkk": guard.get("required_reward_dkk").cloned().unwrap_or(JsonValue::Null),
        "passes": guard.get("passes").cloned().unwrap_or(JsonValue::Null),
        "basis": guard.get("basis").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_position_weight(order: &CandidateOrder) -> JsonValue {
    let weight = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("position_weight"));
    let Some(weight) = weight else {
        return JsonValue::Null;
    };
    json!({
        "verified_from_state": weight.get("verified_from_state").cloned().unwrap_or(JsonValue::Null),
        "max_position_weight": weight.get("max_position_weight").cloned().unwrap_or(JsonValue::Null),
        "portfolio_value_dkk": weight.get("portfolio_value_dkk").cloned().unwrap_or(JsonValue::Null),
        "max_position_value_dkk": weight.get("max_position_value_dkk").cloned().unwrap_or(JsonValue::Null),
        "current_position_value_dkk": weight.get("current_position_value_dkk").cloned().unwrap_or(JsonValue::Null),
        "remaining_headroom_dkk": weight.get("remaining_headroom_dkk").cloned().unwrap_or(JsonValue::Null),
        "approved_value_dkk": weight.get("approved_value_dkk").cloned().unwrap_or(JsonValue::Null),
        "resulting_position_value_dkk": weight.get("resulting_position_value_dkk").cloned().unwrap_or(JsonValue::Null),
        "downsized": weight.get("downsized").cloned().unwrap_or(JsonValue::Null),
        "basis": weight.get("basis").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_holding_limit(order: &CandidateOrder) -> JsonValue {
    let limit = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("holding_limit"));
    let Some(limit) = limit else {
        return JsonValue::Null;
    };
    json!({
        "verified_from_state": limit.get("verified_from_state").cloned().unwrap_or(JsonValue::Null),
        "max_holdings": limit.get("max_holdings").cloned().unwrap_or(JsonValue::Null),
        "holding_count_before": limit.get("holding_count_before").cloned().unwrap_or(JsonValue::Null),
        "already_held": limit.get("already_held").cloned().unwrap_or(JsonValue::Null),
        "basis": limit.get("basis").cloned().unwrap_or(JsonValue::Null),
    })
}

fn compact_concentration(order: &CandidateOrder) -> JsonValue {
    let concentration = order
        .raw
        .get("strategy_metadata")
        .and_then(|value| value.get("concentration"));
    let Some(concentration) = concentration else {
        return JsonValue::Null;
    };
    json!({
        "status": concentration.get("status").cloned().unwrap_or(JsonValue::Null),
        "verified_from_state": concentration.get("verified_from_state").cloned().unwrap_or(JsonValue::Null),
        "max_assets_per_exchange": concentration.get("max_assets_per_exchange").cloned().unwrap_or(JsonValue::Null),
        "max_assets_per_currency": concentration.get("max_assets_per_currency").cloned().unwrap_or(JsonValue::Null),
        "exchange": concentration.get("exchange").cloned().unwrap_or(JsonValue::Null),
        "currency": concentration.get("currency").cloned().unwrap_or(JsonValue::Null),
        "exchange_count_before": concentration.get("exchange_count_before").cloned().unwrap_or(JsonValue::Null),
        "currency_count_before": concentration.get("currency_count_before").cloned().unwrap_or(JsonValue::Null),
        "already_held": concentration.get("already_held").cloned().unwrap_or(JsonValue::Null),
        "unmapped_exchange_symbol_count": concentration.get("unmapped_exchange_symbol_count").cloned().unwrap_or(JsonValue::Null),
        "unmapped_currency_symbol_count": concentration.get("unmapped_currency_symbol_count").cloned().unwrap_or(JsonValue::Null),
        "basis": concentration.get("basis").cloned().unwrap_or(JsonValue::Null),
    })
}

const INDICATOR_MAX_AGE_DAYS: i64 = 5;

/// Server-verified per-share price in DKK for a symbol: close from our own
/// daily indicator run (preferred) or Markov signal, converted via the
/// exchange-implied currency. None when this process has no own price data.
async fn verified_close_dkk(state: &AppState, symbol: &str) -> Option<f64> {
    let signal_close = |signal: Option<JsonValue>, key: &str| -> Option<f64> {
        let signal = signal?;
        if signal.get("status").and_then(JsonValue::as_str) != Some("ok") {
            return None;
        }
        Some(value_f64(&signal, key)).filter(|close| *close > 0.0)
    };
    let indicator = crate::daily_indicators::latest_indicator_signal(state, symbol)
        .await
        .ok()
        .flatten();
    let close = match signal_close(indicator, "close") {
        Some(close) => close,
        None => {
            let markov = latest_markov_signal(state, symbol).await.ok().flatten();
            signal_close(markov, "current_close")?
        }
    };
    let currency = crate::saxo_order::currency_for_exchange(&exchange_code(symbol).to_lowercase())?;
    let fx_rate = crate::fx::cached_or_static_fx_rate_to_dkk(&state.pool, currency).await;
    Some(close * fx_rate)
}

/// Overwrite the model-claimed estimated_value_dkk on a BUY with a value
/// computed from our own price data. Model-supplied estimates have already
/// produced orders ~6x over budget by quoting USD prices as DKK.
async fn verify_buy_value(state: &AppState, order: &mut CandidateOrder) -> bool {
    let Some(per_share_dkk) = verified_close_dkk(state, &order.symbol).await else {
        return false;
    };
    let verified_value = per_share_dkk * order.quantity.max(0.0);
    let claimed = order.estimated_value_dkk;
    order.estimated_value_dkk = Some(verified_value);
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "value_verification".to_string(),
            json!({
                "verified_from_db": true,
                "per_share_dkk": per_share_dkk,
                "verified_value_dkk": verified_value,
                "model_claimed_value_dkk": claimed,
            }),
        );
    }
    true
}

/// True when a non-terminal BUY execution order for this symbol already exists
/// today (UTC); guards against duplicate starters from near-simultaneous
/// reports while still allowing retries after pre-broker validation failures.
async fn has_buy_order_today(state: &AppState, symbol: &str) -> Result<bool> {
    let today = Utc::now().format("%Y-%m-%d");
    let row = sqlx::query(&format!(
        "SELECT id FROM execution_orders
         WHERE symbol = '{}' AND action = 'BUY' AND created_at >= '{today}T00:00:00Z'
           AND COALESCE(status, '') NOT IN (
               'execution_failed',
               'broker_cancelled',
               'cancelled',
               'rejected'
           )
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.is_some())
}

/// Replace model-supplied technical metadata with database-verified daily
/// indicator values when a fresh signal exists for the symbol. Returns true
/// when verified data was applied; the technical gate then runs on numbers
/// this process computed itself instead of model-self-reported claims.
async fn apply_verified_technical(
    state: &AppState,
    order: &mut CandidateOrder,
    today: chrono::NaiveDate,
) -> bool {
    let signal = match crate::daily_indicators::latest_indicator_signal(state, &order.symbol).await
    {
        Ok(Some(signal)) => signal,
        Ok(None) => return false,
        Err(err) => {
            warn!(
                "verified indicator lookup failed for {}: {err:#}",
                order.symbol
            );
            return false;
        }
    };
    if signal.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return false;
    }
    let Some(run_date) = signal
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
    else {
        return false;
    };
    if (today - run_date).num_days() > INDICATOR_MAX_AGE_DAYS {
        return false;
    }
    let technical = json!({
        "status": "ok",
        "source": "daily_indicators_db",
        "verified_from_db": true,
        "run_date": run_date.to_string(),
        "close": signal.get("close").cloned().unwrap_or(JsonValue::Null),
        "atr14": signal.get("atr14").cloned().unwrap_or(JsonValue::Null),
        "sentiment": signal.get("sentiment").cloned().unwrap_or(JsonValue::Null),
        "trend_bias": signal.get("trend_bias").cloned().unwrap_or(JsonValue::Null),
        "confluence_count": signal.get("confluence_count").cloned().unwrap_or(JsonValue::Null),
        "min_confluences": signal.get("min_confluences").cloned().unwrap_or(JsonValue::Null),
        "rsi14": signal.get("rsi14").cloned().unwrap_or(JsonValue::Null),
        "reward_risk": signal.get("reward_risk").cloned().unwrap_or(JsonValue::Null),
        "support": {
            "nearest_support": signal.get("nearest_support").cloned().unwrap_or(JsonValue::Null),
            "next_support": signal.get("next_support").cloned().unwrap_or(JsonValue::Null),
            "downside_to_support_pct": signal.get("downside_to_support_pct").cloned().unwrap_or(JsonValue::Null),
            "downside_after_break_pct": signal.get("downside_after_break_pct").cloned().unwrap_or(JsonValue::Null),
            "break_risk": signal.get("support_break_risk").cloned().unwrap_or(JsonValue::Null),
            "break_risk_label": signal.get("support_break_risk_label").cloned().unwrap_or(JsonValue::Null),
            "confidence": signal.get("support_confidence").cloned().unwrap_or(JsonValue::Null),
            "history_coverage": signal.get("support_history_coverage").cloned().unwrap_or(JsonValue::Null),
            "touch_count": signal.get("support_touch_count").cloned().unwrap_or(JsonValue::Null),
        },
        "confluences": signal.get("confluences_json")
            .and_then(JsonValue::as_str)
            .and_then(|raw| serde_json::from_str::<JsonValue>(raw).ok())
            .unwrap_or(JsonValue::Null),
    });
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert("technical".to_string(), technical);
        return true;
    }
    false
}

/// Latest Markov regime signal for one symbol from the most recent run.
/// Queried server-side so the gate never trusts model-reported signal values.
async fn latest_markov_signal(state: &AppState, symbol: &str) -> Result<Option<JsonValue>> {
    let sql = format!(
        "SELECT run_date, status, current_state, current_close, signed_signal, direction, conviction
         FROM markov_asset_signals
         WHERE symbol = '{}' AND run_id = (
            SELECT id FROM markov_signal_runs ORDER BY run_date DESC, created_at DESC LIMIT 1
         )
         LIMIT 1",
        sql_escape(symbol)
    );
    let row = sqlx::query(&sql).fetch_optional(&state.pool).await?;
    Ok(row.as_ref().map(row_to_json))
}

/// True when the model marked this order with a flatten-family strategy role
/// (`flatten`, `risk_reduction_flatten`, `risk_reduce_flatten`, ...). The
/// label only selects the server-verified risk-off fallback; it never
/// approves anything on its own.
fn is_flatten_role(order: &CandidateOrder) -> bool {
    order
        .strategy_role
        .as_deref()
        .is_some_and(|role| role.to_ascii_lowercase().contains("flatten"))
}

/// Server-verified evidence that a flatten-role SELL is a genuine risk-off
/// exit: the broker-held position is under water against our own fresh
/// daily-indicator close (both in the instrument's local currency), or the
/// latest Markov regime signal for the symbol is negative. Returns a
/// human-readable evidence description, or None when this process cannot
/// independently confirm the model's risk-off claim.
async fn verified_risk_off_evidence(
    state: &AppState,
    symbol: &str,
    today: chrono::NaiveDate,
) -> Option<String> {
    let open_price = sqlx::query(&format!(
        "SELECT open_price_local FROM broker_position_snapshots
         WHERE UPPER(symbol) = UPPER('{}') AND quantity > 0 AND COALESCE(can_be_closed, 1) <> 0
         LIMIT 1",
        sql_escape(symbol)
    ))
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .and_then(|row| row.try_get::<f64, _>("open_price_local").ok())
    .filter(|price| *price > 0.0);
    if let Some(open_price) = open_price {
        let fresh_close = crate::daily_indicators::latest_indicator_signal(state, symbol)
            .await
            .ok()
            .flatten()
            .filter(|signal| signal.get("status").and_then(JsonValue::as_str) == Some("ok"))
            .filter(|signal| {
                signal
                    .get("run_date")
                    .and_then(JsonValue::as_str)
                    .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
                    .is_some_and(|run_date| (today - run_date).num_days() <= INDICATOR_MAX_AGE_DAYS)
            })
            .map(|signal| value_f64(&signal, "close"))
            .filter(|close| *close > 0.0);
        if let Some(close) = fresh_close {
            if close < open_price {
                return Some(format!(
                    "the position is under water (verified close {close:.2} below broker open price {open_price:.2}, local currency)"
                ));
            }
        }
    }
    let markov = latest_markov_signal(state, symbol).await.ok().flatten()?;
    if markov.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return None;
    }
    let fresh = markov
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .is_some_and(|run_date| (today - run_date).num_days() <= INDICATOR_MAX_AGE_DAYS);
    if !fresh {
        return None;
    }
    let signed_signal = value_f64(&markov, "signed_signal");
    if signed_signal < 0.0 {
        let current_state = markov
            .get("current_state")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown");
        return Some(format!(
            "the Markov regime signal is negative ({signed_signal:.2}, state {current_state})"
        ));
    }
    None
}

/// Fallback BUY gate: a fresh database-verified Markov long signal admits a
/// size-capped starter position when daily technical confluence is missing.
fn markov_buy_gate(
    order: &mut CandidateOrder,
    evidence: Option<&JsonValue>,
    config: MarkovGateConfig,
    total_market_value_dkk: f64,
    today: chrono::NaiveDate,
) -> GateDecision {
    let Some(evidence) = evidence else {
        return GateDecision {
            approved: false,
            reason: "No Markov regime signal is available for this symbol.".to_string(),
        };
    };
    if evidence.get("status").and_then(JsonValue::as_str) != Some("ok") {
        return GateDecision {
            approved: false,
            reason: "Latest Markov regime signal for this symbol did not complete.".to_string(),
        };
    }
    let run_date = evidence
        .get("run_date")
        .and_then(JsonValue::as_str)
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    let Some(run_date) = run_date else {
        return GateDecision {
            approved: false,
            reason: "Markov regime signal has no parsable run date.".to_string(),
        };
    };
    let age_days = (today - run_date).num_days();
    if age_days > config.max_signal_age_days {
        return GateDecision {
            approved: false,
            reason: format!(
                "Markov regime signal from {run_date} is {age_days} days old (max {}).",
                config.max_signal_age_days
            ),
        };
    }
    let direction = evidence
        .get("direction")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let signed_signal = value_f64(evidence, "signed_signal");
    if direction != "long" || signed_signal < config.min_signed_signal {
        return GateDecision {
            approved: false,
            reason: format!(
                "Markov signal {signed_signal:.2} ({direction}) does not meet the long threshold {:.2}.",
                config.min_signed_signal
            ),
        };
    }

    // Cap the starter position to a small share of the portfolio.
    let max_value_dkk = total_market_value_dkk * config.max_position_pct;
    let estimated_value_dkk = order.estimated_value_dkk.unwrap_or(0.0);
    let mut scaled = false;
    if max_value_dkk > 0.0 && estimated_value_dkk > max_value_dkk {
        let factor = max_value_dkk / estimated_value_dkk;
        let scaled_quantity = (order.quantity * factor).floor();
        if scaled_quantity < 1.0 {
            return GateDecision {
                approved: false,
                reason: format!(
                    "Starter cap {max_value_dkk:.0} DKK is below the value of a single share."
                ),
            };
        }
        let quantity_ratio = scaled_quantity / order.quantity;
        order.quantity = scaled_quantity;
        order.estimated_value_dkk = Some(estimated_value_dkk * quantity_ratio);
        scaled = true;
    }
    if let Some(metadata) = order
        .raw
        .as_object_mut()
        .map(|raw| raw.entry("strategy_metadata").or_insert_with(|| json!({})))
        .and_then(JsonValue::as_object_mut)
    {
        metadata.insert(
            "markov_gate".to_string(),
            json!({
                "verified_from_db": true,
                "run_date": run_date.to_string(),
                "signed_signal": signed_signal,
                "direction": direction,
                "state": evidence.get("current_state").cloned().unwrap_or(JsonValue::Null),
                "min_signed_signal": config.min_signed_signal,
                "max_position_pct": config.max_position_pct,
                "size_capped": scaled,
            }),
        );
    }
    GateDecision {
        approved: true,
        reason: format!(
            "Starter BUY approved by database-verified Markov long signal {signed_signal:.2} (state {}); position capped at {:.0}% of portfolio{}.",
            evidence
                .get("current_state")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown"),
            config.max_position_pct * 100.0,
            if scaled {
                ", quantity scaled down to fit"
            } else {
                ""
            }
        ),
    }
}

async fn insert_execution_order(
    state: &AppState,
    report: &DecisionReport,
    order: &CandidateOrder,
    approval_reason: &str,
    require_approval: bool,
    overlay_json: &JsonValue,
) -> Result<JsonValue> {
    if let Some(existing) = existing_order_by_strategy_key(state, &order.strategy_key).await? {
        return Ok(json!({
            "id": existing,
            "strategy_key": order.strategy_key,
            "symbol": order.symbol,
            "status": "already_exists"
        }));
    }
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let status = if require_approval {
        "pending_approval"
    } else {
        "pending_execution"
    };
    let approved_at = if require_approval {
        "NULL".to_string()
    } else {
        format!("'{}'", sql_escape(&now))
    };
    // A thesis is captured at BUY admission, before the broker sees the order.
    // It is a compact, read-only record of the decision evidence, not a new
    // gate and not an instruction to retain or exit a position automatically.
    let trade_thesis = compact_trade_thesis(report, order, approval_reason);
    let request_json = json!({
        "source": "rust_trading_manager",
        "approval_reason": approval_reason,
        "decision_report_id": report.id,
        "decision_pulse_key": report.pulse_key,
        "strategy_experiment_overlay": overlay_json,
        "order": order.raw,
    });
    let sql = format!(
        "INSERT INTO execution_orders (
            created_at, report_id, symbol, action, order_type, mode, status, adapter,
            requested_weight_pct, quantity, price_local, limit_price_local, stop_price_local,
            currency, estimated_value_dkk, approval_required, approved_at, strategy_type,
            strategy_session, strategy_key, strategy_role, request_json, execution_result_json,
            error_text, trade_thesis_json
        ) VALUES (
            '{}', {}, '{}', '{}', '{}', '{}', '{}', '{}',
            {}, {}, {}, {}, {},
            {}, {}, {}, {}, {}, {}, '{}', {}, '{}', NULL, NULL, {}
        )",
        sql_escape(&now),
        report.id,
        sql_escape(&order.symbol),
        sql_escape(&order.action),
        sql_escape(&order.order_type),
        sql_escape(
            &yaml_string(&state.config, &["execution", "mode"])
                .unwrap_or_else(|| "simulation".to_string())
        ),
        status,
        sql_escape(
            &yaml_string(&state.config, &["execution", "adapter"])
                .unwrap_or_else(|| "saxo".to_string())
        ),
        sql_num(order.requested_weight_pct),
        order.quantity,
        sql_num(order.price_local),
        sql_num(order.limit_price_local),
        sql_num(order.stop_price_local),
        sql_opt_text(order.currency.as_deref()),
        sql_num(order.estimated_value_dkk),
        if require_approval { 1 } else { 0 },
        approved_at,
        sql_opt_text(order.strategy_type.as_deref()),
        sql_opt_text(order.strategy_session.as_deref()),
        sql_escape(&order.strategy_key),
        sql_opt_text(order.strategy_role.as_deref()),
        sql_escape(&serde_json::to_string(&request_json)?),
        sql_json(&trade_thesis),
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("inserting Trading Manager execution order")?;
    let id = existing_order_by_strategy_key(state, &order.strategy_key)
        .await?
        .unwrap_or(0);
    if id > 0 {
        insert_order_event(state, id, "queued_by_trading_manager", &request_json).await?;
    }
    Ok(json!({
        "id": id,
        "strategy_key": order.strategy_key,
        "symbol": order.symbol,
        "action": order.action,
        "status": status
    }))
}

async fn insert_trading_manager_run(
    state: &AppState,
    report: &DecisionReport,
    status: &str,
    open_codes: &[String],
    manager_json: &JsonValue,
    queue_result: &JsonValue,
    error_text: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let manager_kind = report
        .pulse_key
        .split(':')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("scheduled_report");
    let target_at_utc = report.created_at.clone();
    let sql = format!(
        "INSERT INTO trading_manager_runs (
            created_at, manager_key, manager_kind, manager_label, target_at_utc, report_id,
            status, open_exchange_codes_json, technical_json, manager_json, queue_result_json,
            error_text
        ) VALUES (
            '{}', '{}', '{}', '{}', '{}', {}, '{}', '{}', '{{}}', '{}', '{}', {}
        )",
        sql_escape(&now),
        sql_escape(&report.pulse_key),
        sql_escape(manager_kind),
        sql_escape(&report.pulse_label),
        sql_escape(&target_at_utc),
        report.id,
        sql_escape(status),
        sql_escape(&serde_json::to_string(open_codes)?),
        sql_escape(&serde_json::to_string(manager_json)?),
        sql_escape(&serde_json::to_string(queue_result)?),
        sql_opt_text(error_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording Trading Manager run")?;
    let row = sqlx::query(&format!(
        "SELECT id FROM trading_manager_runs WHERE report_id = {} ORDER BY id DESC LIMIT 1",
        report.id
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<i64, _>("id").ok())
        .unwrap_or(0))
}

async fn existing_order_by_strategy_key(
    state: &AppState,
    strategy_key: &str,
) -> Result<Option<i64>> {
    let row = sqlx::query(&format!(
        "SELECT id FROM execution_orders WHERE strategy_key = '{}' ORDER BY id DESC LIMIT 1",
        sql_escape(strategy_key)
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.and_then(|row| row.try_get::<i64, _>("id").ok()))
}

async fn insert_order_event(
    state: &AppState,
    order_id: i64,
    event_type: &str,
    payload: &JsonValue,
) -> Result<()> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let payload_text = serde_json::to_string(payload)?;
    let signature = format!("{event_type}:{order_id}");
    let sql = format!(
        "INSERT INTO execution_order_events (
            created_at, execution_order_id, broker_order_id, event_type, broker_status,
            broker_substatus, broker_quantity, broker_price_local, event_signature,
            raw_payload_json
        ) VALUES (
            '{}', {}, NULL, '{}', NULL, NULL, NULL, NULL, '{}', '{}'
        )
        ON CONFLICT(event_signature) DO NOTHING",
        sql_escape(&now),
        order_id,
        sql_escape(event_type),
        sql_escape(&signature),
        sql_escape(&payload_text)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("recording execution order event")?;
    Ok(())
}

fn instrument_quarantine_config(state: &AppState) -> InstrumentQuarantineConfig {
    InstrumentQuarantineConfig {
        enabled: yaml_bool(&state.config, &["risk", "instrument_quarantine", "enabled"])
            .unwrap_or(true),
        lookback_days: yaml_i64(
            &state.config,
            &["risk", "instrument_quarantine", "lookback_days"],
        )
        .unwrap_or(14)
        .max(1),
        min_failures: yaml_i64(
            &state.config,
            &["risk", "instrument_quarantine", "min_failures"],
        )
        .unwrap_or(3)
        .max(1) as usize,
        active_days: yaml_i64(
            &state.config,
            &["risk", "instrument_quarantine", "active_days"],
        )
        .unwrap_or(14)
        .max(1),
    }
}

async fn active_instrument_quarantines(state: &AppState) -> Result<Vec<InstrumentQuarantine>> {
    let cfg = instrument_quarantine_config(state);
    if !cfg.enabled {
        return Ok(Vec::new());
    }
    let cutoff = (Utc::now() - Duration::days(cfg.lookback_days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows = sqlx::query(&format!(
        // Protective stops are excluded on purpose. Cancelling one is a healthy,
        // deliberate act -- it is how a decided exit claims Saxo's single
        // permitted resting sell -- but `update_order_broker_status` writes
        // `error_text` for every `broker_cancelled` row, so without this the
        // runtime would accumulate quarantine strikes against exactly the
        // symbols it is successfully trading and eventually refuse to trade
        // them. Quarantine is meant to catch instruments the broker keeps
        // rejecting, not our own housekeeping.
        "SELECT id, created_at, symbol, action, status, error_text, execution_result_json \
             FROM execution_orders \
             WHERE created_at >= '{}' \
               AND COALESCE(strategy_type, '') <> 'protective_stop' \
               AND (error_text IS NOT NULL \
                    OR lower(status) LIKE '%failed%' \
                    OR lower(status) LIKE '%rejected%' \
                    OR status IN ('invalid_quantity', 'broker_expired')) \
             ORDER BY created_at ASC, id ASC \
             LIMIT 1000",
        sql_escape(&cutoff)
    ))
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| row_to_json(&row))
    .collect::<Vec<_>>();
    Ok(active_instrument_quarantines_from_rows(
        &rows,
        Utc::now(),
        cfg,
    ))
}

fn active_instrument_quarantines_from_rows(
    rows: &[JsonValue],
    now: DateTime<Utc>,
    cfg: InstrumentQuarantineConfig,
) -> Vec<InstrumentQuarantine> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut grouped: HashMap<(String, String, String), (usize, DateTime<Utc>, String)> =
        HashMap::new();
    for row in rows {
        let symbol = text(row, "symbol");
        let action = text(row, "action").to_uppercase();
        if symbol.is_empty() || action.is_empty() {
            continue;
        }
        let Some(signature) = classify_execution_failure_signature(row) else {
            continue;
        };
        let Some(created_at) = parse_report_time(&text(row, "created_at")) else {
            continue;
        };
        let sample_error = failure_sample_text(row);
        let key = (symbol, action, signature);
        grouped
            .entry(key)
            .and_modify(|entry| {
                entry.0 += 1;
                if created_at > entry.1 {
                    entry.1 = created_at;
                    entry.2 = sample_error.clone();
                }
            })
            .or_insert((1, created_at, sample_error));
    }

    let mut quarantines = grouped
        .into_iter()
        .filter_map(
            |((symbol, action, signature), (failure_count, latest, sample_error))| {
                if failure_count < cfg.min_failures {
                    return None;
                }
                let expires_at = latest + Duration::days(cfg.active_days);
                if expires_at <= now {
                    return None;
                }
                Some(InstrumentQuarantine {
                    symbol,
                    action,
                    signature,
                    failure_count,
                    latest_failure_at: latest.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    sample_error,
                    override_active: false,
                    override_notes: String::new(),
                    override_updated_at: String::new(),
                })
            },
        )
        .collect::<Vec<_>>();
    quarantines.sort_by(|left, right| {
        left.symbol
            .cmp(&right.symbol)
            .then(left.action.cmp(&right.action))
            .then(left.signature.cmp(&right.signature))
    });
    quarantines
}

fn instrument_quarantine_override_key(symbol: &str, action: &str, signature: &str) -> String {
    format!(
        "{}|{}|{}",
        symbol.trim(),
        action.trim().to_uppercase(),
        signature.trim()
    )
}

fn apply_instrument_quarantine_overrides(
    mut quarantines: Vec<InstrumentQuarantine>,
    overrides: &JsonValue,
) -> Vec<InstrumentQuarantine> {
    let mut by_key: HashMap<String, JsonValue> = HashMap::new();
    if let Some(items) = overrides.get("overrides").and_then(JsonValue::as_array) {
        for item in items {
            if item
                .get("enabled")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
            {
                by_key.insert(
                    instrument_quarantine_override_key(
                        &text(item, "symbol"),
                        &text(item, "action"),
                        &text(item, "signature"),
                    ),
                    item.clone(),
                );
            }
        }
    }
    for quarantine in &mut quarantines {
        if let Some(override_item) = by_key.get(&instrument_quarantine_override_key(
            &quarantine.symbol,
            &quarantine.action,
            &quarantine.signature,
        )) {
            quarantine.override_active = true;
            quarantine.override_notes = text(override_item, "notes");
            quarantine.override_updated_at = text(override_item, "updated_at");
        }
    }
    quarantines
}

fn matching_instrument_quarantine<'a>(
    quarantines: &'a [InstrumentQuarantine],
    order: &CandidateOrder,
) -> Option<&'a InstrumentQuarantine> {
    quarantines.iter().find(|quarantine| {
        quarantine.symbol == order.symbol
            && (quarantine.action == order.action || quarantine.action == "*")
    })
}

fn persisted_execution_failure_signature(row: &JsonValue) -> Option<String> {
    let payload = row.get("execution_result_json")?;
    let payload = match payload {
        JsonValue::String(value) => serde_json::from_str::<JsonValue>(value).ok()?,
        value => value.clone(),
    };
    let code = payload
        .get("error_taxonomy")
        .and_then(|taxonomy| taxonomy.get("code"))
        .and_then(JsonValue::as_str)?;
    matches!(
        code,
        "broker_state_unknown"
            | "broker_cancelled"
            | "broker_rejected"
            | "commission_setup"
            | "done_for_day"
            | "insufficient_cash"
            | "instrument_not_tradable"
            | "market_closed"
            | "order_expired"
            | "position_quantity"
            | "price_invalid"
            | "quantity"
            | "rate_limited"
            | "session_expired"
            | "tick_size"
            | "unknown"
    )
    .then(|| code.to_string())
}

fn classify_execution_failure_signature(row: &JsonValue) -> Option<String> {
    let status = text(row, "status").to_lowercase();
    let combined = format!(
        "{} {} {}",
        text(row, "error_text"),
        text(row, "execution_result_json"),
        status
    )
    .to_lowercase();

    if combined.contains("does not have any commissions configured")
        || combined.contains("commissions configured")
    {
        return Some("commission_not_configured".to_string());
    }
    if combined.contains("tick")
        || combined.contains("increment")
        || combined.contains("invalid price")
        || combined.contains("price step")
    {
        return Some("tick_size_or_price_increment".to_string());
    }
    if combined.contains("not owned")
        || combined.contains("notowned")
        || combined.contains("insufficient holdings")
        || combined.contains("sell quantity")
        || combined.contains("holdings quantity")
    {
        return Some("sell_not_owned_or_flattened".to_string());
    }
    if combined.contains("resolving saxo instrument")
        || combined.contains("instrument not found")
        || combined.contains("could not resolve")
        || combined.contains("no instrument")
    {
        return Some("instrument_resolution".to_string());
    }
    if combined.contains("not tradable")
        || combined.contains("not supported")
        || combined.contains("unsupported order")
    {
        return Some("instrument_not_tradable".to_string());
    }
    None
}

fn failure_sample_text(row: &JsonValue) -> String {
    let text = [text(row, "error_text"), text(row, "status")]
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "execution failure".to_string());
    if text.chars().count() > 220 {
        let mut truncated = text.chars().take(220).collect::<String>();
        truncated.push_str("...");
        truncated
    } else {
        text
    }
}

async fn latest_position_quantity(state: &AppState, symbol: &str) -> Result<f64> {
    if broker_position_snapshots_available(state).await? {
        let row = sqlx::query(&format!(
            "SELECT COALESCE(SUM(quantity), 0) AS quantity
             FROM broker_position_snapshots
             WHERE UPPER(symbol) = UPPER('{}')",
            sql_escape(symbol)
        ))
        .fetch_optional(&state.pool)
        .await?;
        return Ok(row
            .and_then(|row| row.try_get::<f64, _>("quantity").ok())
            .unwrap_or(0.0));
    }

    let latest_batch = sqlx::query(
        "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?
    .and_then(|row| row.try_get::<String, _>("batch_id").ok());
    let where_batch = latest_batch
        .as_ref()
        .map(|batch| format!(" AND batch_id = '{}'", sql_escape(batch)))
        .unwrap_or_default();
    let row = sqlx::query(&format!(
        "SELECT quantity FROM position_snapshots WHERE UPPER(symbol) = UPPER('{}') AND excluded = 0{} ORDER BY id DESC LIMIT 1",
        sql_escape(symbol),
        where_batch
    ))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row
        .and_then(|row| row.try_get::<f64, _>("quantity").ok())
        .unwrap_or(0.0))
}

async fn latest_sellable_position_quantity(state: &AppState, symbol: &str) -> Result<f64> {
    if broker_position_snapshots_available(state).await? {
        let row = sqlx::query(&format!(
            "SELECT COALESCE(SUM(quantity), 0) AS quantity
             FROM broker_position_snapshots
             WHERE UPPER(symbol) = UPPER('{}')
               AND COALESCE(can_be_closed, 1) <> 0",
            sql_escape(symbol)
        ))
        .fetch_optional(&state.pool)
        .await?;
        return Ok(row
            .and_then(|row| row.try_get::<f64, _>("quantity").ok())
            .unwrap_or(0.0));
    }
    latest_position_quantity(state, symbol).await
}

async fn broker_position_snapshots_available(state: &AppState) -> Result<bool> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM broker_position_snapshots")
        .fetch_optional(&state.pool)
        .await?;
    Ok(row
        .and_then(|row| row.try_get::<i64, _>("count").ok())
        .unwrap_or(0)
        > 0)
}

fn skip_order(order: &CandidateOrder, reason: &str) -> JsonValue {
    json!({
        "strategy_key": order.strategy_key,
        "symbol": order.symbol,
        "action": order.action,
        "quantity": order.quantity,
        "currency": order.currency,
        "reference_price_local": order.limit_price_local.or(order.price_local),
        "gate_code": candidate_gate_reason_code(reason),
        // The final gate may have replaced model-provided technical metadata
        // with a fresh database signal. Persist only compact safe fields so the
        // audit UI can explain the decision without raw inputs.
        "final_technical": compact_hermes_preflight_technical(order),
        "final_cost_guard": compact_cost_guard(order),
        "final_holding_limit": compact_holding_limit(order),
        "final_concentration": compact_concentration(order),
        "final_position_weight": compact_position_weight(order),
        "technical_gate": reason,
    })
}

fn candidate_gate_reason_code(reason: &str) -> &'static str {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.starts_with("hermes context") {
        "hermes_context"
    } else if normalized.starts_with("hermes advisory") {
        "hermes_advice"
    } else if normalized.starts_with("candidate limit") {
        "candidate_limit"
    } else if normalized.starts_with("exchange concentration cap") {
        "concentration_exchange"
    } else if normalized.starts_with("currency concentration cap") {
        "concentration_currency"
    } else if normalized.starts_with("concentration cap configuration")
        || normalized.starts_with("concentration cap requires")
    {
        "concentration"
    } else if normalized.starts_with("exchange ") {
        "market_open"
    } else if normalized.starts_with("symbol is excluded") {
        "risk_exclusion"
    } else if normalized.starts_with("instrument quarantine") {
        "instrument_quarantine"
    } else if normalized.starts_with("order quantity") {
        "quantity"
    } else if normalized.starts_with("unsupported order")
        || normalized.contains("orders require")
        || normalized.contains("order shape")
    {
        "order_shape"
    } else if normalized.starts_with("monthly-loss circuit breaker") {
        "monthly_loss_breaker"
    } else if normalized.starts_with("portfolio drawdown guardrail") {
        "drawdown_guardrail"
    } else if normalized.starts_with("buy would exceed available cash budget") {
        "cash_budget"
    } else if normalized.contains("risk-per-trade") {
        "risk_per_trade"
    } else if normalized.contains("position-weight cap") {
        "position_weight"
    } else if normalized.contains("holding cap") {
        "max_holdings"
    } else if normalized.starts_with("selection cap") {
        "max_selected_assets"
    } else if normalized.starts_with("cost guard") {
        "cost_guard"
    } else if normalized.contains("commission-efficiency floor") {
        "commission_floor"
    } else if normalized.starts_with("estimated trade value") {
        "minimum_trade_value"
    } else if normalized.starts_with("no broker-authoritative sellable") {
        "sellable_quantity"
    } else if normalized.contains("markov") {
        "markov"
    } else if normalized.contains("technical") || normalized.starts_with("only ") {
        "technical"
    } else {
        "other"
    }
}

fn unique_strategy_key(strategy_key: String, symbol: &str, action: &str) -> String {
    if strategy_key.contains(symbol) {
        strategy_key
    } else {
        format!("{strategy_key}:{symbol}:{action}")
    }
}

async fn approved_strategy_experiment_overlay(
    state: &AppState,
) -> Result<Option<StrategyExperimentOverlay>> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let saxo_environment =
        yaml_string(&state.config, &["saxo", "environment"]).unwrap_or_else(|| "SIM".into());
    if !experiment_overlays_allowed(&execution_mode, &saxo_environment) {
        return Ok(None);
    }

    let statuses = EXPERIMENT_STATUS_ALLOWLIST
        .iter()
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = sqlx::query(&format!(
        "SELECT id, created_at, status, baseline_id, goal_version, hypothesis,
            changed_variable_path, old_value_json, new_value_json, expected_effect,
            risk_notes, evidence_json, approval_json, metrics_json, source_session_id,
            raw_payload_json
         FROM strategy_experiments
         WHERE status IN ({statuses})
         ORDER BY created_at DESC, id DESC
         LIMIT 10"
    ))
    .fetch_all(&state.pool)
    .await
    .context("loading approved Hermes strategy experiment overlay")?;

    for row in rows.iter().map(row_to_json) {
        if let Some(overlay) = StrategyExperimentOverlay::from_row(&row) {
            return Ok(Some(overlay));
        }
        warn!(
            experiment_id = text(&row, "id"),
            variable = text(&row, "changed_variable_path"),
            "Ignoring unsupported Hermes strategy experiment overlay"
        );
    }
    Ok(None)
}

/// Return the same bounded experiment selection the Trading Manager uses,
/// without creating a manager run or mutating configuration/broker state.
/// This gives the dashboard an audit projection that cannot drift from the
/// runtime overlay eligibility rules.
pub(crate) async fn strategy_experiment_overlay_audit(state: &AppState) -> Result<JsonValue> {
    let execution_mode =
        yaml_string(&state.config, &["execution", "mode"]).unwrap_or_else(|| "simulation".into());
    let saxo_environment =
        yaml_string(&state.config, &["saxo", "environment"]).unwrap_or_else(|| "SIM".into());
    let overlays_allowed = experiment_overlays_allowed(&execution_mode, &saxo_environment);
    let selected = if overlays_allowed {
        approved_strategy_experiment_overlay(state).await?
    } else {
        None
    };

    Ok(json!({
        "state": if !overlays_allowed {
            "disabled_live_environment"
        } else if selected.is_some() {
            "selected_for_next_cycle"
        } else {
            "no_supported_candidate"
        },
        "execution_mode": execution_mode,
        "saxo_environment": saxo_environment,
        "overlays_allowed": overlays_allowed,
        "candidate": selected.map(|overlay| overlay.to_json()).unwrap_or(JsonValue::Null),
    }))
}

fn experiment_overlays_allowed(execution_mode: &str, saxo_environment: &str) -> bool {
    !execution_mode.eq_ignore_ascii_case("live") || saxo_environment.eq_ignore_ascii_case("SIM")
}

impl StrategyExperimentOverlay {
    fn from_row(row: &JsonValue) -> Option<Self> {
        let changed_variable_path = text(row, "changed_variable_path");
        if !SUPPORTED_EXPERIMENT_VARIABLES
            .iter()
            .any(|path| *path == changed_variable_path)
        {
            return None;
        }
        let new_value = row
            .get("new_value_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        json_f64_value(&new_value)?;
        Some(Self {
            id: text(row, "id"),
            status: text(row, "status"),
            goal_version: row
                .get("goal_version")
                .and_then(JsonValue::as_i64)
                .unwrap_or(1),
            changed_variable_path,
            old_value: row
                .get("old_value_json")
                .cloned()
                .unwrap_or(JsonValue::Null),
            new_value,
            hypothesis: text(row, "hypothesis"),
        })
    }

    fn f64_value(&self, variable_path: &str) -> Option<f64> {
        (self.changed_variable_path == variable_path)
            .then(|| json_f64_value(&self.new_value))
            .flatten()
    }

    fn i64_value(&self, variable_path: &str) -> Option<i64> {
        self.f64_value(variable_path)
            .map(|value| value.round() as i64)
    }

    fn to_json(&self) -> JsonValue {
        json!({
            "id": self.id,
            "status": self.status,
            "goal_version": self.goal_version,
            "changed_variable_path": self.changed_variable_path,
            "old_value": self.old_value,
            "new_value": self.new_value,
            "hypothesis": self.hypothesis,
            "scope": "paper_or_saxo_sim_only"
        })
    }
}

fn json_f64_value(value: &JsonValue) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
}

fn capital_budget_from_overview(
    overview: &JsonValue,
    overlay_min_cash_buffer_pct: Option<f64>,
) -> CapitalBudget {
    let summary = overview
        .get("portfolio_summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let cash_policy = overview
        .get("settings")
        .and_then(|value| value.get("cash_buffer"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let total_market_value_dkk = value_f64(&summary, "total_market_value_dkk");
    let invested_market_value_dkk = value_f64(&summary, "invested_market_value_dkk");
    let cash_balance_dkk = value_f64(&summary, "cash_balance_dkk");
    let min_cash_buffer_pct = overlay_min_cash_buffer_pct
        .unwrap_or_else(|| value_f64(&cash_policy, "min_cash_buffer_pct"))
        .clamp(0.0, 1.0);
    let max_deployment_pct = value_f64(&cash_policy, "max_deployment_pct").clamp(0.0, 1.0);
    let reinvestment_pressure_threshold_pct = cash_policy
        .get("reinvestment_pressure_threshold_pct")
        .map(|_| value_f64(&cash_policy, "reinvestment_pressure_threshold_pct"))
        .unwrap_or(0.05)
        .max(0.0);
    let required_cash_buffer_dkk = (total_market_value_dkk * min_cash_buffer_pct).max(0.0);
    let deployment_cap_dkk = if max_deployment_pct > 0.0 {
        total_market_value_dkk * max_deployment_pct
    } else {
        total_market_value_dkk
    };
    let available_cash_above_buffer_dkk = (cash_balance_dkk - required_cash_buffer_dkk).max(0.0);
    let remaining_deployment_capacity_dkk =
        (deployment_cap_dkk - invested_market_value_dkk).max(0.0);
    let available_buy_budget_dkk =
        available_cash_above_buffer_dkk.min(remaining_deployment_capacity_dkk);
    let cash_pct = if total_market_value_dkk > 0.0 {
        cash_balance_dkk / total_market_value_dkk
    } else {
        0.0
    };
    let excess_cash_pct = (cash_pct - min_cash_buffer_pct).max(0.0);
    let reinvestment_pressure_active =
        excess_cash_pct >= reinvestment_pressure_threshold_pct && available_buy_budget_dkk > 0.0;
    CapitalBudget {
        cash_balance_dkk,
        total_market_value_dkk,
        invested_market_value_dkk,
        cash_pct,
        min_cash_buffer_pct,
        max_deployment_pct,
        reinvestment_pressure_threshold_pct,
        required_cash_buffer_dkk,
        available_cash_above_buffer_dkk,
        unreduced_available_buy_budget_dkk: available_buy_budget_dkk,
        available_buy_budget_dkk,
        remaining_deployment_capacity_dkk,
        excess_cash_pct,
        reinvestment_pressure_active,
    }
}

impl CapitalBudget {
    fn reserve_buy(&mut self, estimated_value_dkk: f64) {
        self.available_buy_budget_dkk =
            (self.available_buy_budget_dkk - estimated_value_dkk.max(0.0)).max(0.0);
    }

    fn apply_buy_multiplier(&mut self, multiplier: f64) {
        self.available_buy_budget_dkk *= multiplier.clamp(0.0, 1.0);
        self.reinvestment_pressure_active = self.excess_cash_pct
            >= self.reinvestment_pressure_threshold_pct
            && self.available_buy_budget_dkk > 0.0;
    }

    fn to_json(self) -> JsonValue {
        json!({
            "cash_balance_dkk": self.cash_balance_dkk,
            "total_market_value_dkk": self.total_market_value_dkk,
            "invested_market_value_dkk": self.invested_market_value_dkk,
            "cash_pct": self.cash_pct,
            "min_cash_buffer_pct": self.min_cash_buffer_pct,
            "max_deployment_pct": self.max_deployment_pct,
            "reinvestment_pressure_threshold_pct": self.reinvestment_pressure_threshold_pct,
            "required_cash_buffer_dkk": self.required_cash_buffer_dkk,
            "available_cash_above_buffer_dkk": self.available_cash_above_buffer_dkk,
            "unreduced_available_buy_budget_dkk": self.unreduced_available_buy_budget_dkk,
            "available_buy_budget_dkk": self.available_buy_budget_dkk,
            "remaining_deployment_capacity_dkk": self.remaining_deployment_capacity_dkk,
            "excess_cash_pct": self.excess_cash_pct,
            "reinvestment_pressure_active": self.reinvestment_pressure_active,
        })
    }
}

fn reinvestment_diagnostics(
    budget: &CapitalBudget,
    candidate_order_count: usize,
    buy_candidate_count: usize,
    sell_candidate_count: usize,
    approved_buy_count: usize,
    approved_sell_count: usize,
    skipped: &[JsonValue],
) -> JsonValue {
    let skipped_buy_count = skipped
        .iter()
        .filter(|order| order.get("action").and_then(JsonValue::as_str) == Some("BUY"))
        .count();
    let skipped_sell_count = skipped
        .iter()
        .filter(|order| order.get("action").and_then(JsonValue::as_str) == Some("SELL"))
        .count();
    let mut blocked_buy_gate_counts = HashMap::<String, usize>::new();
    for order in skipped
        .iter()
        .filter(|order| order.get("action").and_then(JsonValue::as_str) == Some("BUY"))
    {
        let gate = order
            .get("gate_code")
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("other")
            .to_string();
        *blocked_buy_gate_counts.entry(gate).or_default() += 1;
    }
    let mut blocked_buy_gates = blocked_buy_gate_counts.into_iter().collect::<Vec<_>>();
    blocked_buy_gates.sort_by(|(left_gate, left_count), (right_gate, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_gate.cmp(right_gate))
    });
    let status = if !budget.reinvestment_pressure_active {
        "within_policy"
    } else if buy_candidate_count == 0 {
        "excess_cash_without_buy_candidates"
    } else if approved_buy_count == 0 {
        "excess_cash_with_blocked_buy_candidates"
    } else {
        "reinvestment_candidates_approved"
    };
    json!({
        "status": status,
        "active": budget.reinvestment_pressure_active,
        "cash_balance_dkk": budget.cash_balance_dkk,
        "cash_pct": budget.cash_pct,
        "min_cash_buffer_pct": budget.min_cash_buffer_pct,
        "excess_cash_pct": budget.excess_cash_pct,
        "available_buy_budget_dkk": budget.available_buy_budget_dkk,
        "threshold_pct": budget.reinvestment_pressure_threshold_pct,
        "candidate_order_count": candidate_order_count,
        "buy_candidate_count": buy_candidate_count,
        "sell_candidate_count": sell_candidate_count,
        "approved_buy_count": approved_buy_count,
        "approved_sell_count": approved_sell_count,
        "skipped_buy_count": skipped_buy_count,
        "skipped_sell_count": skipped_sell_count,
        "blocked_buy_gates": blocked_buy_gates.into_iter().map(|(gate_code, count)| json!({
            "gate_code": gate_code,
            "count": count,
        })).collect::<Vec<_>>(),
        "message": match status {
            "excess_cash_without_buy_candidates" => "Cash is above policy, but the decision report supplied no BUY candidates.",
            "excess_cash_with_blocked_buy_candidates" => "Cash is above policy, but BUY candidates were blocked by exchange, budget, risk, minimum value, or technical gates.",
            "reinvestment_candidates_approved" => "Cash is above policy and at least one BUY candidate was approved for queueing.",
            _ => "Cash is inside the configured policy band or no deployment capacity is available.",
        }
    })
}

fn excluded_symbols(state: &AppState) -> Vec<String> {
    excluded_symbols_for_config(&state.config)
}

fn excluded_symbols_for_config(config: &serde_yaml::Value) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(items) =
        yaml_at(config, &["risk", "excluded_symbols"]).and_then(serde_yaml::Value::as_sequence)
    {
        values.extend(
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string),
        );
    }
    if let Some(csv) = yaml_string(config, &["risk", "excluded_symbols_csv"]) {
        values.extend(
            csv.split([',', ';', '\n', '\r'])
                .map(str::trim)
                .filter(|symbol| !symbol.is_empty())
                .map(ToString::to_string),
        );
    }
    if let Some(items) = yaml_at(config, &["strategy", "swing", "never_trade_symbols"])
        .and_then(serde_yaml::Value::as_sequence)
    {
        values.extend(
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string),
        );
    }
    let mut normalized = Vec::new();
    for value in values {
        let symbol = normalize_symbol(&value);
        if !symbol.is_empty() && !normalized.contains(&symbol) {
            normalized.push(symbol);
        }
    }
    normalized
}

fn is_excluded_symbol(excluded_symbols: &[String], symbol: &str) -> bool {
    let symbol = normalize_symbol(symbol);
    !symbol.is_empty()
        && excluded_symbols
            .iter()
            .any(|candidate| candidate == &symbol)
}

fn normalize_symbol(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange)
        .unwrap_or("")
        .to_uppercase()
}

fn parse_report_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn optional_f64(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
}

fn optional_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn text(value: &JsonValue, key: &str) -> String {
    optional_text(value, key).unwrap_or_default()
}

fn fallback_text(value: &JsonValue, key: &str, fallback: &str) -> String {
    optional_text(value, key).unwrap_or_else(|| fallback.to_string())
}

fn sql_num(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_json(value: &JsonValue) -> String {
    if value.is_null() {
        return "NULL".to_string();
    }
    serde_json::to_string(value)
        .ok()
        .map(|value| format!("'{}'", sql_escape(&value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::{Row, any::AnyPoolOptions};
    use std::{path::PathBuf, sync::Once};

    #[test]
    fn risk_exclusion_csv_merges_with_configured_lists_case_insensitively() {
        let config: serde_yaml::Value = serde_yaml::from_str(
            "risk:\n  excluded_symbols:\n    - ' AAPL:XNAS '\n    - TSLA:xnas\n  excluded_symbols_csv: 'NVDA:xnas, tsla:XNAS; MSTR:xnas, AMD:xnas'\nstrategy:\n  swing:\n    never_trade_symbols:\n      - NOVOB:xcse\n      - aapl:xnas\n",
        )
        .expect("parse test config");

        let excluded = excluded_symbols_for_config(&config);
        assert_eq!(
            excluded,
            vec![
                "aapl:xnas",
                "tsla:xnas",
                "nvda:xnas",
                "mstr:xnas",
                "amd:xnas",
                "novob:xcse",
            ]
        );
        assert!(is_excluded_symbol(&excluded, "TSLA:XNAS"));
        assert!(is_excluded_symbol(&excluded, " novob:XCSE "));
        assert!(!is_excluded_symbol(&excluded, "MSFT:xnas"));
    }

    async fn manager_queue_test_state() -> AppState {
        static INSTALL_DRIVERS: Once = Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory manager test database");
        for statement in [
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                order_type TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                adapter TEXT NOT NULL,
                requested_weight_pct REAL,
                quantity REAL NOT NULL,
                price_local REAL,
                limit_price_local REAL,
                stop_price_local REAL,
                currency TEXT,
                estimated_value_dkk REAL,
                approval_required INTEGER NOT NULL,
                approved_at TEXT,
                strategy_type TEXT,
                strategy_session TEXT,
                strategy_key TEXT NOT NULL UNIQUE,
                strategy_role TEXT,
                request_json TEXT NOT NULL,
                execution_result_json TEXT,
                error_text TEXT,
                trade_thesis_json TEXT
            )",
            "CREATE TABLE execution_order_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                execution_order_id INTEGER NOT NULL,
                broker_order_id TEXT,
                event_type TEXT NOT NULL,
                broker_status TEXT,
                broker_substatus TEXT,
                broker_quantity REAL,
                broker_price_local REAL,
                event_signature TEXT NOT NULL UNIQUE,
                raw_payload_json TEXT NOT NULL
            )",
            "CREATE TABLE trading_manager_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                manager_key TEXT NOT NULL,
                manager_kind TEXT NOT NULL,
                manager_label TEXT NOT NULL,
                target_at_utc TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                open_exchange_codes_json TEXT NOT NULL,
                technical_json TEXT NOT NULL,
                manager_json TEXT NOT NULL,
                queue_result_json TEXT NOT NULL,
                error_text TEXT
            )",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create manager test table");
        }

        AppState {
            config_path: PathBuf::from("manager-queue-test.yaml"),
            config: serde_yaml::from_str("execution:\n  mode: live\n  adapter: saxo\n")
                .expect("parse manager test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    fn order(
        action: &str,
        sentiment: &str,
        trend_bias: &str,
        confluence_count: i64,
    ) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "NVDA:xnas",
            "action": action,
            "quantity": 4,
            "order_type": "Limit",
            "price_local": 215.45,
            "limit_price_local": 215.45,
            "estimated_value_dkk": 5400,
            "strategy_key": format!("test:{action}"),
            "strategy_role": action.to_lowercase(),
            "strategy_metadata": {
                "technical": {
                    "status": "ok",
                    "sentiment": sentiment,
                    "trend_bias": trend_bias,
                    "confluence_count": confluence_count,
                    "min_confluences": 3
                }
            }
        }))
        .unwrap()
    }

    fn scheduled_report(status: &str, created_at: &str, pulse_key: &str) -> DecisionReport {
        DecisionReport {
            id: 42,
            created_at: created_at.to_string(),
            status: status.to_string(),
            pulse_key: pulse_key.to_string(),
            pulse_label: "US open +1h15".to_string(),
            report_json: json!({"strategy_plan": {"swing_orders": []}}),
        }
    }

    #[test]
    fn trade_thesis_is_compact_and_records_only_buy_admission_evidence() {
        let mut report = scheduled_report(
            "completed",
            "2026-07-14T14:45:00Z",
            "us_open_followup:2026-07-14",
        );
        report.report_json = json!({
            "symbol_sentiment": [{
                "symbol": "NVDA:xnas",
                "rationale": "Fresh technical and Markov evidence supports a starter position."
            }],
            "selected_assets": [{
                "symbol": "NVDA:xnas",
                "notes": "Monitor the next earnings release."
            }]
        });
        let buy = order("BUY", "BUY", "bullish", 4);
        let thesis = compact_trade_thesis(
            &report,
            &buy,
            "BUY approved by bullish technical confluence.",
        );

        assert_eq!(text(&thesis, "status"), "recorded");
        assert_eq!(text(&thesis, "symbol"), "NVDA:xnas");
        assert_eq!(text(&thesis, "intended_holding_window"), "next_2_weeks");
        assert!(text(&thesis, "entry_rationale").contains("Markov"));
        assert!(text(&thesis, "invalidation").contains("not an automatic exit"));
        assert!(
            compact_trade_thesis(
                &report,
                &order("SELL", "SELL", "bearish", 4),
                "SELL approved."
            )
            .is_null()
        );
    }

    #[test]
    fn only_fresh_completed_scheduled_reports_are_eligible_for_queueing() {
        let cutoff = DateTime::parse_from_rfc3339("2026-07-14T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        assert!(is_fresh_scheduled_report(
            &scheduled_report("completed", "2026-07-14T08:00:00Z", "us_open"),
            cutoff,
        ));
        assert!(is_fresh_scheduled_report(
            &scheduled_report("xai_fallback", "2026-07-14T08:01:00Z", "eu_open"),
            cutoff,
        ));
    }

    #[test]
    fn unmanaged_report_selector_fails_closed_for_unverifiable_or_non_scheduled_reports() {
        let cutoff = DateTime::parse_from_rfc3339("2026-07-14T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for report in [
            scheduled_report("deferred", "2026-07-14T09:00:00Z", "us_open"),
            scheduled_report(
                "dry_run_completed",
                "2026-07-14T09:00:00Z",
                "manual:2026-07-14T09:00:00Z",
            ),
            scheduled_report("completed", "2026-07-14T07:59:59Z", "us_open"),
            scheduled_report("completed", "not-a-timestamp", "us_open"),
            scheduled_report("completed", "2026-07-14T09:00:00Z", ""),
        ] {
            assert!(
                !is_fresh_scheduled_report(&report, cutoff),
                "unexpectedly eligible report: {:?}",
                report
            );
        }
    }

    #[tokio::test]
    async fn manager_queue_persists_idempotently_without_broker_access() {
        let state = manager_queue_test_state().await;
        let report = scheduled_report(
            "completed",
            "2026-07-14T14:45:00Z",
            "us_open_followup:2026-07-14",
        );
        let candidate = order("BUY", "BUY", "bullish", 4);

        let first = insert_execution_order(
            &state,
            &report,
            &candidate,
            "Technical gate passed.",
            false,
            &JsonValue::Null,
        )
        .await
        .expect("queue manager candidate");
        let order_id = first["id"].as_i64().expect("queued order id");
        assert_eq!(first["status"], json!("pending_execution"));

        let duplicate = insert_execution_order(
            &state,
            &report,
            &candidate,
            "Technical gate passed.",
            false,
            &JsonValue::Null,
        )
        .await
        .expect("deduplicate manager candidate");
        assert_eq!(duplicate["id"], json!(order_id));
        assert_eq!(duplicate["status"], json!("already_exists"));

        let order_count = sqlx::query("SELECT COUNT(*) AS count FROM execution_orders")
            .fetch_one(&state.pool)
            .await
            .expect("count queued orders")
            .try_get::<i64, _>("count")
            .expect("read queued-order count");
        let event_count = sqlx::query("SELECT COUNT(*) AS count FROM execution_order_events")
            .fetch_one(&state.pool)
            .await
            .expect("count queue events")
            .try_get::<i64, _>("count")
            .expect("read queue-event count");
        let thesis = sqlx::query("SELECT trade_thesis_json FROM execution_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&state.pool)
            .await
            .expect("read recorded trade thesis")
            .try_get::<Option<String>, _>("trade_thesis_json")
            .expect("read trade thesis column")
            .and_then(|value| serde_json::from_str::<JsonValue>(&value).ok())
            .expect("recorded BUY thesis");
        assert_eq!(order_count, 1);
        assert_eq!(event_count, 1);
        assert_eq!(text(&thesis, "intended_holding_window"), "next_2_weeks");
        assert_eq!(text(&thesis, "strategy_key"), candidate.strategy_key);

        let run_id = insert_trading_manager_run(
            &state,
            &report,
            "completed",
            &["XNAS".to_string()],
            &json!({"approved_order_count": 1}),
            &json!({"status": "queued", "orders": [first]}),
            None,
        )
        .await
        .expect("persist manager run");
        assert!(run_id > 0);

        let stored = sqlx::query(
            "SELECT report_id, status, manager_key FROM trading_manager_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(&state.pool)
        .await
        .expect("read manager run");
        assert_eq!(stored.try_get::<i64, _>("report_id").unwrap(), report.id);
        assert_eq!(stored.try_get::<String, _>("status").unwrap(), "completed");
        assert_eq!(
            stored.try_get::<String, _>("manager_key").unwrap(),
            report.pulse_key
        );
    }

    #[test]
    fn hermes_context_self_check_extracts_complete_payload() {
        let check = hermes_context_self_check_from_raw(&json!({
            "raw_payload_json": {
                "context_self_check": {
                    "latest_report": true,
                    "markov_signals": true,
                    "end_of_day_report": true,
                    "current_positions": true,
                    "active_experiments": true
                }
            }
        }));

        assert_eq!(
            check.get("complete").and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            check
                .get("missing")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn hermes_context_self_check_reports_missing_payload_fields() {
        let check = hermes_context_self_check_from_raw(&json!({
            "context_self_check": {
                "latest_report": true,
                "current_positions": true
            }
        }));

        assert_eq!(
            check.get("complete").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            check.get("missing").cloned(),
            Some(json!([
                "markov_signals",
                "end_of_day_report",
                "active_experiments"
            ]))
        );
    }

    #[test]
    fn instrument_quarantine_requires_repeated_identical_hard_failures() {
        let cfg = InstrumentQuarantineConfig {
            enabled: true,
            lookback_days: 14,
            min_failures: 3,
            active_days: 14,
        };
        let rows = vec![
            json!({"created_at": "2026-07-01T10:00:00Z", "symbol": "ARKK:xmil", "action": "BUY", "status": "execution_failed", "error_text": "Saxo precheck rejected: account does not have any commissions configured"}),
            json!({"created_at": "2026-07-03T10:00:00Z", "symbol": "ARKK:xmil", "action": "BUY", "status": "execution_failed", "error_text": "Saxo precheck rejected: account does not have any commissions configured"}),
            json!({"created_at": "2026-07-08T10:00:00Z", "symbol": "ARKK:xmil", "action": "BUY", "status": "execution_failed", "error_text": "Saxo precheck rejected: account does not have any commissions configured"}),
            json!({"created_at": "2026-07-08T10:00:00Z", "symbol": "ARKK:xmil", "action": "SELL", "status": "execution_failed", "error_text": "temporary market closed"}),
        ];

        let quarantines = active_instrument_quarantines_from_rows(
            &rows,
            DateTime::parse_from_rfc3339("2026-07-09T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            cfg,
        );

        assert_eq!(quarantines.len(), 1);
        assert_eq!(quarantines[0].symbol, "ARKK:xmil");
        assert_eq!(quarantines[0].action, "BUY");
        assert_eq!(quarantines[0].signature, "commission_not_configured");
        assert_eq!(quarantines[0].failure_count, 3);
    }

    #[test]
    fn instrument_quarantine_expires_after_active_window() {
        let cfg = InstrumentQuarantineConfig {
            enabled: true,
            lookback_days: 14,
            min_failures: 2,
            active_days: 2,
        };
        let rows = vec![
            json!({"created_at": "2026-07-01T10:00:00Z", "symbol": "DEMANT:xcse", "action": "BUY", "status": "execution_failed", "error_text": "price violates tick size"}),
            json!({"created_at": "2026-07-02T10:00:00Z", "symbol": "DEMANT:xcse", "action": "BUY", "status": "execution_failed", "error_text": "invalid price increment"}),
        ];

        let quarantines = active_instrument_quarantines_from_rows(
            &rows,
            DateTime::parse_from_rfc3339("2026-07-05T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            cfg,
        );

        assert!(quarantines.is_empty());
    }

    #[test]
    fn extracts_strategy_plan_swing_orders() {
        let report = json!({
            "strategy_plan": {
                "swing_orders": [
                    {
                        "symbol": "NVDA:xnas",
                        "action": "BUY",
                        "quantity": 4,
                        "strategy_key": "swing:test",
                        "estimated_value_dkk": 5400
                    }
                ]
            }
        });
        let orders = candidate_orders_from_report(&report);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].strategy_key, "swing:test:NVDA:xnas:BUY");
    }

    /// The decision-report schema has no `strategy_type` field, so reading it
    /// from the model's response left every Rust-queued order NULL from the
    /// 2026-05-12 port until 2026-07-25. The runtime knows the provenance and
    /// must set it, and a model that invents the field must not override it.
    #[test]
    fn candidate_orders_carry_runtime_strategy_type_regardless_of_model_output() {
        let report = json!({
            "strategy_plan": {
                "swing_orders": [
                    {
                        "symbol": "NVDA:xnas",
                        "action": "BUY",
                        "quantity": 2,
                        "strategy_key": "us_open_followup:NVDA",
                        "estimated_value_dkk": 5400
                    },
                    {
                        "symbol": "AMD:xnas",
                        "action": "BUY",
                        "quantity": 1,
                        "strategy_key": "us_open_followup:AMD",
                        "estimated_value_dkk": 1200,
                        "strategy_type": "manual"
                    }
                ]
            }
        });
        let orders = candidate_orders_from_report(&report);
        assert_eq!(orders.len(), 2);
        for order in &orders {
            assert_eq!(
                order.strategy_type.as_deref(),
                Some(TRADING_MANAGER_STRATEGY_TYPE),
                "{} must record runtime provenance, not the model's claim",
                order.symbol
            );
        }
    }

    #[test]
    fn hermes_decision_advice_matches_strategy_key_before_symbol_side() {
        let row = json!({
            "status": "received",
            "overall_recommendation": "proceed",
            "summary": "Proceed with one reduction.",
            "order_advice_json": [
                {
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "stand_down",
                    "reason": "Symbol fallback"
                },
                {
                    "strategy_key": "test:BUY",
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "reduce",
                    "max_quantity": 2,
                    "reason": "Strategy-specific reduction"
                }
            ],
            "learning_notes_json": []
        });
        let advice = HermesDecisionAdvice::from_row(
            row,
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );
        let matched = advice
            .for_order(&order("BUY", "BUY", "bullish", 4))
            .unwrap();

        assert_eq!(matched.action, "reduce");
        assert_eq!(matched.max_quantity, Some(2.0));
        assert_eq!(matched.reason, "Strategy-specific reduction");
    }

    #[test]
    fn hermes_advice_delta_records_reduction_and_final_manager_outcome() {
        let candidate = order("BUY", "BUY", "bullish", 4);
        let strategy_key = candidate.strategy_key.clone();
        let advice = HermesDecisionAdvice::from_row(
            json!({
                "status": "received",
                "overall_recommendation": "proceed",
                "summary": "Reduce the candidate.",
                "order_advice_json": [{
                    "strategy_key": strategy_key,
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "reduce",
                    "max_quantity": 2,
                    "reason": "Private rationale stays in the advice record."
                }]
            }),
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        let delta = hermes_advice_delta(&[candidate.clone()], &advice, None);
        assert_eq!(delta["matched_candidate_count"], json!(1));
        assert_eq!(
            delta["candidates"][0]["match_source"],
            json!("strategy_key")
        );
        assert_eq!(delta["candidates"][0]["effect"], json!("reduced"));
        assert_eq!(delta["candidates"][0]["resulting_quantity"], json!(2.0));
        assert!(
            !serde_json::to_string(&delta)
                .unwrap()
                .contains("Private rationale stays in the advice record")
        );

        let finalized = with_hermes_advice_manager_outcomes(
            delta,
            &[(candidate, "technical gate passed".to_string())],
            &[],
        );
        assert_eq!(
            finalized["candidates"][0]["manager_outcome"],
            json!("approved")
        );
        assert_eq!(finalized["manager_outcome_counts"]["approved"], json!(1));
    }

    #[test]
    fn hermes_advice_delta_records_global_review_without_order_allow() {
        let candidate = order("BUY", "BUY", "bullish", 4);
        let advice = HermesDecisionAdvice::from_row(
            json!({
                "status": "received",
                "overall_recommendation": "review",
                "summary": "Review before acting.",
                "order_advice_json": []
            }),
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        let delta = hermes_advice_delta(&[candidate], &advice, None);
        assert_eq!(
            delta["candidates"][0]["effect"],
            json!("review_required_by_global_advice")
        );
        assert_eq!(delta["candidates"][0]["resulting_quantity"], json!(0.0));
    }

    #[test]
    fn hermes_conservative_timeout_requires_review() {
        let advice = HermesDecisionAdvice::fallback(
            "timeout",
            "conservative".to_string(),
            "Hermes timed out.".to_string(),
            42,
        );

        assert_eq!(advice.overall_recommendation, "review");
        assert_eq!(
            advice
                .raw
                .get("overall_recommendation")
                .and_then(JsonValue::as_str),
            Some("review")
        );
    }

    #[test]
    fn hermes_record_only_timeout_proceeds_for_audit_only() {
        let advice = HermesDecisionAdvice::fallback(
            "timeout",
            "record_only".to_string(),
            "Hermes timed out.".to_string(),
            42,
        );

        assert_eq!(advice.overall_recommendation, "proceed");
    }

    #[test]
    fn hermes_conservative_incomplete_self_check_blocks_automatic_queueing() {
        let advice = HermesDecisionAdvice::from_row(
            json!({
                "status": "received",
                "overall_recommendation": "proceed",
                "summary": "Allow the candidate.",
                "order_advice_json": [{
                    "strategy_key": "test:BUY",
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "allow",
                    "reason": "Otherwise acceptable."
                }],
                "raw_payload_json": {
                    "context_self_check": {
                        "latest_report": true,
                        "markov_signals": true,
                        "end_of_day_report": true,
                        "current_positions": false,
                        "active_experiments": true
                    }
                }
            }),
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        let reason = hermes_context_self_check_gate_reason(&advice).unwrap();
        assert!(reason.contains("current_positions"));
        assert!(reason.contains("blocks automatic queueing"));
    }

    #[test]
    fn hermes_complete_self_check_does_not_add_a_conservative_gate() {
        let advice = HermesDecisionAdvice::from_row(
            json!({
                "status": "received",
                "overall_recommendation": "proceed",
                "summary": "Complete evidence reviewed.",
                "order_advice_json": [],
                "raw_payload_json": {
                    "context_self_check": {
                        "latest_report": true,
                        "markov_signals": true,
                        "end_of_day_report": true,
                        "current_positions": true,
                        "active_experiments": true
                    }
                }
            }),
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        assert!(hermes_context_self_check_gate_reason(&advice).is_none());
    }

    #[test]
    fn hermes_preflight_marks_markov_signal_freshness() {
        let signal = json!({
            "status": "ok",
            "run_date": "2026-07-09",
            "current_state": "Bull",
            "direction": "long",
            "signed_signal": 0.42,
            "conviction": 0.42
        });
        let compact = compact_hermes_preflight_markov_signal(
            Some(&signal),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            5,
        );

        assert_eq!(compact["fresh"], json!(true));
        assert_eq!(compact["age_days"], json!(1));
        assert_eq!(compact["signed_signal"], json!(0.42));
    }

    #[test]
    fn hermes_preflight_preserves_markov_run_health_counts() {
        let compact = compact_hermes_preflight_markov_run(&json!({
            "status": "completed",
            "run_date": "2026-07-10",
            "asset_count": 80,
            "success_count": 76,
            "error_count": 4,
            "config_json": {"must_not_be_in_preflight": true}
        }));

        assert_eq!(compact["success_count"], json!(76));
        assert_eq!(compact["error_count"], json!(4));
        assert!(compact.get("config_json").is_none());
    }

    #[test]
    fn hermes_preflight_failure_summary_excludes_raw_error_text() {
        let rows = vec![json!({
            "created_at": "2026-07-10T10:00:00Z",
            "symbol": "DEMANT:xcse",
            "action": "BUY",
            "order_type": "Limit",
            "status": "execution_failed",
            "error_text": "PriceNotInTickSizeIncrements bearer-secret-must-not-leak"
        })];

        let compact = compact_hermes_preflight_failures(&rows);
        let encoded = serde_json::to_string(&compact).unwrap();
        assert_eq!(
            compact[0]["failure_signature"],
            json!("tick_size_or_price_increment")
        );
        assert!(!encoded.contains("bearer-secret-must-not-leak"));
        assert!(!encoded.contains("error_text"));
    }

    #[test]
    fn hermes_preflight_prefers_persisted_saxo_failure_taxonomy() {
        let rows = vec![json!({
            "created_at": "2026-07-21T10:00:00Z",
            "symbol": "DEMANT:xcse",
            "action": "BUY",
            "order_type": "Limit",
            "status": "execution_failed",
            "error_text": "opaque broker response",
            "execution_result_json": {
                "error_taxonomy": {"code": "commission_setup"}
            }
        })];

        let compact = compact_hermes_preflight_failures(&rows);
        assert_eq!(compact[0]["failure_signature"], json!("commission_setup"));
        assert!(
            !serde_json::to_string(&compact)
                .expect("serialize compact Hermes failures")
                .contains("opaque broker response")
        );
    }

    #[test]
    fn hermes_preflight_only_includes_pending_or_active_experiments() {
        let rows = vec![
            json!({"id": "pending", "status": "pending_review", "changed_variable_path": "strategy.swing.daily_indicators.min_confluences"}),
            json!({"id": "active", "status": "active_sim", "changed_variable_path": "strategy.capital.min_cash_buffer_pct"}),
            json!({"id": "rejected", "status": "rejected", "changed_variable_path": "execution.min_trade_value_dkk"}),
        ];

        let compact = compact_hermes_preflight_experiments(&rows);
        assert_eq!(compact.len(), 2);
        assert_eq!(compact[0]["id"], json!("pending"));
        assert_eq!(compact[1]["id"], json!("active"));
    }

    #[test]
    fn hermes_review_allows_explicit_order_advice_to_drive_decision() {
        let row = json!({
            "status": "received",
            "overall_recommendation": "review",
            "summary": "Review report, but allow NVDA.",
            "order_advice_json": [
                {
                    "strategy_key": "test:BUY",
                    "symbol": "NVDA:xnas",
                    "side": "BUY",
                    "action": "allow",
                    "reason": "Covered by Hermes."
                }
            ],
            "learning_notes_json": []
        });
        let advice = HermesDecisionAdvice::from_row(
            row,
            "conservative".to_string(),
            "decision-advice-42".to_string(),
        );

        assert_eq!(advice.overall_recommendation, "review");
        assert_eq!(
            advice
                .for_order(&order("BUY", "BUY", "bullish", 4))
                .unwrap()
                .action,
            "allow"
        );
    }

    #[test]
    fn makes_model_strategy_keys_symbol_specific() {
        assert_eq!(
            unique_strategy_key("rebalance_overweight".to_string(), "MSTR:xnas", "SELL"),
            "rebalance_overweight:MSTR:xnas:SELL"
        );
        assert_eq!(
            unique_strategy_key("pulse:MSTR:xnas:SELL".to_string(), "MSTR:xnas", "SELL"),
            "pulse:MSTR:xnas:SELL"
        );
    }

    #[test]
    fn approves_only_bullish_buy_setups() {
        assert!(technical_gate(&order("BUY", "BUY", "bullish", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "HOLD", "bullish", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "BUY", "neutral", 3), None).approved);
        assert!(!technical_gate(&order("BUY", "BUY", "bullish", 2), None).approved);
    }

    #[test]
    fn approves_risk_reducing_sell_setups() {
        assert!(technical_gate(&order("SELL", "SELL", "neutral", 1), None).approved);
        assert!(technical_gate(&order("SELL", "HOLD", "bearish", 1), None).approved);
        assert!(!technical_gate(&order("SELL", "HOLD", "bullish", 3), None).approved);
    }

    fn flatten_order(strategy_role: &str) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "ARM:xnas",
            "action": "SELL",
            "quantity": 5,
            "order_type": "Market",
            "estimated_value_dkk": 23000,
            "strategy_key": format!("test:flatten:{strategy_role}"),
            "strategy_role": strategy_role,
            "strategy_metadata": {
                "technical": {
                    "status": "ok",
                    "sentiment": "HOLD",
                    "trend_bias": "neutral",
                    "confluence_count": 1,
                    "min_confluences": 3
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn recognizes_flatten_family_strategy_roles() {
        assert!(is_flatten_role(&flatten_order("risk_reduction_flatten")));
        assert!(is_flatten_role(&flatten_order("risk_reduce_flatten")));
        assert!(is_flatten_role(&flatten_order("FLATTEN")));
        assert!(!is_flatten_role(&flatten_order("rebalance_underweight")));
        assert!(!is_flatten_role(&order("SELL", "HOLD", "neutral", 1)));
    }

    #[test]
    fn sell_gate_never_trusts_the_flatten_label_alone() {
        // The model-claimed flatten role must not bypass neutral technicals in
        // the pure gate; only the server-verified risk-off fallback may admit it.
        let gate = technical_gate(&flatten_order("risk_reduction_flatten"), None);
        assert!(!gate.approved, "{}", gate.reason);
        let gate = technical_gate(&flatten_order("FLATTEN"), None);
        assert!(!gate.approved, "{}", gate.reason);
    }

    async fn risk_off_test_state() -> AppState {
        static INSTALL_DRIVERS: Once = Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory risk-off test database");
        for statement in [
            "CREATE TABLE broker_position_snapshots (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                quantity REAL NOT NULL,
                open_price_local REAL,
                can_be_closed INTEGER
            )",
            "CREATE TABLE daily_indicator_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                run_date TEXT NOT NULL
            )",
            "CREATE TABLE daily_indicator_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                close REAL,
                sma20 REAL,
                sma50 REAL,
                sma200 REAL,
                rsi14 REAL,
                macd REAL,
                macd_signal REAL,
                macd_histogram REAL,
                atr14 REAL,
                resistance REAL,
                reward_risk REAL,
                nearest_support REAL,
                next_support REAL,
                downside_to_support_pct REAL,
                downside_after_break_pct REAL,
                support_break_risk REAL,
                support_break_risk_label TEXT,
                support_confidence REAL,
                support_history_coverage REAL,
                support_touch_count INTEGER,
                trend_bias TEXT,
                sentiment TEXT,
                confluence_count INTEGER,
                min_confluences INTEGER,
                confluences_json TEXT
            )",
            "CREATE TABLE markov_signal_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                run_date TEXT NOT NULL
            )",
            "CREATE TABLE markov_asset_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                current_state TEXT,
                current_close REAL,
                signed_signal REAL,
                direction TEXT,
                conviction REAL
            )",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create risk-off test table");
        }

        AppState {
            config_path: PathBuf::from("risk-off-test.yaml"),
            config: serde_yaml::from_str("execution:\n  mode: live\n  adapter: saxo\n")
                .expect("parse risk-off test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[tokio::test]
    async fn flatten_fallback_requires_server_verified_risk_off_evidence() {
        let state = risk_off_test_state().await;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();

        // No broker position and no Markov signal: the model's claim alone
        // produces no evidence.
        assert!(
            verified_risk_off_evidence(&state, "ARM:xnas", today)
                .await
                .is_none()
        );

        sqlx::query(
            "INSERT INTO daily_indicator_runs (created_at, run_date)
             VALUES ('2026-07-15T21:45:00Z', '2026-07-15')",
        )
        .execute(&state.pool)
        .await
        .expect("seed indicator run");
        sqlx::query(
            "INSERT INTO broker_position_snapshots (symbol, updated_at, quantity, open_price_local, can_be_closed)
             VALUES ('ARM:xnas', '2026-07-16T18:31:00Z', 5, 165.0, 1),
                    ('CSCO:xnas', '2026-07-16T18:31:00Z', 14, 50.0, 1)",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker positions");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (run_id, symbol, run_date, status, close, trend_bias, sentiment, confluence_count, min_confluences)
             VALUES (1, 'ARM:xnas', '2026-07-15', 'ok', 147.2, 'neutral', 'HOLD', 1, 3),
                    (1, 'CSCO:xnas', '2026-07-15', 'ok', 60.0, 'neutral', 'HOLD', 1, 3)",
        )
        .execute(&state.pool)
        .await
        .expect("seed indicator signals");
        sqlx::query(
            "INSERT INTO markov_signal_runs (created_at, run_date)
             VALUES ('2026-07-15T21:32:00Z', '2026-07-15')",
        )
        .execute(&state.pool)
        .await
        .expect("seed markov run");
        sqlx::query(
            "INSERT INTO markov_asset_signals (run_id, symbol, run_date, status, current_state, current_close, signed_signal, direction, conviction)
             VALUES (1, 'CSCO:xnas', '2026-07-15', 'ok', 'Bull', 60.0, 0.31, 'long', 0.6),
                    (1, 'NNIT:xcse', '2026-07-15', 'ok', 'Bear', 38.0, -0.32, 'short', 0.5)",
        )
        .execute(&state.pool)
        .await
        .expect("seed markov signals");

        // Under-water broker position against a fresh verified close.
        let arm = verified_risk_off_evidence(&state, "ARM:xnas", today).await;
        assert!(
            arm.as_deref().is_some_and(|e| e.contains("under water")),
            "{arm:?}"
        );

        // Profitable position with a positive Markov regime: no evidence.
        assert!(
            verified_risk_off_evidence(&state, "CSCO:xnas", today)
                .await
                .is_none()
        );

        // A fresh negative Markov regime qualifies even without a broker row.
        let nnit = verified_risk_off_evidence(&state, "NNIT:xcse", today).await;
        assert!(
            nnit.as_deref().is_some_and(|e| e.contains("Markov")),
            "{nnit:?}"
        );
    }

    #[tokio::test]
    async fn flatten_fallback_ignores_stale_signals() {
        let state = risk_off_test_state().await;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();

        sqlx::query(
            "INSERT INTO daily_indicator_runs (created_at, run_date)
             VALUES ('2026-07-05T21:45:00Z', '2026-07-05')",
        )
        .execute(&state.pool)
        .await
        .expect("seed stale indicator run");
        sqlx::query(
            "INSERT INTO broker_position_snapshots (symbol, updated_at, quantity, open_price_local, can_be_closed)
             VALUES ('ARM:xnas', '2026-07-16T18:31:00Z', 5, 165.0, 1)",
        )
        .execute(&state.pool)
        .await
        .expect("seed broker position");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (run_id, symbol, run_date, status, close, trend_bias, sentiment, confluence_count, min_confluences)
             VALUES (1, 'ARM:xnas', '2026-07-05', 'ok', 100.0, 'neutral', 'HOLD', 1, 3)",
        )
        .execute(&state.pool)
        .await
        .expect("seed stale indicator signal");
        sqlx::query(
            "INSERT INTO markov_signal_runs (created_at, run_date)
             VALUES ('2026-07-05T21:32:00Z', '2026-07-05')",
        )
        .execute(&state.pool)
        .await
        .expect("seed stale markov run");
        sqlx::query(
            "INSERT INTO markov_asset_signals (run_id, symbol, run_date, status, current_state, current_close, signed_signal, direction, conviction)
             VALUES (1, 'ARM:xnas', '2026-07-05', 'ok', 'Bear', 100.0, -0.4, 'short', 0.5)",
        )
        .execute(&state.pool)
        .await
        .expect("seed stale markov signal");

        // Both the deep under-water close and the negative regime are older
        // than the freshness window, so neither may authorize the exit.
        assert!(
            verified_risk_off_evidence(&state, "ARM:xnas", today)
                .await
                .is_none()
        );
    }

    #[test]
    fn rejects_limit_order_without_limit_price() {
        let mut order = CandidateOrder::from_json(json!({
            "symbol": "GE:xnys",
            "action": "BUY",
            "quantity": 1,
            "order_type": "Limit",
            "estimated_value_dkk": 2300,
            "strategy_key": "test:limit-missing"
        }))
        .unwrap();

        let gate = order_shape_gate(&mut order);

        assert!(!gate.approved);
        assert!(gate.reason.contains("Limit orders require"));
    }

    #[test]
    fn uses_price_local_as_limit_price_fallback() {
        let mut order = CandidateOrder::from_json(json!({
            "symbol": "GE:xnys",
            "action": "BUY",
            "quantity": 1,
            "order_type": "limit",
            "price_local": 325.25,
            "estimated_value_dkk": 2300,
            "strategy_key": "test:limit-price-fallback"
        }))
        .unwrap();

        let gate = order_shape_gate(&mut order);

        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.order_type, "Limit");
        assert_eq!(order.limit_price_local, Some(325.25));
    }

    #[test]
    fn experiment_overlay_can_adjust_min_confluences() {
        assert!(!technical_gate(&order("BUY", "BUY", "bullish", 2), None).approved);
        assert!(technical_gate(&order("BUY", "BUY", "bullish", 2), Some(2)).approved);
    }

    fn markov_test_config() -> MarkovGateConfig {
        MarkovGateConfig {
            enabled: true,
            min_signed_signal: 0.15,
            max_position_pct: 0.05,
            max_signal_age_days: 5,
        }
    }

    fn buy_order(quantity: f64, estimated_value_dkk: f64) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "AMD:xnas",
            "action": "BUY",
            "quantity": quantity,
            "order_type": "Market",
            "estimated_value_dkk": estimated_value_dkk,
            "strategy_key": "test:starter",
            "strategy_role": "starter"
        }))
        .unwrap()
    }

    fn risk_sizing_order(
        quantity: f64,
        estimated_value_dkk: f64,
        close: f64,
        atr14: f64,
        verified_from_db: bool,
    ) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "AMD:xnas",
            "action": "BUY",
            "quantity": quantity,
            "order_type": "Market",
            "estimated_value_dkk": estimated_value_dkk,
            "strategy_key": "test:risk-sizing",
            "strategy_metadata": {
                "technical": {
                    "status": "ok",
                    "verified_from_db": verified_from_db,
                    "close": close,
                    "atr14": atr14
                }
            }
        }))
        .unwrap()
    }

    fn risk_per_trade_test_config() -> RiskPerTradeConfig {
        RiskPerTradeConfig {
            risk_per_trade_pct: 0.01,
            stop_loss_atr_multiple: 2.0,
            protective_stops_enabled: true,
        }
    }

    fn position_weight_test_config() -> PositionWeightConfig {
        PositionWeightConfig {
            max_position_weight: 0.04,
        }
    }

    fn holding_limit_test_config(max_holdings: i64) -> HoldingLimitConfig {
        HoldingLimitConfig { max_holdings }
    }

    fn concentration_test_config(exchange: i64, currency: i64) -> ConcentrationConfig {
        ConcentrationConfig {
            max_assets_per_exchange: exchange,
            max_assets_per_currency: currency,
        }
    }

    fn position_exposure(values: &[(&str, f64)]) -> PositionExposure {
        PositionExposure {
            values_dkk: values
                .iter()
                .map(|(symbol, value)| (normalize_symbol_key(symbol), *value))
                .collect(),
            invalid_symbols: HashSet::new(),
            held_symbols: values
                .iter()
                .map(|(symbol, _)| normalize_symbol_key(symbol))
                .collect(),
            available: true,
        }
    }

    fn cost_guard_test_config() -> CostGuardConfig {
        CostGuardConfig {
            estimated_slippage_bps: 8.0,
            cost_guard_multiple: 1.5,
        }
    }

    fn cost_guard_order(reward_risk: f64, verified_from_db: bool) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": "AMD:xnas",
            "action": "BUY",
            "quantity": 10.0,
            "order_type": "Market",
            "estimated_value_dkk": 10_000.0,
            "strategy_key": "test:cost-guard",
            "strategy_metadata": {
                "technical": {
                    "status": "ok",
                    "verified_from_db": verified_from_db,
                    "close": 100.0,
                    "atr14": 10.0,
                    "reward_risk": reward_risk
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn cost_guard_passes_verified_reward_that_clears_lower_bound_costs() {
        let mut order = cost_guard_order(2.0, true);
        let gate = cost_guard_gate(&mut order, cost_guard_test_config());

        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(
            order.raw["strategy_metadata"]["cost_guard"]["verified_from_db"],
            json!(true)
        );
        assert_eq!(
            order.raw["strategy_metadata"]["cost_guard"]["passes"],
            json!(true)
        );
    }

    #[test]
    fn cost_guard_rejects_verified_reward_below_lower_bound_costs() {
        let mut order = cost_guard_order(0.01, true);
        let gate = cost_guard_gate(&mut order, cost_guard_test_config());

        assert!(!gate.approved);
        assert!(gate.reason.starts_with("Cost guard rejected BUY"));
        assert_eq!(
            order.raw["strategy_metadata"]["cost_guard"]["passes"],
            json!(false)
        );
    }

    #[test]
    fn cost_guard_rejects_model_supplied_indicator_values() {
        let mut order = cost_guard_order(2.0, false);
        let gate = cost_guard_gate(&mut order, cost_guard_test_config());

        assert!(!gate.approved);
        assert!(gate.reason.contains("model-supplied"), "{}", gate.reason);
    }

    fn candidate_limit_order(symbol: &str, action: &str) -> CandidateOrder {
        CandidateOrder::from_json(json!({
            "symbol": symbol,
            "action": action,
            "quantity": 1.0,
            "order_type": "Market",
            "estimated_value_dkk": 1_000.0,
            "strategy_key": format!("test:candidate-limit:{symbol}:{action}"),
            "strategy_metadata": {
                "technical": {"status": "missing"}
            }
        }))
        .unwrap()
    }

    #[test]
    fn candidate_symbol_limit_preserves_report_order_and_allows_repeat_actions() {
        let candidates = vec![
            candidate_limit_order("AMD:xnas", "BUY"),
            candidate_limit_order("NVDA:xnas", "BUY"),
            candidate_limit_order("amd:xnas", "SELL"),
            candidate_limit_order("MSFT:xnas", "BUY"),
        ];
        let (eligible, skipped) =
            enforce_candidate_symbol_limit(candidates, CandidateLimitConfig { max_symbols: 2 });

        assert_eq!(eligible.len(), 3);
        assert_eq!(
            eligible
                .iter()
                .map(|order| order.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["AMD:xnas", "NVDA:xnas", "amd:xnas"]
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].symbol, "MSFT:xnas");
        assert_eq!(
            skipped[0].raw["strategy_metadata"]["candidate_limit"]["eligible"],
            json!(false)
        );
        assert_eq!(
            eligible[0].raw["strategy_metadata"]["candidate_limit"]["eligible"],
            json!(true)
        );
    }

    #[test]
    fn candidate_symbol_limit_treats_zero_as_unlimited_and_negative_as_invalid() {
        let candidates = vec![
            candidate_limit_order("AMD:xnas", "BUY"),
            candidate_limit_order("NVDA:xnas", "BUY"),
        ];
        let (eligible, skipped) = enforce_candidate_symbol_limit(
            candidates.clone(),
            CandidateLimitConfig { max_symbols: 0 },
        );
        assert_eq!(eligible.len(), 2);
        assert!(skipped.is_empty());

        let (eligible, skipped) =
            enforce_candidate_symbol_limit(candidates, CandidateLimitConfig { max_symbols: -1 });
        assert!(eligible.is_empty());
        assert_eq!(skipped.len(), 2);
        assert!(
            candidate_limit_skip_reason(CandidateLimitConfig { max_symbols: -1 })
                .contains("must be non-negative")
        );
    }

    #[test]
    fn selected_asset_limit_caps_new_buy_symbols_but_preserves_sells_and_repeats() {
        let config = SelectedAssetLimitConfig {
            max_selected_assets: 2,
        };
        let mut selected = HashSet::new();
        let mut amd_buy = candidate_limit_order("AMD:xnas", "BUY");
        let mut nvda_buy = candidate_limit_order("NVDA:xnas", "BUY");
        let mut amd_follow_up = candidate_limit_order("amd:xnas", "BUY");
        let mut msft_buy = candidate_limit_order("MSFT:xnas", "BUY");
        let mut sell = candidate_limit_order("TSLA:xnas", "SELL");

        assert!(selected_asset_limit_gate(&mut amd_buy, config, &mut selected).approved);
        assert!(selected_asset_limit_gate(&mut nvda_buy, config, &mut selected).approved);
        assert!(selected_asset_limit_gate(&mut amd_follow_up, config, &mut selected).approved);
        let blocked = selected_asset_limit_gate(&mut msft_buy, config, &mut selected);
        assert!(!blocked.approved);
        assert!(blocked.reason.starts_with("Selection cap is 2"));
        assert!(selected_asset_limit_gate(&mut sell, config, &mut selected).approved);
        assert_eq!(selected.len(), 2);
        assert_eq!(
            msft_buy.raw["strategy_metadata"]["selected_asset_limit"]["selected_buy_symbol_count_before"],
            json!(2)
        );
    }

    #[test]
    fn selected_asset_limit_treats_zero_as_unlimited_and_negative_as_invalid() {
        let mut unlimited_selected = HashSet::new();
        let mut unlimited = candidate_limit_order("AMD:xnas", "BUY");
        assert!(
            selected_asset_limit_gate(
                &mut unlimited,
                SelectedAssetLimitConfig {
                    max_selected_assets: 0,
                },
                &mut unlimited_selected,
            )
            .approved
        );

        let mut invalid_selected = HashSet::new();
        let mut invalid = candidate_limit_order("AMD:xnas", "BUY");
        let invalid_gate = selected_asset_limit_gate(
            &mut invalid,
            SelectedAssetLimitConfig {
                max_selected_assets: -1,
            },
            &mut invalid_selected,
        );
        assert!(!invalid_gate.approved);
        assert!(invalid_gate.reason.contains("must be non-negative"));
    }

    #[test]
    fn risk_per_trade_gate_downsizes_using_verified_atr_stop_distance() {
        // 10 shares at 1,000 DKK/share; 2 ATR is 20% of a 100-local close,
        // so each share risks 200 DKK. A 100,000 DKK portfolio at 1% may
        // risk 1,000 DKK: five shares, not ten.
        let mut order = risk_sizing_order(10.0, 10_000.0, 100.0, 10.0, true);
        let gate = risk_per_trade_gate(&mut order, 100_000.0, risk_per_trade_test_config(), true);

        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.quantity, 5.0);
        assert_eq!(order.estimated_value_dkk, Some(5_000.0));
        assert_eq!(
            order.raw["strategy_metadata"]["risk_per_trade"]["downsized"],
            json!(true)
        );
        assert_eq!(
            order.raw["strategy_metadata"]["risk_per_trade"]["max_loss_dkk"],
            json!(1_000.0)
        );
    }

    #[test]
    fn risk_per_trade_gate_rejects_when_one_share_exceeds_loss_budget() {
        let mut order = risk_sizing_order(1.0, 1_000.0, 100.0, 60.0, true);
        let gate = risk_per_trade_gate(&mut order, 100_000.0, risk_per_trade_test_config(), true);

        assert!(!gate.approved);
        assert!(gate.reason.contains("below one share"), "{}", gate.reason);
        assert_eq!(order.quantity, 1.0);
    }

    #[test]
    fn risk_per_trade_gate_rejects_model_supplied_indicator_values() {
        let mut order = risk_sizing_order(2.0, 2_000.0, 100.0, 10.0, false);
        let gate = risk_per_trade_gate(&mut order, 100_000.0, risk_per_trade_test_config(), true);

        assert!(!gate.approved);
        assert!(gate.reason.contains("model-supplied"), "{}", gate.reason);
    }

    #[test]
    fn risk_per_trade_gate_requires_automatic_protective_stops() {
        let mut order = risk_sizing_order(2.0, 2_000.0, 100.0, 10.0, true);
        let gate = risk_per_trade_gate(
            &mut order,
            100_000.0,
            RiskPerTradeConfig {
                protective_stops_enabled: false,
                ..risk_per_trade_test_config()
            },
            true,
        );

        assert!(!gate.approved);
        assert!(gate.reason.contains("protective stops"), "{}", gate.reason);
    }

    #[test]
    fn position_weight_gate_downsizes_against_existing_symbol_exposure() {
        // 4% of a 100,000 DKK portfolio is 4,000 DKK. AMD already holds
        // 2,000 DKK, so a 5-share/5,000 DKK proposal is reduced to two.
        let mut order = buy_order(5.0, 5_000.0);
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let gate = position_weight_gate(
            &mut order,
            100_000.0,
            position_weight_test_config(),
            &exposure,
        );

        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.quantity, 2.0);
        assert_eq!(order.estimated_value_dkk, Some(2_000.0));
        assert_eq!(
            order.raw["strategy_metadata"]["position_weight"]["resulting_position_value_dkk"],
            json!(4_000.0)
        );
        assert_eq!(
            order.raw["strategy_metadata"]["position_weight"]["downsized"],
            json!(true)
        );
    }

    #[test]
    fn position_weight_gate_reserves_approved_buys_across_reports() {
        let mut exposure = position_exposure(&[]);
        let mut first = buy_order(3.0, 3_000.0);
        let first_gate = position_weight_gate(
            &mut first,
            100_000.0,
            position_weight_test_config(),
            &exposure,
        );
        assert!(first_gate.approved, "{}", first_gate.reason);
        exposure.reserve_buy(&first.symbol, first.estimated_value_dkk.unwrap());

        let mut second = buy_order(3.0, 3_000.0);
        let second_gate = position_weight_gate(
            &mut second,
            100_000.0,
            position_weight_test_config(),
            &exposure,
        );
        assert!(second_gate.approved, "{}", second_gate.reason);
        assert_eq!(second.quantity, 1.0);
        assert_eq!(second.estimated_value_dkk, Some(1_000.0));
    }

    #[test]
    fn position_weight_gate_fails_closed_when_position_snapshot_is_unavailable() {
        let mut order = buy_order(1.0, 1_000.0);
        let exposure = PositionExposure {
            values_dkk: HashMap::new(),
            invalid_symbols: HashSet::new(),
            held_symbols: HashSet::new(),
            available: false,
        };
        let gate = position_weight_gate(
            &mut order,
            100_000.0,
            position_weight_test_config(),
            &exposure,
        );
        assert!(!gate.approved);
        assert!(
            gate.reason.contains("snapshot is unavailable"),
            "{}",
            gate.reason
        );
    }

    #[test]
    fn holding_limit_blocks_new_symbols_after_all_slots_are_reserved() {
        let mut exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut first = candidate_limit_order("NVDA:xnas", "BUY");
        let first_gate = holding_limit_gate(&mut first, holding_limit_test_config(2), &exposure);
        assert!(first_gate.approved, "{}", first_gate.reason);
        exposure.reserve_buy("NVDA:xnas", 1_000.0);

        let mut second = candidate_limit_order("MSFT:xnas", "BUY");
        let second_gate = holding_limit_gate(&mut second, holding_limit_test_config(2), &exposure);
        assert!(!second_gate.approved);
        assert!(
            second_gate.reason.contains("every slot"),
            "{}",
            second_gate.reason
        );
    }

    #[test]
    fn holding_limit_allows_add_to_existing_symbol_at_cap() {
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut order = candidate_limit_order("AMD:xnas", "BUY");
        let gate = holding_limit_gate(&mut order, holding_limit_test_config(1), &exposure);
        assert!(gate.approved, "{}", gate.reason);
        assert!(gate.reason.contains("does not consume"), "{}", gate.reason);
    }

    #[test]
    fn holding_limit_fails_closed_without_a_position_snapshot() {
        let mut order = candidate_limit_order("AMD:xnas", "BUY");
        let exposure = PositionExposure {
            values_dkk: HashMap::new(),
            invalid_symbols: HashSet::new(),
            held_symbols: HashSet::new(),
            available: false,
        };
        let gate = holding_limit_gate(&mut order, holding_limit_test_config(25), &exposure);
        assert!(!gate.approved);
        assert!(
            gate.reason.contains("snapshot is unavailable"),
            "{}",
            gate.reason
        );
    }

    #[test]
    fn concentration_gate_blocks_new_exchange_symbol_at_cap() {
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut order = candidate_limit_order("NVDA:xnas", "BUY");
        let gate = concentration_gate(&mut order, concentration_test_config(1, 0), &exposure);
        assert!(!gate.approved, "{}", gate.reason);
        assert!(gate.reason.starts_with("Exchange concentration cap"));
        assert_eq!(
            order.raw["strategy_metadata"]["concentration"]["exchange"],
            "XNAS"
        );
        assert_eq!(
            order.raw["strategy_metadata"]["concentration"]["exchange_count_before"],
            1
        );
    }

    #[test]
    fn concentration_gate_blocks_same_currency_across_exchanges() {
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut order = candidate_limit_order("BAC:xnys", "BUY");
        let gate = concentration_gate(&mut order, concentration_test_config(0, 1), &exposure);
        assert!(!gate.approved, "{}", gate.reason);
        assert!(gate.reason.starts_with("Currency concentration cap"));
        assert_eq!(
            order.raw["strategy_metadata"]["concentration"]["currency"],
            "USD"
        );
    }

    #[test]
    fn concentration_gate_allows_add_to_existing_symbol_at_cap() {
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut order = candidate_limit_order("AMD:xnas", "BUY");
        let gate = concentration_gate(&mut order, concentration_test_config(1, 1), &exposure);
        assert!(gate.approved, "{}", gate.reason);
        assert!(gate.reason.contains("existing"), "{}", gate.reason);
    }

    #[test]
    fn concentration_gate_counts_earlier_approved_buys_in_the_same_cycle() {
        let mut exposure = position_exposure(&[]);
        let mut first = candidate_limit_order("AMD:xnas", "BUY");
        let first_gate = concentration_gate(&mut first, concentration_test_config(1, 0), &exposure);
        assert!(first_gate.approved, "{}", first_gate.reason);
        exposure.reserve_buy("AMD:xnas", 1_000.0);

        let mut second = candidate_limit_order("NVDA:xnas", "BUY");
        let second_gate =
            concentration_gate(&mut second, concentration_test_config(1, 0), &exposure);
        assert!(!second_gate.approved, "{}", second_gate.reason);
    }

    #[test]
    fn concentration_gate_rejects_negative_config_and_keeps_zero_unlimited() {
        let exposure = position_exposure(&[("AMD:xnas", 2_000.0)]);
        let mut invalid = candidate_limit_order("NVDA:xnas", "BUY");
        let invalid_gate =
            concentration_gate(&mut invalid, concentration_test_config(-1, 0), &exposure);
        assert!(!invalid_gate.approved);
        assert!(invalid_gate.reason.contains("invalid"));

        let mut unlimited = candidate_limit_order("NVDA:xnas", "BUY");
        let unlimited_gate =
            concentration_gate(&mut unlimited, concentration_test_config(0, 0), &exposure);
        assert!(unlimited_gate.approved, "{}", unlimited_gate.reason);
        assert_eq!(
            unlimited.raw["strategy_metadata"]["concentration"]["status"],
            "unlimited"
        );
    }

    fn markov_evidence(signed_signal: f64, direction: &str, run_date: &str) -> JsonValue {
        json!({
            "status": "ok",
            "run_date": run_date,
            "current_state": "Bull",
            "signed_signal": signed_signal,
            "direction": direction,
            "conviction": signed_signal
        })
    }

    #[test]
    fn markov_gate_approves_fresh_long_signal() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let mut order = buy_order(5.0, 10_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(gate.approved, "{}", gate.reason);
        // 10k is below the 13.3k cap: quantity untouched.
        assert_eq!(order.quantity, 5.0);
        let recorded = order.raw["strategy_metadata"]["markov_gate"].clone();
        assert_eq!(recorded["verified_from_db"], json!(true));
        assert_eq!(recorded["size_capped"], json!(false));
    }

    #[test]
    fn markov_gate_rejects_weak_or_short_signals() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let config = markov_test_config();
        let weak = markov_evidence(0.10, "long", "2026-06-08");
        let short = markov_evidence(0.40, "short", "2026-06-08");
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                Some(&weak),
                config,
                266_000.0,
                today
            )
            .approved
        );
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                Some(&short),
                config,
                266_000.0,
                today
            )
            .approved
        );
        assert!(
            !markov_buy_gate(
                &mut buy_order(5.0, 10_000.0),
                None,
                config,
                266_000.0,
                today
            )
            .approved
        );
    }

    #[test]
    fn markov_gate_rejects_stale_signal() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let stale = markov_evidence(0.60, "long", "2026-06-01");
        let gate = markov_buy_gate(
            &mut buy_order(5.0, 10_000.0),
            Some(&stale),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(!gate.approved);
        assert!(gate.reason.contains("days old"), "{}", gate.reason);
    }

    #[test]
    fn markov_gate_scales_oversized_orders_to_cap() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        // 40k requested vs 13.3k cap (5% of 266k): expect quantity scaled from 20 to 6.
        let mut order = buy_order(20.0, 40_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(gate.approved, "{}", gate.reason);
        assert_eq!(order.quantity, 6.0);
        let estimated = order.estimated_value_dkk.unwrap();
        assert!((estimated - 12_000.0).abs() < 1.0, "estimated {estimated}");
        assert_eq!(
            order.raw["strategy_metadata"]["markov_gate"]["size_capped"],
            json!(true)
        );
    }

    #[test]
    fn markov_gate_rejects_when_one_share_exceeds_cap() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        // Single share worth 20k against a 13.3k cap cannot be scaled down.
        let mut order = buy_order(1.0, 20_000.0);
        let evidence = markov_evidence(0.60, "long", "2026-06-08");
        let gate = markov_buy_gate(
            &mut order,
            Some(&evidence),
            markov_test_config(),
            266_000.0,
            today,
        );
        assert!(!gate.approved);
        assert!(gate.reason.contains("single share"), "{}", gate.reason);
    }

    #[test]
    fn experiment_overlays_are_not_allowed_for_live_saxo_live() {
        assert!(experiment_overlays_allowed("simulation", "LIVE"));
        assert!(experiment_overlays_allowed("live", "SIM"));
        assert!(!experiment_overlays_allowed("live", "LIVE"));
    }

    #[test]
    fn parses_supported_strategy_experiment_overlay() {
        let row = json!({
            "id": "strategy-experiment-test",
            "status": "approved_sim",
            "goal_version": 1,
            "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
            "old_value_json": 0.10,
            "new_value_json": "0.15",
            "hypothesis": "More cash buffer reduces drawdown."
        });
        let overlay = StrategyExperimentOverlay::from_row(&row).unwrap();
        assert_eq!(
            overlay.f64_value("strategy.capital.min_cash_buffer_pct"),
            Some(0.15)
        );
        assert!(
            StrategyExperimentOverlay::from_row(&json!({
                "id": "unsupported",
                "status": "approved_sim",
                "changed_variable_path": "execution.adapter",
                "new_value_json": "saxo"
            }))
            .is_none()
        );
    }

    #[test]
    fn experiment_overlay_acceptance_matches_published_capabilities() {
        for path in SUPPORTED_EXPERIMENT_VARIABLES {
            assert!(
                StrategyExperimentOverlay::from_row(&json!({
                    "id": "published-variable",
                    "status": "approved_sim",
                    "changed_variable_path": path,
                    "new_value_json": 0.15
                }))
                .is_some(),
                "published variable {path} must be loadable by the Trading Manager"
            );
        }

        for path in [
            "strategy.swing.cash_buffer_pct",
            "execution.max_commission_pct_per_side",
            "strategy.swing.markov_gate.max_position_pct",
        ] {
            assert!(
                StrategyExperimentOverlay::from_row(&json!({
                    "id": "unsupported-variable",
                    "status": "approved_sim",
                    "changed_variable_path": path,
                    "new_value_json": 0.15
                }))
                .is_none(),
                "unpublished variable {path} must not affect Trading Manager queueing"
            );
        }
    }

    #[test]
    fn derives_available_buy_budget_after_cash_buffer() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 250000.0,
                "cash_balance_dkk": 50000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let mut budget = capital_budget_from_overview(&overview, None);
        assert_eq!(budget.required_cash_buffer_dkk, 30000.0);
        assert_eq!(budget.available_buy_budget_dkk, 20000.0);
        assert!(budget.reinvestment_pressure_active);
        budget.reserve_buy(7500.0);
        assert_eq!(budget.available_buy_budget_dkk, 12500.0);
    }

    #[test]
    fn reinvestment_diagnostics_flags_missing_buy_candidates() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 220000.0,
                "cash_balance_dkk": 80000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let budget = capital_budget_from_overview(&overview, None);
        let diagnostics = reinvestment_diagnostics(&budget, 1, 0, 1, 0, 1, &[]);
        assert_eq!(
            diagnostics["status"],
            JsonValue::from("excess_cash_without_buy_candidates")
        );
        assert_eq!(diagnostics["active"], JsonValue::from(true));
    }

    #[test]
    fn reinvestment_diagnostics_rank_buy_blocks_by_stable_gate_code() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 220000.0,
                "cash_balance_dkk": 80000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let budget = capital_budget_from_overview(&overview, None);
        let diagnostics = reinvestment_diagnostics(
            &budget,
            4,
            3,
            1,
            0,
            1,
            &[
                json!({"action": "BUY", "gate_code": "market_open"}),
                json!({"action": "BUY", "gate_code": "cash_budget"}),
                json!({"action": "BUY", "gate_code": "cash_budget"}),
                json!({"action": "SELL", "gate_code": "technical"}),
            ],
        );
        assert_eq!(diagnostics["skipped_buy_count"], json!(3));
        assert_eq!(
            diagnostics["blocked_buy_gates"][0],
            json!({"gate_code": "cash_budget", "count": 2})
        );
        assert_eq!(
            diagnostics["blocked_buy_gates"][1],
            json!({"gate_code": "market_open", "count": 1})
        );
    }

    #[test]
    fn experiment_overlay_can_adjust_cash_buffer() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 250000.0,
                "cash_balance_dkk": 50000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90
                }
            }
        });
        let budget = capital_budget_from_overview(&overview, Some(0.15));
        assert_eq!(budget.required_cash_buffer_dkk, 45000.0);
        assert_eq!(budget.available_buy_budget_dkk, 5000.0);
    }

    #[test]
    fn monthly_loss_threshold_respects_disabled_non_negative_floor() {
        assert!(monthly_loss_threshold_breached(-12_000.0, -10_000.0));
        assert!(!monthly_loss_threshold_breached(-9_999.0, -10_000.0));
        assert!(!monthly_loss_threshold_breached(-12_000.0, 0.0));
    }

    #[test]
    fn monthly_loss_soft_band_only_applies_between_valid_loss_floors() {
        assert!(monthly_loss_soft_reduction_active(
            -30_000.0, -25_000.0, -50_000.0
        ));
        assert!(!monthly_loss_soft_reduction_active(
            -24_999.0, -25_000.0, -50_000.0
        ));
        assert!(!monthly_loss_soft_reduction_active(
            -50_000.0, -25_000.0, -50_000.0
        ));
        assert!(!monthly_loss_soft_reduction_active(
            -30_000.0, -60_000.0, -50_000.0
        ));
        assert!(!monthly_loss_soft_reduction_active(
            -30_000.0, -25_000.0, 0.0
        ));
    }

    #[test]
    fn monthly_loss_soft_band_reduces_only_effective_buy_budget() {
        let overview = json!({
            "portfolio_summary": {
                "total_market_value_dkk": 300000.0,
                "invested_market_value_dkk": 250000.0,
                "cash_balance_dkk": 50000.0
            },
            "settings": {
                "cash_buffer": {
                    "min_cash_buffer_pct": 0.10,
                    "max_deployment_pct": 0.90,
                    "reinvestment_pressure_threshold_pct": 0.05
                }
            }
        });
        let mut budget = capital_budget_from_overview(&overview, None);
        budget.apply_buy_multiplier(0.5);
        assert_eq!(budget.unreduced_available_buy_budget_dkk, 20_000.0);
        assert_eq!(budget.available_buy_budget_dkk, 10_000.0);
        assert_eq!(budget.available_cash_above_buffer_dkk, 20_000.0);
        assert!(budget.reinvestment_pressure_active);
    }

    #[test]
    fn overlapping_soft_guardrails_take_the_strictest_multiplier_not_the_product() {
        // A losing month and a drawdown are usually one decline seen twice.
        // Multiplying 0.5 by 0.5 would deploy a quarter of the budget on a
        // rule nobody configured; the strictest single band is predictable.
        assert_eq!(combined_soft_buy_multiplier(&[0.5, 0.5]), Some(0.5));
        assert_eq!(combined_soft_buy_multiplier(&[0.75, 0.25]), Some(0.25));
        assert_eq!(combined_soft_buy_multiplier(&[0.5]), Some(0.5));
    }

    #[test]
    fn no_active_soft_band_leaves_the_buy_budget_untouched() {
        // `None` means "do not call apply_buy_multiplier at all". A 1.0 here
        // would be harmless today but invites a future multiplier of 0.0 or
        // NaN to pass straight through.
        assert_eq!(combined_soft_buy_multiplier(&[]), None);
        assert_eq!(combined_soft_buy_multiplier(&[1.0]), None);
        assert_eq!(combined_soft_buy_multiplier(&[f64::NAN, -1.0]), None);
    }

    #[test]
    fn applies_instrument_quarantine_override_by_exact_signature() {
        let quarantines = vec![InstrumentQuarantine {
            symbol: "ARKK:xmil".to_string(),
            action: "BUY".to_string(),
            signature: "commission_not_configured".to_string(),
            failure_count: 3,
            latest_failure_at: "2026-07-09T10:00:00Z".to_string(),
            expires_at: "2026-07-23T10:00:00Z".to_string(),
            sample_error: "commissions configured".to_string(),
            override_active: false,
            override_notes: String::new(),
            override_updated_at: String::new(),
        }];
        let overrides = json!({
            "overrides": [{
                "symbol": "ARKK:xmil",
                "action": "BUY",
                "signature": "commission_not_configured",
                "enabled": true,
                "notes": "manually verified Saxo commission setup",
                "updated_at": "2026-07-10T06:00:00Z"
            }]
        });

        let result = apply_instrument_quarantine_overrides(quarantines, &overrides);
        assert!(result[0].override_active);
        assert_eq!(
            result[0].override_notes,
            "manually verified Saxo commission setup"
        );
    }

    #[test]
    fn candidate_gate_reason_codes_are_stable_and_safe() {
        assert_eq!(
            candidate_gate_reason_code("Hermes advisory reduced quantity below minimum"),
            "hermes_advice"
        );
        assert_eq!(
            candidate_gate_reason_code("Exchange XNAS is closed"),
            "market_open"
        );
        assert_eq!(
            candidate_gate_reason_code("Monthly-loss circuit breaker is active"),
            "monthly_loss_breaker"
        );
        assert_eq!(
            candidate_gate_reason_code("Technical confluence below configured minimum"),
            "technical"
        );
        assert_eq!(
            candidate_gate_reason_code(
                "Risk-per-trade cap is below one share's estimated stop loss"
            ),
            "risk_per_trade"
        );
        assert_eq!(
            candidate_gate_reason_code("Cost guard rejected BUY: expected reward is below costs"),
            "cost_guard"
        );
        assert_eq!(
            candidate_gate_reason_code("Candidate limit reached: only the first 30 symbols"),
            "candidate_limit"
        );
        assert_eq!(
            candidate_gate_reason_code("Holding cap is 25; every slot is occupied"),
            "max_holdings"
        );
    }
}
