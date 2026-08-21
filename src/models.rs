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
    pub markov_filter: String,
    pub hermes_section: String,
    pub data_freshness: Vec<JsonValue>,
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
    pub tuning: TuningPayload,
}

/// Read-only evidence used to compare the scheduled decision pulses.
///
/// The first Tuning-tab slice deliberately gives execution-eligible reports
/// and observation-only shadow reports separate outcome fields. It is not an
/// execution score, a recommendation, or a composite strategy rating.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningPayload {
    pub generated_at: String,
    pub window_start: String,
    pub window_days: i64,
    pub status: String,
    pub pulse_comparison: Vec<TuningPulseComparison>,
    pub shadow_change_evidence: Vec<TuningShadowChangeEvidence>,
    pub shadow_support_risk_evidence: Vec<TuningShadowSupportRiskEvidence>,
    pub shadow_markov_evidence: Vec<TuningShadowMarkovEvidence>,
    pub shadow_gate_evidence: Vec<TuningShadowGateEvidence>,
    pub shadow_hermes_evidence: Vec<TuningShadowHermesEvidence>,
    pub execution_pulse_outcomes: Vec<TuningExecutionPulseOutcome>,
    pub execution_lifecycle_evidence: Vec<TuningExecutionLifecycleEvidence>,
    pub protective_stop_coverage: TuningProtectiveStopCoverage,
    pub execution_candidate_funnel: Vec<TuningExecutionCandidateFunnel>,
    pub safety: String,
    pub interpretation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningPulseComparison {
    pub pulse_key: String,
    pub pulse_label: String,
    pub authority: String,
    pub report_count: i64,
    pub terminal_success_count: i64,
    pub terminal_success_rate: Option<f64>,
    pub shadow_candidate_count: i64,
    pub shadow_comparable_candidate_count: i64,
    pub shadow_new_candidate_count: i64,
    pub shadow_repeated_candidate_count: i64,
    pub shadow_candidate_novelty_rate: Option<f64>,
    pub shadow_reference_captured_count: i64,
    pub one_session_outcome_count: i64,
    pub five_session_outcome_count: i64,
    pub twenty_session_outcome_count: i64,
    pub five_session_after_cost_count: i64,
    pub five_session_after_cost_positive_rate: Option<f64>,
    pub maturity: String,
    pub outcome_status: String,
}

/// Server-normalized comparison evidence for a midpoint shadow report.
/// Candidate counts are deliberately not used to derive these statuses.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowChangeEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub report_count: i64,
    pub comparison_available_assessment_count: i64,
    pub material_change_count: i64,
    pub no_new_information_count: i64,
    pub no_new_information_rate: Option<f64>,
    pub opening_reference_not_available_count: i64,
    pub not_applicable_count: i64,
    pub comparison_invalid_count: i64,
    pub missing_assessment_count: i64,
    pub unclassified_assessment_count: i64,
}

/// Support-risk context captured with a shadow candidate at report time.
/// It is observational metadata, not an automatic risk gate or a forecast.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowSupportRiskEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub candidate_count: i64,
    pub snapshot_available_count: i64,
    pub low_break_risk_count: i64,
    pub moderate_break_risk_count: i64,
    pub high_break_risk_count: i64,
    pub unavailable_count: i64,
    pub complete_context_count: i64,
    pub average_break_risk: Option<f64>,
    pub average_confidence: Option<f64>,
    pub average_history_coverage: Option<f64>,
    pub unclassified_count: i64,
}

/// Markov context captured with a shadow candidate at report time. It is a
/// saved-signal coverage summary only; it neither reruns Markov nor becomes a
/// Trading Manager gate, forecast, or execution signal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowMarkovEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub candidate_count: i64,
    pub snapshot_available_count: i64,
    pub long_direction_count: i64,
    pub short_direction_count: i64,
    pub neutral_direction_count: i64,
    pub unavailable_count: i64,
    pub complete_signal_count: i64,
    pub average_signed_signal: Option<f64>,
    pub unclassified_count: i64,
}

/// A bounded, decision-time-only signal-gate summary for shadow candidates.
/// It deliberately does not recreate the Trading Manager or broker path.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowGateEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub candidate_count: i64,
    pub technical_source_count: i64,
    pub markov_fallback_source_count: i64,
    pub not_evaluated_source_count: i64,
    pub clear_signal_count: i64,
    pub blocked_signal_count: i64,
    pub insufficient_evidence_count: i64,
    pub unclassified_count: i64,
}

/// Record-only Hermes evidence for shadow candidates. These values never fed
/// a manager gate, queue, broker precheck, or order mutation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowHermesEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub candidate_count: i64,
    pub allow_count: i64,
    pub reduce_count: i64,
    pub stand_down_count: i64,
    pub review_count: i64,
    pub no_matching_advice_count: i64,
    pub unavailable_count: i64,
    pub not_requested_count: i64,
    pub self_check_complete_count: i64,
    pub self_check_incomplete_count: i64,
    pub self_check_not_recorded_count: i64,
    pub approved_policy_source_count: i64,
    pub missing_policy_source_count: i64,
    pub unclassified_effect_count: i64,
}

/// Execution-attributed outcome evidence for one execution-eligible pulse.
/// BUY forward movement and reconciled SELL accounting remain separate fields;
/// neither is comparable to the shadow quote-to-close observation ledger.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningExecutionPulseOutcome {
    pub pulse_key: String,
    pub pulse_label: String,
    pub attributed_order_count: i64,
    pub filled_buy_order_count: i64,
    pub one_session: TuningDirectionalOutcome,
    pub five_session: TuningDirectionalOutcome,
    pub reconciled_sell_order_count: i64,
    pub realised_sell_gain_dkk: f64,
    pub realised_sell_commission_dkk: f64,
    pub realised_sell_tax_dkk: f64,
    pub maturity: String,
    pub interpretation: String,
}

/// Persisted local execution-order lifecycle counts for an execution-eligible
/// pulse. This is status coverage, not a latency or broker-quality claim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningExecutionLifecycleEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub attributed_order_count: i64,
    pub locally_queued_count: i64,
    pub broker_active_count: i64,
    pub broker_state_unknown_count: i64,
    pub executed_count: i64,
    pub failed_count: i64,
    pub expired_count: i64,
    pub cancelled_count: i64,
    pub unclassified_count: i64,
}

/// Current local protective-stop coverage, kept distinct from time-windowed
/// pulse outcome evidence. Only broker-confirmed stop states count as covered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningProtectiveStopCoverage {
    pub status: String,
    pub position_count: i64,
    pub protected_count: i64,
    pub partial_count: i64,
    pub planned_count: i64,
    pub unprotected_count: i64,
    pub exception_count: i64,
    pub confirmed_coverage_ratio: Option<f64>,
}

/// Bounded, persisted manager-path counts for an execution-eligible pulse.
/// The final stage is a local execution row, not a claim of broker submission
/// or a currently pending order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningExecutionCandidateFunnel {
    pub pulse_key: String,
    pub pulse_label: String,
    pub report_count: i64,
    pub manager_run_count: i64,
    pub manager_snapshot_missing_count: i64,
    pub candidate_order_count: i64,
    pub eligible_candidate_order_count: i64,
    pub hermes_matched_candidate_count: i64,
    pub approved_order_count: i64,
    pub skipped_order_count: i64,
    pub local_execution_row_count: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningDirectionalOutcome {
    pub sample_count: i64,
    pub average_directional_return_pct: Option<f64>,
    pub positive_return_rate: Option<f64>,
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

/// Compact operator-facing summary of the currently available AI prompt
/// surfaces. Provider-shaped report data remains compatibility JSON because it
/// is persisted from the report pipeline; this envelope fixes the public API
/// contract without exposing a new provider or execution path.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiPromptsPayload {
    pub generated_at: String,
    pub items: Vec<AiPromptItem>,
    pub latest_decision_report: Option<JsonValue>,
    pub latest_trading_manager_run: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiPromptItem {
    pub kind: String,
    pub title: String,
    pub status: String,
    pub description: String,
}

/// Small latest-report lookup contract for polling clients.
///
/// Decision Report rows remain compatibility JSON because they originate in
/// the persisted provider/report pipeline. This type fixes the public envelope
/// without changing report generation or Trading Manager behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionLatestPayload {
    pub report: Option<JsonValue>,
    pub next_report: Option<JsonValue>,
}

/// Bounded Decision Report list envelope.
///
/// The list itself is stable, but each persisted report remains compatibility
/// JSON while the provider/report pipeline is ported incrementally.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionReportListPayload {
    pub items: Vec<JsonValue>,
}

/// Bounded portfolio position-list envelope.
///
/// Individual position rows remain compatibility JSON while the portfolio
/// read model is converted incrementally. This type fixes the public
/// count/list contract without changing Saxo or execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortfolioPositionsPayload {
    pub total: usize,
    pub items: Vec<JsonValue>,
}

/// Bounded portfolio trade-list envelope.
///
/// Individual trade rows remain compatibility JSON while the persisted
/// portfolio trade read model is converted incrementally. This makes the
/// public list boundary explicit without changing trade-ledger behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortfolioTradesPayload {
    pub items: Vec<JsonValue>,
}

/// Bounded strategy-journal list envelope.
///
/// Individual journal rows remain compatibility JSON while the persisted
/// strategy-learning read model is converted incrementally. This makes the
/// public list boundary explicit without changing Hermes or execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrategyJournalPayload {
    pub items: Vec<JsonValue>,
}

/// Bounded Execution-tab envelope.
///
/// Persisted order, fill, and event rows remain compatibility JSON while the
/// execution read model is ported incrementally. This makes the public
/// read-only boundary explicit without changing broker synchronization or
/// execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExecutionPayload {
    pub orders: Vec<JsonValue>,
    pub fills: Vec<JsonValue>,
    pub events: Vec<JsonValue>,
}

/// Bounded scheduler-status envelope.
///
/// The scheduler status snapshot and persisted cycle rows remain compatibility
/// JSON while the scheduler read model is converted incrementally. This makes
/// the public read-only boundary explicit without changing scheduler behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SchedulerPayload {
    pub status: JsonValue,
    pub cycles: Vec<JsonValue>,
}

/// Bounded Hermes reflection-list envelope.
///
/// Individual persisted reflections remain compatibility JSON while the Hermes
/// read model is converted incrementally. This makes the protected advisory
/// read boundary explicit without changing reflection or proposal behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HermesReflectionsPayload {
    pub items: Vec<JsonValue>,
}

/// Bounded Hermes experiment-list envelope.
///
/// Individual persisted experiment rows remain compatibility JSON while the
/// Hermes read model is converted incrementally. This keeps the protected
/// advisory read boundary explicit without changing proposal lifecycle or
/// activation behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HermesExperimentsPayload {
    pub items: Vec<JsonValue>,
}

/// Bounded market-watchlists envelope.
///
/// Universe metadata and category rows remain compatibility JSON while the
/// read model is converted incrementally. This makes cache timing and the
/// stable top-level watchlist contract explicit without changing quote
/// collection, candidate membership, or Decision Report context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketWatchlistsPayload {
    pub generated_at: String,
    pub cache_ttl_seconds: i64,
    pub universe: JsonValue,
    pub categories: Vec<JsonValue>,
}

/// Bounded market-status envelope.
///
/// Exchange rows plus scheduler and price-monitor details remain compatibility
/// JSON while the read model is converted incrementally. This keeps the public
/// observability boundary explicit without changing market-calendar refreshes
/// or any decision and execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketStatusPayload {
    pub items: Vec<JsonValue>,
    pub summary: JsonValue,
    pub scheduler: JsonValue,
    pub price_monitor: JsonValue,
}

/// Bounded performance envelope.
///
/// History rows, benchmark data, and goal-tracking details remain compatibility
/// JSON while the performance read model is converted incrementally. This makes
/// the stable public response boundary explicit without changing performance
/// collection or any decision and execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePayload {
    pub range_key: String,
    pub history: Vec<JsonValue>,
    pub summary: JsonValue,
    pub benchmarks: JsonValue,
    pub goal_tracking: JsonValue,
}

/// Bounded Decision Gate Replay envelope.
///
/// Scenario and support-risk evidence details remain compatibility JSON while
/// the historical-analysis read model is converted incrementally. This makes
/// the stable public replay boundary explicit without changing report
/// generation, configuration, or any decision and execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionGateReplayPayload {
    pub status: String,
    pub run_count: usize,
    pub scenarios: Vec<JsonValue>,
    pub safety: String,
    pub interpretation: String,
    pub support_risk_evidence: JsonValue,
}

/// Bounded Markov signal-list envelope.
///
/// The latest-run summary and individual signal rows remain compatibility JSON
/// while the persisted Markov read model is converted incrementally. Keeping
/// the outer response typed preserves the established public API boundary
/// without changing regime calculation or downstream advisory behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MarkovSignalsPayload {
    pub latest_run: JsonValue,
    pub items: Vec<JsonValue>,
}

/// Bounded Quiver signal-list envelope.
///
/// The latest-run summary and individual signal rows remain compatibility JSON
/// while the persisted Quiver read model is converted incrementally. Keeping
/// the outer response typed preserves the established public API boundary
/// without changing collection or downstream advisory behavior.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuiverSignalsPayload {
    pub latest_run: JsonValue,
    pub items: Vec<JsonValue>,
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
    pub markov_filter: Option<String>,
    pub hermes_section: Option<String>,
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
