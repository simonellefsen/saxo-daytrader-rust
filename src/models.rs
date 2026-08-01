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
    pub estimated_unrealised_tax_dkk: f64,
    pub after_tax_estimate_status: String,
    pub daily_pnl_dkk: f64,
    pub position_count: i64,
    pub position_decision_stale_after_days: i64,
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
    pub execution_trade_thesis_evidence: JsonValue,
    pub execution_holding_thesis_reviews: JsonValue,
    pub execution_decision_pulse_evidence: JsonValue,
    pub reports: Vec<JsonValue>,
    pub manual_report_in_flight: bool,
    pub decision_pulse_statuses: Vec<JsonValue>,
    pub journal_entries: Vec<JsonValue>,
    pub scheduler_cycles: Vec<JsonValue>,
    pub hermes_reflections: Vec<JsonValue>,
    pub hermes_lessons_pending_review: Vec<JsonValue>,
    pub hermes_learning_memory: Vec<JsonValue>,
    pub hermes_one_variable_audit: Vec<JsonValue>,
    pub hermes_proposal_quality: Vec<JsonValue>,
    pub hermes_experiments: Vec<JsonValue>,
    pub hermes_decision_advice_audit: Vec<JsonValue>,
    pub hermes_counterfactuals: Vec<JsonValue>,
    pub missed_trade_shadows: Vec<JsonValue>,
    pub missed_trade_shadow_evidence: JsonValue,
    pub active_strategy_baseline: JsonValue,
    pub hermes_baseline_evidence_pack: JsonValue,
    pub markov_signals: Vec<JsonValue>,
    pub latest_markov_run: JsonValue,
    pub quiver_signals: Vec<JsonValue>,
    pub latest_quiver_run: JsonValue,
    pub quiver_conflicts: JsonValue,
    pub latest_daily_indicator_run: JsonValue,
    pub run_schedules: JsonValue,
    pub performance_history: Vec<JsonValue>,
    pub performance_summary: JsonValue,
    pub performance_benchmarks: JsonValue,
    pub performance_goal_tracking: JsonValue,
    pub integrity: JsonValue,
    pub execution_protection: JsonValue,
    pub market_status: JsonValue,
    pub trading_manager: JsonValue,
    pub watchlists: JsonValue,
    pub latest_decision: JsonValue,
    pub selected_decision: JsonValue,
    pub decision_gate_replay: JsonValue,
}

/// Bounded and redacted diagnostic payload for a Decision Report.
///
/// The persisted report still contains compatibility JSON because it captures
/// provider payloads. This response intentionally exposes only compact,
/// already-redacted strings so the debug endpoint cannot become a credential
/// or unbounded-payload transport path.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionReportDebugPayload {
    pub report_id: i64,
    pub created_at: String,
    pub status: String,
    pub payloads: DecisionReportDebugPayloads,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionReportDebugPayloads {
    pub prompt: String,
    pub request: String,
    pub provider_response: String,
    pub normalized_report: String,
}

/// Public health contract for the strict Decision Report response schema.
///
/// Schema construction remains dynamic because OpenRouter expects JSON Schema,
/// but callers receive a small typed summary instead of the construction tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionReportSchemaHealth {
    pub status: String,
    pub schema_name: String,
    pub strict: bool,
    pub issue_count: usize,
    pub issues: Vec<DecisionReportSchemaIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionReportSchemaIssue {
    pub path: String,
    pub message: String,
}

/// Minimal public runtime liveness and build-identity contract.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RuntimeHealth {
    pub status: String,
    pub runtime: String,
    pub git_sha: String,
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

/// Public read-only capital-reserve settings contract.
///
/// A request to the settings endpoint can preview a different reserve, but it
/// never persists or activates that request. `config_default_min_cash_buffer_pct`
/// stays pinned to the deployed configuration so callers can distinguish the
/// preview from the enforced baseline.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CashBufferSettings {
    pub min_cash_buffer_pct: f64,
    pub max_deployment_pct: f64,
    pub reinvestment_pressure_threshold_pct: f64,
    pub source: String,
    pub updated_at: Option<String>,
    pub config_default_min_cash_buffer_pct: f64,
}

#[derive(Debug, Deserialize)]
pub struct MonthlyLossBreakerOverrideRequest {
    pub action: String,
    pub notes: Option<String>,
    pub return_to: Option<String>,
}

/// `peak_value_dkk` is required to enable the override: it anchors the grant to
/// the peak the operator judged, so the exemption expires by itself once the
/// book prints a higher one.
#[derive(Debug, Deserialize)]
pub struct DrawdownGuardOverrideRequest {
    pub action: String,
    pub peak_value_dkk: Option<f64>,
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
pub struct ProtectiveStopPrecheckRequest {
    pub symbol: String,
    pub quantity: f64,
    pub stop_price_local: f64,
    pub confirm_sim_precheck: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProtectiveStopLifecyclePlacementRequest {
    pub source_precheck_id: i64,
    pub confirm_sim_placement: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProtectiveStopLifecycleCancellationRequest {
    pub lifecycle_test_id: i64,
    pub confirm_sim_cancellation: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProtectiveStopLifecycleReconcileRequest {
    pub lifecycle_test_id: i64,
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

#[derive(Deserialize)]
pub struct AiApiKeyRequest {
    pub api_key: Option<String>,
    pub return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SaxoCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}
