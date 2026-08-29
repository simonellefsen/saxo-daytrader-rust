//! Read-only market dashboard projections.
//!
//! These helpers only narrow persisted watchlist evidence for SSR. They cannot
//! refresh quotes, change the analysis universe, create a report, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::models::{MarketWatchlistUniversePayload, MarketWatchlistsPayload};

/// Decodes the stable Watchlists envelope used by the Watchlists tab. The row
/// shell is allowlisted while nested decision/support evidence stays staged.
pub(crate) fn dashboard_watchlists_from_json(
    watchlists: JsonValue,
) -> serde_json::Result<MarketWatchlistsPayload> {
    serde_json::from_value(watchlists)
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
}
