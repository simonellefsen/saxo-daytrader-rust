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
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// Version of the comparison method: the parser, the field table, the unit
/// handling and the tolerance policy together.
///
/// Separate from the grading question version on purpose. The arithmetic
/// changed three times under a single `v8` while the worker skipped every
/// report already graded under it, so production held findings from three
/// different algorithms behind one label -- including two that the code had
/// already stopped producing. Recomputing needs no provider call, so a bump
/// here re-derives every stored measurement from the evidence already on disk.
pub(crate) const NUMERIC_METHOD_VERSION: &str = "n6-2026-09-22";

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

/// Comparison words, matched as whole words only.
///
/// Substring matching read the "over" inside "overbought" as a comparison.
const COMPARATOR_ABOVE: &[&str] = &["above", "over", "exceeds", "exceeding", "greater"];
const COMPARATOR_BELOW: &[&str] = &["below", "under", "beneath", "less"];

/// Words that reverse or suspend the claim. Ignoring them accepted "RSI is not
/// above 70" as satisfied by 71.049 -- a false statement recorded as
/// agreement, which is the invisible direction to be wrong in.
const NEGATORS: &[&str] = &[
    "not",
    "no",
    "never",
    "nor",
    "without",
    "isn't",
    "wasn't",
    "longer",
    "n't",
    "fails",
    "failing",
    "false",
    "untrue",
    "incorrect",
    "denies",
    "denied",
    "cannot",
    "can't",
    "doesn't",
    "don't",
    "neither",
    "absent",
    "lacks",
    "lacking",
    "except",
    "excluding",
];

/// Words that put the claim in the past. "RSI was above 70 last week" says
/// nothing about the stored value.
const TEMPORAL: &[&str] = &[
    "was",
    "were",
    "had",
    "previously",
    "earlier",
    "last",
    "formerly",
    "until",
    "since",
    "before",
    "yesterday",
    "recently",
    "once",
];

/// Words that put the claim in the future, or make it conditional on something
/// that has not happened. "RSI is above 70 tomorrow" is a forecast, and the
/// stored value neither confirms nor contradicts it.
const PROSPECTIVE: &[&str] = &[
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "shall",
    "if",
    "unless",
    "when",
    "whenever",
    "assuming",
    "suppose",
    "expect",
    "expects",
    "expected",
    "anticipate",
    "anticipated",
    "projected",
    "projecting",
    "forecast",
    "forecasts",
    "target",
    "targets",
    "targeting",
    "tomorrow",
    "next",
    "upcoming",
    "pending",
    "soon",
    "await",
    "awaiting",
    "hypothetically",
];

/// Words that leave the relation between figure and field unclear. "RSI is
/// overbought at 70" neither asserts equality nor a comparison.
///
/// Local, not sentential: a hedge attaches to the figure beside it, so it is
/// read from the gap between field and figure. "trading near 446 support with
/// RSI at 71.05" hedges the 446 and not the 71.05.
const HEDGES: &[&str] = &[
    "near",
    "around",
    "approximately",
    "roughly",
    "about",
    "overbought",
    "oversold",
    "toward",
    "towards",
    "nearly",
    "almost",
    "circa",
];

/// Words that may stand between a field and its value without changing what is
/// claimed: articles, copulas, prepositions of attribution.
const CONNECTIVES: &[&str] = &[
    "a",
    "an",
    "the",
    "is",
    "are",
    "at",
    "of",
    "with",
    "to",
    "in",
    "on",
    "and",
    "currently",
    "now",
    "sits",
    "sitting",
    "stands",
    "standing",
    "reads",
    "reading",
    "remains",
    "remaining",
    "holds",
    "holding",
    "printing",
    "prints",
    "posting",
    "posts",
    "trading",
    "trades",
    "runs",
    "running",
    "shows",
    "showing",
    "still",
    "just",
    "only",
];

/// Adjectives of degree. They qualify how much, never whether: "long signal is
/// strong at 0.66" claims the same value as "long signal at 0.66".
///
/// Sign words -- "positive", "negative" -- are deliberately absent. They can
/// contradict the figure beside them, and this grammar does not read signs.
const MAGNITUDE_WORDS: &[&str] = &[
    "strong",
    "strongly",
    "weak",
    "weakly",
    "modest",
    "modestly",
    "mild",
    "moderate",
    "solid",
    "firm",
    "deep",
    "slight",
    "slightly",
    "elevated",
    "high",
    "low",
    "decent",
    "healthy",
    "extreme",
    "extremely",
    "very",
];

/// Nouns that continue a field's own name. Derived from the field table so a
/// new `FieldSpec` extends the grammar with it, plus the generic measurement
/// vocabulary notes wrap around a field name.
static FIELD_WORDS: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    let mut set: BTreeSet<&'static str> = FIELDS
        .iter()
        .flat_map(|field| field.keywords.iter())
        .flat_map(|keyword| keyword.split(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect();
    set.extend([
        "signal",
        "signals",
        "signed",
        "long",
        "short",
        "bull",
        "bear",
        "bullish",
        "bearish",
        "prob",
        "probability",
        "state",
        "count",
        "level",
        "levels",
        "value",
        "reading",
        "regime",
        "score",
        "band",
        "zone",
        "trend",
        "price",
        "close",
        "line",
        "ratio",
        "index",
        "measure",
        "metric",
        "figure",
        "continuation",
        "momentum",
        "strength",
        "risk",
    ]);
    set
});

pub(crate) const RELATION_EQUALS: &str = "equals";
pub(crate) const RELATION_ABOVE: &str = "above";
pub(crate) const RELATION_BELOW: &str = "below";
pub(crate) const RELATION_AT_LEAST: &str = "at_least";
pub(crate) const RELATION_AT_MOST: &str = "at_most";
/// The construction is not one this grammar reads, so nothing is compared.
pub(crate) const RELATION_UNSUPPORTED: &str = "unsupported_construction";
/// The figure never reached the grammar: it was not a field value, or no field
/// could be attributed to it. Distinct from `unsupported_construction`, where
/// the field is known and the wording is what could not be read.
pub(crate) const RELATION_NOT_READ: &str = "not_read";

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .collect()
}

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

/// The span of the clause holding the byte at `index`.
///
/// Sentential operators -- negation, tense, conditionals -- scope over a whole
/// clause, so reading them from the gap between field and figure misses every
/// one that sits outside it. "It is false that RSI is above 70" puts the
/// negation before the field; "RSI is above 70 tomorrow" puts the tense after
/// the figure. Neither is visible in the gap.
///
/// A `.` between two digits is a decimal point, not a boundary.
///
/// A comma bounds a clause here. These notes are comma-spliced lists of
/// independent observations -- "currently 9.9% above nearest support, awaiting
/// a tighter entry" states a fact and then an intention -- so letting an
/// operator cross a comma suspends figures it does not govern. The cost is the
/// reverse case: "RSI was, at one point, above 70" hides its tense behind
/// commas and reads as present.
fn clause_bounds(text: &str, index: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    // Walked by character, not by byte. A byte index stepped past a boundary
    // can land inside a multi-byte character, and slicing there panics -- the
    // Unicode minus in "-0.435" reached exactly that.
    let is_boundary = |position: usize, character: char| match character {
        // An em or en dash separates predications the way a semicolon does.
        // The ASCII hyphen does not: it is inside "5-day" and "bull-state".
        ';' | '!' | '?' | ',' | ':' | '\u{2014}' | '\u{2013}' => true,
        '.' => {
            let before_is_digit = position
                .checked_sub(1)
                .is_some_and(|previous| bytes[previous].is_ascii_digit());
            let after_is_digit = bytes.get(position + 1).is_some_and(u8::is_ascii_digit);
            !(before_is_digit && after_is_digit)
        }
        _ => false,
    };
    let (mut start, mut end) = (0, text.len());
    for (position, character) in text.char_indices() {
        if !is_boundary(position, character) {
            continue;
        }
        if position < index {
            start = position + character.len_utf8();
        } else {
            end = position;
            break;
        }
    }
    (start, end.max(start))
}

/// Whether the clause around the figure asserts something in the present,
/// plainly. Anything negated, past or prospective is not a claim about the
/// stored value at all, so no comparison is made.
fn clause_asserts_plainly(text: &str, number_start: usize) -> bool {
    let (start, end) = clause_bounds(text, number_start);
    !words(&text[start..end]).iter().any(|word| {
        NEGATORS.contains(word) || TEMPORAL.contains(word) || PROSPECTIVE.contains(word)
    })
}

/// Reads a symbolic comparison immediately before the figure.
///
/// `words` drops punctuation, so "RSI >= 70" reached the grammar with an empty
/// gap and was read as equality -- reporting a disagreement against 71.049,
/// which satisfies it.
fn symbolic_relation(text: &str, number_start: usize) -> Option<&'static str> {
    let head = text[..number_start].trim_end_matches([' ', '\u{00a0}']);
    for (token, relation) in [
        (">=", RELATION_AT_LEAST),
        ("=>", RELATION_AT_LEAST),
        ("<=", RELATION_AT_MOST),
        ("=<", RELATION_AT_MOST),
        ("\u{2265}", RELATION_AT_LEAST),
        ("\u{2264}", RELATION_AT_MOST),
        (">", RELATION_ABOVE),
        ("<", RELATION_BELOW),
        ("=", RELATION_EQUALS),
    ] {
        if head.ends_with(token) {
            return Some(relation);
        }
    }
    None
}

/// Whether every word may stand between a field and its value without changing
/// what is claimed.
fn gap_is_plain(gap: &[&str]) -> bool {
    gap.iter().all(|word| {
        CONNECTIVES.contains(word) || MAGNITUDE_WORDS.contains(word) || FIELD_WORDS.contains(word)
    })
}

/// Reads the relation a note asserts between a figure and a field, accepting
/// only the constructions enumerated here and abstaining outside them.
///
/// The accepted forms, with the field named before the figure:
///
/// - `field <gap> figure` -- equality, where every word of the gap is a
///   connective, a word of degree, or part of the field's own name.
/// - `field <gap> <comparator> figure` -- `above` or `below`, where the
///   comparator is the word immediately before the figure (allowing a trailing
///   `than`) and the rest of the gap is plain.
/// - `field <gap> <symbol> figure` -- `>`, `>=`, `<`, `<=`, `=` and their
///   Unicode spellings, immediately before the figure.
///
/// And with the field named after the figure, `figure field` is equality:
/// "above 446 EUR support" says 446 *is* the support level. Reading that
/// relationally would ask whether 446 exceeds itself and call a correct note
/// wrong.
///
/// Every form is additionally required to sit in a clause that asserts plainly
/// -- no negation, no past tense, nothing prospective or conditional.
///
/// The known limitation is over-abstention, and it is the safe direction: a
/// sentential operator anywhere in the clause suspends every figure in it,
/// including figures it does not govern. `docs/jev-numeric-grammar.md` records
/// what that costs on the stored corpus.
fn relation_for(text: &str, field_end: Option<usize>, number_start: usize) -> &'static str {
    if !clause_asserts_plainly(text, number_start) {
        return RELATION_UNSUPPORTED;
    }
    let symbolic = symbolic_relation(text, number_start);
    let Some(field_end) = field_end else {
        return symbolic.unwrap_or(RELATION_EQUALS);
    };
    if field_end >= number_start {
        return RELATION_EQUALS;
    }
    let gap = words(&text[field_end..number_start]);
    if let Some(relation) = symbolic {
        return if gap_is_plain(&gap) {
            relation
        } else {
            RELATION_UNSUPPORTED
        };
    }

    let is_comparator =
        |word: &&str| COMPARATOR_ABOVE.contains(word) || COMPARATOR_BELOW.contains(word);
    let comparators = gap.iter().filter(|word| is_comparator(word)).count();
    // "above or below 70" names two relations and asserts neither.
    if comparators > 1 {
        return RELATION_UNSUPPORTED;
    }
    if gap.iter().any(|word| HEDGES.contains(word)) {
        return RELATION_UNSUPPORTED;
    }

    let mut relation = RELATION_EQUALS;
    let mut head = gap.as_slice();
    if comparators == 1 {
        // The comparator has to be the word immediately before the figure. One
        // further back belongs to a construction this grammar does not read.
        if head.last() == Some(&"than") {
            head = &head[..head.len() - 1];
        }
        let Some((last, rest)) = head.split_last() else {
            return RELATION_UNSUPPORTED;
        };
        relation = if COMPARATOR_ABOVE.contains(last) {
            RELATION_ABOVE
        } else if COMPARATOR_BELOW.contains(last) {
            RELATION_BELOW
        } else {
            return RELATION_UNSUPPORTED;
        };
        head = rest;
    }
    if gap_is_plain(head) {
        relation
    } else {
        RELATION_UNSUPPORTED
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
        // The notation is the construction. Leaving the naming phrase of
        // whatever lost the attribution in place would make the grammar read
        // the words between *that* phrase and the figure -- "High conviction
        // setup with 5/3" measured the gap from "conviction", found "setup",
        // and abstained on a reading this rule had just settled.
        phrase_end[index] = None;
        phrase_end[index + 1] = None;
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
            relation: RELATION_NOT_READ,
            verdict: NumericVerdict::NotAFieldValue,
            excerpt,
        };
    }
    let Some(field) = field else {
        return NumericCheck {
            quoted: found.value,
            field: None,
            actual: None,
            relation: RELATION_NOT_READ,
            verdict: NumericVerdict::Unattributed,
            excerpt,
        };
    };
    if uncertain {
        return NumericCheck {
            quoted: found.value,
            field: Some(field.path),
            actual: None,
            relation: RELATION_NOT_READ,
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
    if relation == RELATION_UNSUPPORTED {
        return NumericCheck {
            quoted,
            field: Some(field.path),
            actual: Some(actual),
            relation,
            verdict: NumericVerdict::UncertainAttribution,
            excerpt,
        };
    }
    // An inclusive bound is satisfied at the boundary as well, and the
    // boundary is what the note actually wrote -- so equality there is tested
    // at the precision written, exactly as a bare equality claim is.
    let at_boundary = quoted_from(written, found.decimals, stored, field.discrete);
    let verdict = match relation {
        RELATION_ABOVE if stored > written => NumericVerdict::Matches,
        RELATION_BELOW if stored < written => NumericVerdict::Matches,
        RELATION_AT_LEAST if stored > written || at_boundary => NumericVerdict::Matches,
        RELATION_AT_MOST if stored < written || at_boundary => NumericVerdict::Matches,
        RELATION_ABOVE | RELATION_BELOW | RELATION_AT_LEAST | RELATION_AT_MOST => {
            NumericVerdict::Differs
        }
        _ if at_boundary => NumericVerdict::Matches,
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

    /// Substring matching accepted false statements as agreement. All five of
    /// these returned `above` / `Matches` against an RSI of 71.049.
    #[test]
    fn the_comparison_grammar_refuses_what_it_cannot_read() {
        let ev = json!({"daily_indicators": {"rsi14": 71.04899226674894}});
        let verdict = |note: &str| {
            let c = &numeric_checks(note, &ev)[0];
            (c.relation, c.verdict)
        };

        assert_eq!(
            verdict("RSI is above 70"),
            (RELATION_ABOVE, NumericVerdict::Matches),
            "the supported construction still reads"
        );
        for refused in [
            "RSI is not above 70",
            "RSI is no longer above 70",
            "RSI was above 70 last week",
            "RSI is overbought at 70",
            "RSI is near 70",
        ] {
            assert_eq!(
                verdict(refused),
                (RELATION_UNSUPPORTED, NumericVerdict::UncertainAttribution),
                "{refused}"
            );
        }
    }

    /// Whole words only: "over" inside "overbought" is not a comparison.
    #[test]
    fn a_comparator_inside_a_longer_word_is_not_a_comparison() {
        let ev = json!({"daily_indicators": {"rsi14": 45.0}});
        assert_eq!(
            numeric_checks("RSI 45", &ev)[0].relation,
            RELATION_EQUALS,
            "a plain reading is unaffected"
        );
        // "overbought" must not be read as "over".
        let hedged = &numeric_checks("RSI overbought 45", &ev)[0];
        assert_eq!(hedged.relation, RELATION_UNSUPPORTED);
    }

    /// A comparator that is not the word immediately before the figure belongs
    /// to some other construction, and this grammar does not parse it.
    #[test]
    fn a_comparator_away_from_the_figure_abstains() {
        let ev = json!({"daily_indicators": {"rsi14": 71.049}});
        assert_eq!(
            numeric_checks("RSI above the level we watch of 70", &ev)[0].relation,
            RELATION_UNSUPPORTED
        );
        assert_eq!(
            numeric_checks("RSI greater than 70", &ev)[0].relation,
            RELATION_ABOVE,
            "but an explicit 'greater than' is supported"
        );
    }

    /// An abstention compares nothing, so it can neither agree nor disagree.
    #[test]
    fn an_unsupported_construction_is_never_a_finding() {
        let ev = json!({"daily_indicators": {"rsi14": 20.0}});
        // 71-style claim against a very different value: were this read as
        // equality or as a comparison it would be a disagreement.
        let check = &numeric_checks("RSI is not above 70", &ev)[0];
        assert_eq!(check.verdict, NumericVerdict::UncertainAttribution);
        assert_eq!(summarize(std::slice::from_ref(check))["differs"], 0);
    }
}

#[cfg(test)]
mod grammar_regressions {
    use super::*;
    use serde_json::json;

    fn rsi() -> JsonValue {
        json!({"daily_indicators": {"rsi14": 71.04899226674894}})
    }

    fn reading(text: &str) -> (&'static str, NumericVerdict) {
        let checks = numeric_checks(text, &rsi());
        assert_eq!(checks.len(), 1, "{text:?} -> {checks:?}");
        (checks[0].relation, checks[0].verdict)
    }

    /// Every construction a review found the previous parser reading wrongly.
    ///
    /// The first four are the ones that mattered: each returned a comparison
    /// or an equality against a statement that asserts neither, and three of
    /// the four returned agreement. A false `differs` gets investigated; a
    /// false `matches` never does.
    #[test]
    fn the_constructions_the_word_list_misread_now_abstain() {
        for text in [
            // The negation sits before the field, so the gap between field and
            // figure never saw it.
            "It is false that RSI is above 70",
            "RSI is not above 70",
            "RSI is no longer above 70",
            // The tense sits after the figure, likewise outside the gap.
            "RSI is above 70 tomorrow",
            "RSI was above 70 last week",
            "RSI will be above 70 next week",
            // Two relations asserted at once, of which the old parser took
            // whichever came last.
            "RSI is above or below 70",
            // Not a comparison at all.
            "RSI is overbought at 70",
            // A comparator that is not the word before the figure belongs to
            // some construction this grammar does not read.
            "RSI above the level we watch of 70",
            "RSI is near 70",
        ] {
            let (relation, verdict) = reading(text);
            assert_eq!(relation, RELATION_UNSUPPORTED, "{text:?}");
            assert_eq!(verdict, NumericVerdict::UncertainAttribution, "{text:?}");
        }
    }

    /// Symbolic operators were discarded with the rest of the punctuation, so
    /// "RSI >= 70" arrived with an empty gap and was read as an equality --
    /// reporting a disagreement against 71.049, which satisfies it.
    #[test]
    fn symbolic_operators_are_read_rather_than_discarded() {
        assert_eq!(
            reading("RSI >= 70"),
            (RELATION_AT_LEAST, NumericVerdict::Matches)
        );
        assert_eq!(
            reading("RSI > 70"),
            (RELATION_ABOVE, NumericVerdict::Matches)
        );
        assert_eq!(
            reading("RSI <= 70"),
            (RELATION_AT_MOST, NumericVerdict::Differs)
        );
        assert_eq!(
            reading("RSI < 70"),
            (RELATION_BELOW, NumericVerdict::Differs)
        );
        assert_eq!(
            reading("RSI = 71.05"),
            (RELATION_EQUALS, NumericVerdict::Matches)
        );
        assert_eq!(
            reading("RSI \u{2265} 70"),
            (RELATION_AT_LEAST, NumericVerdict::Matches)
        );

        // An inclusive bound is satisfied at the boundary, where a strict one
        // is not. Folding `>=` into `above` would call a correct note wrong.
        let exact = json!({"daily_indicators": {"rsi14": 70.0}});
        assert_eq!(
            numeric_checks("RSI >= 70", &exact)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("RSI > 70", &exact)[0].verdict,
            NumericVerdict::Differs
        );
    }

    /// The readings the grammar must keep. Abstention is the safe direction,
    /// but a grammar that abstains on everything measures nothing.
    #[test]
    fn the_plain_constructions_are_still_read() {
        for (text, expected) in [
            ("RSI 71.05", RELATION_EQUALS),
            ("RSI at 71.05", RELATION_EQUALS),
            ("RSI is currently 71.05", RELATION_EQUALS),
            ("RSI reads a high 71.05", RELATION_EQUALS),
            ("RSI is above 70", RELATION_ABOVE),
            ("RSI greater than 70", RELATION_ABOVE),
            ("RSI is below 80", RELATION_BELOW),
        ] {
            let (relation, verdict) = reading(text);
            assert_eq!(relation, expected, "{text:?}");
            assert_eq!(verdict, NumericVerdict::Matches, "{text:?}");
        }
    }

    /// A sentential operator suspends its own clause, not the whole note.
    ///
    /// Scoping it to the sentence abstained on "currently 9.9% above nearest
    /// support, awaiting a tighter entry" -- a stated fact followed by an
    /// intention -- because `awaiting` is prospective.
    #[test]
    fn an_operator_in_a_neighbouring_clause_does_not_suspend_the_figure() {
        let evidence = json!({
            "daily_indicators": {
                "rsi14": 71.04899226674894,
                "support": {"nearest_support": 617.21, "downside_to_support_pct": 9.94},
            }
        });
        let checks = numeric_checks(
            "currently 9.9% above nearest support, awaiting a tighter entry",
            &evidence,
        );
        assert_eq!(checks[0].verdict, NumericVerdict::Matches);

        // A conditional in a later clause does not reach back over the
        // semicolon to suspend a figure stated as fact.
        let counted = json!({"daily_indicators": {"confluence_count": 5}});
        let split = numeric_checks(
            "Bullish setup with 5 confluences; monitor for a pullback entry if the regime holds",
            &counted,
        );
        assert_eq!(split[0].relation, RELATION_EQUALS, "{split:?}");
        assert_eq!(split[0].verdict, NumericVerdict::Matches, "{split:?}");

        // And the limitation that buys it: an operator hidden behind commas
        // no longer reaches the figure it governs. Recorded, not fixed.
        let hidden = numeric_checks("RSI was, at one point, above 70", &evidence);
        assert_eq!(hidden[0].relation, RELATION_UNSUPPORTED, "{hidden:?}");
    }

    /// The `N/M` notation settles both figures, so the naming phrase of
    /// whatever lost the attribution must not then be measured against them.
    /// "High conviction setup with 5/3 confluences" gave the 5 to
    /// `conviction`, and the gap from *that* phrase held "setup".
    #[test]
    fn the_count_notation_is_read_whatever_phrase_preceded_it() {
        let evidence = json!({
            "daily_indicators": {"confluence_count": 5, "min_confluences": 3}
        });
        let checks = numeric_checks(
            "High conviction setup with 5/3 confluences and a bullish trend.",
            &evidence,
        );
        for check in checks.iter().take(2) {
            assert_eq!(check.relation, RELATION_EQUALS, "{check:?}");
            assert_eq!(check.verdict, NumericVerdict::Matches, "{check:?}");
        }
    }
}

/// Measurement harness for the comparison grammar, run against the stored
/// corpus of candidate notes.
///
/// `#[ignore]`d because it needs a dump of production notes, which is not in
/// the repository:
///
/// ```text
/// psql -tAc "select jsonb_agg(a->>'notes') from decision_reports r,
///   lateral jsonb_array_elements(r.report_json::jsonb->'selected_assets') a
///   where r.status='completed' and a->>'notes' is not null" > notes.json
/// JEV_NOTES_PATH=notes.json cargo test grammar_coverage -- --ignored --nocapture
/// ```
///
/// It exists because the grammar abstains outside an enumerated set of
/// constructions, and the size of that abstention is the thing to know about
/// it. `docs/jev-numeric-grammar.md` records the figures this prints.
#[cfg(test)]
mod grammar_coverage {
    use super::*;
    use std::collections::BTreeMap;

    /// Why a figure this module attributed was still not compared.
    fn abstention_cause(text: &str, phrase_end: Option<usize>, number_start: usize) -> String {
        if !clause_asserts_plainly(text, number_start) {
            let (start, end) = clause_bounds(text, number_start);
            let token = words(&text[start..end])
                .into_iter()
                .find(|word| {
                    NEGATORS.contains(word) || TEMPORAL.contains(word) || PROSPECTIVE.contains(word)
                })
                .unwrap_or("?");
            return format!("clause:{token}");
        }
        let Some(field_end) = phrase_end else {
            return "none".to_string();
        };
        if field_end >= number_start {
            return "none".to_string();
        }
        let gap = words(&text[field_end..number_start]);
        if let Some(hedge) = gap.iter().find(|word| HEDGES.contains(word)) {
            return format!("hedge:{hedge}");
        }
        let comparators = gap
            .iter()
            .filter(|word| COMPARATOR_ABOVE.contains(word) || COMPARATOR_BELOW.contains(word))
            .count();
        if comparators > 1 {
            return "two_comparators".to_string();
        }
        match gap.iter().find(|word| {
            !(CONNECTIVES.contains(*word)
                || MAGNITUDE_WORDS.contains(*word)
                || FIELD_WORDS.contains(*word)
                || COMPARATOR_ABOVE.contains(*word)
                || COMPARATOR_BELOW.contains(*word))
        }) {
            Some(word) => format!("gap:{word}"),
            None => "comparator_not_adjacent".to_string(),
        }
    }

    #[test]
    #[ignore]
    fn grammar_coverage_on_stored_notes() {
        let path = std::env::var("JEV_NOTES_PATH").expect("JEV_NOTES_PATH");
        let raw = std::fs::read_to_string(path).expect("notes");
        let notes: Vec<String> = serde_json::from_str(&raw).expect("a JSON array of notes");
        let mut relations: BTreeMap<&str, usize> = BTreeMap::new();
        let mut causes: BTreeMap<String, usize> = BTreeMap::new();
        let mut examples: BTreeMap<String, String> = BTreeMap::new();
        let (mut figures, mut attributed) = (0usize, 0usize);
        for note in &notes {
            let lowered = note.to_lowercase();
            let numbers = scan_numbers(&lowered);
            let (fields, uncertain, phrase_end) = attribute_all(&lowered, &numbers);
            for (index, found) in numbers.iter().enumerate() {
                figures += 1;
                if fields[index].is_none()
                    || uncertain[index]
                    || measures_something_else(&lowered, found.end)
                {
                    continue;
                }
                attributed += 1;
                let relation = relation_for(&lowered, phrase_end[index], found.start);
                *relations.entry(relation).or_default() += 1;
                if relation == RELATION_UNSUPPORTED {
                    let cause = abstention_cause(&lowered, phrase_end[index], found.start);
                    let (start, end) = clause_bounds(&lowered, found.start);
                    examples
                        .entry(cause.clone())
                        .or_insert_with(|| lowered[start..end].trim().to_string());
                    *causes.entry(cause).or_default() += 1;
                }
            }
        }
        println!(
            "notes={} figures={figures} attributed={attributed}",
            notes.len()
        );
        println!("relations={relations:?}");
        let mut ranked: Vec<_> = causes.into_iter().collect();
        ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        for (cause, count) in &ranked {
            println!(
                "abstained {count:4}  {cause:28}  {}",
                examples.get(cause).map_or("", String::as_str)
            );
        }
    }
}
