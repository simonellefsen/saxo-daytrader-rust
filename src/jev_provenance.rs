//! Where the Markov figures the evidence lacked came from.
//!
//! The controls reconciliation left one question open. Twelve sampled claims
//! quote a Markov figure while the candidate's grading evidence has `markov`
//! null, and the instrument alone could not say whether those figures came
//! from somewhere in the prompt or were invented.
//!
//! This measures it across the whole frame rather than the sample. For every
//! claim the checker attributes to a `markov.*` field where the evidence has no
//! Markov block, it searches the entire prompt — every numeric leaf, every
//! number written inside a string, and the system text — for a value the
//! figure could have been read from, under the checker's own precision rules.
//! Each claim is assigned the most specific place it was found.
//!
//! Two things keep the answer honest:
//!
//! - **A control.** Markov claims whose evidence *is* present and that the
//!   checker matched must be found in the evidence's own source, for the same
//!   symbol. If the search cannot find those, it cannot be trusted to find
//!   anything.
//! - **A coincidence baseline.** A prompt holds thousands of numbers, and a
//!   three-decimal figure will sometimes match one by chance. Every target
//!   figure is perturbed by a few units in its last place and searched again;
//!   how often a perturbed figure still lands on the same symbol is the rate at
//!   which a same-symbol match means nothing.
//!
//! `#[ignore]`d: it needs the same dump the controls were drawn from.
//!
//! ```text
//! JEV_PROMPTS_PATH=all.json cargo test where_the_missing_markov_figures_came_from -- --ignored --nocapture
//! ```

use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;

/// Where a figure was found, most specific first. A claim is assigned the
/// first place any of its matches falls in.
const PLACES: [&str; 8] = [
    // `markov_method.signals`, the list the grading evidence is built from.
    // Only a control can be found here: a target's evidence is null precisely
    // because its symbol is absent from this list.
    "evidence_list_same_symbol",
    // `markov_method.latest_run.summary_json.signals`, full signal rows the
    // prompt embedded for debugging until 2026-08-03 (5a19a4a).
    "embedded_run_rows_same_symbol",
    // The model's own earlier report. Ranked above any other Markov-named
    // block because a figure found there was written by a model, not supplied
    // by the runtime, whatever the key it sits under.
    "earlier_report_same_symbol",
    "other_markov_block_same_symbol",
    "elsewhere_same_symbol",
    "system_text",
    "other_symbols_only",
    "nowhere",
];

/// The checker's precision rule: the figure agrees with a value if rounding
/// (either convention) or truncating the value at the figure's decimals gives
/// the figure.
fn quote_match(quoted: f64, decimals: usize, value: f64) -> bool {
    let scale = 10f64.powi(decimals as i32);
    let scaled = value * scale;
    let half_even = {
        let rounded = scaled.round();
        if ((scaled - scaled.trunc()).abs() - 0.5).abs() < f64::EPSILON && rounded % 2.0 != 0.0 {
            rounded - scaled.signum()
        } else {
            rounded
        }
    };
    let slack = quoted.abs().max(value.abs()).max(1.0) * 8.0 * f64::EPSILON;
    [scaled.round(), half_even, scaled.trunc()]
        .iter()
        .any(|candidate| (quoted - candidate / scale).abs() <= slack)
}

/// A figure as the note wrote it.
#[derive(Clone, Copy)]
struct Figure {
    value: f64,
    decimals: usize,
    /// An unsigned figure may have dropped the sign of what it quotes ("most
    /// negative Markov, 0.435"); a signed one may not.
    explicit_sign: bool,
}

impl Figure {
    fn found_in(self, value: f64) -> bool {
        quote_match(self.value, self.decimals, value)
            || (!self.explicit_sign && quote_match(self.value, self.decimals, -value))
    }

    /// Whether ordinary rounding alone gives the figure, without the
    /// checker's allowance for truncation.
    fn rounds_to(self, value: f64) -> bool {
        let scale = 10f64.powi(self.decimals as i32);
        let slack = self.value.abs().max(value.abs()).max(1.0) * 8.0 * f64::EPSILON;
        let rounds = |value: f64| ((value * scale).round() / scale - self.value).abs() <= slack;
        rounds(value) || (!self.explicit_sign && rounds(-value))
    }
}

/// Numbers written inside a string, with their decimals.
fn numbers_in(text: &str) -> Vec<f64> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let starts = bytes[index].is_ascii_digit()
            && (index == 0
                || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'.'));
        if !starts {
            index += 1;
            continue;
        }
        let begin = index;
        while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
            index += 1;
        }
        let token = text[begin..index].trim_end_matches('.');
        let negative =
            begin > 0 && (bytes[begin - 1] == b'-' || text[..begin].ends_with('\u{2212}'));
        if let Ok(value) = token.parse::<f64>() {
            found.push(if negative { -value } else { value });
        }
    }
    found
}

/// Every leaf of the prompt, with its path and the symbol of the nearest
/// enclosing object that names one.
fn leaves<'a>(
    value: &'a JsonValue,
    path: &str,
    symbol: Option<&'a str>,
    out: &mut Vec<(String, Option<&'a str>, &'a JsonValue)>,
) {
    match value {
        JsonValue::Object(map) => {
            let symbol = map.get("symbol").and_then(JsonValue::as_str).or(symbol);
            for (key, child) in map {
                leaves(child, &format!("{path}.{key}"), symbol, out);
            }
        }
        JsonValue::Array(items) => {
            for child in items {
                leaves(child, &format!("{path}[]"), symbol, out);
            }
        }
        _ => out.push((path.to_string(), symbol, value)),
    }
}

fn place_of(path: &str, context: Option<&str>, symbol: &str) -> &'static str {
    let same = context == Some(symbol) || path.contains(symbol);
    if !same {
        return "other_symbols_only";
    }
    if path.starts_with(".markov_method.signals[]") {
        "evidence_list_same_symbol"
    } else if path.starts_with(".markov_method.latest_run") {
        "embedded_run_rows_same_symbol"
    } else if path.starts_with(".earlier_same_scope_report") {
        "earlier_report_same_symbol"
    } else if path.contains("markov") {
        "other_markov_block_same_symbol"
    } else {
        "elsewhere_same_symbol"
    }
}

/// The most specific place a figure appears in one prompt, and the leaves it
/// matched there with their values.
fn locate(
    figure: Figure,
    symbol: &str,
    prompt_leaves: &[(String, Option<&str>, &JsonValue)],
    system: &str,
) -> (&'static str, Vec<(String, f64)>) {
    let rank = |place: &str| {
        PLACES
            .iter()
            .position(|p| *p == place)
            .unwrap_or(PLACES.len())
    };
    let mut best = "nowhere";
    let mut matches: Vec<(String, f64)> = Vec::new();
    let mut consider = |place: &'static str, path: &str, value: f64| {
        if rank(place) < rank(best) {
            best = place;
            matches.clear();
        }
        if place == best && !matches.iter().any(|(p, v)| p == path && *v == value) {
            matches.push((path.to_string(), value));
        }
    };
    for (path, context, leaf) in prompt_leaves {
        let values = match leaf {
            JsonValue::Number(number) => number.as_f64().into_iter().collect(),
            JsonValue::String(text) => numbers_in(text),
            _ => Vec::new(),
        };
        for value in values.into_iter().filter(|value| figure.found_in(*value)) {
            consider(place_of(path, *context, symbol), path, value);
        }
    }
    for value in numbers_in(system)
        .into_iter()
        .filter(|value| figure.found_in(*value))
    {
        consider("system_text", "system", value);
    }
    (best, matches)
}

fn tally(places: &[&'static str]) -> JsonValue {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for place in places {
        *counts.entry(place).or_default() += 1;
    }
    json!(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_figure_is_found_at_the_checkers_precision() {
        let figure = Figure {
            value: 0.577,
            decimals: 3,
            explicit_sign: true,
        };
        assert!(figure.found_in(0.5769323255186927), "rounded");
        assert!(figure.found_in(0.5771), "truncated");
        assert!(!figure.found_in(0.5781));
        assert!(
            !figure.found_in(-0.5769323255186927),
            "a written sign binds"
        );
        let unsigned = Figure {
            value: 0.435,
            decimals: 3,
            explicit_sign: false,
        };
        assert!(
            unsigned.found_in(-0.43505904873452383),
            "an unwritten sign does not"
        );
    }

    #[test]
    fn numbers_are_read_out_of_prose() {
        assert_eq!(
            numbers_in("Bull (+0.577), RSI 58; v2.1 and -0.435"),
            vec![0.577, 58.0, -0.435]
        );
    }

    #[test]
    fn the_nearest_symbol_decides_whose_figure_it_is() {
        let prompt = json!({
            "markov_method": {
                "signals": [{"symbol": "AAPL:xnas", "signed_signal": 0.1}],
                "latest_run": {"summary_json": {"signals": [
                    {"symbol": "JPM:xnys", "signed_signal": 0.5769},
                ]}},
            },
        });
        let mut found = Vec::new();
        leaves(&prompt, "", None, &mut found);
        let figure = Figure {
            value: 0.577,
            decimals: 3,
            explicit_sign: true,
        };
        assert_eq!(
            locate(figure, "JPM:xnys", &found, "").0,
            "embedded_run_rows_same_symbol"
        );
        assert_eq!(locate(figure, "V:xnys", &found, "").0, "other_symbols_only");
    }

    /// Every distinct claim in the frame with the checker's reading of it, so
    /// two method versions can be compared claim by claim from two recorded
    /// runs rather than from memory.
    ///
    /// ```text
    /// JEV_PROMPTS_PATH=all.json JEV_CENSUS_OUT=census.json \
    ///   cargo test --release census_of_every_claim -- --ignored
    /// ```
    #[test]
    #[ignore]
    fn census_of_every_claim() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let sources: Vec<JsonValue> =
            serde_json::from_slice(&std::fs::read(&path).expect("prompts")).expect("a JSON array");
        let mut claims: BTreeMap<String, JsonValue> = BTreeMap::new();
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
                let symbol = candidate["symbol"].as_str().unwrap_or_default();
                let lowered = note.to_lowercase();
                for check in crate::jev_numeric::numeric_checks(&lowered, evidence) {
                    let written = crate::jev_numeric::figure_at(&lowered, check.offset)
                        .map(|span| lowered[span.start..span.end].to_string());
                    claims
                        .entry(format!("{report_id}|{symbol}|{}", check.offset))
                        .or_insert_with(|| {
                            json!({
                                "figure": written,
                                "field": check.field,
                                "actual": check.actual,
                                "relation": check.relation,
                                "verdict": check.verdict.as_str(),
                                "excerpt": check.excerpt,
                            })
                        });
                }
            }
        }
        let mut verdicts: BTreeMap<String, usize> = BTreeMap::new();
        for claim in claims.values() {
            *verdicts
                .entry(claim["verdict"].as_str().unwrap_or_default().to_string())
                .or_default() += 1;
        }
        let result = json!({
            "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
            "claims": claims.len(),
            "by_verdict": verdicts,
            "by_claim": claims,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&result["by_verdict"]).expect("json")
        );
        if let Ok(out) = std::env::var("JEV_CENSUS_OUT") {
            std::fs::write(out, serde_json::to_string_pretty(&result).expect("json"))
                .expect("write result");
        }
    }

    /// What reading the embedded rows as evidence changes, claim by claim.
    ///
    /// Like for like and exact. A candidate whose Markov evidence now comes
    /// from the embedded rows had, under `n10`, the same evidence with `markov`
    /// null, so both are rebuilt here from the same prompt. Every other
    /// candidate's evidence is byte-identical under both, and the parser did
    /// not change, so its verdicts cannot move.
    #[test]
    #[ignore]
    fn what_reading_the_embedded_rows_changes() {
        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let sources: Vec<JsonValue> =
            serde_json::from_slice(&std::fs::read(&path).expect("prompts")).expect("a JSON array");
        let mut transitions: BTreeMap<String, usize> = BTreeMap::new();
        let mut changed: BTreeMap<String, JsonValue> = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        let (mut candidates, mut reports) = (0usize, std::collections::BTreeSet::new());
        for entry in &sources {
            let (Some(report), Some(request)) = (entry.get("report"), entry.get("request")) else {
                continue;
            };
            let report_id = entry.get("id").and_then(JsonValue::as_i64).unwrap_or(-1);
            let prompt = crate::xai_decision::decision_prompt_user_payload(request);
            let inputs = crate::jev_review::grading_inputs(report, &prompt);
            for (index, candidate) in inputs.candidates.iter().enumerate() {
                if inputs.numeric[index]["markov_source"] != "embedded_run_rows" {
                    continue;
                }
                let (Some(note), Some(evidence)) = (
                    candidate.get("note").and_then(JsonValue::as_str),
                    inputs.evidence.get(index),
                ) else {
                    continue;
                };
                let symbol = candidate["symbol"].as_str().unwrap_or_default();
                let lowered = note.to_lowercase();
                let mut before = evidence.clone();
                before["markov"] = JsonValue::Null;
                let old = crate::jev_numeric::numeric_checks(&lowered, &before);
                let new = crate::jev_numeric::numeric_checks(&lowered, evidence);
                candidates += 1;
                reports.insert(report_id);
                for check in &new {
                    let key = format!("{report_id}|{symbol}|{}", check.offset);
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let was = old
                        .iter()
                        .find(|earlier| earlier.offset == check.offset)
                        .map_or("absent", |earlier| earlier.verdict.as_str());
                    let now = check.verdict.as_str();
                    *transitions.entry(format!("{was} -> {now}")).or_default() += 1;
                    if was != now || now == "not_in_evidence" {
                        let written = crate::jev_numeric::figure_at(&lowered, check.offset)
                            .map(|span| lowered[span.start..span.end].to_string());
                        changed.insert(
                            key,
                            json!({
                                "report": report_id,
                                "symbol": symbol,
                                "figure": written,
                                "field": check.field,
                                "actual": check.actual,
                                "was": was,
                                "now": now,
                            }),
                        );
                    }
                }
            }
        }
        let result = json!({
            "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
            "candidates_reading_embedded_rows": candidates,
            "reports": reports.len(),
            "transitions": transitions,
            "changed_or_still_unevidenced": changed.values().collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&result).expect("json"));
        if let Ok(out) = std::env::var("JEV_CHANGES_OUT") {
            std::fs::write(out, serde_json::to_string_pretty(&result).expect("json"))
                .expect("write result");
        }
    }

    /// The measurement. See the module comment.
    #[test]
    #[ignore]
    fn where_the_missing_markov_figures_came_from() {
        use sha2::Digest;

        let path = std::env::var("JEV_PROMPTS_PATH").expect("JEV_PROMPTS_PATH");
        let raw = std::fs::read(&path).expect("prompts");
        let dump_sha256 = format!("{:x}", sha2::Sha256::digest(&raw));
        let sources: Vec<JsonValue> = serde_json::from_slice(&raw).expect("a JSON array");

        // Keyed as the controls sample keyed its claims, so a report graded
        // many times contributes once.
        let mut targets: BTreeMap<String, JsonValue> = BTreeMap::new();
        let mut control_places = Vec::new();
        let mut baseline_places = Vec::new();
        let mut baseline_same_field = 0usize;
        let mut reports_with_targets: BTreeMap<i64, JsonValue> = BTreeMap::new();
        for entry in &sources {
            let (Some(report), Some(request)) = (entry.get("report"), entry.get("request")) else {
                continue;
            };
            let report_id = entry.get("id").and_then(JsonValue::as_i64).unwrap_or(-1);
            let prompt = crate::xai_decision::decision_prompt_user_payload(request);
            let system = request
                .get("messages")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .filter(|message| message.get("role").and_then(JsonValue::as_str) == Some("system"))
                .filter_map(|message| message.get("content").and_then(JsonValue::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let embedded = prompt
                .pointer("/markov_method/latest_run/summary_json/signals")
                .and_then(JsonValue::as_array)
                .is_some_and(|rows| !rows.is_empty());
            let mut prompt_leaves = Vec::new();
            leaves(&prompt, "", None, &mut prompt_leaves);
            // The list the grading evidence reads, as the prompt held it.
            let listed: Vec<&str> = prompt
                .pointer("/markov_method/signals")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .filter_map(|signal| signal.get("symbol").and_then(JsonValue::as_str))
                .collect();
            let alphabetical =
                !listed.is_empty() && listed.windows(2).all(|pair| pair[0] <= pair[1]);
            let last_listed = listed.iter().max().copied().unwrap_or_default();

            let inputs = crate::jev_review::grading_inputs(report, &prompt);
            for (index, candidate) in inputs.candidates.iter().enumerate() {
                let (Some(note), Some(evidence)) = (
                    candidate.get("note").and_then(JsonValue::as_str),
                    inputs.evidence.get(index),
                ) else {
                    continue;
                };
                let symbol = candidate
                    .get("symbol")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let lowered = note.to_lowercase();
                for check in crate::jev_numeric::numeric_checks(&lowered, evidence) {
                    let Some(field) = check.field.filter(|field| field.starts_with("markov."))
                    else {
                        continue;
                    };
                    let Some(span) = crate::jev_numeric::figure_at(&lowered, check.offset) else {
                        continue;
                    };
                    let written = &lowered[span.start..span.end];
                    let figure = Figure {
                        value: check.quoted,
                        decimals: span.decimals,
                        explicit_sign: span.explicit_sign,
                    };
                    if evidence
                        .get("markov")
                        .is_some_and(|markov| !markov.is_null())
                    {
                        if check.verdict.as_str() == "matches" {
                            control_places.push(locate(figure, symbol, &prompt_leaves, &system).0);
                        }
                        continue;
                    }
                    let key = format!("{report_id}|{symbol}|{written}|{}", check.offset);
                    if targets.contains_key(&key) {
                        continue;
                    }
                    let (place, matched) = locate(figure, symbol, &prompt_leaves, &system);
                    let leaf = format!(".{}", field.trim_start_matches("markov."));
                    let same_symbol = |place: &str| place.ends_with("_same_symbol");
                    // A few units away in the last place, never across zero.
                    let unit = 10f64.powi(-(span.decimals as i32));
                    for step in [-13.0, -11.0, -7.0, -5.0, 5.0, 7.0, 11.0, 13.0] {
                        let moved = figure.value + step * unit;
                        if moved.signum() != figure.value.signum() || moved == 0.0 {
                            continue;
                        }
                        let perturbed = Figure {
                            value: moved,
                            ..figure
                        };
                        let (moved_place, moved_matches) =
                            locate(perturbed, symbol, &prompt_leaves, &system);
                        baseline_same_field += usize::from(
                            same_symbol(moved_place)
                                && moved_matches.iter().any(|(path, _)| path.ends_with(&leaf)),
                        );
                        baseline_places.push(moved_place);
                    }
                    reports_with_targets.insert(
                        report_id,
                        json!({
                            "embedded_run_rows": embedded,
                            "evidence_list_alphabetical": alphabetical,
                            "evidence_list_ends_at": last_listed,
                        }),
                    );
                    targets.insert(
                        key,
                        json!({
                            "report": report_id,
                            "symbol": symbol,
                            "figure": written,
                            "field": field,
                            "verdict": check.verdict.as_str(),
                            "prompt_embedded_run_rows": embedded,
                            "place": place,
                            "same_field": same_symbol(place)
                                && matched.iter().any(|(path, _)| path.ends_with(&leaf)),
                            "rounds_to_it": matched.iter().any(|(_, value)| figure.rounds_to(*value)),
                            "sorts_after_the_evidence_list": alphabetical && symbol > last_listed,
                            "matched_at": matched.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(),
                            "matched_values": matched.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
                        }),
                    );
                }
            }
        }

        let target_places: Vec<&'static str> = targets
            .values()
            .map(|target| {
                let place = target["place"].as_str().unwrap_or("nowhere");
                PLACES
                    .iter()
                    .copied()
                    .find(|p| *p == place)
                    .unwrap_or("nowhere")
            })
            .collect();
        let split = |embedded: bool| -> Vec<&'static str> {
            targets
                .values()
                .filter(|target| target["prompt_embedded_run_rows"] == embedded)
                .map(|target| {
                    let place = target["place"].as_str().unwrap_or("nowhere");
                    PLACES
                        .iter()
                        .copied()
                        .find(|p| *p == place)
                        .unwrap_or("nowhere")
                })
                .collect()
        };
        let count = |flag: &str| {
            targets
                .values()
                .filter(|target| target[flag] == true)
                .count()
        };
        let result = json!({
            "method_version": crate::jev_numeric::NUMERIC_METHOD_VERSION,
            "dump_sha256": dump_sha256,
            "reports_in_dump": sources.len(),
            "places_in_priority_order": PLACES,
            "targets": {
                "claims": targets.len(),
                "reports": reports_with_targets.len(),
                "by_place": tally(&target_places),
                "same_symbol_and_same_field": count("same_field"),
                "rounds_to_the_value_found": count("rounds_to_it"),
                "symbol_sorts_after_an_alphabetical_evidence_list": count("sorts_after_the_evidence_list"),
                "prompt_embedded_run_rows": tally(&split(true)),
                "prompt_without_embedded_run_rows": tally(&split(false)),
            },
            "control": {
                "what": "markov claims with markov evidence present that the checker matched",
                "claims": control_places.len(),
                "by_place": tally(&control_places),
            },
            "coincidence_baseline": {
                "what": "each target figure moved 5, 7, 11 and 13 units either way in its last place",
                "figures": baseline_places.len(),
                "by_place": tally(&baseline_places),
                "same_symbol_and_same_field": baseline_same_field,
            },
            "reports_with_targets": reports_with_targets,
            "claims": targets.values().collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&result).expect("json"));
        if let Ok(out) = std::env::var("JEV_PROVENANCE_OUT") {
            std::fs::write(out, serde_json::to_string_pretty(&result).expect("json"))
                .expect("write result");
        }
    }
}
