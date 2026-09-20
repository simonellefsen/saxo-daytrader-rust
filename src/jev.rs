//! Transport and typed primitives for Jev, TypeSafe's System One model.
//!
//! Jev does not write prose. It takes one `state` plus a map of typed
//! questions and answers all of them in parallel against that same state,
//! returning a constrained value per question with a probability attached.
//! That makes it an instrument rather than an author: it can measure and
//! grade the unstructured text this system already ingests, but it cannot
//! produce a Decision Report and must never be wrapped to look as if it can.
//!
//! The module is deliberately transport plus parsing only. It knows nothing
//! about editorial feeds, report scheduling, Trading Manager gates, or Saxo,
//! and it cannot place, size, amend, or cancel an order.
//!
//! Two properties drive the parsing rules below.
//!
//! A Jev answer is a *claim*, and every claim this module cannot verify is
//! recorded as absent rather than as a value. A malformed `noul` is the sharp
//! case: `0.0` is not "no answer", it is a confident "strongly no". Defaulting
//! a broken field to zero would turn a protocol failure into a decisive
//! negative, which is the same defect class that made a broken query read as
//! an empty result elsewhere in this codebase.
//!
//! A Jev answer is also *constrained* -- the documented guarantee is that a
//! choice never falls outside the supplied `criteria`. Downstream code will
//! branch on that value, so this module verifies the guarantee instead of
//! trusting it, and files a violation as an issue rather than passing an
//! unknown label through.

use std::{collections::BTreeMap, sync::LazyLock, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;

use crate::config::{yaml_bool, yaml_i64, yaml_string};

/// Published Jev input price, in USD per million input tokens.
///
/// Output tokens are free. This is a rate card, not an invoice: a cost derived
/// from it is labelled `rate_card` so a reader can tell a computed figure from
/// one the provider actually reported.
pub(crate) const RATE_CARD_INPUT_USD_PER_MILLION: f64 = 0.042;
pub(crate) const RATE_CARD_OUTPUT_USD_PER_MILLION: f64 = 0.0;

pub(crate) const COST_SOURCE_RATE_CARD: &str = "rate_card";
pub(crate) const COST_SOURCE_NONE: &str = "not_reported";

const MAX_ATTEMPTS: u32 = 3;
const BASE_BACKOFF_MS: u64 = 250;
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_MODEL: &str = "~typesafe/jev-latest";
const DEFAULT_TIMEOUT_SECONDS: i64 = 15;
const DEFAULT_MAX_STATE_CHARS: i64 = 24_000;
const DEFAULT_MAX_QUESTIONS: i64 = 12;

/// Process-wide client with no global timeout; the deadline is applied per
/// request instead.
///
/// Jev is called once per ingested item, so connection reuse matters here in a
/// way it does not for the handful of Decision Report calls a day. Holding the
/// deadline on the request builder rather than the client keeps that reuse
/// while still letting `jev.http_timeout_seconds` govern each call.
static JEV_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .build()
        .expect("constructing the static Jev HTTP client")
});

/// Why Jev is or is not usable on this deployment.
///
/// A deliberately disabled integration and a missing key are different
/// operational facts and must not collapse into one "off". The first is a
/// choice; the second is a misconfiguration worth surfacing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JevAvailability {
    Ready(Box<JevConfig>),
    Disabled,
    MissingApiKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JevConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub http_timeout_seconds: u64,
    pub max_state_chars: usize,
    pub max_questions_per_request: usize,
}

impl JevConfig {
    /// Reads the `jev:` block. `api_key` resolves through the shared `ENV:`
    /// prefix handled by `config::yaml_string`, so the key never appears in
    /// the YAML itself.
    pub(crate) fn from_yaml(config: &YamlValue) -> JevAvailability {
        if !yaml_bool(config, &["jev", "enabled"]).unwrap_or(false) {
            return JevAvailability::Disabled;
        }
        let Some(api_key) = yaml_string(config, &["jev", "api_key"])
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
        else {
            return JevAvailability::MissingApiKey;
        };
        JevAvailability::Ready(Box::new(Self {
            api_key,
            base_url: yaml_string(config, &["jev", "base_url"])
                .map(|url| url.trim().trim_end_matches('/').to_string())
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: yaml_string(config, &["jev", "model"])
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            http_timeout_seconds: yaml_i64(config, &["jev", "http_timeout_seconds"])
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
                .clamp(1, 120) as u64,
            max_state_chars: yaml_i64(config, &["jev", "max_state_chars"])
                .unwrap_or(DEFAULT_MAX_STATE_CHARS)
                .clamp(256, 120_000) as usize,
            max_questions_per_request: yaml_i64(config, &["jev", "max_questions_per_request"])
                .unwrap_or(DEFAULT_MAX_QUESTIONS)
                .clamp(1, 64) as usize,
        }))
    }

    fn systemone_url(&self) -> String {
        format!("{}/systemone", self.base_url)
    }
}

/// One typed question. `instructions` is a `JsonValue` rather than a `String`
/// because TypeSafe's guidance is to lift background facts into named fields
/// beside the question instead of interpolating them into prose.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Question {
    Noul {
        instructions: JsonValue,
        criteria: Option<JsonValue>,
    },
    Choice {
        instructions: JsonValue,
        criteria: BTreeMap<String, String>,
    },
    Score {
        instructions: JsonValue,
        criteria: Vec<String>,
    },
}

impl Question {
    /// The wire discriminator, used by tests to assert a question set is
    /// shaped as intended. The runtime branches on the enum itself.
    #[cfg(test)]
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Noul { .. } => "noul",
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
        }
    }

    fn to_json(&self) -> JsonValue {
        match self {
            Self::Noul {
                instructions,
                criteria,
            } => {
                let mut value = json!({"type": "noul", "instructions": instructions});
                if let Some(criteria) = criteria {
                    value["criteria"] = criteria.clone();
                }
                value
            }
            Self::Choice {
                instructions,
                criteria,
            } => json!({
                "type": "choice",
                "instructions": instructions,
                "criteria": criteria.iter().map(|(key, description)| {
                    (key.clone(), JsonValue::String(description.clone()))
                }).collect::<serde_json::Map<String, JsonValue>>(),
            }),
            Self::Score {
                instructions,
                criteria,
            } => json!({
                "type": "score",
                "instructions": instructions,
                "criteria": criteria,
            }),
        }
    }
}

/// A verified answer. Every variant carries only values this module could
/// check against the question that produced it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        confidence: Option<f64>,
        legend: BTreeMap<String, String>,
    },
}

impl Answer {
    /// Maps a score onto 0.0..=1.0 using the legend the provider returned.
    ///
    /// The documented range is "position along levels", which does not say
    /// whether levels are numbered from zero or from one. Rather than guess,
    /// this derives the bounds from the legend actually received and returns
    /// `None` when there is no legend to derive them from -- an unscaled score
    /// is still available as the raw field.
    pub(crate) fn normalized_score(&self) -> Option<f64> {
        let Self::Score { score, legend, .. } = self else {
            return None;
        };
        let mut levels: Vec<f64> = legend.keys().filter_map(|key| key.parse().ok()).collect();
        if levels.len() < 2 {
            return None;
        }
        levels.sort_by(|left, right| left.partial_cmp(right).expect("levels are finite"));
        let (low, high) = (levels[0], levels[levels.len() - 1]);
        if (high - low).abs() < f64::EPSILON {
            return None;
        }
        Some(((score - low) / (high - low)).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct JevUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JevResponse {
    pub answers: BTreeMap<String, Answer>,
    /// Questions that were asked but came back missing or unusable, with the
    /// reason. Retained so a partial response is legible rather than silently
    /// short.
    pub issues: Vec<JevAnswerIssue>,
    pub usage: JevUsage,
    /// The model that actually answered, read from the response body. With a
    /// floating alias configured this is the only record of which version
    /// produced a given probability, so a later recalibration is a dated,
    /// visible boundary rather than an invisible drift.
    pub model_resolved: Option<String>,
    pub latency_ms: i64,
    pub attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JevAnswerIssue {
    pub id: String,
    pub reason: String,
}

pub(crate) fn build_request(
    state: &JsonValue,
    model: &str,
    questions: &BTreeMap<String, Question>,
) -> JsonValue {
    json!({
        "state": state,
        "model": model,
        "questions": questions
            .iter()
            .map(|(id, question)| (id.clone(), question.to_json()))
            .collect::<serde_json::Map<String, JsonValue>>(),
    })
}

/// Parses the answer envelope against the questions that were asked.
///
/// Returns `Err` only when the envelope itself is unusable. A single bad
/// answer must not discard the good ones alongside it, so per-answer failures
/// become issues and the remaining answers are returned.
pub(crate) fn parse_answers(
    response: &JsonValue,
    questions: &BTreeMap<String, Question>,
) -> Result<(BTreeMap<String, Answer>, Vec<JevAnswerIssue>)> {
    let envelope = response
        .get("answers")
        .and_then(JsonValue::as_object)
        .context("Jev response has no answers object")?;

    let mut answers = BTreeMap::new();
    let mut issues = Vec::new();
    for (id, question) in questions {
        let Some(raw) = envelope.get(id) else {
            issues.push(JevAnswerIssue {
                id: id.clone(),
                reason: "no answer returned for this question".to_string(),
            });
            continue;
        };
        match parse_one(raw, question) {
            Ok(answer) => {
                answers.insert(id.clone(), answer);
            }
            Err(reason) => issues.push(JevAnswerIssue {
                id: id.clone(),
                reason,
            }),
        }
    }
    Ok((answers, issues))
}

fn parse_one(raw: &JsonValue, question: &Question) -> std::result::Result<Answer, String> {
    match question {
        Question::Noul { .. } => {
            let noul = unit_interval(raw.get("noul")).ok_or_else(|| {
                format!(
                    "noul is missing or outside 0..1: {}",
                    excerpt(raw.get("noul"))
                )
            })?;
            Ok(Answer::Noul { noul })
        }
        Question::Choice { criteria, .. } => {
            let choice = raw
                .get("choice")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "choice is missing or not a string".to_string())?;
            if !criteria.contains_key(choice) {
                return Err(format!(
                    "choice {choice:?} is outside the supplied criteria"
                ));
            }
            Ok(Answer::Choice {
                choice: choice.to_string(),
                probabilities: probability_map(raw.get("probabilities")),
                confidence: unit_interval(raw.get("confidence")),
            })
        }
        Question::Score { .. } => {
            let score = raw
                .get("score")
                .and_then(JsonValue::as_f64)
                .filter(|score| score.is_finite())
                .ok_or_else(|| {
                    format!(
                        "score is missing or not finite: {}",
                        excerpt(raw.get("score"))
                    )
                })?;
            Ok(Answer::Score {
                score,
                confidence: unit_interval(raw.get("confidence")),
                legend: string_map(raw.get("legend")),
            })
        }
    }
}

/// Accepts a probability only when it is finite and inside 0..=1.
///
/// A tiny floating-point overshoot is clamped; anything further out is a
/// protocol violation and returns `None` so the caller records an issue rather
/// than a number.
fn unit_interval(value: Option<&JsonValue>) -> Option<f64> {
    const TOLERANCE: f64 = 1e-6;
    let value = value?.as_f64()?;
    if !value.is_finite() || value < -TOLERANCE || value > 1.0 + TOLERANCE {
        return None;
    }
    Some(value.clamp(0.0, 1.0))
}

fn probability_map(value: Option<&JsonValue>) -> BTreeMap<String, f64> {
    value
        .and_then(JsonValue::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, value)| {
                    unit_interval(Some(value)).map(|value| (key.clone(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(value: Option<&JsonValue>) -> BTreeMap<String, String> {
    value
        .and_then(JsonValue::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn excerpt(value: Option<&JsonValue>) -> String {
    match value {
        None => "absent".to_string(),
        Some(value) => {
            let rendered = value.to_string();
            if rendered.len() <= 60 {
                rendered
            } else {
                format!("{}...", &rendered[..60])
            }
        }
    }
}

pub(crate) fn usage_from_response(response: &JsonValue) -> JevUsage {
    let usage = response.get("usage");
    JevUsage {
        input_tokens: token_count(usage, "input_tokens", "prompt_tokens"),
        output_tokens: token_count(usage, "output_tokens", "completion_tokens"),
    }
}

fn token_count(usage: Option<&JsonValue>, primary: &str, alternate: &str) -> i64 {
    let read = |key: &str| {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(JsonValue::as_i64)
            .filter(|count| *count >= 0)
            .unwrap_or(0)
    };
    read(primary).max(read(alternate))
}

/// Resolves what one request cost, preferring a provider-reported figure.
///
/// A rate-card figure is only produced when there is a positive input token
/// count to apply it to. Multiplying an absent token count by the rate would
/// yield `0.0` -- a claim that the request was free, which is a different and
/// stronger statement than "we do not know".
pub(crate) fn cost_for_request(
    response: &JsonValue,
    usage: &JevUsage,
) -> (Option<f64>, &'static str) {
    if let Some(reported) = response.get("usage") {
        let (cost, source) = crate::llm_usage::cost_from_usage(reported);
        if cost.is_some() {
            return (cost, source);
        }
    }
    if usage.input_tokens <= 0 {
        return (None, COST_SOURCE_NONE);
    }
    let cost = (usage.input_tokens as f64 / 1_000_000.0) * RATE_CARD_INPUT_USD_PER_MILLION
        + (usage.output_tokens.max(0) as f64 / 1_000_000.0) * RATE_CARD_OUTPUT_USD_PER_MILLION;
    (Some(cost), COST_SOURCE_RATE_CARD)
}

/// Whether a status is worth another attempt.
///
/// 429 and 529 are the documented backoff cases and 5xx is transient by
/// definition. 401 and 422 are not: retrying a bad key or a malformed body
/// just spends the rate limit on the same failure.
pub(crate) fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(BASE_BACKOFF_MS * 2_u64.pow(attempt.saturating_sub(1).min(6)))
}

/// Sends one state and its questions to Jev.
///
/// Bounds the state rather than truncating it: a truncated JSON state is both
/// invalid and a silent change to what the model was actually shown, which
/// would make any probability derived from it unreproducible.
pub(crate) async fn ask(
    cfg: &JevConfig,
    state: &JsonValue,
    questions: &BTreeMap<String, Question>,
) -> Result<JevResponse> {
    if questions.is_empty() {
        bail!("Jev request has no questions");
    }
    if questions.len() > cfg.max_questions_per_request {
        bail!(
            "Jev request has {} questions, above the configured maximum of {}",
            questions.len(),
            cfg.max_questions_per_request
        );
    }
    let rendered_state = serde_json::to_string(state).context("serializing Jev state")?;
    if rendered_state.len() > cfg.max_state_chars {
        bail!(
            "Jev state is {} characters, above the configured maximum of {}",
            rendered_state.len(),
            cfg.max_state_chars
        );
    }

    let request = build_request(state, &cfg.model, questions);
    let started = std::time::Instant::now();
    let mut last_error = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        let outcome = JEV_HTTP_CLIENT
            .post(cfg.systemone_url())
            .timeout(Duration::from_secs(cfg.http_timeout_seconds))
            .bearer_auth(&cfg.api_key)
            .json(&request)
            .send()
            .await;

        let (status, body) = match outcome {
            Ok(response) => {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|err| format!("failed to read Jev response body: {err}"));
                (status, body)
            }
            Err(err) => {
                last_error = format!("transport error: {err}");
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    continue;
                }
                break;
            }
        };

        if !status.is_success() {
            last_error = format!(
                "Jev returned {}: {}",
                status.as_u16(),
                truncate(&body, 2_000)
            );
            if is_retryable_status(status.as_u16()) && attempt < MAX_ATTEMPTS {
                tokio::time::sleep(backoff_delay(attempt)).await;
                continue;
            }
            break;
        }

        let parsed: JsonValue =
            serde_json::from_str(&body).context("Jev returned a non-JSON body")?;
        let (answers, issues) = parse_answers(&parsed, questions)?;
        return Ok(JevResponse {
            answers,
            issues,
            usage: usage_from_response(&parsed),
            model_resolved: parsed
                .get("model")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
            latency_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            attempts: attempt,
        });
    }

    bail!("{last_error}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noul_question() -> BTreeMap<String, Question> {
        BTreeMap::from([(
            "about_symbol".to_string(),
            Question::Noul {
                instructions: json!("Is `title` about `symbol`?"),
                criteria: None,
            },
        )])
    }

    fn choice_question() -> BTreeMap<String, Question> {
        BTreeMap::from([(
            "direction".to_string(),
            Question::Choice {
                instructions: json!("Direction implied for `symbol`?"),
                criteria: BTreeMap::from([
                    ("bullish".to_string(), "Price should rise".to_string()),
                    ("bearish".to_string(), "Price should fall".to_string()),
                    ("neutral".to_string(), "No implication".to_string()),
                ]),
            },
        )])
    }

    /// The single most important invariant in this module.
    ///
    /// `noul` is a probability, so `0.0` is not "no answer" -- it is a
    /// confident "strongly no". A malformed, absent, or out-of-range field
    /// must therefore become an issue, never a number, or a transport failure
    /// would read downstream as a decisive negative judgement.
    #[test]
    fn a_malformed_noul_is_recorded_as_absent_not_as_a_confident_no() {
        let questions = noul_question();
        for broken in [
            json!({"answers": {"about_symbol": {"type": "noul"}}}),
            json!({"answers": {"about_symbol": {"type": "noul", "noul": "0.9"}}}),
            json!({"answers": {"about_symbol": {"type": "noul", "noul": 1.4}}}),
            json!({"answers": {"about_symbol": {"type": "noul", "noul": -0.2}}}),
            json!({"answers": {"about_symbol": {"type": "noul", "noul": null}}}),
        ] {
            let (answers, issues) = parse_answers(&broken, &questions).expect("envelope parses");
            assert!(
                answers.is_empty(),
                "a broken noul must not produce an answer: {broken}"
            );
            assert_eq!(issues.len(), 1, "{broken}");
            assert_eq!(issues[0].id, "about_symbol");
        }

        let good = json!({"answers": {"about_symbol": {"type": "noul", "noul": 0.0}}});
        let (answers, issues) = parse_answers(&good, &questions).expect("envelope parses");
        assert!(issues.is_empty());
        assert_eq!(answers["about_symbol"], Answer::Noul { noul: 0.0 });
    }

    /// TypeSafe documents that a choice never falls outside the supplied
    /// options. Downstream code branches on that value, so the guarantee is
    /// verified rather than trusted: an unknown label is an issue, not a
    /// string passed through to a `match`.
    #[test]
    fn a_choice_outside_the_supplied_criteria_is_rejected() {
        let questions = choice_question();
        let response =
            json!({"answers": {"direction": {"type": "choice", "choice": "very_bullish"}}});
        let (answers, issues) = parse_answers(&response, &questions).expect("envelope parses");
        assert!(answers.is_empty());
        assert!(
            issues[0].reason.contains("outside the supplied criteria"),
            "{:?}",
            issues[0]
        );
    }

    #[test]
    fn one_bad_answer_does_not_discard_the_good_ones() {
        let mut questions = choice_question();
        questions.extend(noul_question());
        let response = json!({"answers": {
            "direction": {"type": "choice", "choice": "bullish", "confidence": 0.82},
            "about_symbol": {"type": "noul", "noul": "not a number"},
        }});

        let (answers, issues) = parse_answers(&response, &questions).expect("envelope parses");
        assert_eq!(answers.len(), 1);
        assert_eq!(issues.len(), 1);
        assert!(matches!(
            &answers["direction"],
            Answer::Choice { choice, confidence, .. }
                if choice == "bullish" && *confidence == Some(0.82)
        ));
    }

    #[test]
    fn a_question_that_came_back_missing_is_reported_rather_than_skipped_silently() {
        let questions = noul_question();
        let (answers, issues) =
            parse_answers(&json!({"answers": {}}), &questions).expect("envelope parses");
        assert!(answers.is_empty());
        assert_eq!(issues[0].reason, "no answer returned for this question");
    }

    #[test]
    fn a_response_without_an_answers_object_is_an_error_not_an_empty_result() {
        let error = parse_answers(&json!({"error": "overloaded"}), &noul_question())
            .expect_err("a missing envelope is not an empty answer set");
        assert!(error.to_string().contains("no answers object"));
    }

    #[test]
    fn builds_the_documented_shape_for_all_three_primitives() {
        let questions = BTreeMap::from([
            (
                "is_urgent".to_string(),
                Question::Noul {
                    instructions: json!("Urgent?"),
                    criteria: None,
                },
            ),
            (
                "direction".to_string(),
                Question::Choice {
                    instructions: json!("Direction?"),
                    criteria: BTreeMap::from([("up".to_string(), "rises".to_string())]),
                },
            ),
            (
                "size".to_string(),
                Question::Score {
                    instructions: json!("How large?"),
                    criteria: vec!["small".to_string(), "large".to_string()],
                },
            ),
        ]);
        let request = build_request(
            &json!({"headline": "x"}),
            "~typesafe/jev-latest",
            &questions,
        );

        assert_eq!(request["model"], "~typesafe/jev-latest");
        assert_eq!(request["state"]["headline"], "x");
        assert_eq!(request["questions"]["is_urgent"]["type"], "noul");
        assert!(
            request["questions"]["is_urgent"].get("criteria").is_none(),
            "noul criteria are optional and must be omitted when unset"
        );
        assert_eq!(request["questions"]["direction"]["criteria"]["up"], "rises");
        assert_eq!(request["questions"]["size"]["criteria"][1], "large");
    }

    /// A rate card times an absent token count is `0.0`, which asserts the
    /// request was free. That is a stronger claim than "we do not know" and
    /// must not be summed as if it were an observation -- the same distinction
    /// `llm_usage::cost_from_usage` already draws for a missing `cost` field.
    #[test]
    fn a_rate_card_cost_is_only_computed_when_tokens_were_actually_reported() {
        let (cost, source) = cost_for_request(&json!({}), &JevUsage::default());
        assert_eq!(cost, None);
        assert_eq!(source, COST_SOURCE_NONE);

        let usage = JevUsage {
            input_tokens: 1_000_000,
            output_tokens: 500,
        };
        let (cost, source) =
            cost_for_request(&json!({"usage": {"input_tokens": 1_000_000}}), &usage);
        assert_eq!(source, COST_SOURCE_RATE_CARD);
        assert!(
            (cost.expect("a rate card cost") - RATE_CARD_INPUT_USD_PER_MILLION).abs() < 1e-12,
            "output tokens are free, so a million input tokens costs exactly the input rate"
        );
    }

    #[test]
    fn a_provider_reported_cost_wins_over_the_rate_card() {
        let usage = JevUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
        };
        let (cost, source) = cost_for_request(&json!({"usage": {"cost": 0.25}}), &usage);
        assert_eq!(cost, Some(0.25));
        assert_eq!(source, "billed");
    }

    #[test]
    fn reads_either_token_naming_the_two_gateways_use() {
        assert_eq!(
            usage_from_response(&json!({"usage": {"input_tokens": 120, "output_tokens": 7}})),
            JevUsage {
                input_tokens: 120,
                output_tokens: 7
            }
        );
        assert_eq!(
            usage_from_response(&json!({"usage": {"prompt_tokens": 120, "completion_tokens": 7}})),
            JevUsage {
                input_tokens: 120,
                output_tokens: 7
            }
        );
    }

    /// Retrying a rejected key or a malformed body spends the rate limit on
    /// the identical failure. Only the documented backoff cases and transient
    /// server errors are worth a second attempt.
    #[test]
    fn retryable_statuses_exclude_authentication_and_validation_failures() {
        for retryable in [429, 500, 502, 503, 529] {
            assert!(is_retryable_status(retryable), "{retryable} is transient");
        }
        for terminal in [400, 401, 403, 404, 422] {
            assert!(
                !is_retryable_status(terminal),
                "{terminal} will not improve"
            );
        }
    }

    /// The documented range is "position along levels", which does not say
    /// whether levels start at zero or one. Deriving the bounds from the
    /// legend that actually came back avoids guessing.
    #[test]
    fn a_score_is_normalized_against_the_legend_it_came_with() {
        let one_based = Answer::Score {
            score: 2.5,
            confidence: None,
            legend: BTreeMap::from([
                ("1".to_string(), "negligible".to_string()),
                ("2".to_string(), "modest".to_string()),
                ("3".to_string(), "large".to_string()),
                ("4".to_string(), "extreme".to_string()),
            ]),
        };
        assert_eq!(one_based.normalized_score(), Some(0.5));

        let zero_based = Answer::Score {
            score: 1.5,
            confidence: None,
            legend: BTreeMap::from([
                ("0".to_string(), "negligible".to_string()),
                ("1".to_string(), "modest".to_string()),
                ("2".to_string(), "large".to_string()),
                ("3".to_string(), "extreme".to_string()),
            ]),
        };
        assert_eq!(zero_based.normalized_score(), Some(0.5));

        let no_legend = Answer::Score {
            score: 2.5,
            confidence: None,
            legend: BTreeMap::new(),
        };
        assert_eq!(
            no_legend.normalized_score(),
            None,
            "without a legend there is nothing to scale against, and a guess would be silent"
        );
    }

    fn yaml(source: &str) -> YamlValue {
        serde_yaml::from_str(source).expect("test yaml parses")
    }

    /// A deliberately disabled integration and a missing key are different
    /// operational facts. Collapsing them into one "off" hides a
    /// misconfiguration behind what looks like a choice.
    #[test]
    fn a_missing_key_is_distinguishable_from_a_disabled_integration() {
        assert_eq!(
            JevConfig::from_yaml(&yaml("jev:\n  enabled: false\n")),
            JevAvailability::Disabled
        );
        assert_eq!(
            JevConfig::from_yaml(&yaml("other: 1\n")),
            JevAvailability::Disabled
        );
        assert_eq!(
            JevConfig::from_yaml(&yaml("jev:\n  enabled: true\n  api_key: '   '\n")),
            JevAvailability::MissingApiKey
        );
        assert_eq!(
            JevConfig::from_yaml(&yaml(
                "jev:\n  enabled: true\n  api_key: ENV:JEV_KEY_THAT_IS_NOT_SET\n"
            )),
            JevAvailability::MissingApiKey
        );
    }

    #[test]
    fn applies_documented_defaults_and_strips_a_trailing_slash_from_the_base_url() {
        let JevAvailability::Ready(cfg) = JevConfig::from_yaml(&yaml(
            "jev:\n  enabled: true\n  api_key: literal-key\n  base_url: https://example.test/api/v1/\n",
        )) else {
            panic!("a literal key is ready");
        };
        assert_eq!(cfg.base_url, "https://example.test/api/v1");
        assert_eq!(cfg.systemone_url(), "https://example.test/api/v1/systemone");
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.http_timeout_seconds, DEFAULT_TIMEOUT_SECONDS as u64);
    }

    #[test]
    fn the_floating_alias_survives_config_reading_unchanged() {
        let JevAvailability::Ready(cfg) = JevConfig::from_yaml(&yaml(
            "jev:\n  enabled: true\n  api_key: literal-key\n  model: ~typesafe/jev-latest\n",
        )) else {
            panic!("a literal key is ready");
        };
        assert_eq!(
            cfg.model, "~typesafe/jev-latest",
            "the tilde is part of the alias and must reach the request body intact"
        );
    }

    /// Truncating a JSON state would both invalidate it and silently change
    /// what the model was shown, making any probability derived from it
    /// unreproducible. Refusing is the honest failure.
    #[tokio::test]
    async fn an_oversized_state_is_refused_rather_than_truncated() {
        let cfg = JevConfig {
            api_key: "k".to_string(),
            base_url: "https://example.invalid".to_string(),
            model: DEFAULT_MODEL.to_string(),
            http_timeout_seconds: 1,
            max_state_chars: 64,
            max_questions_per_request: 12,
        };
        let error = ask(&cfg, &json!({"text": "x".repeat(500)}), &noul_question())
            .await
            .expect_err("an oversized state is refused");
        assert!(error.to_string().contains("above the configured maximum"));
    }

    #[tokio::test]
    async fn an_empty_or_oversized_question_set_never_reaches_the_network() {
        let cfg = JevConfig {
            api_key: "k".to_string(),
            base_url: "https://example.invalid".to_string(),
            model: DEFAULT_MODEL.to_string(),
            http_timeout_seconds: 1,
            max_state_chars: 24_000,
            max_questions_per_request: 1,
        };
        let empty = ask(&cfg, &json!({}), &BTreeMap::new())
            .await
            .expect_err("an empty question set is refused");
        assert!(empty.to_string().contains("no questions"));

        let mut too_many = noul_question();
        too_many.extend(choice_question());
        let error = ask(&cfg, &json!({}), &too_many)
            .await
            .expect_err("more questions than configured is refused");
        assert!(error.to_string().contains("above the configured maximum"));
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        assert_eq!(backoff_delay(1), Duration::from_millis(BASE_BACKOFF_MS));
        assert_eq!(backoff_delay(2), Duration::from_millis(BASE_BACKOFF_MS * 2));
        assert_eq!(backoff_delay(3), Duration::from_millis(BASE_BACKOFF_MS * 4));
        assert!(backoff_delay(64) <= Duration::from_millis(BASE_BACKOFF_MS * 64));
    }

    /// The key is the one value in this module that must never be serialized
    /// anywhere a reader can reach. Mirrors the redaction assertions used
    /// throughout `state.rs`.
    #[test]
    fn the_api_key_never_reaches_a_serialized_request() {
        let cfg = JevConfig {
            api_key: "must-not-reach-the-dashboard".to_string(),
            base_url: "https://example.invalid".to_string(),
            model: DEFAULT_MODEL.to_string(),
            http_timeout_seconds: 1,
            max_state_chars: 24_000,
            max_questions_per_request: 12,
        };
        let request = build_request(&json!({"headline": "x"}), &cfg.model, &noul_question());
        assert!(
            !serde_json::to_string(&request)
                .expect("request serializes")
                .contains("must-not-reach-the-dashboard"),
            "the key travels in the Authorization header only, never in the body"
        );
    }

    /// Phase 0 exit criterion: one real call against the live endpoint.
    ///
    /// Ignored by default -- it needs the network and a key, and it spends a
    /// genuine (if negligible) amount of money. Two things the published docs
    /// do not pin down can only be learned here, so both are printed rather
    /// than asserted: whether OpenRouter's `/systemone` returns a `usage`
    /// block, and what shape its errors take.
    ///
    /// Run with:
    ///   set -a && . ./.env && set +a && \
    ///     cargo test jev_smoke -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "makes a real, billable network call; run explicitly with --ignored"]
    async fn jev_smoke_round_trips_all_three_primitives() {
        let Ok(api_key) = std::env::var("JEV_OPENROUTER_API_KEY") else {
            panic!(
                "JEV_OPENROUTER_API_KEY is not exported. \
                 Run: set -a && . ./.env && set +a && cargo test jev_smoke -- --ignored --nocapture"
            );
        };
        let cfg = JevConfig {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            http_timeout_seconds: 30,
            max_state_chars: 24_000,
            max_questions_per_request: 12,
        };

        // Synthetic. No position, symbol, or holding of the real book appears
        // here: a smoke test must not publish portfolio state to a third party.
        let state = json!({
            "headline": "ACME Corp beats Q3 earnings estimates and raises full-year guidance",
            "symbol": "ACME",
        });
        let questions = BTreeMap::from([
            (
                "about_symbol".to_string(),
                Question::Noul {
                    instructions: json!("Is `headline` about the company trading as `symbol`?"),
                    criteria: None,
                },
            ),
            (
                "direction".to_string(),
                Question::Choice {
                    instructions: json!("What direction does `headline` imply for `symbol`?"),
                    criteria: BTreeMap::from([
                        (
                            "bullish".to_string(),
                            "Implies the price should rise".to_string(),
                        ),
                        (
                            "bearish".to_string(),
                            "Implies the price should fall".to_string(),
                        ),
                        (
                            "neutral".to_string(),
                            "No directional implication".to_string(),
                        ),
                    ]),
                },
            ),
            (
                "materiality".to_string(),
                Question::Score {
                    instructions: json!("How large is the move `headline` implies for `symbol`?"),
                    criteria: vec![
                        "Negligible, under 1 percent".to_string(),
                        "Modest, 1 to 3 percent".to_string(),
                        "Large, 3 to 10 percent".to_string(),
                        "Extreme, over 10 percent".to_string(),
                    ],
                },
            ),
        ]);

        let response = ask(&cfg, &state, &questions)
            .await
            .expect("Jev smoke call succeeds");

        println!("model_resolved = {:?}", response.model_resolved);
        println!("usage          = {:?}", response.usage);
        println!("latency_ms     = {}", response.latency_ms);
        println!("attempts       = {}", response.attempts);
        println!("issues         = {:?}", response.issues);
        for (id, answer) in &response.answers {
            println!("answer[{id}] = {answer:?}");
        }

        assert!(
            response.issues.is_empty(),
            "every question should answer cleanly: {:?}",
            response.issues
        );
        assert!(matches!(
            response.answers.get("about_symbol"),
            Some(Answer::Noul { .. })
        ));
        assert!(matches!(
            response.answers.get("direction"),
            Some(Answer::Choice { .. })
        ));
        assert!(matches!(
            response.answers.get("materiality"),
            Some(Answer::Score { .. })
        ));
        assert!(
            response.usage.input_tokens > 0,
            "a usage block with input tokens is what makes the cost ledger work; \
             if this fails, cost_for_request will report not_reported for every Jev call"
        );
    }
}
