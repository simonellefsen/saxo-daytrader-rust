use std::collections::BTreeMap;

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
    pub performance_history: Vec<PerformanceHistoryRowPayload>,
    pub performance_summary: Option<PerformanceSummaryPayload>,
    pub performance_benchmarks: Option<PerformanceBenchmarksPayload>,
    pub performance_goal_tracking: Option<PerformanceGoalTrackingPayload>,
    pub performance_snapshot_evidence: Option<PerformanceSnapshotEvidencePayload>,
    pub performance_pnl_reconciliation: Option<PerformancePnlReconciliationPayload>,
    pub performance_exposure_attribution: Option<PerformanceExposureAttributionPayload>,
    pub performance_realised_sell_outcomes: Option<PerformanceRealisedSellOutcomesPayload>,
    pub integrity: OverviewIntegrityPayload,
    pub execution_protection: ProtectiveStopCoveragePayload,
    pub market_status: MarketStatusPayload,
    pub trading_manager: TradingManagerPayload,
    pub watchlists: MarketWatchlistsPayload,
    pub latest_decision: LatestDecisionStatusPayload,
    pub selected_decision: JsonValue,
    pub decision_gate_replay: DecisionGateReplayPayload,
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
    pub shadow_quiver_evidence: Vec<TuningShadowQuiverEvidence>,
    pub shadow_gate_evidence: Vec<TuningShadowGateEvidence>,
    pub shadow_hermes_evidence: Vec<TuningShadowHermesEvidence>,
    pub execution_pulse_outcomes: Vec<TuningExecutionPulseOutcome>,
    pub execution_lifecycle_evidence: Vec<TuningExecutionLifecycleEvidence>,
    pub protective_stop_coverage: TuningProtectiveStopCoverage,
    pub execution_candidate_funnel: Vec<TuningExecutionCandidateFunnel>,
    pub trade_thesis_evidence: TuningTradeThesisEvidence,
    pub experiment_governance: TuningExperimentGovernance,
    pub portfolio_outcome: TuningPortfolioOutcome,
    pub monthly_goal_progress: TuningMonthlyGoalProgress,
    pub benchmark_comparison: TuningBenchmarkComparison,
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
    pub shadow_candidate_report_count: i64,
    pub shadow_reports_missing_outcome_count: i64,
    pub shadow_awaiting_reference_count: i64,
    pub shadow_comparable_candidate_count: i64,
    pub shadow_new_candidate_count: i64,
    pub shadow_repeated_candidate_count: i64,
    pub shadow_candidate_novelty_rate: Option<f64>,
    pub shadow_reference_captured_count: i64,
    pub shadow_reference_unavailable_retroactive_count: i64,
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

/// Quiver context captured with a shadow candidate at report time. It is a
/// saved advisory-signal coverage summary only; it neither refreshes Quiver
/// nor becomes a Trading Manager gate, forecast, or execution signal.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningShadowQuiverEvidence {
    pub pulse_key: String,
    pub pulse_label: String,
    pub candidate_count: i64,
    pub snapshot_available_count: i64,
    pub fresh_source_count: i64,
    pub partial_source_count: i64,
    pub stale_source_count: i64,
    pub unavailable_source_count: i64,
    pub bullish_direction_count: i64,
    pub bearish_direction_count: i64,
    pub neutral_direction_count: i64,
    pub complete_signal_count: i64,
    pub average_signal: Option<f64>,
    pub average_confidence: Option<f64>,
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

/// Bounded post-fill evidence for recorded BUY theses. Its newest-recorded
/// thesis scope is intentionally separate from the Tuning pulse window and it
/// remains a gross directional observation, not realised P/L.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningTradeThesisEvidence {
    pub status: String,
    pub recorded_thesis_count: i64,
    pub filled_thesis_count: i64,
    pub one_session: TuningDirectionalOutcome,
    pub five_session: TuningDirectionalOutcome,
    pub minimum_complete_observations: i64,
    pub scan_limit: i64,
    pub scope: String,
    pub gross_net_label: String,
    pub interpretation: String,
}

/// Lifecycle inventory for retained one-variable strategy experiments. It is
/// governance metadata only, not evidence of performance or an activation
/// control; experiment values and rationale remain outside this summary.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningExperimentGovernance {
    pub status: String,
    pub total_experiment_count: i64,
    pub pending_review_count: i64,
    pub approved_paper_count: i64,
    pub approved_sim_count: i64,
    pub active_paper_count: i64,
    pub active_sim_count: i64,
    pub ready_for_promotion_count: i64,
    pub promoted_count: i64,
    pub terminal_count: i64,
    pub unclassified_count: i64,
    pub scope: String,
    pub interpretation: String,
}

/// Read-only, one-month local account-value context. It intentionally reports
/// simple snapshot movement rather than realised P/L, time-weighted return, or
/// a portfolio-performance attribution result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningPortfolioOutcome {
    pub status: String,
    pub range_key: String,
    pub snapshot_count: i64,
    pub valid_snapshot_count: i64,
    pub latest_value_dkk: Option<f64>,
    pub change_dkk: Option<f64>,
    pub simple_return_pct: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub latest_recorded_at: String,
    pub latest_snapshot_type: String,
    pub latest_source: String,
    pub age_minutes: Option<i64>,
    pub unreliable_cost_basis_points: i64,
    pub scope: String,
    pub return_kind: String,
    pub caveat: String,
}

/// Read-only calendar-month progress against the configured DKK portfolio
/// target. This is a local account-value comparison, not realised P/L or a
/// risk, sizing, or execution control.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningMonthlyGoalProgress {
    pub status: String,
    pub target_dkk: Option<f64>,
    pub value_change_dkk: Option<f64>,
    pub target_progress: Option<f64>,
    pub baseline_value_dkk: Option<f64>,
    pub period_start: String,
    pub scope: String,
    pub caveat: String,
}

/// Read-only one-month account-value comparison against stored native-currency
/// ETF proxy price returns. It is deliberately not a normalized total-return
/// or performance-attribution measure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningBenchmarkComparison {
    pub status: String,
    pub portfolio_return_pct: Option<f64>,
    pub ready_count: i64,
    pub reference_count: i64,
    pub aligned_count: i64,
    pub prior_close_count: i64,
    pub stale_close_count: i64,
    pub freshness: String,
    pub collector_status: String,
    pub collector_run_at: String,
    pub collector_run_date: String,
    pub collector_reference_count: i64,
    pub collector_success_count: i64,
    pub collector_error_count: i64,
    pub references: Vec<TuningBenchmarkReference>,
    pub scope: String,
    pub return_kind: String,
    pub caveat: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuningBenchmarkReference {
    pub key: String,
    pub label: String,
    pub symbol: String,
    pub status: String,
    pub benchmark_return_pct: Option<f64>,
    pub excess_return_pct: Option<f64>,
    pub freshness: String,
    pub baseline_at: String,
    pub latest_at: String,
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

/// Bounded dashboard integrity envelope.
///
/// Individual findings retain compatibility JSON because each check carries
/// check-specific diagnostic detail and acknowledgement metadata. The stable
/// status, timing, and list boundaries are typed so dashboard health cannot
/// accidentally traverse an arbitrary overview document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewIntegrityPayload {
    pub healthy: bool,
    pub warnings: Vec<JsonValue>,
    pub mismatches: Vec<JsonValue>,
    pub expiry_pending_orders: Vec<JsonValue>,
    pub acknowledged_issue_count: i64,
    pub checked_at: String,
}

/// Bounded dashboard Trading Manager envelope.
///
/// The latest persisted run remains compatibility JSON because its diagnostics
/// evolve with Trading Manager gates. The stable availability and latest-run
/// boundary are typed so dashboard panels do not traverse an arbitrary
/// overview document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradingManagerPayload {
    pub status: String,
    pub latest_run: JsonValue,
}

/// Bounded protective-stop coverage envelope for the Execution dashboard.
///
/// Per-position, exception, and recorded SIM-test details remain compatibility
/// JSON because their fields depend on broker state and lifecycle evidence.
/// This read model neither invokes Saxo nor changes any order state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtectiveStopCoveragePayload {
    pub status: String,
    pub summary: JsonValue,
    pub positions: Vec<JsonValue>,
    pub exceptions: Vec<JsonValue>,
    pub recent_prechecks: Vec<JsonValue>,
    pub recent_lifecycle_tests: Vec<JsonValue>,
    pub safety: String,
    pub interpretation: String,
}

/// Stable latest-Decision-Report metadata used across dashboard tabs.
///
/// The normalized report and provider-shaped detail remain staged JSON in the
/// Decisions view. This compact status never generates a report or changes
/// Trading Manager or broker behavior.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatestDecisionStatusPayload {
    pub id: Option<i64>,
    pub created_at: Option<String>,
    pub status: Option<String>,
    pub model: Option<String>,
    pub error_text: Option<String>,
}

/// Stable metadata for a retained aggregate/position snapshot.
///
/// Per-position rows are intentionally not included here: their detailed
/// values remain compatibility JSON in the surrounding evidence envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedPositionSnapshotMetadataPayload {
    pub snapshot_id: i64,
    pub recorded_at: String,
    pub snapshot_type: String,
    pub source: Option<String>,
    pub position_count: i64,
    pub invested_market_value_dkk: f64,
    pub total_cost_basis_dkk: f64,
    pub total_unrealised_pnl_dkk: f64,
}

/// One recomputable position row retained with a historical portfolio snapshot.
///
/// Every DKK value is tied to the stored quantity, local price, FX rate, and
/// cost basis from the same observation; this is not a live broker position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedPositionSnapshotItemPayload {
    pub symbol: String,
    pub isin: Option<String>,
    pub currency: String,
    pub quantity: f64,
    pub price_local: f64,
    pub fx_rate_to_dkk: f64,
    pub cost_basis_local: f64,
    pub cost_basis_dkk: f64,
    pub market_value_dkk: f64,
    pub unrealised_pnl_dkk: f64,
}

/// Bounded latest retained-position evidence envelope.
///
/// The stored snapshot metadata, availability state, and per-position rows are
/// explicit; only independently staged change and mismatch detail remains
/// compatibility JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedPositionSnapshotEvidencePayload {
    pub status: String,
    pub snapshot: Option<RetainedPositionSnapshotMetadataPayload>,
    pub items: Vec<RetainedPositionSnapshotItemPayload>,
    pub safety: String,
    pub interpretation: Option<String>,
}

/// One symbol's quantity and stored-value change between two retained snapshots.
///
/// This is an observational comparison only: market-value movement includes
/// price, FX, and quantity effects, and does not assert a trade or fill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedPositionSnapshotChangeItemPayload {
    pub symbol: String,
    pub quantity_before: f64,
    pub quantity_after: f64,
    pub quantity_change: f64,
    pub market_value_change_dkk: f64,
    pub cost_basis_change_dkk: f64,
}

/// Bounded retained-position composition-change evidence envelope.
///
/// Snapshot metadata, aggregate change counters, and per-symbol change rows
/// are typed. Optional fields preserve the collecting state before two
/// snapshots exist.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetainedPositionSnapshotChangeEvidencePayload {
    pub status: String,
    pub current_snapshot: Option<RetainedPositionSnapshotMetadataPayload>,
    pub previous_snapshot: Option<RetainedPositionSnapshotMetadataPayload>,
    #[serde(default)]
    pub opened: Vec<RetainedPositionSnapshotChangeItemPayload>,
    #[serde(default)]
    pub closed: Vec<RetainedPositionSnapshotChangeItemPayload>,
    #[serde(default)]
    pub resized: Vec<RetainedPositionSnapshotChangeItemPayload>,
    pub opened_count: Option<i64>,
    pub closed_count: Option<i64>,
    pub resized_count: Option<i64>,
    pub unchanged_quantity_count: Option<i64>,
    pub net_market_value_change_dkk: Option<f64>,
    pub net_cost_basis_change_dkk: Option<f64>,
    pub safety: String,
    pub interpretation: Option<String>,
}

/// Aggregate and retained-position counts for one integrity comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotPositionCountPayload {
    pub aggregate: i64,
    pub detail: i64,
    pub mismatch: bool,
}

/// One structural aggregate-versus-position snapshot mismatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotStructuralMismatchPayload {
    pub snapshot_id: i64,
    pub recorded_at: String,
    pub position_count: PerformanceSnapshotPositionCountPayload,
    pub market_value_difference_dkk: f64,
    pub market_value_mismatch: bool,
    pub cost_basis_difference_dkk: f64,
    pub cost_basis_mismatch: bool,
}

/// One broker-derived aggregate unrealised-P/L difference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotBrokerPnlDifferencePayload {
    pub snapshot_id: i64,
    pub recorded_at: String,
    pub difference_dkk: f64,
    pub aggregate_unrealised_pnl_dkk: f64,
    pub recomputed_unrealised_pnl_dkk: f64,
    pub interpretation: String,
}

/// Absolute and relative tolerances used by snapshot integrity diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotIntegrityTolerancePayload {
    pub absolute_dkk: f64,
    pub relative: f64,
}

/// Bounded aggregate-versus-position snapshot integrity envelope.
///
/// The diagnosis state, counters, mismatch observations, and tolerances are
/// all explicit historical diagnostic fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotIntegrityPayload {
    pub status: String,
    pub checked_snapshot_count: i64,
    pub structural_mismatch_count: i64,
    pub structural_mismatches: Vec<PerformanceSnapshotStructuralMismatchPayload>,
    pub broker_derived_unrealised_difference_count: i64,
    pub broker_derived_unrealised_differences: Vec<PerformanceSnapshotBrokerPnlDifferencePayload>,
    pub tolerance: PerformanceSnapshotIntegrityTolerancePayload,
    pub safety: String,
}

/// Bounded retained-position snapshot evidence envelope.
///
/// The selected-range coverage and retention contract is typed so callers can
/// distinguish collecting, partial, and complete evidence without traversing
/// arbitrary JSON. Its retained evidence and integrity diagnostics are fully
/// typed; history, benchmark, and goal-tracking models remain staged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshotEvidencePayload {
    pub status: String,
    pub range_key: String,
    pub aggregate_snapshot_count: i64,
    pub covered_snapshot_count: i64,
    pub missing_legacy_snapshot_count: i64,
    pub coverage_pct: Option<f64>,
    pub snapshots_with_position_rows: i64,
    pub position_evidence_row_count: i64,
    pub first_covered_at: Option<String>,
    pub latest_covered_at: Option<String>,
    pub latest_snapshot: RetainedPositionSnapshotEvidencePayload,
    pub latest_change: RetainedPositionSnapshotChangeEvidencePayload,
    pub detail_retention: String,
    pub integrity: PerformanceSnapshotIntegrityPayload,
    pub safety: String,
    pub interpretation: String,
}

/// Dashboard aggregate unrealised-P/L evidence for one response.
///
/// It is a local current aggregate, not a broker time-weighted performance
/// measurement or a quote for an individual instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePnlDashboardSourcePayload {
    pub status: String,
    pub unrealised_pnl_dkk: Option<f64>,
    pub source: String,
    pub snapshot_type: String,
}

/// Most recent persisted account-value observation used for P/L comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePnlHistorySourcePayload {
    pub status: String,
    pub unrealised_pnl_dkk: Option<f64>,
    pub source: String,
    pub snapshot_type: String,
    pub recorded_at: Option<String>,
    pub difference_from_dashboard_dkk: Option<f64>,
}

/// Stored Saxo instrument-exposure aggregate used for P/L comparison.
///
/// The timestamp and per-instrument FX basis remain explicit so callers do
/// not treat the stored observation as a real-time broker quote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePnlBrokerExposurePayload {
    pub status: String,
    pub unrealised_pnl_dkk: Option<f64>,
    pub difference_from_dashboard_dkk: Option<f64>,
    pub account_currency: Option<String>,
    pub fx_basis: Option<String>,
    pub instrument_fx_rates_to_dkk: Option<BTreeMap<String, f64>>,
    pub exposure_count: Option<i64>,
    pub updated_at: Option<String>,
}

/// Read-only reconciliation between the dashboard, latest local snapshot, and
/// stored Saxo exposure aggregate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePnlReconciliationPayload {
    pub scope: String,
    pub dashboard: PerformancePnlDashboardSourcePayload,
    pub latest_history: PerformancePnlHistorySourcePayload,
    pub broker_exposure: PerformancePnlBrokerExposurePayload,
}

/// One stored Saxo instrument exposure converted to DKK for attribution.
///
/// Its currency is a grouping label. It does not isolate FX P/L or represent
/// a real-time quote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceExposureAttributionRowPayload {
    pub symbol: String,
    pub instrument_currency: String,
    pub quantity: Option<f64>,
    pub unrealised_pnl_dkk: f64,
    pub profit_loss_instrument_currency: f64,
    pub fx_rate_to_dkk: f64,
    pub calculation_reliability: Option<String>,
    pub updated_at: Option<String>,
}

/// Aggregate exposure attribution for one instrument currency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceExposureCurrencyPayload {
    pub instrument_currency: String,
    pub symbol_count: i64,
    pub unrealised_pnl_dkk: f64,
    pub absolute_contribution_pct: Option<f64>,
}

/// Read-only stored Saxo exposure P/L attribution.
///
/// Exposure P/L is converted using each instrument currency's recorded DKK
/// rate; it must not be interpreted as realised P/L, a trading signal, or a
/// standalone FX-P/L decomposition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceExposureAttributionPayload {
    pub status: String,
    pub scope: String,
    pub account_currency: Option<String>,
    pub fx_basis: Option<String>,
    pub instrument_fx_rates_to_dkk: Option<BTreeMap<String, f64>>,
    pub updated_at: Option<String>,
    pub exposure_count: i64,
    #[serde(default)]
    pub shown_row_count: i64,
    pub total_unrealised_pnl_dkk: Option<f64>,
    #[serde(default)]
    pub rows: Vec<PerformanceExposureAttributionRowPayload>,
    #[serde(default)]
    pub currencies: Vec<PerformanceExposureCurrencyPayload>,
}

/// One closed-sale ledger row used by local realised-outcome evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceRealisedSellRowPayload {
    pub created_at: Option<String>,
    pub symbol: String,
    pub instrument_name: Option<String>,
    pub quantity: Option<f64>,
    pub currency: Option<String>,
    pub realised_gain_dkk: f64,
    pub commission_dkk: f64,
    pub tax_dkk: f64,
    pub cost_basis_sold_dkk: f64,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub execution_order_id: Option<i64>,
    pub linked_order_count: Option<i64>,
    pub exit_strategy_type: Option<String>,
    pub exit_strategy_role: Option<String>,
}

/// One local realised-outcome aggregation by symbol or instrument currency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceRealisedSellAttributionPayload {
    pub symbol: Option<String>,
    pub instrument_currency: Option<String>,
    pub closed_sale_count: i64,
    pub realised_gain_dkk: f64,
    pub commission_dkk: f64,
    pub tax_dkk: f64,
}

/// One recorded SELL-route aggregation from linked local execution evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceRealisedSellExitRoutePayload {
    pub exit_route: String,
    pub link_status: String,
    pub closed_sale_count: i64,
    pub realised_gain_dkk: f64,
    pub commission_dkk: f64,
    pub tax_dkk: f64,
}

/// Read-only local accounting evidence for reconciled SELL ledger rows.
///
/// Partial sales are individual rows. The ledger deliberately makes no claim
/// about holding time, realised slippage, entry-strategy attribution, or a
/// backtested trading edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceRealisedSellOutcomesPayload {
    pub status: String,
    pub scope: String,
    pub counting_unit: String,
    pub sample_requirement: i64,
    #[serde(default)]
    pub scan_limit: i64,
    pub closed_sale_count: i64,
    #[serde(default)]
    pub decisive_sale_count: i64,
    #[serde(default)]
    pub win_count: i64,
    #[serde(default)]
    pub loss_count: i64,
    #[serde(default)]
    pub breakeven_count: i64,
    pub win_rate: Option<f64>,
    pub average_win_dkk: Option<f64>,
    pub average_loss_dkk: Option<f64>,
    pub payoff_ratio: Option<f64>,
    pub total_realised_gain_dkk: Option<f64>,
    pub total_commission_dkk: Option<f64>,
    pub total_tax_dkk: Option<f64>,
    pub total_cost_basis_sold_dkk: Option<f64>,
    #[serde(default)]
    pub attributed_symbol_count: i64,
    #[serde(default)]
    pub shown_symbol_attribution_count: i64,
    #[serde(default)]
    pub symbol_attribution: Vec<PerformanceRealisedSellAttributionPayload>,
    #[serde(default)]
    pub currency_attribution: Vec<PerformanceRealisedSellAttributionPayload>,
    #[serde(default)]
    pub linked_exit_route_count: i64,
    #[serde(default)]
    pub unlinked_ledger_count: i64,
    #[serde(default)]
    pub ambiguous_exit_link_count: i64,
    #[serde(default)]
    pub exit_route_attribution: Vec<PerformanceRealisedSellExitRoutePayload>,
    #[serde(default)]
    pub recent_rows: Vec<PerformanceRealisedSellRowPayload>,
    pub holding_time_status: String,
    pub slippage_status: String,
}

/// Evidence provenance for a performance-range summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSummaryConfidencePayload {
    pub status: String,
    pub valid_points: i64,
    pub latest_recorded_at: Option<String>,
    pub latest_snapshot_type: Option<String>,
    pub latest_source: Option<String>,
    pub age_minutes: Option<i64>,
    pub scope: String,
}

/// Deterministic summary of the selected local account-value history range.
///
/// It is derived only from persisted/current account-value snapshots and never
/// represents a broker time-weighted return or a live quote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSummaryPayload {
    pub points: i64,
    pub first_recorded_at: Option<String>,
    pub latest_recorded_at: Option<String>,
    pub first_total_market_value_dkk: f64,
    pub latest_total_market_value_dkk: f64,
    pub change_dkk: f64,
    pub daily_pnl_dkk: f64,
    pub position_count: i64,
    pub range_return_pct: Option<f64>,
    pub range_max_drawdown_pct: Option<f64>,
    pub confidence: PerformanceSummaryConfidencePayload,
    pub unreliable_cost_basis_points: i64,
}

/// Weekly or monthly local-history goal progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceGoalPeriodPayload {
    pub status: String,
    pub pnl_dkk: Option<f64>,
    pub target_dkk: f64,
    pub progress_pct: Option<f64>,
    pub baseline_value_dkk: Option<f64>,
    pub period_start_utc: String,
}

/// Since-reset local-history performance, which has no target by design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSinceResetPayload {
    pub status: String,
    pub pnl_dkk: Option<f64>,
    pub return_pct: Option<f64>,
    pub baseline_value_dkk: Option<f64>,
    pub baseline_recorded_at: Option<String>,
}

/// Read-only local portfolio-value goal tracking.
///
/// Period baselines are scoped to the active import batch, so a portfolio reset
/// cannot bleed earlier history into current week/month progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceGoalTrackingPayload {
    pub weekly_target_dkk: f64,
    pub monthly_target_dkk: f64,
    pub basis: String,
    pub periods: PerformanceGoalPeriodsPayload,
}

/// Grouped goal periods with deliberately different since-reset semantics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceGoalPeriodsPayload {
    pub week: PerformanceGoalPeriodPayload,
    pub month: PerformanceGoalPeriodPayload,
    pub since_reset: PerformanceSinceResetPayload,
}

/// One configured read-only ETF proxy comparison against the selected local
/// portfolio-value range.
///
/// Return and close fields are absent while the proxy history is still being
/// collected. The comparison is deliberately not a time-weighted or
/// total-return calculation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceBenchmarkReferencePayload {
    pub key: String,
    pub label: String,
    pub symbol: String,
    pub status: String,
    pub portfolio_return_pct: Option<f64>,
    pub benchmark_return_pct: Option<f64>,
    pub excess_return_pct: Option<f64>,
    pub baseline_close: Option<f64>,
    pub latest_close: Option<f64>,
    pub baseline_at: Option<String>,
    pub latest_at: Option<String>,
    pub freshness: Option<String>,
}

/// One persisted local benchmark refresh run.
///
/// This is operational coverage evidence only; it does not change proxy
/// returns, decision context, or execution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceBenchmarkRunPayload {
    pub id: String,
    pub created_at: String,
    pub run_date: String,
    pub status: String,
    pub reference_count: i64,
    pub success_count: i64,
    pub error_count: i64,
}

/// Read-only selected-range comparison against configured ETF price proxies.
///
/// The optional comparison fields and latest run preserve disabled and
/// collecting states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceBenchmarksPayload {
    pub status: String,
    pub latest_run: Option<PerformanceBenchmarkRunPayload>,
    pub portfolio_baseline_at: Option<String>,
    pub portfolio_latest_at: Option<String>,
    pub portfolio_return_pct: Option<f64>,
    pub ready_count: Option<i64>,
    pub reference_count: Option<i64>,
    pub aligned_count: Option<i64>,
    pub prior_close_count: Option<i64>,
    pub stale_close_count: Option<i64>,
    pub freshness: Option<String>,
    pub references: Vec<PerformanceBenchmarkReferencePayload>,
    pub caveat: Option<String>,
}

/// One stored or response-time account-value observation in DKK.
///
/// This is local aggregate evidence, including cash, rather than a
/// broker-computed time-weighted return or a live security quote. Older stored
/// rows may not have a source label, which remains explicit as `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformanceHistoryRowPayload {
    pub recorded_at: String,
    pub snapshot_type: String,
    pub total_market_value_dkk: f64,
    pub invested_market_value_dkk: f64,
    pub cash_balance_dkk: f64,
    pub total_cost_basis_dkk: f64,
    pub total_unrealised_pnl_dkk: f64,
    pub total_daily_pnl_dkk: f64,
    pub position_count: i64,
    pub source: Option<String>,
}

/// Bounded performance envelope.
///
/// History rows, selected-range summary, benchmark comparison, and local goal
/// tracking are typed projections. This does not change performance collection
/// or any decision/execution behavior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PerformancePayload {
    pub range_key: String,
    pub history: Vec<PerformanceHistoryRowPayload>,
    pub summary: PerformanceSummaryPayload,
    pub benchmarks: PerformanceBenchmarksPayload,
    pub goal_tracking: PerformanceGoalTrackingPayload,
    pub snapshot_evidence: PerformanceSnapshotEvidencePayload,
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
