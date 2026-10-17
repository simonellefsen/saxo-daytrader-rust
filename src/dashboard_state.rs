//! The dashboard builder on `AppState`: `dashboard_view`, which assembles
//! every tab's typed `DashboardView`, and the projections only it uses.
//!
//! Read-only. Each tab loads only its own data, every read degrades to a
//! named unavailable state rather than an empty one, and nothing here can
//! change what is traded.

use crate::state::{
    AppState, DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS, MISSED_TRADE_SHADOW_LIMIT,
    TUNING_PULSE_WINDOW_DAYS, dashboard_ai_settings_from_json, dashboard_embedded_json,
    dashboard_latest_run_from_json, dashboard_optional_f64, dashboard_optional_string,
    dashboard_required_boolish, dashboard_required_i64, dashboard_required_string,
    performance_range_limit, redacted_database_url, tuning_benchmark_comparison_unavailable,
    tuning_experiment_governance_unavailable, tuning_monthly_goal_progress_unavailable,
    tuning_portfolio_outcome_unavailable, tuning_trade_thesis_evidence_from_payload,
};
use crate::{
    auth,
    config::{yaml_i64, yaml_string},
    db::{json_f64, json_i64},
    debug_redaction::{DEBUG_PAYLOAD_MAX_CHARS, compact_debug_text, compact_json_redacted},
    decision_state::{
        dashboard_decision_gate_replay_from_json, dashboard_decision_gate_replay_not_loaded,
        dashboard_decision_pulse_statuses_from_json, dashboard_latest_decision_from_json,
        dashboard_selected_decision_from_json as dashboard_selected_decision_projection_from_json,
        decision_report_summaries_from_json,
    },
    execution_state::{
        dashboard_execution_events_from_json, dashboard_execution_fills_from_json,
        dashboard_protective_stop_coverage_from_json,
        dashboard_protective_stop_coverage_not_loaded, execution_order_window,
    },
    hermes_state::{
        LEARNING_MEMORY_LIMIT, LESSONS_PENDING_REVIEW_LIMIT,
        hermes_proposal_quality_from_experiments,
    },
    localization::LocalizationPrefs,
    market_state::{dashboard_watchlists_from_json, dashboard_watchlists_not_loaded},
    markov_state::{
        MARKOV_SIGNALS_PAGE_SIZE, dashboard_markov_signals_from_json, markov_signal_page,
    },
    models::{
        DashboardActiveStrategyBaselinePayload, DashboardDecisionPulseDirectionalOutcomePayload,
        DashboardDecisionPulseEvidencePayload, DashboardDecisionPulseOutcomePayload,
        DashboardDecisionPulseOutcomeRowPayload, DashboardExecutionOrderPayload,
        DashboardHermesBaselineEvidencePackPayload, DashboardHermesBaselineEvidenceWindowPayload,
        DashboardHermesCounterfactualPayload, DashboardHermesDecisionAdviceAuditPayload,
        DashboardHermesExperimentPayload, DashboardHermesLearningMemoryPayload,
        DashboardHermesLessonPendingReviewPayload, DashboardHermesOneVariableAuditPayload,
        DashboardHermesProposalQualityPayload, DashboardHermesReflectionPayload,
        DashboardHoldingThesisReviewPayload, DashboardHoldingThesisReviewsPayload,
        DashboardLatestRunPayload, DashboardMissedTradeShadowEvidencePayload,
        DashboardMissedTradeShadowGatePayload, DashboardMissedTradeShadowOutcomePayload,
        DashboardMissedTradeShadowPayload, DashboardRunSchedulePayload,
        DashboardRunSchedulesPayload, DashboardSaxoAuthPayload,
        DashboardTradeThesisEvidencePayload, DashboardTradeThesisOutcomePayload, DashboardView,
        LatestDecisionStatusPayload, LlmUsageLedgerPayload, MarketCalendarRefreshPayload,
        MarketStatusPayload, MarketStatusSummaryPayload, MarketWatchlistsPayload,
        OverviewIntegrityIssuePayload, OverviewIntegrityPayload, QuiverConflictPayload,
        TradingManagerPayload, TuningPayload, TuningProtectiveStopCoverage,
    },
    overview_state::{
        dashboard_integrity_from_json, dashboard_market_status_from_json,
        dashboard_trading_manager_from_json,
    },
    performance_state::{
        dashboard_performance_benchmarks_from_json,
        dashboard_performance_exposure_attribution_from_json,
        dashboard_performance_goal_tracking_from_json, dashboard_performance_history_from_json,
        dashboard_performance_pnl_reconciliation_from_json,
        dashboard_performance_realised_sell_outcomes_from_json,
        dashboard_performance_snapshot_evidence_from_json, dashboard_performance_summary_from_json,
        performance_summary_from_history,
    },
    portfolio_state::dashboard_positions_from_json,
    quiver_state::{
        QUIVER_SIGNALS_PAGE_SIZE, dashboard_quiver_signals_from_json, quiver_signal_page,
    },
    read_model,
    scheduler_state::{
        SCHEDULER_CYCLES_PAGE_SIZE, scheduler_cycle_page, scheduler_cycle_summaries_from_json,
    },
    strategy_journal_state::dashboard_strategy_journal_entries_from_json,
};
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;
use tracing::{error, warn};

const GATE_REPLAY_DEFAULT_RUN_LIMIT: i64 = 40;

pub(crate) fn dashboard_performance_history_limit(
    active_view: &str,
    range_key: &str,
) -> Option<i64> {
    (active_view == "performance").then(|| performance_range_limit(range_key))
}

/// Decodes the compact reflection fields used by the advisory-only Hermes
/// dashboard section. Provider payloads and detailed findings/actions remain
/// stored locally; only their aggregate counts cross the SSR boundary.
pub(crate) fn dashboard_hermes_reflections_from_json(
    reflections: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesReflectionPayload>> {
    reflections
        .into_iter()
        .map(|reflection| {
            Ok(DashboardHermesReflectionPayload {
                created_at: dashboard_required_string(&reflection, "created_at")?,
                goal_version: dashboard_required_i64(&reflection, "goal_version")?,
                summary: dashboard_required_string(&reflection, "summary")?,
                finding_count: dashboard_json_item_count(&reflection, "findings_json"),
                proposed_action_count: dashboard_json_item_count(
                    &reflection,
                    "proposed_actions_json",
                ),
                source_session_id: dashboard_optional_string(&reflection, "source_session_id")?,
            })
        })
        .collect()
}

/// Decodes the already-redacted, derived Hermes lesson queue for the
/// advisory-only dashboard section. It carries no raw reflection payload or
/// detailed proposed-action document across the SSR boundary.
pub(crate) fn dashboard_hermes_lessons_pending_review_from_json(
    lessons: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesLessonPendingReviewPayload>> {
    lessons
        .into_iter()
        .map(|lesson| {
            Ok(DashboardHermesLessonPendingReviewPayload {
                created_at: dashboard_optional_string(&lesson, "created_at")?,
                period_start: dashboard_optional_string(&lesson, "period_start")?,
                period_end: dashboard_optional_string(&lesson, "period_end")?,
                lesson: dashboard_required_string(&lesson, "lesson")?,
                reflection_summary: dashboard_optional_string(&lesson, "reflection_summary")?,
                source_session_id: dashboard_optional_string(&lesson, "source_session_id")?,
            })
        })
        .collect()
}

/// Decodes the compressed Hermes learning-memory evidence displayed by the
/// advisory-only dashboard. Raw reflections and the memory's internal safety
/// marker remain outside the SSR contract.
pub(crate) fn dashboard_hermes_learning_memory_from_json(
    memory: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesLearningMemoryPayload>> {
    memory
        .into_iter()
        .map(|lesson| {
            let status = dashboard_required_string(&lesson, "status")?;
            if !matches!(status.as_str(), "stable" | "emerging" | "stale") {
                return Err(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unknown Hermes learning-memory status",
                )));
            }
            Ok(DashboardHermesLearningMemoryPayload {
                status,
                lesson: dashboard_required_string(&lesson, "lesson")?,
                observation_count: serde_json::from_value(
                    lesson
                        .get("observation_count")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                first_seen: dashboard_required_string(&lesson, "first_seen")?,
                last_seen: dashboard_required_string(&lesson, "last_seen")?,
                expires_at: dashboard_required_string(&lesson, "expires_at")?,
                cadences: serde_json::from_value(
                    lesson.get("cadences").cloned().unwrap_or(JsonValue::Null),
                )?,
            })
        })
        .collect()
}

/// Decodes the bounded display evidence for the advisory Hermes one-variable
/// audit. Source snapshots and candidate JSON remain outside the SSR boundary;
/// values are rendered here only as capped display strings.
pub(crate) fn dashboard_hermes_one_variable_audit_from_json(
    audit_rows: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesOneVariableAuditPayload>> {
    audit_rows
        .into_iter()
        .map(|row| {
            Ok(DashboardHermesOneVariableAuditPayload {
                kind: dashboard_required_string(&row, "kind")?,
                created_at: dashboard_optional_string(&row, "created_at")?,
                status: dashboard_required_string(&row, "status")?,
                variable: dashboard_optional_string(&row, "variable")?,
                baseline_value: dashboard_short_json(row.get("baseline_value")),
                candidate_value: dashboard_short_json(row.get("candidate_value")),
                reason: dashboard_optional_string(&row, "reason")?,
                scope: dashboard_optional_string(&row, "scope")?,
                last_manager_state: dashboard_optional_string(&row, "last_manager_state")?,
            })
        })
        .collect()
}

/// Decodes only the bounded experiment-proposal fields rendered in the Hermes
/// dashboard section. Persisted approval, metrics, detailed evidence, source
/// session, and provider payloads remain outside SSR; JSON display values are
/// redacted and capped before they become strings.
pub(crate) fn dashboard_hermes_experiments_from_json(
    experiments: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesExperimentPayload>> {
    experiments
        .into_iter()
        .map(|experiment| {
            Ok(DashboardHermesExperimentPayload {
                id: dashboard_required_string(&experiment, "id")?,
                created_at: dashboard_required_string(&experiment, "created_at")?,
                status: dashboard_required_string(&experiment, "status")?,
                changed_variable_path: dashboard_required_string(
                    &experiment,
                    "changed_variable_path",
                )?,
                old_value_display: compact_json_redacted(experiment.get("old_value_json"), 220),
                new_value_display: compact_json_redacted(experiment.get("new_value_json"), 220),
                hypothesis: dashboard_required_string(&experiment, "hypothesis")?,
                expected_effect: dashboard_required_string(&experiment, "expected_effect")?,
                evidence_display: compact_json_redacted(experiment.get("evidence_json"), 220),
            })
        })
        .collect()
}

/// Decodes the active baseline's limited display record. Prompt/source data
/// never reaches SSR, and configuration is redacted before being capped for
/// read-only operator inspection.
pub(crate) fn dashboard_active_strategy_baseline_from_json(
    baseline: JsonValue,
) -> serde_json::Result<Option<DashboardActiveStrategyBaselinePayload>> {
    if baseline.is_null() {
        return Ok(None);
    }
    let config = dashboard_embedded_json(&baseline, "config_json").unwrap_or(JsonValue::Null);
    Ok(Some(DashboardActiveStrategyBaselinePayload {
        id: dashboard_required_string(&baseline, "id")?,
        goal_version: dashboard_required_i64(&baseline, "goal_version")?,
        activated_at: dashboard_required_string(&baseline, "activated_at")?,
        config_display: compact_json_redacted(Some(&config), DEBUG_PAYLOAD_MAX_CHARS),
    }))
}

/// Decodes the local, non-causal baseline evidence pack into precisely the
/// fields the dashboard renders. Raw baseline, experiment, manager, and broker
/// documents are intentionally not represented in the typed SSR payload.
pub(crate) fn dashboard_hermes_baseline_evidence_pack_from_json(
    pack: JsonValue,
) -> serde_json::Result<DashboardHermesBaselineEvidencePackPayload> {
    let baseline = pack.get("baseline").cloned().unwrap_or(JsonValue::Null);
    let activity = pack
        .get("affected_activity")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let experiment = pack.get("experiment").cloned().unwrap_or(JsonValue::Null);
    let evaluation = experiment
        .get("evaluation_window")
        .cloned()
        .unwrap_or(JsonValue::Null);
    let post_promotion = pack
        .get("post_promotion")
        .cloned()
        .unwrap_or(JsonValue::Null);
    Ok(DashboardHermesBaselineEvidencePackPayload {
        status: dashboard_required_string(&pack, "status")?,
        baseline_id: dashboard_optional_string(&baseline, "id")?,
        baseline_variable: dashboard_optional_string(&baseline, "variable")?,
        baseline_activated_at: dashboard_optional_string(&baseline, "activated_at")?,
        manager_run_count: dashboard_count(&activity, "manager_run_count"),
        report_count: dashboard_count(&activity, "report_count"),
        failed_order_count: dashboard_count(&activity, "failed_order_count"),
        experiment_evaluation: dashboard_hermes_baseline_evidence_window_from_json(evaluation),
        post_promotion: dashboard_hermes_baseline_evidence_window_from_json(post_promotion),
    })
}

fn dashboard_hermes_baseline_evidence_window_from_json(
    window: JsonValue,
) -> DashboardHermesBaselineEvidenceWindowPayload {
    DashboardHermesBaselineEvidenceWindowPayload {
        observation_count: dashboard_count(&window, "observation_count"),
        start_total_market_value_dkk: dashboard_optional_f64(
            &window,
            "start_total_market_value_dkk",
        )
        .ok()
        .flatten(),
        end_total_market_value_dkk: dashboard_optional_f64(&window, "end_total_market_value_dkk")
            .ok()
            .flatten(),
        start_cash_utilization_pct: dashboard_optional_f64(&window, "start_cash_utilization_pct")
            .ok()
            .flatten(),
        end_cash_utilization_pct: dashboard_optional_f64(&window, "end_cash_utilization_pct")
            .ok()
            .flatten(),
        return_pct: dashboard_optional_f64(&window, "return_pct").ok().flatten(),
        max_drawdown_pct: dashboard_optional_f64(&window, "max_drawdown_pct")
            .ok()
            .flatten(),
        sharpe_zero_rf_annualized: dashboard_optional_f64(&window, "sharpe_zero_rf_annualized")
            .ok()
            .flatten(),
    }
}

/// Decodes stable execution-row fields while retaining the existing bounded
/// lifecycle and attribution diagnostic documents for the detailed table.
pub(crate) fn dashboard_execution_orders_from_json(
    orders: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardExecutionOrderPayload>> {
    orders
        .into_iter()
        .map(|order| {
            let number =
                |key| dashboard_optional_f64(&order, key).map(|value| value.unwrap_or(0.0));
            Ok(DashboardExecutionOrderPayload {
                id: dashboard_required_i64(&order, "id")?,
                created_at: dashboard_required_string(&order, "created_at")?,
                symbol: dashboard_required_string(&order, "symbol")?,
                action: dashboard_required_string(&order, "action")?,
                order_type: dashboard_optional_string(&order, "order_type")?.unwrap_or_default(),
                status: dashboard_required_string(&order, "status")?,
                quantity: number("quantity")?,
                price_local: number("price_local")?,
                limit_price_local: number("limit_price_local")?,
                stop_price_local: number("stop_price_local")?,
                currency: dashboard_optional_string(&order, "currency")?.unwrap_or_default(),
                strategy_type: dashboard_optional_string(&order, "strategy_type")?
                    .unwrap_or_default(),
                strategy_role: dashboard_optional_string(&order, "strategy_role")?
                    .unwrap_or_default(),
                error_text: dashboard_optional_string(&order, "error_text")?
                    .map(|value| compact_debug_text(&value, 220))
                    .unwrap_or_default(),
                order_duration_type: dashboard_optional_string(&order, "order_duration_type")?
                    .unwrap_or_default(),
                expected_expiry_at_utc: dashboard_optional_string(
                    &order,
                    "expected_expiry_at_utc",
                )?
                .unwrap_or_default(),
                lifecycle_state: dashboard_optional_string(&order, "lifecycle_state")?
                    .unwrap_or_default(),
                execution_result_json: dashboard_embedded_json(&order, "execution_result_json")
                    .unwrap_or(JsonValue::Null),
                attribution: order.get("attribution").cloned().unwrap_or(JsonValue::Null),
            })
        })
        .collect()
}

/// Decodes the deterministic, display-only quality rubric for active Hermes
/// proposals. The underlying experiment documents and their evidence stay
/// outside the SSR boundary.
pub(crate) fn dashboard_hermes_proposal_quality_from_json(
    quality_rows: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesProposalQualityPayload>> {
    quality_rows
        .into_iter()
        .map(|row| {
            Ok(DashboardHermesProposalQualityPayload {
                created_at: dashboard_optional_string(&row, "created_at")?,
                experiment_status: dashboard_required_string(&row, "experiment_status")?,
                variable: dashboard_required_string(&row, "variable")?,
                quality_score: dashboard_required_i64(&row, "quality_score")?,
                quality_status: dashboard_required_string(&row, "quality_status")?,
                evidence_present: dashboard_required_boolish(&row, "evidence_present")?,
                evidence_has_named_sources: dashboard_required_boolish(
                    &row,
                    "evidence_has_named_sources",
                )?,
                measurable_effect: dashboard_required_boolish(&row, "measurable_effect")?,
                values_changed: dashboard_required_boolish(&row, "values_changed")?,
                risk_notes_present: dashboard_required_boolish(&row, "risk_notes_present")?,
                exact_duplicate_count: serde_json::from_value(
                    row.get("exact_duplicate_count")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                related_family_count: serde_json::from_value(
                    row.get("related_family_count")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                gaps: serde_json::from_value(row.get("gaps").cloned().unwrap_or(JsonValue::Null))?,
            })
        })
        .collect()
}

/// Decodes derived, advisory-only Hermes decision-audit evidence for the
/// dashboard. Raw advice/provider payloads and Trading Manager JSON are used
/// only while constructing this bounded projection and never cross the SSR
/// boundary.
pub(crate) fn dashboard_hermes_decision_advice_audit_from_json(
    audit_rows: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesDecisionAdviceAuditPayload>> {
    audit_rows
        .into_iter()
        .map(|row| {
            let advice_raw_payload =
                dashboard_embedded_json(&row, "advice_raw_payload_json").unwrap_or(JsonValue::Null);
            let manager_json =
                dashboard_embedded_json(&row, "manager_json").unwrap_or(JsonValue::Null);
            let manager_advice = manager_json
                .get("hermes_decision_advice")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let self_check = advice_raw_payload
                .get("context_self_check")
                .cloned()
                .or_else(|| manager_advice.get("context_self_check").cloned())
                .unwrap_or(JsonValue::Null);
            let advice_delta = manager_json
                .get("hermes_advice_delta")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let order_advice =
                dashboard_embedded_json(&row, "order_advice_json").unwrap_or(JsonValue::Null);
            let order_advice = order_advice.as_array().cloned().unwrap_or_default();
            let advice_action_count = |action: &str| {
                order_advice
                    .iter()
                    .filter(|item| {
                        item.get("action")
                            .and_then(JsonValue::as_str)
                            .is_some_and(|value| value.trim().eq_ignore_ascii_case(action))
                    })
                    .count()
            };
            let advice_status = dashboard_optional_string(&row, "advice_status")?
                .or_else(|| {
                    manager_advice
                        .get("status")
                        .and_then(JsonValue::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_else(|| "not_seen".to_string());
            let advice_mode = manager_advice
                .get("mode")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            let self_check_missing = self_check
                .get("missing")
                .and_then(JsonValue::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(JsonValue::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok(DashboardHermesDecisionAdviceAuditPayload {
                report_id: dashboard_required_i64(&row, "report_id")?,
                report_created_at: dashboard_required_string(&row, "report_created_at")?,
                analysis_pulse_label: dashboard_optional_string(&row, "analysis_pulse_label")?
                    .unwrap_or_else(|| "n/a".to_string()),
                advice_id: dashboard_optional_string(&row, "advice_id")?,
                advice_created_at: dashboard_optional_string(&row, "advice_created_at")?,
                advice_status,
                advice_source_session_id: dashboard_optional_string(
                    &row,
                    "advice_source_session_id",
                )?,
                advice_recommendation: dashboard_optional_string(&row, "advice_recommendation")?
                    .unwrap_or_else(|| "n/a".to_string()),
                advice_summary: dashboard_optional_string(&row, "advice_summary")?
                    .unwrap_or_else(|| "No advice summary recorded.".to_string()),
                advice_order_count: order_advice.len(),
                advice_allow_count: advice_action_count("allow"),
                advice_reduce_count: advice_action_count("reduce"),
                advice_stand_down_count: advice_action_count("stand_down"),
                advice_review_count: advice_action_count("review"),
                self_check_present: self_check.is_object(),
                self_check_complete: self_check
                    .get("complete")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                self_check_missing,
                advice_mode,
                impact_context_gate_blocked_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "context_gate_blocked",
                ),
                impact_blocked_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "blocked_by_order_advice",
                ) + dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "blocked_by_global_stand_down",
                ) + dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "blocked_by_reduce_below_one_share",
                ),
                impact_review_required_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "review_required_by_global_advice",
                ),
                impact_reduced_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "reduced",
                ),
                impact_allowed_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "allowed",
                ),
                impact_record_only_no_op_count: dashboard_nested_count(
                    &advice_delta,
                    "effect_counts",
                    "record_only_no_op",
                ),
                manager_status: dashboard_optional_string(&row, "manager_status")?
                    .unwrap_or_else(|| "not run".to_string()),
                queued_order_count: dashboard_count(&row, "queued_order_count"),
                executed_order_count: dashboard_count(&row, "executed_order_count"),
                failed_order_count: dashboard_count(&row, "failed_order_count"),
            })
        })
        .collect()
}

fn dashboard_count(row: &JsonValue, key: &str) -> usize {
    row.get(key)
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| {
                    value
                        .as_i64()
                        .filter(|value| *value >= 0)
                        .map(|value| value as u64)
                })
                .or_else(|| value.as_str()?.parse::<u64>().ok())
        })
        .unwrap_or_default() as usize
}

fn dashboard_nested_count(row: &JsonValue, parent: &str, key: &str) -> usize {
    row.get(parent)
        .map(|nested| dashboard_count(nested, key))
        .unwrap_or_default()
}

/// Decodes read-only Hermes counterfactual observations for dashboard display.
/// The underlying manager/advice documents and any broker mutation paths stay
/// outside this SSR boundary.
pub(crate) fn dashboard_hermes_counterfactuals_from_json(
    counterfactuals: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardHermesCounterfactualPayload>> {
    counterfactuals
        .into_iter()
        .map(|row| {
            Ok(DashboardHermesCounterfactualPayload {
                report_id: dashboard_required_i64(&row, "report_id")?,
                created_at: dashboard_required_string(&row, "created_at")?,
                symbol: dashboard_required_string(&row, "symbol")?,
                action: dashboard_required_string(&row, "action")?,
                source_effect: dashboard_required_string(&row, "source_effect")?,
                shadow_quantity: serde_json::from_value(
                    row.get("shadow_quantity")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                reference_price_local: dashboard_optional_f64(&row, "reference_price_local")?,
                reference_price_at: dashboard_optional_string(&row, "reference_price_at")?,
                reference_price_source: dashboard_optional_string(&row, "reference_price_source")?,
                reported_reference_price_local: dashboard_optional_f64(
                    &row,
                    "reported_reference_price_local",
                )?,
                currency: dashboard_optional_string(&row, "currency")?,
                status: dashboard_required_string(&row, "status")?,
                latest_price_local: dashboard_optional_f64(&row, "latest_price_local")?,
                latest_price_at: dashboard_optional_string(&row, "latest_price_at")?,
                estimated_return_pct: dashboard_optional_f64(&row, "estimated_return_pct")?,
                estimated_pnl_local: dashboard_optional_f64(&row, "estimated_pnl_local")?,
            })
        })
        .collect()
}

/// Decodes the read-only missed-trade shadow observations displayed beside
/// Hermes counterfactuals. The manager-gate audit and broker mutation paths
/// remain outside the SSR boundary.
pub(crate) fn dashboard_missed_trade_shadows_from_json(
    shadows: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardMissedTradeShadowPayload>> {
    shadows
        .into_iter()
        .map(|row| {
            Ok(DashboardMissedTradeShadowPayload {
                report_id: dashboard_required_i64(&row, "report_id")?,
                created_at: dashboard_required_string(&row, "created_at")?,
                symbol: dashboard_required_string(&row, "symbol")?,
                action: dashboard_required_string(&row, "action")?,
                source_gate: dashboard_required_string(&row, "source_gate")?,
                shadow_quantity: serde_json::from_value(
                    row.get("shadow_quantity")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                reference_price_local: dashboard_optional_f64(&row, "reference_price_local")?,
                reference_price_at: dashboard_optional_string(&row, "reference_price_at")?,
                reference_price_source: dashboard_optional_string(&row, "reference_price_source")?,
                reported_reference_price_local: dashboard_optional_f64(
                    &row,
                    "reported_reference_price_local",
                )?,
                currency: dashboard_optional_string(&row, "currency")?,
                status: dashboard_required_string(&row, "status")?,
                latest_price_local: dashboard_optional_f64(&row, "latest_price_local")?,
                latest_price_at: dashboard_optional_string(&row, "latest_price_at")?,
                estimated_return_pct: dashboard_optional_f64(&row, "estimated_return_pct")?,
                estimated_pnl_local: dashboard_optional_f64(&row, "estimated_pnl_local")?,
            })
        })
        .collect()
}

/// Decodes the deliberately small missed-trade shadow aggregate. Its raw rows,
/// safety marker, and scan metadata remain in the local evidence store.
pub(crate) fn dashboard_missed_trade_shadow_evidence_from_json(
    evidence: JsonValue,
) -> serde_json::Result<DashboardMissedTradeShadowEvidencePayload> {
    Ok(DashboardMissedTradeShadowEvidencePayload {
        status: dashboard_required_string(&evidence, "status")?,
        recorded_shadow_count: dashboard_required_i64(&evidence, "recorded_shadow_count")?,
        observed_shadow_count: dashboard_required_i64(&evidence, "observed_shadow_count")?,
        overall: dashboard_missed_trade_shadow_outcome_from_json(
            evidence.get("overall").cloned().unwrap_or(JsonValue::Null),
        )?,
        by_gate: evidence
            .get("by_gate")
            .and_then(JsonValue::as_array)
            .cloned()
            .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing by_gate")))?
            .into_iter()
            .map(|gate| {
                Ok(DashboardMissedTradeShadowGatePayload {
                    source_gate: dashboard_required_string(&gate, "source_gate")?,
                    recorded_shadow_count: dashboard_required_i64(&gate, "recorded_shadow_count")?,
                    outcome: dashboard_missed_trade_shadow_outcome_from_json(
                        gate.get("outcome").cloned().unwrap_or(JsonValue::Null),
                    )?,
                })
            })
            .collect::<serde_json::Result<Vec<_>>>()?,
        minimum_complete_observations: dashboard_required_i64(
            &evidence,
            "minimum_complete_observations",
        )?,
        interpretation: dashboard_required_string(&evidence, "interpretation")?,
    })
}

fn dashboard_missed_trade_shadow_outcome_from_json(
    outcome: JsonValue,
) -> serde_json::Result<DashboardMissedTradeShadowOutcomePayload> {
    Ok(DashboardMissedTradeShadowOutcomePayload {
        sample_count: dashboard_required_i64(&outcome, "sample_count")?,
        average_directional_return_pct: dashboard_optional_f64(
            &outcome,
            "average_directional_return_pct",
        )?,
        positive_return_rate: dashboard_optional_f64(&outcome, "positive_return_rate")?,
    })
}

fn dashboard_missed_trade_shadow_evidence_unavailable() -> DashboardMissedTradeShadowEvidencePayload
{
    DashboardMissedTradeShadowEvidencePayload {
        status: "unavailable".to_string(),
        interpretation: "Missed-trade shadow outcome evidence is unavailable right now."
            .to_string(),
        ..DashboardMissedTradeShadowEvidencePayload::default()
    }
}

/// Decodes the deliberately small trade-thesis aggregate. Scan metadata and
/// raw fill, close, and thesis records remain in their local evidence stores.
pub(crate) fn dashboard_trade_thesis_evidence_from_json(
    evidence: JsonValue,
) -> serde_json::Result<DashboardTradeThesisEvidencePayload> {
    Ok(DashboardTradeThesisEvidencePayload {
        status: dashboard_required_string(&evidence, "status")?,
        recorded_thesis_count: dashboard_required_i64(&evidence, "recorded_thesis_count")?,
        filled_thesis_count: dashboard_required_i64(&evidence, "filled_thesis_count")?,
        one_session: dashboard_trade_thesis_outcome_from_json(
            evidence
                .get("one_session")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
        five_session: dashboard_trade_thesis_outcome_from_json(
            evidence
                .get("five_session")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
        minimum_complete_observations: dashboard_required_i64(
            &evidence,
            "minimum_complete_observations",
        )?,
        interpretation: dashboard_required_string(&evidence, "interpretation")?,
    })
}

fn dashboard_trade_thesis_outcome_from_json(
    outcome: JsonValue,
) -> serde_json::Result<DashboardTradeThesisOutcomePayload> {
    Ok(DashboardTradeThesisOutcomePayload {
        sample_count: dashboard_required_i64(&outcome, "sample_count")?,
        average_directional_return_pct: dashboard_optional_f64(
            &outcome,
            "average_directional_return_pct",
        )?,
        positive_return_rate: dashboard_optional_f64(&outcome, "positive_return_rate")?,
    })
}

fn dashboard_trade_thesis_evidence_unavailable() -> DashboardTradeThesisEvidencePayload {
    DashboardTradeThesisEvidencePayload {
        status: "unavailable".to_string(),
        interpretation: "Trade-thesis outcome evidence is unavailable right now.".to_string(),
        ..DashboardTradeThesisEvidencePayload::default()
    }
}

/// Decodes the display-only holding-thesis review queue. Raw position
/// snapshots, execution/fill records, thesis documents, and safety metadata
/// remain in their local stores.
pub(crate) fn dashboard_holding_thesis_reviews_from_json(
    reviews: JsonValue,
) -> serde_json::Result<DashboardHoldingThesisReviewsPayload> {
    Ok(DashboardHoldingThesisReviewsPayload {
        status: dashboard_required_string(&reviews, "status")?,
        held_position_count: dashboard_required_i64(&reviews, "held_position_count")?,
        review_count: dashboard_required_i64(&reviews, "review_count")?,
        decision_stale_after_days: dashboard_required_i64(&reviews, "decision_stale_after_days")?,
        reviews: reviews
            .get("reviews")
            .and_then(JsonValue::as_array)
            .cloned()
            .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing reviews")))?
            .into_iter()
            .map(|review| {
                Ok(DashboardHoldingThesisReviewPayload {
                    symbol: dashboard_required_string(&review, "symbol")?,
                    instrument_name: dashboard_optional_string(&review, "instrument_name")?,
                    status: dashboard_required_string(&review, "status")?,
                    tracked_entry_at: dashboard_required_string(&review, "tracked_entry_at")?,
                    age_days: dashboard_required_i64(&review, "age_days")?,
                    intended_holding_window: dashboard_required_string(
                        &review,
                        "intended_holding_window",
                    )?,
                    entry_rationale: dashboard_required_string(&review, "entry_rationale")?,
                    invalidation: dashboard_required_string(&review, "invalidation")?,
                })
            })
            .collect::<serde_json::Result<Vec<_>>>()?,
        interpretation: dashboard_required_string(&reviews, "interpretation")?,
    })
}

fn dashboard_holding_thesis_reviews_unavailable() -> DashboardHoldingThesisReviewsPayload {
    DashboardHoldingThesisReviewsPayload {
        status: "unavailable".to_string(),
        interpretation: "Holding-thesis review data is unavailable right now.".to_string(),
        ..DashboardHoldingThesisReviewsPayload::default()
    }
}

/// Decodes the bounded execution-attributed decision-pulse aggregates. Manager
/// documents, broker payloads, raw fills, and raw ledger rows remain in their
/// local audit paths.
pub(crate) fn dashboard_decision_pulse_evidence_from_json(
    evidence: JsonValue,
) -> serde_json::Result<DashboardDecisionPulseEvidencePayload> {
    Ok(DashboardDecisionPulseEvidencePayload {
        status: dashboard_required_string(&evidence, "status")?,
        overall: dashboard_decision_pulse_outcome_from_json(
            evidence.get("overall").cloned().unwrap_or(JsonValue::Null),
        )?,
        pulses: evidence
            .get("pulses")
            .and_then(JsonValue::as_array)
            .cloned()
            .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing pulses")))?
            .into_iter()
            .map(|pulse| {
                Ok(DashboardDecisionPulseOutcomeRowPayload {
                    pulse_label: dashboard_required_string(&pulse, "pulse_label")?,
                    outcome: dashboard_decision_pulse_outcome_from_json(
                        pulse.get("outcome").cloned().unwrap_or(JsonValue::Null),
                    )?,
                })
            })
            .collect::<serde_json::Result<Vec<_>>>()?,
        minimum_complete_observations: dashboard_required_i64(
            &evidence,
            "minimum_complete_observations",
        )?,
        interpretation: dashboard_required_string(&evidence, "interpretation")?,
    })
}

fn dashboard_decision_pulse_outcome_from_json(
    outcome: JsonValue,
) -> serde_json::Result<DashboardDecisionPulseOutcomePayload> {
    let realised_sell = outcome
        .get("realised_sell")
        .cloned()
        .unwrap_or(JsonValue::Null);
    Ok(DashboardDecisionPulseOutcomePayload {
        attributed_order_count: dashboard_required_i64(&outcome, "attributed_order_count")?,
        hermes_reviewed_order_count: dashboard_required_i64(
            &outcome,
            "hermes_reviewed_order_count",
        )?,
        one_session: dashboard_decision_pulse_directional_outcome_from_json(
            outcome
                .get("one_session")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
        five_session: dashboard_decision_pulse_directional_outcome_from_json(
            outcome
                .get("five_session")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
        reconciled_sell_order_count: dashboard_required_i64(
            &outcome,
            "reconciled_sell_order_count",
        )?,
        realised_sell_gain_dkk: dashboard_required_f64(&realised_sell, "realised_gain_dkk")?,
        execution_status_counts: serde_json::from_value(
            outcome
                .get("execution_status_counts")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
        hermes_effect_counts: serde_json::from_value(
            outcome
                .get("hermes_effect_counts")
                .cloned()
                .unwrap_or(JsonValue::Null),
        )?,
    })
}

fn dashboard_decision_pulse_directional_outcome_from_json(
    outcome: JsonValue,
) -> serde_json::Result<DashboardDecisionPulseDirectionalOutcomePayload> {
    Ok(DashboardDecisionPulseDirectionalOutcomePayload {
        sample_count: dashboard_required_i64(&outcome, "sample_count")?,
        average_directional_return_pct: dashboard_optional_f64(
            &outcome,
            "average_directional_return_pct",
        )?,
        positive_return_rate: dashboard_optional_f64(&outcome, "positive_return_rate")?,
    })
}

fn dashboard_decision_pulse_evidence_unavailable() -> DashboardDecisionPulseEvidencePayload {
    DashboardDecisionPulseEvidencePayload {
        status: "unavailable".to_string(),
        interpretation: "Decision-pulse outcome evidence is unavailable right now.".to_string(),
        ..DashboardDecisionPulseEvidencePayload::default()
    }
}

fn dashboard_required_f64(row: &JsonValue, key: &str) -> serde_json::Result<f64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn dashboard_short_json(value: Option<&JsonValue>) -> String {
    let Some(value) = value else {
        return "n/a".to_string();
    };
    let rendered = match value {
        JsonValue::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| value.to_string()),
    };
    const MAX_CHARS: usize = 220;
    if rendered.len() > MAX_CHARS {
        format!("{}...", &rendered[..MAX_CHARS])
    } else {
        rendered
    }
}

fn dashboard_json_item_count(row: &JsonValue, key: &str) -> usize {
    match row.get(key) {
        Some(JsonValue::Array(items)) => items.len(),
        Some(JsonValue::Object(map)) => map.len(),
        Some(JsonValue::String(text)) if !text.trim().is_empty() => 1,
        Some(value) if !value.is_null() => 1,
        _ => 0,
    }
}

/// Decodes the dashboard's deliberately sanitized Saxo session-status contract.
/// The broker auth module keeps the full status internally; this outer view omits
/// path and credential-adjacent fields before UI rendering.
pub(crate) fn dashboard_saxo_auth_from_json(
    auth: JsonValue,
) -> serde_json::Result<DashboardSaxoAuthPayload> {
    read_model::decode("dashboard_saxo_auth", auth)
}

pub(crate) fn dashboard_saxo_auth_unavailable() -> DashboardSaxoAuthPayload {
    DashboardSaxoAuthPayload {
        status: "unavailable".to_string(),
        status_text: "No Saxo session status is available.".to_string(),
        ..DashboardSaxoAuthPayload::default()
    }
}

fn dashboard_run_schedule_from_json(
    schedule: JsonValue,
) -> serde_json::Result<DashboardRunSchedulePayload> {
    let mut schedule: DashboardRunSchedulePayload =
        read_model::decode("dashboard_run_schedule", schedule)?;
    schedule.available = true;
    Ok(schedule)
}

/// Decodes only the timing metadata required by dashboard schedule status.
/// Full collector configuration intentionally remains staged JSON.
pub(crate) fn dashboard_run_schedules_from_json(
    schedules: JsonValue,
) -> serde_json::Result<DashboardRunSchedulesPayload> {
    let mut schedules: BTreeMap<String, JsonValue> = serde_json::from_value(schedules)?;
    Ok(DashboardRunSchedulesPayload {
        markov: dashboard_run_schedule_from_json(
            schedules.remove("markov").unwrap_or(JsonValue::Null),
        )?,
        quiver: dashboard_run_schedule_from_json(
            schedules.remove("quiver").unwrap_or(JsonValue::Null),
        )?,
        indicators: dashboard_run_schedule_from_json(
            schedules.remove("indicators").unwrap_or(JsonValue::Null),
        )?,
        performance_benchmarks: schedules
            .remove("performance_benchmarks")
            .unwrap_or(JsonValue::Null),
    })
}

/// Decodes held-position Quiver conflict evidence for the Quiver dashboard.
/// Collector/provider detail remains in the Quiver context and is never used
/// as a gate or broker instruction through this read-only SSR boundary.
pub(crate) fn dashboard_quiver_conflicts_from_json(
    conflicts: JsonValue,
) -> serde_json::Result<QuiverConflictPayload> {
    read_model::decode("dashboard_quiver_conflicts", conflicts)
}

pub(crate) fn dashboard_quiver_conflicts_not_loaded() -> QuiverConflictPayload {
    QuiverConflictPayload {
        status: "not_loaded".to_string(),
        safety: "not_loaded_outside_quiver_tab_no_gate_hermes_or_broker_mutation".to_string(),
        interpretation: "Quiver held-position conflict evidence was not loaded for this view."
            .to_string(),
        ..QuiverConflictPayload::default()
    }
}

fn dashboard_quiver_conflicts_unavailable() -> QuiverConflictPayload {
    QuiverConflictPayload {
        status: "unavailable".to_string(),
        safety: "advisory_unavailable_no_gate_hermes_or_broker_mutation".to_string(),
        interpretation:
            "Quiver held-position conflict evidence could not be decoded for this view.".to_string(),
        ..QuiverConflictPayload::default()
    }
}

pub(crate) fn dashboard_loads_tab_exclusive_data(active_view: &str, tab: &str) -> bool {
    active_view == tab
}

/// Whether a Hermes sub-section's data should be loaded for this request.
///
/// The Hermes tab ran eleven separate queries on every load regardless of what
/// the operator was looking at. Gating each one to the section that renders it
/// extends the existing per-tab lazy-read policy a level deeper, so opening
/// Hermes costs only the section in front of you rather than all of it.
pub(crate) fn dashboard_loads_hermes_section(
    active_view: &str,
    section: &str,
    wanted: &str,
) -> bool {
    active_view == "hermes" && section == wanted
}

impl AppState {
    pub async fn dashboard_view(
        &self,
        localization: LocalizationPrefs,
        sso_session: auth::SsoSession,
        active_view: String,
        performance_range: String,
        selected_report_id: Option<i64>,
        requested_execution_page: i64,
        requested_markov_page: i64,
        markov_filter: String,
        hermes_section: String,
        requested_quiver_page: i64,
        requested_scheduler_page: i64,
    ) -> DashboardView {
        let overview = self.overview_payload().await.unwrap_or_else(|err| {
            error!("overview load failed: {err:#}");
            json!({})
        });
        let positions = self
            .position_items(25)
            .await
            .and_then(|positions| dashboard_positions_from_json(positions).map_err(Into::into))
            .unwrap_or_else(|err| {
                warn!("dashboard typed positions degraded: {err:#}");
                Vec::new()
            });
        let execution_order_total = if active_view == "execution" {
            self.execution_orders_count().await.unwrap_or_else(|err| {
                warn!("dashboard execution-order count degraded: {err:#}");
                0
            })
        } else {
            0
        };
        let execution_order_window = execution_order_window(
            &active_view,
            requested_execution_page,
            execution_order_total,
        );
        let orders = self
            .execution_orders_page(
                execution_order_window.page_size,
                execution_order_window.offset,
            )
            .await
            .and_then(|orders| dashboard_execution_orders_from_json(orders).map_err(Into::into))
            .unwrap_or_else(|err| {
                warn!("dashboard typed execution queue degraded: {err:#}");
                Vec::new()
            });
        let execution_fills = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.execution_fills(50)
                .await
                .and_then(|fills| dashboard_execution_fills_from_json(fills).map_err(Into::into))
                .unwrap_or_else(|err| {
                    warn!("dashboard typed execution fills degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let execution_events = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.execution_events(50)
                .await
                .and_then(|events| dashboard_execution_events_from_json(events).map_err(Into::into))
                .unwrap_or_else(|err| {
                    warn!("dashboard typed execution events degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let execution_trade_thesis_evidence =
            if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
                self.trade_thesis_outcome_evidence()
                    .await
                    .and_then(|evidence| {
                        dashboard_trade_thesis_evidence_from_json(evidence).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed trade-thesis evidence degraded: {err:#}");
                        dashboard_trade_thesis_evidence_unavailable()
                    })
            } else {
                DashboardTradeThesisEvidencePayload::default()
            };
        let execution_holding_thesis_reviews =
            if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
                self.holding_thesis_reviews()
                    .await
                    .and_then(|reviews| {
                        dashboard_holding_thesis_reviews_from_json(reviews).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed holding-thesis reviews degraded: {err:#}");
                        dashboard_holding_thesis_reviews_unavailable()
                    })
            } else {
                DashboardHoldingThesisReviewsPayload::default()
            };
        let execution_decision_pulse_evidence =
            if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
                self.decision_pulse_outcome_evidence()
                    .await
                    .and_then(|evidence| {
                        dashboard_decision_pulse_evidence_from_json(evidence).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed decision-pulse evidence degraded: {err:#}");
                        dashboard_decision_pulse_evidence_unavailable()
                    })
            } else {
                DashboardDecisionPulseEvidencePayload::default()
            };
        let execution_protection = if dashboard_loads_tab_exclusive_data(&active_view, "execution")
        {
            let coverage = self.protective_stop_coverage().await.unwrap_or_else(|err| {
                warn!("dashboard protective-stop coverage degraded: {err:#}");
                json!({
                    "status": "unavailable",
                    "summary": {},
                    "positions": [],
                    "exceptions": [],
                    "recent_prechecks": [],
                    "recent_lifecycle_tests": [],
                    "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
                    "interpretation": "Protective-stop coverage could not be loaded. No Saxo order was placed, replaced, or cancelled.",
                })
            });
            dashboard_protective_stop_coverage_from_json(coverage).unwrap_or_else(|err| {
                warn!("dashboard typed protective-stop coverage degraded: {err:#}");
                let mut unavailable = dashboard_protective_stop_coverage_not_loaded();
                unavailable.status = "unavailable".to_string();
                unavailable.safety = "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation".to_string();
                unavailable.interpretation = "Protective-stop coverage could not be loaded. No Saxo order was placed, replaced, or cancelled.".to_string();
                unavailable
            })
        } else {
            dashboard_protective_stop_coverage_not_loaded()
        };
        let report_limit = match active_view.as_str() {
            "overview" => 5,
            "decisions" => 20,
            _ => 1,
        };
        let mut reports = self
            .decision_report_summaries(report_limit)
            .await
            .unwrap_or_else(|err| {
                warn!("dashboard decision reports degraded: {err:#}");
                Vec::new()
            });
        let needs_selected_report = matches!(active_view.as_str(), "decisions" | "prompts");
        let needs_report_detail = active_view == "decisions";
        let selected_decision = if needs_selected_report {
            let report_id = selected_report_id.or_else(|| {
                reports
                    .first()
                    .and_then(|row| row.get("id").and_then(JsonValue::as_i64))
            });
            match report_id {
                Some(report_id) => {
                    if !reports
                        .iter()
                        .any(|row| row.get("id").and_then(JsonValue::as_i64) == Some(report_id))
                    {
                        if let Some(summary) =
                            self.decision_report_summary(report_id).await.ok().flatten()
                        {
                            reports.insert(0, summary);
                        }
                    }
                    if needs_report_detail {
                        self.decision_report_item(report_id)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or(JsonValue::Null)
                    } else {
                        reports
                            .iter()
                            .find(|row| {
                                row.get("id").and_then(JsonValue::as_i64) == Some(report_id)
                            })
                            .cloned()
                            .unwrap_or(JsonValue::Null)
                    }
                }
                None => JsonValue::Null,
            }
        } else {
            JsonValue::Null
        };
        let selected_decision = if active_view == "decisions" {
            self.attach_decision_candidate_waterfall(selected_decision)
                .await
        } else {
            selected_decision
        };
        let decision_gate_replay = if dashboard_loads_tab_exclusive_data(&active_view, "decisions")
        {
            let replay = self.decision_gate_replay(GATE_REPLAY_DEFAULT_RUN_LIMIT)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard gate replay degraded: {err:#}");
                    json!({
                        "status": "unavailable",
                        "run_count": 0,
                        "scenarios": [],
                        "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
                        "interpretation": "Decision Gate Replay evidence could not be loaded.",
                        "support_risk_evidence": null,
                    })
            });
            dashboard_decision_gate_replay_from_json(replay).unwrap_or_else(|err| {
                warn!("dashboard typed gate replay degraded: {err:#}");
                let mut unavailable = dashboard_decision_gate_replay_not_loaded();
                unavailable.status = "unavailable".to_string();
                unavailable.safety =
                    "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation"
                        .to_string();
                unavailable.interpretation =
                    "Decision Gate Replay evidence could not be loaded.".to_string();
                unavailable
            })
        } else {
            dashboard_decision_gate_replay_not_loaded()
        };
        // The Operations banner is visible on every dashboard tab, so it needs
        // a compact per-pulse report status rather than only the latest global
        // report. The payload deliberately excludes report prompt/response
        // bodies and remains small enough for the shared read model.
        let decision_pulse_statuses = self
            .decision_pulse_statuses()
            .await
            .and_then(|statuses| {
                dashboard_decision_pulse_statuses_from_json(statuses).map_err(Into::into)
            })
            .unwrap_or_else(|err| {
                warn!("dashboard typed decision-pulse statuses degraded: {err:#}");
                Vec::new()
            });
        let journal_entries = if dashboard_loads_tab_exclusive_data(&active_view, "eod") {
            self.strategy_journal_items(20)
                .await
                .and_then(|entries| {
                    dashboard_strategy_journal_entries_from_json(entries).map_err(Into::into)
                })
                .unwrap_or_else(|err| {
                    warn!("dashboard typed end-of-day journal degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let scheduler_cycle_total = if dashboard_loads_tab_exclusive_data(&active_view, "execution")
        {
            self.scheduler_cycles_count().await.unwrap_or_else(|err| {
                warn!("dashboard scheduler cycle count degraded: {err:#}");
                0
            })
        } else {
            0
        };
        let scheduler_cycle_page =
            scheduler_cycle_page(requested_scheduler_page, scheduler_cycle_total);
        let scheduler_cycles = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.scheduler_cycles_page(SCHEDULER_CYCLES_PAGE_SIZE, scheduler_cycle_page.offset)
                .await
                .and_then(|cycles| scheduler_cycle_summaries_from_json(cycles).map_err(Into::into))
                .unwrap_or_else(|err| {
                    warn!("dashboard typed scheduler cycles degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let hermes_reflections =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "reflections") {
                self.hermes_reflections(20)
                    .await
                    .and_then(|reflections| {
                        dashboard_hermes_reflections_from_json(reflections).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes reflections degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_lessons_pending_review =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "reflections") {
                self.hermes_lessons_pending_review(LESSONS_PENDING_REVIEW_LIMIT as i64)
                    .await
                    .and_then(|lessons| {
                        dashboard_hermes_lessons_pending_review_from_json(lessons)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes lessons pending review degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_learning_memory =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "reflections") {
                self.hermes_learning_memory(LEARNING_MEMORY_LIMIT as i64)
                    .await
                    .and_then(|memory| {
                        dashboard_hermes_learning_memory_from_json(memory).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes learning memory degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_experiments =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "experiments") {
                self.hermes_experiments(20)
                    .await
                    .and_then(|experiments| {
                        dashboard_hermes_experiments_from_json(experiments).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes experiments degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_proposal_quality =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "overview") {
                self.hermes_experiments(20)
                    .await
                    .map(|experiments| hermes_proposal_quality_from_experiments(&experiments))
                    .and_then(|quality_rows| {
                        dashboard_hermes_proposal_quality_from_json(quality_rows)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes proposal quality degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_decision_advice_audit =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "advice") {
                self.hermes_decision_advice_audit(20)
                    .await
                    .and_then(|audit_rows| {
                        dashboard_hermes_decision_advice_audit_from_json(audit_rows)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes decision advice audit degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_counterfactuals =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "advice") {
                self.hermes_counterfactuals(30)
                    .await
                    .and_then(|counterfactuals| {
                        dashboard_hermes_counterfactuals_from_json(counterfactuals)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes counterfactuals degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let missed_trade_shadows =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "advice") {
                self.missed_trade_shadows(MISSED_TRADE_SHADOW_LIMIT)
                    .await
                    .and_then(|shadows| {
                        dashboard_missed_trade_shadows_from_json(shadows).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed missed-trade shadows degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let missed_trade_shadow_evidence =
            if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
                self.missed_trade_shadow_outcome_evidence()
                    .await
                    .and_then(|evidence| {
                        dashboard_missed_trade_shadow_evidence_from_json(evidence)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed missed-trade shadow evidence degraded: {err:#}");
                        dashboard_missed_trade_shadow_evidence_unavailable()
                    })
            } else {
                DashboardMissedTradeShadowEvidencePayload::default()
            };
        let active_strategy_baseline_raw =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "baselines") {
                self.active_strategy_baseline().await.unwrap_or_else(|err| {
                    warn!("dashboard active strategy baseline degraded: {err:#}");
                    JsonValue::Null
                })
            } else {
                JsonValue::Null
            };
        let active_strategy_baseline =
            dashboard_active_strategy_baseline_from_json(active_strategy_baseline_raw.clone())
                .unwrap_or_else(|err| {
                    warn!("dashboard typed active strategy baseline degraded: {err:#}");
                    None
                });
        let hermes_baseline_evidence_pack =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "baselines") {
                self.hermes_baseline_evidence_pack(&active_strategy_baseline_raw)
                    .await
                    .and_then(|pack| {
                        dashboard_hermes_baseline_evidence_pack_from_json(pack).map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes baseline evidence pack degraded: {err:#}");
                        DashboardHermesBaselineEvidencePackPayload {
                            status: "unavailable".to_string(),
                            ..Default::default()
                        }
                    })
            } else {
                DashboardHermesBaselineEvidencePackPayload::default()
            };
        let hermes_one_variable_audit =
            if dashboard_loads_hermes_section(&active_view, &hermes_section, "overview") {
                self.hermes_one_variable_audit()
                    .await
                    .and_then(|audit_rows| {
                        dashboard_hermes_one_variable_audit_from_json(audit_rows)
                            .map_err(Into::into)
                    })
                    .unwrap_or_else(|err| {
                        warn!("dashboard typed Hermes one-variable audit degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        // The count and the page must use the same filter, or the pagination
        // controls advertise pages that render empty.
        let markov_min_signed_signal =
            crate::trading_manager::markov_gate_config(self).min_signed_signal;
        let data_freshness = self.data_freshness().await;
        let markov_signal_total = if dashboard_loads_tab_exclusive_data(&active_view, "markov") {
            self.markov_signals_count_filtered(&markov_filter, markov_min_signed_signal)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard Markov signal count degraded: {err:#}");
                    0
                })
        } else {
            0
        };
        let markov_signal_page = markov_signal_page(requested_markov_page, markov_signal_total);
        let markov_signals = if dashboard_loads_tab_exclusive_data(&active_view, "markov") {
            self.markov_signals_page_filtered(
                MARKOV_SIGNALS_PAGE_SIZE,
                markov_signal_page.offset,
                &markov_filter,
                markov_min_signed_signal,
            )
            .await
            .and_then(|signals| dashboard_markov_signals_from_json(signals).map_err(Into::into))
            .unwrap_or_else(|err| {
                warn!("dashboard typed Markov signals degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let latest_markov_run = self.latest_markov_run().await.unwrap_or_else(|err| {
            warn!("dashboard latest Markov run degraded: {err:#}");
            JsonValue::Null
        });
        let latest_markov_run =
            dashboard_latest_run_from_json(latest_markov_run).unwrap_or_else(|err| {
                warn!("dashboard typed latest Markov run degraded: {err:#}");
                DashboardLatestRunPayload::default()
            });
        let quiver_signal_total = if dashboard_loads_tab_exclusive_data(&active_view, "quiver") {
            self.quiver_signals_count().await.unwrap_or_else(|err| {
                warn!("dashboard Quiver signal count degraded: {err:#}");
                0
            })
        } else {
            0
        };
        let quiver_signal_page = quiver_signal_page(requested_quiver_page, quiver_signal_total);
        let quiver_signals = if dashboard_loads_tab_exclusive_data(&active_view, "quiver") {
            self.quiver_signals_page(QUIVER_SIGNALS_PAGE_SIZE, quiver_signal_page.offset)
                .await
                .and_then(|signals| dashboard_quiver_signals_from_json(signals).map_err(Into::into))
                .unwrap_or_else(|err| {
                    warn!("dashboard typed Quiver signals degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let latest_quiver_run = self.latest_quiver_run().await.unwrap_or_else(|err| {
            warn!("dashboard latest Quiver run degraded: {err:#}");
            JsonValue::Null
        });
        let latest_quiver_run =
            dashboard_latest_run_from_json(latest_quiver_run).unwrap_or_else(|err| {
                warn!("dashboard typed latest Quiver run degraded: {err:#}");
                DashboardLatestRunPayload::default()
            });
        let quiver_conflicts = if dashboard_loads_tab_exclusive_data(&active_view, "quiver") {
            let held_positions = self.position_items(250).await.unwrap_or_else(|err| {
                warn!("dashboard Quiver conflict holdings degraded: {err:#}");
                Vec::new()
            });
            let context = crate::quiver::compact_quiver_context(self, 250)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard Quiver conflict context degraded: {err:#}");
                    json!({"signals": []})
                });
            crate::quiver::held_position_conflicts(&held_positions, &context)
        } else {
            JsonValue::Null
        };
        let quiver_conflicts = if quiver_conflicts.is_null() {
            dashboard_quiver_conflicts_not_loaded()
        } else {
            dashboard_quiver_conflicts_from_json(quiver_conflicts).unwrap_or_else(|err| {
                warn!("dashboard typed Quiver conflicts degraded: {err:#}");
                dashboard_quiver_conflicts_unavailable()
            })
        };
        let latest_daily_indicator_run =
            self.latest_daily_indicator_run()
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard latest daily indicator run degraded: {err:#}");
                    JsonValue::Null
                });
        let latest_daily_indicator_run = dashboard_latest_run_from_json(latest_daily_indicator_run)
            .unwrap_or_else(|err| {
                warn!("dashboard typed latest daily indicator run degraded: {err:#}");
                DashboardLatestRunPayload::default()
            });
        let performance_history =
            match dashboard_performance_history_limit(&active_view, &performance_range) {
                Some(limit) => self
                    .performance_history_with_current(&performance_range, limit)
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard performance history degraded: {err:#}");
                        Vec::new()
                    }),
                None => Vec::new(),
            };
        let performance_summary = if active_view == "performance" {
            performance_summary_from_history(&performance_history, Utc::now())
        } else {
            JsonValue::Null
        };
        let performance_benchmarks = if active_view == "performance" {
            crate::performance_benchmarks::performance_benchmark_payload(self, &performance_history)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard performance benchmark comparison degraded: {err:#}");
                    json!({"status": "unavailable", "references": []})
                })
        } else {
            JsonValue::Null
        };
        let performance_goal_tracking = if active_view == "performance" {
            overview
                .get("goal_tracking")
                .cloned()
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let performance_snapshot_evidence = if active_view == "performance" {
            self.portfolio_position_snapshot_coverage(&performance_range)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard position-snapshot coverage degraded: {err:#}");
                    json!({
                        "status": "unavailable",
                        "safety": "local_snapshot_evidence_read_no_provider_hermes_gate_or_order_authority",
                    })
                })
        } else {
            JsonValue::Null
        };
        let performance_pnl_reconciliation = if active_view == "performance" {
            overview
                .get("integrity")
                .and_then(|integrity| integrity.get("pnl_reconciliation"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let performance_exposure_attribution = if active_view == "performance" {
            overview
                .get("integrity")
                .and_then(|integrity| integrity.get("unrealised_pnl_attribution"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let performance_realised_sell_outcomes = if active_view == "performance" {
            overview
                .get("integrity")
                .and_then(|integrity| integrity.get("realised_sell_outcomes"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let performance_history = dashboard_performance_history_from_json(performance_history)
            .unwrap_or_else(|err| {
                warn!("dashboard typed performance history degraded: {err:#}");
                Vec::new()
            });
        let performance_summary = dashboard_performance_summary_from_json(performance_summary)
            .unwrap_or_else(|err| {
                warn!("dashboard typed performance summary degraded: {err:#}");
                None
            });
        let performance_benchmarks = dashboard_performance_benchmarks_from_json(
            performance_benchmarks,
        )
        .unwrap_or_else(|err| {
            warn!("dashboard typed performance benchmarks degraded: {err:#}");
            None
        });
        let performance_goal_tracking =
            dashboard_performance_goal_tracking_from_json(performance_goal_tracking)
                .unwrap_or_else(|err| {
                    warn!("dashboard typed performance goal tracking degraded: {err:#}");
                    None
                });
        let performance_snapshot_evidence =
            dashboard_performance_snapshot_evidence_from_json(performance_snapshot_evidence)
                .unwrap_or_else(|err| {
                    warn!("dashboard typed performance snapshot evidence degraded: {err:#}");
                    None
                });
        let performance_pnl_reconciliation =
            dashboard_performance_pnl_reconciliation_from_json(performance_pnl_reconciliation)
                .unwrap_or_else(|err| {
                    warn!("dashboard typed performance P/L reconciliation degraded: {err:#}");
                    None
                });
        let performance_exposure_attribution =
            dashboard_performance_exposure_attribution_from_json(performance_exposure_attribution)
                .unwrap_or_else(|err| {
                    warn!("dashboard typed performance exposure attribution degraded: {err:#}");
                    None
                });
        let performance_realised_sell_outcomes =
            dashboard_performance_realised_sell_outcomes_from_json(
                performance_realised_sell_outcomes,
            )
            .unwrap_or_else(|err| {
                warn!("dashboard typed performance realised SELL outcomes degraded: {err:#}");
                None
            });
        let market_status = self.market_status_payload().await.unwrap_or_else(|err| {
            warn!("dashboard market status degraded: {err:#}");
            json!({
                "items": [],
                "summary": {
                    "analysis_window_active": false,
                    "active_markets": [],
                    "active_windows": [],
                    "open_active_markets": [],
                    "close_active_markets": [],
                    "pre_sync_markets": [],
                    "last_cycle_status": null,
                    "last_heartbeat_at": null,
                    "next_pulse_at": null,
                    "next_pulse_label": null,
                    "price_monitor_status": null,
                    "price_monitor_updated_at": null,
                    "calendar_refresh": {"status": "unavailable"}
                },
                "scheduler": null,
                "price_monitor": null
            })
        });
        let market_status =
            dashboard_market_status_from_json(market_status).unwrap_or_else(|err| {
                warn!("dashboard typed market status degraded: {err:#}");
                MarketStatusPayload {
                    items: Vec::new(),
                    summary: MarketStatusSummaryPayload {
                        analysis_window_active: false,
                        active_markets: Vec::new(),
                        active_windows: Vec::new(),
                        open_active_markets: Vec::new(),
                        close_active_markets: Vec::new(),
                        pre_sync_markets: Vec::new(),
                        last_cycle_status: None,
                        last_heartbeat_at: None,
                        next_pulse_at: None,
                        next_pulse_label: None,
                        price_monitor_status: None,
                        price_monitor_updated_at: None,
                        calendar_refresh: MarketCalendarRefreshPayload {
                            status: "unavailable".to_string(),
                            source: None,
                            checked_at: None,
                            exchange_count: None,
                        },
                    },
                    scheduler: None,
                    price_monitor: None,
                }
            });
        let integrity = overview
            .get("integrity")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let integrity = dashboard_integrity_from_json(integrity).unwrap_or_else(|err| {
            warn!("dashboard typed integrity degraded: {err:#}");
            OverviewIntegrityPayload {
                healthy: false,
                warnings: vec![OverviewIntegrityIssuePayload {
                    code: "integrity_dashboard_payload_unavailable".to_string(),
                    severity: "warning".to_string(),
                    message: "Integrity status could not be loaded for the dashboard.".to_string(),
                    ..Default::default()
                }],
                mismatches: Vec::new(),
                expiry_pending_orders: Vec::new(),
                acknowledged_issue_count: 0,
                checked_at: String::new(),
            }
        });
        let trading_manager = overview
            .get("trading_manager")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let trading_manager =
            dashboard_trading_manager_from_json(trading_manager).unwrap_or_else(|err| {
                warn!("dashboard typed Trading Manager degraded: {err:#}");
                TradingManagerPayload {
                    status: "unavailable".to_string(),
                    latest_run: None,
                }
            });
        let watchlists = if dashboard_loads_tab_exclusive_data(&active_view, "watchlists") {
            let watchlists = self.watchlists_payload().await.unwrap_or_else(|err| {
                warn!("dashboard watchlists degraded: {err:#}");
                json!({
                    "generated_at": Utc::now().to_rfc3339(),
                    "cache_ttl_seconds": 300,
                    "universe": {},
                    "categories": []
                })
            });
            dashboard_watchlists_from_json(watchlists).unwrap_or_else(|err| {
                warn!("dashboard typed watchlists degraded: {err:#}");
                MarketWatchlistsPayload {
                    generated_at: Utc::now().to_rfc3339(),
                    ..dashboard_watchlists_not_loaded()
                }
            })
        } else {
            dashboard_watchlists_not_loaded()
        };
        let tuning = if dashboard_loads_tab_exclusive_data(&active_view, "tuning") {
            self.tuning_payload().await.unwrap_or_else(|err| {
                warn!("dashboard tuning evidence degraded: {err:#}");
                TuningPayload {
                    generated_at: Utc::now().to_rfc3339(),
                    window_start: String::new(),
                    window_days: TUNING_PULSE_WINDOW_DAYS,
                    status: "unavailable".to_string(),
                    pulse_comparison: Vec::new(),
                    shadow_change_evidence: Vec::new(),
                    shadow_support_risk_evidence: Vec::new(),
                    shadow_markov_evidence: Vec::new(),
                    shadow_quiver_evidence: Vec::new(),
                    shadow_gate_evidence: Vec::new(),
                    shadow_hermes_evidence: Vec::new(),
                    execution_pulse_outcomes: Vec::new(),
                    execution_lifecycle_evidence: Vec::new(),
                    protective_stop_coverage: TuningProtectiveStopCoverage {
                        status: "unavailable".to_string(),
                        position_count: 0,
                        protected_count: 0,
                        partial_count: 0,
                        planned_count: 0,
                        unprotected_count: 0,
                        exception_count: 0,
                        confirmed_coverage_ratio: None,
                    },
                    execution_candidate_funnel: Vec::new(),
                    trade_thesis_evidence: tuning_trade_thesis_evidence_from_payload(&json!({
                        "status": "unavailable"
                    })),
                    experiment_governance: tuning_experiment_governance_unavailable(),
                    portfolio_outcome: tuning_portfolio_outcome_unavailable(),
                    monthly_goal_progress: tuning_monthly_goal_progress_unavailable(),
                    benchmark_comparison: tuning_benchmark_comparison_unavailable(),
                    safety: "read_only_local_decision_and_shadow_outcome_evidence_no_provider_hermes_broker_or_order_mutation".to_string(),
                    interpretation: "Tuning evidence could not be loaded. It does not affect report scheduling, gates, Hermes, configuration, or Saxo orders.".to_string(),
                }
            })
        } else {
            TuningPayload {
                generated_at: String::new(),
                window_start: String::new(),
                window_days: TUNING_PULSE_WINDOW_DAYS,
                status: "not_loaded".to_string(),
                pulse_comparison: Vec::new(),
                shadow_change_evidence: Vec::new(),
                shadow_support_risk_evidence: Vec::new(),
                shadow_markov_evidence: Vec::new(),
                shadow_quiver_evidence: Vec::new(),
                shadow_gate_evidence: Vec::new(),
                shadow_hermes_evidence: Vec::new(),
                execution_pulse_outcomes: Vec::new(),
                execution_lifecycle_evidence: Vec::new(),
                protective_stop_coverage: TuningProtectiveStopCoverage {
                    status: "not_loaded".to_string(),
                    position_count: 0,
                    protected_count: 0,
                    partial_count: 0,
                    planned_count: 0,
                    unprotected_count: 0,
                    exception_count: 0,
                    confirmed_coverage_ratio: None,
                },
                execution_candidate_funnel: Vec::new(),
                trade_thesis_evidence: tuning_trade_thesis_evidence_from_payload(&json!({
                    "status": "not_loaded"
                })),
                experiment_governance: tuning_experiment_governance_unavailable(),
                portfolio_outcome: tuning_portfolio_outcome_unavailable(),
                monthly_goal_progress: tuning_monthly_goal_progress_unavailable(),
                benchmark_comparison: tuning_benchmark_comparison_unavailable(),
                safety: "not_loaded_outside_tuning_tab".to_string(),
                interpretation: String::new(),
            }
        };
        let latest_decision = if active_view == "prompts" {
            selected_decision.clone()
        } else {
            reports.first().cloned().unwrap_or(JsonValue::Null)
        };
        let latest_decision =
            dashboard_latest_decision_from_json(latest_decision).unwrap_or_else(|err| {
                warn!("dashboard typed latest decision degraded: {err:#}");
                LatestDecisionStatusPayload::default()
            });
        let selected_decision = dashboard_selected_decision_projection_from_json(selected_decision)
            .unwrap_or_else(|err| {
                warn!("dashboard typed selected Decision Report degraded: {err:#}");
                None
            });
        let reports = decision_report_summaries_from_json(reports).unwrap_or_else(|err| {
            warn!("dashboard typed Decision Report summaries degraded: {err:#}");
            Vec::new()
        });
        let summary = overview
            .get("portfolio_summary")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let after_tax_summary = overview
            .get("after_tax_summary")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let execution = overview
            .get("execution")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let saxo_auth = overview
            .get("saxo_auth")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let saxo_auth = dashboard_saxo_auth_from_json(saxo_auth).unwrap_or_else(|err| {
            warn!("dashboard typed Saxo auth status degraded: {err:#}");
            dashboard_saxo_auth_unavailable()
        });
        let ai_settings = self.ai_settings_value().await.unwrap_or_else(|err| {
            warn!("dashboard AI settings degraded: {err:#}");
            self.default_ai_settings_value()
        });
        let ai_settings = dashboard_ai_settings_from_json(ai_settings).unwrap_or_else(|err| {
            warn!("dashboard typed AI settings degraded: {err:#}");
            self.default_dashboard_ai_settings()
        });
        let ai_provider_capabilities =
            if dashboard_loads_tab_exclusive_data(&active_view, "prompts") {
                self.ai_provider_capabilities(500)
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard AI provider capability matrix degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let llm_usage = if dashboard_loads_tab_exclusive_data(&active_view, "prompts") {
            self.llm_usage_ledger(500, 30).await.unwrap_or_else(|err| {
                warn!("dashboard LLM usage ledger degraded: {err:#}");
                LlmUsageLedgerPayload::default()
            })
        } else {
            LlmUsageLedgerPayload::default()
        };
        let jev_models = if dashboard_loads_tab_exclusive_data(&active_view, "prompts") {
            self.jev_models().await
        } else {
            crate::models::JevModelsPayload::default()
        };
        let operator_actions = if dashboard_loads_tab_exclusive_data(&active_view, "overview") {
            crate::operator_actions::recent(&self.pool, 20).await
        } else {
            crate::models::OperatorActionsPayload::default()
        };
        let jev_operator = if dashboard_loads_tab_exclusive_data(&active_view, "prompts") {
            self.jev_operator().await
        } else {
            crate::models::JevOperatorPayload::default()
        };
        let run_schedules = dashboard_run_schedules_from_json(json!({
            "markov": crate::markov_method::markov_config_json_for_state(self),
            "quiver": crate::quiver::quiver_config_json_for_state(self),
            "indicators": crate::daily_indicators::indicator_config_json_for_state(self),
            "performance_benchmarks": crate::performance_benchmarks::benchmark_config_json_for_state(self),
        }))
        .unwrap_or_else(|err| {
            warn!("dashboard typed run schedules degraded: {err:#}");
            DashboardRunSchedulesPayload::default()
        });

        DashboardView {
            app_name: yaml_string(&self.config, &["app", "project_name"])
                .unwrap_or_else(|| "saxo-rust".to_string()),
            environment: yaml_string(&self.config, &["app", "environment"])
                .unwrap_or_else(|| "local".to_string()),
            db_label: redacted_database_url(&self.db_url),
            total_value_dkk: json_f64(&summary, "total_market_value_dkk"),
            invested_value_dkk: json_f64(&summary, "invested_market_value_dkk"),
            cash_dkk: json_f64(&summary, "cash_balance_dkk"),
            initial_cash_dkk: json_f64(&summary, "initial_cash_dkk"),
            cash_from_trades_dkk: json_f64(&summary, "cash_from_trades_dkk"),
            unrealised_pnl_dkk: json_f64(&summary, "total_unrealised_pnl_dkk"),
            unrealised_after_tax_dkk: json_f64(&after_tax_summary, "unrealised_pnl_after_tax_dkk"),
            estimated_unrealised_tax_dkk: json_f64(&after_tax_summary, "estimated_tax_dkk"),
            after_tax_estimate_status: after_tax_summary
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or("unavailable")
                .to_string(),
            daily_pnl_dkk: json_f64(&summary, "total_daily_pnl_dkk"),
            position_count: json_i64(&summary, "position_count"),
            position_decision_stale_after_days: yaml_i64(
                &self.config,
                &["strategy", "swing", "position_decision_stale_after_days"],
            )
            .unwrap_or(DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS)
            .max(1),
            execution_mode: execution
                .get("mode")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
                .to_string(),
            execution_adapter: execution
                .get("adapter")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
                .to_string(),
            saxo_status: if saxo_auth.status_text.is_empty() {
                if saxo_auth.status.is_empty() {
                    "not connected".to_string()
                } else {
                    saxo_auth.status.clone()
                }
            } else {
                saxo_auth.status_text.clone()
            },
            saxo_auth,
            sso_session,
            ai_settings,
            ai_provider_capabilities,
            llm_usage,
            jev_models,
            jev_operator,
            operator_actions,
            localization,
            active_view,
            performance_range,
            selected_report_id,
            execution_page: execution_order_window.page,
            execution_page_size: execution_order_window.page_size,
            execution_order_total,
            markov_page: markov_signal_page.page,
            markov_page_size: MARKOV_SIGNALS_PAGE_SIZE,
            markov_signal_total,
            markov_filter,
            hermes_section,
            data_freshness,
            quiver_page: quiver_signal_page.page,
            quiver_page_size: QUIVER_SIGNALS_PAGE_SIZE,
            quiver_signal_total,
            scheduler_page: scheduler_cycle_page.page,
            scheduler_page_size: SCHEDULER_CYCLES_PAGE_SIZE,
            scheduler_cycle_total,
            positions,
            orders,
            execution_fills,
            execution_events,
            execution_trade_thesis_evidence,
            execution_holding_thesis_reviews,
            execution_decision_pulse_evidence,
            reports,
            manual_report_in_flight: self.manual_decision_report_in_flight().await,
            decision_pulse_statuses,
            journal_entries,
            scheduler_cycles,
            hermes_reflections,
            hermes_lessons_pending_review,
            hermes_learning_memory,
            hermes_one_variable_audit,
            hermes_proposal_quality,
            hermes_experiments,
            hermes_decision_advice_audit,
            hermes_counterfactuals,
            missed_trade_shadows,
            missed_trade_shadow_evidence,
            active_strategy_baseline,
            hermes_baseline_evidence_pack,
            markov_signals,
            latest_markov_run,
            quiver_signals,
            latest_quiver_run,
            quiver_conflicts,
            latest_daily_indicator_run,
            run_schedules,
            performance_history,
            performance_summary,
            performance_benchmarks,
            performance_goal_tracking,
            performance_snapshot_evidence,
            performance_pnl_reconciliation,
            performance_exposure_attribution,
            performance_realised_sell_outcomes,
            integrity,
            execution_protection,
            market_status,
            trading_manager,
            watchlists,
            latest_decision,
            selected_decision,
            decision_gate_replay,
            tuning,
        }
    }
}
