//! Read-only market dashboard projections.
//!
//! These helpers only narrow persisted watchlist evidence for SSR. They cannot
//! refresh quotes, change the analysis universe, create a report, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{MarketWatchlistUniversePayload, MarketWatchlistsPayload};
use crate::read_model;

/// Decodes the stable Watchlists envelope used by the Watchlists tab. The row
/// shell is allowlisted while nested decision/support evidence stays staged.
pub(crate) fn dashboard_watchlists_from_json(
    watchlists: JsonValue,
) -> serde_json::Result<MarketWatchlistsPayload> {
    read_model::decode("dashboard_watchlists", watchlists)
}

/// Supplies the explicit state for views that do not load Watchlists data.
pub(crate) fn dashboard_watchlists_not_loaded() -> MarketWatchlistsPayload {
    MarketWatchlistsPayload {
        generated_at: String::new(),
        cache_ttl_seconds: 300,
        universe: MarketWatchlistUniversePayload::default(),
        categories: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn watchlists_require_the_stable_outer_contract() {
        let watchlists = dashboard_watchlists_from_json(json!({
            "generated_at": "2026-08-23T12:00:00Z",
            "cache_ttl_seconds": 300,
            "universe": {"source": "configured_analysis_universe"},
            "categories": [{"key": "nordic", "items": [{"symbol": "NOVO-B:xcse", "raw_provider_document": {"must": "stay internal"}}]}]
        }))
        .expect("watchlists fixture has the dashboard contract");

        assert_eq!(watchlists.cache_ttl_seconds, 300);
        assert_eq!(watchlists.categories[0].key, "nordic");
        assert!(
            !serde_json::to_string(&watchlists)
                .expect("typed watchlists serialize")
                .contains("raw_provider_document")
        );
        assert!(dashboard_watchlists_not_loaded().categories.is_empty());
        assert!(dashboard_watchlists_from_json(json!({"categories": []})).is_err());
    }

    /// The 2026-08-31 outage shape: the builder emits `"currency": null` for a
    /// symbol with no technical data, and one such symbol blanked the tab.
    #[test]
    fn an_explicit_null_never_blanks_the_watchlists_tab() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!({
                "generated_at": "2026-08-31T08:40:58Z",
                "cache_ttl_seconds": 300,
                "universe": {
                    "source": "configured_analysis_universe",
                    "configured_symbol_count": 216,
                    "category_count": 1,
                    "monitored_symbol_count": 216
                },
                "categories": [{
                    "key": "all",
                    "label": "All monitored",
                    "target_limit": 0,
                    "total": 2,
                    "items": [
                        {
                            "symbol": "AAPL:xnas",
                            "instrument_name": "Apple Inc.",
                            "exchange": "xnas",
                            "region": "US",
                            "currency": "USD",
                            "current_price_local": 231.4,
                            "change_pct": 0.4,
                            "quote_status": "live",
                            "decision": {
                                "sentiment": "neutral",
                                "action": "hold",
                                "created_at": "2026-08-31T08:15:00Z",
                                "rationale": "no change",
                                "trend_bias": "up"
                            },
                            "support_risk": {
                                "run_date": "2026-08-30",
                                "status": "ok",
                                "nearest_support": 220.0,
                                "break_risk_label": "low"
                            }
                        },
                        {"symbol": "ABB:xome", "currency": null, "quote_status": "decision_snapshot"}
                    ]
                }]
            }),
            dashboard_watchlists_from_json,
        );
    }
}
