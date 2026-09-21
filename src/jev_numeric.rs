//! Deterministic verification of the numeric assertions in a candidate note.
//!
//! A figure quoted in prose either matches the evidence or it does not, and
//! that is arithmetic. Asking a model to do it costs money, varies between
//! runs, and returns a probability where a comparison would do -- the grader
//! returned `supported` at 0.17 and flipped a `contradicted` to `supported`
//! on a re-ask, both on claims that are simply numbers.
//!
//! So the numbers are checked here and the model is left the semantic
//! judgements it is actually good at: whether "consolidating securely"
//! overstates a break risk the evidence labels *moderate*, or whether
//! "positive Markov state" is fair when the state field reads *Sideways*.
//!
//! Three rules follow from the same absence discipline used elsewhere.
//!
//! A figure this module cannot confidently attribute to a field is recorded
//! `Unattributed`, never compared. A mismatch is a finding about the report,
//! and manufacturing one out of a failed guess at what a number referred to
//! would be worse than staying silent.
//!
//! Tolerance comes from the precision the note used. "0.061" against
//! 0.06147651902511747 agrees, because a figure quoted to three decimals
//! asserts only what three decimals can carry.
//!
//! This module is pure. It reads two JSON values and returns findings; it
//! cannot reach a gate, a queue, an order, or a provider.

use serde_json::Value as JsonValue;

/// Version of the comparison method: the parser, the field table, the unit
/// handling and the tolerance policy together.
///
/// Separate from the grading question version on purpose. The arithmetic
/// changed three times under a single `v8` while the worker skipped every
/// report already graded under it, so production held findings from three
/// different algorithms behind one label -- including two that the code had
/// already stopped producing. Recomputing needs no provider call, so a bump
/// here re-derives every stored measurement from the evidence already on disk.
pub(crate) const NUMERIC_METHOD_VERSION: &str = "n2-2026-09-21";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumericVerdict {
    /// The quoted figure agrees with the evidence at the precision quoted.
    Matches,
    /// Both are present and they disagree.
    Differs,
    /// The figure was attributed to a field the evidence does not carry.
    NotInEvidence,
    /// No field could be identified for this figure.
    Unattributed,
    /// A field was identified but the stored value is orders of magnitude
    /// away, which means the phrase was matched to the wrong figure far more
    /// often than it means the report is wrong.
    ImplausibleAttribution,
}

impl NumericVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Matches => "matches",
            Self::Differs => "differs",
            Self::NotInEvidence => "not_in_evidence",
            Self::Unattributed => "unattributed",
            Self::ImplausibleAttribution => "implausible_attribution",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NumericCheck {
    pub quoted: f64,
    pub field: Option<&'static str>,
    pub actual: Option<f64>,
    pub verdict: NumericVerdict,
    /// The fragment the figure was read from, for adjudication.
    pub excerpt: String,
}

/// How a quoted figure relates to the stored one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// Stored in the same units it is quoted in, and never written as a
    /// percentage -- a price, a count, an index level, a ratio.
    AsQuoted,
    /// Stored in the same units and naturally written with a `%`.
    AsQuotedPercentage,
    /// Stored as a fraction of one; a `%` quote is divided by 100.
    FractionOfOne,
}

impl Unit {
    /// Whether a figure carrying a `%` could be this field.
    ///
    /// "currently 9.9% above nearest support" was attributed to the support
    /// *price* and compared 9.9 against 617.21. A percentage is never a price
    /// or a count, and the suffix says so without any guessing.
    fn admits_percent(self) -> bool {
        !matches!(self, Self::AsQuoted)
    }
}

struct FieldSpec {
    /// Dotted path within one candidate's evidence object.
    path: &'static str,
    /// Whether the field takes only whole values.
    ///
    /// A count is quoted exactly, so "4 confluences" against five is a
    /// disagreement. A continuous quantity is routinely written short --
    /// "295 DKK support" for 295.73302 -- and that is truncation, not
    /// disagreement.
    discrete: bool,
    /// Whether a figure written with an explicit `+` or `-` could be this
    /// field. "low support break risk, and +0.466 Bull Markov regime" gave the
    /// Markov figure to the break risk, because "break risk" sat closer -- but
    /// a break risk is never written signed, and the sign says so.
    signed: bool,
    /// Phrases that identify the field. The longest phrase matched anywhere
    /// wins, so a more specific one beats a more general one that contains it
    /// -- "break risk" must win over "support" in "support break risk".
    keywords: &'static [&'static str],
    unit: Unit,
}

const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        path: "daily_indicators.min_confluences",
        discrete: true,
        signed: false,
        // Reached only through the `N/M` rule below; no note names it directly.
        keywords: &["minimum confluences"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.confluence_count",
        discrete: true,
        signed: false,
        keywords: &["confluences", "confluence"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.support.break_risk",
        discrete: false,
        signed: false,
        keywords: &["support break risk", "break risk", "break-risk"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.reward_risk",
        discrete: false,
        signed: false,
        keywords: &["reward/risk", "reward-risk", "reward risk"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.support.downside_to_support_pct",
        discrete: false,
        signed: false,
        keywords: &[
            "downside to nearest support",
            "downside to support",
            "above nearest support",
            "above support",
            "downside",
        ],
        unit: Unit::AsQuotedPercentage,
    },
    FieldSpec {
        path: "daily_indicators.support.nearest_support",
        discrete: false,
        signed: false,
        keywords: &["support hold above", "nearest support", "support"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.rsi14",
        discrete: false,
        signed: false,
        keywords: &["rsi"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "markov.conviction",
        discrete: false,
        signed: true,
        keywords: &["markov conviction", "regime conviction", "conviction"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "markov.bear_prob",
        discrete: false,
        signed: false,
        keywords: &["bear probability", "bear prob"],
        unit: Unit::FractionOfOne,
    },
    FieldSpec {
        path: "markov.bull_prob",
        discrete: false,
        signed: false,
        keywords: &["bull probability", "bull prob"],
        unit: Unit::FractionOfOne,
    },
    FieldSpec {
        path: "markov.signed_signal",
        discrete: false,
        signed: true,
        keywords: &[
            "markov regime",
            "markov signal",
            "markov score",
            "markov direction",
            "markov",
        ],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "quiver.signal",
        discrete: false,
        signed: true,
        keywords: &[
            "congressional buying signal",
            "congressional buying",
            "congressional selling",
            "congressional",
            "quiver",
        ],
        unit: Unit::AsQuoted,
    },
];

/// How far from a figure a naming phrase may sit and still be its own.
const CONTEXT_CHARS: usize = 40;

/// Checks every numeric assertion in `note` against `evidence`.
pub(crate) fn numeric_checks(note: &str, evidence: &JsonValue) -> Vec<NumericCheck> {
    let lowered = note.to_lowercase();
    let numbers = scan_numbers(&lowered);
    let attributions = attribute_all(&lowered, &numbers);
    numbers
        .iter()
        .zip(attributions)
        .map(|(found, field)| check_one(&lowered, found, field, evidence))
        .collect()
}

struct FoundNumber {
    value: f64,
    decimals: usize,
    percent: bool,
    /// Written with a leading `+` or `-`.
    explicit_sign: bool,
    start: usize,
    end: usize,
}

/// Finds signed decimal figures, hand-rolled because this crate carries no
/// regex dependency and the grammar is small.
fn scan_numbers(text: &str) -> Vec<FoundNumber> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        // A digit inside a word -- "brk-b", "sma200" -- is not a quoted figure.
        let preceded_by_letter = index > 0 && bytes[index - 1].is_ascii_alphabetic();
        let mut start = index;
        let mut sign = 1.0;
        let mut explicit_sign = false;
        if start > 0 && (bytes[start - 1] == b'+' || bytes[start - 1] == b'-') {
            if bytes[start - 1] == b'-' {
                sign = -1.0;
            }
            explicit_sign = true;
            start -= 1;
        } else if let Some(minus_start) = start.checked_sub(3) {
            // U+2212 MINUS SIGN, three bytes. Discarding it read a negative
            // figure as positive and reported a sign error as agreement --
            // silently, and in the direction that hides a disagreement.
            if &bytes[minus_start..start] == "\u{2212}".as_bytes() {
                sign = -1.0;
                explicit_sign = true;
                start = minus_start;
            }
        }
        let digits_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let mut decimals = 0usize;
        if index + 1 < bytes.len() && bytes[index] == b'.' && bytes[index + 1].is_ascii_digit() {
            index += 1;
            let decimal_start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            decimals = index - decimal_start;
        }
        let followed_by_letter = index < bytes.len() && bytes[index].is_ascii_alphabetic();
        let percent = index < bytes.len() && bytes[index] == b'%';
        let end = if percent { index + 1 } else { index };

        if !preceded_by_letter && !followed_by_letter {
            let numeral_end = if percent { end - 1 } else { end };
            if let Ok(magnitude) = text[digits_start..numeral_end].parse::<f64>() {
                found.push(FoundNumber {
                    value: sign * magnitude,
                    decimals,
                    percent,
                    explicit_sign,
                    start,
                    end,
                });
            }
        }
        if percent {
            index = end;
        }
    }
    found
}

/// Pairs naming phrases with figures, closest pair first.
///
/// Neither a symmetric window nor nearest-phrase alone works. A window around
/// "0.061" in "low break risk (0.061), and +0.429 Bull regime conviction"
/// reaches "regime conviction", which is longer than "break risk" and so won,
/// comparing a break risk against a Markov conviction. And in "low break risk
/// (0.061); Markov signal remains neutral/sideways (+0.429)" the phrase
/// "Markov signal" sits nearer to 0.061 than to its own figure.
///
/// Both are fixed by resolving globally rather than per figure: every
/// (phrase, figure) pair is ranked by distance, and the closest pair claims
/// its figure. "break risk" is two characters from 0.061 and takes it, so when
/// "Markov signal" is considered three characters away the figure is spoken
/// for, and the phrase falls to +0.429 instead.
fn attribute_all(text: &str, numbers: &[FoundNumber]) -> Vec<Option<&'static FieldSpec>> {
    struct Pair {
        number: usize,
        /// Where the phrase starts in the text.
        ///
        /// Used as its identity so a region names one figure, not every figure
        /// it happens to sit near: in "-0.900 Markov signal, 42 unrelated" the
        /// phrase belongs to -0.900, and letting it also name 42 compared an
        /// unrelated count against a Markov signal.
        ///
        /// Overlapping matches are one naming, not several: "support break
        /// risk" also matches "break risk" starting eight characters later,
        /// and "markov signal" also matches the bare "markov" at the same
        /// index. Identity is therefore the span, compared by overlap.
        span: (usize, usize),
        distance: usize,
        length: usize,
        field: &'static FieldSpec,
    }

    let mut pairs: Vec<Pair> = Vec::new();
    for field in FIELDS {
        for keyword in field.keywords {
            let mut from = 0usize;
            while let Some(offset) = text[from..].find(keyword) {
                let start = from + offset;
                let end = start + keyword.len();
                from = start + 1;
                for (index, number) in numbers.iter().enumerate() {
                    let distance = if number.end <= start {
                        start - number.end
                    } else if end <= number.start {
                        number.start - end
                    } else {
                        0
                    };
                    // A `%` figure cannot name a price, a count or an index
                    // level, whatever phrase sits beside it.
                    if number.percent && !field.unit.admits_percent() {
                        continue;
                    }
                    if number.explicit_sign && !field.signed {
                        continue;
                    }
                    if distance <= CONTEXT_CHARS {
                        pairs.push(Pair {
                            number: index,
                            span: (start, end),
                            distance,
                            length: keyword.len(),
                            field,
                        });
                    }
                }
            }
        }
    }
    // Closest first; a longer phrase breaks a tie in distance, because a more
    // specific name is the better reading of the same position.
    pairs.sort_by(|left, right| {
        left.distance
            .cmp(&right.distance)
            .then_with(|| right.length.cmp(&left.length))
    });

    let mut assigned: Vec<Option<&'static FieldSpec>> = vec![None; numbers.len()];
    let mut settled = vec![false; numbers.len()];
    let mut consumed: Vec<(usize, usize)> = Vec::new();
    let overlaps = |spans: &[(usize, usize)], span: (usize, usize)| {
        spans
            .iter()
            .any(|(start, end)| span.0 < *end && *start < span.1)
    };
    for pair in &pairs {
        if settled[pair.number] || overlaps(&consumed, pair.span) {
            continue;
        }
        // Two different fields naming the same figure equally well is an
        // ambiguity, and resolving it arbitrarily would be a coin flip
        // presented as arithmetic.
        let ambiguous = pairs.iter().any(|other| {
            other.number == pair.number
                && other.distance == pair.distance
                && other.length == pair.length
                && !std::ptr::eq(other.field, pair.field)
        });
        settled[pair.number] = true;
        consumed.push(pair.span);
        assigned[pair.number] = if ambiguous { None } else { Some(pair.field) };
    }
    // "6/3 technical confluences" is the count over the minimum. Without this
    // the 6 went unattributed and the 3 was compared against the count of 6,
    // reporting a disagreement that is purely notation -- and #279 MU carried
    // exactly that in production.
    for index in 0..numbers.len().saturating_sub(1) {
        let (left, right) = (&numbers[index], &numbers[index + 1]);
        if right.start != left.end + 1 || text.as_bytes().get(left.end) != Some(&b'/') {
            continue;
        }
        let names_confluences = assigned[index]
            .or(assigned[index + 1])
            .is_some_and(|field| field.path.ends_with("confluence_count"));
        if !names_confluences {
            continue;
        }
        assigned[index] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.confluence_count");
        assigned[index + 1] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.min_confluences");
    }
    assigned
}

fn check_one(
    text: &str,
    found: &FoundNumber,
    field: Option<&'static FieldSpec>,
    evidence: &JsonValue,
) -> NumericCheck {
    let window_start = text[..found.start]
        .char_indices()
        .rev()
        .nth(CONTEXT_CHARS)
        .map_or(0, |(offset, _)| offset);
    let window_end = text[found.end..]
        .char_indices()
        .nth(CONTEXT_CHARS)
        .map_or(text.len(), |(offset, _)| found.end + offset);
    let window = &text[window_start..window_end];
    let excerpt = window.trim().to_string();

    let Some(field) = field else {
        return NumericCheck {
            quoted: found.value,
            field: None,
            actual: None,
            verdict: NumericVerdict::Unattributed,
            excerpt,
        };
    };

    // A `%` quote of a value stored as a fraction of one is divided; otherwise
    // the figure is compared in the units it was written in.
    // A `%` quote of a value stored as a fraction of one is compared in the
    // units the note wrote, by scaling the stored value rather than the quote:
    // the convention test has to run at the precision that was actually
    // written.
    let percent_scaled = matches!((field.unit, found.percent), (Unit::FractionOfOne, true));
    let quoted = if percent_scaled {
        found.value / 100.0
    } else {
        found.value
    };

    let Some(actual) = lookup(evidence, field.path) else {
        return NumericCheck {
            quoted,
            field: Some(field.path),
            actual: None,
            verdict: NumericVerdict::NotInEvidence,
            excerpt,
        };
    };

    // `conviction` is the magnitude of the signal, so a note writing "+0.734
    // conviction" is quoting an unsigned quantity with a direction marker.
    let comparable = if field.path == "markov.conviction" {
        quoted.abs()
    } else {
        quoted
    };

    // Test in the units the note wrote them in.
    let (written, stored) = if percent_scaled {
        (found.value.abs(), actual * 100.0)
    } else if field.path == "markov.conviction" {
        (found.value.abs(), actual)
    } else {
        (found.value, actual)
    };

    let verdict = if quoted_from(written, found.decimals, stored, field.discrete) {
        NumericVerdict::Matches
    } else if plausible_magnitude(comparable, actual) {
        NumericVerdict::Differs
    } else {
        NumericVerdict::ImplausibleAttribution
    };
    NumericCheck {
        quoted,
        field: Some(field.path),
        actual: Some(actual),
        verdict,
        excerpt,
    }
}

/// Whether the quoted text could have been produced from the stored value.
///
/// Not a tolerance band. A band of one unit in the last place accepted "392"
/// for a stored 391 and "0.060" for 0.061, neither of which any convention
/// produces -- and on a count it accepted "4.5" against five, though a count
/// is never written with a fraction. Accepting rounding and truncation means
/// testing what each actually yields, not allowing everything between them.
///
/// A discrete field requires a whole number and exact equality.
fn quoted_from(quoted: f64, decimals: usize, actual: f64, discrete: bool) -> bool {
    if discrete {
        return quoted.fract().abs() < f64::EPSILON && (quoted - actual).abs() < 1e-9;
    }
    let scale = 10f64.powi(decimals as i32);
    let scaled = actual * scale;
    let candidates = [
        // Round half away from zero, the common convention.
        scaled.round() / scale,
        // Round half to even, which differs only at an exact half.
        round_half_even(scaled) / scale,
        // Truncate, which is how "295" and "+0.4199" were written.
        scaled.trunc() / scale,
    ];
    let slack = quoted.abs().max(actual.abs()).max(1.0) * 8.0 * f64::EPSILON;
    candidates
        .iter()
        .any(|candidate| (quoted - candidate).abs() <= slack)
}

fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    let fraction = value - floor;
    if (fraction - 0.5).abs() < f64::EPSILON {
        if (floor / 2.0).fract().abs() < f64::EPSILON {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
    }
}

/// Whether a figure is even in the right range to be this field.
///
/// "steady near support with 6 technical confluences" gave the 6 to the
/// support price of 428.11, because "support" sat closer than "confluences".
/// A figure seventy times away from the stored value is a phrase matched to
/// the wrong number far more often than it is a report error, so it is
/// recorded as unchecked rather than as a finding.
fn plausible_magnitude(quoted: f64, actual: f64) -> bool {
    const RATIO: f64 = 10.0;
    /// Above this, a figure is a price, a level or a count rather than a
    /// probability or a signal.
    const UNIT_RANGE: f64 = 1.5;

    let (quoted, actual) = (quoted.abs(), actual.abs());
    // On a field bounded to roughly the unit interval, every in-range figure
    // is a possible value and the ratio says nothing: a break risk of 0.900
    // against a stored 0.061 is fifteen times larger and still a genuine
    // disagreement rather than a phrase matched to the wrong number.
    if quoted.max(actual) <= UNIT_RANGE {
        return true;
    }
    if quoted < f64::EPSILON || actual < f64::EPSILON {
        return true;
    }
    quoted.max(actual) / quoted.min(actual) <= RATIO
}

fn lookup(evidence: &JsonValue, path: &str) -> Option<f64> {
    let mut cursor = evidence;
    for segment in path.split('.') {
        cursor = cursor.get(segment)?;
    }
    cursor.as_f64().filter(|value| value.is_finite())
}

/// Counts for the grade payload.
pub(crate) fn summarize(checks: &[NumericCheck]) -> serde_json::Value {
    let count = |verdict: NumericVerdict| {
        checks
            .iter()
            .filter(|check| check.verdict == verdict)
            .count() as i64
    };
    serde_json::json!({
        "matches": count(NumericVerdict::Matches),
        "differs": count(NumericVerdict::Differs),
        "not_in_evidence": count(NumericVerdict::NotInEvidence),
        "unattributed": count(NumericVerdict::Unattributed),
        "implausible_attribution": count(NumericVerdict::ImplausibleAttribution),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The evidence shape one candidate gets, from a real stored prompt.
    fn evidence() -> JsonValue {
        json!({
            "symbol": "EQNR:xosl",
            "daily_indicators": {
                "close": 421.2,
                "rsi14": 66.6411359502835,
                "reward_risk": 2.9612,
                "confluence_count": 5,
                "support": {
                    "break_risk": 0.06147651902511747,
                    "nearest_support": 391.0,
                    "downside_to_support_pct": 7.169990503323834,
                },
            },
            "markov": {
                "signed_signal": 0.42922624945640564,
                "conviction": 0.42922624945640564,
                "bear_prob": 0.0041,
                "bull_prob": 0.53,
            },
            "quiver": {"signal": -0.4290364384651184},
        })
    }

    fn verdict_for(note: &str, path: &str) -> NumericVerdict {
        numeric_checks(note, &evidence())
            .into_iter()
            .find(|check| check.field == Some(path))
            .unwrap_or_else(|| panic!("no check attributed to {path} in {note:?}"))
            .verdict
    }

    /// The whole point: a figure quoted to three decimals asserts only what
    /// three decimals carry. "0.061" against 0.06147651902511747 agrees, and
    /// the model never has to be asked.
    #[test]
    fn a_figure_agrees_at_the_precision_it_was_quoted_to() {
        assert_eq!(
            verdict_for(
                "Top European energy candidate with 5 technical confluences, 0.061 support \
                 break risk, and fresh +0.429 Bull Markov regime.",
                "daily_indicators.support.break_risk"
            ),
            NumericVerdict::Matches
        );
        // One more decimal than the evidence rounds to is a real disagreement.
        assert_eq!(
            verdict_for(
                "support break risk 0.0620",
                "daily_indicators.support.break_risk"
            ),
            NumericVerdict::Differs
        );
    }

    /// "support break risk" must beat "support", or a probability is compared
    /// against a price and the mismatch is this module's fault, not the
    /// report's.
    #[test]
    fn the_most_specific_phrase_wins_over_one_that_contains_it() {
        let checks = numeric_checks("0.061 support break risk", &evidence());
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].field, Some("daily_indicators.support.break_risk"));
        assert_eq!(checks[0].verdict, NumericVerdict::Matches);

        assert_eq!(
            verdict_for(
                "steady support hold above 391.0 EUR base",
                "daily_indicators.support.nearest_support"
            ),
            NumericVerdict::Matches
        );
        assert_eq!(
            verdict_for(
                "downside to nearest support (6.96%)",
                "daily_indicators.support.downside_to_support_pct"
            ),
            NumericVerdict::Differs,
            "6.96 against a stored 7.17 disagrees at two decimals"
        );
    }

    #[test]
    fn every_markov_phrasing_the_reports_actually_use_is_attributed() {
        for note in [
            "+0.429 Bull Markov regime",
            "+0.429 Markov signal",
            "+0.429 Markov score",
            "+0.429 long Markov direction",
        ] {
            assert_eq!(
                verdict_for(note, "markov.signed_signal"),
                NumericVerdict::Matches,
                "{note}"
            );
        }
        for note in [
            "Markov conviction (+0.429)",
            "+0.429 Bull regime conviction",
        ] {
            assert_eq!(
                verdict_for(note, "markov.conviction"),
                NumericVerdict::Matches,
                "{note}"
            );
        }
    }

    #[test]
    fn a_sign_error_is_a_disagreement_not_a_rounding_difference() {
        assert_eq!(
            verdict_for("-0.429 Markov signal", "markov.signed_signal"),
            NumericVerdict::Differs
        );
        assert_eq!(
            verdict_for("heavy congressional selling (-0.429)", "quiver.signal"),
            NumericVerdict::Matches
        );
        assert_eq!(
            verdict_for("congressional buying (+0.429)", "quiver.signal"),
            NumericVerdict::Differs
        );
    }

    /// A percentage quote of a value stored as a fraction of one is converted;
    /// one stored already as a percentage is not.
    #[test]
    fn percentages_are_converted_only_where_the_stored_value_is_a_fraction() {
        assert_eq!(
            verdict_for("0% bear probability", "markov.bear_prob"),
            NumericVerdict::Matches,
            "0.0041 rounds to 0% at whole-number precision"
        );
        assert_eq!(
            verdict_for("53% bull probability", "markov.bull_prob"),
            NumericVerdict::Matches
        );
        assert_eq!(
            verdict_for(
                "downside to support (7.17%)",
                "daily_indicators.support.downside_to_support_pct"
            ),
            NumericVerdict::Matches,
            "this one is already stored as a percentage"
        );
    }

    /// A figure whose field cannot be identified is never compared. Guessing
    /// would manufacture a finding about the report out of this module's own
    /// mistake.
    #[test]
    fn an_unidentifiable_figure_is_recorded_rather_than_guessed_at() {
        let checks = numeric_checks(
            "initiates as a conservative starter position sized at 6 shares",
            &evidence(),
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, NumericVerdict::Unattributed);
        assert_eq!(checks[0].field, None);
        assert_eq!(checks[0].actual, None);
    }

    /// Two different fields matched by equally specific phrases is an
    /// ambiguity, and resolving it arbitrarily would be a coin flip presented
    /// as arithmetic.
    #[test]
    fn an_ambiguous_attribution_is_left_alone() {
        let checks = numeric_checks("markov 0.429 quiver", &evidence());
        assert_eq!(checks[0].verdict, NumericVerdict::Unattributed);
    }

    /// Identifiers are not measurements.
    #[test]
    fn digits_inside_a_word_are_not_read_as_quoted_figures() {
        for note in ["holds brk-b and sma200", "rated a1 by the agency"] {
            assert!(
                numeric_checks(note, &evidence()).is_empty(),
                "{note} carries no quoted figure"
            );
        }
    }

    /// A field the evidence does not carry is distinguished from one that
    /// disagrees. The first is a coverage gap, the second is a finding.
    #[test]
    fn a_field_absent_from_the_evidence_is_not_reported_as_a_disagreement() {
        let thin = json!({"daily_indicators": {"confluence_count": 5}});
        let checks = numeric_checks("0.061 support break risk", &thin);
        assert_eq!(checks[0].verdict, NumericVerdict::NotInEvidence);
        assert_eq!(checks[0].actual, None);
    }

    /// The fourteen notes production has actually produced, end to end.
    #[test]
    fn the_real_notes_parse_without_a_spurious_disagreement() {
        let notes = [
            "Top technical and Markov setup: 5 BUY confluences, bullish trend bias, 2.96 \
             reward/risk, low break risk (0.061), and +0.429 Bull regime conviction.",
            "5 technical confluences with low break risk (0.061); Markov signal remains \
             neutral/sideways (+0.429), keeping it on close watch.",
            "Top European energy candidate with 5 technical confluences, 0.061 support break \
             risk, and fresh +0.429 Bull Markov regime.",
            "High-conviction Danish healthcare holding leading Markov conviction (+0.429) with \
             solid unrealized profit.",
            "Quality German insurer exhibiting 5 confluences, bullish trend bias, and +0.429 \
             long Markov direction.",
            "Solid technical setup (5 confluences) and +0.429 Markov score, but heavy \
             congressional selling (-0.429) suggests monitoring.",
        ];
        for note in notes {
            let checks = numeric_checks(note, &evidence());
            let differs: Vec<&NumericCheck> = checks
                .iter()
                .filter(|check| check.verdict == NumericVerdict::Differs)
                .collect();
            assert!(
                differs.is_empty(),
                "note agreeing with the evidence must not disagree: {note:?} -> {differs:?}"
            );
            assert!(
                checks
                    .iter()
                    .any(|check| check.verdict == NumericVerdict::Matches),
                "and must verify something: {note:?}"
            );
        }
    }

    #[test]
    fn the_summary_counts_each_verdict() {
        let checks = numeric_checks(
            "5 confluences, 0.061 support break risk, -0.900 Markov signal, 42 unrelated",
            &evidence(),
        );
        let summary = summarize(&checks);
        assert_eq!(summary["matches"], 2);
        assert_eq!(summary["differs"], 1);
        assert_eq!(summary["unattributed"], 1);
    }

    /// Both false positives production produced, as regressions.
    ///
    /// "9.9% above nearest support" was compared against the support price of
    /// 617.21, because the phrase named the price field and the `%` was
    /// ignored. And "23.13" against a stored 23.135 is a legitimate
    /// truncation whose difference computes a hair over an exactly-half tolerance.
    #[test]
    fn the_two_disagreements_production_reported_were_this_modules_own() {
        let support_evidence = json!({
            "daily_indicators": {
                "support": {"nearest_support": 617.21, "downside_to_support_pct": 9.94},
            }
        });
        let checks = numeric_checks(
            "currently 9.9% above nearest support, awaiting a tighter entry",
            &support_evidence,
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(
            checks[0].field,
            Some("daily_indicators.support.downside_to_support_pct"),
            "a percentage is never a price"
        );
        assert_eq!(checks[0].verdict, NumericVerdict::Matches);

        let boundary = json!({
            "daily_indicators": {"support": {"nearest_support": 23.135}}
        });
        assert_eq!(
            numeric_checks("steady support hold above 23.13 EUR base", &boundary)[0].verdict,
            NumericVerdict::Matches,
            "23.13 is a legitimate truncation of 23.135"
        );
    }

    /// The gate must not swallow percentages that genuinely belong to a field.
    #[test]
    fn a_percentage_still_reaches_the_fields_that_can_be_percentages() {
        assert_eq!(
            verdict_for("0% bear probability", "markov.bear_prob"),
            NumericVerdict::Matches
        );
        assert_eq!(
            verdict_for(
                "downside to support (7.17%)",
                "daily_indicators.support.downside_to_support_pct"
            ),
            NumericVerdict::Matches
        );
    }

    /// A price quoted with a `%` beside no percentage-capable field has no
    /// home, and staying silent beats comparing it to a price.
    #[test]
    fn a_percentage_beside_only_price_fields_is_left_unattributed() {
        let checks = numeric_checks("trading 5.0% clear of support", &evidence());
        assert_eq!(checks[0].verdict, NumericVerdict::Unattributed);
    }

    /// The slack is small enough that a real disagreement still registers.
    #[test]
    fn the_boundary_slack_does_not_hide_a_genuine_disagreement() {
        let boundary = json!({
            "daily_indicators": {"support": {"nearest_support": 23.14}}
        });
        assert_eq!(
            numeric_checks("support hold above 23.12 EUR base", &boundary)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// The three production notes whose figures were reported as
    /// disagreements and were all this module's own doing.
    #[test]
    fn the_three_misattributed_production_notes_are_regressions() {
        // 296 FORTUM: "break risk" sat closer to +0.466 than "Markov regime"
        // did, but a break risk is never written with a sign.
        let fortum = json!({
            "daily_indicators": {"support": {"break_risk": 0.2292374167990884}},
            "markov": {"signed_signal": 0.4661},
        });
        let checks = numeric_checks(
            "Utility leader with 5 confluences, low support break risk, and +0.466 Bull \
             Markov regime.",
            &fortum,
        );
        let signed = checks
            .iter()
            .find(|check| (check.quoted - 0.466).abs() < 1e-9)
            .expect("the signed figure");
        assert_eq!(signed.field, Some("markov.signed_signal"));
        assert_eq!(signed.verdict, NumericVerdict::Matches);

        // 295 TSM: "support" sat closer to the 6 than "confluences" did, and
        // the 6 was compared against a price of 428.11.
        let tsm = json!({
            "daily_indicators": {
                "confluence_count": 6,
                "support": {"nearest_support": 428.11, "break_risk": 0.221},
            }
        });
        let checks = numeric_checks(
            "Held semiconductor leader steady near support with 6 technical confluences, low \
             0.221 break risk, and active stop at 415.40 USD.",
            &tsm,
        );
        assert!(
            !checks
                .iter()
                .any(|check| check.verdict == NumericVerdict::Differs),
            "no disagreement should be reported here: {checks:?}"
        );

        // 293 ISP: a truncation, not a disagreement.
        let isp = json!({"markov": {"signed_signal": 0.4199841320514679}});
        assert_eq!(
            numeric_checks("solid +0.4199 Bull Markov signal", &isp)[0].verdict,
            NumericVerdict::Matches
        );
    }

    /// A figure orders of magnitude from the stored value says the phrase was
    /// matched to the wrong number, not that the report is wrong. Recording it
    /// as a disagreement would put a finding on the report for this module's
    /// mistake.
    #[test]
    fn an_implausible_attribution_is_not_reported_as_a_disagreement() {
        let evidence = json!({
            "daily_indicators": {"support": {"nearest_support": 428.11}}
        });
        let checks = numeric_checks("steady near support with 6 shares", &evidence);
        assert_eq!(checks[0].verdict, NumericVerdict::ImplausibleAttribution);
        assert_eq!(summarize(&checks)["differs"], 0);
        assert_eq!(summarize(&checks)["implausible_attribution"], 1);
    }

    /// The guard must not swallow a disagreement of ordinary size.
    #[test]
    fn a_plausible_sized_disagreement_still_registers() {
        let evidence = json!({
            "daily_indicators": {"support": {"break_risk": 0.229}}
        });
        assert_eq!(
            numeric_checks("break risk 0.466", &evidence)[0].verdict,
            NumericVerdict::Differs,
            "twice the stored value is a disagreement, not a misattribution"
        );
    }

    /// Truncation is accepted; a genuinely different figure at the same
    /// precision is not.
    #[test]
    fn the_truncation_tolerance_does_not_accept_a_different_figure() {
        let evidence = json!({"markov": {"signed_signal": 0.4199841320514679}});
        assert_eq!(
            numeric_checks("+0.4199 Markov signal", &evidence)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("+0.4185 Markov signal", &evidence)[0].verdict,
            NumericVerdict::Differs
        );
        // A count keeps half a unit, so an adjacent integer is still wrong.
        let counts = json!({"daily_indicators": {"confluence_count": 5}});
        assert_eq!(
            numeric_checks("4 confluences", &counts)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// An unsigned figure still reaches the signed fields; the gate only
    /// excludes signed figures from fields that are never written signed.
    #[test]
    fn the_sign_gate_only_narrows_figures_that_carry_a_sign() {
        let evidence = json!({"markov": {"signed_signal": 0.4291}});
        assert_eq!(
            numeric_checks("Markov signal of 0.429", &evidence)[0].verdict,
            NumericVerdict::Matches
        );
    }

    /// The guard is a test of whether a figure could belong to a field at all.
    /// On a field bounded to the unit interval every in-range value could, so
    /// the ratio says nothing there -- 0.900 against 0.061 is fifteen times
    /// larger and still a real disagreement about a break risk.
    #[test]
    fn the_magnitude_guard_does_not_apply_inside_the_unit_interval() {
        let evidence = json!({
            "daily_indicators": {"support": {"break_risk": 0.06147651902511747}}
        });
        assert_eq!(
            numeric_checks("0.900 support break risk", &evidence)[0].verdict,
            NumericVerdict::Differs
        );
        assert_eq!(
            numeric_checks("0.001 support break risk", &evidence)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// "295 DKK support" for a stored 295.73302 is truncation. A count is not
    /// written that way, so "4 confluences" against five stays a disagreement.
    #[test]
    fn a_whole_number_truncates_on_a_price_but_not_on_a_count() {
        let price = json!({
            "daily_indicators": {"support": {"nearest_support": 295.73302}}
        });
        assert_eq!(
            numeric_checks("consolidating near 295 DKK support", &price)[0].verdict,
            NumericVerdict::Matches
        );

        let counts = json!({"daily_indicators": {"confluence_count": 5}});
        assert_eq!(
            numeric_checks("4 technical confluences", &counts)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// The first genuine numeric finding production produced: a note calling
    /// the close a support level. 593.0 is the close; support is 551.5.
    #[test]
    fn a_price_named_as_support_that_is_not_the_support_still_disagrees() {
        let fls = json!({
            "daily_indicators": {
                "close": 593.0,
                "confluence_count": 4,
                "support": {"nearest_support": 551.5},
            },
            "markov": {"signed_signal": 0.391},
        });
        let checks = numeric_checks(
            "Danish industrial exhibiting 4 technical confluences and +0.391 Bull Markov \
             reading; consolidating near 593 DKK support.",
            &fls,
        );
        let support = checks
            .iter()
            .find(|check| check.field == Some("daily_indicators.support.nearest_support"))
            .expect("the support claim");
        assert_eq!(support.verdict, NumericVerdict::Differs);
        assert_eq!(support.quoted, 593.0);
        assert_eq!(support.actual, Some(551.5));
        assert!(
            checks
                .iter()
                .filter(|check| check.verdict == NumericVerdict::Differs)
                .count()
                == 1,
            "the confluence count and Markov reading in the same note are correct"
        );
    }

    /// Gaps an isolated probe found after the module already had thirty
    /// passing tests. Each was reachable from wording production had produced
    /// or could produce, and three of the five hid an error rather than
    /// inventing one.
    #[test]
    fn the_probed_parser_and_tolerance_gaps_are_closed() {
        // U+2212 is not ASCII '-'. Discarding it read a negative figure as
        // positive, so a sign error was reported as agreement.
        let mkv = json!({"markov": {"signed_signal": 0.429226}});
        let unicode = &numeric_checks("\u{2212}0.429 Markov signal", &mkv)[0];
        assert_eq!(unicode.quoted, -0.429);
        assert_eq!(unicode.verdict, NumericVerdict::Differs);
        assert_eq!(
            numeric_checks("-0.429 Markov signal", &mkv)[0].verdict,
            NumericVerdict::Differs,
            "the ASCII spelling must behave identically"
        );

        // "6/3 confluences" is the count over the minimum. #279 MU carried
        // this, and the 3 was compared against the count of 6.
        let counts = json!({
            "daily_indicators": {"confluence_count": 6, "min_confluences": 3}
        });
        let checks = numeric_checks("Top-tier setup with 6/3 technical confluences", &counts);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].field, Some("daily_indicators.confluence_count"));
        assert_eq!(checks[0].verdict, NumericVerdict::Matches);
        assert_eq!(checks[1].field, Some("daily_indicators.min_confluences"));
        assert_eq!(checks[1].verdict, NumericVerdict::Matches);

        // A band of one unit accepted values no convention produces.
        let support = json!({"daily_indicators": {"support": {"nearest_support": 391.0}}});
        assert_eq!(
            numeric_checks("support at 392 DKK", &support)[0].verdict,
            NumericVerdict::Differs,
            "neither rounding nor truncating 391 yields 392"
        );
        let risk = json!({"daily_indicators": {"support": {"break_risk": 0.061}}});
        assert_eq!(
            numeric_checks("break risk 0.060", &risk)[0].verdict,
            NumericVerdict::Differs
        );

        // A count is never written with a fraction.
        let five = json!({"daily_indicators": {"confluence_count": 5}});
        assert_eq!(
            numeric_checks("4.5 confluences", &five)[0].verdict,
            NumericVerdict::Differs
        );
        assert_eq!(
            numeric_checks("5 confluences", &five)[0].verdict,
            NumericVerdict::Matches
        );
    }

    /// The convention test has to keep accepting what it was built to accept.
    #[test]
    fn rounding_and_truncating_are_both_still_accepted() {
        let support = json!({"daily_indicators": {"support": {"nearest_support": 295.73302}}});
        assert_eq!(
            numeric_checks("near 295 DKK support", &support)[0].verdict,
            NumericVerdict::Matches,
            "truncated"
        );
        assert_eq!(
            numeric_checks("near 296 DKK support", &support)[0].verdict,
            NumericVerdict::Matches,
            "rounded"
        );
        assert_eq!(
            numeric_checks("near 297 DKK support", &support)[0].verdict,
            NumericVerdict::Differs,
            "and nothing else"
        );

        let markov = json!({"markov": {"signed_signal": 0.4199841320514679}});
        assert_eq!(
            numeric_checks("+0.4199 Markov signal", &markov)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("+0.4200 Markov signal", &markov)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("+0.4198 Markov signal", &markov)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// A percentage of a value stored as a fraction is tested at the precision
    /// the note wrote, not at the stored precision.
    #[test]
    fn a_percentage_quote_is_tested_in_the_units_written() {
        let probs = json!({"markov": {"bear_prob": 0.0041, "bull_prob": 0.5349}});
        assert_eq!(
            numeric_checks("0% bear probability", &probs)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("53% bull probability", &probs)[0].verdict,
            NumericVerdict::Matches,
            "53.49 truncates to 53"
        );
        assert_eq!(
            numeric_checks("55% bull probability", &probs)[0].verdict,
            NumericVerdict::Differs
        );
    }
}
