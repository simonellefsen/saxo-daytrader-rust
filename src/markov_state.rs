//! Read-only Markov dashboard projections.
//!
//! Pagination and typed projections here are deliberately independent of the
//! Markov regime model, stored signals, scheduler, and trading gates. They
//! only bound and decode what the dashboard asks the persisted-signal reader
//! to display.

use serde_json::Value as JsonValue;

use crate::{debug_redaction::compact_debug_text, models::DashboardMarkovSignalPayload};

pub(crate) const MARKOV_SIGNALS_PAGE_SIZE: i64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MarkovSignalPage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn markov_signal_page(requested_page: i64, total_signals: i64) -> MarkovSignalPage {
    let total_pages =
        ((total_signals.max(0) + MARKOV_SIGNALS_PAGE_SIZE - 1) / MARKOV_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    MarkovSignalPage {
        page,
        offset: (page - 1) * MARKOV_SIGNALS_PAGE_SIZE,
    }
}

/// Decodes the rendered Markov signal-table fields while retained model
/// artifacts and raw/provider diagnostics stay on their dedicated paths.
/// This read-only projection cannot refresh a run or influence a manager gate,
/// Decision Report, queue, precheck, or Saxo order.
pub(crate) fn dashboard_markov_signals_from_json(
    signals: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardMarkovSignalPayload>> {
    signals
        .into_iter()
        .map(|signal| {
            let stationary = embedded_json(&signal, "stationary_json").unwrap_or(JsonValue::Null);
            let optional_f64_or_zero =
                |key: &str| optional_f64(&signal, key).map(|value| value.unwrap_or(0.0));
            let stationary_probability = |key: &str| {
                stationary
                    .get(key)
                    .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0)
            };
            let error_text = optional_string(&signal, "error_text")?
                .map(|value| compact_debug_text(&value, 220))
                .unwrap_or_default();
            Ok(DashboardMarkovSignalPayload {
                symbol: required_string(&signal, "symbol")?,
                instrument_name: optional_string(&signal, "instrument_name")?.unwrap_or_default(),
                current_state: optional_string(&signal, "current_state")?
                    .unwrap_or_else(|| "n/a".to_string()),
                signed_signal: optional_f64_or_zero("signed_signal")?,
                direction: optional_string(&signal, "direction")?
                    .unwrap_or_else(|| "n/a".to_string()),
                bull_prob: optional_f64_or_zero("bull_prob")?,
                sideways_prob: optional_f64_or_zero("sideways_prob")?,
                bear_prob: optional_f64_or_zero("bear_prob")?,
                stationary_bull_prob: stationary_probability("Bull"),
                stationary_sideways_prob: stationary_probability("Sideways"),
                stationary_bear_prob: stationary_probability("Bear"),
                rolling_return: optional_f64_or_zero("rolling_return")?,
                sample_count: serde_json::from_value(
                    signal
                        .get("sample_count")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                status: required_string(&signal, "status")?,
                error_text,
            })
        })
        .collect()
}

fn embedded_json(row: &JsonValue, key: &str) -> Option<JsonValue> {
    match row.get(key)? {
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_string(row: &JsonValue, key: &str) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_f64(row: &JsonValue, key: &str) -> serde_json::Result<Option<f64>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clamps_requested_page_and_uses_the_bounded_signal_offset() {
        assert_eq!(
            markov_signal_page(2, 81),
            MarkovSignalPage {
                page: 2,
                offset: MARKOV_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            markov_signal_page(9, 41),
            MarkovSignalPage {
                page: 2,
                offset: MARKOV_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            markov_signal_page(0, 0),
            MarkovSignalPage { page: 1, offset: 0 }
        );
    }

    #[test]
    fn dashboard_signals_keep_model_documents_outside_ssr() {
        let signals = dashboard_markov_signals_from_json(vec![json!({
            "id": "markov-91",
            "run_id": "run-91",
            "created_at": "2026-08-26T08:30:00Z",
            "run_date": "2026-08-26",
            "status": "error",
            "symbol": "EXAMPLE:xnas",
            "instrument_name": "Example Corp",
            "current_state": "Bull",
            "sample_count": 240,
            "rolling_return": 0.04,
            "stationary_json": {"Bull": 0.6, "Sideways": 0.3, "Bear": 0.1},
            "bull_prob": 0.7,
            "sideways_prob": 0.2,
            "bear_prob": 0.1,
            "signed_signal": 0.6,
            "direction": "long",
            "error_text": "Saxo response included sk-must-not-reach-the-dashboard-1234567890",
            "transition_matrix_json": {"api_key": "must-not-reach-the-dashboard"},
            "forecasts_json": {"token": "must-not-reach-the-dashboard"},
            "raw_payload_json": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable Markov signal display row decodes");

        assert_eq!(signals[0].symbol, "EXAMPLE:xnas");
        assert_eq!(signals[0].stationary_bull_prob, 0.6);
        assert!(signals[0].error_text.contains("[redacted]"));
        assert!(
            !serde_json::to_string(&signals)
                .expect("typed Markov signals serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_markov_signals_from_json(vec![json!({
                "symbol": "EXAMPLE:xnas"
            })])
            .is_err()
        );
    }

    /// A collector row carries nulls for every symbol it could not resolve, so a
    /// null must cost that value rather than the whole signal list.
    #[test]
    fn an_explicit_null_never_blanks_the_markov_signal_list() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "id": "markov-91",
                "run_id": "run-91",
                "created_at": "2026-08-26T08:30:00Z",
                "run_date": "2026-08-26",
                "status": "error",
                "symbol": "EXAMPLE:xnas",
                "instrument_name": "Example Corp",
                "current_state": "Bull",
                "sample_count": 240,
                "rolling_return": 0.04,
                "stationary_json": {"Bull": 0.6, "Sideways": 0.3, "Bear": 0.1},
                "bull_prob": 0.7,
                "sideways_prob": 0.2,
                "bear_prob": 0.1,
                "signed_signal": 0.6,
                "direction": "long",
                "error_text": "Saxo response included sk-must-not-reach-the-dashboard-1234567890",
                "transition_matrix_json": {"api_key": "must-not-reach-the-dashboard"},
                "forecasts_json": {"token": "must-not-reach-the-dashboard"},
                "raw_payload_json": {"api_key": "must-not-reach-the-dashboard"}
            }]),
            |value| {
                dashboard_markov_signals_from_json(
                    value.as_array().cloned().expect("fixture is a list"),
                )
            },
        );
    }
}
