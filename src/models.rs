use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::localization::LocalizationPrefs;

// Rust structs play the same role as typed objects/interfaces in TypeScript.
// `derive` asks the compiler to generate standard behavior, like cloning and
// JSON serialization, instead of hand-writing boilerplate methods.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DashboardView {
    pub app_name: String,
    pub environment: String,
    pub db_label: String,
    pub total_value_dkk: f64,
    pub invested_value_dkk: f64,
    pub cash_dkk: f64,
    pub initial_cash_dkk: f64,
    pub cash_from_trades_dkk: f64,
    pub unrealised_pnl_dkk: f64,
    pub unrealised_after_tax_dkk: f64,
    pub daily_pnl_dkk: f64,
    pub position_count: i64,
    pub execution_mode: String,
    pub execution_adapter: String,
    pub saxo_status: String,
    pub saxo_auth: JsonValue,
    pub sso_session: JsonValue,
    pub ai_settings: JsonValue,
    pub localization: LocalizationPrefs,
    pub active_view: String,
    pub performance_range: String,
    pub selected_report_id: Option<i64>,
    pub execution_page: i64,
    pub execution_page_size: i64,
    pub execution_order_total: i64,
    pub markov_page: i64,
    pub markov_page_size: i64,
    pub markov_signal_total: i64,
    pub quiver_page: i64,
    pub quiver_page_size: i64,
    pub quiver_signal_total: i64,
    pub scheduler_page: i64,
    pub scheduler_page_size: i64,
    pub scheduler_cycle_total: i64,
    pub positions: Vec<JsonValue>,
    pub orders: Vec<JsonValue>,
    pub execution_fills: Vec<JsonValue>,
    pub execution_events: Vec<JsonValue>,
    pub reports: Vec<JsonValue>,
    pub decision_pulse_statuses: Vec<JsonValue>,
    pub journal_entries: Vec<JsonValue>,
    pub scheduler_cycles: Vec<JsonValue>,
    pub hermes_reflections: Vec<JsonValue>,
    pub hermes_experiments: Vec<JsonValue>,
    pub hermes_decision_advice_audit: Vec<JsonValue>,
    pub hermes_counterfactuals: Vec<JsonValue>,
    pub active_strategy_baseline: JsonValue,
    pub markov_signals: Vec<JsonValue>,
    pub latest_markov_run: JsonValue,
    pub quiver_signals: Vec<JsonValue>,
    pub latest_quiver_run: JsonValue,
    pub latest_daily_indicator_run: JsonValue,
    pub performance_history: Vec<JsonValue>,
    pub performance_summary: JsonValue,
    pub integrity: JsonValue,
    pub market_status: JsonValue,
    pub trading_manager: JsonValue,
    pub watchlists: JsonValue,
    pub latest_decision: JsonValue,
    pub selected_decision: JsonValue,
}

#[derive(Debug, Deserialize)]
pub struct LimitParams {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct HermesReflectionRequest {
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub goal_version: Option<i64>,
    pub summary: String,
    pub findings: Option<JsonValue>,
    pub proposed_actions: Option<JsonValue>,
    pub source_session_id: Option<String>,
    pub raw_payload: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
pub struct HermesExperimentRequest {
    pub baseline_id: Option<String>,
    pub goal_version: Option<i64>,
    pub hypothesis: String,
    pub changed_variable_path: String,
    pub old_value: JsonValue,
    pub new_value: JsonValue,
    pub expected_effect: String,
    pub risk_notes: Option<String>,
    pub evidence: Option<JsonValue>,
    pub source_session_id: Option<String>,
    pub raw_payload: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
pub struct HermesDecisionAdviceRequest {
    pub decision_report_id: i64,
    pub source_session_id: Option<String>,
    pub overall_recommendation: String,
    pub summary: String,
    pub order_advice: Option<JsonValue>,
    pub learning_notes: Option<JsonValue>,
    pub context_self_check: Option<JsonValue>,
    pub raw_payload: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
pub struct HermesExperimentTransitionRequest {
    pub action: String,
    pub notes: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PerformanceParams {
    pub range_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ViewParams {
    pub view: Option<String>,
    pub range_key: Option<String>,
    pub report_id: Option<i64>,
    pub execution_page: Option<i64>,
    pub markov_page: Option<i64>,
    pub quiver_page: Option<i64>,
    pub scheduler_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CashBufferRequest {
    pub min_cash_buffer_pct: f64,
}

#[derive(Debug, Deserialize)]
pub struct MonthlyLossBreakerOverrideRequest {
    pub action: String,
    pub notes: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InstrumentQuarantineOverrideRequest {
    pub operation: String,
    pub symbol: String,
    pub side: String,
    pub signature: String,
    pub notes: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OverviewIntegrityAcknowledgementRequest {
    pub operation: String,
    pub issue_key: String,
    pub code: String,
    pub severity: String,
    pub notes: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LocalizationSettingsRequest {
    pub locale: Option<String>,
    pub time_zone: Option<String>,
    pub hour_cycle: Option<String>,
    pub week_start: Option<String>,
    pub group_separator: Option<String>,
    pub decimal_separator: Option<String>,
    pub measurement_system: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AiSettingsRequest {
    pub model: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SaxoCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}
