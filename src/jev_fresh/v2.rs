//! fresh-v2: the second held-out evaluation, fixed before its frame exists.
//!
//! fresh-v1 measured `n15`; review of its results and of the fixes since asked
//! for these changes before the next one, and this version makes them:
//!
//! - **The conventions are stated, not chosen by the labeller.** A figure is
//!   consistent if it is the stored value rounded at the precision written,
//!   either way at a half; a truncation rounding does not produce is not. A
//!   signed figure beside "conviction" states the signed signal. Both are in
//!   the rubric, and the key's own arithmetic follows the first.
//! - **Valid-context changes are kept apart from evidence corruption.** The
//!   evidence is changed as a valid context only for the two groups of fields
//!   whose every dependent this generator reproduces exactly: the Markov
//!   signal, conviction, probabilities and direction, and the Quiver signal,
//!   direction and confidence. Every other evidence change -- a field moved
//!   alone, or a field whose dependents the runtime computes from inputs this
//!   generator cannot reproduce -- is `corruption`, reported apart.
//! - **Every valid-context case passes `evidence_problems`**, whatever made
//!   it, and so must the note's original evidence. Nothing dropped is hidden:
//!   every case not generated is counted with its reason.
//! - **A removal takes the field and everything that determines it.**
//! - **The frame is checked against a recorded manifest** of every report in
//!   the window, so a partial export cannot become the frozen evaluation.
//! - **The checker is pinned by its source hash**, not only its version.
//!
//! The method under evaluation is `n20`, frozen at `07cf157`. The frame is
//! prospective: every completed report created after the freeze, up to the
//! close. None exists yet, so nothing in it can have shaped the method.

use super::*;

pub(crate) const FRESH_V2_VERSION: &str = "fresh-v2-2026-09-26";
/// Revision 2, from review of the first, before any report was in the frame.
pub(crate) const PROTOCOL_V2: &str = "fresh-protocol-v2-2026-09-26-r2";
/// The method under evaluation. Nothing is scored under any other.
pub(crate) const FROZEN_METHOD_V2: &str = "n20-2026-09-26";
pub(crate) const FROZEN_COMMIT_V2: &str = "07cf157";
/// SHA-256 of `src/jev_numeric.rs` at `07cf157`. The version string alone
/// cannot catch an implementation change made without a version bump.
pub(crate) const CHECKER_SOURCE_SHA256: &str =
    "7816fd23fee51c7e7ddd103a3ed28f1ee92da25f462ab747191d484eac27893b";
/// Names each note; fixed, recorded, used for nothing else.
pub(crate) const SEED_V2: &str = "jev-fresh-v2";
/// The frame opens when `n20` was frozen: `07cf157`'s commit time.
pub(crate) const FRAME_OPENS: &str = "2026-09-26T09:39:03Z";
/// And closes at the end of the second full trading week after it. Fixed on
/// 2026-09-26. An amendment must be dated and justified without reference to
/// anything the frame contains.
pub(crate) const FRAME_CLOSES: &str = "2026-10-10T00:00:00Z";

/// Every report in the window, whatever its status, for the manifest.
pub(crate) const MANIFEST_QUERY: &str = "select id, created_at, status, report_json is not null \
     as has_report, request_json is not null as has_request from decision_reports where \
     created_at > '2026-09-26T09:39:03Z' and created_at < '2026-10-10T00:00:00Z' order by id";
/// The eligible reports themselves, for the dump.
pub(crate) const DUMP_QUERY: &str = "select jsonb_agg(jsonb_build_object('id', id, 'created_at', \
     created_at, 'status', status, 'report', report_json::jsonb, 'request', \
     request_json::jsonb) order by id) from decision_reports where created_at > \
     '2026-09-26T09:39:03Z' and created_at < '2026-10-10T00:00:00Z' and status = 'completed' \
     and report_json is not null and request_json is not null";

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

/// SHA-256 of the checker's source as compiled into this build.
pub(crate) fn checker_source_sha256() -> String {
    format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(include_bytes!("../jev_numeric.rs"))
    )
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

fn text_at<'a>(evidence: &'a JsonValue, path: &str) -> Option<&'a str> {
    path.split('.')
        .try_fold(evidence, |cursor, segment| cursor.get(segment))
        .and_then(JsonValue::as_str)
}

/// Everything wrong with an evidence object: the Markov model's links and
/// bounds (`context_problems`), and the runtime's other derived fields and
/// bounds. Every relation here holds in the evidence of every stored note --
/// 1,002 with Markov rows, 489 with indicators, 372 with Quiver -- so a case
/// that breaks one was made by the generator, not found.
pub(crate) fn evidence_problems(evidence: &JsonValue) -> Vec<&'static str> {
    let mut problems = context_problems(evidence);
    let number = |path: &str| number_at(evidence, path);
    // Quiver: `quiver.rs` bounds the signal with tanh and a clamp, names the
    // direction by a threshold of 0.15, and adds 0.35 of the signal's
    // magnitude and 0.10 to at most 0.55 from the event count.
    if let Some(signal) = number("quiver.signal") {
        if !(-1.0..=1.0).contains(&signal) {
            problems.push("quiver_signal_outside_minus_1_to_1");
        }
        if let Some(direction) = text_at(evidence, "quiver.direction") {
            let named = if signal > 0.15 {
                "bullish"
            } else if signal < -0.15 {
                "bearish"
            } else {
                "neutral"
            };
            if direction != named {
                problems.push("quiver_direction_is_not_the_signals_band");
            }
        }
        if let Some(confidence) = number("quiver.confidence").filter(|value| *value > 0.0) {
            let from_events = confidence - 0.35 * signal.abs() - 0.10;
            if !(-1e-6..=0.55 + 1e-6).contains(&from_events) {
                problems.push("quiver_confidence_is_not_the_signals");
            }
        }
    }
    // The support's label is its break risk's band, and its distance is the
    // close's distance above it (`daily_indicators.rs`).
    if let (Some(risk), Some(label)) = (
        number("daily_indicators.support.break_risk"),
        text_at(evidence, "daily_indicators.support.break_risk_label"),
    ) {
        let named = if risk >= 0.65 {
            "high"
        } else if risk >= 0.35 {
            "moderate"
        } else {
            "low"
        };
        if label != named {
            problems.push("break_risk_label_is_not_its_band");
        }
    }
    if let (Some(close), Some(support), Some(distance)) = (
        number("daily_indicators.close"),
        number(PRICE_LEVEL),
        number("daily_indicators.support.downside_to_support_pct"),
    ) && close > 0.0
        && (distance - ((close - support) / close * 100.0).max(0.0)).abs() > 1e-6
    {
        problems.push("support_distance_is_not_the_closes");
    }
    for path in DISCRETE_FIELDS {
        if number(path).is_some_and(|value| value < 0.0 || value.fract() != 0.0) {
            problems.push("count_not_a_whole_number");
        }
    }
    problems
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
/// The Quiver fields the runtime computes from the signal.
const QUIVER_LINKED: &[&str] = &["quiver.signal", "quiver.confidence"];

fn set_text(evidence: &mut JsonValue, block: &str, key: &str, value: &str) {
    if let Some(object) = evidence.get_mut(block).and_then(JsonValue::as_object_mut) {
        object.insert(key.to_string(), json!(value));
    }
}

/// The evidence with `field` set to `value` and every field the runtime
/// derives with it moved too, and the linked fields moved. Only the Markov and
/// Quiver groups are reproduced exactly; any other field is
/// `dependents_not_reproduced`. A move outside a field's bounds is
/// `outside_bounds`.
pub(crate) fn coherently(
    evidence: &JsonValue,
    field: &str,
    value: f64,
) -> Result<(JsonValue, Vec<String>), &'static str> {
    let mut next = evidence.clone();
    let mut linked = Vec::new();
    let inside = |v: f64, low: f64, high: f64| (low..=high).contains(&v);
    if MARKOV_LINKED.contains(&field) {
        let markov = |name: &str| number_at(evidence, &format!("markov.{name}"));
        let (Some(sideways), Some(signal)) = (markov("sideways_prob"), markov("signed_signal"))
        else {
            return Err("dependents_not_reproduced");
        };
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
        if !inside(bull, 0.0, 1.0) || !inside(bear, 0.0, 1.0) || !inside(new_signal, -1.0, 1.0) {
            return Err("outside_bounds");
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
        let direction = if new_signal > 1e-9 {
            "long"
        } else if new_signal < -1e-9 {
            "short"
        } else {
            "flat"
        };
        set_text(&mut next, "markov", "direction", direction);
    } else if field == "quiver.signal" {
        let (Some(signal), Some(confidence)) = (
            number_at(evidence, "quiver.signal"),
            number_at(evidence, "quiver.confidence").filter(|value| *value > 0.0),
        ) else {
            return Err("dependents_not_reproduced");
        };
        if !inside(value, -1.0, 1.0) {
            return Err("outside_bounds");
        }
        let new_confidence = confidence + 0.35 * (value.abs() - signal.abs());
        if !inside(new_confidence, 0.0, 1.0) {
            return Err("outside_bounds");
        }
        set_number(&mut next, "quiver.signal", Some(value));
        set_number(&mut next, "quiver.confidence", Some(new_confidence));
        linked.push("quiver.confidence".to_string());
        let direction = if value > 0.15 {
            "bullish"
        } else if value < -0.15 {
            "bearish"
        } else {
            "neutral"
        };
        set_text(&mut next, "quiver", "direction", direction);
    } else {
        return Err("dependents_not_reproduced");
    }
    Ok((next, linked))
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
    } else if QUIVER_LINKED.contains(&field) {
        QUIVER_LINKED.iter().map(|path| path.to_string()).collect()
    } else if field == PRICE_LEVEL {
        // The close and the distance above the support determine it.
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
    let block = if MARKOV_LINKED.contains(&field) {
        Some("markov")
    } else if QUIVER_LINKED.contains(&field) {
        Some("quiver")
    } else {
        None
    };
    if let Some(block) = block
        && let Some(object) = next.get_mut(block).and_then(JsonValue::as_object_mut)
    {
        object.remove("direction");
    }
    gone.retain(|path| path != field);
    (next, gone)
}

/// The seeded cases, and every case attempted but not generated, counted by
/// change and reason.
#[derive(Debug, Default)]
pub(crate) struct GeneratedV2 {
    pub cases: Vec<SeededCase>,
    pub not_generated: BTreeMap<String, BTreeMap<&'static str, usize>>,
}

impl GeneratedV2 {
    fn reject(&mut self, mutation: &str, reason: &'static str) {
        *self
            .not_generated
            .entry(mutation.to_string())
            .or_default()
            .entry(reason)
            .or_default() += 1;
    }
}

/// Every seeded case, from the committed labels and nothing else. Pure and
/// deterministic, and it never calls the checker.
pub(crate) fn generate_seeded_v2(cases: &[NoteCase], labels: &[FreshLabel]) -> GeneratedV2 {
    let labels_by_id: BTreeMap<&str, &FreshLabel> = labels
        .iter()
        .map(|label| (label.id.as_str(), label))
        .collect();
    let mut ordered: Vec<&NoteCase> = cases.iter().collect();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));
    let mut out = GeneratedV2::default();
    for case in ordered {
        let Some(label) = labels_by_id
            .get(case.id.as_str())
            .filter(|label| label.status.trim() == "labelled")
        else {
            continue;
        };
        let note = case.note.to_lowercase();
        let anchors = anchors_v2(case, label);
        let original_coherent = evidence_problems(&case.evidence).is_empty();
        let mut attempts: Vec<(Vec<Anchor>, usize, Change, &'static str)> = Vec::new();
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
            macro_rules! attempt {
                ($change:expr, $family:expr $(,)?) => {
                    attempts.push((anchors.clone(), index, $change, $family))
                };
            }
            // The note changes, and the claim becomes false.
            for (mutation, up) in [("note_value_up", true), ("note_value_down", false)] {
                if let Some(figure) = moved(anchor.written.value, field, up)
                    .and_then(|value| write_like(value, anchor.written))
                {
                    attempt!(in_note(mutation, "true_to_false", figure), "valid_context");
                }
            }
            if SIGNED_FIELDS.contains(&field) && anchor.written.value != 0.0 {
                let signed = Written {
                    explicit_sign: true,
                    ..anchor.written
                };
                if let Some(figure) = write_like(-anchor.written.value, signed) {
                    attempt!(
                        in_note("sign_flipped", "true_to_false", figure),
                        "valid_context",
                    );
                }
            }
            // The evidence changes under an unchanged note: as a valid context
            // where the generator can move every dependent, and alone as
            // corruption.
            for (up, valid, alone) in [
                (true, "evidence_moved_up", "single_field_up"),
                (false, "evidence_moved_down", "single_field_down"),
            ] {
                let Some(stored) = moved(anchor.stored, field, up) else {
                    continue;
                };
                match coherently(&case.evidence, field, stored) {
                    Ok((evidence, linked)) => attempt!(
                        Change {
                            mutation: valid,
                            direction: "true_to_false",
                            rewrite: None,
                            insert: None,
                            evidence,
                            evidence_field: Some(field),
                            linked,
                        },
                        "valid_context",
                    ),
                    Err(reason) => out.reject(valid, reason),
                }
                let mut evidence = case.evidence.clone();
                set_number(&mut evidence, field, Some(stored));
                attempt!(
                    Change {
                        mutation: alone,
                        direction: "true_to_false",
                        rewrite: None,
                        insert: None,
                        evidence,
                        evidence_field: Some(field),
                        linked: Vec::new(),
                    },
                    "corruption",
                );
            }
            // Note and evidence move together, and the claim stays true.
            if let Some(stored) = moved(anchor.stored, field, true) {
                let scale = 10f64.powi(anchor.written.decimals as i32);
                if let Some(figure) = write_like((stored * scale).round() / scale, anchor.written) {
                    match coherently(&case.evidence, field, stored) {
                        Ok((evidence, linked)) => {
                            let mut change = in_note("both_moved", "stays_true", figure.clone());
                            change.evidence = evidence;
                            change.evidence_field = Some(field);
                            change.linked = linked;
                            attempt!(change, "valid_context");
                        }
                        Err(reason) => out.reject("both_moved", reason),
                    }
                    let mut change = in_note("single_field_both_moved", "stays_true", figure);
                    set_number(&mut change.evidence, field, Some(stored));
                    change.evidence_field = Some(field);
                    attempt!(change, "corruption");
                }
            }
            // The field, and everything that determines it, is gone.
            let (evidence, linked) = without(&case.evidence, field);
            attempt!(
                Change {
                    mutation: "removed_with_its_links",
                    direction: "unsettleable",
                    rewrite: None,
                    insert: None,
                    evidence,
                    evidence_field: Some(field),
                    linked,
                },
                "valid_context",
            );
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
                    attempt!(
                        in_note("other_symbol_value", "true_to_false", figure),
                        "valid_context",
                    );
                }
            }
            // An out-of-scope stop placed beside a support level.
            if field == PRICE_LEVEL
                && let Some(stop) = write_like(anchor.stored * 0.97, anchor.written)
            {
                attempt!(
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
                        linked: Vec::new(),
                    },
                    "valid_context",
                );
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
            attempts.push((
                with_repair,
                anchors.len() + index,
                Change {
                    mutation: "repaired",
                    direction: "false_to_true",
                    rewrite: Some((target, figure)),
                    insert: None,
                    evidence: case.evidence.clone(),
                    evidence_field: None,
                    linked: Vec::new(),
                },
                "valid_context",
            ));
        }
        // Every attempt is built and checked the same way, whatever made it.
        for (anchors, index, change, family) in attempts {
            let mutation = change.mutation;
            if !original_coherent {
                out.reject(mutation, "original_evidence_incoherent");
                continue;
            }
            if family == "valid_context" && !evidence_problems(&change.evidence).is_empty() {
                out.reject(mutation, "evidence_problem");
                continue;
            }
            match build(case, &note, &anchors, index, change, &truth_v2) {
                Some(mut built) => {
                    built.family = family.to_string();
                    out.cases.push(built);
                }
                None => out.reject(mutation, "key_not_borne_out"),
            }
        }
    }
    out
}

/// Everything wrong with a frame's manifest and dump, and the eligible report
/// ids when nothing is. The manifest is the recorded result of
/// `MANIFEST_QUERY` over every report in the window, whatever its status; the
/// dump must hold exactly its eligible reports, and a frame with none is not a
/// frame.
pub(crate) fn validate_frame(
    manifest: &JsonValue,
    dump: &[JsonValue],
) -> Result<Vec<i64>, Vec<String>> {
    let mut problems = Vec::new();
    if manifest["manifest_query"] != MANIFEST_QUERY {
        problems.push("the manifest was not taken with MANIFEST_QUERY".to_string());
    }
    if manifest["dump_query"] != DUMP_QUERY {
        problems.push("the dump was not taken with DUMP_QUERY".to_string());
    }
    let taken_at = manifest["taken_at"].as_str().unwrap_or_default();
    if taken_at.len() != FRAME_CLOSES.len() || taken_at < FRAME_CLOSES {
        problems.push(format!(
            "the manifest was taken at {taken_at:?}, before the frame closed"
        ));
    }
    let rows = manifest["rows"].as_array().cloned().unwrap_or_default();
    let mut eligible = Vec::new();
    let mut seen = BTreeSet::new();
    for row in &rows {
        let id = row["id"].as_i64().unwrap_or(-1);
        if !seen.insert(id) {
            problems.push(format!("report {id} is listed twice"));
        }
        let created_at = row["created_at"].as_str().unwrap_or_default();
        if !in_frame(created_at) {
            problems.push(format!(
                "report {id} was created at {created_at:?}, outside the frame"
            ));
        }
        let completed =
            row["status"] == "completed" && row["has_report"] == true && row["has_request"] == true;
        if completed {
            eligible.push(id);
        } else if row["exclusion"].as_str().is_none_or(str::is_empty) {
            problems.push(format!("report {id} is excluded without a recorded reason"));
        }
    }
    if eligible.is_empty() {
        problems.push("no eligible report is in the frame".to_string());
    }
    let listed: Vec<i64> = manifest["eligible"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(JsonValue::as_i64)
        .collect();
    if listed != eligible {
        problems.push("the manifest's eligible list is not its completed rows".to_string());
    }
    let dumped: Vec<i64> = dump
        .iter()
        .filter_map(|entry| entry["id"].as_i64())
        .collect();
    if dumped != eligible {
        problems.push(format!(
            "the dump holds {} reports, not the manifest's {} eligible ones",
            dumped.len(),
            eligible.len()
        ));
    }
    let by_id: BTreeMap<i64, &JsonValue> = rows
        .iter()
        .filter_map(|row| Some((row["id"].as_i64()?, row)))
        .collect();
    for entry in dump {
        let id = entry["id"].as_i64().unwrap_or(-1);
        if entry["status"] != "completed" {
            problems.push(format!("report {id} in the dump is not completed"));
        }
        if by_id.get(&id).map(|row| &row["created_at"]) != Some(&entry["created_at"]) {
            problems.push(format!(
                "report {id}'s creation time differs from the manifest"
            ));
        }
    }
    if problems.is_empty() {
        Ok(eligible)
    } else {
        Err(problems)
    }
}

/// Scores the seeded cases with the checker as it now stands, which must be
/// the frozen method, byte for byte. Each family is reported apart.
pub(crate) fn score_seeded_v2(seeded: &[SeededCase]) -> JsonValue {
    if crate::jev_numeric::NUMERIC_METHOD_VERSION != FROZEN_METHOD_V2
        || checker_source_sha256() != CHECKER_SOURCE_SHA256
    {
        return json!({
            "status": "method_moved",
            "reading": format!(
                "The checker is {} with source {}, not the frozen {FROZEN_METHOD_V2} with source \
                 {CHECKER_SOURCE_SHA256}. Score at {FROZEN_COMMIT_V2}.",
                crate::jev_numeric::NUMERIC_METHOD_VERSION,
                checker_source_sha256()
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
        "checker_source_sha256": CHECKER_SOURCE_SHA256,
        "cases": seeded.len(),
        "families": families,
        "reading": "Each keyed figure's outcome under its truth, per family. `valid_context` \
                    changes keep the evidence one the runtime could produce, by the checks in \
                    `evidence_problems`; `corruption` moves one field alone, and asks only \
                    whether the checker reads the field it names. `changed` is the figure the \
                    change made or unsettled; `untouched` are the other anchored figures.",
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
                "support": {"nearest_support": 90.0, "downside_to_support_pct": 10.0,
                            "break_risk": 0.8, "break_risk_label": "high"},
            },
            "markov": {"signed_signal": -0.124, "conviction": 0.124, "bull_prob": 0.3,
                       "bear_prob": 0.424, "sideways_prob": 0.276, "direction": "short",
                       "horizon_days": 5},
            "quiver": {"signal": 0.5, "direction": "bullish", "confidence": 0.575},
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
        assert!(MANIFEST_QUERY.contains(FRAME_OPENS) && MANIFEST_QUERY.contains(FRAME_CLOSES));
        assert!(DUMP_QUERY.contains(FRAME_OPENS) && DUMP_QUERY.contains(FRAME_CLOSES));
    }

    /// Until this set is scored, the checker is the frozen source, byte for
    /// byte. A change here is `n21`, and must wait or amend the
    /// preregistration with a dated reason.
    #[test]
    fn the_checker_is_the_frozen_source_until_scored() {
        if std::path::Path::new("docs/jev-fresh-v2-results.json").exists() {
            return;
        }
        assert_eq!(
            checker_source_sha256(),
            CHECKER_SOURCE_SHA256,
            "src/jev_numeric.rs changed while n20 is frozen for fresh-v2 \
             (docs/jev-fresh-evaluation-v2.md)"
        );
        assert_eq!(crate::jev_numeric::NUMERIC_METHOD_VERSION, FROZEN_METHOD_V2);
    }

    /// Moving one Markov or Quiver field moves the others the runtime derives
    /// with it, and the result passes every check.
    #[test]
    fn a_valid_context_change_moves_every_dependent() {
        let evidence = markov_evidence();
        assert_eq!(evidence_problems(&evidence), Vec::<&str>::new());
        let (moved, linked) =
            coherently(&evidence, "markov.signed_signal", -0.17).expect("in bounds");
        assert_eq!(evidence_problems(&moved), Vec::<&str>::new());
        assert_eq!(number_at(&moved, "markov.conviction"), Some(0.17));
        assert_eq!(
            linked,
            vec!["markov.conviction", "markov.bull_prob", "markov.bear_prob"]
        );
        let (by_conviction, _) =
            coherently(&evidence, "markov.conviction", 0.17).expect("in bounds");
        assert_eq!(
            number_at(&by_conviction, "markov.signed_signal"),
            Some(-0.17)
        );
        // Quiver: the confidence and the direction follow the signal.
        let (quiver, linked) = coherently(&evidence, "quiver.signal", 0.1).expect("in bounds");
        assert_eq!(evidence_problems(&quiver), Vec::<&str>::new());
        assert_eq!(text_at(&quiver, "quiver.direction"), Some("neutral"));
        assert!((number_at(&quiver, "quiver.confidence").unwrap() - 0.435).abs() < 1e-12);
        assert_eq!(linked, vec!["quiver.confidence"]);
        // Review's case: 0.8 moved by 1.37 is 1.096, past Quiver's bound.
        let mut strong = evidence.clone();
        strong["quiver"] = json!({"signal": 0.8, "direction": "bullish", "confidence": 0.78});
        assert_eq!(evidence_problems(&strong), Vec::<&str>::new());
        assert_eq!(
            coherently(&strong, "quiver.signal", 0.8 * UP).err(),
            Some("outside_bounds")
        );
        assert_eq!(
            coherently(&evidence, "markov.signed_signal", -0.9).err(),
            Some("outside_bounds")
        );
        // Fields whose dependents the runtime computes from inputs this
        // generator cannot reproduce are never moved as a valid context.
        for field in [
            "daily_indicators.rsi14",
            "daily_indicators.confluence_count",
            PRICE_LEVEL,
            "daily_indicators.support.break_risk",
        ] {
            assert_eq!(
                coherently(&evidence, field, 1.0).err(),
                Some("dependents_not_reproduced"),
                "{field}"
            );
        }
    }

    #[test]
    fn the_evidence_check_names_each_broken_relation() {
        let mut broken = markov_evidence();
        broken["quiver"]["signal"] = json!(1.096);
        // 0.05 is less than 0.35 of the signal's magnitude plus 0.10.
        broken["quiver"]["confidence"] = json!(0.05);
        broken["daily_indicators"]["support"]["break_risk_label"] = json!("low");
        broken["daily_indicators"]["support"]["downside_to_support_pct"] = json!(12.0);
        broken["daily_indicators"]["confluence_count"] = json!(4.5);
        let problems = evidence_problems(&broken);
        for expected in [
            "quiver_signal_outside_minus_1_to_1",
            "quiver_confidence_is_not_the_signals",
            "break_risk_label_is_not_its_band",
            "support_distance_is_not_the_closes",
            "count_not_a_whole_number",
        ] {
            assert!(problems.contains(&expected), "{expected}: {problems:?}");
        }
    }

    /// A removal takes everything that could settle the field.
    #[test]
    fn a_removal_leaves_nothing_that_settles_the_field() {
        let evidence = markov_evidence();
        for field in
            MARKOV_LINKED
                .iter()
                .chain(&[PRICE_LEVEL, "daily_indicators.rsi14", "quiver.signal"])
        {
            let (gone, _) = without(&evidence, field);
            assert!(number_at(&gone, field).is_none(), "{field}");
            assert!(!derivable_after_removal(&gone, field), "{field}");
        }
    }

    fn note(id: &str, symbol: &str, text: &str, evidence: JsonValue) -> NoteCase {
        NoteCase {
            id: id.to_string(),
            source_report: 400,
            symbol: symbol.to_string(),
            note: text.to_string(),
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

    fn seeded_note() -> (Vec<NoteCase>, Vec<FreshLabel>) {
        let mut other = markov_evidence();
        other["markov"] = json!({"signed_signal": 0.231, "conviction": 0.231, "bull_prob": 0.4775,
                                 "bear_prob": 0.2465, "sideways_prob": 0.276, "direction": "long",
                                 "horizon_days": 5});
        let cases = vec![
            note(
                "a",
                "AAA",
                "Five confluences, support 90.0 EUR, negative Markov conviction (-0.124), rsi 61.3, quiver 0.80.",
                {
                    let mut evidence = markov_evidence();
                    evidence["quiver"] =
                        json!({"signal": 0.8, "direction": "bullish", "confidence": 0.78});
                    evidence
                },
            ),
            note("b", "BBB", "Nothing to add.", other),
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
                    claim("support 90.0", "90.0", PRICE_LEVEL, "consistent"),
                    claim(
                        "conviction (-0.124)",
                        "-0.124",
                        "markov.signed_signal",
                        "consistent",
                    ),
                    claim("rsi 61.3", "61.3", "daily_indicators.rsi14", "consistent"),
                    claim("quiver 0.80", "0.80", "quiver.signal", "consistent"),
                ],
            ),
            labelled("b", vec![]),
        ];
        (cases, labels)
    }

    /// Every valid-context case passes the evidence check, every key is the
    /// key's own arithmetic, corruption is its own family, and every case not
    /// generated is counted with its reason.
    #[test]
    fn valid_context_cases_pass_the_check_and_rejections_are_counted() {
        let (cases, labels) = seeded_note();
        let generated = generate_seeded_v2(&cases, &labels);
        let made: BTreeSet<(&str, &str)> = generated
            .cases
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
            ("corruption", "single_field_both_moved"),
        ] {
            assert!(made.contains(&expected), "{expected:?}: {made:?}");
        }
        for case in &generated.cases {
            if case.family == "valid_context" {
                assert_eq!(
                    evidence_problems(&case.evidence),
                    Vec::<&str>::new(),
                    "{}",
                    case.id
                );
            } else {
                assert_eq!(case.family, "corruption", "{}", case.id);
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
        // Review's case: Quiver 0.8 cannot move up coherently, and the
        // refusals are counted, not hidden.
        assert!(
            !generated
                .cases
                .iter()
                .any(|case| case.family == "valid_context"
                    && number_at(&case.evidence, "quiver.signal")
                        .is_some_and(|signal| signal > 1.0))
        );
        assert!(generated.not_generated["evidence_moved_up"]["outside_bounds"] >= 1);
        assert!(generated.not_generated["both_moved"]["outside_bounds"] >= 1);
        assert!(generated.not_generated["evidence_moved_up"]["dependents_not_reproduced"] >= 3);
    }

    /// Review's other case: a note whose own evidence the runtime could not
    /// have produced seeds nothing, not even a change to the note alone.
    #[test]
    fn a_note_with_incoherent_evidence_seeds_nothing() {
        let (mut cases, labels) = seeded_note();
        cases[0].evidence["markov"]["conviction"] = json!(0.2);
        let generated = generate_seeded_v2(&cases, &labels);
        assert!(generated.cases.iter().all(|case| case.from_note != "a"));
        assert!(generated.not_generated["note_value_up"]["original_evidence_incoherent"] >= 1);
    }

    fn manifest(rows: JsonValue, eligible: JsonValue) -> JsonValue {
        json!({
            "manifest_query": MANIFEST_QUERY,
            "dump_query": DUMP_QUERY,
            "taken_at": "2026-10-10T06:00:00Z",
            "rows": rows,
            "eligible": eligible,
        })
    }

    fn report(id: i64, created_at: &str) -> JsonValue {
        json!({"id": id, "created_at": created_at, "status": "completed", "report": {}, "request": {}})
    }

    #[test]
    fn a_frame_is_built_only_from_its_complete_manifest() {
        let rows = json!([
            {"id": 400, "created_at": "2026-09-28T08:49:05Z", "status": "completed", "has_report": true, "has_request": true},
            {"id": 401, "created_at": "2026-09-28T12:22:48Z", "status": "failed", "has_report": false, "has_request": true, "exclusion": "failed: no report"},
            {"id": 402, "created_at": "2026-09-28T14:47:03Z", "status": "completed", "has_report": true, "has_request": true},
        ]);
        let good = manifest(rows.clone(), json!([400, 402]));
        let dump = vec![
            report(400, "2026-09-28T08:49:05Z"),
            report(402, "2026-09-28T14:47:03Z"),
        ];
        assert_eq!(validate_frame(&good, &dump), Ok(vec![400, 402]));

        // A partial export, an empty one, and one with a stray report.
        assert!(validate_frame(&good, &dump[..1]).is_err());
        assert!(validate_frame(&good, &[]).is_err());
        let stray = [dump.clone(), vec![report(403, "2026-09-29T08:49:05Z")]].concat();
        assert!(validate_frame(&good, &stray).is_err());
        // An empty frame, an exclusion without a reason, a manifest taken too
        // early, and a query other than the recorded one.
        assert!(validate_frame(&manifest(json!([]), json!([])), &[]).is_err());
        let mut unexplained = good.clone();
        unexplained["rows"][1]["exclusion"] = json!("");
        assert!(validate_frame(&unexplained, &dump).is_err());
        let mut early = good.clone();
        early["taken_at"] = json!("2026-10-09T18:00:00Z");
        assert!(validate_frame(&early, &dump).is_err());
        let mut other_query = good;
        other_query["manifest_query"] = json!("select 1");
        assert!(validate_frame(&other_query, &dump).is_err());
    }

    #[test]
    fn generation_is_deterministic_and_never_calls_the_checker() {
        let (cases, labels) = seeded_note();
        let once = serde_json::to_string(&generate_seeded_v2(&cases, &labels).cases).unwrap();
        let mut shuffled = cases.clone();
        shuffled.reverse();
        assert_eq!(
            once,
            serde_json::to_string(&generate_seeded_v2(&shuffled, &labels).cases).unwrap()
        );
        let source = include_str!("v2.rs");
        let generator = &source[source.find("pub(crate) fn rounds(").unwrap()
            ..source.find("pub(crate) fn validate_frame").unwrap()];
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
/// psql -c "<MANIFEST_QUERY>"  -> docs/jev-fresh-v2-frame-manifest.json (with taken_at, eligible, exclusions)
/// psql -tAc "<DUMP_QUERY>"    -> fresh-v2-frame.json
/// JEV_FRESH_PATH=fresh-v2-frame.json cargo test build_the_fresh_v2_instrument -- --ignored
/// cargo test generate_the_v2_seeded_cases -- --ignored
/// cargo test score_the_fresh_v2_set -- --ignored --nocapture
/// ```
#[cfg(test)]
mod frozen {
    use super::*;

    const MANIFEST_PATH: &str = "docs/jev-fresh-v2-frame-manifest.json";
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

    /// The generated cases and refusals, as they read back from a file.
    fn generated_as_stored(cases: &[NoteCase], labels: &[FreshLabel]) -> JsonValue {
        let generated = generate_seeded_v2(cases, labels);
        let text = serde_json::to_string(&json!({
            "cases": generated.cases,
            "not_generated": generated.not_generated,
        }))
        .expect("serialize");
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

    /// The frame is exactly the manifest's eligible reports: none missing,
    /// none extra, all inside the window.
    #[test]
    fn the_frame_matches_its_manifest() {
        let (Some((cases, keys, key)), Some(manifest)) = (frozen(), document(MANIFEST_PATH)) else {
            return;
        };
        assert_eq!(key["method_version"], FROZEN_METHOD_V2);
        assert_eq!(key["checker_source_sha256"], CHECKER_SOURCE_SHA256);
        assert_eq!(key["protocol"], PROTOCOL_V2);
        assert_eq!(key["frame"]["manifest_sha256"], sha256_of(MANIFEST_PATH));
        let reported: Vec<i64> = key["frame"]["reports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|report| report["id"].as_i64())
            .collect();
        assert_eq!(json!(reported), manifest["eligible"]);
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
        let generated = generated_as_stored(&cases, &labels);
        assert_eq!(seeded["cases"], generated["cases"]);
        assert_eq!(seeded["not_generated"], generated["not_generated"]);
    }

    #[test]
    #[ignore]
    fn build_the_fresh_v2_instrument() {
        assert_eq!(crate::jev_numeric::NUMERIC_METHOD_VERSION, FROZEN_METHOD_V2);
        assert_eq!(checker_source_sha256(), CHECKER_SOURCE_SHA256);
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(
            now.as_str() >= FRAME_CLOSES,
            "the frame closes at {FRAME_CLOSES}; it is {now}"
        );
        assert!(
            !std::path::Path::new(INSTRUMENT_PATH).exists(),
            "built once"
        );
        let manifest = document(MANIFEST_PATH).expect("the recorded frame manifest");
        let path = std::env::var("JEV_FRESH_PATH").expect("JEV_FRESH_PATH");
        let raw = std::fs::read(&path).expect("the frame dump");
        let sources: Vec<JsonValue> = serde_json::from_slice(&raw).expect("a JSON array");
        let eligible = validate_frame(&manifest, &sources)
            .unwrap_or_else(|problems| panic!("the frame is not complete: {problems:#?}"));
        let (reports, notes) = frame_notes(&sources, SEED_V2, |_, created_at| in_frame(created_at));
        let reported: Vec<i64> = reports
            .iter()
            .filter_map(|report| report["id"].as_i64())
            .collect();
        assert_eq!(reported, eligible, "every eligible report is in the frame");
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
                "checker_source_sha256": CHECKER_SOURCE_SHA256,
                "frozen_commit": FROZEN_COMMIT_V2,
                "protocol": PROTOCOL_V2,
                "warning": "Do not read this before labelling.",
                "frame": {
                    "opens": FRAME_OPENS,
                    "closes": FRAME_CLOSES,
                    "manifest_sha256": sha256_of(MANIFEST_PATH),
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
        let generated = generate_seeded_v2(&cases, &labels);
        std::fs::write(
            SEEDED_PATH,
            serde_json::to_string_pretty(&json!({
                "version": FRESH_V2_VERSION,
                "protocol": PROTOCOL_V2,
                "generated_from": {
                    "instrument_sha256": sha256_of(INSTRUMENT_PATH),
                    "labels_sha256": sha256_of(LABELS_PATH),
                },
                "not_generated": generated.not_generated,
                "cases": generated.cases,
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
        let generated = generated_as_stored(&cases, &labels);
        assert_eq!(seeded["cases"], generated["cases"]);
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
            "checker_source_sha256": CHECKER_SOURCE_SHA256,
            "labelled_by": labels_document["labelled_by"],
            "labelled_at": labels_document["labelled_at"],
            "fingerprints": {
                "manifest_sha256": sha256_of(MANIFEST_PATH),
                "instrument_sha256": sha256_of(INSTRUMENT_PATH),
                "key_sha256": sha256_of(KEY_PATH),
                "labels_sha256": sha256_of(LABELS_PATH),
                "seeded_sha256": sha256_of(SEEDED_PATH),
            },
            "not_generated": seeded["not_generated"],
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
