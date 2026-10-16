//! Read-only Hermes dashboard and evidence projections.
//!
//! This module intentionally contains only deterministic transformations of
//! persisted reflection, experiment, manager, order, and portfolio records. It
//! cannot create experiments, alter runtime configuration, invoke a provider,
//! or reach Saxo.

use crate::models::{
    HermesDecisionCandidatePayload, HermesExperimentSummaryPayload, HermesReflectionSummaryPayload,
};
use crate::state::{
    AppState, ContextDetail, HERMES_CONTEXT_ALWAYS_PRESENT, HERMES_CONTEXT_SECTIONS,
    SUPPORTED_EXPERIMENT_VARIABLES, canonical_symbol_key, compact_context_node,
    compact_scheduler_cycle_document, decode_shadow_json_field,
    hermes_counterfactual_quote_metrics, json_text, shadow_report_approved_policy_source,
    shadow_report_context_strategy_key, shadow_report_outcome_report_is_eligible,
    sql_optional_text, strip_experiment_bulk_fields, strip_thesis_narrative,
};
use crate::{
    config::{yaml_bool, yaml_f64, yaml_i64},
    db::{clamp_limit, sql_escape, value_f64, value_i64},
    models::{
        HermesCapabilitiesPayload, HermesContextDecisionsPayload, HermesContextEndOfDayPayload,
        HermesContextExecutionPayload, HermesContextHermesPayload, HermesContextItemsPayload,
        HermesContextLearningMemoryPayload, HermesContextPayload, HermesContextPerformancePayload,
        HermesContextSafetyPayload, HermesContextSchedulerPayload,
        HermesContextSelfCheckCapabilitiesPayload, HermesDecisionAdviceCapabilitiesPayload,
        HermesDecisionAdviceRequest, HermesDecisionReportOutcomePayload,
        HermesExperimentOverlayCapabilitiesPayload, HermesExperimentRequest,
        HermesReflectionRequest,
    },
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use tracing::{info, warn};

pub(crate) const LESSONS_PENDING_REVIEW_REFLECTION_LIMIT: i64 = 50;
pub(crate) const LESSONS_PENDING_REVIEW_LIMIT: usize = 30;
const LESSON_TEXT_MAX_CHARS: usize = 500;
pub(crate) const LEARNING_MEMORY_REFLECTION_LIMIT: i64 = 80;
pub(crate) const LEARNING_MEMORY_LIMIT: usize = 30;
const LEARNING_MEMORY_EMERGING_TTL_DAYS: i64 = 7;
const LEARNING_MEMORY_STABLE_TTL_DAYS: i64 = 21;
const LEARNING_MEMORY_STABLE_MIN_REFLECTIONS: usize = 2;

pub(crate) const HERMES_EXPERIMENT_DUPLICATE_BLOCKING_STATUSES: &[&str] = &[
    "pending_review",
    "approved_paper",
    "active_paper",
    "approved_sim",
    "active_sim",
    "ready_for_promotion",
];

/// Lifecycle states whose experiment values have passed operator review and
/// may be shown to Hermes while it gives Trading Manager advice. A proposal in
/// `pending_review` remains visible to operators through the normal dashboard
/// and lifecycle API, but its proposed value must never become advisory input.
pub(crate) const HERMES_ADVISORY_EXPERIMENT_STATUSES: &[&str] = &[
    "approved_paper",
    "active_paper",
    "approved_sim",
    "active_sim",
    "ready_for_promotion",
];

pub(crate) fn hermes_experiment_status_is_advisory_eligible(status: &str) -> bool {
    HERMES_ADVISORY_EXPERIMENT_STATUSES.contains(&status.trim())
}

/// Projects report-to-manager-to-execution evidence into the compact Hermes
/// advisory boundary. It is intentionally local and retrospective: no current
/// quote, broker request, provider output, or future-performance inference is
/// introduced while reviewing an earlier decision.
/// Cap on candidates exposed per report. Real reports carry a handful; the
/// bound only stops a pathological row from bloating the MCP response the way
/// embedded Markov signals once bloated the advisory prompt.
const HERMES_MAX_REPORT_CANDIDATES: usize = 25;

/// Project `report_json.suggested_trades` into the advisory candidate list.
///
/// Deliberately field-by-field rather than a passthrough: `strategy_metadata`
/// and any future provider content stay out of the MCP surface, and only rows
/// that actually name a symbol are exposed, so Hermes never sees a candidate it
/// cannot key advice to.
fn hermes_decision_candidates_from_report(
    report: &JsonValue,
) -> Vec<HermesDecisionCandidatePayload> {
    let Some(trades) = report.get("suggested_trades").and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    trades
        .iter()
        .filter(|trade| {
            trade
                .get("symbol")
                .and_then(JsonValue::as_str)
                .is_some_and(|symbol| !symbol.trim().is_empty())
        })
        .take(HERMES_MAX_REPORT_CANDIDATES)
        .map(|trade| {
            let text = |key: &str| {
                trade
                    .get(key)
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            HermesDecisionCandidatePayload {
                symbol: text("symbol"),
                action: text("action"),
                quantity: trade
                    .get("quantity")
                    .and_then(JsonValue::as_f64)
                    .unwrap_or_default(),
                order_type: text("order_type"),
                strategy_key: text("strategy_key"),
                strategy_role: text("strategy_role"),
                limit_price_local: trade.get("limit_price_local").and_then(JsonValue::as_f64),
                estimated_value_dkk: trade.get("estimated_value_dkk").and_then(JsonValue::as_f64),
            }
        })
        .collect()
}

pub(crate) fn hermes_decision_report_outcomes_from_rows(
    rows: Vec<JsonValue>,
) -> Vec<HermesDecisionReportOutcomePayload> {
    rows.into_iter()
        .map(|row| {
            let report = embedded_json(&row, "report_json");
            let quality = report
                .as_ref()
                .and_then(|report| report.get("decision_quality"))
                .filter(|quality| quality.is_object());
            let quality_status = quality
                .and_then(|quality| quality.get("status"))
                .and_then(JsonValue::as_str)
                .filter(|status| matches!(*status, "ready" | "review"))
                .unwrap_or("not_recorded")
                .to_string();
            HermesDecisionReportOutcomePayload {
                report_id: value_i64(&row, "report_id"),
                created_at: json_text(&row, "created_at"),
                report_date: json_text(&row, "report_date"),
                report_status: json_text(&row, "report_status"),
                analysis_pulse_key: json_text(&row, "analysis_pulse_key"),
                analysis_pulse_label: json_text(&row, "analysis_pulse_label"),
                pulse_mode: json_text(&row, "pulse_mode"),
                queue_eligible: value_i64(&row, "queue_eligible") > 0,
                decision_quality_status: quality_status,
                decision_quality_score: quality.and_then(|quality| {
                    quality
                        .get("score")
                        .and_then(JsonValue::as_i64)
                        .filter(|score| (0..=100).contains(score))
                }),
                decision_quality_warning_count: quality.and_then(|quality| {
                    quality
                        .get("warning_count")
                        .and_then(JsonValue::as_i64)
                        .filter(|count| *count >= 0)
                }),
                candidate_count: quality.and_then(|quality| {
                    quality
                        .get("candidate_count")
                        .and_then(JsonValue::as_i64)
                        .filter(|count| *count >= 0)
                }),
                candidates: report
                    .as_ref()
                    .map(hermes_decision_candidates_from_report)
                    .unwrap_or_default(),
                manager_status: optional_text(&row, "manager_status"),
                execution_order_count: value_i64(&row, "execution_order_count"),
                pending_execution_count: value_i64(&row, "pending_execution_count"),
                broker_working_count: value_i64(&row, "broker_working_count"),
                partial_fill_count: value_i64(&row, "partial_fill_count"),
                filled_count: value_i64(&row, "filled_count"),
                expired_count: value_i64(&row, "expired_count"),
                cancelled_count: value_i64(&row, "cancelled_count"),
                failed_or_rejected_count: value_i64(&row, "failed_or_rejected_count"),
                realised_sell_count: value_i64(&row, "realised_sell_count"),
                realised_sell_gain_dkk: value_f64(&row, "realised_sell_gain_dkk"),
            }
        })
        .collect()
}

fn embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn optional_text(row: &JsonValue, key: &str) -> Option<String> {
    let value = json_text(row, key);
    (!value.is_empty()).then_some(value)
}

const HERMES_EXPERIMENT_REVIEW_FAMILIES: &[(&str, &str)] = &[
    ("strategy.capital.min_cash_buffer_pct", "cash_buffer_policy"),
    ("strategy.swing.cash_buffer_pct", "cash_buffer_policy"),
];

#[derive(Clone, Debug)]
struct LearningMemoryEntry {
    lesson: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    reflection_ids: HashSet<String>,
    cadences: HashSet<String>,
}

/// Convert reflection `proposed_actions` into a bounded, display-safe operator
/// queue. The rows are deliberately derived rather than persisted as a second
/// workflow: an item is advisory context, not an approved experiment or task.
pub(crate) fn lessons_pending_review_from_reflections(
    reflections: &[JsonValue],
    limit: usize,
) -> Vec<JsonValue> {
    let mut lessons = Vec::new();
    let mut seen = HashSet::new();

    for reflection in reflections {
        let reflection_id = reflection
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .trim();
        if reflection_id.is_empty() {
            continue;
        }
        let Some(actions) = reflection.get("proposed_actions_json") else {
            continue;
        };
        for (action_index, action) in proposed_action_entries(actions).into_iter().enumerate() {
            let Some(lesson) = proposed_action_text(action) else {
                continue;
            };
            let normalized = lesson.to_lowercase();
            if !seen.insert(normalized) {
                continue;
            }
            lessons.push(json!({
                "id": format!("{reflection_id}:{action_index}"),
                "reflection_id": reflection_id,
                "created_at": reflection.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "period_start": reflection.get("period_start").cloned().unwrap_or(JsonValue::Null),
                "period_end": reflection.get("period_end").cloned().unwrap_or(JsonValue::Null),
                "goal_version": reflection.get("goal_version").cloned().unwrap_or(JsonValue::Null),
                "lesson": lesson,
                "reflection_summary": reflection.get("summary").cloned().unwrap_or(JsonValue::Null),
                "source_session_id": reflection.get("source_session_id").cloned().unwrap_or(JsonValue::Null),
            }));
            if lessons.len() >= limit.max(1) {
                return lessons;
            }
        }
    }
    lessons
}

/// Compress repeated safe reflection actions into an expiring, read-only
/// learning-memory view. Repetition across separate reflections makes a
/// lesson stable; one-off actions remain emerging and all observations expire
/// deterministically. This is advisory context only, not an experiment or
/// strategy/configuration mutation path.
pub(crate) fn learning_memory_from_reflections(
    reflections: &[JsonValue],
    now: DateTime<Utc>,
    limit: usize,
) -> Vec<JsonValue> {
    let mut entries = HashMap::<String, LearningMemoryEntry>::new();
    for reflection in reflections {
        let reflection_id = json_text(reflection, "id");
        if reflection_id.is_empty() {
            continue;
        }
        let created_at = json_text(reflection, "created_at");
        let Some(created_at) = DateTime::parse_from_rfc3339(&created_at)
            .ok()
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        let cadence = reflection_cadence(reflection);
        let Some(actions) = reflection.get("proposed_actions_json") else {
            continue;
        };
        let mut actions_seen_in_reflection = HashSet::new();
        for action in proposed_action_entries(actions) {
            let Some(lesson) = proposed_action_text(action) else {
                continue;
            };
            let normalized = lesson.to_ascii_lowercase();
            if !actions_seen_in_reflection.insert(normalized.clone()) {
                continue;
            }
            let entry = entries
                .entry(normalized)
                .or_insert_with(|| LearningMemoryEntry {
                    lesson: lesson.clone(),
                    first_seen: created_at,
                    last_seen: created_at,
                    reflection_ids: HashSet::new(),
                    cadences: HashSet::new(),
                });
            entry.first_seen = entry.first_seen.min(created_at);
            entry.last_seen = entry.last_seen.max(created_at);
            entry.reflection_ids.insert(reflection_id.clone());
            entry.cadences.insert(cadence.clone());
        }
    }

    let mut memory = entries
        .into_iter()
        .map(|(normalized_lesson, entry)| {
            let observation_count = entry.reflection_ids.len();
            let stable = observation_count >= LEARNING_MEMORY_STABLE_MIN_REFLECTIONS;
            let ttl_days = if stable {
                LEARNING_MEMORY_STABLE_TTL_DAYS
            } else {
                LEARNING_MEMORY_EMERGING_TTL_DAYS
            };
            let expires_at = entry.last_seen + Duration::days(ttl_days);
            let status = if now >= expires_at {
                "stale"
            } else if stable {
                "stable"
            } else {
                "emerging"
            };
            let mut cadences = entry.cadences.into_iter().collect::<Vec<_>>();
            cadences.sort();
            json!({
                "id": format!("lesson-memory:{}:{normalized_lesson}", entry.first_seen.timestamp_micros()),
                "lesson": entry.lesson,
                "status": status,
                "observation_count": observation_count,
                "first_seen": entry.first_seen.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "last_seen": entry.last_seen.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "expires_at": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "cadences": cadences,
                "safety": "derived_reflection_context_not_a_trading_instruction",
            })
        })
        .collect::<Vec<_>>();
    memory.sort_by(|left, right| {
        let rank = |row: &JsonValue| match json_text(row, "status").as_str() {
            "stable" => 0,
            "emerging" => 1,
            _ => 2,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| json_text(right, "last_seen").cmp(&json_text(left, "last_seen")))
    });
    memory.truncate(limit.max(1));
    memory
}

pub(crate) fn safe_display_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }
    if lesson_text_looks_sensitive(&normalized) {
        return "[redacted potentially sensitive Hermes text]".to_string();
    }
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

/// Decodes stable reflection metadata for the protected API list. Detailed
/// advisory documents remain in the local audit store and do not cross this
/// boundary.
pub(crate) fn hermes_reflection_summaries_from_json(
    reflections: Vec<JsonValue>,
) -> serde_json::Result<Vec<HermesReflectionSummaryPayload>> {
    reflections
        .into_iter()
        .map(|reflection| {
            Ok(HermesReflectionSummaryPayload {
                id: required_string(&reflection, "id")?,
                created_at: required_string(&reflection, "created_at")?,
                period_start: required_string(&reflection, "period_start")?,
                period_end: required_string(&reflection, "period_end")?,
                goal_version: required_i64(&reflection, "goal_version")?,
                summary: required_string(&reflection, "summary")?,
                source_session_id: optional_string(&reflection, "source_session_id")?,
            })
        })
        .collect()
}

/// Decodes stable experiment metadata for the protected API list. Proposed
/// values and supporting documents remain in the local audit store and do not
/// cross this boundary.
pub(crate) fn hermes_experiment_summaries_from_json(
    experiments: Vec<JsonValue>,
) -> serde_json::Result<Vec<HermesExperimentSummaryPayload>> {
    experiments
        .into_iter()
        .map(|experiment| {
            Ok(HermesExperimentSummaryPayload {
                id: required_string(&experiment, "id")?,
                created_at: required_string(&experiment, "created_at")?,
                status: required_string(&experiment, "status")?,
                baseline_id: optional_string(&experiment, "baseline_id")?,
                goal_version: required_i64(&experiment, "goal_version")?,
                changed_variable_path: required_string(&experiment, "changed_variable_path")?,
                source_session_id: optional_string(&experiment, "source_session_id")?,
            })
        })
        .collect()
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_string(row: &JsonValue, key: &str) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_i64(row: &JsonValue, key: &str) -> serde_json::Result<i64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn reflection_cadence(reflection: &JsonValue) -> String {
    let session = json_text(reflection, "source_session_id").to_ascii_lowercase();
    if session.contains("weekly") {
        "weekly".to_string()
    } else if session.contains("daily") {
        "daily".to_string()
    } else {
        "other".to_string()
    }
}

fn proposed_action_entries(value: &JsonValue) -> Vec<&JsonValue> {
    match value {
        JsonValue::Array(entries) => entries.iter().collect(),
        JsonValue::Object(object) => object
            .get("actions")
            .and_then(JsonValue::as_array)
            .map(|entries| entries.iter().collect())
            .unwrap_or_else(|| vec![value]),
        JsonValue::String(_) => vec![value],
        _ => Vec::new(),
    }
}

fn proposed_action_text(value: &JsonValue) -> Option<String> {
    let text = match value {
        JsonValue::String(text) => Some(text.as_str()),
        JsonValue::Object(object) => [
            "action",
            "recommendation",
            "proposal",
            "summary",
            "title",
            "detail",
        ]
        .iter()
        .find_map(|field| object.get(*field).and_then(JsonValue::as_str)),
        _ => None,
    };
    let text = text?;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if lesson_text_looks_sensitive(&normalized) {
        return Some("[redacted potentially sensitive reflection action]".to_string());
    }
    Some(safe_display_text(&normalized, LESSON_TEXT_MAX_CHARS))
}

fn lesson_text_looks_sensitive(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "refresh_token",
        "access_token",
        "client_secret",
        "authorization:",
        "bearer ",
        "api_key=",
        "openrouter_api_key",
        "accountkey=",
        "clientkey=",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

/// Produce a small, display-safe view of the one-variable state. Baselines
/// remain audit records and overlays remain runtime candidates; neither row
/// asserts that a persistent config rewrite or live activation occurred.
pub(crate) fn hermes_one_variable_audit_from_snapshot(
    baseline: &JsonValue,
    overlay_audit: &JsonValue,
    latest_manager_run: &JsonValue,
) -> Vec<JsonValue> {
    let mut rows = Vec::new();
    if !baseline.is_null() {
        let config = baseline.get("config_json").unwrap_or(&JsonValue::Null);
        rows.push(json!({
            "kind": "promoted_baseline",
            "id": json_text(baseline, "id"),
            "created_at": baseline.get("activated_at").cloned().unwrap_or(JsonValue::Null),
            "status": "record_only",
            "variable": json_text(config, "changed_variable_path"),
            "baseline_value": config.get("old_value").cloned().unwrap_or(JsonValue::Null),
            "candidate_value": config.get("new_value").cloned().unwrap_or(JsonValue::Null),
            "reason": safe_display_text(&json_text(config, "hypothesis"), 220),
            "scope": "baseline audit record only; no live activation",
            "last_manager_state": "not an overlay",
        }));
    }

    let candidate = overlay_audit.get("candidate").unwrap_or(&JsonValue::Null);
    if !candidate.is_null() {
        let candidate_id = json_text(candidate, "id");
        let last_overlay = latest_manager_run
            .get("manager_json")
            .and_then(|value| value.get("strategy_experiment_overlay"))
            .unwrap_or(&JsonValue::Null);
        let observed_last_run =
            !candidate_id.is_empty() && candidate_id == json_text(last_overlay, "id");
        rows.push(json!({
            "kind": "selected_overlay",
            "id": candidate_id,
            "created_at": latest_manager_run.get("created_at").cloned().unwrap_or(JsonValue::Null),
            "status": json_text(overlay_audit, "state"),
            "experiment_status": json_text(candidate, "status"),
            "variable": json_text(candidate, "changed_variable_path"),
            "baseline_value": candidate.get("old_value").cloned().unwrap_or(JsonValue::Null),
            "candidate_value": candidate.get("new_value").cloned().unwrap_or(JsonValue::Null),
            "reason": safe_display_text(&json_text(candidate, "hypothesis"), 220),
            "scope": json_text(candidate, "scope"),
            "last_manager_state": if observed_last_run {
                "observed in latest manager run"
            } else {
                "selected for the next eligible manager cycle"
            },
            "execution_mode": json_text(overlay_audit, "execution_mode"),
            "saxo_environment": json_text(overlay_audit, "saxo_environment"),
        }));
    } else if rows.is_empty() {
        rows.push(json!({
            "kind": "none",
            "status": json_text(overlay_audit, "state"),
            "scope": "no promoted baseline record or supported SIM/paper overlay selected",
            "last_manager_state": "no one-variable difference is currently selected",
        }));
    }
    rows
}

/// Score active Hermes proposals using only their persisted, non-sensitive
/// fields. The score is advisory review context, not a lifecycle gate: an
/// operator still owns every approval, activation, rejection, and promotion.
pub(crate) fn hermes_proposal_quality_from_experiments(
    experiments: &[JsonValue],
) -> Vec<JsonValue> {
    experiments
        .iter()
        .filter(|experiment| {
            hermes_experiment_status_blocks_duplicate(&json_text(experiment, "status"))
        })
        .map(|experiment| {
            let id = json_text(experiment, "id");
            let variable = normalize_hermes_experiment_variable_path(&json_text(
                experiment,
                "changed_variable_path",
            ));
            let one_variable = !variable.is_empty()
                && variable.contains('.')
                && !variable.contains([' ', ',', ';', '\n', '\r']);
            let values_changed =
                experiment.get("old_value_json") != experiment.get("new_value_json");
            let risk_notes_present = !json_text(experiment, "risk_notes").trim().is_empty();
            let evidence = experiment.get("evidence_json").unwrap_or(&JsonValue::Null);
            let evidence_present = match evidence {
                JsonValue::Array(values) => !values.is_empty(),
                JsonValue::Object(values) => !values.is_empty(),
                JsonValue::String(value) => !value.trim().is_empty(),
                _ => false,
            };
            let evidence_has_named_sources = evidence
                .as_object()
                .map(|values| {
                    [
                        "report_id",
                        "report_ids",
                        "decision_reports",
                        "end_of_day",
                        "markov",
                        "quiver",
                        "metrics",
                        "observations",
                    ]
                    .iter()
                    .any(|key| values.contains_key(*key))
                })
                .unwrap_or(false);
            let expected_effect = json_text(experiment, "expected_effect");
            let effect_lower = expected_effect.to_ascii_lowercase();
            let measurable_effect = !effect_lower.trim().is_empty()
                && [
                    "%", "pct", "return", "drawdown", "sharpe", "p/l", "pnl", "dkk", "cash",
                    "budget", "failure", "order", "rate",
                ]
                .iter()
                .any(|marker| effect_lower.contains(marker));
            let exact_duplicates = experiments
                .iter()
                .filter(|other| {
                    json_text(other, "id") != id
                        && hermes_experiment_status_blocks_duplicate(&json_text(other, "status"))
                        && normalize_hermes_experiment_variable_path(&json_text(
                            other,
                            "changed_variable_path",
                        )) == variable
                })
                .count();
            let review_family = hermes_experiment_review_family(&variable);
            let related_family = review_family
                .map(|family| {
                    experiments
                        .iter()
                        .filter(|other| {
                            json_text(other, "id") != id
                                && hermes_experiment_status_blocks_duplicate(&json_text(
                                    other, "status",
                                ))
                                && hermes_experiment_review_family(&json_text(
                                    other,
                                    "changed_variable_path",
                                )) == Some(family)
                        })
                        .count()
                })
                .unwrap_or(0);

            let mut score = 0_i64;
            let mut strengths = Vec::new();
            let mut gaps = Vec::new();
            if one_variable {
                score += 20;
                strengths.push("one variable".to_string());
            } else {
                gaps.push("requires one unambiguous variable path".to_string());
            }
            if evidence_present {
                score += 20;
                strengths.push("evidence attached".to_string());
            } else {
                gaps.push("attach evidence".to_string());
            }
            if evidence_has_named_sources {
                score += 10;
                strengths.push("named evidence source".to_string());
            } else if evidence_present {
                gaps.push("name report, EOD, Markov, Quiver, or metrics source".to_string());
            }
            if measurable_effect {
                score += 20;
                strengths.push("measurable expected effect".to_string());
            } else {
                gaps.push("define a measurable expected effect".to_string());
            }
            if values_changed && risk_notes_present {
                score += 20;
                strengths.push("changed value and risk notes".to_string());
            } else {
                if !values_changed {
                    gaps.push("old and new values must differ".to_string());
                }
                if !risk_notes_present {
                    gaps.push("add risk notes".to_string());
                }
            }
            if exact_duplicates == 0 && related_family == 0 {
                score += 10;
                strengths.push("no active duplicate risk".to_string());
            } else if exact_duplicates > 0 {
                gaps.push("exact active/pending duplicate".to_string());
            } else {
                gaps.push("related active/pending variable family".to_string());
                score += 5;
            }
            let quality_status = if exact_duplicates > 0 {
                "duplicate_risk"
            } else if score < 80 {
                "needs_evidence"
            } else if related_family > 0 {
                "related_review"
            } else {
                "review_ready"
            };
            json!({
                "id": id,
                "created_at": experiment.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "experiment_status": json_text(experiment, "status"),
                "variable": variable,
                "quality_score": score,
                "quality_status": quality_status,
                "evidence_present": evidence_present,
                "evidence_has_named_sources": evidence_has_named_sources,
                "measurable_effect": measurable_effect,
                "one_variable": one_variable,
                "values_changed": values_changed,
                "risk_notes_present": risk_notes_present,
                "exact_duplicate_count": exact_duplicates,
                "related_family_count": related_family,
                "strengths": strengths,
                "gaps": gaps,
            })
        })
        .collect()
}

/// Derive a compact evidence pack for a promoted baseline without introducing a
/// second promotion workflow. All figures are local, persisted observations;
/// they are useful review context, not proof that an experiment caused a
/// return or a signal that it is active in live trading.
pub(crate) fn hermes_baseline_evidence_pack_from_snapshot(
    baseline: &JsonValue,
    experiment: &JsonValue,
    manager_runs: &[JsonValue],
    orders: &[JsonValue],
    portfolio_history: &[JsonValue],
) -> JsonValue {
    if baseline.is_null() {
        return json!({
            "status": "no_active_baseline",
            "safety": "read_only_observational_not_causal",
        });
    }

    let config = baseline.get("config_json").unwrap_or(&JsonValue::Null);
    let source_experiment_id = json_text(config, "source_experiment_id");
    let activated_at = json_text(baseline, "activated_at");
    let variable = json_text(config, "changed_variable_path");
    if source_experiment_id.is_empty() || experiment.is_null() {
        return json!({
            "status": "source_experiment_unavailable",
            "safety": "read_only_observational_not_causal",
            "baseline": {
                "id": json_text(baseline, "id"),
                "activated_at": activated_at,
                "variable": variable,
                "source_experiment_id": source_experiment_id,
            },
        });
    }

    let experiment_created_at = json_text(experiment, "created_at");
    let matching_runs = manager_runs
        .iter()
        .filter(|run| {
            run.get("manager_json")
                .and_then(|value| value.get("strategy_experiment_overlay"))
                .is_some_and(|overlay| json_text(overlay, "id") == source_experiment_id)
        })
        .collect::<Vec<_>>();
    let report_ids = matching_runs
        .iter()
        .map(|run| value_i64(run, "report_id"))
        .filter(|report_id| *report_id > 0)
        .collect::<HashSet<_>>();
    let approved_orders = matching_runs
        .iter()
        .map(|run| {
            run.get("manager_json")
                .map(|value| value_i64(value, "approved_order_count"))
                .unwrap_or(0)
        })
        .sum::<i64>();
    let skipped_orders = matching_runs
        .iter()
        .map(|run| {
            run.get("manager_json")
                .map(|value| value_i64(value, "skipped_order_count"))
                .unwrap_or(0)
        })
        .sum::<i64>();
    let related_orders = orders
        .iter()
        .filter(|order| report_ids.contains(&value_i64(order, "report_id")))
        .collect::<Vec<_>>();
    let mut executed_orders = 0_i64;
    let mut working_orders = 0_i64;
    let mut failed_orders = 0_i64;
    let mut other_orders = 0_i64;
    for order in &related_orders {
        let status = json_text(order, "status").to_ascii_lowercase();
        if status.contains("executed") || status.contains("filled") {
            executed_orders += 1;
        } else if status.contains("working") || status.contains("submitted") || status == "queued" {
            working_orders += 1;
        } else if status.contains("failed")
            || status.contains("error")
            || status.contains("reject")
            || status.contains("expired")
        {
            failed_orders += 1;
        } else {
            other_orders += 1;
        }
    }

    let experiment_history = portfolio_history
        .iter()
        .filter(|row| {
            let recorded_at = json_text(row, "recorded_at");
            !recorded_at.is_empty()
                && (experiment_created_at.is_empty()
                    || recorded_at.as_str() >= experiment_created_at.as_str())
                && (activated_at.is_empty() || recorded_at.as_str() <= activated_at.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let post_promotion_history = portfolio_history
        .iter()
        .filter(|row| {
            let recorded_at = json_text(row, "recorded_at");
            !activated_at.is_empty() && recorded_at.as_str() > activated_at.as_str()
        })
        .cloned()
        .collect::<Vec<_>>();
    let experiment_metrics = hermes_portfolio_evidence_metrics(&experiment_history);
    let post_promotion_metrics = hermes_portfolio_evidence_metrics(&post_promotion_history);
    let status = if post_promotion_metrics["observation_count"]
        .as_i64()
        .unwrap_or(0)
        > 0
    {
        "observing"
    } else {
        "awaiting_post_promotion_observation"
    };

    json!({
        "status": status,
        "safety": "read_only_observational_not_causal",
        "baseline": {
            "id": json_text(baseline, "id"),
            "activated_at": activated_at,
            "variable": variable,
            "source_experiment_id": source_experiment_id,
        },
        "experiment": {
            "created_at": experiment_created_at,
            "status": json_text(experiment, "status"),
            "evaluation_window": experiment_metrics,
        },
        "affected_activity": {
            "manager_run_count": matching_runs.len(),
            "report_count": report_ids.len(),
            "approved_order_count": approved_orders,
            "skipped_order_count": skipped_orders,
            "execution_order_count": related_orders.len(),
            "executed_order_count": executed_orders,
            "working_order_count": working_orders,
            "failed_order_count": failed_orders,
            "other_order_count": other_orders,
        },
        "post_promotion": post_promotion_metrics,
    })
}

pub(crate) fn normalize_hermes_experiment_variable_path(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Why a proposal does not start from the running configuration, if it does not.
///
/// An experiment moves one variable away from its running value. A proposal
/// that misstates that value, or proposes the value already running, has no
/// treatment to test. Promoting it would write a baseline record of a change
/// no evidence produced. `strategy-experiment-1789762547119303` did both: it
/// proposed 0.15 -> 0.20 for a gate that had run at 0.20 for eighteen days.
/// The refusal carries the running value, so the proposer can correct
/// itself in the same run.
pub(crate) fn hermes_experiment_starting_point_problem(
    config: &serde_yaml::Value,
    changed_variable_path: &str,
    old_value: &JsonValue,
    new_value: &JsonValue,
) -> Option<JsonValue> {
    let path = normalize_hermes_experiment_variable_path(changed_variable_path);
    let keys: Vec<&str> = path.split('.').collect();
    let running = crate::config::yaml_at(config, &keys).and_then(Setting::from_yaml);
    let Some(running) = running else {
        return Some(json!({
            "status": "unknown_variable",
            "detail": "The running configuration holds no single value at this path, so the proposal's starting point cannot be checked. Propose a supported variable, from its running value.",
            "changed_variable_path": path,
        }));
    };
    if Setting::from_json(old_value).as_ref() != Some(&running) {
        return Some(json!({
            "status": "stale_starting_point",
            "detail": "old_value is not the value this variable runs at. Propose from the running value, or record the idea in reflection proposed_actions if it no longer applies.",
            "changed_variable_path": path,
            "old_value": old_value,
            "running_value": running.to_json(),
        }));
    }
    if Setting::from_json(new_value).as_ref() == Some(&running) {
        return Some(json!({
            "status": "no_change",
            "detail": "new_value is already the running value, so there is nothing to test.",
            "changed_variable_path": path,
            "running_value": running.to_json(),
        }));
    }
    None
}

/// One configuration setting, compared as the runtime reads it: `0.2`, `0.20`
/// and `"0.20"` are the same number.
#[derive(Clone, Debug)]
enum Setting {
    Number(f64),
    Flag(bool),
    Text(String),
}

impl Setting {
    fn from_text(text: &str) -> Self {
        let text = text.trim();
        match text.parse::<f64>() {
            Ok(number) if number.is_finite() => Self::Number(number),
            _ => Self::Text(text.to_string()),
        }
    }

    fn from_yaml(value: &serde_yaml::Value) -> Option<Self> {
        match value {
            serde_yaml::Value::Number(number) => number.as_f64().map(Self::Number),
            serde_yaml::Value::Bool(flag) => Some(Self::Flag(*flag)),
            serde_yaml::Value::String(text) => Some(Self::from_text(text)),
            _ => None,
        }
    }

    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Number(number) => number.as_f64().map(Self::Number),
            JsonValue::Bool(flag) => Some(Self::Flag(*flag)),
            JsonValue::String(text) => Some(Self::from_text(text)),
            _ => None,
        }
    }

    fn to_json(&self) -> JsonValue {
        match self {
            Self::Number(number) => json!(number),
            Self::Flag(flag) => json!(flag),
            Self::Text(text) => json!(text),
        }
    }
}

impl PartialEq for Setting {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            // Only the binary representation's error is forgiven, so a real
            // difference in the last written decimal still counts.
            (Self::Number(left), Self::Number(right)) => {
                (left - right).abs() <= 1e-9 * left.abs().max(right.abs()).max(1.0)
            }
            (Self::Flag(left), Self::Flag(right)) => left == right,
            (Self::Text(left), Self::Text(right)) => left == right,
            _ => false,
        }
    }
}

pub(crate) fn hermes_experiment_review_family(value: &str) -> Option<&'static str> {
    let normalized = normalize_hermes_experiment_variable_path(value);
    HERMES_EXPERIMENT_REVIEW_FAMILIES
        .iter()
        .find_map(|(path, family)| (*path == normalized).then_some(*family))
}

pub(crate) fn hermes_experiment_status_blocks_duplicate(status: &str) -> bool {
    HERMES_EXPERIMENT_DUPLICATE_BLOCKING_STATUSES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(status.trim()))
}

fn hermes_portfolio_evidence_metrics(rows: &[JsonValue]) -> JsonValue {
    let mut snapshots = rows
        .iter()
        .filter_map(|row| {
            let recorded_at = json_text(row, "recorded_at");
            let total = value_f64(row, "total_market_value_dkk");
            (total.is_finite() && total > 0.0 && !recorded_at.is_empty()).then(|| {
                (
                    recorded_at,
                    total,
                    value_f64(row, "invested_market_value_dkk"),
                    value_f64(row, "cash_balance_dkk"),
                )
            })
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| left.0.cmp(&right.0));
    let Some((start_at, start_value, start_invested, start_cash)) = snapshots.first() else {
        return json!({
            "observation_count": 0,
            "return_pct": JsonValue::Null,
            "max_drawdown_pct": JsonValue::Null,
            "sharpe_zero_rf_annualized": JsonValue::Null,
        });
    };
    let (end_at, end_value, end_invested, end_cash) = snapshots.last().expect("non-empty");
    let return_pct = if snapshots.len() >= 2 {
        Some((end_value / start_value - 1.0) * 100.0)
    } else {
        None
    };
    let mut peak = *start_value;
    let mut max_drawdown_pct = 0.0_f64;
    for (_, value, _, _) in &snapshots {
        peak = peak.max(*value);
        max_drawdown_pct = max_drawdown_pct.min((value / peak - 1.0) * 100.0);
    }

    let mut daily_closes: Vec<(String, f64)> = Vec::new();
    for (recorded_at, value, _, _) in &snapshots {
        let day = recorded_at.chars().take(10).collect::<String>();
        if let Some((previous_day, previous_value)) = daily_closes.last_mut()
            && *previous_day == day
        {
            *previous_value = *value;
            continue;
        }
        daily_closes.push((day, *value));
    }
    let daily_returns = daily_closes
        .windows(2)
        .filter_map(|window| {
            let previous = window[0].1;
            let current = window[1].1;
            (previous > 0.0).then_some(current / previous - 1.0)
        })
        .collect::<Vec<_>>();
    let sharpe = if daily_returns.len() >= 3 {
        let mean = daily_returns.iter().sum::<f64>() / daily_returns.len() as f64;
        let variance = daily_returns
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (daily_returns.len() - 1) as f64;
        let volatility = variance.sqrt();
        (volatility > f64::EPSILON).then_some(mean / volatility * 252.0_f64.sqrt())
    } else {
        None
    };
    let utilization =
        |invested: f64, total: f64| (total > 0.0).then_some(invested.max(0.0) / total * 100.0);
    json!({
        "observation_count": snapshots.len(),
        "start_at": start_at,
        "end_at": end_at,
        "start_total_market_value_dkk": start_value,
        "end_total_market_value_dkk": end_value,
        "start_cash_balance_dkk": start_cash,
        "end_cash_balance_dkk": end_cash,
        "start_cash_utilization_pct": utilization(*start_invested, *start_value),
        "end_cash_utilization_pct": utilization(*end_invested, *end_value),
        "return_pct": return_pct,
        "max_drawdown_pct": if snapshots.len() >= 2 { json!(max_drawdown_pct) } else { JsonValue::Null },
        "sharpe_zero_rf_annualized": sharpe,
        "daily_return_observation_count": daily_returns.len(),
    })
}

const PROTECTIVE_STOP_HERMES_POSITION_LIMIT: usize = 50;

fn runtime_id(prefix: &str) -> String {
    format!("{prefix}-{}", Utc::now().timestamp_micros())
}

pub(crate) fn hermes_counterfactual_shadow_quantity(
    effect: &str,
    requested_quantity: f64,
    resulting_quantity: f64,
) -> Option<f64> {
    if !matches!(
        effect,
        "context_gate_blocked"
            | "blocked_by_order_advice"
            | "blocked_by_reduce_below_one_share"
            | "blocked_by_global_stand_down"
            | "review_required_by_global_advice"
            | "reduced"
    ) {
        return None;
    }
    let quantity = requested_quantity - resulting_quantity;
    (quantity.is_finite() && quantity > 0.0).then_some(quantity)
}

pub(crate) fn compact_protective_stop_coverage_for_hermes(
    coverage: &JsonValue,
    limit: usize,
) -> JsonValue {
    let mut compact = coverage.clone();
    let position_limit = limit.clamp(1, PROTECTIVE_STOP_HERMES_POSITION_LIMIT);
    if let Some(positions) = compact
        .get_mut("positions")
        .and_then(JsonValue::as_array_mut)
    {
        positions.truncate(position_limit);
    }
    if let Some(exceptions) = compact
        .get_mut("exceptions")
        .and_then(JsonValue::as_array_mut)
    {
        exceptions.truncate(position_limit);
    }
    compact
}

/// Returns the recognised sections and, separately, the names that matched
/// nothing -- reported rather than dropped, since a typo that silently widens
/// the payload back to everything is the failure this function invites.
fn normalize_hermes_context_sections(requested: &[String]) -> (Vec<String>, Vec<String>) {
    let mut sections: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for name in requested {
        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if HERMES_CONTEXT_SECTIONS.contains(&key.as_str()) {
            if !sections.contains(&key) {
                sections.push(key);
            }
        } else if !unknown.contains(&key) {
            unknown.push(key);
        }
    }
    sections.sort();
    unknown.sort();
    (sections, unknown)
}

/// Shrink a Hermes context in place, without dropping a fact.
///
/// Three passes: lift fields repeated identically across a list, drop keys
/// whose value is null all the way down, and drop the two bulk narrative
/// fields from the experiment inventory. Returns a descriptor for the payload,
/// because a reader who cannot tell a compacted payload from a sparse one will
/// misread the second as the first.
pub(crate) fn compact_hermes_context(value: &mut JsonValue) -> JsonValue {
    let before = value.to_string().len();
    strip_experiment_bulk_fields(value);
    strip_thesis_narrative(value);
    compact_context_node(value);
    let after = value.to_string().len();
    json!({
        "detail": "compact",
        "bytes_before": before,
        "bytes_after": after,
        "applied": [
            "Fields identical on every element of a list were lifted into a sibling <key>_shared object. Read them as belonging to every element of that list.",
            "Keys whose value was null, or whose every nested leaf was null, were removed. An empty list or object is left as-is and does mean none.",
            "The experiment inventory omits evidence_json and raw_payload_json.",
            "Trade theses and thesis reviews omit approval_evidence, entry_rationale and catalyst_or_monitor, which restate in prose the technical and markov objects kept beside them.",
        ],
    })
}

pub(crate) fn scope_hermes_context(value: &mut JsonValue, sections: &[String]) {
    if sections.is_empty() {
        return;
    }
    let (requested, unknown) = normalize_hermes_context_sections(sections);
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let mut omitted: Vec<String> = Vec::new();
    if !requested.is_empty() {
        object.retain(|key, _| {
            let keep = HERMES_CONTEXT_ALWAYS_PRESENT.contains(&key.as_str())
                || requested.iter().any(|section| section == key);
            if !keep {
                omitted.push(key.clone());
            }
            keep
        });
        omitted.sort();
    }
    // A withheld section and an empty one are different facts. Without this, a
    // reader concludes "no execution activity today" from a payload that was
    // simply never asked for it.
    object.insert(
        "sections".to_string(),
        json!({
            "requested": requested,
            "omitted": omitted,
            "unknown": unknown,
            "note": "Sections named in omitted were not requested and are absent from this payload. Absence here is not evidence that the underlying data is empty. An unrecognised name filters nothing.",
        }),
    );
}

pub(crate) fn hermes_experiment_next_status(
    current_status: &str,
    action: &str,
) -> Option<&'static str> {
    match (current_status, action.trim()) {
        ("pending_review", "approve_paper") => Some("approved_paper"),
        ("pending_review", "reject") => Some("rejected"),
        ("pending_review", "expire_stale") => Some("expired_stale"),
        ("approved_paper", "activate_paper") => Some("active_paper"),
        ("approved_paper", "reject") => Some("rejected"),
        ("active_paper", "approve_sim") => Some("approved_sim"),
        ("active_paper", "mark_paper_failed") => Some("paper_failed"),
        ("active_paper", "reject") => Some("rejected"),
        ("approved_sim", "activate_sim") => Some("active_sim"),
        ("approved_sim", "reject") => Some("rejected"),
        ("active_sim", "ready_for_promotion") => Some("ready_for_promotion"),
        ("active_sim", "mark_sim_failed") => Some("sim_failed"),
        ("active_sim", "reject") => Some("rejected"),
        ("ready_for_promotion", "promote") => Some("promoted"),
        ("ready_for_promotion", "reject") => Some("rejected"),
        _ => None,
    }
}

fn shadow_report_hermes_advice_match(
    strategy_key: &str,
    symbol: &str,
    action: &str,
    advice: &[JsonValue],
) -> (String, String) {
    let symbol_key = canonical_symbol_key(symbol);
    let action = action.trim().to_ascii_uppercase();
    let matches = |item: &&JsonValue| {
        let advice_action = json_text(item, "action").to_ascii_lowercase();
        matches!(
            advice_action.as_str(),
            "allow" | "reduce" | "stand_down" | "review"
        )
    };
    if !strategy_key.trim().is_empty() {
        if let Some(item) = advice
            .iter()
            .find(|item| matches(item) && json_text(item, "strategy_key") == strategy_key)
        {
            return (
                json_text(item, "action").to_ascii_lowercase(),
                "strategy_key".to_string(),
            );
        }
    }
    if let Some(item) = advice.iter().find(|item| {
        matches(item)
            && canonical_symbol_key(&json_text(item, "symbol")) == symbol_key
            && json_text(item, "side").to_ascii_uppercase() == action
    }) {
        return (
            json_text(item, "action").to_ascii_lowercase(),
            "symbol_side".to_string(),
        );
    }
    if let Some(item) = advice.iter().find(|item| {
        matches(item) && canonical_symbol_key(&json_text(item, "symbol")) == symbol_key
    }) {
        return (
            json_text(item, "action").to_ascii_lowercase(),
            "symbol".to_string(),
        );
    }
    ("none".to_string(), "none".to_string())
}

fn compact_shadow_report_hermes_effect(
    outcome: &JsonValue,
    advice_row: Option<&JsonValue>,
    request_status: &str,
    source_session_id: &str,
) -> JsonValue {
    let context = decode_shadow_json_field(outcome.get("report_time_context_json"));
    let strategy_key = shadow_report_context_strategy_key(&context);
    let symbol = json_text(outcome, "symbol");
    let action = json_text(outcome, "action");
    let Some(advice_row) = advice_row else {
        return json!({
            "status": request_status,
            "mode": "record_only_shadow",
            "source_session_id": source_session_id,
            "effect": "record_only_unavailable",
            "advice_action": "not_recorded",
            "match_source": "none",
            "safety": "record_only_shadow_advice_cannot_change_queue_gate_or_saxo_authority",
        });
    };
    let order_advice = decode_shadow_json_field(advice_row.get("order_advice_json"));
    let order_advice = order_advice.as_array().cloned().unwrap_or_default();
    let (advice_action, match_source) =
        shadow_report_hermes_advice_match(&strategy_key, &symbol, &action, &order_advice);
    let effect = if advice_action == "none" {
        "record_only_no_matching_advice".to_string()
    } else {
        format!("record_only_{advice_action}")
    };
    let raw_payload = decode_shadow_json_field(advice_row.get("raw_payload_json"));
    let context_self_check = raw_payload
        .get("context_self_check")
        .cloned()
        .unwrap_or(JsonValue::Null);
    json!({
        "status": json_text(advice_row, "status"),
        "mode": "record_only_shadow",
        "advice_id": json_text(advice_row, "id"),
        "source_session_id": source_session_id,
        "overall_recommendation": json_text(advice_row, "overall_recommendation"),
        "effect": effect,
        "advice_action": advice_action,
        "match_source": match_source,
        "context_self_check_complete": context_self_check.get("complete").and_then(JsonValue::as_bool),
        "safety": "record_only_shadow_advice_cannot_change_queue_gate_or_saxo_authority",
    })
}

/// Reduce the account-value series to one observation per calendar day.
///
/// A snapshot is written every scheduler cycle, so a one-month range was 495
/// points and 186 KB of ten-field rows -- roughly sixteen readings per trading
/// day of a series whose shape is daily. The last observation of each day is
/// kept, which preserves the closing level the range is read for; the newest
/// point is always retained even when its day is still in progress, since that
/// is the current value.
pub(crate) fn compact_performance_history(history: Vec<JsonValue>) -> Vec<JsonValue> {
    let mut by_day: Vec<(String, JsonValue)> = Vec::new();
    for point in history {
        let day = point
            .get("recorded_at")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .chars()
            .take(10)
            .collect::<String>();
        match by_day.last_mut() {
            // The series arrives in order, so a repeated day overwrites rather
            // than appending: the last reading of a day is that day's close.
            Some((last_day, slot)) if *last_day == day => *slot = point,
            _ => by_day.push((day, point)),
        }
    }
    by_day.into_iter().map(|(_, point)| point).collect()
}

/// Narrative and metric documents carried on each strategy-journal row.
///
/// `diary_json` (7.5 KB per row) and `metrics_json` (3.7 KB) are the rendered
/// end-of-day documents. The advisory reader works from `summary` and
/// `learnings_json`; twenty rows of the full documents were 233 KB.
const HERMES_JOURNAL_DOCUMENT_KEYS: &[&str] = &["diary_json", "metrics_json"];

pub(crate) fn compact_journal_rows(rows: Vec<JsonValue>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|mut row| {
            if let Some(object) = row.as_object_mut() {
                for key in HERMES_JOURNAL_DOCUMENT_KEYS {
                    object.remove(*key);
                }
            }
            row
        })
        .collect()
}

/// Strip the two operational documents the dashboard overview embeds verbatim.
///
/// `overview` is built for the UI, which renders both of these. Reusing it for
/// the advisory context carried `markov_method.latest_run.summary_json` at
/// 173 KB -- the same embedded signal array U12 removed from the decision
/// prompt, in a second place that fix never reached -- plus another copy of the
/// full scheduler cycle document at 28 KB.
pub(crate) fn compact_overview_for_hermes(mut overview: JsonValue) -> JsonValue {
    if let Some(run) = overview
        .get_mut("markov_method")
        .and_then(|markov| markov.get_mut("latest_run"))
    {
        *run = crate::markov_method::trim_markov_run_for_prompt(run);
    }
    if let Some(status) = overview.get_mut("scheduler_status") {
        if let Some(cycle) = status.get("last_cycle_json") {
            let compact = compact_scheduler_cycle_document(cycle);
            if let Some(object) = status.as_object_mut() {
                object.insert("last_cycle_json".to_string(), compact);
            }
        }
    }
    overview
}

/// Apply `compact_scheduler_cycle_document` to the `cycle_json` of each row.
fn compact_scheduler_cycles(rows: Vec<JsonValue>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|mut row| {
            if let Some(object) = row.as_object_mut() {
                if let Some(cycle) = object.get("cycle_json") {
                    let compact = compact_scheduler_cycle_document(cycle);
                    object.insert("cycle_json".to_string(), compact);
                }
            }
            row
        })
        .collect()
}

/// Stored Saxo request/response documents that must never reach Hermes.
///
/// Measured on the live 2026-08-31 payload, `get_context` carried `AccountKey`
/// 25 times and `ClientKey` 20 times, every one of them inside a broker
/// document nested in these two columns. The execution-order events API already
/// allowlists its columns and excludes them; this path reused the dashboard
/// readers wholesale and never got the same treatment.
const HERMES_RAW_BROKER_DOCUMENT_KEYS: &[&str] = &["raw_payload_json", "execution_result_json"];

/// Project the raw broker documents out of execution rows bound for Hermes.
///
/// Dropped whole rather than redacted key by key: the credentials sit at
/// varying depths inside provider-shaped documents, so a field-level denylist
/// would silently reopen the moment Saxo nests them somewhere new. The
/// classified failure taxonomy carries the diagnostic signal Hermes actually
/// reasons from.
pub(crate) fn strip_raw_broker_documents(rows: Vec<JsonValue>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|mut row| {
            if let Some(object) = row.as_object_mut() {
                for key in HERMES_RAW_BROKER_DOCUMENT_KEYS {
                    object.remove(*key);
                }
            }
            row
        })
        .collect()
}

/// Per-field honesty record for the goal contract.
///
/// `runtime_enforced` -- a gate can change or block an order because of it.
/// `evaluation_only`  -- Hermes should weigh it; no gate reads it.
/// `structural`       -- true because the code path does not exist at all.
/// `documentation`    -- prose explaining a neighbouring field.
/// `not_enforced`     -- advertised, but nothing applies it. Each of these is a
///                       named debt, not a shrug.
///
/// Before this record existed the contract read as a list of enforced limits,
/// of which almost none were enforced, and Hermes was judging every experiment
/// against an envelope no gate defended.
fn hermes_goal_contract_enforcement() -> JsonValue {
    json!({
            "note": "How the runtime treats each objective and constraint. Fields marked evaluation_only or not_enforced must not be read as limits the system will defend.",
            "objective.target_return_30d": {
                "status": "evaluation_only",
                "detail": "The goal Hermes measures experiments against. No gate reads it."
            },
            "objective.target_return_note": {
                "status": "documentation",
                "detail": "Explains how the 30-day figure maps to the annual goal."
            },
            "objective.max_drawdown": {
                "status": "runtime_enforced",
                "detail": "The drawdown guardrail suspends new BUYs at this depth below the trailing peak and reduces the BUY budget in the soft band beneath it. SELLs are never blocked. Value is read from strategy.capital.drawdown_halt_pct, the same key the guardrail applies."
            },
            "objective.min_sharpe": {
                "status": "evaluation_only",
                "detail": "A promotion criterion for Hermes experiments. Sharpe is computed for evidence packs; no gate reads it."
            },
            "objective.failure_below_30d_return": {
                "status": "evaluation_only",
                "detail": "A rollback criterion for Hermes experiments."
            },
            "objective.reflection_every": {
                "status": "evaluation_only",
                "detail": "Reflection cadence expectation for Hermes."
            },
            "objective.one_variable_only": {
                "status": "runtime_enforced",
                "detail": "An overlay carries exactly one changed_variable_path, and only paths on the experiment variable allowlist are applied."
            },
            "constraints.max_positions": {
                "status": "runtime_enforced",
                "detail": "Caps new-symbol BUYs using persisted positive-quantity positions plus new-symbol BUYs approved earlier in the scheduler cycle. Adds to an existing holding do not consume a slot; an unavailable position snapshot blocks a new-symbol BUY. Value is read from strategy.swing.max_holdings."
            },
            "constraints.slippage_tolerance": {
                "status": "not_enforced",
                "detail": "This 2% tolerance is never compared against anything. A cost model does exist and does gate BUYs: cost_guard_gate estimates one-way slippage as strategy.estimated_slippage_bps (8 bps) of the order value, adds the exchange minimum round-trip commission scaled by strategy.cost_guard_multiple, and refuses a BUY whose verified reward target cannot clear that floor. So slippage is modelled and enforced -- the runtime simply uses its own far stricter estimate rather than this contract value, and no check would fire if realised slippage approached 2%."
            },
            "constraints.min_cash_buffer_pct": {
                "status": "runtime_enforced",
                "detail": "Bounds the cycle-wide BUY budget in the capital plan."
            },
            "constraints.allow_shorting": {
                "status": "structural",
                "detail": "The runtime has no short path, so this is false regardless of configuration."
            },
            "constraints.require_human_approval": {
                "status": "runtime_enforced",
                "detail": "An experiment overlay is applied only from an operator-approved status; proposals cannot self-activate."
            },
            "constraints.require_backtest_before_activation": {
                "status": "not_enforced",
                "detail": "No backtest engine exists. Activation is gated on operator approval and SIM/paper observation instead."
            },
            "constraints.require_paper_or_sim_observation": {
                "status": "runtime_enforced",
                "detail": "Overlays are refused when execution mode is live against a non-SIM Saxo environment."
            }
    })
}

/// The Hermes goal contract, built from configuration alone so the whole
/// payload -- including the enforcement record -- is testable without a live
/// `AppState`.
pub(crate) fn hermes_goal_contract_from_config(config: &YamlValue) -> JsonValue {
    // Every drawdown limit the contract quotes comes from the one key the
    // guardrail enforces, so the advertised and applied numbers cannot diverge.
    let max_drawdown = crate::drawdown_guard::DrawdownPolicy::from_config(config).halt_pct;
    json!({
        "enabled": true,
        "mode": "recommend_only",
        // Goal version 2 (2026-07-25) replaces the previous 47%/30d
        // "10x in 6 months" objective, which was roughly 70x the operator's
        // actual target and pushed Hermes to evaluate every experiment against
        // a return it could only reach by taking far more risk than the loss
        // floors allow.
        "goal_version": 2,
        "objective": {
            "target_return_30d": 0.0117,
            "target_return_note": "Approximately +15% per year compounded monthly: 1.0117^12 ~= 1.15",
            "max_drawdown": max_drawdown,
            "min_sharpe": 1.0,
            "failure_below_30d_return": -0.02,
            "reflection_every": "7d",
            "one_variable_only": true
        },
        "constraints": {
            "max_positions": yaml_i64(config, &["strategy", "swing", "max_holdings"]).unwrap_or(25),
            "slippage_tolerance": 0.02,
            "min_cash_buffer_pct": yaml_f64(config, &["strategy", "capital", "min_cash_buffer_pct"]).unwrap_or(0.10),
            "allow_shorting": yaml_bool(config, &["risk", "allow_shorting"]).unwrap_or(false),
            "require_human_approval": true,
            "require_backtest_before_activation": true,
            "require_paper_or_sim_observation": true
        },
        // Every objective and constraint above declares what the runtime
        // actually does with it. `hermes_goal_contract_declares_enforcement_for_every_field`
        // fails if a field is added without an entry, which is what keeps the
        // two halves from drifting apart again.
        "enforcement": hermes_goal_contract_enforcement(),
        "experiment_policy": {
            "proposal_cadence": {
                "daily": "May create at most one pending-review proposal when a same-day learning is specific, evidence-backed, and safe to test in paper/SIM.",
                "weekly": "Should create one pending-review proposal when the week contains enough evidence and no duplicate active proposal already covers the same variable."
            },
            "proposal_requirement": "Hermes should turn concrete learnings into reviewable one-variable proposals instead of stopping at narrative reflection.",
            "min_observation_days": 7,
            "min_closed_trades": 5,
            "daily_exception": {
                "allowed": true,
                "reason": "A daily proposal is allowed for operational learnings such as repeated execution failures, stale signals, missed scheduled reports, or clear risk-budget/cash-buffer friction.",
                "still_requires_review": true
            },
            "promote_only_if": {
                "return_30d_gte": 0.0117,
                "drawdown_lte": max_drawdown,
                "sharpe_gte": 1.0
            },
            "rollback_if": {
                "return_30d_lte": -0.02,
                "drawdown_gt": max_drawdown,
                "safety_violation": true
            }
        }
    })
}

pub(crate) fn hermes_capabilities_payload(goal_contract: JsonValue) -> HermesCapabilitiesPayload {
    HermesCapabilitiesPayload {
        status: "ok".to_string(),
        runtime: "saxo-rust".to_string(),
        namespace: "saxo".to_string(),
        database_namespace: "saxo".to_string(),
        safe_endpoints: [
            "/api/hermes/capabilities",
            "/api/hermes/context",
            "/api/hermes/reflections",
            "/api/hermes/experiments",
            "/api/hermes/experiments/{id}/transition",
            "/api/health",
            "/api/overview",
            "/api/markov/signals",
            "/api/decision/latest",
            "/api/decision/reports",
            "/api/scheduler",
            "/api/execution",
            "/api/strategy-journal",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        read_models: [
            "overview",
            "scheduler_status",
            "scheduler_cycle_history",
            "decision_reports.report_json",
            "strategy_journal_entries",
            "execution_orders",
            "execution_order_events",
            "execution_fills",
            "portfolio_value_history",
            "markov_signal_runs",
            "markov_asset_signals",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        restricted_writes: [
            "hermes_reflections",
            "strategy_experiments",
            "hermes_decision_advice",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        decision_advice: HermesDecisionAdviceCapabilitiesPayload {
            scope: "per_decision_report_trading_manager_preflight".to_string(),
            write_tool: "create_decision_advice".to_string(),
            allowed_recommendations: ["proceed", "stand_down", "review"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            allowed_order_actions: ["allow", "reduce", "stand_down", "review"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            required_context_self_check: HermesContextSelfCheckCapabilitiesPayload {
                fields: hermes_context_self_check_required_fields()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                format: "Include context_self_check with one boolean per field, optional sources, notes, and missing context explanations.".to_string(),
                required_sources: [
                    "get_decision_reports",
                    "get_markov_signals",
                    "get_end_of_day_reports",
                    "get_context",
                    "list_reflections",
                    "list_experiments",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
            safety: "advisory only; cannot add trades, increase size, approve live orders, or call Saxo mutation endpoints".to_string(),
        },
        supported_experiment_overlays: HermesExperimentOverlayCapabilitiesPayload {
            scope: "paper_or_saxo_sim_only".to_string(),
            statuses: ["approved_sim", "active_sim", "approved_paper", "active_paper"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            // Every entry must be a key the runtime actually reads. A dead
            // setting cannot be a valid SIM experiment variable.
            variables: SUPPORTED_EXPERIMENT_VARIABLES
                .iter()
                .map(|path| (*path).to_string())
                .collect(),
        },
        forbidden: [
            "saxo_sessions",
            "Saxo OAuth token/session reads",
            "order precheck/place/replace/cancel",
            "live order approval",
            "Kubernetes secret mutation",
            "live broker baseline activation",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        notes: [
            "Hermes proposals are recommend-only until reviewed by the daytrader UI/operator flow.",
            "Promoted baselines are audit records; they do not activate live broker behavior.",
            "Strategy experiments must change exactly one variable while one_variable_only is true.",
            "Markov method signals are advisory analytics and do not place or approve orders.",
            "Gate replay is a read-only historical target-gate comparison; a target-gate clear is not a full approval.",
            "QuiverQuant alternative-data signals are advisory analytics and do not place or approve orders.",
            "Public editorial-research items are attributable secondary context only and do not place, approve, block, size, or otherwise modify orders.",
            "Scheduled decision reports target two daily open-followup pulses: Nordic/EU open +1h15 and US open +1h15.",
            "Daily end-of-day reports are exposed as sanitized strategy journal rows.",
            "The Hermes adapter intentionally excludes raw request_json/response_json payloads from decision reports.",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        goal_contract,
    }
}

/// Read-only outcome accounting for closed SELL ledger rows. A partial sale is
/// one realised row, not a complete round-trip trade. The local ledger does
/// not retain a durable lot-to-sale link or broker quote-at-submission record,
/// so this deliberately excludes holding time and realised slippage.
/// The realised-outcome summary Hermes reads: aggregates only.
///
/// The dashboard payload also carries twelve recent sale rows and three
/// attribution tables, which is detail for someone reading a screen rather than
/// evidence for a loop deciding whether a threshold should move. Lifted from
/// the overview the context already builds, so there is one computation of
/// these numbers rather than a second that can disagree with the panel.
pub(crate) fn realised_outcomes_for_hermes(overview: &JsonValue) -> JsonValue {
    let Some(outcomes) = overview
        .get("integrity")
        .and_then(|integrity| integrity.get("realised_sell_outcomes"))
    else {
        return json!({"status": "unavailable"});
    };
    let mut summary = serde_json::Map::new();
    for key in [
        "status",
        "sample_requirement",
        "closed_sale_count",
        "win_count",
        "loss_count",
        "breakeven_count",
        "win_rate",
        "average_win_dkk",
        "average_loss_dkk",
        "payoff_ratio",
        "total_realised_gain_dkk",
        "total_commission_dkk",
        "baseline",
        "holding_period",
    ] {
        if let Some(value) = outcomes.get(key) {
            summary.insert(key.to_string(), value.clone());
        }
    }
    summary.insert(
        "interpretation".to_string(),
        json!(
            "Reconciled local SELL-ledger accounting for the current book only; `baseline` names the import batch it starts at and what the superseded book held. It is not a backtest, a broker poll, or a trading gate, and a sale is counted when the ledger row is reconciled rather than when a position closes."
        ),
    );
    JsonValue::Object(summary)
}

fn hermes_experiment_duplicate_blocking_statuses_sql() -> String {
    HERMES_EXPERIMENT_DUPLICATE_BLOCKING_STATUSES
        .iter()
        .copied()
        .filter(|status| hermes_experiment_status_blocks_duplicate(status))
        .map(|status| format!("'{}'", sql_escape(status)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn hermes_context_self_check_required_fields() -> Vec<&'static str> {
    vec![
        "latest_report",
        "markov_signals",
        "end_of_day_report",
        "current_positions",
        "active_experiments",
    ]
}

pub(crate) fn normalize_hermes_context_self_check(value: JsonValue) -> JsonValue {
    let mut object = value
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    let mut missing = Vec::new();
    for field in hermes_context_self_check_required_fields() {
        if object.get(field).and_then(JsonValue::as_bool) != Some(true) {
            missing.push(JsonValue::String(field.to_string()));
        }
    }
    object.insert(
        "required".to_string(),
        json!(hermes_context_self_check_required_fields()),
    );
    object.insert("missing".to_string(), JsonValue::Array(missing.clone()));
    object.insert("complete".to_string(), JsonValue::Bool(missing.is_empty()));
    JsonValue::Object(object)
}

impl AppState {
    pub fn hermes_goal_contract_value(&self) -> JsonValue {
        hermes_goal_contract_from_config(&self.config)
    }

    pub fn hermes_capabilities(&self) -> HermesCapabilitiesPayload {
        hermes_capabilities_payload(self.hermes_goal_contract_value())
    }

    pub fn hermes_capabilities_value(&self) -> JsonValue {
        serde_json::to_value(self.hermes_capabilities())
            .expect("Hermes capabilities must serialize")
    }

    pub async fn hermes_context(&self, limit: i64) -> Result<HermesContextPayload> {
        let limit = clamp_limit(limit, 1, 50);
        let overview =
            compact_overview_for_hermes(self.overview_payload().await.unwrap_or_else(|err| {
                warn!("Hermes overview context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            }));
        let mut scheduler_status = self.scheduler_status_value().await.unwrap_or_else(|err| {
            warn!("Hermes scheduler status degraded: {err:#}");
            json!({"status": "degraded", "detail": err.to_string()})
        });
        // The status row embeds the same full cycle document as the history.
        if let Some(object) = scheduler_status.as_object_mut() {
            if let Some(cycle) = object.get("last_cycle_json") {
                let compact = compact_scheduler_cycle_document(cycle);
                object.insert("last_cycle_json".to_string(), compact);
            }
        }
        let scheduler_cycles =
            compact_scheduler_cycles(self.scheduler_cycles(limit).await.unwrap_or_default());
        let decision_reports = self.hermes_decision_report_items(limit).await?;
        // `strategy_journal` and `end_of_day` read the same table and returned
        // byte-identical rows, so the payload carried one dataset twice. The
        // end-of-day framing is the one the advisory prompt and the context
        // self-check refer to, so it keeps the rows; the journal key stays as a
        // pointer rather than being removed, since removing a documented key is
        // a harsher contract change than emptying it.
        let journal_count = self
            .strategy_journal_items(limit)
            .await
            .unwrap_or_default()
            .len();
        let end_of_day_reports =
            compact_journal_rows(self.hermes_end_of_day_report_items(limit).await?);
        // These readers are shared with the dashboard, which is allowed to see
        // the stored broker documents. This context is not: see
        // `strip_raw_broker_documents`.
        let execution_orders =
            strip_raw_broker_documents(self.execution_orders(limit).await.unwrap_or_default());
        let execution_failures =
            strip_raw_broker_documents(self.hermes_execution_failures(limit).await?);
        let execution_events =
            strip_raw_broker_documents(self.execution_events(limit).await.unwrap_or_default());
        let execution_fills =
            strip_raw_broker_documents(self.execution_fills(limit).await.unwrap_or_default());
        let protective_stop_coverage = self
            .protective_stop_coverage()
            .await
            .map(|coverage| {
                compact_protective_stop_coverage_for_hermes(&coverage, limit as usize)
            })
            .unwrap_or_else(|err| {
                warn!("Hermes protective-stop coverage degraded: {err:#}");
                json!({
                    "status": "unavailable",
                    "summary": {},
                    "positions": [],
                    "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
                })
            });
        let holding_thesis_reviews = self.holding_thesis_reviews().await.unwrap_or_else(|err| {
            warn!("Hermes holding-thesis review context degraded: {err:#}");
            json!({
                "status": "unavailable",
                "safety": "read_only_local_broker_position_snapshot_execution_order_thesis_and_fill_audit_no_saxo_provider_hermes_or_order_mutation",
            })
        });
        let performance = compact_performance_history(
            self.performance_history_with_current("1M", 500)
                .await
                .unwrap_or_default(),
        );
        let active_experiments = self.hermes_experiments(10).await.unwrap_or_default();
        let learning_memory = self.hermes_learning_memory(limit).await.unwrap_or_default();
        let active_learning_memory = learning_memory
            .iter()
            .filter(|lesson| json_text(lesson, "status") != "stale")
            .cloned()
            .collect::<Vec<_>>();
        let stale_learning_memory_count = learning_memory
            .iter()
            .filter(|lesson| json_text(lesson, "status") == "stale")
            .count();
        let active_strategy_baseline = self
            .active_strategy_baseline()
            .await
            .unwrap_or(JsonValue::Null);
        let gate_replay = self
            .decision_gate_replay(limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes gate replay context degraded: {err:#}");
                json!({"status": "unavailable"})
            });
        let markov = crate::markov_method::compact_markov_context(self, limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes Markov context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            });
        let quiver = crate::quiver::compact_quiver_context(self, limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes Quiver context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            });
        let quiver_positions = self.position_items(250).await.unwrap_or_else(|err| {
            warn!("Hermes Quiver conflict holdings degraded: {err:#}");
            Vec::new()
        });
        let quiver_conflicts = crate::quiver::held_position_conflicts(&quiver_positions, &quiver);
        let editorial_research =
            crate::editorial_research::compact_editorial_research_context(self, limit)
                .await
                .unwrap_or_else(|err| {
                    warn!("Hermes editorial research context degraded: {err:#}");
                    json!({"status": "degraded", "detail": err.to_string()})
                });
        let daily_indicators = crate::daily_indicators::compact_indicator_context(self, limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes daily-indicator context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            });

        // Computed before `overview` is moved into the payload, so the loop and
        // the panel read one set of numbers rather than two that can disagree.
        let realised_outcomes = realised_outcomes_for_hermes(&overview);
        Ok(HermesContextPayload {
            status: "ok".to_string(),
            generated_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            capabilities: self.hermes_capabilities(),
            goal_contract: self.hermes_goal_contract_value(),
            overview,
            scheduler: HermesContextSchedulerPayload {
                status: scheduler_status,
                cycles: scheduler_cycles,
            },
            decisions: HermesContextDecisionsPayload {
                cadence: "two_daily_open_followups".to_string(),
                pulses: crate::xai_decision::decision_pulse_summary(self)
                    .get("pulses")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                reports: decision_reports,
            },
            end_of_day: HermesContextEndOfDayPayload {
                cadence: "daily".to_string(),
                reports: end_of_day_reports,
            },
            strategy_journal: HermesContextItemsPayload {
                items: vec![json!({
                    "note": "Strategy journal rows are the end-of-day reports; see end_of_day.reports to avoid carrying the same dataset twice.",
                    "row_count": journal_count,
                })],
            },
            execution: HermesContextExecutionPayload {
                orders: execution_orders,
                failures: execution_failures,
                events: execution_events,
                fills: execution_fills,
                protective_stop_coverage,
                holding_thesis_reviews,
            },
            performance: HermesContextPerformancePayload {
                range: "1M".to_string(),
                history: performance,
                realised_outcomes,
            },
            markov_method: markov,
            quiver_signals: quiver,
            quiver_conflicts,
            editorial_research,
            daily_indicators,
            hermes: HermesContextHermesPayload {
                experiments: active_experiments,
                active_strategy_baseline,
                gate_replay,
                learning_memory: HermesContextLearningMemoryPayload {
                    active: active_learning_memory,
                    stale_count: stale_learning_memory_count,
                    policy: "repeated reflection actions become stable after two distinct reflections; emerging lessons expire after 7d and stable lessons after 21d".to_string(),
                },
            },
            safety: HermesContextSafetyPayload {
                saxo_sessions_excluded: true,
                broker_mutations_excluded: true,
                raw_oauth_payloads_excluded: true,
                raw_broker_payloads_excluded: true,
            },
        })
    }

    /// `sections` narrows the payload to the named top-level keys.
    ///
    /// The full context is ~430 KB. A caller that only needs the day's
    /// decisions and outcomes otherwise pays for the quiver, editorial,
    /// overview and capability blocks it never reads -- and pays again on
    /// every turn of the agent loop, not once. An empty list keeps every
    /// section, so callers that do not ask are unaffected.
    pub async fn hermes_context_value(
        &self,
        limit: i64,
        sections: &[String],
        detail: ContextDetail,
    ) -> Result<JsonValue> {
        let mut value = serde_json::to_value(self.hermes_context(limit).await?)?;
        scope_hermes_context(&mut value, sections);
        if detail == ContextDetail::Compact {
            let descriptor = compact_hermes_context(&mut value);
            if let Some(object) = value.as_object_mut() {
                object.insert("compaction".to_string(), descriptor);
            }
        }
        Ok(value)
    }

    pub async fn hermes_decision_report_items(
        &self,
        limit: i64,
    ) -> Result<Vec<HermesDecisionReportOutcomePayload>> {
        let sql = format!(
            "SELECT
                dr.id AS report_id,
                dr.created_at,
                dr.report_date,
                dr.status AS report_status,
                dr.analysis_pulse_key,
                dr.analysis_pulse_label,
                dr.pulse_mode,
                dr.queue_eligible,
                dr.report_json,
                (
                    SELECT tm.status
                    FROM trading_manager_runs tm
                    WHERE tm.report_id = dr.id
                    ORDER BY tm.created_at DESC, tm.id DESC
                    LIMIT 1
                ) AS manager_status,
                COUNT(eo.id) AS execution_order_count,
                SUM(CASE WHEN eo.status = 'pending_execution' THEN 1 ELSE 0 END) AS pending_execution_count,
                SUM(CASE WHEN eo.status = 'broker_working' THEN 1 ELSE 0 END) AS broker_working_count,
                SUM(CASE WHEN eo.status = 'broker_partially_filled' THEN 1 ELSE 0 END) AS partial_fill_count,
                SUM(CASE WHEN eo.status = 'filled' THEN 1 ELSE 0 END) AS filled_count,
                SUM(CASE WHEN eo.status IN ('expired_local', 'broker_expired', 'broker_done_for_day') THEN 1 ELSE 0 END) AS expired_count,
                SUM(CASE WHEN eo.status = 'broker_cancelled' THEN 1 ELSE 0 END) AS cancelled_count,
                SUM(CASE WHEN eo.status IN ('execution_failed', 'broker_rejected', 'local_rejected') THEN 1 ELSE 0 END) AS failed_or_rejected_count,
                SUM(CASE WHEN eo.status = 'filled' AND tl.side = 'SELL' THEN 1 ELSE 0 END) AS realised_sell_count,
                COALESCE(SUM(CASE WHEN eo.status = 'filled' AND tl.side = 'SELL' THEN tl.realised_gain_dkk ELSE 0 END), 0) AS realised_sell_gain_dkk
             FROM decision_reports dr
             LEFT JOIN execution_orders eo ON eo.report_id = dr.id
             LEFT JOIN trade_ledger tl ON tl.id = eo.ledger_id
             GROUP BY dr.id, dr.created_at, dr.report_date, dr.status, dr.analysis_pulse_key,
                      dr.analysis_pulse_label, dr.pulse_mode, dr.queue_eligible, dr.report_json
             ORDER BY dr.created_at DESC, dr.id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(
            crate::hermes_state::hermes_decision_report_outcomes_from_rows(
                self.select_json(&sql).await.unwrap_or_default(),
            ),
        )
    }

    pub async fn hermes_decision_advice_audit(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT
                dr.id AS report_id,
                dr.created_at AS report_created_at,
                dr.status AS report_status,
                dr.analysis_pulse_key,
                dr.analysis_pulse_label,
                dr.model,
                (
                    SELECT h.id
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_id,
                (
                    SELECT h.created_at
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_created_at,
                (
                    SELECT h.status
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_status,
                (
                    SELECT h.source_session_id
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_source_session_id,
                (
                    SELECT h.overall_recommendation
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_recommendation,
                (
                    SELECT h.summary
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_summary,
                (
                    SELECT h.order_advice_json
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS order_advice_json,
                (
                    SELECT h.raw_payload_json
                    FROM hermes_decision_advice h
                    WHERE h.decision_report_id = dr.id
                    ORDER BY h.created_at DESC, h.id DESC
                    LIMIT 1
                ) AS advice_raw_payload_json,
                (
                    SELECT tm.status
                    FROM trading_manager_runs tm
                    WHERE tm.report_id = dr.id
                    ORDER BY tm.created_at DESC, tm.id DESC
                    LIMIT 1
                ) AS manager_status,
                (
                    SELECT tm.created_at
                    FROM trading_manager_runs tm
                    WHERE tm.report_id = dr.id
                    ORDER BY tm.created_at DESC, tm.id DESC
                    LIMIT 1
                ) AS manager_created_at,
                (
                    SELECT tm.manager_json
                    FROM trading_manager_runs tm
                    WHERE tm.report_id = dr.id
                    ORDER BY tm.created_at DESC, tm.id DESC
                    LIMIT 1
                ) AS manager_json,
                (
                    SELECT tm.queue_result_json
                    FROM trading_manager_runs tm
                    WHERE tm.report_id = dr.id
                    ORDER BY tm.created_at DESC, tm.id DESC
                    LIMIT 1
                ) AS queue_result_json,
                (
                    SELECT COUNT(*)
                    FROM execution_orders eo
                    WHERE eo.report_id = dr.id
                ) AS queued_order_count,
                (
                    SELECT COUNT(*)
                    FROM execution_orders eo
                    WHERE eo.report_id = dr.id AND eo.status = 'executed'
                ) AS executed_order_count,
                (
                    SELECT COUNT(*)
                    FROM execution_orders eo
                    WHERE eo.report_id = dr.id AND eo.status IN ('execution_failed', 'broker_rejected', 'local_rejected')
                ) AS failed_order_count
             FROM decision_reports dr
             ORDER BY dr.created_at DESC, dr.id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// Records the portion of a decision-report order that Hermes prevented or
    /// reduced as a quote-to-quote shadow observation. This is audit-only: it
    /// never creates a broker order and intentionally excludes fees, FX and
    /// slippage, so it must not be treated as realised strategy performance.
    pub async fn record_hermes_counterfactuals(
        &self,
        report_id: i64,
        manager_run_id: i64,
        advice_delta: &JsonValue,
    ) -> Result<JsonValue> {
        let candidates = advice_delta
            .get("candidates")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut created = 0usize;
        let mut awaiting_reference = 0usize;
        let mut skipped = 0usize;

        for (index, candidate) in candidates.into_iter().enumerate() {
            let effect = json_text(&candidate, "effect");
            let requested_quantity = value_f64(&candidate, "requested_quantity");
            let resulting_quantity = value_f64(&candidate, "resulting_quantity");
            let Some(shadow_quantity) = hermes_counterfactual_shadow_quantity(
                &effect,
                requested_quantity,
                resulting_quantity,
            ) else {
                skipped += 1;
                continue;
            };
            let strategy_key = json_text(&candidate, "strategy_key");
            let symbol = json_text(&candidate, "symbol");
            let action = json_text(&candidate, "action").to_uppercase();
            if strategy_key.trim().is_empty()
                || symbol.trim().is_empty()
                || !matches!(action.as_str(), "BUY" | "SELL")
            {
                skipped += 1;
                continue;
            }
            let reported_reference_price_local = candidate
                .get("reference_price_local")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0);
            // A provider-supplied report price is useful context, but it is
            // not an auditable market reference: it lacks Saxo provenance and
            // capture time. Every new shadow waits for the read-only Saxo
            // infoprice loop to establish its baseline.
            awaiting_reference += 1;
            let reported_reference_sql = reported_reference_price_local
                .map(|value| value.to_string())
                .unwrap_or_else(|| "NULL".to_string());
            let currency = json_text(&candidate, "currency");
            let id = format!("hermes-counterfactual-{manager_run_id}-{index}");
            let result = sqlx::query(&format!(
                "INSERT INTO hermes_counterfactuals (
                    id, created_at, updated_at, report_id, manager_run_id,
                    strategy_key, symbol, action, source_effect, shadow_quantity,
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
                sql_escape(&effect),
                shadow_quantity,
                reported_reference_sql,
                sql_optional_text(Some(&currency)),
            ))
            .execute(&self.pool)
            .await
            .context("recording Hermes counterfactual")?;
            created += result.rows_affected() as usize;
        }

        Ok(json!({
            "status": "ok",
            "created": created,
            "awaiting_reference": awaiting_reference,
            "skipped": skipped,
            "safety": "quote_to_quote_observation_only"
        }))
    }

    pub async fn hermes_counterfactuals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, updated_at, report_id, manager_run_id, strategy_key,
                    symbol, action, source_effect, shadow_quantity, reference_price_local,
                    reference_price_at, reference_price_source, reported_reference_price_local,
                    currency, status, latest_price_local, latest_price_at,
                    estimated_return_pct, estimated_pnl_local, observation_count
             FROM hermes_counterfactuals
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn active_hermes_counterfactual_symbols(&self) -> Result<Vec<String>> {
        let rows = self
            .select_json(
                "SELECT DISTINCT symbol
                 FROM hermes_counterfactuals
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

    pub async fn refresh_hermes_counterfactual_price(
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
                 FROM hermes_counterfactuals
                 WHERE symbol = '{}' AND status IN ('tracking', 'awaiting_reference')",
                sql_escape(symbol)
            ))
            .await?;
        let mut updated = 0usize;
        for row in rows {
            let id = json_text(&row, "id");
            if json_text(&row, "status") == "awaiting_reference" {
                let result = sqlx::query(&format!(
                    "UPDATE hermes_counterfactuals
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
                .context("capturing Hermes counterfactual reference quote")?;
                updated += result.rows_affected() as usize;
                continue;
            }
            let action = json_text(&row, "action");
            let shadow_quantity = value_f64(&row, "shadow_quantity");
            let reference_price_local = value_f64(&row, "reference_price_local");
            let Some((estimated_return_pct, estimated_pnl_local)) =
                hermes_counterfactual_quote_metrics(
                    &action,
                    shadow_quantity,
                    reference_price_local,
                    latest_price_local,
                )
            else {
                continue;
            };
            let result = sqlx::query(&format!(
                "UPDATE hermes_counterfactuals
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
            .context("updating Hermes counterfactual quote")?;
            updated += result.rows_affected() as usize;
        }
        Ok(updated)
    }

    /// Builds the only advisory context a shadow report may send to Hermes.
    /// It is a compact local projection of durable rows; raw prompts, provider
    /// content, broker payloads, sessions, and every execution surface remain
    /// outside this method.
    pub async fn shadow_report_hermes_advisory_context(&self, report_id: i64) -> Result<JsonValue> {
        let report = self
            .first_json(&format!(
                "SELECT id, report_date, analysis_pulse_key, analysis_pulse_label, pulse_mode, queue_eligible, status\n                 FROM decision_reports WHERE id = {} LIMIT 1",
                report_id.max(0)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        if !shadow_report_outcome_report_is_eligible(&report) {
            return Ok(json!({
                "status": "not_shadow_report",
                "report_id": report_id,
                "safety": "no_hermes_queue_or_saxo_authority",
            }));
        }
        let rows = self
            .select_json(&format!(
                "SELECT id, candidate_rank, symbol, action, proposed_quantity, deterministic_gate_code, deterministic_gate_json, approved_policy_source, report_time_context_json\n                 FROM shadow_report_outcomes\n                 WHERE report_id = {}\n                 ORDER BY candidate_rank ASC, id ASC",
                report_id
            ))
            .await?;
        let candidates = rows
            .iter()
            .map(|row| {
                let context = decode_shadow_json_field(row.get("report_time_context_json"));
                json!({
                    "candidate_rank": value_i64(row, "candidate_rank"),
                    "symbol": json_text(row, "symbol"),
                    "action": json_text(row, "action"),
                    "proposed_quantity": value_f64(row, "proposed_quantity"),
                    "strategy_key": shadow_report_context_strategy_key(&context),
                    "decision_time_signal_gate": decode_shadow_json_field(row.get("deterministic_gate_json")),
                    "approved_policy_source": row.get("approved_policy_source").cloned().unwrap_or(JsonValue::Null),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "status": if candidates.is_empty() { "no_shadow_candidates" } else { "eligible" },
            "report_id": report_id,
            "pulse_key": json_text(&report, "analysis_pulse_key"),
            "pulse_label": json_text(&report, "analysis_pulse_label"),
            "report_date": json_text(&report, "report_date"),
            "candidates": candidates,
            "mode": "record_only_shadow",
            "safety": "compact_local_shadow_context_no_queue_manager_gate_or_saxo_authority",
        }))
    }

    /// Applies a compact Hermes record-only audit to persisted shadow rows.
    /// It never reads a broker surface, invokes a manager, changes a gate, or
    /// creates an execution order. A missing/timeout advisory is persisted as
    /// unavailable evidence rather than being treated as no-op advice.
    /// Re-classify shadow rows whose Hermes effect was written before the
    /// advice existed.
    ///
    /// `record_shadow_report_hermes_effects` runs immediately after the shadow
    /// advisory is *requested*, and Hermes answers asynchronously -- on report
    /// 256 the outcome rows were written at 12:20:06Z and the advice landed at
    /// 12:20:51Z. The lookup therefore found nothing, wrote
    /// `record_only_unavailable`, and nothing revisited it, so 13 of 15 shadow
    /// candidates carried no Hermes dimension at all even though matching
    /// per-order advice existed for them. That is the half of the shadow book
    /// meant to answer whether Hermes helps.
    ///
    /// Idempotent and bounded: it only touches rows still marked unresolved,
    /// and only where the advice has since been recorded.
    pub async fn repair_shadow_report_hermes_effects(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(
                "SELECT DISTINCT o.report_id
                 FROM shadow_report_outcomes o
                 JOIN hermes_decision_advice a
                   ON a.decision_report_id = o.report_id
                 WHERE o.hermes_effect IN ('record_only_unavailable', 'not_requested_shadow')
                 ORDER BY o.report_id DESC
                 LIMIT 20",
            )
            .await
            .unwrap_or_default();
        let mut repaired = 0usize;
        let mut reports = Vec::new();
        for row in &rows {
            let report_id = value_i64(row, "report_id");
            if report_id <= 0 {
                continue;
            }
            let request = json!({
                "status": "repaired_after_advice_arrived",
                "source_session_id": format!("shadow-decision-advice-{report_id}"),
            });
            match self
                .record_shadow_report_hermes_effects(report_id, &request)
                .await
            {
                Ok(result) => {
                    let updated = value_i64(&result, "updated");
                    if updated > 0 {
                        repaired += updated as usize;
                        reports.push(report_id);
                    }
                }
                Err(err) => warn!(report_id, "shadow Hermes effect repair degraded: {err:#}"),
            }
        }
        if repaired > 0 {
            info!(
                repaired,
                report_count = reports.len(),
                "repaired shadow Hermes effects recorded before their advice arrived"
            );
        }
        Ok(json!({
            "status": "ok",
            "candidates_repaired": repaired,
            "reports": reports,
            "safety": "record_only_shadow_repair_no_queue_gate_or_saxo_authority",
        }))
    }

    pub async fn record_shadow_report_hermes_effects(
        &self,
        report_id: i64,
        request_summary: &JsonValue,
    ) -> Result<JsonValue> {
        let source_session_id = json_text(request_summary, "source_session_id");
        let expected_session_id = format!("shadow-decision-advice-{}", report_id.max(0));
        if source_session_id != expected_session_id {
            return Ok(json!({
                "status": "invalid_shadow_advice_session",
                "updated": 0,
                "safety": "no_queue_gate_or_saxo_authority",
            }));
        }
        let context = self
            .shadow_report_hermes_advisory_context(report_id)
            .await?;
        if json_text(&context, "status") != "eligible" {
            return Ok(json!({
                "status": json_text(&context, "status"),
                "updated": 0,
                "safety": "no_queue_gate_or_saxo_authority",
            }));
        }
        let advice = self
            .hermes_decision_advice_by_session(&source_session_id)
            .await?;
        let rows = self
            .select_json(&format!(
                "SELECT id, symbol, action, approved_policy_source, report_time_context_json\n                 FROM shadow_report_outcomes\n                 WHERE report_id = {}\n                 ORDER BY candidate_rank ASC, id ASC",
                report_id
            ))
            .await?;
        let request_status = json_text(request_summary, "status");
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut updated = 0usize;
        let mut effect_counts = BTreeMap::<String, usize>::new();
        for row in rows {
            let snapshot = compact_shadow_report_hermes_effect(
                &row,
                advice.as_ref(),
                if request_status.is_empty() {
                    "unavailable"
                } else {
                    &request_status
                },
                &source_session_id,
            );
            let effect = json_text(&snapshot, "effect");
            let context = decode_shadow_json_field(row.get("report_time_context_json"));
            let policy_source = if json_text(&row, "approved_policy_source").is_empty() {
                shadow_report_approved_policy_source(&context)
            } else {
                None
            };
            let result = sqlx::query(&format!(
                "UPDATE shadow_report_outcomes\n                 SET updated_at = '{}', hermes_effect = '{}', hermes_advice_snapshot_json = '{}',\n                     approved_policy_source = COALESCE(approved_policy_source, {})\n                 WHERE id = '{}'",
                sql_escape(&now),
                sql_escape(&effect),
                sql_escape(&serde_json::to_string(&snapshot)?),
                sql_optional_text(policy_source.as_deref()),
                sql_escape(&json_text(&row, "id")),
            ))
            .execute(&self.pool)
            .await
            .context("recording shadow Hermes advisory effect")?;
            updated += result.rows_affected() as usize;
            *effect_counts.entry(effect).or_default() += 1;
        }
        Ok(json!({
            "status": if advice.is_some() { "recorded" } else { "unavailable_recorded" },
            "updated": updated,
            "source_session_id": source_session_id,
            "effect_counts": effect_counts,
            "safety": "record_only_shadow_advice_no_queue_manager_gate_or_saxo_authority",
        }))
    }

    pub async fn hermes_end_of_day_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, metrics_json, learnings_json, source_report_id, diary_json
             FROM strategy_journal_entries
             WHERE cadence = 'daily'
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// Return the active baseline's compact, locally-derived evidence pack.
    /// This never reads broker payloads or alters an experiment/baseline; it
    /// only joins already persisted local observations for operator review.
    pub async fn hermes_baseline_evidence_pack(&self, baseline: &JsonValue) -> Result<JsonValue> {
        if baseline.is_null() {
            return Ok(hermes_baseline_evidence_pack_from_snapshot(
                baseline,
                &JsonValue::Null,
                &[],
                &[],
                &[],
            ));
        }
        let config = baseline.get("config_json").unwrap_or(&JsonValue::Null);
        let experiment_id = json_text(config, "source_experiment_id");
        if experiment_id.is_empty() {
            return Ok(hermes_baseline_evidence_pack_from_snapshot(
                baseline,
                &JsonValue::Null,
                &[],
                &[],
                &[],
            ));
        }
        let experiment = self
            .first_json(&format!(
                "SELECT id, created_at, status, changed_variable_path
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(&experiment_id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        if experiment.is_null() {
            return Ok(hermes_baseline_evidence_pack_from_snapshot(
                baseline,
                &experiment,
                &[],
                &[],
                &[],
            ));
        }
        let experiment_created_at = json_text(&experiment, "created_at");
        let manager_runs = self
            .select_json(&format!(
                "SELECT id, created_at, report_id, status, manager_json
                 FROM trading_manager_runs
                 WHERE created_at >= '{}'
                 ORDER BY created_at ASC, id ASC
                 LIMIT 500",
                sql_escape(&experiment_created_at)
            ))
            .await?;
        let orders = self
            .select_json(&format!(
                "SELECT id, created_at, report_id, status, action
                 FROM execution_orders
                 WHERE created_at >= '{}'
                 ORDER BY created_at ASC, id ASC
                 LIMIT 1000",
                sql_escape(&experiment_created_at)
            ))
            .await?;
        let portfolio_history = self
            .select_json(&format!(
                "SELECT recorded_at, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk
                 FROM portfolio_value_history
                 WHERE recorded_at >= '{}'
                 ORDER BY recorded_at ASC, id ASC
                 LIMIT 1000",
                sql_escape(&experiment_created_at)
            ))
            .await?;
        Ok(hermes_baseline_evidence_pack_from_snapshot(
            baseline,
            &experiment,
            &manager_runs,
            &orders,
            &portfolio_history,
        ))
    }

    /// A read-only view of the promoted baseline artifact and the exact
    /// experiment overlay the Trading Manager will consider. It deliberately
    /// uses the manager's selection helper instead of a dashboard-local
    /// allowlist, so operator visibility follows runtime behavior.
    pub async fn hermes_one_variable_audit(&self) -> Result<Vec<JsonValue>> {
        let baseline = self.active_strategy_baseline().await?;
        let overlay_audit = crate::trading_manager::strategy_experiment_overlay_audit(self)
            .await
            .context("loading Hermes one-variable overlay audit")?;
        // Avoid `latest_trading_manager_run` here because it may repair legacy
        // advice metadata. This audit must remain a query-only dashboard read.
        let latest_manager_run = self
            .first_json(
                "SELECT created_at, manager_json
                 FROM trading_manager_runs
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(hermes_one_variable_audit_from_snapshot(
            &baseline,
            &overlay_audit,
            &latest_manager_run,
        ))
    }

    pub async fn hermes_execution_failures(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_id, symbol, action, order_type, mode, status, adapter, quantity, currency, estimated_value_dkk, approval_required, strategy_type, strategy_session, strategy_key, strategy_role, error_text, execution_result_json
             FROM execution_orders
             WHERE error_text IS NOT NULL OR lower(status) LIKE '%failed%' OR lower(status) LIKE '%error%' OR lower(status) LIKE '%rejected%'
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_reflections(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, period_start, period_end, goal_version, summary, findings_json, proposed_actions_json, source_session_id, raw_payload_json
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_reflection_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, period_start, period_end, goal_version, summary, source_session_id
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_lessons_pending_review(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, period_start, period_end, goal_version, summary, proposed_actions_json, source_session_id
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            LESSONS_PENDING_REVIEW_REFLECTION_LIMIT
        );
        let reflections = self.select_json(&sql).await.unwrap_or_default();
        Ok(lessons_pending_review_from_reflections(
            &reflections,
            clamp_limit(limit, 1, LESSONS_PENDING_REVIEW_LIMIT as i64) as usize,
        ))
    }

    pub async fn hermes_learning_memory(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, proposed_actions_json, source_session_id
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            LEARNING_MEMORY_REFLECTION_LIMIT
        );
        let reflections = self.select_json(&sql).await.unwrap_or_default();
        Ok(learning_memory_from_reflections(
            &reflections,
            Utc::now(),
            clamp_limit(limit, 1, LEARNING_MEMORY_LIMIT as i64) as usize,
        ))
    }

    pub async fn record_hermes_reflection(
        &self,
        request: &HermesReflectionRequest,
    ) -> Result<JsonValue> {
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let id = runtime_id("hermes-reflection");
        let period_start = request.period_start.as_deref().unwrap_or("");
        let period_end = request.period_end.as_deref().unwrap_or("");
        let findings = request.findings.clone().unwrap_or_else(|| json!([]));
        let proposed_actions = request
            .proposed_actions
            .clone()
            .unwrap_or_else(|| json!([]));
        let raw_payload = request.raw_payload.clone().unwrap_or(JsonValue::Null);
        let sql = format!(
            "INSERT INTO hermes_reflections (
                id, created_at, period_start, period_end, goal_version, summary,
                findings_json, proposed_actions_json, source_session_id, raw_payload_json
            ) VALUES (
                '{}', '{}', '{}', '{}', {}, '{}', '{}', '{}', {}, '{}'
            )",
            sql_escape(&id),
            sql_escape(&created_at),
            sql_escape(period_start),
            sql_escape(period_end),
            request.goal_version.unwrap_or(1),
            sql_escape(request.summary.trim()),
            sql_escape(&serde_json::to_string(&findings)?),
            sql_escape(&serde_json::to_string(&proposed_actions)?),
            sql_optional_text(request.source_session_id.as_deref()),
            sql_escape(&serde_json::to_string(&raw_payload)?)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording Hermes reflection")?;
        Ok(self
            .first_json(&format!(
                "SELECT id, created_at, period_start, period_end, goal_version, summary, findings_json, proposed_actions_json, source_session_id, raw_payload_json
                 FROM hermes_reflections WHERE id = '{}' LIMIT 1",
                sql_escape(&id)
            ))
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn hermes_experiments(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
             FROM strategy_experiments
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn hermes_experiment_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, status, baseline_id, goal_version, changed_variable_path, source_session_id
             FROM strategy_experiments
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// Return the only experiment rows that can be used as Hermes advisory
    /// context. Pending proposals are intentionally omitted with their values
    /// redacted: seeing a proposed threshold or setting is enough for an LLM
    /// to let it influence an allow/reduce/block recommendation even when the
    /// proposal has never passed operator review.
    pub async fn hermes_advisory_experiments(&self, limit: i64) -> Result<JsonValue> {
        let statuses = HERMES_ADVISORY_EXPERIMENT_STATUSES
            .iter()
            .map(|status| format!("'{}'", sql_escape(status)))
            .collect::<Vec<_>>()
            .join(", ");
        let pending_review_excluded_count = self
            .first_json(
                "SELECT COUNT(*) AS count
                 FROM strategy_experiments
                 WHERE status = 'pending_review'",
            )
            .await?
            .and_then(|row| row.get("count").and_then(JsonValue::as_i64))
            .unwrap_or(0)
            .max(0);
        let sql = format!(
            "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
             FROM strategy_experiments
             WHERE status IN ({statuses})
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(json!({
            "items": self.select_json(&sql).await.unwrap_or_default(),
            "pending_review_excluded_count": pending_review_excluded_count,
            "pending_review_values_included": false,
        }))
    }

    /// Closes unreviewed Hermes proposals after a deliberately longer period
    /// than the alert threshold. This only changes pending review records; it
    /// never changes an approved experiment, a baseline, configuration, or a
    /// broker-facing path.
    pub async fn expire_stale_hermes_experiments(&self) -> Result<JsonValue> {
        let enabled = yaml_bool(
            &self.config,
            &[
                "hermes",
                "experiments",
                "auto_expire_pending_review_enabled",
            ],
        )
        .unwrap_or(true);
        let stale_after_days = yaml_i64(
            &self.config,
            &["hermes", "experiments", "auto_expire_pending_review_days"],
        )
        .unwrap_or(30)
        .max(1);
        if !enabled {
            return Ok(json!({
                "status": "disabled",
                "expired_count": 0,
                "stale_after_days": stale_after_days,
            }));
        }
        self.expire_stale_hermes_experiments_at(Utc::now(), stale_after_days)
            .await
    }

    pub(crate) async fn expire_stale_hermes_experiments_at(
        &self,
        now: DateTime<Utc>,
        stale_after_days: i64,
    ) -> Result<JsonValue> {
        let stale_after_days = stale_after_days.max(1);
        let cutoff = (now - Duration::days(stale_after_days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let experiments = self
            .select_json(&format!(
                "SELECT id, created_at, changed_variable_path
                 FROM strategy_experiments
                 WHERE status = 'pending_review'
                   AND created_at <= '{}'
                 ORDER BY created_at ASC, id ASC",
                sql_escape(&cutoff)
            ))
            .await?;
        let recorded_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut expired = Vec::new();
        for experiment in experiments {
            let id = json_text(&experiment, "id");
            if id.is_empty() {
                continue;
            }
            let approval = json!({
                "action": "expire_stale",
                "from_status": "pending_review",
                "to_status": "expired_stale",
                "actor": "scheduler",
                "notes": "Proposal exceeded the configured pending-review window and was closed without activation.",
                "recorded_at": recorded_at,
                "stale_after_days": stale_after_days,
            });
            let result = sqlx::query(&format!(
                "UPDATE strategy_experiments
                 SET status = 'expired_stale', approval_json = '{}'
                 WHERE id = '{}'
                   AND status = 'pending_review'",
                sql_escape(&serde_json::to_string(&approval)?),
                sql_escape(&id)
            ))
            .execute(&self.pool)
            .await
            .context("expiring stale Hermes experiment proposal")?;
            if result.rows_affected() == 1 {
                expired.push(json!({
                    "id": id,
                    "created_at": experiment.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "changed_variable_path": experiment.get("changed_variable_path").cloned().unwrap_or(JsonValue::Null),
                }));
            }
        }
        Ok(json!({
            "status": "ok",
            "stale_after_days": stale_after_days,
            "cutoff": cutoff,
            "expired_count": expired.len(),
            "expired": expired,
        }))
    }

    pub async fn record_hermes_experiment(
        &self,
        request: &HermesExperimentRequest,
    ) -> Result<JsonValue> {
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let id = runtime_id("strategy-experiment");
        let evidence = request.evidence.clone().unwrap_or_else(|| json!({}));
        let raw_payload = request.raw_payload.clone().unwrap_or(JsonValue::Null);
        let sql = format!(
            "INSERT INTO strategy_experiments (
                id, created_at, status, baseline_id, goal_version, hypothesis,
                changed_variable_path, old_value_json, new_value_json, expected_effect,
                risk_notes, evidence_json, approval_json, metrics_json, source_session_id,
                raw_payload_json
            ) VALUES (
                '{}', '{}', 'pending_review', {}, {}, '{}',
                '{}', '{}', '{}', '{}',
                '{}', '{}', NULL, NULL, {}, '{}'
            )",
            sql_escape(&id),
            sql_escape(&created_at),
            sql_optional_text(request.baseline_id.as_deref()),
            request.goal_version.unwrap_or(1),
            sql_escape(request.hypothesis.trim()),
            sql_escape(request.changed_variable_path.trim()),
            sql_escape(&serde_json::to_string(&request.old_value)?),
            sql_escape(&serde_json::to_string(&request.new_value)?),
            sql_escape(request.expected_effect.trim()),
            sql_escape(request.risk_notes.as_deref().unwrap_or("")),
            sql_escape(&serde_json::to_string(&evidence)?),
            sql_optional_text(request.source_session_id.as_deref()),
            sql_escape(&serde_json::to_string(&raw_payload)?)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording Hermes strategy experiment")?;
        Ok(self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(&id)
            ))
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn find_duplicate_hermes_experiment(
        &self,
        changed_variable_path: &str,
    ) -> Result<Option<JsonValue>> {
        let changed_variable_path =
            normalize_hermes_experiment_variable_path(changed_variable_path);
        if changed_variable_path.is_empty() {
            return Ok(None);
        }
        self.first_json(&format!(
            "SELECT id, created_at, status, changed_variable_path, hypothesis, source_session_id
             FROM strategy_experiments
             WHERE LOWER(changed_variable_path) = LOWER('{}')
               AND status IN ({})
             ORDER BY created_at DESC, id DESC
            LIMIT 1",
            sql_escape(&changed_variable_path),
            hermes_experiment_duplicate_blocking_statuses_sql()
        ))
        .await
    }

    /// Inspect a proposal before inserting it through either the protected HTTP
    /// adapter or the Hermes MCP tool. Exact path matches are blocking; related
    /// variable families are an operator-review signal only.
    pub async fn inspect_hermes_experiment_proposal(
        &self,
        changed_variable_path: &str,
    ) -> Result<JsonValue> {
        let normalized_changed_variable_path =
            normalize_hermes_experiment_variable_path(changed_variable_path);
        let exact_duplicate = self
            .find_duplicate_hermes_experiment(&normalized_changed_variable_path)
            .await?;
        let review_family = hermes_experiment_review_family(&normalized_changed_variable_path);
        let related_active_or_pending_experiments = match review_family {
            Some(review_family) => self
                .select_json(&format!(
                    "SELECT id, created_at, status, changed_variable_path, hypothesis, source_session_id
                     FROM strategy_experiments
                     WHERE status IN ({})
                       AND LOWER(changed_variable_path) <> LOWER('{}')
                     ORDER BY created_at DESC, id DESC
                     LIMIT 100",
                    hermes_experiment_duplicate_blocking_statuses_sql(),
                    sql_escape(&normalized_changed_variable_path),
                ))
                .await?
                .into_iter()
                .filter(|experiment| {
                    hermes_experiment_review_family(&json_text(
                        experiment,
                        "changed_variable_path",
                    )) == Some(review_family)
                })
                .take(10)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        Ok(json!({
            "normalized_changed_variable_path": normalized_changed_variable_path,
            "exact_duplicate": exact_duplicate,
            "review_family": review_family,
            "related_active_or_pending_experiments": related_active_or_pending_experiments,
            "related_family_is_advisory": true,
        }))
    }

    pub async fn hermes_decision_advice_by_session(
        &self,
        source_session_id: &str,
    ) -> Result<Option<JsonValue>> {
        if source_session_id.trim().is_empty() {
            return Ok(None);
        }
        self.first_json(&format!(
            "SELECT id, created_at, decision_report_id, status, source_session_id,
                    overall_recommendation, summary, order_advice_json,
                    learning_notes_json, raw_payload_json
             FROM hermes_decision_advice
             WHERE source_session_id = '{}'
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            sql_escape(source_session_id)
        ))
        .await
    }

    pub async fn hermes_decision_advice_by_report(
        &self,
        decision_report_id: i64,
    ) -> Result<Option<JsonValue>> {
        if decision_report_id <= 0 {
            return Ok(None);
        }
        self.first_json(&format!(
            "SELECT id, created_at, decision_report_id, status, source_session_id,
                    overall_recommendation, summary, order_advice_json,
                    learning_notes_json, raw_payload_json
             FROM hermes_decision_advice
             WHERE decision_report_id = {}
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            decision_report_id
        ))
        .await
    }

    pub async fn record_hermes_decision_advice(
        &self,
        request: &HermesDecisionAdviceRequest,
    ) -> Result<JsonValue> {
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let id = runtime_id("hermes-decision-advice");
        let source_session_id = request
            .source_session_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        let recommendation = request.overall_recommendation.trim().to_lowercase();
        if !matches!(recommendation.as_str(), "proceed" | "stand_down" | "review") {
            bail!("invalid Hermes decision advice recommendation: {recommendation}");
        }
        let order_advice = request.order_advice.clone().unwrap_or_else(|| json!([]));
        let learning_notes = request.learning_notes.clone().unwrap_or_else(|| json!([]));
        let mut raw_payload = request.raw_payload.clone().unwrap_or_else(|| json!({}));
        if let Some(context_self_check) = request.context_self_check.clone() {
            let normalized = normalize_hermes_context_self_check(context_self_check);
            if let Some(raw) = raw_payload.as_object_mut() {
                raw.insert("context_self_check".to_string(), normalized);
            } else {
                raw_payload = json!({
                    "raw_payload": raw_payload,
                    "context_self_check": normalized
                });
            }
        } else if let Some(context_self_check) = raw_payload.get("context_self_check").cloned() {
            let normalized = normalize_hermes_context_self_check(context_self_check);
            if let Some(raw) = raw_payload.as_object_mut() {
                raw.insert("context_self_check".to_string(), normalized);
            }
        }
        // Validate refresh requests at the point of record so what is stored is
        // already the allowlisted set, with every refusal kept for audit.
        let requested = request
            .data_requests
            .clone()
            .or_else(|| raw_payload.get("data_requests").cloned());
        let (honored, rejected) =
            crate::hermes_data_requests::normalize_hermes_data_requests(requested.as_ref());
        if !honored.is_empty() || !rejected.is_empty() {
            let normalized = json!({
                "honored": honored
                    .iter()
                    .map(|item| json!({
                        "source": item.source,
                        "symbols": item.symbols,
                        "reason": item.reason,
                    }))
                    .collect::<Vec<_>>(),
                "rejected": rejected,
            });
            if let Some(raw) = raw_payload.as_object_mut() {
                raw.insert("data_requests".to_string(), normalized);
            } else {
                raw_payload = json!({
                    "raw_payload": raw_payload,
                    "data_requests": normalized,
                });
            }
        }
        let sql = format!(
            "INSERT INTO hermes_decision_advice (
                id, created_at, decision_report_id, status, source_session_id,
                overall_recommendation, summary, order_advice_json,
                learning_notes_json, raw_payload_json
            ) VALUES (
                '{}', '{}', {}, 'received', {}, '{}', '{}', '{}', '{}', '{}'
            )",
            sql_escape(&id),
            sql_escape(&created_at),
            request.decision_report_id,
            sql_optional_text(Some(&source_session_id)),
            sql_escape(&recommendation),
            sql_escape(request.summary.trim()),
            sql_escape(&serde_json::to_string(&order_advice)?),
            sql_escape(&serde_json::to_string(&learning_notes)?),
            sql_escape(&serde_json::to_string(&raw_payload)?)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording Hermes decision advice")?;
        Ok(self
            .first_json(&format!(
                "SELECT id, created_at, decision_report_id, status, source_session_id,
                        overall_recommendation, summary, order_advice_json,
                        learning_notes_json, raw_payload_json
                 FROM hermes_decision_advice WHERE id = '{}' LIMIT 1",
                sql_escape(&id)
            ))
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn transition_hermes_experiment(
        &self,
        experiment_id: &str,
        action: &str,
        notes: Option<&str>,
        actor: &str,
    ) -> Result<JsonValue> {
        let experiment_id = experiment_id.trim();
        if experiment_id.is_empty() {
            bail!("experiment id is required");
        }
        let experiment = self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(experiment_id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        if experiment.is_null() {
            bail!("Hermes experiment not found: {experiment_id}");
        }
        let current_status = json_text(&experiment, "status");
        let next_status =
            hermes_experiment_next_status(&current_status, action).with_context(|| {
                format!("invalid Hermes experiment transition {current_status} -> {action}")
            })?;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut baseline_id = None;
        if next_status == "promoted" {
            baseline_id = Some(
                self.promote_hermes_experiment_baseline(&experiment, &now)
                    .await?,
            );
        }
        let approval = json!({
            "action": action.trim(),
            "from_status": current_status,
            "to_status": next_status,
            "actor": actor,
            "notes": notes.unwrap_or("").trim(),
            "recorded_at": now,
            "baseline_id": baseline_id
        });
        sqlx::query(&format!(
            "UPDATE strategy_experiments
             SET status = '{}', approval_json = '{}'
             WHERE id = '{}'",
            sql_escape(next_status),
            sql_escape(&serde_json::to_string(&approval)?),
            sql_escape(experiment_id)
        ))
        .execute(&self.pool)
        .await
        .context("updating Hermes experiment transition")?;

        let updated = self
            .first_json(&format!(
                "SELECT id, created_at, status, baseline_id, goal_version, hypothesis, changed_variable_path, old_value_json, new_value_json, expected_effect, risk_notes, evidence_json, approval_json, metrics_json, source_session_id, raw_payload_json
                 FROM strategy_experiments WHERE id = '{}' LIMIT 1",
                sql_escape(experiment_id)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(json!({
            "status": "ok",
            "experiment": updated,
            "transition": approval
        }))
    }

    async fn promote_hermes_experiment_baseline(
        &self,
        experiment: &JsonValue,
        activated_at: &str,
    ) -> Result<String> {
        let baseline_id = runtime_id("strategy-baseline");
        let config_json = json!({
            "source_experiment_id": json_text(experiment, "id"),
            "goal_version": experiment.get("goal_version").cloned().unwrap_or_else(|| json!(1)),
            "changed_variable_path": json_text(experiment, "changed_variable_path"),
            "old_value": experiment.get("old_value_json").cloned().unwrap_or(JsonValue::Null),
            "new_value": experiment.get("new_value_json").cloned().unwrap_or(JsonValue::Null),
            "hypothesis": json_text(experiment, "hypothesis"),
            "expected_effect": json_text(experiment, "expected_effect"),
            "risk_notes": json_text(experiment, "risk_notes"),
            "scope": "baseline_record_only",
            "live_activation": false
        });
        let prompt_json = json!({
            "source": "hermes_experiment_promotion",
            "raw_payload": experiment.get("raw_payload_json").cloned().unwrap_or(JsonValue::Null)
        });
        sqlx::query("UPDATE strategy_baselines SET status = 'superseded' WHERE status = 'active'")
            .execute(&self.pool)
            .await
            .context("superseding prior strategy baselines")?;
        sqlx::query(&format!(
            "INSERT INTO strategy_baselines (
                id, created_at, activated_at, status, goal_version, config_json, prompt_json, source
            ) VALUES (
                '{}', '{}', '{}', 'active', {}, '{}', '{}', '{}'
            )",
            sql_escape(&baseline_id),
            sql_escape(activated_at),
            sql_escape(activated_at),
            experiment
                .get("goal_version")
                .and_then(JsonValue::as_i64)
                .unwrap_or(1),
            sql_escape(&serde_json::to_string(&config_json)?),
            sql_escape(&serde_json::to_string(&prompt_json)?),
            sql_escape(&format!(
                "hermes_experiment:{}",
                json_text(experiment, "id")
            ))
        ))
        .execute(&self.pool)
        .await
        .context("creating promoted Hermes strategy baseline")?;
        Ok(baseline_id)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn learning_memory_makes_repeated_lessons_stable_and_redacts_sensitive_text() {
        let reflections = vec![
            json!({
                "id": "daily-1",
                "created_at": "2026-07-30T17:00:00Z",
                "source_session_id": "daily-reflection",
                "proposed_actions_json": {"actions": [{"action": "Review fills before changing risk."}]},
            }),
            json!({
                "id": "weekly-1",
                "created_at": "2026-07-31T17:00:00Z",
                "source_session_id": "weekly-reflection",
                "proposed_actions_json": {"actions": [
                    {"action": "Review fills before changing risk."},
                    {"action": "Bearer not-a-real-token"}
                ]},
            }),
        ];
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).single().unwrap();
        let memory = learning_memory_from_reflections(&reflections, now, 10);
        assert_eq!(memory[0]["status"], "stable");
        assert_eq!(memory[0]["observation_count"], 2);
        assert!(
            memory.iter().any(|row| {
                row["lesson"] == "[redacted potentially sensitive reflection action]"
            })
        );
    }

    #[test]
    fn pending_review_experiment_is_not_advisory_eligible() {
        assert!(!hermes_experiment_status_is_advisory_eligible(
            "pending_review"
        ));
        for status in HERMES_ADVISORY_EXPERIMENT_STATUSES {
            assert!(hermes_experiment_status_is_advisory_eligible(status));
        }
        assert!(!hermes_experiment_status_is_advisory_eligible("rejected"));
    }

    #[test]
    fn decision_outcomes_are_normalized_and_exclude_provider_and_broker_documents() {
        let outcomes = hermes_decision_report_outcomes_from_rows(vec![json!({
            "report_id": 42,
            "created_at": "2026-08-31T08:30:00Z",
            "report_date": "2026-08-31",
            "report_status": "completed",
            "analysis_pulse_key": "us_open_followup:2026-08-31",
            "analysis_pulse_label": "US opening follow-up",
            "pulse_mode": "execution_eligible",
            "queue_eligible": 1,
            "report_json": {
                "decision_quality": {
                    "status": "ready",
                    "score": 100,
                    "warning_count": 0,
                    "candidate_count": 2
                },
                "provider_rationale": "must-not-reach-hermes",
                "raw_broker_document": {"AccountKey": "must-not-reach-hermes"}
            },
            "manager_status": "completed",
            "execution_order_count": 2,
            "pending_execution_count": 0,
            "broker_working_count": 1,
            "partial_fill_count": 0,
            "filled_count": 1,
            "expired_count": 0,
            "cancelled_count": 0,
            "failed_or_rejected_count": 0,
            "realised_sell_count": 1,
            "realised_sell_gain_dkk": 125.5
        })]);

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].decision_quality_status, "ready");
        assert_eq!(outcomes[0].filled_count, 1);
        assert_eq!(outcomes[0].realised_sell_gain_dkk, 125.5);
        let serialized = serde_json::to_string(&outcomes).expect("outcomes serialize");
        assert!(!serialized.contains("must-not-reach-hermes"));
    }

    #[test]
    fn the_mcp_report_view_names_each_candidate_not_just_a_count() {
        // Regression for the blanket review hold on 2026-08-31: Hermes is asked
        // for per-order advice, found only `candidate_count` over MCP, reported
        // that candidate symbols were not exposed, and zeroed BMW, ALV and VWS.
        let report = json!({
            "decision_quality": {"status": "ready", "candidate_count": 2},
            "suggested_trades": [
                {
                    "action": "BUY",
                    "symbol": "BMW:xetr",
                    "quantity": 20,
                    "order_type": "Limit",
                    "strategy_key": "manual:2026-08-31T08:25:17Z:BMW:xetr:BUY",
                    "strategy_role": "swing_entry",
                    "limit_price_local": 62.64,
                    "estimated_value_dkk": 9364.57,
                    "strategy_metadata": {"provider_notes": "must-not-reach-hermes"}
                },
                {
                    "action": "BUY",
                    "symbol": "ALV:xetr",
                    "quantity": 3,
                    "order_type": "Limit",
                    "strategy_key": "manual:2026-08-31T08:25:17Z:ALV:xetr:BUY",
                    "strategy_role": "swing_entry",
                    "limit_price_local": 388.1,
                    "estimated_value_dkk": 8687.0
                }
            ]
        });
        let candidates = hermes_decision_candidates_from_report(&report);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["BMW:xetr", "ALV:xetr"]
        );
        assert_eq!(candidates[0].quantity, 20.0);
        assert_eq!(candidates[0].limit_price_local, Some(62.64));
        assert_eq!(
            candidates[0].strategy_key, "manual:2026-08-31T08:25:17Z:BMW:xetr:BUY",
            "advice has to be keyable back to the exact candidate"
        );

        // strategy_metadata is projected away, not passed through.
        let serialized = serde_json::to_string(&candidates).expect("candidates serialize");
        assert!(!serialized.contains("must-not-reach-hermes"));
        assert!(!serialized.contains("strategy_metadata"));
    }

    #[test]
    fn candidates_without_a_symbol_are_dropped_and_the_list_is_bounded() {
        let mut trades = vec![json!({"action": "BUY", "quantity": 1})];
        trades.push(json!({"symbol": "   ", "action": "BUY"}));
        for index in 0..40 {
            trades.push(json!({"symbol": format!("SYM{index}:xetr"), "action": "BUY"}));
        }
        let candidates =
            hermes_decision_candidates_from_report(&json!({"suggested_trades": trades}));

        assert_eq!(candidates.len(), HERMES_MAX_REPORT_CANDIDATES);
        assert!(
            candidates.iter().all(|c| !c.symbol.trim().is_empty()),
            "an unkeyable candidate is worse than an absent one"
        );
    }

    #[test]
    fn a_report_without_suggested_trades_yields_no_candidates() {
        assert!(hermes_decision_candidates_from_report(&json!({})).is_empty());
        assert!(
            hermes_decision_candidates_from_report(&json!({"suggested_trades": "not-an-array"}))
                .is_empty()
        );
    }

    /// Protected Hermes list rows are assembled from audit records that carry
    /// nulls for an experiment without a baseline or a reflection without a
    /// session, so a null must not fail the list.
    #[test]
    fn an_explicit_null_never_blanks_a_hermes_list() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "id": "reflection-91",
                "created_at": "2026-08-31T18:00:00Z",
                "period_start": "2026-08-24",
                "period_end": "2026-08-31",
                "goal_version": 2,
                "summary": "Weekly reflection",
                "source_session_id": null
            }]),
            |value| {
                hermes_reflection_summaries_from_json(
                    value.as_array().cloned().expect("fixture is a list"),
                )
            },
        );

        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "id": "experiment-14",
                "created_at": "2026-08-31T18:05:00Z",
                "status": "pending_review",
                "baseline_id": null,
                "goal_version": 2,
                "changed_variable_path": "strategy.markov.min_signed_signal",
                "source_session_id": null
            }]),
            |value| {
                hermes_experiment_summaries_from_json(
                    value.as_array().cloned().expect("fixture is a list"),
                )
            },
        );
    }

    fn gate_config() -> serde_yaml::Value {
        serde_yaml::from_str(
            "strategy:\n  swing:\n    markov_gate:\n      min_signed_signal: 0.20\n",
        )
        .expect("config")
    }

    /// The 2026-09-18 proposal: 0.15 -> 0.20 for a gate already at 0.20.
    #[test]
    fn a_proposal_from_a_stale_value_is_refused_with_the_running_value() {
        let path = "strategy.swing.markov_gate.min_signed_signal";
        let problem = hermes_experiment_starting_point_problem(
            &gate_config(),
            path,
            &json!("0.15"),
            &json!("0.20"),
        )
        .expect("refused");
        assert_eq!(problem["status"], "stale_starting_point");
        assert_eq!(problem["running_value"], 0.2);
    }

    #[test]
    fn a_proposal_of_the_running_value_is_refused_as_no_change() {
        let path = "strategy.swing.markov_gate.min_signed_signal";
        let problem = hermes_experiment_starting_point_problem(
            &gate_config(),
            path,
            &json!(0.2),
            &json!("0.20"),
        )
        .expect("refused");
        assert_eq!(problem["status"], "no_change");
    }

    #[test]
    fn a_proposal_from_the_running_value_is_accepted_in_either_form() {
        let path = " Strategy.Swing.Markov_Gate.Min_Signed_Signal ";
        for old in [json!(0.2), json!("0.20"), json!("0.2")] {
            assert!(
                hermes_experiment_starting_point_problem(&gate_config(), path, &old, &json!(0.25))
                    .is_none(),
                "{old}"
            );
        }
        let problem = hermes_experiment_starting_point_problem(
            &gate_config(),
            "strategy.swing.no_such_key",
            &json!(1),
            &json!(2),
        )
        .expect("refused");
        assert_eq!(problem["status"], "unknown_variable");
    }

    /// Every variable Hermes may propose has a value in both shipped configs,
    /// so the check can never refuse a supported variable as unknown.
    #[test]
    fn every_supported_experiment_variable_has_a_running_value() {
        for file in ["config.yaml", "deploy/k8s/base/config.k8s.yaml"] {
            let config: serde_yaml::Value =
                serde_yaml::from_str(&std::fs::read_to_string(file).expect("config"))
                    .expect("yaml");
            for path in crate::state::SUPPORTED_EXPERIMENT_VARIABLES {
                let problem = hermes_experiment_starting_point_problem(
                    &config,
                    path,
                    &json!(null),
                    &json!(null),
                );
                assert_ne!(
                    problem.as_ref().map(|value| value["status"].clone()),
                    Some(json!("unknown_variable")),
                    "{file}: {path}"
                );
            }
        }
    }
}
