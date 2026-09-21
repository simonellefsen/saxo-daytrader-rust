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
pub(crate) const NUMERIC_METHOD_VERSION: &str = "n4-2026-09-21";

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
    /// The figure measures something other than a field value -- a horizon in
    /// days, a share count. "5-day Markov continuation signal of 0.6590" had
    /// its 5 compared against the signal.
    NotAFieldValue,
    /// Two fields name the figure nearly equally well. Comparing against
    /// either would be a guess reported as arithmetic.
    UncertainAttribution,
}

impl NumericVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Matches => "matches",
            Self::Differs => "differs",
            Self::NotInEvidence => "not_in_evidence",
            Self::Unattributed => "unattributed",
            Self::ImplausibleAttribution => "implausible_attribution",
            Self::NotAFieldValue => "not_a_field_value",
            Self::UncertainAttribution => "uncertain_attribution",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NumericCheck {
    pub quoted: f64,
    pub field: Option<&'static str>,
    pub actual: Option<f64>,
    /// How the note related the figure to the field: equality unless it wrote
    /// "above" or "below" with the field named first.
    pub relation: &'static str,
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

/// `static`, not `const`. A `const` may be materialised separately at each use
/// site, so identity comparisons between its elements are not dependable --
/// and both the tie check and the abstention check below compare fields.
static FIELDS: &[FieldSpec] = &[
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

/// How much closer the winning phrase must be than the nearest phrase naming a
/// different field.
///
/// Below this the two name the figure nearly equally well, and picking the
/// closer one is a guess presented as arithmetic. "steady near support with 6
/// technical confluences" put "support" six characters from the 6 and
/// "confluences" eleven -- close enough that the wrong one won.
const ATTRIBUTION_MARGIN_CHARS: usize = 6;

/// Words that make a preceding figure a quantity of something rather than the
/// value of a field.
const NON_FIELD_UNITS: &[&str] = &[
    "day", "days", "session", "sessions", "week", "weeks", "month", "months", "hour", "hours",
    "bar", "bars", "year", "years", "share", "shares",
];

/// Words that make a claim relational rather than an equality.
const RELATIONAL_ABOVE: &[&str] = &["above", "over", "exceeds", "exceeding"];
const RELATIONAL_BELOW: &[&str] = &["below", "under", "beneath"];

pub(crate) const RELATION_EQUALS: &str = "equals";
pub(crate) const RELATION_ABOVE: &str = "above";
pub(crate) const RELATION_BELOW: &str = "below";

/// Whether the figure is a quantity of something rather than a field value.
///
/// Looks at what immediately follows: "5-day", "6 shares", "20 sessions".
fn measures_something_else(text: &str, end: usize) -> bool {
    let tail = text[end..].trim_start_matches(['-', ' ', '\u{2011}']);
    NON_FIELD_UNITS.iter().any(|unit| {
        tail.starts_with(unit)
            && tail[unit.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// Reads a relational claim, but only where the field is named *before* the
/// comparison word.
///
/// "RSI is above 70" names the field, then compares: the claim is that rsi14
/// exceeds 70, and 71.049 satisfies it. "above 446 EUR support" is the other
/// order -- there 446 *is* the support level, and equality is the right
/// reading. Getting this backwards turns every true "above" into a
/// disagreement.
fn relation_for(text: &str, field_end: Option<usize>, number_start: usize) -> &'static str {
    let Some(field_end) = field_end else {
        return RELATION_EQUALS;
    };
    if field_end >= number_start {
        return RELATION_EQUALS;
    }
    let between = &text[field_end..number_start];
    if RELATIONAL_ABOVE.iter().any(|word| between.contains(word)) {
        RELATION_ABOVE
    } else if RELATIONAL_BELOW.iter().any(|word| between.contains(word)) {
        RELATION_BELOW
    } else {
        RELATION_EQUALS
    }
}

/// Checks every numeric assertion in `note` against `evidence`.
pub(crate) fn numeric_checks(note: &str, evidence: &JsonValue) -> Vec<NumericCheck> {
    let lowered = note.to_lowercase();
    let numbers = scan_numbers(&lowered);
    let (fields, uncertain, phrase_end) = attribute_all(&lowered, &numbers);
    numbers
        .iter()
        .enumerate()
        .map(|(index, found)| {
            check_one(
                &lowered,
                found,
                fields[index],
                uncertain[index],
                phrase_end[index],
                evidence,
            )
        })
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
        while index < bytes.len() {
            if bytes[index].is_ascii_digit() {
                index += 1;
                continue;
            }
            // A comma between digits, followed by exactly three more, is a
            // group separator: "12,816 DKK" is one figure, and reading it as
            // 12 and 816 leaves two loose numbers able to attach themselves to
            // a field.
            let grouped = bytes[index] == b','
                && index + 3 < bytes.len()
                && bytes[index + 1..index + 4].iter().all(u8::is_ascii_digit)
                && bytes
                    .get(index + 4)
                    .is_none_or(|byte| !byte.is_ascii_digit());
            if grouped {
                index += 4;
                continue;
            }
            break;
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
            if let Ok(magnitude) = text[digits_start..numeral_end]
                .replace(',', "")
                .parse::<f64>()
            {
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
type Attribution = (
    Vec<Option<&'static FieldSpec>>,
    Vec<bool>,
    Vec<Option<usize>>,
);

fn attribute_all(text: &str, numbers: &[FoundNumber]) -> Attribution {
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

    // A quantity of something is not a field value, and must not take part in
    // attribution at all. In "5-day Markov continuation signal of 0.6590" the
    // 5 sits five characters from "markov" and the signal twenty-eight, so the
    // horizon claimed the phrase and the figure it named went unattributed.
    let eligible: Vec<bool> = numbers
        .iter()
        .map(|number| !measures_something_else(text, number.end))
        .collect();

    let mut pairs: Vec<Pair> = Vec::new();
    for field in FIELDS {
        for keyword in field.keywords {
            let mut from = 0usize;
            while let Some(offset) = text[from..].find(keyword) {
                let start = from + offset;
                let end = start + keyword.len();
                from = start + 1;
                for (index, number) in numbers.iter().enumerate() {
                    if !eligible[index] {
                        continue;
                    }
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
    let mut uncertain = vec![false; numbers.len()];
    let mut phrase_end: Vec<Option<usize>> = vec![None; numbers.len()];
    let mut winner: Vec<Option<((usize, usize), usize, &'static str)>> = vec![None; numbers.len()];
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
                && other.field.path != pair.field.path
        });
        // Abstain when another field names the figure nearly as well. Picking
        // the marginally closer one is a guess reported as arithmetic.
        settled[pair.number] = true;
        consumed.push(pair.span);
        winner[pair.number] = Some((pair.span, pair.distance, pair.field.path));
        phrase_end[pair.number] = Some(pair.span.1);
        assigned[pair.number] = if ambiguous { None } else { Some(pair.field) };
    }
    // Uncertainty is judged only once every figure has been assigned.
    //
    // A phrase that another figure ended up using is not available to name
    // this one and cannot make the reading doubtful -- "confluences," sits two
    // characters from the 0.061 in "5 technical confluences, 0.061 support
    // break risk" and would otherwise contest it, although the 5 is what it
    // names. Evaluating this during assignment cannot work: at that moment the
    // phrase has not been claimed yet.
    for (index, win) in winner.iter().enumerate() {
        let Some((win_span, win_distance, win_path)) = *win else {
            continue;
        };
        uncertain[index] = pairs.iter().any(|other| {
            other.number == index
                && other.field.path != win_path
                // The same region read at a different specificity is not a
                // rival: "support" inside "support break risk" at the same
                // distance, where the longer phrase simply wins.
                && !(other.span.0 < win_span.1 && win_span.0 < other.span.1)
                && !overlaps(&consumed, other.span)
                && other.distance.saturating_sub(win_distance) < ATTRIBUTION_MARGIN_CHARS
        });
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
        // Either side naming the count is enough. `a.or(b)` short-circuits on
        // the first `Some`, so when "High conviction setup with 5/3" gave the
        // 5 to markov.conviction, the check read only that and the remap never
        // fired -- leaving the 3 compared against a count of 5.
        let names_confluences = [assigned[index], assigned[index + 1]]
            .iter()
            .flatten()
            .any(|field| field.path.ends_with("confluence_count"));
        if !names_confluences {
            continue;
        }
        // The pattern is unambiguous, so it settles both figures. Leaving an
        // earlier doubt in place would abstain on a reading this rule has
        // just determined.
        assigned[index] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.confluence_count");
        assigned[index + 1] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.min_confluences");
        uncertain[index] = false;
        uncertain[index + 1] = false;
    }
    (assigned, uncertain, phrase_end)
}

fn check_one(
    text: &str,
    found: &FoundNumber,
    field: Option<&'static FieldSpec>,
    uncertain: bool,
    phrase_end: Option<usize>,
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

    // A quantity of something is not the value of a field.
    if measures_something_else(text, found.end) {
        return NumericCheck {
            quoted: found.value,
            field: None,
            actual: None,
            relation: RELATION_EQUALS,
            verdict: NumericVerdict::NotAFieldValue,
            excerpt,
        };
    }
    let Some(field) = field else {
        return NumericCheck {
            quoted: found.value,
            field: None,
            actual: None,
            relation: RELATION_EQUALS,
            verdict: NumericVerdict::Unattributed,
            excerpt,
        };
    };
    if uncertain {
        return NumericCheck {
            quoted: found.value,
            field: Some(field.path),
            actual: None,
            relation: RELATION_EQUALS,
            verdict: NumericVerdict::UncertainAttribution,
            excerpt,
        };
    }
    let relation = relation_for(text, phrase_end, found.start);

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
            relation,
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

    // A relational claim is satisfied by any value on the right side of the
    // threshold, so "RSI is above 70" is true of 71.049 and testing it for
    // equality turns every such note into a disagreement.
    let verdict = match relation {
        RELATION_ABOVE if stored > written => NumericVerdict::Matches,
        RELATION_BELOW if stored < written => NumericVerdict::Matches,
        RELATION_ABOVE | RELATION_BELOW => NumericVerdict::Differs,
        _ if quoted_from(written, found.decimals, stored, field.discrete) => {
            NumericVerdict::Matches
        }
        _ if plausible_magnitude(comparable, actual) => NumericVerdict::Differs,
        _ => NumericVerdict::ImplausibleAttribution,
    };
    NumericCheck {
        quoted,
        field: Some(field.path),
        actual: Some(actual),
        relation,
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
        "not_a_field_value": count(NumericVerdict::NotAFieldValue),
        "uncertain_attribution": count(NumericVerdict::UncertainAttribution),
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
            "initiates as a conservative starter rated 7 overall",
            &evidence(),
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].verdict, NumericVerdict::Unattributed);
        assert_eq!(checks[0].field, None);
        assert_eq!(checks[0].actual, None);

        // "6 shares" is a quantity of something, which is a stronger statement
        // than "no field matched" and is reported as its own outcome.
        assert_eq!(
            numeric_checks("sized at 6 shares", &evidence())[0].verdict,
            NumericVerdict::NotAFieldValue
        );
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
        let checks = numeric_checks("nearest support 6", &evidence);
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

    /// Both notes that survived the n2 recomputation as apparent findings, and
    /// were not.
    ///
    /// `a.or(b)` short-circuits on the first `Some`, so when "High conviction
    /// setup with 5/3" gave the 5 to markov.conviction, the count/minimum
    /// remap read only that side, never fired, and left the 3 compared against
    /// a count of 5. The same happened where "support break-risk" claimed the
    /// 5 first.
    #[test]
    fn the_count_minimum_remap_fires_whichever_side_names_the_count() {
        let evidence = json!({
            "daily_indicators": {
                "confluence_count": 5,
                "min_confluences": 3,
                "support": {"nearest_support": 20.99, "break_risk": 0.25},
            },
            "markov": {"signed_signal": 0.243},
        });

        for note in [
            "High conviction setup with 5/3 confluences and +0.243 Bull Markov signal.",
            "Bullish daily trend, 5/3 confluences, low support break-risk (0.25), and \
             supportive +0.243 Bull Markov signal.",
        ] {
            let checks = numeric_checks(note, &evidence);
            assert!(
                !checks
                    .iter()
                    .any(|check| check.verdict == NumericVerdict::Differs),
                "no disagreement is present in {note:?}: {checks:?}"
            );
            let count = checks
                .iter()
                .find(|check| check.field == Some("daily_indicators.confluence_count"))
                .expect("the count");
            assert_eq!(count.quoted, 5.0);
            assert_eq!(count.verdict, NumericVerdict::Matches);
            let minimum = checks
                .iter()
                .find(|check| check.field == Some("daily_indicators.min_confluences"))
                .expect("the minimum");
            assert_eq!(minimum.quoted, 3.0);
            assert_eq!(minimum.verdict, NumericVerdict::Matches);
        }
    }

    /// "12,816 DKK" is one figure. Read as 12 and 816 it leaves two loose
    /// numbers free to attach themselves to a field.
    #[test]
    fn a_grouped_thousand_is_one_figure() {
        let evidence = json!({"daily_indicators": {"close": 12816.0}});
        let checks = numeric_checks("unit price of ~12,816 DKK exceeds the budget", &evidence);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].quoted, 12816.0);

        let mixed = numeric_checks("available budget of 7,579.89 DKK", &evidence);
        assert_eq!(mixed.len(), 1);
        assert!(
            (mixed[0].quoted - 7579.89).abs() < 1e-9,
            "{}",
            mixed[0].quoted
        );

        // A comma that is not a group separator still ends the figure.
        let listed = numeric_checks("5 confluences, 20 sessions", &evidence);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].quoted, 5.0);
        assert_eq!(listed[1].quoted, 20.0);
    }
}

#[cfg(test)]
mod heldout_regressions {
    use super::*;
    use serde_json::json;

    /// The two disagreements the held-out pass produced, both of which were
    /// this module's own, plus the readings that must not change.
    #[test]
    fn the_held_out_failures_are_fixed_without_breaking_equality_claims() {
        // #117 BAC: "RSI is above 70" against 71.049. A relational claim
        // tested for equality turns every true "above" into a disagreement.
        let bac = json!({"daily_indicators": {"rsi14": 71.04899226674894}});
        let above = &numeric_checks("Keep size modest because RSI is above 70.", &bac)[0];
        assert_eq!(above.field, Some("daily_indicators.rsi14"));
        assert_eq!(above.relation, RELATION_ABOVE);
        assert_eq!(above.verdict, NumericVerdict::Matches);
        // And a relational claim that is actually false still fails.
        let low = json!({"daily_indicators": {"rsi14": 44.8}});
        assert_eq!(
            numeric_checks("RSI is above 70", &low)[0].verdict,
            NumericVerdict::Differs
        );
        assert_eq!(
            numeric_checks("RSI is below 70", &low)[0].verdict,
            NumericVerdict::Matches
        );

        // #185 DSV: the 5 in "5-day" is a horizon, not the signal.
        let dsv = json!({"markov": {"signed_signal": 0.6590335965156555}});
        let checks = numeric_checks(
            "exceptionally strong 5-day Markov continuation signal of 0.6590",
            &dsv,
        );
        let horizon = checks
            .iter()
            .find(|check| check.quoted == 5.0)
            .expect("the horizon");
        assert_eq!(horizon.verdict, NumericVerdict::NotAFieldValue);
        assert!(horizon.field.is_none());
        let signal = checks
            .iter()
            .find(|check| (check.quoted - 0.659).abs() < 1e-9)
            .expect("the signal");
        assert_eq!(signal.verdict, NumericVerdict::Matches);
    }

    /// The field named *after* the figure means the figure is that field's
    /// value, not a threshold on it. Reading "above 446 EUR support"
    /// relationally would ask whether 446 > 446 and call a correct note wrong.
    #[test]
    fn a_figure_the_field_name_follows_is_still_an_equality_claim() {
        let evidence = json!({
            "daily_indicators": {"support": {"nearest_support": 446.0}}
        });
        let check =
            &numeric_checks("consolidating comfortably above 446 EUR support", &evidence)[0];
        assert_eq!(check.relation, RELATION_EQUALS);
        assert_eq!(check.verdict, NumericVerdict::Matches);
    }

    /// Where two fields name a figure nearly equally well, comparing against
    /// either is a guess. "steady near support with 6 technical confluences"
    /// put "support" six characters away and "confluences" eleven.
    #[test]
    fn a_contested_attribution_abstains_instead_of_picking_the_closer_field() {
        let evidence = json!({
            "daily_indicators": {
                "confluence_count": 6,
                "support": {"nearest_support": 428.11},
            }
        });
        let checks = numeric_checks(
            "steady near support with 6 technical confluences",
            &evidence,
        );
        let six = checks
            .iter()
            .find(|check| check.quoted == 6.0)
            .expect("the six");
        assert_eq!(
            six.verdict,
            NumericVerdict::UncertainAttribution,
            "neither field is confidently the right one"
        );
        assert!(
            six.actual.is_none(),
            "an abstention compares nothing, so it reports no stored value"
        );
    }

    /// Abstention must not swallow an unambiguous reading.
    #[test]
    fn an_uncontested_attribution_is_still_compared() {
        let evidence = json!({"daily_indicators": {"confluence_count": 6}});
        assert_eq!(
            numeric_checks("6 technical confluences", &evidence)[0].verdict,
            NumericVerdict::Matches
        );
        let support = json!({"daily_indicators": {"support": {"nearest_support": 446.0}}});
        assert_eq!(
            numeric_checks("nearest support 446.0", &support)[0].verdict,
            NumericVerdict::Matches
        );
    }

    /// Other quantities of things, not just horizons.
    #[test]
    fn share_and_session_counts_are_not_field_values() {
        let evidence = json!({"daily_indicators": {"confluence_count": 5}});
        for note in ["sized at 6 shares", "held for 20 sessions", "over 3 weeks"] {
            assert_eq!(
                numeric_checks(note, &evidence)[0].verdict,
                NumericVerdict::NotAFieldValue,
                "{note}"
            );
        }
    }
}
