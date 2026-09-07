//! Server-measured yardstick for the mid-session shadow report.
//!
//! The shadow pass asks the model whether anything material changed since the
//! same day's opening report. Its verdict used to be whatever the model said it
//! was: the server checked only the shape of the answer -- `material_change`
//! must list at least one item, `no_new_information` must list none -- and never
//! checked whether the listed items were changes at all.
//!
//! Over the first 15 shadow reports (2026-08-26 to 09-04) that produced
//! `material_change` 13 times, 86.7%, which is close enough to always to carry
//! no information. Classifying the 55 claims behind those verdicts, 36 of them
//! were the system reading its own homework: "the opening BUY executed", "cash
//! declined from X to Y", "available buy budget is now Z". Those are the
//! deterministic consequences of a decision the runtime had already made and
//! already knew about, restated back to it as news.
//!
//! What actually differs between the two reports is narrower than it looks.
//! Comparing report 273 (US open) with report 274 (its shadow) three hours
//! later, the Markov block was the same run for all 200 symbols and the daily
//! indicator block the same run for all 80 -- byte-identical inputs, because
//! both are computed once a day. No signal-based delta is possible by
//! construction. The only genuinely new evidence at mid-session is where prices
//! have moved since the opening, and on that day one position of eleven had
//! moved more than 1% (CEG +1.94%).
//!
//! So the yardstick is price movement, measured here against prices the server
//! recorded at the opening report rather than against anything the provider
//! reports. The model's prose is kept as commentary; it no longer decides.

use serde_json::{Value as JsonValue, json};

/// Default minimum move for one symbol to count as a material change.
///
/// 1.5% would have fired once in the 2026-09-04 US session, on the one name
/// that moved 1.94%, and stayed quiet for the other ten. A threshold that fires
/// on every session is the failure this replaces, so it is set to notice a name
/// doing something rather than to notice a market being open.
pub(crate) const DEFAULT_MIN_PRICE_MOVE_PCT: f64 = 0.015;

/// One symbol whose price moved past the threshold since the opening report.
fn price_delta(
    symbol: &str,
    earlier_price: f64,
    current_price: f64,
    currency: &str,
) -> Option<JsonValue> {
    if !earlier_price.is_finite() || !current_price.is_finite() || earlier_price <= 0.0 {
        return None;
    }
    let move_pct = current_price / earlier_price - 1.0;
    if !move_pct.is_finite() {
        return None;
    }
    Some(json!({
        "kind": "price_move",
        "symbol": symbol,
        "currency": currency,
        "earlier_price_local": earlier_price,
        "current_price_local": current_price,
        "move_pct": move_pct,
    }))
}

/// Measures every symbol held at both points and returns those past the bar.
///
/// Only symbols present on both sides are comparable. A position opened by the
/// opening report itself has no earlier price and is therefore not a change --
/// which is the point, since "the trade I just made now exists" was the single
/// largest category of claimed material change.
pub(crate) fn measured_price_changes(
    earlier_prices: &JsonValue,
    current_positions: &[JsonValue],
    min_move_pct: f64,
) -> Vec<JsonValue> {
    let Some(earlier) = earlier_prices.as_object() else {
        return Vec::new();
    };
    let threshold = if min_move_pct.is_finite() && min_move_pct > 0.0 {
        min_move_pct
    } else {
        DEFAULT_MIN_PRICE_MOVE_PCT
    };
    let mut changes: Vec<JsonValue> = current_positions
        .iter()
        .filter_map(|position| {
            let symbol = position.get("symbol").and_then(JsonValue::as_str)?;
            let current = position
                .get("current_price_local")
                .and_then(JsonValue::as_f64)?;
            let earlier_price = earlier.get(symbol).and_then(JsonValue::as_f64)?;
            let currency = position
                .get("currency")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            price_delta(symbol, earlier_price, current, currency)
        })
        .filter(|change| {
            change
                .get("move_pct")
                .and_then(JsonValue::as_f64)
                .is_some_and(|pct| pct.abs() >= threshold)
        })
        .collect();
    changes.sort_by(|left, right| {
        let magnitude = |value: &JsonValue| {
            value
                .get("move_pct")
                .and_then(JsonValue::as_f64)
                .map(f64::abs)
                .unwrap_or_default()
        };
        magnitude(right)
            .partial_cmp(&magnitude(left))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    changes
}

/// Whether the signal blocks are the same runs the opening report saw.
///
/// Recorded rather than assumed: Markov and the daily indicators are computed
/// once a day, so a shadow three hours later normally reads identical inputs
/// and no signal delta is possible. Saying so in the payload is what stops the
/// absence of signal evidence from looking like a broken comparison, and if a
/// refresh ever does land between the two reports this flips and says so.
pub(crate) fn signal_inputs_unchanged(earlier_runs: &JsonValue, current: &JsonValue) -> JsonValue {
    let run_id = |source: &JsonValue, block: &str| {
        source
            .get(block)
            .and_then(|value| value.get("latest_run"))
            .and_then(|run| run.get("id"))
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let markov_earlier = earlier_runs
        .get("markov_run_id")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    let indicators_earlier = earlier_runs
        .get("daily_indicator_run_id")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    let markov_now = run_id(current, "markov_method");
    let indicators_now = run_id(current, "daily_indicators");
    let comparable = !markov_earlier.is_empty() || !indicators_earlier.is_empty();
    json!({
        "comparable": comparable,
        "markov_unchanged": comparable && markov_earlier == markov_now,
        "daily_indicators_unchanged": comparable && indicators_earlier == indicators_now,
        "markov_run_id": markov_now,
        "daily_indicator_run_id": indicators_now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(symbol: &str, price: f64) -> JsonValue {
        json!({"symbol": symbol, "current_price_local": price, "currency": "USD"})
    }

    /// The threshold is the whole point: a session where nothing moved must
    /// report nothing, or the verdict goes back to being always-yes.
    #[test]
    fn only_moves_past_the_threshold_are_returned() {
        let earlier = json!({"CEG:xnas": 290.25, "V:xnys": 373.83, "AJG:xnys": 265.15});
        let positions = vec![
            position("CEG:xnas", 295.87), // +1.94%
            position("V:xnys", 375.83),   // +0.54%
            position("AJG:xnys", 264.71), // -0.17%
        ];

        let changes = measured_price_changes(&earlier, &positions, 0.015);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0]["symbol"], "CEG:xnas");
        assert!(
            (changes[0]["move_pct"].as_f64().expect("pct") - 0.019363).abs() < 1e-5,
            "{changes:?}"
        );
    }

    /// "The trade I just made now exists" was the single largest category of
    /// claimed material change. A position the opening report itself opened has
    /// no earlier price, so it is not a change and must not become one.
    #[test]
    fn a_position_opened_by_the_opening_report_is_not_a_change() {
        let earlier = json!({ "CEG:xnas": 290.25 });
        let positions = vec![position("CEG:xnas", 290.30), position("SPOT:xnys", 548.17)];

        let changes = measured_price_changes(&earlier, &positions, 0.015);
        assert!(
            changes.is_empty(),
            "a symbol with no opening price is not comparable: {changes:?}"
        );
    }

    /// Sorted by magnitude so the payload leads with the move that matters,
    /// and direction is preserved because a 2% fall is as material as a rise.
    #[test]
    fn changes_lead_with_the_largest_move_in_either_direction() {
        let earlier = json!({"UP:xnys": 100.0, "DOWN:xnys": 100.0});
        let positions = vec![position("UP:xnys", 102.0), position("DOWN:xnys", 95.0)];

        let changes = measured_price_changes(&earlier, &positions, 0.015);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["symbol"], "DOWN:xnys", "-5% leads +2%");
        assert!(changes[0]["move_pct"].as_f64().expect("pct") < 0.0);
    }

    /// A zero or absent opening price cannot produce a percentage. Dividing by
    /// it would yield infinity and a spurious material change.
    #[test]
    fn an_unusable_opening_price_is_skipped_rather_than_divided_by() {
        let earlier = json!({"ZERO:xnys": 0.0, "TEXT:xnys": "n/a"});
        let positions = vec![position("ZERO:xnys", 50.0), position("TEXT:xnys", 50.0)];

        assert!(measured_price_changes(&earlier, &positions, 0.015).is_empty());
    }

    /// Both signal blocks are daily, so a shadow three hours after the opening
    /// normally reads the identical run and no signal delta is possible. The
    /// payload has to say that, or the absence of signal evidence looks like a
    /// broken comparison rather than an expected one.
    #[test]
    fn identical_daily_runs_are_reported_as_unchanged_inputs() {
        let earlier = json!({
            "markov_run_id": "markov-1788532395914769",
            "daily_indicator_run_id": "indicators-1788472274888374"
        });
        let current = json!({
            "markov_method": {"latest_run": {"id": "markov-1788532395914769"}},
            "daily_indicators": {"latest_run": {"id": "indicators-1788472274888374"}}
        });

        let verdict = signal_inputs_unchanged(&earlier, &current);
        assert_eq!(verdict["comparable"], true);
        assert_eq!(verdict["markov_unchanged"], true);
        assert_eq!(verdict["daily_indicators_unchanged"], true);

        let refreshed = json!({
            "markov_method": {"latest_run": {"id": "markov-newer"}},
            "daily_indicators": {"latest_run": {"id": "indicators-1788472274888374"}}
        });
        let verdict = signal_inputs_unchanged(&earlier, &refreshed);
        assert_eq!(
            verdict["markov_unchanged"], false,
            "a refresh between the two reports must flip this"
        );
    }
}
