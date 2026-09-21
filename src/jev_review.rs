//! Two observational Jev passes that run beside existing logic, never inside it.
//!
//! Both are deliberately sidecars. Grading happens after a Decision Report is
//! already stored, so the report path is untouched and a Jev outage cannot
//! delay or alter a report. Failure classification runs over errors the
//! substring cascade already labelled, so the order path keeps its synchronous,
//! deterministic classifier and gains only a second opinion on the ones it
//! could not name.
//!
//! Neither pass can change a gate, a candidate, a queue entry, or a broker
//! call. They write rows and nothing else reads those rows to make a decision.

use anyhow::Context;
use chrono::Utc;
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    db::row_to_json,
    jev::{JevAvailability, JevConfig},
    jev_signals, jev_store,
    state::AppState,
};

/// Failure categories `decision_provider_state::local_failure_category` knows
/// how to name. Jev may pick one of these or `other`; it cannot invent a
/// category nothing downstream understands.
const KNOWN_FAILURE_CODES: &[(&str, &str)] = &[
    (
        "schema",
        "The provider rejected or violated the structured output schema",
    ),
    (
        "timeout",
        "The request exceeded its deadline without a reply",
    ),
    (
        "truncated",
        "The reply hit the output token ceiling and stopped mid-answer",
    ),
    (
        "parse",
        "The reply arrived but could not be read as valid JSON",
    ),
    (
        "rate_limit",
        "The provider throttled the request for sending too many too quickly",
    ),
    (
        // Added after the first live run: ten failures the cascade filed as
        // `other` were xAI 403s reading "used all available credits or reached
        // its monthly spending limit". Jev filed them under `rate_limit`, the
        // closest label on offer, because this one did not exist. The closed
        // answer set worked -- it could not invent a category -- but a missing
        // option makes every answer approximate.
        "quota_exhausted",
        "The account ran out of credits or hit a spending limit, so the provider refused to bill further work",
    ),
    (
        "auth",
        "The provider rejected the credentials or the account lacks access",
    ),
    (
        "upstream_unavailable",
        "The provider or its upstream was overloaded, down, or unreachable",
    ),
];

const REVIEW_BATCH_LIMIT: usize = 10;

/// Versions of the two review question sets.
///
/// A subject is skipped only when it has already been judged *at the current
/// version*. Changing a question without bumping these would leave every
/// existing subject answered under the old wording and silently pool two
/// different measurements; bumping without the version-aware skip would leave
/// them permanently unasked.
///
/// Unlike the editorial signals, a re-grade here carries no decision-time
/// hazard: a grade is an opinion about a stored report, never a feature of a
/// decision.
const REPORT_GRADING_VERSION: &str = "v8";
const FAILURE_CLASSIFICATION_VERSION: &str = "v2";

/// One sequential worker in the singleton scheduler process. No Jev network
/// work is awaited by report creation, ingestion, or the execution cycle.
/// Disabled deployments do not create a polling worker.
pub(crate) async fn run_observation_loop(state: AppState) {
    if availability(&state).is_none() {
        return;
    }
    loop {
        let recomputed = recompute_numeric_measurements(&state).await;
        let grades = grade_reports(&state).await;
        let failures = classify_unknown_failures(&state).await;
        let editorial = crate::editorial_research::score_items_with_jev(&state).await;
        info!(
            ?recomputed,
            ?grades,
            ?failures,
            ?editorial,
            "Jev observation cycle finished"
        );
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
    }
}

fn availability(state: &AppState) -> Option<Box<JevConfig>> {
    match JevConfig::from_yaml(&state.config) {
        JevAvailability::Ready(cfg) => Some(cfg),
        _ => None,
    }
}

/// Grades recent completed Decision Reports on the judgements
/// `decision_quality.rs` cannot make.
///
/// Those eleven checks verify structure and the presence of evidence. These
/// ask whether the prose is actually supported by the evidence sitting beside
/// it, whether the stated market view agrees with the positions taken, and how
/// much of the reasoning is specific rather than boilerplate.
///
/// Observational only, and attached to nothing: the grade is stored against
/// the report id and is never read by the Trading Manager.
pub(crate) async fn grade_reports(state: &AppState) -> JsonValue {
    let Some(cfg) = availability(state) else {
        return json!({"status": "disabled"});
    };
    let judged = match jev_store::judged_subjects_at_version(
        &state.pool,
        jev_store::PURPOSE_REPORT_GRADING,
        REPORT_GRADING_VERSION,
    )
    .await
    {
        Ok(judged) => judged,
        Err(err) => {
            warn!(error = %err, "reading graded reports");
            return json!({"status": "error", "stage": "select"});
        }
    };

    let rows = match sqlx::query(
        "SELECT id, report_json, request_json FROM decision_reports
         WHERE status = 'completed' AND report_json IS NOT NULL
         ORDER BY created_at DESC, id DESC
         LIMIT 40",
    )
    .fetch_all(&state.pool)
    .await
    .context("reading completed reports for Jev grading")
    {
        Ok(rows) => rows.iter().map(row_to_json).collect::<Vec<_>>(),
        Err(err) => {
            warn!(error = %err, "reading completed reports for Jev grading");
            return json!({"status": "error", "stage": "select"});
        }
    };

    let mut graded = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let Some(report_id) = row.get("id").and_then(JsonValue::as_i64) else {
            continue;
        };
        let subject = report_id.to_string();
        if judged.contains(&subject) {
            continue;
        }
        let Some(report) = parse_embedded(row.get("report_json")) else {
            continue;
        };
        // The indicator snapshot the model actually saw, taken from the stored
        // request rather than the live table. Today's indicators would be
        // lookahead: grading a report against numbers that did not exist when
        // it was written.
        let prompt = parse_embedded(row.get("request_json"))
            .map(|request| crate::xai_decision::decision_prompt_user_payload(&request))
            .unwrap_or(JsonValue::Null);
        if graded + failed >= REVIEW_BATCH_LIMIT {
            break;
        }

        let inputs = grading_inputs(&report, &prompt);
        let jev_state =
            jev_signals::report_grading_state(&inputs.report, &inputs.candidates, &inputs.evidence);
        let questions = jev_signals::report_grading_questions(inputs.candidates.len());
        // No guard needed for a report with nothing to check: zero candidates
        // generate zero claim questions, so there is no conjunction over an
        // empty set to answer vacuously. Earlier versions graded two empty
        // reports at 0.92 and 0.94 -- high-quality-looking and meaningless --
        // which the structure now makes unrepresentable rather than filtered.
        let requested_at = jev_signals::now_rfc3339();
        let request_id = stable_id("jevgrade", &format!("{subject}|{requested_at}"));

        match crate::jev::ask(&cfg, &jev_state, &questions).await {
            Ok(response) => {
                let result = grade_payload(&response.answers, &response.issues, &inputs);
                if let Err(err) = jev_store::record_success(
                    &state.pool,
                    jev_store::RecordedRequest {
                        id: &request_id,
                        created_at: &requested_at,
                        purpose: jev_store::PURPOSE_REPORT_GRADING,
                        subject: Some(&subject),
                        model_requested: &cfg.model,
                        question_count: questions.len() as i64,
                    },
                    &response,
                    Some(&result),
                )
                .await
                {
                    warn!(error = %err, "recording a Jev report grade");
                    failed += 1;
                } else {
                    graded += 1;
                }
            }
            Err(err) => {
                failed += 1;
                warn!(report_id, "Jev report grading failed: {err:#}");
                let _ = jev_store::record_failure(
                    &state.pool,
                    jev_store::RecordedRequest {
                        id: &request_id,
                        created_at: &requested_at,
                        purpose: jev_store::PURPOSE_REPORT_GRADING,
                        subject: Some(&subject),
                        model_requested: &cfg.model,
                        question_count: questions.len() as i64,
                    },
                    &format!("{err:#}"),
                )
                .await;
                break; // Stop this batch on provider failure; the next cycle can retry.
            }
        }
    }

    if graded > 0 || failed > 0 {
        info!(graded, failed, "Jev report grading completed");
    }
    json!({
        "status": if failed > 0 && graded == 0 { "error" } else { "ok" },
        "graded": graded,
        "failed": failed,
        "admission": "observational_only",
    })
}

/// Builds the two halves of the grading state.
///
/// A stored prompt can run to six figures of tokens, far beyond Jev's window,
/// so the evidence is the report's own signal metadata -- the material each
/// candidate cited. That makes `rationale_supported` a question about whether
/// the prose matches the numbers next to it, which is the answerable version.
/// Returns the two halves of the state plus how many candidate claims have
/// signals to be checked against.
///
/// `selected_assets` is the watchlist and `suggested_trades` is what was
/// actually proposed, and they are routinely different sizes -- four selected
/// against one proposed in both graded reports that had any. Only the proposed
/// ones carry `strategy_metadata`, so asking about the whole watchlist asks
/// about claims with nothing to check. The list is therefore filtered to the
/// symbols that do have signals, so question and evidence are aligned by
/// construction rather than by an instruction asking Jev to ignore the rest.
/// At most this many candidates are judged in one request.
///
/// Two report-level questions share the budget with them, and `jev.max_questions_per_request`
/// is 12. Any candidate beyond this is reported as excluded rather than dropped.
const MAX_JUDGED_CANDIDATES: usize = 10;

struct GradingInputs {
    report: JsonValue,
    candidates: Vec<JsonValue>,
    evidence: Vec<JsonValue>,
    coverage: JsonValue,
    /// Deterministic numeric findings, settled before any model call.
    numeric: Vec<JsonValue>,
}

/// Builds the grading state, aligned by index, with explicit coverage.
///
/// Evidence is the technical, Markov and Quiver material persisted inside the
/// stored request -- what the model was given, never what it wrote and never
/// today's tables. A candidate with no snapshot cannot be judged, and saying
/// so is the point: report 306 listed five candidates and had indicators for
/// four, and a grade that silently covered 4/5 read as if it covered all five.
fn grading_inputs(report: &JsonValue, prompt: &JsonValue) -> GradingInputs {
    let by_symbol = |block: &str| -> std::collections::HashMap<String, JsonValue> {
        prompt
            .get(block)
            .and_then(|value| value.get("signals"))
            .and_then(JsonValue::as_array)
            .map(|signals| {
                signals
                    .iter()
                    .filter_map(|signal| {
                        Some((signal.get("symbol")?.as_str()?.to_string(), signal.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let indicators = by_symbol("daily_indicators");
    let markov = by_symbol("markov_method");
    let quiver = by_symbol("quiver_signals");

    let selected: Vec<&JsonValue> = report
        .get("selected_assets")
        .and_then(JsonValue::as_array)
        .map(|assets| assets.iter().collect())
        .unwrap_or_default();

    let mut candidates = Vec::new();
    let mut evidence = Vec::new();
    let mut excluded = Vec::new();
    let mut numeric_summaries = Vec::new();
    for asset in &selected {
        let Some(symbol) = asset.get("symbol").and_then(JsonValue::as_str) else {
            continue;
        };
        // Quiver is US-only by design, so its absence is not a coverage gap.
        // A missing indicator snapshot is: without it there is nothing to
        // check the technical assertions against.
        let Some(indicator) = indicators.get(symbol) else {
            excluded.push(json!({"symbol": symbol, "reason": "no_indicator_snapshot_in_prompt"}));
            continue;
        };
        if candidates.len() >= MAX_JUDGED_CANDIDATES {
            excluded.push(json!({"symbol": symbol, "reason": "beyond_question_budget"}));
            continue;
        }
        let note = asset
            .get("notes")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let candidate_evidence = json!({
            "symbol": symbol,
            "daily_indicators": indicator,
            "markov": markov.get(symbol),
            "quiver": quiver.get(symbol).map(|signal| json!({
                "signal": signal.get("signal"),
                "direction": signal.get("direction"),
                "confidence": signal.get("confidence"),
                "run_date": signal.get("run_date"),
            })),
        });
        // Settled by comparison before the model is asked anything, and handed
        // to it so it judges wording against verified figures rather than
        // re-deriving them.
        let checks = crate::jev_numeric::numeric_checks(note, &candidate_evidence);
        numeric_summaries.push(json!({
            "symbol": symbol,
            "summary": crate::jev_numeric::summarize(&checks),
            "checks": checks
                .iter()
                .map(|check| json!({
                    "quoted": check.quoted,
                    "field": check.field,
                    "actual": check.actual,
                    "verdict": check.verdict.as_str(),
                    "excerpt": check.excerpt,
                }))
                .collect::<Vec<_>>(),
        }));
        candidates.push(json!({
            "symbol": symbol,
            "note": asset.get("notes"),
            "numeric_checks": checks
                .iter()
                .map(|check| json!({
                    "quoted": check.quoted,
                    "field": check.field,
                    "actual": check.actual,
                    "verdict": check.verdict.as_str(),
                }))
                .collect::<Vec<_>>(),
        }));
        evidence.push(json!({
            "symbol": symbol,
            "daily_indicators": indicator,
            "markov": markov.get(symbol),
            // The summary fields only. `top_events` carries named individuals
            // and is far larger than anything a note cites.
            "quiver": quiver.get(symbol).map(|signal| json!({
                "signal": signal.get("signal"),
                "direction": signal.get("direction"),
                "confidence": signal.get("confidence"),
                "run_date": signal.get("run_date"),
            })),
        }));
    }

    let coverage = json!({
        "selected_candidate_count": selected.len(),
        "judged_candidate_count": candidates.len(),
        "excluded": excluded,
        "evidence_sources": ["daily_indicators", "markov_method", "quiver_signals"],
        "note": "A grade covers only the judged candidates. Comparing grades across \
                 reports requires reading the coverage alongside them.",
    });

    GradingInputs {
        report: json!({
            "report_title": report.get("report_title"),
            "market_view": report.get("market_view"),
            "reasoning_steps": report.get("reasoning_steps"),
            "symbol_sentiment": report.get("symbol_sentiment"),
            "suggested_trades": report
                .get("suggested_trades")
                .and_then(JsonValue::as_array)
                .map(|trades| {
                    trades
                        .iter()
                        .map(|trade| json!({
                            "symbol": trade.get("symbol"),
                            "action": trade.get("action"),
                            "strategy_role": trade.get("strategy_role"),
                        }))
                        .collect::<Vec<_>>()
                }),
        }),
        candidates,
        evidence,
        coverage,
        numeric: numeric_summaries,
    }
}

/// The numeric half of a grade, derived from evidence alone.
fn numeric_payload(inputs: &GradingInputs) -> JsonValue {
    let total = |key: &str| {
        inputs
            .numeric
            .iter()
            .filter_map(|entry| entry["summary"][key].as_i64())
            .sum::<i64>()
    };
    json!({
        "matches": total("matches"),
        "differs": total("differs"),
        "not_in_evidence": total("not_in_evidence"),
        "unattributed": total("unattributed"),
        "implausible_attribution": total("implausible_attribution"),
        "per_candidate": inputs.numeric,
        "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
        "method": "the quoted text is tested against what rounding and truncating the stored \
                   value actually produce at the precision written; a discrete field requires \
                   a whole number and exact equality",
    })
}

/// Re-derives the numeric half of every grade computed by an older method.
///
/// No provider call: the evidence is the stored prompt and the arithmetic is
/// local, so a corrected method can be applied to the whole history for
/// nothing. This exists because three different arithmetics once shipped under
/// a single grading version while the worker skipped anything already graded
/// under it -- so production kept findings the code had stopped producing,
/// including two it had specifically been fixed to stop producing.
pub(crate) async fn recompute_numeric_measurements(state: &AppState) -> JsonValue {
    if availability(state).is_none() {
        return json!({"status": "disabled"});
    }
    let stale = match jev_store::grades_with_stale_numeric_method(
        &state.pool,
        crate::jev_numeric::NUMERIC_METHOD_VERSION,
        200,
    )
    .await
    {
        Ok(stale) => stale,
        Err(err) => {
            warn!(error = %err, "reading grades with a stale numeric method");
            return json!({"status": "error", "stage": "select"});
        }
    };
    if stale.is_empty() {
        return json!({"status": "ok", "recomputed": 0});
    }

    let mut recomputed = 0usize;
    for (id, subject) in &stale {
        let Ok(subject_id) = subject.parse::<i64>() else {
            continue;
        };
        let row =
            sqlx::query("SELECT report_json, request_json FROM decision_reports WHERE id = $1")
                .bind(subject_id)
                .fetch_optional(&state.pool)
                .await;
        let Ok(Some(row)) = row else { continue };
        let row = row_to_json(&row);
        let Some(report) = parse_embedded(row.get("report_json")) else {
            continue;
        };
        let prompt = parse_embedded(row.get("request_json"))
            .map(|request| crate::xai_decision::decision_prompt_user_payload(&request))
            .unwrap_or(JsonValue::Null);

        let inputs = grading_inputs(&report, &prompt);
        let superseded = json!({
            "reason": "recomputed by a corrected numeric method",
            "superseded_at": jev_signals::now_rfc3339(),
        });
        if let Err(err) = jev_store::replace_numeric_measurement(
            &state.pool,
            id,
            &numeric_payload(&inputs),
            &superseded,
        )
        .await
        {
            warn!(error = %err, "replacing a numeric measurement");
        } else {
            recomputed += 1;
        }
    }
    info!(
        recomputed,
        method = crate::jev_numeric::NUMERIC_METHOD_VERSION,
        "Jev numeric measurements recomputed"
    );
    json!({
        "status": "ok",
        "recomputed": recomputed,
        "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
        "provider_calls": 0,
    })
}

/// Composes the deterministic numeric findings and the model's wording
/// verdicts, keeping them apart.
fn grade_payload(
    answers: &std::collections::BTreeMap<String, crate::jev::Answer>,
    issues: &[crate::jev::JevAnswerIssue],
    inputs: &GradingInputs,
) -> JsonValue {
    let noul = |id: &str| match answers.get(id) {
        Some(crate::jev::Answer::Noul { noul }) => Some(*noul),
        _ => None,
    };

    let mut fair = 0i64;
    let mut overstated = 0i64;
    let mut misdescribes = 0i64;
    let mut no_claim = 0i64;
    let mut unanswered = 0i64;
    let mut low_confidence = 0i64;
    let mut verdicts = Vec::new();

    for (index, candidate) in inputs.candidates.iter().enumerate() {
        let id = jev_signals::claim_question_id(index);
        let (verdict, confidence, probabilities) = match answers.get(&id) {
            Some(crate::jev::Answer::Choice {
                choice,
                confidence,
                probabilities,
            }) => (Some(choice.as_str()), *confidence, Some(probabilities)),
            _ => (None, None, None),
        };
        match verdict {
            Some(jev_signals::WORDING_FAIR) => fair += 1,
            Some(jev_signals::WORDING_OVERSTATED) => overstated += 1,
            Some(jev_signals::WORDING_MISDESCRIBES) => misdescribes += 1,
            Some(jev_signals::WORDING_NONE) => no_claim += 1,
            _ => unanswered += 1,
        }
        if confidence.is_some_and(|value| value < jev_signals::LOW_CONFIDENCE_THRESHOLD) {
            low_confidence += 1;
        }

        // The probability of the option returned, and of the nearest
        // alternative. `confidence` describes how peaked the distribution is,
        // not how likely the selected option is, so these are the numbers to
        // reason about when asking how firmly a verdict was held.
        let selected_probability =
            verdict.and_then(|choice| probabilities.and_then(|values| values.get(choice)).copied());
        let runner_up = probabilities.and_then(|values| {
            values
                .iter()
                .filter(|(option, _)| Some(option.as_str()) != verdict)
                .max_by(|left, right| {
                    left.1
                        .partial_cmp(right.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(option, value)| (option.clone(), *value))
        });

        verdicts.push(json!({
            "symbol": candidate.get("symbol"),
            "verdict": verdict,
            "confidence": confidence,
            "selected_probability": selected_probability,
            "runner_up": runner_up.as_ref().map(|(option, _)| option.clone()),
            "runner_up_probability": runner_up.as_ref().map(|(_, value)| *value),
            "margin": selected_probability
                .zip(runner_up.as_ref().map(|(_, value)| *value))
                .map(|(selected, runner_up)| selected - runner_up),
            "low_confidence": confidence
                .is_some_and(|value| value < jev_signals::LOW_CONFIDENCE_THRESHOLD),
        }));
    }

    json!({
        "version": REPORT_GRADING_VERSION,
        "evidence_scope": "decision_time_prompt_snapshot_not_independent_source_verification",
        "granularity": "one_verdict_per_candidate_note",
        // Settled by comparison, not by a model. These do not vary between
        // runs and carry no confidence because none is needed.
        "numeric_checks": numeric_payload(inputs),
        // Model judgements about wording only.
        "wording_verdict_counts": {
            "fair": fair,
            "overstated": overstated,
            "misdescribes_category": misdescribes,
            "no_qualitative_claim": no_claim,
            "unanswered": unanswered,
        },
        "wording_verdicts": verdicts,
        "confidence_flag": {
            "low_confidence_count": low_confidence,
            "threshold": jev_signals::LOW_CONFIDENCE_THRESHOLD,
            "meaning": "`confidence` is how peaked the answer distribution is, not the \
                        probability of the option returned. This threshold is a hand-picked \
                        review flag, not a calibrated accuracy boundary. Read \
                        `selected_probability` and `margin` for how firmly a verdict was held.",
        },
        "coverage": &inputs.coverage,
        "view_consistent": noul("view_consistent"),
        "specificity": answers
            .get("specificity")
            .and_then(crate::jev::Answer::normalized_score),
        "unanswered_questions": issues.iter().map(|issue| issue.id.clone()).collect::<Vec<_>>(),
        "admission": "observational_only",
        "interpretation": "Numeric findings are comparisons and stand on their own. Wording \
                           verdicts are model classifications, not confirmed findings, and are \
                           not validated against adjudicated examples. `overstated` and \
                           `misdescribes_category` mark wording worth a human reading against \
                           docs/jev-adjudication-rubric.md; neither settles an interpretation \
                           on its own.",
        "safety": "A grade records an opinion about a stored report. It cannot approve a \
                   report, override a Trading Manager gate, create a queue entry, or reach Saxo.",
    })
}

/// Names Decision Report failures the substring cascade could not.
///
/// `local_failure_category` falls through to `other` whenever a provider
/// rewords an error, and `other` is where a real pattern goes to hide. This
/// asks Jev to pick from the categories the runtime already handles, so an
/// answer is always actionable.
pub(crate) async fn classify_unknown_failures(state: &AppState) -> JsonValue {
    let Some(cfg) = availability(state) else {
        return json!({"status": "disabled"});
    };
    let judged = match jev_store::judged_subjects_at_version(
        &state.pool,
        jev_store::PURPOSE_ERROR_CLASSIFICATION,
        FAILURE_CLASSIFICATION_VERSION,
    )
    .await
    {
        Ok(judged) => judged,
        Err(err) => {
            warn!(error = %err, "reading classified failures");
            return json!({"status": "error", "stage": "select"});
        }
    };

    let rows = match sqlx::query(
        "SELECT id, status, error_text FROM decision_reports
         WHERE error_text IS NOT NULL AND error_text <> ''
         ORDER BY created_at DESC, id DESC
         LIMIT 60",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows.iter().map(row_to_json).collect::<Vec<_>>(),
        Err(err) => {
            warn!(error = %err, "reading failed reports for Jev classification");
            return json!({"status": "error", "stage": "select"});
        }
    };

    let questions = jev_signals::error_classification_questions(KNOWN_FAILURE_CODES);
    let mut classified = 0usize;
    let mut failed = 0usize;

    for row in rows {
        let Some(id) = row.get("id").and_then(JsonValue::as_i64) else {
            continue;
        };
        let subject = id.to_string();
        if judged.contains(&subject) {
            continue;
        }
        let error_text = row
            .get("error_text")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        // Only the ones the deterministic cascade could not name. It stays the
        // authority for everything it recognises.
        if crate::decision_provider_state::local_failure_category_for(error_text) != "other" {
            continue;
        }
        if classified + failed >= REVIEW_BATCH_LIMIT {
            break;
        }

        let status = row
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let jev_state = jev_signals::error_classification_state(status, error_text);
        let now = Utc::now().to_rfc3339();
        let request_id = stable_id("jeverror", &format!("{subject}|{now}"));

        match crate::jev::ask(&cfg, &jev_state, &questions).await {
            Ok(response) => {
                let (category, confidence) = match response.answers.get("category") {
                    Some(crate::jev::Answer::Choice {
                        choice, confidence, ..
                    }) => (Some(choice.clone()), *confidence),
                    _ => (None, None),
                };
                let result = json!({
                    "version": FAILURE_CLASSIFICATION_VERSION,
                    "cascade_category": "other",
                    "jev_category": category,
                    "confidence": confidence,
                    "admission": "observational_only",
                    "safety": "A category records an opinion about a stored failure. It cannot \
                               retry a request, change a gate, or reach Saxo.",
                });
                if let Err(err) = jev_store::record_success(
                    &state.pool,
                    jev_store::RecordedRequest {
                        id: &request_id,
                        created_at: &now,
                        purpose: jev_store::PURPOSE_ERROR_CLASSIFICATION,
                        subject: Some(&subject),
                        model_requested: &cfg.model,
                        question_count: questions.len() as i64,
                    },
                    &response,
                    Some(&result),
                )
                .await
                {
                    warn!(error = %err, "recording a Jev failure classification");
                    failed += 1;
                } else {
                    classified += 1;
                }
            }
            Err(err) => {
                failed += 1;
                warn!(report_id = id, "Jev failure classification failed: {err:#}");
                let _ = jev_store::record_failure(
                    &state.pool,
                    jev_store::RecordedRequest {
                        id: &request_id,
                        created_at: &now,
                        purpose: jev_store::PURPOSE_ERROR_CLASSIFICATION,
                        subject: Some(&subject),
                        model_requested: &cfg.model,
                        question_count: questions.len() as i64,
                    },
                    &format!("{err:#}"),
                )
                .await;
                break;
            }
        }
    }

    if classified > 0 || failed > 0 {
        info!(classified, failed, "Jev failure classification completed");
    }
    json!({
        "status": if failed > 0 && classified == 0 { "error" } else { "ok" },
        "classified": classified,
        "failed": failed,
        "admission": "observational_only",
    })
}

fn parse_embedded(value: Option<&JsonValue>) -> Option<JsonValue> {
    match value? {
        JsonValue::String(text) => serde_json::from_str(text).ok(),
        other => Some(other.clone()),
    }
}

fn stable_id(prefix: &str, value: &str) -> String {
    format!("{prefix}-{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::Answer;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn disabled_worker_exits_without_reading_tables_or_calling_a_provider() {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let state = AppState {
            config_path: std::path::PathBuf::from("test.yaml"),
            config: serde_yaml::from_str("jev:\n  enabled: false\n").unwrap(),
            db_url: "sqlite::memory:".into(),
            pool,
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run_observation_loop(state),
        )
        .await
        .expect("disabled worker exits instead of polling");
    }

    fn quiver_signal(symbol: &str, signal: f64) -> JsonValue {
        json!({
            "symbol": symbol,
            "ticker": symbol.split(':').next().unwrap_or(symbol),
            "signal": signal,
            "direction": if signal >= 0.0 { "bullish" } else { "bearish" },
            "confidence": 0.8001627326011658,
            "run_date": "2026-09-17",
            "top_events": [{"representative": "A Name", "amount": 1001.0}],
        })
    }

    fn markov_signal(symbol: &str, signed_signal: f64, state: &str) -> JsonValue {
        json!({
            "symbol": symbol,
            "state": state,
            "run_date": "2026-09-17",
            "direction": if signed_signal >= 0.0 { "long" } else { "short" },
            "signed_signal": signed_signal,
            "conviction": signed_signal.abs(),
            "bull_prob": 0.53,
            "bear_prob": 0.18,
            "sideways_prob": 0.29,
            "horizon_days": 5,
            "close": 421.2,
            "close_dkk": 291.12,
            "currency": "NOK",
        })
    }

    /// Builds a candidate's indicator snapshot in the shape the stored prompt
    /// carries, taken from a real one.
    fn indicator_signal(symbol: &str) -> JsonValue {
        json!({
            "uic": 9_751_025,
            "symbol": symbol,
            "close": 421.2,
            "close_dkk": 291.1250121116638,
            "currency": "NOK",
            "asset_type": "Stock",
            "sma20": 402.93,
            "sma50": 384.954,
            "sma200": 326.2625,
            "rsi14": 66.6411359502835,
            "atr14": 10.012015427677552,
            "macd_histogram": 1.4962398451997796,
            "reward_risk": 0.23971197580912282,
            "sentiment": "BUY",
            "trend_bias": "bullish",
            "confluences": null,
            "confluence_count": 5,
            "min_confluences": 3,
            "support": {
                "break_risk": 0.06147651902511747,
                "break_risk_label": "low",
                "confidence": 0.7523809523809524,
                "touch_count": 1,
                "nearest_support": 391.0,
                "next_support": 377.05,
                "history_coverage": 0.9523809523809523,
                "downside_to_support_pct": 7.169990503323834,
                "downside_after_break_pct": 3.567774936061378,
            },
        })
    }

    /// The figures the notes actually cite, from all three prompt blocks.
    ///
    /// Report 304's note cited "0.061 support break risk" -- exactly
    /// `support.break_risk` in the snapshot the model was given. Report 306's
    /// cited Quiver at -0.429 and +0.575, matching the stored -0.429036 and
    /// +0.574918. Each was graded unsupported in turn, purely because the
    /// block carrying it had not been joined yet.
    #[test]
    fn the_evidence_carries_technicals_markov_and_quiver() {
        let report = json!({
            "selected_assets": [{
                "symbol": "AAPL:xnas",
                "notes": "5 confluences, 0.061 break risk, +0.429 Bull Markov, -0.429 Quiver",
            }],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("AAPL:xnas")]},
            "markov_method": {"signals": [markov_signal("AAPL:xnas", 0.429226, "Bull")]},
            "quiver_signals": {"signals": [quiver_signal("AAPL:xnas", -0.4290364384651184)]},
        });
        let inputs = grading_inputs(&report, &prompt);

        assert_eq!(inputs.candidates.len(), 1);
        let evidence = &inputs.evidence[0];
        assert_eq!(evidence["daily_indicators"]["confluence_count"], 5);
        assert_eq!(evidence["markov"]["signed_signal"], 0.429226);
        assert_eq!(evidence["quiver"]["signal"], -0.4290364384651184);
        assert!(
            evidence["quiver"].get("top_events").is_none(),
            "Quiver's event list names individuals and is larger than anything a note cites"
        );
    }

    /// Report 306 listed five candidates and had indicator snapshots for four.
    /// A grade that silently covered four of five reads as though it covered
    /// all five.
    #[test]
    fn a_candidate_without_evidence_is_excluded_by_name_and_reason() {
        let report = json!({
            "selected_assets": [
                {"symbol": "PANW:xnas", "notes": "a"},
                {"symbol": "UBER:xnys", "notes": "b"},
            ],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("PANW:xnas")]},
        });
        let inputs = grading_inputs(&report, &prompt);

        assert_eq!(inputs.coverage["selected_candidate_count"], 2);
        assert_eq!(inputs.coverage["judged_candidate_count"], 1);
        assert_eq!(inputs.coverage["excluded"][0]["symbol"], "UBER:xnys");
        assert_eq!(
            inputs.coverage["excluded"][0]["reason"],
            "no_indicator_snapshot_in_prompt"
        );
    }

    /// Quiver is US-only by design, so its absence on a European name is not a
    /// coverage gap and must not exclude the candidate.
    #[test]
    fn a_european_candidate_is_judged_without_quiver() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.429, "Bull")]},
        });
        let inputs = grading_inputs(&report, &prompt);
        assert_eq!(inputs.coverage["judged_candidate_count"], 1);
        assert!(inputs.evidence[0]["quiver"].is_null());
        assert_eq!(
            inputs.coverage["excluded"].as_array().map(Vec::len),
            Some(0)
        );
    }

    /// Evidence must come from what the model was given, never from what it
    /// wrote. Grading a claim against the report's own echo is circular: a
    /// fabricated number, echoed consistently, checks out against itself.
    #[test]
    fn a_report_cannot_supply_the_evidence_it_is_graded_against() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "fresh +0.900 Bull Markov"}],
            "suggested_trades": [{
                "symbol": "EQNR:xosl",
                "strategy_metadata": {"markov": {"signed_signal": 0.9, "state": "Bull"}},
            }],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.429226, "Bull")]},
        });
        let inputs = grading_inputs(&report, &prompt);
        assert_eq!(inputs.evidence[0]["markov"]["signed_signal"], 0.429226);
        assert_eq!(
            serde_json::to_string(&inputs.evidence)
                .expect("serializes")
                .matches("0.9,")
                .count(),
            0,
            "the echoed figure must not appear in the evidence at all"
        );
    }

    /// Zero candidates generate zero claim questions, so there is no
    /// conjunction over an empty set to answer vacuously. Earlier versions
    /// graded two empty reports at 0.92 and 0.94.
    #[test]
    fn a_report_with_nothing_to_check_asks_no_claim_questions() {
        let empty = json!({"selected_assets": [], "suggested_trades": []});
        let inputs = grading_inputs(&empty, &json!({}));
        assert_eq!(inputs.candidates.len(), 0);

        let questions = jev_signals::report_grading_questions(0);
        assert!(!questions.contains_key(&jev_signals::claim_question_id(0)));
        assert_eq!(questions.len(), 2, "only the two report-level questions");

        let payload = grade_payload(&BTreeMap::new(), &[], &inputs);
        assert_eq!(payload["wording_verdict_counts"]["fair"], 0);
        assert_eq!(payload["wording_verdict_counts"]["unanswered"], 0);
        assert_eq!(payload["coverage"]["judged_candidate_count"], 0);
        assert_eq!(payload["numeric_checks"]["matches"], 0);
    }

    fn choice(verdict: &str, confidence: f64, probabilities: &[(&str, f64)]) -> Answer {
        Answer::Choice {
            choice: verdict.to_string(),
            probabilities: probabilities
                .iter()
                .map(|(option, value)| ((*option).to_string(), *value))
                .collect(),
            confidence: Some(confidence),
        }
    }

    /// The two kinds of finding must not share a counter. A figure that
    /// disagrees is arithmetic; wording that overstates is a judgement.
    #[test]
    fn numeric_findings_and_wording_verdicts_are_reported_separately() {
        let report = json!({
            "selected_assets": [{
                "symbol": "EQNR:xosl",
                "notes": "consolidating securely with 5 confluences and 0.900 break risk",
            }],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.429226, "Bull")]},
        });
        let inputs = grading_inputs(&report, &prompt);
        let payload = grade_payload(
            &BTreeMap::from([(
                jev_signals::claim_question_id(0),
                choice(
                    jev_signals::WORDING_OVERSTATED,
                    0.71,
                    &[
                        ("overstated", 0.66),
                        ("fair", 0.3),
                        ("misdescribes_category", 0.04),
                    ],
                ),
            )]),
            &[],
            &inputs,
        );

        // 5 confluences agrees; 0.900 break risk does not.
        assert_eq!(payload["numeric_checks"]["matches"], 1);
        assert_eq!(payload["numeric_checks"]["differs"], 1);
        assert_eq!(payload["wording_verdict_counts"]["overstated"], 1);
        assert_eq!(payload["wording_verdict_counts"]["fair"], 0);
        assert!(
            payload["numeric_checks"]["per_candidate"][0]["checks"]
                .as_array()
                .is_some_and(|checks| !checks.is_empty()),
            "the per-figure detail is retained for adjudication"
        );
    }

    /// `confidence` is how peaked the distribution is, not the probability of
    /// the option returned. I reasoned about it as the latter and called 0.17
    /// "below chance", which is a statement about a quantity this field does
    /// not carry. The probability of the selected option and the margin over
    /// the runner-up are the numbers to reason about, so they are reported.
    #[test]
    fn the_selected_probability_and_margin_are_reported_beside_confidence() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        let prompt = json!({"daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]}});
        let inputs = grading_inputs(&report, &prompt);
        let payload = grade_payload(
            &BTreeMap::from([(
                jev_signals::claim_question_id(0),
                choice(
                    jev_signals::WORDING_FAIR,
                    0.17,
                    &[
                        ("fair", 0.41),
                        ("overstated", 0.39),
                        ("no_qualitative_claim", 0.2),
                    ],
                ),
            )]),
            &[],
            &inputs,
        );
        let verdict = &payload["wording_verdicts"][0];

        assert_eq!(verdict["confidence"], 0.17);
        assert_eq!(verdict["selected_probability"], 0.41);
        assert_eq!(verdict["runner_up"], "overstated");
        assert_eq!(verdict["runner_up_probability"], 0.39);
        let margin = verdict["margin"].as_f64().expect("a margin");
        assert!(
            (margin - 0.02).abs() < 1e-9,
            "a near-tie is visible as a margin rather than inferred from confidence: {margin}"
        );
        assert!(verdict["low_confidence"].as_bool().unwrap());
        assert!(
            payload["confidence_flag"]["meaning"]
                .as_str()
                .unwrap()
                .contains("not the"),
            "the payload states what confidence is, since I had it wrong"
        );
    }

    #[test]
    fn an_unanswered_wording_verdict_carries_no_probabilities() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        let prompt = json!({"daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]}});
        let inputs = grading_inputs(&report, &prompt);
        let payload = grade_payload(&BTreeMap::new(), &[], &inputs);

        assert_eq!(payload["wording_verdict_counts"]["unanswered"], 1);
        assert!(payload["wording_verdicts"][0]["selected_probability"].is_null());
        assert!(payload["wording_verdicts"][0]["margin"].is_null());
        assert_eq!(
            payload["numeric_checks"]["matches"], 1,
            "the numeric finding stands whether or not the model answered"
        );
    }

    /// One tiny fixture proves nothing about the byte budget, and indicator
    /// snapshots are roughly 700 bytes each.
    #[test]
    fn a_full_watchlist_of_candidates_fits_the_configured_state_budget() {
        let symbols: Vec<String> = (0..12).map(|i| format!("SYM{i}:xcse")).collect();
        let report = json!({
            "report_title": "Morning read",
            "market_view": {"summary": "x".repeat(600)},
            "reasoning_steps": (0..6).map(|_| "y".repeat(400)).collect::<Vec<_>>(),
            "symbol_sentiment": symbols
                .iter()
                .map(|s| json!({"symbol": s, "sentiment": "BUY", "confidence": 70.0}))
                .collect::<Vec<_>>(),
            "selected_assets": symbols
                .iter()
                .map(|s| json!({"symbol": s, "notes": "z".repeat(220), "score": 0.8}))
                .collect::<Vec<_>>(),
            "suggested_trades": symbols
                .iter()
                .map(|s| json!({"symbol": s, "action": "BUY", "strategy_role": "w".repeat(120)}))
                .collect::<Vec<_>>(),
        });
        let prompt = json!({
            "daily_indicators": {
                "signals": symbols.iter().map(|s| indicator_signal(s)).collect::<Vec<_>>(),
            },
            "markov_method": {
                "signals": symbols.iter().map(|s| markov_signal(s, 0.42, "Bull")).collect::<Vec<_>>(),
            },
            "quiver_signals": {
                "signals": symbols.iter().map(|s| quiver_signal(s, -0.3)).collect::<Vec<_>>(),
            },
        });
        let inputs = grading_inputs(&report, &prompt);

        assert_eq!(
            inputs.candidates.len(),
            MAX_JUDGED_CANDIDATES,
            "the budget caps judged candidates"
        );
        assert_eq!(
            inputs.coverage["excluded"].as_array().map(Vec::len),
            Some(2),
            "and the two beyond it are named rather than dropped"
        );
        let questions = jev_signals::report_grading_questions(inputs.candidates.len());
        assert_eq!(questions.len(), MAX_JUDGED_CANDIDATES + 2);

        let rendered = serde_json::to_string(&jev_signals::report_grading_state(
            &inputs.report,
            &inputs.candidates,
            &inputs.evidence,
        ))
        .expect("serializes");
        assert!(
            rendered.len() < 24_000,
            "a full watchlist must fit, or every grade on a busy day is refused: {}",
            rendered.len()
        );
    }

    /// A verdict that never fires looks identical to a clean set of reports,
    /// so each one has to be shown reachable before a count of zero means
    /// anything.
    ///
    /// Probes wording only. The numeric side is arithmetic and is covered by
    /// `jev_numeric`'s own tests without spending anything.
    ///
    ///   set -a && . ./.env && set +a && \
    ///     cargo test wording_probe -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "makes a real, billable network call; run explicitly with --ignored"]
    async fn wording_probe_shows_each_verdict_can_fire() {
        let Ok(api_key) = std::env::var("JEV_OPENROUTER_API_KEY") else {
            panic!("JEV_OPENROUTER_API_KEY is not exported");
        };
        let cfg = crate::jev::JevConfig {
            api_key,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "~typesafe/jev-latest".to_string(),
            http_timeout_seconds: 30,
            max_state_chars: 24_000,
            max_questions_per_request: 12,
        };
        // break_risk 0.0614, labelled low; markov Bull at +0.429.
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.429226, "Bull")]},
        });

        for (label, note, expected) in [
            (
                "fair",
                "Constructive setup with low support break risk and a positive Bull Markov \
                 regime.",
                jev_signals::WORDING_FAIR,
            ),
            (
                // Purely about intensity. An earlier fixture said "absolutely
                // no downside risk whatsoever", which came back
                // misdescribes_category at p=0.53 against overstated at 0.47 --
                // a near tie, because claiming there is no risk is arguably a
                // claim about the risk category rather than an overstatement
                // of degree. The margin and confidence reported it correctly;
                // the fixture was the problem.
                "overstated",
                "A spectacular, once-in-a-decade technical setup, far stronger than anything \
                 else on the venue.",
                jev_signals::WORDING_OVERSTATED,
            ),
            (
                "miscategorised",
                "The Markov state is Bear and the technical sentiment is SELL.",
                jev_signals::WORDING_MISDESCRIBES,
            ),
        ] {
            let report = json!({
                "selected_assets": [{"symbol": "EQNR:xosl", "notes": note}],
                "suggested_trades": [],
            });
            let inputs = grading_inputs(&report, &prompt);
            let questions = jev_signals::report_grading_questions(inputs.candidates.len());
            let state = jev_signals::report_grading_state(
                &inputs.report,
                &inputs.candidates,
                &inputs.evidence,
            );
            let response = crate::jev::ask(&cfg, &state, &questions)
                .await
                .expect("probe call succeeds");
            let payload = grade_payload(&response.answers, &response.issues, &inputs);
            let verdict = &payload["wording_verdicts"][0];

            println!(
                "{label:16} -> {} (p={:?}, runner-up {} p={:?}, confidence {:?})",
                verdict["verdict"],
                verdict["selected_probability"],
                verdict["runner_up"],
                verdict["runner_up_probability"],
                verdict["confidence"],
            );
            assert_eq!(
                verdict["verdict"], expected,
                "{label}: a verdict that never fires is indistinguishable from clean reports"
            );
        }
    }
}
