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

/// How far back the grader looks for ungraded reports, and how many it judges
/// per cycle.
///
/// The defaults keep the worker on recent reports. Raising them lets a
/// held-out or historical set be graded without editing code -- grading the
/// backlog is the only way to measure against reports that were never used to
/// tune the checker.
fn grading_window(state: &AppState) -> i64 {
    crate::config::yaml_i64(&state.config, &["jev", "grading_candidate_window"])
        .unwrap_or(40)
        .clamp(1, 5_000)
}

fn grading_batch_limit(state: &AppState) -> usize {
    crate::config::yaml_i64(&state.config, &["jev", "grading_batch_limit"])
        .unwrap_or(REVIEW_BATCH_LIMIT as i64)
        .clamp(1, 500) as usize
}

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
const REPORT_GRADING_VERSION: &str = "v16";

/// The grading version a measurement was taken against.
#[cfg(test)]
pub(crate) fn report_grading_version() -> &'static str {
    REPORT_GRADING_VERSION
}
const FAILURE_CLASSIFICATION_VERSION: &str = "v2";

/// The pause between observation cycles.
pub(crate) const OBSERVATION_CYCLE_SECONDS: i64 = 600;

/// One sequential worker in the singleton scheduler process. No Jev network
/// work is awaited by report creation, ingestion, or the execution cycle.
/// Disabled deployments do not create a polling worker.
pub(crate) async fn run_observation_loop(state: AppState) {
    if availability(&state).is_none() {
        return;
    }
    let mut unrecorded = jev_store::Unrecorded::default();
    loop {
        let started_at = jev_signals::now_rfc3339();
        if unrecorded.pending() > 0 {
            let written = unrecorded.flush(&state.pool).await;
            info!(
                written,
                still_held = unrecorded.pending(),
                "Jev requests held after a failed write"
            );
        }
        let recomputed = recompute_numeric_measurements(&state).await;
        let grades = grade_reports(&state, &mut unrecorded).await;
        let failures = classify_unknown_failures(&state, &mut unrecorded).await;
        let editorial =
            crate::editorial_research::score_items_with_jev(&state, &mut unrecorded).await;
        info!(
            ?recomputed,
            ?grades,
            ?failures,
            ?editorial,
            "Jev observation cycle finished"
        );
        record_cycle(
            &state,
            &started_at,
            &unrecorded,
            &[
                (jev_store::STEP_NUMERIC_RECOMPUTE, &recomputed),
                (jev_store::PURPOSE_REPORT_GRADING, &grades),
                (jev_store::PURPOSE_ERROR_CLASSIFICATION, &failures),
                (jev_store::PURPOSE_EDITORIAL_ITEM, &editorial),
            ],
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(
            OBSERVATION_CYCLE_SECONDS as u64,
        ))
        .await;
    }
}

/// Records the cycle for the operator panel. A failed write is logged and
/// left: the panel then shows the worker as silent, which is the truth about
/// what it can see.
async fn record_cycle(
    state: &AppState,
    started_at: &str,
    unrecorded: &jev_store::Unrecorded,
    steps: &[(&str, &JsonValue)],
) {
    let finished_at = jev_signals::now_rfc3339();
    if let Err(err) =
        jev_store::record_worker_cycle(&state.pool, started_at, &finished_at, steps, unrecorded)
            .await
    {
        warn!(error = %err, "recording the Jev worker cycle");
    }
}

/// Calls Jev and records what the call says about the provider.
///
/// An answer reopens the provider gate; a failure that reached the provider
/// shuts it until the backoff ends. A call refused before any request -- an
/// oversized state, too many questions -- says nothing about the provider and
/// leaves the gate alone.
pub(crate) async fn ask_recorded(
    state: &AppState,
    cfg: &JevConfig,
    jev_state: &JsonValue,
    questions: &std::collections::BTreeMap<String, crate::jev::Question>,
) -> anyhow::Result<crate::jev::JevResponse> {
    let outcome = crate::jev::ask(cfg, jev_state, questions).await;
    let now = Utc::now();
    match &outcome {
        Ok(_) => {
            if let Err(err) = jev_store::note_provider_success(&state.pool, now).await {
                warn!(error = %err, "reopening the Jev provider gate");
            }
        }
        Err(err) => {
            if let Some(failure) =
                crate::jev::JevCallError::of(err).filter(|failure| failure.reached_provider())
            {
                match jev_store::note_provider_failure(
                    &state.pool,
                    now,
                    &jev_store::failure_kind(&failure.message),
                    failure.retry_after_seconds,
                )
                .await
                {
                    Ok(not_before) => info!(%not_before, "Jev provider calls deferred"),
                    Err(err) => warn!(error = %err, "shutting the Jev provider gate"),
                }
            }
        }
    }
    outcome
}

/// The summary for a step that must not call the provider now, or `None`
/// when it may. An unreadable gate fails closed: a provider that asked to be
/// left alone is not asked again because the record of that was unreadable.
pub(crate) async fn deferred_step(state: &AppState) -> Option<JsonValue> {
    match jev_store::provider_deferred_until(&state.pool, Utc::now()).await {
        Ok(None) => None,
        Ok(Some(not_before)) => Some(json!({
            "status": "deferred",
            "not_before": not_before,
            "failed": 0,
        })),
        Err(err) => {
            warn!(error = %err, "reading the Jev provider gate");
            Some(json!({"status": "error", "stage": "provider_state", "failed": 0}))
        }
    }
}

/// Whether a call was refused before anything was sent: a property of the
/// input, so retrying it is pointless and it says nothing about the provider.
pub(crate) fn refused_locally(err: &anyhow::Error) -> bool {
    crate::jev::JevCallError::of(err).is_some_and(|failure| !failure.reached_provider())
}

/// The ledger row for a call that failed, with the attempts it made.
pub(crate) fn failed_call_row(
    meta: jev_store::RecordedRequest<'_>,
    error_text: &str,
    err: &anyhow::Error,
) -> jev_store::RequestRow {
    let attempts = crate::jev::JevCallError::of(err)
        .map(|failure| failure.attempt_log.as_slice())
        .unwrap_or_default();
    jev_store::failed_row(meta, error_text, attempts)
}

/// Writes a ledger row, or holds it for a later cycle when the write fails.
/// Returns whether it was written.
pub(crate) async fn record_or_hold(
    state: &AppState,
    unrecorded: &mut jev_store::Unrecorded,
    row: jev_store::RequestRow,
) -> bool {
    match jev_store::insert_request_row(&state.pool, &row).await {
        Ok(()) => true,
        Err(err) => {
            warn!(
                error = %err,
                id = %row.id,
                status = %row.status,
                cost_usd = ?row.cost_usd,
                "a Jev request could not be recorded; holding it for the next cycle"
            );
            unrecorded.hold(row);
            false
        }
    }
}

/// The backlog fields every step reports, so the panel reads one shape.
fn backlog(waiting: usize, processed: usize, lower_bound: bool) -> JsonValue {
    json!({
        "processed": processed,
        "waiting": waiting,
        "remaining": waiting.saturating_sub(processed),
        "waiting_is_lower_bound": lower_bound,
    })
}

fn merged(mut summary: JsonValue, extra: JsonValue) -> JsonValue {
    if let (Some(summary), JsonValue::Object(extra)) = (summary.as_object_mut(), extra) {
        summary.extend(extra);
    }
    summary
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
pub(crate) async fn grade_reports(
    state: &AppState,
    unrecorded: &mut jev_store::Unrecorded,
) -> JsonValue {
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

    let rows = match sqlx::query(&format!(
        "SELECT id, report_json, request_json FROM decision_reports
         WHERE status = 'completed' AND report_json IS NOT NULL
         ORDER BY created_at DESC, id DESC
         LIMIT {}",
        grading_window(state)
    ))
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

    // Counted before the batch, so the panel sees what the batch limit
    // leaves waiting in the window rather than only what one cycle took.
    let pending = rows
        .into_iter()
        .filter_map(|row| {
            let report_id = row.get("id").and_then(JsonValue::as_i64)?;
            let subject = report_id.to_string();
            if judged.contains(&subject) {
                return None;
            }
            let report = parse_embedded(row.get("report_json"))?;
            Some((report_id, subject, report, row))
        })
        .collect::<Vec<_>>();
    let waiting = pending.len();
    if let Some(deferred) = deferred_step(state).await {
        return merged(deferred, backlog(waiting, 0, false));
    }
    let mut graded = 0usize;
    let mut failed = 0usize;
    let mut held = 0usize;
    let mut refused = 0usize;

    for (report_id, subject, report, row) in pending.into_iter().take(grading_batch_limit(state)) {
        // The indicator snapshot the model actually saw, taken from the stored
        // request rather than the live table. Today's indicators would be
        // lookahead: grading a report against numbers that did not exist when
        // it was written.
        let prompt = parse_embedded(row.get("request_json"))
            .map(|request| crate::xai_decision::decision_prompt_user_payload(&request))
            .unwrap_or(JsonValue::Null);

        let inputs = grading_inputs(&report, &prompt);
        let jev_state = jev_signals::report_grading_state(
            &inputs.report,
            &inputs.candidates,
            &inputs.evidence,
            inputs.policy.as_ref(),
        );
        let questions = jev_signals::report_grading_questions(inputs.candidates.len());
        // No guard needed for a report with nothing to check: zero candidates
        // generate zero claim questions, so there is no conjunction over an
        // empty set to answer vacuously. Earlier versions graded two empty
        // reports at 0.92 and 0.94 -- high-quality-looking and meaningless --
        // which the structure now makes unrepresentable rather than filtered.
        let requested_at = jev_signals::now_rfc3339();
        let request_id = stable_id("jevgrade", &format!("{subject}|{requested_at}"));

        let meta = jev_store::RecordedRequest {
            id: &request_id,
            created_at: &requested_at,
            purpose: jev_store::PURPOSE_REPORT_GRADING,
            subject: Some(&subject),
            model_requested: &cfg.model,
            question_count: questions.len() as i64,
        };
        match ask_recorded(state, &cfg, &jev_state, &questions).await {
            Ok(response) => {
                let result = grade_payload(&response.answers, &response.issues, &inputs);
                let row = jev_store::completed_row(meta, &response, Some(&result));
                if !record_or_hold(state, unrecorded, row).await {
                    failed += 1;
                    held += 1;
                    break; // Accounting is failing: spend nothing more until it records.
                }
                graded += 1;
            }
            Err(err) if refused_locally(&err) => {
                refused += 1;
                warn!(
                    report_id,
                    "Jev report grading refused before sending: {err:#}"
                );
                let row = jev_store::refused_row(meta, &format!("{err:#}"), REPORT_GRADING_VERSION);
                if !record_or_hold(state, unrecorded, row).await {
                    held += 1;
                    break;
                }
            }
            Err(err) => {
                failed += 1;
                warn!(report_id, "Jev report grading failed: {err:#}");
                let row = failed_call_row(meta, &format!("{err:#}"), &err);
                if !record_or_hold(state, unrecorded, row).await {
                    held += 1;
                }
                break; // Stop this batch on provider failure; the gate decides when to retry.
            }
        }
    }

    if graded > 0 || failed > 0 || refused > 0 {
        info!(
            graded,
            failed, held, refused, "Jev report grading completed"
        );
    }
    merged(
        json!({
            "status": if failed > 0 && graded == 0 { "error" } else { "ok" },
            "graded": graded,
            "failed": failed,
            "unrecorded": held,
            "refused": refused,
            "admission": "observational_only",
        }),
        // The window is the deliberate bound, not a truncated selection:
        // reports older than it are never graded by this worker.
        backlog(waiting, graded + refused, false),
    )
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
/// Each candidate asks two questions -- wording and evidence sufficiency --
/// and two report-level questions share the budget, so ten candidates is 22
/// against a configured maximum of 24. Any candidate beyond this is reported
/// as excluded rather than dropped.
const MAX_JUDGED_CANDIDATES: usize = 10;

pub(crate) struct GradingInputs {
    pub(crate) report: JsonValue,
    pub(crate) candidates: Vec<JsonValue>,
    pub(crate) evidence: Vec<JsonValue>,
    pub(crate) coverage: JsonValue,
    /// Decision-time thresholds, as the report recorded them. `None` for
    /// reports written before the prompt carried them.
    pub(crate) policy: Option<JsonValue>,
    /// Deterministic numeric findings, settled before any model call.
    pub(crate) numeric: Vec<JsonValue>,
}

/// The thresholds in force when the report was written, from whichever shape
/// that report recorded them in.
///
/// `decision_time_gate_policy` has been in the prompt since 2026-08-20 and 87
/// stored reports carry it; I added a second `decision_policy` block without
/// checking, and read only mine, so every one of those reported no policy at
/// all. The duplicate is gone and both shapes normalise here.
///
/// Configuration is deliberately not consulted. `min_signed_signal` was 0.15
/// until it was recalibrated to 0.20 on 2026-08-31, so today's value would
/// misjudge any report written before that.
///
/// Availability is per field, because a report can record the Markov gate and
/// not the confluence minimum.
fn decision_time_policy(prompt: &JsonValue) -> Option<JsonValue> {
    let gate = prompt.get("decision_time_gate_policy");
    let legacy = prompt.get("decision_policy");
    let read = |value: Option<&JsonValue>, path: &[&str]| -> Option<JsonValue> {
        let mut cursor = value?;
        for key in path {
            cursor = cursor.get(key)?;
        }
        (!cursor.is_null()).then(|| cursor.clone())
    };

    let min_signed_signal = read(gate, &["markov_starter", "min_signed_signal"])
        .or_else(|| read(legacy, &["markov_gate", "min_signed_signal"]));
    let min_confluences = read(gate, &["daily_technical", "min_confluences"])
        .or_else(|| read(legacy, &["daily_indicators", "min_confluences"]));
    if min_signed_signal.is_none() && min_confluences.is_none() {
        return None;
    }
    Some(json!({
        "markov_gate": {"min_signed_signal": min_signed_signal},
        "daily_indicators": {"min_confluences": min_confluences},
        "provenance": if gate.is_some() { "decision_time_gate_policy" } else { "decision_policy" },
        "interpretation": "The thresholds this report recorded at the time it was written. A \
                           field that is null was not recorded, and a claim about it cannot be \
                           judged. Current configuration is not a substitute: these values \
                           change.",
    }))
}

/// Builds the grading state, aligned by index, with explicit coverage.
///
/// Evidence is the technical, Markov and Quiver material persisted inside the
/// stored request -- what the model was given, never what it wrote and never
/// today's tables. A candidate with no snapshot cannot be judged, and saying
/// so is the point: report 306 listed five candidates and had indicators for
/// four, and a grade that silently covered 4/5 read as if it covered all five.
pub(crate) fn grading_inputs(report: &JsonValue, prompt: &JsonValue) -> GradingInputs {
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
    let listed_markov = by_symbol("markov_method");
    let embedded_markov = crate::markov_method::embedded_prompt_signal_rows(prompt);
    // The compact list first; the embedded rows only for a symbol it left out.
    let markov_for = |symbol: &str| -> (Option<&JsonValue>, Option<&'static str>) {
        match (listed_markov.get(symbol), embedded_markov.get(symbol)) {
            (Some(row), _) => (Some(row), Some("evidence_list")),
            (None, Some(row)) => (Some(row), Some("embedded_run_rows")),
            (None, None) => (None, None),
        }
    };
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
        let (markov, markov_source) = markov_for(symbol);
        let candidate_evidence = json!({
            "symbol": symbol,
            "daily_indicators": indicator,
            "markov": markov,
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
            // Which of the prompt's two Markov lists the evidence came from,
            // so a comparison made against an embedded row is never mistaken
            // for one made against the list.
            "markov_source": markov_source,
            "summary": crate::jev_numeric::summarize(&checks),
            "checks": checks
                .iter()
                .map(|check| json!({
                    "quoted": check.quoted,
                    "field": check.field,
                    "actual": check.actual,
                    // What the note claimed about the figure, and the reason a
                    // verdict of `uncertain_attribution` was reached: the
                    // grammar refusing the wording is a different finding from
                    // no field being attributable at all.
                    "relation": check.relation,
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
                    "relation": check.relation,
                    "verdict": check.verdict.as_str(),
                }))
                .collect::<Vec<_>>(),
        }));
        evidence.push(json!({
            "symbol": symbol,
            "daily_indicators": indicator,
            "markov": markov,
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

    let policy = decision_time_policy(prompt);

    let coverage = json!({
        "decision_policy_available": policy.is_some(),
        "decision_policy_fields": policy
            .as_ref()
            .map(|p| json!({
                "min_signed_signal": !p["markov_gate"]["min_signed_signal"].is_null(),
                "min_confluences": !p["daily_indicators"]["min_confluences"].is_null(),
            })),
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
        policy,
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
    // Identity of the evidence the measurement was made against.
    //
    // The history preserves each prior measurement, and this is what makes the
    // comparison between two of them meaningful: identical fingerprints isolate
    // the method as the only thing that changed, and differing ones say the
    // evidence moved underneath and the measurements are not comparable at all.
    let evidence_sha256 = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_string(&inputs.evidence)
                .unwrap_or_default()
                .as_bytes()
        )
    );
    json!({
        "evidence_sha256": evidence_sha256,
        "matches": total("matches"),
        "differs": total("differs"),
        "not_in_evidence": total("not_in_evidence"),
        "unattributed": total("unattributed"),
        "implausible_attribution": total("implausible_attribution"),
        // Present in the per-candidate detail but previously missing from the
        // totals, so the totals did not account for every figure found.
        "not_a_field_value": total("not_a_field_value"),
        "uncertain_attribution": total("uncertain_attribution"),
        "compared": total("matches") + total("differs"),
        "abstained": total("not_in_evidence")
            + total("unattributed")
            + total("implausible_attribution")
            + total("not_a_field_value")
            + total("uncertain_attribution"),
        "per_candidate": inputs.numeric,
        "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
        "method": "A quoted figure is attributed to a field only when one phrase names it more \
                   closely than any rival, and abstains otherwise. An equality is tested against \
                   what rounding the stored value produces at the precision written, either way \
                   at a half; truncation is not accepted. A discrete field requires a whole \
                   number and exact equality. The comparison grammar is closed: it reads a figure only when its \
                   clause asserts plainly -- nothing negated, past or prospective -- and the \
                   words between the field and the figure are connectives, words of degree or \
                   part of the field's own name, with at most one comparator and that \
                   comparator immediately before the figure. Symbolic operators are read the \
                   same way. Everything outside those forms is recorded \
                   `unsupported_construction` and not compared; `not_read` means no field could \
                   be attributed at all. `compared` plus `abstained` is every figure found. The \
                   grammar rejects 2.9% of attributed figures on the stored corpus, which is \
                   not the same as the share that goes uncompared: a recognised relation can \
                   still end in `not_in_evidence` or `implausible_attribution`. `relation` is \
                   the parsed claim and `verdict` is the outcome; only `matches` and `differs` \
                   were compared. A threshold is tested at face value, and the rounding \
                   allowance applies to equality alone. A field's name is matched with \
                   underscores, hyphens and slashes read as spaces; a name followed by a \
                   determiner is a verb and names nothing; a name in a different sentence \
                   names nothing; and `N/M` is read as a count over its minimum from the \
                   notation alone. Markov evidence comes from the prompt's compact list and, \
                   for a symbol absent from it, from the full signal rows the prompt embedded \
                   under markov_method.latest_run until 2026-08-03 -- never from a report's own \
                   output; `markov_source` names which. Known limitations are listed in \
                   docs/jev-numeric-grammar.md.",
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
/// The outcome of looking up the report a grade refers to.
///
/// The distinction is the whole point: one of these is permanent and the other
/// is a moment's unavailability, and a single `let Ok(Some(..))` treated them
/// alike.
enum ReportRow<T> {
    Found(T),
    /// The lookup succeeded and there is no such report.
    Gone,
    /// The lookup itself failed.
    Unreadable(sqlx::Error),
}

impl<T> ReportRow<T> {
    fn of(result: Result<Option<T>, sqlx::Error>) -> Self {
        match result {
            Ok(Some(row)) => Self::Found(row),
            Ok(None) => Self::Gone,
            Err(err) => Self::Unreadable(err),
        }
    }
}

/// How many stale grades one recompute pass selects.
const RECOMPUTE_SELECTION_LIMIT: i64 = 200;

pub(crate) async fn recompute_numeric_measurements(state: &AppState) -> JsonValue {
    if availability(state).is_none() {
        return json!({"status": "disabled"});
    }
    let stale = match jev_store::grades_with_stale_numeric_method(
        &state.pool,
        crate::jev_numeric::NUMERIC_METHOD_VERSION,
        RECOMPUTE_SELECTION_LIMIT,
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
        return merged(
            json!({"status": "ok", "recomputed": 0, "failed": 0}),
            backlog(0, 0, false),
        );
    }
    let waiting = stale.len();

    let mut recomputed = 0usize;
    let mut unable = Vec::new();
    let mut blocked = 0usize;
    // A grade that can never be recomputed is marked so the selection stops
    // handing it back. Skipping without recording was a live-lock: the same
    // two hundred old reports filled every batch, `recomputed=0 unable=200`
    // cycle after cycle, while grades that could be recomputed waited behind
    // them. Named rather than stamped -- the absence stays an absence.
    let block = |unable: &mut Vec<JsonValue>, id: &str, subject: &str, reason: &'static str| {
        unable.push(json!({"subject": subject, "reason": reason, "blocked": true}));
        (id.to_string(), reason)
    };
    let mut to_block: Vec<(String, &'static str)> = Vec::new();
    for (id, subject) in &stale {
        let Ok(subject_id) = subject.parse::<i64>() else {
            to_block.push(block(
                &mut unable,
                id,
                subject,
                "subject is not a report id",
            ));
            continue;
        };
        let row =
            sqlx::query("SELECT report_json, request_json FROM decision_reports WHERE id = $1")
                .bind(subject_id)
                .fetch_optional(&state.pool)
                .await;
        let row = match ReportRow::of(row) {
            ReportRow::Found(row) => row,
            ReportRow::Gone => {
                to_block.push(block(&mut unable, id, subject, "report row is gone"));
                continue;
            }
            // Not blocked. A database unavailable for a moment is not a report
            // that no longer exists, and `let Ok(Some(row)) = ...` read the two
            // as the same thing -- so one timeout would have excluded a valid
            // grade from every future recomputation.
            ReportRow::Unreadable(err) => {
                warn!(error = %err, subject, "reading a report for numeric recomputation");
                unable.push(json!({
                    "subject": subject,
                    "reason": "report row could not be read",
                    "blocked": false,
                }));
                continue;
            }
        };
        let row = row_to_json(&row);
        let Some(report) = parse_embedded(row.get("report_json")) else {
            to_block.push(block(
                &mut unable,
                id,
                subject,
                "report json could not be decoded",
            ));
            continue;
        };
        let Some(prompt) = parse_embedded(row.get("request_json"))
            .map(|request| crate::xai_decision::decision_prompt_user_payload(&request))
        else {
            to_block.push(block(
                &mut unable,
                id,
                subject,
                "request json could not be decoded",
            ));
            continue;
        };
        // A prompt carrying no indicator block is absent evidence, not a
        // report that selected nothing. Recomputing against it yields zero
        // checks, and stamping that with the current method would record a
        // clean revalidation of something never examined. The earlier
        // measurement is left exactly where it is.
        if prompt
            .get("daily_indicators")
            .and_then(|block| block.get("signals"))
            .and_then(JsonValue::as_array)
            .is_none()
        {
            to_block.push(block(
                &mut unable,
                id,
                subject,
                "stored prompt carries no daily indicator snapshot",
            ));
            continue;
        }

        let inputs = grading_inputs(&report, &prompt);
        if let Err(err) = jev_store::replace_numeric_measurement(
            &state.pool,
            id,
            &numeric_payload(&inputs),
            "recomputed by a corrected numeric method",
        )
        .await
        {
            warn!(error = %err, "replacing a numeric measurement");
            unable.push(json!({"subject": subject, "reason": "write failed"}));
        } else {
            recomputed += 1;
        }
    }
    for (id, reason) in &to_block {
        if let Err(err) = jev_store::mark_recompute_blocked(&state.pool, id, reason).await {
            warn!(error = %err, "marking a grade unrecomputable");
        } else {
            blocked += 1;
        }
    }
    info!(
        recomputed,
        unable = unable.len(),
        blocked,
        method = crate::jev_numeric::NUMERIC_METHOD_VERSION,
        "Jev numeric measurements recomputed"
    );
    // Faults, not verdicts: a blocked grade is recorded and leaves the
    // selection, while one that could not be read or written comes back.
    let faults = unable
        .iter()
        .filter(|entry| entry.get("blocked").and_then(JsonValue::as_bool) != Some(true))
        .count();
    json!({
        "status": "ok",
        "recomputed": recomputed,
        "processed": recomputed,
        "failed": faults,
        "waiting": waiting,
        "remaining": waiting.saturating_sub(recomputed + blocked),
        "waiting_is_lower_bound": waiting as i64 >= RECOMPUTE_SELECTION_LIMIT,
        // A measurement that could not be re-derived keeps its earlier
        // findings and its earlier method version, so it stays visibly stale
        // rather than reading as revalidated.
        // Named, not counted as done, and capped: the whole list ran to
        // twenty thousand characters in a log line every ten minutes.
        "unable_to_recompute": unable.iter().take(10).collect::<Vec<_>>(),
        "unable_to_recompute_count": unable.len(),
        "newly_blocked": blocked,
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
    let mut unsupported = 0i64;
    let mut no_claim = 0i64;
    let mut unanswered = 0i64;
    let mut sufficiency_answered = 0i64;
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
        // Read independently of the wording verdict, so a note can be recorded
        // as both overstating what the evidence shows and asserting something
        // it does not contain.
        let sufficiency = match answers.get(&jev_signals::sufficiency_question_id(index)) {
            Some(crate::jev::Answer::Noul { noul }) => Some(*noul),
            _ => None,
        };
        if sufficiency.is_some() {
            sufficiency_answered += 1;
        }
        if sufficiency.is_some_and(|value| value >= 0.5) {
            unsupported += 1;
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
            "asserts_absent_evidence": sufficiency,
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
        // A separate axis, answered per candidate alongside the wording
        // verdict rather than competing with it.
        "evidence_sufficiency": {
            "asserts_absent_evidence": unsupported,
            // Without these a count of zero says nothing: all answered and
            // none above the threshold reads identically to none answered at
            // all, and only one of those is a result.
            "answered": sufficiency_answered,
            "unanswered": inputs.candidates.len() as i64 - sufficiency_answered,
            "judged_candidates": inputs.candidates.len() as i64,
            "threshold": 0.5,
            "meaning": "Counts candidates whose note states a value or condition for something \
                        the evidence does not contain. Independent of the wording verdict: a \
                        note can overstate what the evidence shows and also assert something \
                        absent from it. `asserts_absent_evidence` is a count out of `answered`, \
                        never out of `judged_candidates`.",
        },
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
pub(crate) async fn classify_unknown_failures(
    state: &AppState,
    unrecorded: &mut jev_store::Unrecorded,
) -> JsonValue {
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
    // Only the ones the deterministic cascade could not name. It stays the
    // authority for everything it recognises.
    let pending = rows
        .into_iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(JsonValue::as_i64)?;
            let subject = id.to_string();
            if judged.contains(&subject) {
                return None;
            }
            let error_text = row
                .get("error_text")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            (crate::decision_provider_state::local_failure_category_for(error_text) == "other")
                .then_some((id, subject, row))
        })
        .collect::<Vec<_>>();
    let waiting = pending.len();
    if let Some(deferred) = deferred_step(state).await {
        return merged(deferred, backlog(waiting, 0, false));
    }
    let mut classified = 0usize;
    let mut failed = 0usize;
    let mut held = 0usize;
    let mut refused = 0usize;

    for (id, subject, row) in pending.into_iter().take(REVIEW_BATCH_LIMIT) {
        let error_text = row
            .get("error_text")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();

        let status = row
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let jev_state = jev_signals::error_classification_state(status, error_text);
        let now = Utc::now().to_rfc3339();
        let request_id = stable_id("jeverror", &format!("{subject}|{now}"));
        let meta = jev_store::RecordedRequest {
            id: &request_id,
            created_at: &now,
            purpose: jev_store::PURPOSE_ERROR_CLASSIFICATION,
            subject: Some(&subject),
            model_requested: &cfg.model,
            question_count: questions.len() as i64,
        };

        match ask_recorded(state, &cfg, &jev_state, &questions).await {
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
                let row = jev_store::completed_row(meta, &response, Some(&result));
                if !record_or_hold(state, unrecorded, row).await {
                    failed += 1;
                    held += 1;
                    break; // Accounting is failing: spend nothing more until it records.
                }
                classified += 1;
            }
            Err(err) if refused_locally(&err) => {
                refused += 1;
                warn!(
                    report_id = id,
                    "Jev failure classification refused before sending: {err:#}"
                );
                let row = jev_store::refused_row(
                    meta,
                    &format!("{err:#}"),
                    FAILURE_CLASSIFICATION_VERSION,
                );
                if !record_or_hold(state, unrecorded, row).await {
                    held += 1;
                    break;
                }
            }
            Err(err) => {
                failed += 1;
                warn!(report_id = id, "Jev failure classification failed: {err:#}");
                let row = failed_call_row(meta, &format!("{err:#}"), &err);
                if !record_or_hold(state, unrecorded, row).await {
                    held += 1;
                }
                break;
            }
        }
    }

    if classified > 0 || failed > 0 || refused > 0 {
        info!(
            classified,
            failed, held, refused, "Jev failure classification completed"
        );
    }
    merged(
        json!({
            "status": if failed > 0 && classified == 0 { "error" } else { "ok" },
            "classified": classified,
            "failed": failed,
            "unrecorded": held,
            "refused": refused,
            "admission": "observational_only",
        }),
        backlog(waiting, classified + refused, false),
    )
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
    use sqlx::Row;
    use std::collections::BTreeMap;

    /// What a step left waiting is what it saw minus what it did; failures
    /// stay in the backlog, and the merge keeps the step's own fields.
    #[test]
    fn a_step_reports_its_backlog_in_one_shape() {
        let summary = merged(
            json!({"status": "ok", "graded": 9, "failed": 1}),
            backlog(14, 9, false),
        );
        assert_eq!(summary["remaining"], 5);
        assert_eq!(summary["waiting"], 14);
        assert_eq!(summary["processed"], 9);
        assert_eq!(summary["graded"], 9);
        assert_eq!(summary["waiting_is_lower_bound"], false);
        assert_eq!(backlog(3, 5, true)["remaining"], 0, "never negative");
    }

    /// A worker state whose Jev calls go to `base_url`, with two stored
    /// report failures the deterministic cascade cannot name.
    async fn worker_state(base_url: &str) -> AppState {
        worker_state_with(base_url, "").await
    }

    async fn worker_state_with(base_url: &str, extra_jev_yaml: &str) -> AppState {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for sql in jev_store::create_schema_sql() {
            sqlx::query(sql).execute(&pool).await.expect("jev schema");
        }
        sqlx::query(
            "CREATE TABLE decision_reports (
                id INTEGER, created_at TEXT, status TEXT, error_text TEXT,
                report_json TEXT, request_json TEXT)",
        )
        .execute(&pool)
        .await
        .expect("reports table");
        for (id, created_at) in [(1, "2026-10-16T07:00:00Z"), (2, "2026-10-16T07:30:00Z")] {
            sqlx::query(
                "INSERT INTO decision_reports (id, created_at, status, error_text)
                 VALUES ($1, $2, 'failed', 'an unprecedented upstream refusal')",
            )
            .bind(id)
            .bind(created_at)
            .execute(&pool)
            .await
            .expect("a failed report");
        }
        AppState {
            config_path: std::path::PathBuf::from("test.yaml"),
            config: serde_yaml::from_str(&format!(
                "jev:\n  enabled: true\n  api_key: test-key\n  base_url: {base_url}\n  http_timeout_seconds: 5\n{extra_jev_yaml}"
            ))
            .unwrap(),
            db_url: "sqlite::memory:".into(),
            pool,
        }
    }

    /// The A.3 gap, end to end: a provider that asks for an hour is not asked
    /// again on the next cycle. The failure is recorded with its attempt, the
    /// gate holds, and the second cycle reports the backlog it is leaving.
    #[tokio::test]
    async fn a_long_retry_after_holds_across_worker_cycles() {
        let provider =
            crate::jev::fake_provider::serve(vec![(429, Some("3600"), "rate limited".to_string())])
                .await;
        let state = worker_state(&provider.base_url).await;
        let mut unrecorded = jev_store::Unrecorded::default();

        let first = classify_unknown_failures(&state, &mut unrecorded).await;
        assert_eq!(first["failed"], 1, "{first}");
        assert_eq!(provider.hits(), 1);
        let gate = jev_store::provider_deferred_until(&state.pool, Utc::now())
            .await
            .unwrap()
            .expect("the gate is shut");
        let reopens = chrono::DateTime::parse_from_rfc3339(&gate).unwrap();
        assert!(
            (reopens.with_timezone(&Utc) - Utc::now()).num_seconds() > 3_500,
            "shut for the provider's hour, not one cycle: {gate}"
        );
        let attempts: i64 = sqlx::query("SELECT attempts FROM jev_requests")
            .fetch_one(&state.pool)
            .await
            .unwrap()
            .try_get("attempts")
            .unwrap();
        assert_eq!(attempts, 1, "a failed call counts the request it made");

        let second = classify_unknown_failures(&state, &mut unrecorded).await;
        assert_eq!(second["status"], "deferred", "{second}");
        assert_eq!(second["waiting"], 2);
        assert_eq!(second["remaining"], 2);
        assert_eq!(provider.hits(), 1, "the provider was not asked again");
        let grading = grade_reports(&state, &mut unrecorded).await;
        assert_eq!(
            grading["status"], "deferred",
            "every provider step: {grading}"
        );
        assert_eq!(provider.hits(), 1);
    }

    /// A billed answer whose ledger write fails is held, and the batch stops
    /// rather than spending on calls it cannot account for. Once writes work
    /// the held row lands with its cost.
    #[tokio::test]
    async fn a_billed_answer_whose_write_fails_is_held_and_stops_the_batch() {
        let provider = crate::jev::fake_provider::serve(vec![
            (200, None, crate::jev::fake_provider::billed_answer()),
            (200, None, crate::jev::fake_provider::billed_answer()),
        ])
        .await;
        let state = worker_state(&provider.base_url).await;
        sqlx::query(
            "CREATE TRIGGER refuse_jev_requests BEFORE INSERT ON jev_requests
             BEGIN SELECT RAISE(ABORT, 'disk full'); END",
        )
        .execute(&state.pool)
        .await
        .expect("trigger");
        let mut unrecorded = jev_store::Unrecorded::default();

        let summary = classify_unknown_failures(&state, &mut unrecorded).await;
        assert_eq!(summary["classified"], 0, "{summary}");
        assert_eq!(summary["unrecorded"], 1);
        assert_eq!(
            provider.hits(),
            1,
            "the second subject was not bought blind"
        );
        assert_eq!(unrecorded.pending(), 1);
        assert_eq!(unrecorded.pending_cost_usd(), Some(0.0000042));

        sqlx::query("DROP TRIGGER refuse_jev_requests")
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(unrecorded.flush(&state.pool).await, 1);
        let row = sqlx::query("SELECT status, cost_usd, subject FROM jev_requests")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        let row = row_to_json(&row);
        assert_eq!(
            row["status"], "partial",
            "every question came back an issue"
        );
        assert_eq!(row["subject"], "2", "the newest failure was asked first");
        assert_eq!(row["cost_usd"].as_f64(), Some(0.0000042));
    }

    /// `/api/ai/jev` built typed against a real schema: every section reads,
    /// the agreement and stored results decode, and a disagreement's marker
    /// flag is a boolean rather than the stored 0 or 1.
    #[tokio::test]
    async fn the_observations_payload_builds_typed_against_the_schema() {
        let state = worker_state("http://127.0.0.1:9").await;
        sqlx::query(
            "CREATE TABLE editorial_research_items (id TEXT PRIMARY KEY, title TEXT, source_name TEXT)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO jev_editorial_signals
                (item_id, symbol, created_at, instruction_shaped, marker_screened, evidence_title)
             VALUES ('item-1', 'NOVO:xcse', $1, 0.93, 0, 'Reads like an instruction')",
        )
        .bind(jev_signals::now_rfc3339())
        .execute(&state.pool)
        .await
        .unwrap();
        let grade = crate::jev::JevResponse {
            answers: BTreeMap::new(),
            issues: Vec::new(),
            usage: crate::jev::JevUsage {
                input_tokens: 6_930,
                output_tokens: 0,
            },
            cost_usd: Some(0.00029),
            cost_source: "billed",
            measurement: json!({"schema_version": "test"}),
            model_resolved: Some("typesafe/jev-1.14-20261014".to_string()),
            latency_ms: 900,
            attempts: 1,
            attempt_log: Vec::new(),
            request_bytes: 30_000,
            estimated_input_tokens: 20_000,
        };
        jev_store::record_success(
            &state.pool,
            jev_store::RecordedRequest {
                id: "grade-401",
                created_at: "2026-10-16T07:30:00Z",
                purpose: jev_store::PURPOSE_REPORT_GRADING,
                subject: Some("401"),
                model_requested: "~typesafe/jev-latest",
                question_count: 22,
            },
            &grade,
            Some(&json!({"version": REPORT_GRADING_VERSION, "numeric_checks": []})),
        )
        .await
        .unwrap();

        let payload = state.jev_observations(100, 30).await.expect("builds");
        assert_eq!(payload.status, "ok");
        assert_eq!(payload.signal_count, 1);
        assert_eq!(payload.screening_agreement.jev_only, 1);
        assert!(!payload.screening_disagreements[0].marker_screened);
        assert_eq!(payload.report_grades.len(), 1);
        assert_eq!(payload.report_grades[0].subject.as_deref(), Some("401"));
        assert_eq!(
            payload.report_grades[0].result_json["version"], REPORT_GRADING_VERSION,
            "the measurement document is kept whole"
        );
        assert_eq!(
            payload.models.current_model.as_deref(),
            Some("typesafe/jev-1.14-20261014")
        );
        assert_eq!(payload.operator.purposes[0].max_input_tokens, Some(6_930));
        assert!(payload.operator.unavailable_sources.is_empty());

        let served = serde_json::to_value(&payload).unwrap();
        for key in [
            "retention",
            "screening_agreement",
            "screening_disagreements",
            "symbol_rankings",
            "models",
            "operator",
            "report_grades",
            "failure_classifications",
        ] {
            assert!(served.get(key).is_some(), "{key} is served");
        }
        assert_eq!(
            served["screening_disagreements"][0]["marker_screened"],
            false
        );
    }

    /// The A.3 follow-on: a subject too large to send is refused once and
    /// skipped from then on. Selection is newest first, so before this it
    /// stopped its step's batch on every cycle and nothing behind it was
    /// ever asked.
    #[tokio::test]
    async fn a_subject_too_large_to_send_is_refused_once_and_does_not_block_the_queue() {
        let provider = crate::jev::fake_provider::serve(vec![
            (200, None, crate::jev::fake_provider::billed_answer()),
            (200, None, crate::jev::fake_provider::billed_answer()),
        ])
        .await;
        let state = worker_state_with(&provider.base_url, "  max_state_chars: 1000\n").await;
        sqlx::query(
            "INSERT INTO decision_reports (id, created_at, status, error_text)
             VALUES (3, '2026-10-16T08:00:00Z', 'failed', $1)",
        )
        .bind("an unprecedented upstream refusal ".repeat(100))
        .execute(&state.pool)
        .await
        .expect("a long failure, newest");
        let mut unrecorded = jev_store::Unrecorded::default();

        let first = classify_unknown_failures(&state, &mut unrecorded).await;
        assert_eq!(first["refused"], 1, "{first}");
        assert_eq!(first["classified"], 2, "the queue behind it moved");
        assert_eq!(first["failed"], 0);
        assert_eq!(provider.hits(), 2);
        assert!(
            jev_store::provider_deferred_until(&state.pool, Utc::now())
                .await
                .unwrap()
                .is_none(),
            "a refusal says nothing about the provider"
        );

        let second = classify_unknown_failures(&state, &mut unrecorded).await;
        assert_eq!(
            second["waiting"], 0,
            "the refused subject is not selected again: {second}"
        );
        assert_eq!(provider.hits(), 2);
    }

    /// Every request shape the worker sends fits both documented limits at
    /// its largest: a full grading question set with a state at the operator
    /// cap. So the budget cannot refuse a real grading call, only something
    /// larger than the cap allows. The calibration requests stay overcounted.
    #[test]
    fn every_request_shape_fits_the_documented_context_at_its_largest() {
        let limits = crate::jev::RequestLimits::from_yaml(&serde_yaml::Value::Null);
        fn budget_for(
            state: &JsonValue,
            questions: &BTreeMap<String, crate::jev::Question>,
        ) -> crate::jev::RequestBudget {
            let request = crate::jev::build_request(state, "~typesafe/jev-latest", questions);
            crate::jev::request_budget(&request, state.to_string().len())
        }

        let full_state = json!({"report": "x".repeat(limits.max_state_chars - 20)});
        let grading = budget_for(
            &full_state,
            &jev_signals::report_grading_questions(MAX_JUDGED_CANDIDATES),
        );
        assert!(
            grading.request_tokens <= limits.max_request_tokens,
            "a full grading request: {grading:?}"
        );
        assert!(grading.largest_question_tokens <= limits.max_question_tokens);

        let news = jev_signals::news_state(
            "TSLA:xnas",
            Some("Tesla"),
            "App Economy Insights",
            "Tesla: Cash Burn Begins",
            "Robotaxi bets meet margin pressure.",
            Some("2026-07-24T12:00:00Z"),
        );
        let editorial = budget_for(&news, &jev_signals::news_questions());
        assert!(
            editorial.request_tokens >= 861,
            "the largest editorial call was billed 861 tokens: {editorial:?}"
        );
        let classification = budget_for(
            &jev_signals::error_classification_state("failed", ""),
            &jev_signals::error_classification_questions(KNOWN_FAILURE_CODES),
        );
        assert!(
            classification.request_tokens >= 527,
            "the smallest classification was billed 527 tokens: {classification:?}"
        );
    }

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

    /// A full signal row as the prompt embedded it under
    /// `markov_method.latest_run` until 2026-08-03: raw column names, and a
    /// status of its own.
    fn embedded_row(symbol: &str, signed_signal: f64, status: &str) -> JsonValue {
        json!({
            "symbol": symbol,
            "status": status,
            "run_date": "2026-07-29",
            "current_state": "Bull",
            "current_close": 344.71,
            "signal_horizon_days": 5,
            "bull_prob": 0.6159,
            "bear_prob": 0.0389,
            "sideways_prob": 0.3452,
            "signed_signal": signed_signal,
            "direction": if signed_signal >= 0.0 { "long" } else { "short" },
            "conviction": signed_signal.abs(),
            "raw_payload_json": {"recent_labels": []},
        })
    }

    fn prompt_with_embedded_rows(listed: Vec<JsonValue>, embedded: Vec<JsonValue>) -> JsonValue {
        json!({
            "daily_indicators": {"signals": [indicator_signal("JPM:xnys")]},
            "markov_method": {
                "signals": listed,
                "latest_run": {"status": "completed", "summary_json": {"signals": embedded}},
            },
        })
    }

    /// Report #199 quoted JPM at +0.577 from the rows the prompt embedded,
    /// because the alphabetical list stopped before J. Reading the list alone
    /// called a faithful quote `not_in_evidence`.
    #[test]
    fn a_symbol_the_list_left_out_is_read_from_the_embedded_rows() {
        let report = json!({
            "selected_assets": [{"symbol": "JPM:xnys", "notes": "BUY sentiment and +0.577 Markov signal."}],
            "suggested_trades": [],
        });
        let prompt = prompt_with_embedded_rows(
            vec![markov_signal("AAPL:xnas", 0.2, "Bull")],
            vec![embedded_row("JPM:xnys", 0.5769323255186927, "ok")],
        );
        let inputs = grading_inputs(&report, &prompt);
        let markov = &inputs.evidence[0]["markov"];
        assert_eq!(markov["signed_signal"], 0.5769323255186927);
        assert_eq!(
            markov["state"], "Bull",
            "projected onto the list's field names"
        );
        assert_eq!(markov["horizon_days"], 5);
        assert_eq!(markov["currency"], "USD");
        assert_eq!(inputs.numeric[0]["markov_source"], "embedded_run_rows");
        let verdicts: Vec<&str> = inputs.numeric[0]["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .filter(|check| check["field"] == "markov.signed_signal")
            .filter_map(|check| check["verdict"].as_str())
            .collect();
        assert_eq!(verdicts, vec!["matches"]);
    }

    /// Where both carry a symbol, the list is the evidence. The rows are only
    /// ever the fallback.
    #[test]
    fn the_list_wins_where_both_carry_a_symbol() {
        let report = json!({
            "selected_assets": [{"symbol": "JPM:xnys", "notes": "+0.40 Markov signal"}],
            "suggested_trades": [],
        });
        let prompt = prompt_with_embedded_rows(
            vec![markov_signal("JPM:xnys", 0.40, "Bull")],
            vec![embedded_row("JPM:xnys", 0.90, "ok")],
        );
        let inputs = grading_inputs(&report, &prompt);
        assert_eq!(inputs.evidence[0]["markov"]["signed_signal"], 0.40);
        assert_eq!(inputs.numeric[0]["markov_source"], "evidence_list");
    }

    /// A row that did not compute is not evidence, and a prompt without the
    /// embedded rows is read exactly as before.
    #[test]
    fn a_failed_row_or_a_trimmed_prompt_supplies_nothing() {
        let report = json!({
            "selected_assets": [{"symbol": "JPM:xnys", "notes": "+0.577 Markov signal"}],
            "suggested_trades": [],
        });
        let failed =
            prompt_with_embedded_rows(vec![], vec![embedded_row("JPM:xnys", 0.577, "error")]);
        let inputs = grading_inputs(&report, &failed);
        assert!(inputs.evidence[0]["markov"].is_null());
        assert!(inputs.numeric[0]["markov_source"].is_null());

        let trimmed = json!({
            "daily_indicators": {"signals": [indicator_signal("JPM:xnys")]},
            "markov_method": {"signals": [], "latest_run": {"summary_json": {"status": "completed"}}},
        });
        assert!(grading_inputs(&report, &trimmed).evidence[0]["markov"].is_null());
    }

    /// Report #260 quoted NESTE at +0.2331, a figure found only in the
    /// earlier report #259 -- which had attached DTE:xetr's signal to NESTE.
    /// A model's earlier output is never evidence, however it is labelled.
    #[test]
    fn an_earlier_report_never_supplies_markov_evidence() {
        let report = json!({
            "selected_assets": [{"symbol": "JPM:xnys", "notes": "+0.2331 Markov signal"}],
            "suggested_trades": [],
        });
        let mut prompt = prompt_with_embedded_rows(vec![], vec![]);
        prompt["earlier_same_scope_report"] = json!({
            "report": {"suggested_trades": [{
                "symbol": "JPM:xnys",
                "strategy_metadata": {"markov": {"signed_signal": 0.23313425481319427}},
            }]},
        });
        let inputs = grading_inputs(&report, &prompt);
        assert!(inputs.evidence[0]["markov"].is_null());
        assert_eq!(
            inputs.numeric[0]["checks"][0]["verdict"], "not_in_evidence",
            "the figure stays unverified rather than borrowing the model's own echo"
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

    /// Every figure's relation reaches both places it has to reach: the
    /// stored measurement and the state the model is shown.
    ///
    /// The grammar computed the relation and the two payload builders dropped
    /// it, so a correct `above` reading was indistinguishable in storage from
    /// an equality -- 1,429 numeric checks across 167 stored v10 grades, none
    /// of them carrying a relation. A parser test would have passed throughout.
    #[test]
    fn the_relation_is_serialized_into_storage_and_into_the_model_state() {
        let report = json!({
            "selected_assets": [{
                "symbol": "EQNR:xosl",
                "notes": "RSI is above 60 with 5 confluences and break risk near 0.061",
            }],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
        });
        let inputs = grading_inputs(&report, &prompt);

        let relation_of = |checks: &JsonValue, quoted: f64| -> String {
            checks
                .as_array()
                .expect("checks")
                .iter()
                .find(|check| check["quoted"].as_f64() == Some(quoted))
                .and_then(|check| check["relation"].as_str())
                .unwrap_or_else(|| panic!("no relation for {quoted} in {checks}"))
                .to_string()
        };

        let payload = grade_payload(&BTreeMap::new(), &[], &inputs);
        let stored = &payload["numeric_checks"]["per_candidate"][0]["checks"];
        assert_eq!(relation_of(stored, 60.0), "above");
        assert_eq!(relation_of(stored, 5.0), "equals");
        assert_eq!(relation_of(stored, 0.061), "unsupported_construction");

        let shown = &inputs.candidates[0]["numeric_checks"];
        assert_eq!(relation_of(shown, 60.0), "above");
        assert_eq!(relation_of(shown, 5.0), "equals");
        assert_eq!(relation_of(shown, 0.061), "unsupported_construction");

        // And it survives the assembly of the state actually sent.
        let state = jev_signals::report_grading_state(
            &inputs.report,
            &inputs.candidates,
            &inputs.evidence,
            inputs.policy.as_ref(),
        );
        assert_eq!(
            relation_of(&state["candidates"][0]["numeric_checks"], 60.0),
            "above",
            "{state}"
        );

        // Every figure is in exactly one of the two totals.
        let total = |key: &str| payload["numeric_checks"][key].as_i64().unwrap_or(-1);
        assert_eq!(
            total("compared") + total("abstained"),
            stored.as_array().map_or(0, Vec::len) as i64
        );
    }

    /// A count of flagged candidates with no denominator cannot distinguish
    /// "all answered, none flagged" from "none answered".
    #[test]
    fn evidence_sufficiency_reports_how_many_candidates_answered() {
        let report = json!({
            "selected_assets": [
                {"symbol": "EQNR:xosl", "notes": "5 confluences"},
                {"symbol": "NOVO-B:xcse", "notes": "5 confluences"},
            ],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [
                indicator_signal("EQNR:xosl"),
                indicator_signal("NOVO-B:xcse"),
            ]},
        });
        let inputs = grading_inputs(&report, &prompt);
        assert_eq!(inputs.candidates.len(), 2);

        let none = grade_payload(&BTreeMap::new(), &[], &inputs);
        assert_eq!(none["evidence_sufficiency"]["asserts_absent_evidence"], 0);
        assert_eq!(none["evidence_sufficiency"]["answered"], 0);
        assert_eq!(none["evidence_sufficiency"]["unanswered"], 2);

        let answered = grade_payload(
            &BTreeMap::from([(
                jev_signals::sufficiency_question_id(0),
                Answer::Noul { noul: 0.1 },
            )]),
            &[],
            &inputs,
        );
        // Still zero flagged, and now that means something different.
        assert_eq!(
            answered["evidence_sufficiency"]["asserts_absent_evidence"],
            0
        );
        assert_eq!(answered["evidence_sufficiency"]["answered"], 1);
        assert_eq!(answered["evidence_sufficiency"]["unanswered"], 1);
        assert_eq!(answered["evidence_sufficiency"]["judged_candidates"], 2);
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
        assert_eq!(
            questions.len(),
            MAX_JUDGED_CANDIDATES * 2 + 2,
            "one wording and one sufficiency question per candidate, plus two report-level"
        );

        let rendered = serde_json::to_string(&jev_signals::report_grading_state(
            &inputs.report,
            &inputs.candidates,
            &inputs.evidence,
            inputs.policy.as_ref(),
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
            max_request_tokens: 64_000,
            max_question_tokens: 32_000,
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
                inputs.policy.as_ref(),
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

    /// Zero checks must not read as a clean revalidation. A prompt with no
    /// indicator snapshot is absent evidence, and recomputing against it
    /// produced an empty result stamped with the current method -- a report
    /// that had never been examined, recorded as examined and clean.
    #[test]
    fn a_prompt_without_evidence_yields_nothing_to_stamp_as_current() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        for absent in [json!({}), JsonValue::Null, json!({"daily_indicators": {}})] {
            let inputs = grading_inputs(&report, &absent);
            assert_eq!(
                inputs.candidates.len(),
                0,
                "no evidence means nothing is checkable: {absent}"
            );
            let payload = numeric_payload(&inputs);
            assert_eq!(payload["matches"], 0);
            assert_eq!(payload["differs"], 0);
            assert!(
                absent
                    .get("daily_indicators")
                    .and_then(|block| block.get("signals"))
                    .and_then(JsonValue::as_array)
                    .is_none(),
                "and the recompute path recognises it as absent rather than empty"
            );
        }

        // A prompt that does carry the block, for a report that selected
        // nothing, is a legitimate zero and must remain recomputable.
        let present = json!({"daily_indicators": {"signals": []}});
        assert!(
            present
                .get("daily_indicators")
                .and_then(|block| block.get("signals"))
                .and_then(JsonValue::as_array)
                .is_some(),
            "an empty snapshot is evidence that nothing matched, not missing evidence"
        );
    }

    /// A preserved measurement is only comparable to its replacement if both
    /// were made against the same evidence. The fingerprint isolates the
    /// method as the variable; without it, a changed figure could equally mean
    /// the evidence moved underneath.
    #[test]
    fn a_measurement_carries_the_identity_of_the_evidence_it_was_made_against() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        let prompt = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.429226, "Bull")]},
        });
        let first = numeric_payload(&grading_inputs(&report, &prompt));
        let again = numeric_payload(&grading_inputs(&report, &prompt));
        assert_eq!(
            first["evidence_sha256"], again["evidence_sha256"],
            "the same evidence fingerprints identically, so a later measurement is comparable"
        );
        assert_eq!(first["evidence_sha256"].as_str().map(str::len), Some(64));

        let moved = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.9, "Bull")]},
        });
        assert_ne!(
            first["evidence_sha256"],
            numeric_payload(&grading_inputs(&report, &moved))["evidence_sha256"],
            "evidence that moved is visible as such rather than read as a method change"
        );
    }

    /// A threshold claim can only be judged against the value that applied
    /// when the note was written. `min_signed_signal` was 0.15 until it was
    /// recalibrated to 0.20 on 2026-08-31, so reading configuration today to
    /// assess an older report gives the wrong answer -- and #163 AAKI was
    /// called a grader false positive on exactly that basis.
    #[test]
    fn the_thresholds_that_applied_are_taken_from_the_report_not_from_config() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "just above threshold"}],
            "suggested_trades": [],
        });
        let base = json!({
            "daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]},
            "markov_method": {"signals": [markov_signal("EQNR:xosl", 0.1592, "Sideways")]},
        });

        // A report written before the prompt carried policy cannot have its
        // threshold claims judged, and that is recorded rather than guessed.
        let older = grading_inputs(&report, &base);
        assert!(older.policy.is_none());
        assert_eq!(older.coverage["decision_policy_available"], false);
        let state = jev_signals::report_grading_state(
            &older.report,
            &older.candidates,
            &older.evidence,
            older.policy.as_ref(),
        );
        assert!(state["decision_policy"].is_null());

        // A report that recorded it supplies the value that applied.
        let mut with_policy = base.clone();
        with_policy["decision_policy"] = json!({"markov_gate": {"min_signed_signal": 0.15}});
        let recent = grading_inputs(&report, &with_policy);
        assert_eq!(recent.coverage["decision_policy_available"], true);
        let state = jev_signals::report_grading_state(
            &recent.report,
            &recent.candidates,
            &recent.evidence,
            recent.policy.as_ref(),
        );
        assert_eq!(
            state["decision_policy"]["markov_gate"]["min_signed_signal"], 0.15,
            "the gate in force then, not the 0.20 it was later recalibrated to"
        );
    }

    /// A note asserting what the evidence does not contain is neither fair nor
    /// an overstatement, and forcing it into either loses the finding. #110
    /// NNIT claims a strongly negative Markov signal where its prompt carries
    /// no Markov data, and contains no number for the checker to reach.
    #[test]
    fn an_assertion_with_no_evidence_behind_it_has_its_own_outcome() {
        let questions = jev_signals::report_grading_questions(1);
        let crate::jev::Question::Choice { criteria, .. } =
            &questions[&jev_signals::claim_question_id(0)]
        else {
            panic!("a candidate verdict is a choice");
        };
        assert!(
            !criteria.contains_key(jev_signals::WORDING_UNSUPPORTED),
            "sufficiency is its own question now, not a rival wording verdict"
        );
        assert_eq!(criteria.len(), 4);
        assert!(questions.contains_key(&jev_signals::sufficiency_question_id(0)));

        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "strongly negative Markov signal"}],
            "suggested_trades": [],
        });
        // Indicators present, Markov absent -- exactly #110's shape.
        let prompt = json!({"daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]}});
        let inputs = grading_inputs(&report, &prompt);
        assert!(
            inputs.evidence[0]["markov"].is_null(),
            "the candidate is judged with no Markov evidence, which is the point"
        );

        // Both findings at once: the wording overstates what the evidence does
        // show, and a separate assertion concerns something it does not
        // contain. A single verdict could not carry both.
        let payload = grade_payload(
            &BTreeMap::from([
                (
                    jev_signals::claim_question_id(0),
                    choice(jev_signals::WORDING_OVERSTATED, 0.8, &[("overstated", 0.8)]),
                ),
                (
                    jev_signals::sufficiency_question_id(0),
                    Answer::Noul { noul: 0.93 },
                ),
            ]),
            &[],
            &inputs,
        );
        assert_eq!(payload["wording_verdict_counts"]["overstated"], 1);
        assert_eq!(
            payload["evidence_sufficiency"]["asserts_absent_evidence"],
            1
        );
        assert_eq!(
            payload["wording_verdicts"][0]["asserts_absent_evidence"], 0.93,
            "the per-candidate probability is kept beside its wording verdict"
        );
    }
}

#[cfg(test)]
mod recompute_lookup_tests {
    use super::*;

    /// A database that is briefly unavailable is not a report that no longer
    /// exists.
    ///
    /// `let Ok(Some(row)) = lookup else { block permanently }` read the two as
    /// the same thing, so one timeout would have excluded a valid grade from
    /// every future recomputation -- silently, and in the direction that looks
    /// like progress.
    #[test]
    fn a_failed_read_is_not_a_missing_report() {
        assert!(matches!(ReportRow::of(Ok(Some(7))), ReportRow::Found(7)));
        assert!(matches!(ReportRow::<i32>::of(Ok(None)), ReportRow::Gone));
        assert!(matches!(
            ReportRow::<i32>::of(Err(sqlx::Error::PoolTimedOut)),
            ReportRow::Unreadable(_)
        ));
    }
}
