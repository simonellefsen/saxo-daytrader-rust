//! A fresh, held-out evaluation of the frozen numeric checker.
//!
//! Every earlier instrument drew on the 233-report dump the checker was
//! developed against. The controls, the whole-note audit and the census all
//! read it, and each correction since `n10` was shaped by what they found. So
//! none of them can say how the checker does on notes it has never met; review
//! has said so each time.
//!
//! This one draws only on reports the development never read. That is every
//! completed report after #319, the last in the dump, up to and including
//! #362, the last created before `n15` was frozen at `cc7b0a4`. Nothing in
//! those reports was inspected before the method was frozen.
//!
//! Two parts, both fixed here before any label exists:
//!
//! - **Natural notes.** Every note in the frame goes to an independent
//!   labeller. For each numeric claim they give the quote, the figure, the
//!   field and a verdict. The key holds what `n15` extracted from each note.
//!   Matching the two gives false alarms on clean claims, errors accepted,
//!   omissions and abstentions, on notes nobody tuned against.
//! - **Seeded cases**, generated from the labels once they are committed. A
//!   claim the labeller reads as consistent, and which the key's own
//!   arithmetic confirms, is changed one way at a time. The changes run in
//!   both directions: the note or the evidence is moved so the claim becomes
//!   false, or both are moved so it stays true. A false claim is repaired. A
//!   field is removed. Another symbol's value is copied in. An out-of-scope
//!   stop is placed beside a support level. The truth of each result follows
//!   from the change. **The checker chooses nothing here.** The challenge set
//!   anchored on figures the checker already read, so it could not detect an
//!   attribution that was wrong from the start. These anchors come from the
//!   labeller, and generation never calls the checker.
//!
//! The rules the earlier instruments earned apply from the start: blind,
//! labels committed as delivered, scored once, reconciled beside the labels
//! rather than over them. I do not fill in the labels; I wrote the checker.
//! The method is frozen: scoring refuses to run under any method but `n15`.

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::collections::{BTreeMap, BTreeSet};

use crate::jev_whole_notes::{Extracted, NoteCase, NoteKey, quote_span};

pub(crate) const FRESH_VERSION: &str = "fresh-v1-2026-09-25";
/// The method under evaluation. Nothing is scored under any other.
pub(crate) const FROZEN_METHOD: &str = "n15-2026-09-25";
pub(crate) const FROZEN_COMMIT: &str = "cc7b0a4";
/// Names each note; fixed, recorded, used for nothing else.
pub(crate) const SEED: &str = "jev-fresh-v1";
/// The last report in the development dump.
pub(crate) const LAST_DEVELOPMENT_REPORT: i64 = 319;
/// The last report created before `n15` was frozen.
pub(crate) const LAST_REPORT_BEFORE_FREEZE: i64 = 362;

/// The analysis plan, fixed here before any label exists.
///
/// **The frame is taken whole.** Every note in it is labelled, so its counts
/// carry no sampling error. They describe these eleven reports. Generalising
/// to later reports needs a sampling model this set does not have, and no rate
/// is extrapolated.
///
/// **Natural notes.** Each labelled numeric claim is matched to the figure
/// `n15` extracted over the labeller's figure. The checker's *reading* of it:
///
/// - `omitted`: nothing extracted there;
/// - `abstained`: extracted, not compared;
/// - `accepted`: compared and agreed;
/// - `flagged`: compared and disagreed.
///
/// A comparison against a field other than the labeller's is also counted
/// as `wrong_field`. Claims are described in full. Because claims within a
/// note are not independent, the headline is per note, twice: over the notes
/// production showed the checker, and over every note, given its evidence,
/// which is the method's own reading of the notes production never showed it.
///
/// The outcomes:
///
/// - `false_alarm`: a claim labelled consistent was flagged;
/// - `accepted_error`: a claim labelled inconsistent was accepted;
/// - `unflagged_error`: a claim labelled inconsistent was not flagged;
/// - `omission`: a labelled claim had no extracted figure;
/// - `wrong_field`: a claim was compared against another field.
///
/// A `cannot_tell` claim leaves the first three unresolved where it would
/// otherwise decide them, and an unresolved note is reported both ways.
///
/// **Seeded cases.** Each keyed figure's outcome follows from its truth:
///
/// - `holds`: correct on `matches` against the keyed field. A `differs` is a
///   false alarm.
/// - `fails`: correct on `differs` against the keyed field. A `matches` is an
///   error accepted.
/// - `unsettleable` and `out_of_scope`: correct when not compared at all.
///
/// Anything else is an abstention, which is neither correct nor wrong. It is
/// counted in its own column, so a checker that abstains everywhere scores
/// nothing, not a clean sheet.
///
/// **Nothing is scored while any note is unlabelled**, and a labels file with
/// any malformed entry is rejected whole.
pub(crate) const PROTOCOL_VERSION: &str = "fresh-protocol-v1-2026-09-25";

pub(crate) const STRATUM_WITH_FIGURES: &str = "reached_with_figures";
pub(crate) const STRATUM_WITHOUT_FIGURES: &str = "reached_without_figures";
pub(crate) const STRATUM_NEVER_REACHED: &str = "never_reached";

const VERDICTS: [&str; 3] = ["consistent", "inconsistent", "cannot_tell"];
const NOTE_OUTCOMES: [&str; 5] = [
    "false_alarm",
    "accepted_error",
    "unflagged_error",
    "omission",
    "wrong_field",
];

/// Whole-number fields: a seeded change moves them by two, not by a ratio.
const DISCRETE_FIELDS: &[&str] = &[
    "daily_indicators.confluence_count",
    "daily_indicators.min_confluences",
    "markov.horizon_days",
];
/// Fields whose sign is part of the value. `markov.conviction` is not one: a
/// note's "+0.734 conviction" marks direction on a magnitude.
const SIGNED_FIELDS: &[&str] = &["markov.signed_signal", "quiver.signal"];
/// A price level, beside which an out-of-scope stop can be placed.
const PRICE_LEVEL: &str = "daily_indicators.support.nearest_support";

/// Ratios for a seeded change of value: far enough that no rounding or
/// truncation convention can reach the new figure from the old.
const UP: f64 = 1.37;
const DOWN: f64 = 0.63;

/// One numeric claim a labeller found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FreshClaim {
    /// The exact words, as short as makes the claim; must occur once in the
    /// note.
    pub quote: String,
    /// The number as written, which must occur once in the quote.
    pub figure: String,
    /// The evidence path the claim concerns, or `none`.
    pub field: String,
    pub verdict: String,
    #[serde(default)]
    pub note: String,
}

/// What a labeller records for one note.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FreshLabel {
    pub id: String,
    /// `labelled` once the whole note has been read, even if it holds nothing.
    pub status: String,
    #[serde(default)]
    pub claims: Vec<FreshClaim>,
}

/// The value at a dotted path, when it is a finite number.
fn number_at(evidence: &JsonValue, path: &str) -> Option<f64> {
    let mut cursor = evidence;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    cursor.as_f64().filter(|value| value.is_finite())
}

/// Whether `text[at..at + len]` is a whole token: not inside a word or a
/// longer number.
fn stands_alone(text: &str, at: usize, len: usize) -> bool {
    let bytes = text.as_bytes();
    let inside_number = |index: usize, step_back: bool| {
        let neighbour = if step_back {
            index.checked_sub(1).and_then(|i| bytes.get(i))
        } else {
            bytes.get(index + 1)
        };
        neighbour.is_some_and(u8::is_ascii_digit)
    };
    let before = at.checked_sub(1).map(|index| (index, bytes[index]));
    let after = bytes.get(at + len).map(|byte| (at + len, *byte));
    let clear_before = match before {
        None => true,
        Some((_, byte)) if byte.is_ascii_alphanumeric() => false,
        Some((index, b'.' | b',')) => !inside_number(index, true),
        Some(_) => true,
    };
    let clear_after = match after {
        None => true,
        Some((_, byte)) if byte.is_ascii_alphanumeric() => false,
        Some((index, b'.' | b',')) => !inside_number(index, false),
        Some(_) => true,
    };
    clear_before && clear_after
}

/// Where a claim's figure sits in the lowercased note. The quote must occur
/// once in the note, and the figure once in the quote, as a whole token.
pub(crate) fn figure_span(note: &str, claim: &FreshClaim) -> Result<(usize, usize), &'static str> {
    let (start, end) = quote_span(note, &claim.quote)?;
    let lowered = note.to_lowercase();
    let quote = &lowered[start..end];
    let figure = claim.figure.trim().to_lowercase();
    if figure.is_empty() {
        return Err("empty figure");
    }
    let mut found = quote
        .match_indices(figure.as_str())
        .filter(|(at, _)| stands_alone(quote, *at, figure.len()));
    let Some((at, _)) = found.next() else {
        return Err("figure is not in the quote as a whole number");
    };
    if found.next().is_some() {
        return Err("figure occurs more than once in the quote");
    }
    Ok((start + at, start + at + figure.len()))
}

/// Number words, zero to twelve, and their values.
const WORDS: [&str; 13] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve",
];

/// A figure as written: its value, and what writing it again must keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Written {
    pub value: f64,
    pub decimals: usize,
    pub explicit_sign: bool,
    pub word: bool,
}

/// Reads a figure the seeded changes can rewrite: plain digits with an
/// optional sign and decimals, or a number word from zero to twelve. A
/// percentage or a grouped thousand is not rewritten, so it anchors nothing.
pub(crate) fn parse_written(figure: &str) -> Option<Written> {
    let figure = figure.trim().to_lowercase();
    if let Some(value) = WORDS.iter().position(|word| *word == figure) {
        return Some(Written {
            value: value as f64,
            decimals: 0,
            explicit_sign: false,
            word: true,
        });
    }
    let (sign, digits, explicit_sign) = match figure.as_bytes().first()? {
        b'+' => (1.0, &figure[1..], true),
        b'-' => (-1.0, &figure[1..], true),
        _ => (1.0, figure.as_str(), false),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let plain = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    if whole.is_empty() || !plain(whole) || !plain(fraction) || digits.ends_with('.') {
        return None;
    }
    Some(Written {
        value: sign * digits.parse::<f64>().ok()?,
        decimals: fraction.len(),
        explicit_sign,
        word: false,
    })
}

/// Writes a value in the form `like` was written in. `None` for a word
/// outside zero to twelve.
pub(crate) fn write_like(value: f64, like: Written) -> Option<String> {
    if like.word {
        let rounded = value.round();
        return (0.0..=12.0)
            .contains(&rounded)
            .then(|| WORDS[rounded as usize].to_string());
    }
    let magnitude = format!("{:.*}", like.decimals, value.abs());
    let zero = magnitude.chars().all(|c| c == '0' || c == '.');
    Some(if value < 0.0 && !zero {
        format!("-{magnitude}")
    } else if like.explicit_sign {
        format!("+{magnitude}")
    } else {
        magnitude
    })
}

fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    if (value - floor - 0.5).abs() < 1e-12 {
        if floor.rem_euclid(2.0) == 0.0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
    }
}

/// Whether a written figure could have come from a stored value by rounding
/// either way at a half, or by truncating, at the precision written. A
/// whole-number field needs the exact number.
///
/// The key's own arithmetic, not a call into the checker it scores.
pub(crate) fn producible(written: Written, stored: f64, discrete: bool) -> bool {
    if discrete {
        return written.decimals == 0 && (written.value - stored).abs() < 1e-9;
    }
    let scale = 10f64.powi(written.decimals as i32);
    let scaled = stored * scale;
    let slack = 1e-9 * written.value.abs().max(1.0);
    [scaled.round(), round_half_even(scaled), scaled.trunc()]
        .iter()
        .any(|candidate| (candidate / scale - written.value).abs() <= slack)
}

/// Everything wrong with a labels file, as sets so the report does not depend
/// on the order of the file.
pub(crate) fn label_problems(
    cases: &[NoteCase],
    labels: &[FreshLabel],
) -> BTreeMap<&'static str, BTreeSet<String>> {
    let by_id: BTreeMap<&str, &NoteCase> =
        cases.iter().map(|case| (case.id.as_str(), case)).collect();
    let mut problems: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    let mut flag = |kind: &'static str, what: String| {
        problems.entry(kind).or_default().insert(what);
    };
    let mut seen = BTreeSet::new();
    for label in labels {
        let id = label.id.as_str();
        let Some(case) = by_id.get(id) else {
            flag("unknown_ids", id.to_string());
            continue;
        };
        if !seen.insert(id) {
            flag("duplicate_ids", id.to_string());
        }
        let status = label.status.trim();
        if !status.is_empty() && status != "labelled" {
            flag("unreadable_status", id.to_string());
        }
        if status.is_empty() && !label.claims.is_empty() {
            flag("claims_on_an_unlabelled_note", id.to_string());
        }
        let mut spans = BTreeSet::new();
        for (index, claim) in label.claims.iter().enumerate() {
            let at = format!("{id}#{index}");
            if !VERDICTS.contains(&claim.verdict.trim()) {
                flag("unreadable_verdict", at.clone());
            }
            let field = claim.field.trim();
            if field != "none" && number_at(&case.evidence, field).is_none() {
                flag(
                    "field_not_a_number_in_the_evidence",
                    format!("{at}: {field}"),
                );
            }
            match figure_span(&case.note, claim) {
                Ok(span) => {
                    if !spans.insert(span) {
                        flag("figure_claimed_twice", at.clone());
                    }
                }
                Err(reason) => flag("unusable_figure", format!("{at}: {reason}")),
            }
        }
    }
    problems
}

/// What the checker did with a claim's figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    Omitted,
    Abstained,
    Accepted,
    Flagged,
}

impl Reading {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Omitted => "omitted",
            Self::Abstained => "abstained",
            Self::Accepted => "accepted",
            Self::Flagged => "flagged",
        }
    }

    fn decided(self) -> bool {
        matches!(self, Self::Accepted | Self::Flagged)
    }
}

/// The extracted figure over a claim's figure, and the reading it gives.
pub(crate) fn reading(
    span: (usize, usize),
    extracted: &[Extracted],
) -> (Reading, Option<&Extracted>) {
    let Some(hit) = extracted
        .iter()
        .find(|figure| figure.start < span.1 && span.0 < figure.end)
    else {
        return (Reading::Omitted, None);
    };
    let reading = match hit.verdict.as_str() {
        "matches" => Reading::Accepted,
        "differs" => Reading::Flagged,
        _ => Reading::Abstained,
    };
    (reading, Some(hit))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binary {
    Yes,
    No,
    Unresolved,
}

/// The note-level outcomes for one labelled note.
pub(crate) fn note_outcomes(
    case: &NoteCase,
    key: &NoteKey,
    label: &FreshLabel,
) -> BTreeMap<&'static str, Binary> {
    let (mut alarm, mut alarm_unsure) = (false, false);
    let (mut accepted, mut accepted_unsure) = (false, false);
    let (mut unflagged, mut unflagged_unsure) = (false, false);
    let (mut omission, mut wrong_field) = (false, false);
    for claim in &label.claims {
        let Ok(span) = figure_span(&case.note, claim) else {
            continue;
        };
        let (read, hit) = reading(span, &key.extracted);
        let verdict = claim.verdict.trim();
        omission |= read == Reading::Omitted;
        wrong_field |= read.decided()
            && hit.and_then(|figure| figure.field.as_deref()) != Some(claim.field.trim());
        match (read, verdict) {
            (Reading::Flagged, "consistent") => alarm = true,
            (Reading::Flagged, "cannot_tell") => alarm_unsure = true,
            (Reading::Accepted, "inconsistent") => accepted = true,
            (Reading::Accepted, "cannot_tell") => accepted_unsure = true,
            _ => {}
        }
        if read != Reading::Flagged {
            unflagged |= verdict == "inconsistent";
            unflagged_unsure |= verdict == "cannot_tell";
        }
    }
    let resolve = |yes: bool, unsure: bool| match (yes, unsure) {
        (true, _) => Binary::Yes,
        (false, true) => Binary::Unresolved,
        (false, false) => Binary::No,
    };
    let plain = |yes: bool| if yes { Binary::Yes } else { Binary::No };
    BTreeMap::from([
        ("false_alarm", resolve(alarm, alarm_unsure)),
        ("accepted_error", resolve(accepted, accepted_unsure)),
        ("unflagged_error", resolve(unflagged, unflagged_unsure)),
        ("omission", plain(omission)),
        ("wrong_field", plain(wrong_field)),
    ])
}

/// Scores the natural notes under `PROTOCOL_VERSION`.
pub(crate) fn score_natural(
    cases: &[NoteCase],
    keys: &[NoteKey],
    labels: &[FreshLabel],
) -> JsonValue {
    let problems = label_problems(cases, labels);
    if !problems.is_empty() {
        return json!({
            "version": FRESH_VERSION,
            "protocol": PROTOCOL_VERSION,
            "status": "rejected",
            "label_problems": problems,
            "reading": "Rejected, and nothing is scored until the input is corrected.",
        });
    }
    let labels_by_id: BTreeMap<&str, &FreshLabel> = labels
        .iter()
        .map(|label| (label.id.as_str(), label))
        .collect();
    let keys_by_id: BTreeMap<&str, &NoteKey> =
        keys.iter().map(|key| (key.id.as_str(), key)).collect();

    let mut unlabelled = 0usize;
    // stratum -> outcome -> [yes, no, unresolved]
    let mut notes: BTreeMap<String, BTreeMap<&'static str, [usize; 3]>> = BTreeMap::new();
    let mut notes_per_stratum: BTreeMap<String, usize> = BTreeMap::new();
    // stratum -> "reading|verdict" -> claims
    let mut claims: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut wrong_field_claims: BTreeMap<String, usize> = BTreeMap::new();
    let mut unclaimed: BTreeMap<String, usize> = BTreeMap::new();
    for case in cases {
        let Some(key) = keys_by_id.get(case.id.as_str()) else {
            continue;
        };
        *notes_per_stratum.entry(key.stratum.clone()).or_default() += 1;
        let Some(label) = labels_by_id
            .get(case.id.as_str())
            .filter(|label| label.status.trim() == "labelled")
        else {
            unlabelled += 1;
            continue;
        };
        for (outcome, value) in note_outcomes(case, key, label) {
            let tally = notes
                .entry(key.stratum.clone())
                .or_default()
                .entry(outcome)
                .or_default();
            tally[match value {
                Binary::Yes => 0,
                Binary::No => 1,
                Binary::Unresolved => 2,
            }] += 1;
        }
        let mut spans = Vec::new();
        for claim in &label.claims {
            let span = figure_span(&case.note, claim).expect("validated");
            spans.push(span);
            let (read, hit) = reading(span, &key.extracted);
            *claims
                .entry(key.stratum.clone())
                .or_default()
                .entry(format!("{}|{}", read.as_str(), claim.verdict.trim()))
                .or_default() += 1;
            if read.decided()
                && hit.and_then(|figure| figure.field.as_deref()) != Some(claim.field.trim())
            {
                *wrong_field_claims
                    .entry(format!("{}|{}", read.as_str(), claim.verdict.trim()))
                    .or_default() += 1;
            }
        }
        for figure in &key.extracted {
            if !spans
                .iter()
                .any(|span| figure.start < span.1 && span.0 < figure.end)
            {
                *unclaimed.entry(figure.verdict.clone()).or_default() += 1;
            }
        }
    }

    let complete = unlabelled == 0;
    let summary = |strata: &[&str]| {
        let in_scope: usize = strata
            .iter()
            .map(|stratum| notes_per_stratum.get(*stratum).copied().unwrap_or(0))
            .sum();
        let mut outcomes = serde_json::Map::new();
        for outcome in NOTE_OUTCOMES {
            let [yes, _, unresolved] = strata.iter().fold([0usize; 3], |sum, stratum| {
                let row = notes
                    .get(*stratum)
                    .and_then(|outcomes| outcomes.get(outcome))
                    .copied()
                    .unwrap_or_default();
                [sum[0] + row[0], sum[1] + row[1], sum[2] + row[2]]
            });
            outcomes.insert(
                outcome.to_string(),
                json!({"notes": [yes, yes + unresolved], "of": in_scope}),
            );
        }
        let count = |reading: &str, verdict: &str| -> usize {
            strata
                .iter()
                .filter_map(|stratum| claims.get(*stratum))
                .filter_map(|by| by.get(&format!("{reading}|{verdict}")))
                .sum()
        };
        let decided = |verdict: &str| count("accepted", verdict) + count("flagged", verdict);
        let inconsistent: usize = ["omitted", "abstained", "accepted", "flagged"]
            .iter()
            .map(|reading| count(reading, "inconsistent"))
            .sum();
        json!({
            "notes": outcomes,
            "claims": {
                "consistent_flagged": [count("flagged", "consistent"), decided("consistent")],
                "inconsistent_accepted": [count("accepted", "inconsistent"), decided("inconsistent")],
                "inconsistent_flagged": [count("flagged", "inconsistent"), inconsistent],
            },
        })
    };
    let headline = complete.then(|| {
        json!({
            "notes_the_checker_read": summary(&[STRATUM_WITH_FIGURES, STRATUM_WITHOUT_FIGURES]),
            "every_note": summary(&[
                STRATUM_WITH_FIGURES,
                STRATUM_WITHOUT_FIGURES,
                STRATUM_NEVER_REACHED,
            ]),
        })
    });
    json!({
        "version": FRESH_VERSION,
        "protocol": PROTOCOL_VERSION,
        "status": if complete { "complete" } else { "labelling_incomplete" },
        "notes": cases.len(),
        "unlabelled": unlabelled,
        "headline": headline,
        "notes_by_stratum": notes,
        "claims_by_stratum": claims,
        "decided_against_another_field": wrong_field_claims,
        "extracted_figures_no_claim_covers": unclaimed,
        "reading": "Counts over the whole frame, which has no sampling error and describes \
                    these reports only. `notes_the_checker_read` is production: the notes the \
                    checker was shown. `every_note` is the method, given every note and its \
                    evidence. `[a, b]` is `a` notes, or `b` if every unresolved note went the \
                    same way. Claim pairs are [count, of].",
    })
}

/// One figure in a seeded note whose truth is known.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SeededKey {
    /// Byte span of the figure in the seeded note.
    pub start: usize,
    pub end: usize,
    pub figure: String,
    /// The field the claim concerns; `None` for an inserted out-of-scope
    /// figure, which concerns none.
    pub field: Option<String>,
    /// `holds`, `fails`, `unsettleable` or `out_of_scope`.
    pub truth: String,
    /// Whether this is the figure the change made, rather than one it left.
    pub changed: bool,
}

/// A note and its evidence, changed one way, with every anchored figure's
/// truth.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SeededCase {
    pub id: String,
    pub from_note: String,
    pub mutation: String,
    /// `true_to_false`, `false_to_true`, `stays_true`, `unsettleable` or
    /// `out_of_scope`.
    pub direction: String,
    /// Lowercased: the checker lowercases before reading, and changing the
    /// text it reads removes a class of offset errors.
    pub note: String,
    pub evidence: JsonValue,
    pub keys: Vec<SeededKey>,
}

/// A figure the labeller read as consistent, and the key's own arithmetic
/// confirms.
#[derive(Clone, Debug)]
struct Anchor {
    start: usize,
    end: usize,
    field: String,
    stored: f64,
    written: Written,
}

fn discrete(field: &str) -> bool {
    DISCRETE_FIELDS.contains(&field)
}

fn set_number(evidence: &mut JsonValue, path: &str, value: Option<f64>) {
    let segments: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    let mut cursor = evidence;
    for segment in parents {
        let Some(next) = cursor.get_mut(*segment) else {
            return;
        };
        cursor = next;
    }
    let Some(object) = cursor.as_object_mut() else {
        return;
    };
    match value {
        Some(value) => {
            object.insert((*last).to_string(), json!(value));
        }
        None => {
            object.remove(*last);
        }
    }
}

/// Moves a value one way: by two for a whole-number field, by a ratio
/// otherwise.
fn moved(value: f64, field: &str, up: bool) -> Option<f64> {
    if discrete(field) {
        let next = if up { value + 2.0 } else { value - 2.0 };
        (next >= 0.0).then_some(next)
    } else {
        (value != 0.0).then_some(value * if up { UP } else { DOWN })
    }
}

/// The anchors in one labelled note, in the order they appear.
fn anchors(case: &NoteCase, label: &FreshLabel) -> Vec<Anchor> {
    let mut found: Vec<Anchor> = Vec::new();
    for claim in &label.claims {
        let field = claim.field.trim();
        if claim.verdict.trim() != "consistent" || field == "none" {
            continue;
        }
        let (Ok((start, end)), Some(stored), Some(written)) = (
            figure_span(&case.note, claim),
            number_at(&case.evidence, field),
            parse_written(&claim.figure),
        ) else {
            continue;
        };
        if !producible(written, stored, discrete(field))
            || found.iter().any(|anchor| anchor.start == start)
        {
            continue;
        }
        found.push(Anchor {
            start,
            end,
            field: field.to_string(),
            stored,
            written,
        });
    }
    found.sort_by_key(|anchor| anchor.start);
    found
}

/// Where the clause holding `index` ends: the next comma, semicolon or full
/// stop that is not a decimal point.
fn clause_end(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    (index..bytes.len())
        .find(|&position| match bytes[position] {
            b',' | b';' | b'!' | b'?' => {
                !(bytes[position] == b','
                    && position > 0
                    && bytes[position - 1].is_ascii_digit()
                    && bytes.get(position + 1).is_some_and(u8::is_ascii_digit))
            }
            b'.' => {
                !(position > 0
                    && bytes[position - 1].is_ascii_digit()
                    && bytes.get(position + 1).is_some_and(u8::is_ascii_digit))
            }
            _ => false,
        })
        .unwrap_or(bytes.len())
}

/// One change to a note or its evidence.
struct Change<'a> {
    mutation: &'a str,
    /// `true_to_false`, `false_to_true`, `stays_true`, `unsettleable` or
    /// `out_of_scope`, which the changed figure's truth must bear out.
    direction: &'a str,
    /// The anchor rewritten, and its new figure.
    rewrite: Option<(usize, String)>,
    /// Text inserted at a byte offset, and the figure inside it.
    insert: Option<(usize, String, String)>,
    evidence: JsonValue,
    /// The field whose stored value the change moved or removed.
    evidence_field: Option<&'a str>,
}

/// The truth of a written figure against the evidence as it now stands, by
/// the key's own arithmetic.
fn truth_of(figure: &str, field: &str, evidence: &JsonValue) -> &'static str {
    match (number_at(evidence, field), parse_written(figure)) {
        (None, _) => "unsettleable",
        (Some(stored), Some(written)) if producible(written, stored, discrete(field)) => "holds",
        _ => "fails",
    }
}

/// Applies one change and keys every anchored figure where it now sits. `None`
/// when the changed figure's truth does not bear out the direction, so no case
/// is ever keyed by intention rather than arithmetic.
fn build(
    case: &NoteCase,
    note: &str,
    anchors: &[Anchor],
    index: usize,
    change: Change,
) -> Option<SeededCase> {
    let (at, removed, added) = match (&change.rewrite, &change.insert) {
        (Some((target, figure)), _) => {
            let anchor = &anchors[*target];
            (anchor.start, anchor.end - anchor.start, figure.clone())
        }
        (None, Some((offset, inserted, _))) => (*offset, 0, inserted.clone()),
        (None, None) => (0, 0, String::new()),
    };
    let mut text = note.to_string();
    text.replace_range(at..at + removed, &added);
    // Anchors never overlap, so each lies wholly before the edit or after it.
    let place = |start: usize| {
        if start >= at + removed {
            start + added.len() - removed
        } else {
            start
        }
    };
    let mut keys = Vec::new();
    for (which, anchor) in anchors.iter().enumerate() {
        let rewritten = matches!(&change.rewrite, Some((target, _)) if *target == which);
        let start = if rewritten {
            anchor.start
        } else {
            place(anchor.start)
        };
        let end = if rewritten {
            anchor.start + added.len()
        } else {
            start + (anchor.end - anchor.start)
        };
        let figure = text[start..end].to_string();
        keys.push(SeededKey {
            start,
            end,
            truth: truth_of(&figure, &anchor.field, &change.evidence).to_string(),
            figure,
            field: Some(anchor.field.clone()),
            changed: rewritten || change.evidence_field == Some(anchor.field.as_str()),
        });
    }
    if let Some((offset, inserted, figure)) = &change.insert {
        let within = inserted.find(figure.as_str())?;
        keys.push(SeededKey {
            start: offset + within,
            end: offset + within + figure.len(),
            figure: figure.clone(),
            field: None,
            truth: "out_of_scope".to_string(),
            changed: true,
        });
    }
    keys.sort_by_key(|key| key.start);
    let expected = match change.direction {
        "true_to_false" => "fails",
        "false_to_true" | "stays_true" => "holds",
        "unsettleable" => "unsettleable",
        _ => "out_of_scope",
    };
    let borne_out = keys
        .iter()
        .filter(|key| key.changed)
        .all(|key| key.truth == expected);
    let untouched_hold = keys
        .iter()
        .filter(|key| !key.changed)
        .all(|key| key.truth == "holds");
    (borne_out && untouched_hold && keys.iter().any(|key| key.changed)).then(|| SeededCase {
        id: format!("{}|{index}|{}", case.id, change.mutation),
        from_note: case.id.clone(),
        mutation: change.mutation.to_string(),
        direction: change.direction.to_string(),
        note: text,
        evidence: change.evidence,
        keys,
    })
}

/// Every seeded case, from the committed labels and nothing else. Pure and
/// deterministic, and it never calls the checker.
pub(crate) fn generate_seeded(cases: &[NoteCase], labels: &[FreshLabel]) -> Vec<SeededCase> {
    let labels_by_id: BTreeMap<&str, &FreshLabel> = labels
        .iter()
        .map(|label| (label.id.as_str(), label))
        .collect();
    let mut ordered: Vec<&NoteCase> = cases.iter().collect();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));
    let mut seeded = Vec::new();
    for case in ordered {
        let Some(label) = labels_by_id
            .get(case.id.as_str())
            .filter(|label| label.status.trim() == "labelled")
        else {
            continue;
        };
        let note = case.note.to_lowercase();
        let anchors = anchors(case, label);
        let mut emit = |anchors: &[Anchor], index: usize, change: Change| {
            if let Some(built) = build(case, &note, anchors, index, change) {
                seeded.push(built);
            }
        };
        for (index, anchor) in anchors.iter().enumerate() {
            let field = anchor.field.as_str();
            let in_note = |mutation, direction, figure: String| Change {
                mutation,
                direction,
                rewrite: Some((index, figure)),
                insert: None,
                evidence: case.evidence.clone(),
                evidence_field: None,
            };
            let in_evidence = |mutation, direction, stored: Option<f64>| {
                let mut evidence = case.evidence.clone();
                set_number(&mut evidence, field, stored);
                Change {
                    mutation,
                    direction,
                    rewrite: None,
                    insert: None,
                    evidence,
                    evidence_field: Some(field),
                }
            };
            // The note changes, and the claim becomes false.
            for (mutation, up) in [("note_value_up", true), ("note_value_down", false)] {
                if let Some(figure) = moved(anchor.written.value, field, up)
                    .and_then(|value| write_like(value, anchor.written))
                {
                    emit(&anchors, index, in_note(mutation, "true_to_false", figure));
                }
            }
            // The sign changes, where the sign is part of the value.
            if SIGNED_FIELDS.contains(&field) && anchor.written.value != 0.0 {
                let signed = Written {
                    explicit_sign: true,
                    ..anchor.written
                };
                if let Some(figure) = write_like(-anchor.written.value, signed) {
                    emit(
                        &anchors,
                        index,
                        in_note("sign_flipped", "true_to_false", figure),
                    );
                }
            }
            // The evidence changes under an unchanged note.
            for (mutation, up) in [("evidence_value_up", true), ("evidence_value_down", false)] {
                if let Some(stored) = moved(anchor.stored, field, up) {
                    emit(
                        &anchors,
                        index,
                        in_evidence(mutation, "true_to_false", Some(stored)),
                    );
                }
            }
            // Both change together, and the claim stays true.
            if let Some(stored) = moved(anchor.stored, field, true) {
                let scale = 10f64.powi(anchor.written.decimals as i32);
                if let Some(figure) = write_like((stored * scale).round() / scale, anchor.written) {
                    let mut change = in_note("both_moved", "stays_true", figure);
                    set_number(&mut change.evidence, field, Some(stored));
                    change.evidence_field = Some(field);
                    emit(&anchors, index, change);
                }
            }
            // The field is gone from the evidence.
            emit(
                &anchors,
                index,
                in_evidence("evidence_removed", "unsettleable", None),
            );
            // Another symbol's value for the same field, copied in: the
            // carry-over review found in report #259.
            if !anchor.written.word {
                let mut others: Vec<&NoteCase> = cases
                    .iter()
                    .filter(|other| {
                        other.source_report == case.source_report && other.symbol != case.symbol
                    })
                    .collect();
                others.sort_by(|left, right| left.symbol.cmp(&right.symbol));
                let copied = others.iter().find_map(|other| {
                    let figure = write_like(number_at(&other.evidence, field)?, anchor.written)?;
                    (truth_of(&figure, field, &case.evidence) == "fails").then_some(figure)
                });
                if let Some(figure) = copied {
                    emit(
                        &anchors,
                        index,
                        in_note("other_symbol_value", "true_to_false", figure),
                    );
                }
            }
            // An out-of-scope stop placed beside a support level.
            if field == PRICE_LEVEL
                && let Some(stop) = write_like(anchor.stored * 0.97, anchor.written)
            {
                emit(
                    &anchors,
                    index,
                    Change {
                        mutation: "out_of_scope_stop_inserted",
                        direction: "out_of_scope",
                        rewrite: None,
                        insert: Some((
                            clause_end(&note, anchor.end),
                            format!(", stop-loss at {stop}"),
                            stop,
                        )),
                        evidence: case.evidence.clone(),
                        evidence_field: None,
                    },
                );
            }
        }
        // A claim the labeller read as wrong, repaired to the stored value.
        for (index, claim) in label.claims.iter().enumerate() {
            let field = claim.field.trim();
            if claim.verdict.trim() != "inconsistent" || field == "none" {
                continue;
            }
            let (Ok((start, end)), Some(stored), Some(written)) = (
                figure_span(&case.note, claim),
                number_at(&case.evidence, field),
                parse_written(&claim.figure),
            ) else {
                continue;
            };
            // Only a figure the arithmetic also finds wrong. A label the
            // arithmetic contradicts is a matter for reconciliation, not a
            // repair.
            if producible(written, stored, discrete(field)) {
                continue;
            }
            let scale = 10f64.powi(written.decimals as i32);
            let Some(figure) = write_like((stored * scale).round() / scale, written) else {
                continue;
            };
            let mut with_repair = anchors.clone();
            with_repair.retain(|anchor| anchor.start != start);
            with_repair.push(Anchor {
                start,
                end,
                field: field.to_string(),
                stored,
                written,
            });
            with_repair.sort_by_key(|anchor| anchor.start);
            let target = with_repair
                .iter()
                .position(|anchor| anchor.start == start)
                .expect("just added");
            emit(
                &with_repair,
                anchors.len() + index,
                Change {
                    mutation: "repaired",
                    direction: "false_to_true",
                    rewrite: Some((target, figure)),
                    insert: None,
                    evidence: case.evidence.clone(),
                    evidence_field: None,
                },
            );
        }
    }
    seeded
}

/// The outcome for one keyed figure, from the checker's reading of the
/// seeded note.
pub(crate) fn seeded_outcome(
    key: &SeededKey,
    verdict: Option<&str>,
    field: Option<&str>,
) -> &'static str {
    let decided = matches!(verdict, Some("matches" | "differs"));
    let same_field = field == key.field.as_deref();
    match (key.truth.as_str(), verdict) {
        ("holds", Some("matches")) if same_field => "correct",
        ("holds", Some("matches")) => "right_verdict_wrong_field",
        ("holds", Some("differs")) => "false_alarm",
        ("fails", Some("differs")) if same_field => "correct",
        ("fails", Some("differs")) => "flagged_wrong_field",
        ("fails", Some("matches")) => "error_accepted",
        ("unsettleable", _) if decided => "decided_without_evidence",
        ("out_of_scope", _) if decided => "misattributed",
        ("unsettleable" | "out_of_scope", _) => "correct",
        _ => "abstained",
    }
}

/// Scores the seeded cases with the checker as it now stands, which must be
/// the frozen method.
pub(crate) fn score_seeded(seeded: &[SeededCase]) -> JsonValue {
    if crate::jev_numeric::NUMERIC_METHOD_VERSION != FROZEN_METHOD {
        return json!({
            "status": "method_moved",
            "reading": format!(
                "The method is {}, not the frozen {FROZEN_METHOD}. Score at {FROZEN_COMMIT}.",
                crate::jev_numeric::NUMERIC_METHOD_VERSION
            ),
        });
    }
    // mutation -> changed|untouched -> truth -> outcome -> figures
    type ByOutcome = BTreeMap<&'static str, usize>;
    let mut tally: BTreeMap<String, BTreeMap<&str, BTreeMap<String, ByOutcome>>> = BTreeMap::new();
    let mut totals: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    for case in seeded {
        let checks = crate::jev_numeric::numeric_checks(&case.note, &case.evidence);
        let spans: Vec<((usize, usize), &crate::jev_numeric::NumericCheck)> = checks
            .iter()
            .filter_map(|check| {
                let span = crate::jev_numeric::figure_at(&case.note, check.offset)?;
                Some(((span.start, span.end), check))
            })
            .collect();
        for key in &case.keys {
            let hit = spans
                .iter()
                .find(|((start, end), _)| *start < key.end && key.start < *end)
                .map(|(_, check)| *check);
            let outcome = seeded_outcome(
                key,
                hit.map(|check| check.verdict.as_str()),
                hit.and_then(|check| check.field),
            );
            let which = if key.changed { "changed" } else { "untouched" };
            *tally
                .entry(case.mutation.clone())
                .or_default()
                .entry(which)
                .or_default()
                .entry(key.truth.clone())
                .or_default()
                .entry(outcome)
                .or_default() += 1;
            *totals
                .entry(format!("{which}|{}", key.truth))
                .or_default()
                .entry(outcome)
                .or_default() += 1;
        }
    }
    json!({
        "status": "complete",
        "method_version": FROZEN_METHOD,
        "cases": seeded.len(),
        "by_mutation": tally,
        "totals": totals,
        "reading": "Each keyed figure's outcome under its truth. `changed` is the figure the \
                    change made or unsettled; `untouched` are the other anchored figures in the \
                    same note, which test that an error is placed on the right figure.",
    })
}

/// Every keyed figure's outcome, case by case and in key order. The same
/// reading `score_seeded` tallied, kept per figure so a reconciliation can
/// partition it; the scorer itself is left as it ran. The reconciliation test
/// asserts these reproduce the recorded totals exactly.
pub(crate) fn seeded_readings(seeded: &[SeededCase]) -> Vec<Vec<&'static str>> {
    seeded
        .iter()
        .map(|case| {
            let checks = crate::jev_numeric::numeric_checks(&case.note, &case.evidence);
            let spans: Vec<((usize, usize), &crate::jev_numeric::NumericCheck)> = checks
                .iter()
                .filter_map(|check| {
                    let span = crate::jev_numeric::figure_at(&case.note, check.offset)?;
                    Some(((span.start, span.end), check))
                })
                .collect();
            case.keys
                .iter()
                .map(|key| {
                    let hit = spans
                        .iter()
                        .find(|((start, end), _)| *start < key.end && key.start < *end)
                        .map(|(_, check)| *check);
                    seeded_outcome(
                        key,
                        hit.map(|check| check.verdict.as_str()),
                        hit.and_then(|check| check.field),
                    )
                })
                .collect()
        })
        .collect()
}

/// The recorded run's `totals` table, over the keyed figures `keep` admits.
pub(crate) fn seeded_totals(
    seeded: &[SeededCase],
    readings: &[Vec<&'static str>],
    keep: impl Fn(&SeededCase, &SeededKey) -> bool,
) -> JsonValue {
    let mut totals: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    for (case, outcomes) in seeded.iter().zip(readings) {
        for (key, outcome) in case.keys.iter().zip(outcomes) {
            if !keep(case, key) {
                continue;
            }
            let which = if key.changed { "changed" } else { "untouched" };
            *totals
                .entry(format!("{which}|{}", key.truth))
                .or_default()
                .entry(outcome)
                .or_default() += 1;
        }
    }
    json!(totals)
}

/// What the Markov model builds into its evidence, and the bounds each field
/// keeps, broken in `evidence`.
///
/// `markov_method` computes `signed_signal` as the bull probability less the
/// bear, and `conviction` as its magnitude; the three probabilities sum to
/// one, and `direction` is the sign. All four hold in every one of the 51
/// notes' evidence. A seeded change that moves one of these fields alone
/// leaves evidence the model could not have produced -- competing values for
/// one quantity -- and a ratio of 1.37 can carry a probability-scaled field
/// past one.
///
/// The support's distance and Quiver's direction are not checked: neither
/// relation holds in every original note.
pub(crate) fn context_problems(evidence: &JsonValue) -> Vec<&'static str> {
    let markov = |field: &str| number_at(evidence, &format!("markov.{field}"));
    let mut problems = Vec::new();
    let signed = markov("signed_signal");
    let conviction = markov("conviction");
    let (bull, bear, sideways) = (
        markov("bull_prob"),
        markov("bear_prob"),
        markov("sideways_prob"),
    );
    let close = |left: f64, right: f64| (left - right).abs() <= 1e-6;
    if let (Some(signed), Some(conviction)) = (signed, conviction)
        && !close(conviction, signed.abs())
    {
        problems.push("conviction_is_not_the_signals_magnitude");
    }
    if let (Some(signed), Some(bull), Some(bear)) = (signed, bull, bear)
        && !close(signed, bull - bear)
    {
        problems.push("signal_is_not_bull_less_bear");
    }
    if let (Some(bull), Some(bear), Some(sideways)) = (bull, bear, sideways)
        && !close(bull + bear + sideways, 1.0)
    {
        problems.push("probabilities_do_not_sum_to_one");
    }
    let direction = evidence
        .get("markov")
        .and_then(|markov| markov.get("direction"))
        .and_then(JsonValue::as_str);
    if let (Some(signed), Some(direction)) = (signed, direction) {
        let sign = if signed > 1e-9 {
            "long"
        } else if signed < -1e-9 {
            "short"
        } else {
            "flat"
        };
        if sign != direction {
            problems.push("direction_is_not_the_signals_sign");
        }
    }
    let outside =
        |value: Option<f64>, low: f64, high: f64| value.is_some_and(|v| !(low..=high).contains(&v));
    if [bull, bear, sideways]
        .into_iter()
        .any(|p| outside(p, 0.0, 1.0))
    {
        problems.push("probability_outside_0_to_1");
    }
    if outside(signed, -1.0, 1.0) {
        problems.push("signal_outside_minus_1_to_1");
    }
    if outside(conviction, 0.0, 1.0) {
        problems.push("conviction_outside_0_to_1");
    }
    if outside(
        number_at(evidence, "daily_indicators.support.break_risk"),
        0.0,
        1.0,
    ) {
        problems.push("break_risk_outside_0_to_1");
    }
    if outside(number_at(evidence, "daily_indicators.rsi14"), 0.0, 100.0) {
        problems.push("rsi_outside_0_to_100");
    }
    problems
}

/// Whether a field removed from `evidence` can still be read off the fields
/// the model links to it: the signal as bull less bear, conviction as the
/// signal's magnitude, one probability as one less the other two.
pub(crate) fn derivable_after_removal(evidence: &JsonValue, field: &str) -> bool {
    let has = |name: &str| number_at(evidence, &format!("markov.{name}")).is_some();
    match field {
        "markov.signed_signal" => has("bull_prob") && has("bear_prob"),
        "markov.conviction" => has("signed_signal") || (has("bull_prob") && has("bear_prob")),
        "markov.bull_prob" => has("bear_prob") && (has("sideways_prob") || has("signed_signal")),
        "markov.bear_prob" => has("bull_prob") && (has("sideways_prob") || has("signed_signal")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, report: i64, symbol: &str, note: &str, evidence: JsonValue) -> NoteCase {
        NoteCase {
            id: id.to_string(),
            source_report: report,
            symbol: symbol.to_string(),
            note: note.to_string(),
            evidence,
        }
    }

    fn claim(quote: &str, figure: &str, field: &str, verdict: &str) -> FreshClaim {
        FreshClaim {
            quote: quote.to_string(),
            figure: figure.to_string(),
            field: field.to_string(),
            verdict: verdict.to_string(),
            note: String::new(),
        }
    }

    fn labelled(id: &str, claims: Vec<FreshClaim>) -> FreshLabel {
        FreshLabel {
            id: id.to_string(),
            status: "labelled".to_string(),
            claims,
        }
    }

    fn figure(start: usize, end: usize, field: Option<&str>, verdict: &str) -> Extracted {
        Extracted {
            start,
            end,
            figure: String::new(),
            field: field.map(str::to_string),
            verdict: verdict.to_string(),
        }
    }

    fn evidence() -> JsonValue {
        json!({
            "daily_indicators": {
                "confluence_count": 5,
                "rsi14": 61.34,
                "support": {"nearest_support": 446.0},
            },
            "markov": {"signed_signal": -0.523, "horizon_days": 5},
        })
    }

    #[test]
    fn a_figure_is_found_only_as_a_whole_number_once_in_its_quote() {
        let note = "RSI 61.3, 5 confluences and 15 names";
        assert_eq!(
            figure_span(note, &claim("5 confluences", "5", "x", "consistent")),
            Ok((10, 11))
        );
        assert!(figure_span(note, &claim("RSI 61.3", "1", "x", "consistent")).is_err());
        assert!(figure_span(note, &claim("RSI 61.3", "61", "x", "consistent")).is_err());
        assert_eq!(
            figure_span(note, &claim("RSI 61.3", "61.3", "x", "consistent")),
            Ok((4, 8))
        );
    }

    #[test]
    fn malformed_labels_reject_the_input_whatever_their_order() {
        let cases = vec![case("a", 320, "X", "RSI 61.3, 5 confluences", evidence())];
        let bad = vec![
            labelled(
                "a",
                vec![claim(
                    "RSI 61.3",
                    "61.3",
                    "daily_indicators.rsi",
                    "consistent",
                )],
            ),
            labelled("a", vec![]),
            labelled("zz", vec![]),
            labelled("a", vec![claim("RSI 99", "99", "none", "consistent")]),
            labelled("a", vec![claim("RSI 61.3", "61.3", "none", "maybe")]),
        ];
        let problems = label_problems(&cases, &bad);
        let mut reversed = bad.clone();
        reversed.reverse();
        assert_eq!(problems, label_problems(&cases, &reversed));
        for kind in [
            "field_not_a_number_in_the_evidence",
            "duplicate_ids",
            "unknown_ids",
            "unusable_figure",
            "unreadable_verdict",
        ] {
            assert!(problems.contains_key(kind), "{kind}: {problems:?}");
        }
        let report = score_natural(&cases, &[], &bad);
        assert_eq!(report["status"], "rejected");
    }

    #[test]
    fn the_reading_is_the_extracted_figure_over_the_claims_figure() {
        let extracted = [
            figure(4, 8, Some("daily_indicators.rsi14"), "matches"),
            figure(10, 11, Some("daily_indicators.confluence_count"), "differs"),
            figure(20, 21, None, "unattributed"),
        ];
        assert_eq!(reading((4, 8), &extracted).0, Reading::Accepted);
        assert_eq!(reading((10, 11), &extracted).0, Reading::Flagged);
        assert_eq!(reading((20, 21), &extracted).0, Reading::Abstained);
        assert_eq!(reading((30, 31), &extracted).0, Reading::Omitted);
        // A labeller who writes "0.55" where the scanner read "+0.55" names
        // the same figure.
        assert_eq!(
            reading((5, 9), &[figure(4, 9, None, "matches")]).0,
            Reading::Accepted
        );
    }

    #[test]
    fn note_outcomes_follow_the_labels_and_the_readings() {
        let note = "RSI 61.3, 5 confluences, markov -0.523";
        let case = case("a", 320, "X", note, evidence());
        let key = NoteKey {
            id: "a".to_string(),
            stratum: STRATUM_WITH_FIGURES.to_string(),
            extracted: vec![
                figure(4, 8, Some("daily_indicators.rsi14"), "differs"),
                figure(10, 11, Some("markov.horizon_days"), "matches"),
            ],
        };
        let label = labelled(
            "a",
            vec![
                claim("RSI 61.3", "61.3", "daily_indicators.rsi14", "consistent"),
                claim(
                    "5 confluences",
                    "5",
                    "daily_indicators.confluence_count",
                    "inconsistent",
                ),
                claim(
                    "markov -0.523",
                    "-0.523",
                    "markov.signed_signal",
                    "cannot_tell",
                ),
            ],
        );
        let outcomes = note_outcomes(&case, &key, &label);
        assert_eq!(outcomes["false_alarm"], Binary::Yes);
        assert_eq!(outcomes["accepted_error"], Binary::Yes);
        assert_eq!(outcomes["unflagged_error"], Binary::Yes);
        assert_eq!(outcomes["omission"], Binary::Yes, "the Markov figure");
        assert_eq!(
            outcomes["wrong_field"],
            Binary::Yes,
            "the count read as a horizon"
        );

        let clean = labelled(
            "a",
            vec![claim(
                "markov -0.523",
                "-0.523",
                "markov.signed_signal",
                "cannot_tell",
            )],
        );
        let outcomes = note_outcomes(&case, &key, &clean);
        assert_eq!(outcomes["false_alarm"], Binary::No);
        assert_eq!(outcomes["unflagged_error"], Binary::Unresolved);
    }

    #[test]
    fn nothing_is_scored_while_a_note_is_unlabelled() {
        let cases = vec![
            case("a", 320, "X", "RSI 61.3", evidence()),
            case("b", 320, "Y", "RSI 61.3", evidence()),
        ];
        let keys: Vec<NoteKey> = ["a", "b"]
            .iter()
            .map(|id| NoteKey {
                id: id.to_string(),
                stratum: STRATUM_WITH_FIGURES.to_string(),
                extracted: vec![figure(4, 8, Some("daily_indicators.rsi14"), "matches")],
            })
            .collect();
        let one = vec![labelled(
            "a",
            vec![claim(
                "RSI 61.3",
                "61.3",
                "daily_indicators.rsi14",
                "consistent",
            )],
        )];
        let report = score_natural(&cases, &keys, &one);
        assert_eq!(report["status"], "labelling_incomplete");
        assert!(report["headline"].is_null());
        let both = vec![one[0].clone(), labelled("b", vec![])];
        let report = score_natural(&cases, &keys, &both);
        assert_eq!(report["status"], "complete");
        assert_eq!(
            report["headline"]["notes_the_checker_read"]["claims"]["consistent_flagged"],
            json!([0, 1])
        );
        assert_eq!(
            report["headline"]["notes_the_checker_read"]["notes"]["false_alarm"],
            json!({"notes": [0, 0], "of": 2})
        );
    }

    #[test]
    fn the_keys_arithmetic_accepts_rounding_and_truncation_only() {
        let written = |figure: &str| parse_written(figure).expect("parses");
        assert!(producible(written("0.400"), 0.4006, false), "truncated");
        assert!(producible(written("0.401"), 0.4006, false), "rounded");
        assert!(!producible(written("0.402"), 0.4006, false));
        assert!(producible(written("-0.52"), -0.523, false));
        assert!(
            !producible(written("0.52"), -0.523, false),
            "the sign counts"
        );
        assert!(producible(written("five"), 5.0, true));
        assert!(
            !producible(written("5.0"), 5.0, true),
            "a count has no decimals"
        );
        assert!(parse_written("61%").is_none());
        assert!(parse_written("12,816").is_none());
        assert_eq!(
            write_like(0.717, written("+0.52")).as_deref(),
            Some("+0.72")
        );
        assert_eq!(
            write_like(-0.0001, written("0.00")).as_deref(),
            Some("0.00")
        );
        assert_eq!(write_like(7.0, written("five")).as_deref(), Some("seven"));
        assert_eq!(write_like(13.0, written("five")), None);
    }

    fn seeded_note() -> (Vec<NoteCase>, Vec<FreshLabel>) {
        let mut other = evidence();
        other["markov"]["signed_signal"] = json!(0.231);
        other["daily_indicators"]["support"]["nearest_support"] = json!(90.5);
        let cases = vec![
            case(
                "a",
                320,
                "AAA",
                "Five confluences, support 446.0 EUR, Markov -0.523 over 6 days.",
                evidence(),
            ),
            case("b", 320, "BBB", "Nothing to add.", other),
        ];
        let labels = vec![
            labelled(
                "a",
                vec![
                    claim(
                        "Five confluences",
                        "Five",
                        "daily_indicators.confluence_count",
                        "consistent",
                    ),
                    claim(
                        "support 446.0",
                        "446.0",
                        "daily_indicators.support.nearest_support",
                        "consistent",
                    ),
                    claim(
                        "Markov -0.523",
                        "-0.523",
                        "markov.signed_signal",
                        "consistent",
                    ),
                    claim("6 days", "6", "markov.horizon_days", "inconsistent"),
                ],
            ),
            labelled("b", vec![]),
        ];
        (cases, labels)
    }

    /// Every seeded figure's truth comes from the change and the key's own
    /// arithmetic, and every key points at its figure in the seeded note.
    #[test]
    fn each_change_keys_its_figures_where_they_now_sit() {
        let (cases, labels) = seeded_note();
        let seeded = generate_seeded(&cases, &labels);
        let mutations: BTreeSet<&str> = seeded.iter().map(|case| case.mutation.as_str()).collect();
        for mutation in [
            "note_value_up",
            "note_value_down",
            "sign_flipped",
            "evidence_value_up",
            "evidence_value_down",
            "both_moved",
            "evidence_removed",
            "other_symbol_value",
            "out_of_scope_stop_inserted",
        ] {
            assert!(mutations.contains(mutation), "{mutation}: {mutations:?}");
        }
        for case in &seeded {
            assert_eq!(case.note, case.note.to_lowercase());
            for key in &case.keys {
                assert_eq!(case.note[key.start..key.end], key.figure, "{}", case.id);
                let Some(field) = key.field.as_deref() else {
                    assert_eq!(key.truth, "out_of_scope");
                    continue;
                };
                let written = parse_written(&key.figure).expect("a rewritable figure");
                let stored = number_at(&case.evidence, field);
                match key.truth.as_str() {
                    "holds" => assert!(
                        producible(written, stored.unwrap(), discrete(field)),
                        "{}: {key:?}",
                        case.id
                    ),
                    "fails" => assert!(
                        !producible(written, stored.unwrap(), discrete(field)),
                        "{}: {key:?}",
                        case.id
                    ),
                    "unsettleable" => assert!(stored.is_none(), "{}", case.id),
                    other => panic!("{other}"),
                }
            }
            assert!(
                case.keys.iter().any(|key| key.changed),
                "{}: every case changes something",
                case.id
            );
        }
        // The count is a word, and stays one.
        let up = seeded
            .iter()
            .find(|case| case.id == "a|0|note_value_up")
            .expect("the count moved");
        assert!(up.note.starts_with("seven confluences"), "{}", up.note);
        // Another symbol's signal, copied in.
        let copied = seeded
            .iter()
            .find(|case| case.mutation == "other_symbol_value" && case.id.starts_with("a|2"))
            .expect("the signal copied");
        assert!(copied.note.contains("markov +0.231"), "{}", copied.note);
        // A stop, placed after the support's clause.
        let stop = seeded
            .iter()
            .find(|case| case.mutation == "out_of_scope_stop_inserted")
            .expect("a stop");
        assert!(
            stop.note
                .contains("support 446.0 eur, stop-loss at 432.6, markov"),
            "{}",
            stop.note
        );
        // The horizon the labeller read as wrong, 6 against a stored 5,
        // repaired to 5.
        let repaired = seeded
            .iter()
            .find(|case| case.mutation == "repaired")
            .expect("a repair");
        assert_eq!(repaired.direction, "false_to_true");
        assert!(repaired.note.ends_with("over 5 days."), "{}", repaired.note);
        assert!(
            repaired
                .keys
                .iter()
                .any(|key| key.changed && key.truth == "holds")
        );
    }

    /// A label the arithmetic contradicts is neither an anchor nor a repair.
    #[test]
    fn a_label_the_arithmetic_contradicts_seeds_nothing() {
        let (cases, mut labels) = seeded_note();
        labels[0].claims = vec![
            claim("6 days", "6", "markov.horizon_days", "consistent"),
            claim(
                "Markov -0.523",
                "-0.523",
                "markov.signed_signal",
                "inconsistent",
            ),
        ];
        assert!(generate_seeded(&cases, &labels).is_empty());
    }

    #[test]
    fn generation_is_deterministic_and_never_calls_the_checker() {
        let (cases, labels) = seeded_note();
        let once = serde_json::to_string(&generate_seeded(&cases, &labels)).unwrap();
        let mut shuffled = cases.clone();
        shuffled.reverse();
        let twice = serde_json::to_string(&generate_seeded(&shuffled, &labels)).unwrap();
        assert_eq!(once, twice);
        let source = include_str!("jev_fresh.rs");
        let generator = &source[source.find("struct Anchor {").unwrap()
            ..source.find("pub(crate) fn seeded_outcome").unwrap()];
        assert!(
            !generator.contains("jev_numeric"),
            "the seeded key must not depend on the checker"
        );
    }

    #[test]
    fn an_abstention_is_neither_correct_nor_wrong() {
        let key = |truth: &str| SeededKey {
            start: 0,
            end: 1,
            figure: "5".to_string(),
            field: Some("a.b".to_string()),
            truth: truth.to_string(),
            changed: true,
        };
        assert_eq!(
            seeded_outcome(&key("fails"), Some("differs"), Some("a.b")),
            "correct"
        );
        assert_eq!(
            seeded_outcome(&key("fails"), Some("differs"), Some("c.d")),
            "flagged_wrong_field"
        );
        assert_eq!(
            seeded_outcome(&key("fails"), Some("matches"), Some("c.d")),
            "error_accepted"
        );
        assert_eq!(
            seeded_outcome(&key("fails"), Some("unattributed"), None),
            "abstained"
        );
        assert_eq!(seeded_outcome(&key("fails"), None, None), "abstained");
        assert_eq!(
            seeded_outcome(&key("holds"), Some("differs"), Some("a.b")),
            "false_alarm"
        );
        assert_eq!(
            seeded_outcome(&key("unsettleable"), Some("differs"), Some("c.d")),
            "decided_without_evidence"
        );
        assert_eq!(
            seeded_outcome(&key("unsettleable"), Some("not_in_evidence"), Some("a.b")),
            "correct"
        );
        assert_eq!(
            seeded_outcome(&key("out_of_scope"), Some("differs"), Some("a.b")),
            "misattributed"
        );
    }

    /// The model's own links, checked on evidence it produced and on
    /// evidence a single-field change produced.
    #[test]
    fn a_single_field_change_can_leave_evidence_the_model_cannot_produce() {
        let coherent = json!({
            "markov": {"signed_signal": -0.124, "conviction": 0.124, "bull_prob": 0.2,
                       "bear_prob": 0.324, "sideways_prob": 0.476, "direction": "short"},
            "daily_indicators": {"rsi14": 44.0, "support": {"break_risk": 0.8}},
        });
        assert!(context_problems(&coherent).is_empty());
        let mut moved = coherent.clone();
        moved["markov"]["signed_signal"] = json!(-0.17);
        assert_eq!(
            context_problems(&moved),
            vec![
                "conviction_is_not_the_signals_magnitude",
                "signal_is_not_bull_less_bear"
            ]
        );
        let mut past_one = coherent.clone();
        past_one["daily_indicators"]["support"]["break_risk"] = json!(0.8 * UP);
        assert_eq!(
            context_problems(&past_one),
            vec!["break_risk_outside_0_to_1"]
        );
        let mut removed = coherent.clone();
        set_number(&mut removed, "markov.signed_signal", None);
        assert!(derivable_after_removal(&removed, "markov.signed_signal"));
        assert!(!derivable_after_removal(&removed, "daily_indicators.rsi14"));
    }
}

/// Builds the instrument, the key and an empty labels file from the frame;
/// generates the seeded cases once labels are committed; and scores once.
/// The builders are `#[ignore]`d because they need the report dump.
///
/// ```text
/// JEV_FRESH_PATH=fresh-frame.json cargo test build_the_fresh_instrument -- --ignored
/// cargo test generate_the_seeded_cases -- --ignored
/// cargo test score_the_fresh_set -- --ignored --nocapture
/// ```
#[cfg(test)]
mod frozen {
    use super::*;
    use sha2::Digest;

    const INSTRUMENT_PATH: &str = "docs/jev-fresh-v1.json";
    const KEY_PATH: &str = "docs/jev-fresh-v1-key.json";
    const LABELS_PATH: &str = "docs/jev-fresh-v1-labels.json";
    const SEEDED_PATH: &str = "docs/jev-fresh-v1-seeded.json";
    const RESULTS_PATH: &str = "docs/jev-fresh-v1-results.json";
    const RECONCILIATION_PATH: &str = "docs/jev-fresh-v1-reconciliation.json";

    /// The frame as counted when it was built.
    const FRAME_REPORTS: usize = 11;
    const FRAME_NOTES: usize = 51;

    fn note_id(report: i64, symbol: &str) -> String {
        let digest = sha2::Sha256::digest(format!("{SEED}|{report}|{symbol}").as_bytes());
        format!("fresh-{}", &format!("{digest:x}")[..10])
    }

    fn by_symbol<'a>(prompt: &'a JsonValue, block: &str, symbol: &str) -> Option<&'a JsonValue> {
        prompt
            .get(block)?
            .get("signals")?
            .as_array()?
            .iter()
            .find(|signal| signal.get("symbol").and_then(JsonValue::as_str) == Some(symbol))
    }

    /// The evidence `grading_inputs` would build, for a symbol it never
    /// judged. The same construction as the whole-note audit's.
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
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    fn sha256_of(path: &str) -> String {
        format!(
            "{:x}",
            sha2::Sha256::digest(std::fs::read(path).unwrap_or_default())
        )
    }

    /// The seeded cases the labels generate, as they read back from a file.
    ///
    /// serde_json's default parser does not round-trip every float: it read
    /// 0.46908117592334747 back as 0.4690811759233474. So cases compared
    /// straight from memory never equal the committed ones, though the file is
    /// exactly what was generated. Both sides are compared as read from text.
    fn generated_as_stored(cases: &[NoteCase], labels: &[FreshLabel]) -> JsonValue {
        let text = serde_json::to_string(&generate_seeded(cases, labels)).expect("serialize");
        serde_json::from_str(&text).expect("parse")
    }

    /// Every key's truth, recomputed against the evidence as the file holds
    /// it. A float read back one unit off cannot move a figure across a
    /// rounding boundary without this failing.
    fn keys_hold_as_stored(seeded: &[SeededCase]) -> Vec<String> {
        let mut wrong = Vec::new();
        for case in seeded {
            for key in &case.keys {
                let Some(field) = key.field.as_deref() else {
                    continue;
                };
                if truth_of(&key.figure, field, &case.evidence) != key.truth {
                    wrong.push(format!("{} {}", case.id, key.figure));
                }
            }
        }
        wrong
    }

    fn frozen() -> Option<(Vec<NoteCase>, Vec<NoteKey>, JsonValue)> {
        let instrument = document(INSTRUMENT_PATH)?;
        let key = document(KEY_PATH)?;
        let cases = serde_json::from_value(instrument["cases"].clone()).ok()?;
        let keys = serde_json::from_value(key["keys"].clone()).ok()?;
        Some((cases, keys, key))
    }

    fn labels() -> Option<(Vec<FreshLabel>, JsonValue)> {
        let document = document(LABELS_PATH)?;
        let labels = serde_json::from_value(document["labels"].clone()).ok()?;
        Some((labels, document))
    }

    /// Nothing outside the note text may say what the checker did.
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

    /// The frame is what it says: only reports after the development dump
    /// and up to the freeze, every note in them, each with a key.
    #[test]
    fn the_frame_is_fresh_and_whole() {
        let Some((cases, keys, key)) = frozen() else {
            return;
        };
        assert_eq!(key["method_version"], FROZEN_METHOD);
        assert_eq!(key["protocol"], PROTOCOL_VERSION);
        let reports: BTreeSet<i64> = cases.iter().map(|case| case.source_report).collect();
        assert!(
            reports
                .iter()
                .all(|id| *id > LAST_DEVELOPMENT_REPORT && *id <= LAST_REPORT_BEFORE_FREEZE),
            "{reports:?}"
        );
        assert_eq!(
            key["frame"]["reports"].as_array().map(Vec::len),
            Some(FRAME_REPORTS)
        );
        assert_eq!(cases.len(), FRAME_NOTES, "every note in the frame");
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
        }
    }

    /// The labels are not mine to write. This asserts they are unfilled, or
    /// were filled by someone named, and prints the natural score.
    #[test]
    fn the_labels_are_not_filled_in_by_the_author() {
        let (Some((cases, keys, _)), Some((labels, document))) = (frozen(), labels()) else {
            return;
        };
        let filled = labels
            .iter()
            .any(|label| !label.status.trim().is_empty() || !label.claims.is_empty());
        let by = document["labelled_by"].as_str().unwrap_or_default();
        assert!(
            !filled || !by.trim().is_empty(),
            "labels arrived with nobody named as having made them"
        );
        let report = score_natural(&cases, &keys, &labels);
        assert_ne!(report["status"], "rejected", "{report:#}");
    }

    /// Once the seeded cases exist, they are exactly what the committed
    /// labels generate.
    #[test]
    fn the_seeded_cases_are_what_the_labels_generate() {
        let (Some((cases, _, _)), Some((labels, _)), Some(seeded)) =
            (frozen(), labels(), document(SEEDED_PATH))
        else {
            return;
        };
        assert_eq!(seeded["cases"], generated_as_stored(&cases, &labels));
        let stored: Vec<SeededCase> =
            serde_json::from_value(seeded["cases"].clone()).expect("seeded cases");
        assert_eq!(keys_hold_as_stored(&stored), Vec::<String>::new());
    }

    /// The labels with a reconciliation's readings applied, in memory only.
    fn with_readings(labels: &[FreshLabel], readings: &[JsonValue]) -> Vec<FreshLabel> {
        let mut read = labels.to_vec();
        for reading in readings {
            let (id, index) = reading["claim"]
                .as_str()
                .and_then(|claim| claim.split_once('#'))
                .expect("note-id#index");
            let label = read
                .iter_mut()
                .find(|label| label.id == id)
                .expect("labelled");
            let claim = &mut label.claims[index.parse::<usize>().expect("index")];
            if let Some(verdict) = reading["verdict"].as_str() {
                claim.verdict = verdict.to_string();
            }
            if let Some(field) = reading["field"].as_str() {
                claim.field = field.to_string();
            }
        }
        read
    }

    /// Every figure a reconciliation reports, computed from the frozen
    /// inputs. Nothing in it is typed by hand.
    fn reconciliation_figures(
        cases: &[NoteCase],
        keys: &[NoteKey],
        labels: &[FreshLabel],
        seeded: &[SeededCase],
        reconciliation: &JsonValue,
    ) -> JsonValue {
        let mut natural = serde_json::Map::new();
        natural.insert(
            "as_labelled".to_string(),
            score_natural(cases, keys, labels)["headline"].clone(),
        );
        for scenario in reconciliation["scenarios"].as_array().into_iter().flatten() {
            let readings = scenario["readings"].as_array().expect("readings");
            natural.insert(
                scenario["name"].as_str().expect("name").to_string(),
                score_natural(cases, keys, &with_readings(labels, readings))["headline"].clone(),
            );
        }

        let dispute = &reconciliation["disputed_attribution"]["anchor"];
        let disputed = |case: &SeededCase, key: &SeededKey| {
            case.from_note == dispute["note"].as_str().unwrap_or_default()
                && key.field.as_deref() == dispute["field"].as_str()
        };
        let coherent = |case: &SeededCase| context_problems(&case.evidence).is_empty();
        // The seeded readings are the checker's, and only the frozen method's
        // are the ones this set measured. Under a later method they cannot be
        // recomputed from this tree; check them out at `FROZEN_COMMIT`.
        let frozen = crate::jev_numeric::NUMERIC_METHOD_VERSION == FROZEN_METHOD;
        let seeded_figures = if frozen {
            let readings = seeded_readings(seeded);
            json!({
                "as_scored": seeded_totals(seeded, &readings, |_, _| true),
                "coherent_context_only": seeded_totals(seeded, &readings, |case, _| coherent(case)),
                "without_the_disputed_anchor": seeded_totals(seeded, &readings, |case, key| !disputed(case, key)),
                "coherent_without_the_disputed_anchor": seeded_totals(seeded, &readings, |case, key| {
                    coherent(case) && !disputed(case, key)
                }),
            })
        } else {
            JsonValue::Null
        };

        let mut by_mutation: BTreeMap<&str, (usize, usize, BTreeMap<&str, usize>)> =
            BTreeMap::new();
        let mut derivable: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for case in seeded {
            let problems = context_problems(&case.evidence);
            let entry = by_mutation.entry(case.mutation.as_str()).or_default();
            entry.0 += 1;
            if problems.is_empty() {
                entry.1 += 1;
            }
            for problem in problems {
                *entry.2.entry(problem).or_default() += 1;
            }
            if case.mutation == "evidence_removed" {
                for key in case.keys.iter().filter(|key| key.changed) {
                    let field = key.field.clone().unwrap_or_default();
                    let tally = derivable.entry(field.clone()).or_default();
                    tally.1 += 1;
                    if derivable_after_removal(&case.evidence, &field) {
                        tally.0 += 1;
                    }
                }
            }
        }
        let context: serde_json::Map<String, JsonValue> = by_mutation
            .into_iter()
            .map(|(mutation, (cases, coherent, problems))| {
                (
                    mutation.to_string(),
                    json!({"cases": cases, "coherent": coherent, "problems": problems}),
                )
            })
            .collect();
        let removed: serde_json::Map<String, JsonValue> = derivable
            .into_iter()
            .map(|(field, (derivable, of))| (field, json!([derivable, of])))
            .collect();
        json!({
            "natural": natural,
            "seeded": seeded_figures,
            "context_by_mutation": context,
            "removed_yet_derivable": removed,
        })
    }

    /// The reconciliation sits beside the delivered labels, the seeded cases
    /// and the recorded score, and edits none of them. Every entry quotes the
    /// labels and the key as they are, and every figure is recomputed here.
    #[test]
    fn the_reconciliation_is_beside_the_labels_not_over_them() {
        let (
            Some(reconciliation),
            Some((cases, keys, _)),
            Some((labels, _)),
            Some(seeded),
            Some(results),
        ) = (
            document(RECONCILIATION_PATH),
            frozen(),
            labels(),
            document(SEEDED_PATH),
            document(RESULTS_PATH),
        )
        else {
            return;
        };
        for (field, path) in [
            ("labels_sha256", LABELS_PATH),
            ("seeded_sha256", SEEDED_PATH),
            ("results_sha256", RESULTS_PATH),
            ("instrument_sha256", INSTRUMENT_PATH),
            ("key_sha256", KEY_PATH),
        ] {
            assert_eq!(
                reconciliation["reconciles"][field],
                sha256_of(path),
                "{path} was edited"
            );
        }
        let notes: BTreeMap<&str, &NoteCase> =
            cases.iter().map(|case| (case.id.as_str(), case)).collect();
        let key_by_id: BTreeMap<&str, &NoteKey> =
            keys.iter().map(|key| (key.id.as_str(), key)).collect();
        for entry in reconciliation["claims"].as_array().expect("claims") {
            let reference = entry["claim"].as_str().expect("claim");
            let (id, index) = reference.split_once('#').expect("note-id#index");
            let label = labels
                .iter()
                .find(|label| label.id == id)
                .expect("labelled");
            let claim = &label.claims[index.parse::<usize>().expect("index")];
            assert_eq!(
                entry["quote"], claim.quote,
                "{reference} misquotes the label"
            );
            assert_eq!(entry["figure"], claim.figure, "{reference}");
            assert_eq!(entry["label"]["field"], claim.field, "{reference}");
            assert_eq!(entry["label"]["verdict"], claim.verdict, "{reference}");
            let span = figure_span(&notes[id].note, claim).expect("span");
            let (read, hit) = reading(span, &key_by_id[id].extracted);
            assert_eq!(
                entry["checker"]["reading"],
                read.as_str(),
                "{reference} misquotes the key"
            );
            assert_eq!(
                entry["checker"]["field"],
                json!(hit.and_then(|figure| figure.field.clone())),
                "{reference}"
            );
            for (path, value) in entry["evidence"].as_object().expect("evidence") {
                assert_eq!(
                    number_at(&notes[id].evidence, path),
                    value.as_f64(),
                    "{reference} misquotes the evidence at {path}"
                );
            }
        }
        let seeded_cases: Vec<SeededCase> =
            serde_json::from_value(seeded["cases"].clone()).expect("seeded cases");
        let computed =
            reconciliation_figures(&cases, &keys, &labels, &seeded_cases, &reconciliation);
        println!("{}", serde_json::to_string_pretty(&computed).expect("json"));
        assert_eq!(
            computed["natural"]["as_labelled"], results["natural"]["headline"],
            "the natural headline reproduces the recorded run"
        );
        for part in ["natural", "context_by_mutation", "removed_yet_derivable"] {
            assert_eq!(reconciliation["figures"][part], computed[part], "{part}");
        }
        if computed["seeded"].is_null() {
            println!("The method has moved past {FROZEN_METHOD}: seeded figures not recomputed.");
            return;
        }
        assert_eq!(
            computed["seeded"]["as_scored"], results["seeded"]["totals"],
            "the per-figure reading reproduces the recorded run"
        );
        assert_eq!(reconciliation["figures"], computed);
    }

    #[test]
    #[ignore]
    fn build_the_fresh_instrument() {
        assert_eq!(crate::jev_numeric::NUMERIC_METHOD_VERSION, FROZEN_METHOD);
        let path = std::env::var("JEV_FRESH_PATH").expect("JEV_FRESH_PATH");
        let raw = std::fs::read(&path).expect("the frame dump");
        let sources: Vec<JsonValue> = serde_json::from_slice(&raw).expect("a JSON array");
        let mut reports = Vec::new();
        let mut notes: BTreeMap<String, (NoteCase, NoteKey)> = BTreeMap::new();
        for entry in &sources {
            let report_id = entry.get("id").and_then(JsonValue::as_i64).expect("id");
            assert!(
                report_id > LAST_DEVELOPMENT_REPORT && report_id <= LAST_REPORT_BEFORE_FREEZE,
                "report {report_id} is outside the frame"
            );
            let Some(report) = entry.get("report") else {
                continue;
            };
            reports.push(json!({"id": report_id, "created_at": entry.get("created_at")}));
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
                let id = note_id(report_id, symbol);
                if notes.contains_key(&id) {
                    continue;
                }
                let judged = inputs
                    .candidates
                    .iter()
                    .position(|candidate| candidate["symbol"].as_str() == Some(symbol));
                let lowered = note.to_lowercase();
                let evidence = match judged {
                    Some(index) => inputs.evidence[index].clone(),
                    None => evidence_for(&prompt, symbol),
                };
                // What the method reads in every note, given its evidence --
                // including the notes production never showed it.
                let extracted: Vec<Extracted> =
                    crate::jev_numeric::numeric_checks(&lowered, &evidence)
                        .into_iter()
                        .filter_map(|check| {
                            let span = crate::jev_numeric::figure_at(&lowered, check.offset)?;
                            Some(Extracted {
                                start: span.start,
                                end: span.end,
                                figure: lowered[span.start..span.end].to_string(),
                                field: check.field.map(str::to_string),
                                verdict: check.verdict.as_str().to_string(),
                            })
                        })
                        .collect();
                let stratum = match (judged, extracted.is_empty()) {
                    (None, _) => STRATUM_NEVER_REACHED,
                    (Some(_), false) => STRATUM_WITH_FIGURES,
                    (Some(_), true) => STRATUM_WITHOUT_FIGURES,
                };
                notes.insert(
                    id.clone(),
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
        let mut population: BTreeMap<String, usize> = BTreeMap::new();
        for (_, key) in notes.values() {
            *population.entry(key.stratum.clone()).or_default() += 1;
        }
        // Ordered by id, which is a hash, so reports are interleaved.
        let (cases, keys): (Vec<NoteCase>, Vec<NoteKey>) = notes.into_values().unzip();
        println!(
            "{} reports, {} notes, by stratum {population:?}",
            reports.len(),
            cases.len()
        );
        std::fs::write(
            INSTRUMENT_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_VERSION,
                "what_this_is": "Every note from eleven decision reports, each with the evidence \
                                 the report was given about its symbol. What any checker made of \
                                 them is not here.",
                "how_to_label": {
                    "task": "Read each whole note and list every numeric claim it makes about the \
                             supplied evidence: every figure, in digits or in words, that states \
                             or compares a value the evidence holds. Judge each against the \
                             evidence beside it. Qualitative claims are not asked for.",
                    "quote": "The exact words of the claim from the note, as short as still makes \
                              the claim. It must occur exactly once in the note; lengthen it if \
                              it repeats.",
                    "figure": "The number exactly as the note writes it, as it appears in the \
                               quote: \"0.612\", \"+0.55\", \"61%\", \"5\", \"five\". It must \
                               occur once in the quote as a whole number, not inside a longer \
                               one. One claim per figure: \"4/3 confluences\" is two claims, the \
                               4 and the 3.",
                    "field": "The dotted path of the evidence field the figure states, exactly as \
                              it appears in that note's evidence, such as \
                              \"daily_indicators.rsi14\" or \"markov.signed_signal\". The path \
                              must hold a number. Write \"none\" when no single field states \
                              it: a distance, a ratio, a threshold, or a field the evidence \
                              lacks.",
                    "verdict": "\"consistent\" if the evidence supports the figure at the \
                                precision written, \"inconsistent\" if it contradicts it, \
                                \"cannot_tell\" if two readings are both defensible or the \
                                evidence it needs is missing. Missing evidence is \
                                \"cannot_tell\", never \"inconsistent\".",
                    "note": "Optional: why, for anything uncertain.",
                    "status": "Set \"labelled\" once you have read the whole note, including when \
                               it makes no numeric claim. A note left without it counts as unread.",
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
                "version": FRESH_VERSION,
                "method_version": FROZEN_METHOD,
                "frozen_commit": FROZEN_COMMIT,
                "protocol": PROTOCOL_VERSION,
                "warning": "Do not read this before labelling. It holds every figure the checker \
                            extracted from each note, with its verdict.",
                "strata": "`never_reached`: production never showed the checker this note, \
                           because the candidate had no indicator snapshot or fell past the \
                           question budget. Its `extracted` is what the method reads when given \
                           the note and the same evidence anyway.",
                "frame": {
                    "rule": format!(
                        "Every completed report after #{LAST_DEVELOPMENT_REPORT}, the last in the \
                         development dump, up to and including #{LAST_REPORT_BEFORE_FREEZE}, the \
                         last created before {FROZEN_METHOD} was frozen at {FROZEN_COMMIT}; \
                         every distinct note those reports attached to a selected symbol."
                    ),
                    "dump_sha256": format!("{:x}", sha2::Sha256::digest(&raw)),
                    "reports": reports,
                },
                "population": population,
                "keys": keys,
            }))
            .expect("serialize"),
        )
        .expect("write key");
        if !std::path::Path::new(LABELS_PATH).exists() {
            let template: Vec<FreshLabel> = cases
                .iter()
                .map(|case| FreshLabel {
                    id: case.id.clone(),
                    ..FreshLabel::default()
                })
                .collect();
            std::fs::write(
                LABELS_PATH,
                serde_json::to_string_pretty(&json!({
                    "version": FRESH_VERSION,
                    "labelled_by": "",
                    "labelled_at": "",
                    "labels": template,
                }))
                .expect("serialize"),
            )
            .expect("write labels");
        }
    }

    /// Run once the labels are committed, and before scoring.
    #[test]
    #[ignore]
    fn generate_the_seeded_cases() {
        let (Some((cases, keys, _)), Some((labels, _))) = (frozen(), labels()) else {
            panic!("the instrument and the labels are needed");
        };
        let natural = score_natural(&cases, &keys, &labels);
        assert_eq!(natural["status"], "complete", "{natural:#}");
        let seeded = generate_seeded(&cases, &labels);
        std::fs::write(
            SEEDED_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_VERSION,
                "protocol": PROTOCOL_VERSION,
                "generated_from": {
                    "instrument_sha256": sha256_of(INSTRUMENT_PATH),
                    "labels_sha256": sha256_of(LABELS_PATH),
                },
                "cases": seeded,
            }))
            .expect("serialize"),
        )
        .expect("write seeded");
        println!("{} seeded cases", seeded.len());
    }

    /// The single scoring run. Refuses to overwrite a recorded result.
    #[test]
    #[ignore]
    fn score_the_fresh_set() {
        assert!(
            !std::path::Path::new(RESULTS_PATH).exists(),
            "scored once; the result is recorded"
        );
        let (Some((cases, keys, _)), Some((labels, labels_document)), Some(seeded)) =
            (frozen(), labels(), document(SEEDED_PATH))
        else {
            panic!("the instrument, the labels and the seeded cases are needed");
        };
        let seeded_cases: Vec<SeededCase> =
            serde_json::from_value(seeded["cases"].clone()).expect("seeded cases");
        assert_eq!(
            seeded["cases"],
            generated_as_stored(&cases, &labels),
            "the seeded cases are what the labels generate"
        );
        assert_eq!(
            keys_hold_as_stored(&seeded_cases),
            Vec::<String>::new(),
            "every key holds against the evidence as stored"
        );
        let natural = score_natural(&cases, &keys, &labels);
        let seeded_score = score_seeded(&seeded_cases);
        assert_eq!(natural["status"], "complete", "{natural:#}");
        assert_eq!(seeded_score["status"], "complete", "{seeded_score:#}");
        let result = json!({
            "version": FRESH_VERSION,
            "protocol": PROTOCOL_VERSION,
            "method_version": FROZEN_METHOD,
            "labelled_by": labels_document["labelled_by"],
            "labelled_at": labels_document["labelled_at"],
            "fingerprints": {
                "instrument_sha256": sha256_of(INSTRUMENT_PATH),
                "key_sha256": sha256_of(KEY_PATH),
                "labels_sha256": sha256_of(LABELS_PATH),
                "seeded_sha256": sha256_of(SEEDED_PATH),
            },
            "natural": natural,
            "seeded": seeded_score,
        });
        std::fs::write(
            RESULTS_PATH,
            serde_json::to_string_pretty(&result).expect("serialize"),
        )
        .expect("write results");
        println!("{}", serde_json::to_string_pretty(&result).expect("json"));
    }
}
