//! fresh-v2: the second held-out evaluation, fixed before its frame exists.
//!
//! fresh-v1 measured `n15`; review of its results and of the fixes since asked
//! for three changes before the next one, and this version makes them:
//!
//! - **The conventions are stated, not chosen by the labeller.** A figure is
//!   consistent if it is the stored value rounded at the precision written,
//!   either way at a half; a truncation rounding does not produce is not. A
//!   signed figure beside "conviction" states the signed signal. Both are in
//!   the rubric, and the key's own arithmetic follows the first.
//! - **Valid-context changes are kept apart from evidence corruption.** A
//!   change to the evidence moves every field the Markov model links to the
//!   one it changes, keeps every field inside its bounds, and is dropped if
//!   `context_problems` finds anything. A single-field change that breaks
//!   those links is still made, but only in its own `corruption` family,
//!   whose question is just whether the checker reads the field it names.
//! - **A removal takes the field and everything that determines it**, so an
//!   "unsettleable" figure is one no linked field can settle.
//!
//! The method under evaluation is `n20`, frozen at `07cf157`. The frame is
//! prospective: every completed report created after the freeze, up to the
//! close. None exists yet, so nothing in it can have shaped the method.

use super::*;

pub(crate) const FRESH_V2_VERSION: &str = "fresh-v2-2026-09-26";
/// Everything v1 fixed, plus the conventions and the two families above.
pub(crate) const PROTOCOL_V2: &str = "fresh-protocol-v2-2026-09-26";
/// The method under evaluation. Nothing is scored under any other.
pub(crate) const FROZEN_METHOD_V2: &str = "n20-2026-09-26";
pub(crate) const FROZEN_COMMIT_V2: &str = "07cf157";
/// Names each note; fixed, recorded, used for nothing else.
pub(crate) const SEED_V2: &str = "jev-fresh-v2";
/// The frame opens when `n20` was frozen: `07cf157`'s commit time.
pub(crate) const FRAME_OPENS: &str = "2026-09-26T09:39:03Z";
/// And closes at the end of the second full trading week after it. A default
/// Simon may move before the frame is built, never after.
pub(crate) const FRAME_CLOSES: &str = "2026-10-10T00:00:00Z";

/// The conventions a labeller applies, stated in the instrument.
pub(crate) const CONVENTIONS: &[(&str, &str)] = &[
    (
        "rounding",
        "A figure written short is consistent only if it is the stored value rounded to \
         the precision written, either way at an exact half. A truncation that rounding \
         does not produce is inconsistent: against 502.89, \"503\" is consistent and \
         \"502\" is not.",
    ),
    (
        "signed_conviction",
        "Beside the word \"conviction\", a figure written with a sign (+ or -) states \
         markov.signed_signal. An unsigned figure states markov.conviction, the magnitude.",
    ),
    (
        "missing_evidence",
        "Missing evidence is cannot_tell, never inconsistent.",
    ),
];

/// Whether a report's creation time falls inside the frame. Timestamps are
/// the database's UTC `YYYY-MM-DDTHH:MM:SSZ`, which order as strings.
pub(crate) fn in_frame(created_at: &str) -> bool {
    created_at.len() == FRAME_OPENS.len()
        && created_at.ends_with('Z')
        && created_at > FRAME_OPENS
        && created_at < FRAME_CLOSES
}

/// Whether a written figure is the stored value rounded at the precision
/// written -- the settled convention -- by the key's own arithmetic. `None`
/// where a double cannot settle it, and such a figure seeds nothing.
///
/// Decided in whole units of the last written place, allowing only a bound on
/// the binary error of the scaled stored value. Past what a double resolves,
/// only the stored double itself is its rounding.
pub(crate) fn rounds(written: Written, stored: f64, discrete: bool) -> Option<bool> {
    if discrete {
        return Some(written.decimals == 0 && (written.value - stored).abs() < 1e-9);
    }
    let factor = 10f64.powi(written.decimals.min(22) as i32);
    let scaled = stored * factor;
    let error = 4.0 * f64::EPSILON * scaled.abs().max(1.0);
    if error <= 0.05 {
        return Some((scaled - (written.value * factor).round()).abs() <= 0.5 + error);
    }
    if written.value == stored {
        Some(true)
    } else if (written.value - stored).abs() * factor > 0.5 + error {
        Some(false)
    } else {
        None
    }
}

/// The truth of a written figure against the evidence as it now stands.
fn truth_v2(figure: &str, field: &str, evidence: &JsonValue) -> Option<&'static str> {
    let Some(stored) = number_at(evidence, field) else {
        return Some("unsettleable");
    };
    let written = parse_written(figure)?;
    Some(if rounds(written, stored, discrete(field))? {
        "holds"
    } else {
        "fails"
    })
}

/// The anchors in one labelled note: claims the labeller reads as consistent
/// whose figure the key's arithmetic confirms under the convention.
fn anchors_v2(case: &NoteCase, label: &FreshLabel) -> Vec<Anchor> {
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
        if rounds(written, stored, discrete(field)) != Some(true)
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

/// The Markov fields the model computes together.
const MARKOV_LINKED: &[&str] = &[
    "markov.signed_signal",
    "markov.conviction",
    "markov.bull_prob",
    "markov.bear_prob",
];

fn set_direction(evidence: &mut JsonValue, signal: f64) {
    if let Some(markov) = evidence
        .get_mut("markov")
        .and_then(JsonValue::as_object_mut)
    {
        let direction = if signal > 1e-9 {
            "long"
        } else if signal < -1e-9 {
            "short"
        } else {
            "flat"
        };
        markov.insert("direction".to_string(), json!(direction));
    }
}

/// Whether the support's distance is derived from the close in this evidence,
/// as it is in most notes but not all.
fn support_distance_linked(evidence: &JsonValue) -> bool {
    let (Some(close), Some(support), Some(distance)) = (
        number_at(evidence, "daily_indicators.close"),
        number_at(evidence, PRICE_LEVEL),
        number_at(evidence, "daily_indicators.support.downside_to_support_pct"),
    ) else {
        return false;
    };
    close > 0.0 && (distance - (close - support) / close * 100.0).abs() < 0.01
}

/// The evidence with `field` set to `value` and every field the model links
/// to it moved with it, and the linked fields moved. `None` if that takes a
/// field outside its bounds, or leaves evidence `context_problems` rejects.
pub(crate) fn coherently(
    evidence: &JsonValue,
    field: &str,
    value: f64,
) -> Option<(JsonValue, Vec<String>)> {
    let mut next = evidence.clone();
    let mut linked = Vec::new();
    if MARKOV_LINKED.contains(&field) {
        let markov = |name: &str| number_at(evidence, &format!("markov.{name}"));
        let sideways = markov("sideways_prob")?;
        let signal = markov("signed_signal")?;
        // The sideways probability stays; the others follow the new signal.
        let new_signal = match field {
            "markov.signed_signal" => value,
            "markov.conviction" if signal < 0.0 => -value,
            "markov.conviction" => value,
            "markov.bull_prob" => 2.0 * value - (1.0 - sideways),
            _ => (1.0 - sideways) - 2.0 * value,
        };
        let bull = (1.0 - sideways + new_signal) / 2.0;
        let bear = (1.0 - sideways - new_signal) / 2.0;
        let inside = |v: f64, low: f64, high: f64| (low..=high).contains(&v);
        if !inside(bull, 0.0, 1.0) || !inside(bear, 0.0, 1.0) || !inside(new_signal, -1.0, 1.0) {
            return None;
        }
        for (path, moved) in [
            ("markov.signed_signal", new_signal),
            ("markov.conviction", new_signal.abs()),
            ("markov.bull_prob", bull),
            ("markov.bear_prob", bear),
        ] {
            set_number(&mut next, path, Some(moved));
            if path != field {
                linked.push(path.to_string());
            }
        }
        set_direction(&mut next, new_signal);
    } else {
        let bounded = match field {
            "daily_indicators.support.break_risk" => (0.0..=1.0).contains(&value),
            "daily_indicators.rsi14" => (0.0..=100.0).contains(&value),
            _ if discrete(field) => value >= 0.0,
            _ => true,
        };
        if !bounded {
            return None;
        }
        set_number(&mut next, field, Some(value));
        if field == PRICE_LEVEL && support_distance_linked(evidence) {
            let close = number_at(evidence, "daily_indicators.close")?;
            if value >= close {
                return None;
            }
            let distance = "daily_indicators.support.downside_to_support_pct";
            set_number(&mut next, distance, Some((close - value) / close * 100.0));
            linked.push(distance.to_string());
        }
    }
    context_problems(&next).is_empty().then_some((next, linked))
}

/// The evidence without `field` or anything that determines it, and the
/// fields removed with it.
pub(crate) fn without(evidence: &JsonValue, field: &str) -> (JsonValue, Vec<String>) {
    let mut next = evidence.clone();
    let mut gone: Vec<String> = if MARKOV_LINKED.contains(&field) {
        MARKOV_LINKED
            .iter()
            .chain(&["markov.sideways_prob"])
            .map(|path| path.to_string())
            .collect()
    } else if field == PRICE_LEVEL && support_distance_linked(evidence) {
        vec![
            field.to_string(),
            "daily_indicators.support.downside_to_support_pct".to_string(),
        ]
    } else {
        vec![field.to_string()]
    };
    for path in &gone {
        set_number(&mut next, path, None);
    }
    if MARKOV_LINKED.contains(&field)
        && let Some(markov) = next.get_mut("markov").and_then(JsonValue::as_object_mut)
    {
        markov.remove("direction");
    }
    gone.retain(|path| path != field);
    (next, gone)
}

/// Every seeded case, from the committed labels and nothing else. Pure and
/// deterministic, and it never calls the checker.
pub(crate) fn generate_seeded_v2(cases: &[NoteCase], labels: &[FreshLabel]) -> Vec<SeededCase> {
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
        let anchors = anchors_v2(case, label);
        let mut emit = |anchors: &[Anchor], index: usize, change: Change, family: &str| {
            if let Some(mut built) = build(case, &note, anchors, index, change, &truth_v2) {
                built.family = family.to_string();
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
                linked: Vec::new(),
            };
            // The note changes, and the claim becomes false.
            for (mutation, up) in [("note_value_up", true), ("note_value_down", false)] {
                if let Some(figure) = moved(anchor.written.value, field, up)
                    .and_then(|value| write_like(value, anchor.written))
                {
                    emit(
                        &anchors,
                        index,
                        in_note(mutation, "true_to_false", figure),
                        "valid_context",
                    );
                }
            }
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
                        "valid_context",
                    );
                }
            }
            // The evidence changes -- coherently -- under an unchanged note.
            for (mutation, up) in [("evidence_moved_up", true), ("evidence_moved_down", false)] {
                if let Some((evidence, linked)) = moved(anchor.stored, field, up)
                    .and_then(|value| coherently(&case.evidence, field, value))
                {
                    let change = Change {
                        mutation,
                        direction: "true_to_false",
                        rewrite: None,
                        insert: None,
                        evidence,
                        evidence_field: Some(field),
                        linked,
                    };
                    emit(&anchors, index, change, "valid_context");
                }
                // The same move to this field alone, where that breaks the
                // model's links: corruption, reported apart.
                if let Some(stored) = moved(anchor.stored, field, up) {
                    let mut evidence = case.evidence.clone();
                    set_number(&mut evidence, field, Some(stored));
                    if !context_problems(&evidence).is_empty() {
                        let change = Change {
                            mutation: if up {
                                "single_field_up"
                            } else {
                                "single_field_down"
                            },
                            direction: "true_to_false",
                            rewrite: None,
                            insert: None,
                            evidence,
                            evidence_field: Some(field),
                            linked: Vec::new(),
                        };
                        emit(&anchors, index, change, "corruption");
                    }
                }
            }
            // Note and evidence move together, and the claim stays true.
            if let Some(stored) = moved(anchor.stored, field, true) {
                let scale = 10f64.powi(anchor.written.decimals as i32);
                if let (Some(figure), Some((evidence, linked))) = (
                    write_like((stored * scale).round() / scale, anchor.written),
                    coherently(&case.evidence, field, stored),
                ) {
                    let mut change = in_note("both_moved", "stays_true", figure);
                    change.evidence = evidence;
                    change.evidence_field = Some(field);
                    change.linked = linked;
                    emit(&anchors, index, change, "valid_context");
                }
            }
            // The field, and everything that determines it, is gone.
            let (evidence, linked) = without(&case.evidence, field);
            if !derivable_after_removal(&evidence, field) {
                let change = Change {
                    mutation: "removed_with_its_links",
                    direction: "unsettleable",
                    rewrite: None,
                    insert: None,
                    evidence,
                    evidence_field: Some(field),
                    linked,
                };
                emit(&anchors, index, change, "valid_context");
            }
            // Another symbol's value for the same field, copied in.
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
                    (truth_v2(&figure, field, &case.evidence) == Some("fails")).then_some(figure)
                });
                if let Some(figure) = copied {
                    emit(
                        &anchors,
                        index,
                        in_note("other_symbol_value", "true_to_false", figure),
                        "valid_context",
                    );
                }
            }
            // An out-of-scope stop placed beside a support level.
            if field == PRICE_LEVEL
                && let Some(stop) = write_like(anchor.stored * 0.97, anchor.written)
            {
                let change = Change {
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
                    linked: Vec::new(),
                };
                emit(&anchors, index, change, "valid_context");
            }
        }
        // A claim the labeller read as wrong, and the arithmetic finds wrong,
        // repaired to the stored value.
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
            if rounds(written, stored, discrete(field)) != Some(false) {
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
            let change = Change {
                mutation: "repaired",
                direction: "false_to_true",
                rewrite: Some((target, figure)),
                insert: None,
                evidence: case.evidence.clone(),
                evidence_field: None,
                linked: Vec::new(),
            };
            emit(&with_repair, anchors.len() + index, change, "valid_context");
        }
    }
    seeded
}

/// Scores the seeded cases with the checker as it now stands, which must be
/// the frozen method. Each family is reported apart.
pub(crate) fn score_seeded_v2(seeded: &[SeededCase]) -> JsonValue {
    if crate::jev_numeric::NUMERIC_METHOD_VERSION != FROZEN_METHOD_V2 {
        return json!({
            "status": "method_moved",
            "reading": format!(
                "The method is {}, not the frozen {FROZEN_METHOD_V2}. Score at {FROZEN_COMMIT_V2}.",
                crate::jev_numeric::NUMERIC_METHOD_VERSION
            ),
        });
    }
    let readings = seeded_readings(seeded);
    let mut families = serde_json::Map::new();
    for family in ["valid_context", "corruption"] {
        let mut by_mutation = serde_json::Map::new();
        let mutations: BTreeSet<&str> = seeded
            .iter()
            .filter(|case| case.family == family)
            .map(|case| case.mutation.as_str())
            .collect();
        for mutation in mutations {
            by_mutation.insert(
                mutation.to_string(),
                seeded_totals(seeded, &readings, |case, _| {
                    case.family == family && case.mutation == mutation
                }),
            );
        }
        families.insert(
            family.to_string(),
            json!({
                "cases": seeded.iter().filter(|case| case.family == family).count(),
                "totals": seeded_totals(seeded, &readings, |case, _| case.family == family),
                "by_mutation": by_mutation,
            }),
        );
    }
    json!({
        "status": "complete",
        "method_version": FROZEN_METHOD_V2,
        "cases": seeded.len(),
        "families": families,
        "reading": "Each keyed figure's outcome under its truth, per family. `valid_context` \
                    changes keep the evidence one the Markov model could produce; `corruption` \
                    changes one linked field alone, and asks only whether the checker reads the \
                    field it names. `changed` is the figure the change made or unsettled; \
                    `untouched` are the other anchored figures in the note.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markov_evidence() -> JsonValue {
        json!({
            "daily_indicators": {
                "close": 100.0,
                "confluence_count": 5,
                "rsi14": 61.34,
                "support": {"nearest_support": 90.0, "downside_to_support_pct": 10.0, "break_risk": 0.8},
            },
            "markov": {"signed_signal": -0.124, "conviction": 0.124, "bull_prob": 0.3,
                       "bear_prob": 0.424, "sideways_prob": 0.276, "direction": "short",
                       "horizon_days": 5},
        })
    }

    fn written(figure: &str) -> Written {
        parse_written(figure).expect("parses")
    }

    #[test]
    fn the_key_holds_figures_to_rounding_alone() {
        assert_eq!(rounds(written("503"), 502.89, false), Some(true));
        assert_eq!(
            rounds(written("502"), 502.89, false),
            Some(false),
            "truncated"
        );
        assert_eq!(
            rounds(written("23.14"), 23.135, false),
            Some(true),
            "a decimal half"
        );
        assert_eq!(
            rounds(written("23.13"), 23.135, false),
            Some(true),
            "either way"
        );
        assert_eq!(
            rounds(written("0.1234567890"), 0.123456789071, false),
            Some(false)
        );
        assert_eq!(
            rounds(written("0.12345678907000104"), 0.123456789070001, false),
            None
        );
        assert_eq!(rounds(written("5"), 5.0, true), Some(true));
        assert_eq!(rounds(written("5.0"), 5.0, true), Some(false));
    }

    #[test]
    fn the_frame_is_after_the_freeze_and_before_the_close() {
        assert!(!in_frame("2026-09-25T18:17:19Z"), "#363, before the freeze");
        assert!(!in_frame("2026-09-26T09:39:03Z"), "the freeze itself");
        assert!(in_frame("2026-09-28T08:49:05Z"));
        assert!(in_frame("2026-10-09T18:16:00Z"));
        assert!(!in_frame("2026-10-10T08:49:05Z"));
        assert!(
            !in_frame("2026-09-28 08:49:05"),
            "an unknown format is not guessed at"
        );
    }

    /// Moving one Markov field moves the others the model computes with it,
    /// and the result passes every check `context_problems` makes.
    #[test]
    fn a_valid_context_change_keeps_the_models_links() {
        let evidence = markov_evidence();
        assert!(context_problems(&evidence).is_empty());
        let (moved, linked) =
            coherently(&evidence, "markov.signed_signal", -0.17).expect("in bounds");
        assert!(context_problems(&moved).is_empty());
        assert_eq!(number_at(&moved, "markov.conviction"), Some(0.17));
        let bull = number_at(&moved, "markov.bull_prob").unwrap();
        let bear = number_at(&moved, "markov.bear_prob").unwrap();
        assert!((bull - bear + 0.17).abs() < 1e-12);
        assert_eq!(number_at(&moved, "markov.sideways_prob"), Some(0.276));
        assert_eq!(
            linked,
            vec!["markov.conviction", "markov.bull_prob", "markov.bear_prob"]
        );
        // Conviction moves the signal with its own sign.
        let (by_conviction, _) =
            coherently(&evidence, "markov.conviction", 0.17).expect("in bounds");
        assert_eq!(
            number_at(&by_conviction, "markov.signed_signal"),
            Some(-0.17)
        );
        // Out of bounds is not generated at all.
        assert!(coherently(&evidence, "markov.signed_signal", -0.9).is_none());
        assert!(coherently(&evidence, "daily_indicators.support.break_risk", 0.8 * UP).is_none());
        // The support's distance follows the support, where it is derived.
        let (support, linked) = coherently(&evidence, PRICE_LEVEL, 80.0).expect("below the close");
        assert_eq!(
            number_at(&support, "daily_indicators.support.downside_to_support_pct"),
            Some(20.0)
        );
        assert_eq!(
            linked,
            vec!["daily_indicators.support.downside_to_support_pct"]
        );
        assert!(
            coherently(&evidence, PRICE_LEVEL, 101.0).is_none(),
            "above the close"
        );
    }

    /// A removal takes everything that could settle the field.
    #[test]
    fn a_removal_leaves_nothing_that_settles_the_field() {
        let evidence = markov_evidence();
        for field in MARKOV_LINKED
            .iter()
            .chain(&[PRICE_LEVEL, "daily_indicators.rsi14"])
        {
            let (gone, _) = without(&evidence, field);
            assert!(number_at(&gone, field).is_none(), "{field}");
            assert!(!derivable_after_removal(&gone, field), "{field}");
        }
    }

    fn seeded_note() -> (Vec<NoteCase>, Vec<FreshLabel>) {
        let mut other = markov_evidence();
        other["markov"]["signed_signal"] = json!(0.231);
        other["markov"]["conviction"] = json!(0.231);
        other["markov"]["bull_prob"] = json!(0.4775);
        other["markov"]["bear_prob"] = json!(0.2465);
        other["markov"]["direction"] = json!("long");
        let note = |id: &str, symbol: &str, text: &str, evidence: JsonValue| NoteCase {
            id: id.to_string(),
            source_report: 400,
            symbol: symbol.to_string(),
            note: text.to_string(),
            evidence,
        };
        let claim = |quote: &str, figure: &str, field: &str, verdict: &str| FreshClaim {
            quote: quote.to_string(),
            figure: figure.to_string(),
            field: field.to_string(),
            verdict: verdict.to_string(),
            note: String::new(),
        };
        let cases = vec![
            note(
                "a",
                "AAA",
                "Five confluences, support 90.0 EUR, negative Markov conviction (-0.124), rsi 61.3.",
                markov_evidence(),
            ),
            note("b", "BBB", "Nothing to add.", other),
        ];
        let labels = vec![
            FreshLabel {
                id: "a".to_string(),
                status: "labelled".to_string(),
                claims: vec![
                    claim(
                        "Five confluences",
                        "Five",
                        "daily_indicators.confluence_count",
                        "consistent",
                    ),
                    claim("support 90.0", "90.0", PRICE_LEVEL, "consistent"),
                    claim(
                        "conviction (-0.124)",
                        "-0.124",
                        "markov.signed_signal",
                        "consistent",
                    ),
                    claim("rsi 61.3", "61.3", "daily_indicators.rsi14", "consistent"),
                ],
            },
            FreshLabel {
                id: "b".to_string(),
                status: "labelled".to_string(),
                claims: vec![],
            },
        ];
        (cases, labels)
    }

    /// Every valid-context case holds evidence the model could produce, every
    /// key is the key's own arithmetic, and corruption is its own family.
    #[test]
    fn valid_context_cases_are_coherent_and_corruption_is_apart() {
        let (cases, labels) = seeded_note();
        let seeded = generate_seeded_v2(&cases, &labels);
        let mutations: BTreeSet<(&str, &str)> = seeded
            .iter()
            .map(|case| (case.family.as_str(), case.mutation.as_str()))
            .collect();
        for expected in [
            ("valid_context", "note_value_up"),
            ("valid_context", "sign_flipped"),
            ("valid_context", "evidence_moved_up"),
            ("valid_context", "evidence_moved_down"),
            ("valid_context", "both_moved"),
            ("valid_context", "removed_with_its_links"),
            ("valid_context", "other_symbol_value"),
            ("valid_context", "out_of_scope_stop_inserted"),
            ("corruption", "single_field_up"),
            ("corruption", "single_field_down"),
        ] {
            assert!(mutations.contains(&expected), "{expected:?}: {mutations:?}");
        }
        for case in &seeded {
            if case.family == "valid_context" {
                assert!(context_problems(&case.evidence).is_empty(), "{}", case.id);
            } else {
                assert_eq!(case.family, "corruption", "{}", case.id);
                assert!(!context_problems(&case.evidence).is_empty(), "{}", case.id);
            }
            for key in &case.keys {
                assert_eq!(case.note[key.start..key.end], key.figure, "{}", case.id);
                if let Some(field) = key.field.as_deref() {
                    assert_eq!(
                        truth_v2(&key.figure, field, &case.evidence),
                        Some(key.truth.as_str()),
                        "{}",
                        case.id
                    );
                }
            }
        }
        // Corruption only where one field alone breaks the model's links: the
        // RSI has none, so its single-field change is not corruption.
        assert!(!seeded.iter().any(|case| {
            case.family == "corruption"
                && case.keys.iter().any(|key| {
                    key.changed && key.field.as_deref() == Some("daily_indicators.rsi14")
                })
        }));
    }

    #[test]
    fn generation_is_deterministic_and_never_calls_the_checker() {
        let (cases, labels) = seeded_note();
        let once = serde_json::to_string(&generate_seeded_v2(&cases, &labels)).unwrap();
        let mut shuffled = cases.clone();
        shuffled.reverse();
        assert_eq!(
            once,
            serde_json::to_string(&generate_seeded_v2(&shuffled, &labels)).unwrap()
        );
        let source = include_str!("v2.rs");
        let generator = &source[source.find("pub(crate) fn rounds(").unwrap()
            ..source.find("pub(crate) fn score_seeded_v2").unwrap()];
        assert!(
            !generator.contains("jev_numeric"),
            "the key must not depend on the checker"
        );
    }
}

/// The instrument, the key, the labels template, the seeded cases and the
/// single score, once the frame has closed. Builders are `#[ignore]`d.
///
/// ```text
/// JEV_FRESH_PATH=fresh-v2-frame.json cargo test build_the_fresh_v2_instrument -- --ignored
/// cargo test generate_the_v2_seeded_cases -- --ignored
/// cargo test score_the_fresh_v2_set -- --ignored --nocapture
/// ```
#[cfg(test)]
mod frozen {
    use super::*;

    const INSTRUMENT_PATH: &str = "docs/jev-fresh-v2.json";
    const KEY_PATH: &str = "docs/jev-fresh-v2-key.json";
    const LABELS_PATH: &str = "docs/jev-fresh-v2-labels.json";
    const SEEDED_PATH: &str = "docs/jev-fresh-v2-seeded.json";
    const RESULTS_PATH: &str = "docs/jev-fresh-v2-results.json";

    fn frozen() -> Option<(Vec<NoteCase>, Vec<NoteKey>, JsonValue)> {
        let instrument = document(INSTRUMENT_PATH)?;
        let key = document(KEY_PATH)?;
        Some((
            serde_json::from_value(instrument["cases"].clone()).ok()?,
            serde_json::from_value(key["keys"].clone()).ok()?,
            key,
        ))
    }

    fn labels() -> Option<(Vec<FreshLabel>, JsonValue)> {
        let document = document(LABELS_PATH)?;
        Some((
            serde_json::from_value(document["labels"].clone()).ok()?,
            document,
        ))
    }

    fn generated_as_stored(cases: &[NoteCase], labels: &[FreshLabel]) -> JsonValue {
        let text = serde_json::to_string(&generate_seeded_v2(cases, labels)).expect("serialize");
        serde_json::from_str(&text).expect("parse")
    }

    #[test]
    fn the_instrument_is_blind_and_carries_the_conventions() {
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
        for (name, _) in CONVENTIONS {
            assert!(instrument["conventions"].get(*name).is_some(), "{name}");
        }
    }

    #[test]
    fn the_frame_is_prospective_and_whole() {
        let Some((cases, keys, key)) = frozen() else {
            return;
        };
        assert_eq!(key["method_version"], FROZEN_METHOD_V2);
        assert_eq!(key["protocol"], PROTOCOL_V2);
        for report in key["frame"]["reports"].as_array().expect("reports") {
            assert!(
                in_frame(report["created_at"].as_str().unwrap_or_default()),
                "{report}"
            );
        }
        assert_eq!(cases.len(), keys.len());
        let notes: BTreeMap<&str, &NoteCase> =
            cases.iter().map(|case| (case.id.as_str(), case)).collect();
        for entry in &keys {
            let lowered = notes[entry.id.as_str()].note.to_lowercase();
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
            "labels arrived with nobody named"
        );
        assert_ne!(score_natural(&cases, &keys, &labels)["status"], "rejected");
    }

    #[test]
    fn the_seeded_cases_are_what_the_labels_generate() {
        let (Some((cases, _, _)), Some((labels, _)), Some(seeded)) =
            (frozen(), labels(), document(SEEDED_PATH))
        else {
            return;
        };
        assert_eq!(seeded["cases"], generated_as_stored(&cases, &labels));
    }

    #[test]
    #[ignore]
    fn build_the_fresh_v2_instrument() {
        assert_eq!(crate::jev_numeric::NUMERIC_METHOD_VERSION, FROZEN_METHOD_V2);
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(
            now.as_str() >= FRAME_CLOSES,
            "the frame closes at {FRAME_CLOSES}; it is {now}"
        );
        assert!(
            !std::path::Path::new(INSTRUMENT_PATH).exists(),
            "built once"
        );
        let path = std::env::var("JEV_FRESH_PATH").expect("JEV_FRESH_PATH");
        let raw = std::fs::read(&path).expect("the frame dump");
        let sources: Vec<JsonValue> = serde_json::from_slice(&raw).expect("a JSON array");
        let (reports, notes) = frame_notes(&sources, SEED_V2, |_, created_at| in_frame(created_at));
        let mut population: BTreeMap<String, usize> = BTreeMap::new();
        for (_, key) in notes.values() {
            *population.entry(key.stratum.clone()).or_default() += 1;
        }
        let (cases, keys): (Vec<NoteCase>, Vec<NoteKey>) = notes.into_values().unzip();
        let conventions: serde_json::Map<String, JsonValue> = CONVENTIONS
            .iter()
            .map(|(name, text)| (name.to_string(), json!(text)))
            .collect();
        std::fs::write(
            INSTRUMENT_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_V2_VERSION,
                "what_this_is": "Every note from the decision reports created in the frame, each \
                                 with the evidence the report was given about its symbol. What any \
                                 checker made of them is not here.",
                "how_to_label": {
                    "task": "Read each whole note and list every numeric claim it makes about the \
                             supplied evidence: every figure, in digits or in words, that states \
                             or compares a value the evidence holds. Judge each against the \
                             evidence beside it, under the conventions below. Qualitative claims \
                             are not asked for.",
                    "quote": "The exact words of the claim from the note, as short as still makes \
                              the claim. It must occur exactly once in the note.",
                    "figure": "The number exactly as the note writes it, occurring once in the \
                               quote as a whole number. One claim per figure.",
                    "field": "The dotted path of the evidence field the figure states, exactly as \
                              it appears in that note's evidence; it must hold a number. \"none\" \
                              when no single field states it.",
                    "verdict": "\"consistent\", \"inconsistent\" or \"cannot_tell\", under the \
                                conventions.",
                    "note": "Optional: why, for anything uncertain.",
                    "status": "Set \"labelled\" once you have read the whole note, including when \
                               it makes no numeric claim.",
                },
                "conventions": conventions,
                "out_of_scope": "Portfolio capital, holdings, allocations, unrealised profit, \
                                 trading costs, orders, stops and executions, and recommendations \
                                 or intentions. List no claim for them.",
                "cases": cases,
            }))
            .expect("serialize"),
        )
        .expect("write instrument");
        std::fs::write(
            KEY_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_V2_VERSION,
                "method_version": FROZEN_METHOD_V2,
                "frozen_commit": FROZEN_COMMIT_V2,
                "protocol": PROTOCOL_V2,
                "warning": "Do not read this before labelling.",
                "frame": {
                    "opens": FRAME_OPENS,
                    "closes": FRAME_CLOSES,
                    "dump_sha256": format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&raw)),
                    "reports": reports,
                },
                "population": population,
                "keys": keys,
            }))
            .expect("serialize"),
        )
        .expect("write key");
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
                "version": FRESH_V2_VERSION,
                "labelled_by": "",
                "labelled_at": "",
                "labels": template,
            }))
            .expect("serialize"),
        )
        .expect("write labels");
    }

    #[test]
    #[ignore]
    fn generate_the_v2_seeded_cases() {
        let (Some((cases, keys, _)), Some((labels, _))) = (frozen(), labels()) else {
            panic!("the instrument and the labels are needed");
        };
        assert_eq!(score_natural(&cases, &keys, &labels)["status"], "complete");
        let seeded = generate_seeded_v2(&cases, &labels);
        std::fs::write(
            SEEDED_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_V2_VERSION,
                "protocol": PROTOCOL_V2,
                "generated_from": {
                    "instrument_sha256": sha256_of(INSTRUMENT_PATH),
                    "labels_sha256": sha256_of(LABELS_PATH),
                },
                "cases": seeded,
            }))
            .expect("serialize"),
        )
        .expect("write seeded");
    }

    #[test]
    #[ignore]
    fn score_the_fresh_v2_set() {
        assert!(!std::path::Path::new(RESULTS_PATH).exists(), "scored once");
        let (Some((cases, keys, _)), Some((labels, labels_document)), Some(seeded)) =
            (frozen(), labels(), document(SEEDED_PATH))
        else {
            panic!("the instrument, the labels and the seeded cases are needed");
        };
        assert_eq!(seeded["cases"], generated_as_stored(&cases, &labels));
        let seeded_cases: Vec<SeededCase> =
            serde_json::from_value(seeded["cases"].clone()).expect("seeded cases");
        let mut natural = score_natural(&cases, &keys, &labels);
        natural["version"] = json!(FRESH_V2_VERSION);
        natural["protocol"] = json!(PROTOCOL_V2);
        let seeded_score = score_seeded_v2(&seeded_cases);
        assert_eq!(natural["status"], "complete", "{natural:#}");
        assert_eq!(seeded_score["status"], "complete", "{seeded_score:#}");
        let result = json!({
            "version": FRESH_V2_VERSION,
            "protocol": PROTOCOL_V2,
            "method_version": FROZEN_METHOD_V2,
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
