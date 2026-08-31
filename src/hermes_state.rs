//! Read-only Hermes dashboard and evidence projections.
//!
//! This module intentionally contains only deterministic transformations of
//! persisted reflection, experiment, manager, order, and portfolio records. It
//! cannot create experiments, alter runtime configuration, invoke a provider,
//! or reach Saxo.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};

use crate::{
    db::{value_f64, value_i64},
    models::{
        HermesDecisionCandidatePayload, HermesDecisionReportOutcomePayload,
        HermesExperimentSummaryPayload, HermesReflectionSummaryPayload,
    },
    state::json_text,
};

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
}
