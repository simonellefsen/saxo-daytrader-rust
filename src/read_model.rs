//! Shared decoding boundary for typed read models built over dynamic JSON.
//!
//! Every dashboard and public-API projection in this repository is a typed
//! struct decoded from JSON that is *assembled at runtime* from database rows,
//! broker snapshots, and analysis output. Those builders legitimately emit an
//! explicit `null` for a value they do not have.
//!
//! `#[serde(default)]` does not cover that. It fills in a field whose key is
//! *absent*; an explicit `null` still fails with `invalid type: null, expected
//! a string`. Serde rejects the whole payload on the first bad field, so one
//! null anywhere blanks an entire tab, and the symptom — every count zero,
//! every list empty — reads as a data outage rather than as a decoder problem.
//! That is exactly how the Watchlists tab failed on 2026-08-31, when a single
//! symbol carried `"currency": null`.
//!
//! [`decode`] restores one invariant across every boundary at once:
//!
//! > **An explicit `null` is never worse than an absent key.**
//!
//! It decodes strictly first, so a payload that is valid today keeps its exact
//! current meaning, including nulls deliberately retained inside staged
//! diagnostic `JsonValue` fields. Only when the strict decode fails does it
//! retry with null object members dropped, which is precisely the shape
//! `#[serde(default)]` already handles. A genuinely required field still fails
//! closed, the same way it does when its key is missing — this widens
//! tolerance for optional data, it does not weaken a contract.
//!
//! The retry is logged. A payload that survives only the tolerant pass means a
//! builder emits `null` where the read model expects a value, and that is
//! worth fixing at the source rather than tolerating silently forever.
//!
//! This boundary is for read models only. Inbound MCP request bodies, provider
//! responses, and broker payloads keep their strict decoders: fail-closed is
//! the right answer when the decoded value can authorize work.

use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use tracing::warn;

/// Decodes one typed read-model payload under the null-is-absent invariant.
///
/// `boundary` names the projection in the warning emitted when a payload only
/// decodes after its nulls are dropped.
pub(crate) fn decode<T>(boundary: &'static str, value: JsonValue) -> serde_json::Result<T>
where
    T: DeserializeOwned,
{
    let (decoded, tolerated) = decode_tolerating_nulls(value)?;
    if let Some(strict) = tolerated {
        warn_tolerated(boundary, &strict, 1);
    }
    Ok(decoded)
}

/// Decodes a list of typed read-model rows under the same invariant.
///
/// The conversion stays all-or-nothing on purpose: a row that cannot be
/// decoded at all is a malformed row, and silently dropping it would draw a
/// partial list that looks complete.
///
/// A list warns **once** for the whole batch rather than once per row. A field
/// that is null in one row is usually null in most of them, and a page of
/// identical warnings buries the boundary name it is trying to report.
pub(crate) fn decode_each<T>(
    boundary: &'static str,
    values: Vec<JsonValue>,
) -> serde_json::Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let mut decoded = Vec::with_capacity(values.len());
    let mut first_tolerated: Option<serde_json::Error> = None;
    let mut tolerated_rows = 0_usize;
    for value in values {
        let (row, tolerated) = decode_tolerating_nulls(value)?;
        if let Some(strict) = tolerated {
            tolerated_rows += 1;
            first_tolerated.get_or_insert(strict);
        }
        decoded.push(row);
    }
    if let Some(strict) = first_tolerated {
        warn_tolerated(boundary, &strict, tolerated_rows);
    }
    Ok(decoded)
}

/// Decodes strictly, then retries with nulls dropped. The returned error is
/// `Some` when the retry was what saved the payload, carrying the strict error
/// so the caller can report why.
fn decode_tolerating_nulls<T>(
    value: JsonValue,
) -> serde_json::Result<(T, Option<serde_json::Error>)>
where
    T: DeserializeOwned,
{
    match T::deserialize(&value) {
        Ok(decoded) => Ok((decoded, None)),
        Err(strict) => match serde_json::from_value(null_as_absent(value)) {
            Ok(decoded) => Ok((decoded, Some(strict))),
            // The strict error names the first real problem with the payload.
            // The tolerant retry can only report whatever is left once the
            // nulls are gone, which is the less useful diagnostic.
            Err(_) => Err(strict),
        },
    }
}

fn warn_tolerated(boundary: &'static str, strict: &serde_json::Error, rows: usize) {
    warn!(
        boundary,
        rows,
        error = %strict,
        "typed read model decoded only after explicit nulls were treated as absent keys"
    );
}

/// Rewrites a payload so an explicit `null` reads as an absent key.
///
/// Null *object members* are dropped. Array elements are positional, so a null
/// element is left in place rather than renumbering everything after it; a
/// null row is a builder bug and should still be visible as a decode failure.
fn null_as_absent(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => JsonValue::Object(
            map.into_iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(key, member)| (key, null_as_absent(member)))
                .collect(),
        ),
        JsonValue::Array(items) => {
            JsonValue::Array(items.into_iter().map(null_as_absent).collect())
        }
        other => other,
    }
}

/// One step of a path to an object member inside a JSON payload.
#[cfg(test)]
#[derive(Clone, Debug)]
enum Step {
    Key(String),
    Index(usize),
}

/// Asserts the null-is-absent invariant at one payload boundary.
///
/// For every object member anywhere in `fixture` — nested payloads and rows
/// inside arrays included — this checks that setting the member to an explicit
/// `null` still decodes whenever removing it decodes. That is the property the
/// 2026-08-31 Watchlists outage violated, and testing every position is what
/// keeps a field added later from quietly reopening the hole.
///
/// The check is deliberately one-directional. A staged `JsonValue` field
/// accepts a null and rejects a missing key, so `null` may legitimately decode
/// where absent does not.
#[cfg(test)]
pub(crate) fn assert_null_is_never_worse_than_absent<T, E, F>(
    fixture: &JsonValue,
    decode_boundary: F,
) where
    E: std::fmt::Display,
    F: Fn(JsonValue) -> Result<T, E>,
{
    let paths = object_member_paths(fixture);
    assert!(
        !paths.is_empty(),
        "the fixture carries no object member, so it cannot exercise the invariant"
    );
    if let Err(err) = decode_boundary(fixture.clone()) {
        panic!(
            "the fixture must decode before any member is rewritten, or every case below is \
             skipped: {err}"
        );
    }
    for path in paths {
        if decode_boundary(with_member(fixture, &path, None)).is_err() {
            continue;
        }
        if let Err(err) = decode_boundary(with_member(fixture, &path, Some(JsonValue::Null))) {
            panic!(
                "`{}` decodes when its key is absent but fails when it is explicitly null: {err}",
                render_path(&path)
            );
        }
    }
}

#[cfg(test)]
fn object_member_paths(value: &JsonValue) -> Vec<Vec<Step>> {
    fn walk(value: &JsonValue, prefix: &mut Vec<Step>, paths: &mut Vec<Vec<Step>>) {
        match value {
            JsonValue::Object(map) => {
                for (key, member) in map {
                    prefix.push(Step::Key(key.clone()));
                    paths.push(prefix.clone());
                    walk(member, prefix, paths);
                    prefix.pop();
                }
            }
            JsonValue::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    prefix.push(Step::Index(index));
                    walk(item, prefix, paths);
                    prefix.pop();
                }
            }
            _ => {}
        }
    }

    let mut paths = Vec::new();
    walk(value, &mut Vec::new(), &mut paths);
    paths
}

/// Returns `fixture` with the member at `path` replaced, or removed when
/// `member` is `None`. Every path ends at an object key by construction.
#[cfg(test)]
fn with_member(fixture: &JsonValue, path: &[Step], member: Option<JsonValue>) -> JsonValue {
    fn apply(value: &mut JsonValue, path: &[Step], member: Option<JsonValue>) {
        let Some((step, rest)) = path.split_first() else {
            return;
        };
        if rest.is_empty() {
            if let (Step::Key(key), Some(map)) = (step, value.as_object_mut()) {
                match member {
                    Some(member) => {
                        map.insert(key.clone(), member);
                    }
                    None => {
                        map.remove(key);
                    }
                }
            }
            return;
        }
        let next = match step {
            Step::Key(key) => value.get_mut(key.as_str()),
            Step::Index(index) => value.get_mut(*index),
        };
        if let Some(next) = next {
            apply(next, rest, member);
        }
    }

    let mut rewritten = fixture.clone();
    apply(&mut rewritten, path, member);
    rewritten
}

#[cfg(test)]
fn render_path(path: &[Step]) -> String {
    let mut rendered = String::new();
    for step in path {
        match step {
            Step::Key(key) => {
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(key);
            }
            Step::Index(index) => rendered.push_str(&format!("[{index}]")),
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use super::*;

    #[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
    struct Row {
        symbol: String,
        #[serde(default)]
        currency: String,
        #[serde(default)]
        quantity: i64,
        #[serde(default)]
        active: bool,
        #[serde(default)]
        change_pct: Option<f64>,
        #[serde(default)]
        diagnostics: JsonValue,
    }

    #[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
    struct Envelope {
        generated_at: String,
        #[serde(default)]
        rows: Vec<Row>,
    }

    /// The shape that blanked the Watchlists tab: a defaulted field carrying an
    /// explicit null rather than omitting its key.
    #[test]
    fn an_explicit_null_decodes_like_an_absent_key() {
        let row: Row = decode(
            "test_row",
            json!({"symbol": "ABB:xome", "currency": null, "quantity": null, "active": null}),
        )
        .expect("a null on a defaulted field must not fail the payload");

        assert_eq!(row.symbol, "ABB:xome");
        assert_eq!(row.currency, "");
        assert_eq!(row.quantity, 0);
        assert!(!row.active);
    }

    /// One bad row must not be able to blank the whole envelope, which is the
    /// property that actually keeps a tab populated.
    #[test]
    fn a_null_bearing_row_does_not_fail_its_envelope() {
        let envelope: Envelope = decode(
            "test_envelope",
            json!({
                "generated_at": "2026-08-31T08:40:58Z",
                "rows": [
                    {"symbol": "AAPL:xnas", "currency": "USD"},
                    {"symbol": "ABB:xome", "currency": null}
                ]
            }),
        )
        .expect("a nested null must not fail the envelope");

        assert_eq!(envelope.rows.len(), 2);
        assert_eq!(envelope.rows[1].currency, "");
    }

    /// Lists take the same invariant row by row. The batch warns once rather
    /// than once per row, so this also covers the path where several rows need
    /// the tolerant pass.
    #[test]
    fn a_list_tolerates_nulls_in_several_rows_at_once() {
        let rows: Vec<Row> = decode_each(
            "test_rows",
            vec![
                json!({"symbol": "AAPL:xnas", "currency": "USD"}),
                json!({"symbol": "ABB:xome", "currency": null}),
                json!({"symbol": "NOVO-B:xcse", "currency": null, "quantity": null}),
            ],
        )
        .expect("nulls across rows must not fail the list");

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].currency, "USD");
        assert_eq!(rows[1].currency, "");
        assert_eq!(rows[2].quantity, 0);
    }

    /// A list stays all-or-nothing: a row that cannot be decoded at all is
    /// malformed, and dropping it would draw a partial list that looks
    /// complete.
    #[test]
    fn a_list_still_fails_on_a_row_that_is_malformed_rather_than_null() {
        assert!(
            decode_each::<Row>(
                "test_rows",
                vec![
                    json!({"symbol": "AAPL:xnas"}),
                    json!({"symbol": {"not": "a string"}}),
                ],
            )
            .is_err()
        );
    }

    /// Tolerance is for values the read model already treats as optional. A
    /// field with no default still fails closed on a null, exactly as it does
    /// when its key is missing, so this cannot turn a structural contract into
    /// a silent default.
    #[test]
    fn a_required_field_still_fails_closed() {
        assert!(decode::<Row>("test_row", json!({"symbol": null})).is_err());
        assert!(decode::<Row>("test_row", json!({})).is_err());
    }

    /// The strict pass runs first, so a staged diagnostic document keeps the
    /// nulls an operator needs to read it correctly.
    #[test]
    fn a_valid_payload_keeps_the_nulls_inside_staged_diagnostics() {
        let row: Row = decode(
            "test_row",
            json!({"symbol": "AAPL:xnas", "diagnostics": {"gate": null, "reason": "cash"}}),
        )
        .expect("a valid payload decodes strictly");

        assert_eq!(row.diagnostics, json!({"gate": null, "reason": "cash"}));
    }

    #[test]
    fn null_array_elements_stay_positional() {
        assert_eq!(
            null_as_absent(json!({"rows": [{"a": null}, null, {"b": 1}], "gone": null})),
            json!({"rows": [{}, null, {"b": 1}]})
        );
    }

    #[test]
    fn the_property_helper_visits_every_object_member() {
        let fixture = json!({
            "generated_at": "2026-08-31T08:40:58Z",
            "rows": [{"symbol": "AAPL:xnas", "diagnostics": {"gate": "cash"}}]
        });

        // Object members are visited in `serde_json::Map` order, so compare a
        // sorted list rather than depending on it.
        let mut rendered: Vec<String> = object_member_paths(&fixture)
            .iter()
            .map(|path| render_path(path))
            .collect();
        rendered.sort();
        assert_eq!(
            rendered,
            vec![
                "generated_at",
                "rows",
                "rows[0].diagnostics",
                "rows[0].diagnostics.gate",
                "rows[0].symbol",
            ]
        );

        assert_null_is_never_worse_than_absent(&fixture, |value| {
            decode::<Envelope>("test_envelope", value)
        });
    }

    /// The helper has to be able to fail, or a boundary test proves nothing.
    #[test]
    #[should_panic(expected = "explicitly null")]
    fn the_property_helper_catches_a_field_that_rejects_null() {
        #[derive(Debug, Deserialize)]
        struct Strict {
            #[serde(default)]
            label: String,
        }

        assert_eq!(
            serde_json::from_value::<Strict>(json!({"label": "x"}))
                .expect("the fixture decodes before any member is rewritten")
                .label,
            "x"
        );
        assert_null_is_never_worse_than_absent(&json!({"label": "x"}), |value| {
            serde_json::from_value::<Strict>(value)
        });
    }
}
