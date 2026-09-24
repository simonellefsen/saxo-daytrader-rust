//! A blind whole-note audit: what the numeric checker never extracts.
//!
//! The controls could not see this. They show a labeller one figure the
//! scanner already found, so a claim the scanner never recognised -- a number
//! written in words, a figure in a form it does not read, a note that never
//! reached the checker at all -- cannot enter that sample and cannot be
//! labelled wrong. `docs/jev-controls.md` named the gap: extraction omissions
//! need a whole-note audit, a different instrument.
//!
//! This is that instrument. A labeller reads each **whole note** beside the
//! evidence the report was given, and lists every checkable claim in it: the
//! quote, whether it states a number, the field, and whether it agrees. The
//! key holds what the checker extracted from the same note, with exact spans.
//! Afterwards each labelled numeric claim is matched to the figures inside its
//! quote, which says whether the scanner found it, compared it, flagged it or
//! never saw it.
//!
//! The rules the controls earned apply here from the start:
//!
//! - **Drawn, not chosen**, by `sha256(seed + report + symbol)` within strata.
//! - **Blind.** The instrument carries no verdict, no span and no stratum; a
//!   test searches it for them.
//! - **The analysis is fixed before any label exists**, in
//!   `PROTOCOL_VERSION`: note-level outcomes, exact finite-population bounds
//!   per stratum, Bonferroni across the sampled strata, unresolved cases
//!   counted both ways, and nothing estimated while any note is unlabelled.
//! - **Malformed input is rejected, not absorbed**: a duplicate or unknown
//!   id, a quote that is not in its note or is in it twice, an undefined kind
//!   or verdict.
//! - **I do not fill in the labels.** I wrote the checker.
//!
//! It measures the numeric checker's extraction. Qualitative claims are
//! recorded and counted, but the wording grader judges whole notes, not spans,
//! so they are not matched to anything.

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const WHOLE_NOTES_VERSION: &str = "whole-notes-v1-2026-09-24";
/// Fixed, recorded, and used for nothing else.
pub(crate) const SAMPLE_SEED: &str = "jev-whole-notes-v1";

/// The analysis plan, fixed here before any label exists.
///
/// **The unit is the note.** Claims within a note are not independent, so the
/// population figures are note-level: the share of notes in which something
/// happened. Claim-level counts are reported as description, not estimated.
///
/// **Three note-level outcomes**, each yes, no or unresolved:
///
/// - `omission`: the note holds a numeric claim no scanned figure lies inside.
///   This does not depend on the verdict, so it is never unresolved.
/// - `unflagged_numeric_error`: the note holds a numeric claim the labeller
///   reads as `inconsistent` that the checker did not flag. Unresolved when
///   there is none, but a `cannot_tell` numeric claim went unflagged.
/// - `qualitative_error`: the note holds a qualitative claim read as
///   `inconsistent`. Unresolved likewise on `cannot_tell`. Descriptive of the
///   notes, not of any checker, because nothing here matches qualitative
///   claims to the wording grader.
///
/// **The population figure is an interval.** Within each stratum, every
/// unresolved note is counted once as no and once as yes. A sampled stratum is
/// widened by an exact hypergeometric bound at `1 - 0.05/S` for `S` sampled
/// strata; a stratum taken whole is exact. Strata are weighted by their note
/// counts, and every stratum stays in the denominator. The guarantee assumes
/// the hash-ordered draw behaves as a simple random sample within each stratum
/// and that the labels are right. It covers sampling error only.
///
/// **Nothing is estimated while any note is unlabelled**, and a note with no
/// claims is labelled only when its status says it was read.
pub(crate) const PROTOCOL_VERSION: &str = "whole-notes-protocol-v1-2026-09-24";

pub(crate) const STRATUM_WITH_FIGURES: &str = "reached_with_figures";
pub(crate) const STRATUM_WITHOUT_FIGURES: &str = "reached_without_figures";
pub(crate) const STRATUM_NEVER_REACHED: &str = "never_reached";

const CLAIM_KINDS: [&str; 2] = ["numeric", "qualitative"];
const LABEL_VERDICTS: [&str; 3] = ["consistent", "inconsistent", "cannot_tell"];
const OUTCOMES: [&str; 3] = ["omission", "unflagged_numeric_error", "qualitative_error"];

/// One note, as a labeller sees it: no spans, no verdicts, no stratum.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NoteCase {
    pub id: String,
    pub source_report: i64,
    pub symbol: String,
    pub note: String,
    /// Everything the report was given about this symbol.
    pub evidence: JsonValue,
}

/// A figure the scanner found, and what the checker made of it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Extracted {
    /// Byte span in the lowercased note, where the scanner works.
    pub start: usize,
    pub end: usize,
    pub figure: String,
    pub field: Option<String>,
    pub verdict: String,
}

/// The checker's side, kept out of the instrument.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NoteKey {
    pub id: String,
    pub stratum: String,
    pub extracted: Vec<Extracted>,
}

/// One claim a labeller found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ClaimLabel {
    /// The exact words, as short as makes the claim; must occur once in the note.
    pub quote: String,
    /// `numeric` if the claim states a number, in digits or words.
    pub kind: String,
    pub field: String,
    pub verdict: String,
    #[serde(default)]
    pub note: String,
}

/// What a labeller records for one note.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct NoteLabel {
    pub id: String,
    /// `labelled` once the whole note has been read, even if it holds nothing.
    pub status: String,
    #[serde(default)]
    pub claims: Vec<ClaimLabel>,
}

/// What the checker did with one labelled numeric claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handling {
    /// No figure the scanner found lies inside the quote.
    Omitted,
    /// Found, compared, and at least one figure disagreed.
    Flagged,
    /// Found and compared, and every figure agreed.
    Accepted,
    /// Found, but not every figure was compared.
    Abstained,
}

impl Handling {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Omitted => "omitted",
            Self::Flagged => "flagged",
            Self::Accepted => "accepted",
            Self::Abstained => "abstained",
        }
    }
}

/// The figures inside a claim's span decide its handling. A flag on any of
/// them flags the claim: the system raised something about it.
pub(crate) fn handling(span: (usize, usize), extracted: &[Extracted]) -> Handling {
    let inside: Vec<&Extracted> = extracted
        .iter()
        .filter(|figure| span.0 <= figure.start && figure.end <= span.1)
        .collect();
    if inside.is_empty() {
        Handling::Omitted
    } else if inside.iter().any(|figure| figure.verdict == "differs") {
        Handling::Flagged
    } else if inside.iter().all(|figure| figure.verdict == "matches") {
        Handling::Accepted
    } else {
        Handling::Abstained
    }
}

/// Where a quote sits in the lowercased note, if it occurs exactly once.
pub(crate) fn quote_span(note: &str, quote: &str) -> Result<(usize, usize), &'static str> {
    let (note, quote) = (note.to_lowercase(), quote.trim().to_lowercase());
    if quote.is_empty() {
        return Err("empty quote");
    }
    let mut found = note.match_indices(quote.as_str());
    let Some((start, _)) = found.next() else {
        return Err("quote is not in the note");
    };
    if found.next().is_some() {
        return Err("quote occurs more than once; lengthen it");
    }
    Ok((start, start + quote.len()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binary {
    Yes,
    No,
    Unresolved,
}

/// The three note-level outcomes for one labelled note.
pub(crate) fn note_outcomes(
    case: &NoteCase,
    key: &NoteKey,
    label: &NoteLabel,
) -> BTreeMap<&'static str, Binary> {
    let mut omission = Binary::No;
    let (mut numeric_wrong, mut numeric_unsure) = (false, false);
    let (mut qualitative_wrong, mut qualitative_unsure) = (false, false);
    for claim in &label.claims {
        let verdict = claim.verdict.trim();
        if claim.kind.trim() == "numeric" {
            let Ok(span) = quote_span(&case.note, &claim.quote) else {
                continue;
            };
            let handled = handling(span, &key.extracted);
            if handled == Handling::Omitted {
                omission = Binary::Yes;
            }
            if handled != Handling::Flagged {
                numeric_wrong |= verdict == "inconsistent";
                numeric_unsure |= verdict == "cannot_tell";
            }
        } else {
            qualitative_wrong |= verdict == "inconsistent";
            qualitative_unsure |= verdict == "cannot_tell";
        }
    }
    let resolve = |wrong: bool, unsure: bool| match (wrong, unsure) {
        (true, _) => Binary::Yes,
        (false, true) => Binary::Unresolved,
        (false, false) => Binary::No,
    };
    BTreeMap::from([
        ("omission", omission),
        (
            "unflagged_numeric_error",
            resolve(numeric_wrong, numeric_unsure),
        ),
        (
            "qualitative_error",
            resolve(qualitative_wrong, qualitative_unsure),
        ),
    ])
}

/// Everything wrong with a labels file, as sets so the report does not depend
/// on the order of the file.
fn label_problems(
    cases: &[NoteCase],
    labels: &[NoteLabel],
) -> BTreeMap<&'static str, BTreeSet<String>> {
    let by_id: BTreeMap<&str, &NoteCase> =
        cases.iter().map(|case| (case.id.as_str(), case)).collect();
    let mut problems: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for label in labels {
        let id = label.id.as_str();
        let Some(case) = by_id.get(id) else {
            problems
                .entry("unknown_ids")
                .or_default()
                .insert(id.to_string());
            continue;
        };
        if !seen.insert(id) {
            problems
                .entry("duplicate_ids")
                .or_default()
                .insert(id.to_string());
        }
        let status = label.status.trim();
        if !status.is_empty() && status != "labelled" {
            problems
                .entry("unreadable_status")
                .or_default()
                .insert(id.to_string());
        }
        if status.is_empty() && !label.claims.is_empty() {
            problems
                .entry("claims_on_an_unlabelled_note")
                .or_default()
                .insert(id.to_string());
        }
        for (index, claim) in label.claims.iter().enumerate() {
            let at = format!("{id}#{index}");
            if !CLAIM_KINDS.contains(&claim.kind.trim()) {
                problems
                    .entry("unreadable_kind")
                    .or_default()
                    .insert(at.clone());
            }
            if !LABEL_VERDICTS.contains(&claim.verdict.trim()) {
                problems
                    .entry("unreadable_verdict")
                    .or_default()
                    .insert(at.clone());
            }
            if let Err(reason) = quote_span(&case.note, &claim.quote) {
                problems
                    .entry("unusable_quote")
                    .or_default()
                    .insert(format!("{at}: {reason}"));
            }
        }
    }
    problems
}

/// Scores a labels file under `PROTOCOL_VERSION`.
pub(crate) fn score(
    cases: &[NoteCase],
    keys: &[NoteKey],
    labels: &[NoteLabel],
    population: &BTreeMap<String, i64>,
) -> JsonValue {
    let problems = label_problems(cases, labels);
    if !problems.is_empty() {
        return json!({
            "version": WHOLE_NOTES_VERSION,
            "protocol": PROTOCOL_VERSION,
            "status": "rejected",
            "label_problems": problems,
            "reading": "Rejected, and nothing is scored until the input is corrected.",
        });
    }
    let labels_by_id: BTreeMap<&str, &NoteLabel> = labels
        .iter()
        .map(|label| (label.id.as_str(), label))
        .collect();
    let keys_by_id: BTreeMap<&str, &NoteKey> =
        keys.iter().map(|key| (key.id.as_str(), key)).collect();

    // stratum -> outcome -> (yes, no, unresolved); and per-stratum sample sizes.
    let mut tallies: BTreeMap<String, BTreeMap<&'static str, (i64, i64, i64)>> = BTreeMap::new();
    let mut sampled: BTreeMap<String, i64> = BTreeMap::new();
    let mut unlabelled: BTreeMap<String, i64> = BTreeMap::new();
    let mut claims: BTreeMap<String, i64> = BTreeMap::new();
    let mut unclaimed_figures: BTreeMap<String, i64> = BTreeMap::new();
    for stratum in population.keys() {
        tallies.entry(stratum.clone()).or_default();
    }
    for case in cases {
        let Some(key) = keys_by_id.get(case.id.as_str()) else {
            continue;
        };
        *sampled.entry(key.stratum.clone()).or_default() += 1;
        let label = labels_by_id
            .get(case.id.as_str())
            .filter(|label| label.status.trim() == "labelled");
        let Some(label) = label else {
            *unlabelled.entry(key.stratum.clone()).or_default() += 1;
            continue;
        };
        for (outcome, value) in note_outcomes(case, key, label) {
            let entry = tallies
                .entry(key.stratum.clone())
                .or_default()
                .entry(outcome)
                .or_default();
            match value {
                Binary::Yes => entry.0 += 1,
                Binary::No => entry.1 += 1,
                Binary::Unresolved => entry.2 += 1,
            }
        }
        let mut claimed_spans = Vec::new();
        for claim in &label.claims {
            let kind = claim.kind.trim();
            let verdict = claim.verdict.trim();
            let bucket = if kind == "numeric" {
                let span = quote_span(&case.note, &claim.quote).expect("validated");
                claimed_spans.push(span);
                format!(
                    "numeric|{}|{verdict}",
                    handling(span, &key.extracted).as_str()
                )
            } else {
                format!("qualitative|{verdict}")
            };
            *claims.entry(bucket).or_default() += 1;
        }
        for figure in &key.extracted {
            let inside = claimed_spans
                .iter()
                .any(|span| span.0 <= figure.start && figure.end <= span.1);
            if !inside {
                *unclaimed_figures.entry(figure.verdict.clone()).or_default() += 1;
            }
        }
    }

    let size = |stratum: &str| population.get(stratum).copied().unwrap_or(0);
    let is_sampled = |stratum: &str| {
        let n = sampled.get(stratum).copied().unwrap_or(0);
        n > 0 && n < size(stratum)
    };
    let sampled_strata = population
        .keys()
        .filter(|stratum| is_sampled(stratum))
        .count()
        .max(1);
    let tail = (1.0 - crate::jev_controls::JOINT_LEVEL) / (2.0 * sampled_strata as f64);
    let bounds = |stratum: &str, yes: i64, unresolved: i64| -> (i64, i64) {
        let n = sampled.get(stratum).copied().unwrap_or(0) as usize;
        let draw = crate::jev_controls::Hypergeometric::new(
            size(stratum).max(0) as usize,
            n.min(size(stratum).max(0) as usize),
        );
        (
            draw.lower(yes as usize, tail) as i64,
            draw.upper((yes + unresolved) as usize, tail) as i64,
        )
    };
    let incomplete: i64 = unlabelled.values().sum();
    let total: i64 = population.values().sum();

    let mut per_stratum = BTreeMap::new();
    for stratum in population.keys() {
        let mut outcomes = BTreeMap::new();
        for outcome in OUTCOMES {
            let (yes, no, unresolved) = tallies
                .get(stratum)
                .and_then(|tally| tally.get(outcome))
                .copied()
                .unwrap_or_default();
            let settled = yes + no;
            outcomes.insert(
                outcome,
                json!({
                    "yes": yes,
                    "no": no,
                    "unresolved": unresolved,
                    "among_settled_only": if settled == 0 { JsonValue::Null } else { json!(yes as f64 / settled as f64) },
                    "notes_in_population": if incomplete > 0 { JsonValue::Null } else {
                        let (from, to) = bounds(stratum, yes, unresolved);
                        json!([from, to])
                    },
                }),
            );
        }
        per_stratum.insert(
            stratum.clone(),
            json!({
                "population": size(stratum),
                "sampled": sampled.get(stratum).copied().unwrap_or(0),
                "unlabelled": unlabelled.get(stratum).copied().unwrap_or(0),
                "outcomes": outcomes,
            }),
        );
    }
    let population_estimate = if incomplete > 0 || total == 0 {
        JsonValue::Null
    } else {
        let mut estimate = BTreeMap::new();
        for outcome in OUTCOMES {
            let (mut from, mut to) = (0i64, 0i64);
            for stratum in population.keys() {
                let (yes, _, unresolved) = tallies
                    .get(stratum)
                    .and_then(|tally| tally.get(outcome))
                    .copied()
                    .unwrap_or_default();
                let (low, high) = bounds(stratum, yes, unresolved);
                from += low;
                to += high;
            }
            estimate.insert(
                outcome,
                json!({
                    "notes": [from, to],
                    "share": [from as f64 / total as f64, to as f64 / total as f64],
                }),
            );
        }
        json!({
            "population": total,
            "joint_level": crate::jev_controls::JOINT_LEVEL,
            "sampled_strata": sampled_strata,
            "tail_per_stratum": tail,
            "outcomes": estimate,
        })
    };

    json!({
        "version": WHOLE_NOTES_VERSION,
        "protocol": PROTOCOL_VERSION,
        "status": if incomplete > 0 { "labelling_incomplete" } else { "complete" },
        "notes": cases.len(),
        "labelled": cases.len() as i64 - incomplete,
        "per_stratum": per_stratum,
        "population_estimate": population_estimate,
        "claims_described": claims,
        "scanned_figures_no_claim_covers": unclaimed_figures,
        "reading": "Note-level outcomes, bounded exactly per stratum and summed over strata \
                    weighted by their note counts; unresolved notes count both ways, and \
                    nothing is estimated while a note is unlabelled. Claim counts describe the \
                    sample. `scanned_figures_no_claim_covers` are figures the scanner found that \
                    no labelled claim contains: out-of-scope quantities, or claims the labeller \
                    did not see.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, note: &str) -> NoteCase {
        NoteCase {
            id: id.into(),
            source_report: 1,
            symbol: "AMD:xnas".into(),
            note: note.into(),
            evidence: JsonValue::Null,
        }
    }

    fn key(id: &str, stratum: &str, extracted: Vec<Extracted>) -> NoteKey {
        NoteKey {
            id: id.into(),
            stratum: stratum.into(),
            extracted,
        }
    }

    fn figure(start: usize, end: usize, verdict: &str) -> Extracted {
        Extracted {
            start,
            end,
            figure: String::new(),
            field: None,
            verdict: verdict.into(),
        }
    }

    fn claim(quote: &str, kind: &str, verdict: &str) -> ClaimLabel {
        ClaimLabel {
            quote: quote.into(),
            kind: kind.into(),
            field: "daily_indicators.rsi14".into(),
            verdict: verdict.into(),
            note: String::new(),
        }
    }

    fn labelled(id: &str, claims: Vec<ClaimLabel>) -> NoteLabel {
        NoteLabel {
            id: id.into(),
            status: "labelled".into(),
            claims,
        }
    }

    /// "RSI 58" with the 58 at bytes 4..6.
    #[test]
    fn a_claim_is_handled_by_the_figures_inside_its_quote() {
        let span = quote_span("RSI 58, bullish", "RSI 58").expect("span");
        assert_eq!(span, (0, 6));
        assert_eq!(
            handling(span, &[figure(4, 6, "matches")]),
            Handling::Accepted
        );
        assert_eq!(
            handling(span, &[figure(4, 6, "differs")]),
            Handling::Flagged
        );
        assert_eq!(
            handling(span, &[figure(4, 6, "unattributed")]),
            Handling::Abstained
        );
        assert_eq!(
            handling(span, &[figure(8, 10, "matches")]),
            Handling::Omitted
        );
        // "5/3": one differing figure flags the claim, whatever the other did.
        assert_eq!(
            handling((0, 3), &[figure(0, 1, "matches"), figure(2, 3, "differs")]),
            Handling::Flagged
        );
    }

    /// A number written in words is never scanned, so it is always omitted.
    #[test]
    fn a_number_in_words_is_an_omission() {
        let note = "five confluences and RSI 58";
        let outcomes = note_outcomes(
            &case("a", note),
            &key("a", STRATUM_WITH_FIGURES, vec![figure(25, 27, "matches")]),
            &labelled(
                "a",
                vec![
                    claim("five confluences", "numeric", "consistent"),
                    claim("RSI 58", "numeric", "consistent"),
                ],
            ),
        );
        assert_eq!(outcomes["omission"], Binary::Yes);
        assert_eq!(outcomes["unflagged_numeric_error"], Binary::No);
    }

    /// A wrong claim the checker flagged is not an unflagged error; one it
    /// accepted, abstained on or never saw is.
    #[test]
    fn an_error_counts_unless_the_checker_flagged_it() {
        let note = "RSI 58 and signal 0.4";
        let wrong = |verdict: &str| {
            note_outcomes(
                &case("a", note),
                &key("a", STRATUM_WITH_FIGURES, vec![figure(4, 6, verdict)]),
                &labelled("a", vec![claim("RSI 58", "numeric", "inconsistent")]),
            )["unflagged_numeric_error"]
        };
        assert_eq!(wrong("differs"), Binary::No);
        assert_eq!(wrong("matches"), Binary::Yes);
        assert_eq!(wrong("unattributed"), Binary::Yes);
        let unsure = note_outcomes(
            &case("a", note),
            &key("a", STRATUM_NEVER_REACHED, vec![]),
            &labelled("a", vec![claim("signal 0.4", "numeric", "cannot_tell")]),
        );
        assert_eq!(unsure["unflagged_numeric_error"], Binary::Unresolved);
        assert_eq!(
            unsure["omission"],
            Binary::Yes,
            "never reached: every numeric claim is omitted"
        );
    }

    /// A quote must identify one place in the note, and every field must be
    /// one the protocol defines. The file is rejected, in any order.
    #[test]
    fn malformed_labels_reject_the_input_whatever_their_order() {
        let cases = vec![case("a", "RSI 58, RSI 58 again")];
        let keys = vec![key("a", STRATUM_WITH_FIGURES, vec![])];
        let population = BTreeMap::from([(STRATUM_WITH_FIGURES.to_string(), 10)]);
        let ambiguous = labelled("a", vec![claim("RSI 58", "numeric", "consistent")]);
        let report = score(&cases, &keys, std::slice::from_ref(&ambiguous), &population);
        assert_eq!(report["status"], "rejected");
        assert!(
            report["label_problems"]["unusable_quote"][0]
                .as_str()
                .is_some_and(|reason| reason.contains("more than once"))
        );

        let first = labelled("a", vec![claim("RSI 58 again", "numeric", "consistent")]);
        let second = labelled("a", vec![claim("RSI 58 again", "numbers", "maybe")]);
        let stray = labelled("zzz", vec![]);
        let forward = score(
            &cases,
            &keys,
            &[first.clone(), second.clone(), stray.clone()],
            &population,
        );
        let reversed = score(&cases, &keys, &[stray, second, first], &population);
        assert_eq!(forward, reversed);
        assert_eq!(forward["label_problems"]["duplicate_ids"][0], "a");
        assert_eq!(forward["label_problems"]["unknown_ids"][0], "zzz");
        assert!(forward["label_problems"]["unreadable_kind"].is_array());
        assert!(forward["label_problems"]["unreadable_verdict"].is_array());
    }

    /// A note read and found empty is labelled; a note not read is not, and
    /// withholds the estimate.
    #[test]
    fn an_empty_note_is_labelled_only_when_it_says_so() {
        let cases = vec![case("a", "bullish trend"), case("b", "bearish trend")];
        let keys = vec![
            key("a", STRATUM_WITHOUT_FIGURES, vec![]),
            key("b", STRATUM_WITHOUT_FIGURES, vec![]),
        ];
        let population = BTreeMap::from([(STRATUM_WITHOUT_FIGURES.to_string(), 2)]);
        let read = labelled("a", vec![]);
        let unread = NoteLabel {
            id: "b".into(),
            ..NoteLabel::default()
        };
        let partial = score(&cases, &keys, &[read.clone(), unread], &population);
        assert_eq!(partial["status"], "labelling_incomplete");
        assert_eq!(partial["population_estimate"], JsonValue::Null);
        let whole = score(&cases, &keys, &[read, labelled("b", vec![])], &population);
        assert_eq!(whole["status"], "complete");
        assert_eq!(
            whole["population_estimate"]["outcomes"]["omission"]["notes"],
            json!([0, 0]),
            "both notes taken whole and both empty"
        );
    }

    /// Unresolved notes count both ways; every stratum stays in.
    #[test]
    fn unresolved_notes_widen_the_interval_rather_than_leaving_it() {
        let cases = vec![case("a", "signal 0.4"), case("b", "signal 0.5")];
        let keys = vec![
            key("a", STRATUM_NEVER_REACHED, vec![]),
            key("b", STRATUM_WITH_FIGURES, vec![figure(7, 10, "matches")]),
        ];
        let population = BTreeMap::from([
            (STRATUM_NEVER_REACHED.to_string(), 1),
            (STRATUM_WITH_FIGURES.to_string(), 1),
        ]);
        let report = score(
            &cases,
            &keys,
            &[
                labelled("a", vec![claim("signal 0.4", "numeric", "cannot_tell")]),
                labelled("b", vec![claim("signal 0.5", "numeric", "consistent")]),
            ],
            &population,
        );
        assert_eq!(
            report["population_estimate"]["outcomes"]["unflagged_numeric_error"]["notes"],
            json!([0, 1])
        );
        assert_eq!(report["population_estimate"]["population"], 2);
    }
}

/// Draws the sample and writes the instrument, the key and an empty labels
/// template. `#[ignore]`d: it needs the report dump the controls were drawn
/// from.
///
/// ```text
/// JEV_PROMPTS_PATH=all.json cargo test regenerate_the_whole_note_sample -- --ignored --nocapture
/// ```
#[cfg(test)]
mod sampling {
    use super::*;
    use sha2::Digest;

    const INSTRUMENT_PATH: &str = "docs/jev-whole-notes-v1.json";
    const KEY_PATH: &str = "docs/jev-whole-notes-v1-key.json";
    const LABELS_PATH: &str = "docs/jev-whole-notes-v1-labels.json";
    const RECONCILIATION_PATH: &str = "docs/jev-whole-notes-v1-reconciliation.json";

    /// How many notes to draw from each stratum. Weighted toward the notes
    /// the checker read, where an omission is a failure of extraction rather
    /// than of coverage; the other two are sampled enough to bound them.
    const STRATA: &[(&str, usize)] = &[
        (STRATUM_WITH_FIGURES, 40),
        (STRATUM_WITHOUT_FIGURES, 15),
        (STRATUM_NEVER_REACHED, 20),
    ];

    fn draw_key(report: i64, symbol: &str) -> String {
        format!(
            "{:x}",
            sha2::Sha256::digest(format!("{SAMPLE_SEED}|{report}|{symbol}").as_bytes())
        )
    }

    fn by_symbol<'a>(prompt: &'a JsonValue, block: &str, symbol: &str) -> Option<&'a JsonValue> {
        prompt
            .get(block)?
            .get("signals")?
            .as_array()?
            .iter()
            .find(|signal| signal.get("symbol").and_then(JsonValue::as_str) == Some(symbol))
    }

    /// The same evidence `grading_inputs` builds, for a symbol it never judged.
    fn evidence_for(prompt: &JsonValue, symbol: &str) -> JsonValue {
        let markov = by_symbol(prompt, "markov_method", symbol)
            .cloned()
            .or_else(|| {
                crate::markov_method::embedded_prompt_signal_rows(prompt)
                    .get(symbol)
                    .cloned()
            });
        json!({
            "symbol": symbol,
            "daily_indicators": by_symbol(prompt, "daily_indicators", symbol),
            "markov": markov,
            "quiver": by_symbol(prompt, "quiver_signals", symbol).map(|signal| json!({
                "signal": signal.get("signal"),
                "direction": signal.get("direction"),
                "confidence": signal.get("confidence"),
                "run_date": signal.get("run_date"),
            })),
        })
    }

    fn document(path: &str) -> Option<JsonValue> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// The instrument, the key, the population and the key document.
    type Frozen = (
        Vec<NoteCase>,
        Vec<NoteKey>,
        BTreeMap<String, i64>,
        JsonValue,
    );

    fn frozen() -> Option<Frozen> {
        let instrument = document(INSTRUMENT_PATH)?;
        let key = document(KEY_PATH)?;
        let cases: Vec<NoteCase> = serde_json::from_value(instrument["cases"].clone()).ok()?;
        let keys: Vec<NoteKey> = serde_json::from_value(key["keys"].clone()).ok()?;
        let population: BTreeMap<String, i64> =
            serde_json::from_value(key["population"].clone()).ok()?;
        Some((cases, keys, population, key))
    }

    /// Nothing outside the note text may say what the checker did: no
    /// verdict, no span, no stratum. The notes are the model's prose and are
    /// searched separately only for what they cannot contain.
    #[test]
    fn the_instrument_is_blind() {
        let Some(instrument) = document(INSTRUMENT_PATH) else {
            return;
        };
        let mut stripped = instrument["cases"].clone();
        for case in stripped.as_array_mut().into_iter().flatten() {
            case["note"] = JsonValue::Null;
        }
        let text = serde_json::to_string(&stripped).expect("serialize");
        for leak in [
            "matches",
            "differs",
            "not_in_evidence",
            "unattributed",
            "uncertain_attribution",
            "implausible_attribution",
            "not_a_field_value",
            "stratum",
            "extracted",
            "reached",
            "\"start\"",
            "\"end\"",
        ] {
            assert!(!text.contains(leak), "the instrument leaks {leak:?}");
        }
    }

    /// Every note has a key, every key a note, and every extracted span
    /// points at its figure.
    #[test]
    fn the_key_describes_the_instrument() {
        let Some((cases, keys, population, key)) = frozen() else {
            return;
        };
        assert_eq!(key["protocol"], PROTOCOL_VERSION);
        assert_eq!(cases.len(), keys.len());
        let notes: BTreeMap<&str, &NoteCase> =
            cases.iter().map(|case| (case.id.as_str(), case)).collect();
        for entry in &keys {
            let case = notes.get(entry.id.as_str()).expect("a key without a note");
            let lowered = case.note.to_lowercase();
            for figure in &entry.extracted {
                assert_eq!(
                    lowered[figure.start..figure.end],
                    figure.figure,
                    "{}",
                    entry.id
                );
            }
            assert!(population.contains_key(&entry.stratum), "{}", entry.stratum);
        }
        assert_eq!(
            population.values().sum::<i64>(),
            1116,
            "the frame the controls counted"
        );
    }

    /// The labels are not mine to write. This asserts they are unfilled, and
    /// becomes a score the moment someone else fills them in.
    #[test]
    fn the_labels_are_not_filled_in_by_the_author() {
        let (Some((cases, keys, population, _)), Some(labels_document)) =
            (frozen(), document(LABELS_PATH))
        else {
            return;
        };
        let labels: Vec<NoteLabel> =
            serde_json::from_value(labels_document["labels"].clone()).unwrap_or_default();
        let filled = labels
            .iter()
            .any(|label| !label.status.trim().is_empty() || !label.claims.is_empty());
        let by = labels_document["labelled_by"].as_str().unwrap_or_default();
        let report = score(&cases, &keys, &labels, &population);
        assert!(
            !filled || !by.trim().is_empty(),
            "labels arrived with nobody named as having made them"
        );
        assert_ne!(report["status"], "rejected", "{report:#}");
        println!("{}", serde_json::to_string_pretty(&report).expect("json"));
    }

    /// The figures a reconciliation reports, small enough to write into the
    /// file and check against the scorer.
    fn headline(report: &JsonValue) -> JsonValue {
        let mut strata = serde_json::Map::new();
        for (stratum, detail) in report["per_stratum"].as_object().into_iter().flatten() {
            let mut outcomes = serde_json::Map::new();
            for outcome in OUTCOMES {
                let row = &detail["outcomes"][outcome];
                outcomes.insert(
                    outcome.to_string(),
                    json!([row["yes"], row["no"], row["unresolved"]]),
                );
            }
            strata.insert(stratum.clone(), JsonValue::Object(outcomes));
        }
        let mut population = serde_json::Map::new();
        for outcome in OUTCOMES {
            population.insert(
                outcome.to_string(),
                report["population_estimate"]["outcomes"][outcome]["notes"].clone(),
            );
        }
        json!({
            "yes_no_unresolved_by_stratum": strata,
            "notes_in_population": population,
            "claims_described": report["claims_described"],
        })
    }

    /// The labels with some readings put beside them -- on a copy, never on
    /// the delivered file. A reading names a claim as `note-id#index`.
    fn with_readings(labels: &[NoteLabel], readings: &[JsonValue]) -> Vec<NoteLabel> {
        let mut copy = labels.to_vec();
        for reading in readings {
            let reference = reading["claim"].as_str().expect("claim");
            let (id, index) = reference.split_once('#').expect("note-id#index");
            let index: usize = index.parse().expect("index");
            let claim = copy
                .iter_mut()
                .find(|label| label.id == id)
                .and_then(|label| label.claims.get_mut(index))
                .unwrap_or_else(|| panic!("a reading for {reference}, which has no claim"));
            claim.verdict = reading["verdict"].as_str().expect("verdict").into();
            claim.field = reading["field"].as_str().expect("field").into();
        }
        copy
    }

    /// The reconciliation sits beside the labels, not over them. It must leave
    /// the delivered file byte for byte as it was, quote the labels and the
    /// key faithfully, account for every `cannot_tell` exactly once, and
    /// report the figures the frozen scorer actually gives.
    #[test]
    fn the_reconciliation_is_beside_the_labels_not_over_them() {
        use sha2::Digest;
        let (Some(reconciliation), Some((cases, keys, population, _)), Some(labels_document)) = (
            document(RECONCILIATION_PATH),
            frozen(),
            document(LABELS_PATH),
        ) else {
            return;
        };
        let delivered = std::fs::read(LABELS_PATH).expect("labels");
        assert_eq!(
            reconciliation["reconciles"]["labels_sha256"],
            format!("{:x}", sha2::Sha256::digest(&delivered)),
            "the delivered labels were edited"
        );
        assert_eq!(reconciliation["protocol"], PROTOCOL_VERSION);
        let labels: Vec<NoteLabel> =
            serde_json::from_value(labels_document["labels"].clone()).expect("labels");
        let notes: BTreeMap<&str, &NoteCase> =
            cases.iter().map(|case| (case.id.as_str(), case)).collect();
        let key_by_id: BTreeMap<&str, &NoteKey> =
            keys.iter().map(|key| (key.id.as_str(), key)).collect();
        let claim_at = |reference: &str| -> (&NoteCase, &NoteKey, &ClaimLabel) {
            let (id, index) = reference.split_once('#').expect("note-id#index");
            let label = labels
                .iter()
                .find(|label| label.id == id)
                .expect("labelled");
            let claim = &label.claims[index.parse::<usize>().expect("index")];
            (notes[id], key_by_id[id], claim)
        };

        let entries = reconciliation["entries"].as_array().expect("entries");
        for entry in entries {
            let reference = entry["claim"].as_str().expect("claim");
            let (case, key, claim) = claim_at(reference);
            assert_eq!(
                entry["quote"], claim.quote,
                "{reference} misquotes the label"
            );
            assert_eq!(entry["kind"], claim.kind, "{reference}");
            assert_eq!(entry["label"]["verdict"], claim.verdict, "{reference}");
            assert_eq!(entry["label"]["field"], claim.field, "{reference}");
            let span = quote_span(&case.note, &claim.quote).expect("span");
            assert_eq!(
                entry["checker"]["handling"],
                handling(span, &key.extracted).as_str(),
                "{reference} misquotes the key"
            );
            let figures: Vec<JsonValue> = key
                .extracted
                .iter()
                .filter(|figure| span.0 <= figure.start && figure.end <= span.1)
                .map(|figure| json!({"figure": figure.figure, "field": figure.field, "verdict": figure.verdict}))
                .collect();
            assert_eq!(entry["checker"]["figures"], json!(figures), "{reference}");
            let changes = entry["reconciled"]["verdict"] != entry["label"]["verdict"]
                || entry["reconciled"]["field"] != entry["label"]["field"];
            assert_eq!(entry["changes_label"], changes, "{reference}");
        }

        let mut classified: Vec<&str> = reconciliation["cannot_tell_by_reason"]
            .as_object()
            .expect("reasons")
            .values()
            .flat_map(|refs| refs.as_array().expect("refs").iter())
            .map(|reference| reference.as_str().expect("ref"))
            .collect();
        classified.sort_unstable();
        let mut unresolved: Vec<String> = labels
            .iter()
            .flat_map(|label| {
                label
                    .claims
                    .iter()
                    .enumerate()
                    .filter(|(_, claim)| claim.verdict == "cannot_tell")
                    .map(move |(index, _)| format!("{}#{index}", label.id))
            })
            .collect();
        unresolved.sort_unstable();
        assert_eq!(
            classified, unresolved,
            "each cannot_tell classified exactly once"
        );

        let reconciled: Vec<JsonValue> = entries
            .iter()
            .map(|entry| {
                json!({
                    "claim": entry["claim"],
                    "verdict": entry["reconciled"]["verdict"],
                    "field": entry["reconciled"]["field"],
                })
            })
            .collect();
        let computed = json!({
            "as_labelled": headline(&score(&cases, &keys, &labels, &population)),
            "reconciled": headline(&score(&cases, &keys, &with_readings(&labels, &reconciled), &population)),
        });
        println!("{}", serde_json::to_string_pretty(&computed).expect("json"));
        let figures = &reconciliation["figures"];
        assert_eq!(figures["as_labelled"], computed["as_labelled"]);
        assert_eq!(figures["reconciled"], computed["reconciled"]);
        for scenario in figures["scenarios"].as_array().expect("scenarios") {
            let readings = scenario["readings"].as_array().expect("readings");
            let under = headline(&score(
                &cases,
                &keys,
                &with_readings(&labels, readings),
                &population,
            ));
            println!("{}: {}", scenario["name"], under);
            assert_eq!(scenario["figures"], under, "{}", scenario["name"]);
        }
    }

    /// The guarantee, enumerated for these strata: every error count each
    /// sampled stratum could hold is covered at least `1 - 0.05/3` of the time.
    #[test]
    fn every_sampled_stratum_covers_whatever_its_true_count() {
        let Some((_, keys, population, _)) = frozen() else {
            return;
        };
        let tail = (1.0 - crate::jev_controls::JOINT_LEVEL) / (2.0 * 3.0);
        for (stratum, size) in &population {
            let sample = keys.iter().filter(|key| &key.stratum == stratum).count();
            let size = *size as usize;
            let draw = crate::jev_controls::Hypergeometric::new(size, sample);
            let bounds: Vec<(usize, usize)> = (0..=sample)
                .map(|found| (draw.lower(found, tail), draw.upper(found, tail)))
                .collect();
            for wrong in 0..=size {
                let covered: f64 = bounds
                    .iter()
                    .enumerate()
                    .filter(|(_, (low, high))| *low <= wrong && wrong <= *high)
                    .map(|(found, _)| draw.pmf(wrong, found))
                    .sum();
                assert!(
                    covered >= 1.0 - 2.0 * tail - 1e-9,
                    "{stratum} at {wrong}: {covered}"
                );
            }
        }
    }

    /// What this method would report from a perfect result, fixed before any
    /// label exists: every note read, and nothing omitted or wrong anywhere.
    /// Up to 183 of 1,116 notes (16.4%) could still hold an omission; 66 of
    /// the 608 notes the checker read figures in. A clean result bounds the
    /// share; it cannot show it is small.
    #[test]
    fn the_best_this_sample_can_say() {
        let Some((cases, keys, population, _)) = frozen() else {
            return;
        };
        let clean: Vec<NoteLabel> = cases
            .iter()
            .map(|case| NoteLabel {
                id: case.id.clone(),
                status: "labelled".into(),
                claims: Vec::new(),
            })
            .collect();
        let report = score(&cases, &keys, &clean, &population);
        // Checked against exact integer arithmetic: 66 + 60 + 57 = 183.
        for outcome in OUTCOMES {
            assert_eq!(
                report["population_estimate"]["outcomes"][outcome]["notes"],
                json!([0, 183]),
                "{outcome}"
            );
        }
        for (stratum, upper) in [
            (STRATUM_WITH_FIGURES, 66),
            (STRATUM_WITHOUT_FIGURES, 60),
            (STRATUM_NEVER_REACHED, 57),
        ] {
            assert_eq!(
                report["per_stratum"][stratum]["outcomes"]["omission"]["notes_in_population"],
                json!([0, upper]),
                "{stratum}"
            );
        }
    }

    #[test]
    #[ignore]
    fn regenerate_the_whole_note_sample() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let sources: Vec<JsonValue> =
            serde_json::from_slice(&std::fs::read(&path).expect("prompts")).expect("a JSON array");

        // Every distinct note a report attached to a selected symbol.
        let mut notes: BTreeMap<String, (NoteCase, NoteKey)> = BTreeMap::new();
        for entry in &sources {
            let Some(report) = entry.get("report") else {
                continue;
            };
            let report_id = entry.get("id").and_then(JsonValue::as_i64).unwrap_or(-1);
            let prompt = entry
                .get("request")
                .map(crate::xai_decision::decision_prompt_user_payload)
                .unwrap_or(JsonValue::Null);
            let inputs = crate::jev_review::grading_inputs(report, &prompt);
            for asset in report
                .get("selected_assets")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(symbol), Some(note)) = (
                    asset.get("symbol").and_then(JsonValue::as_str),
                    asset.get("notes").and_then(JsonValue::as_str),
                ) else {
                    continue;
                };
                if note.trim().is_empty() {
                    continue;
                }
                let draw = draw_key(report_id, symbol);
                if notes.contains_key(&draw) {
                    continue;
                }
                let judged = inputs
                    .candidates
                    .iter()
                    .position(|candidate| candidate["symbol"].as_str() == Some(symbol));
                let lowered = note.to_lowercase();
                let (evidence, extracted, stratum) = match judged {
                    Some(index) => {
                        let evidence = inputs.evidence[index].clone();
                        let extracted: Vec<Extracted> =
                            crate::jev_numeric::numeric_checks(&lowered, &evidence)
                                .into_iter()
                                .filter_map(|check| {
                                    let span =
                                        crate::jev_numeric::figure_at(&lowered, check.offset)?;
                                    Some(Extracted {
                                        start: span.start,
                                        end: span.end,
                                        figure: lowered[span.start..span.end].to_string(),
                                        field: check.field.map(str::to_string),
                                        verdict: check.verdict.as_str().to_string(),
                                    })
                                })
                                .collect();
                        let stratum = if extracted.is_empty() {
                            STRATUM_WITHOUT_FIGURES
                        } else {
                            STRATUM_WITH_FIGURES
                        };
                        (evidence, extracted, stratum)
                    }
                    None => (
                        evidence_for(&prompt, symbol),
                        Vec::new(),
                        STRATUM_NEVER_REACHED,
                    ),
                };
                let id = format!("note-{}", &draw[..10]);
                notes.insert(
                    draw,
                    (
                        NoteCase {
                            id: id.clone(),
                            source_report: report_id,
                            symbol: symbol.to_string(),
                            note: note.to_string(),
                            evidence,
                        },
                        NoteKey {
                            id,
                            stratum: stratum.to_string(),
                            extracted,
                        },
                    ),
                );
            }
        }

        let mut population: BTreeMap<String, i64> = BTreeMap::new();
        for (_, key) in notes.values() {
            *population.entry(key.stratum.clone()).or_default() += 1;
        }
        println!(
            "distinct notes: {}, by stratum: {population:?}",
            notes.len()
        );

        let (mut cases, mut keys) = (Vec::new(), Vec::new());
        for (stratum, want) in STRATA {
            // BTreeMap iterates in draw-key order, which is the draw.
            let drawn: Vec<&(NoteCase, NoteKey)> = notes
                .values()
                .filter(|(_, key)| key.stratum == *stratum)
                .take(*want)
                .collect();
            println!("stratum {stratum:24} drawn {}", drawn.len());
            for (case, key) in drawn {
                cases.push(case.clone());
                keys.push(key.clone());
            }
        }
        cases.sort_by(|left, right| left.id.cmp(&right.id));
        keys.sort_by(|left, right| left.id.cmp(&right.id));

        std::fs::write(
            INSTRUMENT_PATH,
            serde_json::to_string_pretty(&json!({
                "version": WHOLE_NOTES_VERSION,
                "seed": SAMPLE_SEED,
                "what_this_is": "Whole notes drawn from stored reports, each with the evidence the \
                                 report was given about its symbol. What any checker made of them \
                                 is not here.",
                "how_to_label": {
                    "task": "Read each whole note and list every claim it makes about the supplied \
                             evidence: every figure, and every qualitative statement about an \
                             indicator, a trend, a support level, a Markov state or signal, or a \
                             Quiver signal. Judge each claim against the evidence beside it.",
                    "quote": "The exact words of the claim from the note, as short as still makes \
                              the claim. It must occur exactly once in the note; lengthen it if \
                              it repeats.",
                    "kind": "`numeric` if the claim states a number, whether in digits or in \
                             words (\"five confluences\"); `qualitative` otherwise.",
                    "field": "The dotted evidence field the claim concerns, or `none`.",
                    "verdict": "`consistent` if the evidence supports the claim at the precision \
                                written, `inconsistent` if it contradicts it, `cannot_tell` if two \
                                readings are both defensible or the evidence the claim needs is \
                                missing. Missing evidence is `cannot_tell`, never `inconsistent`.",
                    "status": "Set `labelled` once you have read the whole note, including when it \
                               makes no checkable claim. A note left without it counts as unread.",
                },
                "out_of_scope": "Portfolio capital, holdings, allocations, unrealised profit, \
                                 trading costs, orders, stops and executions, and recommendations \
                                 or intentions. Their supporting material is not in the evidence. \
                                 List no claim for them.",
                "cases": cases,
            }))
            .expect("serialize"),
        )
        .expect("write instrument");
        std::fs::write(
            KEY_PATH,
            serde_json::to_string_pretty(&json!({
                "version": WHOLE_NOTES_VERSION,
                "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
                "protocol": PROTOCOL_VERSION,
                "warning": "Do not read this before labelling. It holds the stratum of every note \
                            and every figure the checker extracted, with its verdict.",
                "population": population,
                "keys": keys,
            }))
            .expect("serialize"),
        )
        .expect("write key");
        if !std::path::Path::new(LABELS_PATH).exists() {
            let template: Vec<NoteLabel> = cases
                .iter()
                .map(|case| NoteLabel {
                    id: case.id.clone(),
                    ..NoteLabel::default()
                })
                .collect();
            std::fs::write(
                LABELS_PATH,
                serde_json::to_string_pretty(&json!({
                    "version": WHOLE_NOTES_VERSION,
                    "labelled_by": "",
                    "labelled_at": "",
                    "labels": template,
                }))
                .expect("serialize"),
            )
            .expect("write labels");
        }
        println!("{} notes written to {INSTRUMENT_PATH}", cases.len());
    }
}
