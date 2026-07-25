use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    process,
    sync::{OnceLock, RwLock},
    time::Duration as StdDuration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use tokio::time::sleep;
use tracing::{error, info, warn};
use url::Url;

use crate::{
    auth,
    config::{database_url, yaml_bool, yaml_f64, yaml_i64, yaml_string},
    db::{clamp_limit, json_f64, json_i64, pct, row_to_json, sql_escape, value_f64, value_i64},
    localization::LocalizationPrefs,
    models::{
        DashboardView, HermesDecisionAdviceRequest, HermesExperimentRequest,
        HermesReflectionRequest,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
    pub config: YamlValue,
    pub db_url: String,
    pub pool: AnyPool,
}

static SAXO_EXCHANGE_CALENDAR_CACHE: OnceLock<RwLock<Option<SaxoExchangeCalendarCache>>> =
    OnceLock::new();

const SAXO_SESSION_REFRESH_LEASE_SECONDS: i64 = 45;
const SAXO_SESSION_REFRESH_LEASE_WAIT_ATTEMPTS: usize = 50;
const INTEGRITY_MONEY_ABS_TOLERANCE_DKK: f64 = 50.0;
const INTEGRITY_MONEY_REL_TOLERANCE: f64 = 0.002;
const INTEGRITY_BROKER_CASH_ABS_TOLERANCE_DKK: f64 = 500.0;
const INTEGRITY_BROKER_CASH_REL_TOLERANCE: f64 = 0.05;
const INTEGRITY_BROKER_EXPOSURE_ABS_TOLERANCE_DKK: f64 = 1_000.0;
const INTEGRITY_BROKER_EXPOSURE_REL_TOLERANCE: f64 = 0.10;
const INTEGRITY_BROKER_QUANTITY_ABS_TOLERANCE: f64 = 1e-6;
const INTEGRITY_IMPLAUSIBLE_UNIT_COST_DKK: f64 = 100_000.0;
const DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES: i64 = 10;
const DECISION_REPORT_SUMMARY_COLUMNS: &str = "id, created_at, report_date, model, status, analysis_window_active, response_id, error_text, analysis_pulse_key, analysis_pulse_label";
const DECISION_REPORT_DETAIL_COLUMNS: &str = "id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label";
const DEFAULT_SCHEDULER_HISTORY_MAX_ROWS: i64 = 250;
const DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS: i64 = 30;
const DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS: i64 = 7;
const RETIRED_RUNTIME_SETTING_KEYS: &[&str] = &["strategy.capital.cash_buffer"];
const HERMES_LESSONS_PENDING_REVIEW_REFLECTION_LIMIT: i64 = 50;
const HERMES_LESSONS_PENDING_REVIEW_LIMIT: usize = 30;
const HERMES_LESSON_TEXT_MAX_CHARS: usize = 500;
const HERMES_LEARNING_MEMORY_REFLECTION_LIMIT: i64 = 80;
const HERMES_LEARNING_MEMORY_LIMIT: usize = 30;
const HERMES_LEARNING_MEMORY_EMERGING_TTL_DAYS: i64 = 7;
const HERMES_LEARNING_MEMORY_STABLE_TTL_DAYS: i64 = 21;
const HERMES_LEARNING_MEMORY_STABLE_MIN_REFLECTIONS: usize = 2;
const GATE_REPLAY_DEFAULT_RUN_LIMIT: i64 = 40;
const GATE_REPLAY_MAX_CHANGE_ROWS: usize = 30;
const GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL: f64 = 0.25;
const GATE_REPLAY_MIN_CONFLUENCES: i64 = 4;
const SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS: i64 = 180;
const SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS: usize = 30;
const SUPPORT_RISK_LABELS: [&str; 3] = ["low", "moderate", "high"];

#[derive(Clone, Debug)]
struct SaxoExchangeCalendarCache {
    checked_date: NaiveDate,
    checked_at: DateTime<Utc>,
    exchanges: HashMap<String, SaxoExchangeCalendar>,
    source: String,
}

#[derive(Clone, Debug)]
struct HermesLearningMemoryEntry {
    lesson: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    reflection_ids: HashSet<String>,
    cadences: HashSet<String>,
}

#[derive(Clone, Debug)]
struct SaxoExchangeCalendar {
    exchange_id: String,
    name: Option<String>,
    timezone_id: Option<String>,
    sessions: Vec<SaxoExchangeSession>,
}

#[derive(Clone, Debug)]
struct SaxoExchangeSession {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    state: String,
}

#[derive(Clone, Debug)]
struct ExchangeDaySession {
    open_at: DateTime<Utc>,
    close_at: DateTime<Utc>,
}

fn redacted_database_url(value: &str) -> String {
    // This value reaches both logs and the operator dashboard. Render only
    // connection topology, never URL user-info, query parameters, or a local
    // filesystem path. A structured URL parser avoids fragile string masking.
    let Ok(url) = Url::parse(value) else {
        return "Configured database".to_string();
    };
    match url.scheme() {
        "postgres" | "postgresql" => {
            let host = url.host_str().unwrap_or("configured host");
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            let database = url.path().trim_matches('/');
            if database.is_empty() {
                format!("PostgreSQL · {host}{port}")
            } else {
                format!("PostgreSQL · {host}{port}/{database}")
            }
        }
        "sqlite" => "SQLite · local database".to_string(),
        scheme if !scheme.is_empty() => format!("{} database", scheme.to_ascii_uppercase()),
        _ => "Configured database".to_string(),
    }
}

fn runtime_id(prefix: &str) -> String {
    format!("{prefix}-{}", Utc::now().timestamp_micros())
}

fn sql_optional_text(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => format!("'{}'", sql_escape(value)),
        None => "NULL".to_string(),
    }
}

fn sql_f64(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "0".to_string()
    }
}

/// Convert reflection `proposed_actions` into a bounded, display-safe operator
/// queue. The rows are deliberately derived rather than persisted as a second
/// workflow: an item is advisory context, not an approved experiment or task.
fn hermes_lessons_pending_review_from_reflections(
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
        for (action_index, action) in hermes_proposed_action_entries(actions)
            .into_iter()
            .enumerate()
        {
            let Some(lesson) = hermes_proposed_action_text(action) else {
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
fn hermes_learning_memory_from_reflections(
    reflections: &[JsonValue],
    now: DateTime<Utc>,
    limit: usize,
) -> Vec<JsonValue> {
    let mut entries = HashMap::<String, HermesLearningMemoryEntry>::new();
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
        let cadence = hermes_reflection_cadence(reflection);
        let Some(actions) = reflection.get("proposed_actions_json") else {
            continue;
        };
        let mut actions_seen_in_reflection = HashSet::new();
        for action in hermes_proposed_action_entries(actions) {
            let Some(lesson) = hermes_proposed_action_text(action) else {
                continue;
            };
            let normalized = lesson.to_ascii_lowercase();
            if !actions_seen_in_reflection.insert(normalized.clone()) {
                continue;
            }
            let entry = entries
                .entry(normalized)
                .or_insert_with(|| HermesLearningMemoryEntry {
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
            let stable = observation_count >= HERMES_LEARNING_MEMORY_STABLE_MIN_REFLECTIONS;
            let ttl_days = if stable {
                HERMES_LEARNING_MEMORY_STABLE_TTL_DAYS
            } else {
                HERMES_LEARNING_MEMORY_EMERGING_TTL_DAYS
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

fn hermes_reflection_cadence(reflection: &JsonValue) -> String {
    let session = json_text(reflection, "source_session_id").to_ascii_lowercase();
    if session.contains("weekly") {
        "weekly".to_string()
    } else if session.contains("daily") {
        "daily".to_string()
    } else {
        "other".to_string()
    }
}

/// Produce a small, display-safe view of the one-variable state. Baselines
/// remain audit records and overlays remain runtime candidates; neither row
/// asserts that a persistent config rewrite or live activation occurred.
fn hermes_one_variable_audit_from_snapshot(
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
            "reason": hermes_safe_display_text(&json_text(config, "hypothesis"), 220),
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
            "reason": hermes_safe_display_text(&json_text(candidate, "hypothesis"), 220),
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
fn hermes_proposal_quality_from_experiments(experiments: &[JsonValue]) -> Vec<JsonValue> {
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
fn hermes_baseline_evidence_pack_from_snapshot(
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

fn hermes_proposed_action_entries(value: &JsonValue) -> Vec<&JsonValue> {
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

fn hermes_proposed_action_text(value: &JsonValue) -> Option<String> {
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
    if hermes_lesson_text_looks_sensitive(&normalized) {
        return Some("[redacted potentially sensitive reflection action]".to_string());
    }
    Some(hermes_safe_display_text(
        &normalized,
        HERMES_LESSON_TEXT_MAX_CHARS,
    ))
}

fn hermes_safe_display_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }
    if hermes_lesson_text_looks_sensitive(&normalized) {
        return "[redacted potentially sensitive Hermes text]".to_string();
    }
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

fn hermes_lesson_text_looks_sensitive(value: &str) -> bool {
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

fn hermes_counterfactual_shadow_quantity(
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

fn hermes_counterfactual_quote_metrics(
    action: &str,
    shadow_quantity: f64,
    reference_price_local: f64,
    latest_price_local: f64,
) -> Option<(f64, f64)> {
    if !shadow_quantity.is_finite()
        || shadow_quantity <= 0.0
        || !reference_price_local.is_finite()
        || reference_price_local <= 0.0
        || !latest_price_local.is_finite()
        || latest_price_local <= 0.0
    {
        return None;
    }
    let price_change = match action.trim().to_uppercase().as_str() {
        "BUY" => latest_price_local - reference_price_local,
        "SELL" => reference_price_local - latest_price_local,
        _ => return None,
    };
    Some((
        price_change / reference_price_local,
        shadow_quantity * price_change,
    ))
}

fn json_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn candidate_scoring_key(value: &JsonValue) -> String {
    let strategy_key = json_text(value, "strategy_key");
    if !strategy_key.trim().is_empty() {
        return format!("strategy:{strategy_key}");
    }
    format!(
        "order:{}:{}",
        json_text(value, "symbol"),
        json_text(value, "action").to_ascii_uppercase()
    )
}

fn candidate_gate_code_from_reason(reason: &str) -> &'static str {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.starts_with("hermes context") {
        "hermes_context"
    } else if normalized.starts_with("hermes advisory") {
        "hermes_advice"
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
    } else if normalized.starts_with("buy would exceed available cash budget") {
        "cash_budget"
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

fn candidate_gate_code(value: &JsonValue) -> String {
    let configured = json_text(value, "gate_code");
    if matches!(
        configured.as_str(),
        "approved"
            | "hermes_context"
            | "hermes_advice"
            | "market_open"
            | "risk_exclusion"
            | "instrument_quarantine"
            | "quantity"
            | "order_shape"
            | "monthly_loss_breaker"
            | "cash_budget"
            | "commission_floor"
            | "minimum_trade_value"
            | "sellable_quantity"
            | "markov"
            | "technical"
            | "other"
    ) {
        return configured;
    }
    candidate_gate_code_from_reason(&json_text(value, "technical_gate")).to_string()
}

fn compact_candidate_technical(value: &JsonValue) -> JsonValue {
    let technical = value.get("technical").unwrap_or(&JsonValue::Null);
    json!({
        "status": json_text(technical, "status"),
        "sentiment": json_text(technical, "sentiment"),
        "trend_bias": json_text(technical, "trend_bias"),
        "confluence_count": value_i64(technical, "confluence_count"),
        "min_confluences": value_i64(technical, "min_confluences"),
    })
}

fn compact_candidate_final_technical(value: &JsonValue) -> JsonValue {
    let technical = value.get("final_technical").unwrap_or(&JsonValue::Null);
    if technical.is_object() {
        return json!({
            "status": json_text(technical, "status"),
            "source": json_text(technical, "source"),
            "run_date": json_text(technical, "run_date"),
            "sentiment": json_text(technical, "sentiment"),
            "trend_bias": json_text(technical, "trend_bias"),
            "confluence_count": value_i64(technical, "confluence_count"),
            "min_confluences": value_i64(technical, "min_confluences"),
        });
    }
    legacy_final_technical_from_gate_reason(value).unwrap_or(JsonValue::Null)
}

/// Recover only the known, deterministic SELL gate result from pre-persistence
/// manager runs. This never exposes the stored reason text to the dashboard.
fn legacy_final_technical_from_gate_reason(value: &JsonValue) -> Option<JsonValue> {
    let reason = json_text(value, "technical_gate");
    let normalized = reason.trim().to_ascii_lowercase();
    let remainder = normalized.strip_prefix("sell not approved; technical sentiment is ")?;
    let (sentiment, trend) = remainder.split_once(" with ")?;
    let (trend, _) = trend.split_once(" trend.")?;
    let sentiment = match sentiment {
        "sell" => "SELL",
        "underweight" => "UNDERWEIGHT",
        "hold" => "HOLD",
        "overweight" => "OVERWEIGHT",
        "buy" => "BUY",
        _ => return None,
    };
    if !matches!(trend, "bullish" | "neutral" | "bearish") {
        return None;
    }
    Some(json!({
        "status": "ok",
        "source": "recorded_gate_reason",
        "run_date": "",
        "sentiment": sentiment,
        "trend_bias": trend,
        "confluence_count": 0,
        "min_confluences": 0,
    }))
}

fn compact_candidate_markov(value: &JsonValue) -> JsonValue {
    let markov = value.get("markov").unwrap_or(&JsonValue::Null);
    json!({
        "status": json_text(markov, "status"),
        "fresh": markov.get("fresh").and_then(JsonValue::as_bool).unwrap_or(false),
        "direction": json_text(markov, "direction"),
        "signed_signal": value_f64(markov, "signed_signal"),
        "age_days": value_i64(markov, "age_days"),
    })
}

fn candidate_recorded_technical(candidate: &JsonValue) -> &JsonValue {
    let final_technical = candidate.get("final_technical").unwrap_or(&JsonValue::Null);
    if json_text(final_technical, "status") == "ok" {
        final_technical
    } else {
        candidate.get("technical").unwrap_or(&JsonValue::Null)
    }
}

fn replay_buy_technical_passes(candidate: &JsonValue, min_confluences: i64) -> Option<bool> {
    if json_text(candidate, "action").to_ascii_uppercase() != "BUY" {
        return None;
    }
    let technical = candidate_recorded_technical(candidate);
    if json_text(technical, "status") != "ok" {
        return None;
    }
    let sentiment = json_text(technical, "sentiment").to_ascii_uppercase();
    let trend_bias = json_text(technical, "trend_bias").to_ascii_lowercase();
    Some(
        matches!(sentiment.as_str(), "BUY" | "OVERWEIGHT")
            && trend_bias == "bullish"
            && value_i64(technical, "confluence_count") >= min_confluences.max(1),
    )
}

fn replay_recorded_min_confluences(candidate: &JsonValue) -> Option<i64> {
    let minimum = value_i64(candidate_recorded_technical(candidate), "min_confluences");
    (minimum > 0).then_some(minimum)
}

fn replay_recorded_markov_minimum(run: &JsonValue) -> Option<f64> {
    run.get("manager_json")
        .and_then(|value| value.get("hermes_preflight"))
        .and_then(|value| value.get("markov"))
        .and_then(|value| value.get("min_signed_signal"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn gate_replay_change_row(
    run: &JsonValue,
    candidate: &JsonValue,
    effect: &str,
    recorded_value: JsonValue,
    proposed_value: JsonValue,
) -> JsonValue {
    json!({
        "manager_run_id": value_i64(run, "id"),
        "report_id": value_i64(run, "report_id"),
        "created_at": json_text(run, "created_at"),
        "symbol": json_text(candidate, "symbol"),
        "action": json_text(candidate, "action"),
        "recorded_outcome": json_text(candidate, "outcome"),
        "recorded_gate": json_text(candidate, "gate_code"),
        "effect": effect,
        "recorded_value": recorded_value,
        "proposed_value": proposed_value,
    })
}

fn gate_replay_markov_scenario(runs: &[JsonValue]) -> JsonValue {
    let mut candidate_count = 0usize;
    let mut evaluated_count = 0usize;
    let mut would_block_count = 0usize;
    let mut would_clear_target_gate_only_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut insufficient_evidence_count = 0usize;
    let mut changes = Vec::new();

    for run in runs {
        let waterfall = candidate_scoring_waterfall_from_manager_run(run);
        for candidate in waterfall
            .get("candidates")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            candidate_count += 1;
            if json_text(candidate, "action").to_ascii_uppercase() != "BUY" {
                not_reached_count += 1;
                continue;
            }
            let Some(recorded_minimum) = replay_recorded_markov_minimum(run) else {
                insufficient_evidence_count += 1;
                continue;
            };
            let Some(technical_passes) = replay_buy_technical_passes(
                candidate,
                replay_recorded_min_confluences(candidate).unwrap_or(1),
            ) else {
                insufficient_evidence_count += 1;
                continue;
            };
            if technical_passes {
                not_reached_count += 1;
                continue;
            }
            let markov = candidate.get("markov").unwrap_or(&JsonValue::Null);
            if json_text(markov, "status") != "ok"
                || !markov
                    .get("fresh")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                || json_text(markov, "direction") != "long"
            {
                insufficient_evidence_count += 1;
                continue;
            }
            evaluated_count += 1;
            let signal = value_f64(markov, "signed_signal");
            let recorded_passes = signal >= recorded_minimum;
            let proposed_passes = signal >= GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL;
            let effect = match (recorded_passes, proposed_passes) {
                (true, false) => "would_block_target_gate",
                (false, true) => "would_clear_target_gate_only",
                _ => "unchanged_target_gate",
            };
            match effect {
                "would_block_target_gate" => would_block_count += 1,
                "would_clear_target_gate_only" => would_clear_target_gate_only_count += 1,
                _ => unchanged_count += 1,
            }
            if effect != "unchanged_target_gate" && changes.len() < GATE_REPLAY_MAX_CHANGE_ROWS {
                changes.push(gate_replay_change_row(
                    run,
                    candidate,
                    effect,
                    json!({"min_signed_signal": recorded_minimum, "signed_signal": signal}),
                    json!({"min_signed_signal": GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL, "signed_signal": signal}),
                ));
            }
        }
    }

    json!({
        "variable_path": "strategy.swing.markov_gate.min_signed_signal",
        "proposed_value": GATE_REPLAY_MARKOV_MIN_SIGNED_SIGNAL,
        "comparison": "fresh long starter fallback only; historical technical-gate evidence must first reject the BUY",
        "summary": {
            "candidate_count": candidate_count,
            "evaluated_count": evaluated_count,
            "would_block_target_gate_count": would_block_count,
            "would_clear_target_gate_only_count": would_clear_target_gate_only_count,
            "unchanged_target_gate_count": unchanged_count,
            "not_reached_count": not_reached_count,
            "insufficient_evidence_count": insufficient_evidence_count,
        },
        "changes": changes,
    })
}

fn gate_replay_technical_scenario(runs: &[JsonValue]) -> JsonValue {
    let mut candidate_count = 0usize;
    let mut evaluated_count = 0usize;
    let mut would_block_count = 0usize;
    let mut would_clear_target_gate_only_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut insufficient_evidence_count = 0usize;
    let mut changes = Vec::new();

    for run in runs {
        let waterfall = candidate_scoring_waterfall_from_manager_run(run);
        for candidate in waterfall
            .get("candidates")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            candidate_count += 1;
            let Some(recorded_minimum) = replay_recorded_min_confluences(candidate) else {
                insufficient_evidence_count += 1;
                continue;
            };
            let Some(recorded_passes) = replay_buy_technical_passes(candidate, recorded_minimum)
            else {
                if json_text(candidate, "action").to_ascii_uppercase() == "BUY" {
                    insufficient_evidence_count += 1;
                } else {
                    not_reached_count += 1;
                }
                continue;
            };
            evaluated_count += 1;
            let proposed_passes =
                replay_buy_technical_passes(candidate, GATE_REPLAY_MIN_CONFLUENCES)
                    .unwrap_or(false);
            let effect = match (recorded_passes, proposed_passes) {
                (true, false) => "would_block_target_gate",
                (false, true) => "would_clear_target_gate_only",
                _ => "unchanged_target_gate",
            };
            match effect {
                "would_block_target_gate" => would_block_count += 1,
                "would_clear_target_gate_only" => would_clear_target_gate_only_count += 1,
                _ => unchanged_count += 1,
            }
            if effect != "unchanged_target_gate" && changes.len() < GATE_REPLAY_MAX_CHANGE_ROWS {
                changes.push(gate_replay_change_row(
                    run,
                    candidate,
                    effect,
                    json!({
                        "min_confluences": recorded_minimum,
                        "confluence_count": value_i64(candidate_recorded_technical(candidate), "confluence_count"),
                    }),
                    json!({
                        "min_confluences": GATE_REPLAY_MIN_CONFLUENCES,
                        "confluence_count": value_i64(candidate_recorded_technical(candidate), "confluence_count"),
                    }),
                ));
            }
        }
    }

    json!({
        "variable_path": "strategy.swing.daily_indicators.min_confluences",
        "proposed_value": GATE_REPLAY_MIN_CONFLUENCES,
        "comparison": "BUY technical gate only; SELL rules do not use the confluence threshold",
        "summary": {
            "candidate_count": candidate_count,
            "evaluated_count": evaluated_count,
            "would_block_target_gate_count": would_block_count,
            "would_clear_target_gate_only_count": would_clear_target_gate_only_count,
            "unchanged_target_gate_count": unchanged_count,
            "not_reached_count": not_reached_count,
            "insufficient_evidence_count": insufficient_evidence_count,
        },
        "changes": changes,
    })
}

/// Read-only counterfactual projection over persisted manager snapshots. It
/// isolates each threshold and never re-runs a decision report, calls Saxo, or
/// changes runtime configuration.
fn gate_replay_from_manager_runs(runs: &[JsonValue]) -> JsonValue {
    json!({
        "status": if runs.is_empty() { "no_history" } else { "available" },
        "run_count": runs.len(),
        "scenarios": [
            gate_replay_markov_scenario(runs),
            gate_replay_technical_scenario(runs),
        ],
        "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
        "interpretation": "A target-gate clear is not an approval: other recorded gates, capital, holdings, and market conditions remain outside this isolated comparison.",
    })
}

#[derive(Default)]
struct SupportRiskEvidenceStats {
    signal_count: usize,
    one_run_count: usize,
    one_run_return_sum_pct: f64,
    one_run_negative_count: usize,
    five_run_count: usize,
    five_run_return_sum_pct: f64,
    five_run_negative_count: usize,
    break_risk_sum: f64,
    confidence_sum: f64,
    history_coverage_sum: f64,
}

fn average_or_null(total: f64, count: usize) -> JsonValue {
    if count == 0 {
        JsonValue::Null
    } else {
        json!(total / count as f64)
    }
}

fn fraction_or_null(numerator: usize, denominator: usize) -> JsonValue {
    if denominator == 0 {
        JsonValue::Null
    } else {
        json!(numerator as f64 / denominator as f64)
    }
}

/// Groups stored daily signals by their recorded support-break label and
/// observes subsequent stored closes. This deliberately measures only the
/// next available indicator runs; it neither assumes every market traded nor
/// claims a causal backtest from sparse observations.
fn support_risk_evidence_from_indicator_rows(rows: &[JsonValue]) -> JsonValue {
    let mut grouped: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for row in rows {
        let symbol = json_text(row, "symbol");
        let run_date = json_text(row, "run_date");
        if !symbol.trim().is_empty() && !run_date.trim().is_empty() {
            // The query is ordered by created_at within a symbol/date. A manual
            // rerun supersedes the older daily row instead of manufacturing an
            // extra next-run outcome for the same market date.
            let symbol_rows = grouped.entry(symbol).or_default();
            if symbol_rows
                .last()
                .is_some_and(|previous| json_text(previous, "run_date") == run_date)
            {
                let last_index = symbol_rows.len() - 1;
                symbol_rows[last_index] = row;
            } else {
                symbol_rows.push(row);
            }
        }
    }

    let mut stats: HashMap<&'static str, SupportRiskEvidenceStats> = SUPPORT_RISK_LABELS
        .iter()
        .copied()
        .map(|label| (label, SupportRiskEvidenceStats::default()))
        .collect();

    for symbol_rows in grouped.values() {
        for (index, row) in symbol_rows.iter().enumerate() {
            let label = json_text(row, "support_break_risk_label");
            let Some(label) = SUPPORT_RISK_LABELS
                .iter()
                .copied()
                .find(|candidate| *candidate == label)
            else {
                continue;
            };
            let close = value_f64(row, "close");
            if !close.is_finite() || close <= 0.0 {
                continue;
            }
            let entry = stats.get_mut(label).expect("known support-risk label");
            entry.signal_count += 1;
            entry.break_risk_sum += value_f64(row, "support_break_risk").clamp(0.0, 1.0);
            entry.confidence_sum += value_f64(row, "support_confidence").clamp(0.0, 1.0);
            entry.history_coverage_sum +=
                value_f64(row, "support_history_coverage").clamp(0.0, 1.0);

            for (horizon, count, return_sum, negative_count) in [
                (
                    1usize,
                    &mut entry.one_run_count,
                    &mut entry.one_run_return_sum_pct,
                    &mut entry.one_run_negative_count,
                ),
                (
                    5usize,
                    &mut entry.five_run_count,
                    &mut entry.five_run_return_sum_pct,
                    &mut entry.five_run_negative_count,
                ),
            ] {
                let Some(next_row) = symbol_rows.get(index + horizon) else {
                    continue;
                };
                let next_close = value_f64(next_row, "close");
                if !next_close.is_finite() || next_close <= 0.0 {
                    continue;
                }
                let return_pct = (next_close - close) / close * 100.0;
                *count += 1;
                *return_sum += return_pct;
                if return_pct < 0.0 {
                    *negative_count += 1;
                }
            }
        }
    }

    let labels = SUPPORT_RISK_LABELS
        .iter()
        .map(|label| {
            let entry = stats.get(label).expect("known support-risk label");
            json!({
                "label": label,
                "signal_count": entry.signal_count,
                "next_run": {
                    "sample_count": entry.one_run_count,
                    "average_return_pct": average_or_null(entry.one_run_return_sum_pct, entry.one_run_count),
                    "negative_return_rate": fraction_or_null(entry.one_run_negative_count, entry.one_run_count),
                },
                "five_run": {
                    "sample_count": entry.five_run_count,
                    "average_return_pct": average_or_null(entry.five_run_return_sum_pct, entry.five_run_count),
                    "negative_return_rate": fraction_or_null(entry.five_run_negative_count, entry.five_run_count),
                },
                "average_break_risk": average_or_null(entry.break_risk_sum, entry.signal_count),
                "average_confidence": average_or_null(entry.confidence_sum, entry.signal_count),
                "average_history_coverage": average_or_null(entry.history_coverage_sum, entry.signal_count),
            })
        })
        .collect::<Vec<_>>();
    let eligible_signal_count = stats
        .values()
        .map(|entry| entry.signal_count)
        .sum::<usize>();
    let one_run_complete_count = stats
        .values()
        .map(|entry| entry.one_run_count)
        .sum::<usize>();
    let five_run_complete_count = stats
        .values()
        .map(|entry| entry.five_run_count)
        .sum::<usize>();
    let status = if eligible_signal_count == 0 {
        "no_observations"
    } else if five_run_complete_count < SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };

    json!({
        "status": status,
        "lookback_days": SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS,
        "minimum_complete_observations": SUPPORT_RISK_EVIDENCE_MIN_COMPLETE_OBSERVATIONS,
        "eligible_signal_count": eligible_signal_count,
        "next_run_complete_count": one_run_complete_count,
        "five_run_complete_count": five_run_complete_count,
        "labels": labels,
        "safety": "read_only_observation_of_stored_daily_indicator_closes",
        "interpretation": "Outcomes use the next available one or five stored daily indicator runs for the same symbol. They are descriptive, not causal, do not account for trading costs or market gaps, and cannot change a gate, Hermes proposal, configuration, or Saxo order.",
    })
}

fn compact_candidate_market(value: &JsonValue) -> JsonValue {
    let quarantine_active = value
        .get("instrument_quarantine")
        .and_then(|value| value.get("active"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    json!({
        "exchange": json_text(value, "exchange"),
        "exchange_open": value.get("exchange_open").and_then(JsonValue::as_bool).unwrap_or(false),
        "risk_excluded": value.get("risk_excluded").and_then(JsonValue::as_bool).unwrap_or(false),
        "quarantine_active": quarantine_active,
    })
}

fn compact_candidate_advice(value: Option<&JsonValue>) -> JsonValue {
    let Some(value) = value else {
        return json!({"effect": "not_recorded", "requested_quantity": 0.0, "resulting_quantity": 0.0});
    };
    json!({
        "effect": json_text(value, "effect"),
        "requested_quantity": value_f64(value, "requested_quantity"),
        "resulting_quantity": value_f64(value, "resulting_quantity"),
    })
}

/// Reconstructs the deterministic manager gate snapshot for a report without
/// exposing raw Hermes rationale, broker payloads, or raw execution errors.
fn candidate_scoring_waterfall_from_manager_run(run: &JsonValue) -> JsonValue {
    let Some(manager_json) = run.get("manager_json") else {
        return json!({
            "status": "not_processed",
            "candidates": [],
            "summary": {"candidate_count": 0, "approved_count": 0, "skipped_count": 0, "not_reached_count": 0, "gate_counts": {}},
            "safety": "sanitized_local_manager_audit",
        });
    };
    let preflight = manager_json
        .get("hermes_preflight")
        .and_then(|value| value.get("candidate_waterfall"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let advice = manager_json
        .get("hermes_advice_delta")
        .and_then(|value| value.get("candidates"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut advice_by_key = HashMap::new();
    for row in advice {
        advice_by_key.insert(candidate_scoring_key(&row), row);
    }

    let mut outcomes = HashMap::new();
    for (outcome, rows) in [
        ("approved", manager_json.get("approved_orders")),
        ("skipped", manager_json.get("skipped_orders")),
    ] {
        for row in rows.and_then(JsonValue::as_array).into_iter().flatten() {
            outcomes.insert(
                candidate_scoring_key(row),
                json!({
                    "strategy_key": json_text(row, "strategy_key"),
                    "symbol": json_text(row, "symbol"),
                    "action": json_text(row, "action"),
                    "outcome": outcome,
                    "gate_code": candidate_gate_code(row),
                    "final_technical": compact_candidate_final_technical(row),
                }),
            );
        }
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for row in preflight {
        let key = candidate_scoring_key(&row);
        seen.insert(key.clone());
        let outcome = outcomes
            .get(&key)
            .cloned()
            .unwrap_or_else(|| json!({"outcome": "not_reached", "gate_code": "other"}));
        candidates.push(json!({
            "strategy_key": json_text(&row, "strategy_key"),
            "symbol": json_text(&row, "symbol"),
            "action": json_text(&row, "action"),
            "order_type": json_text(&row, "order_type"),
            "quantity": value_f64(&row, "quantity"),
            "market": compact_candidate_market(&row),
            "technical": compact_candidate_technical(&row),
            "final_technical": outcome.get("final_technical").cloned().unwrap_or(JsonValue::Null),
            "markov": compact_candidate_markov(&row),
            "hermes": compact_candidate_advice(advice_by_key.get(&key)),
            "outcome": json_text(&outcome, "outcome"),
            "gate_code": json_text(&outcome, "gate_code"),
        }));
    }
    for (key, outcome) in outcomes {
        if seen.contains(&key) {
            continue;
        }
        let strategy_key = json_text(&outcome, "strategy_key");
        let strategy_key = if strategy_key.trim().is_empty() {
            key.strip_prefix("strategy:")
                .unwrap_or_default()
                .to_string()
        } else {
            strategy_key
        };
        candidates.push(json!({
            "strategy_key": strategy_key,
            "symbol": json_text(&outcome, "symbol"),
            "action": json_text(&outcome, "action"),
            "order_type": "",
            "quantity": 0.0,
            "market": {"exchange": "", "exchange_open": false, "risk_excluded": false, "quarantine_active": false},
            "technical": {"status": "unavailable", "sentiment": "", "trend_bias": "", "confluence_count": 0, "min_confluences": 0},
            "final_technical": outcome.get("final_technical").cloned().unwrap_or(JsonValue::Null),
            "markov": {"status": "unavailable", "fresh": false, "direction": "", "signed_signal": 0.0, "age_days": 0},
            "hermes": compact_candidate_advice(advice_by_key.get(&key)),
            "outcome": json_text(&outcome, "outcome"),
            "gate_code": json_text(&outcome, "gate_code"),
        }));
    }

    let mut approved_count = 0usize;
    let mut skipped_count = 0usize;
    let mut not_reached_count = 0usize;
    let mut gate_counts: HashMap<String, usize> = HashMap::new();
    for candidate in &candidates {
        match json_text(candidate, "outcome").as_str() {
            "approved" => approved_count += 1,
            "skipped" => skipped_count += 1,
            _ => not_reached_count += 1,
        }
        *gate_counts
            .entry(json_text(candidate, "gate_code"))
            .or_default() += 1;
    }

    json!({
        "status": "available",
        "run_id": value_i64(run, "id"),
        "created_at": json_text(run, "created_at"),
        "manager_status": json_text(run, "status"),
        "candidates": candidates,
        "summary": {
            "candidate_count": approved_count + skipped_count + not_reached_count,
            "approved_count": approved_count,
            "skipped_count": skipped_count,
            "not_reached_count": not_reached_count,
            "gate_counts": gate_counts,
        },
        "safety": "sanitized_local_manager_audit",
    })
}

fn nested_json_text(value: &JsonValue, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        current = next;
    }
    match current {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

fn money_mismatch_exceeds_tolerance(
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

fn is_duplicate_column_error(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("duplicate column")
        || message.contains("already exists")
        || message.contains("column exists")
}

fn saxo_refresh_lease_owner(source: &str) -> String {
    let host = env::var("HOSTNAME").unwrap_or_else(|_| "local".to_string());
    let nonce = Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_micros());
    format!(
        "{}:{}:{}:{}",
        host,
        process::id(),
        source.replace(':', "_"),
        nonce
    )
}

fn saxo_session_needs_refresh(session: &JsonValue) -> bool {
    let access_token = json_text(session, "access_token");
    if access_token.is_empty() {
        return saxo_session_refresh_token_usable(session);
    }
    let Some(access_expires_at) = parse_rfc3339_utc(session.get("access_token_expires_at")) else {
        return saxo_session_refresh_token_usable(session);
    };
    access_expires_at <= Utc::now() + Duration::seconds(15 * 60)
        && saxo_session_refresh_token_usable(session)
}

fn saxo_session_refresh_token_usable(session: &JsonValue) -> bool {
    if !json_text(session, "refresh_token_invalid_at")
        .trim()
        .is_empty()
    {
        return false;
    }
    if json_text(session, "refresh_token").trim().is_empty() {
        return false;
    }
    let Some(refresh_expires_at) = parse_rfc3339_utc(session.get("refresh_token_expires_at"))
    else {
        return false;
    };
    refresh_expires_at > Utc::now() + Duration::seconds(15 * 60)
}

fn parse_rfc3339_utc(value: Option<&JsonValue>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?.as_str()?.replace('Z', "+00:00").as_str())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn matching_order_advice(
    value: Option<&JsonValue>,
    strategy_key: &str,
    symbol: &str,
    action: &str,
) -> Option<JsonValue> {
    let items = value?.as_array()?;
    let strategy_key = strategy_key.trim();
    let symbol = symbol.trim();
    let action = action.trim();
    if !strategy_key.is_empty() {
        if let Some(item) = items
            .iter()
            .find(|item| json_text(item, "strategy_key").trim() == strategy_key)
        {
            return Some(item.clone());
        }
    }
    items
        .iter()
        .find(|item| {
            json_text(item, "symbol")
                .trim()
                .eq_ignore_ascii_case(symbol)
                && json_text(item, "action")
                    .trim()
                    .eq_ignore_ascii_case(action)
        })
        .cloned()
}

fn matching_manager_preflight_candidate(
    manager_run: &JsonValue,
    strategy_key: &str,
    symbol: &str,
    action: &str,
) -> JsonValue {
    manager_run
        .get("manager_json")
        .and_then(|value| value.get("hermes_preflight"))
        .and_then(|value| value.get("candidate_waterfall"))
        .and_then(|value| matching_order_advice(Some(value), strategy_key, symbol, action))
        .unwrap_or(JsonValue::Null)
}

fn compact_attribution_technical(value: &JsonValue, evidence_source: &str) -> JsonValue {
    if !value.is_object() || json_text(value, "status") != "ok" {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": evidence_source,
        "run_date": json_text(value, "run_date"),
        "sentiment": json_text(value, "sentiment"),
        "trend_bias": json_text(value, "trend_bias"),
        "confluence_count": value_i64(value, "confluence_count"),
        "min_confluences": value_i64(value, "min_confluences"),
        "reward_risk": value_f64(value, "reward_risk"),
    })
}

fn compact_attribution_markov(value: &JsonValue, evidence_source: &str) -> JsonValue {
    if !value.is_object() || json_text(value, "status") != "ok" {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": evidence_source,
        "run_date": json_text(value, "run_date"),
        "current_state": json_text(value, "current_state"),
        "direction": json_text(value, "direction"),
        "signed_signal": value_f64(value, "signed_signal"),
        "conviction": value_f64(value, "conviction"),
        "bull_prob": value_f64(value, "bull_prob"),
        "bear_prob": value_f64(value, "bear_prob"),
    })
}

fn compact_attribution_capital(value: &JsonValue) -> JsonValue {
    if !value.is_object() {
        return JsonValue::Null;
    }
    json!({
        "evidence_source": "manager_run",
        "cash_balance_dkk": value_f64(value, "cash_balance_dkk"),
        "cash_pct": value_f64(value, "cash_pct"),
        "required_cash_buffer_dkk": value_f64(value, "required_cash_buffer_dkk"),
        "available_buy_budget_dkk": value_f64(value, "available_buy_budget_dkk"),
        "remaining_deployment_capacity_dkk": value_f64(value, "remaining_deployment_capacity_dkk"),
        "reinvestment_pressure_active": value
            .get("reinvestment_pressure_active")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
    })
}

fn compact_execution_ledger_outcome(
    order: &JsonValue,
    summary: &JsonValue,
    evidence_source: &str,
) -> JsonValue {
    let fill_count = value_i64(summary, "fill_count");
    if fill_count <= 0 {
        return JsonValue::Null;
    }
    let ledger_entry_count = value_i64(summary, "ledger_entry_count");
    let filled_quantity = value_f64(summary, "filled_quantity");
    let target_quantity = value_f64(order, "quantity");
    json!({
        "evidence_source": evidence_source,
        "status": if ledger_entry_count >= fill_count {
            "reconciled"
        } else {
            "pending_ledger_reconciliation"
        },
        "side": json_text(order, "action").to_uppercase(),
        "fill_count": fill_count,
        "ledger_entry_count": ledger_entry_count,
        "filled_quantity": filled_quantity,
        "target_quantity": target_quantity,
        "fully_filled": target_quantity > 0.0 && filled_quantity + 1e-9 >= target_quantity,
        "last_fill_at": json_text(summary, "last_fill_at"),
        "commission_dkk": value_f64(summary, "commission_dkk"),
        "tax_dkk": value_f64(summary, "tax_dkk"),
        "realised_gain_dkk": value_f64(summary, "realised_gain_dkk"),
        "cost_basis_sold_dkk": value_f64(summary, "cost_basis_sold_dkk"),
    })
}

fn attribution_delta_label(
    hermes_order: &JsonValue,
    manager_order: &JsonValue,
    order: &JsonValue,
) -> String {
    let hermes_action = json_text(hermes_order, "action").trim().to_lowercase();
    let manager_decision = json_text(manager_order, "manager_decision");
    let status = json_text(order, "status").trim().to_lowercase();
    if hermes_action.is_empty() {
        return if manager_decision == "approved" {
            "manager_only".to_string()
        } else {
            "no_advice".to_string()
        };
    }
    if matches!(hermes_action.as_str(), "stand_down" | "review") && manager_decision == "approved" {
        return "manager_overrode_review".to_string();
    }
    if hermes_action == "reduce" && manager_decision == "approved" {
        return "reduced_or_capped".to_string();
    }
    if hermes_action == "allow" && manager_decision == "approved" {
        return if status == "executed" {
            "allowed_executed".to_string()
        } else {
            "allowed_queued".to_string()
        };
    }
    if manager_decision == "skipped" {
        return format!("{}_skipped", hermes_action);
    }
    hermes_action
}

/// Midnight at the start of a local calendar date, rendered as the UTC
/// RFC3339 string format used by portfolio_value_history.recorded_at.
fn local_date_start_to_utc_string(date: NaiveDate, tz: Tz) -> String {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    let local = tz
        .from_local_datetime(&midnight)
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&midnight));
    local
        .with_timezone(&Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn performance_range_limit(range_key: &str) -> i64 {
    // Rust match expressions are similar to Python's match/case or a JS switch, but they
    // must cover every possible input. The final `_` arm is the default case.
    match range_key {
        "1D" => 120,
        "1W" => 600,
        "1M" => 2500,
        "3M" => 5000,
        "YTD" => 5000,
        "1Y" => 5000,
        "ALL" => 5000,
        _ => 120,
    }
}

fn dashboard_performance_history_limit(active_view: &str, range_key: &str) -> Option<i64> {
    (active_view == "performance").then(|| performance_range_limit(range_key))
}

fn dashboard_loads_tab_exclusive_data(active_view: &str, tab: &str) -> bool {
    active_view == tab
}

const EXECUTION_ORDERS_PAGE_SIZE: i64 = 25;
const OVERVIEW_EXECUTION_ORDERS_LIMIT: i64 = 12;
const SHARED_EXECUTION_ORDERS_LIMIT: i64 = 20;
const MARKOV_SIGNALS_PAGE_SIZE: i64 = 40;
const QUIVER_SIGNALS_PAGE_SIZE: i64 = 40;
const SCHEDULER_CYCLES_PAGE_SIZE: i64 = 12;

fn dashboard_execution_order_window(
    active_view: &str,
    requested_page: i64,
    total_orders: i64,
) -> (i64, i64, i64) {
    if active_view != "execution" {
        let limit = if active_view == "overview" {
            OVERVIEW_EXECUTION_ORDERS_LIMIT
        } else {
            SHARED_EXECUTION_ORDERS_LIMIT
        };
        return (1, limit, 0);
    }

    let total_pages = ((total_orders.max(0) + EXECUTION_ORDERS_PAGE_SIZE - 1)
        / EXECUTION_ORDERS_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    let offset = (page - 1) * EXECUTION_ORDERS_PAGE_SIZE;
    (page, EXECUTION_ORDERS_PAGE_SIZE, offset)
}

fn dashboard_markov_signal_window(requested_page: i64, total_signals: i64) -> (i64, i64) {
    let total_pages =
        ((total_signals.max(0) + MARKOV_SIGNALS_PAGE_SIZE - 1) / MARKOV_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    (page, (page - 1) * MARKOV_SIGNALS_PAGE_SIZE)
}

fn dashboard_quiver_signal_window(requested_page: i64, total_signals: i64) -> (i64, i64) {
    let total_pages =
        ((total_signals.max(0) + QUIVER_SIGNALS_PAGE_SIZE - 1) / QUIVER_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    (page, (page - 1) * QUIVER_SIGNALS_PAGE_SIZE)
}

fn dashboard_scheduler_cycle_window(requested_page: i64, total_cycles: i64) -> (i64, i64) {
    let total_pages = ((total_cycles.max(0) + SCHEDULER_CYCLES_PAGE_SIZE - 1)
        / SCHEDULER_CYCLES_PAGE_SIZE)
        .max(1);
    let page = requested_page.max(1).min(total_pages);
    (page, (page - 1) * SCHEDULER_CYCLES_PAGE_SIZE)
}

fn scheduler_history_policy_values(
    configured_max_rows: Option<i64>,
    configured_retention_days: Option<i64>,
) -> (i64, i64) {
    (
        configured_max_rows
            .unwrap_or(DEFAULT_SCHEDULER_HISTORY_MAX_ROWS)
            .max(0),
        configured_retention_days
            .unwrap_or(DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS)
            .max(0),
    )
}

fn hermes_experiment_next_status(current_status: &str, action: &str) -> Option<&'static str> {
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

impl AppState {
    // Associated functions are like static/class methods. `Self` means
    // `AppState`, so this returns a fully initialized application state.
    pub async fn load() -> Result<Self> {
        let config_path =
            env::var("DAYTRADER_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
        let config_path = PathBuf::from(config_path);
        info!(config_path = %config_path.display(), "loading application config");
        let config_text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading config {}", config_path.display()))?;
        let config: YamlValue = serde_yaml::from_str(&config_text)
            .with_context(|| format!("parsing config {}", config_path.display()))?;
        let db_url = database_url(&config, &config_path)?;
        let safe_db_url = redacted_database_url(&db_url);
        info!(database_url = %safe_db_url, "connecting to database");
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .with_context(|| format!("connecting to database {safe_db_url}"))?;
        info!(database_url = %safe_db_url, "database connection pool ready");
        let state = Self {
            config_path,
            config,
            db_url,
            pool,
        };
        state.ensure_runtime_state_schema().await?;
        if let Err(err) = state.sync_saxo_session_storage().await {
            warn!("Saxo session database sync skipped during startup: {err:#}");
        }
        Ok(state)
    }

    pub async fn dashboard_view(
        &self,
        localization: LocalizationPrefs,
        sso_session: JsonValue,
        active_view: String,
        performance_range: String,
        selected_report_id: Option<i64>,
        requested_execution_page: i64,
        requested_markov_page: i64,
        requested_quiver_page: i64,
        requested_scheduler_page: i64,
    ) -> DashboardView {
        let overview = self.overview_payload().await.unwrap_or_else(|err| {
            error!("overview load failed: {err:#}");
            json!({})
        });
        let positions = self.position_items(25).await.unwrap_or_else(|err| {
            warn!("dashboard positions degraded: {err:#}");
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
        let (execution_page, execution_page_size, execution_orders_offset) =
            dashboard_execution_order_window(
                &active_view,
                requested_execution_page,
                execution_order_total,
            );
        let orders = self
            .execution_orders_page(execution_page_size, execution_orders_offset)
            .await
            .unwrap_or_else(|err| {
                warn!("dashboard execution queue degraded: {err:#}");
                Vec::new()
            });
        let execution_fills = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.execution_fills(50).await.unwrap_or_else(|err| {
                warn!("dashboard execution fills degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let execution_events = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.execution_events(50).await.unwrap_or_else(|err| {
                warn!("dashboard execution events degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
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
        let needs_report_detail = matches!(active_view.as_str(), "decisions" | "prompts");
        let selected_decision = if needs_report_detail {
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
                    self.decision_report_item(report_id)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(JsonValue::Null)
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
            self.decision_gate_replay(GATE_REPLAY_DEFAULT_RUN_LIMIT)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard gate replay degraded: {err:#}");
                    json!({
                        "status": "unavailable",
                        "run_count": 0,
                        "scenarios": [],
                        "safety": "offline_historical_target_gate_only_no_model_broker_or_configuration_mutation",
                    })
                })
        } else {
            JsonValue::Null
        };
        // The Operations banner is visible on every dashboard tab, so it needs
        // a compact per-pulse report status rather than only the latest global
        // report. The payload deliberately excludes report prompt/response
        // bodies and remains small enough for the shared read model.
        let decision_pulse_statuses = self.decision_pulse_statuses().await.unwrap_or_else(|err| {
            warn!("dashboard decision pulse statuses degraded: {err:#}");
            Vec::new()
        });
        let journal_entries = if dashboard_loads_tab_exclusive_data(&active_view, "eod") {
            self.strategy_journal_items(20).await.unwrap_or_else(|err| {
                warn!("dashboard end-of-day journal degraded: {err:#}");
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
        let (scheduler_page, scheduler_cycles_offset) =
            dashboard_scheduler_cycle_window(requested_scheduler_page, scheduler_cycle_total);
        let scheduler_cycles = if dashboard_loads_tab_exclusive_data(&active_view, "execution") {
            self.scheduler_cycles_page(SCHEDULER_CYCLES_PAGE_SIZE, scheduler_cycles_offset)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard scheduler cycles degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let hermes_reflections = if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
            self.hermes_reflections(20).await.unwrap_or_else(|err| {
                warn!("dashboard Hermes reflections degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let hermes_lessons_pending_review =
            if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
                self.hermes_lessons_pending_review(HERMES_LESSONS_PENDING_REVIEW_LIMIT as i64)
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard Hermes lessons pending review degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_learning_memory = if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
            self.hermes_learning_memory(HERMES_LEARNING_MEMORY_LIMIT as i64)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard Hermes learning memory degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let hermes_experiments = if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
            self.hermes_experiments(20).await.unwrap_or_else(|err| {
                warn!("dashboard Hermes experiments degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let hermes_proposal_quality = if dashboard_loads_tab_exclusive_data(&active_view, "hermes")
        {
            hermes_proposal_quality_from_experiments(&hermes_experiments)
        } else {
            Vec::new()
        };
        let hermes_decision_advice_audit =
            if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
                self.hermes_decision_advice_audit(20)
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard Hermes decision advice audit degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let hermes_counterfactuals = if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
            self.hermes_counterfactuals(30).await.unwrap_or_else(|err| {
                warn!("dashboard Hermes counterfactuals degraded: {err:#}");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let active_strategy_baseline = if dashboard_loads_tab_exclusive_data(&active_view, "hermes")
        {
            self.active_strategy_baseline().await.unwrap_or_else(|err| {
                warn!("dashboard active strategy baseline degraded: {err:#}");
                JsonValue::Null
            })
        } else {
            JsonValue::Null
        };
        let hermes_baseline_evidence_pack =
            if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
                self.hermes_baseline_evidence_pack(&active_strategy_baseline)
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard Hermes baseline evidence pack degraded: {err:#}");
                        json!({
                            "status": "unavailable",
                            "safety": "read_only_observational_not_causal",
                        })
                    })
            } else {
                JsonValue::Null
            };
        let hermes_one_variable_audit =
            if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
                self.hermes_one_variable_audit()
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard Hermes one-variable audit degraded: {err:#}");
                        Vec::new()
                    })
            } else {
                Vec::new()
            };
        let markov_signal_total = if dashboard_loads_tab_exclusive_data(&active_view, "markov") {
            self.markov_signals_count().await.unwrap_or_else(|err| {
                warn!("dashboard Markov signal count degraded: {err:#}");
                0
            })
        } else {
            0
        };
        let (markov_page, markov_signals_offset) =
            dashboard_markov_signal_window(requested_markov_page, markov_signal_total);
        let markov_signals = if dashboard_loads_tab_exclusive_data(&active_view, "markov") {
            self.markov_signals_page(MARKOV_SIGNALS_PAGE_SIZE, markov_signals_offset)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard Markov signals degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let latest_markov_run = self.latest_markov_run().await.unwrap_or_else(|err| {
            warn!("dashboard latest Markov run degraded: {err:#}");
            JsonValue::Null
        });
        let quiver_signal_total = if dashboard_loads_tab_exclusive_data(&active_view, "quiver") {
            self.quiver_signals_count().await.unwrap_or_else(|err| {
                warn!("dashboard Quiver signal count degraded: {err:#}");
                0
            })
        } else {
            0
        };
        let (quiver_page, quiver_signals_offset) =
            dashboard_quiver_signal_window(requested_quiver_page, quiver_signal_total);
        let quiver_signals = if dashboard_loads_tab_exclusive_data(&active_view, "quiver") {
            self.quiver_signals_page(QUIVER_SIGNALS_PAGE_SIZE, quiver_signals_offset)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard Quiver signals degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let latest_quiver_run = self.latest_quiver_run().await.unwrap_or_else(|err| {
            warn!("dashboard latest Quiver run degraded: {err:#}");
            JsonValue::Null
        });
        let latest_daily_indicator_run =
            self.latest_daily_indicator_run()
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard latest daily indicator run degraded: {err:#}");
                    JsonValue::Null
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
            self.performance_summary(&performance_history)
        } else {
            JsonValue::Null
        };
        let market_status = self.market_status_payload().await.unwrap_or_else(|err| {
            warn!("dashboard market status degraded: {err:#}");
            json!({"items": [], "summary": {"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []}})
        });
        let watchlists = if dashboard_loads_tab_exclusive_data(&active_view, "watchlists") {
            self.watchlists_payload().await.unwrap_or_else(|err| {
                warn!("dashboard watchlists degraded: {err:#}");
                json!({"generated_at": Utc::now().to_rfc3339(), "categories": []})
            })
        } else {
            JsonValue::Null
        };
        let latest_decision = if active_view == "prompts" {
            selected_decision.clone()
        } else {
            reports.first().cloned().unwrap_or(JsonValue::Null)
        };
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
        let saxo_auth_object = saxo_auth.as_object().cloned().unwrap_or_default();

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
            saxo_status: saxo_auth
                .get("status_text")
                .or_else(|| saxo_auth.get("status"))
                .and_then(JsonValue::as_str)
                .unwrap_or("not connected")
                .to_string(),
            saxo_auth: JsonValue::Object(saxo_auth_object),
            sso_session,
            ai_settings: self.ai_settings_value().await.unwrap_or_else(|err| {
                warn!("dashboard AI settings degraded: {err:#}");
                self.default_ai_settings_value()
            }),
            localization,
            active_view,
            performance_range,
            selected_report_id,
            execution_page,
            execution_page_size,
            execution_order_total,
            markov_page,
            markov_page_size: MARKOV_SIGNALS_PAGE_SIZE,
            markov_signal_total,
            quiver_page,
            quiver_page_size: QUIVER_SIGNALS_PAGE_SIZE,
            quiver_signal_total,
            scheduler_page,
            scheduler_page_size: SCHEDULER_CYCLES_PAGE_SIZE,
            scheduler_cycle_total,
            positions,
            orders,
            execution_fills,
            execution_events,
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
            active_strategy_baseline,
            hermes_baseline_evidence_pack,
            markov_signals,
            latest_markov_run,
            quiver_signals,
            latest_quiver_run,
            latest_daily_indicator_run,
            run_schedules: json!({
                "markov": crate::markov_method::markov_config_json_for_state(self),
                "quiver": crate::quiver::quiver_config_json_for_state(self),
                "indicators": crate::daily_indicators::indicator_config_json_for_state(self),
            }),
            performance_history,
            performance_summary,
            integrity: overview
                .get("integrity")
                .cloned()
                .unwrap_or_else(|| json!({"healthy": false, "warnings": [], "mismatches": []})),
            market_status,
            trading_manager: overview
                .get("trading_manager")
                .cloned()
                .unwrap_or(JsonValue::Null),
            watchlists,
            latest_decision,
            selected_decision,
            decision_gate_replay,
        }
    }

    pub async fn overview_payload(&self) -> Result<JsonValue> {
        // `&self` is a borrowed receiver: callers can use AppState without
        // transferring ownership, similar to passing an object reference in
        // Python or JavaScript.
        let latest_history = self
            .first_json(
                "SELECT recorded_at, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count FROM portfolio_value_history ORDER BY recorded_at DESC LIMIT 1",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let latest_batch = self.latest_batch_id().await?;
        let broker_positions_available = self.broker_positions_available().await.unwrap_or(false);
        let aggregate = if broker_positions_available {
            self.position_aggregate(latest_batch.as_deref()).await?
        } else if latest_history.as_object().is_some_and(|o| !o.is_empty()) {
            latest_history.clone()
        } else {
            self.position_aggregate(latest_batch.as_deref()).await?
        };
        let total_value = value_f64(&aggregate, "total_market_value_dkk");
        let cash_summary = self.cash_summary_from_ledger().await?;
        let initial_cash = aggregate
            .get("initial_cash_dkk")
            .map(|_| value_f64(&aggregate, "initial_cash_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "initial_cash_dkk"));
        let cash_from_trades = aggregate
            .get("cash_from_trades_dkk")
            .map(|_| value_f64(&aggregate, "cash_from_trades_dkk"))
            .unwrap_or_else(|| value_f64(&cash_summary, "cash_from_trades_dkk"));
        let max_daily_orders =
            yaml_i64(&self.config, &["execution", "max_daily_orders"]).unwrap_or(0);
        let executed_today = self.executed_orders_today().await.unwrap_or(0);
        let decision_refresh = crate::xai_decision::decision_pulse_summary(self);
        let integrity = self
            .overview_integrity(&aggregate, &latest_history, &cash_summary)
            .await
            .unwrap_or_else(|err| {
                json!({
                    "healthy": false,
                    "warnings": [{
                        "code": "integrity_check_failed",
                        "severity": "warning",
                        "message": format!("Overview integrity checks failed: {err:#}")
                    }],
                    "mismatches": [],
                    "unreconciled_orders": [],
                    "checked_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                })
            });

        Ok(json!({
            "app": {
                "project_name": yaml_string(&self.config, &["app", "project_name"]),
                "environment": yaml_string(&self.config, &["app", "environment"]),
                "config_path": self.config_path.display().to_string(),
                "runtime": "rust-dioxus"
            },
            "execution": {
                "mode": yaml_string(&self.config, &["execution", "mode"]),
                "adapter": yaml_string(&self.config, &["execution", "adapter"]),
                "require_approval_live": yaml_bool(&self.config, &["execution", "require_approval_live"]).unwrap_or(true),
                "max_daily_orders": max_daily_orders,
                "daily_order_capacity": {
                    "max": max_daily_orders,
                    "used": executed_today,
                    "remaining": (max_daily_orders - executed_today).max(0)
                },
                "counts": self.execution_counts().await.unwrap_or_else(|_| json!({
                    "queued": 0,
                    "pending_approval": 0,
                    "broker_live": 0,
                    "failed": 0
                })),
            },
            "portfolio_summary": {
                "recorded_at": aggregate.get("recorded_at").cloned().unwrap_or(JsonValue::Null),
                "total_market_value_dkk": total_value,
                "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
                "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
                "initial_cash_dkk": initial_cash,
                "cash_from_trades_dkk": cash_from_trades,
                "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
                "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
                "position_count": value_i64(&aggregate, "position_count"),
            },
            "after_tax_summary": {
                "unrealised_pnl_after_tax_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                "estimated_tax_dkk": 0.0
            },
            "goal_tracking": self.goal_tracking(total_value).await,
            "integrity": integrity,
            "analysis_summary": self.market_status_payload().await.unwrap_or_else(|_| json!({"summary": {"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []}})).get("summary").cloned().unwrap_or_else(|| json!({"analysis_window_active": false, "active_markets": [], "active_windows": [], "pre_sync_markets": []})),
            "latest_decision": self.latest_decision_summary().await.unwrap_or_else(|_| json!({"id": null, "created_at": null, "status": null})),
            "scheduler_status": self.scheduler_status_value().await.unwrap_or(JsonValue::Null),
            "scheduler_health": {"status": "ok", "message": "Rust scheduler maintains Saxo sessions, submits/polls deferred xAI decision reports, runs the Trading Manager, refreshes daily Markov regime signals when due, and creates due end-of-day journals."},
            "trading_manager": {
                "status": "available",
                "latest_run": self.latest_trading_manager_run().await.unwrap_or(JsonValue::Null)
            },
            "markov_method": {
                "status": "available",
                "config": crate::markov_method::markov_config_json_for_state(self),
                "latest_run": self.latest_markov_run().await.unwrap_or(JsonValue::Null),
            },
            "quiver_signals": {
                "status": "available",
                "config": crate::quiver::quiver_config_json_for_state(self),
                "latest_run": self.latest_quiver_run().await.unwrap_or(JsonValue::Null),
            },
            "saxo_auth": self.saxo_auth_status_value().await,
            "settings": {
                "cash_buffer": self.cash_buffer_value(),
                "ai": self.ai_settings_value().await.unwrap_or_else(|_| self.default_ai_settings_value())
            },
            "refresh": {
                "price_poll_interval_minutes": yaml_i64(&self.config, &["price_monitor", "poll_interval_minutes"]).unwrap_or(1),
                "scheduler_poll_interval_minutes": yaml_i64(&self.config, &["scheduler", "poll_interval_minutes"]).unwrap_or(10),
                "decision_cadence": "rust_dashboard",
                "decision_cadence_label": "Rust dashboard",
                "decision_pulses": decision_refresh.get("pulses").cloned().unwrap_or_else(|| json!([])),
                "next_decision_pulse_at": decision_refresh.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
                "next_decision_pulse_label": decision_refresh.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null)
            }
        }))
    }

    pub async fn performance_payload(&self, range_key: &str) -> Result<JsonValue> {
        let history = self
            .performance_history_with_current(range_key, performance_range_limit(range_key))
            .await?;
        let latest = history.last().cloned().unwrap_or_else(|| json!({}));
        let total = value_f64(&latest, "total_market_value_dkk");
        Ok(json!({
            "range_key": range_key,
            "history": history,
            "summary": self.performance_summary(&history),
            "goal_tracking": self.goal_tracking(total).await
        }))
    }

    pub fn performance_summary(&self, history: &[JsonValue]) -> JsonValue {
        let first = history.first();
        let latest = history.last();
        let first_total = first
            .map(|row| value_f64(row, "total_market_value_dkk"))
            .unwrap_or(0.0);
        let latest_total = latest
            .map(|row| value_f64(row, "total_market_value_dkk"))
            .unwrap_or(0.0);
        let latest_daily = latest
            .map(|row| value_f64(row, "total_daily_pnl_dkk"))
            .unwrap_or(0.0);
        let latest_positions = latest
            .map(|row| value_i64(row, "position_count"))
            .unwrap_or(0);
        json!({
            "points": history.len(),
            "first_recorded_at": first.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "latest_recorded_at": latest.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "first_total_market_value_dkk": first_total,
            "latest_total_market_value_dkk": latest_total,
            "change_dkk": latest_total - first_total,
            "daily_pnl_dkk": latest_daily,
            "position_count": latest_positions
        })
    }

    pub async fn market_status_payload(&self) -> Result<JsonValue> {
        let calendar_refresh = match self.refresh_saxo_exchange_calendars_if_stale().await {
            Ok(value) => value,
            Err(err) => {
                warn!("Saxo exchange calendar refresh skipped: {err:#}");
                json!({"status": "error", "error": err.to_string()})
            }
        };
        let items = self.market_exchange_rows();
        let scheduler = self
            .scheduler_status_value()
            .await
            .unwrap_or(JsonValue::Null);
        let price_monitor = self
            .price_monitor_status_value()
            .await
            .unwrap_or(JsonValue::Null);
        let cycle = scheduler
            .get("last_cycle_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let manager_status = cycle
            .get("trading_manager")
            .and_then(|value| value.get("manager_status"))
            .cloned()
            .unwrap_or(JsonValue::Null);
        let open_active_markets = market_names_where(&items, "open_analysis_window_active");
        let close_active_markets = market_names_where(&items, "close_analysis_window_active");
        let pre_sync_markets = market_names_where(&items, "pre_analysis_sync_active");
        let active_markets = open_active_markets
            .iter()
            .chain(close_active_markets.iter())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let summary = json!({
            "analysis_window_active": !active_markets.is_empty(),
            "active_markets": active_markets,
            "active_windows": manager_status.get("active_pulses").cloned().unwrap_or_else(|| json!([])),
            "open_active_markets": open_active_markets,
            "close_active_markets": close_active_markets,
            "pre_sync_markets": pre_sync_markets,
            "last_cycle_status": scheduler.get("last_cycle_status").cloned().unwrap_or(JsonValue::Null),
            "last_heartbeat_at": scheduler.get("last_heartbeat_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_at": manager_status.get("next_pulse_at").cloned().unwrap_or(JsonValue::Null),
            "next_pulse_label": manager_status.get("next_pulse_label").cloned().unwrap_or(JsonValue::Null),
            "price_monitor_status": price_monitor.get("status").cloned().unwrap_or(JsonValue::Null),
            "price_monitor_updated_at": price_monitor.get("updated_at").cloned().unwrap_or(JsonValue::Null),
            "calendar_refresh": calendar_refresh,
        });
        Ok(json!({
            "items": items,
            "summary": summary,
            "scheduler": scheduler,
            "price_monitor": price_monitor
        }))
    }

    pub async fn refresh_saxo_exchange_calendars_if_stale(&self) -> Result<JsonValue> {
        let today = Utc::now().date_naive();
        if let Some(cache) = current_saxo_exchange_calendar_cache() {
            if cache.checked_date == today {
                return Ok(json!({
                    "status": "fresh",
                    "source": cache.source,
                    "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "exchange_count": cache.exchanges.len(),
                }));
            }
        }

        let cache = self
            .fetch_saxo_exchange_calendar_cache(today)
            .await
            .context("refreshing Saxo exchange calendar cache")?;
        let result = json!({
            "status": "refreshed",
            "source": cache.source,
            "checked_at": cache.checked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "exchange_count": cache.exchanges.len(),
        });
        let lock = saxo_exchange_calendar_cache_lock();
        *lock
            .write()
            .map_err(|_| anyhow!("Saxo exchange calendar cache lock is poisoned"))? = Some(cache);
        Ok(result)
    }

    async fn fetch_saxo_exchange_calendar_cache(
        &self,
        checked_date: NaiveDate,
    ) -> Result<SaxoExchangeCalendarCache> {
        let session = self
            .ensure_saxo_session_json("exchange_calendar")
            .await
            .context("loading Saxo session for exchange calendar lookup")?;
        let data = self
            .fetch_saxo_exchange_summaries(&session)
            .await
            .context("fetching Saxo ref/v1/exchanges")?;
        let mut exchanges = HashMap::new();
        for exchange in default_exchanges() {
            let Some(summary) = data
                .iter()
                .find(|item| saxo_exchange_matches(item, exchange.code))
            else {
                continue;
            };
            let exchange_id = saxo_exchange_text(summary, "ExchangeId")
                .unwrap_or_else(|| exchange.code.to_string());
            let mut detail = summary.clone();
            if parse_saxo_exchange_sessions(&detail).is_empty() {
                match saxo_reference_get_json(
                    self,
                    &session,
                    &format!("/ref/v1/exchanges/{exchange_id}"),
                    &[],
                )
                .await
                {
                    Ok(value) => detail = value,
                    Err(err) => warn!(
                        exchange = exchange.code,
                        exchange_id, "Saxo exchange detail lookup failed: {err:#}"
                    ),
                }
            }
            if let Some(calendar) = saxo_exchange_calendar_from_detail(&detail, &exchange_id) {
                exchanges.insert(exchange.code.to_string(), calendar);
            }
        }
        if exchanges.is_empty() {
            bail!("Saxo ref/v1/exchanges did not match any configured exchange MICs");
        }
        Ok(SaxoExchangeCalendarCache {
            checked_date,
            checked_at: Utc::now(),
            exchanges,
            source: "saxo_ref_v1_exchanges".to_string(),
        })
    }

    async fn fetch_saxo_exchange_summaries(&self, session: &JsonValue) -> Result<Vec<JsonValue>> {
        let mut skip = 0usize;
        let top = 1000usize;
        let mut all = Vec::new();
        loop {
            let payload = saxo_reference_get_json(
                self,
                session,
                "/ref/v1/exchanges",
                &[("$skip", skip.to_string()), ("$top", top.to_string())],
            )
            .await?;
            let page = payload
                .get("Data")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| anyhow!("Saxo ref/v1/exchanges response did not contain Data"))?;
            let page_len = page.len();
            all.extend(page.iter().cloned());
            let total_count = payload
                .get("__count")
                .and_then(JsonValue::as_u64)
                .map(|value| value as usize);
            let has_next = payload
                .get("__next")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if page_len < top || !has_next || total_count.is_some_and(|total| all.len() >= total) {
                break;
            }
            skip += top;
            if skip > 10_000 {
                bail!("Saxo ref/v1/exchanges pagination exceeded 10000 rows");
            }
        }
        Ok(all)
    }

    pub async fn watchlists_payload(&self) -> Result<JsonValue> {
        // Quote- and decision-derived entries older than this are dropped.
        // The price monitor refreshes live symbols every few minutes, so an
        // old portfolio_price_snapshots row is an orphan of a former holding,
        // and recycling its sentiment ("Existing portfolio holding ...") into
        // new decision prompts misleads the model into suggesting SELLs of
        // positions the broker no longer holds.
        let stale_after_days = yaml_i64(
            &self.config,
            &["strategy", "swing", "position_decision_stale_after_days"],
        )
        .unwrap_or(7)
        .max(1);
        let stale_cutoff = (Utc::now() - Duration::days(stale_after_days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let configured_universe = configured_watchlist_universe_symbols(&self.config);
        // A versioned universe is the durable source of candidate membership.
        // Position, broker, fresh report, and explicit extra-watch inputs remain
        // additive. The archived sentiment table is only a migration fallback
        // for an installation that has not configured its universe yet.
        let legacy_archive_fallback = configured_universe.is_empty();
        if legacy_archive_fallback {
            warn!(
                "watchlist universe is not configured; retaining archived sentiment membership as a temporary fallback"
            );
        }
        let configured_extra_symbols = configured_extra_watch_symbols(&self.config);
        let mut seen = HashSet::new();
        let mut monitored = Vec::new();
        for row in self.position_items(250).await.unwrap_or_default() {
            let symbol = text_value(&row, "symbol");
            if !symbol.is_empty() && seen.insert(watchlist_symbol_key(&symbol)) {
                monitored.push(row);
            }
        }
        for row in self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, change_pct, currency, source, status FROM portfolio_price_snapshots ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            if text_value(&row, "updated_at") < stale_cutoff {
                if legacy_archive_fallback && seen.insert(watchlist_symbol_key(&symbol)) {
                    // Keep the symbol as a universe member only while
                    // migrating installations without a configured universe.
                    // The dead quote itself never masquerades as live data.
                    monitored.push(json!({
                        "symbol": symbol,
                        "quote_status": "stale_quote_dropped",
                        "source": "price_snapshot_archive",
                    }));
                }
                continue;
            }
            if !seen.insert(watchlist_symbol_key(&symbol)) {
                continue;
            }
            let mut item = row.as_object().cloned().unwrap_or_default();
            item.insert("instrument_name".to_string(), JsonValue::from(symbol));
            monitored.push(JsonValue::Object(item));
        }
        for row in self
            .select_json(
                "SELECT symbol, instrument_name, updated_at, quantity, currency, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, calculation_reliability FROM broker_instrument_exposures ORDER BY updated_at DESC, symbol ASC",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() || !seen.insert(watchlist_symbol_key(&symbol)) {
                continue;
            }
            monitored.push(row);
        }
        let decisions: HashMap<String, JsonValue> = self
            .latest_symbol_decisions()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, decision)| text_value(decision, "created_at") >= stale_cutoff)
            .collect();
        for (symbol, decision) in &decisions {
            if symbol.is_empty() || !seen.insert(watchlist_symbol_key(symbol)) {
                continue;
            }
            let source = decision.get("source").cloned().unwrap_or_else(|| json!({}));
            let technical = source
                .get("technical")
                .cloned()
                .unwrap_or_else(|| json!({}));
            monitored.push(json!({
                "symbol": symbol,
                "instrument_name": instrument_name_for_symbol(symbol),
                "updated_at": decision.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "currency": technical.get("currency").cloned().unwrap_or(JsonValue::Null),
                "current_price_local": technical.get("latest_close").cloned().unwrap_or(JsonValue::Null),
                "change_pct": JsonValue::Null,
                "market_value_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "quote_status": "decision_snapshot",
                "technical_status": technical.get("status").cloned().unwrap_or(JsonValue::Null),
                "source": source.get("source").cloned().unwrap_or_else(|| JsonValue::from("decision_report")),
                "decision": decision,
                "exchange": exchange_code(symbol).to_uppercase(),
                "region": exchange_region(symbol),
            }));
        }
        for row in self
            .select_json(
                "SELECT s.symbol, s.sentiment, s.confidence, s.macro_bias, s.rationale, s.source_json, s.report_id, dr.created_at AS decision_created_at, dr.status AS decision_status, dr.analysis_pulse_key, dr.analysis_pulse_label
                 FROM swing_sentiment_snapshots s
                 LEFT JOIN decision_reports dr ON dr.id = s.report_id
                 ORDER BY s.report_id DESC, s.id DESC
                 LIMIT 600",
            )
            .await
            .unwrap_or_default()
        {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            if text_value(&row, "decision_created_at") < stale_cutoff {
                if legacy_archive_fallback && seen.insert(watchlist_symbol_key(&symbol)) {
                    // Historic sentiment is a temporary membership fallback
                    // only. Its prices, rationale, and sentiment stay out of
                    // prompts and decision evidence.
                    monitored.push(json!({
                        "symbol": symbol,
                        "quote_status": "stale_history",
                        "source": "sentiment_archive",
                    }));
                }
                continue;
            }
            if !seen.insert(watchlist_symbol_key(&symbol)) {
                continue;
            }
            let source = row
                .get("source_json")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let technical = source
                .get("technical")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let decision = json!({
                "symbol": symbol,
                "report_id": row.get("report_id").cloned().unwrap_or(JsonValue::Null),
                "created_at": row.get("decision_created_at").cloned().unwrap_or(JsonValue::Null),
                "status": row.get("decision_status").cloned().unwrap_or(JsonValue::Null),
                "pulse_key": row.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                "pulse_label": row.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
                "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
                "confidence": value_f64(&row, "confidence"),
                "macro_bias": row.get("macro_bias").cloned().unwrap_or(JsonValue::Null),
                "rationale": row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                "source": source.clone(),
            });
            monitored.push(json!({
                "symbol": symbol,
                "instrument_name": instrument_name_for_symbol(&symbol),
                "currency": technical.get("currency").cloned().unwrap_or(JsonValue::Null),
                "current_price_local": technical.get("latest_close").cloned().unwrap_or(JsonValue::Null),
                "change_pct": JsonValue::Null,
                "market_value_dkk": 0.0,
                "daily_pnl_dkk": 0.0,
                "allocation_pct": 0.0,
                "quote_status": "decision_snapshot",
                "technical_status": technical.get("status").cloned().unwrap_or(JsonValue::Null),
                "source": "swing_sentiment_snapshots",
                "decision": decision,
                "exchange": exchange_code(&symbol).to_uppercase(),
                "region": exchange_region(&symbol),
            }));
        }
        let mut configured_symbols_added = 0usize;
        for symbol in configured_universe {
            if seen.insert(watchlist_symbol_key(&symbol)) {
                monitored.push(configured_watchlist_row(
                    &symbol,
                    "configured_analysis_universe",
                ));
                configured_symbols_added += 1;
            }
        }
        let mut extra_symbols_added = 0usize;
        for symbol in configured_extra_symbols {
            if seen.insert(watchlist_symbol_key(&symbol)) {
                monitored.push(configured_watchlist_row(&symbol, "configured_extra_watch"));
                extra_symbols_added += 1;
            }
        }
        let indicator_support_by_symbol: HashMap<String, JsonValue> = self
            .select_json(
                "SELECT symbol, run_date, status, nearest_support, next_support,
                        downside_to_support_pct, downside_after_break_pct,
                        support_break_risk, support_break_risk_label, support_confidence,
                        support_history_coverage, support_touch_count
                 FROM daily_indicator_signals
                 WHERE run_id = (
                    SELECT id FROM daily_indicator_runs
                    ORDER BY run_date DESC, created_at DESC LIMIT 1
                 )",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (watchlist_symbol_key(&text_value(&row, "symbol")), row))
            .collect();
        for item in &mut monitored {
            let symbol = text_value(item, "symbol");
            if let Some(obj) = item.as_object_mut() {
                obj.entry("decision".to_string())
                    .or_insert_with(|| decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null));
                obj.entry("exchange".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_code(&symbol).to_uppercase()));
                obj.entry("region".to_string())
                    .or_insert_with(|| JsonValue::from(exchange_region(&symbol)));
                obj.entry("instrument_name".to_string())
                    .or_insert_with(|| JsonValue::from(instrument_name_for_symbol(&symbol)));
                obj.entry("quote_status".to_string())
                    .or_insert_with(|| JsonValue::from("current_source"));
                if let Some(indicator) =
                    indicator_support_by_symbol.get(&watchlist_symbol_key(&symbol))
                {
                    obj.insert(
                        "technical_risk".to_string(),
                        json!({
                            "run_date": indicator.get("run_date").cloned().unwrap_or(JsonValue::Null),
                            "status": indicator.get("status").cloned().unwrap_or(JsonValue::Null),
                            "nearest_support": indicator.get("nearest_support").cloned().unwrap_or(JsonValue::Null),
                            "next_support": indicator.get("next_support").cloned().unwrap_or(JsonValue::Null),
                            "downside_to_support_pct": indicator.get("downside_to_support_pct").cloned().unwrap_or(JsonValue::Null),
                            "downside_after_break_pct": indicator.get("downside_after_break_pct").cloned().unwrap_or(JsonValue::Null),
                            "break_risk": indicator.get("support_break_risk").cloned().unwrap_or(JsonValue::Null),
                            "break_risk_label": indicator.get("support_break_risk_label").cloned().unwrap_or(JsonValue::Null),
                            "confidence": indicator.get("support_confidence").cloned().unwrap_or(JsonValue::Null),
                            "history_coverage": indicator.get("support_history_coverage").cloned().unwrap_or(JsonValue::Null),
                            "touch_count": indicator.get("support_touch_count").cloned().unwrap_or(JsonValue::Null),
                        }),
                    );
                }
            }
        }
        let mut nordic = Vec::new();
        let mut uk = Vec::new();
        let mut us = Vec::new();
        let mut eu = Vec::new();
        for item in &monitored {
            match exchange_region(&text_value(item, "symbol")).as_str() {
                "Nordics" => nordic.push(item.clone()),
                "UK" => uk.push(item.clone()),
                "US" => us.push(item.clone()),
                _ => eu.push(item.clone()),
            }
        }
        let nordic_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "nordic_limit"]).unwrap_or(100);
        let uk_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "uk_limit"]).unwrap_or(25);
        let us_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "us_limit"]).unwrap_or(100);
        let eu_limit =
            yaml_i64(&self.config, &["market_data", "watchlists", "eu_limit"]).unwrap_or(75);
        Ok(json!({
            "generated_at": Utc::now().to_rfc3339(),
            "cache_ttl_seconds": 300,
            "universe": {
                "source": if legacy_archive_fallback { "legacy_sentiment_archive_fallback" } else { "configured_analysis_universe" },
                "configured_symbol_count": configured_watchlist_universe_symbols(&self.config).len(),
                "configured_symbols_added": configured_symbols_added,
                "extra_symbols_added": extra_symbols_added,
            },
            "categories": [
                {"key": "all", "label": "All monitored", "target_limit": monitored.len(), "total_universe": monitored.len(), "items": monitored},
                {"key": "nordic", "label": "Nordics", "target_limit": nordic_limit, "total_universe": nordic.len(), "items": nordic},
                {"key": "uk", "label": "UK", "target_limit": uk_limit, "total_universe": uk.len(), "items": uk},
                {"key": "us", "label": "US", "target_limit": us_limit, "total_universe": us.len(), "items": us},
                {"key": "eu", "label": "Europe", "target_limit": eu_limit, "total_universe": eu.len(), "items": eu}
            ],
        }))
    }

    pub async fn localization_for_user(
        &self,
        mut prefs: LocalizationPrefs,
        sso_session: &JsonValue,
    ) -> LocalizationPrefs {
        let key = localization_settings_key(sso_session);
        match self.runtime_setting(&key).await {
            Ok(Some(value)) => prefs.apply_settings_json(&value),
            Ok(None) => {}
            Err(err) => warn!(key = %key, "localization settings lookup failed: {err:#}"),
        }
        prefs
    }

    pub async fn save_localization_settings(
        &self,
        sso_session: &JsonValue,
        mut value: JsonValue,
    ) -> Result<JsonValue> {
        let key = localization_settings_key(sso_session);
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "updated_at".to_string(),
                JsonValue::from(Utc::now().to_rfc3339()),
            );
        }
        self.save_runtime_setting(&key, &value).await?;
        Ok(value)
    }

    async fn latest_batch_id(&self) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT batch_id FROM import_batches ORDER BY imported_at DESC, batch_id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| row.try_get::<String, _>("batch_id").ok()))
    }

    async fn broker_positions_available(&self) -> Result<bool> {
        let row = self
            .first_json("SELECT COUNT(*) AS count FROM broker_position_snapshots")
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count") > 0)
    }

    async fn effective_position_rows(&self, limit: Option<i64>) -> Result<Vec<JsonValue>> {
        let latest_batch = self.latest_batch_id().await?;
        let where_clause = match latest_batch {
            Some(batch_id) => format!(
                "WHERE batch_id = '{}' AND excluded = 0",
                sql_escape(&batch_id)
            ),
            None => "WHERE excluded = 0".to_string(),
        };
        let base_rows = self
            .select_json(&format!(
                "SELECT instrument_name, symbol, isin, quantity, currency, open_price_local, open_price_local AS paid_price_local, current_price_local, cost_basis_local, cost_basis_dkk, market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk, allocation_pct, asset_class, market_status, value_date FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default();
        let broker_rows = self
            .select_json(
                "SELECT symbol, updated_at, instrument_name, isin, uic, asset_type, quantity, currency, open_price_local, open_price_including_costs_local, execution_time_open, value_date, market_state, can_be_closed FROM broker_position_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default();
        if broker_rows.is_empty() {
            let mut rows = base_rows;
            rows.sort_by(|left, right| {
                value_f64(right, "market_value_dkk")
                    .partial_cmp(&value_f64(left, "market_value_dkk"))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
            });
            if let Some(limit) = limit {
                rows.truncate(clamp_limit(limit, 1, 250) as usize);
            }
            return Ok(rows);
        }

        let base_by_symbol = base_rows
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let price_by_symbol = self
            .select_json(
                "SELECT symbol, updated_at, current_price_local, current_fx_rate_to_dkk, baseline_price_local, baseline_fx_rate_to_dkk, change_pct, currency, status FROM portfolio_price_snapshots ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let exposure_by_symbol = self
            .select_json(
                "SELECT symbol, quantity, average_open_price, profit_loss_on_trade, instrument_price_day_percent_change, currency, calculation_reliability FROM broker_instrument_exposures ORDER BY symbol ASC",
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| (text_value(&row, "symbol"), row))
            .collect::<HashMap<_, _>>();
        let account_currency = self
            .first_json("SELECT account_currency FROM broker_account_snapshots WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .and_then(|row| row.get("account_currency").cloned())
            .and_then(|value| value.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "DKK".to_string());
        let account_fx_rate =
            crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &account_currency).await;
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");

        let mut rows = Vec::new();
        for broker in broker_rows {
            let symbol = text_value(&broker, "symbol");
            let quantity = value_f64(&broker, "quantity");
            if symbol.is_empty() || quantity <= 1e-9 {
                continue;
            }
            let base = base_by_symbol.get(&symbol);
            let price = price_by_symbol.get(&symbol);
            let exposure = exposure_by_symbol.get(&symbol);
            let currency = text_value(&broker, "currency")
                .trim()
                .to_string()
                .if_empty_then(|| {
                    price
                        .map(|row| text_value(row, "currency"))
                        .filter(|value| !value.is_empty())
                })
                .or_else(|| base.map(|row| text_value(row, "currency")))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "DKK".to_string());
            let broker_open_price = value_f64(&broker, "open_price_including_costs_local")
                .max(value_f64(&broker, "open_price_local"));
            let base_quantity = base.map(|row| value_f64(row, "quantity")).unwrap_or(0.0);
            let base_market_local = base
                .map(|row| value_f64(row, "market_value_local"))
                .unwrap_or(0.0);
            let base_market_dkk = base
                .map(|row| value_f64(row, "market_value_dkk"))
                .unwrap_or(0.0);
            let inferred_fx_rate = if base_market_local.abs() > 1e-9 {
                base_market_dkk / base_market_local
            } else {
                crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &currency).await
            };
            let current_price_local = price
                .map(|row| value_f64(row, "current_price_local"))
                .filter(|value| *value > 0.0)
                .or_else(|| {
                    base.map(|row| value_f64(row, "current_price_local"))
                        .filter(|value| *value > 0.0)
                })
                .unwrap_or(broker_open_price);
            let current_fx_rate = price
                .map(|row| value_f64(row, "current_fx_rate_to_dkk"))
                .filter(|value| *value > 0.0)
                .unwrap_or(inferred_fx_rate);
            let unit_cost_dkk = if base_quantity > 0.0 {
                value_f64(base.unwrap(), "cost_basis_dkk") / base_quantity
            } else {
                broker_open_price * current_fx_rate
            };
            let cost_basis_dkk = unit_cost_dkk * quantity;
            let cost_basis_local_total = if base_quantity > 0.0 {
                let base_cost_local_total = value_f64(base.unwrap(), "cost_basis_local");
                if base_cost_local_total > 0.0 {
                    base_cost_local_total / base_quantity * quantity
                } else {
                    broker_open_price * quantity
                }
            } else {
                broker_open_price * quantity
            };
            let market_value_dkk = quantity * current_price_local * current_fx_rate;
            let daily_pnl_dkk = match price {
                Some(price) if value_f64(price, "baseline_price_local") > 0.0 => {
                    quantity
                        * (current_price_local * current_fx_rate
                            - value_f64(price, "baseline_price_local")
                                * value_f64(price, "baseline_fx_rate_to_dkk"))
                }
                _ if base_quantity > 0.0 => {
                    value_f64(base.unwrap(), "daily_pnl_dkk") * quantity / base_quantity
                }
                _ => 0.0,
            };
            let unrealised_pnl_dkk = exposure
                .map(|row| value_f64(row, "profit_loss_on_trade"))
                .filter(|value| value.abs() > 1e-9)
                .map(|value| value * account_fx_rate)
                .unwrap_or(market_value_dkk - cost_basis_dkk);
            rows.push(json!({
                "instrument_name": text_value(&broker, "instrument_name")
                    .if_empty_then(|| base.map(|row| text_value(row, "instrument_name")))
                    .unwrap_or_else(|| instrument_name_for_symbol(&symbol)),
                "symbol": symbol,
                "isin": broker.get("isin").cloned().unwrap_or(JsonValue::Null),
                "quantity": quantity,
                "currency": currency,
                "paid_price_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                "open_price_local": broker_open_price,
                "cost_basis_local": if quantity > 0.0 { cost_basis_local_total / quantity } else { broker_open_price },
                "current_price_local": current_price_local,
                "cost_basis_dkk": cost_basis_dkk,
                "market_value_dkk": market_value_dkk,
                "unrealised_pnl_dkk": unrealised_pnl_dkk,
                "daily_pnl_dkk": daily_pnl_dkk,
                "daily_change_pct": exposure.map(|row| value_f64(row, "instrument_price_day_percent_change")).unwrap_or(0.0),
                "total_return_pct": if cost_basis_dkk.abs() > 1e-9 { unrealised_pnl_dkk / cost_basis_dkk } else { 0.0 },
                "allocation_pct": 0.0,
                "asset_class": text_value(&broker, "asset_type")
                    .if_empty_then(|| base.map(|row| text_value(row, "asset_class")))
                    .unwrap_or_else(|| "Equity".to_string()),
                "market_status": "Saxo broker snapshot",
                "value_date": broker.get("value_date").cloned().unwrap_or(JsonValue::Null),
                "latest_quote_updated_at": price.and_then(|row| row.get("updated_at")).cloned().unwrap_or(JsonValue::Null),
                "quote_status": price.and_then(|row| row.get("status")).cloned().unwrap_or_else(|| JsonValue::from("broker_snapshot")),
                "broker_profit_loss_on_trade": exposure.map(|row| value_f64(row, "profit_loss_on_trade")).unwrap_or(0.0),
                "broker_calculation_reliability": exposure.and_then(|row| row.get("calculation_reliability")).cloned().unwrap_or(JsonValue::Null),
            }));
        }
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let total_value = invested + cash_balance;
        for row in &mut rows {
            let market_value_dkk = value_f64(row, "market_value_dkk");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "allocation_pct".to_string(),
                    JsonValue::from(if total_value > 0.0 {
                        market_value_dkk / total_value
                    } else {
                        0.0
                    }),
                );
            }
        }
        rows.sort_by(|left, right| {
            value_f64(right, "market_value_dkk")
                .partial_cmp(&value_f64(left, "market_value_dkk"))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| text_value(left, "symbol").cmp(&text_value(right, "symbol")))
        });
        if let Some(limit) = limit {
            rows.truncate(clamp_limit(limit, 1, 250) as usize);
        }
        Ok(rows)
    }

    async fn position_aggregate(&self, batch_id: Option<&str>) -> Result<JsonValue> {
        let rows = if self.broker_positions_available().await? {
            self.effective_position_rows(None).await?
        } else {
            let where_clause = match batch_id {
                Some(batch_id) => format!(
                    "WHERE batch_id = '{}' AND excluded = 0",
                    sql_escape(batch_id)
                ),
                None => "WHERE excluded = 0".to_string(),
            };
            self.select_json(&format!(
                "SELECT market_value_dkk, cost_basis_dkk, unrealised_pnl_dkk, daily_pnl_dkk FROM position_snapshots {where_clause}"
            ))
            .await
            .unwrap_or_default()
        };
        let invested = rows
            .iter()
            .map(|row| value_f64(row, "market_value_dkk"))
            .sum::<f64>();
        let cash_summary = self.cash_summary_from_ledger().await?;
        let cash_balance = value_f64(&cash_summary, "cash_balance_dkk");
        let initial_cash = value_f64(&cash_summary, "initial_cash_dkk");
        let cash_from_trades = value_f64(&cash_summary, "cash_from_trades_dkk");
        Ok(json!({
            "total_market_value_dkk": invested + cash_balance,
            "invested_market_value_dkk": invested,
            "cash_balance_dkk": cash_balance,
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "total_cost_basis_dkk": rows.iter().map(|row| value_f64(row, "cost_basis_dkk")).sum::<f64>(),
            "total_unrealised_pnl_dkk": rows.iter().map(|row| value_f64(row, "unrealised_pnl_dkk")).sum::<f64>(),
            "total_daily_pnl_dkk": rows.iter().map(|row| value_f64(row, "daily_pnl_dkk")).sum::<f64>(),
            "position_count": rows.len() as i64,
            "source": if self.broker_positions_available().await? { "saxo_broker_snapshot" } else { "position_snapshots" }
        }))
    }

    async fn cash_summary_from_ledger(&self) -> Result<JsonValue> {
        let initial_cash =
            yaml_f64(&self.config, &["portfolio", "initial_cash_dkk"]).unwrap_or(0.0);
        let row = self
            .first_json(
                "SELECT COALESCE(SUM(net_amount_dkk), 0) AS cash_from_trades_dkk FROM trade_ledger WHERE status IN ('executed', 'approved')",
            )
            .await?
            .unwrap_or_else(|| json!({}));
        let cash_from_trades = value_f64(&row, "cash_from_trades_dkk");
        Ok(json!({
            "initial_cash_dkk": initial_cash,
            "cash_from_trades_dkk": cash_from_trades,
            "cash_balance_dkk": initial_cash + cash_from_trades,
        }))
    }

    async fn overview_integrity(
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
        if broker_exposures.is_empty() {
            checks.insert(
                "broker_exposure_aggregate".to_string(),
                json!("skipped_no_snapshot"),
            );
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
            let broker_account_fx =
                crate::fx::cached_or_static_fx_rate_to_dkk(&self.pool, &broker_account_currency)
                    .await;
            let exposure_unrealised_pnl_dkk = broker_exposures
                .iter()
                .map(|row| value_f64(row, "profit_loss_on_trade") * broker_account_fx)
                .sum::<f64>();
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
                    "broker_account_fx_rate_to_dkk": broker_account_fx,
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
                .map(|row| (text_value(&row, "symbol"), row))
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
            .select_json(&format!(
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
                        AND created_at < '{}') \
                    OR (status = 'executed' \
                        AND ledger_id IS NULL \
                        AND created_at < '{}') \
                 ORDER BY created_at ASC, id ASC \
                 LIMIT 20",
                sql_escape(&stale_cutoff),
                sql_escape(&fill_cutoff)
            ))
            .await
            .unwrap_or_default();
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
            "expiry_pending_orders": expiry_pending_orders,
            "acknowledgements": acknowledgements
                .get("acknowledgements")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "acknowledged_issue_count": acknowledged_issue_count,
            "checks": checks,
            "checked_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }))
    }

    pub async fn position_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let decisions = self.latest_symbol_decisions().await.unwrap_or_default();
        let mut rows = self.effective_position_rows(Some(limit)).await?;
        for row in &mut rows {
            let symbol = text_value(row, "symbol");
            if let Some(obj) = row.as_object_mut() {
                obj.insert(
                    "ladder_status".to_string(),
                    json!({"text": "idle", "active_orders": 0, "filled_entry_rungs": 0, "total_entry_rungs": 0, "progress_pct": 0.0, "trailing": false}),
                );
                obj.insert(
                    "decision".to_string(),
                    decisions.get(&symbol).cloned().unwrap_or(JsonValue::Null),
                );
                obj.entry("latest_quote_updated_at".to_string())
                    .or_insert(JsonValue::Null);
            }
        }
        Ok(rows)
    }

    async fn latest_symbol_decisions(&self) -> Result<HashMap<String, JsonValue>> {
        let Some(report) = self
            .first_json(
                "SELECT dr.id, dr.created_at, dr.status, dr.analysis_pulse_key, dr.analysis_pulse_label
                 FROM decision_reports dr
                 WHERE dr.report_json IS NOT NULL
                   AND (
                     EXISTS (SELECT 1 FROM swing_sentiment_snapshots s WHERE s.report_id = dr.id)
                     OR EXISTS (SELECT 1 FROM swing_position_targets t WHERE t.report_id = dr.id)
                   )
                 ORDER BY dr.id DESC
                 LIMIT 1",
            )
            .await?
        else {
            return Ok(HashMap::new());
        };
        let report_id = value_i64(&report, "id");
        let mut decisions = HashMap::new();
        let sentiment_rows = self
            .select_json(&format!(
                "SELECT symbol, sentiment, confidence, macro_bias, rationale, catalysts_json, risk_notes_json, source_json FROM swing_sentiment_snapshots WHERE report_id = {} ORDER BY symbol ASC, id DESC",
                report_id
            ))
            .await
            .unwrap_or_default();
        for row in sentiment_rows {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            decisions.insert(
                symbol.clone(),
                json!({
                    "symbol": symbol,
                    "report_id": report_id,
                    "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                    "pulse_key": report.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                    "pulse_label": report.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
                    "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null),
                    "confidence": value_f64(&row, "confidence"),
                    "macro_bias": row.get("macro_bias").cloned().unwrap_or(JsonValue::Null),
                    "rationale": row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                    "catalysts": row.get("catalysts_json").cloned().unwrap_or_else(|| json!([])),
                    "risk_notes": row.get("risk_notes_json").cloned().unwrap_or_else(|| json!([])),
                    "source": row.get("source_json").cloned().unwrap_or_else(|| json!({})),
                }),
            );
        }
        let target_rows = self
            .select_json(&format!(
                "SELECT symbol, sentiment, action, current_weight_pct, target_weight_pct, current_quantity, target_quantity, estimated_delta_quantity, estimated_value_dkk, priority, confidence, rationale, risk_json FROM swing_position_targets WHERE report_id = {} ORDER BY symbol ASC, id DESC",
                report_id
            ))
            .await
            .unwrap_or_default();
        for row in target_rows {
            let symbol = text_value(&row, "symbol");
            if symbol.is_empty() {
                continue;
            }
            let entry = decisions.entry(symbol.clone()).or_insert_with(|| {
                json!({
                    "symbol": symbol,
                    "report_id": report_id,
                    "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                    "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                    "sentiment": row.get("sentiment").cloned().unwrap_or(JsonValue::Null)
                })
            });
            if let Some(obj) = entry.as_object_mut() {
                obj.insert(
                    "action".to_string(),
                    row.get("action").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "priority".to_string(),
                    row.get("priority").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "target_confidence".to_string(),
                    JsonValue::from(value_f64(&row, "confidence")),
                );
                obj.insert(
                    "target_rationale".to_string(),
                    row.get("rationale").cloned().unwrap_or(JsonValue::Null),
                );
                obj.insert(
                    "current_weight_pct".to_string(),
                    JsonValue::from(value_f64(&row, "current_weight_pct")),
                );
                obj.insert(
                    "target_weight_pct".to_string(),
                    JsonValue::from(value_f64(&row, "target_weight_pct")),
                );
                obj.insert(
                    "risk".to_string(),
                    row.get("risk_json").cloned().unwrap_or_else(|| json!({})),
                );
            }
        }
        Ok(decisions)
    }

    pub async fn execution_orders(&self, limit: i64) -> Result<Vec<JsonValue>> {
        self.execution_orders_page(limit, 0).await
    }

    pub async fn execution_orders_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_id, symbol, action, order_type, mode, status, adapter, quantity, price_local, limit_price_local, stop_price_local, currency, estimated_value_dkk, approval_required, approved_at, ledger_id, parent_execution_order_id, strategy_type, strategy_session, strategy_key, strategy_role, error_text, broker_order_id, execution_result_json FROM execution_orders ORDER BY created_at DESC, id DESC LIMIT {} OFFSET {}",
            clamp_limit(limit, 1, 500),
            offset.max(0).min(100_000),
        );
        let mut orders = self.select_json(&sql).await.unwrap_or_default();
        let market_rows = self.market_exchange_rows();
        for order in &mut orders {
            enrich_execution_order_lifecycle(order, &market_rows);
            match self.execution_order_attribution(order).await {
                Ok(attribution) => {
                    if let Some(object) = order.as_object_mut() {
                        object.insert("attribution".to_string(), attribution);
                    }
                }
                Err(err) => {
                    warn!("execution attribution degraded: {err:#}");
                }
            }
        }
        Ok(orders)
    }

    pub async fn execution_orders_count(&self) -> Result<i64> {
        let row = self
            .first_json("SELECT COUNT(*) AS count FROM execution_orders")
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count"))
    }

    async fn execution_order_attribution(&self, order: &JsonValue) -> Result<JsonValue> {
        let report_id = value_i64(order, "report_id");
        let symbol = json_text(order, "symbol");
        let action = json_text(order, "action");
        let strategy_key = json_text(order, "strategy_key");

        let report = if report_id > 0 {
            self.first_json(&format!(
                "SELECT id, created_at, status, model, analysis_pulse_key, analysis_pulse_label
                 FROM decision_reports WHERE id = {} LIMIT 1",
                report_id
            ))
            .await?
            .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };

        let manager_run = if report_id > 0 {
            self.first_json(&format!(
                "SELECT id, created_at, status, manager_key, manager_kind, manager_label, manager_json, queue_result_json
                 FROM trading_manager_runs
                 WHERE report_id = {}
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                report_id
            ))
            .await?
            .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };

        let advice = if report_id > 0 {
            self.hermes_decision_advice_by_report(report_id)
                .await?
                .unwrap_or(JsonValue::Null)
        } else {
            JsonValue::Null
        };
        let hermes_order = matching_order_advice(
            advice.get("order_advice_json"),
            &strategy_key,
            &symbol,
            &action,
        )
        .or_else(|| {
            manager_run
                .get("manager_json")
                .and_then(|value| value.get("hermes_decision_advice"))
                .and_then(|value| value.get("raw"))
                .and_then(|value| value.get("order_advice_json"))
                .and_then(|value| {
                    matching_order_advice(Some(value), &strategy_key, &symbol, &action)
                })
        })
        .unwrap_or(JsonValue::Null);

        let manager_order = manager_run
            .get("manager_json")
            .and_then(|value| value.get("approved_orders"))
            .and_then(|value| matching_order_advice(Some(value), &strategy_key, &symbol, &action))
            .map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("manager_decision".to_string(), JsonValue::from("approved"));
                }
                value
            })
            .or_else(|| {
                manager_run
                    .get("manager_json")
                    .and_then(|value| value.get("skipped_orders"))
                    .and_then(|value| {
                        matching_order_advice(Some(value), &strategy_key, &symbol, &action)
                    })
                    .map(|mut value| {
                        if let Some(object) = value.as_object_mut() {
                            object
                                .insert("manager_decision".to_string(), JsonValue::from("skipped"));
                        }
                        value
                    })
            })
            .unwrap_or(JsonValue::Null);

        let preflight_candidate =
            matching_manager_preflight_candidate(&manager_run, &strategy_key, &symbol, &action);
        let technical = compact_attribution_technical(
            manager_order
                .get("final_technical")
                .unwrap_or(&JsonValue::Null),
            "manager_final",
        );
        let technical = if technical.is_null() {
            let preflight = compact_attribution_technical(
                preflight_candidate
                    .get("technical")
                    .unwrap_or(&JsonValue::Null),
                "manager_preflight",
            );
            if preflight.is_null() {
                compact_attribution_technical(
                    &self.latest_indicator_signal_summary(&symbol).await?,
                    "latest_fallback",
                )
            } else {
                preflight
            }
        } else {
            technical
        };
        let markov = compact_attribution_markov(
            preflight_candidate
                .get("markov")
                .unwrap_or(&JsonValue::Null),
            "manager_preflight",
        );
        let markov = if markov.is_null() {
            compact_attribution_markov(
                &self.latest_markov_signal_summary(&symbol).await?,
                "latest_fallback",
            )
        } else {
            markov
        };
        let capital = manager_run
            .get("manager_json")
            .and_then(|value| value.get("capital_budget"))
            .map(compact_attribution_capital)
            .unwrap_or(JsonValue::Null);
        let ledger_outcome = match self.execution_order_ledger_outcome(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution ledger attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let delta = attribution_delta_label(&hermes_order, &manager_order, order);

        Ok(json!({
            "delta": delta,
            "report": {
                "id": report.get("id").cloned().unwrap_or(JsonValue::Null),
                "created_at": report.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "status": report.get("status").cloned().unwrap_or(JsonValue::Null),
                "model": report.get("model").cloned().unwrap_or(JsonValue::Null),
                "pulse_key": report.get("analysis_pulse_key").cloned().unwrap_or(JsonValue::Null),
                "pulse_label": report.get("analysis_pulse_label").cloned().unwrap_or(JsonValue::Null),
            },
            "trading_manager": {
                "run_id": manager_run.get("id").cloned().unwrap_or(JsonValue::Null),
                "status": manager_run.get("status").cloned().unwrap_or(JsonValue::Null),
                "manager_key": manager_run.get("manager_key").cloned().unwrap_or(JsonValue::Null),
                "decision": manager_order,
            },
            "hermes": {
                "advice_id": advice.get("id").cloned().unwrap_or(JsonValue::Null),
                "status": advice.get("status").cloned().unwrap_or(JsonValue::Null),
                "recommendation": advice.get("overall_recommendation").cloned().unwrap_or(JsonValue::Null),
                "summary": advice.get("summary").cloned().unwrap_or(JsonValue::Null),
                "order_advice": hermes_order,
            },
            "technical": technical,
            "markov": markov,
            "capital_budget": capital,
            "ledger_outcome": ledger_outcome,
        }))
    }

    async fn execution_order_ledger_outcome(&self, order: &JsonValue) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let status = json_text(order, "status");
        let ledger_id = value_i64(order, "ledger_id");
        if order_id <= 0 || (ledger_id <= 0 && status != "broker_partially_filled") {
            return Ok(JsonValue::Null);
        }

        let fill_summary = self
            .first_json(&format!(
                "SELECT COUNT(f.id) AS fill_count,
                        COUNT(l.id) AS ledger_entry_count,
                        COALESCE(SUM(f.delta_quantity), 0) AS filled_quantity,
                        MAX(f.created_at) AS last_fill_at,
                        COALESCE(SUM(l.commission_dkk), 0) AS commission_dkk,
                        COALESCE(SUM(l.tax_dkk), 0) AS tax_dkk,
                        COALESCE(SUM(l.realised_gain_dkk), 0) AS realised_gain_dkk,
                        COALESCE(SUM(l.cost_basis_sold_dkk), 0) AS cost_basis_sold_dkk
                 FROM execution_fills f
                 LEFT JOIN trade_ledger l ON l.id = f.ledger_id
                 WHERE f.execution_order_id = {}",
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let outcome = compact_execution_ledger_outcome(order, &fill_summary, "reconciled_fills");
        if !outcome.is_null() || ledger_id <= 0 {
            return Ok(outcome);
        }

        let legacy_ledger = self
            .first_json(&format!(
                "SELECT 1 AS fill_count,
                        1 AS ledger_entry_count,
                        quantity AS filled_quantity,
                        created_at AS last_fill_at,
                        commission_dkk,
                        tax_dkk,
                        realised_gain_dkk,
                        cost_basis_sold_dkk
                 FROM trade_ledger
                 WHERE id = {}
                 LIMIT 1",
                ledger_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(compact_execution_ledger_outcome(
            order,
            &legacy_ledger,
            "legacy_order_ledger",
        ))
    }

    async fn latest_indicator_signal_summary(&self, symbol: &str) -> Result<JsonValue> {
        let sql = format!(
            "SELECT run_date, status, close, rsi14, macd_histogram, atr14, reward_risk,
                    nearest_support, next_support, downside_to_support_pct,
                    downside_after_break_pct, support_break_risk, support_break_risk_label,
                    support_confidence, support_history_coverage, support_touch_count,
                    trend_bias, sentiment, confluence_count, min_confluences, error_text
             FROM daily_indicator_signals
             WHERE symbol = '{}' AND run_id = (
                SELECT id FROM daily_indicator_runs ORDER BY run_date DESC, created_at DESC LIMIT 1
             )
             LIMIT 1",
            sql_escape(symbol)
        );
        Ok(self.first_json(&sql).await?.unwrap_or(JsonValue::Null))
    }

    async fn latest_markov_signal_summary(&self, symbol: &str) -> Result<JsonValue> {
        let sql = format!(
            "SELECT run_date, status, current_state, current_close, rolling_return,
                    bull_prob, sideways_prob, bear_prob, signed_signal, direction, conviction,
                    error_text
             FROM markov_asset_signals
             WHERE symbol = '{}' AND run_id = (
                SELECT id FROM markov_signal_runs ORDER BY run_date DESC, created_at DESC LIMIT 1
             )
             LIMIT 1",
            sql_escape(symbol)
        );
        Ok(self.first_json(&sql).await?.unwrap_or(JsonValue::Null))
    }

    pub async fn execution_fills(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_fills ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn execution_events(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM execution_order_events ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 500)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_summaries(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_SUMMARY_COLUMNS} FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_summary(&self, report_id: i64) -> Result<Option<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_SUMMARY_COLUMNS} FROM decision_reports WHERE id = {} LIMIT 1",
            report_id.max(0)
        );
        self.first_json(&sql).await
    }

    pub async fn decision_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_DETAIL_COLUMNS} FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn decision_report_item(&self, report_id: i64) -> Result<Option<JsonValue>> {
        let sql = format!(
            "SELECT {DECISION_REPORT_DETAIL_COLUMNS} FROM decision_reports WHERE id = {} LIMIT 1",
            report_id.max(0)
        );
        self.first_json(&sql).await
    }

    async fn attach_decision_candidate_waterfall(&self, mut report: JsonValue) -> JsonValue {
        let report_id = value_i64(&report, "id");
        if report_id <= 0 {
            return report;
        }
        match self.decision_candidate_waterfall(report_id).await {
            Ok(waterfall) => {
                if let Some(object) = report.as_object_mut() {
                    object.insert("candidate_scoring_waterfall".to_string(), waterfall);
                }
            }
            Err(err) => {
                warn!(report_id, "candidate scoring waterfall degraded: {err:#}");
            }
        }
        report
    }

    async fn decision_candidate_waterfall(&self, report_id: i64) -> Result<JsonValue> {
        let run = self
            .first_json(&format!(
                "SELECT id, created_at, status, manager_json
                 FROM trading_manager_runs
                 WHERE report_id = {}
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                report_id.max(0)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(candidate_scoring_waterfall_from_manager_run(&run))
    }

    pub async fn decision_gate_replay(&self, limit: i64) -> Result<JsonValue> {
        let sql = format!(
            "SELECT id, report_id, created_at, status, manager_json
             FROM trading_manager_runs
             WHERE manager_json IS NOT NULL
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        let runs = self.select_json(&sql).await?;
        let mut replay = gate_replay_from_manager_runs(&runs);
        let support_risk_evidence = self.support_risk_evidence().await.unwrap_or_else(|err| {
            warn!("support-risk evidence projection degraded: {err:#}");
            json!({
                "status": "unavailable",
                "safety": "read_only_observation_of_stored_daily_indicator_closes",
                "interpretation": "Support-risk evidence could not be loaded. It does not affect gates, Hermes, configuration, or Saxo orders.",
            })
        });
        replay["support_risk_evidence"] = support_risk_evidence;
        Ok(replay)
    }

    async fn support_risk_evidence(&self) -> Result<JsonValue> {
        let cutoff = (Utc::now().date_naive()
            - Duration::days(SUPPORT_RISK_EVIDENCE_LOOKBACK_DAYS))
        .to_string();
        let rows = self
            .select_json(&format!(
                "SELECT symbol, run_date, close, support_break_risk, support_break_risk_label,
                        support_confidence, support_history_coverage
                 FROM daily_indicator_signals
                 WHERE status = 'ok'
                   AND close > 0
                   AND run_date >= '{}'
                 ORDER BY symbol ASC, run_date ASC, created_at ASC",
                sql_escape(&cutoff)
            ))
            .await?;
        Ok(support_risk_evidence_from_indicator_rows(&rows))
    }

    pub async fn decision_pulse_statuses(&self) -> Result<Vec<JsonValue>> {
        let pulses = [
            (
                "europe_open_followup",
                "europe_open_followup:",
                "Nordic/EU Open +1h15",
            ),
            ("us_open_followup", "us_open_followup:", "US Open +1h15"),
            ("manual", "manual:", "Manual / Dry Run"),
        ];
        let attempt_cutoff =
            (Utc::now() - Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut statuses = Vec::new();
        for (key, prefix, label) in pulses {
            let latest = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let last_success = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND status IN ('completed', 'xai_fallback')
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let last_failure = self
                .first_json(&format!(
                    "SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND status IN ('xai_error', 'error', 'failed', 'parse_error')
                     ORDER BY created_at DESC, id DESC
                     LIMIT 1",
                    sql_escape(prefix)
                ))
                .await?
                .unwrap_or(JsonValue::Null);
            let attempts = self
                .first_json(&format!(
                    "SELECT COUNT(*) AS attempts_7d
                     FROM decision_reports
                     WHERE analysis_pulse_key LIKE '{}%'
                       AND created_at >= '{}'",
                    sql_escape(prefix),
                    sql_escape(&attempt_cutoff)
                ))
                .await?
                .unwrap_or_else(|| json!({"attempts_7d": 0}));
            statuses.push(json!({
                "key": key,
                "prefix": prefix,
                "label": label,
                "latest": latest,
                "last_success": last_success,
                "last_failure": last_failure,
                "attempts_7d": value_i64(&attempts, "attempts_7d"),
            }));
        }
        Ok(statuses)
    }

    pub async fn markov_signals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        crate::markov_method::latest_markov_signals(self, limit).await
    }

    pub async fn markov_signals_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        crate::markov_method::latest_markov_signals_page(self, limit, offset).await
    }

    pub async fn markov_signals_count(&self) -> Result<i64> {
        crate::markov_method::latest_markov_signal_count(self).await
    }

    pub async fn latest_markov_run(&self) -> Result<JsonValue> {
        crate::markov_method::latest_markov_run(self).await
    }

    pub async fn quiver_signals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        crate::quiver::latest_quiver_signals(self, limit).await
    }

    pub async fn quiver_signals_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        crate::quiver::latest_quiver_signals_page(self, limit, offset).await
    }

    pub async fn quiver_signals_count(&self) -> Result<i64> {
        crate::quiver::latest_quiver_signal_count(self).await
    }

    pub async fn latest_quiver_run(&self) -> Result<JsonValue> {
        crate::quiver::latest_quiver_run(self).await
    }

    pub async fn latest_daily_indicator_run(&self) -> Result<JsonValue> {
        let sql = "SELECT id, created_at, run_date, status, asset_count, success_count, error_count, config_json, summary_json
                   FROM daily_indicator_runs
                   ORDER BY run_date DESC, created_at DESC
                   LIMIT 1";
        Ok(self.first_json(sql).await?.unwrap_or(JsonValue::Null))
    }

    #[allow(dead_code)]
    pub async fn generate_decision_report_fallback(&self) -> Result<JsonValue> {
        // This is a conservative Rust-side generator used by the manual button.
        // It does not call the external xAI service; instead it persists a
        // transparent deterministic report with the same database shape that the
        // Rust UI consumes. This keeps manual operator snapshots auditable when
        // the primary deferred xAI path is unavailable or bypassed.
        let created_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let report_date = Utc::now().date_naive().to_string();
        let batch_id = self.latest_batch_id().await?.unwrap_or_default();
        let positions = self.position_items(250).await.unwrap_or_default();
        let watchlists = self
            .watchlists_payload()
            .await
            .unwrap_or_else(|_| json!({}));
        let selected_assets = deterministic_selected_assets(&positions, &watchlists);
        let suggested_trades = deterministic_suggested_trades(&positions, &watchlists);
        let symbol_sentiment = deterministic_symbol_sentiment(&positions, &selected_assets);
        let report_json = json!({
            "report_title": "Manual Rust fallback Decision Report",
            "status": "rust_fallback",
            "created_at": created_at,
            "reasoning_steps": [
                "Manual trigger was requested from the Rust dashboard.",
                "The primary deferred xAI decision path was unavailable or bypassed for this fallback invocation.",
                "This fallback report uses current portfolio, watchlist, cash, and allocation data to create an auditable operator snapshot."
            ],
            "market_view": {
                "bias": "neutral",
                "summary": "Deterministic Rust fallback: review current watchlist and portfolio state before submitting trades."
            },
            "portfolio_summary": {
                "position_count": positions.len(),
                "cash_balance_dkk": self.cash_buffer_value().get("cash_balance_dkk").cloned().unwrap_or(JsonValue::Null)
            },
            "strategy_status": "Rust manual fallback generated. Review suggested trades manually; no broker orders are queued by this action.",
            "strategy_flow": {
                "portfolio": positions.len(),
                "selected": selected_assets.len(),
                "trades": suggested_trades.len()
            },
            "selected_assets": selected_assets,
            "candidate_assets": symbol_sentiment,
            "symbol_sentiment": symbol_sentiment,
            "suggested_trades": suggested_trades,
        });
        let prompt_text = json!({
            "system": "Rust dashboard manual fallback. No external model call was made.",
            "user": "Generate an auditable decision snapshot from current stored portfolio/watchlist data."
        });
        let request_json = json!({
            "source": "rust_dashboard",
            "manual": true,
            "position_count": positions.len()
        });
        let sql = format!(
            "INSERT INTO decision_reports (
                created_at, report_date, batch_id, model, status, analysis_window_active,
                response_id, prompt_text, request_json, response_json, report_json,
                error_text, analysis_pulse_key, analysis_pulse_label
            ) VALUES (
                '{}', '{}', '{}', 'rust-deterministic-fallback', 'rust_fallback', 0,
                NULL, '{}', '{}', NULL, '{}',
                '{}', '{}', '{}'
            )",
            sql_escape(&created_at),
            sql_escape(&report_date),
            sql_escape(&batch_id),
            sql_escape(&serde_json::to_string(&prompt_text)?),
            sql_escape(&serde_json::to_string(&request_json)?),
            sql_escape(&serde_json::to_string(&report_json)?),
            sql_escape(
                "Generated by Rust fallback because external xAI decision generation was unavailable or bypassed."
            ),
            sql_escape(&format!("manual:{report_date}")),
            sql_escape("Manual Decision Report")
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("inserting manual Rust fallback decision report")?;
        let report = self
            .first_json(&format!(
                "SELECT id, created_at, report_date, model, status, analysis_window_active, response_id, prompt_text, request_json, response_json, report_json, error_text, analysis_pulse_key, analysis_pulse_label FROM decision_reports WHERE created_at = '{}' ORDER BY id DESC LIMIT 1",
                sql_escape(&created_at)
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        Ok(report)
    }

    async fn latest_decision_summary(&self) -> Result<JsonValue> {
        let report = self
            .first_json("SELECT id, created_at, status FROM decision_reports ORDER BY created_at DESC, id DESC LIMIT 1")
            .await?;
        Ok(report.unwrap_or_else(|| json!({"id": null, "created_at": null, "status": null})))
    }

    pub async fn strategy_journal_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, journal_date, cadence, status, summary, metrics_json, learnings_json, source_report_id, diary_json FROM strategy_journal_entries ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn scheduler_status_value(&self) -> Result<JsonValue> {
        Ok(self
            .first_json("SELECT singleton_key, started_at, last_heartbeat_at, last_cycle_started_at, last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid FROM scheduler_status WHERE singleton_key = 'main' LIMIT 1")
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn scheduler_cycles(&self, limit: i64) -> Result<Vec<JsonValue>> {
        self.scheduler_cycles_page(limit, 0).await
    }

    pub async fn scheduler_cycles_page(&self, limit: i64, offset: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, started_at, completed_at, status, analysis_window_active,
                    generated_decision, queue_status, notifications_status,
                    broker_alerts_status, cycle_json
             FROM scheduler_cycle_history
             ORDER BY started_at DESC, id DESC
             LIMIT {} OFFSET {}",
            clamp_limit(limit, 1, 100),
            offset.max(0).min(100_000)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    pub async fn scheduler_cycles_count(&self) -> Result<i64> {
        Ok(self
            .first_json("SELECT COUNT(*) AS count FROM scheduler_cycle_history")
            .await?
            .and_then(|row| row.get("count").and_then(JsonValue::as_i64))
            .unwrap_or(0))
    }

    pub async fn prune_scheduler_cycles(&self, now: DateTime<Utc>) -> Result<i64> {
        let (max_rows, retention_days) = scheduler_history_policy_values(
            yaml_i64(&self.config, &["scheduler", "history_max_rows"]),
            yaml_i64(&self.config, &["scheduler", "history_retention_days"]),
        );
        let mut deleted_rows = 0;
        if retention_days > 0 {
            let keep_since_started_at = (now - Duration::days(retention_days))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            deleted_rows += sqlx::query(&format!(
                "DELETE FROM scheduler_cycle_history WHERE started_at < '{}'",
                sql_escape(&keep_since_started_at)
            ))
            .execute(&self.pool)
            .await
            .context("pruning scheduler cycle history by retention age")?
            .rows_affected() as i64;
        }
        if max_rows > 0 {
            deleted_rows += sqlx::query(&format!(
                "DELETE FROM scheduler_cycle_history
                 WHERE id NOT IN (
                    SELECT id
                    FROM scheduler_cycle_history
                    ORDER BY id DESC
                    LIMIT {max_rows}
                 )"
            ))
            .execute(&self.pool)
            .await
            .context("pruning scheduler cycle history by row cap")?
            .rows_affected() as i64;
        }
        Ok(deleted_rows)
    }

    pub async fn price_monitor_status_value(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT singleton_key, updated_at, status, summary_json
                 FROM price_monitor_status
                 WHERE singleton_key = 'main'
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
    }

    pub async fn record_price_monitor_status(&self, summary: &JsonValue) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let status = json_text(summary, "status");
        let status = if status.is_empty() {
            "unknown"
        } else {
            &status
        };
        let summary_json = summary.to_string();
        let updated = sqlx::query(&format!(
            "UPDATE price_monitor_status
             SET updated_at = '{}', status = '{}', summary_json = '{}'
             WHERE singleton_key = 'main'",
            sql_escape(&now),
            sql_escape(status),
            sql_escape(&summary_json)
        ))
        .execute(&self.pool)
        .await
        .context("updating price monitor status")?;
        if updated.rows_affected() == 0 {
            sqlx::query(&format!(
                "INSERT INTO price_monitor_status
                    (singleton_key, updated_at, status, summary_json)
                 VALUES ('main', '{}', '{}', '{}')",
                sql_escape(&now),
                sql_escape(status),
                sql_escape(&summary_json)
            ))
            .execute(&self.pool)
            .await
            .context("inserting price monitor status")?;
        }
        Ok(())
    }

    pub fn hermes_goal_contract_value(&self) -> JsonValue {
        json!({
            "enabled": true,
            "mode": "recommend_only",
            "goal_version": 1,
            "objective": {
                "target_return_30d": 0.47,
                "target_return_note": "Approximately 10x in 6 months if compounded monthly: 1.47^6 ~= 10.1",
                "max_drawdown": 0.20,
                "min_sharpe": 1.0,
                "failure_below_30d_return": -0.04,
                "reflection_every": "7d",
                "one_variable_only": true
            },
            "constraints": {
                "max_positions": yaml_i64(&self.config, &["strategy", "swing", "max_holdings"]).unwrap_or(25),
                "slippage_tolerance": 0.02,
                "gas_reserve": 0.05,
                "min_cash_buffer_pct": yaml_f64(&self.config, &["strategy", "capital", "min_cash_buffer_pct"]).unwrap_or(0.10),
                "allow_shorting": yaml_bool(&self.config, &["risk", "allow_shorting"]).unwrap_or(false),
                "require_human_approval": true,
                "require_backtest_before_activation": true,
                "require_paper_or_sim_observation": true
            },
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
                    "return_30d_gte": 0.47,
                    "drawdown_lte": 0.20,
                    "sharpe_gte": 1.0
                },
                "rollback_if": {
                    "return_30d_lte": -0.04,
                    "drawdown_gt": 0.20,
                    "safety_violation": true
                }
            }
        })
    }

    pub fn hermes_capabilities_value(&self) -> JsonValue {
        json!({
            "status": "ok",
            "runtime": "saxo-rust",
            "namespace": "saxo",
            "database_namespace": "saxo",
            "safe_endpoints": [
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
                "/api/strategy-journal"
            ],
            "read_models": [
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
                "markov_asset_signals"
            ],
            "restricted_writes": [
                "hermes_reflections",
                "strategy_experiments",
                "hermes_decision_advice"
            ],
            "decision_advice": {
                "scope": "per_decision_report_trading_manager_preflight",
                "write_tool": "create_decision_advice",
                "allowed_recommendations": ["proceed", "stand_down", "review"],
                "allowed_order_actions": ["allow", "reduce", "stand_down", "review"],
                "required_context_self_check": {
                    "fields": hermes_context_self_check_required_fields(),
                    "format": "Include context_self_check with one boolean per field, optional sources, notes, and missing context explanations.",
                    "required_sources": [
                        "get_decision_reports",
                        "get_markov_signals",
                        "get_end_of_day_reports",
                        "get_context",
                        "list_reflections",
                        "list_experiments"
                    ]
                },
                "safety": "advisory only; cannot add trades, increase size, approve live orders, or call Saxo mutation endpoints"
            },
            "supported_experiment_overlays": {
                "scope": "paper_or_saxo_sim_only",
                "statuses": ["approved_sim", "active_sim", "approved_paper", "active_paper"],
                "variables": [
                    "execution.min_trade_value_dkk",
                    "strategy.capital.min_cash_buffer_pct",
                    "strategy.swing.cash_buffer_pct",
                    "strategy.swing.daily_indicators.min_confluences",
                    "strategy.swing.markov_gate.min_signed_signal"
                ]
            },
            "forbidden": [
                "saxo_sessions",
                "Saxo OAuth token/session reads",
                "order precheck/place/replace/cancel",
                "live order approval",
                "Kubernetes secret mutation",
                "live broker baseline activation"
            ],
            "notes": [
                "Hermes proposals are recommend-only until reviewed by the daytrader UI/operator flow.",
                "Promoted baselines are audit records; they do not activate live broker behavior.",
                "Strategy experiments must change exactly one variable while one_variable_only is true.",
                "Markov method signals are advisory analytics and do not place or approve orders.",
                "Gate replay is a read-only historical target-gate comparison; a target-gate clear is not a full approval.",
                "QuiverQuant alternative-data signals are advisory analytics and do not place or approve orders.",
                "Scheduled decision reports target two daily open-followup pulses: Nordic/EU open +1h15 and US open +1h15.",
                "Daily end-of-day reports are exposed as sanitized strategy journal rows.",
                "The Hermes adapter intentionally excludes raw request_json/response_json payloads from decision reports."
            ],
            "goal_contract": self.hermes_goal_contract_value()
        })
    }

    pub async fn hermes_context(&self, limit: i64) -> Result<JsonValue> {
        let limit = clamp_limit(limit, 1, 50);
        let overview = self.overview_payload().await.unwrap_or_else(|err| {
            warn!("Hermes overview context degraded: {err:#}");
            json!({"status": "degraded", "detail": err.to_string()})
        });
        let scheduler_status = self.scheduler_status_value().await.unwrap_or_else(|err| {
            warn!("Hermes scheduler status degraded: {err:#}");
            json!({"status": "degraded", "detail": err.to_string()})
        });
        let scheduler_cycles = self.scheduler_cycles(limit).await.unwrap_or_default();
        let decision_reports = self.hermes_decision_report_items(limit).await?;
        let journals = self.strategy_journal_items(limit).await.unwrap_or_default();
        let end_of_day_reports = self.hermes_end_of_day_report_items(limit).await?;
        let execution_orders = self.execution_orders(limit).await.unwrap_or_default();
        let execution_failures = self.hermes_execution_failures(limit).await?;
        let execution_events = self.execution_events(limit).await.unwrap_or_default();
        let execution_fills = self.execution_fills(limit).await.unwrap_or_default();
        let performance = self
            .performance_history_with_current("1M", 500)
            .await
            .unwrap_or_default();
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
        let daily_indicators = crate::daily_indicators::compact_indicator_context(self, limit)
            .await
            .unwrap_or_else(|err| {
                warn!("Hermes daily-indicator context degraded: {err:#}");
                json!({"status": "degraded", "detail": err.to_string()})
            });

        Ok(json!({
            "status": "ok",
            "generated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "capabilities": self.hermes_capabilities_value(),
            "goal_contract": self.hermes_goal_contract_value(),
            "overview": overview,
            "scheduler": {
                "status": scheduler_status,
                "cycles": scheduler_cycles
            },
            "decisions": {
                "cadence": "two_daily_open_followups",
                "pulses": crate::xai_decision::decision_pulse_summary(self).get("pulses").cloned().unwrap_or_else(|| json!([])),
                "reports": decision_reports
            },
            "end_of_day": {
                "cadence": "daily",
                "reports": end_of_day_reports
            },
            "strategy_journal": {
                "items": journals
            },
            "execution": {
                "orders": execution_orders,
                "failures": execution_failures,
                "events": execution_events,
                "fills": execution_fills
            },
            "performance": {
                "range": "1M",
                "history": performance
            },
            "markov_method": markov,
            "quiver_signals": quiver,
            "daily_indicators": daily_indicators,
            "hermes": {
                "experiments": active_experiments,
                "active_strategy_baseline": active_strategy_baseline,
                "gate_replay": gate_replay,
                "learning_memory": {
                    "active": active_learning_memory,
                    "stale_count": stale_learning_memory_count,
                    "policy": "repeated reflection actions become stable after two distinct reflections; emerging lessons expire after 7d and stable lessons after 21d"
                }
            },
            "safety": {
                "saxo_sessions_excluded": true,
                "broker_mutations_excluded": true,
                "raw_oauth_payloads_excluded": true
            }
        }))
    }

    pub async fn hermes_decision_report_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, report_date, model, status, analysis_window_active, report_json, error_text, analysis_pulse_key, analysis_pulse_label
             FROM decision_reports
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, 100)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
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
        let mut unpriced = 0usize;
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
            let reference_price_local = candidate
                .get("reference_price_local")
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0);
            let status = if reference_price_local.is_some() {
                "tracking"
            } else {
                unpriced += 1;
                "unpriced"
            };
            let reference_sql = reference_price_local
                .map(|value| value.to_string())
                .unwrap_or_else(|| "NULL".to_string());
            let currency = json_text(&candidate, "currency");
            let id = format!("hermes-counterfactual-{manager_run_id}-{index}");
            let result = sqlx::query(&format!(
                "INSERT INTO hermes_counterfactuals (
                    id, created_at, updated_at, report_id, manager_run_id,
                    strategy_key, symbol, action, source_effect, shadow_quantity,
                    reference_price_local, currency, status, observation_count
                ) VALUES (
                    '{}', '{}', '{}', {}, {}, '{}', '{}', '{}', '{}', {}, {}, {}, '{}', 0
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
                reference_sql,
                sql_optional_text(Some(&currency)),
                status,
            ))
            .execute(&self.pool)
            .await
            .context("recording Hermes counterfactual")?;
            created += result.rows_affected() as usize;
        }

        Ok(json!({
            "status": "ok",
            "created": created,
            "unpriced": unpriced,
            "skipped": skipped,
            "safety": "quote_to_quote_observation_only"
        }))
    }

    pub async fn hermes_counterfactuals(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, updated_at, report_id, manager_run_id, strategy_key,
                    symbol, action, source_effect, shadow_quantity, reference_price_local,
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
                 WHERE status = 'tracking' AND reference_price_local > 0
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
                "SELECT id, action, shadow_quantity, reference_price_local
                 FROM hermes_counterfactuals
                 WHERE symbol = '{}' AND status = 'tracking' AND reference_price_local > 0",
                sql_escape(symbol)
            ))
            .await?;
        let mut updated = 0usize;
        for row in rows {
            let id = json_text(&row, "id");
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

    pub async fn active_strategy_baseline(&self) -> Result<JsonValue> {
        Ok(self
            .first_json(
                "SELECT id, created_at, activated_at, status, goal_version, config_json, prompt_json, source
                 FROM strategy_baselines
                 WHERE status = 'active'
                 ORDER BY activated_at DESC, created_at DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null))
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

    pub async fn hermes_lessons_pending_review(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, period_start, period_end, goal_version, summary, proposed_actions_json, source_session_id
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            HERMES_LESSONS_PENDING_REVIEW_REFLECTION_LIMIT
        );
        let reflections = self.select_json(&sql).await.unwrap_or_default();
        Ok(hermes_lessons_pending_review_from_reflections(
            &reflections,
            clamp_limit(limit, 1, HERMES_LESSONS_PENDING_REVIEW_LIMIT as i64) as usize,
        ))
    }

    pub async fn hermes_learning_memory(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, proposed_actions_json, source_session_id
             FROM hermes_reflections
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            HERMES_LEARNING_MEMORY_REFLECTION_LIMIT
        );
        let reflections = self.select_json(&sql).await.unwrap_or_default();
        Ok(hermes_learning_memory_from_reflections(
            &reflections,
            Utc::now(),
            clamp_limit(limit, 1, HERMES_LEARNING_MEMORY_LIMIT as i64) as usize,
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

    async fn expire_stale_hermes_experiments_at(
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

    pub async fn latest_trading_manager_run(&self) -> Result<JsonValue> {
        let mut run = self
            .first_json(
                "SELECT id, created_at, manager_key, manager_kind, manager_label, target_at_utc, report_id, status, open_exchange_codes_json, technical_json, manager_json, queue_result_json, error_text
                 FROM trading_manager_runs
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
            )
            .await?
            .unwrap_or(JsonValue::Null);

        let report_id = run
            .get("report_id")
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
            .unwrap_or(0);
        let current_advice_status = run
            .get("manager_json")
            .and_then(|value| value.get("hermes_decision_advice"))
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let should_repair_advice =
            report_id > 0 && matches!(current_advice_status, "" | "timeout" | "error");
        if should_repair_advice {
            if let Some(advice) = self.hermes_decision_advice_by_report(report_id).await? {
                let existing_mode = run
                    .get("manager_json")
                    .and_then(|value| value.get("hermes_decision_advice"))
                    .and_then(|value| value.get("mode"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or("record_only")
                    .to_string();
                if let Some(manager_json) = run
                    .get_mut("manager_json")
                    .and_then(JsonValue::as_object_mut)
                {
                    manager_json.insert(
                        "hermes_decision_advice".to_string(),
                        json!({
                            "status": json_text(&advice, "status"),
                            "mode": existing_mode,
                            "source_session_id": json_text(&advice, "source_session_id"),
                            "overall_recommendation": json_text(&advice, "overall_recommendation"),
                            "summary": json_text(&advice, "summary"),
                            "attached_from": "report_fallback",
                            "raw": advice,
                        }),
                    );
                }
            }
        }

        Ok(run)
    }

    pub async fn record_scheduler_cycle(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler cycle JSON")?;
        let queue_status = cycle_json
            .get("trading_manager")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or(status);
        let analysis_window_active = cycle_json
            .get("market")
            .and_then(|value| value.get("analysis_window_active"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let notifications_status = cycle_json
            .get("notifications")
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            .unwrap_or("not_run");
        let broker_alerts_status = notifications_status;
        let sql = format!(
            "INSERT INTO scheduler_cycle_history (
                started_at, completed_at, status, analysis_window_active,
                generated_decision, queue_status, notifications_status, broker_alerts_status,
                cycle_json
            ) VALUES (
                '{}', '{}', '{}', {}, 0, '{}', '{}', '{}', '{}'
            )",
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            if analysis_window_active { 1 } else { 0 },
            sql_escape(queue_status),
            sql_escape(notifications_status),
            sql_escape(broker_alerts_status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording scheduler cycle")?;
        self.update_scheduler_status(started_at, completed_at, status, cycle_json)
            .await
    }

    pub async fn update_scheduler_heartbeat(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', NULL, NULL, 'heartbeat', '{{}}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at",
            sql_escape(&now),
            sql_escape(&now)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler heartbeat")?;
        Ok(())
    }

    async fn update_scheduler_status(
        &self,
        started_at: &str,
        completed_at: &str,
        status: &str,
        cycle_json: &JsonValue,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let cycle_text =
            serde_json::to_string(cycle_json).context("serializing scheduler status JSON")?;
        let sql = format!(
            "INSERT INTO scheduler_status (
                singleton_key, started_at, last_heartbeat_at, last_cycle_started_at,
                last_cycle_completed_at, last_cycle_status, last_cycle_json, scheduler_pid
            ) VALUES (
                'main', '{}', '{}', '{}', '{}', '{}', '{}', NULL
            )
            ON CONFLICT(singleton_key) DO UPDATE SET
                last_heartbeat_at = excluded.last_heartbeat_at,
                last_cycle_started_at = excluded.last_cycle_started_at,
                last_cycle_completed_at = excluded.last_cycle_completed_at,
                last_cycle_status = excluded.last_cycle_status,
                last_cycle_json = excluded.last_cycle_json",
            sql_escape(started_at),
            sql_escape(&now),
            sql_escape(started_at),
            sql_escape(completed_at),
            sql_escape(status),
            sql_escape(&cycle_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("updating scheduler status")?;
        Ok(())
    }

    pub async fn performance_history_for_range(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let columns = "recorded_at, snapshot_type, total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk, total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk, position_count, source";
        let limit = clamp_limit(limit, 1, 5000);
        let mut rows = Vec::new();
        let where_clause = match performance_start_at(range_key) {
            Some(start_at) => {
                let escaped_start = sql_escape(&start_at);
                let anchor_sql = format!(
                    "SELECT {columns} FROM portfolio_value_history WHERE recorded_at < '{escaped_start}' ORDER BY recorded_at DESC, id DESC LIMIT 1"
                );
                rows.extend(self.select_json(&anchor_sql).await.unwrap_or_default());
                format!("WHERE recorded_at >= '{escaped_start}'")
            }
            None => String::new(),
        };
        let remaining = (limit - rows.len() as i64).max(1);
        let sql = format!(
            "SELECT {columns} FROM portfolio_value_history {where_clause} ORDER BY recorded_at ASC, id ASC LIMIT {}",
            clamp_limit(remaining, 1, 5000)
        );
        rows.extend(self.select_json(&sql).await.unwrap_or_default());
        rows.sort_by(|left, right| {
            text_value(left, "recorded_at")
                .cmp(&text_value(right, "recorded_at"))
                .then_with(|| {
                    text_value(left, "snapshot_type").cmp(&text_value(right, "snapshot_type"))
                })
        });
        Ok(rows)
    }

    pub async fn performance_history_with_current(
        &self,
        range_key: &str,
        limit: i64,
    ) -> Result<Vec<JsonValue>> {
        let mut history = self.performance_history_for_range(range_key, limit).await?;
        let current = self.current_performance_row().await?;
        let latest_matches_current = history
            .last()
            .is_some_and(|latest| performance_rows_have_same_values(latest, &current));
        if !latest_matches_current {
            history.push(current);
        } else if history.len() == 1 {
            history[0] = current;
        }
        Ok(history)
    }

    pub async fn record_portfolio_value_snapshot(
        &self,
        snapshot_type: &str,
        baseline_session_date: Option<&str>,
        source: &str,
        extra_payload: JsonValue,
    ) -> Result<JsonValue> {
        let recorded_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let latest_batch = self.latest_batch_id().await?;
        let aggregate = self.position_aggregate(latest_batch.as_deref()).await?;
        let payload = json!({
            "summary": aggregate,
            "snapshot_type": snapshot_type,
            "baseline_session_date": baseline_session_date,
            "source": source,
            "extra": extra_payload,
        });
        let payload_text =
            serde_json::to_string(&payload).context("serializing portfolio snapshot payload")?;
        let sql = format!(
            "INSERT INTO portfolio_value_history (
                recorded_at,
                snapshot_type,
                baseline_session_date,
                batch_id,
                total_market_value_dkk,
                invested_market_value_dkk,
                cash_balance_dkk,
                total_cost_basis_dkk,
                total_unrealised_pnl_dkk,
                total_daily_pnl_dkk,
                position_count,
                source,
                raw_payload_json
            ) VALUES (
                '{}',
                '{}',
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                {},
                '{}'
            )",
            sql_escape(&recorded_at),
            sql_escape(snapshot_type),
            sql_optional_text(baseline_session_date),
            sql_optional_text(latest_batch.as_deref()),
            sql_f64(value_f64(&aggregate, "total_market_value_dkk")),
            sql_f64(value_f64(&aggregate, "invested_market_value_dkk")),
            sql_f64(value_f64(&aggregate, "cash_balance_dkk")),
            sql_f64(value_f64(&aggregate, "total_cost_basis_dkk")),
            sql_f64(value_f64(&aggregate, "total_unrealised_pnl_dkk")),
            sql_f64(value_f64(&aggregate, "total_daily_pnl_dkk")),
            value_i64(&aggregate, "position_count"),
            sql_optional_text(Some(source)),
            sql_escape(&payload_text)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("recording portfolio value snapshot")?;
        let snapshot = self
            .first_json(&format!(
                "SELECT recorded_at, snapshot_type, baseline_session_date, batch_id,
                        total_market_value_dkk, invested_market_value_dkk, cash_balance_dkk,
                        total_cost_basis_dkk, total_unrealised_pnl_dkk, total_daily_pnl_dkk,
                        position_count, source
                 FROM portfolio_value_history
                 WHERE recorded_at = '{}' AND snapshot_type = '{}' AND source = '{}'
                 ORDER BY id DESC
                 LIMIT 1",
                sql_escape(&recorded_at),
                sql_escape(snapshot_type),
                sql_escape(source)
            ))
            .await?
            .unwrap_or_else(|| {
                json!({
                    "recorded_at": recorded_at,
                    "snapshot_type": snapshot_type,
                    "baseline_session_date": baseline_session_date,
                    "batch_id": latest_batch,
                    "total_market_value_dkk": value_f64(&aggregate, "total_market_value_dkk"),
                    "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
                    "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
                    "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
                    "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
                    "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
                    "position_count": value_i64(&aggregate, "position_count"),
                    "source": source,
                })
            });
        Ok(json!({
            "status": "ok",
            "snapshot": snapshot,
        }))
    }

    async fn current_performance_row(&self) -> Result<JsonValue> {
        let latest_batch = self.latest_batch_id().await?;
        let aggregate = self.position_aggregate(latest_batch.as_deref()).await?;
        Ok(json!({
            "recorded_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "snapshot_type": "runtime_current",
            "total_market_value_dkk": value_f64(&aggregate, "total_market_value_dkk"),
            "invested_market_value_dkk": value_f64(&aggregate, "invested_market_value_dkk"),
            "cash_balance_dkk": value_f64(&aggregate, "cash_balance_dkk"),
            "total_cost_basis_dkk": value_f64(&aggregate, "total_cost_basis_dkk"),
            "total_unrealised_pnl_dkk": value_f64(&aggregate, "total_unrealised_pnl_dkk"),
            "total_daily_pnl_dkk": value_f64(&aggregate, "total_daily_pnl_dkk"),
            "position_count": value_i64(&aggregate, "position_count"),
            "source": text_value(&aggregate, "source"),
        }))
    }

    pub async fn portfolio_trades_items(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT * FROM trade_ledger ORDER BY created_at DESC, id DESC LIMIT {}",
            clamp_limit(limit, 1, 250)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    async fn execution_counts(&self) -> Result<JsonValue> {
        let rows = self
            .select_json("SELECT status, COUNT(*) AS count FROM execution_orders GROUP BY status")
            .await?;
        let mut queued = 0;
        let mut pending_approval = 0;
        let mut broker_live = 0;
        let mut failed = 0;
        for row in rows {
            let status = row.get("status").and_then(JsonValue::as_str).unwrap_or("");
            let count = value_i64(&row, "count");
            match status {
                "pending_execution"
                | "waiting_for_market_open"
                | "waiting_for_cash_settlement"
                | "waiting_for_virtual_cash_budget" => queued += count,
                "pending_approval" => pending_approval += count,
                "submitted_to_broker"
                | "submitting_to_broker"
                | "broker_working"
                | "broker_amended"
                | "broker_partially_filled"
                | "broker_replace_requested"
                | "broker_cancel_requested" => broker_live += count,
                "execution_failed" => failed += count,
                _ => {}
            }
        }
        Ok(
            json!({"queued": queued, "pending_approval": pending_approval, "broker_live": broker_live, "failed": failed}),
        )
    }

    async fn executed_orders_today(&self) -> Result<i64> {
        let today = Utc::now().date_naive().to_string();
        let sql = format!(
            "SELECT COUNT(*) AS count FROM execution_orders WHERE substr(created_at, 1, 10) = '{}' AND status = 'executed'",
            sql_escape(&today)
        );
        let row = self.first_json(&sql).await?.unwrap_or_else(|| json!({}));
        Ok(value_i64(&row, "count"))
    }

    pub async fn goal_tracking(&self, total_value: f64) -> JsonValue {
        let weekly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "weekly_target_dkk"],
        )
        .unwrap_or(5000.0);
        let monthly_target = yaml_f64(
            &self.config,
            &["xai", "performance_goals", "monthly_target_dkk"],
        )
        .unwrap_or(20000.0);
        let tz = yaml_string(&self.config, &["localization", "time_zone"])
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        let now_local = Utc::now().with_timezone(&tz);
        let week_start_date = now_local.date_naive()
            - Duration::days(now_local.weekday().num_days_from_monday() as i64);
        let month_start_date = now_local
            .date_naive()
            .with_day(1)
            .unwrap_or_else(|| now_local.date_naive());
        let week_start_utc = local_date_start_to_utc_string(week_start_date, tz);
        let month_start_utc = local_date_start_to_utc_string(month_start_date, tz);
        // Scope baselines to the active import batch so portfolio resets
        // start a fresh P&L baseline instead of bleeding through as losses.
        let batch_id = self.latest_batch_id().await.ok().flatten();
        let week_baseline = self
            .portfolio_value_at(&week_start_utc, batch_id.as_deref())
            .await
            .unwrap_or(None);
        let month_baseline = self
            .portfolio_value_at(&month_start_utc, batch_id.as_deref())
            .await
            .unwrap_or(None);
        let period_json = |baseline: Option<f64>, target: f64, period_start: &str| {
            let pnl = baseline
                .map(|baseline| total_value - baseline)
                .unwrap_or(0.0);
            json!({
                "pnl_dkk": pnl,
                "target_dkk": target,
                "progress_pct": pct(pnl, target),
                "baseline_value_dkk": baseline,
                "period_start_utc": period_start,
            })
        };
        json!({
            "weekly_target_dkk": weekly_target,
            "monthly_target_dkk": monthly_target,
            "basis": "pnl_dkk is total portfolio value change since the period start, measured against the portfolio value history baseline.",
            "periods": {
                "week": period_json(week_baseline, weekly_target, &week_start_utc),
                "month": period_json(month_baseline, monthly_target, &month_start_utc)
            }
        })
    }

    /// Portfolio value at a moment in time: the last snapshot before the
    /// cutoff, falling back to the first snapshot after it when history
    /// starts mid-period. Restricted to one import batch when given so
    /// values from before a portfolio reset are never used as baselines.
    async fn portfolio_value_at(
        &self,
        cutoff_utc: &str,
        batch_id: Option<&str>,
    ) -> Result<Option<f64>> {
        let batch_filter = batch_id
            .map(|batch_id| format!(" AND batch_id = '{}'", sql_escape(batch_id)))
            .unwrap_or_default();
        let before = self
            .first_json(&format!(
                "SELECT total_market_value_dkk FROM portfolio_value_history \
                 WHERE recorded_at < '{}'{} ORDER BY recorded_at DESC LIMIT 1",
                sql_escape(cutoff_utc),
                batch_filter
            ))
            .await?;
        if let Some(row) = before {
            return Ok(Some(value_f64(&row, "total_market_value_dkk")));
        }
        let after = self
            .first_json(&format!(
                "SELECT total_market_value_dkk FROM portfolio_value_history \
                 WHERE recorded_at >= '{}'{} ORDER BY recorded_at ASC LIMIT 1",
                sql_escape(cutoff_utc),
                batch_filter
            ))
            .await?;
        Ok(after.map(|row| value_f64(&row, "total_market_value_dkk")))
    }

    pub fn cash_buffer_value(&self) -> JsonValue {
        let min_cash_buffer_pct = yaml_f64(
            &self.config,
            &["strategy", "capital", "min_cash_buffer_pct"],
        )
        .unwrap_or(0.10);
        let max_deployment_pct =
            yaml_f64(&self.config, &["strategy", "capital", "max_deployment_pct"]).unwrap_or(0.90);
        let reinvestment_pressure_threshold_pct = yaml_f64(
            &self.config,
            &["strategy", "capital", "reinvestment_pressure_threshold_pct"],
        )
        .unwrap_or(0.05);
        json!({
            "min_cash_buffer_pct": min_cash_buffer_pct,
            "max_deployment_pct": max_deployment_pct,
            "reinvestment_pressure_threshold_pct": reinvestment_pressure_threshold_pct,
            "source": "config",
            "updated_at": null,
            "config_default_min_cash_buffer_pct": min_cash_buffer_pct
        })
    }

    fn default_ai_settings_value(&self) -> JsonValue {
        let provider = yaml_string(&self.config, &["xai", "provider"])
            .or_else(|| yaml_string(&self.config, &["xai", "inference_provider"]))
            .unwrap_or_else(|| "openrouter".to_string());
        let config_model = yaml_string(&self.config, &["xai", "model"]).unwrap_or_else(|| {
            if provider == "openrouter" {
                "openai/gpt-5.5".to_string()
            } else {
                "grok-4.3".to_string()
            }
        });
        json!({
            "provider": provider,
            "model": config_model,
            "config_model": config_model,
            "source": "config",
            "updated_at": null
        })
    }

    pub async fn ai_settings_value(&self) -> Result<JsonValue> {
        let mut value = self.default_ai_settings_value();
        if let Some(saved) = self.runtime_setting("ai_settings").await? {
            let model = saved
                .get("model")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
            if let Some(model) = model {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("model".to_string(), JsonValue::from(model));
                    obj.insert("source".to_string(), JsonValue::from("runtime"));
                    obj.insert(
                        "updated_at".to_string(),
                        saved.get("updated_at").cloned().unwrap_or(JsonValue::Null),
                    );
                }
            }
        }
        if let Some(obj) = value.as_object_mut() {
            obj.insert("api_key".to_string(), self.ai_api_key_status_value().await?);
        }
        Ok(value)
    }

    /// Masked status of the AI provider API key. Never contains the key
    /// itself — only whether one is configured, where it comes from, and a
    /// short masked preview so the operator can recognize which key is live.
    pub async fn ai_api_key_status_value(&self) -> Result<JsonValue> {
        let override_entry = self.runtime_setting("ai_api_key").await?;
        let override_key = override_entry
            .as_ref()
            .and_then(|entry| entry.get("api_key"))
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string);
        let config_key = yaml_string(&self.config, &["xai", "api_key"])
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let (source, key) = if let Some(key) = override_key {
            ("runtime", Some(key))
        } else if let Some(key) = config_key {
            ("config", Some(key))
        } else {
            ("missing", None)
        };
        Ok(json!({
            "configured": key.is_some(),
            "source": source,
            "masked": key.as_deref().map(mask_api_key),
            "updated_at": override_entry
                .as_ref()
                .and_then(|entry| entry.get("updated_at"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        }))
    }

    /// The AI provider API key the process should use right now: the
    /// runtime override saved from Settings wins over the config/env value,
    /// so a rotated key takes effect without a redeploy.
    pub async fn effective_ai_api_key(&self) -> Option<String> {
        if let Ok(Some(saved)) = self.runtime_setting("ai_api_key").await {
            if let Some(key) = saved
                .get("api_key")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
            {
                return Some(key.to_string());
            }
        }
        yaml_string(&self.config, &["xai", "api_key"])
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }

    /// Claims the single manual decision-report slot. Returns false when a
    /// fresh claim is already held, so double-clicks and concurrent operators
    /// cannot start overlapping manual pipelines. A claim older than the
    /// stale window is treated as abandoned (crashed task) and taken over.
    pub async fn claim_manual_decision_report(&self) -> Result<bool> {
        const STALE_AFTER_SECONDS: i64 = 900;
        if let Some(existing) = self.runtime_setting("manual_report_claim").await? {
            let fresh = existing
                .get("started_at")
                .and_then(JsonValue::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|started| {
                    Utc::now().signed_duration_since(started.with_timezone(&Utc))
                        < Duration::seconds(STALE_AFTER_SECONDS)
                })
                .unwrap_or(false);
            if fresh {
                return Ok(false);
            }
        }
        let value = json!({
            "started_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        self.save_runtime_setting("manual_report_claim", &value)
            .await?;
        Ok(true)
    }

    pub async fn release_manual_decision_report_claim(&self) -> Result<()> {
        sqlx::query("DELETE FROM runtime_settings WHERE key = 'manual_report_claim'")
            .execute(&self.pool)
            .await
            .context("releasing manual decision report claim")?;
        Ok(())
    }

    /// True while a spawned manual decision-report pipeline holds a fresh
    /// claim — drives the dashboard's pending banner and button state.
    pub async fn manual_decision_report_in_flight(&self) -> bool {
        const STALE_AFTER_SECONDS: i64 = 900;
        match self.runtime_setting("manual_report_claim").await {
            Ok(Some(existing)) => existing
                .get("started_at")
                .and_then(JsonValue::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|started| {
                    Utc::now().signed_duration_since(started.with_timezone(&Utc))
                        < Duration::seconds(STALE_AFTER_SECONDS)
                })
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Saves (or, for an empty submission, clears) the runtime API-key
    /// override. Returns the masked status only — the key is never echoed.
    pub async fn save_ai_api_key(&self, api_key: &str) -> Result<JsonValue> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            sqlx::query("DELETE FROM runtime_settings WHERE key = 'ai_api_key'")
                .execute(&self.pool)
                .await
                .context("clearing AI API key override")?;
            return self.ai_api_key_status_value().await;
        }
        if api_key.len() > 400 {
            anyhow::bail!("API key is too long");
        }
        if !api_key.chars().all(|ch| ch.is_ascii_graphic()) {
            anyhow::bail!("API key contains whitespace or non-printable characters");
        }
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "api_key": api_key,
            "updated_at": updated_at
        });
        self.save_runtime_setting("ai_api_key", &value).await?;
        self.ai_api_key_status_value().await
    }

    pub async fn save_ai_settings(&self, model: &str) -> Result<JsonValue> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("AI model cannot be empty");
        }
        if model.len() > 160 {
            anyhow::bail!("AI model is too long");
        }
        // '~' is OpenRouter's floating-alias prefix (e.g. ~openai/gpt-5) and
        // must round-trip unmodified.
        if !model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.' | ':' | '~'))
        {
            anyhow::bail!("AI model contains unsupported characters");
        }
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "model": model,
            "updated_at": updated_at
        });
        self.save_runtime_setting("ai_settings", &value).await?;
        self.ai_settings_value().await
    }

    fn strategy_month_key(&self) -> String {
        let tz = yaml_string(&self.config, &["price_monitor", "timezone"])
            .and_then(|value| value.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::Europe::Copenhagen);
        Utc::now().with_timezone(&tz).format("%Y-%m").to_string()
    }

    pub async fn monthly_loss_breaker_override_value(&self) -> Result<JsonValue> {
        let current_month_key = self.strategy_month_key();
        let saved = self
            .runtime_setting("monthly_loss_breaker_override")
            .await?;
        let mut value = json!({
            "enabled": false,
            "current_month_key": current_month_key,
            "month_key": null,
            "active_for_current_month": false,
            "notes": "",
            "updated_at": null
        });
        if let Some(saved) = saved {
            let enabled = saved
                .get("enabled")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let month_key = saved
                .get("month_key")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            let active_for_current_month = enabled && month_key == current_month_key;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("enabled".to_string(), JsonValue::from(enabled));
                obj.insert("month_key".to_string(), JsonValue::from(month_key));
                obj.insert(
                    "active_for_current_month".to_string(),
                    JsonValue::from(active_for_current_month),
                );
                obj.insert(
                    "notes".to_string(),
                    saved
                        .get("notes")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::from("")),
                );
                obj.insert(
                    "updated_at".to_string(),
                    saved.get("updated_at").cloned().unwrap_or(JsonValue::Null),
                );
            }
        }
        Ok(value)
    }

    pub async fn save_monthly_loss_breaker_override(
        &self,
        enabled: bool,
        notes: &str,
    ) -> Result<JsonValue> {
        if notes.len() > 500 {
            anyhow::bail!("Monthly-loss breaker override notes are too long");
        }
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let value = json!({
            "enabled": enabled,
            "month_key": self.strategy_month_key(),
            "notes": notes,
            "updated_at": updated_at
        });
        self.save_runtime_setting("monthly_loss_breaker_override", &value)
            .await?;
        self.monthly_loss_breaker_override_value().await
    }

    pub async fn instrument_quarantine_overrides_value(&self) -> Result<JsonValue> {
        let saved = self
            .runtime_setting("instrument_quarantine_overrides")
            .await?;
        let overrides = saved
            .as_ref()
            .and_then(|value| value.get("overrides"))
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                item.get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false)
                    && item
                        .get("symbol")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    && item
                        .get("action")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    && item
                        .get("signature")
                        .and_then(JsonValue::as_str)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "overrides": overrides,
            "updated_at": saved
                .as_ref()
                .and_then(|value| value.get("updated_at"))
                .cloned()
                .unwrap_or(JsonValue::Null)
        }))
    }

    pub async fn save_instrument_quarantine_override(
        &self,
        symbol: &str,
        action: &str,
        signature: &str,
        enabled: bool,
        notes: &str,
    ) -> Result<JsonValue> {
        let symbol = symbol.trim();
        let action = action.trim().to_uppercase();
        let signature = signature.trim();
        if symbol.is_empty() || action.is_empty() || signature.is_empty() {
            anyhow::bail!("Instrument quarantine override requires symbol, side, and signature");
        }
        if symbol.len() > 80 || action.len() > 12 || signature.len() > 120 {
            anyhow::bail!("Instrument quarantine override identifier is too long");
        }
        if notes.len() > 500 {
            anyhow::bail!("Instrument quarantine override notes are too long");
        }
        let existing = self.instrument_quarantine_overrides_value().await?;
        let mut overrides = existing
            .get("overrides")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                !(item.get("symbol").and_then(JsonValue::as_str) == Some(symbol)
                    && item.get("action").and_then(JsonValue::as_str) == Some(action.as_str())
                    && item.get("signature").and_then(JsonValue::as_str) == Some(signature))
            })
            .collect::<Vec<_>>();
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        if enabled {
            overrides.push(json!({
                "symbol": symbol,
                "action": action,
                "signature": signature,
                "enabled": true,
                "notes": notes,
                "updated_at": updated_at
            }));
        }
        let value = json!({
            "overrides": overrides,
            "updated_at": updated_at
        });
        self.save_runtime_setting("instrument_quarantine_overrides", &value)
            .await?;
        self.instrument_quarantine_overrides_value().await
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

    pub async fn effective_xai_model(&self) -> Result<String> {
        Ok(self
            .ai_settings_value()
            .await?
            .get("model")
            .and_then(JsonValue::as_str)
            .unwrap_or("openai/gpt-5.5")
            .to_string())
    }

    pub async fn saxo_auth_status_value(&self) -> JsonValue {
        if let Err(err) = self.ensure_saxo_session_json("auth_status").await {
            warn!("Saxo leased session refresh before auth status skipped: {err:#}");
        }
        auth::auth_status(&self.config, &self.config_path, false).await
    }

    pub async fn saxo_session_value(&self) -> JsonValue {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before session API failed: {err:#}");
        }
        auth::session_api(&self.config, &self.config_path).await
    }

    pub async fn refresh_saxo_session(&self) -> Result<JsonValue> {
        let lease_owner = self
            .prepare_saxo_session_refresh_lease_if_needed("refresh")
            .await?;
        let result = match auth::refresh_session(&self.config, &self.config_path).await {
            Ok(status) => {
                self.persist_saxo_session_file_to_db("refresh").await?;
                Ok(status)
            }
            Err(err) => {
                if let Err(persist_err) = self
                    .persist_invalid_saxo_session_file_to_db("refresh_invalid")
                    .await
                {
                    warn!("Saxo invalid session database persistence failed: {persist_err:#}");
                }
                Err(err)
            }
        };
        if let Some(owner) = lease_owner {
            if let Err(err) = self.release_saxo_session_refresh_lease(&owner).await {
                warn!("Saxo session refresh lease release failed: {err:#}");
            }
        }
        result
    }

    pub async fn ensure_saxo_session_json(&self, source: &str) -> Result<JsonValue> {
        let lease_owner = self
            .prepare_saxo_session_refresh_lease_if_needed(source)
            .await?;
        let result = match auth::ensure_session_json(&self.config, &self.config_path).await {
            Ok(session) => {
                self.persist_saxo_session_file_to_db(source).await?;
                Ok(session)
            }
            Err(err) => {
                let invalid_source = format!("{source}_invalid");
                if let Err(persist_err) = self
                    .persist_invalid_saxo_session_file_to_db(&invalid_source)
                    .await
                {
                    warn!("Saxo invalid session database persistence failed: {persist_err:#}");
                }
                Err(err)
            }
        };
        if let Some(owner) = lease_owner {
            if let Err(err) = self.release_saxo_session_refresh_lease(&owner).await {
                warn!("Saxo session refresh lease release failed: {err:#}");
            }
        }
        result
    }

    pub async fn user_logout_saxo_session(&self) -> Result<JsonValue> {
        // User SSO and Saxo OAuth are different security domains. Logging out
        // of the dashboard user must not delete the service-level Saxo refresh
        // token, because the scheduler keeps renewing that token without any
        // browser session. This endpoint therefore reports the current Saxo
        // status and leaves the durable `saxo_sessions` row untouched.
        if let Err(err) = self.ensure_saxo_session_json("user_logout_keepalive").await {
            warn!("Saxo leased session refresh during user logout no-op skipped: {err:#}");
        }
        let mut status = auth::auth_status(&self.config, &self.config_path, false).await;
        if let Some(obj) = status.as_object_mut() {
            obj.insert("logout_scope".to_string(), json!("user"));
            obj.insert(
                "message".to_string(),
                json!("User logout does not disconnect the service-level Saxo session."),
            );
        }
        Ok(status)
    }

    pub async fn disconnect_saxo_session(&self) -> Result<JsonValue> {
        let status = auth::logout_session(&self.config, &self.config_path)?;
        self.clear_saxo_session_from_db().await?;
        Ok(status)
    }

    async fn ensure_runtime_state_schema(&self) -> Result<()> {
        // The database is the durable runtime state for tokens and operator
        // preferences. The on-disk session file is only an ephemeral working
        // copy for the OAuth helper functions.
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS portfolio_value_history (
                    id BIGSERIAL PRIMARY KEY,
                    recorded_at TEXT NOT NULL,
                    snapshot_type TEXT NOT NULL,
                    baseline_session_date TEXT,
                    batch_id TEXT,
                    total_market_value_dkk REAL NOT NULL,
                    invested_market_value_dkk REAL NOT NULL,
                    cash_balance_dkk REAL NOT NULL,
                    total_cost_basis_dkk REAL NOT NULL,
                    total_unrealised_pnl_dkk REAL NOT NULL,
                    total_daily_pnl_dkk REAL NOT NULL,
                    position_count INTEGER NOT NULL,
                    source TEXT,
                    raw_payload_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating portfolio value history table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS portfolio_value_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at TEXT NOT NULL,
                    snapshot_type TEXT NOT NULL,
                    baseline_session_date TEXT,
                    batch_id TEXT,
                    total_market_value_dkk REAL NOT NULL,
                    invested_market_value_dkk REAL NOT NULL,
                    cash_balance_dkk REAL NOT NULL,
                    total_cost_basis_dkk REAL NOT NULL,
                    total_unrealised_pnl_dkk REAL NOT NULL,
                    total_daily_pnl_dkk REAL NOT NULL,
                    position_count INTEGER NOT NULL,
                    source TEXT,
                    raw_payload_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating portfolio value history table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_portfolio_value_history_recorded
             ON portfolio_value_history(recorded_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating portfolio value history recorded index")?;
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS notification_deliveries (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    summary_date TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    status TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    message_text TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    error_text TEXT,
                    summary_kind TEXT NOT NULL DEFAULT 'daily'
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating notification deliveries table")?;
            sqlx::query(
                "ALTER TABLE notification_deliveries
                 ADD COLUMN IF NOT EXISTS summary_kind TEXT NOT NULL DEFAULT 'daily'",
            )
            .execute(&self.pool)
            .await
            .context("ensuring notification deliveries summary_kind column")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS notification_deliveries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    summary_date TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    status TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    message_text TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    error_text TEXT,
                    summary_kind TEXT NOT NULL DEFAULT 'daily'
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating notification deliveries table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_notification_deliveries_summary
             ON notification_deliveries(summary_date, channel, status, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating notification deliveries summary index")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notification_channel_state (
                channel TEXT PRIMARY KEY,
                summary_date TEXT,
                last_attempt_at TEXT,
                next_attempt_after TEXT,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                last_status TEXT,
                last_error_text TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating notification channel state table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notification_alert_state (
                scope_key TEXT PRIMARY KEY,
                severity TEXT NOT NULL,
                last_sent_at TEXT,
                last_alert_key TEXT,
                last_summary_kind TEXT,
                last_delivery_id INTEGER
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating notification alert state table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS saxo_sessions (
                singleton_key TEXT PRIMARY KEY,
                session_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source TEXT NOT NULL,
                refresh_lease_owner TEXT,
                refresh_lease_expires_at TEXT,
                refresh_lease_source TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Saxo session state table")?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_owner TEXT")
            .await?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_expires_at TEXT")
            .await?;
        self.ensure_table_column("saxo_sessions", "refresh_lease_source TEXT")
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runtime_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating runtime settings table")?;
        let retired_settings = self.purge_retired_runtime_settings().await?;
        if retired_settings > 0 {
            info!(
                count = retired_settings,
                "removed retired legacy runtime settings"
            );
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS price_monitor_status (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                status TEXT NOT NULL,
                summary_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating price monitor status table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS currency_fx_rates (
                currency_code TEXT NOT NULL,
                base_currency TEXT NOT NULL DEFAULT 'DKK',
                rate_to_dkk REAL NOT NULL,
                source TEXT NOT NULL,
                observed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                raw_payload_json TEXT NOT NULL,
                PRIMARY KEY (currency_code, base_currency)
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating currency FX rates table")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_currency_fx_rates_expires
             ON currency_fx_rates(expires_at)",
        )
        .execute(&self.pool)
        .await
        .context("creating currency FX rates expiry index")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_position_snapshots (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                instrument_name TEXT,
                isin TEXT,
                uic INTEGER,
                asset_type TEXT,
                quantity REAL NOT NULL,
                currency TEXT,
                open_price_local REAL,
                open_price_including_costs_local REAL,
                execution_time_open TEXT,
                value_date TEXT,
                market_state TEXT,
                can_be_closed INTEGER,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker position snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_instrument_exposures (
                symbol TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                uic INTEGER,
                asset_type TEXT,
                quantity REAL,
                average_open_price REAL,
                profit_loss_on_trade REAL,
                instrument_price_day_percent_change REAL,
                currency TEXT,
                calculation_reliability TEXT,
                can_be_closed INTEGER,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker instrument exposures table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_balance_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                currency TEXT,
                cash_available_for_trading REAL,
                margin_available_for_trading REAL,
                cash_balance REAL,
                transactions_not_booked REAL,
                settlement_value REAL,
                total_value REAL,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker balance snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS broker_account_snapshots (
                singleton_key TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                account_key TEXT,
                account_id TEXT,
                account_currency TEXT,
                is_trial_account INTEGER,
                fractional_order_enabled INTEGER,
                fractional_order_enabled_asset_types_json TEXT,
                can_use_cash_positions_as_margin_collateral INTEGER,
                use_cash_positions_as_margin_collateral INTEGER,
                legal_asset_types_json TEXT,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating broker account snapshots table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_baselines (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                activated_at TEXT,
                status TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                config_json TEXT NOT NULL,
                prompt_json TEXT NOT NULL,
                source TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy baselines table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_reflections (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                period_start TEXT NOT NULL,
                period_end TEXT NOT NULL,
                goal_version INTEGER NOT NULL,
                summary TEXT NOT NULL,
                findings_json TEXT NOT NULL,
                proposed_actions_json TEXT NOT NULL,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                baseline_id TEXT,
                goal_version INTEGER NOT NULL,
                hypothesis TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                old_value_json TEXT NOT NULL,
                new_value_json TEXT NOT NULL,
                expected_effect TEXT NOT NULL,
                risk_notes TEXT NOT NULL,
                evidence_json TEXT NOT NULL,
                approval_json TEXT,
                metrics_json TEXT,
                source_session_id TEXT,
                raw_payload_json TEXT
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_decision_advice (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                decision_report_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                source_session_id TEXT,
                overall_recommendation TEXT NOT NULL,
                summary TEXT NOT NULL,
                order_advice_json TEXT NOT NULL,
                learning_notes_json TEXT NOT NULL,
                raw_payload_json TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice table")?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS hermes_counterfactuals (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_effect TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactuals table")?;
        for sql in crate::markov_method::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Markov method runtime tables")?;
        }
        for sql in crate::quiver::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating Quiver runtime tables")?;
        }
        for sql in crate::daily_indicators::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating daily indicator runtime tables")?;
        }
        for column in [
            "nearest_support DOUBLE PRECISION",
            "next_support DOUBLE PRECISION",
            "downside_to_support_pct DOUBLE PRECISION",
            "downside_after_break_pct DOUBLE PRECISION",
            "support_break_risk DOUBLE PRECISION",
            "support_break_risk_label TEXT",
            "support_confidence DOUBLE PRECISION",
            "support_history_coverage DOUBLE PRECISION",
            "support_touch_count INTEGER",
        ] {
            self.ensure_table_column("daily_indicator_signals", column)
                .await
                .context("migrating daily indicator support-risk columns")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_reflections_created
             ON hermes_reflections(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes reflections created index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_strategy_experiments_status
             ON strategy_experiments(status, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating strategy experiments status index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_decision_advice_report
             ON hermes_decision_advice(decision_report_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice report index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_decision_advice_session
             ON hermes_decision_advice(source_session_id)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes decision advice session index")?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_hermes_counterfactuals_manager_strategy
             ON hermes_counterfactuals(manager_run_id, strategy_key)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactual manager strategy index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_hermes_counterfactuals_tracking
             ON hermes_counterfactuals(status, symbol, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating Hermes counterfactual tracking index")?;
        Ok(())
    }

    async fn ensure_table_column(&self, table: &str, column_spec: &str) -> Result<()> {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column_spec}");
        match sqlx::query(&sql).execute(&self.pool).await {
            Ok(_) => Ok(()),
            Err(err) if is_duplicate_column_error(&err) => Ok(()),
            Err(err) => Err(err).with_context(|| format!("ensuring {table}.{column_spec}")),
        }
    }

    async fn sync_saxo_session_storage(&self) -> Result<()> {
        let file_session = auth::export_session_json(&self.config, &self.config_path).ok();
        let db_session = self.load_saxo_session_from_db().await?;

        match (file_session, db_session) {
            (Some(file), Some(db)) => {
                if saxo_session_score(&db) >= saxo_session_score(&file) {
                    auth::import_session_json(&self.config, &self.config_path, &db)
                        .context("restoring Saxo session file from database")?;
                    info!("Saxo session file restored from database state");
                } else {
                    self.save_saxo_session_to_db(&file, "startup_file_sync")
                        .await?;
                    info!("Saxo session database state updated from local file");
                }
            }
            (Some(file), None) => {
                self.save_saxo_session_to_db(&file, "startup_file_import")
                    .await?;
                info!("Saxo session database state initialized from local file");
            }
            (None, Some(db)) => {
                auth::import_session_json(&self.config, &self.config_path, &db)
                    .context("restoring Saxo session file from database")?;
                info!("Saxo session file initialized from database state");
            }
            (None, None) => {
                info!("No Saxo session is cached in the file system or database");
            }
        }
        Ok(())
    }

    async fn load_saxo_session_from_db(&self) -> Result<Option<JsonValue>> {
        let Some(row) = self
            .first_json(
                "SELECT session_json, updated_at, source FROM saxo_sessions WHERE singleton_key = 'default' LIMIT 1",
            )
            .await?
        else {
            return Ok(None);
        };
        let value = row.get("session_json").cloned().unwrap_or(JsonValue::Null);
        if value.is_object() {
            return Ok(Some(value));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(
                serde_json::from_str(text).context("parsing Saxo session JSON from database")?,
            ));
        }
        Ok(None)
    }

    async fn save_saxo_session_to_db(&self, session: &JsonValue, source: &str) -> Result<()> {
        let session_text =
            serde_json::to_string(session).context("serializing Saxo session for database")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO saxo_sessions (singleton_key, session_json, updated_at, source)
             VALUES ('default', '{}', '{}', '{}')
             ON CONFLICT(singleton_key) DO UPDATE SET
                session_json = excluded.session_json,
                updated_at = excluded.updated_at,
                source = excluded.source",
            sql_escape(&session_text),
            sql_escape(&updated_at),
            sql_escape(source)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting Saxo session to database")?;
        Ok(())
    }

    pub async fn persist_saxo_session_file_to_db(&self, source: &str) -> Result<()> {
        let session = auth::export_session_json(&self.config, &self.config_path)
            .context("reading Saxo session file for database persistence")?;
        self.save_saxo_session_to_db(&session, source).await
    }

    async fn persist_invalid_saxo_session_file_to_db(&self, source: &str) -> Result<()> {
        let session = auth::export_session_json(&self.config, &self.config_path)
            .context("reading Saxo session file for invalid database persistence")?;
        if saxo_session_refresh_invalid(&session) {
            self.save_saxo_session_to_db(&session, source).await?;
        }
        Ok(())
    }

    async fn prepare_saxo_session_refresh_lease_if_needed(
        &self,
        source: &str,
    ) -> Result<Option<String>> {
        if let Err(err) = self.sync_saxo_session_storage().await {
            warn!("Saxo session restore before refresh lease check failed: {err:#}");
        }
        if !self.current_saxo_session_needs_refresh() {
            return Ok(None);
        }
        if let Some(owner) = self.acquire_saxo_session_refresh_lease(source).await? {
            return Ok(Some(owner));
        }

        info!(
            source,
            "Saxo session refresh lease is held by another process; waiting for durable session update"
        );
        for _ in 0..SAXO_SESSION_REFRESH_LEASE_WAIT_ATTEMPTS {
            sleep(StdDuration::from_secs(1)).await;
            if let Err(err) = self.sync_saxo_session_storage().await {
                warn!("Saxo session restore while waiting for refresh lease failed: {err:#}");
            }
            if !self.current_saxo_session_needs_refresh() {
                return Ok(None);
            }
            if let Some(owner) = self.acquire_saxo_session_refresh_lease(source).await? {
                return Ok(Some(owner));
            }
        }
        bail!("Saxo session refresh lease is still held by another process; refresh not attempted");
    }

    fn current_saxo_session_needs_refresh(&self) -> bool {
        auth::export_session_json(&self.config, &self.config_path)
            .map(|session| saxo_session_needs_refresh(&session))
            .unwrap_or(false)
    }

    async fn acquire_saxo_session_refresh_lease(&self, source: &str) -> Result<Option<String>> {
        let owner = saxo_refresh_lease_owner(source);
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let expires_at = (Utc::now() + Duration::seconds(SAXO_SESSION_REFRESH_LEASE_SECONDS))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let result = sqlx::query(&format!(
            "UPDATE saxo_sessions
             SET refresh_lease_owner = '{}',
                 refresh_lease_expires_at = '{}',
                 refresh_lease_source = '{}'
             WHERE singleton_key = 'default'
               AND (
                    refresh_lease_owner IS NULL
                    OR refresh_lease_owner = ''
                    OR refresh_lease_owner = '{}'
                    OR refresh_lease_expires_at IS NULL
                    OR refresh_lease_expires_at <= '{}'
               )",
            sql_escape(&owner),
            sql_escape(&expires_at),
            sql_escape(source),
            sql_escape(&owner),
            sql_escape(&now)
        ))
        .execute(&self.pool)
        .await
        .context("acquiring Saxo session refresh lease")?;
        if result.rows_affected() == 1 {
            info!(source, owner, "acquired Saxo session refresh lease");
            Ok(Some(owner))
        } else {
            Ok(None)
        }
    }

    async fn release_saxo_session_refresh_lease(&self, owner: &str) -> Result<()> {
        sqlx::query(&format!(
            "UPDATE saxo_sessions
             SET refresh_lease_owner = NULL,
                 refresh_lease_expires_at = NULL,
                 refresh_lease_source = NULL
             WHERE singleton_key = 'default'
               AND refresh_lease_owner = '{}'",
            sql_escape(owner)
        ))
        .execute(&self.pool)
        .await
        .context("releasing Saxo session refresh lease")?;
        Ok(())
    }

    pub async fn clear_saxo_session_from_db(&self) -> Result<()> {
        sqlx::query("DELETE FROM saxo_sessions WHERE singleton_key = 'default'")
            .execute(&self.pool)
            .await
            .context("clearing Saxo session from database")?;
        Ok(())
    }

    async fn runtime_setting(&self, key: &str) -> Result<Option<JsonValue>> {
        let Some(row) = self
            .first_json(&format!(
                "SELECT value_json FROM runtime_settings WHERE key = '{}' LIMIT 1",
                sql_escape(key)
            ))
            .await?
        else {
            return Ok(None);
        };
        let value = row.get("value_json").cloned().unwrap_or(JsonValue::Null);
        if value.is_object() {
            return Ok(Some(value));
        }
        if let Some(text) = value.as_str() {
            return Ok(Some(
                serde_json::from_str(text).context("parsing runtime setting JSON")?,
            ));
        }
        Ok(None)
    }

    /// Removes database settings that were only read by the retired Python
    /// scheduler. Keeping one would make a future compatibility refactor able
    /// to resurrect an unreviewed trading override.
    async fn purge_retired_runtime_settings(&self) -> Result<u64> {
        let keys = RETIRED_RUNTIME_SETTING_KEYS
            .iter()
            .map(|key| format!("'{}'", sql_escape(key)))
            .collect::<Vec<_>>()
            .join(", ");
        let result = sqlx::query(&format!(
            "DELETE FROM runtime_settings WHERE key IN ({keys})"
        ))
        .execute(&self.pool)
        .await
        .context("removing retired legacy runtime settings")?;
        Ok(result.rows_affected())
    }

    async fn save_runtime_setting(&self, key: &str, value: &JsonValue) -> Result<()> {
        let value_text = serde_json::to_string(value).context("serializing runtime setting")?;
        let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let sql = format!(
            "INSERT INTO runtime_settings (key, value_json, updated_at)
             VALUES ('{}', '{}', '{}')
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = excluded.updated_at",
            sql_escape(key),
            sql_escape(&value_text),
            sql_escape(&updated_at)
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .context("persisting runtime setting")?;
        Ok(())
    }

    pub(crate) fn market_exchange_rows(&self) -> Vec<JsonValue> {
        let cache = current_saxo_exchange_calendar_cache();
        market_exchange_rows_for_config(&self.config, Utc::now(), cache.as_ref())
    }

    async fn first_json(&self, sql: &str) -> Result<Option<JsonValue>> {
        let row = sqlx::query(sql).fetch_optional(&self.pool).await?;
        Ok(row.map(|row| row_to_json(&row)))
    }

    async fn select_json(&self, sql: &str) -> Result<Vec<JsonValue>> {
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_json).collect())
    }
}

/// Short recognizable preview of an API key: first 6 + last 4 characters
/// for long keys, fully redacted for short ones.
fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() >= 16 {
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    } else {
        "•••".to_string()
    }
}

fn annotate_overview_integrity_acknowledgements(
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

fn overview_integrity_issue_key(issue: &JsonValue) -> String {
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
        _ => "general".to_string(),
    };
    format!(
        "{}:{}:{}",
        integrity_key_part(&code),
        integrity_key_part(&severity),
        integrity_key_part(&scope)
    )
}

fn broker_cash_reconciliation_enabled(config: &YamlValue) -> bool {
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

fn broker_exposure_quantity_mismatches(
    exposures: &[JsonValue],
    positions: &HashMap<String, JsonValue>,
) -> Vec<JsonValue> {
    exposures
        .iter()
        .filter_map(|exposure| {
            let symbol = text_value(exposure, "symbol");
            if symbol.is_empty() {
                return None;
            }
            let exposure_quantity = value_f64(exposure, "quantity");
            let position_quantity = positions
                .get(&symbol)
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
                "position_updated_at": positions
                    .get(&text_value(exposure, "symbol"))
                    .and_then(|row| row.get("updated_at"))
                    .cloned()
                    .unwrap_or(JsonValue::Null)
            }))
        })
        .take(20)
        .collect()
}

async fn saxo_reference_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = json_text(session, "access_token");
    if access_token.trim().is_empty() {
        bail!("Saxo access token is missing from session");
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!(
            "{}{}",
            saxo_openapi_base_url(state, session)?,
            path
        ))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
        if let Some(error_text) = extract_saxo_error_text(&payload) {
            bail!("Saxo reference lookup failed: {error_text}");
        }
        let snippet: String = body.chars().take(300).collect();
        bail!(
            "Saxo reference lookup failed: HTTP {}: {}",
            status.as_u16(),
            snippet
        );
    }
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).context("parsing Saxo reference response")
}

fn extract_saxo_error_text(payload: &JsonValue) -> Option<String> {
    for key in ["Message", "ErrorMessage", "ErrorCode"] {
        if let Some(text) = payload.get(key).and_then(JsonValue::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    payload
        .get("ErrorInfo")
        .and_then(|value| {
            value
                .get("Message")
                .or_else(|| value.get("ErrorMessage"))
                .or_else(|| value.get("ErrorCode"))
        })
        .and_then(JsonValue::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
}

fn saxo_openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = json_text(session, "environment")
        .trim()
        .to_string()
        .to_lowercase();
    let environment = if environment.is_empty() {
        yaml_string(&state.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string())
            .to_lowercase()
    } else {
        environment
    };
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn saxo_exchange_calendar_cache_lock() -> &'static RwLock<Option<SaxoExchangeCalendarCache>> {
    SAXO_EXCHANGE_CALENDAR_CACHE.get_or_init(|| RwLock::new(None))
}

fn current_saxo_exchange_calendar_cache() -> Option<SaxoExchangeCalendarCache> {
    saxo_exchange_calendar_cache_lock()
        .read()
        .ok()
        .and_then(|cache| cache.clone())
}

fn market_exchange_rows_for_config(
    config: &YamlValue,
    now_utc: DateTime<Utc>,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> Vec<JsonValue> {
    let offset_minutes =
        yaml_i64(config, &["analysis_windows", "offset_minutes_after_open"]).unwrap_or(30);
    let pre_sync_minutes = yaml_i64(
        config,
        &["analysis_windows", "pre_sync_minutes_before_analysis"],
    )
    .unwrap_or(5);
    let end_buffer_minutes = yaml_i64(
        config,
        &["analysis_windows", "end_buffer_minutes_before_close"],
    )
    .unwrap_or(15);
    default_exchanges()
        .into_iter()
        .map(|exchange| {
            market_exchange_row(
                &exchange,
                now_utc,
                offset_minutes,
                pre_sync_minutes,
                end_buffer_minutes,
                cache,
            )
        })
        .collect()
}

fn market_exchange_row(
    exchange: &ExchangeRuntime,
    now_utc: DateTime<Utc>,
    offset_minutes: i64,
    pre_sync_minutes: i64,
    end_buffer_minutes: i64,
    cache: Option<&SaxoExchangeCalendarCache>,
) -> JsonValue {
    let tz = exchange
        .timezone
        .parse::<Tz>()
        .unwrap_or(chrono_tz::Europe::Copenhagen);
    let local_now = now_utc.with_timezone(&tz);
    let local_date = local_now.date_naive();
    let is_weekend = local_now.weekday().number_from_monday() >= 6;
    let saxo_calendar = cache.and_then(|cache| cache.exchanges.get(exchange.code));
    let configured_holiday = if !is_weekend {
        configured_holiday_name(exchange.code, local_date)
    } else {
        None
    };
    let saxo_day_session =
        saxo_calendar.and_then(|calendar| saxo_trading_session_for_date(calendar, tz, local_date));
    let day_session = saxo_day_session.or_else(|| {
        if saxo_calendar.is_none() && !is_weekend && configured_holiday.is_none() {
            let open_local = local_session_time(tz, local_date, exchange.open_time);
            let close_local = local_session_time(tz, local_date, exchange.close_time);
            Some(ExchangeDaySession {
                open_at: open_local.with_timezone(&Utc),
                close_at: close_local.with_timezone(&Utc),
            })
        } else {
            None
        }
    });
    let holiday_name = if day_session.is_none() && !is_weekend {
        configured_holiday
    } else {
        None
    };

    let current_saxo_state = saxo_calendar.and_then(|calendar| {
        calendar
            .sessions
            .iter()
            .find(|session| session.start_at <= now_utc && now_utc < session.end_at)
            .map(|session| session.state.as_str())
    });
    let calendar_source = if saxo_calendar.is_some() {
        cache
            .map(|cache| cache.source.as_str())
            .unwrap_or("saxo_ref_v1_exchanges")
    } else if holiday_name.is_some() {
        "configured_holiday"
    } else {
        "configured"
    };
    let calendar_last_checked = cache
        .map(|cache| {
            cache
                .checked_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_default();

    if let Some(day_session) = day_session {
        let open_local = day_session.open_at.with_timezone(&tz);
        let close_local = day_session.close_at.with_timezone(&tz);
        let tradable_close_local =
            close_local - Duration::minutes(exchange.tradable_close_offset_minutes);
        let tradable_close_at = tradable_close_local.with_timezone(&Utc);
        let is_open = current_saxo_state
            .map(is_saxo_open_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc <= day_session.close_at);
        let is_tradable = current_saxo_state
            .map(is_saxo_trading_state)
            .unwrap_or(now_utc >= day_session.open_at && now_utc < tradable_close_at)
            && now_utc < tradable_close_at;
        let open_analysis_start = open_local + Duration::minutes(offset_minutes);
        let open_analysis_end = std::cmp::max(
            open_analysis_start,
            tradable_close_local - Duration::minutes(end_buffer_minutes),
        );
        let pre_sync_start = std::cmp::max(
            open_local,
            open_analysis_start - Duration::minutes(pre_sync_minutes),
        );
        let pre_analysis_sync_active =
            local_now >= pre_sync_start && local_now < open_analysis_start;
        let open_analysis_window_active =
            local_now >= open_analysis_start && local_now <= open_analysis_end;
        let next_open = saxo_calendar
            .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
            .map(|value| value.with_timezone(&tz))
            .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
        let status_reason = current_saxo_state
            .map(saxo_status_reason)
            .unwrap_or_else(|| {
                if local_now < open_local {
                    "Pre-open"
                } else if local_now >= tradable_close_local && local_now <= close_local {
                    "Closed - Closing auction / post-trade"
                } else if local_now > close_local {
                    "Closed - After hours"
                } else {
                    "Open"
                }
            });
        return json!({
            "code": exchange.code,
            "market": exchange.name,
            "timezone": exchange.timezone,
            "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
            "status_reason": status_reason,
            "holiday_name": JsonValue::Null,
            "session_open_local": open_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_close_local": close_local.format("%Y-%m-%d %H:%M").to_string(),
            "tradable_close_local": tradable_close_local.format("%Y-%m-%d %H:%M").to_string(),
            "session_open_at_utc": day_session.open_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "session_close_at_utc": day_session.close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "tradable_close_at_utc": tradable_close_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "is_open": is_open,
            "is_tradable": is_tradable,
            "pre_analysis_sync_active": pre_analysis_sync_active,
            "open_analysis_window_active": open_analysis_window_active,
            "close_analysis_window_active": false,
            "analysis_window_active": open_analysis_window_active,
            "pre_analysis_sync_start_at_utc": pre_sync_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_start_at_utc": open_analysis_start.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "open_analysis_window_end_at_utc": open_analysis_end.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
            "calendar_source": calendar_source,
            "calendar_last_checked": calendar_last_checked,
            "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
            "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
            "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
            "saxo_session_state": current_saxo_state.unwrap_or_default(),
        });
    }

    let next_open = saxo_calendar
        .and_then(|calendar| next_saxo_open_time(calendar, now_utc))
        .map(|value| value.with_timezone(&tz))
        .unwrap_or_else(|| next_open_time(tz, exchange, local_now));
    let status_reason = if is_weekend {
        "Closed - Weekend".to_string()
    } else if let Some(holiday) = holiday_name {
        format!("Closed - {holiday}")
    } else if saxo_calendar.is_some() {
        "Closed - No Saxo trading session".to_string()
    } else {
        let open_local = local_session_time(tz, local_date, exchange.open_time);
        if local_now < open_local {
            "Pre-open".to_string()
        } else {
            "Closed - After hours".to_string()
        }
    };

    json!({
        "code": exchange.code,
        "market": exchange.name,
        "timezone": exchange.timezone,
        "local_time": local_now.format("%Y-%m-%d %H:%M").to_string(),
        "status_reason": status_reason,
        "holiday_name": holiday_name.unwrap_or_default(),
        "session_open_local": "n/a",
        "session_close_local": "n/a",
        "tradable_close_local": "n/a",
        "session_open_at_utc": JsonValue::Null,
        "session_close_at_utc": JsonValue::Null,
        "tradable_close_at_utc": JsonValue::Null,
        "is_open": false,
        "is_tradable": false,
        "pre_analysis_sync_active": false,
        "open_analysis_window_active": false,
        "close_analysis_window_active": false,
        "analysis_window_active": false,
        "pre_analysis_sync_start_at_utc": JsonValue::Null,
        "open_analysis_window_start_at_utc": JsonValue::Null,
        "open_analysis_window_end_at_utc": JsonValue::Null,
        "next_open_at_utc": next_open.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "next_open": next_open.format("%Y-%m-%d %H:%M").to_string(),
        "calendar_source": calendar_source,
        "calendar_last_checked": calendar_last_checked,
        "saxo_exchange_id": saxo_calendar.map(|calendar| calendar.exchange_id.clone()).unwrap_or_default(),
        "saxo_exchange_name": saxo_calendar.and_then(|calendar| calendar.name.clone()).unwrap_or_default(),
        "saxo_timezone_id": saxo_calendar.and_then(|calendar| calendar.timezone_id.clone()).unwrap_or_default(),
        "saxo_session_state": current_saxo_state.unwrap_or_default(),
    })
}

fn saxo_session_score(session: &JsonValue) -> (i64, i64) {
    let now = Utc::now().timestamp();
    let refresh_invalid = saxo_session_refresh_invalid(session);
    let has_refresh = non_empty_session_text(session.get("refresh_token")).is_some();
    let has_access = non_empty_session_text(session.get("access_token")).is_some();
    let refresh_expires_at = parse_session_time(session.get("refresh_token_expires_at"));
    let access_expires_at = parse_session_time(session.get("access_token_expires_at"));

    // Compare health before recency. A freshly marked-invalid cache should never
    // overwrite an older cache that still has a usable refresh token.
    let health = if refresh_invalid {
        0
    } else if has_refresh && refresh_expires_at.is_none_or(|expires_at| expires_at > now) {
        3
    } else if has_access && access_expires_at.is_some_and(|expires_at| expires_at > now) {
        1
    } else {
        0
    };

    (health, saxo_session_rank(session))
}

fn saxo_session_refresh_invalid(session: &JsonValue) -> bool {
    non_empty_session_text(session.get("refresh_token_invalid_at")).is_some()
}

struct ExchangeRuntime {
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_time: NaiveTime,
    close_time: NaiveTime,
    tradable_close_offset_minutes: i64,
}

fn default_exchanges() -> Vec<ExchangeRuntime> {
    vec![
        exchange("XCSE", "Copenhagen", "Europe/Copenhagen", 9, 0, 17, 0, 0),
        exchange("XLON", "London", "Europe/London", 8, 0, 16, 30, 0),
        exchange(
            "XETR",
            "Frankfurt / Xetra",
            "Europe/Berlin",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange(
            "XAMS",
            "Amsterdam / Euronext",
            "Europe/Amsterdam",
            9,
            0,
            17,
            30,
            0,
        ),
        exchange("XNAS", "Nasdaq US", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XNYS", "NYSE", "America/New_York", 9, 30, 16, 0, 0),
        exchange("XSTO", "Stockholm", "Europe/Stockholm", 9, 0, 17, 30, 0),
        exchange("XOSL", "Oslo", "Europe/Oslo", 9, 0, 16, 30, 5),
        exchange("XHEL", "Helsinki", "Europe/Helsinki", 10, 0, 18, 30, 0),
        exchange("XMIL", "Milan", "Europe/Rome", 9, 0, 17, 30, 0),
    ]
}

fn exchange(
    code: &'static str,
    name: &'static str,
    timezone: &'static str,
    open_hour: u32,
    open_minute: u32,
    close_hour: u32,
    close_minute: u32,
    tradable_close_offset_minutes: i64,
) -> ExchangeRuntime {
    ExchangeRuntime {
        code,
        name,
        timezone,
        open_time: NaiveTime::from_hms_opt(open_hour, open_minute, 0).unwrap_or(NaiveTime::MIN),
        close_time: NaiveTime::from_hms_opt(close_hour, close_minute, 0).unwrap_or(NaiveTime::MIN),
        tradable_close_offset_minutes,
    }
}

fn saxo_exchange_calendar_from_detail(
    detail: &JsonValue,
    fallback_exchange_id: &str,
) -> Option<SaxoExchangeCalendar> {
    let exchange_id = saxo_exchange_text(detail, "ExchangeId")
        .unwrap_or_else(|| fallback_exchange_id.to_string());
    let sessions = parse_saxo_exchange_sessions(detail);
    if sessions.is_empty() {
        return None;
    }
    Some(SaxoExchangeCalendar {
        exchange_id,
        name: saxo_exchange_text(detail, "Name"),
        timezone_id: saxo_exchange_text(detail, "TimeZoneId"),
        sessions,
    })
}

fn parse_saxo_exchange_sessions(detail: &JsonValue) -> Vec<SaxoExchangeSession> {
    let Some(sessions) = detail.get("ExchangeSessions").and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter_map(|session| {
            let start = saxo_exchange_text(session, "StartTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let end = saxo_exchange_text(session, "EndTime")
                .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())?
                .with_timezone(&Utc);
            let state =
                saxo_exchange_text(session, "State").unwrap_or_else(|| "Undefined".to_string());
            Some(SaxoExchangeSession {
                start_at: start,
                end_at: end,
                state,
            })
        })
        .collect()
}

fn saxo_exchange_matches(value: &JsonValue, code: &str) -> bool {
    ["ExchangeId", "Mic", "IsoMic", "OperatingMic"]
        .iter()
        .filter_map(|key| saxo_exchange_text(value, key))
        .any(|value| value.eq_ignore_ascii_case(code))
}

fn saxo_exchange_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn saxo_trading_session_for_date(
    calendar: &SaxoExchangeCalendar,
    tz: Tz,
    local_date: NaiveDate,
) -> Option<ExchangeDaySession> {
    let sessions = calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_continuous_trading_state(&session.state))
        .filter(|session| session_overlaps_local_date(session, tz, local_date))
        .collect::<Vec<_>>();
    let open_at = sessions.iter().map(|session| session.start_at).min()?;
    let close_at = sessions.iter().map(|session| session.end_at).max()?;
    Some(ExchangeDaySession { open_at, close_at })
}

fn next_saxo_open_time(
    calendar: &SaxoExchangeCalendar,
    now_utc: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    calendar
        .sessions
        .iter()
        .filter(|session| is_saxo_continuous_trading_state(&session.state))
        .filter(|session| session.start_at > now_utc)
        .map(|session| session.start_at)
        .min()
}

fn session_overlaps_local_date(
    session: &SaxoExchangeSession,
    tz: Tz,
    local_date: NaiveDate,
) -> bool {
    let start_date = session.start_at.with_timezone(&tz).date_naive();
    let end_date = (session.end_at - Duration::seconds(1))
        .with_timezone(&tz)
        .date_naive();
    start_date <= local_date && local_date <= end_date
}

fn is_saxo_trading_state(state: &str) -> bool {
    is_saxo_continuous_trading_state(state)
}

fn is_saxo_continuous_trading_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "automatedtrading" | "pittrading"
    )
}

fn is_saxo_open_state(state: &str) -> bool {
    !matches!(
        state.to_ascii_lowercase().as_str(),
        "closed" | "break" | "halt" | "suspended" | "undefined"
    )
}

fn saxo_status_reason(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "automatedtrading" | "pittrading" | "callauctiontrading" | "auction" | "openingauction"
        | "tradingatlast" => "Open",
        "preautomatedtrading" | "premarket" | "pretrading" => "Pre-open",
        "postautomatedtrading" | "postmarket" | "posttrading" => {
            "Closed - Closing auction / post-trade"
        }
        "break" => "Closed - Exchange break",
        "halt" => "Closed - Halted",
        "suspended" => "Closed - Suspended",
        "closed" => "Closed",
        _ => "Closed - Unknown Saxo session state",
    }
}

fn configured_holiday_name(exchange_code: &str, local_date: NaiveDate) -> Option<&'static str> {
    match (
        exchange_code,
        local_date.year(),
        local_date.month(),
        local_date.day(),
    ) {
        ("XCSE", 2026, 1, 1) => Some("New Year's Day"),
        ("XCSE", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XCSE", 2026, 4, 3) => Some("Good Friday"),
        ("XCSE", 2026, 4, 6) => Some("Easter Monday"),
        ("XCSE", 2026, 5, 14) => Some("Ascension Day"),
        ("XCSE", 2026, 5, 15) => Some("Day after Ascension Day"),
        ("XCSE", 2026, 5, 25) => Some("Whit Monday"),
        ("XCSE", 2026, 6, 5) => Some("Constitution Day"),
        ("XCSE", 2026, 12, 24) => Some("Christmas Eve"),
        ("XCSE", 2026, 12, 25) => Some("Christmas Day"),
        ("XCSE", 2026, 12, 31) => Some("New Year's Eve"),
        ("XLON", 2026, 1, 1) => Some("New Year's Day"),
        ("XLON", 2026, 4, 3) => Some("Good Friday"),
        ("XLON", 2026, 4, 6) => Some("Easter Monday"),
        ("XLON", 2026, 5, 4) => Some("Early May bank holiday"),
        ("XLON", 2026, 5, 25) => Some("Spring bank holiday"),
        ("XLON", 2026, 8, 31) => Some("Summer bank holiday"),
        ("XLON", 2026, 12, 25) => Some("Christmas Day"),
        ("XLON", 2026, 12, 28) => Some("Boxing Day (substitute day)"),
        ("XETR", 2026, 1, 1) => Some("New Year's Day"),
        ("XETR", 2026, 4, 3) => Some("Good Friday"),
        ("XETR", 2026, 4, 6) => Some("Easter Monday"),
        ("XETR", 2026, 12, 24) => Some("Christmas Eve"),
        ("XETR", 2026, 12, 25) => Some("Christmas Day"),
        ("XETR", 2026, 12, 31) => Some("New Year's Eve"),
        ("XAMS", 2026, 1, 1) => Some("New Year's Day"),
        ("XAMS", 2026, 4, 3) => Some("Good Friday"),
        ("XAMS", 2026, 4, 6) => Some("Easter Monday"),
        ("XAMS", 2026, 5, 1) => Some("Labour Day"),
        ("XAMS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNAS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNAS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNAS", 2026, 2, 16) => Some("Presidents Day"),
        ("XNAS", 2026, 4, 3) => Some("Good Friday"),
        ("XNAS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNAS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNAS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNAS", 2026, 9, 7) => Some("Labor Day"),
        ("XNAS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNAS", 2026, 12, 25) => Some("Christmas Day"),
        ("XNYS", 2026, 1, 1) => Some("New Year's Day"),
        ("XNYS", 2026, 1, 19) => Some("Martin Luther King Jr. Day"),
        ("XNYS", 2026, 2, 16) => Some("Washington's Birthday"),
        ("XNYS", 2026, 4, 3) => Some("Good Friday"),
        ("XNYS", 2026, 5, 25) => Some("Memorial Day"),
        ("XNYS", 2026, 6, 19) => Some("Juneteenth"),
        ("XNYS", 2026, 7, 3) => Some("Independence Day (observed)"),
        ("XNYS", 2026, 9, 7) => Some("Labor Day"),
        ("XNYS", 2026, 11, 26) => Some("Thanksgiving Day"),
        ("XNYS", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 1, 1) => Some("New Year's Day"),
        ("XSTO", 2026, 1, 6) => Some("Epiphany"),
        ("XSTO", 2026, 4, 3) => Some("Good Friday"),
        ("XSTO", 2026, 4, 6) => Some("Easter Monday"),
        ("XSTO", 2026, 5, 1) => Some("Labour Day"),
        ("XSTO", 2026, 5, 14) => Some("Ascension Day"),
        ("XSTO", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XSTO", 2026, 12, 24) => Some("Christmas Eve"),
        ("XSTO", 2026, 12, 25) => Some("Christmas Day"),
        ("XSTO", 2026, 12, 31) => Some("New Year's Eve"),
        ("XOSL", 2026, 1, 1) => Some("New Year's Day"),
        ("XOSL", 2026, 4, 2) => Some("Maundy Thursday"),
        ("XOSL", 2026, 4, 3) => Some("Good Friday"),
        ("XOSL", 2026, 4, 6) => Some("Easter Monday"),
        ("XOSL", 2026, 5, 1) => Some("Labour Day"),
        ("XOSL", 2026, 5, 14) => Some("Ascension Day"),
        ("XOSL", 2026, 5, 25) => Some("Whit Monday"),
        ("XOSL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XOSL", 2026, 12, 25) => Some("Christmas Day"),
        ("XOSL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XHEL", 2026, 1, 1) => Some("New Year's Day"),
        ("XHEL", 2026, 1, 6) => Some("Epiphany"),
        ("XHEL", 2026, 4, 3) => Some("Good Friday"),
        ("XHEL", 2026, 4, 6) => Some("Easter Monday"),
        ("XHEL", 2026, 5, 1) => Some("Labour Day"),
        ("XHEL", 2026, 5, 14) => Some("Ascension Day"),
        ("XHEL", 2026, 6, 19) => Some("Midsummer Eve"),
        ("XHEL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XHEL", 2026, 12, 25) => Some("Christmas Day"),
        ("XHEL", 2026, 12, 31) => Some("New Year's Eve"),
        ("XMIL", 2026, 1, 1) => Some("New Year's Day"),
        ("XMIL", 2026, 4, 3) => Some("Good Friday"),
        ("XMIL", 2026, 4, 6) => Some("Easter Monday"),
        ("XMIL", 2026, 5, 1) => Some("Labour Day"),
        ("XMIL", 2026, 12, 24) => Some("Christmas Eve"),
        ("XMIL", 2026, 12, 25) => Some("Christmas Day"),
        ("XMIL", 2026, 12, 31) => Some("New Year's Eve"),
        _ => None,
    }
}

fn local_session_time(tz: Tz, date: chrono::NaiveDate, time: NaiveTime) -> DateTime<Tz> {
    tz.with_ymd_and_hms(
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute(),
        0,
    )
    .single()
    .unwrap_or_else(|| Utc::now().with_timezone(&tz))
}

fn next_open_time(tz: Tz, exchange: &ExchangeRuntime, local_now: DateTime<Tz>) -> DateTime<Tz> {
    for offset in 0..10 {
        let candidate_date = local_now.date_naive() + Duration::days(offset);
        if candidate_date.weekday().number_from_monday() >= 6 {
            continue;
        }
        if configured_holiday_name(exchange.code, candidate_date).is_some() {
            continue;
        }
        let candidate = local_session_time(tz, candidate_date, exchange.open_time);
        if candidate > local_now {
            return candidate;
        }
    }
    local_session_time(
        tz,
        local_now.date_naive() + Duration::days(1),
        exchange.open_time,
    )
}

fn market_names_where(items: &[JsonValue], key: &str) -> Vec<String> {
    items
        .iter()
        .filter(|row| row.get(key).and_then(JsonValue::as_bool).unwrap_or(false))
        .filter_map(|row| row.get("market").and_then(JsonValue::as_str))
        .map(ToString::to_string)
        .collect()
}

fn performance_start_at(range_key: &str) -> Option<String> {
    let now = Utc::now();
    let start = match range_key.to_uppercase().as_str() {
        "1D" => now - Duration::days(1),
        "1W" => now - Duration::weeks(1),
        "1M" => now - Duration::days(31),
        "3M" => now - Duration::days(93),
        "YTD" => Utc
            .with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now - Duration::days(31)),
        "1Y" => now - Duration::days(366),
        "ALL" => return None,
        _ => now - Duration::days(1),
    };
    Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn performance_rows_have_same_values(left: &JsonValue, right: &JsonValue) -> bool {
    const EPSILON_DKK: f64 = 0.01;
    let numeric_keys = [
        "total_market_value_dkk",
        "invested_market_value_dkk",
        "cash_balance_dkk",
        "total_cost_basis_dkk",
        "total_unrealised_pnl_dkk",
        "total_daily_pnl_dkk",
    ];
    numeric_keys
        .iter()
        .all(|key| (value_f64(left, key) - value_f64(right, key)).abs() <= EPSILON_DKK)
        && value_i64(left, "position_count") == value_i64(right, "position_count")
}

fn text_value(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

trait BlankStringExt {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>;
    fn non_empty_or_none(self) -> Option<String>;
}

impl BlankStringExt for String {
    fn if_empty_then<F>(self, fallback: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        if self.trim().is_empty() {
            fallback()
        } else {
            Some(self)
        }
    }

    fn non_empty_or_none(self) -> Option<String> {
        if self.trim().is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

const HERMES_EXPERIMENT_DUPLICATE_BLOCKING_STATUSES: &[&str] = &[
    "pending_review",
    "approved_paper",
    "active_paper",
    "approved_sim",
    "active_sim",
    "ready_for_promotion",
];

const HERMES_EXPERIMENT_REVIEW_FAMILIES: &[(&str, &str)] = &[
    ("strategy.capital.min_cash_buffer_pct", "cash_buffer_policy"),
    ("strategy.swing.cash_buffer_pct", "cash_buffer_policy"),
];

fn normalize_hermes_experiment_variable_path(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn hermes_experiment_review_family(value: &str) -> Option<&'static str> {
    let normalized = normalize_hermes_experiment_variable_path(value);
    HERMES_EXPERIMENT_REVIEW_FAMILIES
        .iter()
        .find_map(|(path, family)| (*path == normalized).then_some(*family))
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

fn hermes_experiment_status_blocks_duplicate(status: &str) -> bool {
    HERMES_EXPERIMENT_DUPLICATE_BLOCKING_STATUSES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(status.trim()))
}

fn exchange_code(symbol: &str) -> String {
    symbol
        .split_once(':')
        .map(|(_, exchange)| exchange.to_string())
        .unwrap_or_default()
}

/// Returns the versioned, source-controlled analysis universe. It deliberately
/// does not read historical report, sentiment, or quote tables: those tables
/// describe observations, not the set of assets the system is meant to study.
fn configured_watchlist_universe_symbols(config: &YamlValue) -> Vec<String> {
    configured_watchlist_symbols_at(
        config,
        &["market_data", "watchlists", "universe_symbols"],
        false,
    )
}

/// Extra watches are candidates in addition to the main universe. Their
/// mapping form may carry an ISIN for Saxo resolution; membership needs only
/// the explicit symbol.
fn configured_extra_watch_symbols(config: &YamlValue) -> Vec<String> {
    configured_watchlist_symbols_at(
        config,
        &["market_data", "watchlists", "extra_symbols"],
        true,
    )
}

fn configured_watchlist_symbols_at(
    config: &YamlValue,
    keys: &[&str],
    accept_mapping_symbols: bool,
) -> Vec<String> {
    let mut seen = HashSet::new();
    crate::config::yaml_at(config, keys)
        .and_then(YamlValue::as_sequence)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .as_str()
                        .or_else(|| {
                            accept_mapping_symbols
                                .then(|| entry.get("symbol").and_then(YamlValue::as_str))
                                .flatten()
                        })
                        .and_then(normalize_watchlist_symbol)
                })
                .filter(|symbol| seen.insert(watchlist_symbol_key(symbol)))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_watchlist_symbol(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (base, exchange) = trimmed
        .split_once(':')
        .map(|(base, exchange)| (base.trim(), Some(exchange.trim())))
        .unwrap_or((trimmed, None));
    if base.is_empty() || exchange.is_some_and(str::is_empty) {
        return None;
    }
    Some(match exchange {
        Some(exchange) => format!(
            "{}:{}",
            base.to_ascii_uppercase(),
            exchange.to_ascii_lowercase()
        ),
        None => base.to_ascii_uppercase(),
    })
}

fn watchlist_symbol_key(symbol: &str) -> String {
    symbol.trim().to_ascii_lowercase()
}

fn configured_watchlist_row(symbol: &str, source: &str) -> JsonValue {
    json!({
        "symbol": symbol,
        "instrument_name": instrument_name_for_symbol(symbol),
        "quote_status": "configured_universe",
        "source": source,
    })
}

fn enrich_execution_order_lifecycle(order: &mut JsonValue, market_rows: &[JsonValue]) {
    let status = json_text(order, "status").to_ascii_lowercase();
    let active_broker_order = matches!(
        status.as_str(),
        "submitted_to_broker"
            | "broker_working"
            | "broker_amended"
            | "broker_partially_filled"
            | "broker_replace_requested"
            | "broker_cancel_requested"
    );
    if !active_broker_order {
        return;
    }

    let duration_type = execution_order_duration_type(order);
    let exchange = exchange_code(
        order
            .get("symbol")
            .and_then(JsonValue::as_str)
            .unwrap_or_default(),
    )
    .to_ascii_lowercase();
    let lifecycle = if duration_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("DayOrder"))
    {
        market_rows
            .iter()
            .find(|row| {
                row.get("code")
                    .and_then(JsonValue::as_str)
                    .map(|code| code.eq_ignore_ascii_case(&exchange))
                    .unwrap_or(false)
            })
            .map(|market| {
                (
                    json_text(market, "tradable_close_at_utc")
                        .non_empty_or_none()
                        .or_else(|| json_text(market, "session_close_at_utc").non_empty_or_none()),
                    market
                        .get("market")
                        .cloned()
                        .unwrap_or_else(|| json!(exchange)),
                    market.get("timezone").cloned().unwrap_or(JsonValue::Null),
                )
            })
    } else {
        None
    };

    let Some(object) = order.as_object_mut() else {
        return;
    };
    if let Some(duration_type) = duration_type.as_deref() {
        object.insert("order_duration_type".to_string(), json!(duration_type));
    }
    if let Some((expiry, market, timezone)) = lifecycle {
        if let Some(expiry) = expiry {
            let expiry_pending = parse_rfc3339_utc(Some(&json!(expiry.clone())))
                .map(|expiry_at| {
                    expiry_at + Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES) <= Utc::now()
                })
                .unwrap_or(false);
            object.insert("expected_expiry_at_utc".to_string(), json!(expiry));
            if expiry_pending {
                object.insert(
                    "lifecycle_state".to_string(),
                    json!("expiry_pending_broker_sync"),
                );
            }
        }
        object.insert(
            "expected_expiry_source".to_string(),
            json!("exchange_calendar"),
        );
        object.insert("expected_expiry_market".to_string(), market);
        object.insert("expected_expiry_timezone".to_string(), timezone);
        object.insert(
            "lifecycle_note".to_string(),
            if object
                .get("lifecycle_state")
                .and_then(JsonValue::as_str)
                == Some("expiry_pending_broker_sync")
            {
                json!("Expected DayOrder expiry has passed; waiting for Saxo broker sync to confirm fill, cancel, reject, or expiry.")
            } else {
                json!("DayOrder remains live until broker fill, cancel, reject, or exchange-day expiry sync.")
            },
        );
    }
}

fn execution_order_duration_type(order: &JsonValue) -> Option<String> {
    nested_json_text(
        order,
        &[
            "execution_result_json",
            "payload",
            "OrderDuration",
            "DurationType",
        ],
    )
    .non_empty_or_none()
    .or_else(|| {
        nested_json_text(
            order,
            &[
                "execution_result_json",
                "broker_sync",
                "broker_payload",
                "Duration",
                "DurationType",
            ],
        )
        .non_empty_or_none()
    })
    .or_else(|| {
        nested_json_text(
            order,
            &[
                "execution_result_json",
                "broker_sync",
                "broker_payload",
                "OrderDuration",
                "DurationType",
            ],
        )
        .non_empty_or_none()
    })
}

fn exchange_region(symbol: &str) -> String {
    match exchange_code(symbol).to_lowercase().as_str() {
        "xcse" | "xsto" | "xosl" | "xhel" => "Nordics",
        "xlon" => "UK",
        "xnas" | "xnys" => "US",
        _ => "Europe",
    }
    .to_string()
}

fn localization_settings_key(sso_session: &JsonValue) -> String {
    let user_key = sso_session
        .get("user")
        .and_then(|user| user.get("email"))
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default");
    format!("localization:{user_key}")
}

fn instrument_name_for_symbol(symbol: &str) -> String {
    let base = symbol
        .split_once(':')
        .map(|(base, _)| base)
        .unwrap_or(symbol);
    match base.to_uppercase().as_str() {
        "AAPL" => "Apple".to_string(),
        "ADBE" => "Adobe".to_string(),
        "ADI" => "Analog Devices".to_string(),
        "AMD" => "Advanced Micro Devices".to_string(),
        "AMZN" => "Amazon.com".to_string(),
        "ASML" => "ASML ADR".to_string(),
        "AVGO" => "Broadcom".to_string(),
        "DDOG" => "Datadog".to_string(),
        "GOOGL" => "Alphabet Inc. Class A".to_string(),
        "IBM" => "IBM".to_string(),
        "INTC" => "Intel".to_string(),
        "MA" => "Mastercard".to_string(),
        "MDB" => "MongoDB".to_string(),
        "MSTR" => "MicroStrategy".to_string(),
        "NVDA" => "NVIDIA".to_string(),
        "PANW" => "Palo Alto Networks".to_string(),
        "PLTR" => "Palantir Technologies".to_string(),
        "QCOM" => "Qualcomm".to_string(),
        "SNOW" => "Snowflake".to_string(),
        "V" => "Visa".to_string(),
        other => other.to_string(),
    }
}

fn saxo_session_rank(session: &JsonValue) -> i64 {
    // The latest useful timestamp wins within the same health tier. Refreshes update
    // `last_refreshed_at`, while a fresh OAuth callback may only have `created_at`.
    [
        "last_refreshed_at",
        "created_at",
        "access_token_expires_at",
        "refresh_token_expires_at",
    ]
    .iter()
    .filter_map(|key| parse_session_time(session.get(*key)))
    .max()
    .unwrap_or(0)
}

fn parse_session_time(value: Option<&JsonValue>) -> Option<i64> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.timestamp())
}

fn non_empty_session_text(value: Option<&JsonValue>) -> Option<&str> {
    let text = value?.as_str()?;
    if text.is_empty() { None } else { Some(text) }
}

#[allow(dead_code)]
fn deterministic_selected_assets(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter().take(12) {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        selected.push(json!({
            "symbol": symbol,
            "score": (value_f64(row, "allocation_pct") * 100.0).max(50.0),
            "notes": "Existing portfolio holding included in the manual fallback review.",
            "source": "portfolio"
        }));
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items.iter().take(6) {
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() || !seen.insert(symbol.to_string()) {
                    continue;
                }
                selected.push(json!({
                    "symbol": symbol,
                    "score": value_f64(row, "change_pct").abs().max(50.0),
                    "notes": "Watchlist symbol included in the manual fallback review.",
                    "source": category.get("key").and_then(JsonValue::as_str).unwrap_or("watchlist")
                }));
                if selected.len() >= 20 {
                    return selected;
                }
            }
        }
    }
    selected
}

#[allow(dead_code)]
fn deterministic_symbol_sentiment(
    positions: &[JsonValue],
    selected_assets: &[JsonValue],
) -> Vec<JsonValue> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for row in positions.iter() {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let daily = value_f64(row, "daily_pnl_dkk");
        let sentiment = if allocation > 0.15 {
            "UNDERWEIGHT"
        } else if daily < -500.0 {
            "UNDERWEIGHT"
        } else {
            "HOLD"
        };
        rows.push(json!({
            "symbol": symbol,
            "sentiment": sentiment,
            "confidence": 50.0,
            "rationale": "Manual Rust fallback based on current allocation and daily P/L.",
            "risk_notes": ["Review manually before creating orders."]
        }));
    }
    for row in selected_assets {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() || !seen.insert(symbol.to_string()) {
            continue;
        }
        rows.push(json!({
            "symbol": symbol,
            "sentiment": "HOLD",
            "confidence": value_f64(row, "score"),
            "rationale": row.get("notes").cloned().unwrap_or_else(|| JsonValue::from("Manual fallback candidate.")),
            "risk_notes": ["No automated broker order was created."]
        }));
    }
    rows
}

#[allow(dead_code)]
fn deterministic_suggested_trades(
    positions: &[JsonValue],
    watchlists: &JsonValue,
) -> Vec<JsonValue> {
    let mut trades = Vec::new();
    for row in positions {
        let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
        if symbol.is_empty() {
            continue;
        }
        let allocation = value_f64(row, "allocation_pct");
        let unrealised = value_f64(row, "unrealised_pnl_dkk");
        if allocation > 0.15 || unrealised < -4000.0 {
            trades.push(json!({
                "symbol": symbol,
                "action": "SELL",
                "priority": "medium",
                "confidence": if allocation > 0.15 { 56.0 } else { 52.0 },
                "quantity_hint": "Reduce toward target allocation",
                "target_weight_pct": 5.56,
                "rationale": "Manual fallback flagged concentration or drawdown for operator review.",
                "risk_notes": ["No automatic order was queued."]
            }));
        }
        if trades.len() >= 6 {
            return trades;
        }
    }
    if let Some(categories) = watchlists.get("categories").and_then(JsonValue::as_array) {
        for category in categories {
            let Some(items) = category.get("items").and_then(JsonValue::as_array) else {
                continue;
            };
            for row in items {
                if value_f64(row, "change_pct") <= 2.0 {
                    continue;
                }
                let symbol = row.get("symbol").and_then(JsonValue::as_str).unwrap_or("");
                if symbol.is_empty() {
                    continue;
                }
                trades.push(json!({
                    "symbol": symbol,
                    "action": "BUY",
                    "priority": "medium",
                    "confidence": value_f64(row, "change_pct").min(75.0),
                    "quantity_hint": "Review for possible starter allocation",
                    "target_weight_pct": 5.56,
                    "rationale": "Manual fallback highlighted positive watchlist momentum.",
                    "risk_notes": ["Confirm thesis, liquidity, and market window before trading."]
                }));
                if trades.len() >= 6 {
                    return trades;
                }
            }
        }
    }
    trades
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

fn normalize_hermes_context_self_check(value: JsonValue) -> JsonValue {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_lessons_pending_review_projects_safe_recent_actions() {
        let reflections = vec![
            json!({
                "id": "newer-reflection",
                "created_at": "2026-07-23T14:00:00Z",
                "period_start": "2026-07-23",
                "period_end": "2026-07-23",
                "goal_version": 1,
                "summary": "Fresh evidence supports an operator review.",
                "source_session_id": "daily-eod-reflection-2026-07-23",
                "raw_payload_json": {"secret": "must-not-leak"},
                "proposed_actions_json": {
                    "actions": [
                        {"action": "  Compare the Markov horizon against the next five reports.  "},
                        {"recommendation": "Review the cash-buffer evidence before proposing a change."}
                    ]
                }
            }),
            json!({
                "id": "older-reflection",
                "created_at": "2026-07-22T14:00:00Z",
                "summary": "Older evidence.",
                "proposed_actions_json": [
                    "Compare the Markov horizon against the next five reports.",
                    {"unsupported": "not surfaced"}
                ]
            }),
        ];

        let lessons = hermes_lessons_pending_review_from_reflections(&reflections, 30);

        assert_eq!(lessons.len(), 2);
        assert_eq!(
            lessons[0]["lesson"],
            json!("Compare the Markov horizon against the next five reports.")
        );
        assert_eq!(
            lessons[1]["lesson"],
            json!("Review the cash-buffer evidence before proposing a change.")
        );
        assert_eq!(lessons[0]["reflection_id"], json!("newer-reflection"));
        assert_eq!(
            lessons[0]["source_session_id"],
            json!("daily-eod-reflection-2026-07-23")
        );
        assert!(lessons[0].get("raw_payload_json").is_none());
    }

    #[test]
    fn hermes_lessons_pending_review_caps_and_ignores_empty_actions() {
        let reflections = vec![json!({
            "id": "reflection",
            "proposed_actions_json": ["", "First review", "Second review"]
        })];

        let lessons = hermes_lessons_pending_review_from_reflections(&reflections, 1);

        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0]["lesson"], json!("First review"));
    }

    #[test]
    fn hermes_lessons_pending_review_redacts_sensitive_action_text() {
        let reflections = vec![json!({
            "id": "reflection",
            "proposed_actions_json": ["Investigate refresh_token=do-not-display"]
        })];

        let lessons = hermes_lessons_pending_review_from_reflections(&reflections, 30);

        assert_eq!(lessons.len(), 1);
        assert_eq!(
            lessons[0]["lesson"],
            json!("[redacted potentially sensitive reflection action]")
        );
        assert!(
            !serde_json::to_string(&lessons)
                .expect("serialize lessons")
                .contains("do-not-display")
        );
    }

    #[test]
    fn one_variable_audit_distinguishes_baseline_from_selected_overlay() {
        let baseline = json!({
            "id": "baseline-1",
            "activated_at": "2026-07-23T08:00:00Z",
            "config_json": {
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value": 0.05,
                "new_value": 0.02,
                "hypothesis": "Use less idle cash when qualified signals are available."
            }
        });
        let overlay_audit = json!({
            "state": "selected_for_next_cycle",
            "execution_mode": "live",
            "saxo_environment": "SIM",
            "candidate": {
                "id": "experiment-2",
                "status": "active_sim",
                "changed_variable_path": "strategy.swing.daily_indicators.min_confluences",
                "old_value": 4,
                "new_value": 3,
                "hypothesis": "Allow a controlled SIM comparison."
            }
        });
        let latest_manager_run = json!({
            "created_at": "2026-07-23T09:15:00Z",
            "manager_json": {
                "strategy_experiment_overlay": {"id": "experiment-2"}
            }
        });

        let rows =
            hermes_one_variable_audit_from_snapshot(&baseline, &overlay_audit, &latest_manager_run);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], json!("promoted_baseline"));
        assert_eq!(rows[0]["status"], json!("record_only"));
        assert_eq!(rows[1]["kind"], json!("selected_overlay"));
        assert_eq!(rows[1]["status"], json!("selected_for_next_cycle"));
        assert_eq!(
            rows[1]["last_manager_state"],
            json!("observed in latest manager run")
        );
        assert!(
            serde_json::to_string(&rows)
                .expect("serialize audit rows")
                .contains("controlled SIM comparison")
        );
    }

    #[test]
    fn one_variable_audit_never_claims_a_live_config_change() {
        let rows = hermes_one_variable_audit_from_snapshot(
            &JsonValue::Null,
            &json!({
                "state": "disabled_live_environment",
                "execution_mode": "live",
                "saxo_environment": "LIVE",
                "candidate": null,
            }),
            &JsonValue::Null,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], json!("none"));
        assert_eq!(rows[0]["status"], json!("disabled_live_environment"));
        assert!(
            rows[0]["scope"]
                .as_str()
                .unwrap_or_default()
                .contains("no promoted baseline")
        );
    }

    #[test]
    fn proposal_quality_rubric_marks_complete_safe_proposal_review_ready() {
        let quality = hermes_proposal_quality_from_experiments(&[json!({
            "id": "proposal-ready",
            "created_at": "2026-07-23T10:00:00Z",
            "status": "pending_review",
            "changed_variable_path": "strategy.swing.markov_gate.min_signed_signal",
            "old_value_json": 0.15,
            "new_value_json": 0.20,
            "expected_effect": "Raise the minimum signal threshold to reduce failure rate and drawdown.",
            "risk_notes": "Could reduce eligible BUY opportunities.",
            "evidence_json": {
                "report_ids": [201, 202],
                "markov": {"fresh": true},
                "metrics": {"failure_rate": 0.20}
            }
        })]);

        assert_eq!(quality.len(), 1);
        assert_eq!(quality[0]["quality_score"], json!(100));
        assert_eq!(quality[0]["quality_status"], json!("review_ready"));
        assert_eq!(quality[0]["exact_duplicate_count"], json!(0));
        assert_eq!(quality[0]["related_family_count"], json!(0));
        assert!(quality[0].get("evidence_json").is_none());
        assert!(quality[0].get("risk_notes").is_none());
    }

    #[test]
    fn proposal_quality_rubric_surfaces_missing_evidence_and_related_family() {
        let quality = hermes_proposal_quality_from_experiments(&[
            json!({
                "id": "proposal-cash-buffer",
                "status": "pending_review",
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve outcomes.",
                "risk_notes": "May retain more cash.",
                "evidence_json": []
            }),
            json!({
                "id": "proposal-swing-buffer",
                "status": "active_paper",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"observations": ["operator reviewed"]}
            }),
        ]);

        let cash_buffer = quality
            .iter()
            .find(|row| row["id"] == json!("proposal-cash-buffer"))
            .expect("cash-buffer proposal quality");
        assert_eq!(cash_buffer["quality_status"], json!("needs_evidence"));
        assert_eq!(cash_buffer["related_family_count"], json!(1));
        assert!(
            cash_buffer["gaps"]
                .as_array()
                .expect("gaps array")
                .iter()
                .any(|gap| gap == "attach evidence")
        );
    }

    #[test]
    fn proposal_quality_rubric_requires_review_for_an_otherwise_ready_related_family() {
        let quality = hermes_proposal_quality_from_experiments(&[
            json!({
                "id": "proposal-capital-buffer",
                "status": "pending_review",
                "changed_variable_path": "strategy.capital.min_cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget by 1 pct while preserving drawdown.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"report_ids": [301], "metrics": {"cash": 0.02}}
            }),
            json!({
                "id": "proposal-swing-buffer",
                "status": "active_paper",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "old_value_json": 0.02,
                "new_value_json": 0.03,
                "expected_effect": "Improve cash budget by 1 pct while preserving drawdown.",
                "risk_notes": "May retain more cash.",
                "evidence_json": {"report_ids": [302], "metrics": {"cash": 0.02}}
            }),
        ]);

        let capital_buffer = quality
            .iter()
            .find(|row| row["id"] == json!("proposal-capital-buffer"))
            .expect("capital-buffer proposal quality");
        assert_eq!(capital_buffer["quality_score"], json!(95));
        assert_eq!(capital_buffer["quality_status"], json!("related_review"));
    }

    #[test]
    fn baseline_evidence_pack_links_only_matching_overlay_activity() {
        let pack = hermes_baseline_evidence_pack_from_snapshot(
            &json!({
                "id": "baseline-1",
                "activated_at": "2026-07-10T12:00:00Z",
                "config_json": {
                    "source_experiment_id": "experiment-1",
                    "changed_variable_path": "strategy.swing.markov_gate.min_signed_signal",
                    "raw_payload": "must not appear"
                }
            }),
            &json!({
                "id": "experiment-1",
                "created_at": "2026-07-01T12:00:00Z",
                "status": "promoted",
                "evidence_json": {"secret": "must not appear"}
            }),
            &[
                json!({
                    "id": 1,
                    "created_at": "2026-07-04T12:00:00Z",
                    "report_id": 41,
                    "manager_json": {
                        "strategy_experiment_overlay": {"id": "experiment-1"},
                        "approved_order_count": 2,
                        "skipped_order_count": 1
                    }
                }),
                json!({
                    "id": 2,
                    "created_at": "2026-07-05T12:00:00Z",
                    "report_id": 42,
                    "manager_json": {
                        "strategy_experiment_overlay": {"id": "different-experiment"},
                        "approved_order_count": 9,
                        "skipped_order_count": 9
                    }
                }),
            ],
            &[
                json!({"report_id": 41, "status": "executed"}),
                json!({"report_id": 41, "status": "execution_failed"}),
                json!({"report_id": 42, "status": "executed"}),
            ],
            &[
                json!({"recorded_at": "2026-07-01T12:00:00Z", "total_market_value_dkk": 100.0, "invested_market_value_dkk": 80.0, "cash_balance_dkk": 20.0}),
                json!({"recorded_at": "2026-07-04T12:00:00Z", "total_market_value_dkk": 110.0, "invested_market_value_dkk": 88.0, "cash_balance_dkk": 22.0}),
                json!({"recorded_at": "2026-07-08T12:00:00Z", "total_market_value_dkk": 105.0, "invested_market_value_dkk": 84.0, "cash_balance_dkk": 21.0}),
                json!({"recorded_at": "2026-07-10T12:00:00Z", "total_market_value_dkk": 120.0, "invested_market_value_dkk": 96.0, "cash_balance_dkk": 24.0}),
                json!({"recorded_at": "2026-07-11T12:00:00Z", "total_market_value_dkk": 126.0, "invested_market_value_dkk": 100.0, "cash_balance_dkk": 26.0}),
                json!({"recorded_at": "2026-07-12T12:00:00Z", "total_market_value_dkk": 132.0, "invested_market_value_dkk": 106.0, "cash_balance_dkk": 26.0}),
            ],
        );

        assert_eq!(pack["status"], json!("observing"));
        assert_eq!(pack["affected_activity"]["manager_run_count"], json!(1));
        assert_eq!(pack["affected_activity"]["report_count"], json!(1));
        assert_eq!(pack["affected_activity"]["approved_order_count"], json!(2));
        assert_eq!(pack["affected_activity"]["failed_order_count"], json!(1));
        assert!(
            (pack["experiment"]["evaluation_window"]["return_pct"]
                .as_f64()
                .expect("experiment return")
                - 20.0)
                .abs()
                < 1e-9
        );
        assert!(
            (pack["post_promotion"]["return_pct"]
                .as_f64()
                .expect("post-promotion return")
                - 100.0 / 21.0)
                .abs()
                < 1e-9
        );
        assert!(
            pack.to_string()
                .contains("read_only_observational_not_causal")
        );
        assert!(!pack.to_string().contains("must not appear"));
        assert!(pack.get("evidence_json").is_none());
    }

    #[test]
    fn baseline_evidence_pack_waits_for_a_source_experiment() {
        let pack = hermes_baseline_evidence_pack_from_snapshot(
            &json!({
                "id": "baseline-missing-source",
                "activated_at": "2026-07-10T12:00:00Z",
                "config_json": {"source_experiment_id": "missing"}
            }),
            &JsonValue::Null,
            &[],
            &[],
            &[],
        );

        assert_eq!(pack["status"], json!("source_experiment_unavailable"));
        assert_eq!(pack["safety"], json!("read_only_observational_not_causal"));
    }

    #[test]
    fn learning_memory_promotes_repeated_lessons_and_expires_old_ones() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = hermes_learning_memory_from_reflections(
            &[
                json!({
                    "id": "reflection-1",
                    "created_at": "2026-07-20T12:00:00Z",
                    "source_session_id": "daily-reflection-2026-07-20",
                    "proposed_actions_json": ["Keep Markov signals fresh before approving BUYs."]
                }),
                json!({
                    "id": "reflection-2",
                    "created_at": "2026-07-21T12:00:00Z",
                    "source_session_id": "weekly-reflection-2026-07-21",
                    "proposed_actions_json": ["Keep Markov signals fresh before approving BUYs."]
                }),
                json!({
                    "id": "reflection-old",
                    "created_at": "2026-06-01T12:00:00Z",
                    "source_session_id": "daily-reflection-2026-06-01",
                    "proposed_actions_json": ["Retire this old one-off lesson."]
                }),
            ],
            now,
            10,
        );

        let stable = memory
            .iter()
            .find(|row| row["lesson"] == json!("Keep Markov signals fresh before approving BUYs."))
            .expect("stable lesson");
        assert_eq!(stable["status"], json!("stable"));
        assert_eq!(stable["observation_count"], json!(2));
        assert_eq!(stable["cadences"], json!(["daily", "weekly"]));
        let stale = memory
            .iter()
            .find(|row| row["lesson"] == json!("Retire this old one-off lesson."))
            .expect("stale lesson");
        assert_eq!(stale["status"], json!("stale"));
    }

    #[test]
    fn learning_memory_redacts_sensitive_actions_before_grouping() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = hermes_learning_memory_from_reflections(
            &[json!({
                "id": "reflection-sensitive",
                "created_at": "2026-07-22T12:00:00Z",
                "source_session_id": "daily-reflection-2026-07-22",
                "proposed_actions_json": ["Use bearer token super-secret-value in the next report."]
            })],
            now,
            10,
        );

        assert_eq!(memory.len(), 1);
        assert_eq!(
            memory[0]["lesson"],
            json!("[redacted potentially sensitive reflection action]")
        );
        assert!(!memory[0].to_string().contains("super-secret-value"));
    }

    #[test]
    fn learning_memory_counts_duplicate_actions_once_per_reflection() {
        let now = DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
            .expect("valid current time")
            .with_timezone(&Utc);
        let memory = hermes_learning_memory_from_reflections(
            &[json!({
                "id": "reflection-duplicate-actions",
                "created_at": "2026-07-22T12:00:00Z",
                "source_session_id": "daily-reflection-2026-07-22",
                "proposed_actions_json": [
                    "Wait for fresh Markov signals.",
                    "Wait for fresh Markov signals."
                ]
            })],
            now,
            10,
        );

        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0]["status"], json!("emerging"));
        assert_eq!(memory[0]["observation_count"], json!(1));
    }

    #[test]
    fn configured_watchlist_universe_is_versioned_and_case_normalized() {
        let config: YamlValue = serde_yaml::from_str(
            r#"
market_data:
  watchlists:
    universe_symbols:
      - amd:XNAS
      - AMD:xnas
      - "  DSV:XCSE  "
      - ""
    extra_symbols:
      - symbol: SPCX:xnas
        isin: US84615Q1031
      - rklb:XNAS
      - symbol: ""
"#,
        )
        .expect("parse watchlist universe config");

        assert_eq!(
            configured_watchlist_universe_symbols(&config),
            vec!["AMD:xnas".to_string(), "DSV:xcse".to_string()]
        );
        assert_eq!(
            configured_extra_watch_symbols(&config),
            vec!["SPCX:xnas".to_string(), "RKLB:xnas".to_string()]
        );
    }

    async fn runtime_settings_test_state(config_yaml: &str) -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory runtime-settings test database");
        sqlx::query(
            "CREATE TABLE runtime_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create runtime-settings test table");
        AppState {
            config_path: std::path::PathBuf::from("runtime-settings-test.yaml"),
            config: serde_yaml::from_str(config_yaml).expect("parse runtime-settings test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    #[tokio::test]
    async fn ai_api_key_override_wins_over_config_and_is_never_echoed() {
        let state = runtime_settings_test_state(
            "xai:\n  provider: openrouter\n  api_key: sk-or-old-config-key-1234\n",
        )
        .await;

        // Before any override the config key is effective.
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-old-config-key-1234")
        );
        let status = state
            .ai_api_key_status_value()
            .await
            .expect("read initial key status");
        assert_eq!(status["source"], json!("config"));
        assert_eq!(status["configured"], json!(true));

        // Saving a rotated key makes it effective immediately, and the
        // returned status carries only a masked preview.
        let status = state
            .save_ai_api_key("  sk-or-v1-new-rotated-key-5678  ")
            .await
            .expect("save rotated key");
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-v1-new-rotated-key-5678")
        );
        assert_eq!(status["source"], json!("runtime"));
        let masked = status["masked"].as_str().expect("masked preview");
        assert_eq!(masked, "sk-or-…5678");
        assert!(!status.to_string().contains("sk-or-v1-new-rotated-key-5678"));

        // Clearing the override falls back to the config key.
        let status = state.save_ai_api_key("").await.expect("clear override");
        assert_eq!(status["source"], json!("config"));
        assert_eq!(
            state.effective_ai_api_key().await.as_deref(),
            Some("sk-or-old-config-key-1234")
        );
    }

    #[tokio::test]
    async fn purge_retired_runtime_settings_removes_only_the_legacy_cash_buffer() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        state
            .save_runtime_setting(
                "strategy.capital.cash_buffer",
                &json!({"min_cash_buffer_pct": 0.0, "max_deployment_pct": 1.0}),
            )
            .await
            .expect("seed retired cash-buffer override");
        state
            .save_runtime_setting("ai_model", &json!({"model": "openrouter/fusion"}))
            .await
            .expect("seed active model override");

        assert_eq!(
            state
                .purge_retired_runtime_settings()
                .await
                .expect("purge retired settings"),
            1
        );
        assert!(
            state
                .runtime_setting("strategy.capital.cash_buffer")
                .await
                .expect("read retired setting")
                .is_none()
        );
        assert_eq!(
            state
                .runtime_setting("ai_model")
                .await
                .expect("read active setting")
                .expect("active setting remains")["model"],
            json!("openrouter/fusion")
        );
        assert_eq!(
            state
                .purge_retired_runtime_settings()
                .await
                .expect("repeat purge is safe"),
            0
        );
    }

    #[tokio::test]
    async fn ai_api_key_rejects_non_printable_input_and_reports_missing() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;

        assert!(state.save_ai_api_key("bad key with spaces").await.is_err());
        assert!(state.save_ai_api_key("bad\nkey").await.is_err());

        let status = state
            .ai_api_key_status_value()
            .await
            .expect("read missing key status");
        assert_eq!(status["configured"], json!(false));
        assert_eq!(status["source"], json!("missing"));
        assert_eq!(status["masked"], JsonValue::Null);
        assert_eq!(state.effective_ai_api_key().await, None);
    }

    #[tokio::test]
    async fn manual_report_claim_is_exclusive_until_released() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;

        assert!(!state.manual_decision_report_in_flight().await);
        assert!(state.claim_manual_decision_report().await.expect("claim"));
        assert!(state.manual_decision_report_in_flight().await);
        // A second click while the pipeline runs must not start another.
        assert!(
            !state
                .claim_manual_decision_report()
                .await
                .expect("re-claim")
        );

        state
            .release_manual_decision_report_claim()
            .await
            .expect("release claim");
        assert!(!state.manual_decision_report_in_flight().await);
        assert!(
            state
                .claim_manual_decision_report()
                .await
                .expect("claim again")
        );
    }

    #[tokio::test]
    async fn stale_manual_report_claim_is_taken_over() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        // A claim from a crashed task, older than the stale window.
        state
            .save_runtime_setting(
                "manual_report_claim",
                &json!({"started_at": "2026-07-17T00:00:00Z"}),
            )
            .await
            .expect("seed stale claim");

        assert!(!state.manual_decision_report_in_flight().await);
        assert!(
            state
                .claim_manual_decision_report()
                .await
                .expect("take over")
        );
    }

    #[test]
    fn masked_api_key_never_reveals_short_keys() {
        assert_eq!(mask_api_key("sk-or-v1-abcdefgh-9999"), "sk-or-…9999");
        assert_eq!(mask_api_key("short-key"), "•••");
    }

    #[tokio::test]
    async fn ai_model_setting_accepts_openrouter_floating_alias() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        let saved = state
            .save_ai_settings("~openai/gpt-5.6-terra")
            .await
            .expect("save floating-alias model");
        assert_eq!(saved["model"], json!("~openai/gpt-5.6-terra"));
    }

    #[test]
    fn hermes_context_self_check_marks_complete_when_all_sources_seen() {
        let normalized = normalize_hermes_context_self_check(json!({
            "latest_report": true,
            "markov_signals": true,
            "end_of_day_report": true,
            "current_positions": true,
            "active_experiments": true,
            "sources": ["get_decision_reports", "get_markov_signals"]
        }));

        assert_eq!(
            normalized.get("complete").and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            normalized
                .get("missing")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn hermes_context_self_check_reports_missing_sources() {
        let normalized = normalize_hermes_context_self_check(json!({
            "latest_report": true,
            "markov_signals": false,
            "current_positions": true
        }));

        assert_eq!(
            normalized.get("complete").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            normalized.get("missing").cloned(),
            Some(json!([
                "markov_signals",
                "end_of_day_report",
                "active_experiments"
            ]))
        );
    }

    #[test]
    fn money_integrity_tolerance_requires_absolute_and_relative_drift() {
        assert!(!money_mismatch_exceeds_tolerance(
            100_000.0, 100_020.0, 50.0, 0.002
        ));
        assert!(!money_mismatch_exceeds_tolerance(
            100_000.0, 100_100.0, 50.0, 0.002
        ));
        assert!(money_mismatch_exceeds_tolerance(
            100_000.0, 100_500.0, 50.0, 0.002
        ));
        assert!(money_mismatch_exceeds_tolerance(
            f64::NAN,
            100_000.0,
            50.0,
            0.002
        ));
    }

    #[test]
    fn hermes_counterfactuals_only_shadow_advice_that_changed_quantity() {
        assert_eq!(
            hermes_counterfactual_shadow_quantity("reduced", 5.0, 2.0),
            Some(3.0)
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("blocked_by_order_advice", 5.0, 0.0),
            Some(5.0)
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("allowed", 5.0, 5.0),
            None
        );
        assert_eq!(
            hermes_counterfactual_shadow_quantity("reduced", 2.0, 2.0),
            None
        );
    }

    #[test]
    fn hermes_counterfactual_quote_metrics_are_directional() {
        assert_eq!(
            hermes_counterfactual_quote_metrics("BUY", 2.0, 100.0, 110.0),
            Some((0.1, 20.0))
        );
        assert_eq!(
            hermes_counterfactual_quote_metrics("SELL", 2.0, 100.0, 90.0),
            Some((0.1, 20.0))
        );
        assert_eq!(
            hermes_counterfactual_quote_metrics("HOLD", 2.0, 100.0, 90.0),
            None
        );
    }

    #[test]
    fn broker_cash_reconciliation_requires_explicit_opt_in() {
        let default_config: YamlValue =
            serde_yaml::from_str("portfolio: {}").expect("parse default portfolio config");
        let enabled_config: YamlValue =
            serde_yaml::from_str("portfolio:\n  broker_cash_reconciliation_enabled: true\n")
                .expect("parse enabled broker cash config");

        assert!(!broker_cash_reconciliation_enabled(&default_config));
        assert!(broker_cash_reconciliation_enabled(&enabled_config));
    }

    #[test]
    fn overview_integrity_acknowledgement_matches_stable_issue_key() {
        let mut mismatches = vec![json!({
            "code": "implausible_position_lot_cost_basis",
            "severity": "error",
            "message": "Bad lot",
            "lots": [{"lot_id": 42}, {"lot_id": 43}]
        })];
        let mut warnings = vec![json!({
            "code": "broker_cash_drift",
            "severity": "warning",
            "message": "Broker cash drift",
            "broker_currency": "DKK"
        })];
        let key = overview_integrity_issue_key(&mismatches[0]);
        assert_eq!(key, "implausible_position_lot_cost_basis:error:42_43");

        let acknowledged = annotate_overview_integrity_acknowledgements(
            &mut mismatches,
            &mut warnings,
            &json!({
                "acknowledgements": [{
                    "issue_key": key,
                    "enabled": true,
                    "notes": "accepted while import repair is pending",
                    "updated_at": "2026-07-10T06:00:00Z"
                }]
            }),
        );

        assert_eq!(acknowledged, 1);
        assert_eq!(
            mismatches[0]
                .get("acknowledged")
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            warnings[0].get("acknowledged").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            mismatches[0]
                .get("acknowledgement")
                .and_then(|value| value.get("notes"))
                .and_then(JsonValue::as_str),
            Some("accepted while import repair is pending")
        );
    }

    #[test]
    fn broker_exposure_quantity_mismatch_reports_symbol_drift() {
        let exposures = vec![
            json!({"symbol": "BAC:xnys", "quantity": 3.0, "updated_at": "2026-07-10T06:00:00Z"}),
            json!({"symbol": "AMD:xnas", "quantity": 1.0, "updated_at": "2026-07-10T06:00:00Z"}),
        ];
        let positions = HashMap::from([
            (
                "BAC:xnys".to_string(),
                json!({"symbol": "BAC:xnys", "quantity": 2.0, "updated_at": "2026-07-10T06:01:00Z"}),
            ),
            (
                "AMD:xnas".to_string(),
                json!({"symbol": "AMD:xnas", "quantity": 1.0, "updated_at": "2026-07-10T06:01:00Z"}),
            ),
        ]);

        let mismatches = broker_exposure_quantity_mismatches(&exposures, &positions);

        assert_eq!(mismatches.len(), 1);
        assert_eq!(json_text(&mismatches[0], "symbol"), "BAC:xnys");
        assert_eq!(value_f64(&mismatches[0], "difference"), 1.0);
    }

    #[test]
    fn broker_exposure_integrity_issue_key_includes_scope() {
        let key = overview_integrity_issue_key(&json!({
            "code": "broker_exposure_quantity_drift",
            "severity": "warning",
            "symbols": [{"symbol": "BAC:xnys"}, {"symbol": "AMD:xnas"}]
        }));

        assert_eq!(
            key,
            "broker_exposure_quantity_drift:warning:BAC:xnys_AMD:xnas"
        );
    }

    #[test]
    fn enriches_active_day_order_with_exchange_expiry() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": "2026-07-09T19:45:00Z",
            "session_close_at_utc": "2026-07-09T20:00:00Z"
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(json_text(&order, "order_duration_type"), "DayOrder");
        assert_eq!(
            json_text(&order, "expected_expiry_at_utc"),
            "2026-07-09T19:45:00Z"
        );
        assert_eq!(
            json_text(&order, "expected_expiry_market"),
            "New York Stock Exchange"
        );
    }

    #[test]
    fn marks_day_order_expiry_pending_after_expected_expiry_passes() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let expired_at = (Utc::now() - Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES + 1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": expired_at
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(
            json_text(&order, "lifecycle_state"),
            "expiry_pending_broker_sync"
        );
        assert!(json_text(&order, "lifecycle_note").contains("waiting for Saxo broker sync"));
    }

    #[test]
    fn does_not_mark_day_order_expiry_pending_inside_grace_window() {
        let mut order = json!({
            "symbol": "BAC:xnys",
            "status": "broker_working",
            "execution_result_json": {
                "payload": {
                    "OrderDuration": {"DurationType": "DayOrder"}
                }
            }
        });
        let expired_at = (Utc::now() - Duration::minutes(DAY_ORDER_EXPIRY_SYNC_GRACE_MINUTES - 1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let market_rows = vec![json!({
            "code": "xnys",
            "market": "New York Stock Exchange",
            "timezone": "America/New_York",
            "tradable_close_at_utc": expired_at
        })];

        enrich_execution_order_lifecycle(&mut order, &market_rows);

        assert_eq!(json_text(&order, "lifecycle_state"), "");
        assert!(json_text(&order, "lifecycle_note").contains("remains live"));
    }

    #[test]
    fn matching_order_advice_prefers_strategy_key_then_symbol_action() {
        let advice = json!([
            {
                "strategy_key": "pulse|AMD:xnas|BUY|primary",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "reason": "strategy-key match"
            },
            {
                "strategy_key": "other",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "reason": "symbol-action fallback"
            }
        ]);

        let matched = matching_order_advice(
            Some(&advice),
            "pulse|AMD:xnas|BUY|primary",
            "AMD:xnas",
            "BUY",
        )
        .unwrap();
        assert_eq!(json_text(&matched, "reason"), "strategy-key match");

        let fallback = matching_order_advice(Some(&advice), "", "AMD:XNAS", "buy").unwrap();
        assert_eq!(json_text(&fallback, "reason"), "strategy-key match");
    }

    #[test]
    fn execution_attribution_prefers_persisted_manager_snapshots() {
        let manager_run = json!({
            "manager_json": {
                "capital_budget": {
                    "cash_balance_dkk": 18_075.0,
                    "cash_pct": 0.064,
                    "required_cash_buffer_dkk": 5_658.0,
                    "available_buy_budget_dkk": 12_417.0,
                    "remaining_deployment_capacity_dkk": 18_000.0,
                    "reinvestment_pressure_active": true,
                    "unrelated_sensitive_value": "excluded"
                },
                "hermes_preflight": {
                    "candidate_waterfall": [{
                        "strategy_key": "open|AMD:xnas|BUY|primary",
                        "symbol": "AMD:xnas",
                        "action": "BUY",
                        "technical": {
                            "status": "ok",
                            "run_date": "2026-07-21",
                            "sentiment": "BUY",
                            "trend_bias": "bullish",
                            "confluence_count": 4,
                            "min_confluences": 3
                        },
                        "markov": {
                            "status": "ok",
                            "run_date": "2026-07-21",
                            "current_state": "Bull",
                            "direction": "long",
                            "signed_signal": 0.548,
                            "conviction": 0.78
                        }
                    }]
                }
            }
        });
        let final_technical = json!({
            "status": "ok",
            "run_date": "2026-07-21",
            "sentiment": "HOLD",
            "trend_bias": "neutral",
            "confluence_count": 1,
            "min_confluences": 3
        });

        let candidate = matching_manager_preflight_candidate(
            &manager_run,
            "open|AMD:xnas|BUY|primary",
            "AMD:xnas",
            "BUY",
        );
        let technical = compact_attribution_technical(&final_technical, "manager_final");
        let markov = compact_attribution_markov(
            candidate.get("markov").unwrap_or(&JsonValue::Null),
            "manager_preflight",
        );
        let capital = compact_attribution_capital(
            manager_run["manager_json"]
                .get("capital_budget")
                .expect("persisted capital budget"),
        );

        assert_eq!(json_text(&technical, "evidence_source"), "manager_final");
        assert_eq!(json_text(&technical, "sentiment"), "HOLD");
        assert_eq!(json_text(&markov, "evidence_source"), "manager_preflight");
        assert_eq!(json_text(&markov, "direction"), "long");
        assert_eq!(value_f64(&markov, "signed_signal"), 0.548);
        assert_eq!(json_text(&capital, "evidence_source"), "manager_run");
        assert_eq!(value_f64(&capital, "available_buy_budget_dkk"), 12_417.0);
        assert!(capital.get("unrelated_sensitive_value").is_none());
    }

    #[test]
    fn execution_ledger_attribution_aggregates_reconciled_sell_fills() {
        let order = json!({"action": "SELL", "quantity": 4.0});
        let summary = json!({
            "fill_count": 2,
            "ledger_entry_count": 2,
            "filled_quantity": 4.0,
            "last_fill_at": "2026-07-21T18:00:00Z",
            "commission_dkk": 21.0,
            "tax_dkk": 0.0,
            "realised_gain_dkk": 1_234.5,
            "cost_basis_sold_dkk": 6_500.0
        });

        let outcome = compact_execution_ledger_outcome(&order, &summary, "reconciled_fills");

        assert_eq!(json_text(&outcome, "status"), "reconciled");
        assert_eq!(json_text(&outcome, "side"), "SELL");
        assert_eq!(json_text(&outcome, "evidence_source"), "reconciled_fills");
        assert_eq!(value_i64(&outcome, "fill_count"), 2);
        assert_eq!(value_f64(&outcome, "realised_gain_dkk"), 1_234.5);
        assert_eq!(
            outcome.get("fully_filled").and_then(JsonValue::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn execution_ledger_outcome_reads_reconciled_fill_rows() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                id INTEGER PRIMARY KEY,
                execution_order_id INTEGER NOT NULL,
                ledger_id INTEGER,
                delta_quantity REAL NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-fill attribution table");
        sqlx::query(
            "CREATE TABLE trade_ledger (
                id INTEGER PRIMARY KEY,
                commission_dkk REAL NOT NULL,
                tax_dkk REAL NOT NULL,
                realised_gain_dkk REAL NOT NULL,
                cost_basis_sold_dkk REAL NOT NULL,
                quantity REAL NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create ledger attribution table");
        sqlx::query(
            "INSERT INTO trade_ledger (
                id, commission_dkk, tax_dkk, realised_gain_dkk,
                cost_basis_sold_dkk, quantity, created_at
            ) VALUES
                (10, 4.0, 0.0, 300.0, 1200.0, 1.0, '2026-07-21T17:00:00Z'),
                (11, 5.0, 0.0, 500.0, 1800.0, 2.0, '2026-07-21T18:00:00Z')",
        )
        .execute(&state.pool)
        .await
        .expect("seed ledger attribution rows");
        sqlx::query(
            "INSERT INTO execution_fills (
                id, execution_order_id, ledger_id, delta_quantity, created_at
            ) VALUES
                (1, 42, 10, 1.0, '2026-07-21T17:00:00Z'),
                (2, 42, 11, 2.0, '2026-07-21T18:00:00Z')",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-fill attribution rows");

        let outcome = state
            .execution_order_ledger_outcome(&json!({
                "id": 42,
                "ledger_id": 11,
                "status": "executed",
                "action": "SELL",
                "quantity": 3.0
            }))
            .await
            .expect("read reconciled ledger outcome");

        assert_eq!(json_text(&outcome, "evidence_source"), "reconciled_fills");
        assert_eq!(value_i64(&outcome, "fill_count"), 2);
        assert_eq!(value_f64(&outcome, "filled_quantity"), 3.0);
        assert_eq!(value_f64(&outcome, "commission_dkk"), 9.0);
        assert_eq!(value_f64(&outcome, "realised_gain_dkk"), 800.0);
    }

    #[test]
    fn attribution_delta_label_describes_hermes_manager_difference() {
        assert_eq!(
            attribution_delta_label(
                &json!({"action": "allow"}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "executed"})
            ),
            "allowed_executed"
        );
        assert_eq!(
            attribution_delta_label(
                &json!({"action": "review"}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "queued"})
            ),
            "manager_overrode_review"
        );
        assert_eq!(
            attribution_delta_label(
                &json!({}),
                &json!({"manager_decision": "approved"}),
                &json!({"status": "queued"})
            ),
            "manager_only"
        );
    }

    #[test]
    fn saxo_session_score_prefers_refreshable_session_over_invalid_recent_session() {
        let old_refreshable = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "refresh_token_expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339(),
            "last_refreshed_at": (Utc::now() - Duration::minutes(30)).to_rfc3339(),
        });
        let recently_invalid = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "refresh_token_invalid_at": Utc::now().to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339(),
            "last_refreshed_at": Utc::now().to_rfc3339(),
        });

        assert!(saxo_session_score(&old_refreshable) > saxo_session_score(&recently_invalid));
    }

    #[test]
    fn saxo_session_needs_refresh_only_for_usable_near_expiry_tokens() {
        let valid = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::hours(2)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
        });
        let near_expiry = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
        });
        let invalid_refresh = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "access_token_expires_at": (Utc::now() + Duration::minutes(5)).to_rfc3339(),
            "refresh_token_expires_at": (Utc::now() + Duration::hours(4)).to_rfc3339(),
            "refresh_token_invalid_at": Utc::now().to_rfc3339(),
        });

        assert!(!saxo_session_needs_refresh(&valid));
        assert!(saxo_session_needs_refresh(&near_expiry));
        assert!(!saxo_session_needs_refresh(&invalid_refresh));
    }

    #[test]
    fn configured_holiday_fallback_closes_copenhagen_and_oslo_on_whit_monday_2026() {
        let config = YamlValue::Null;
        let now = DateTime::parse_from_rfc3339("2026-05-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = market_exchange_rows_for_config(&config, now, None);
        let copenhagen = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XCSE"))
            .unwrap();
        let oslo = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XOSL"))
            .unwrap();

        assert_eq!(
            copenhagen.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            oslo.get("status_reason").and_then(JsonValue::as_str),
            Some("Closed - Whit Monday")
        );
        assert_eq!(
            copenhagen.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            oslo.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            copenhagen
                .get("next_open_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        assert_eq!(
            oslo.get("next_open_at_utc").and_then(JsonValue::as_str),
            Some("2026-05-26T07:00:00Z")
        );
        for (code, reason) in [
            ("XLON", "Closed - Spring bank holiday"),
            ("XNAS", "Closed - Memorial Day"),
            ("XNYS", "Closed - Memorial Day"),
        ] {
            let row = rows
                .iter()
                .find(|row| row.get("code").and_then(JsonValue::as_str) == Some(code))
                .unwrap();
            assert_eq!(
                row.get("status_reason").and_then(JsonValue::as_str),
                Some(reason)
            );
            assert_eq!(
                row.get("is_tradable").and_then(JsonValue::as_bool),
                Some(false)
            );
        }
    }

    #[test]
    fn saxo_opening_auction_does_not_anchor_decision_window() {
        let config: YamlValue = serde_yaml::from_str(
            r#"
analysis_windows:
  offset_minutes_after_open: 75
  pre_sync_minutes_before_analysis: 5
  end_buffer_minutes_before_close: 15
"#,
        )
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-06-18T06:40:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut exchanges = HashMap::new();
        exchanges.insert(
            "XCSE".to_string(),
            SaxoExchangeCalendar {
                exchange_id: "XCSE".to_string(),
                name: Some("Copenhagen".to_string()),
                timezone_id: Some("Europe/Copenhagen".to_string()),
                sessions: vec![
                    SaxoExchangeSession {
                        start_at: DateTime::parse_from_rfc3339("2026-06-18T05:30:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        end_at: DateTime::parse_from_rfc3339("2026-06-18T07:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        state: "OpeningAuction".to_string(),
                    },
                    SaxoExchangeSession {
                        start_at: DateTime::parse_from_rfc3339("2026-06-18T07:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        end_at: DateTime::parse_from_rfc3339("2026-06-18T15:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        state: "AutomatedTrading".to_string(),
                    },
                ],
            },
        );
        let cache = SaxoExchangeCalendarCache {
            checked_date: now.date_naive(),
            checked_at: now,
            exchanges,
            source: "test".to_string(),
        };

        let rows = market_exchange_rows_for_config(&config, now, Some(&cache));
        let copenhagen = rows
            .iter()
            .find(|row| row.get("code").and_then(JsonValue::as_str) == Some("XCSE"))
            .unwrap();

        assert_eq!(
            copenhagen
                .get("session_open_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-06-18T07:00:00Z")
        );
        assert_eq!(
            copenhagen
                .get("open_analysis_window_start_at_utc")
                .and_then(JsonValue::as_str),
            Some("2026-06-18T08:15:00Z")
        );
        assert_eq!(
            copenhagen
                .get("session_open_local")
                .and_then(JsonValue::as_str),
            Some("2026-06-18 09:00")
        );
        assert_eq!(
            copenhagen
                .get("open_analysis_window_active")
                .and_then(JsonValue::as_bool),
            Some(false)
        );
        assert_eq!(
            copenhagen.get("is_tradable").and_then(JsonValue::as_bool),
            Some(false)
        );
    }

    #[test]
    fn validates_hermes_experiment_lifecycle_transitions() {
        assert_eq!(
            hermes_experiment_next_status("pending_review", "approve_paper"),
            Some("approved_paper")
        );
        assert_eq!(
            hermes_experiment_next_status("active_sim", "ready_for_promotion"),
            Some("ready_for_promotion")
        );
        assert_eq!(
            hermes_experiment_next_status("ready_for_promotion", "promote"),
            Some("promoted")
        );
        assert_eq!(
            hermes_experiment_next_status("pending_review", "promote"),
            None
        );
        assert_eq!(
            hermes_experiment_next_status("pending_review", "expire_stale"),
            Some("expired_stale")
        );
    }

    #[test]
    fn hermes_experiment_duplicate_statuses_are_active_or_pending_only() {
        for status in [
            "pending_review",
            "approved_paper",
            "active_paper",
            "approved_sim",
            "active_sim",
            "ready_for_promotion",
        ] {
            assert!(hermes_experiment_status_blocks_duplicate(status));
        }

        for status in [
            "rejected",
            "paper_failed",
            "sim_failed",
            "failed",
            "expired_stale",
            "promoted",
            "",
        ] {
            assert!(!hermes_experiment_status_blocks_duplicate(status));
        }
    }

    #[tokio::test]
    async fn expires_only_stale_pending_hermes_experiments() {
        let state = runtime_settings_test_state("hermes:\n  experiments: {}\n").await;
        sqlx::query(
            "CREATE TABLE strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                approval_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create strategy experiments table");
        for (id, created_at, status) in [
            ("old-pending", "2026-06-01T12:00:00Z", "pending_review"),
            ("fresh-pending", "2026-06-25T12:00:00Z", "pending_review"),
            ("old-approved", "2026-06-01T12:00:00Z", "approved_paper"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO strategy_experiments (id, created_at, status, changed_variable_path)
                 VALUES ('{}', '{}', '{}', 'strategy.swing.daily_indicators.min_confluences')",
                sql_escape(id),
                sql_escape(created_at),
                sql_escape(status),
            ))
            .execute(&state.pool)
            .await
            .expect("seed experiment");
        }

        let now = DateTime::parse_from_rfc3339("2026-07-01T12:00:00Z")
            .expect("valid now")
            .with_timezone(&Utc);
        let result = state
            .expire_stale_hermes_experiments_at(now, 30)
            .await
            .expect("expire stale pending proposal");
        assert_eq!(result["expired_count"], json!(1));
        assert_eq!(result["expired"][0]["id"], json!("old-pending"));

        let rows = state
            .select_json(
                "SELECT id, status, approval_json
                 FROM strategy_experiments ORDER BY id ASC",
            )
            .await
            .expect("read experiment statuses");
        let old_pending = rows
            .iter()
            .find(|row| json_text(row, "id") == "old-pending")
            .expect("old pending row");
        assert_eq!(old_pending["status"], json!("expired_stale"));
        assert_eq!(old_pending["approval_json"]["actor"], json!("scheduler"));
        assert_eq!(
            old_pending["approval_json"]["action"],
            json!("expire_stale")
        );
        assert_eq!(
            rows.iter()
                .find(|row| json_text(row, "id") == "fresh-pending")
                .expect("fresh pending row")["status"],
            json!("pending_review")
        );
        assert_eq!(
            rows.iter()
                .find(|row| json_text(row, "id") == "old-approved")
                .expect("old approved row")["status"],
            json!("approved_paper")
        );
        assert_eq!(
            state
                .expire_stale_hermes_experiments_at(now, 30)
                .await
                .expect("repeated expiry is safe")["expired_count"],
            json!(0)
        );
    }

    #[test]
    fn normalizes_hermes_experiment_variable_paths_for_duplicate_lookup() {
        assert_eq!(
            normalize_hermes_experiment_variable_path(
                " Strategy.Swing.Daily_Indicators.Min_Confluences "
            ),
            "strategy.swing.daily_indicators.min_confluences"
        );
    }

    #[test]
    fn maps_only_explicit_hermes_experiment_review_families() {
        assert_eq!(
            hermes_experiment_review_family("strategy.capital.min_cash_buffer_pct"),
            Some("cash_buffer_policy")
        );
        assert_eq!(
            hermes_experiment_review_family(" Strategy.Swing.Cash_Buffer_Pct "),
            Some("cash_buffer_policy")
        );
        assert_eq!(
            hermes_experiment_review_family("strategy.capital.max_positions"),
            None
        );
    }

    #[tokio::test]
    async fn hermes_experiment_preinsert_review_distinguishes_exact_and_related_proposals() {
        let state = runtime_settings_test_state("hermes:\n  experiments: {}\n").await;
        sqlx::query(
            "CREATE TABLE strategy_experiments (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_variable_path TEXT NOT NULL,
                hypothesis TEXT NOT NULL,
                source_session_id TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create strategy experiments table");
        for (id, status, path) in [
            (
                "exact-pending",
                "pending_review",
                "Strategy.Capital.Min_Cash_Buffer_Pct",
            ),
            (
                "related-active",
                "active_sim",
                "strategy.swing.cash_buffer_pct",
            ),
            (
                "related-terminal",
                "expired_stale",
                "strategy.swing.cash_buffer_pct",
            ),
            (
                "unrelated-pending",
                "pending_review",
                "strategy.swing.daily_indicators.min_confluences",
            ),
        ] {
            sqlx::query(&format!(
                "INSERT INTO strategy_experiments (id, created_at, status, changed_variable_path, hypothesis)
                 VALUES ('{}', '2026-07-22T09:00:00Z', '{}', '{}', 'test')",
                sql_escape(id),
                sql_escape(status),
                sql_escape(path),
            ))
            .execute(&state.pool)
            .await
            .expect("seed experiment proposal");
        }

        let review = state
            .inspect_hermes_experiment_proposal("strategy.capital.min_cash_buffer_pct")
            .await
            .expect("inspect proposal");

        assert_eq!(review["exact_duplicate"]["id"], json!("exact-pending"));
        assert_eq!(review["review_family"], json!("cash_buffer_policy"));
        assert_eq!(
            review["related_active_or_pending_experiments"],
            json!([{
                "id": "related-active",
                "created_at": "2026-07-22T09:00:00Z",
                "status": "active_sim",
                "changed_variable_path": "strategy.swing.cash_buffer_pct",
                "hypothesis": "test",
                "source_session_id": null,
            }])
        );
        assert_eq!(review["related_family_is_advisory"], json!(true));
    }

    #[test]
    fn candidate_scoring_waterfall_sanitizes_legacy_manager_reasons() {
        let run = json!({
            "id": 42,
            "created_at": "2026-07-13T12:00:00Z",
            "status": "completed",
            "manager_json": {
                "hermes_preflight": {
                    "candidate_waterfall": [{
                        "strategy_key": "swing:NVDA:xnas:BUY",
                        "symbol": "NVDA:xnas",
                        "action": "BUY",
                        "order_type": "Limit",
                        "quantity": 2,
                        "exchange": "XNAS",
                        "exchange_open": true,
                        "risk_excluded": false,
                        "instrument_quarantine": null,
                        "technical": {
                            "status": "ok",
                            "sentiment": "bullish",
                            "trend_bias": "up",
                            "confluence_count": 3,
                            "min_confluences": 3
                        },
                        "markov": {
                            "status": "ok",
                            "fresh": true,
                            "direction": "long",
                            "signed_signal": 0.34,
                            "age_days": 0
                        }
                    }]
                },
                "hermes_advice_delta": {
                    "candidates": [{
                        "strategy_key": "swing:NVDA:xnas:BUY",
                        "effect": "reduced",
                        "requested_quantity": 2,
                        "resulting_quantity": 1,
                        "raw_rationale": "do not render this"
                    }]
                },
                "skipped_orders": [{
                    "strategy_key": "swing:NVDA:xnas:BUY",
                    "symbol": "NVDA:xnas",
                    "action": "BUY",
                    "final_technical": {
                        "status": "ok",
                        "source": "daily_indicators_db",
                        "run_date": "2026-07-13",
                        "sentiment": "HOLD",
                        "trend_bias": "neutral",
                        "confluence_count": 1,
                        "min_confluences": 3
                    },
                    "technical_gate": "Hermes advisory rejected because do not render this"
                }]
            }
        });

        let waterfall = candidate_scoring_waterfall_from_manager_run(&run);
        assert_eq!(waterfall["status"], "available");
        assert_eq!(waterfall["summary"]["skipped_count"], 1);
        assert_eq!(waterfall["candidates"][0]["symbol"], "NVDA:xnas");
        assert_eq!(waterfall["candidates"][0]["gate_code"], "hermes_advice");
        assert_eq!(waterfall["candidates"][0]["hermes"]["effect"], "reduced");
        assert_eq!(
            waterfall["candidates"][0]["final_technical"]["sentiment"],
            "HOLD"
        );
        assert_eq!(
            waterfall["candidates"][0]["final_technical"]["source"],
            "daily_indicators_db"
        );
        assert!(!waterfall.to_string().contains("do not render this"));
        assert!(!waterfall.to_string().contains("technical_gate"));
    }

    #[test]
    fn gate_replay_isolates_threshold_flips_without_claiming_full_approval() {
        let run = json!({
            "id": 77,
            "report_id": 91,
            "created_at": "2026-07-22T12:00:00Z",
            "status": "completed",
            "manager_json": {
                "hermes_preflight": {
                    "markov": {"min_signed_signal": 0.15},
                    "candidate_waterfall": [
                        {
                            "strategy_key": "markov-starter",
                            "symbol": "MARKOV:xnas",
                            "action": "BUY",
                            "technical": {"status": "ok", "sentiment": "HOLD", "trend_bias": "neutral", "confluence_count": 1, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "long", "signed_signal": 0.20, "age_days": 0}
                        },
                        {
                            "strategy_key": "technical-buy",
                            "symbol": "TECH:xnas",
                            "action": "BUY",
                            "technical": {"status": "ok", "sentiment": "BUY", "trend_bias": "bullish", "confluence_count": 3, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "long", "signed_signal": 0.40, "age_days": 0}
                        },
                        {
                            "strategy_key": "sell",
                            "symbol": "SELL:xnas",
                            "action": "SELL",
                            "technical": {"status": "ok", "sentiment": "SELL", "trend_bias": "bearish", "confluence_count": 0, "min_confluences": 3},
                            "markov": {"status": "ok", "fresh": true, "direction": "short", "signed_signal": -0.40, "age_days": 0}
                        }
                    ]
                },
                "approved_orders": [
                    {"strategy_key": "markov-starter", "symbol": "MARKOV:xnas", "action": "BUY", "gate_code": "approved"},
                    {"strategy_key": "technical-buy", "symbol": "TECH:xnas", "action": "BUY", "gate_code": "approved"},
                    {"strategy_key": "sell", "symbol": "SELL:xnas", "action": "SELL", "gate_code": "approved"}
                ]
            }
        });

        let replay = gate_replay_from_manager_runs(&[run]);
        assert_eq!(replay["status"], "available");
        assert_eq!(replay["run_count"], 1);
        assert_eq!(
            replay["scenarios"][0]["variable_path"],
            "strategy.swing.markov_gate.min_signed_signal"
        );
        assert_eq!(
            replay["scenarios"][0]["summary"]["would_block_target_gate_count"],
            1
        );
        assert_eq!(
            replay["scenarios"][0]["changes"][0]["symbol"],
            "MARKOV:xnas"
        );
        assert_eq!(
            replay["scenarios"][0]["changes"][0]["effect"],
            "would_block_target_gate"
        );
        assert_eq!(
            replay["scenarios"][1]["variable_path"],
            "strategy.swing.daily_indicators.min_confluences"
        );
        assert_eq!(
            replay["scenarios"][1]["summary"]["would_block_target_gate_count"],
            1
        );
        assert_eq!(replay["scenarios"][1]["changes"][0]["symbol"], "TECH:xnas");
        assert!(
            replay["interpretation"]
                .as_str()
                .unwrap_or_default()
                .contains("not an approval")
        );
    }

    #[test]
    fn support_risk_evidence_uses_next_available_closes_without_claiming_causality() {
        let rows = vec![
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-01", "close": 100.0, "support_break_risk_label": "low", "support_break_risk": 0.2, "support_confidence": 0.8, "support_history_coverage": 1.0}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-02", "close": 105.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-03", "close": 106.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-04", "close": 107.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-05", "close": 108.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "LOW:xnas", "run_date": "2026-07-06", "close": 110.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-01", "close": 100.0, "support_break_risk_label": "high", "support_break_risk": 0.8, "support_confidence": 0.6, "support_history_coverage": 0.5}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-02", "close": 92.0, "support_break_risk_label": "unavailable"}),
            json!({"symbol": "HIGH:xnas", "run_date": "2026-07-02", "close": 90.0, "support_break_risk_label": "unavailable"}),
        ];

        let evidence = support_risk_evidence_from_indicator_rows(&rows);
        assert_eq!(evidence["status"], "collecting");
        assert_eq!(evidence["eligible_signal_count"], 2);
        assert_eq!(evidence["next_run_complete_count"], 2);
        assert_eq!(evidence["five_run_complete_count"], 1);
        assert_eq!(evidence["labels"][0]["label"], "low");
        assert_eq!(evidence["labels"][0]["next_run"]["average_return_pct"], 5.0);
        assert_eq!(
            evidence["labels"][0]["five_run"]["average_return_pct"],
            10.0
        );
        assert_eq!(evidence["labels"][2]["label"], "high");
        assert_eq!(
            evidence["labels"][2]["next_run"]["average_return_pct"],
            -10.0
        );
        assert!(
            evidence["interpretation"]
                .as_str()
                .unwrap_or_default()
                .contains("descriptive, not causal")
        );
    }

    #[test]
    fn candidate_waterfall_recovers_only_known_legacy_sell_gate_evidence() {
        let technical = compact_candidate_final_technical(&json!({
            "technical_gate": "SELL not approved; technical sentiment is HOLD with neutral trend. (database-verified daily indicators)"
        }));
        assert_eq!(technical["status"], "ok");
        assert_eq!(technical["source"], "recorded_gate_reason");
        assert_eq!(technical["sentiment"], "HOLD");
        assert_eq!(technical["trend_bias"], "neutral");

        let untrusted = compact_candidate_final_technical(&json!({
            "technical_gate": "Hermes advisory rejected because do not render this"
        }));
        assert!(untrusted.is_null());
    }

    #[test]
    fn database_display_label_excludes_credentials_and_query_parameters() {
        let label = redacted_database_url(
            "postgresql://daytrader:super-secret@daytrader-postgres-rw.saxo.svc.cluster.local:5432/daytrader?sslmode=require&token=also-secret",
        );
        assert_eq!(
            label,
            "PostgreSQL · daytrader-postgres-rw.saxo.svc.cluster.local:5432/daytrader"
        );
        assert!(!label.contains("daytrader:"));
        assert!(!label.contains("super-secret"));
        assert!(!label.contains("also-secret"));
        assert_eq!(
            redacted_database_url("sqlite:///Users/example/private/ledger.db"),
            "SQLite · local database"
        );
    }

    #[test]
    fn decision_report_summary_projection_excludes_heavy_payload_columns() {
        for column in [
            "prompt_text",
            "request_json",
            "response_json",
            "report_json",
        ] {
            assert!(
                !DECISION_REPORT_SUMMARY_COLUMNS.contains(column),
                "summary projection must not load {column}"
            );
            assert!(
                DECISION_REPORT_DETAIL_COLUMNS.contains(column),
                "detail projection must retain {column}"
            );
        }
    }

    #[test]
    fn dashboard_loads_performance_history_only_for_performance_view() {
        assert_eq!(
            dashboard_performance_history_limit("performance", "1W"),
            Some(600)
        );
        for view in ["overview", "decisions", "prompts", "execution", "hermes"] {
            assert_eq!(
                dashboard_performance_history_limit(view, "1M"),
                None,
                "{view} must not load performance history"
            );
        }
    }

    #[test]
    fn dashboard_loads_tab_exclusive_collections_only_for_their_tab() {
        for tab in [
            "decisions",
            "eod",
            "execution",
            "hermes",
            "markov",
            "quiver",
            "watchlists",
        ] {
            assert!(dashboard_loads_tab_exclusive_data(tab, tab));
            assert!(!dashboard_loads_tab_exclusive_data("overview", tab));
            assert!(!dashboard_loads_tab_exclusive_data("performance", tab));
        }
    }

    #[test]
    fn dashboard_execution_order_window_pages_execution_and_bounds_other_tabs() {
        assert_eq!(
            dashboard_execution_order_window("execution", 2, 56),
            (2, EXECUTION_ORDERS_PAGE_SIZE, 25)
        );
        assert_eq!(
            dashboard_execution_order_window("execution", 99, 26),
            (2, EXECUTION_ORDERS_PAGE_SIZE, 25)
        );
        assert_eq!(
            dashboard_execution_order_window("overview", 5, 500),
            (1, OVERVIEW_EXECUTION_ORDERS_LIMIT, 0)
        );
        assert_eq!(
            dashboard_execution_order_window("markov", 5, 500),
            (1, SHARED_EXECUTION_ORDERS_LIMIT, 0)
        );
    }

    #[test]
    fn dashboard_markov_signal_window_clamps_page_and_calculates_offset() {
        assert_eq!(
            dashboard_markov_signal_window(2, 81),
            (2, MARKOV_SIGNALS_PAGE_SIZE)
        );
        assert_eq!(
            dashboard_markov_signal_window(9, 41),
            (2, MARKOV_SIGNALS_PAGE_SIZE)
        );
        assert_eq!(dashboard_markov_signal_window(0, 0), (1, 0));
    }

    #[test]
    fn dashboard_quiver_signal_window_clamps_page_and_calculates_offset() {
        assert_eq!(
            dashboard_quiver_signal_window(2, 81),
            (2, QUIVER_SIGNALS_PAGE_SIZE)
        );
        assert_eq!(
            dashboard_quiver_signal_window(9, 41),
            (2, QUIVER_SIGNALS_PAGE_SIZE)
        );
        assert_eq!(dashboard_quiver_signal_window(0, 0), (1, 0));
    }

    #[test]
    fn dashboard_scheduler_cycle_window_clamps_page_and_calculates_offset() {
        assert_eq!(
            dashboard_scheduler_cycle_window(2, 25),
            (2, SCHEDULER_CYCLES_PAGE_SIZE)
        );
        assert_eq!(
            dashboard_scheduler_cycle_window(9, 13),
            (2, SCHEDULER_CYCLES_PAGE_SIZE)
        );
        assert_eq!(dashboard_scheduler_cycle_window(0, 0), (1, 0));
    }

    #[test]
    fn scheduler_history_policy_uses_defaults_and_disables_negative_values() {
        assert_eq!(
            scheduler_history_policy_values(None, None),
            (
                DEFAULT_SCHEDULER_HISTORY_MAX_ROWS,
                DEFAULT_SCHEDULER_HISTORY_RETENTION_DAYS
            )
        );
        assert_eq!(scheduler_history_policy_values(Some(-1), Some(-2)), (0, 0));
        assert_eq!(
            scheduler_history_policy_values(Some(500), Some(14)),
            (500, 14)
        );
    }
}
