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
/// **Input.** A duplicate id, a label for a case not in the key, or a verdict
/// other than `consistent`, `inconsistent` or `cannot_tell` rejects the whole
/// file, and nothing is scored until it is corrected. Keeping one of two
/// duplicates would let whichever came last decide the result, so identical
/// duplicates are rejected too: one rule, no judgement about which conflicts
/// matter. An empty verdict is a case not yet labelled.
///
/// **Two kinds of unresolved, kept apart.** `unlabelled` is unfinished work;
/// `cannot_tell` is a finding. While any case is unlabelled the population
/// estimate is withheld, because a stratum missing from it would change the
/// population it describes.
///
/// **A rate among settled cases is conditional, and named so.** It says
/// nothing about the `cannot_tell` cases, which may be exactly the hard ones.
///
/// **The population figure is an interval, and the one that assumes nothing
/// about the unresolved cases is the headline.** Within a stratum, every
/// `cannot_tell` is counted once as consistent and once as wrong. Sampling
/// widens each end by an **exact hypergeometric bound** — the distribution of
/// errors found when drawing without replacement from this fixed population —
/// at `1 − 0.05/S` for `S` sampled strata. Each stratum's bound covers its true
/// error count at least `1 − 0.05/S` of the time, whatever that count is, so
/// the population sum covers at least 95% (Bonferroni). The guarantee assumes
/// the hash-ordered draw behaves as a simple random sample within each stratum
/// and that the labels are right; it says nothing about label error. A stratum
/// taken whole is exact. Strata are weighted by population share, and **every
/// stratum stays in the denominator**: one with nothing settled contributes its
/// full width instead of leaving the estimate.
///
/// v3 used Wilson bounds here and called the result "about 95%". Two wrong
/// claims among the 1,153 `matches` showed it covering 89.9%: Wilson's lower
/// bound on one error in sixty sits above the true proportion.
///
/// **A point estimate appears only under a stated assumption** — that the
/// unresolved cases in each stratum resemble the settled ones — beside the
/// interval, never instead of it, and not at all when some stratum has nothing
/// settled for the assumption to extrapolate from.
///
/// Wrong claims among sampled `matches` estimate **the error proportion among
/// claims the checker accepted** — not a conventional false-negative rate,
/// which would need a denominator of all errors. `missed_by_abstention` stays
/// separately visible and also counts toward **errors the system did not flag,
/// end to end**.
pub(crate) const PROTOCOL_VERSION: &str = "controls-protocol-v4-2026-09-23";

/// The joint coverage the population interval is built for.
const JOINT_LEVEL: f64 = 0.95;

/// The only verdicts a label may carry. Anything else rejects the file.
const LABEL_VERDICTS: [&str; 3] = ["consistent", "inconsistent", "cannot_tell"];

/// Errors found in a sample drawn without replacement from a fixed population
/// of claims: the hypergeometric distribution, computed exactly rather than
/// approximated.
struct Hypergeometric {
    population: usize,
    sample: usize,
    ln_factorial: Vec<f64>,
}

impl Hypergeometric {
    fn new(population: usize, sample: usize) -> Self {
        let mut ln_factorial = vec![0.0; population + 1];
        for k in 1..=population {
            ln_factorial[k] = ln_factorial[k - 1] + (k as f64).ln();
        }
        Self {
            population,
            sample,
            ln_factorial,
        }
    }

    fn ln_choose(&self, n: usize, k: usize) -> f64 {
        self.ln_factorial[n] - self.ln_factorial[k] - self.ln_factorial[n - k]
    }

    /// The chance the sample finds `found` wrong when the population holds
    /// `wrong`.
    fn pmf(&self, wrong: usize, found: usize) -> f64 {
        if found > wrong || found > self.sample || self.sample - found > self.population - wrong {
            return 0.0;
        }
        (self.ln_choose(wrong, found)
            + self.ln_choose(self.population - wrong, self.sample - found)
            - self.ln_choose(self.population, self.sample))
        .exp()
    }

    /// The fewest wrong claims the population can hold for which finding
    /// `found` or more is not rarer than `tail`.
    fn lower(&self, found: usize, tail: f64) -> usize {
        (0..=self.population)
            .find(|&wrong| {
                (found..=self.sample)
                    .map(|k| self.pmf(wrong, k))
                    .sum::<f64>()
                    > tail
            })
            .unwrap_or(self.population)
    }

    /// The most wrong claims the population can hold for which finding
    /// `found` or fewer is not rarer than `tail`.
    fn upper(&self, found: usize, tail: f64) -> usize {
        (0..=self.population)
            .rev()
            .find(|&wrong| (0..=found).map(|k| self.pmf(wrong, k)).sum::<f64>() > tail)
            .unwrap_or(0)
    }
}

/// One stratum's labels, counted.
#[derive(Default)]
struct Tally {
    sampled: i64,
    settled: i64,
    cannot_tell: i64,
    unlabelled: i64,
    wrong: i64,
    unflagged: i64,
    /// `cannot_tell` cases that would go unflagged if they were wrong — every
    /// one the checker did not itself flag.
    unresolved_unflagged: i64,
}

/// The confusion tables, both axes, by stratum, and the population interval.
pub(crate) fn score(
    keys: &[ControlKey],
    labels: &[ControlLabel],
    population: &std::collections::BTreeMap<String, i64>,
) -> JsonValue {
    use std::collections::{BTreeMap, BTreeSet};

    let known: BTreeSet<&str> = keys.iter().map(|key| key.id.as_str()).collect();
    let mut grouped: BTreeMap<&str, Vec<&ControlLabel>> = BTreeMap::new();
    let (mut unknown, mut unreadable) = (BTreeSet::new(), BTreeSet::new());
    for label in labels {
        if !known.contains(label.id.as_str()) {
            unknown.insert(label.id.as_str());
            continue;
        }
        let verdict = label.verdict.trim();
        if !verdict.is_empty() && !LABEL_VERDICTS.contains(&verdict) {
            unreadable.insert(label.id.as_str());
        }
        grouped.entry(label.id.as_str()).or_default().push(label);
    }
    let duplicates: BTreeSet<&str> = grouped
        .iter()
        .filter(|(_, labels)| labels.len() > 1)
        .map(|(id, _)| *id)
        .collect();
    let mut strata: BTreeSet<&str> = keys.iter().map(|key| key.stratum.as_str()).collect();
    strata.extend(population.keys().map(String::as_str));
    if !duplicates.is_empty() || !unknown.is_empty() || !unreadable.is_empty() {
        // Sets, so the report is the same whatever order the file was in.
        return json!({
            "version": CONTROLS_VERSION,
            "protocol": PROTOCOL_VERSION,
            "status": "rejected",
            "label_problems": {
                "duplicate_ids": duplicates,
                "unknown_ids": unknown,
                "unreadable_verdicts": unreadable,
            },
            "reading": "Rejected, and nothing is scored until the input is corrected. Keeping \
                        one of two duplicate labels would let whichever came last decide the \
                        result.",
        });
    }
    let by_id: BTreeMap<&str, &ControlLabel> = grouped
        .into_iter()
        .map(|(id, labels)| (id, labels[0]))
        .collect();

    let mut verdicts: BTreeMap<&str, BTreeMap<&'static str, i64>> = BTreeMap::new();
    let mut fields: BTreeMap<&str, BTreeMap<&'static str, i64>> = BTreeMap::new();
    let mut verdict_totals: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut field_totals: BTreeMap<&'static str, i64> = BTreeMap::new();
    let mut tallies: BTreeMap<&str, Tally> = strata
        .iter()
        .map(|stratum| (*stratum, Tally::default()))
        .collect();
    for key in keys {
        let label = by_id.get(key.id.as_str()).copied();
        let outcome = outcome_for(key, label);
        let field = field_agreement(key, label);
        *verdicts
            .entry(key.stratum.as_str())
            .or_default()
            .entry(outcome.as_str())
            .or_default() += 1;
        *fields
            .entry(key.stratum.as_str())
            .or_default()
            .entry(field.as_str())
            .or_default() += 1;
        *verdict_totals.entry(outcome.as_str()).or_default() += 1;
        *field_totals.entry(field.as_str()).or_default() += 1;
        let tally = tallies.entry(key.stratum.as_str()).or_default();
        tally.sampled += 1;
        match outcome {
            Outcome::Unlabelled => tally.unlabelled += 1,
            Outcome::Unsettled => {
                tally.cannot_tell += 1;
                tally.unresolved_unflagged += i64::from(key.verdict != "differs");
            }
            _ => {
                tally.settled += 1;
                tally.wrong += i64::from(outcome.labeller_says_wrong());
                tally.unflagged += i64::from(matches!(
                    outcome,
                    Outcome::FalseNegative | Outcome::MissedByAbstention
                ));
            }
        }
    }

    let size = |stratum: &str| population.get(stratum).copied().unwrap_or(0);
    // A key whose population is smaller than its own sample cannot be
    // weighted, and says so rather than being bounded as if it could.
    let valid = |stratum: &str, tally: &Tally| size(stratum) > 0 && tally.sampled <= size(stratum);
    let census = |stratum: &str, tally: &Tally| tally.sampled > 0 && tally.sampled == size(stratum);
    let inconsistent: Vec<&str> = tallies
        .iter()
        .filter(|(stratum, tally)| !valid(stratum, tally))
        .map(|(stratum, _)| *stratum)
        .collect();
    // Alpha is spent only where there is sampling error to cover.
    let sampled_strata = tallies
        .iter()
        .filter(|(stratum, tally)| {
            valid(stratum, tally) && tally.sampled > 0 && !census(stratum, tally)
        })
        .count()
        .max(1);
    let tail = (1.0 - JOINT_LEVEL) / (2.0 * sampled_strata as f64);
    // The widest reading of one stratum, as counts of wrong claims in its
    // population: `low` counts every unresolved case as consistent, `high`
    // counts it as wrong, and the exact sampling bound widens both ends. A
    // stratum taken whole comes out exact; one not sampled at all, `0..=N`.
    let bounds = |stratum: &str, tally: &Tally, low: i64, high: i64| -> Option<(i64, i64)> {
        if !valid(stratum, tally) {
            return None;
        }
        let draw = Hypergeometric::new(size(stratum) as usize, tally.sampled as usize);
        Some((
            draw.lower(low as usize, tail) as i64,
            draw.upper(high as usize, tail) as i64,
        ))
    };
    let conditional = |x: i64, of: i64| {
        if of == 0 {
            JsonValue::Null
        } else {
            json!(x as f64 / of as f64)
        }
    };

    let mut per_stratum = BTreeMap::new();
    for (stratum, tally) in &tallies {
        let interval = |low: i64, high: i64| match bounds(stratum, tally, low, high) {
            Some((from, to)) if tally.unlabelled == 0 => json!({
                "proportion": [
                    from as f64 / size(stratum) as f64,
                    to as f64 / size(stratum) as f64
                ],
                "wrong_in_population": [from, to],
            }),
            _ => JsonValue::Null,
        };
        per_stratum.insert(
            *stratum,
            json!({
                "population": size(stratum),
                "sampled": tally.sampled,
                "census": census(stratum, tally),
                "settled": tally.settled,
                "cannot_tell": tally.cannot_tell,
                "unlabelled": tally.unlabelled,
                "labelled_wrong": tally.wrong,
                "not_flagged_and_wrong": tally.unflagged,
                "among_settled_only": {
                    "labelled_wrong": conditional(tally.wrong, tally.settled),
                    "not_flagged_and_wrong": conditional(tally.unflagged, tally.settled),
                },
                "interval": {
                    "labelled_wrong": interval(tally.wrong, tally.wrong + tally.cannot_tell),
                    "not_flagged_and_wrong": interval(
                        tally.unflagged,
                        tally.unflagged + tally.unresolved_unflagged
                    ),
                },
            }),
        );
    }

    let unlabelled: i64 = tallies.values().map(|tally| tally.unlabelled).sum();
    let total: i64 = strata.iter().map(|stratum| size(stratum)).sum();
    let nothing_settled: Vec<&str> = tallies
        .iter()
        .filter(|(_, tally)| tally.settled == 0)
        .map(|(stratum, _)| *stratum)
        .collect();
    let estimate = |pick: &dyn Fn(&Tally) -> (i64, i64)| {
        let (mut from, mut to, mut point) = (0i64, 0i64, Some(0.0));
        for (stratum, tally) in &tallies {
            let share = size(stratum) as f64 / total as f64;
            let (wrong, unresolved) = pick(tally);
            let (low, high) =
                bounds(stratum, tally, wrong, wrong + unresolved).unwrap_or((0, size(stratum)));
            from += low;
            to += high;
            point = point
                .filter(|_| tally.settled > 0)
                .map(|sum| sum + share * wrong as f64 / tally.settled as f64);
        }
        json!({
            "interval": [from as f64 / total as f64, to as f64 / total as f64],
            "wrong_in_population": [from, to],
            "if_unresolved_resemble_settled": point,
        })
    };
    let status = if unlabelled > 0 {
        "labelling_incomplete"
    } else {
        "complete"
    };
    let withheld = if unlabelled > 0 {
        Some(format!(
            "{unlabelled} cases are unlabelled; unfinished work is not ambiguity"
        ))
    } else if !inconsistent.is_empty() || total == 0 {
        Some(format!(
            "the key's population does not cover its own sample for {inconsistent:?}"
        ))
    } else {
        None
    };

    json!({
        "version": CONTROLS_VERSION,
        "protocol": PROTOCOL_VERSION,
        "status": status,
        "cases": keys.len(),
        "labelled": keys.len() as i64 - unlabelled,
        "label_problems": {
            "duplicate_ids": [],
            "unknown_ids": [],
            "unreadable_verdicts": [],
        },
        "verdict_axis": {"by_stratum": verdicts, "totals": verdict_totals},
        "attribution_axis": {"by_stratum": fields, "totals": field_totals},
        "per_stratum": per_stratum,
        "population_estimate_withheld_because": withheld,
        "population_estimate": if withheld.is_some() {
            JsonValue::Null
        } else {
            json!({
                "population": total,
                "joint_level": JOINT_LEVEL,
                "sampled_strata": sampled_strata,
                "tail_per_stratum": tail,
                "method": "exact hypergeometric bounds per sampled stratum, Bonferroni across \
                           them; unresolved cases counted both ways",
                "claims_the_labeller_reads_as_wrong":
                    estimate(&|tally| (tally.wrong, tally.cannot_tell)),
                "wrong_and_not_flagged_end_to_end":
                    estimate(&|tally| (tally.unflagged, tally.unresolved_unflagged)),
                "point_withheld_because_nothing_settled_in": nothing_settled,
                "assumption_behind_the_point": "the unresolved cases in each stratum resemble \
                                                the settled ones. The interval does not assume it.",
            })
        },
        "reading": "The two axes are separate on purpose. A verdict that agrees about a \
                    different field is not agreement, and a blank field is `unknown` rather \
                    than assumed to match. A rate among settled cases says nothing about the \
                    `cannot_tell` ones. The population figure is the interval, which counts \
                    every unresolved case both ways, widens sampled strata by an exact \
                    hypergeometric bound and keeps every stratum in the denominator; it is withheld while any case \
                    is unlabelled. Totals across strata describe the sample, not the \
                    population.",
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

    fn case(id: &str, stratum: &str) -> ControlKey {
        ControlKey {
            id: id.into(),
            stratum: stratum.into(),
            verdict: stratum.into(),
            field: None,
        }
    }

    fn labelled(id: &str, verdict: &str) -> ControlLabel {
        ControlLabel {
            id: id.into(),
            verdict: verdict.into(),
            ..ControlLabel::default()
        }
    }

    fn interval(report: &JsonValue, metric: &str) -> (f64, f64) {
        let pair = &report["population_estimate"][metric]["interval"];
        (
            pair[0].as_f64().expect("from"),
            pair[1].as_f64().expect("to"),
        )
    }

    /// A duplicate was reported and then used anyway: the later label
    /// overwrote the earlier, so two files differing only in order scored
    /// differently under the same warning. Now the input is rejected, and the
    /// rejection is the same whatever the order.
    #[test]
    fn duplicate_labels_reject_the_input_whatever_their_order() {
        let keys = vec![case("c", "matches")];
        let population = BTreeMap::from([("matches".to_string(), 10i64)]);
        let first = labelled("c", "consistent");
        let second = labelled("c", "inconsistent");
        let forward = score(&keys, &[first.clone(), second.clone()], &population);
        let reversed = score(&keys, &[second, first.clone()], &population);
        assert_eq!(
            forward, reversed,
            "the order of the file decided the result"
        );
        assert_eq!(forward["status"], "rejected");
        assert_eq!(forward["label_problems"]["duplicate_ids"][0], "c");
        for withheld in [
            "per_stratum",
            "population_estimate",
            "verdict_axis",
            "attribution_axis",
        ] {
            assert!(
                forward.get(withheld).is_none(),
                "{withheld} published from rejected input"
            );
        }
        // Agreeing duplicates are still duplicates: one rule, no judgement
        // about which conflicts matter.
        let agreeing = score(&keys, &[first.clone(), first], &population);
        assert_eq!(agreeing["status"], "rejected");
    }

    /// A label for a case not in the key, or a verdict nobody defined, is a
    /// mistake in the file, not a case to score around.
    #[test]
    fn unknown_ids_and_unreadable_verdicts_reject_the_input() {
        let keys = vec![case("c", "matches")];
        let population = BTreeMap::from([("matches".to_string(), 10i64)]);
        let stray = score(
            &keys,
            &[
                labelled("c", "consistent"),
                labelled("not-a-case", "consistent"),
            ],
            &population,
        );
        assert_eq!(stray["status"], "rejected");
        assert_eq!(stray["label_problems"]["unknown_ids"][0], "not-a-case");
        let typo = score(&keys, &[labelled("c", "consistant")], &population);
        assert_eq!(typo["status"], "rejected");
        assert_eq!(typo["label_problems"]["unreadable_verdicts"][0], "c");
    }

    /// Rates are per stratum and conditional on the settled cases. The
    /// population figure weights each stratum by its share, and one case per
    /// stratum leaves it very wide.
    #[test]
    fn rates_are_per_stratum_and_the_population_figure_is_weighted() {
        let keys = vec![case("a", "matches"), case("b", "unattributed")];
        let labels = [labelled("a", "inconsistent"), labelled("b", "consistent")];
        let population = BTreeMap::from([
            ("matches".to_string(), 1000i64),
            ("unattributed".to_string(), 100i64),
        ]);
        let report = score(&keys, &labels, &population);
        assert_eq!(report["status"], "complete");
        let settled_only = |stratum: &str| {
            report["per_stratum"][stratum]["among_settled_only"]["labelled_wrong"].clone()
        };
        assert_eq!(settled_only("matches"), 1.0);
        assert_eq!(settled_only("unattributed"), 0.0);
        // 1000/1100 of the population sits in the stratum that came back
        // wrong — under the assumption, and inside the interval.
        let point = report["population_estimate"]["claims_the_labeller_reads_as_wrong"]
            ["if_unresolved_resemble_settled"]
            .as_f64()
            .expect("point");
        assert!((point - 1000.0 / 1100.0).abs() < 1e-9, "{point}");
        let (from, to) = interval(&report, "claims_the_labeller_reads_as_wrong");
        assert!(from < point && point < to, "{from}..{to}");
        assert!(to - from > 0.5, "one case a stratum gave {from}..{to}");
    }

    /// A stratum with nothing settled used to drop out of the weighted
    /// denominator, so the estimate quietly described a different population.
    /// It stays, at its full width.
    #[test]
    fn a_wholly_unresolved_stratum_widens_the_estimate_rather_than_leaving_it() {
        // Both strata taken whole, so the width is the ambiguity alone.
        let keys = vec![
            case("a1", "matches"),
            case("a2", "matches"),
            case("b1", "unattributed"),
            case("b2", "unattributed"),
        ];
        let labels = [
            labelled("a1", "consistent"),
            labelled("a2", "consistent"),
            labelled("b1", "cannot_tell"),
            labelled("b2", "cannot_tell"),
        ];
        let population = BTreeMap::from([
            ("matches".to_string(), 2i64),
            ("unattributed".to_string(), 2i64),
        ]);
        let report = score(&keys, &labels, &population);
        assert_eq!(report["status"], "complete");
        assert_eq!(
            report["population_estimate"]["population"], 4,
            "the unresolved stratum left the denominator"
        );
        assert_eq!(
            report["per_stratum"]["unattributed"]["among_settled_only"]["labelled_wrong"],
            JsonValue::Null
        );
        for metric in [
            "claims_the_labeller_reads_as_wrong",
            "wrong_and_not_flagged_end_to_end",
        ] {
            assert_eq!(interval(&report, metric), (0.0, 0.5), "{metric}");
            assert_eq!(
                report["population_estimate"][metric]["if_unresolved_resemble_settled"],
                JsonValue::Null,
                "nothing settled for the assumption to extrapolate from"
            );
        }
        assert_eq!(
            report["population_estimate"]["point_withheld_because_nothing_settled_in"][0],
            "unattributed"
        );
    }

    /// One settled label must not speak for a stratum whose other cases could
    /// not be settled. The conditional rate says nothing is wrong; the
    /// interval says the labels allow nearly anything.
    #[test]
    fn a_mostly_unresolved_stratum_is_not_carried_by_its_one_settled_label() {
        let keys: Vec<ControlKey> = (0..10).map(|n| case(&format!("w{n}"), "matches")).collect();
        let mut labels = vec![labelled("w0", "consistent")];
        labels.extend((1..10).map(|n| labelled(&format!("w{n}"), "cannot_tell")));

        // Taken whole: the width is the ambiguity alone.
        let whole = score(
            &keys,
            &labels,
            &BTreeMap::from([("matches".to_string(), 10i64)]),
        );
        let stratum = &whole["per_stratum"]["matches"];
        assert_eq!(stratum["among_settled_only"]["labelled_wrong"], 0.0);
        assert_eq!(stratum["cannot_tell"], 9);
        assert_eq!(
            interval(&whole, "claims_the_labeller_reads_as_wrong"),
            (0.0, 0.9)
        );

        // Sampled, as the real `matches` is: sampling widens it further.
        let sampled = score(
            &keys,
            &labels,
            &BTreeMap::from([("matches".to_string(), 1153i64)]),
        );
        let (from, to) = interval(&sampled, "claims_the_labeller_reads_as_wrong");
        assert!(from < 1e-12 && to > 0.9, "{from}..{to}");
    }

    /// Unfinished work is not ambiguity. While any case is unlabelled the
    /// population figure is withheld; the counts and conditional rates still
    /// show progress.
    #[test]
    fn incomplete_labelling_withholds_the_population_estimate() {
        let keys = vec![
            case("a", "matches"),
            case("b", "matches"),
            case("c", "matches"),
        ];
        let labels = [
            labelled("a", "consistent"),
            labelled("b", "cannot_tell"),
            labelled("c", ""),
        ];
        let report = score(
            &keys,
            &labels,
            &BTreeMap::from([("matches".to_string(), 100i64)]),
        );
        assert_eq!(report["status"], "labelling_incomplete");
        assert_eq!(report["population_estimate"], JsonValue::Null);
        let stratum = &report["per_stratum"]["matches"];
        assert_eq!(
            (
                stratum["settled"].as_i64(),
                stratum["cannot_tell"].as_i64(),
                stratum["unlabelled"].as_i64()
            ),
            (Some(1), Some(1), Some(1))
        );
        assert_eq!(stratum["interval"]["labelled_wrong"], JsonValue::Null);
        assert_eq!(stratum["among_settled_only"]["labelled_wrong"], 0.0);
    }

    /// A stratum taken whole has no sampling error. One drawn from a larger
    /// population has it, even when every label came back clean.
    #[test]
    fn a_clean_sample_still_carries_sampling_error() {
        let keys: Vec<ControlKey> = (0..60).map(|n| case(&format!("m{n}"), "matches")).collect();
        let labels: Vec<ControlLabel> = (0..60)
            .map(|n| labelled(&format!("m{n}"), "consistent"))
            .collect();
        let whole = score(
            &keys,
            &labels,
            &BTreeMap::from([("matches".to_string(), 60i64)]),
        );
        assert_eq!(
            interval(&whole, "claims_the_labeller_reads_as_wrong"),
            (0.0, 0.0)
        );
        let drawn = score(
            &keys,
            &labels,
            &BTreeMap::from([("matches".to_string(), 1153i64)]),
        );
        // Nought of 60 drawn from 1,153: up to 66 wrong claims are still
        // not rarer than 2.5% to miss entirely.
        assert_eq!(
            drawn["population_estimate"]["claims_the_labeller_reads_as_wrong"]["wrong_in_population"],
            json!([0, 66])
        );
    }

    /// The five sampled strata of `controls-v1`, as (population, drawn).
    const SAMPLED_STRATA: [(usize, usize); 5] = [(1153, 60), (151, 18), (48, 12), (56, 8), (22, 8)];

    /// The guarantee, enumerated rather than asserted. For every error count
    /// each sampled stratum could hold, the exact bound covers it at least
    /// `1 − 0.05/5` of the time; Bonferroni over the five then gives at least
    /// 95% for the population sum. Testing the formula at a few points is how
    /// v3's Wilson bounds under-covered unnoticed.
    #[test]
    fn every_sampled_stratum_covers_whatever_its_true_error_count() {
        let tail = (1.0 - JOINT_LEVEL) / (2.0 * SAMPLED_STRATA.len() as f64);
        for (population, sample) in SAMPLED_STRATA {
            let draw = Hypergeometric::new(population, sample);
            let bounds: Vec<(usize, usize)> = (0..=sample)
                .map(|found| (draw.lower(found, tail), draw.upper(found, tail)))
                .collect();
            let mut worst = (1.0f64, 0usize);
            for wrong in 0..=population {
                let covered: f64 = bounds
                    .iter()
                    .enumerate()
                    .filter(|(_, (low, high))| *low <= wrong && wrong <= *high)
                    .map(|(found, _)| draw.pmf(wrong, found))
                    .sum();
                if covered < worst.0 {
                    worst = (covered, wrong);
                }
            }
            println!(
                "{sample} of {population}: worst coverage {:.5} at {} wrong",
                worst.0, worst.1
            );
            assert!(
                worst.0 >= 1.0 - 2.0 * tail - 1e-9,
                "{sample} of {population}: covers {} when {} are wrong",
                worst.0,
                worst.1
            );
        }
    }

    /// The case that showed v3 under-covering: two wrong claims among the
    /// 1,153 `matches`, every other claim right, so 2 of 1,445 overall. A
    /// sample finds one or both 10.1% of the time, and Wilson's lower bound
    /// then sat above the truth, covering 89.9%. Enumerated exactly, through
    /// the scorer.
    #[test]
    fn two_wrong_accepted_claims_are_covered() {
        let frozen = [
            ("matches", 1153i64, 60usize),
            ("unattributed", 151, 18),
            ("uncertain_attribution", 48, 12),
            ("not_in_evidence", 56, 8),
            ("not_a_field_value", 22, 8),
            ("implausible_attribution", 6, 6),
            ("differs", 9, 9),
        ];
        let population: BTreeMap<String, i64> = frozen
            .iter()
            .map(|(stratum, size, _)| (stratum.to_string(), *size))
            .collect();
        let keys: Vec<ControlKey> = frozen
            .iter()
            .flat_map(|(stratum, _, drawn)| {
                (0..*drawn).map(move |n| case(&format!("{stratum}-{n}"), stratum))
            })
            .collect();
        let draw = Hypergeometric::new(1153, 60);
        let truth = 2.0 / 1445.0;
        let mut covered = 0.0;
        for found in 0..=2 {
            let labels: Vec<ControlLabel> = keys
                .iter()
                .map(|key| {
                    let wrong = (0..found).any(|n| key.id == format!("matches-{n}"));
                    labelled(&key.id, if wrong { "inconsistent" } else { "consistent" })
                })
                .collect();
            let report = score(&keys, &labels, &population);
            let (from, to) = interval(&report, "claims_the_labeller_reads_as_wrong");
            if from <= truth && truth <= to {
                covered += draw.pmf(2, found);
            }
        }
        println!("two wrong accepted claims: covered {covered:.5}");
        assert!(covered >= 0.95, "covers {covered}");
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

    /// The plan the key was issued under is the plan the scorer runs.
    #[test]
    fn the_key_carries_the_protocol_in_force() {
        let Ok(raw) = std::fs::read_to_string(KEY_PATH) else {
            return;
        };
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        assert_eq!(document["protocol"], PROTOCOL_VERSION);
    }

    /// What this method would report from a perfect result, fixed before any
    /// label exists. If every one of the 121 labels came back `consistent`,
    /// the population interval would still run to 12.3%, and `matches` alone
    /// to 8.2%. Those are outputs of this method — Bonferroni and exact
    /// discreteness are both conservative — not a limit on what the sample
    /// could establish.
    #[test]
    fn the_best_this_sample_can_say() {
        let keys: Vec<ControlKey> = load(KEY_PATH, "keys");
        if keys.is_empty() {
            return;
        }
        let raw = std::fs::read_to_string(KEY_PATH).expect("key");
        let document: JsonValue = serde_json::from_str(&raw).expect("json");
        let population: BTreeMap<String, i64> =
            serde_json::from_value(document["population"].clone()).expect("population");
        let clean: Vec<ControlLabel> = keys
            .iter()
            .map(|key| ControlLabel {
                id: key.id.clone(),
                verdict: "consistent".into(),
                ..ControlLabel::default()
            })
            .collect();
        let report = score(&keys, &clean, &population);
        assert_eq!(report["population_estimate"]["sampled_strata"], 5);
        // 178 of 1,445 overall, 12.3%; 94 of 1,153 accepted claims, 8.2%.
        assert_eq!(
            report["population_estimate"]["claims_the_labeller_reads_as_wrong"]["wrong_in_population"],
            json!([0, 178])
        );
        assert_eq!(
            report["per_stratum"]["matches"]["interval"]["labelled_wrong"]["wrong_in_population"],
            json!([0, 94])
        );
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
        assert_ne!(
            report["status"], "rejected",
            "the labels file is malformed and nothing was scored: {report:#}"
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
