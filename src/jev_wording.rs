//! A seeded challenge set for the wording grader.
//!
//! The numeric checker could be measured because arithmetic settles itself.
//! Wording cannot: whether "consolidating securely" overstates a moderate
//! break risk is a judgement, and a set built from judgements measures the
//! person who built it.
//!
//! So this set asks only questions the evidence already answers.
//!
//! **A categorical field takes one value.** If every categorical field in a
//! candidate's evidence reads positive — `trend_bias: bullish`, `sentiment:
//! BUY`, `state: Bull`, `direction: long`, `break_risk_label: low` — then a
//! note calling the setup bearish is wrong, and it is wrong without anyone
//! deciding how strong a word is. That is the whole of the detection arm.
//!
//! **A note built from figures alone makes no qualitative claim.** That is the
//! fourth option in the rubric, and a note assembled out of the evidence's own
//! numbers is an instance of it by construction.
//!
//! **A claim about something the evidence does not contain is unsupported.**
//! Analyst targets, insider ownership and earnings surprises appear nowhere in
//! a candidate's evidence, so appending one is a seeded instance of the
//! sufficiency question — which has never been measured at all.
//!
//! **And two readings of the same claim should agree.** The same note asked
//! twice, and the same note with a synonym substituted, should get the same
//! verdict. This is the arm with no seeded error in it: it measures whether
//! the grader is stable enough for a disagreement to mean anything.
//!
//! What is deliberately absent is an arm for `overstated`. Every construction
//! of one needs a threshold — how small is "modest", how strong is "strong" —
//! and a threshold I choose is my judgement wearing an answer key's clothes.
//! The consequence is that **the most common non-`fair` verdict in production
//! is the one this set cannot measure**, and that is stated here rather than
//! buried.
//!
//! Scoring needs real provider calls, so it cannot run in the build. The
//! generator and the case invariants are hermetic; the scorer is `#[ignore]`d
//! and writes its results to a dated document.
//!
//! The payload is the same shape the production grading loop already sends to
//! the same provider on every cycle: a trimmed report, candidate notes, and
//! per-candidate indicator, Markov and Quiver summaries. `top_events`, which
//! names individuals, is excluded upstream. No account identifier, position
//! size or credential is in it.

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

pub(crate) const WORDING_SET_VERSION: &str = "wording-v1-2026-09-22";

/// What the evidence settles about the mutated note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Expect {
    /// No ground truth. Recorded so a paired case has something to match.
    Observed,
    /// A characterisation the evidence contradicts, so any verdict but `fair`
    /// is a detection.
    Flagged,
    /// The note is figures and field names only.
    NoQualitativeClaim,
    /// Nothing about the wording's quality changed, so the verdict should not
    /// change either.
    SameAsBaseline,
    /// The note asserts something with no counterpart in the evidence.
    AssertsAbsentEvidence,
}

/// One case: a real report with one candidate's note changed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WordingCase {
    pub id: String,
    pub arm: String,
    pub expect: Expect,
    pub rationale: String,
    pub symbol: String,
    pub source_report: i64,
    /// Which candidate in `candidates` was changed.
    pub target_index: usize,
    /// For `SameAsBaseline`, the case whose verdict this one must equal.
    pub baseline_id: Option<String>,
    pub report: JsonValue,
    pub candidates: Vec<JsonValue>,
    pub evidence: Vec<JsonValue>,
    pub policy: Option<JsonValue>,
}

/// The categorical fields a note's adjectives can refer to.
pub(crate) fn categorical_values(evidence: &JsonValue) -> Vec<(&'static str, String)> {
    let read = |path: &'static str| -> Option<(&'static str, String)> {
        path.split('.')
            .try_fold(evidence, |cursor, segment| cursor.get(segment))
            .and_then(JsonValue::as_str)
            .map(|value| (path, value.to_lowercase()))
    };
    [
        "daily_indicators.trend_bias",
        "daily_indicators.sentiment",
        "daily_indicators.support.break_risk_label",
        "markov.state",
        "markov.direction",
        "quiver.direction",
    ]
    .into_iter()
    .filter_map(read)
    .collect()
}

/// Which side of the setup a categorical value is on.
///
/// `moderate`, `hold`, `neutral` and `sideways` are on neither, and a case is
/// only built where no field sits on the fence — a flip has to be contradicted
/// by every field it could possibly refer to, not most of them.
pub(crate) fn camp(value: &str) -> Option<&'static str> {
    match value {
        "bullish" | "bull" | "buy" | "long" | "low" => Some("positive"),
        "bearish" | "bear" | "sell" | "short" | "high" => Some("negative"),
        _ => None,
    }
}

/// Whether every categorical field in this evidence is unambiguously positive.
pub(crate) fn wholly_positive(evidence: &JsonValue) -> bool {
    let values = categorical_values(evidence);
    !values.is_empty()
        && values
            .iter()
            .all(|(_, value)| camp(value) == Some("positive"))
}

/// Phrases whose replacement asserts a categorical value the evidence denies.
///
/// Each is a positive reading turned negative. They are applied only to a
/// candidate whose every categorical field is positive, so whichever field the
/// phrase refers to, the replacement is false.
pub(crate) const CATEGORY_FLIPS: &[(&str, &str)] = &[
    ("bull-state markov", "bear-state markov"),
    ("bull markov", "bear markov"),
    ("bullish trend", "bearish trend"),
    ("bullish daily trend", "bearish daily trend"),
    ("bullish technical", "bearish technical"),
    ("bullish", "bearish"),
    ("markov long", "markov short"),
    ("long markov", "short markov"),
    ("low support-break risk", "high support-break risk"),
    (
        "low modeled support-break risk",
        "high modeled support-break risk",
    ),
    ("low break risk", "high break risk"),
    ("buy technical", "sell technical"),
];

/// Synonym substitutions that change no characterisation.
///
/// None of them is a word of degree: swapping "strong" for "powerful" would
/// change what is claimed, which is the opposite of the point.
pub(crate) const PARAPHRASES: &[(&str, &str)] = &[
    ("remains", "continues to be"),
    ("supported by", "backed by"),
    ("candidate", "name"),
    ("setup", "configuration"),
    ("qualified", "eligible"),
    ("existing", "current"),
    ("fresh", "recent"),
    ("starter", "opening position"),
];

/// Claims with no counterpart anywhere in a candidate's evidence.
///
/// Deliberately outside the categories the question declares out of scope by
/// design — portfolio capital, holdings, unrealised profit, trading costs —
/// so an answer of "sufficient" cannot be excused as disregarding them.
pub(crate) const INVENTED: &[&str] = &[
    " Analyst consensus implies 18% upside to target.",
    " Insider ownership rose to 12% last quarter.",
    " The most recent earnings beat estimates by 9%.",
    " Short interest stands at 4.2% of float.",
    " The sector rotation model ranks it first of forty.",
];

/// How a grader's answer to a case turned out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The seeded error was flagged, the paraphrase held its verdict, the
    /// figures-only note was called figures-only, or the invented claim was
    /// marked unsupported.
    Correct,
    /// The seeded error was called `fair`, or the invented claim was called
    /// supported.
    Missed,
    /// A paired case changed its verdict although nothing about the wording
    /// changed.
    Unstable,
    /// The case carries no ground truth and is recorded only.
    Observed,
    /// No answer came back for this candidate.
    Unanswered,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::Missed => "missed",
            Self::Unstable => "unstable",
            Self::Observed => "observed",
            Self::Unanswered => "unanswered",
        }
    }
}

/// Scores one answered case.
///
/// `verdict` is the wording choice and `sufficiency` the separate probability;
/// `baseline` is the verdict the paired control received, where there is one.
pub(crate) fn outcome_for(
    case: &WordingCase,
    verdict: Option<&str>,
    sufficiency: Option<f64>,
    baseline: Option<&str>,
) -> Outcome {
    match case.expect {
        Expect::Observed => {
            if verdict.is_some() {
                Outcome::Observed
            } else {
                Outcome::Unanswered
            }
        }
        Expect::Flagged => match verdict {
            None => Outcome::Unanswered,
            Some(crate::jev_signals::WORDING_FAIR) => Outcome::Missed,
            Some(_) => Outcome::Correct,
        },
        Expect::NoQualitativeClaim => match verdict {
            None => Outcome::Unanswered,
            Some(crate::jev_signals::WORDING_NONE) => Outcome::Correct,
            Some(_) => Outcome::Missed,
        },
        Expect::SameAsBaseline => match (verdict, baseline) {
            (None, _) | (_, None) => Outcome::Unanswered,
            (Some(verdict), Some(baseline)) if verdict == baseline => Outcome::Correct,
            _ => Outcome::Unstable,
        },
        Expect::AssertsAbsentEvidence => match sufficiency {
            None => Outcome::Unanswered,
            Some(value) if value >= 0.5 => Outcome::Correct,
            Some(_) => Outcome::Missed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(expect: Expect) -> WordingCase {
        WordingCase {
            id: "t".into(),
            arm: "a".into(),
            expect,
            rationale: "r".into(),
            symbol: "S".into(),
            source_report: 0,
            target_index: 0,
            baseline_id: None,
            report: json!({}),
            candidates: vec![],
            evidence: vec![],
            policy: None,
        }
    }

    /// A flip is only seeded where the evidence leaves no room to read the
    /// original as wrong. One field on the fence and the case is not built.
    #[test]
    fn a_flip_needs_every_categorical_field_on_the_same_side() {
        let positive = json!({
            "daily_indicators": {"trend_bias": "bullish", "sentiment": "BUY",
                                 "support": {"break_risk_label": "low"}},
            "markov": {"state": "Bull", "direction": "long"},
            "quiver": {"direction": "bullish"},
        });
        assert!(wholly_positive(&positive));

        for fence in ["neutral", "moderate", "hold", "sideways"] {
            let mut mixed = positive.clone();
            mixed["daily_indicators"]["trend_bias"] = json!(fence);
            assert!(!wholly_positive(&mixed), "{fence}");
        }
        let mut negative = positive.clone();
        negative["markov"]["direction"] = json!("short");
        assert!(!wholly_positive(&negative));

        // Nothing categorical at all is not "wholly positive" either.
        assert!(!wholly_positive(
            &json!({"daily_indicators": {"rsi14": 50.0}})
        ));
    }

    /// Any verdict but `fair` counts as detection. Requiring the exact
    /// category would measure the rubric's wording as much as the grader's
    /// judgement, and `overstated` and `misdescribes_category` genuinely
    /// overlap on a flipped label.
    #[test]
    fn detection_is_any_verdict_other_than_fair() {
        let seeded = case(Expect::Flagged);
        assert_eq!(
            outcome_for(&seeded, Some(crate::jev_signals::WORDING_FAIR), None, None),
            Outcome::Missed
        );
        for flagged in [
            crate::jev_signals::WORDING_OVERSTATED,
            crate::jev_signals::WORDING_MISDESCRIBES,
        ] {
            assert_eq!(
                outcome_for(&seeded, Some(flagged), None, None),
                Outcome::Correct
            );
        }
        assert_eq!(outcome_for(&seeded, None, None, None), Outcome::Unanswered);
    }

    /// A paired case has no seeded error in it: it is wrong only by
    /// disagreeing with itself.
    #[test]
    fn a_paraphrase_is_wrong_only_when_the_verdict_moves() {
        let paired = case(Expect::SameAsBaseline);
        assert_eq!(
            outcome_for(&paired, Some("fair"), None, Some("fair")),
            Outcome::Correct
        );
        assert_eq!(
            outcome_for(&paired, Some("overstated"), None, Some("overstated")),
            Outcome::Correct,
            "agreeing on a flag is agreement too"
        );
        assert_eq!(
            outcome_for(&paired, Some("fair"), None, Some("overstated")),
            Outcome::Unstable
        );
        assert_eq!(
            outcome_for(&paired, Some("fair"), None, None),
            Outcome::Unanswered,
            "with no baseline there is nothing to be stable against"
        );
    }

    /// The sufficiency axis is read from its own probability, not from the
    /// wording verdict.
    #[test]
    fn an_invented_claim_is_judged_on_the_sufficiency_answer_alone() {
        let invented = case(Expect::AssertsAbsentEvidence);
        assert_eq!(
            outcome_for(&invented, Some("fair"), Some(0.9), None),
            Outcome::Correct
        );
        assert_eq!(
            outcome_for(&invented, Some("overstated"), Some(0.2), None),
            Outcome::Missed
        );
        assert_eq!(
            outcome_for(&invented, Some("fair"), None, None),
            Outcome::Unanswered
        );
    }
}

/// Builds the frozen set, and scores it against the live grader.
///
/// Both are `#[ignore]`d. The generator needs a dump of production reports
/// with their prompts; the scorer needs the API key and spends money:
///
/// ```text
/// JEV_PROMPTS_PATH=prompts.json cargo test regenerate_the_wording_set -- --ignored --nocapture
/// set -a && . ./.env && set +a
/// cargo test score_the_wording_set -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live {
    use super::*;
    use std::collections::BTreeMap;

    const WORDING_PATH: &str = "docs/jev-wording-challenge-v1.json";
    const PER_ARM: usize = 12;
    /// Flip cases are capped per phrase, not per arm.
    ///
    /// The first run drew nine of its twelve from `bullish trend` and only
    /// three from `bull markov` -- and those three were the ones the grader
    /// missed. A arm-wide cap lets whichever phrase happens to be commonest in
    /// the corpus decide what the arm measures.
    const PER_FLIP_PHRASE: usize = 5;

    fn load() -> Vec<WordingCase> {
        let raw = std::fs::read_to_string(WORDING_PATH).expect("the frozen wording set");
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        serde_json::from_value(document["cases"].clone()).expect("cases")
    }

    /// A candidate of a real report, with everything a grading call needs.
    #[derive(Clone)]
    struct Source {
        report_id: i64,
        symbol: String,
        note: String,
        index: usize,
        report: JsonValue,
        candidates: Vec<JsonValue>,
        evidence: Vec<JsonValue>,
        policy: Option<JsonValue>,
    }

    fn with_note(source: &Source, note: String) -> (Vec<JsonValue>, Vec<JsonValue>) {
        let mut candidates = source.candidates.clone();
        candidates[source.index]["note"] = json!(note);
        // The numeric checks travel with the candidate and are re-derived, so a
        // mutated note is never shown beside findings about the old one.
        let checks = crate::jev_numeric::numeric_checks(
            candidates[source.index]["note"]
                .as_str()
                .unwrap_or_default(),
            &source.evidence[source.index],
        );
        candidates[source.index]["numeric_checks"] = json!(
            checks
                .iter()
                .map(|check| json!({
                    "quoted": check.quoted,
                    "field": check.field,
                    "actual": check.actual,
                    "relation": check.relation,
                    "verdict": check.verdict.as_str(),
                }))
                .collect::<Vec<_>>()
        );
        (candidates, source.evidence.clone())
    }

    fn case(
        source: &Source,
        id: String,
        arm: &str,
        expect: Expect,
        rationale: String,
        note: String,
        baseline_id: Option<String>,
    ) -> WordingCase {
        let (candidates, evidence) = with_note(source, note);
        WordingCase {
            id,
            arm: arm.to_string(),
            expect,
            rationale,
            symbol: source.symbol.clone(),
            source_report: source.report_id,
            target_index: source.index,
            baseline_id,
            report: source.report.clone(),
            candidates,
            evidence,
            policy: source.policy.clone(),
        }
    }

    /// A note made of the evidence's own figures and nothing else.
    fn figures_only(evidence: &JsonValue) -> Option<String> {
        let number = |path: &str| -> Option<f64> {
            path.split('.')
                .try_fold(evidence, |cursor, segment| cursor.get(segment))
                .and_then(JsonValue::as_f64)
        };
        let count = number("daily_indicators.confluence_count")?;
        let minimum = number("daily_indicators.min_confluences")?;
        let rsi = number("daily_indicators.rsi14")?;
        let signal = number("markov.signed_signal")?;
        Some(format!(
            "{count:.0}/{minimum:.0} confluences, rsi {rsi:.1}, markov signed signal {signal:.3}."
        ))
    }

    #[test]
    #[ignore]
    fn regenerate_the_wording_set() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let raw = std::fs::read_to_string(path).expect("prompts");
        let sources_json: Vec<JsonValue> = serde_json::from_str(&raw).expect("a JSON array");

        let mut sources = Vec::new();
        for entry in &sources_json {
            let (Some(report), Some(request)) = (entry.get("report"), entry.get("request")) else {
                continue;
            };
            let report_id = entry.get("id").and_then(JsonValue::as_i64).unwrap_or(-1);
            let prompt = crate::xai_decision::decision_prompt_user_payload(request);
            let inputs = crate::jev_review::grading_inputs(report, &prompt);
            for (index, candidate) in inputs.candidates.iter().enumerate() {
                let (Some(note), Some(evidence)) = (
                    candidate.get("note").and_then(JsonValue::as_str),
                    inputs.evidence.get(index),
                ) else {
                    continue;
                };
                if !wholly_positive(evidence) {
                    continue;
                }
                sources.push(Source {
                    report_id,
                    symbol: candidate
                        .get("symbol")
                        .and_then(JsonValue::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    note: note.to_string(),
                    index,
                    report: inputs.report.clone(),
                    candidates: inputs.candidates.clone(),
                    evidence: inputs.evidence.clone(),
                    policy: inputs.policy.clone(),
                });
            }
        }
        println!("sources={}", sources.len());

        let mut cases: Vec<WordingCase> = Vec::new();
        let mut taken: BTreeMap<&str, usize> = BTreeMap::new();
        let mut per_phrase: BTreeMap<&str, usize> = BTreeMap::new();
        for source in &sources {
            let lowered = source.note.to_lowercase();
            let mut count = |arm: &'static str, cap: usize| -> Option<usize> {
                let seen = taken.entry(arm).or_default();
                (*seen < cap).then(|| {
                    *seen += 1;
                    *seen
                })
            };

            // The flip, and with it the control the paired arms compare to.
            if let Some((phrase, replacement)) = CATEGORY_FLIPS
                .iter()
                .find(|(phrase, _)| lowered.contains(phrase))
            {
                let seen = per_phrase.entry(*phrase).or_insert(0usize);
                let room = *seen < PER_FLIP_PHRASE;
                if room {
                    *seen += 1;
                }
                if let Some(nth) = room.then(|| count("category_flip", usize::MAX)).flatten() {
                    let start = lowered.find(phrase).expect("found above");
                    let flipped = format!(
                        "{}{replacement}{}",
                        &source.note[..start],
                        &source.note[start + phrase.len()..]
                    );
                    cases.push(case(
                        source,
                        format!("category_flip-{nth:03}"),
                        "category_flip",
                        Expect::Flagged,
                        format!(
                            "\"{phrase}\" replaced by \"{replacement}\"; every categorical field \
                             in this candidate's evidence reads the other way"
                        ),
                        flipped,
                        None,
                    ));
                }
            }

            if let Some(nth) = count("control", PER_ARM) {
                let control = format!("control-{nth:03}");
                cases.push(case(
                    source,
                    control.clone(),
                    "control",
                    Expect::Observed,
                    "the note as written; no ground truth, recorded so the paired arms have \
                     something to agree with"
                        .into(),
                    source.note.clone(),
                    None,
                ));
                cases.push(case(
                    source,
                    format!("repeat-{nth:03}"),
                    "repeat",
                    Expect::SameAsBaseline,
                    "the same note asked a second time".into(),
                    source.note.clone(),
                    Some(control.clone()),
                ));
                if let Some((word, synonym)) =
                    PARAPHRASES.iter().find(|(word, _)| lowered.contains(word))
                {
                    let start = lowered.find(word).expect("found above");
                    cases.push(case(
                        source,
                        format!("paraphrase-{nth:03}"),
                        "paraphrase",
                        Expect::SameAsBaseline,
                        format!(
                            "\"{word}\" replaced by \"{synonym}\", which changes no
                                 characterisation"
                        ),
                        format!(
                            "{}{synonym}{}",
                            &source.note[..start],
                            &source.note[start + word.len()..]
                        ),
                        Some(control),
                    ));
                }
            }

            if let Some(stripped) = figures_only(&source.evidence[source.index]) {
                if let Some(nth) = count("stripped", PER_ARM) {
                    cases.push(case(
                        source,
                        format!("stripped-{nth:03}"),
                        "stripped",
                        Expect::NoQualitativeClaim,
                        "the note replaced by the evidence's own figures, with no word of \
                         characterisation in it"
                            .into(),
                        stripped,
                        None,
                    ));
                }
            }

            if let Some(nth) = count("invented_evidence", PER_ARM) {
                let claim = INVENTED[(nth - 1) % INVENTED.len()];
                cases.push(case(
                    source,
                    format!("invented_evidence-{nth:03}"),
                    "invented_evidence",
                    Expect::AssertsAbsentEvidence,
                    format!(
                        "appends \"{}\", which has no counterpart in the evidence",
                        claim.trim()
                    ),
                    format!("{}{claim}", source.note),
                    None,
                ));
            }
        }

        for arm in [
            "control",
            "repeat",
            "paraphrase",
            "category_flip",
            "stripped",
            "invented_evidence",
        ] {
            println!("arm {arm:20} {}", taken.get(arm).copied().unwrap_or(0));
        }
        let document = json!({
            "version": WORDING_SET_VERSION,
            "generated": "2026-09-22",
            "grading_version": crate::jev_review::report_grading_version(),
            "answer_key": "what the evidence settles about the changed note, decided by the \
                           change and never by asking the grader",
            "cases": cases,
        });
        std::fs::write(
            WORDING_PATH,
            serde_json::to_string_pretty(&document).expect("serialize"),
        )
        .expect("write");
        println!("cases={} written to {WORDING_PATH}", cases.len());
    }

    /// Asks the live grader every case and writes the results.
    ///
    /// One call per case, in the production shape: the whole report's question
    /// set, with one candidate's note changed. Asking a single candidate in
    /// isolation would measure a prompt the grader never actually sees.
    #[tokio::test]
    #[ignore]
    async fn score_the_wording_set() {
        let Ok(api_key) = std::env::var("JEV_OPENROUTER_API_KEY") else {
            panic!(
                "JEV_OPENROUTER_API_KEY is not exported. Run: \
                 set -a && . ./.env && set +a && \
                 cargo test score_the_wording_set -- --ignored --nocapture"
            );
        };
        let cfg = crate::jev::JevConfig {
            api_key,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "~typesafe/jev-latest".to_string(),
            http_timeout_seconds: 30,
            max_state_chars: 24_000,
            max_questions_per_request: 24,
        };

        let cases = load();
        let mut verdicts: BTreeMap<String, String> = BTreeMap::new();
        let mut answered: Vec<(String, Option<String>, Option<f64>, Option<f64>)> = Vec::new();
        let mut spent = 0.0f64;
        let mut failures = Vec::new();

        for entry in &cases {
            let state = crate::jev_signals::report_grading_state(
                &entry.report,
                &entry.candidates,
                &entry.evidence,
                entry.policy.as_ref(),
            );
            let questions = crate::jev_signals::report_grading_questions(entry.candidates.len());
            match crate::jev::ask(&cfg, &state, &questions).await {
                Ok(response) => {
                    spent += response.cost_usd.unwrap_or(0.0);
                    let claim = crate::jev_signals::claim_question_id(entry.target_index);
                    let (verdict, confidence) = match response.answers.get(&claim) {
                        Some(crate::jev::Answer::Choice {
                            choice, confidence, ..
                        }) => (Some(choice.clone()), *confidence),
                        _ => (None, None),
                    };
                    let sufficiency =
                        match response
                            .answers
                            .get(&crate::jev_signals::sufficiency_question_id(
                                entry.target_index,
                            )) {
                            Some(crate::jev::Answer::Noul { noul }) => Some(*noul),
                            _ => None,
                        };
                    if let Some(verdict) = &verdict {
                        verdicts.insert(entry.id.clone(), verdict.clone());
                    }
                    println!(
                        "{:28} {:22} verdict={:24} sufficiency={:?}",
                        entry.id,
                        entry.arm,
                        verdict.clone().unwrap_or_else(|| "-".into()),
                        sufficiency
                    );
                    answered.push((entry.id.clone(), verdict, confidence, sufficiency));
                }
                Err(err) => {
                    // Named, not swallowed. A case that never got an answer is
                    // not a case that passed.
                    failures.push(json!({"id": entry.id, "error": err.to_string()}));
                    answered.push((entry.id.clone(), None, None, None));
                }
            }
        }

        let mut by_arm: BTreeMap<String, BTreeMap<&'static str, i64>> = BTreeMap::new();
        let mut detail = Vec::new();
        for (entry, (_, verdict, confidence, sufficiency)) in cases.iter().zip(&answered) {
            let baseline = entry
                .baseline_id
                .as_ref()
                .and_then(|id| verdicts.get(id))
                .map(String::as_str);
            let outcome = outcome_for(entry, verdict.as_deref(), *sufficiency, baseline);
            *by_arm
                .entry(entry.arm.clone())
                .or_default()
                .entry(outcome.as_str())
                .or_default() += 1;
            detail.push(json!({
                "id": entry.id,
                "arm": entry.arm,
                "expect": entry.expect,
                "outcome": outcome.as_str(),
                "verdict": verdict,
                "confidence": confidence,
                "sufficiency": sufficiency,
                "baseline_verdict": baseline,
                "rationale": entry.rationale,
                "note": entry.candidates[entry.target_index]["note"],
            }));
        }

        let results = json!({
            "set": WORDING_SET_VERSION,
            "grading_version": crate::jev_review::report_grading_version(),
            "run_at": crate::jev_signals::now_rfc3339(),
            "cases": cases.len(),
            "billed_usd": spent,
            "by_arm": by_arm,
            "failures": failures,
            "detail": detail,
            "reading": "`correct` means the seeded error was flagged, the figures-only note was \
                        called figures-only, the invented claim was marked unsupported, or a \
                        paired case held its verdict. `missed` is a seeded error called fair. \
                        `unstable` is a verdict that moved although the wording did not. \
                        `observed` carries no ground truth. One run, so every number here is a \
                        single sample of a probabilistic answer.",
        });
        let path = "docs/jev-wording-results-2026-09-22.json";
        std::fs::write(
            path,
            serde_json::to_string_pretty(&results).expect("serialize"),
        )
        .expect("write");
        println!("\n{}", serde_json::to_string_pretty(&by_arm).expect("json"));
        println!(
            "billed USD {spent:.6}; {} failures; written to {path}",
            failures.len()
        );
    }

    /// Every invariant the cases must satisfy, checked without a provider.
    #[test]
    fn the_wording_cases_are_well_formed() {
        let Ok(raw) = std::fs::read_to_string(WORDING_PATH) else {
            return;
        };
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        let cases: Vec<WordingCase> =
            serde_json::from_value(document["cases"].clone()).expect("cases");
        assert!(cases.len() >= 50, "the set shrank: {}", cases.len());

        let ids: std::collections::BTreeSet<&str> =
            cases.iter().map(|case| case.id.as_str()).collect();
        for entry in &cases {
            assert!(
                entry.target_index < entry.candidates.len(),
                "{}: target out of range",
                entry.id
            );
            let evidence = &entry.evidence[entry.target_index];
            // The detection arm is only sound where the evidence leaves no
            // room to read the original as already wrong.
            if entry.arm == "category_flip" {
                assert!(wholly_positive(evidence), "{}", entry.id);
            }
            if let Some(baseline) = &entry.baseline_id {
                assert!(
                    ids.contains(baseline.as_str()),
                    "{}: dangling baseline",
                    entry.id
                );
            }
            if entry.expect == Expect::SameAsBaseline {
                assert!(entry.baseline_id.is_some(), "{}: no baseline", entry.id);
            }
            // A figures-only note must contain no word of characterisation, or
            // it is not an instance of the option it claims to be.
            if entry.arm == "stripped" {
                let note = entry.candidates[entry.target_index]["note"]
                    .as_str()
                    .unwrap_or_default();
                for word in [
                    "bullish", "bearish", "strong", "weak", "low", "high", "secure",
                ] {
                    assert!(!note.contains(word), "{}: {note:?}", entry.id);
                }
            }
        }
    }
}
