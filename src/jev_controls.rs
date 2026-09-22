//! A blind labelling instrument for claims the checker did **not** flag.
//!
//! Everything measured so far reads in one direction. The seeded sets plant an
//! error and ask whether it is caught; the adjudication reads the nine claims
//! the checker flagged and asks whether it was right. Both measure precision.
//! Neither can measure a miss, because a claim the checker never flagged never
//! reaches anyone's desk — and 283 of the 1,445 distinct claims in the history,
//! **19.6%**, were never compared at all.
//!
//! This module does not measure anything. It builds the thing a measurement
//! needs and cannot produce for itself: a frozen, randomly drawn, **blind**
//! sample of natural claims, with the evidence beside each one and the
//! checker's answer removed.
//!
//! Three properties make it worth labelling.
//!
//! **The verdict is not in the instrument.** `docs/jev-controls-v1.json` gives
//! the note, the figure and the candidate's evidence. The checker's verdict and
//! the field it attributed live in a separate key file. A labeller working from
//! the instrument cannot be anchored by an answer they have not seen.
//!
//! **The sample is drawn, not chosen.** Ordering within each stratum is by
//! `sha256(seed + case key)`, so the draw is reproducible from the seed and I
//! could not have preferred a claim I liked the look of. The seed is recorded.
//!
//! **Abstentions are sampled deliberately, not incidentally.** They are where a
//! missed error would sit, and sampling in proportion to their frequency would
//! have drawn too few of the rarer classes to say anything about them.
//!
//! The nine already-adjudicated `differs` are included, unmarked. They are the
//! attention check: a labelling pass that disagrees with the five confirmed
//! contradictions is telling you something about the pass.
//!
//! **I do not fill in the labels.** I wrote the checker; a label from me is the
//! same circularity in a new file. The `labels` field is empty by construction
//! and a test asserts it.

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

pub(crate) const CONTROLS_VERSION: &str = "controls-v1-2026-09-22";
/// Fixed, recorded, and used for nothing else. Changing it redraws the sample,
/// which is why it is written into the instrument.
pub(crate) const SAMPLE_SEED: &str = "jev-controls-v1";

/// One claim, as a labeller sees it. No verdict, no attributed field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ControlCase {
    pub id: String,
    pub source_report: i64,
    pub symbol: String,
    /// The candidate note in full, so the figure can be read in context.
    pub note: String,
    /// The figure as the note wrote it, and where it sits in `note`.
    pub figure: String,
    pub figure_offset: usize,
    /// Everything the report was given about this candidate.
    pub evidence: JsonValue,
}

/// What a labeller records. Every field starts empty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ControlLabel {
    pub id: String,
    /// `consistent`, `inconsistent`, or `cannot_tell`.
    pub verdict: String,
    /// The evidence field the figure names, dotted, or `none` where it names
    /// no field, or empty where the labeller could not tell.
    pub field: String,
    pub note: String,
}

/// The checker's answer, kept out of the instrument.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ControlKey {
    pub id: String,
    pub stratum: String,
    pub verdict: String,
    pub field: Option<String>,
}

/// How one labelled case scores against the checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Both say the claim holds, and on the same field.
    Agreed,
    /// The checker compared and agreed; the labeller says the claim is wrong.
    /// **The number this instrument exists to produce.**
    FalseNegative,
    /// The checker flagged; the labeller says the claim holds.
    FalsePositive,
    /// The checker never compared, and the labeller says the claim is wrong —
    /// an error it could not have caught.
    MissedByAbstention,
    /// The checker never compared, and there was nothing to catch.
    BenignAbstention,
    /// Both call the claim wrong.
    AgreedOnError,
    /// Same verdict, different field: right answer through a different
    /// attribution, which is not the same thing.
    FieldDisagreement,
    /// The labeller could not settle it.
    Unsettled,
    /// No label for this case.
    Unlabelled,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Agreed => "agreed",
            Self::FalseNegative => "false_negative",
            Self::FalsePositive => "false_positive",
            Self::MissedByAbstention => "missed_by_abstention",
            Self::BenignAbstention => "benign_abstention",
            Self::AgreedOnError => "agreed_on_error",
            Self::FieldDisagreement => "field_disagreement",
            Self::Unsettled => "unsettled",
            Self::Unlabelled => "unlabelled",
        }
    }
}

fn compared(verdict: &str) -> bool {
    verdict == "matches" || verdict == "differs"
}

/// Scores one case. Written before any label exists, so it cannot be tuned to
/// the labels it will meet.
pub(crate) fn outcome_for(key: &ControlKey, label: Option<&ControlLabel>) -> Outcome {
    let Some(label) = label else {
        return Outcome::Unlabelled;
    };
    match label.verdict.as_str() {
        "cannot_tell" => Outcome::Unsettled,
        "inconsistent" if key.verdict == "matches" => Outcome::FalseNegative,
        "inconsistent" if key.verdict == "differs" => Outcome::AgreedOnError,
        "inconsistent" => Outcome::MissedByAbstention,
        "consistent" if key.verdict == "differs" => Outcome::FalsePositive,
        "consistent" if !compared(&key.verdict) => Outcome::BenignAbstention,
        // Compared and agreed. The field still has to be the same one, or the
        // checker reached the right answer about something else.
        "consistent" => {
            if label.field.is_empty() || key.field.as_deref() == Some(label.field.as_str()) {
                Outcome::Agreed
            } else {
                Outcome::FieldDisagreement
            }
        }
        _ => Outcome::Unlabelled,
    }
}

/// The confusion table, by stratum and in total.
pub(crate) fn score(keys: &[ControlKey], labels: &[ControlLabel]) -> JsonValue {
    use std::collections::BTreeMap;

    let by_id: BTreeMap<&str, &ControlLabel> = labels
        .iter()
        .map(|label| (label.id.as_str(), label))
        .collect();
    let mut by_stratum: BTreeMap<String, BTreeMap<&'static str, i64>> = BTreeMap::new();
    let mut totals: BTreeMap<&'static str, i64> = BTreeMap::new();
    for key in keys {
        let outcome = outcome_for(key, by_id.get(key.id.as_str()).copied());
        *by_stratum
            .entry(key.stratum.clone())
            .or_default()
            .entry(outcome.as_str())
            .or_default() += 1;
        *totals.entry(outcome.as_str()).or_default() += 1;
    }
    let labelled: i64 = totals
        .iter()
        .filter(|(outcome, _)| **outcome != "unlabelled")
        .map(|(_, count)| count)
        .sum();
    json!({
        "version": CONTROLS_VERSION,
        "cases": keys.len(),
        "labelled": labelled,
        "totals": totals,
        "by_stratum": by_stratum,
        "reading": "`false_negative` is a claim the checker compared and agreed with that the \
                    labeller reads as wrong -- the quantity nothing measured so far could \
                    reach. `missed_by_abstention` is a wrong claim the checker never compared, \
                    which is a coverage failure rather than a judgement failure and is counted \
                    apart from it. `field_disagreement` is the same verdict about a different \
                    field. An unlabelled case is not a pass.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(verdict: &str, field: Option<&str>) -> ControlKey {
        ControlKey {
            id: "c".into(),
            stratum: "s".into(),
            verdict: verdict.into(),
            field: field.map(str::to_string),
        }
    }

    fn label(verdict: &str, field: &str) -> ControlLabel {
        ControlLabel {
            id: "c".into(),
            verdict: verdict.into(),
            field: field.into(),
            note: String::new(),
        }
    }

    /// The point of the instrument. A claim the checker compared and agreed
    /// with, that a labeller reads as wrong, is the one quantity none of the
    /// seeded sets or the adjudication could reach.
    #[test]
    fn a_wrong_claim_the_checker_agreed_with_is_a_false_negative() {
        assert_eq!(
            outcome_for(
                &key("matches", Some("daily_indicators.rsi14")),
                Some(&label("inconsistent", "daily_indicators.rsi14"))
            ),
            Outcome::FalseNegative
        );
    }

    /// A wrong claim the checker never compared is a different failure, and
    /// folding it into the first would blame the judgement for a gap in
    /// coverage.
    #[test]
    fn a_wrong_claim_never_compared_is_counted_apart() {
        for verdict in [
            "unattributed",
            "uncertain_attribution",
            "not_in_evidence",
            "not_a_field_value",
            "implausible_attribution",
        ] {
            assert_eq!(
                outcome_for(&key(verdict, None), Some(&label("inconsistent", "none"))),
                Outcome::MissedByAbstention,
                "{verdict}"
            );
            assert_eq!(
                outcome_for(&key(verdict, None), Some(&label("consistent", "none"))),
                Outcome::BenignAbstention,
                "{verdict}"
            );
        }
    }

    /// Agreement on the verdict is not agreement, if the two are talking about
    /// different fields.
    #[test]
    fn the_same_verdict_about_a_different_field_is_not_agreement() {
        assert_eq!(
            outcome_for(
                &key("matches", Some("daily_indicators.rsi14")),
                Some(&label("consistent", "markov.signed_signal"))
            ),
            Outcome::FieldDisagreement
        );
        // A labeller who did not record a field is taken at their word on the
        // verdict rather than being scored on something they did not answer.
        assert_eq!(
            outcome_for(
                &key("matches", Some("daily_indicators.rsi14")),
                Some(&label("consistent", ""))
            ),
            Outcome::Agreed
        );
    }

    /// An unlabelled case is not a pass, and `cannot_tell` is its own result.
    #[test]
    fn an_unlabelled_case_is_not_a_pass() {
        assert_eq!(
            outcome_for(&key("matches", None), None),
            Outcome::Unlabelled
        );
        assert_eq!(
            outcome_for(&key("matches", None), Some(&label("cannot_tell", ""))),
            Outcome::Unsettled
        );
        let report = score(&[key("matches", None)], &[]);
        assert_eq!(report["labelled"], 0);
        assert_eq!(report["totals"]["unlabelled"], 1);
    }

    #[test]
    fn a_flagged_claim_the_labeller_accepts_is_a_false_positive() {
        assert_eq!(
            outcome_for(
                &key("differs", Some("daily_indicators.rsi14")),
                Some(&label("consistent", "daily_indicators.rsi14"))
            ),
            Outcome::FalsePositive
        );
        assert_eq!(
            outcome_for(
                &key("differs", Some("daily_indicators.rsi14")),
                Some(&label("inconsistent", "daily_indicators.rsi14"))
            ),
            Outcome::AgreedOnError
        );
    }
}

/// Draws the sample and writes the instrument.
///
/// `#[ignore]`d: it needs a dump of every completed report with its prompt.
///
/// ```text
/// psql -tAc "select jsonb_agg(jsonb_build_object('id', id,
///   'report', report_json::jsonb, 'request', request_json::jsonb))
///   from (select id, report_json, request_json from decision_reports
///   where status='completed' and report_json is not null
///   and request_json is not null order by id) t" > all.json
/// JEV_PROMPTS_PATH=all.json cargo test regenerate_the_control_sample -- --ignored --nocapture
/// ```
#[cfg(test)]
mod sampling {
    use super::*;
    use sha2::Digest;
    use std::collections::BTreeMap;

    const INSTRUMENT_PATH: &str = "docs/jev-controls-v1.json";
    const KEY_PATH: &str = "docs/jev-controls-v1-key.json";
    const LABELS_PATH: &str = "docs/jev-controls-v1-labels.json";

    /// How many to draw from each stratum.
    ///
    /// Not proportional. `matches` is where a false negative hides and gets the
    /// largest share; the abstention classes are rare individually and would
    /// each draw a handful under proportional sampling, which is too few to say
    /// anything about the class. Every `differs` is included.
    const STRATA: &[(&str, usize)] = &[
        ("matches", 60),
        ("differs", usize::MAX),
        ("unattributed", 18),
        ("uncertain_attribution", 12),
        ("not_in_evidence", 8),
        ("not_a_field_value", 8),
        ("implausible_attribution", 8),
    ];

    struct Claim {
        report: i64,
        symbol: String,
        note: String,
        figure: String,
        offset: usize,
        evidence: JsonValue,
        verdict: &'static str,
        field: Option<&'static str>,
    }

    /// Reproducible ordering. A draw I could steer is a draw that measures me.
    fn draw_order(claim: &Claim) -> String {
        format!(
            "{:x}",
            sha2::Sha256::digest(
                format!(
                    "{SAMPLE_SEED}|{}|{}|{}|{}",
                    claim.report, claim.symbol, claim.figure, claim.offset
                )
                .as_bytes()
            )
        )
    }

    #[test]
    #[ignore]
    fn regenerate_the_control_sample() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let raw = std::fs::read_to_string(path).expect("prompts");
        let sources: Vec<JsonValue> = serde_json::from_str(&raw).expect("a JSON array");

        // Every distinct claim in the history, keyed so a report graded many
        // times contributes once.
        let mut claims: BTreeMap<String, Claim> = BTreeMap::new();
        for entry in &sources {
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
                let lowered = note.to_lowercase();
                for check in crate::jev_numeric::numeric_checks(&lowered, evidence) {
                    let Some(span) = crate::jev_numeric::figure_at(&lowered, check.offset) else {
                        continue;
                    };
                    let figure = lowered[span.start..span.end].to_string();
                    let symbol = candidate
                        .get("symbol")
                        .and_then(JsonValue::as_str)
                        .unwrap_or_default()
                        .to_string();
                    claims.insert(
                        format!("{report_id}|{symbol}|{figure}|{}", check.offset),
                        Claim {
                            report: report_id,
                            symbol,
                            // The note as written, not lowercased: a labeller
                            // reads prose, and the checker's lowercasing is its
                            // own business.
                            note: note.to_string(),
                            figure,
                            offset: check.offset,
                            evidence: evidence.clone(),
                            verdict: check.verdict.as_str(),
                            field: check.field,
                        },
                    );
                }
            }
        }
        println!("distinct claims: {}", claims.len());
        let mut population: BTreeMap<&str, usize> = BTreeMap::new();
        for claim in claims.values() {
            *population.entry(claim.verdict).or_default() += 1;
        }
        println!("population by verdict: {population:?}");

        let mut cases = Vec::new();
        let mut keys = Vec::new();
        for (stratum, want) in STRATA {
            let mut pool: Vec<&Claim> = claims
                .values()
                .filter(|claim| claim.verdict == *stratum)
                .collect();
            pool.sort_by_key(|claim| draw_order(claim));
            let drawn = pool.len().min(*want);
            for (nth, claim) in pool.into_iter().take(drawn).enumerate() {
                // Opaque. A id carrying the stratum would put the answer back
                // into the instrument.
                let id = format!("ctl-{}", &draw_order(claim)[..10]);
                cases.push(ControlCase {
                    id: id.clone(),
                    source_report: claim.report,
                    symbol: claim.symbol.clone(),
                    note: claim.note.clone(),
                    figure: claim.figure.clone(),
                    figure_offset: claim.offset,
                    evidence: claim.evidence.clone(),
                });
                keys.push(ControlKey {
                    id,
                    stratum: (*stratum).to_string(),
                    verdict: claim.verdict.to_string(),
                    field: claim.field.map(str::to_string),
                });
                let _ = nth;
            }
            println!("stratum {stratum:24} drawn {drawn}");
        }
        // Shuffled by the same hash, so the strata do not arrive in blocks a
        // labeller could read the pattern of.
        cases.sort_by_key(|case| case.id.clone());

        std::fs::write(
            INSTRUMENT_PATH,
            serde_json::to_string_pretty(&json!({
                "version": CONTROLS_VERSION,
                "seed": SAMPLE_SEED,
                "what_this_is": "Natural claims drawn from stored reports, with the evidence the \
                                 report was written from. The checker's answer is not here.",
                "how_to_label": {
                    "verdict": "`consistent` if the figure agrees with the evidence at the \
                                precision the note used, `inconsistent` if it does not, \
                                `cannot_tell` if two readings are both defensible. Ambiguity is \
                                a result.",
                    "field": "The dotted evidence field the figure names, `none` if it names no \
                              field, empty if you could not tell.",
                    "note": "Anything a reader of the result would need.",
                },
                "out_of_scope": "Assertions about portfolio capital, holdings, unrealised \
                                 profit and trading costs. Their supporting material is \
                                 deliberately not in the evidence.",
                "cases": cases,
            }))
            .expect("serialize"),
        )
        .expect("write instrument");
        std::fs::write(
            KEY_PATH,
            serde_json::to_string_pretty(&json!({
                "version": CONTROLS_VERSION,
                "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
                "warning": "Do not read this before labelling. It holds the checker's verdict \
                            and attributed field for every case in the instrument.",
                "keys": keys,
            }))
            .expect("serialize"),
        )
        .expect("write key");
        if !std::path::Path::new(LABELS_PATH).exists() {
            std::fs::write(
                LABELS_PATH,
                serde_json::to_string_pretty(&json!({
                    "version": CONTROLS_VERSION,
                    "labelled_by": "",
                    "labelled_at": "",
                    "labels": [],
                }))
                .expect("serialize"),
            )
            .expect("write labels");
        }
        println!("{} cases written to {INSTRUMENT_PATH}", cases.len());
    }

    fn load<T: serde::de::DeserializeOwned>(path: &str, field: &str) -> Vec<T> {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        serde_json::from_value(document[field].clone()).unwrap_or_default()
    }

    /// The instrument must not carry the answer, in any field, under any name.
    #[test]
    fn the_instrument_is_blind() {
        let raw = match std::fs::read_to_string(INSTRUMENT_PATH) {
            Ok(raw) => raw,
            Err(_) => return,
        };
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        let cases: Vec<ControlCase> =
            serde_json::from_value(document["cases"].clone()).expect("cases");
        assert!(cases.len() >= 100, "the sample shrank: {}", cases.len());

        // Serialized and searched as text, because a leak added later would be
        // a field nobody thought to assert on.
        let text = serde_json::to_string(&document["cases"]).expect("serialize");
        for leak in [
            "verdict",
            "matches",
            "differs",
            "unattributed",
            "uncertain_attribution",
            "not_in_evidence",
            "not_a_field_value",
            "implausible_attribution",
            "stratum",
        ] {
            assert!(!text.contains(leak), "the instrument leaks {leak:?}");
        }

        let keys: Vec<ControlKey> = load(KEY_PATH, "keys");
        assert_eq!(keys.len(), cases.len(), "key and instrument disagree");
        let ids: std::collections::BTreeSet<&str> =
            cases.iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids.len(), cases.len(), "duplicate case ids");
        for key in &keys {
            assert!(ids.contains(key.id.as_str()), "key {} has no case", key.id);
        }
    }

    /// Every case must be answerable: the figure has to be where the case says
    /// it is, and the evidence has to be there to answer it from.
    #[test]
    fn every_case_can_be_labelled() {
        let cases: Vec<ControlCase> = load(INSTRUMENT_PATH, "cases");
        for case in &cases {
            let lowered = case.note.to_lowercase();
            assert!(
                case.figure_offset + case.figure.len() <= lowered.len()
                    && lowered[case.figure_offset..case.figure_offset + case.figure.len()]
                        == case.figure,
                "{}: figure {:?} is not at offset {}",
                case.id,
                case.figure,
                case.figure_offset
            );
            assert!(
                case.evidence.get("daily_indicators").is_some(),
                "{}: no evidence to label against",
                case.id
            );
        }
    }

    /// The labels are not mine to write. This asserts they are still empty,
    /// and turns into a score the moment someone else fills them in.
    #[test]
    fn the_labels_are_not_filled_in_by_the_author() {
        let keys: Vec<ControlKey> = load(KEY_PATH, "keys");
        if keys.is_empty() {
            return;
        }
        let labels: Vec<ControlLabel> = load(LABELS_PATH, "labels");
        let report = score(&keys, &labels);
        let raw = std::fs::read_to_string(LABELS_PATH).unwrap_or_default();
        let document: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
        let by = document["labelled_by"].as_str().unwrap_or_default();
        assert!(
            !by.trim().is_empty() || labels.is_empty(),
            "labels arrived with nobody named as having made them: {report:#}"
        );
        println!("{}", serde_json::to_string_pretty(&report).expect("json"));
    }
}
