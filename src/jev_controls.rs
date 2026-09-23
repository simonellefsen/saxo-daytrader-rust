//! A blind labelling instrument for claims the checker did **not** flag.
//!
//! Everything measured so far reads in one direction. The seeded sets plant an
//! error and ask whether it is caught — detection of known errors, on examples
//! I constructed. The adjudication reads the nine claims the checker flagged
//! and asks whether it was right, which is precision. Neither can measure a
//! miss, because a claim the checker never flagged never reaches anyone's desk
//! — and 283 of the 1,445 distinct claims in the history, **19.6%**, were never
//! compared at all.
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
//! The nine already-adjudicated `differs` are included, unmarked. A
//! disagreement with them is a **reconciliation point, not a verdict on the
//! labeller**: those five confirmations are supported triage by the party that
//! wrote the checker, amended once already under review, and they are not gold
//! labels. Where the two differ, the evidence settles it, not seniority.
//!
//! **The frame is the checker's, and that bounds what this can measure.** Cases
//! reach it through `grading_inputs` and `numeric_checks`, so a candidate with
//! no indicator snapshot, a candidate past the question budget, and any figure
//! the scanner does not recognise cannot enter the sample. Across the history
//! that is **279 of 1,116 selected candidates excluded outright** — 271 for a
//! missing snapshot, 8 past the budget — and of the 837 that did reach the
//! checker, 229 yielded no extracted claim. This is an audit of extracted
//! claims from 608 notes. Extraction omissions need a whole-note audit, which
//! is a different instrument.
//!
//! **I do not fill in the labels.** I wrote the checker; a label from me is the
//! same circularity in a new file. A name in `labelled_by` records authorship
//! and establishes nothing about independence — that is the reader's judgement
//! to make, and no test here can make it for them.

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

/// How one labelled case scores on the **verdict** axis.
///
/// Attribution is a separate axis. Folding it in here let a verdict that agreed
/// about a different field read as agreement, which is the error this whole
/// exercise keeps finding in its own instruments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The checker compared and agreed; so did the labeller.
    Agreed,
    /// The checker compared and agreed; the labeller says the claim is wrong.
    /// **The quantity nothing built before this could reach.**
    FalseNegative,
    /// The checker flagged; the labeller says the claim holds.
    FalsePositive,
    /// Both call the claim wrong.
    AgreedOnError,
    /// The checker never compared, and the labeller says the claim is wrong —
    /// an error it could not have caught. A coverage failure, not a judgement
    /// failure, and counted apart from one. It still counts as a missed error
    /// end to end.
    MissedByAbstention,
    /// The checker never compared, and there was nothing to catch.
    BenignAbstention,
    /// The labeller could not settle it. Excluded from every rate, reported.
    Unsettled,
    /// No label for this case. **Not a pass.**
    Unlabelled,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Agreed => "agreed",
            Self::FalseNegative => "false_negative",
            Self::FalsePositive => "false_positive",
            Self::AgreedOnError => "agreed_on_error",
            Self::MissedByAbstention => "missed_by_abstention",
            Self::BenignAbstention => "benign_abstention",
            Self::Unsettled => "unsettled",
            Self::Unlabelled => "unlabelled",
        }
    }

    /// Whether the labeller read the claim as wrong, however the checker
    /// answered.
    fn labeller_says_wrong(self) -> bool {
        matches!(
            self,
            Self::FalseNegative | Self::AgreedOnError | Self::MissedByAbstention
        )
    }

    /// Whether this case contributes to a rate. `unsettled` and `unlabelled`
    /// do not.
    fn counts(self) -> bool {
        !matches!(self, Self::Unsettled | Self::Unlabelled)
    }
}

/// Whether the two identified the same field. Never folded into the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldAgreement {
    Same,
    Different,
    /// The labeller left it blank. **Not agreement** — an unvalidated
    /// attribution must not become a verified one by silence.
    Unknown,
    NotLabelled,
}

impl FieldAgreement {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Different => "different",
            Self::Unknown => "unknown",
            Self::NotLabelled => "not_labelled",
        }
    }
}

fn compared(verdict: &str) -> bool {
    verdict == "matches" || verdict == "differs"
}

/// Scores the verdict axis. Written before any label exists, so it cannot be
/// tuned to the labels it will meet.
pub(crate) fn outcome_for(key: &ControlKey, label: Option<&ControlLabel>) -> Outcome {
    let Some(label) = label else {
        return Outcome::Unlabelled;
    };
    match label.verdict.trim() {
        "cannot_tell" => Outcome::Unsettled,
        "inconsistent" if key.verdict == "matches" => Outcome::FalseNegative,
        "inconsistent" if key.verdict == "differs" => Outcome::AgreedOnError,
        "inconsistent" => Outcome::MissedByAbstention,
        "consistent" if key.verdict == "differs" => Outcome::FalsePositive,
        "consistent" if compared(&key.verdict) => Outcome::Agreed,
        "consistent" => Outcome::BenignAbstention,
        _ => Outcome::Unlabelled,
    }
}

/// Scores the attribution axis, independently of the verdict.
pub(crate) fn field_agreement(key: &ControlKey, label: Option<&ControlLabel>) -> FieldAgreement {
    let Some(label) = label else {
        return FieldAgreement::NotLabelled;
    };
    let labelled = label.field.trim();
    if labelled.is_empty() {
        return FieldAgreement::Unknown;
    }
    if labelled == key.field.as_deref().unwrap_or("none") {
        FieldAgreement::Same
    } else {
        FieldAgreement::Different
    }
}

/// The analysis plan, fixed here rather than chosen once the labels are in.
///
/// - A rate is computed over **labelled, settled** cases only. `cannot_tell`
///   and `unlabelled` are excluded from every numerator and denominator, and
///   reported beside the rate so their size is visible.
/// - Rates are **per stratum**. The strata are sampled at wildly different
///   rates — every `implausible_attribution` and six in a thousand `matches` —
///   so a total across all 121 cases describes the sample and nothing else.
/// - A population figure is a **weighted** estimate: each stratum's rate
///   carries its population share. It is an estimate from small strata, not a
///   measurement, and the counts behind it are printed so it can be checked.
/// - Wrong claims among sampled `matches` estimate **the error proportion
///   among claims the checker accepted** — not a conventional false-negative
///   rate, which would need a denominator of all errors.
/// - `missed_by_abstention` stays separately visible, and also counts toward
///   **errors the system did not flag, end to end**.
pub(crate) const PROTOCOL_VERSION: &str = "controls-protocol-v2-2026-09-23";

/// The confusion tables, both axes, by stratum and weighted.
pub(crate) fn score(
    keys: &[ControlKey],
    labels: &[ControlLabel],
    population: &std::collections::BTreeMap<String, i64>,
) -> JsonValue {
    use std::collections::{BTreeMap, BTreeSet};

    // Duplicates are rejected rather than silently overwritten, and a label
    // for a case that is not in the key is reported rather than dropped.
    let known: BTreeSet<&str> = keys.iter().map(|key| key.id.as_str()).collect();
    let mut by_id: BTreeMap<&str, &ControlLabel> = BTreeMap::new();
    let (mut duplicates, mut unknown) = (Vec::new(), Vec::new());
    for label in labels {
        if !known.contains(label.id.as_str()) {
            unknown.push(label.id.clone());
            continue;
        }
        if by_id.insert(label.id.as_str(), label).is_some() {
            duplicates.push(label.id.clone());
        }
    }

    let mut verdicts: BTreeMap<String, BTreeMap<&'static str, i64>> = BTreeMap::new();
    let mut fields: BTreeMap<String, BTreeMap<&'static str, i64>> = BTreeMap::new();
    let mut verdict_totals: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut field_totals: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut settled: BTreeMap<String, (i64, i64, i64)> = BTreeMap::new();
    for key in keys {
        let label = by_id.get(key.id.as_str()).copied();
        let outcome = outcome_for(key, label);
        let field = field_agreement(key, label);
        *verdicts
            .entry(key.stratum.clone())
            .or_default()
            .entry(outcome.as_str())
            .or_default() += 1;
        *fields
            .entry(key.stratum.clone())
            .or_default()
            .entry(field.as_str())
            .or_default() += 1;
        *verdict_totals.entry(outcome.as_str()).or_default() += 1;
        *field_totals.entry(field.as_str()).or_default() += 1;
        let row = settled.entry(key.stratum.clone()).or_insert((0, 0, 0));
        if outcome.counts() {
            row.0 += 1;
            row.1 += i64::from(outcome.labeller_says_wrong());
            row.2 += i64::from(matches!(
                outcome,
                Outcome::FalseNegative | Outcome::MissedByAbstention
            ));
        }
    }

    let mut rates = BTreeMap::new();
    let (mut weight, mut wrong_mass, mut unflagged_mass) = (0.0f64, 0.0f64, 0.0f64);
    for (stratum, (counted, wrong, unflagged)) in &settled {
        let share = population.get(stratum).copied().unwrap_or(0) as f64;
        let entry = if *counted == 0 {
            json!({
                "population": share,
                "settled": 0,
                "error_proportion": JsonValue::Null,
                "note": "nothing settled in this stratum, so it contributes no estimate",
            })
        } else {
            weight += share;
            wrong_mass += share * (*wrong as f64 / *counted as f64);
            unflagged_mass += share * (*unflagged as f64 / *counted as f64);
            json!({
                "population": share,
                "settled": counted,
                "labelled_wrong": wrong,
                "not_flagged_and_wrong": unflagged,
                "error_proportion": *wrong as f64 / *counted as f64,
            })
        };
        rates.insert(stratum.clone(), entry);
    }

    json!({
        "version": CONTROLS_VERSION,
        "protocol": PROTOCOL_VERSION,
        "cases": keys.len(),
        "labelled": verdict_totals
            .iter()
            .filter(|(outcome, _)| **outcome != "unlabelled")
            .map(|(_, count)| count)
            .sum::<i64>(),
        "label_problems": {
            "duplicate_ids": duplicates,
            "unknown_ids": unknown,
        },
        "verdict_axis": {"by_stratum": verdicts, "totals": verdict_totals},
        "attribution_axis": {"by_stratum": fields, "totals": field_totals},
        "per_stratum": rates,
        "weighted_estimates": if weight == 0.0 {
            JsonValue::Null
        } else {
            json!({
                "claims_the_labeller_reads_as_wrong": wrong_mass / weight,
                "wrong_and_not_flagged_end_to_end": unflagged_mass / weight,
                "population_covered": weight,
                "caveat": "an estimate from strata of six to sixty cases, not a measurement",
            })
        },
        "reading": "The two axes are separate on purpose. A verdict that agrees about a \
                    different field is not agreement, and a blank field is `unknown` rather \
                    than assumed to match. Totals across strata describe the sample, not the \
                    population, because the strata are sampled at different rates; the \
                    population figures are the weighted ones. `unsettled` and `unlabelled` \
                    enter no rate, and an unlabelled case is not a pass.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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

    /// The two axes are scored apart. A verdict that agrees about a different
    /// field used to read as plain agreement, and a blank field used to read
    /// as a match — an unvalidated attribution becoming a verified one by
    /// silence.
    #[test]
    fn attribution_is_scored_on_its_own_axis() {
        let compared = key("matches", Some("daily_indicators.rsi14"));
        let elsewhere = label("consistent", "markov.signed_signal");
        assert_eq!(outcome_for(&compared, Some(&elsewhere)), Outcome::Agreed);
        assert_eq!(
            field_agreement(&compared, Some(&elsewhere)),
            FieldAgreement::Different,
            "the verdict agreed; the attribution did not"
        );

        let blank = label("consistent", "");
        assert_eq!(
            field_agreement(&compared, Some(&blank)),
            FieldAgreement::Unknown,
            "a blank field is unknown, never a match"
        );
        assert_eq!(
            field_agreement(
                &compared,
                Some(&label("consistent", "daily_indicators.rsi14"))
            ),
            FieldAgreement::Same
        );

        // A flagged claim both call wrong is still only agreement on the
        // verdict until the fields are compared.
        let flagged = key("differs", Some("daily_indicators.rsi14"));
        let other_field = label("inconsistent", "daily_indicators.reward_risk");
        assert_eq!(
            outcome_for(&flagged, Some(&other_field)),
            Outcome::AgreedOnError
        );
        assert_eq!(
            field_agreement(&flagged, Some(&other_field)),
            FieldAgreement::Different
        );

        // Where the checker attributed nothing, "none" is agreement.
        let unattributed = key("unattributed", None);
        assert_eq!(
            field_agreement(&unattributed, Some(&label("consistent", "none"))),
            FieldAgreement::Same
        );
    }

    /// A duplicate label silently overwrote its predecessor, and a label for
    /// a case not in the key vanished. Both are reported now.
    #[test]
    fn duplicate_and_unknown_labels_are_rejected_not_absorbed() {
        let keys = vec![key("matches", None)];
        let mut first = label("consistent", "");
        first.id = "c".into();
        let mut second = label("inconsistent", "");
        second.id = "c".into();
        let mut stray = label("consistent", "");
        stray.id = "not-a-case".into();
        let report = score(&keys, &[first, second, stray], &BTreeMap::new());
        assert_eq!(report["label_problems"]["duplicate_ids"][0], "c");
        assert_eq!(report["label_problems"]["unknown_ids"][0], "not-a-case");
    }

    /// Rates are per stratum and weighted by population, because the strata
    /// are sampled at wildly different rates. A total across the sample
    /// describes the sample.
    #[test]
    fn rates_are_per_stratum_and_weighted() {
        let mut wrong = key("matches", None);
        wrong.id = "a".into();
        wrong.stratum = "matches".into();
        let mut right = key("unattributed", None);
        right.id = "b".into();
        right.stratum = "unattributed".into();

        let mut a = label("inconsistent", "");
        a.id = "a".into();
        let mut b = label("consistent", "");
        b.id = "b".into();

        let population = BTreeMap::from([
            ("matches".to_string(), 1000i64),
            ("unattributed".to_string(), 100i64),
        ]);
        let report = score(&[wrong, right], &[a, b], &population);
        assert_eq!(report["per_stratum"]["matches"]["error_proportion"], 1.0);
        assert_eq!(
            report["per_stratum"]["unattributed"]["error_proportion"],
            0.0
        );
        // 1000/1100 of the population sits in the stratum that came back wrong.
        let weighted = report["weighted_estimates"]["claims_the_labeller_reads_as_wrong"]
            .as_f64()
            .expect("weighted");
        assert!((weighted - 1000.0 / 1100.0).abs() < 1e-9, "{weighted}");
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
        let report = score(&[key("matches", None)], &[], &BTreeMap::new());
        assert_eq!(report["labelled"], 0);
        assert_eq!(report["verdict_axis"]["totals"]["unlabelled"], 1);
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
        // Carried into the key so the analysis can weight each stratum by its
        // share of the population rather than by how many were drawn from it.
        let mut population_by_stratum: BTreeMap<String, i64> = BTreeMap::new();
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
            population_by_stratum.insert(
                (*stratum).to_string(),
                population.get(stratum).copied().unwrap_or(0) as i64,
            );
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
                "warning": "Do not read this before labelling, and do not read the earlier \
                            adjudication either. It holds the checker's verdict and attributed \
                            field for every case in the instrument.",
                "protocol": PROTOCOL_VERSION,
                "population": population_by_stratum,
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
        let population = std::fs::read_to_string(KEY_PATH)
            .ok()
            .and_then(|raw| serde_json::from_str::<JsonValue>(&raw).ok())
            .and_then(|document| {
                serde_json::from_value::<std::collections::BTreeMap<String, i64>>(
                    document["population"].clone(),
                )
                .ok()
            })
            .unwrap_or_default();
        let report = score(&keys, &labels, &population);
        let raw = std::fs::read_to_string(LABELS_PATH).unwrap_or_default();
        let document: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
        let by = document["labelled_by"].as_str().unwrap_or_default();
        // A name records authorship. It does not establish independence, and
        // nothing in this file can.
        assert!(
            !by.trim().is_empty() || labels.is_empty(),
            "labels arrived with nobody named as having made them: {report:#}"
        );
        println!("{}", serde_json::to_string_pretty(&report).expect("json"));
    }
}

#[cfg(test)]
mod frame_audit {
    use super::*;
    use std::collections::BTreeMap;

    /// What the sampling frame leaves out, counted rather than asserted.
    #[test]
    #[ignore]
    fn measure_the_frame() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let raw = std::fs::read_to_string(path).expect("prompts");
        let sources: Vec<JsonValue> = serde_json::from_str(&raw).expect("a JSON array");
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in &sources {
            let (Some(report), Some(request)) = (entry.get("report"), entry.get("request")) else {
                continue;
            };
            let prompt = crate::xai_decision::decision_prompt_user_payload(request);
            let inputs = crate::jev_review::grading_inputs(report, &prompt);
            let selected = report
                .get("selected_assets")
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len);
            *counts.entry("selected_candidates").or_default() += selected;
            *counts.entry("reached_grading").or_default() += inputs.candidates.len();
            for reason in inputs
                .coverage
                .get("excluded")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
            {
                let key = match reason.get("reason").and_then(JsonValue::as_str) {
                    Some("no_indicator_snapshot_in_prompt") => "excluded_no_snapshot",
                    Some("beyond_question_budget") => "excluded_beyond_budget",
                    _ => "excluded_other",
                };
                *counts.entry(key).or_default() += 1;
            }
            for (index, candidate) in inputs.candidates.iter().enumerate() {
                let (Some(note), Some(evidence)) = (
                    candidate.get("note").and_then(JsonValue::as_str),
                    inputs.evidence.get(index),
                ) else {
                    continue;
                };
                let lowered = note.to_lowercase();
                *counts.entry("notes_with_a_claim").or_default() +=
                    usize::from(!crate::jev_numeric::numeric_checks(&lowered, evidence).is_empty());
                *counts.entry("notes_reaching_the_scanner").or_default() += 1;
            }
        }
        println!("frame {counts:?}");
    }
}
