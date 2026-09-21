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
const REPORT_GRADING_VERSION: &str = "v4";
const FAILURE_CLASSIFICATION_VERSION: &str = "v2";

/// One sequential worker in the singleton scheduler process. No Jev network
/// work is awaited by report creation, ingestion, or the execution cycle.
/// Disabled deployments do not create a polling worker.
pub(crate) async fn run_observation_loop(state: AppState) {
    if availability(&state).is_none() {
        return;
    }
    loop {
        let grades = grade_reports(&state).await;
        let failures = classify_unknown_failures(&state).await;
        let editorial = crate::editorial_research::score_items_with_jev(&state).await;
        info!(
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

    let questions = jev_signals::report_grading_questions();
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

        let (compact, evidence, matched_candidates) = grading_inputs(&report, &prompt);
        let jev_state = jev_signals::report_grading_state(&compact, &evidence);
        // A report that proposed nothing has no candidate claims to check. Ask
        // anyway and the answer is vacuously true -- the first live run graded
        // two empty reports at 0.92 and 0.94, which reads as high quality and
        // means nothing. The question is withheld instead, and recorded as
        // not applicable.
        let mut questions = questions.clone();
        if matched_candidates == 0 {
            questions.remove("rationale_supported");
        }
        let now = Utc::now().to_rfc3339();
        let request_id = stable_id("jevgrade", &format!("{subject}|{now}"));

        match crate::jev::ask(&cfg, &jev_state, &questions).await {
            Ok(response) => {
                let result = grade_payload(&response.answers, &response.issues, matched_candidates);
                if let Err(err) = jev_store::record_success(
                    &state.pool,
                    jev_store::RecordedRequest {
                        id: &request_id,
                        created_at: &now,
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
                        created_at: &now,
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
fn grading_inputs(report: &JsonValue, prompt: &JsonValue) -> (JsonValue, JsonValue, usize) {
    // The indicator snapshot carried in the stored request, keyed by symbol.
    // This is the evidence the report was written against, which is why it
    // comes from the request rather than from daily_indicator_signals today.
    let indicators: std::collections::HashMap<&str, &JsonValue> = prompt
        .get("daily_indicators")
        .and_then(|block| block.get("signals"))
        .and_then(JsonValue::as_array)
        .map(|signals| {
            signals
                .iter()
                .filter_map(|signal| Some((signal.get("symbol")?.as_str()?, signal)))
                .collect()
        })
        .unwrap_or_default();

    let markov: std::collections::HashMap<&str, &JsonValue> = report
        .get("suggested_trades")
        .and_then(JsonValue::as_array)
        .map(|trades| {
            trades
                .iter()
                .filter_map(|trade| {
                    Some((
                        trade.get("symbol")?.as_str()?,
                        trade.get("strategy_metadata")?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    // A candidate is checkable when its indicator snapshot is present. That is
    // the evidence the notes actually cite -- confluence counts, support break
    // risk, reward/risk, support levels -- none of which the report echoes
    // back in strategy_metadata.
    let checkable: Vec<JsonValue> = report
        .get("selected_assets")
        .and_then(JsonValue::as_array)
        .map(|assets| {
            assets
                .iter()
                .filter(|asset| {
                    asset
                        .get("symbol")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|symbol| indicators.contains_key(symbol))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let matched = checkable.len();

    let candidate_evidence: Vec<JsonValue> = checkable
        .iter()
        .filter_map(|asset| {
            let symbol = asset.get("symbol")?.as_str()?;
            Some(json!({
                "symbol": symbol,
                "daily_indicators": indicators.get(symbol),
                "markov": markov
                    .get(symbol)
                    .and_then(|metadata| metadata.get("markov")),
            }))
        })
        .collect();

    let compact = json!({
        "report_title": report.get("report_title"),
        "market_view": report.get("market_view"),
        "reasoning_steps": report.get("reasoning_steps"),
        "symbol_sentiment": report.get("symbol_sentiment"),
        // Only the claims that have evidence to be checked against.
        "selected_assets": checkable,
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
    });
    let evidence = json!({ "candidate_evidence": candidate_evidence });
    (compact, evidence, matched)
}

fn grade_payload(
    answers: &std::collections::BTreeMap<String, crate::jev::Answer>,
    issues: &[crate::jev::JevAnswerIssue],
    matched_candidates: usize,
) -> JsonValue {
    let noul = |id: &str| match answers.get(id) {
        Some(crate::jev::Answer::Noul { noul }) => Some(*noul),
        _ => None,
    };
    json!({
        // v2 rescopes rationale_supported to the candidate claims, which are
        // the ones whose supporting evidence fits in the window. v1 grades
        // remain in the table under their own version and must not be pooled
        // with these: they answered a different question over a fifth of the
        // material that question referenced.
        "version": REPORT_GRADING_VERSION,
        "evidence_scope": "report_self_consistency_not_independent_source_verification",
        "rationale_supported": noul("rationale_supported"),
        "rationale_supported_scope": "candidate_claims_with_signals_only",
        // The denominator. Without it a null reads as a failure to answer
        // rather than as nothing having been asked.
        "checkable_candidate_count": matched_candidates,
        "rationale_supported_status": if matched_candidates == 0 {
            "not_applicable_no_candidate_claims_to_check"
        } else if noul("rationale_supported").is_some() {
            "answered"
        } else {
            "unanswered"
        },
        "view_consistent": noul("view_consistent"),
        "specificity": answers
            .get("specificity")
            .and_then(crate::jev::Answer::normalized_score),
        "unanswered": issues.iter().map(|issue| issue.id.clone()).collect::<Vec<_>>(),
        "admission": "observational_only",
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
    use crate::jev::{Answer, JevAnswerIssue};
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

    /// The claim that motivated this: report 304's note cited "0.061 support
    /// break risk", which is exactly `support.break_risk` in the snapshot the
    /// model was given. Grading it against `strategy_metadata` -- which
    /// carries no support figures at all -- scored it 0.09, a measurement of
    /// the missing evidence rather than of the claim.
    #[test]
    fn the_evidence_carries_the_figures_the_notes_actually_cite() {
        let report = json!({
            "selected_assets": [{
                "symbol": "EQNR:xosl",
                "notes": "5 technical confluences, 0.061 support break risk, +0.429 Bull Markov",
            }],
            "suggested_trades": [{
                "symbol": "EQNR:xosl",
                "strategy_metadata": {"markov": {"signed_signal": 0.429226, "state": "Bull"}},
            }],
        });
        let prompt = json!({"daily_indicators": {"signals": [indicator_signal("EQNR:xosl")]}});
        let (_, evidence, matched) = grading_inputs(&report, &prompt);

        assert_eq!(matched, 1);
        let candidate = &evidence["candidate_evidence"][0];
        assert_eq!(candidate["symbol"], "EQNR:xosl");
        assert_eq!(candidate["daily_indicators"]["confluence_count"], 5);
        assert_eq!(candidate["markov"]["signed_signal"], 0.429226);
        let break_risk = candidate["daily_indicators"]["support"]["break_risk"]
            .as_f64()
            .expect("break risk is present");
        assert!(
            (break_risk - 0.061).abs() < 0.001,
            "the cited 0.061 is checkable: {break_risk}"
        );
    }

    /// Checkability now follows the indicator snapshot rather than
    /// suggested_trades. Both graded reports that had candidates listed four
    /// and proposed one, so three quarters of the claims were unjudgeable
    /// under v3 despite the evidence existing in the stored prompt.
    #[test]
    fn a_watchlist_candidate_that_was_not_proposed_is_still_checkable() {
        let symbols = ["EQNR:xosl", "NOVO:xcse", "FORTUM:xhel", "ALV:xetr"];
        let report = json!({
            "selected_assets": symbols
                .iter()
                .map(|symbol| json!({"symbol": symbol, "notes": "5 confluences"}))
                .collect::<Vec<_>>(),
            // Only one of the four was actually proposed.
            "suggested_trades": [{
                "symbol": "EQNR:xosl",
                "strategy_metadata": {"markov": {"signed_signal": 0.429}},
            }],
        });
        let prompt = json!({
            "daily_indicators": {
                "signals": symbols.iter().map(|s| indicator_signal(s)).collect::<Vec<_>>(),
            }
        });
        let (_, evidence, matched) = grading_inputs(&report, &prompt);

        assert_eq!(matched, 4, "all four have indicator evidence");
        assert_eq!(
            evidence["candidate_evidence"].as_array().map(Vec::len),
            Some(4)
        );
        assert!(
            evidence["candidate_evidence"][1]["markov"].is_null(),
            "a watchlist name that was not proposed has no markov block, and that is \
             recorded as absent rather than fabricated"
        );
    }

    /// A candidate with no snapshot in the stored prompt cannot be judged.
    /// Older reports predate the persisted snapshot entirely.
    #[test]
    fn a_candidate_without_a_stored_snapshot_is_not_put_up_for_judgement() {
        let report = json!({
            "selected_assets": [{"symbol": "EQNR:xosl", "notes": "5 confluences"}],
            "suggested_trades": [],
        });
        let (_, _, matched) = grading_inputs(&report, &json!({}));
        assert_eq!(matched, 0);
        let (_, _, matched) = grading_inputs(&report, &JsonValue::Null);
        assert_eq!(matched, 0);
    }

    /// One tiny fixture proves nothing about the byte budget. A full watchlist
    /// with complete indicator snapshots is the realistic worst case, and it
    /// has to fit `jev.max_state_chars`.
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
                .map(|s| json!({
                    "symbol": s,
                    "action": "BUY",
                    "strategy_role": "w".repeat(120),
                    "strategy_metadata": {"markov": {"signed_signal": 0.42, "state": "Bull"}},
                }))
                .collect::<Vec<_>>(),
        });
        let prompt = json!({
            "daily_indicators": {
                "signals": symbols.iter().map(|s| indicator_signal(s)).collect::<Vec<_>>(),
            }
        });
        let (compact, evidence, matched) = grading_inputs(&report, &prompt);
        assert_eq!(matched, 12);

        let rendered =
            serde_json::to_string(&jev_signals::report_grading_state(&compact, &evidence))
                .expect("serializes");
        // The same ceiling `jev.max_state_chars` applies, which `ask` enforces
        // by refusing rather than truncating.
        assert!(
            rendered.len() < 24_000,
            "a full watchlist must fit, or every grade on a busy day is refused: {}",
            rendered.len()
        );
    }

    /// An unanswered grade must read as unanswered. `false` is a finding --
    /// "the rationale is not supported" -- and inventing one from a transport
    /// failure would put an accusation on a report nobody assessed.
    #[test]
    fn an_unanswered_grade_is_null_and_named_rather_than_false() {
        let payload = grade_payload(
            &BTreeMap::from([("view_consistent".to_string(), Answer::Noul { noul: 0.91 })]),
            &[JevAnswerIssue {
                id: "rationale_supported".to_string(),
                reason: "no answer returned for this question".to_string(),
            }],
            2,
        );
        assert_eq!(payload["view_consistent"], 0.91);
        assert!(payload["rationale_supported"].is_null());
        assert_eq!(payload["unanswered"][0], "rationale_supported");
        assert_eq!(payload["admission"], "observational_only");
    }

    #[test]
    fn classification_offers_only_categories_the_runtime_already_handles() {
        let questions = jev_signals::error_classification_questions(KNOWN_FAILURE_CODES);
        let crate::jev::Question::Choice { criteria, .. } = &questions["category"] else {
            panic!("category is a choice");
        };
        for (code, _) in KNOWN_FAILURE_CODES {
            assert!(criteria.contains_key(*code), "{code}");
        }
        assert!(criteria.contains_key("other"));
        assert_eq!(criteria.len(), KNOWN_FAILURE_CODES.len() + 1);
    }

    /// A report that proposed nothing has no candidate claims. Asking anyway
    /// returns a vacuous truth: the first live run graded two empty reports at
    /// 0.92 and 0.94, which reads as high quality and means nothing.
    #[test]
    fn a_report_with_nothing_to_check_records_not_applicable_not_a_high_score() {
        let empty = json!({"selected_assets": [], "suggested_trades": []});
        let (_, _, matched) = grading_inputs(&empty, &json!({"daily_indicators": {"signals": []}}));
        assert_eq!(matched, 0);

        let payload = grade_payload(
            &BTreeMap::from([("view_consistent".to_string(), Answer::Noul { noul: 0.9 })]),
            &[],
            matched,
        );
        assert!(
            payload["rationale_supported"].is_null(),
            "nothing was asked, so nothing is recorded"
        );
        assert_eq!(
            payload["rationale_supported_status"],
            "not_applicable_no_candidate_claims_to_check"
        );
        assert_eq!(payload["checkable_candidate_count"], 0);

        let answered = grade_payload(
            &BTreeMap::from([(
                "rationale_supported".to_string(),
                Answer::Noul { noul: 0.88 },
            )]),
            &[],
            3,
        );
        assert_eq!(answered["rationale_supported_status"], "answered");
        assert_eq!(answered["checkable_candidate_count"], 3);
    }
}
