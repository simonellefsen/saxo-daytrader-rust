use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    config::{database_url, yaml_at, yaml_bool, yaml_f64, yaml_i64, yaml_string},
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
const RETIRED_RUNTIME_SETTING_KEYS: &[&str] = &[
    "strategy.capital.cash_buffer",
    "strategy.swing.cash_buffer_pct",
];
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
const TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT: i64 = 50;
const TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS: usize = 20;
const HOLDING_THESIS_REVIEW_LIMIT: i64 = 50;
const DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT: i64 = 50;
const MISSED_TRADE_SHADOW_LIMIT: i64 = 50;
const MISSED_TRADE_SHADOW_EVIDENCE_LIMIT: i64 = 200;
const MISSED_TRADE_SHADOW_MIN_COMPLETE_OBSERVATIONS: usize = 20;
const PROTECTIVE_STOP_HERMES_POSITION_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShareIncomeTaxBracket {
    up_to_dkk: Option<f64>,
    rate: f64,
}

fn share_income_tax_brackets(config: &YamlValue) -> Option<Vec<ShareIncomeTaxBracket>> {
    let values = yaml_at(config, &["taxation", "share_income", "brackets"])?.as_sequence()?;
    if values.is_empty() {
        return None;
    }

    let mut brackets = Vec::with_capacity(values.len());
    let mut lower_bound = 0.0;
    for (index, value) in values.iter().enumerate() {
        let rate = value.get("rate").and_then(YamlValue::as_f64)?;
        if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
            return None;
        }
        let up_to_dkk = value.get("up_to_dkk").and_then(YamlValue::as_f64);
        match up_to_dkk {
            Some(upper_bound) if upper_bound.is_finite() && upper_bound > lower_bound => {
                lower_bound = upper_bound;
            }
            Some(_) => return None,
            None if index + 1 == values.len() => {}
            None => return None,
        }
        brackets.push(ShareIncomeTaxBracket { up_to_dkk, rate });
    }
    Some(brackets)
}

fn share_income_tax_due_dkk(income_dkk: f64, brackets: &[ShareIncomeTaxBracket]) -> Option<f64> {
    if !income_dkk.is_finite() || brackets.is_empty() {
        return None;
    }
    let taxable_income = income_dkk.max(0.0);
    let mut lower_bound = 0.0;
    let mut tax_dkk = 0.0;
    for bracket in brackets {
        let taxable_slice = match bracket.up_to_dkk {
            Some(upper_bound) => (taxable_income.min(upper_bound) - lower_bound).max(0.0),
            None => (taxable_income - lower_bound).max(0.0),
        };
        tax_dkk += taxable_slice * bracket.rate;
        if bracket.up_to_dkk.is_none() || taxable_income <= bracket.up_to_dkk.unwrap_or_default() {
            return Some(tax_dkk);
        }
        lower_bound = bracket.up_to_dkk.unwrap_or(lower_bound);
    }
    None
}

fn incremental_share_income_tax_dkk(
    realised_gain_ytd_dkk: f64,
    unrealised_pnl_dkk: f64,
    brackets: &[ShareIncomeTaxBracket],
) -> Option<f64> {
    if !realised_gain_ytd_dkk.is_finite() || !unrealised_pnl_dkk.is_finite() {
        return None;
    }
    Some(
        share_income_tax_due_dkk(realised_gain_ytd_dkk + unrealised_pnl_dkk, brackets)?
            - share_income_tax_due_dkk(realised_gain_ytd_dkk, brackets)?,
    )
}

fn unavailable_after_tax_summary(
    gross_unrealised_pnl_dkk: f64,
    tax_year: i32,
    reason: &str,
) -> JsonValue {
    json!({
        "status": "unavailable",
        "tax_year": tax_year,
        "gross_unrealised_pnl_dkk": gross_unrealised_pnl_dkk,
        "estimated_tax_dkk": 0.0,
        "unrealised_pnl_after_tax_dkk": gross_unrealised_pnl_dkk,
        "reason": reason
    })
}

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

/// Gates whose blocks are useful to observe as an unfilled, quote-to-quote
/// shadow. This intentionally excludes technical, Markov, risk exclusions,
/// and instrument quarantine: those are validity failures, not candidates the
/// runtime elected not to deploy because of capital, timing, or capacity.
fn missed_trade_shadow_gate_is_eligible(gate_code: &str) -> bool {
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

fn candidate_gate_code(value: &JsonValue) -> String {
    let configured = json_text(value, "gate_code");
    if matches!(
        configured.as_str(),
        "approved"
            | "hermes_context"
            | "hermes_advice"
            | "candidate_limit"
            | "market_open"
            | "risk_exclusion"
            | "instrument_quarantine"
            | "quantity"
            | "order_shape"
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

fn compact_candidate_cost_guard(value: &JsonValue) -> JsonValue {
    let guard = value.get("final_cost_guard").unwrap_or(&JsonValue::Null);
    if !guard.is_object() {
        return JsonValue::Null;
    }
    json!({
        "verified_from_db": guard.get("verified_from_db").and_then(JsonValue::as_bool).unwrap_or(false),
        "estimated_slippage_bps": value_f64(guard, "estimated_slippage_bps"),
        "cost_guard_multiple": value_f64(guard, "cost_guard_multiple"),
        "expected_reward_dkk": value_f64(guard, "expected_reward_dkk"),
        "round_trip_commission_dkk": value_f64(guard, "round_trip_commission_dkk"),
        "one_way_slippage_dkk": value_f64(guard, "one_way_slippage_dkk"),
        "required_reward_dkk": value_f64(guard, "required_reward_dkk"),
        "passes": guard.get("passes").and_then(JsonValue::as_bool).unwrap_or(false),
        "basis": json_text(guard, "basis"),
    })
}

fn compact_candidate_concentration(value: &JsonValue) -> JsonValue {
    let concentration = value.get("final_concentration").unwrap_or(&JsonValue::Null);
    if !concentration.is_object() {
        return JsonValue::Null;
    }
    json!({
        "status": json_text(concentration, "status"),
        "verified_from_state": concentration.get("verified_from_state").and_then(JsonValue::as_bool).unwrap_or(false),
        "max_assets_per_exchange": value_i64(concentration, "max_assets_per_exchange"),
        "max_assets_per_currency": value_i64(concentration, "max_assets_per_currency"),
        "exchange": json_text(concentration, "exchange"),
        "currency": json_text(concentration, "currency"),
        "exchange_count_before": value_i64(concentration, "exchange_count_before"),
        "currency_count_before": value_i64(concentration, "currency_count_before"),
        "already_held": concentration.get("already_held").and_then(JsonValue::as_bool).unwrap_or(false),
        "unmapped_exchange_symbol_count": value_i64(concentration, "unmapped_exchange_symbol_count"),
        "unmapped_currency_symbol_count": value_i64(concentration, "unmapped_currency_symbol_count"),
    })
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

#[derive(Default)]
struct TradeThesisOutcomeStats {
    recorded_thesis_count: usize,
    filled_thesis_count: usize,
    one_session_count: usize,
    one_session_return_sum_pct: f64,
    one_session_positive_count: usize,
    five_session_count: usize,
    five_session_return_sum_pct: f64,
    five_session_positive_count: usize,
}

#[derive(Default)]
struct DecisionPulseOutcomeStats {
    attributed_order_count: usize,
    buy_order_count: usize,
    sell_order_count: usize,
    execution_status_counts: BTreeMap<String, usize>,
    hermes_reviewed_order_count: usize,
    hermes_effect_counts: BTreeMap<String, usize>,
    filled_buy_order_count: usize,
    reconciled_sell_order_count: usize,
    one_session_count: usize,
    one_session_return_sum_pct: f64,
    one_session_positive_count: usize,
    five_session_count: usize,
    five_session_return_sum_pct: f64,
    five_session_positive_count: usize,
    realised_sell_gain_dkk: f64,
    realised_sell_commission_dkk: f64,
    realised_sell_tax_dkk: f64,
}

#[derive(Default)]
struct MissedTradeShadowOutcomeStats {
    recorded_shadow_count: usize,
    observed_shadow_count: usize,
    directional_return_sum_pct: f64,
    positive_return_count: usize,
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

/// Summarizes only the recorded BUY theses that have reconciled-fill outcome
/// evidence. This is observational aggregation, not a backtest: it does not
/// include blocked candidates, broker adjustments, FX, costs, or later
/// position changes, and it does not claim that the thesis caused an outcome.
fn trade_thesis_outcome_evidence_from_holding_outcomes(outcomes: &[JsonValue]) -> JsonValue {
    let mut stats = TradeThesisOutcomeStats {
        recorded_thesis_count: outcomes.len(),
        ..TradeThesisOutcomeStats::default()
    };
    for outcome in outcomes {
        if outcome.is_null() || value_f64(outcome, "filled_quantity") <= 0.0 {
            continue;
        }
        stats.filled_thesis_count += 1;
        for (session, count, return_sum, positive_count) in [
            (
                outcome.get("one_session").unwrap_or(&JsonValue::Null),
                &mut stats.one_session_count,
                &mut stats.one_session_return_sum_pct,
                &mut stats.one_session_positive_count,
            ),
            (
                outcome.get("five_session").unwrap_or(&JsonValue::Null),
                &mut stats.five_session_count,
                &mut stats.five_session_return_sum_pct,
                &mut stats.five_session_positive_count,
            ),
        ] {
            if session.is_null() {
                continue;
            }
            let directional_return_pct = value_f64(session, "directional_return_pct");
            if !directional_return_pct.is_finite() {
                continue;
            }
            *count += 1;
            *return_sum += directional_return_pct;
            if directional_return_pct > 0.0 {
                *positive_count += 1;
            }
        }
    }
    let status = if stats.recorded_thesis_count == 0 {
        "no_recorded_theses"
    } else if stats.five_session_count < TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };
    let session_summary = |count: usize, return_sum_pct: f64, positive_count: usize| {
        json!({
            "sample_count": count,
            "average_directional_return_pct": average_or_null(return_sum_pct, count),
            "positive_return_rate": fraction_or_null(positive_count, count),
        })
    };
    json!({
        "status": status,
        "recorded_thesis_count": stats.recorded_thesis_count,
        "filled_thesis_count": stats.filled_thesis_count,
        "one_session": session_summary(
            stats.one_session_count,
            stats.one_session_return_sum_pct,
            stats.one_session_positive_count,
        ),
        "five_session": session_summary(
            stats.five_session_count,
            stats.five_session_return_sum_pct,
            stats.five_session_positive_count,
        ),
        "minimum_complete_observations": TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS,
        "scan_limit": TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT,
        "safety": "read_only_local_execution_fills_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "Directional returns compare reconciled BUY fills with later stored daily closes. They exclude blocked candidates, FX, commission, tax, slippage, later position changes, broker adjustments, and any causal claim about the thesis."
    })
}

fn normalized_decision_pulse(row: &JsonValue) -> (String, String) {
    let configured_key = json_text(row, "analysis_pulse_key");
    let configured_label = json_text(row, "analysis_pulse_label");
    let strategy_type = json_text(row, "strategy_type");
    let (key, fallback_label) = if strategy_type.eq_ignore_ascii_case("portfolio_sync") {
        ("portfolio_sync", "Portfolio Sync")
    } else if configured_key.starts_with("europe_open_followup") {
        ("europe_open_followup", "EU Open +1h15")
    } else if configured_key.starts_with("us_open_followup") {
        ("us_open_followup", "US Open +1h15")
    } else if configured_key.starts_with("manual_dry_run") {
        ("manual_dry_run", "Manual Dry Run")
    } else if configured_key.starts_with("manual") {
        ("manual", "Manual")
    } else {
        ("other", "Other / legacy")
    };
    let label = if configured_label.trim().is_empty() {
        fallback_label.to_string()
    } else {
        configured_label
    };
    (key.to_string(), label)
}

fn decision_pulse_outcome_summary(stats: &DecisionPulseOutcomeStats) -> JsonValue {
    let directional_summary = |count: usize, return_sum_pct: f64, positive_count: usize| {
        json!({
            "sample_count": count,
            "average_directional_return_pct": average_or_null(return_sum_pct, count),
            "positive_return_rate": fraction_or_null(positive_count, count),
        })
    };
    json!({
        "attributed_order_count": stats.attributed_order_count,
        "buy_order_count": stats.buy_order_count,
        "sell_order_count": stats.sell_order_count,
        "execution_status_counts": stats.execution_status_counts,
        "hermes_reviewed_order_count": stats.hermes_reviewed_order_count,
        "hermes_effect_counts": stats.hermes_effect_counts,
        "filled_buy_order_count": stats.filled_buy_order_count,
        "reconciled_sell_order_count": stats.reconciled_sell_order_count,
        "one_session": directional_summary(
            stats.one_session_count,
            stats.one_session_return_sum_pct,
            stats.one_session_positive_count,
        ),
        "five_session": directional_summary(
            stats.five_session_count,
            stats.five_session_return_sum_pct,
            stats.five_session_positive_count,
        ),
        "realised_sell": {
            "realised_gain_dkk": stats.realised_sell_gain_dkk,
            "commission_dkk": stats.realised_sell_commission_dkk,
            "tax_dkk": stats.realised_sell_tax_dkk,
        },
    })
}

/// Separates observed execution outcomes by their report pulse. BUYs use later
/// stored closes as directional movement while SELLs use reconciled local-ledger
/// DKK gains. The evidence is deliberately observational: Hermes presence is
/// shown as review coverage, not proof that advice caused an outcome.
fn decision_pulse_outcome_evidence_from_observations(observations: &[JsonValue]) -> JsonValue {
    let mut overall = DecisionPulseOutcomeStats::default();
    let mut by_pulse: BTreeMap<String, (String, DecisionPulseOutcomeStats)> = BTreeMap::new();
    for observation in observations {
        let (pulse_key, pulse_label) = normalized_decision_pulse(observation);
        let pulse = by_pulse
            .entry(pulse_key)
            .or_insert_with(|| (pulse_label, DecisionPulseOutcomeStats::default()));
        for stats in [&mut overall, &mut pulse.1] {
            stats.attributed_order_count += 1;
            let execution_status = json_text(observation, "execution_status");
            if !execution_status.is_empty() && execution_status != "not_recorded" {
                *stats
                    .execution_status_counts
                    .entry(execution_status)
                    .or_default() += 1;
            }
            if observation
                .get("hermes_reviewed")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
            {
                stats.hermes_reviewed_order_count += 1;
            }
            let hermes_effect = json_text(observation, "hermes_effect");
            if !hermes_effect.is_empty() && hermes_effect != "not_recorded" {
                *stats.hermes_effect_counts.entry(hermes_effect).or_default() += 1;
            }
            let action = json_text(observation, "action").to_uppercase();
            if action == "BUY" {
                stats.buy_order_count += 1;
                let outcome = observation
                    .get("holding_period_outcome")
                    .unwrap_or(&JsonValue::Null);
                if value_f64(outcome, "filled_quantity") > 0.0 {
                    stats.filled_buy_order_count += 1;
                }
                for (session, count, return_sum, positive_count) in [
                    (
                        outcome.get("one_session").unwrap_or(&JsonValue::Null),
                        &mut stats.one_session_count,
                        &mut stats.one_session_return_sum_pct,
                        &mut stats.one_session_positive_count,
                    ),
                    (
                        outcome.get("five_session").unwrap_or(&JsonValue::Null),
                        &mut stats.five_session_count,
                        &mut stats.five_session_return_sum_pct,
                        &mut stats.five_session_positive_count,
                    ),
                ] {
                    if session.is_null() {
                        continue;
                    }
                    let directional_return_pct = value_f64(session, "directional_return_pct");
                    if !directional_return_pct.is_finite() {
                        continue;
                    }
                    *count += 1;
                    *return_sum += directional_return_pct;
                    if directional_return_pct > 0.0 {
                        *positive_count += 1;
                    }
                }
            } else if action == "SELL" {
                stats.sell_order_count += 1;
                let outcome = observation
                    .get("ledger_outcome")
                    .unwrap_or(&JsonValue::Null);
                if json_text(outcome, "status") == "reconciled" {
                    stats.reconciled_sell_order_count += 1;
                    stats.realised_sell_gain_dkk += value_f64(outcome, "realised_gain_dkk");
                    stats.realised_sell_commission_dkk += value_f64(outcome, "commission_dkk");
                    stats.realised_sell_tax_dkk += value_f64(outcome, "tax_dkk");
                }
            }
        }
    }
    let status = if overall.attributed_order_count == 0 {
        "no_attributable_orders"
    } else if overall.five_session_count < TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS {
        "collecting"
    } else {
        "preliminary"
    };
    let pulses = by_pulse
        .into_iter()
        .map(|(pulse_key, (pulse_label, stats))| {
            json!({
                "pulse_key": pulse_key,
                "pulse_label": pulse_label,
                "outcome": decision_pulse_outcome_summary(&stats),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": status,
        "overall": decision_pulse_outcome_summary(&overall),
        "pulses": pulses,
        "minimum_complete_observations": TRADE_THESIS_OUTCOME_MIN_COMPLETE_OBSERVATIONS,
        "scan_limit": DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT,
        "safety": "read_only_local_execution_orders_fills_ledger_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "BUY rows show equal-weighted forward directional price movement after reconciled fills; they are not unrealised P/L. SELL rows sum local-ledger realised DKK gain, commission, and tax only after reconciliation. Hermes effects come only from the durable Trading Manager advice-delta snapshot matched to the stored execution-order strategy key. They classify the advice applied to an order but do not establish causal performance impact. Portfolio-sync rows are imported-state context, not system-initiated trades."
    })
}

/// Summarizes quote-to-quote observations for the selected deterministic
/// manager blocks. Each shadow gets equal weight so local-currency P/L is not
/// combined across instruments. This remains an observational diagnostic, not
/// evidence that a gate was wrong or a trading-rule backtest.
fn missed_trade_shadow_outcome_evidence_from_rows(rows: &[JsonValue]) -> JsonValue {
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

/// Reconciles the latest persisted broker-position snapshot with locally
/// recorded SELL Stop/StopLimit orders. It deliberately does not query Saxo
/// and only classifies broker-confirmed order states as active protection.
/// Queued or uncertain orders remain planned so the dashboard cannot imply a
/// protection guarantee that the broker has not acknowledged.
/// Default when `strategy.ladder.stop_loss_atr_multiple` is absent. Matches the
/// value both shipped configs carry.
/// One-variable overlay paths Hermes may propose. Cross-checked against the
/// config contract by test, so a variable nothing reads cannot be offered.
///
/// Legacy cash-buffer paths are retired: the single active reserve is
/// `strategy.capital.min_cash_buffer_pct`.
pub(crate) const SUPPORTED_EXPERIMENT_VARIABLES: &[&str] = &[
    "execution.min_trade_value_dkk",
    "strategy.capital.min_cash_buffer_pct",
    "strategy.swing.daily_indicators.min_confluences",
    "strategy.swing.markov_gate.min_signed_signal",
];

const DEFAULT_STOP_LOSS_ATR_MULTIPLE: f64 = 2.0;

/// Marks an `execution_orders` row as a broker-hosted protective stop rather
/// than a discretionary order. Three code paths depend on telling them apart: a
/// resting GTC stop must not pin the scheduler at its fast poll interval, must
/// not reserve the quantity its own position needs to exit, and must be
/// cancelled before a discretionary SELL on the same symbol reaches Saxo.
pub(crate) const PROTECTIVE_STOP_STRATEGY_TYPE: &str = "protective_stop";

/// A protective stop level derived from stored daily indicators.
///
/// This is arithmetic on data the nightly indicator run already persisted: it
/// makes no Saxo call and places nothing. The price is deliberately *not*
/// tick-normalized here, because normalization needs Saxo instrument details;
/// the precheck and placement paths normalize before any order is built.
fn proposed_protective_stop(
    indicator: Option<&JsonValue>,
    quantity: f64,
    atr_multiple: f64,
) -> Option<JsonValue> {
    let indicator = indicator?;
    let close = value_f64(indicator, "close");
    let atr14 = value_f64(indicator, "atr14");
    if !close.is_finite() || close <= 0.0 || !atr14.is_finite() || atr14 <= 0.0 {
        return None;
    }
    if !atr_multiple.is_finite() || atr_multiple <= 0.0 {
        return None;
    }
    let distance = atr14 * atr_multiple;
    let stop_price = close - distance;
    // A stop at or below zero is not a protective level; report no proposal
    // rather than a nonsensical one.
    if !stop_price.is_finite() || stop_price <= 0.0 {
        return None;
    }
    Some(json!({
        "stop_price_local": stop_price,
        "quantity": quantity,
        "reference_close": close,
        "atr14": atr14,
        "atr_multiple": atr_multiple,
        "distance_local": distance,
        "distance_pct": (distance / close) * 100.0,
        "indicator_run_date": json_text(indicator, "run_date"),
        "tick_normalized": false,
        "basis": "close_minus_atr14_times_multiple",
        "safety": "computed_from_stored_indicators_no_saxo_call_and_places_nothing",
    }))
}

fn protective_stop_coverage_from_rows(
    position_rows: &[JsonValue],
    execution_order_rows: &[JsonValue],
    lifecycle_test_rows: &[JsonValue],
    indicator_rows: &[JsonValue],
    atr_multiple: f64,
) -> JsonValue {
    let mut indicators_by_symbol: HashMap<String, &JsonValue> = HashMap::new();
    for row in indicator_rows {
        let symbol = json_text(row, "symbol");
        if !symbol.trim().is_empty() {
            // Rows arrive newest-first, so the first entry per symbol wins.
            indicators_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_insert(row);
        }
    }
    let mut stops_by_symbol: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for order in execution_order_rows {
        if !json_text(order, "action").eq_ignore_ascii_case("SELL") {
            continue;
        }
        let order_type = json_text(order, "order_type").to_ascii_lowercase();
        if order_type != "stop" && order_type != "stoplimit" {
            continue;
        }
        let symbol = json_text(order, "symbol");
        if !symbol.trim().is_empty() {
            stops_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_default()
                .push(order);
        }
    }

    // Once a stop is adopted into `execution_orders` it is the same broker
    // order as its lifecycle-test row. Counting both would report two stops
    // covering one position and inflate the active stop count.
    let adopted_broker_order_ids = execution_order_rows
        .iter()
        .filter_map(|order| {
            let broker_order_id = json_text(order, "broker_order_id").trim().to_string();
            (!broker_order_id.is_empty()).then_some(broker_order_id)
        })
        .collect::<HashSet<String>>();

    // A lifecycle test is distinct from the normal execution queue. Count it
    // only after reconciliation has confirmed a broker-working SIM Stop with
    // an order identifier. Submitted, cancelled, failed, and ambiguous tests
    // deliberately provide no coverage evidence.
    let mut lifecycle_stops_by_symbol: HashMap<String, Vec<&JsonValue>> = HashMap::new();
    for test in lifecycle_test_rows {
        if !json_text(test, "environment").eq_ignore_ascii_case("sim")
            || json_text(test, "status") != "broker_working"
            || json_text(test, "broker_order_id").trim().is_empty()
            || adopted_broker_order_ids.contains(json_text(test, "broker_order_id").trim())
        {
            continue;
        }
        let symbol = json_text(test, "symbol");
        if !symbol.trim().is_empty() {
            lifecycle_stops_by_symbol
                .entry(symbol.trim().to_ascii_uppercase())
                .or_default()
                .push(test);
        }
    }

    let mut positions = Vec::new();
    let mut exceptions = Vec::new();
    let mut protected_count = 0usize;
    let mut partial_count = 0usize;
    let mut planned_count = 0usize;
    let mut unprotected_count = 0usize;
    let mut total_quantity = 0.0;
    let mut confirmed_covered_quantity = 0.0;

    for position in position_rows {
        let quantity = value_f64(position, "quantity");
        if !quantity.is_finite() || quantity <= 0.0 {
            continue;
        }
        let symbol = json_text(position, "symbol");
        if symbol.trim().is_empty() {
            continue;
        }
        total_quantity += quantity;
        let orders = stops_by_symbol
            .get(&symbol.trim().to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        let lifecycle_tests = lifecycle_stops_by_symbol
            .get(&symbol.trim().to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        let active_orders = orders
            .iter()
            .copied()
            .filter(|order| {
                matches!(
                    json_text(order, "status").as_str(),
                    "submitted_to_broker"
                        | "broker_working"
                        | "broker_amended"
                        | "broker_partially_filled"
                        | "broker_replace_requested"
                )
            })
            .collect::<Vec<_>>();
        let planned_orders = orders
            .iter()
            .copied()
            .filter(|order| {
                matches!(
                    json_text(order, "status").as_str(),
                    "pending_execution"
                        | "pending_approval"
                        | "submitting_to_broker"
                        | "planned_stop_loss"
                        | "planned_child_order"
                        | "waiting_for_market_open"
                        | "waiting_for_cash_settlement"
                        | "waiting_for_virtual_cash_budget"
                )
            })
            .collect::<Vec<_>>();
        let execution_order_covered_quantity = active_orders
            .iter()
            .map(|order| value_f64(order, "quantity").max(0.0))
            .sum::<f64>();
        let lifecycle_test_covered_quantity = lifecycle_tests
            .iter()
            .map(|test| value_f64(test, "quantity").max(0.0))
            .sum::<f64>();
        let confirmed_quantity =
            (execution_order_covered_quantity + lifecycle_test_covered_quantity).min(quantity);
        let active_stop_price = active_orders
            .iter()
            .filter_map(|order| {
                let price = value_f64(order, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            })
            .chain(lifecycle_tests.iter().filter_map(|test| {
                let price = value_f64(test, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            }))
            .max_by(f64::total_cmp);
        let planned_stop_price = planned_orders
            .iter()
            .filter_map(|order| {
                let price = value_f64(order, "stop_price_local");
                (price.is_finite() && price > 0.0).then_some(price)
            })
            .max_by(f64::total_cmp);
        let active_stop_count = active_orders.len() + lifecycle_tests.len();
        let protection_status = if active_stop_count > 0 && confirmed_quantity + 1e-6 >= quantity {
            protected_count += 1;
            "protected"
        } else if active_stop_count > 0 {
            partial_count += 1;
            "partial_protection"
        } else if !planned_orders.is_empty() {
            planned_count += 1;
            "planned"
        } else {
            unprotected_count += 1;
            "unprotected"
        };
        confirmed_covered_quantity += confirmed_quantity;
        let unprotected_quantity = (quantity - confirmed_quantity).max(0.0);
        // Only the uncovered share needs a stop, so the proposal is sized to it.
        let proposed_stop = (unprotected_quantity > 0.0)
            .then(|| {
                proposed_protective_stop(
                    indicators_by_symbol
                        .get(&symbol.trim().to_ascii_uppercase())
                        .copied(),
                    unprotected_quantity,
                    atr_multiple,
                )
            })
            .flatten();
        if protection_status != "protected" {
            let (kind, reason) = match protection_status {
                "partial_protection" => (
                    "partial_protective_stop_coverage",
                    "Only part of the persisted broker position has broker-confirmed protective-stop coverage.",
                ),
                "planned" => (
                    "planned_stop_not_broker_confirmed",
                    "A local stop plan exists, but Saxo has not confirmed a working protective stop.",
                ),
                _ => (
                    "unprotected_broker_position",
                    "No broker-confirmed protective stop is recorded for this persisted broker position.",
                ),
            };
            exceptions.push(json!({
                "kind": kind,
                "severity": "warning",
                "symbol": symbol,
                "broker_quantity": quantity,
                "confirmed_covered_quantity": confirmed_quantity,
                "unprotected_quantity": unprotected_quantity,
                "reason": reason,
                "proposed_stop": proposed_stop.clone(),
                "operator_action": "Review the persisted broker position and stop evidence. The SIM lifecycle test is manual and does not place, change, or cancel any order without its separate confirmation.",
            }));
        }
        positions.push(json!({
            "symbol": symbol,
            "quantity": quantity,
            "currency": json_text(position, "currency"),
            "snapshot_updated_at": json_text(position, "updated_at"),
            "protection_status": protection_status,
            "confirmed_covered_quantity": confirmed_quantity,
            "coverage_ratio": (confirmed_quantity / quantity).clamp(0.0, 1.0),
            "active_stop_count": active_stop_count,
            "execution_order_stop_count": active_orders.len(),
            "lifecycle_test_stop_count": lifecycle_tests.len(),
            "coverage_evidence": {
                "execution_orders": active_orders.len(),
                "manual_sim_lifecycle_tests": lifecycle_tests.len(),
            },
            "planned_stop_count": planned_orders.len(),
            "active_stop_price_local": active_stop_price,
            "planned_stop_price_local": planned_stop_price,
            "proposed_stop": proposed_stop,
        }));
    }

    let status = if positions.is_empty() {
        "no_positive_broker_positions_recorded"
    } else if unprotected_count > 0 || partial_count > 0 {
        "attention_required"
    } else if planned_count > 0 {
        "planned_only"
    } else {
        "covered"
    };
    json!({
        "status": status,
        "summary": {
            "position_count": positions.len(),
            "protected_count": protected_count,
            "partial_count": partial_count,
            "planned_count": planned_count,
            "unprotected_count": unprotected_count,
            "total_quantity": total_quantity,
            "confirmed_covered_quantity": confirmed_covered_quantity,
            "exception_count": exceptions.len(),
        },
        "positions": positions,
        "exceptions": exceptions,
        "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
        "interpretation": "Coverage is inferred from the latest persisted broker-position snapshot, local SELL Stop or StopLimit records, and reconciled manual SIM lifecycle-test records. Only broker-confirmed stop states and lifecycle tests reconciled as broker-working count as protection; queued, unresolved, stale, cancelled, or failed records do not. A broker-hosted stop can still fill away from its stop price during a market gap.",
    })
}

fn compact_protective_stop_coverage_for_hermes(coverage: &JsonValue, limit: usize) -> JsonValue {
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
                    "cost_guard": compact_candidate_cost_guard(row),
                    "concentration": compact_candidate_concentration(row),
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
            "cost_guard": outcome.get("cost_guard").cloned().unwrap_or(JsonValue::Null),
            "concentration": outcome.get("concentration").cloned().unwrap_or(JsonValue::Null),
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
            "cost_guard": outcome.get("cost_guard").cloned().unwrap_or(JsonValue::Null),
            "concentration": outcome.get("concentration").cloned().unwrap_or(JsonValue::Null),
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

/// Post-fill market movement is attribution evidence, not realised P/L. It
/// intentionally uses only persisted fills and daily-indicator closes: no
/// Saxo/quote request occurs while rendering an execution row.
fn compact_holding_period_outcome(
    order: &JsonValue,
    fill_summary: &JsonValue,
    subsequent_closes: &[JsonValue],
) -> JsonValue {
    let side = json_text(order, "action").to_uppercase();
    if !matches!(side.as_str(), "BUY" | "SELL") {
        return JsonValue::Null;
    }
    let filled_quantity = value_f64(fill_summary, "filled_quantity");
    let fill_price_local = value_f64(fill_summary, "average_fill_price_local");
    let first_fill_at = json_text(fill_summary, "first_fill_at");
    if filled_quantity <= 0.0 || fill_price_local <= 0.0 || first_fill_at.is_empty() {
        return JsonValue::Null;
    }

    let directional_multiplier = if side == "BUY" { 1.0 } else { -1.0 };
    let session_outcome = |session: usize, close: Option<&JsonValue>| {
        let Some(close) = close else {
            return JsonValue::Null;
        };
        let close_local = value_f64(close, "close");
        if close_local <= 0.0 {
            return JsonValue::Null;
        }
        let market_return_pct = close_local / fill_price_local - 1.0;
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
    let status = if !five_session.is_null() {
        "complete"
    } else if !one_session.is_null() {
        "partial"
    } else {
        "pending_daily_close"
    };
    json!({
        "status": status,
        "evidence_source": "reconciled_fills_and_daily_indicator_closes",
        "side": side,
        "filled_quantity": filled_quantity,
        "fill_price_local": fill_price_local,
        "currency": json_text(fill_summary, "currency"),
        "first_fill_at": first_fill_at,
        "available_sessions": subsequent_closes.len(),
        "one_session": one_session,
        "five_session": five_session,
        "interpretation": "Directional return is a read-only post-fill price comparison. It excludes FX, commissions, tax, slippage, and later position changes; it is not realised P/L."
    })
}

/// Turns durable BUY-thesis records and the latest local broker-position
/// snapshot into an operator review queue. A due review is deliberately not
/// an exit recommendation: imported lots and later position changes can make
/// a recorded entry only partial provenance for the broker holding.
fn compact_holding_thesis_reviews(
    positions: &[JsonValue],
    thesis_rows: &[JsonValue],
    stale_after_days: i64,
    now: DateTime<Utc>,
) -> JsonValue {
    let stale_after_days = stale_after_days.max(1);
    let held = positions
        .iter()
        .filter_map(|position| {
            let symbol = json_text(position, "symbol");
            (value_f64(position, "quantity") > 1e-9 && !symbol.is_empty())
                .then(|| (watchlist_symbol_key(&symbol), position))
        })
        .collect::<HashMap<_, _>>();
    let held_position_count = held.len();
    let mut latest_thesis_by_symbol = HashMap::<String, (&JsonValue, JsonValue)>::new();
    for row in thesis_rows {
        let symbol = json_text(row, "symbol");
        let raw = row
            .get("trade_thesis_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        let thesis = if raw.is_object() {
            raw
        } else {
            raw.as_str()
                .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                .unwrap_or(JsonValue::Null)
        };
        if symbol.is_empty() || json_text(&thesis, "status") != "recorded" {
            continue;
        }
        latest_thesis_by_symbol
            .entry(watchlist_symbol_key(&symbol))
            .or_insert((row, thesis));
    }

    let mut reviews = Vec::new();
    for (symbol, position) in held {
        let Some((row, thesis)) = latest_thesis_by_symbol.get(&symbol) else {
            continue;
        };
        let mut tracked_at = json_text(row, "first_fill_at");
        if tracked_at.is_empty() {
            tracked_at = json_text(row, "created_at");
        }
        let Some(tracked_at) = DateTime::parse_from_rfc3339(&tracked_at)
            .ok()
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        let age_days = (now - tracked_at).num_days().max(0);
        let intended_window = json_text(thesis, "intended_holding_window");
        let thesis_window_days = match intended_window.as_str() {
            "next_1_week" => Some(7),
            "next_2_weeks" => Some(14),
            "next_1_month" => Some(30),
            _ => None,
        };
        let decision_evidence_stale = age_days >= stale_after_days;
        let thesis_window_elapsed = thesis_window_days.is_some_and(|days| age_days >= days);
        if !decision_evidence_stale && !thesis_window_elapsed {
            continue;
        }
        let status = if thesis_window_elapsed {
            "thesis_window_elapsed"
        } else {
            "decision_evidence_stale"
        };
        reviews.push(json!({
            "symbol": json_text(position, "symbol"),
            "instrument_name": json_text(position, "instrument_name"),
            "position_quantity": value_f64(position, "quantity"),
            "latest_thesis_order_id": value_i64(row, "id"),
            "tracked_entry_at": tracked_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "age_days": age_days,
            "decision_stale_after_days": stale_after_days,
            "intended_holding_window": intended_window,
            "intended_holding_window_days": thesis_window_days,
            "status": status,
            "entry_rationale": compact_review_text(&json_text(thesis, "entry_rationale"), 240),
            "invalidation": compact_review_text(&json_text(thesis, "invalidation"), 320),
            "operator_next_step": "Request or wait for a fresh decision pulse, then compare current verified technical and Markov evidence with the recorded entry thesis. This review does not instruct an exit.",
        }));
    }
    reviews.sort_by(|left, right| {
        value_i64(right, "age_days")
            .cmp(&value_i64(left, "age_days"))
            .then_with(|| json_text(left, "symbol").cmp(&json_text(right, "symbol")))
    });
    json!({
        "status": if reviews.is_empty() { "no_reviews_due" } else { "review_due" },
        "held_position_count": held_position_count,
        "review_count": reviews.len(),
        "decision_stale_after_days": stale_after_days,
        "reviews": reviews,
        "safety": "read_only_local_broker_position_snapshot_execution_order_thesis_and_fill_audit_no_saxo_provider_hermes_or_order_mutation",
        "interpretation": "A review identifies a held symbol with a recorded BUY thesis whose decision evidence is stale or intended window has elapsed. It is not a sell signal, sizing instruction, gate, or broker action."
    })
}

fn compact_review_text(value: &str, max_chars: usize) -> String {
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

/// Reconstruct the local, reconciled-fill sequence around one order without
/// treating it as broker position truth. Historical fills can be incomplete
/// (for example after a portfolio import), so a SELL without an observed BUY
/// is explicitly reported as partial rather than inferred as a reduction.
fn compact_execution_position_lifecycle(
    order: &JsonValue,
    observed_fills: &[JsonValue],
) -> JsonValue {
    let order_id = value_i64(order, "id");
    let side = json_text(order, "action").to_uppercase();
    if order_id <= 0 || !matches!(side.as_str(), "BUY" | "SELL") {
        return JsonValue::Null;
    }

    let mut observed_net: f64 = 0.0;
    let mut minimum_observed_net: f64 = 0.0;
    let mut net_before_current = None;
    let mut net_after_current = None;
    let mut current_fill_count = 0_i64;
    let mut observed_order_ids = HashSet::new();
    let mut first_fill_at = String::new();
    let mut latest_fill_at = String::new();
    let mut first_side = String::new();

    for fill in observed_fills {
        let fill_side = json_text(fill, "side").to_uppercase();
        let quantity = value_f64(fill, "delta_quantity");
        if quantity <= 0.0 || !matches!(fill_side.as_str(), "BUY" | "SELL") {
            continue;
        }
        let fill_order_id = value_i64(fill, "execution_order_id");
        if fill_order_id > 0 {
            observed_order_ids.insert(fill_order_id);
        }
        let created_at = json_text(fill, "created_at");
        if first_fill_at.is_empty() {
            first_fill_at = created_at.clone();
            first_side = fill_side.clone();
        }
        latest_fill_at = created_at;

        if fill_order_id == order_id {
            net_before_current.get_or_insert(observed_net);
        }
        let signed_quantity = if fill_side == "BUY" {
            quantity
        } else {
            -quantity
        };
        observed_net += signed_quantity;
        minimum_observed_net = minimum_observed_net.min(observed_net);
        if fill_order_id == order_id {
            current_fill_count += 1;
            net_after_current = Some(observed_net);
        }
    }

    let (Some(net_before), Some(net_after)) = (net_before_current, net_after_current) else {
        return JsonValue::Null;
    };
    let history_status = if first_side == "SELL" || minimum_observed_net < -1e-9 {
        "partial_history"
    } else {
        "observed_local_fills"
    };
    let phase = if history_status == "partial_history" {
        "partial_history"
    } else if side == "BUY" && net_before <= 1e-9 {
        "entry"
    } else if side == "BUY" {
        "add"
    } else if net_before <= 1e-9 || net_after < -1e-9 {
        "partial_history"
    } else if net_after <= 1e-9 {
        "exit"
    } else {
        "reduce"
    };

    json!({
        "evidence_source": "reconciled_execution_fills",
        "history_status": history_status,
        "phase": phase,
        "side": side,
        "observed_net_before": net_before,
        "observed_net_after": net_after,
        "current_order_fill_count": current_fill_count,
        "observed_fill_count": observed_fills.len(),
        "observed_order_count": observed_order_ids.len(),
        "first_observed_fill_at": first_fill_at,
        "latest_observed_fill_at": latest_fill_at,
        "interpretation": "Read-only local reconciled-fill sequence. It excludes outside-ledger inventory and later broker adjustments; it is not broker position truth."
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

/// Use the price-monitor calculation whenever it is available. In SIM,
/// `InstrumentPriceDayPercentChange` may be zero for an approximated broker
/// exposure even though the current infoprice and LastClose establish a move.
fn daily_change_pct_from_sources(
    price_snapshot: Option<&JsonValue>,
    broker_exposure: Option<&JsonValue>,
) -> f64 {
    price_snapshot
        .and_then(|row| row.get("change_pct"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite())
        .or_else(|| {
            broker_exposure
                .and_then(|row| row.get("instrument_price_day_percent_change"))
                .and_then(JsonValue::as_f64)
                .filter(|value| value.is_finite())
        })
        .unwrap_or(0.0)
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
        // Report config keys that are configured but unwired before anything
        // starts trusting them. Covers every process mode because api,
        // scheduler, and MCP all load state through here.
        crate::config_contract::log_config_contract_audit(&config);
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
        let execution_trade_thesis_evidence = if dashboard_loads_tab_exclusive_data(
            &active_view,
            "execution",
        ) {
            self.trade_thesis_outcome_evidence().await.unwrap_or_else(|err| {
                    warn!("dashboard trade-thesis evidence degraded: {err:#}");
                    json!({
                        "status": "unavailable",
                        "safety": "read_only_local_execution_fills_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
                        "interpretation": "Trade-thesis outcome evidence could not be loaded. It does not affect gates, Hermes, configuration, or Saxo orders.",
                    })
                })
        } else {
            JsonValue::Null
        };
        let execution_holding_thesis_reviews = if dashboard_loads_tab_exclusive_data(
            &active_view,
            "execution",
        ) {
            self.holding_thesis_reviews().await.unwrap_or_else(|err| {
                warn!("dashboard holding-thesis reviews degraded: {err:#}");
                json!({
                    "status": "unavailable",
                    "safety": "read_only_local_broker_position_snapshot_execution_order_thesis_and_fill_audit_no_saxo_provider_hermes_or_order_mutation",
                    "interpretation": "Holding-thesis reviews could not be loaded. They do not affect gates, Hermes, configuration, or Saxo orders.",
                })
            })
        } else {
            JsonValue::Null
        };
        let execution_decision_pulse_evidence = if dashboard_loads_tab_exclusive_data(
            &active_view,
            "execution",
        ) {
            self.decision_pulse_outcome_evidence().await.unwrap_or_else(|err| {
                warn!("dashboard decision-pulse outcome evidence degraded: {err:#}");
                json!({
                    "status": "unavailable",
                    "safety": "read_only_local_execution_orders_fills_ledger_and_daily_indicator_closes_no_saxo_provider_hermes_or_order_mutation",
                    "interpretation": "Decision-pulse outcome evidence could not be loaded. It does not affect gates, Hermes, configuration, or Saxo orders.",
                })
            })
        } else {
            JsonValue::Null
        };
        let execution_protection = if dashboard_loads_tab_exclusive_data(&active_view, "execution")
        {
            self.protective_stop_coverage().await.unwrap_or_else(|err| {
                warn!("dashboard protective-stop coverage degraded: {err:#}");
                json!({
                    "status": "unavailable",
                    "summary": {},
                    "positions": [],
                    "safety": "read_only_local_broker_position_snapshot_and_execution_order_audit_no_saxo_call_or_order_mutation",
                    "interpretation": "Protective-stop coverage could not be loaded. No Saxo order was placed, replaced, or cancelled.",
                })
            })
        } else {
            JsonValue::Null
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
        let missed_trade_shadows = if dashboard_loads_tab_exclusive_data(&active_view, "hermes") {
            self.missed_trade_shadows(MISSED_TRADE_SHADOW_LIMIT)
                .await
                .unwrap_or_else(|err| {
                    warn!("dashboard missed-trade shadows degraded: {err:#}");
                    Vec::new()
                })
        } else {
            Vec::new()
        };
        let missed_trade_shadow_evidence = if dashboard_loads_tab_exclusive_data(
            &active_view,
            "hermes",
        ) {
            self.missed_trade_shadow_outcome_evidence()
                    .await
                    .unwrap_or_else(|err| {
                        warn!("dashboard missed-trade shadow evidence degraded: {err:#}");
                        json!({
                            "status": "unavailable",
                            "safety": "read_only_local_quote_to_quote_observations_no_saxo_provider_hermes_or_order_mutation",
                            "interpretation": "Missed-trade shadow evidence could not be loaded. It does not affect gates, Hermes, configuration, or Saxo orders.",
                        })
                    })
        } else {
            JsonValue::Null
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
            run_schedules: json!({
                "markov": crate::markov_method::markov_config_json_for_state(self),
                "quiver": crate::quiver::quiver_config_json_for_state(self),
                "indicators": crate::daily_indicators::indicator_config_json_for_state(self),
                "performance_benchmarks": crate::performance_benchmarks::benchmark_config_json_for_state(self),
            }),
            performance_history,
            performance_summary,
            performance_benchmarks,
            performance_goal_tracking,
            integrity: overview
                .get("integrity")
                .cloned()
                .unwrap_or_else(|| json!({"healthy": false, "warnings": [], "mismatches": []})),
            execution_protection,
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

        let after_tax_summary = self
            .after_tax_summary(value_f64(&aggregate, "total_unrealised_pnl_dkk"))
            .await;

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
            "after_tax_summary": after_tax_summary,
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
            "benchmarks": crate::performance_benchmarks::performance_benchmark_payload(self, &history).await?,
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
        let (range_return_pct, range_max_drawdown_pct) = performance_range_metrics(history);
        json!({
            "points": history.len(),
            "first_recorded_at": first.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "latest_recorded_at": latest.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
            "first_total_market_value_dkk": first_total,
            "latest_total_market_value_dkk": latest_total,
            "change_dkk": latest_total - first_total,
            "daily_pnl_dkk": latest_daily,
            "position_count": latest_positions,
            "range_return_pct": range_return_pct,
            "range_max_drawdown_pct": range_max_drawdown_pct,
            "confidence": performance_confidence(history, Utc::now()),
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
            // The price monitor derives this from Saxo's LastClose and the
            // current infoprice. Broker exposures can legitimately report
            // zero for delayed/approximated SIM prices, so they are only a
            // fallback when no monitored quote exists.
            let daily_change_pct = daily_change_pct_from_sources(price, exposure);
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
                // Watchlist rows use `change_pct`; position views retain the
                // explicit daily name. Keep both projections aligned.
                "change_pct": daily_change_pct,
                "daily_change_pct": daily_change_pct,
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

    async fn after_tax_summary(&self, gross_unrealised_pnl_dkk: f64) -> JsonValue {
        let tax_year = Utc::now().year();
        let currency = yaml_string(&self.config, &["taxation", "share_income", "currency"])
            .unwrap_or_else(|| "DKK".to_string());
        if !currency.eq_ignore_ascii_case("DKK") {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "unsupported_tax_currency",
            );
        }
        let Some(brackets) = share_income_tax_brackets(&self.config) else {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "invalid_tax_brackets",
            );
        };
        let realised_row = match self
            .first_json(&format!(
                "SELECT COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_ytd_dkk \
                 FROM trade_ledger WHERE side = 'SELL' AND tax_year = {tax_year}"
            ))
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => json!({}),
            Err(err) => {
                warn!(
                    "after-tax estimate unavailable because the trade ledger could not be read: {err:#}"
                );
                return unavailable_after_tax_summary(
                    gross_unrealised_pnl_dkk,
                    tax_year,
                    "trade_ledger_unavailable",
                );
            }
        };
        let realised_gain_ytd_dkk = value_f64(&realised_row, "realised_gain_ytd_dkk");
        let Some(estimated_tax_dkk) = incremental_share_income_tax_dkk(
            realised_gain_ytd_dkk,
            gross_unrealised_pnl_dkk,
            &brackets,
        ) else {
            return unavailable_after_tax_summary(
                gross_unrealised_pnl_dkk,
                tax_year,
                "invalid_tax_inputs",
            );
        };
        json!({
            "status": "estimated",
            "tax_year": tax_year,
            "currency": "DKK",
            "gross_unrealised_pnl_dkk": gross_unrealised_pnl_dkk,
            "realised_gain_ytd_dkk": realised_gain_ytd_dkk,
            "estimated_tax_dkk": estimated_tax_dkk,
            "unrealised_pnl_after_tax_dkk": gross_unrealised_pnl_dkk - estimated_tax_dkk,
            "basis": "incremental_share_income_tax_on_realised_gain_plus_unrealised_pnl"
        })
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
            .select_json(&unreconciled_orders_sql(&stale_cutoff, &fill_cutoff))
            .await
            .unwrap_or_default();
        let adopted_orders_without_ledger = self
            .select_json(ADOPTED_ORDERS_WITHOUT_LEDGER_SQL)
            .await
            .ok()
            .and_then(|rows| rows.first().map(|row| value_f64(row, "count") as i64))
            .unwrap_or(0);
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

        // Config contract. Known unwired keys are reported as visible context
        // rather than as a warning: they are already documented in
        // wiki/urgent-todo.md and would otherwise hold the whole overview
        // permanently unhealthy, which trains the operator to ignore this panel.
        // A warning is reserved for genuine new drift -- a key added to config
        // without a contract entry, or an enforced key config stopped supplying
        // -- both of which are actionable and self-clearing.
        let (contract_summary, contract_findings) =
            crate::config_contract::audit_config(&self.config);
        let unused_risk_keys = contract_findings
            .iter()
            .filter(|finding| {
                finding.kind == crate::config_contract::FindingKind::UnusedKeyPresent
                    && finding.risk_surface
            })
            .map(|finding| json!({"key": finding.path, "note": finding.note}))
            .collect::<Vec<_>>();
        let drift_keys = contract_findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.kind,
                    crate::config_contract::FindingKind::UncontractedKey
                ) || (finding.kind == crate::config_contract::FindingKind::ContractedKeyMissing
                    && finding.risk_surface)
            })
            .map(|finding| {
                json!({
                    "key": finding.path,
                    "kind": finding.kind.as_str(),
                    "note": finding.note
                })
            })
            .collect::<Vec<_>>();
        let config_contract = json!({
            "enforced": contract_summary.enforced,
            "advisory": contract_summary.advisory,
            "unused": contract_summary.unused,
            "unused_risk_surface": contract_summary.unused_risk_surface,
            "uncontracted": contract_summary.uncontracted,
            "missing": contract_summary.missing,
            "unused_risk_keys": unused_risk_keys,
            "drift_keys": drift_keys
        });
        if drift_keys.is_empty() {
            checks.insert(
                "config_contract".to_string(),
                if contract_summary.unused_risk_surface > 0 {
                    json!("unused_risk_keys")
                } else {
                    json!("ok")
                },
            );
        } else {
            checks.insert("config_contract".to_string(), json!("warning"));
            let keys = drift_keys
                .iter()
                .map(|row| text_value(row, "key"))
                .collect::<Vec<_>>();
            warnings.push(json!({
                "code": "config_contract_drift",
                "severity": "warning",
                "message": "Configuration keys are not described by the config contract, or an enforced key is missing from configuration. Update CONTRACT in src/config_contract.rs so the key's real effect is recorded.",
                "count": drift_keys.len(),
                "keys": keys,
                "details": drift_keys.clone()
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
            "adopted_orders_without_ledger": adopted_orders_without_ledger,
            "expiry_pending_orders": expiry_pending_orders,
            "acknowledgements": acknowledgements
                .get("acknowledgements")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "acknowledged_issue_count": acknowledged_issue_count,
            "config_contract": config_contract,
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

    pub(crate) async fn protective_stop_coverage(&self) -> Result<JsonValue> {
        let positions = self
            .select_json(
                "SELECT symbol, updated_at, quantity, currency
                 FROM broker_position_snapshots
                 WHERE quantity > 0
                 ORDER BY symbol ASC",
            )
            .await?;
        let orders = self
            .select_json(
                "SELECT symbol, action, order_type, status, quantity, stop_price_local, broker_order_id
                 FROM execution_orders
                 WHERE action = 'SELL'
                 ORDER BY created_at DESC, id DESC",
            )
            .await?;
        let prechecks = self
            .select_json(
                "SELECT id, created_at, environment, symbol, quantity, stop_price_local, status, result_json
                 FROM protective_stop_prechecks
                 ORDER BY created_at DESC, id DESC
                 LIMIT 10",
            )
            .await?;
        let lifecycle_tests = self
            .select_json(
                "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                        stop_price_local, status, broker_order_id, external_reference, request_id,
                        placement_result_json, cancellation_result_json, reconciliation_json
                 FROM protective_stop_lifecycle_tests
                 ORDER BY created_at DESC, id DESC
                 LIMIT 10",
            )
            .await?;
        let active_lifecycle_tests = self
            .select_json(
                "SELECT id, updated_at, environment, symbol, quantity, stop_price_local, status, broker_order_id
                 FROM protective_stop_lifecycle_tests
                 WHERE environment = 'sim'
                   AND status = 'broker_working'
                   AND broker_order_id IS NOT NULL
                   AND broker_order_id <> ''
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 100",
            )
            .await?;
        // Latest stored close and ATR14 per symbol, so an unprotected position
        // can be shown with the concrete stop level it should carry. Bounded and
        // newest-first; `proposed_protective_stop` takes the first row per
        // symbol. No Saxo call is made and nothing is placed.
        let indicators = self
            .select_json(
                "SELECT symbol, run_date, close, atr14
                 FROM daily_indicator_signals
                 WHERE close IS NOT NULL AND atr14 IS NOT NULL
                 ORDER BY run_date DESC, id DESC
                 LIMIT 600",
            )
            .await
            .unwrap_or_default();
        let atr_multiple = yaml_f64(
            &self.config,
            &["strategy", "ladder", "stop_loss_atr_multiple"],
        )
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(DEFAULT_STOP_LOSS_ATR_MULTIPLE);
        let mut coverage = protective_stop_coverage_from_rows(
            &positions,
            &orders,
            &active_lifecycle_tests,
            &indicators,
            atr_multiple,
        );
        if let Some(object) = coverage.as_object_mut() {
            object.insert("recent_prechecks".to_string(), JsonValue::Array(prechecks));
            object.insert(
                "recent_lifecycle_tests".to_string(),
                JsonValue::Array(lifecycle_tests),
            );
        }
        Ok(coverage)
    }

    pub async fn record_protective_stop_precheck(
        &self,
        symbol: &str,
        quantity: f64,
        stop_price_local: f64,
        status: &str,
        result: &JsonValue,
    ) -> Result<i64> {
        let environment = yaml_string(&self.config, &["saxo", "environment"])
            .unwrap_or_else(|| "sim".to_string())
            .to_ascii_lowercase();
        sqlx::query(
            "INSERT INTO protective_stop_prechecks (
                created_at, environment, symbol, quantity, stop_price_local, status, result_json
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(environment)
        .bind(symbol.trim())
        .bind(quantity)
        .bind(stop_price_local)
        .bind(status)
        .bind(result.to_string())
        .execute(&self.pool)
        .await
        .context("recording sanitized protective-stop precheck")?;
        // AnyPool does not expose last-insert-id portably, so read the row back
        // by its exact identity instead of trusting a driver-specific handle.
        let id = self
            .first_json(&format!(
                "SELECT id FROM protective_stop_prechecks
                 WHERE symbol = '{}' AND status = '{}'
                 ORDER BY id DESC LIMIT 1",
                sql_escape(symbol.trim()),
                sql_escape(status)
            ))
            .await?
            .map(|row| value_i64(&row, "id"))
            .unwrap_or_default();
        Ok(id)
    }

    /// Lifecycle tests left in `placement_preparing` with no broker order id.
    ///
    /// Axum drops a handler future when the client disconnects, so a
    /// double-clicked placement can commit the prepared row and never reach the
    /// broker call. The orphan then counts as active forever and blocks every
    /// retry for that precheck. Observed 2026-07-25 on lifecycle test 1.
    ///
    /// These are *not* safe to expire on a timer: the future could equally have
    /// been dropped after a successful placement. The caller must reconcile each
    /// one against Saxo and only abandon those the broker does not know about.
    pub async fn stale_protective_stop_preparations(
        &self,
        older_than_seconds: i64,
    ) -> Result<Vec<JsonValue>> {
        let cutoff = (Utc::now() - Duration::seconds(older_than_seconds.max(0)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.select_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id
             FROM protective_stop_lifecycle_tests
             WHERE status = 'placement_preparing'
               AND (broker_order_id IS NULL OR broker_order_id = '')
               AND updated_at < '{}'
             ORDER BY id ASC
             LIMIT 20",
            sql_escape(&cutoff)
        ))
        .await
    }

    /// Placed stops still awaiting broker confirmation.
    ///
    /// `placement_submitted` is not coverage: the audit counts a stop only once
    /// Saxo reports it working. Left unconfirmed, the position keeps appearing
    /// as an exception and a later batch retries it -- which Saxo rejects with
    /// `SellOrdersAlreadyExistForOwnedContracts`, because the stop it does not
    /// know about is already resting. Observed 2026-07-25 across nine symbols.
    pub async fn unconfirmed_protective_stop_placements(
        &self,
        older_than_seconds: i64,
    ) -> Result<Vec<JsonValue>> {
        let cutoff = (Utc::now() - Duration::seconds(older_than_seconds.max(0)))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.select_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id
             FROM protective_stop_lifecycle_tests
             WHERE status = 'placement_submitted'
               AND broker_order_id IS NOT NULL
               AND broker_order_id <> ''
               AND updated_at < '{}'
             ORDER BY id ASC
             LIMIT 25",
            sql_escape(&cutoff)
        ))
        .await
    }

    /// Symbols with a protective-stop lifecycle test that is not in a terminal
    /// state. Saxo permits one resting sell per owned holding, so a batch must
    /// not attempt a second one even while local coverage still lags.
    pub async fn symbols_with_active_protective_stops(&self) -> Result<Vec<String>> {
        Ok(self
            .select_json(
                "SELECT DISTINCT symbol FROM protective_stop_lifecycle_tests
                 WHERE status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                                  'broker_state_unknown', 'broker_amended', 'reconciliation_pending')",
            )
            .await?
            .iter()
            .map(|row| text_value(row, "symbol").trim().to_ascii_uppercase())
            .filter(|symbol| !symbol.is_empty())
            .collect())
    }

    /// Marks the lifecycle-test row behind a protective stop as released once
    /// the stop has been cancelled at Saxo to make room for a decided sell.
    /// Keeping the two tables agreed matters: `symbols_with_active_protective_stops`
    /// reads this table, and a stale `broker_working` row there would stop the
    /// position from ever being re-protected after the sell.
    pub async fn release_protective_stop_lifecycle_test(
        &self,
        broker_order_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = 'broker_cancelled', cancellation_result_json = $2
             WHERE broker_order_id = $3 AND status <> 'broker_cancelled'",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(
            json!({
                "cancelled_by": "automatic_release_before_a_decided_sell",
                "verified": "broker_confirmed_not_working_before_the_sell_was_built",
                "safety": "cancellation_is_scoped_to_the_symbol_being_sold"
            })
            .to_string(),
        )
        .bind(broker_order_id)
        .execute(&self.pool)
        .await
        .context("releasing protective-stop lifecycle test after cancellation")?;
        Ok(())
    }

    /// Adopts every broker-confirmed protective stop into `execution_orders`.
    ///
    /// This is the load-bearing half of U1 slice 3. `sync_saxo_broker_orders`
    /// reads `execution_orders` and nothing else, so a stop living only in
    /// `protective_stop_lifecycle_tests` can fill at Saxo without producing a
    /// ledger row, a position update, or any Trading Manager awareness. Giving
    /// each stop an execution-order row inherits broker sync, fill
    /// reconciliation, the trade ledger, and execution notifications with no
    /// new plumbing.
    ///
    /// Adoption is idempotent on `broker_order_id` and creates no broker
    /// traffic: it records an order Saxo already holds. The lifecycle-test row
    /// stays as the placement audit trail.
    pub async fn adopt_protective_stops_into_execution_orders(&self) -> Result<Vec<JsonValue>> {
        let candidates = self
            .select_json(
                "SELECT t.id, t.created_at, t.symbol, t.quantity, t.stop_price_local,
                        t.broker_order_id, t.external_reference, t.request_id
                 FROM protective_stop_lifecycle_tests t
                 WHERE t.status = 'broker_working'
                   AND t.broker_order_id IS NOT NULL
                   AND t.broker_order_id <> ''
                   AND NOT EXISTS (
                         SELECT 1 FROM execution_orders o
                         WHERE o.broker_order_id = t.broker_order_id
                       )
                 ORDER BY t.id ASC
                 LIMIT 50",
            )
            .await?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mode = yaml_string(&self.config, &["execution", "mode"])
            .unwrap_or_else(|| "simulation".to_string());
        let adapter = yaml_string(&self.config, &["execution", "adapter"])
            .unwrap_or_else(|| "saxo".to_string());
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut adopted = Vec::new();
        for candidate in &candidates {
            let test_id = value_i64(candidate, "id");
            let symbol = text_value(candidate, "symbol").trim().to_string();
            let broker_order_id = text_value(candidate, "broker_order_id").trim().to_string();
            let quantity = value_f64(candidate, "quantity");
            let stop_price_local = value_f64(candidate, "stop_price_local");
            if symbol.is_empty() || broker_order_id.is_empty() || quantity <= 0.0 {
                continue;
            }
            let request_json = json!({
                "source": "protective_stop_adoption",
                "adopted_at": now,
                "lifecycle_test_id": test_id,
                "external_reference": text_value(candidate, "external_reference"),
                "request_id": text_value(candidate, "request_id"),
                "placed_at": text_value(candidate, "created_at"),
                "note": "Broker-confirmed protective stop adopted so broker sync, fill reconciliation, and the trade ledger cover it. No broker call was made by the adoption itself."
            });
            // `NOT EXISTS` above is checked outside a transaction, so a
            // concurrent scheduler pod could reach here with the same row. The
            // unique strategy key is the second guard: the insert is skipped
            // rather than duplicated.
            let strategy_key = format!("protective_stop:{test_id}");
            if self
                .select_json(&format!(
                    "SELECT id FROM execution_orders WHERE strategy_key = '{}' LIMIT 1",
                    sql_escape(&strategy_key)
                ))
                .await?
                .first()
                .is_some()
            {
                continue;
            }
            let inserted = sqlx::query(
                "INSERT INTO execution_orders (
                    created_at, symbol, action, order_type, mode, status, adapter,
                    quantity, stop_price_local, approval_required, approved_at,
                    strategy_type, strategy_key, strategy_role, broker_order_id, request_json
                 ) VALUES ($1, $2, 'SELL', 'stop', $3, 'broker_working', $4,
                           $5, $6, 0, $7, $8, $9, 'protective_stop', $10, $11)",
            )
            .bind(&now)
            .bind(&symbol)
            .bind(&mode)
            .bind(&adapter)
            .bind(quantity)
            .bind(stop_price_local)
            .bind(&now)
            .bind(PROTECTIVE_STOP_STRATEGY_TYPE)
            .bind(&strategy_key)
            .bind(&broker_order_id)
            .bind(serde_json::to_string(&request_json)?)
            .execute(&self.pool)
            .await
            .context("adopting protective stop into execution orders")?;
            if inserted.rows_affected() == 1 {
                adopted.push(json!({
                    "lifecycle_test_id": test_id,
                    "symbol": symbol,
                    "quantity": quantity,
                    "stop_price_local": stop_price_local,
                    "broker_order_id": broker_order_id,
                    "strategy_key": strategy_key
                }));
            }
        }
        Ok(adopted)
    }

    /// Marks a prepared lifecycle test abandoned after a reconcile confirmed the
    /// broker never saw it. `placement_abandoned` is deliberately absent from
    /// the active-status list, so the precheck becomes reusable.
    pub async fn abandon_protective_stop_preparation(&self, test_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = 'placement_abandoned', placement_result_json = $2
             WHERE id = $3 AND status = 'placement_preparing'
               AND (broker_order_id IS NULL OR broker_order_id = '')",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(
            json!({
                "placement": "not_sent",
                "abandoned_reason": "prepared_request_did_not_reach_the_broker",
                "verified_by": "saxo_open_order_and_audit_activity_lookup_found_nothing",
                "safety": "verified_absent_at_broker_before_abandoning_never_expired_on_a_timer"
            })
            .to_string(),
        )
        .bind(test_id)
        .execute(&self.pool)
        .await
        .context("abandoning unreached protective-stop preparation")?;
        Ok(())
    }

    pub async fn prepare_protective_stop_lifecycle_test(
        &self,
        source_precheck_id: i64,
    ) -> Result<JsonValue> {
        if source_precheck_id <= 0 {
            bail!("A successful SIM protective-stop precheck is required");
        }
        let source = self
            .first_json(&format!(
                "SELECT id, environment, symbol, quantity, stop_price_local, status
                 FROM protective_stop_prechecks WHERE id = {} LIMIT 1",
                source_precheck_id
            ))
            .await?
            .ok_or_else(|| {
                anyhow!("Protective-stop precheck {source_precheck_id} was not found")
            })?;
        if json_text(&source, "status") != "precheck_ok"
            || !json_text(&source, "environment").eq_ignore_ascii_case("sim")
        {
            bail!("Protective-stop lifecycle tests require a successful SIM precheck");
        }
        let active = self
            .first_json(&format!(
                "SELECT id FROM protective_stop_lifecycle_tests
                 WHERE source_precheck_id = {}
                   AND status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                                  'broker_state_unknown', 'cancellation_submitted', 'reconciliation_pending')
                 ORDER BY id DESC LIMIT 1",
                source_precheck_id
            ))
            .await?;
        if let Some(active) = active {
            bail!(
                "Protective-stop precheck {source_precheck_id} already has active lifecycle test {}",
                value_i64(&active, "id")
            );
        }
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let nonce = Utc::now().timestamp_micros();
        let request_id = format!("saxo-stop-test-{source_precheck_id}-{nonce}");
        let external_reference = format!("stop-test:{source_precheck_id}:{nonce}");
        let result = json!({
            "safety": "manual_sim_only_single_position_precheck_before_place_no_scheduler_or_hermes",
            "source_precheck_id": source_precheck_id,
            "placement": "not_sent"
        });
        sqlx::query(
            "INSERT INTO protective_stop_lifecycle_tests (
                created_at, updated_at, source_precheck_id, environment, symbol, quantity, stop_price_local,
                status, broker_order_id, external_reference, request_id, placement_result_json,
                cancellation_result_json, reconciliation_json
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10, $11, $12, $13)",
        )
        .bind(&now)
        .bind(&now)
        .bind(source_precheck_id)
        .bind("sim")
        .bind(json_text(&source, "symbol"))
        .bind(value_f64(&source, "quantity"))
        .bind(value_f64(&source, "stop_price_local"))
        .bind("placement_preparing")
        .bind(&external_reference)
        .bind(&request_id)
        .bind(result.to_string())
        .bind(json!({"status": "not_requested"}).to_string())
        .bind(json!({"status": "not_requested"}).to_string())
        .execute(&self.pool)
        .await
        .context("creating SIM protective-stop lifecycle test")?;
        self.protective_stop_lifecycle_test_by_request_id(&request_id)
            .await?
            .ok_or_else(|| anyhow!("Could not reload prepared protective-stop lifecycle test"))
    }

    pub async fn protective_stop_lifecycle_test(&self, test_id: i64) -> Result<Option<JsonValue>> {
        if test_id <= 0 {
            return Ok(None);
        }
        self.first_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id,
                    placement_result_json, cancellation_result_json, reconciliation_json
             FROM protective_stop_lifecycle_tests WHERE id = {} LIMIT 1",
            test_id
        ))
        .await
    }

    async fn protective_stop_lifecycle_test_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<JsonValue>> {
        self.first_json(&format!(
            "SELECT id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                    stop_price_local, status, broker_order_id, external_reference, request_id,
                    placement_result_json, cancellation_result_json, reconciliation_json
             FROM protective_stop_lifecycle_tests
             WHERE request_id = '{}' LIMIT 1",
            sql_escape(request_id)
        ))
        .await
    }

    pub async fn record_protective_stop_lifecycle_placement(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(
            test_id,
            status,
            broker_order_id,
            Some(result),
            None,
            None,
        )
        .await
    }

    pub async fn record_protective_stop_lifecycle_cancellation(
        &self,
        test_id: i64,
        status: &str,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(test_id, status, None, None, Some(result), None)
            .await
    }

    pub async fn record_protective_stop_lifecycle_reconciliation(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        result: &JsonValue,
    ) -> Result<()> {
        self.update_protective_stop_lifecycle_test(
            test_id,
            status,
            broker_order_id,
            None,
            None,
            Some(result),
        )
        .await
    }

    async fn update_protective_stop_lifecycle_test(
        &self,
        test_id: i64,
        status: &str,
        broker_order_id: Option<&str>,
        placement: Option<&JsonValue>,
        cancellation: Option<&JsonValue>,
        reconciliation: Option<&JsonValue>,
    ) -> Result<()> {
        let current = self
            .protective_stop_lifecycle_test(test_id)
            .await?
            .ok_or_else(|| anyhow!("Protective-stop lifecycle test {test_id} was not found"))?;
        let broker_order_id = broker_order_id.map(ToString::to_string).or_else(|| {
            let value = json_text(&current, "broker_order_id");
            (!value.is_empty()).then_some(value)
        });
        let placement_text = placement.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "placement_result_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let cancellation_text = cancellation.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "cancellation_result_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let reconciliation_text = reconciliation.map(ToString::to_string).unwrap_or_else(|| {
            let value = json_text(&current, "reconciliation_json");
            if value.is_empty() {
                "{}".to_string()
            } else {
                value
            }
        });
        let updated = sqlx::query(
            "UPDATE protective_stop_lifecycle_tests
             SET updated_at = $1, status = $2, broker_order_id = $3, placement_result_json = $4,
                 cancellation_result_json = $5, reconciliation_json = $6
             WHERE id = $7",
        )
        .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .bind(status)
        .bind(broker_order_id)
        .bind(placement_text)
        .bind(cancellation_text)
        .bind(reconciliation_text)
        .bind(test_id)
        .execute(&self.pool)
        .await
        .context("recording SIM protective-stop lifecycle test state")?;
        if updated.rows_affected() != 1 {
            bail!("Protective-stop lifecycle test {test_id} changed while updating");
        }
        Ok(())
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
        let holding_period_outcome = match self.execution_order_holding_period_outcome(order).await
        {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution holding-period attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let position_lifecycle = match self.execution_order_position_lifecycle(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution position-lifecycle attribution degraded: {err:#}"
                );
                JsonValue::Null
            }
        };
        let trade_thesis = match self.execution_order_trade_thesis(order).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    order_id = value_i64(order, "id"),
                    "execution trade-thesis attribution degraded: {err:#}"
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
            "holding_period_outcome": holding_period_outcome,
            "position_lifecycle": position_lifecycle,
            "trade_thesis": trade_thesis,
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

    async fn execution_order_holding_period_outcome(&self, order: &JsonValue) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        if order_id <= 0 {
            return Ok(JsonValue::Null);
        }
        let fill_summary = self
            .first_json(&format!(
                "SELECT MIN(created_at) AS first_fill_at,
                        COALESCE(SUM(delta_quantity), 0) AS filled_quantity,
                        CASE WHEN COALESCE(SUM(delta_quantity), 0) > 0
                             THEN SUM(delta_quantity * average_price_local) / SUM(delta_quantity)
                             ELSE 0 END AS average_fill_price_local,
                        MAX(currency) AS currency
                 FROM execution_fills
                 WHERE execution_order_id = {} AND delta_quantity > 0",
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let first_fill_at = json_text(&fill_summary, "first_fill_at");
        let fill_date = first_fill_at.chars().take(10).collect::<String>();
        if fill_date.len() != 10 {
            return Ok(JsonValue::Null);
        }
        let symbol = json_text(order, "symbol");
        if symbol.is_empty() {
            return Ok(JsonValue::Null);
        }
        // The next distinct daily closes are trading-session observations. A
        // weekend/holiday does not manufacture a synthetic one-day outcome.
        let subsequent_closes = self
            .select_json(&format!(
                "SELECT run_date, MAX(close) AS close
                 FROM daily_indicator_signals
                 WHERE symbol = '{}' AND status = 'ok' AND close > 0 AND run_date > '{}'
                 GROUP BY run_date
                 ORDER BY run_date ASC
                 LIMIT 5",
                sql_escape(&symbol),
                sql_escape(&fill_date)
            ))
            .await?;
        Ok(compact_holding_period_outcome(
            order,
            &fill_summary,
            &subsequent_closes,
        ))
    }

    async fn execution_order_position_lifecycle(&self, order: &JsonValue) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let symbol = json_text(order, "symbol");
        if order_id <= 0 || symbol.is_empty() {
            return Ok(JsonValue::Null);
        }
        let fills = self
            .select_json(&format!(
                "SELECT id, execution_order_id, created_at, side, delta_quantity
                 FROM execution_fills
                 WHERE symbol = '{}' AND delta_quantity > 0
                 ORDER BY created_at ASC, id ASC",
                sql_escape(&symbol)
            ))
            .await?;
        Ok(compact_execution_position_lifecycle(order, &fills))
    }

    async fn execution_order_trade_thesis(&self, order: &JsonValue) -> Result<JsonValue> {
        let order_id = value_i64(order, "id");
        let symbol = json_text(order, "symbol");
        let created_at = json_text(order, "created_at");
        if order_id <= 0 || symbol.is_empty() || created_at.is_empty() {
            return Ok(JsonValue::Null);
        }
        let thesis = self
            .first_json(&format!(
                "SELECT trade_thesis_json
                 FROM execution_orders
                 WHERE symbol = '{}' AND action = 'BUY'
                   AND trade_thesis_json IS NOT NULL
                   AND (created_at < '{}' OR (created_at = '{}' AND id <= {}))
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                sql_escape(&symbol),
                sql_escape(&created_at),
                sql_escape(&created_at),
                order_id
            ))
            .await?
            .unwrap_or(JsonValue::Null);
        let raw = thesis
            .get("trade_thesis_json")
            .cloned()
            .unwrap_or(JsonValue::Null);
        if raw.is_object() {
            return Ok(raw);
        }
        let Some(raw) = raw.as_str() else {
            return Ok(JsonValue::Null);
        };
        Ok(serde_json::from_str(raw).unwrap_or(JsonValue::Null))
    }

    async fn trade_thesis_outcome_evidence(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT id, created_at, symbol, action, quantity, currency, trade_thesis_json
                 FROM execution_orders
                 WHERE action = 'BUY'
                   AND trade_thesis_json IS NOT NULL
                 ORDER BY created_at DESC, id DESC
                 LIMIT {}",
                TRADE_THESIS_OUTCOME_EVIDENCE_LIMIT
            ))
            .await?;
        let mut outcomes = Vec::with_capacity(rows.len());
        for row in rows {
            let raw_thesis = row
                .get("trade_thesis_json")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let thesis = if raw_thesis.is_object() {
                raw_thesis
            } else {
                raw_thesis
                    .as_str()
                    .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                    .unwrap_or(JsonValue::Null)
            };
            if json_text(&thesis, "status") != "recorded" {
                continue;
            }
            outcomes.push(self.execution_order_holding_period_outcome(&row).await?);
        }
        Ok(trade_thesis_outcome_evidence_from_holding_outcomes(
            &outcomes,
        ))
    }

    async fn holding_thesis_reviews(&self) -> Result<JsonValue> {
        let positions = self.position_items(250).await?;
        let thesis_rows = self
            .select_json(&format!(
                "SELECT eo.id, eo.created_at, eo.symbol, eo.trade_thesis_json,
                        (SELECT MIN(f.created_at)
                         FROM execution_fills f
                         WHERE f.execution_order_id = eo.id
                           AND f.delta_quantity > 0) AS first_fill_at
                 FROM execution_orders eo
                 WHERE eo.action = 'BUY'
                   AND eo.trade_thesis_json IS NOT NULL
                 ORDER BY eo.created_at DESC, eo.id DESC
                 LIMIT {}",
                HOLDING_THESIS_REVIEW_LIMIT
            ))
            .await?;
        let stale_after_days = yaml_i64(
            &self.config,
            &["strategy", "swing", "position_decision_stale_after_days"],
        )
        .unwrap_or(DEFAULT_POSITION_DECISION_STALE_AFTER_DAYS)
        .max(1);
        Ok(compact_holding_thesis_reviews(
            &positions,
            &thesis_rows,
            stale_after_days,
            Utc::now(),
        ))
    }

    async fn decision_pulse_outcome_evidence(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT eo.id, eo.created_at, eo.report_id, eo.symbol, eo.action, eo.quantity,
                        eo.currency, eo.strategy_type, eo.strategy_key, eo.status AS execution_status,
                        dr.analysis_pulse_key, dr.analysis_pulse_label,
                        CASE WHEN EXISTS (
                            SELECT 1
                            FROM hermes_decision_advice h
                            WHERE h.decision_report_id = eo.report_id
                        ) THEN 1 ELSE 0 END AS hermes_reviewed,
                        (
                            SELECT tm.manager_json
                            FROM trading_manager_runs tm
                            WHERE tm.report_id = eo.report_id
                            ORDER BY tm.created_at DESC, tm.id DESC
                            LIMIT 1
                        ) AS manager_json
                 FROM execution_orders eo
                 LEFT JOIN decision_reports dr ON dr.id = eo.report_id
                 WHERE eo.action IN ('BUY', 'SELL')
                   AND (eo.report_id IS NOT NULL OR eo.strategy_type = 'portfolio_sync')
                 ORDER BY eo.created_at DESC, eo.id DESC
                 LIMIT {}",
                DECISION_PULSE_OUTCOME_EVIDENCE_LIMIT
            ))
            .await?;
        let mut observations = Vec::with_capacity(rows.len());
        for row in rows {
            let action = json_text(&row, "action").to_uppercase();
            let raw_manager_json = row.get("manager_json").cloned().unwrap_or(JsonValue::Null);
            let manager_json = if raw_manager_json.is_object() {
                raw_manager_json
            } else {
                raw_manager_json
                    .as_str()
                    .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                    .unwrap_or(JsonValue::Null)
            };
            let hermes_effect = manager_json
                .get("hermes_advice_delta")
                .and_then(|value| value.get("candidates"))
                .and_then(|value| {
                    matching_order_advice(
                        Some(value),
                        &json_text(&row, "strategy_key"),
                        &json_text(&row, "symbol"),
                        &action,
                    )
                })
                .map(|value| json_text(&value, "effect"))
                .unwrap_or_else(|| "not_recorded".to_string());
            let holding_period_outcome = if action == "BUY" {
                self.execution_order_holding_period_outcome(&row).await?
            } else {
                JsonValue::Null
            };
            let ledger_outcome = if action == "SELL" {
                self.execution_order_ledger_outcome(&row).await?
            } else {
                JsonValue::Null
            };
            observations.push(json!({
                "analysis_pulse_key": json_text(&row, "analysis_pulse_key"),
                "analysis_pulse_label": json_text(&row, "analysis_pulse_label"),
                "strategy_type": json_text(&row, "strategy_type"),
                "action": action,
                "execution_status": json_text(&row, "execution_status"),
                "hermes_reviewed": value_i64(&row, "hermes_reviewed") > 0,
                "hermes_effect": hermes_effect,
                "holding_period_outcome": holding_period_outcome,
                "ledger_outcome": ledger_outcome,
            }));
        }
        Ok(decision_pulse_outcome_evidence_from_observations(
            &observations,
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
                "enabled": if key == "manual" {
                    true
                } else {
                    crate::xai_decision::scheduled_pulse_is_enabled(self, key)
                },
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
        hermes_goal_contract_from_config(&self.config)
    }
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
                "detail": "No cost model exists, so slippage is never estimated before queueing. Tracked with strategy.estimated_slippage_bps and strategy.cost_guard_multiple in the config contract audit."
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
fn hermes_goal_contract_from_config(config: &YamlValue) -> JsonValue {
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

impl AppState {
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
                // Every entry must be a key the runtime actually reads.
                // `strategy.swing.cash_buffer_pct` was removed on 2026-07-25:
                // the config-contract audit proved nothing reads it, so an
                // experiment on it could be proposed, run in SIM, observed, and
                // promoted while changing nothing at all.
                "variables": SUPPORTED_EXPERIMENT_VARIABLES
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
                "Public editorial-research items are attributable secondary context only and do not place, approve, block, size, or otherwise modify orders.",
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
                "fills": execution_fills,
                "protective_stop_coverage": protective_stop_coverage,
                "holding_thesis_reviews": holding_thesis_reviews
            },
            "performance": {
                "range": "1M",
                "history": performance
            },
            "markov_method": markov,
            "quiver_signals": quiver,
            "quiver_conflicts": quiver_conflicts,
            "editorial_research": editorial_research,
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
        let mut unpriced = 0usize;
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
            let currency = json_text(candidate, "currency");
            let id = format!("missed-trade-shadow-{manager_run_id}-{index}");
            let result = sqlx::query(&format!(
                "INSERT INTO missed_trade_shadows (
                    id, created_at, updated_at, report_id, manager_run_id,
                    strategy_key, symbol, action, source_gate, shadow_quantity,
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
                sql_escape(&gate_code),
                shadow_quantity,
                reference_sql,
                sql_optional_text(Some(&currency)),
                status,
            ))
            .execute(&self.pool)
            .await
            .context("recording missed-trade shadow")?;
            created += result.rows_affected() as usize;
        }

        Ok(json!({
            "status": "ok",
            "created": created,
            "unpriced": unpriced,
            "skipped": skipped,
            "safety": "quote_to_quote_observation_only_no_gate_or_order_mutation",
        }))
    }

    pub async fn missed_trade_shadows(&self, limit: i64) -> Result<Vec<JsonValue>> {
        let sql = format!(
            "SELECT id, created_at, updated_at, report_id, manager_run_id, strategy_key,
                    symbol, action, source_gate, shadow_quantity, reference_price_local,
                    currency, status, latest_price_local, latest_price_at,
                    estimated_return_pct, estimated_pnl_local, observation_count
             FROM missed_trade_shadows
             ORDER BY created_at DESC, id DESC
             LIMIT {}",
            clamp_limit(limit, 1, MISSED_TRADE_SHADOW_LIMIT)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    async fn missed_trade_shadow_outcome_evidence(&self) -> Result<JsonValue> {
        let rows = self
            .select_json(&format!(
                "SELECT source_gate, estimated_return_pct
                 FROM missed_trade_shadows
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
                "SELECT id, action, shadow_quantity, reference_price_local
                 FROM missed_trade_shadows
                 WHERE symbol = '{}' AND status = 'tracking' AND reference_price_local > 0",
                sql_escape(symbol)
            ))
            .await?;
        let mut updated = 0usize;
        for row in rows {
            let id = json_text(&row, "id");
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
        } else if let Some(latest) = history.last_mut() {
            // Preserve the chart shape but expose the source and timestamp of
            // the aggregate that was actually read for this response.
            *latest = current;
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

    /// Portfolio value snapshots for the drawdown guardrail's trailing window,
    /// oldest first, with the live position aggregate appended.
    ///
    /// The live row matters: the stored snapshots are written on a schedule, so
    /// on their own they can be hours stale, and a guardrail that reacts hours
    /// late to a decline is not much of a guardrail. `observe_drawdown` filters
    /// out a non-positive aggregate, so a batch that has not loaded yet is
    /// dropped rather than read as a collapse to zero.
    pub(crate) async fn portfolio_drawdown_history(
        &self,
        lookback_days: i64,
    ) -> Result<Vec<JsonValue>> {
        let mut rows = self.portfolio_drawdown_window(lookback_days).await?;
        rows.push(self.current_performance_row().await?);
        Ok(rows)
    }

    /// Snapshot rows for the drawdown window, without the live current row.
    /// Split out so the window bounds can be tested on their own.
    async fn portfolio_drawdown_window(&self, lookback_days: i64) -> Result<Vec<JsonValue>> {
        let lookback_day = (Utc::now() - Duration::days(lookback_days.clamp(1, 3_650)))
            .format("%Y-%m-%d")
            .to_string();
        // Never look back past a re-baselining. Dates are compared on the
        // leading YYYY-MM-DD because `recorded_at` and `created_at` carry a
        // mix of `Z` and `+00:00` offsets, which do not order consistently as
        // whole strings.
        let start_day = match self.latest_external_cash_flow_day().await? {
            Some(flow_day) if flow_day >= lookback_day => flow_day,
            _ => lookback_day,
        };
        let sql = format!(
            "SELECT recorded_at, total_market_value_dkk FROM portfolio_value_history \
             WHERE SUBSTR(recorded_at, 1, 10) > '{}' \
             ORDER BY recorded_at ASC, id ASC LIMIT 20000",
            sql_escape(&start_day)
        );
        Ok(self.select_json(&sql).await.unwrap_or_default())
    }

    /// The day of the most recent deposit, withdrawal, or reconciliation
    /// adjustment, if any.
    ///
    /// These rows mark the moments the portfolio value stopped being
    /// comparable to what came before, so they bound how far back a drawdown
    /// peak may reach. The whole day is excluded rather than the instant: an
    /// adjustment is usually settled against snapshots taken around it, and
    /// half a re-baselined day is not a value worth defending a peak with.
    async fn latest_external_cash_flow_day(&self) -> Result<Option<String>> {
        let row = self
            .first_json(
                "SELECT MAX(SUBSTR(created_at, 1, 10)) AS flow_day FROM trade_ledger \
                 WHERE side IN ('DEPOSIT', 'WITHDRAWAL', 'ADJUSTMENT')",
            )
            .await?;
        Ok(row
            .as_ref()
            .map(|row| json_text(row, "flow_day"))
            .filter(|day| day.len() == 10))
    }

    /// The saved drawdown-guardrail override, as stored. Whether it still
    /// applies is decided in `drawdown_guard` against the peak actually being
    /// measured, so the expiry rule lives with the rule it modifies.
    pub async fn drawdown_guard_override_value(&self) -> Result<JsonValue> {
        let saved = self.runtime_setting("drawdown_guard_override").await?;
        let mut value = json!({
            "enabled": false,
            "peak_value_dkk": null,
            "notes": "",
            "updated_at": null
        });
        if let Some(saved) = saved
            && let Some(object) = value.as_object_mut()
        {
            for key in ["enabled", "peak_value_dkk", "notes", "updated_at"] {
                if let Some(entry) = saved.get(key) {
                    object.insert(key.to_string(), entry.clone());
                }
            }
        }
        Ok(value)
    }

    /// Grant or clear the drawdown override. The peak it is granted against is
    /// recorded so the grant can expire by itself; without one the override is
    /// refused rather than becoming permanent.
    pub async fn save_drawdown_guard_override(
        &self,
        enabled: bool,
        peak_value_dkk: Option<f64>,
        notes: &str,
    ) -> Result<JsonValue> {
        if notes.len() > 500 {
            anyhow::bail!("Drawdown guardrail override notes are too long");
        }
        let peak_value_dkk = match (enabled, peak_value_dkk) {
            (true, Some(peak)) if peak.is_finite() && peak > 0.0 => Some(peak),
            (true, _) => anyhow::bail!(
                "Enabling the drawdown guardrail override requires the peak value it is granted against"
            ),
            (false, _) => None,
        };
        let value = json!({
            "enabled": enabled,
            "peak_value_dkk": peak_value_dkk,
            "notes": notes,
            "updated_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });
        self.save_runtime_setting("drawdown_guard_override", &value)
            .await?;
        self.drawdown_guard_override_value().await
    }

    pub(crate) async fn current_performance_row(&self) -> Result<JsonValue> {
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
        let since_reset_baseline = self
            .portfolio_value_since_reset(batch_id.as_deref())
            .await
            .unwrap_or(None);
        json!({
            "weekly_target_dkk": weekly_target,
            "monthly_target_dkk": monthly_target,
            "basis": "pnl_dkk is total portfolio value change since the period start, measured against the portfolio value history baseline.",
            "periods": {
                "week": goal_period_value(week_baseline, total_value, weekly_target, &week_start_utc),
                "month": goal_period_value(month_baseline, total_value, monthly_target, &month_start_utc),
                "since_reset": since_reset_performance_value(since_reset_baseline, total_value)
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

    /// The earliest persisted account-value snapshot in the active import
    /// batch. A missing row is intentionally not substituted with zero or a
    /// value from an earlier reset because neither is a comparable baseline.
    async fn portfolio_value_since_reset(
        &self,
        batch_id: Option<&str>,
    ) -> Result<Option<JsonValue>> {
        let Some(batch_id) = batch_id else {
            return Ok(None);
        };
        self.first_json(&format!(
            "SELECT recorded_at, total_market_value_dkk FROM portfolio_value_history \
             WHERE batch_id = '{}' ORDER BY recorded_at ASC, id ASC LIMIT 1",
            sql_escape(batch_id)
        ))
        .await
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
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_prechecks (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    result_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop prechecks table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_prechecks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    result_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop prechecks table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_protective_stop_prechecks_created
             ON protective_stop_prechecks(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating protective-stop prechecks index")?;
        if self.db_url.starts_with("postgres://") || self.db_url.starts_with("postgresql://") {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_lifecycle_tests (
                    id BIGSERIAL PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    source_precheck_id BIGINT NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    broker_order_id TEXT,
                    external_reference TEXT NOT NULL,
                    request_id TEXT NOT NULL UNIQUE,
                    placement_result_json TEXT NOT NULL,
                    cancellation_result_json TEXT NOT NULL,
                    reconciliation_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop lifecycle tests table")?;
        } else {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS protective_stop_lifecycle_tests (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    source_precheck_id INTEGER NOT NULL,
                    environment TEXT NOT NULL,
                    symbol TEXT NOT NULL,
                    quantity REAL NOT NULL,
                    stop_price_local REAL NOT NULL,
                    status TEXT NOT NULL,
                    broker_order_id TEXT,
                    external_reference TEXT NOT NULL,
                    request_id TEXT NOT NULL UNIQUE,
                    placement_result_json TEXT NOT NULL,
                    cancellation_result_json TEXT NOT NULL,
                    reconciliation_json TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await
            .context("creating protective-stop lifecycle tests table")?;
        }
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_protective_stop_lifecycle_tests_created
             ON protective_stop_lifecycle_tests(created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating protective-stop lifecycle tests index")?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_protective_stop_lifecycle_tests_active_source
             ON protective_stop_lifecycle_tests(source_precheck_id)
             WHERE status IN ('placement_preparing', 'placement_submitted', 'broker_working',
                              'broker_state_unknown', 'cancellation_submitted', 'reconciliation_pending')",
        )
        .execute(&self.pool)
        .await
        .context("creating active protective-stop lifecycle test uniqueness guard")?;
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
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS missed_trade_shadows (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_gate TEXT NOT NULL,
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
        .context("creating missed-trade shadows table")?;
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
        for sql in crate::editorial_research::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating editorial research runtime tables")?;
        }
        for sql in crate::daily_indicators::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating daily indicator runtime tables")?;
        }
        for sql in crate::performance_benchmarks::create_schema_sql() {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .context("creating performance benchmark runtime tables")?;
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
        self.ensure_table_column("execution_orders", "trade_thesis_json TEXT")
            .await
            .context("migrating execution-order trade-thesis provenance")?;
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
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_missed_trade_shadows_manager_strategy
             ON missed_trade_shadows(manager_run_id, strategy_key)",
        )
        .execute(&self.pool)
        .await
        .context("creating missed-trade shadow manager strategy index")?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_missed_trade_shadows_tracking
             ON missed_trade_shadows(status, symbol, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .context("creating missed-trade shadow tracking index")?;
        self.backfill_trading_manager_strategy_type().await?;
        Ok(())
    }

    /// Backfill `strategy_type` on Trading Manager orders queued before the
    /// runtime started setting it (2026-05-12 to 2026-07-25).
    ///
    /// `report_id IS NOT NULL` is the exact discriminator: on 2026-07-25 all
    /// 101 unset rows carried a report id, and every row with another
    /// `strategy_type` -- `portfolio_sync`, `clean_reconciliation`, `manual` --
    /// carried none, because those come from adoption and manual paths that do
    /// not originate in a decision report. The update is idempotent and cannot
    /// overwrite a value another path already set.
    async fn backfill_trading_manager_strategy_type(&self) -> Result<()> {
        let result = sqlx::query(&format!(
            "UPDATE execution_orders SET strategy_type = '{}' \
             WHERE strategy_type IS NULL AND report_id IS NOT NULL",
            sql_escape(crate::trading_manager::TRADING_MANAGER_STRATEGY_TYPE)
        ))
        .execute(&self.pool)
        .await
        .context("backfilling Trading Manager execution-order strategy type")?;
        if result.rows_affected() > 0 {
            info!(
                rows = result.rows_affected(),
                strategy_type = crate::trading_manager::TRADING_MANAGER_STRATEGY_TYPE,
                "backfilled execution-order strategy type"
            );
        }
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

    /// The ENS activity backfill is a read-only daily reconciliation aid. Keep
    /// its scheduler cursor in the database so a rollout does not turn one
    /// intended broker read into a request on every scheduler heartbeat.
    pub(crate) async fn ens_activity_backfill_completed_date(&self) -> Result<Option<String>> {
        Ok(self
            .runtime_setting("ens_activity_backfill")
            .await?
            .and_then(|value| {
                value
                    .get("completed_date")
                    .and_then(JsonValue::as_str)
                    .map(ToString::to_string)
            }))
    }

    pub(crate) async fn record_ens_activity_backfill(
        &self,
        completed_date: &str,
        summary: &JsonValue,
    ) -> Result<()> {
        self.save_runtime_setting(
            "ens_activity_backfill",
            &json!({
                "completed_date": completed_date,
                "completed_at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "summary": summary,
            }),
        )
        .await
    }

    pub(crate) fn market_exchange_rows(&self) -> Vec<JsonValue> {
        let cache = current_saxo_exchange_calendar_cache();
        market_exchange_rows_for_config(&self.config, Utc::now(), cache.as_ref())
    }

    async fn first_json(&self, sql: &str) -> Result<Option<JsonValue>> {
        let row = sqlx::query(sql).fetch_optional(&self.pool).await?;
        Ok(row.map(|row| row_to_json(&row)))
    }

    pub(crate) async fn select_json(&self, sql: &str) -> Result<Vec<JsonValue>> {
        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(row_to_json).collect())
    }
}

fn goal_period_value(
    baseline: Option<f64>,
    total_value: f64,
    target: f64,
    period_start: &str,
) -> JsonValue {
    let baseline = baseline.filter(|value| value.is_finite());
    if let Some(baseline_value) = baseline {
        let pnl = total_value - baseline_value;
        return json!({
            "status": "ready",
            "pnl_dkk": pnl,
            "target_dkk": target,
            "progress_pct": pct(pnl, target),
            "baseline_value_dkk": baseline_value,
            "period_start_utc": period_start,
        });
    }

    json!({
        "status": "pending_baseline",
        "pnl_dkk": JsonValue::Null,
        "target_dkk": target,
        "progress_pct": JsonValue::Null,
        "baseline_value_dkk": JsonValue::Null,
        "period_start_utc": period_start,
    })
}

fn since_reset_performance_value(baseline: Option<JsonValue>, total_value: f64) -> JsonValue {
    let baseline_at = baseline
        .as_ref()
        .and_then(|row| row.get("recorded_at"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let baseline_value = baseline
        .as_ref()
        .and_then(|row| row.get("total_market_value_dkk"))
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    if let (Some(baseline_at), Some(baseline_value)) = (baseline_at, baseline_value)
        && total_value.is_finite()
        && total_value > 0.0
    {
        let pnl = total_value - baseline_value;
        return json!({
            "status": "ready",
            "pnl_dkk": pnl,
            "return_pct": pct(pnl, baseline_value) * 100.0,
            "baseline_value_dkk": baseline_value,
            "baseline_recorded_at": baseline_at,
        });
    }

    json!({
        "status": "pending_baseline",
        "pnl_dkk": JsonValue::Null,
        "return_pct": JsonValue::Null,
        "baseline_value_dkk": JsonValue::Null,
        "baseline_recorded_at": JsonValue::Null,
    })
}

fn performance_range_metrics(history: &[JsonValue]) -> (Option<f64>, Option<f64>) {
    let values = history
        .iter()
        .filter_map(|row| {
            row.get("total_market_value_dkk")
                .and_then(JsonValue::as_f64)
        })
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect::<Vec<_>>();
    let Some(start_value) = values.first().copied() else {
        return (None, None);
    };
    let Some(end_value) = values.last().copied() else {
        return (None, None);
    };
    if values.len() < 2 {
        return (None, None);
    }

    let mut peak = start_value;
    let mut max_drawdown_pct = 0.0_f64;
    for value in values {
        peak = peak.max(value);
        max_drawdown_pct = max_drawdown_pct.min((value / peak - 1.0) * 100.0);
    }
    (
        Some((end_value / start_value - 1.0) * 100.0),
        Some(max_drawdown_pct),
    )
}

/// Describes the evidence behind the account-value display without making any
/// claim about individual quote, benchmark, or broker-order freshness.
fn performance_confidence(history: &[JsonValue], now: DateTime<Utc>) -> JsonValue {
    let latest = history.last();
    let valid_points = history
        .iter()
        .filter(|row| {
            row.get("total_market_value_dkk")
                .and_then(JsonValue::as_f64)
                .is_some_and(|value| value.is_finite() && value > 0.0)
        })
        .count();
    let latest_value_valid = latest
        .and_then(|row| row.get("total_market_value_dkk"))
        .and_then(JsonValue::as_f64)
        .is_some_and(|value| value.is_finite() && value > 0.0);
    let latest_recorded_at = latest
        .and_then(|row| row.get("recorded_at"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let latest_snapshot_type = latest
        .map(|row| text_value(row, "snapshot_type"))
        .filter(|value| !value.is_empty());
    let latest_source = latest
        .map(|row| text_value(row, "source"))
        .filter(|value| !value.is_empty());
    let age_minutes = latest_recorded_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            now.signed_duration_since(value.with_timezone(&Utc))
                .num_minutes()
                .max(0)
        });
    let status = if !latest_value_valid {
        "unavailable"
    } else if valid_points < 2 {
        "partial"
    } else if latest_snapshot_type.as_deref() == Some("runtime_current") {
        "current"
    } else if age_minutes.is_none_or(|minutes| minutes > 90) {
        "stale"
    } else {
        "stored"
    };
    json!({
        "status": status,
        "valid_points": valid_points,
        "latest_recorded_at": latest_recorded_at,
        "latest_snapshot_type": latest_snapshot_type,
        "latest_source": latest_source,
        "age_minutes": age_minutes,
        "scope": "account_value_only",
    })
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

/// Adopted broker positions are bookkeeping for holdings that already existed
/// at the broker when this system took over the book. No trade happened under
/// this system, so they can never acquire a trade-ledger row.
///
/// Counting them as unreconciled held overview `healthy` false continuously
/// from the 2026-05-05 adoption onward, which trains an operator to read this
/// warning as background noise -- exactly how a genuine ledger-less fill would
/// go unnoticed. They are reported as a separate count instead.
const ADOPTED_ORDER_EXCLUSION: &str = "COALESCE(strategy_type, '') <> 'portfolio_sync'";

/// Execution orders that are genuinely stuck or missing accounting. Shared by
/// `overview_integrity` and its regression test so the two cannot drift.
fn unreconciled_orders_sql(stale_cutoff: &str, fill_cutoff: &str) -> String {
    format!(
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
                AND {} \
                AND created_at < '{}') \
            OR (status = 'executed' \
                AND ledger_id IS NULL \
                AND {} \
                AND created_at < '{}') \
         ORDER BY created_at ASC, id ASC \
         LIMIT 20",
        RESTING_PROTECTIVE_STOP_EXCLUSION,
        sql_escape(stale_cutoff),
        ADOPTED_ORDER_EXCLUSION,
        sql_escape(fill_cutoff)
    )
}

/// A protective stop is GoodTillCancel and is *supposed* to rest at
/// `broker_working` for as long as the position is held, so age alone says
/// nothing about it. Without this exclusion every adopted stop becomes a
/// permanent integrity warning 24 hours after placement, which is the same
/// false positive that adopted positions produced before 2026-07-25 -- and a
/// panel that is always warning is a panel nobody reads. The
/// `broker_state_unknown` and executed-without-a-ledger-row branches
/// deliberately still apply: those are real faults for a stop too.
const RESTING_PROTECTIVE_STOP_EXCLUSION: &str = "COALESCE(strategy_type, '') <> 'protective_stop'";

const ADOPTED_ORDERS_WITHOUT_LEDGER_SQL: &str = "SELECT COUNT(*) AS count FROM execution_orders \
     WHERE status = 'executed' \
       AND ledger_id IS NULL \
       AND COALESCE(strategy_type, '') = 'portfolio_sync'";

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
        // Scope by the exact key set so acknowledging today's drift cannot
        // suppress tomorrow's newly uncontracted key.
        "config_contract_drift" => issue
            .get("keys")
            .and_then(JsonValue::as_array)
            .map(|keys| {
                let mut keys = keys
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                keys.sort();
                keys.join(",")
            })
            .filter(|scope| !scope.is_empty())
            .unwrap_or_else(|| "config-contract".to_string()),
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

/// The exchange suffix of a `BASE:exchange` symbol, for callers outside this
/// module.
pub(crate) fn exchange_code_for(symbol: &str) -> String {
    exchange_code(symbol)
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
    fn goal_period_value_marks_missing_baseline_pending_instead_of_zero_progress() {
        let missing = goal_period_value(None, 250_000.0, 880.0, "2026-07-27T00:00:00Z");
        assert_eq!(missing["status"], json!("pending_baseline"));
        assert!(missing["pnl_dkk"].is_null());
        assert!(missing["progress_pct"].is_null());

        let ready = goal_period_value(Some(249_000.0), 250_000.0, 880.0, "2026-07-27T00:00:00Z");
        assert_eq!(ready["status"], json!("ready"));
        assert_eq!(ready["pnl_dkk"], json!(1_000.0));
        assert!(
            (ready["progress_pct"].as_f64().unwrap_or_default() - (1_000.0 / 880.0)).abs() < 1e-12
        );
    }

    #[test]
    fn since_reset_performance_requires_an_active_batch_baseline() {
        let missing = since_reset_performance_value(None, 250_000.0);
        assert_eq!(missing["status"], json!("pending_baseline"));
        assert!(missing["pnl_dkk"].is_null());
        assert!(missing["return_pct"].is_null());

        let ready = since_reset_performance_value(
            Some(json!({
                "recorded_at": "2026-07-01T08:00:00Z",
                "total_market_value_dkk": 240_000.0,
            })),
            250_000.0,
        );
        assert_eq!(ready["status"], json!("ready"));
        assert_eq!(ready["pnl_dkk"], json!(10_000.0));
        assert!((ready["return_pct"].as_f64().unwrap_or_default() - 4.166_666_666_7).abs() < 1e-8);
    }

    #[test]
    fn performance_range_metrics_requires_history_and_measures_peak_to_trough_loss() {
        assert_eq!(performance_range_metrics(&[]), (None, None));
        assert_eq!(
            performance_range_metrics(&[json!({"total_market_value_dkk": 100.0})]),
            (None, None)
        );

        let (return_pct, drawdown_pct) = performance_range_metrics(&[
            json!({"total_market_value_dkk": 100.0}),
            json!({"total_market_value_dkk": 120.0}),
            json!({"total_market_value_dkk": 90.0}),
            json!({"total_market_value_dkk": 110.0}),
        ]);
        assert!((return_pct.unwrap_or_default() - 10.0).abs() < 1e-9);
        assert!((drawdown_pct.unwrap_or_default() + 25.0).abs() < 1e-9);
    }

    #[test]
    fn performance_confidence_distinguishes_current_partial_stale_and_unavailable_evidence() {
        let now = DateTime::parse_from_rfc3339("2026-07-30T12:00:00Z")
            .expect("parses fixed timestamp")
            .with_timezone(&Utc);
        let current = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-29T12:00:00Z", "total_market_value_dkk": 250_000.0}),
                json!({"recorded_at": "2026-07-30T12:00:00Z", "snapshot_type": "runtime_current", "source": "saxo_broker_snapshot", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(current["status"], json!("current"));
        assert_eq!(current["valid_points"], json!(2));

        let partial = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-30T12:00:00Z", "snapshot_type": "runtime_current", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(partial["status"], json!("partial"));

        let stale = performance_confidence(
            &[
                json!({"recorded_at": "2026-07-28T12:00:00Z", "total_market_value_dkk": 250_000.0}),
                json!({"recorded_at": "2026-07-30T10:00:00Z", "snapshot_type": "daily_close", "total_market_value_dkk": 251_000.0}),
            ],
            now,
        );
        assert_eq!(stale["status"], json!("stale"));

        let unavailable = performance_confidence(
            &[json!({"snapshot_type": "runtime_current", "total_market_value_dkk": 0.0})],
            now,
        );
        assert_eq!(unavailable["status"], json!("unavailable"));
    }

    #[test]
    fn share_income_tax_estimate_applies_progressive_brackets_incrementally() {
        let config: YamlValue = serde_yaml::from_str(
            "taxation:\n  share_income:\n    brackets:\n      - up_to_dkk: 79000\n        rate: 0.27\n      - up_to_dkk:\n        rate: 0.42\n",
        )
        .expect("parses tax configuration");
        let brackets = share_income_tax_brackets(&config).expect("valid tax brackets");

        let tax = share_income_tax_due_dkk(100_000.0, &brackets).expect("tax estimate");
        assert!((tax - 30_150.0).abs() < 1e-9);

        let incremental = incremental_share_income_tax_dkk(70_000.0, 20_000.0, &brackets)
            .expect("incremental tax estimate");
        assert!((incremental - 7_050.0).abs() < 1e-9);

        let loss_offset = incremental_share_income_tax_dkk(90_000.0, -20_000.0, &brackets)
            .expect("loss offset estimate");
        assert!((loss_offset + 7_050.0).abs() < 1e-9);
    }

    #[test]
    fn share_income_tax_rejects_malformed_or_nonterminal_open_brackets() {
        let malformed: YamlValue = serde_yaml::from_str(
            "taxation:\n  share_income:\n    brackets:\n      - up_to_dkk:\n        rate: 0.27\n      - up_to_dkk: 79000\n        rate: 0.42\n",
        )
        .expect("parses malformed tax configuration");
        assert!(share_income_tax_brackets(&malformed).is_none());
    }

    fn goal_contract_field_paths(contract: &JsonValue) -> Vec<String> {
        ["objective", "constraints"]
            .iter()
            .flat_map(|section| {
                contract
                    .get(section)
                    .and_then(JsonValue::as_object)
                    .map(|fields| {
                        fields
                            .keys()
                            .map(|key| format!("{section}.{key}"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The mechanism behind U3. The goal contract used to advertise a risk
    /// envelope the runtime did not implement, and nothing made that visible.
    /// Adding a field without declaring how the runtime treats it now fails the
    /// build, so the contract cannot quietly start claiming things again.
    #[test]
    fn hermes_goal_contract_declares_enforcement_for_every_field() {
        let contract = hermes_goal_contract_from_config(&YamlValue::Null);
        let enforcement = contract
            .get("enforcement")
            .and_then(JsonValue::as_object)
            .expect("contract carries an enforcement record");

        for path in goal_contract_field_paths(&contract) {
            let entry = enforcement
                .get(&path)
                .unwrap_or_else(|| panic!("{path} has no enforcement entry"));
            let status = entry
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            assert!(
                matches!(
                    status,
                    "runtime_enforced"
                        | "evaluation_only"
                        | "structural"
                        | "documentation"
                        | "not_enforced"
                ),
                "{path} declares unknown enforcement status {status:?}"
            );
            assert!(
                !entry
                    .get("detail")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .trim()
                    .is_empty(),
                "{path} declares a status with no explanation"
            );
        }

        // The reverse direction: a stale entry for a field that no longer
        // exists would misrepresent the contract just as badly.
        let declared_fields = goal_contract_field_paths(&contract);
        for key in enforcement.keys().filter(|key| key.as_str() != "note") {
            assert!(
                declared_fields.contains(key),
                "enforcement declares {key}, which is not a contract field"
            );
        }
    }

    /// The drawdown limit Hermes is told about must be the one a gate applies,
    /// otherwise U3 has only moved the dishonesty behind a config key.
    #[test]
    fn hermes_goal_contract_publishes_the_enforced_drawdown_limit() {
        let config: YamlValue =
            serde_yaml::from_str("strategy:\n  capital:\n    drawdown_halt_pct: 0.15\n")
                .expect("parses");
        let contract = hermes_goal_contract_from_config(&config);
        let enforced = crate::drawdown_guard::DrawdownPolicy::from_config(&config).halt_pct;

        assert_eq!(contract["objective"]["max_drawdown"], json!(enforced));
        assert_eq!(
            contract["experiment_policy"]["promote_only_if"]["drawdown_lte"],
            json!(enforced)
        );
        assert_eq!(
            contract["experiment_policy"]["rollback_if"]["drawdown_gt"],
            json!(enforced)
        );
        assert_eq!(
            contract["enforcement"]["objective.max_drawdown"]["status"],
            "runtime_enforced"
        );
    }

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

    #[test]
    fn monitored_quote_change_wins_over_zero_broker_exposure() {
        assert_eq!(
            daily_change_pct_from_sources(
                Some(&json!({"change_pct": 0.01914580265095732})),
                Some(&json!({"instrument_price_day_percent_change": 0.0})),
            ),
            0.01914580265095732
        );
        assert_eq!(
            daily_change_pct_from_sources(
                Some(&json!({"change_pct": 0.0})),
                Some(&json!({"instrument_price_day_percent_change": 0.03})),
            ),
            0.0,
            "a verified flat quote must not be replaced by broker fallback data"
        );
        assert_eq!(
            daily_change_pct_from_sources(
                None,
                Some(&json!({"instrument_price_day_percent_change": -0.01})),
            ),
            -0.01
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

    async fn drawdown_history_test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory drawdown test database");
        for statement in [
            "CREATE TABLE trade_ledger (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, side TEXT NOT NULL)",
            "CREATE TABLE portfolio_value_history (id INTEGER PRIMARY KEY, recorded_at TEXT NOT NULL, total_market_value_dkk REAL NOT NULL)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create drawdown test table");
        }
        AppState {
            config_path: std::path::PathBuf::from("drawdown-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// A drawdown peak must never reach back across a re-baselining.
    ///
    /// In mid-May 2026 a run of operator cash adjustments and a "Live export
    /// reset" moved the book from roughly 351,000 to 265,000 DKK. Nothing was
    /// lost, but a peak spanning that boundary reads as a 27% drawdown, which
    /// under the 20% floor would have suspended all buying indefinitely.
    #[tokio::test]
    async fn the_drawdown_window_starts_after_the_latest_external_cash_flow() {
        let state = drawdown_history_test_state().await;
        for (created_at, side) in [
            ("2026-05-13T04:11:30+00:00", "ADJUSTMENT"),
            ("2026-05-18T18:53:24Z", "DEPOSIT"),
            ("2026-05-19T11:01:10Z", "ADJUSTMENT"),
            ("2026-06-04T09:00:00Z", "BUY"),
        ] {
            sqlx::query(&format!(
                "INSERT INTO trade_ledger (created_at, side) VALUES ('{created_at}', '{side}')"
            ))
            .execute(&state.pool)
            .await
            .expect("insert ledger row");
        }
        for (recorded_at, total) in [
            ("2026-05-08T21:00:00Z", 344_775.0),
            ("2026-05-19T21:00:00Z", 351_559.0),
            ("2026-06-04T21:00:00Z", 265_500.0),
            ("2026-06-05T21:00:00Z", 266_232.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO portfolio_value_history (recorded_at, total_market_value_dkk) \
                 VALUES ('{recorded_at}', {total})"
            ))
            .execute(&state.pool)
            .await
            .expect("insert snapshot");
        }

        let rows = state
            .portfolio_drawdown_window(3_650)
            .await
            .expect("history");
        let days = rows
            .iter()
            .map(|row| {
                json_text(row, "recorded_at")
                    .chars()
                    .take(10)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        // The whole adjustment day is excluded, and so is everything before it.
        assert!(
            !days.iter().any(|day| day.as_str() <= "2026-05-19"),
            "pre-re-baselining snapshots leaked into the window: {days:?}"
        );
        assert!(days.contains(&"2026-06-04".to_string()));
        assert!(days.contains(&"2026-06-05".to_string()));
    }

    /// Without any external cash flow the window is the plain lookback, and a
    /// flow older than the lookback must not extend it.
    #[tokio::test]
    async fn a_stale_cash_flow_does_not_widen_the_drawdown_window() {
        let state = drawdown_history_test_state().await;
        sqlx::query(
            "INSERT INTO trade_ledger (created_at, side) VALUES ('2020-01-01T00:00:00Z', 'DEPOSIT')",
        )
        .execute(&state.pool)
        .await
        .expect("insert ledger row");
        for (recorded_at, total) in [
            ("2020-06-01T21:00:00Z", 100_000.0),
            ("2026-07-20T21:00:00Z", 255_000.0),
        ] {
            sqlx::query(&format!(
                "INSERT INTO portfolio_value_history (recorded_at, total_market_value_dkk) \
                 VALUES ('{recorded_at}', {total})"
            ))
            .execute(&state.pool)
            .await
            .expect("insert snapshot");
        }

        let rows = state.portfolio_drawdown_window(90).await.expect("history");
        let days = rows
            .iter()
            .map(|row| {
                json_text(row, "recorded_at")
                    .chars()
                    .take(10)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            !days.contains(&"2020-06-01".to_string()),
            "a 2020 deposit must not pull six-year-old snapshots into a 90-day window: {days:?}"
        );
        assert!(days.contains(&"2026-07-20".to_string()));
    }

    /// Adopted broker positions (`strategy_type = 'portfolio_sync'`) are
    /// bookkeeping for holdings that already existed at the broker when this
    /// system took over the book, so they never acquire a trade-ledger row.
    /// Before 2026-07-25 the integrity check counted them as unreconciled,
    /// which held `healthy` false continuously from the 2026-05-05 adoption and
    /// buried the signal this check exists to raise.
    /// The backfill must reach Trading Manager orders and nothing else.
    /// `report_id IS NOT NULL` is the discriminator: adoption, reconciliation,
    /// and manual rows do not originate in a decision report.
    /// A double-clicked placement left lifecycle test 1 in `placement_preparing`
    /// with no broker order id on 2026-07-25: axum dropped the first handler
    /// future when the browser cancelled it, after the prepared row had already
    /// committed. The orphan then blocked its precheck permanently.
    ///
    /// Stale rows must be findable, and abandoning one must be safe only for the
    /// exact shape that was never sent — never for a row that carries a broker
    /// order id, since that interruption could have happened after placement.

    /// A variable offered to Hermes as tunable must be one the runtime actually
    /// reads. `strategy.swing.cash_buffer_pct` was on this list until
    /// 2026-07-25 while nothing read it, so an experiment could have been
    /// proposed, run in SIM, observed, and promoted while changing nothing --
    /// and whatever the portfolio did would have been attributed to it.
    #[test]
    fn supported_experiment_variables_are_all_read_by_the_runtime() {
        use crate::config_contract::{ContractStatus, status_for_path};
        let mut dead = Vec::new();
        for path in SUPPORTED_EXPERIMENT_VARIABLES {
            // Paths outside the audited roots are not described by the contract
            // and cannot be checked here.
            if let Some(status) = status_for_path(path) {
                if status == ContractStatus::Unused {
                    dead.push((*path).to_string());
                }
            }
        }
        assert!(
            dead.is_empty(),
            "Hermes may propose experiments on variables nothing reads: {dead:?}"
        );
    }

    #[test]
    fn hermes_capabilities_publish_the_checked_variable_list() {
        // The published payload must be the same list the test above checks,
        // so the two cannot drift apart.
        let expected = SUPPORTED_EXPERIMENT_VARIABLES
            .iter()
            .map(|path| json!(path))
            .collect::<Vec<_>>();
        assert_eq!(json!(SUPPORTED_EXPERIMENT_VARIABLES), json!(expected));
        assert!(
            !SUPPORTED_EXPERIMENT_VARIABLES.contains(&"strategy.swing.cash_buffer_pct"),
            "the dead cash-buffer path must not return"
        );
    }

    #[tokio::test]
    async fn stale_protective_stop_preparations_are_findable_and_safely_abandoned() {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT,
                request_id TEXT,
                placement_result_json TEXT,
                cancellation_result_json TEXT,
                reconciliation_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create lifecycle test table");
        let old = "2020-01-01T00:00:00Z";
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        sqlx::query(&format!(
            "INSERT INTO protective_stop_lifecycle_tests
                (id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                 stop_price_local, status, broker_order_id, external_reference, request_id) VALUES
                (1, '{old}', '{old}', 1, 'sim', 'V:xnys', 13, 340.84, 'placement_preparing', NULL, 'stop-test:1:1', 'r1'),
                (2, '{old}', '{old}', 2, 'sim', 'AMD:xnas', 7, 448.78, 'placement_preparing', '5099', 'stop-test:2:1', 'r2'),
                (3, '{now}', '{now}', 3, 'sim', 'BAC:xnys', 58, 59.67, 'placement_preparing', NULL, 'stop-test:3:1', 'r3'),
                (4, '{old}', '{old}', 4, 'sim', 'V:xnys', 13, 340.84, 'broker_working', '5100', 'stop-test:4:1', 'r4')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert lifecycle rows");

        let stale = state
            .stale_protective_stop_preparations(90)
            .await
            .expect("read stale preparations");
        let ids = stale
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![1],
            "only an old preparing row with no broker order id is stale; \
             row 2 has a broker order id, row 3 is recent, row 4 is already working"
        );

        state
            .abandon_protective_stop_preparation(1)
            .await
            .expect("abandon row 1");
        // Abandoning a row that carries a broker order id must be a no-op even
        // if it is somehow attempted.
        state
            .abandon_protective_stop_preparation(2)
            .await
            .expect("attempt row 2");

        let rows = state
            .select_json("SELECT id, status FROM protective_stop_lifecycle_tests ORDER BY id")
            .await
            .expect("read back");
        let statuses = rows
            .iter()
            .map(|row| (value_f64(row, "id") as i64, text_value(row, "status")))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                (1, "placement_abandoned".to_string()),
                (2, "placement_preparing".to_string()),
                (3, "placement_preparing".to_string()),
                (4, "broker_working".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn broker_confirmed_protective_stops_are_adopted_once_into_execution_orders() {
        let state = runtime_settings_test_state(
            "saxo:\n  environment: sim\nexecution:\n  mode: live\n  adapter: saxo\n",
        )
        .await;
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER,
                environment TEXT,
                symbol TEXT,
                quantity REAL,
                stop_price_local REAL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT,
                request_id TEXT,
                placement_result_json TEXT,
                cancellation_result_json TEXT,
                reconciliation_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create lifecycle test table");
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                order_type TEXT NOT NULL DEFAULT 'Market',
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                adapter TEXT NOT NULL,
                quantity REAL,
                stop_price_local REAL,
                approval_required INTEGER NOT NULL DEFAULT 0,
                approved_at TEXT,
                strategy_type TEXT,
                strategy_key TEXT,
                strategy_role TEXT,
                broker_order_id TEXT,
                request_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution order table");
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        sqlx::query(&format!(
            "INSERT INTO protective_stop_lifecycle_tests
                (id, created_at, updated_at, source_precheck_id, environment, symbol, quantity,
                 stop_price_local, status, broker_order_id, external_reference, request_id) VALUES
                (1, '{now}', '{now}', 1, 'sim', 'V:xnys', 13, 340.84, 'broker_working', '5100', 'stop-test:1:1', 'r1'),
                (2, '{now}', '{now}', 2, 'sim', 'AMD:xnas', 7, 448.78, 'placement_submitted', '5101', 'stop-test:2:1', 'r2'),
                (3, '{now}', '{now}', 3, 'sim', 'BAC:xnys', 58, 59.67, 'broker_working', '', 'stop-test:3:1', 'r3'),
                (4, '{now}', '{now}', 4, 'sim', 'AAPL:xnas', 4, 316.49, 'placement_failed', NULL, 'stop-test:4:1', 'r4')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert lifecycle rows");

        let adopted = state
            .adopt_protective_stops_into_execution_orders()
            .await
            .expect("adopt protective stops");
        assert_eq!(
            adopted.len(),
            1,
            "only a broker-confirmed stop carrying a broker order id may be adopted; \
             a submitted-but-unconfirmed, an empty-id, and a failed row must all be skipped"
        );

        // Adoption runs every scheduler cycle. A second pass must not create a
        // duplicate SELL, which at Saxo would be a second resting sell order.
        let repeat = state
            .adopt_protective_stops_into_execution_orders()
            .await
            .expect("re-run adoption");
        assert!(repeat.is_empty(), "adoption must be idempotent");

        let orders = state
            .select_json(
                "SELECT symbol, action, order_type, status, mode, adapter, quantity,
                        stop_price_local, strategy_type, strategy_role, broker_order_id
                 FROM execution_orders ORDER BY id",
            )
            .await
            .expect("read execution orders");
        assert_eq!(orders.len(), 1);
        let order = &orders[0];
        assert_eq!(text_value(order, "symbol"), "V:xnys");
        assert_eq!(text_value(order, "action"), "SELL");
        assert_eq!(text_value(order, "order_type"), "stop");
        // The status has to be one broker sync polls for, or the adoption
        // achieves nothing: an unwatched row is exactly the state this change
        // exists to end.
        assert_eq!(text_value(order, "status"), "broker_working");
        assert_eq!(text_value(order, "mode"), "live");
        assert_eq!(text_value(order, "adapter"), "saxo");
        assert_eq!(text_value(order, "strategy_type"), "protective_stop");
        assert_eq!(text_value(order, "strategy_role"), "protective_stop");
        assert_eq!(text_value(order, "broker_order_id"), "5100");
        assert_eq!(value_f64(order, "quantity"), 13.0);
        assert_eq!(value_f64(order, "stop_price_local"), 340.84);
    }

    #[tokio::test]
    async fn resting_protective_stops_are_not_reported_as_stale_orders() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT,
                action TEXT,
                status TEXT NOT NULL,
                quantity REAL,
                currency TEXT,
                limit_price_local REAL,
                ledger_id INTEGER,
                broker_order_id TEXT,
                error_text TEXT,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution order table");
        let old = "2020-01-01T00:00:00Z";
        sqlx::query(&format!(
            "INSERT INTO execution_orders
                (id, created_at, symbol, action, status, quantity, broker_order_id, strategy_type) VALUES
                (1, '{old}', 'V:xnys', 'SELL', 'broker_working', 13, '5100', 'protective_stop'),
                (2, '{old}', 'AMD:xnas', 'BUY', 'broker_working', 7, '5101', 'swing'),
                (3, '{old}', 'BAC:xnys', 'SELL', 'broker_state_unknown', 58, NULL, 'protective_stop')"
        ))
        .execute(&state.pool)
        .await
        .expect("insert orders");

        let cutoff = "2021-01-01T00:00:00Z";
        let rows = state
            .select_json(&unreconciled_orders_sql(cutoff, cutoff))
            .await
            .expect("run the shared integrity query");
        let ids = rows
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![2, 3],
            "a GoodTillCancel protective stop resting at the broker is doing its job, \
             so age alone must not flag it; an ordinary stale order and a protective stop \
             with an unresolved placement are both still real faults"
        );
    }

    #[tokio::test]
    async fn strategy_type_backfill_targets_only_report_derived_orders() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                report_id INTEGER,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution_orders test table");
        sqlx::query(
            "INSERT INTO execution_orders (id, report_id, strategy_type) VALUES \
                (1, 42, NULL), \
                (2, 43, NULL), \
                (3, NULL, 'portfolio_sync'), \
                (4, NULL, 'clean_reconciliation'), \
                (5, NULL, 'manual'), \
                (6, NULL, NULL), \
                (7, 44, 'swing')",
        )
        .execute(&state.pool)
        .await
        .expect("insert execution_orders test rows");

        state
            .backfill_trading_manager_strategy_type()
            .await
            .expect("backfill strategy type");
        // Idempotent: a second pass must be a no-op.
        state
            .backfill_trading_manager_strategy_type()
            .await
            .expect("backfill strategy type again");

        let rows = state
            .select_json("SELECT id, COALESCE(strategy_type, 'NULL') AS stype FROM execution_orders ORDER BY id")
            .await
            .expect("read back execution orders");
        let observed = rows
            .iter()
            .map(|row| (value_f64(row, "id") as i64, text_value(row, "stype")))
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                (1, "swing".to_string()),
                (2, "swing".to_string()),
                (3, "portfolio_sync".to_string()),
                (4, "clean_reconciliation".to_string()),
                (5, "manual".to_string()),
                // No report id and no type: not a Trading Manager order, so it
                // stays unset rather than being guessed at.
                (6, "NULL".to_string()),
                (7, "swing".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn unreconciled_orders_check_excludes_adopted_positions_but_not_real_fills() {
        let state = runtime_settings_test_state("app: {}\n").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                quantity REAL,
                currency TEXT,
                limit_price_local REAL,
                ledger_id INTEGER,
                broker_order_id TEXT,
                error_text TEXT,
                strategy_type TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution_orders test table");
        sqlx::query(
            "INSERT INTO execution_orders \
                (id, created_at, symbol, action, status, ledger_id, strategy_type) VALUES \
                (1, '2026-05-05T05:20:09+00:00', 'NVDA:xnas', 'BUY', 'executed', NULL, 'portfolio_sync'), \
                (2, '2026-05-05T05:20:09+00:00', 'ORSTED:xcse', 'BUY', 'executed', NULL, 'portfolio_sync'), \
                (3, '2026-05-06T09:00:00+00:00', 'AMD:xnas', 'BUY', 'executed', NULL, 'swing'), \
                (4, '2026-05-06T09:00:00+00:00', 'V:xnys', 'BUY', 'executed', 7, 'swing')",
        )
        .execute(&state.pool)
        .await
        .expect("insert execution_orders test rows");

        let cutoff = "2026-07-01T00:00:00+00:00";
        let flagged = state
            .select_json(&unreconciled_orders_sql(cutoff, cutoff))
            .await
            .expect("query unreconciled orders");
        let flagged_ids = flagged
            .iter()
            .map(|row| value_f64(row, "id") as i64)
            .collect::<Vec<_>>();
        assert_eq!(
            flagged_ids,
            vec![3],
            "only the genuine ledger-less fill should be flagged"
        );

        let adopted = state
            .select_json(ADOPTED_ORDERS_WITHOUT_LEDGER_SQL)
            .await
            .expect("count adopted orders");
        assert_eq!(
            adopted.first().map(|row| value_f64(row, "count") as i64),
            Some(2),
            "adopted positions stay visible as context rather than vanishing"
        );
    }

    #[tokio::test]
    async fn protective_stop_lifecycle_requires_accepted_sim_precheck_and_blocks_duplicate_active_test()
     {
        let state = runtime_settings_test_state("saxo:\n  environment: sim\n").await;
        sqlx::query(
            "CREATE TABLE protective_stop_prechecks (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                result_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create protective-stop prechecks test table");
        sqlx::query(
            "CREATE TABLE protective_stop_lifecycle_tests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_precheck_id INTEGER NOT NULL,
                environment TEXT NOT NULL,
                symbol TEXT NOT NULL,
                quantity REAL NOT NULL,
                stop_price_local REAL NOT NULL,
                status TEXT NOT NULL,
                broker_order_id TEXT,
                external_reference TEXT NOT NULL,
                request_id TEXT NOT NULL UNIQUE,
                placement_result_json TEXT NOT NULL,
                cancellation_result_json TEXT NOT NULL,
                reconciliation_json TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create protective-stop lifecycle test table");
        sqlx::query(
            "INSERT INTO protective_stop_prechecks (
                id, created_at, environment, symbol, quantity, stop_price_local, status, result_json
             ) VALUES (1, '2026-07-25T08:00:00Z', 'sim', 'TSLA:xnas', 1, 300, 'precheck_ok', '{}')",
        )
        .execute(&state.pool)
        .await
        .expect("seed accepted SIM precheck");

        let first = state
            .prepare_protective_stop_lifecycle_test(1)
            .await
            .expect("prepare lifecycle test");
        assert_eq!(first["status"], json!("placement_preparing"));
        assert!(
            state
                .prepare_protective_stop_lifecycle_test(1)
                .await
                .is_err()
        );
        assert!(
            state
                .prepare_protective_stop_lifecycle_test(999)
                .await
                .is_err()
        );
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
    async fn purge_retired_runtime_settings_removes_all_legacy_cash_buffer_paths() {
        let state = runtime_settings_test_state("xai:\n  provider: openrouter\n").await;
        state
            .save_runtime_setting(
                "strategy.capital.cash_buffer",
                &json!({"min_cash_buffer_pct": 0.0, "max_deployment_pct": 1.0}),
            )
            .await
            .expect("seed retired cash-buffer override");
        state
            .save_runtime_setting(
                "strategy.swing.cash_buffer_pct",
                &json!({"cash_buffer_pct": 0.0}),
            )
            .await
            .expect("seed retired swing cash-buffer override");
        state
            .save_runtime_setting("ai_model", &json!({"model": "openrouter/fusion"}))
            .await
            .expect("seed active model override");

        assert_eq!(
            state
                .purge_retired_runtime_settings()
                .await
                .expect("purge retired settings"),
            2
        );
        assert!(
            state
                .runtime_setting("strategy.capital.cash_buffer")
                .await
                .expect("read retired setting")
                .is_none()
        );
        assert!(
            state
                .runtime_setting("strategy.swing.cash_buffer_pct")
                .await
                .expect("read retired swing setting")
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
    fn missed_trade_shadows_only_track_timing_capital_and_capacity_blocks() {
        assert!(missed_trade_shadow_gate_is_eligible("cash_budget"));
        assert!(missed_trade_shadow_gate_is_eligible("market_open"));
        assert!(missed_trade_shadow_gate_is_eligible("drawdown_guardrail"));
        assert!(missed_trade_shadow_gate_is_eligible("max_holdings"));
        assert!(!missed_trade_shadow_gate_is_eligible("technical"));
        assert!(!missed_trade_shadow_gate_is_eligible("markov"));
        assert!(!missed_trade_shadow_gate_is_eligible("risk_exclusion"));
        assert!(!missed_trade_shadow_gate_is_eligible(
            "instrument_quarantine"
        ));
    }

    #[test]
    fn missed_trade_shadow_outcomes_are_bounded_and_observational() {
        let evidence = missed_trade_shadow_outcome_evidence_from_rows(&[
            json!({"source_gate": "cash_budget", "estimated_return_pct": 0.10}),
            json!({"source_gate": "cash_budget", "estimated_return_pct": -0.05}),
            json!({"source_gate": "market_open", "estimated_return_pct": null}),
        ]);
        assert_eq!(evidence["status"], json!("collecting"));
        assert_eq!(evidence["recorded_shadow_count"], json!(3));
        assert_eq!(evidence["observed_shadow_count"], json!(2));
        assert_eq!(evidence["overall"]["sample_count"], json!(2));
        assert!(
            (value_f64(&evidence["overall"], "average_directional_return_pct") - 0.025).abs()
                < 1e-9
        );
        assert!((value_f64(&evidence["overall"], "positive_return_rate") - 0.5).abs() < 1e-9);
        assert_eq!(evidence["by_gate"][0]["source_gate"], json!("cash_budget"));
        assert_eq!(evidence["by_gate"][0]["outcome"]["sample_count"], json!(2));
        assert!(
            evidence["interpretation"]
                .as_str()
                .is_some_and(|value| value.contains("exclude fees"))
        );
    }

    #[tokio::test]
    async fn missed_trade_shadows_record_selected_blocks_and_refresh_quotes() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE missed_trade_shadows (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                report_id INTEGER NOT NULL,
                manager_run_id INTEGER NOT NULL,
                strategy_key TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                source_gate TEXT NOT NULL,
                shadow_quantity REAL NOT NULL,
                reference_price_local REAL,
                currency TEXT,
                status TEXT NOT NULL,
                latest_price_local REAL,
                latest_price_at TEXT,
                estimated_return_pct REAL,
                estimated_pnl_local REAL,
                observation_count INTEGER NOT NULL DEFAULT 0,
                UNIQUE(manager_run_id, strategy_key)
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create missed-trade shadow table");
        let candidates = vec![
            json!({
                "strategy_key": "cash-buy:AMD:xnas:BUY",
                "symbol": "AMD:xnas",
                "action": "BUY",
                "quantity": 2.0,
                "currency": "USD",
                "reference_price_local": 100.0,
                "gate_code": "cash_budget",
            }),
            json!({
                "strategy_key": "technical-buy:NVDA:xnas:BUY",
                "symbol": "NVDA:xnas",
                "action": "BUY",
                "quantity": 3.0,
                "currency": "USD",
                "reference_price_local": 200.0,
                "gate_code": "technical",
            }),
        ];

        let result = state
            .record_missed_trade_shadows(41, 77, &candidates)
            .await
            .expect("record missed-trade shadows");
        assert_eq!(result["created"], json!(1));
        assert_eq!(result["skipped"], json!(1));
        assert_eq!(
            state
                .active_missed_trade_shadow_symbols()
                .await
                .expect("list active missed-trade symbols"),
            vec!["AMD:xnas".to_string()]
        );
        assert_eq!(
            state
                .refresh_missed_trade_shadow_price("AMD:xnas", 110.0, "2026-07-27T10:00:00Z")
                .await
                .expect("refresh missed-trade shadow quote"),
            1
        );
        let rows = state
            .missed_trade_shadows(10)
            .await
            .expect("read missed-trade shadows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source_gate"], json!("cash_budget"));
        assert!((value_f64(&rows[0], "estimated_return_pct") - 0.1).abs() < 1e-9);
        assert!((value_f64(&rows[0], "estimated_pnl_local") - 20.0).abs() < 1e-9);
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

    #[test]
    fn holding_period_attribution_uses_available_trading_sessions_and_side() {
        let fill = json!({
            "first_fill_at": "2026-07-20T15:34:00Z",
            "filled_quantity": 2.0,
            "average_fill_price_local": 100.0,
            "currency": "USD"
        });
        let closes = vec![
            json!({"run_date": "2026-07-21", "close": 105.0}),
            json!({"run_date": "2026-07-22", "close": 102.0}),
            json!({"run_date": "2026-07-23", "close": 104.0}),
            json!({"run_date": "2026-07-24", "close": 106.0}),
            json!({"run_date": "2026-07-27", "close": 110.0}),
        ];

        let buy = compact_holding_period_outcome(&json!({"action": "BUY"}), &fill, &closes);
        assert_eq!(json_text(&buy, "status"), "complete");
        assert_eq!(value_i64(&buy, "available_sessions"), 5);
        assert!((value_f64(&buy["one_session"], "market_return_pct") - 0.05).abs() < 1e-9);
        assert!((value_f64(&buy["five_session"], "directional_return_pct") - 0.10).abs() < 1e-9);

        let sell = compact_holding_period_outcome(&json!({"action": "SELL"}), &fill, &closes);
        assert!((value_f64(&sell["one_session"], "directional_return_pct") + 0.05).abs() < 1e-9);
    }

    #[test]
    fn holding_period_attribution_stays_partial_until_five_closes_exist() {
        let outcome = compact_holding_period_outcome(
            &json!({"action": "BUY"}),
            &json!({
                "first_fill_at": "2026-07-20T15:34:00Z",
                "filled_quantity": 1.0,
                "average_fill_price_local": 100.0,
                "currency": "USD"
            }),
            &[json!({"run_date": "2026-07-21", "close": 98.0})],
        );

        assert_eq!(json_text(&outcome, "status"), "partial");
        assert!((value_f64(&outcome["one_session"], "directional_return_pct") + 0.02).abs() < 1e-9);
        assert!(outcome["five_session"].is_null());
    }

    #[test]
    fn holding_thesis_reviews_only_flag_current_positions_with_due_recorded_theses() {
        let now = DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z")
            .expect("fixed test timestamp")
            .with_timezone(&Utc);
        let reviews = compact_holding_thesis_reviews(
            &[
                json!({
                    "symbol": "AMD:xnas",
                    "instrument_name": "AMD",
                    "quantity": 2.0
                }),
                json!({
                    "symbol": "NVDA:xnas",
                    "instrument_name": "NVIDIA",
                    "quantity": 1.0
                }),
            ],
            &[
                json!({
                    "id": 22,
                    "symbol": "AMD:xnas",
                    "created_at": "2026-07-12T15:00:00Z",
                    "first_fill_at": "2026-07-12T15:30:00Z",
                    "trade_thesis_json": {
                        "status": "recorded",
                        "intended_holding_window": "next_2_weeks",
                        "entry_rationale": "Verified technical setup",
                        "invalidation": "Fresh evidence no longer supports the long setup."
                    }
                }),
                json!({
                    "id": 23,
                    "symbol": "MSFT:xnas",
                    "created_at": "2026-07-01T15:00:00Z",
                    "first_fill_at": "2026-07-01T15:30:00Z",
                    "trade_thesis_json": {
                        "status": "recorded",
                        "intended_holding_window": "next_2_weeks"
                    }
                }),
            ],
            7,
            now,
        );

        assert_eq!(json_text(&reviews, "status"), "review_due");
        assert_eq!(value_i64(&reviews, "held_position_count"), 2);
        assert_eq!(value_i64(&reviews, "review_count"), 1);
        let item = reviews["reviews"][0].clone();
        assert_eq!(json_text(&item, "symbol"), "AMD:xnas");
        assert_eq!(json_text(&item, "status"), "thesis_window_elapsed");
        assert_eq!(value_i64(&item, "age_days"), 14);
        assert_eq!(value_i64(&item, "latest_thesis_order_id"), 22);
        assert!(json_text(&item, "operator_next_step").contains("not instruct an exit"));
        assert!(json_text(&reviews, "safety").contains("no_saxo"));
    }

    #[test]
    fn holding_thesis_reviews_wait_until_the_stale_window() {
        let now = DateTime::parse_from_rfc3339("2026-07-27T12:00:00Z")
            .expect("fixed test timestamp")
            .with_timezone(&Utc);
        let reviews = compact_holding_thesis_reviews(
            &[json!({"symbol": "AMD:xnas", "quantity": 2.0})],
            &[json!({
                "id": 22,
                "symbol": "AMD:xnas",
                "created_at": "2026-07-24T15:00:00Z",
                "first_fill_at": "2026-07-24T15:30:00Z",
                "trade_thesis_json": {
                    "status": "recorded",
                    "intended_holding_window": "next_2_weeks"
                }
            })],
            7,
            now,
        );

        assert_eq!(json_text(&reviews, "status"), "no_reviews_due");
        assert_eq!(value_i64(&reviews, "review_count"), 0);
    }

    #[test]
    fn position_lifecycle_attribution_tracks_observed_add_reduce_and_exit() {
        let fills = vec![
            json!({"id": 1, "execution_order_id": 10, "created_at": "2026-07-20T15:00:00Z", "side": "BUY", "delta_quantity": 2.0}),
            json!({"id": 2, "execution_order_id": 11, "created_at": "2026-07-21T15:00:00Z", "side": "BUY", "delta_quantity": 3.0}),
            json!({"id": 3, "execution_order_id": 12, "created_at": "2026-07-22T15:00:00Z", "side": "SELL", "delta_quantity": 1.0}),
            json!({"id": 4, "execution_order_id": 13, "created_at": "2026-07-23T15:00:00Z", "side": "SELL", "delta_quantity": 4.0}),
        ];

        let add = compact_execution_position_lifecycle(&json!({"id": 11, "action": "BUY"}), &fills);
        assert_eq!(json_text(&add, "phase"), "add");
        assert_eq!(json_text(&add, "history_status"), "observed_local_fills");
        assert_eq!(value_f64(&add, "observed_net_before"), 2.0);
        assert_eq!(value_f64(&add, "observed_net_after"), 5.0);

        let reduce =
            compact_execution_position_lifecycle(&json!({"id": 12, "action": "SELL"}), &fills);
        assert_eq!(json_text(&reduce, "phase"), "reduce");
        assert_eq!(value_f64(&reduce, "observed_net_before"), 5.0);
        assert_eq!(value_f64(&reduce, "observed_net_after"), 4.0);

        let exit =
            compact_execution_position_lifecycle(&json!({"id": 13, "action": "SELL"}), &fills);
        assert_eq!(json_text(&exit, "phase"), "exit");
        assert_eq!(value_f64(&exit, "observed_net_after"), 0.0);
    }

    #[test]
    fn position_lifecycle_attribution_refuses_to_infer_unobserved_inventory() {
        let outcome = compact_execution_position_lifecycle(
            &json!({"id": 14, "action": "SELL"}),
            &[json!({
                "id": 1,
                "execution_order_id": 14,
                "created_at": "2026-07-20T15:00:00Z",
                "side": "SELL",
                "delta_quantity": 2.0
            })],
        );

        assert_eq!(json_text(&outcome, "phase"), "partial_history");
        assert_eq!(json_text(&outcome, "history_status"), "partial_history");
        assert!(json_text(&outcome, "interpretation").contains("not broker position truth"));
    }

    #[tokio::test]
    async fn holding_period_attribution_reads_reconciled_fills_and_daily_closes() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                execution_order_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                delta_quantity REAL NOT NULL,
                average_price_local REAL NOT NULL,
                currency TEXT NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution fills");
        sqlx::query(
            "CREATE TABLE daily_indicator_signals (
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                close REAL NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create daily indicator signals");
        sqlx::query(
            "INSERT INTO execution_fills
                (execution_order_id, created_at, delta_quantity, average_price_local, currency)
             VALUES (42, '2026-07-20T15:34:00Z', 1, 100, 'USD'),
                    (42, '2026-07-20T15:35:00Z', 3, 110, 'USD')",
        )
        .execute(&state.pool)
        .await
        .expect("insert reconciled fills");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (symbol, run_date, status, close) VALUES
                ('AMD:xnas', '2026-07-20', 'ok', 99),
                ('AMD:xnas', '2026-07-21', 'ok', 108),
                ('AMD:xnas', '2026-07-22', 'ok', 109),
                ('AMD:xnas', '2026-07-23', 'ok', 110),
                ('AMD:xnas', '2026-07-24', 'ok', 111),
                ('AMD:xnas', '2026-07-27', 'ok', 112)",
        )
        .execute(&state.pool)
        .await
        .expect("insert daily closes");

        let outcome = state
            .execution_order_holding_period_outcome(&json!({
                "id": 42,
                "symbol": "AMD:xnas",
                "action": "BUY"
            }))
            .await
            .expect("read holding-period attribution");

        assert_eq!(json_text(&outcome, "status"), "complete");
        assert_eq!(value_f64(&outcome, "filled_quantity"), 4.0);
        assert!((value_f64(&outcome, "fill_price_local") - 107.5).abs() < 1e-9);
        assert_eq!(json_text(&outcome["one_session"], "as_of"), "2026-07-21");
        assert_eq!(json_text(&outcome["five_session"], "as_of"), "2026-07-27");
    }

    #[tokio::test]
    async fn trade_thesis_outcome_evidence_reads_only_recorded_buy_theses() {
        let state = runtime_settings_test_state("{}").await;
        for statement in [
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                quantity REAL NOT NULL,
                currency TEXT NOT NULL,
                trade_thesis_json TEXT
            )",
            "CREATE TABLE execution_fills (
                execution_order_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                delta_quantity REAL NOT NULL,
                average_price_local REAL NOT NULL,
                currency TEXT NOT NULL
            )",
            "CREATE TABLE daily_indicator_signals (
                symbol TEXT NOT NULL,
                run_date TEXT NOT NULL,
                status TEXT NOT NULL,
                close REAL NOT NULL
            )",
        ] {
            sqlx::query(statement)
                .execute(&state.pool)
                .await
                .expect("create trade-thesis evidence table");
        }
        sqlx::query(
            "INSERT INTO execution_orders
                (id, created_at, symbol, action, quantity, currency, trade_thesis_json)
             VALUES
                (41, '2026-07-20T15:00:00Z', 'AMD:xnas', 'BUY', 1, 'USD',
                 '{\"status\":\"recorded\"}'),
                (42, '2026-07-20T15:00:00Z', 'NVDA:xnas', 'BUY', 1, 'USD', NULL)",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-order theses");
        sqlx::query(
            "INSERT INTO execution_fills
                (execution_order_id, created_at, delta_quantity, average_price_local, currency)
             VALUES (41, '2026-07-20T15:30:00Z', 1, 100, 'USD')",
        )
        .execute(&state.pool)
        .await
        .expect("seed reconciled thesis fill");
        sqlx::query(
            "INSERT INTO daily_indicator_signals (symbol, run_date, status, close) VALUES
                ('AMD:xnas', '2026-07-21', 'ok', 101),
                ('AMD:xnas', '2026-07-22', 'ok', 102),
                ('AMD:xnas', '2026-07-23', 'ok', 103),
                ('AMD:xnas', '2026-07-24', 'ok', 104),
                ('AMD:xnas', '2026-07-27', 'ok', 105)",
        )
        .execute(&state.pool)
        .await
        .expect("seed later AMD closes");

        let evidence = state
            .trade_thesis_outcome_evidence()
            .await
            .expect("read trade-thesis outcome evidence");

        assert_eq!(json_text(&evidence, "status"), "collecting");
        assert_eq!(value_i64(&evidence, "recorded_thesis_count"), 1);
        assert_eq!(value_i64(&evidence, "filled_thesis_count"), 1);
        assert_eq!(value_i64(&evidence["one_session"], "sample_count"), 1);
        assert_eq!(value_i64(&evidence["five_session"], "sample_count"), 1);
        assert!(
            (value_f64(&evidence["five_session"], "average_directional_return_pct") - 0.05).abs()
                < 1e-9
        );
    }

    #[test]
    fn decision_pulse_outcome_evidence_keeps_buy_price_movement_and_sell_ledger_gains_separate() {
        let evidence = decision_pulse_outcome_evidence_from_observations(&[
            json!({
                "analysis_pulse_key": "europe_open_followup:2026-07-20",
                "analysis_pulse_label": "EU Open +1h15",
                "action": "BUY",
                "execution_status": "executed",
                "hermes_reviewed": true,
                "hermes_effect": "reduced",
                "holding_period_outcome": {
                    "filled_quantity": 2.0,
                    "one_session": {"directional_return_pct": 0.04},
                    "five_session": {"directional_return_pct": 0.06},
                },
                "ledger_outcome": null,
            }),
            json!({
                "analysis_pulse_key": "us_open_followup:2026-07-20",
                "analysis_pulse_label": "US Open +1h15",
                "action": "SELL",
                "execution_status": "executed",
                "hermes_reviewed": false,
                "hermes_effect": "not_recorded",
                "holding_period_outcome": null,
                "ledger_outcome": {
                    "status": "reconciled",
                    "realised_gain_dkk": 125.0,
                    "commission_dkk": 3.0,
                    "tax_dkk": 0.0,
                },
            }),
            json!({
                "strategy_type": "portfolio_sync",
                "action": "BUY",
                "execution_status": "not_recorded",
                "hermes_reviewed": false,
                "hermes_effect": "not_recorded",
                "holding_period_outcome": {"filled_quantity": 0.0},
                "ledger_outcome": null,
            }),
        ]);

        assert_eq!(json_text(&evidence, "status"), "collecting");
        assert_eq!(value_i64(&evidence["overall"], "attributed_order_count"), 3);
        assert_eq!(
            value_i64(&evidence["overall"], "hermes_reviewed_order_count"),
            1
        );
        assert_eq!(
            value_i64(&evidence["overall"]["one_session"], "sample_count"),
            1
        );
        assert!(
            (value_f64(
                &evidence["overall"]["five_session"],
                "average_directional_return_pct"
            ) - 0.06)
                .abs()
                < 1e-9
        );
        assert!(
            (value_f64(&evidence["overall"]["realised_sell"], "realised_gain_dkk") - 125.0).abs()
                < 1e-9
        );
        assert_eq!(
            evidence["overall"]["hermes_effect_counts"]["reduced"],
            json!(1)
        );
        assert_eq!(
            evidence["overall"]["execution_status_counts"]["executed"],
            json!(2)
        );
        let pulses = evidence["pulses"].as_array().expect("pulse rows");
        assert!(pulses.iter().any(|row| {
            json_text(row, "pulse_key") == "portfolio_sync"
                && json_text(row, "pulse_label") == "Portfolio Sync"
        }));
    }

    #[tokio::test]
    async fn position_lifecycle_attribution_reads_symbol_scoped_reconciled_fills() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_fills (
                id INTEGER PRIMARY KEY,
                execution_order_id INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                created_at TEXT NOT NULL,
                side TEXT NOT NULL,
                delta_quantity REAL NOT NULL
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-fill lifecycle table");
        sqlx::query(
            "INSERT INTO execution_fills
                (id, execution_order_id, symbol, created_at, side, delta_quantity)
             VALUES
                (1, 41, 'AMD:xnas', '2026-07-20T15:00:00Z', 'BUY', 2),
                (2, 42, 'AMD:xnas', '2026-07-21T15:00:00Z', 'SELL', 1),
                (3, 99, 'NVDA:xnas', '2026-07-21T15:00:00Z', 'BUY', 9)",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-fill lifecycle rows");

        let outcome = state
            .execution_order_position_lifecycle(&json!({
                "id": 42,
                "symbol": "AMD:xnas",
                "action": "SELL"
            }))
            .await
            .expect("read position lifecycle attribution");

        assert_eq!(json_text(&outcome, "phase"), "reduce");
        assert_eq!(value_i64(&outcome, "observed_fill_count"), 2);
        assert_eq!(value_i64(&outcome, "observed_order_count"), 2);
        assert_eq!(value_f64(&outcome, "observed_net_before"), 2.0);
        assert_eq!(value_f64(&outcome, "observed_net_after"), 1.0);
    }

    #[tokio::test]
    async fn trade_thesis_attribution_uses_the_latest_prior_buy_for_the_symbol() {
        let state = runtime_settings_test_state("{}").await;
        sqlx::query(
            "CREATE TABLE execution_orders (
                id INTEGER PRIMARY KEY,
                created_at TEXT NOT NULL,
                symbol TEXT NOT NULL,
                action TEXT NOT NULL,
                trade_thesis_json TEXT
            )",
        )
        .execute(&state.pool)
        .await
        .expect("create execution-order thesis table");
        sqlx::query(
            "INSERT INTO execution_orders (id, created_at, symbol, action, trade_thesis_json)
             VALUES
                (41, '2026-07-20T15:00:00Z', 'AMD:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"starter_long\"}'),
                (42, '2026-07-21T15:00:00Z', 'AMD:xnas', 'SELL', NULL),
                (43, '2026-07-22T15:00:00Z', 'AMD:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"add_on_strength\"}'),
                (99, '2026-07-21T15:00:00Z', 'NVDA:xnas', 'BUY',
                 '{\"status\":\"recorded\",\"strategy_key\":\"other_symbol\"}')",
        )
        .execute(&state.pool)
        .await
        .expect("seed execution-order theses");

        let thesis = state
            .execution_order_trade_thesis(&json!({
                "id": 42,
                "created_at": "2026-07-21T15:00:00Z",
                "symbol": "AMD:xnas"
            }))
            .await
            .expect("read prior BUY thesis");

        assert_eq!(json_text(&thesis, "status"), "recorded");
        assert_eq!(json_text(&thesis, "strategy_key"), "starter_long");
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

    /// Makes `strategy.ladder.stop_loss_atr_multiple` real. The proposal covers
    /// only the uncovered share of a position, is absent when the position is
    /// fully protected or the indicator data is unusable, and never claims to be
    /// tick-normalized -- normalization needs Saxo instrument details and
    /// happens in the precheck/placement path.
    #[test]
    fn proposed_protective_stop_sizes_to_uncovered_quantity_and_fails_closed() {
        let positions = vec![
            json!({"symbol": "OPEN:xnas", "quantity": 10.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "HALF:xnas", "quantity": 10.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "SAFE:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "NOATR:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "DEEP:xnas", "quantity": 4.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
        ];
        let orders = vec![
            json!({"symbol": "HALF:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 6.0, "stop_price_local": 90.0}),
            json!({"symbol": "SAFE:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 4.0, "stop_price_local": 90.0}),
        ];
        let indicators = vec![
            json!({"symbol": "OPEN:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            json!({"symbol": "HALF:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            json!({"symbol": "SAFE:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 4.0}),
            // Unusable ATR must produce no proposal rather than a bad level.
            json!({"symbol": "NOATR:xnas", "run_date": "2026-07-24", "close": 100.0, "atr14": 0.0}),
            // A stop below zero is not a protective level.
            json!({"symbol": "DEEP:xnas", "run_date": "2026-07-24", "close": 5.0, "atr14": 4.0}),
        ];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &orders, &[], &indicators, 2.0);
        let rows = coverage["positions"].as_array().expect("coverage rows");
        let find = |symbol: &str| {
            rows.iter()
                .find(|row| row["symbol"] == json!(symbol))
                .expect("symbol row")
        };

        // 100 - (4 * 2) = 92, on the full unprotected quantity.
        let open = &find("OPEN:xnas")["proposed_stop"];
        assert_eq!(open["stop_price_local"], json!(92.0));
        assert_eq!(open["quantity"], json!(10.0));
        assert_eq!(open["distance_pct"], json!(8.0));
        assert_eq!(open["tick_normalized"], json!(false));

        // Only the 4 uncovered of 10 need a stop.
        assert_eq!(find("HALF:xnas")["proposed_stop"]["quantity"], json!(4.0));

        for symbol in ["SAFE:xnas", "NOATR:xnas", "DEEP:xnas"] {
            assert_eq!(
                find(symbol)["proposed_stop"],
                JsonValue::Null,
                "{symbol} must not carry a proposed stop"
            );
        }

        // The exception rows carry the same proposal so the operator sees the
        // level next to the reason.
        let exceptions = coverage["exceptions"].as_array().expect("exceptions");
        let open_exception = exceptions
            .iter()
            .find(|row| row["symbol"] == json!("OPEN:xnas"))
            .expect("unprotected exception");
        assert_eq!(
            open_exception["proposed_stop"]["stop_price_local"],
            json!(92.0)
        );
    }

    #[test]
    fn protective_stop_coverage_requires_broker_confirmed_stop_state() {
        let positions = vec![
            json!({"symbol": "FULL:xnas", "quantity": 5.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "PART:xnas", "quantity": 5.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "PLAN:xnas", "quantity": 3.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
            json!({"symbol": "FAIL:xnas", "quantity": 2.0, "currency": "USD", "updated_at": "2026-07-25T12:00:00Z"}),
        ];
        let orders = vec![
            json!({"symbol": "FULL:xnas", "action": "SELL", "order_type": "Stop", "status": "broker_working", "quantity": 5.0, "stop_price_local": 95.0, "raw_payload": "must not appear"}),
            json!({"symbol": "PART:xnas", "action": "SELL", "order_type": "StopLimit", "status": "submitted_to_broker", "quantity": 2.0, "stop_price_local": 90.0}),
            json!({"symbol": "PLAN:xnas", "action": "SELL", "order_type": "Stop", "status": "pending_execution", "quantity": 3.0, "stop_price_local": 85.0}),
            json!({"symbol": "FAIL:xnas", "action": "SELL", "order_type": "Stop", "status": "execution_failed", "quantity": 2.0, "stop_price_local": 80.0}),
        ];

        let coverage = protective_stop_coverage_from_rows(&positions, &orders, &[], &[], 2.0);
        let rows = coverage["positions"].as_array().expect("coverage rows");
        let find = |symbol: &str| {
            rows.iter()
                .find(|row| row["symbol"] == json!(symbol))
                .expect("symbol row")
        };
        assert_eq!(coverage["status"], "attention_required");
        assert_eq!(coverage["summary"]["protected_count"], 1);
        assert_eq!(coverage["summary"]["partial_count"], 1);
        assert_eq!(coverage["summary"]["planned_count"], 1);
        assert_eq!(coverage["summary"]["unprotected_count"], 1);
        assert_eq!(find("FULL:xnas")["protection_status"], "protected");
        assert_eq!(find("PART:xnas")["protection_status"], "partial_protection");
        assert_eq!(find("PLAN:xnas")["protection_status"], "planned");
        assert_eq!(find("FAIL:xnas")["protection_status"], "unprotected");
        assert!(!coverage.to_string().contains("must not appear"));
    }

    #[test]
    fn protective_stop_coverage_accepts_only_reconciled_sim_lifecycle_tests() {
        let positions = vec![json!({
            "symbol": "TEST:xnas",
            "quantity": 5.0,
            "currency": "USD",
            "updated_at": "2026-07-25T12:00:00Z",
        })];
        let lifecycle_tests = vec![
            json!({
                "id": "working",
                "environment": "sim",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 95.0,
                "status": "broker_working",
                "broker_order_id": "12345",
            }),
            json!({
                "id": "ambiguous",
                "environment": "sim",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 94.0,
                "status": "placement_submitted",
                "broker_order_id": "67890",
            }),
            json!({
                "id": "wrong-environment",
                "environment": "live",
                "symbol": "TEST:xnas",
                "quantity": 5.0,
                "stop_price_local": 93.0,
                "status": "broker_working",
                "broker_order_id": "98765",
            }),
        ];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &[], &lifecycle_tests, &[], 2.0);
        let row = &coverage["positions"][0];
        assert_eq!(coverage["status"], "covered");
        assert_eq!(coverage["summary"]["protected_count"], 1);
        assert_eq!(coverage["summary"]["exception_count"], 0);
        assert_eq!(row["confirmed_covered_quantity"], 5.0);
        assert_eq!(row["coverage_evidence"]["execution_orders"], 0);
        assert_eq!(row["coverage_evidence"]["manual_sim_lifecycle_tests"], 1);
    }

    #[test]
    fn protective_stop_coverage_marks_unconfirmed_lifecycle_test_as_exception() {
        let positions = vec![json!({
            "symbol": "PENDING:xnas",
            "quantity": 2.0,
            "currency": "USD",
            "updated_at": "2026-07-25T12:00:00Z",
        })];
        let lifecycle_tests = vec![json!({
            "environment": "sim",
            "symbol": "PENDING:xnas",
            "quantity": 2.0,
            "stop_price_local": 90.0,
            "status": "placement_submitted",
            "broker_order_id": "12345",
        })];

        let coverage =
            protective_stop_coverage_from_rows(&positions, &[], &lifecycle_tests, &[], 2.0);
        assert_eq!(coverage["status"], "attention_required");
        assert_eq!(coverage["summary"]["unprotected_count"], 1);
        assert_eq!(coverage["summary"]["exception_count"], 1);
        assert_eq!(
            coverage["exceptions"][0]["kind"],
            "unprotected_broker_position"
        );
        assert_eq!(coverage["exceptions"][0]["unprotected_quantity"], 2.0);
    }

    #[test]
    fn protective_stop_coverage_for_hermes_is_bounded() {
        let coverage = json!({
            "status": "covered",
            "summary": {"position_count": 3},
            "positions": [json!({"symbol": "A:xnas"}), json!({"symbol": "B:xnas"}), json!({"symbol": "C:xnas"})],
            "exceptions": [json!({"symbol": "A:xnas"}), json!({"symbol": "B:xnas"}), json!({"symbol": "C:xnas"})],
        });
        let compact = compact_protective_stop_coverage_for_hermes(&coverage, 2);
        assert_eq!(compact["positions"].as_array().map(Vec::len), Some(2));
        assert_eq!(compact["exceptions"].as_array().map(Vec::len), Some(2));
        assert_eq!(coverage["positions"].as_array().map(Vec::len), Some(3));
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
    fn candidate_waterfall_preserves_stable_manager_gate_codes() {
        for gate_code in [
            "candidate_limit",
            "drawdown_guardrail",
            "position_weight",
            "max_holdings",
            "max_selected_assets",
            "cost_guard",
        ] {
            assert_eq!(
                candidate_gate_code(&json!({"gate_code": gate_code})),
                gate_code
            );
        }
        assert_eq!(
            candidate_gate_code_from_reason("Portfolio drawdown guardrail is active"),
            "drawdown_guardrail"
        );
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
