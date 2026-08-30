//! Read-only Quiver dashboard projections.
//!
//! Pagination and typed projections are intentionally separate from Quiver API
//! collection, persisted signals, scheduler execution, and any trading advice
//! derived from them.

use serde_json::Value as JsonValue;

use crate::{debug_redaction::compact_debug_text, models::DashboardQuiverSignalPayload};

pub(crate) const QUIVER_SIGNALS_PAGE_SIZE: i64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuiverSignalPage {
    pub(crate) page: i64,
    pub(crate) offset: i64,
}

pub(crate) fn quiver_signal_page(requested_page: i64, total_signals: i64) -> QuiverSignalPage {
    let total_pages =
        ((total_signals.max(0) + QUIVER_SIGNALS_PAGE_SIZE - 1) / QUIVER_SIGNALS_PAGE_SIZE).max(1);
    let page = requested_page.max(1).min(total_pages);
    QuiverSignalPage {
        page,
        offset: (page - 1) * QUIVER_SIGNALS_PAGE_SIZE,
    }
}

/// Decodes the rendered Quiver signal-table fields while source-status,
/// top-event, and provider diagnostics stay on their dedicated read-only
/// paths. This projection cannot refresh Quiver data or influence a Decision
/// Report, manager gate, queue, precheck, or Saxo order.
pub(crate) fn dashboard_quiver_signals_from_json(
    signals: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardQuiverSignalPayload>> {
    signals
        .into_iter()
        .map(|signal| {
            let error_text = optional_string(&signal, "error_text")?
                .map(|value| compact_debug_text(&value, 220))
                .unwrap_or_default();
            Ok(DashboardQuiverSignalPayload {
                symbol: required_string(&signal, "symbol")?,
                ticker: required_string(&signal, "ticker")?,
                instrument_name: required_string(&signal, "instrument_name")?,
                signal: required_f64(&signal, "signal")?,
                direction: required_string(&signal, "direction")?,
                confidence: required_f64(&signal, "confidence")?,
                event_count: required_i64(&signal, "event_count")?,
                congress_purchase_count: required_i64(&signal, "congress_purchase_count")?,
                congress_sale_count: required_i64(&signal, "congress_sale_count")?,
                net_congress_amount: required_f64(&signal, "net_congress_amount")?,
                latest_event_date: optional_string(&signal, "latest_event_date")?
                    .unwrap_or_else(|| "n/a".to_string()),
                status: required_string(&signal, "status")?,
                error_text,
            })
        })
        .collect()
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn optional_string(row: &JsonValue, key: &str) -> serde_json::Result<Option<String>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_i64(row: &JsonValue, key: &str) -> serde_json::Result<i64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_f64(row: &JsonValue, key: &str) -> serde_json::Result<f64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clamps_requested_page_and_uses_the_bounded_signal_offset() {
        assert_eq!(
            quiver_signal_page(2, 81),
            QuiverSignalPage {
                page: 2,
                offset: QUIVER_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            quiver_signal_page(9, 41),
            QuiverSignalPage {
                page: 2,
                offset: QUIVER_SIGNALS_PAGE_SIZE,
            }
        );
        assert_eq!(
            quiver_signal_page(0, 0),
            QuiverSignalPage { page: 1, offset: 0 }
        );
    }

    #[test]
    fn dashboard_signals_keep_source_documents_outside_ssr() {
        let signals = dashboard_quiver_signals_from_json(vec![json!({
            "id": "quiver-91",
            "run_id": "run-91",
            "created_at": "2026-08-26T08:30:00Z",
            "run_date": "2026-08-26",
            "status": "error",
            "symbol": "EXAMPLE:xnas",
            "ticker": "EXAMPLE",
            "instrument_name": "Example Corp",
            "signal": 0.4,
            "direction": "bullish",
            "confidence": 0.8,
            "event_count": 3,
            "congress_purchase_count": 2,
            "congress_sale_count": 1,
            "net_congress_amount": 120000.0,
            "latest_event_date": "2026-08-25",
            "error_text": "Quiver response included sk-must-not-reach-the-dashboard-1234567890",
            "source_status_json": {"api_key": "must-not-reach-the-dashboard"},
            "top_events_json": [{"token": "must-not-reach-the-dashboard"}]
        })])
        .expect("stable Quiver signal display row decodes");

        assert_eq!(signals[0].symbol, "EXAMPLE:xnas");
        assert_eq!(signals[0].event_count, 3);
        assert!(signals[0].error_text.contains("[redacted]"));
        assert!(
            !serde_json::to_string(&signals)
                .expect("typed Quiver signals serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(
            dashboard_quiver_signals_from_json(vec![json!({
                "symbol": "EXAMPLE:xnas"
            })])
            .is_err()
        );
    }
}
