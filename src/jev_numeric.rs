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
///
/// `n11` changes none of the parser, field table, units or tolerance. It widens
/// what the evidence reads: a symbol missing from the prompt's compact Markov
/// list is now read from the rows the prompt embedded under
/// `markov_method.latest_run` (`jev_review::embedded_markov_rows`). The results
/// change, so the version does too.
///
/// `n12` reads two names `n11` missed: "R/R" as `reward_risk`, and the Markov
/// horizon written as a duration ("5-day") as `markov.horizon_days`, which the
/// gap grammar also sets aside where it qualifies a Markov field.
///
/// `n13` reads numbers written as words, in two closed forms only: a
/// confluence count ("five-confluence", "Five bullish technical confluences")
/// and the Markov horizon ("five-day"). Every omission the whole-note audit
/// found in a note the checker read was one of these.
///
/// `n14` abstains where `n13` guessed. A figure named ahead of its field was
/// read as the field's exact value whatever came before it, so "more than 5
/// confluences" matched a count of 5 and was flagged against 6 -- in digits
/// since the grammar was written, and in words since `n13`. A quantity lead-in,
/// a range, or an open bound now abstains; "twenty-five" is no longer read as
/// five.
///
/// `n15` reads the whole numeric expression before any part of it. `n14`
/// checked a number word's immediate neighbours, so "one hundred and five-day"
/// and "five point two confluences" still read a five and a two, and a
/// non-breaking hyphen in "twenty-five" hid the compound altogether. The text
/// is now reduced to one spelling of every dash and space first; a range
/// joins number words across any dash; and of the spatial words, only
/// "over", which before a horizon means across, is read as an equality there.
///
/// `n16` matches a field's name only as a whole word. "support" was found
/// inside "supported", and contested the count in "supported by 5
/// confluences": the fresh evaluation's one concrete parser defect.
///
/// `n17` compares by rounding alone. Truncation is no longer accepted: the
/// convention was settled on 2026-09-26, after truncation had decided a case
/// in each of three instruments against a labeller who rounded.
///
/// `n18` holds the rounding allowance at a decimal half to floating-point
/// error. `n17`'s grew with the value, and accepted truncation again for a
/// figure written to about nine decimals or more.
pub(crate) const NUMERIC_METHOD_VERSION: &str = "n18-2026-09-26";

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
    /// Byte offset of the figure in the lowercased note. Not serialized: it
    /// exists so a seeded challenge case can name exactly which figure it
    /// mutated, rather than matching on a value the checker itself reported.
    pub offset: usize,
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
    /// "296 DKK support" for 295.73302 -- and that is rounding, not
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
        keywords: &["support break risk", "break risk"],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "daily_indicators.reward_risk",
        discrete: false,
        signed: false,
        // "R/R 0.75" read as nothing: the separator rule makes it "r r", which
        // no keyword spelled. An initialism names the field only standing
        // alone -- "r r" also occurs inside "higher rsi".
        keywords: &["reward risk", "r/r"],
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
        path: "markov.horizon_days",
        discrete: true,
        signed: false,
        // Reached only through `markov_horizon_unit_end`: a duration is a
        // quantity everywhere else, and no phrase names this field on its own.
        keywords: &[],
        unit: Unit::AsQuoted,
    },
    FieldSpec {
        path: "markov.signed_signal",
        discrete: false,
        signed: true,
        keywords: &[
            // `signed signal` names this field directly and sits right
            // beside its figure. Without it, "markov bull_prob 0.72 /
            // signed_signal 0.61" gave the 0.61 to whichever other phrase was
            // nearest -- and once `bull_prob` started matching, that was
            // `bull_prob`.
            "signed signal",
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
        // A single letter is part of an initialism ("r/r"), not a word that
        // can continue a field's name in a gap.
        .filter(|word| word.len() > 1)
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
    let tail = text[end..].trim_start_matches(['-', ' ']);
    NON_FIELD_UNITS.iter().any(|unit| {
        tail.starts_with(unit)
            && tail[unit.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// Words that, straight after "N-day", say the days are the Markov signal's
/// horizon: "5-day markov signal", "markov 5-day signal", "the 5-day horizon",
/// "5-day signed signal".
const HORIZON_FOLLOWERS: &[&str] = &["horizon", "markov", "signal", "signed"];

/// Where the unit ends, if a figure is the Markov signal's horizon written as
/// a duration.
///
/// Durations are otherwise not field values -- "20 sessions", "6 shares" --
/// and that exclusion is load-bearing: in "5-day Markov continuation signal of
/// 0.6590" it keeps the 5 from taking `markov` away from the signal. This reads
/// the one duration the evidence carries, `markov.horizon_days`, in a closed
/// set of forms, and only where the figure's own clause names a Markov field:
///
/// - `N-day` or `N day(s)` followed by `horizon`, `markov`, `signal` or
///   `signed`;
/// - `<figure> over N days`, the horizon of the figure just quoted.
///
/// A whole, unsigned number only. "50-day SMA" is followed by none of those
/// words, so it stays a quantity. All thirteen durations in the stored notes
/// are the Markov horizon, in seven phrasings; each is one of the forms above.
fn markov_horizon_unit_end(text: &str, number: &FoundNumber) -> Option<usize> {
    if number.decimals != 0 || number.percent || number.explicit_sign || number.value < 1.0 {
        return None;
    }
    let tail = &text[number.end..];
    let joined = tail.trim_start_matches(['-', ' ']);
    let unit = ["days", "day"].into_iter().find(|unit| {
        joined.starts_with(unit)
            && joined[unit.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric())
    })?;
    let unit_end = number.end + (tail.len() - joined.len()) + unit.len();
    let (clause_start, clause_end) = clause_bounds(text, number.start);
    let following = words(&text[unit_end.min(clause_end)..clause_end]);
    let followed = following
        .first()
        .is_some_and(|word| HORIZON_FOLLOWERS.contains(word));
    let clause = with_separators_normalised(&text[clause_start..clause_end]);
    let names_markov = FIELDS
        .iter()
        .zip(NORMALISED_KEYWORDS.iter())
        .filter(|(field, _)| field.path.starts_with("markov."))
        .flat_map(|(_, keywords)| keywords.iter())
        .any(|keyword| {
            clause
                .match_indices(keyword.as_str())
                .any(|(at, _)| names_a_field(&clause, at, at + keyword.len(), keyword))
        });
    ((followed || over_a_figure(text, number)) && names_markov).then_some(unit_end)
}

/// Whether a figure is the N of "<figure> over N days": the horizon of the
/// figure just quoted, where "over" means across.
fn over_a_figure(text: &str, number: &FoundNumber) -> bool {
    let (clause_start, _) = clause_bounds(text, number.start);
    let preceding = words(&text[clause_start.min(number.start)..number.start]);
    matches!(preceding.as_slice(), [.., figure, over]
        if *over == "over" && figure.chars().all(|c| c.is_ascii_digit()))
}

/// The text between a field's name and its figure, with any Markov horizon
/// the checker itself read taken out.
///
/// "markov 5-day signal is 0.560" names which signal: the horizon qualifies
/// the field and claims nothing on its own. Left in, the 5 and the `day` made
/// the gap unreadable and a correct figure went uncompared. Only a horizon
/// `markov_horizon_unit_end` accepts is removed, so nothing else loosens.
fn without_markov_horizons(text: &str, from: usize, to: usize) -> String {
    let mut kept = String::new();
    let mut cursor = from;
    for number in scan_figures(text) {
        if number.start < cursor || number.end > to {
            continue;
        }
        if let Some(unit_end) = markov_horizon_unit_end(text, &number).filter(|end| *end <= to) {
            kept.push_str(&text[cursor..number.start]);
            kept.push(' ');
            cursor = unit_end;
        }
    }
    kept.push_str(&text[cursor..to]);
    kept
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
    let head = text[..number_start].trim_end_matches(' ');
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
        // An approximation marker is a hedge written as punctuation. "rsi ~55"
        // against a stored 61.34 was compared as an exact equality and
        // reported as a disagreement, though the note claims only that it is
        // about 55.
        ("~", RELATION_UNSUPPORTED),
        ("\u{2248}", RELATION_UNSUPPORTED),
    ] {
        if head.ends_with(token) {
            return Some(relation);
        }
    }
    None
}

/// Words that, immediately before a figure named ahead of its field, make it
/// something other than the field's value: "more than 5 confluences", "at
/// least five confluences", "about 5 confluences". The field named after the
/// figure used to mean equality whatever came before it, so each of these was
/// compared as an exact value -- false agreement at 5, a false alarm at 6.
const QUANTITY_LEAD_INS: &[&str] = &[
    "than",
    "least",
    "most",
    "fewer",
    "more",
    "less",
    "beyond",
    "exceeding",
    "exceeds",
    "about",
    "around",
    "approximately",
    "roughly",
    "nearly",
    "almost",
    "circa",
    "some",
];

/// Spatial words before a figure named ahead of its field. For a level they
/// name its position -- "above 446 EUR support" says the support *is* 446, and
/// "near 593 DKK support" is the FLS finding -- so for a level they stay an
/// equality. A count has no position: "over 5 confluences" is a quantity.
const SPATIAL_LEAD_INS: &[&str] = &["above", "below", "over", "under", "beneath", "near"];

/// Words that follow a figure and make it an open bound: "5 or more".
const OPEN_BOUND_FOLLOWERS: &[&str] =
    &["more", "fewer", "less", "higher", "lower", "above", "below"];

/// Whether a word is a numeral: digits, or a number word from zero to ninety.
fn is_numeral_word(word: &str) -> bool {
    matches!(
        numeral_kind(word),
        Numeral::Figure | Numeral::Unit | Numeral::Teen | Numeral::Tens
    )
}

/// Whether `text` begins with a numeral, in digits or in a word.
fn starts_with_numeral(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_digit())
        || words(text)
            .first()
            .is_some_and(|word| text.starts_with(word) && is_numeral_word(word))
}

/// Whether `text` ends with a numeral, in digits or in a word.
fn ends_with_numeral(text: &str) -> bool {
    text.ends_with(|c: char| c.is_ascii_digit())
        || words(text)
            .last()
            .is_some_and(|word| text.ends_with(word) && is_numeral_word(word))
}

/// What follows a dash that could join two ends of a range, if one starts
/// `text`: a hyphen or an en dash, spaced or not, or an em dash written
/// tight. A spaced em dash separates clauses -- "RSI 60 — 5 confluences" --
/// and joins nothing.
fn past_a_joining_dash(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix('\u{2014}') {
        return Some(rest);
    }
    let spaced = text.trim_start_matches(' ');
    spaced
        .strip_prefix('-')
        .or_else(|| spaced.strip_prefix('\u{2013}'))
        .map(|rest| rest.trim_start_matches(' '))
}

/// The mirror of `past_a_joining_dash`, for what precedes a figure.
fn before_a_joining_dash(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_suffix('\u{2014}') {
        return Some(rest);
    }
    let spaced = text.trim_end_matches(' ');
    spaced
        .strip_suffix('-')
        .or_else(|| spaced.strip_suffix('\u{2013}'))
        .map(|rest| rest.trim_end_matches(' '))
}

/// Whether a figure is one end of a range, or an open bound: "5 to 6",
/// "5-6", "five or six", "five–six", "between four and six", "5 or more",
/// "5+". A range states no single value, so reading either end as exact is a
/// guess.
///
/// A dash joins numerals written in words as well as in digits. `n14` read
/// only a digit across it, and the en dash is also a clause boundary, so in
/// "five–six confluences" the five was never seen and the six was compared as
/// an exact count.
fn in_a_range(text: &str, found: &FoundNumber) -> bool {
    let (clause_start, clause_end) = clause_bounds(text, found.start);
    let clause_end = clause_end.max(found.end);
    let after = &text[found.end..];
    let joined_after = past_a_joining_dash(after).is_some_and(starts_with_numeral);
    let before = &text[..found.start];
    let joined_before = if found.explicit_sign {
        // The scanner read "5-6" as 5 and -6; the "-" is the join.
        ends_with_numeral(before.trim_end_matches(' '))
    } else {
        before_a_joining_dash(before).is_some_and(ends_with_numeral)
    };
    if joined_after || joined_before || after.trim_start_matches(' ').starts_with('+') {
        return true;
    }
    let following = words(&text[found.end.min(clause_end)..clause_end]);
    let preceding = words(&text[clause_start.min(found.start)..found.start]);
    let range_after = matches!(following.as_slice(), [join, next, ..]
        if matches!(*join, "to" | "or") && (is_numeral_word(next) || OPEN_BOUND_FOLLOWERS.contains(next)));
    let plus_after = following.first() == Some(&"plus");
    // "and" joins a range only after "between": in "signed_signal 0.5555 and
    // 4/3 confluences" it joins two fields' figures, not the ends of a range.
    let range_before = matches!(preceding.as_slice(), [.., number, join]
        if matches!(*join, "to" | "or") && is_numeral_word(number))
        || matches!(preceding.as_slice(), [.., between, number, join]
            if *between == "between" && *join == "and" && is_numeral_word(number));
    range_after || plus_after || range_before
}

/// Whether an en or em dash touches the figure's first digit: "–0.435". It
/// is the far end of a range or a minus sign, and neither is read as the
/// figure's value. The scanner reads only `-` as a sign, so this figure
/// arrived unsigned.
fn touched_by_a_dash(text: &str, found: &FoundNumber) -> bool {
    !found.word && !found.explicit_sign && text[..found.start].ends_with(['\u{2013}', '\u{2014}'])
}

/// Whether a figure named ahead of its field is led in by a word this grammar
/// does not read.
fn led_in_unreadably(text: &str, found: &FoundNumber, field: &FieldSpec) -> bool {
    let (clause_start, _) = clause_bounds(text, found.start);
    let preceding = words(&text[clause_start.min(found.start)..found.start]);
    let Some(last) = preceding.last() else {
        return false;
    };
    let up_to = preceding.len() >= 2 && preceding[preceding.len() - 2] == "up" && *last == "to";
    // Before a horizon, "over" means across, not more than: "-0.5230 over 5
    // days", "dominance over 5-day horizon". That word only. `n14` exempted
    // the horizon from every spatial word, so "under five-day Markov horizon"
    // matched a five-day horizon.
    let across = field.path == "markov.horizon_days" && *last == "over";
    let spatial_matters = field.discrete && !across;
    QUANTITY_LEAD_INS.contains(last)
        || up_to
        || (spatial_matters && SPATIAL_LEAD_INS.contains(last))
}

/// The relation read for one figure: the grammar's reading, unless the figure
/// sits in a range, carries a sign it cannot read, or is named ahead of its
/// field behind a lead-in the grammar does not read. Each is an abstention,
/// not a guess.
fn relation_of(
    text: &str,
    found: &FoundNumber,
    field: &FieldSpec,
    field_end: Option<usize>,
) -> &'static str {
    if in_a_range(text, found) || touched_by_a_dash(text, found) {
        return RELATION_UNSUPPORTED;
    }
    let named_after = field_end.is_none_or(|end| end >= found.start);
    if named_after && led_in_unreadably(text, found, field) {
        return RELATION_UNSUPPORTED;
    }
    relation_for(text, field_end, found.start)
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
    let between = without_markov_horizons(text, field_end, number_start);
    let gap = words(&between);
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

/// Where a figure sits in the text, and how it was written.
///
/// Exposed for the seeded challenge set, which rewrites exactly the figure a
/// case names rather than searching for it. Test-only: nothing in the runtime
/// needs to edit a note.
#[cfg(test)]
pub(crate) struct FigureSpan {
    pub start: usize,
    pub end: usize,
    pub decimals: usize,
    pub percent: bool,
    pub explicit_sign: bool,
}

#[cfg(test)]
pub(crate) fn figure_at(text: &str, offset: usize) -> Option<FigureSpan> {
    let canonical = Canonical::of(text);
    scan_figures(&canonical.text).into_iter().find_map(|found| {
        let (start, end) = canonical.in_note(found.start, found.end);
        (start == offset).then_some(FigureSpan {
            start,
            end,
            decimals: found.decimals,
            percent: found.percent,
            explicit_sign: found.explicit_sign,
        })
    })
}

/// The clause around a byte offset, for a challenge case that negates or
/// tenses exactly the predication its figure sits in.
#[cfg(test)]
pub(crate) fn clause_span(text: &str, offset: usize) -> (usize, usize) {
    clause_bounds(text, offset)
}

/// Whether a field takes only whole values, so a challenge case does not seed
/// a fractional count.
#[cfg(test)]
pub(crate) fn field_is_discrete(path: &str) -> bool {
    FIELDS
        .iter()
        .find(|field| field.path == path)
        .is_some_and(|field| field.discrete)
}

/// Checks every numeric assertion in `note` against `evidence`.
pub(crate) fn numeric_checks(note: &str, evidence: &JsonValue) -> Vec<NumericCheck> {
    let lowered = note.to_lowercase();
    let canonical = Canonical::of(&lowered);
    let text = canonical.text.as_str();
    let numbers = scan_figures(text);
    let (fields, uncertain, phrase_end) = attribute_all(text, &numbers);
    numbers
        .iter()
        .enumerate()
        .map(|(index, found)| {
            let (start, end) = canonical.in_note(found.start, found.end);
            let placement = Placement {
                offset: start,
                excerpt: excerpt_around(&lowered, start, end),
            };
            check_one(
                text,
                found,
                (fields[index], uncertain[index], phrase_end[index]),
                evidence,
                placement,
            )
        })
        .collect()
}

/// The note as the grammar reads it, with one spelling for every character
/// the notes write in several, and where each byte came from.
///
/// Review found a construction read two ways by spelling alone:
/// "twenty-five-day Markov signal" was a compound, and with a non-breaking
/// hyphen it was a five. Each helper kept its own list of the characters it
/// trimmed -- one knew the non-breaking hyphen, one the non-breaking space,
/// one the Unicode minus -- and no two lists agreed. Reducing the text once,
/// before anything reads it, gives every rule the same spelling.
///
/// The en and em dashes keep their own spellings. They bound clauses, and a
/// hyphen does not: it sits inside "5-day" and "bull-state".
struct Canonical {
    text: String,
    /// For each byte of `text`, the span of the note's character it came from.
    origin: Vec<(usize, usize)>,
    note_len: usize,
}

impl Canonical {
    fn of(note: &str) -> Self {
        let mut text = String::with_capacity(note.len());
        let mut origin = Vec::with_capacity(note.len());
        for (start, character) in note.char_indices() {
            let Some(written) = canonical_char(character) else {
                continue;
            };
            let before = text.len();
            text.push(written);
            let span = (start, start + character.len_utf8());
            origin.extend(std::iter::repeat_n(span, text.len() - before));
        }
        Self {
            text,
            origin,
            note_len: note.len(),
        }
    }

    /// Where `start..end` of the canonical text sits in the note. Offsets are
    /// reported in the note, so a figure is found in the text a reader has.
    fn in_note(&self, start: usize, end: usize) -> (usize, usize) {
        let from = self.origin.get(start).map_or(self.note_len, |span| span.0);
        let to = end
            .checked_sub(1)
            .filter(|last| *last >= start)
            .and_then(|last| self.origin.get(last))
            .map_or(from, |span| span.1);
        (from, to.max(from))
    }
}

/// The one spelling the grammar reads a character in, or `None` for a
/// character that writes nothing.
fn canonical_char(character: char) -> Option<char> {
    Some(match character {
        // Hyphens, the minus sign and the figure dash: one short stroke.
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2212}' | '\u{fe63}' | '\u{ff0d}' => '-',
        // The horizontal bar is written as an em dash.
        '\u{2015}' => '\u{2014}',
        // Every other horizontal space.
        '\t'
        | '\u{00a0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200a}'
        | '\u{202f}'
        | '\u{205f}'
        | '\u{3000}' => ' ',
        // The typographic apostrophe.
        '\u{2019}' | '\u{02bc}' => '\'',
        // A soft hyphen, the zero-width characters, the word joiner and the
        // byte-order mark write nothing.
        '\u{00ad}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' => return None,
        other => other,
    })
}

/// The note's words around a figure, for adjudication.
fn excerpt_around(text: &str, start: usize, end: usize) -> String {
    let window_start = text[..start]
        .char_indices()
        .rev()
        .nth(CONTEXT_CHARS)
        .map_or(0, |(offset, _)| offset);
    let window_end = text[end..]
        .char_indices()
        .nth(CONTEXT_CHARS)
        .map_or(text.len(), |(offset, _)| end + offset);
    text[window_start..window_end].trim().to_string()
}

/// Where a check's figure sits in the note, as a reader has it.
struct Placement {
    offset: usize,
    excerpt: String,
}

struct FoundNumber {
    value: f64,
    decimals: usize,
    percent: bool,
    /// Written with a leading `+` or `-`.
    explicit_sign: bool,
    start: usize,
    end: usize,
    /// Written as a word -- "five" -- rather than in digits.
    word: bool,
}

/// Every figure the checker reads: those written in digits, and those written
/// as words in the two forms where a word states a field's value -- each only
/// where it is a whole numeric expression by itself.
fn scan_figures(text: &str) -> Vec<FoundNumber> {
    let digits = scan_numbers(text);
    let tokens = numeral_tokens(text, &digits);
    let compound = in_a_compound(text, &tokens);
    let inside: BTreeSet<usize> = tokens
        .iter()
        .zip(&compound)
        .filter(|(_, inside)| **inside)
        .map(|(token, _)| token.start)
        .collect();
    let mut found: Vec<FoundNumber> = digits
        .into_iter()
        .filter(|number| !inside.contains(&number.start))
        .collect();
    found.extend(number_words(text, &tokens, &compound));
    found.sort_by_key(|number| number.start);
    found
}

/// Number words, and the whole value each one states.
const NUMBER_WORDS: &[(&str, f64)] = &[
    ("zero", 0.0),
    ("one", 1.0),
    ("two", 2.0),
    ("three", 3.0),
    ("four", 4.0),
    ("five", 5.0),
    ("six", 6.0),
    ("seven", 7.0),
    ("eight", 8.0),
    ("nine", 9.0),
    ("ten", 10.0),
    ("eleven", 11.0),
    ("twelve", 12.0),
];

/// Words that may stand between a number word and "confluences": "Five
/// bullish technical confluences". A closed set, so "five names with
/// confluences" is not read as a count.
const COUNT_WORD_ADJECTIVES: &[&str] = &["bullish", "bearish", "technical", "daily"];

/// Where the "confluence" a number word counts sits, if the word is written
/// as a confluence count: "five-confluence", "four confluences", "Five bullish
/// technical confluences" -- at most two of the adjectives between, all in
/// one clause.
fn word_count_phrase(text: &str, number: &FoundNumber) -> Option<(usize, usize)> {
    let (_, clause_end) = clause_bounds(text, number.start);
    let mut cursor = number.end;
    let mut adjectives = 0;
    loop {
        let tail = text.get(cursor..clause_end)?;
        let trimmed = tail.trim_start_matches(['-', ' ']);
        let start = cursor + (tail.len() - trimmed.len());
        let length = trimmed
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(trimmed.len());
        if length == 0 {
            // Punctuation or the clause's end: no field word follows.
            return None;
        }
        let word = &trimmed[..length];
        if word == "confluence" || word == "confluences" {
            return Some((start, start + length));
        }
        if adjectives < 2 && COUNT_WORD_ADJECTIVES.contains(&word) {
            adjectives += 1;
            cursor = start + length;
            continue;
        }
        return None;
    }
}

/// Numbers written as words, kept only where a word states a field's value.
///
/// The scanner read digits alone, so "five-day Markov signal", "Five
/// bullish technical confluences" and "five-confluence trend" were never
/// checked. Those were every omission the whole-note audit found in a note the
/// checker read. But most number words in the notes are not field values --
/// "one share" alone appears 27 times, beside "two weeks", "one position",
/// "zero buy budget" -- so a word is kept only in two closed forms, both of
/// whole-number fields:
///
/// - a confluence count, as `word_count_phrase` reads it;
/// - the Markov horizon, as `markov_horizon_unit_end` reads it.
///
/// Anything else is not a figure at all. "Zero bear probability" names a
/// field, but a word states no precision for a probability, and compared at
/// none it would accept anything under 0.5; it is left unread.
fn number_words(text: &str, tokens: &[Token], compound: &[bool]) -> Vec<FoundNumber> {
    tokens
        .iter()
        .zip(compound)
        .filter(|(token, inside)| !**inside && token.kind != Numeral::Figure)
        .filter_map(|(token, _)| {
            let word = &text[token.start..token.end];
            let (_, value) = NUMBER_WORDS.iter().find(|(number, _)| *number == word)?;
            let number = FoundNumber {
                value: *value,
                decimals: 0,
                percent: false,
                explicit_sign: false,
                start: token.start,
                end: token.end,
                word: true,
            };
            (word_count_phrase(text, &number).is_some()
                || markov_horizon_unit_end(text, &number).is_some())
            .then_some(number)
        })
        .collect()
}

/// The part a word or a digit figure can play in a number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Numeral {
    /// A figure written in digits.
    Figure,
    /// "zero" to "nine".
    Unit,
    /// "ten" to "nineteen".
    Teen,
    /// "twenty" to "ninety".
    Tens,
    /// "hundred", "thousand", "million", "billion", "dozen".
    Scale,
    /// "point", as in "five point two".
    Point,
    /// "and", as in "one hundred and five" and "five and a half".
    And,
    /// "a", as in "a hundred" and "and a half".
    A,
    /// "half", "quarter", "third".
    Fraction,
    /// Any other word.
    Other,
}

fn numeral_kind(word: &str) -> Numeral {
    match word {
        "zero" | "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine" => {
            Numeral::Unit
        }
        "ten" | "eleven" | "twelve" | "thirteen" | "fourteen" | "fifteen" | "sixteen"
        | "seventeen" | "eighteen" | "nineteen" => Numeral::Teen,
        "twenty" | "thirty" | "forty" | "fifty" | "sixty" | "seventy" | "eighty" | "ninety" => {
            Numeral::Tens
        }
        "hundred" | "hundreds" | "thousand" | "thousands" | "million" | "millions" | "billion"
        | "billions" | "dozen" | "dozens" => Numeral::Scale,
        "point" => Numeral::Point,
        "and" => Numeral::And,
        "a" => Numeral::A,
        "half" | "halves" | "quarter" | "quarters" | "third" | "thirds" => Numeral::Fraction,
        _ if !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) => Numeral::Figure,
        _ => Numeral::Other,
    }
}

/// A word, or a figure in digits, and the part it can play in a number.
struct Token {
    start: usize,
    end: usize,
    kind: Numeral,
}

/// The text as a run of words and digit figures, in order.
fn numeral_tokens(text: &str, figures: &[FoundNumber]) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut figures = figures.iter().peekable();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while figures.next_if(|figure| figure.start < index).is_some() {}
        if let Some(figure) = figures.next_if(|figure| figure.start == index) {
            tokens.push(Token {
                start: figure.start,
                end: figure.end,
                kind: Numeral::Figure,
            });
            index = figure.end;
            continue;
        }
        if bytes[index].is_ascii_alphanumeric() {
            let end = text[index..]
                .find(|c: char| !c.is_ascii_alphanumeric())
                .map_or(text.len(), |length| index + length);
            tokens.push(Token {
                start: index,
                end,
                kind: numeral_kind(&text[index..end]),
            });
            index = end;
            continue;
        }
        index += text[index..].chars().next().map_or(1, char::len_utf8);
    }
    tokens
}

/// Whether the text between two tokens can join them into one number:
/// spaces, and at most one hyphen.
fn joins(between: &str) -> bool {
    !between.is_empty()
        && between.chars().all(|c| c == ' ' || c == '-')
        && between.matches('-').count() <= 1
}

/// Whether each token is a numeral inside a longer numeric expression.
///
/// `n14` looked only at a number word's neighbours, so "one hundred and
/// five-day" read a five past the "and", and "five point two confluences" a
/// two past the "point". This reads the expression instead. Two tokens joined
/// by spaces or a hyphen belong to one number when English writes one number
/// that way:
///
/// - a tens word and a unit: "twenty-five";
/// - a numeral and a scale, or a scale and a numeral: "five hundred", "5
///   hundred", "a hundred", "hundred and five";
/// - numerals either side of "point": "five point two", "point two five";
/// - a numeral and a fraction: "five and a half", "two thirds";
/// - two number words side by side, "five six", which is no single figure.
///
/// A numeral inside an expression is not a figure at all, in words or in
/// digits, rather than read in part. A digit figure beside a number word is
/// two figures, as in "+0.551 five-day markov signal"; so are two number
/// words joined by a hyphen, "five-six", which is a range `in_a_range` reads.
fn in_a_compound(text: &str, tokens: &[Token]) -> Vec<bool> {
    use Numeral::{A, And, Figure, Fraction, Point, Scale, Teen, Tens, Unit};
    let numeral = |kind: Numeral| matches!(kind, Figure | Unit | Teen | Tens);
    let worded = |kind: Numeral| matches!(kind, Unit | Teen | Tens);
    let between = |index: usize| &text[tokens[index].end..tokens[index + 1].start];
    let joined: Vec<bool> = (0..tokens.len().saturating_sub(1))
        .map(|index| joins(between(index)))
        .collect();
    // Whether tokens `index` and `index + 1` are joined and of these kinds.
    let link = |index: usize, left: &dyn Fn(Numeral) -> bool, right: &dyn Fn(Numeral) -> bool| {
        joined.get(index).copied().unwrap_or(false)
            && left(tokens[index].kind)
            && right(tokens[index + 1].kind)
    };
    let is = |wanted: Numeral| move |kind: Numeral| kind == wanted;
    let bonded: Vec<bool> = (0..joined.len())
        .map(|index| {
            if !joined[index] {
                return false;
            }
            let previous = |left: &dyn Fn(Numeral) -> bool, right: &dyn Fn(Numeral) -> bool| {
                index > 0 && link(index - 1, left, right)
            };
            match (tokens[index].kind, tokens[index + 1].kind) {
                (Tens, Unit) => true,
                (left, Scale) => numeral(left) || left == A,
                (Scale, right) if numeral(right) => true,
                (Scale, And) => link(index + 1, &is(And), &numeral),
                (And, right) if numeral(right) => previous(&is(Scale), &is(And)),
                (left, Point) if numeral(left) => {
                    link(index + 1, &is(Point), &|kind| matches!(kind, Unit | Figure))
                }
                (Point, Unit | Figure) => previous(&numeral, &is(Point)),
                (left, And) if numeral(left) => {
                    link(index + 1, &is(And), &is(A)) && link(index + 2, &is(A), &is(Fraction))
                }
                (And, A) => previous(&numeral, &is(And)) && link(index + 1, &is(A), &is(Fraction)),
                (A, Fraction) => {
                    previous(&is(And), &is(A)) && index > 1 && link(index - 2, &numeral, &is(And))
                }
                (left, Fraction) => numeral(left),
                (left, right) if worded(left) && worded(right) => !between(index).contains('-'),
                _ => false,
            }
        })
        .collect();
    (0..tokens.len())
        .map(|index| {
            numeral(tokens[index].kind)
                && (index
                    .checked_sub(1)
                    .is_some_and(|previous| bonded[previous])
                    || bonded.get(index).copied().unwrap_or(false))
        })
        .collect()
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
        // U+2212 MINUS SIGN arrives here as `-`, which `Canonical` writes it
        // as. Discarding it once read a negative figure as positive and
        // reported a sign error as agreement.
        if start > 0 && (bytes[start - 1] == b'+' || bytes[start - 1] == b'-') {
            if bytes[start - 1] == b'-' {
                sign = -1.0;
            }
            explicit_sign = true;
            start -= 1;
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
                    word: false,
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

/// Whether two adjacent figures are the count-over-minimum notation.
///
/// `N/M` is how every report in this system writes confluences against the
/// minimum required, and the rule used to fire only when one side had already
/// been attributed to the count -- so "bullish 4/3, markov long 0.555", which
/// never writes the word, had its 3 compared against a Markov signal.
///
/// Recognised from the notation alone: two whole numbers, unsigned, no
/// percentage, separated by a single slash that is not part of a longer chain.
/// Across all 1,105 stored notes every one of the 298 occurrences of this shape
/// is a confluence count, and the bound on their size keeps a date or a version
/// out. The limitation it accepts is a genuine small fraction -- "1/2 position"
/// would be read as a confluence count.
fn count_over_minimum(text: &str, left: &FoundNumber, right: &FoundNumber) -> bool {
    /// Larger than any confluence count this system produces, and smaller than
    /// a year or a price.
    const LARGEST: f64 = 12.0;
    let bytes = text.as_bytes();
    right.start == left.end + 1
        && bytes.get(left.end) == Some(&b'/')
        && bytes.get(right.end) != Some(&b'/')
        && left.start.checked_sub(1).map(|before| bytes[before]) != Some(b'/')
        && left.decimals == 0
        && right.decimals == 0
        && !left.explicit_sign
        && !right.explicit_sign
        && !left.percent
        && !right.percent
        && (1.0..=LARGEST).contains(&left.value)
        && (1.0..=LARGEST).contains(&right.value)
}

/// Whether a sentence ends between two positions.
///
/// A phrase in the next sentence does not name this figure. "markov is
/// bull/long with +0.404 signal. quiver is supportive only" put `quiver` nine
/// characters from the figure and `markov` nineteen, so the closest-phrase
/// rule handed a Markov signal to Quiver and reported a disagreement.
///
/// Only full stops and semicolons count. Commas were tried and cost more than
/// they saved: these notes are comma-spliced lists, and a figure is routinely
/// separated from its own field name by one.
fn sentence_ends_between(text: &str, left: usize, right: usize) -> bool {
    if left >= right {
        return false;
    }
    let bytes = text.as_bytes();
    (left..right).any(|position| {
        let byte = bytes[position];
        if byte == b';' || byte == b'!' || byte == b'?' {
            return true;
        }
        if byte != b'.' {
            return false;
        }
        let before_is_digit = position
            .checked_sub(1)
            .is_some_and(|previous| bytes[previous].is_ascii_digit());
        let after_is_digit = bytes.get(position + 1).is_some_and(u8::is_ascii_digit);
        !(before_is_digit && after_is_digit)
    })
}

/// Words that cannot follow a noun, and so mark the word before them as a verb.
///
/// "the 453.0 EUR daily close support a controlled limit entry" uses `support`
/// as a verb; the field table reads it as the support level and reported a
/// disagreement against 434.6. English does not put a determiner after a noun,
/// so this is a narrow syntactic test rather than a guess.
const DETERMINERS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "its", "their", "our", "my", "his", "her",
    "any", "some",
];

/// Whether the phrase ending at `end` is being used as a verb.
fn used_as_a_verb(text: &str, end: usize) -> bool {
    let tail = text[end..].trim_start_matches(' ');
    if tail.len() == text[end..].len() {
        // Nothing separated them, so this is not a following word at all.
        return false;
    }
    DETERMINERS.iter().any(|determiner| {
        tail.strip_prefix(determiner)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|next| next == ' ')
    })
}

/// Maps every separator a note may write inside a field's name to a space.
///
/// Notes write `bull_prob`, `signed_signal`, `break-risk` and `reward/risk`
/// for names the table spells with spaces, and the keyword search is a plain
/// substring match -- so `bull_prob` matched nothing, and in all of production
/// `markov.bull_prob` and `markov.bear_prob` were never checked once. Two of
/// twelve fields, silently unverified.
///
/// Normalising both sides matches every spelling with one rule rather than a
/// list of variants. Each separator is a single ASCII byte, so every offset in
/// the text is preserved exactly and a phrase found in the normalised copy
/// sits at the same place in the original.
fn with_separators_normalised(text: &str) -> String {
    text.replace(['_', '-', '/'], " ")
}

/// A keyword made only of single letters, such as "r r" for "R/R".
fn is_initialism(keyword: &str) -> bool {
    keyword.split(' ').all(|token| token.len() == 1)
}

/// Whether the match at `start..end` is not part of a longer word.
fn stands_alone(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let before = start
        .checked_sub(1)
        .is_none_or(|previous| !bytes[previous].is_ascii_alphanumeric());
    let after = bytes
        .get(end)
        .is_none_or(|next| !next.is_ascii_alphanumeric());
    before && after
}

/// Whether a keyword matched at `start..end` is a whole name, rather than
/// part of a longer word.
///
/// The search was a plain substring match, so "support" was found inside
/// "supported" and contested the count in "supported by 5 confluences". That
/// was the fresh evaluation's one concrete parser defect. A name now needs a
/// non-alphanumeric character, or the text's edge, before it, and no letter
/// after it. A digit may follow, because notes write the period onto the
/// indicator: "rsi14". An initialism still needs a boundary on both sides:
/// the "r r" of "r/r" also sits inside "higher rsi".
fn names_a_field(text: &str, start: usize, end: usize, keyword: &str) -> bool {
    if is_initialism(keyword) {
        return stands_alone(text, start, end);
    }
    let bytes = text.as_bytes();
    let before = start
        .checked_sub(1)
        .is_none_or(|previous| !bytes[previous].is_ascii_alphanumeric());
    let after = bytes
        .get(end)
        .is_none_or(|next| !next.is_ascii_alphabetic());
    before && after
}

/// The field table's keywords, separator-normalised once.
static NORMALISED_KEYWORDS: LazyLock<Vec<Vec<String>>> = LazyLock::new(|| {
    FIELDS
        .iter()
        .map(|field| {
            field
                .keywords
                .iter()
                .map(|keyword| with_separators_normalised(keyword))
                .collect()
        })
        .collect()
});

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
        // A number word never contests a phrase: it is kept only where its
        // own form already names the field.
        .map(|number| !number.word && !measures_something_else(text, number.end))
        .collect();

    let searchable = with_separators_normalised(text);
    let mut pairs: Vec<Pair> = Vec::new();
    for (field, keywords) in FIELDS.iter().zip(NORMALISED_KEYWORDS.iter()) {
        for keyword in keywords {
            let mut from = 0usize;
            while let Some(offset) = searchable[from..].find(keyword.as_str()) {
                let start = from + offset;
                let end = start + keyword.len();
                from = start + 1;
                // Part of a longer word is not a name, and nor is a verb.
                if !names_a_field(&searchable, start, end, keyword)
                    || used_as_a_verb(&searchable, end)
                {
                    continue;
                }
                for (index, number) in numbers.iter().enumerate() {
                    if !eligible[index] {
                        continue;
                    }
                    // A phrase in the next sentence names nothing here.
                    if sentence_ends_between(text, number.end.min(start), start.max(number.end))
                        && (number.end <= start || end <= number.start)
                    {
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
    // The Markov horizon written as a duration, settled before anything reads
    // the phrases and using none of them, so the signal it qualifies keeps its
    // `markov`. These figures are already ineligible for phrases.
    let horizon = FIELDS
        .iter()
        .find(|field| field.path == "markov.horizon_days");
    for (index, number) in numbers.iter().enumerate() {
        if markov_horizon_unit_end(text, number).is_some() {
            assigned[index] = horizon;
            settled[index] = true;
        }
    }
    // A count written as a word, and the "confluences" it names, which is
    // used up by it so no other figure can claim it.
    let count = FIELDS
        .iter()
        .find(|field| field.path == "daily_indicators.confluence_count");
    for (index, number) in numbers.iter().enumerate() {
        if !number.word || settled[index] {
            continue;
        }
        if let Some(phrase) = word_count_phrase(text, number) {
            assigned[index] = count;
            settled[index] = true;
            consumed.push(phrase);
        }
    }
    // The count-over-minimum notation, settled before anything else reads the
    // phrases.
    //
    // It used to run last and only when one side had already been attributed
    // to the count, so "bullish 4/3, markov long 0.555" left the 3 compared
    // against the Markov signal. Worse, the figures it was about to settle had
    // meanwhile claimed phrases they had no business holding -- the 4 took
    // `markov` at sixteen characters, which then could not contest the 0.576
    // six characters from it, and a Markov signal was reported against Quiver.
    // Settling the notation first leaves those phrases where they belong.
    for index in 0..numbers.len().saturating_sub(1) {
        if !count_over_minimum(text, &numbers[index], &numbers[index + 1]) {
            continue;
        }
        if !eligible[index] || !eligible[index + 1] {
            continue;
        }
        assigned[index] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.confluence_count");
        assigned[index + 1] = FIELDS
            .iter()
            .find(|field| field.path == "daily_indicators.min_confluences");
        settled[index] = true;
        settled[index + 1] = true;
        // No `phrase_end` is recorded: a gap measured from some other phrase
        // would be measured from one this figure never used.
        //
        // But where the word is written too -- "6/3 technical confluences" --
        // that phrase is part of this naming and is used up by it. Leaving it
        // free let "confluences," contest the 2.71 two characters after it in
        // "6/3 technical confluences, 2.71 reward/risk", and a reading that
        // had been correct became an abstention.
        if let Some(naming) = pairs
            .iter()
            .filter(|pair| {
                (pair.number == index || pair.number == index + 1)
                    && (pair.field.path.ends_with("confluence_count")
                        || pair.field.path.ends_with("min_confluences"))
            })
            .min_by_key(|pair| pair.distance)
        {
            consumed.push(naming.span);
        }
    }

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

    (assigned, uncertain, phrase_end)
}

fn check_one(
    text: &str,
    found: &FoundNumber,
    (field, uncertain, phrase_end): (Option<&'static FieldSpec>, bool, Option<usize>),
    evidence: &JsonValue,
    placement: Placement,
) -> NumericCheck {
    let Placement { offset, excerpt } = placement;

    // A quantity of something is not the value of a field -- except the one
    // duration the evidence carries, which attribution has already named.
    let horizon = field.is_some_and(|field| field.path == "markov.horizon_days");
    if measures_something_else(text, found.end) && !horizon {
        return NumericCheck {
            quoted: found.value,
            field: None,
            actual: None,
            relation: RELATION_NOT_READ,
            verdict: NumericVerdict::NotAFieldValue,
            excerpt,
            offset,
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
            offset,
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
            offset,
        };
    }
    let relation = relation_of(text, found, field, phrase_end);

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
            offset,
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
            offset,
        };
    }
    // A threshold is compared at face value. The rounding and truncation
    // allowance belongs to equality alone, where a note quotes a stored value
    // short: "295 DKK support" for 295.733. A bound is not a value quoted
    // short -- "RSI <= 70" asserts 70, not 70-point-something -- and letting
    // it borrow that allowance made "RSI <= 70" and "RSI > 70" both true of a
    // stored 70.9, because 70 is a legitimate truncation of it. A grader that
    // accepts two mutually exclusive claims is agreeing with whatever it is
    // shown.
    // A threshold an order of magnitude away from the field is not a claim
    // about that field. The guard used to protect equality only, so "top held
    // conviction position trading above 525 dkk" -- a price, given to
    // `markov.conviction` -- was reported as a disagreement with 0.749 rather
    // than as the misattribution it is. A relation does not make a wrong field
    // right.
    if !plausible_magnitude(comparable, actual) {
        return NumericCheck {
            quoted,
            field: Some(field.path),
            actual: Some(actual),
            relation,
            verdict: NumericVerdict::ImplausibleAttribution,
            excerpt,
            offset,
        };
    }
    let verdict = match relation {
        RELATION_ABOVE if stored > written => NumericVerdict::Matches,
        RELATION_BELOW if stored < written => NumericVerdict::Matches,
        RELATION_AT_LEAST if stored >= written => NumericVerdict::Matches,
        RELATION_AT_MOST if stored <= written => NumericVerdict::Matches,
        RELATION_ABOVE | RELATION_BELOW | RELATION_AT_LEAST | RELATION_AT_MOST => {
            NumericVerdict::Differs
        }
        _ if quoted_from(written, found.decimals, stored, field.discrete) => {
            NumericVerdict::Matches
        }
        _ => NumericVerdict::Differs,
    };
    NumericCheck {
        quoted,
        field: Some(field.path),
        actual: Some(actual),
        relation,
        verdict,
        excerpt,
        offset,
    }
}

/// Whether the quoted text could have been produced from the stored value.
///
/// Not a tolerance band. A band of one unit in the last place accepted "392"
/// for a stored 391 and "0.060" for 0.061, neither of which any convention
/// produces -- and on a count it accepted "4.5" against five, though a count
/// is never written with a fraction.
///
/// **The convention is rounding** to the nearest value at the precision
/// written, either way at a half. Simon chose it on 2026-09-26. Truncation is
/// no longer accepted: it had decided a case in each of the controls (LMND,
/// RSI 60 for 60.66), the whole-note audit (ASML, 0.400 for 0.4006) and the
/// fresh evaluation (CHEMM, 502 for 502.89), each time against a labeller who
/// rounded.
///
/// "Either way at a half" keeps both half conventions this accepted before --
/// away from zero and to even -- and also covers a decimal half the binary
/// value falls just short of: 23.135 is stored as 23.13499..., and "23.14" is
/// its rounding.
///
/// A discrete field requires a whole number and exact equality.
fn quoted_from(quoted: f64, decimals: usize, actual: f64, discrete: bool) -> bool {
    if discrete {
        return quoted.fract().abs() < f64::EPSILON && (quoted - actual).abs() < 1e-9;
    }
    written_by_rounding(quoted, decimals, actual)
}

/// Whether `written`, at `decimals` places, is `actual` rounded: to the
/// nearest value, either way at a half. The one rounding test every figure a
/// report writes is held to -- here and in the completion audit's metadata
/// provenance.
///
/// "Either way at a half" allows for the binary error in a decimal half:
/// 23.135 is stored as 23.13499..., and "23.14" is its rounding. That
/// allowance is a few units of floating-point error on the scaled value, no
/// more. `n17` allowed 1e-9 of it, which grows with the value: from about nine
/// decimals on it exceeded half a unit, and truncation was accepted again.
pub(crate) fn written_by_rounding(written: f64, decimals: usize, actual: f64) -> bool {
    let scale = 10f64.powi(decimals.min(15) as i32);
    let scaled = actual * scale;
    let half = 0.5 + 8.0 * f64::EPSILON * scaled.abs().max(1.0);
    let slack = written.abs().max(actual.abs()).max(1.0) * 8.0 * f64::EPSILON;
    [scaled.floor(), scaled.ceil()].iter().any(|neighbour| {
        (scaled - neighbour).abs() <= half && (written - neighbour / scale).abs() <= slack
    })
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

        // 293 ISP: the Markov figure, not the break risk. Since n17 a
        // truncation is a disagreement: 0.41998 rounds to 0.4200.
        let isp = json!({"markov": {"signed_signal": 0.4199841320514679}});
        let checks = numeric_checks("solid +0.4199 Bull Markov signal", &isp);
        assert_eq!(checks[0].field, Some("markov.signed_signal"));
        assert_eq!(checks[0].verdict, NumericVerdict::Differs);
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

    /// Rounding is accepted; truncation, and a genuinely different figure at
    /// the same precision, are not.
    #[test]
    fn rounding_is_accepted_and_truncation_is_not() {
        let evidence = json!({"markov": {"signed_signal": 0.4199841320514679}});
        assert_eq!(
            numeric_checks("+0.4200 Markov signal", &evidence)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("+0.4199 Markov signal", &evidence)[0].verdict,
            NumericVerdict::Differs,
            "truncated"
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

    /// "296 DKK support" for a stored 295.73302 is rounding; since n17 "295"
    /// is a truncation and a disagreement. A count is exact, so "4
    /// confluences" against five stays a disagreement.
    #[test]
    fn a_whole_number_rounds_on_a_price_and_is_exact_on_a_count() {
        let price = json!({
            "daily_indicators": {"support": {"nearest_support": 295.73302}}
        });
        assert_eq!(
            numeric_checks("consolidating near 296 DKK support", &price)[0].verdict,
            NumericVerdict::Matches
        );
        assert_eq!(
            numeric_checks("consolidating near 295 DKK support", &price)[0].verdict,
            NumericVerdict::Differs
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

    /// The convention is rounding, either way at a half, and nothing else.
    /// Truncation was accepted until n17.
    #[test]
    fn rounding_either_way_at_a_half_is_the_only_convention() {
        let support = json!({"daily_indicators": {"support": {"nearest_support": 295.73302}}});
        assert_eq!(
            numeric_checks("near 295 DKK support", &support)[0].verdict,
            NumericVerdict::Differs,
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
            NumericVerdict::Differs,
            "truncated"
        );
        assert_eq!(
            numeric_checks("+0.4200 Markov signal", &markov)[0].verdict,
            NumericVerdict::Matches
        );

        // A decimal half: 23.135 is stored as 23.13499..., so binary rounding
        // alone gives 23.13, and 23.14 is the rounding a person writes. Both
        // are rounding; 23.12 is neither.
        let half = json!({"daily_indicators": {"support": {"nearest_support": 23.135}}});
        for (note, verdict) in [
            ("support 23.13 eur", NumericVerdict::Matches),
            ("support 23.14 eur", NumericVerdict::Matches),
            ("support 23.12 eur", NumericVerdict::Differs),
        ] {
            assert_eq!(numeric_checks(note, &half)[0].verdict, verdict, "{note}");
        }
        // A long figure is held to rounding too. n17's allowance grew with
        // the value and let this ten-decimal truncation through.
        let long = json!({"markov": {"signed_signal": 0.123456789071}});
        for (note, verdict) in [
            ("markov signal 0.1234567891", NumericVerdict::Matches),
            ("markov signal 0.1234567890", NumericVerdict::Differs),
        ] {
            assert_eq!(numeric_checks(note, &long)[0].verdict, verdict, "{note}");
        }
        // And an exact binary half, either way.
        let exact = json!({"daily_indicators": {"rsi14": 62.5}});
        for (note, verdict) in [
            ("rsi 62", NumericVerdict::Matches),
            ("rsi 63", NumericVerdict::Matches),
            ("rsi 61", NumericVerdict::Differs),
        ] {
            assert_eq!(numeric_checks(note, &exact)[0].verdict, verdict, "{note}");
        }
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
            "53.49 rounds to 53"
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

        // #185 DSV: the 5 in "5-day" is a horizon, not the signal. Since n12
        // the horizon is read as what it is, `markov.horizon_days`, rather
        // than left as a bare quantity; what this case protects is unchanged
        // -- the 5 must not take `markov` away from the signal.
        let dsv = json!({"markov": {"signed_signal": 0.6590335965156555}});
        let checks = numeric_checks(
            "exceptionally strong 5-day Markov continuation signal of 0.6590",
            &dsv,
        );
        let horizon = checks
            .iter()
            .find(|check| check.quoted == 5.0)
            .expect("the horizon");
        assert_eq!(horizon.field, Some("markov.horizon_days"));
        assert_eq!(
            horizon.verdict,
            NumericVerdict::NotInEvidence,
            "this evidence carries no horizon"
        );
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

        // An approximation marker is a hedge written as punctuation. #106
        // ARKK's "rsi ~55" against a stored 61.34 was compared as an exact
        // equality and reported as a disagreement.
        assert_eq!(
            reading("RSI ~55"),
            (RELATION_UNSUPPORTED, NumericVerdict::UncertainAttribution)
        );
        assert_eq!(
            reading("RSI \u{2248}55"),
            (RELATION_UNSUPPORTED, NumericVerdict::UncertainAttribution)
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

    /// A threshold is compared at face value, so contradictory bounds cannot
    /// both hold.
    ///
    /// Inclusive bounds borrowed equality's allowance, and 70 was then a
    /// legitimate truncation of 70.9 -- so against a stored 70.9 both "RSI <=
    /// 70" and "RSI > 70" were satisfied. A grader
    /// that accepts two mutually exclusive claims agrees with whatever it is
    /// shown, which is the failure this whole module exists to avoid.
    #[test]
    fn contradictory_bounds_cannot_both_be_satisfied() {
        let evidence = json!({"daily_indicators": {"rsi14": 70.9}});
        for (claim, opposite) in [("RSI <= 70", "RSI > 70"), ("RSI >= 71", "RSI < 71")] {
            let held = numeric_checks(claim, &evidence)[0].verdict;
            let other = numeric_checks(opposite, &evidence)[0].verdict;
            assert_ne!(held, other, "{claim:?} and {opposite:?} cannot both hold");
        }
        assert_eq!(
            numeric_checks("RSI <= 70", &evidence)[0].verdict,
            NumericVerdict::Differs
        );
        assert_eq!(
            numeric_checks("RSI > 70", &evidence)[0].verdict,
            NumericVerdict::Matches
        );

        // Equality keeps the allowance: a note quoting a stored value short is
        // rounding, not asserting a bound.
        assert_eq!(
            numeric_checks("RSI 70.9", &evidence)[0].verdict,
            NumericVerdict::Matches
        );
        let support = json!({"daily_indicators": {"support": {"nearest_support": 295.73302}}});
        assert_eq!(
            numeric_checks("296 DKK support", &support)[0].verdict,
            NumericVerdict::Matches
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

    /// A relation does not make a wrong field right.
    ///
    /// #313 CHEMM: "top held conviction position trading above 525 dkk with
    /// +0.749 markov bull regime" gave the price to `markov.conviction` and,
    /// because the magnitude guard protected equality only, reported a
    /// disagreement between 525 and 0.749 instead of naming the
    /// misattribution.
    #[test]
    fn a_threshold_an_order_of_magnitude_from_the_field_is_a_misattribution() {
        let evidence = json!({"markov": {"conviction": 0.7490864992141724}});
        let check = &numeric_checks(
            "Top held conviction holding trading above 525 DKK with +0.749 Markov Bull regime",
            &evidence,
        )[0];
        assert_eq!(check.field, Some("markov.conviction"));
        assert_eq!(check.verdict, NumericVerdict::ImplausibleAttribution);

        // A threshold beside its own field is still compared.
        let rsi = json!({"daily_indicators": {"rsi14": 71.04899226674894}});
        assert_eq!(
            numeric_checks("RSI is above 70", &rsi)[0].verdict,
            NumericVerdict::Matches
        );
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

#[cfg(test)]
mod separator_regressions {
    use super::*;
    use serde_json::json;

    fn evidence() -> JsonValue {
        json!({
            "markov": {
                "bull_prob": 0.72,
                "bear_prob": 0.11,
                "signed_signal": 0.6124185919761658,
            },
            "daily_indicators": {
                "reward_risk": 0.2397,
                "support": {"break_risk": 0.0614},
            },
        })
    }

    /// A field name written with an underscore is the same name.
    ///
    /// The keyword search was a plain substring match, so `bull_prob` never
    /// matched `bull prob` and in all of production `markov.bull_prob` and
    /// `markov.bear_prob` were checked zero times. Two of twelve fields
    /// silently unverified, and the challenge set found it by being unable to
    /// build a single case for them.
    #[test]
    fn a_name_written_with_an_underscore_reaches_its_field() {
        let checks = numeric_checks(
            "markov bull_prob 0.72 / bear_prob 0.11 / signed_signal 0.6124",
            &evidence(),
        );
        let field_of = |quoted: f64| {
            checks
                .iter()
                .find(|check| (check.quoted - quoted).abs() < 1e-9)
                .unwrap_or_else(|| panic!("no check for {quoted} in {checks:?}"))
        };
        assert_eq!(field_of(0.72).field, Some("markov.bull_prob"));
        assert_eq!(field_of(0.72).verdict, NumericVerdict::Matches);
        assert_eq!(field_of(0.11).field, Some("markov.bear_prob"));
        assert_eq!(field_of(0.11).verdict, NumericVerdict::Matches);
        // And the figure beside it keeps its own field. Before `signed signal`
        // was a name in its own right, the nearest phrase to 0.6124 was
        // `bull_prob` -- so fixing one attribution would have broken another.
        assert_eq!(field_of(0.6124).field, Some("markov.signed_signal"));
        assert_eq!(field_of(0.6124).verdict, NumericVerdict::Matches);

        // A wrong probability is now a finding rather than silence.
        let wrong = numeric_checks("markov bull_prob 0.80", &evidence());
        assert_eq!(wrong[0].field, Some("markov.bull_prob"));
        assert_eq!(wrong[0].verdict, NumericVerdict::Differs);
    }

    /// The hyphen and slash spellings the table used to carry explicitly are
    /// still read, now by the same rule rather than by their own entries.
    #[test]
    fn the_hyphen_and_slash_spellings_still_match() {
        for note in [
            "low support break-risk 0.0614 and reward/risk 0.2397",
            "low support break risk 0.0614 and reward risk 0.2397",
            "low support_break_risk 0.0614 and reward_risk 0.2397",
        ] {
            let checks = numeric_checks(note, &evidence());
            assert!(
                checks
                    .iter()
                    .all(|check| check.verdict == NumericVerdict::Matches),
                "{note:?} -> {checks:?}"
            );
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
///
/// What it measures is **grammar acceptance, not comparison.** It reads notes
/// with no evidence to check them against, so it stops at `relation_for`. A
/// figure the grammar accepts can still end `not_in_evidence` or
/// `implausible_attribution` when a real check runs -- 179 of them did on the
/// stored history -- so this rate is a floor on how much goes uncompared, not
/// the figure itself. That one only exists in production.
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
        if symbolic_relation(text, number_start) == Some(RELATION_UNSUPPORTED) {
            return "approximation_marker".to_string();
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
            let numbers = scan_figures(&lowered);
            let (fields, uncertain, phrase_end) = attribute_all(&lowered, &numbers);
            for (index, found) in numbers.iter().enumerate() {
                figures += 1;
                let Some(field) = fields[index] else {
                    continue;
                };
                if uncertain[index]
                    || (measures_something_else(&lowered, found.end)
                        && field.path != "markov.horizon_days")
                {
                    continue;
                }
                attributed += 1;
                let relation = relation_of(&lowered, found, field, phrase_end[index]);
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
        println!(
            "grammar_accepted_or_rejected={relations:?} (not verdicts: nothing is compared here)"
        );
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

#[cfg(test)]
mod attribution_regressions {
    use super::*;
    use serde_json::json;

    fn fields(note: &str, evidence: &JsonValue) -> Vec<(f64, Option<&'static str>, &'static str)> {
        numeric_checks(note, evidence)
            .into_iter()
            .map(|check| (check.quoted, check.field, check.verdict.as_str()))
            .collect()
    }

    /// The count-over-minimum notation is recognised from its own shape.
    ///
    /// The rule used to fire only when one side had already been attributed to
    /// the count, so "bullish 4/3, markov long 0.555" -- which never writes the
    /// word -- gave the 3 to `markov.signed_signal` and reported a
    /// disagreement against 0.555. It cost the Markov figure too: with its
    /// phrase taken, 0.555 went unattributed.
    #[test]
    fn a_bare_count_over_minimum_is_read_without_the_word() {
        let evidence = json!({
            "daily_indicators": {"confluence_count": 4, "min_confluences": 3, "rsi14": 70.9},
            "markov": {"signed_signal": 0.5549628138542175},
        });
        assert_eq!(
            fields(
                "Affordable US-pulse starter add; bullish 4/3, Markov long 0.555. RSI 70.9 \
                 elevated, capped size.",
                &evidence
            ),
            vec![
                (4.0, Some("daily_indicators.confluence_count"), "matches"),
                (3.0, Some("daily_indicators.min_confluences"), "matches"),
                (0.555, Some("markov.signed_signal"), "matches"),
                (70.9, Some("daily_indicators.rsi14"), "matches"),
            ]
        );

        // The notation still uses up the word where the note writes it, so
        // "confluences," does not then contest the figure beside it.
        let neighbour = json!({
            "daily_indicators": {"confluence_count": 6, "min_confluences": 3, "reward_risk": 2.71},
        });
        assert_eq!(
            fields("6/3 technical confluences, 2.71 reward/risk", &neighbour)[2],
            (2.71, Some("daily_indicators.reward_risk"), "matches")
        );
    }

    /// A phrase in the next sentence names nothing here.
    ///
    /// "markov is bull/long with +0.404 signal. quiver is supportive only" put
    /// `quiver` nine characters from the figure and `markov` nineteen, so a
    /// Markov signal was compared against the Quiver score and reported as a
    /// disagreement.
    #[test]
    fn a_field_named_in_the_next_sentence_does_not_claim_the_figure() {
        let evidence = json!({
            "daily_indicators": {"confluence_count": 4, "min_confluences": 3},
            "markov": {"signed_signal": 0.404},
            "quiver": {"signal": 0.32695674896240234},
        });
        let checks = fields(
            "New US financial-services starter. Daily technicals are OVERWEIGHT with bullish \
             trend and 4/3 confluences; Markov is Bull/long with +0.404 signal. Quiver is \
             supportive only.",
            &evidence,
        );
        assert_eq!(checks[2], (0.404, Some("markov.signed_signal"), "matches"));
    }

    /// Within one sentence the rivalry stands, and a near tie abstains.
    ///
    /// "Markov long 0.576 and Quiver bullish" names both within a character of
    /// each other. Reporting the marginally closer one had the figure compared
    /// against the wrong field; abstaining says only that the note does not
    /// settle which.
    #[test]
    fn two_fields_naming_one_figure_in_the_same_sentence_still_abstain() {
        let evidence = json!({
            "daily_indicators": {"confluence_count": 4, "min_confluences": 3},
            "markov": {"signed_signal": 0.576},
            "quiver": {"signal": 0.35456347465515137},
        });
        let checks = fields(
            "Constructive bank setup: daily OVERWEIGHT, bullish trend, 4/3 confluences, \
             Markov long 0.576 and Quiver bullish.",
            &evidence,
        );
        assert_eq!(checks[2].0, 0.576);
        assert_eq!(checks[2].2, "uncertain_attribution");
    }

    /// A verb is not a field name.
    ///
    /// "the 453.0 EUR daily close support a controlled limit entry" uses
    /// `support` as a verb, and the figure is the daily close. Reading it as
    /// the support level reported a disagreement against 434.6. English does
    /// not put a determiner after a noun, which is the whole test.
    #[test]
    fn a_keyword_used_as_a_verb_names_nothing() {
        let evidence = json!({
            "daily_indicators": {
                "confluence_count": 5,
                "min_confluences": 3,
                "support": {"break_risk": 0.037, "nearest_support": 434.6},
            },
            "markov": {"signed_signal": 0.2467},
        });
        let checks = fields(
            "Bullish 5/3 BUY technical configuration, fresh +0.2467 long Markov signal, and a \
             current 452.5 EUR quote below the 453.0 EUR daily close support a controlled limit \
             entry.",
            &evidence,
        );
        let four_five_three = checks
            .iter()
            .find(|(quoted, ..)| *quoted == 453.0)
            .expect("the close");
        assert_eq!(four_five_three.1, None);
        assert_eq!(four_five_three.2, "unattributed");

        // And the noun still names its field.
        let noun = json!({"daily_indicators": {"support": {"nearest_support": 446.0}}});
        assert_eq!(
            fields("consolidating comfortably above 446 EUR support", &noun)[0],
            (
                446.0,
                Some("daily_indicators.support.nearest_support"),
                "matches"
            )
        );
    }
}

/// `n12`: the two names `n11` missed, and what they must not catch.
#[cfg(test)]
mod n12_attribution {
    use super::*;
    use serde_json::json;

    fn evidence() -> JsonValue {
        json!({
            "daily_indicators": {"reward_risk": 0.7499180385286995, "rsi14": 58.2, "confluence_count": 5},
            "markov": {"signed_signal": 0.5596567728803779, "bull_prob": 0.61, "horizon_days": 5},
        })
    }

    fn check(note: &str, quoted: f64) -> NumericCheck {
        numeric_checks(note, &evidence())
            .into_iter()
            .find(|check| (check.quoted - quoted).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no figure {quoted} in {note:?}"))
    }

    /// Report #106 BAC. "R/R" read as nothing, because the separator rule
    /// makes it "r r" and no keyword spelled that.
    #[test]
    fn r_slash_r_is_the_reward_risk() {
        let note = "daily BUY, 5 confluences, R/R 0.75, RSI 58. Best non-ETF entry";
        let reward = check(note, 0.75);
        assert_eq!(reward.field, Some("daily_indicators.reward_risk"));
        assert_eq!(reward.verdict, NumericVerdict::Matches);
        assert_eq!(check(note, 58.0).field, Some("daily_indicators.rsi14"));
    }

    /// The "r r" of "r/r" also sits inside "higher rsi". An initialism names a
    /// field only standing alone.
    #[test]
    fn an_initialism_inside_other_words_names_nothing() {
        let evidence = json!({"daily_indicators": {"reward_risk": 62.0, "rsi14": 62.0}});
        let checks = numeric_checks("momentum with higher rsi 62 today", &evidence);
        assert_eq!(checks[0].field, Some("daily_indicators.rsi14"));
        assert!(
            !FIELD_WORDS.contains("r"),
            "a single letter is not a gap word"
        );
    }

    /// Every phrasing of the horizon in the stored notes, each read as
    /// `markov.horizon_days` and compared.
    #[test]
    fn the_markov_horizon_is_read_in_each_stored_phrasing() {
        for note in [
            "strong 5-day markov long signal of 0.560 and rising",
            "rsi weak, and markov 5-day signal is short at -0.2776",
            "positive bull_prob dominance over 5-day horizon",
            "bullish, but markov is negative over the 5-day horizon; hold",
            "overweight, but markov 5-day signed_signal is negative; wait",
            "regime is bear with signed_signal -0.5230 over 5 days; flatten",
            "markov direction is mildly positive but current 5-day signed signal is only 0.095",
        ] {
            let horizon = check(note, 5.0);
            assert_eq!(horizon.field, Some("markov.horizon_days"), "{note}");
            assert_eq!(horizon.verdict, NumericVerdict::Matches, "{note}");
        }
    }

    /// A horizon that is not the one in the evidence is a disagreement.
    #[test]
    fn a_wrong_horizon_is_flagged() {
        let horizon = check("markov 10-day signal is 0.560", 10.0);
        assert_eq!(horizon.field, Some("markov.horizon_days"));
        assert_eq!(horizon.verdict, NumericVerdict::Differs);
    }

    /// Report #180, "Markov 5-day signal is 0.560", went uncompared under
    /// `n11`: the grammar read the 5 and the `day` as words it did not know.
    /// A horizon the checker itself read qualifies the field, and is set aside.
    #[test]
    fn a_horizon_between_a_markov_field_and_its_figure_is_not_a_gap() {
        let signal = check(
            "existing position. markov 5-day signal is 0.560. high weight",
            0.56,
        );
        assert_eq!(signal.field, Some("markov.signed_signal"));
        assert_eq!(signal.relation, RELATION_EQUALS);
        assert_eq!(signal.verdict, NumericVerdict::Matches);
    }

    /// Durations that are not the Markov horizon stay quantities: no Markov
    /// field in the clause, or a following word that is not one of the four.
    #[test]
    fn other_durations_stay_quantities() {
        for (note, quoted) in [
            ("price holding above the 50-day sma", 50.0),
            ("held for 20 days without a stop", 20.0),
            ("markov bull regime; 30-day high in sight", 30.0),
            ("markov signal 0.56 after 3 days of consolidation", 3.0),
        ] {
            let quantity = check(note, quoted);
            assert_eq!(quantity.verdict, NumericVerdict::NotAFieldValue, "{note}");
            assert!(quantity.field.is_none(), "{note}");
        }
    }

    /// The horizon is settled without using a phrase, so the signal beside it
    /// keeps `markov` -- the reason durations were excluded in the first place.
    #[test]
    fn the_horizon_does_not_take_the_signals_name() {
        let note = "exceptionally strong 5-day markov continuation signal of 0.5597";
        assert_eq!(check(note, 5.0).field, Some("markov.horizon_days"));
        let signal = check(note, 0.5597);
        assert_eq!(signal.field, Some("markov.signed_signal"));
        assert_eq!(signal.verdict, NumericVerdict::Matches);
    }
}

/// `n13`: numbers written as words, read in two closed forms and nowhere else.
#[cfg(test)]
mod n13_number_words {
    use super::*;
    use serde_json::json;

    fn evidence(count: i64) -> JsonValue {
        json!({
            "daily_indicators": {"confluence_count": count, "min_confluences": 3,
                                 "support": {"break_risk": 0.2146940568}},
            "markov": {"signed_signal": 0.138, "horizon_days": 5, "bear_prob": 0.3},
        })
    }

    fn checks(note: &str, count: i64) -> Vec<NumericCheck> {
        numeric_checks(note, &evidence(count))
    }

    /// Where each whole number word sits in the lowercased note.
    fn word_positions(note: &str) -> Vec<usize> {
        let lowered = note.to_lowercase();
        NUMBER_WORDS
            .iter()
            .flat_map(|(word, _)| {
                lowered
                    .match_indices(word)
                    .map(|(start, _)| (start, start + word.len()))
                    .collect::<Vec<_>>()
            })
            .filter(|(start, end)| {
                let bytes = lowered.as_bytes();
                start
                    .checked_sub(1)
                    .is_none_or(|previous| !bytes[previous].is_ascii_alphanumeric())
                    && bytes
                        .get(*end)
                        .is_none_or(|next| !next.is_ascii_alphanumeric())
            })
            .map(|(start, _)| start)
            .collect()
    }

    /// Each phrasing the whole-note audit found omitted, and the other count
    /// forms in the stored notes.
    #[test]
    fn a_count_written_as_a_word_is_read() {
        for (note, count) in [
            (
                "Five-confluence BUY trend, low 0.215 support-break risk.",
                5,
            ),
            (
                "Five bullish technical confluences, low support-break risk",
                5,
            ),
            ("Five technical confluences, bullish trend", 5),
            (
                "bullish technical trend, four confluences, and low support-break risk",
                4,
            ),
            ("Six-confluence bullish trend and positive Markov", 6),
            (
                "BUY technicals, bullish trend, five confluences, low risk",
                5,
            ),
        ] {
            let at = word_positions(note)[0];
            let word = checks(note, count)
                .into_iter()
                .find(|check| check.offset == at)
                .unwrap_or_else(|| panic!("the count was not read in {note:?}"));
            assert_eq!(
                word.field,
                Some("daily_indicators.confluence_count"),
                "{note}"
            );
            assert_eq!(word.quoted, count as f64, "{note}");
            assert_eq!(word.verdict, NumericVerdict::Matches, "{note}");
        }
    }

    /// A count that is not the one in the evidence is flagged, like a digit.
    #[test]
    fn a_wrong_count_in_words_is_flagged() {
        let found = checks("four confluences and a bullish trend", 5);
        assert_eq!(found[0].field, Some("daily_indicators.confluence_count"));
        assert_eq!(found[0].verdict, NumericVerdict::Differs);
    }

    /// The horizon written as a word, and the signal beside it still read.
    #[test]
    fn the_horizon_written_as_a_word_is_read() {
        let found = checks(
            "Bullish technical setup, but five-day Markov signal is only 0.138 and falling",
            5,
        );
        let horizon = found
            .iter()
            .find(|check| check.quoted == 5.0)
            .expect("horizon");
        assert_eq!(horizon.field, Some("markov.horizon_days"));
        assert_eq!(horizon.verdict, NumericVerdict::Matches);
        let signal = found
            .iter()
            .find(|check| check.quoted == 0.138)
            .expect("signal");
        assert_eq!(signal.field, Some("markov.signed_signal"));
        assert_eq!(signal.verdict, NumericVerdict::Matches);
        // And set aside in a gap, as a digit horizon is.
        let gap = checks(
            "existing position; markov five-day signal is 0.138. hold",
            5,
        );
        let signal = gap
            .iter()
            .find(|check| check.quoted == 0.138)
            .expect("signal");
        assert_eq!(signal.verdict, NumericVerdict::Matches);
    }

    /// The count uses up its own "confluence", so the figure beside it keeps
    /// its field.
    #[test]
    fn the_counted_phrase_is_not_left_for_a_neighbour() {
        let found = checks("Five-confluence BUY trend, low 0.215 support-break risk", 5);
        let risk = found
            .iter()
            .find(|check| check.quoted == 0.215)
            .expect("risk");
        assert_eq!(risk.field, Some("daily_indicators.support.break_risk"));
        assert_eq!(risk.verdict, NumericVerdict::Matches);
    }

    /// Every other use of a number word in the stored notes stays unread: not
    /// an abstention, not a figure at all.
    #[test]
    fn number_words_that_are_not_field_values_are_not_figures() {
        for note in [
            "Markov is also positive, but one share is about 13289.31 DKK",
            "Bearish Congress flow limits the order to four shares.",
            "Restrained two-share entry limits momentum-extension risk",
            "Not actionable today because one or two shares would breach the limit",
            "avoid new exposure in the next two weeks.",
            "budget only accommodates one position.",
            "deferred by zero buy budget.",
            "leaving near-zero buffer flexibility.",
            "the strongest of the five names with confluences",
        ] {
            let words = word_positions(note);
            assert!(!words.is_empty(), "{note}");
            assert!(
                checks(note, 5)
                    .iter()
                    .all(|check| !words.contains(&check.offset)),
                "a number word was read in {note:?}"
            );
        }
        // Inside another word, it is not a number word at all.
        assert!(word_positions("someone noted a bullish trend").is_empty());
    }

    /// "Zero bear probability" names a field, but a word states no precision
    /// for a probability; compared at none it would accept anything under 0.5.
    #[test]
    fn zero_for_a_probability_is_left_unread() {
        assert!(checks("conviction with zero bear probability; high priority", 5).is_empty());
    }
}

/// `n14`: the boundary cases review found in `n13`, and their digit forms,
/// which had the same fault from the start.
#[cfg(test)]
mod n14_boundaries {
    use super::*;
    use serde_json::json;

    fn evidence(count: i64) -> JsonValue {
        json!({
            "daily_indicators": {"confluence_count": count, "support": {"nearest_support": 446.0}},
            "markov": {"horizon_days": 5, "signed_signal": -0.523},
        })
    }

    /// The verdicts, in order, for every figure in the note.
    fn verdicts(note: &str, count: i64) -> Vec<(f64, &'static str, &'static str)> {
        numeric_checks(note, &evidence(count))
            .into_iter()
            .map(|check| (check.quoted, check.relation, check.verdict.as_str()))
            .collect()
    }

    /// Review's cases: a comparison became an exact count, giving false
    /// agreement at 5 and a false alarm at 6. Now each abstains, in words and
    /// in digits, and so do the other quantity lead-ins.
    #[test]
    fn a_comparison_before_a_count_abstains() {
        for note in [
            "more than five confluences",
            "at least five confluences",
            "more than 5 confluences",
            "at least 5 confluences",
            "fewer than 5 confluences",
            "up to 5 confluences",
            "over 5 confluences",
            "about five confluences",
            "roughly 5 confluences",
        ] {
            for count in [5, 6] {
                assert_eq!(
                    verdicts(note, count),
                    vec![(5.0, RELATION_UNSUPPORTED, "uncertain_attribution")],
                    "{note} against {count}"
                );
            }
        }
    }

    /// A range or an open bound states no single value; neither end is read
    /// as exact.
    #[test]
    fn a_range_or_open_bound_is_not_an_exact_count() {
        for note in [
            "five to six confluences",
            "5 to 6 confluences",
            "5-6 confluences",
            "five or six confluences",
            "between four and six confluences",
            "5 or more confluences",
        ] {
            assert!(
                verdicts(note, 6)
                    .iter()
                    .all(|(_, _, verdict)| !matches!(*verdict, "matches" | "differs")),
                "{note}: {:?}",
                verdicts(note, 6)
            );
        }
    }

    /// Review's other case: the "five" of "twenty-five" was read alone. A
    /// compound is not read at all, rather than read in part.
    #[test]
    fn a_compound_number_is_not_read_in_part() {
        for note in [
            "twenty-five confluences",
            "twenty five confluences",
            "twenty-five-day markov signal",
            "five hundred confluences",
        ] {
            assert!(
                verdicts(note, 5).is_empty(),
                "{note}: {:?}",
                verdicts(note, 5)
            );
        }
    }

    /// Found by the census, in n14's first draft: a digit figure before a
    /// number word is not part of it, and "and" between two fields' figures is
    /// not a range. Each of these was correct under n13 and must stay so.
    #[test]
    fn a_list_is_not_a_range_and_a_figure_is_not_a_compound() {
        assert_eq!(
            verdicts("and +0.551 five-day markov signal. hold", 5)
                .iter()
                .filter(|(quoted, ..)| *quoted == 5.0)
                .count(),
            1,
            "the horizon after a digit figure is still read"
        );
        assert_eq!(
            verdicts("markov signed_signal -0.523 and 4/3 confluences", 4)[1],
            (4.0, RELATION_EQUALS, "matches")
        );
        assert_eq!(
            verdicts("between 4 and 6 confluences", 6)
                .iter()
                .find(|(quoted, ..)| *quoted == 6.0)
                .map(|(_, relation, _)| *relation),
            Some(RELATION_UNSUPPORTED),
            "after between, it is a range"
        );
    }

    /// What must not change: a plain count, a level's position words, the
    /// horizon's "over N days", and the N/M notation.
    #[test]
    fn plain_counts_levels_and_the_horizon_are_unchanged() {
        assert_eq!(
            verdicts("four confluences", 4),
            vec![(4.0, RELATION_EQUALS, "matches")]
        );
        for note in [
            "trading above 446 eur support",
            "consolidating near 446 eur support",
        ] {
            assert_eq!(
                verdicts(note, 5),
                vec![(446.0, RELATION_EQUALS, "matches")],
                "{note}"
            );
        }
        assert_eq!(
            verdicts(
                "regime is bear with signed_signal -0.5230 over 5 days; flatten",
                5
            ),
            vec![
                (-0.523, RELATION_EQUALS, "matches"),
                (5.0, RELATION_EQUALS, "matches"),
            ]
        );
        assert_eq!(
            verdicts("5/3 confluences", 5)[0],
            (5.0, RELATION_EQUALS, "matches")
        );
    }
}

/// `n15`: review's cases against `n14`, and the digit forms of the same
/// faults. These are regression cases, not validation evidence: each was
/// found by probing this checker.
#[cfg(test)]
mod n15_expressions {
    use super::*;
    use serde_json::json;

    fn evidence(count: i64, horizon: i64) -> JsonValue {
        json!({
            "daily_indicators": {"confluence_count": count, "support": {"nearest_support": 446.0}},
            "markov": {"horizon_days": horizon, "signed_signal": -0.523},
        })
    }

    /// The verdicts, in order, for every figure in the note.
    fn verdicts(note: &str, count: i64, horizon: i64) -> Vec<(f64, &'static str, &'static str)> {
        numeric_checks(note, &evidence(count, horizon))
            .into_iter()
            .map(|check| (check.quoted, check.relation, check.verdict.as_str()))
            .collect()
    }

    /// Review's first case: part of a compound read as a smaller number,
    /// past an "and", past a "point", or behind a non-breaking hyphen. No
    /// numeral inside a longer expression is read, in words or in digits.
    #[test]
    fn a_numeral_inside_a_longer_expression_is_not_read() {
        for (note, count) in [
            ("one hundred and five-day markov signal", 4),
            ("a hundred and five-day markov signal", 4),
            ("one hundred and 5-day markov signal", 4),
            ("five point two confluences", 2),
            ("five point two confluences", 5),
            ("5 point 2 confluences", 2),
            ("5 hundred confluences", 5),
            ("twenty\u{2011}five-day markov signal", 4),
            ("twenty\u{2011}five confluences", 5),
            ("twenty\u{2010}five confluences", 5),
            ("twenty\u{00a0}five confluences", 5),
            ("five six confluences", 6),
        ] {
            assert!(
                verdicts(note, count, 5).is_empty(),
                "{note}: {:?}",
                verdicts(note, count, 5)
            );
        }
    }

    /// Review's second case: "five–six confluences" compared the six as an
    /// exact count. The en dash bounds a clause, which hid the five, and the
    /// range check read only digits across a dash.
    #[test]
    fn a_range_in_words_is_a_range_across_any_dash() {
        for note in [
            "five\u{2013}six confluences",
            "five \u{2013} six confluences",
            "five-six confluences",
            "five\u{2011}six confluences",
            "five\u{2014}six confluences",
            "5\u{2013}six confluences",
            "five\u{2013}6 confluences",
            "5\u{2014}6 confluences",
        ] {
            for count in [5, 6] {
                let read = verdicts(note, count, 5);
                assert!(
                    read.iter()
                        .all(|(_, _, verdict)| !matches!(*verdict, "matches" | "differs")),
                    "{note} against {count}: {read:?}"
                );
            }
        }
    }

    /// Review's third case: `n14` exempted the horizon from every spatial
    /// word, so "under five-day Markov horizon" matched a five-day horizon.
    /// Only "over", which before a horizon means across, is still read.
    #[test]
    fn only_over_is_read_as_across_before_a_horizon() {
        for note in [
            "under five-day markov horizon",
            "under 5-day markov horizon",
            "below 5-day markov horizon",
            "above five-day markov horizon",
            "near five-day markov signal",
        ] {
            for horizon in [5, 6] {
                assert_eq!(
                    verdicts(note, 4, horizon),
                    vec![(5.0, RELATION_UNSUPPORTED, "uncertain_attribution")],
                    "{note} against {horizon}"
                );
            }
        }
        for note in [
            "positive bull_prob dominance over 5-day horizon",
            "bullish, but markov is negative over five-day horizon",
        ] {
            assert_eq!(
                verdicts(note, 4, 5),
                vec![(5.0, RELATION_EQUALS, "matches")],
                "{note}"
            );
        }
    }

    /// One reading per construction, however a dash, space or sign is
    /// spelled.
    #[test]
    fn every_spelling_of_a_character_reads_the_same() {
        for (ascii, unicode) in [
            ("-0.523 markov signal", "\u{2212}0.523 markov signal"),
            ("-0.523 markov signal", "\u{fe63}0.523 markov signal"),
            ("5-day markov signal", "5\u{2011}day markov signal"),
            ("5-day markov signal", "5\u{2010}day markov signal"),
            ("five-confluence setup", "five\u{2011}confluence setup"),
            ("5 confluences", "5\u{00a0}confluences"),
            ("5 confluences", "5\u{202f}confluences"),
            ("4/3 confluences", "4/3\u{200b} confluences"),
            ("4/3 confluences", "4\u{00ad}/3 confluences"),
            ("5-6 confluences", "5\u{2012}6 confluences"),
            (
                "twenty-five confluences, markov signal -0.523",
                "twenty\u{2011}five confluences, markov signal -0.523",
            ),
        ] {
            let plain = numeric_checks(ascii, &evidence(5, 5));
            let written = numeric_checks(unicode, &evidence(5, 5));
            assert!(!plain.is_empty(), "{ascii} reads something");
            let reading = |checks: &[NumericCheck]| {
                checks
                    .iter()
                    .map(|check| (check.quoted, check.field, check.relation, check.verdict))
                    .collect::<Vec<_>>()
            };
            assert_eq!(reading(&plain), reading(&written), "{unicode:?}");
        }
    }

    /// Offsets and excerpts are the note's, not the canonical text's, so a
    /// figure is found where a reader sees it.
    #[test]
    fn offsets_and_excerpts_are_the_notes_own() {
        let note = "strong\u{2011}setup, five\u{2011}day markov signal \u{2212}0.523";
        let checks = numeric_checks(note, &evidence(5, 5));
        assert_eq!(
            checks.iter().map(|check| check.offset).collect::<Vec<_>>(),
            vec![note.find("five").unwrap(), note.find('\u{2212}').unwrap()]
        );
        assert_eq!(
            checks.iter().map(|check| check.verdict).collect::<Vec<_>>(),
            vec![NumericVerdict::Matches, NumericVerdict::Matches]
        );
        for check in &checks {
            assert!(check.excerpt.contains('\u{2011}'), "{}", check.excerpt);
            let span = figure_at(note, check.offset).expect("found where the note has it");
            assert!(note[span.start..span.end].contains(['5', 'f']));
        }
    }

    /// An en or em dash touching a digit is a minus sign the scanner does not
    /// read, or the far end of a range. The figure arrived unsigned, and was
    /// compared as positive.
    #[test]
    fn a_dash_touching_a_figure_abstains() {
        for note in ["markov signal \u{2013}0.523", "markov signal\u{2014}0.523"] {
            assert_eq!(
                verdicts(note, 5, 5),
                vec![(0.523, RELATION_UNSUPPORTED, "uncertain_attribution")],
                "{note}"
            );
        }
    }

    /// What `n14` read correctly and must still read: a digit figure beside a
    /// number word, a list joined by "and", a count, the N/M notation, and a
    /// spaced em dash, which separates clauses and joins no range.
    #[test]
    fn separate_figures_are_still_read() {
        assert_eq!(
            verdicts("and +0.551 five-day markov signal. hold", 5, 5)
                .iter()
                .filter(|(quoted, ..)| *quoted == 5.0)
                .count(),
            1
        );
        assert_eq!(
            verdicts("markov signed_signal -0.523 and 4/3 confluences", 4, 5),
            vec![
                (-0.523, RELATION_EQUALS, "matches"),
                (4.0, RELATION_EQUALS, "matches"),
                (3.0, RELATION_EQUALS, "not_in_evidence"),
            ]
        );
        assert_eq!(
            verdicts("four confluences", 4, 5),
            vec![(4.0, RELATION_EQUALS, "matches")]
        );
        assert_eq!(
            verdicts("rsi weak \u{2014} 5 confluences", 5, 5),
            vec![(5.0, RELATION_EQUALS, "matches")]
        );
    }
}

/// `n16`: a field's name counts only as a whole word. Regression cases, not
/// validation evidence: the defect was found by the fresh evaluation, which
/// therefore cannot validate the fix.
#[cfg(test)]
mod n16_whole_names {
    use super::*;
    use serde_json::json;

    fn evidence() -> JsonValue {
        json!({
            "daily_indicators": {
                "confluence_count": 5,
                "rsi14": 61.34,
                "reward_risk": 2.1,
                "support": {"nearest_support": 446.0},
            },
            "markov": {"horizon_days": 5, "signed_signal": -0.523},
        })
    }

    fn read(note: &str) -> Vec<(f64, Option<&'static str>, &'static str)> {
        numeric_checks(note, &evidence())
            .into_iter()
            .map(|check| (check.quoted, check.field, check.verdict.as_str()))
            .collect()
    }

    /// The fresh evaluation's case: "support" inside "supported" contested
    /// the count, and the checker abstained.
    #[test]
    fn a_name_inside_a_longer_word_names_nothing() {
        for note in [
            "actionable technical buy supported by 5 confluences",
            "unsupported by 5 confluences",
            "supportive setup with 5 confluences",
        ] {
            assert_eq!(
                read(note),
                vec![(5.0, Some("daily_indicators.confluence_count"), "matches")],
                "{note}"
            );
        }
    }

    /// The same rule where a clause must name a Markov field before a
    /// duration is read as the horizon.
    #[test]
    fn a_markov_name_inside_a_longer_word_does_not_make_a_horizon() {
        assert_eq!(
            read("5-day signal from a markovian model"),
            vec![(5.0, None, "not_a_field_value")]
        );
        assert_eq!(
            read("5-day signal from the markov model"),
            vec![(5.0, Some("markov.horizon_days"), "matches")]
        );
    }

    /// What a whole-word rule must not lose: a period written onto the
    /// indicator, a plural the table lists, a possessive, and an initialism.
    #[test]
    fn whole_names_are_still_read() {
        // Still named; the gap grammar does not read the "14", as before.
        assert_eq!(
            read("rsi14 61.3"),
            vec![(
                61.3,
                Some("daily_indicators.rsi14"),
                "uncertain_attribution"
            )]
        );
        assert_eq!(
            read("5 technical confluences"),
            vec![(5.0, Some("daily_indicators.confluence_count"), "matches")]
        );
        // Still named; the "'s" is outside the gap grammar, as before.
        assert_eq!(
            read("markov's signal -0.523"),
            vec![(
                -0.523,
                Some("markov.signed_signal"),
                "uncertain_attribution"
            )]
        );
        assert_eq!(
            read("r/r 2.1"),
            vec![(2.1, Some("daily_indicators.reward_risk"), "matches")]
        );
        assert_eq!(
            read("above 446 eur support"),
            vec![(
                446.0,
                Some("daily_indicators.support.nearest_support"),
                "matches"
            )]
        );
    }
}
