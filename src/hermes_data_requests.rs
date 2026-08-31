//! Server-side handling of Hermes data refresh requests.
//!
//! Hermes reviews each decision report in conservative mode, where a blanket
//! `review` recommendation zeroes every candidate. Before this existed, the
//! only thing Hermes could do about a stale or missing input was block on it,
//! which turned an evidence gap into a trading halt.
//!
//! A data request lets Hermes name what it needs instead. The naming is all it
//! gets: the source must match a fixed allowlist of read-only recomputes, the
//! server decides how to satisfy it, and symbols only ever filter the existing
//! configured universe. Hermes cannot reach a new instrument, a broker
//! endpoint, or a tuning value through this path, and satisfying a request
//! never approves anything -- it only lets the next advisory round see fresher
//! evidence.

use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::state::AppState;

/// Refreshes Hermes may ask for, by name. Every entry is a read-only recompute
/// of data the system already collects on a schedule.
pub(crate) const HERMES_REFRESHABLE_SOURCES: &[&str] =
    &["markov_signals", "technical_analysis", "fx_rates"];

/// Bounds on one advisory round's requests, so a malformed or runaway response
/// cannot turn into an unbounded amount of upstream work.
pub(crate) const HERMES_MAX_DATA_REQUESTS: usize = 6;
pub(crate) const HERMES_MAX_REQUEST_SYMBOLS: usize = 25;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HermesDataRequest {
    pub(crate) source: String,
    pub(crate) symbols: Vec<String>,
    pub(crate) reason: String,
}

/// Map the names Hermes is likely to use onto the allowlist.
///
/// Hermes writes prose, so it reaches for `markov`, `technical`, or `fx` as
/// often as the canonical key. Accepting the obvious synonyms keeps a usable
/// request from being discarded as unsupported; anything genuinely unknown
/// still falls through and is rejected.
fn canonical_source(raw: &str) -> Option<&'static str> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "markov_signals" | "markov" | "markov_signal" | "markov_method" => Some("markov_signals"),
        "technical_analysis" | "technical" | "technicals" | "daily_indicators" | "indicators" => {
            Some("technical_analysis")
        }
        "fx_rates" | "fx" | "fx_rate" | "exchange_rates" | "currency_rates" => Some("fx_rates"),
        _ => None,
    }
}

/// Validate what Hermes asked for into an honorable set plus an audit trail of
/// what was refused and why.
pub(crate) fn normalize_hermes_data_requests(
    raw: Option<&JsonValue>,
) -> (Vec<HermesDataRequest>, Vec<JsonValue>) {
    let Some(entries) = raw.and_then(JsonValue::as_array) else {
        return (Vec::new(), Vec::new());
    };
    let mut honored: Vec<HermesDataRequest> = Vec::new();
    let mut rejected = Vec::new();
    for entry in entries {
        let requested_source = entry
            .get("source")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(source) = canonical_source(&requested_source) else {
            rejected.push(json!({
                "source": requested_source,
                "reason": "unsupported_source",
                "allowed": HERMES_REFRESHABLE_SOURCES,
            }));
            continue;
        };
        if honored.len() >= HERMES_MAX_DATA_REQUESTS {
            rejected.push(json!({"source": source, "reason": "request_limit_reached"}));
            continue;
        }
        let symbols = entry
            .get("symbols")
            .and_then(JsonValue::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|symbol| !symbol.is_empty())
                    .take(HERMES_MAX_REQUEST_SYMBOLS)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let reason = entry
            .get("reason")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        // Fold a repeated source into the first request for it rather than
        // recomputing the same source twice in one round.
        if let Some(existing) = honored.iter_mut().find(|request| request.source == source) {
            for symbol in symbols {
                if !existing.symbols.contains(&symbol) {
                    existing.symbols.push(symbol);
                }
            }
            existing.symbols.truncate(HERMES_MAX_REQUEST_SYMBOLS);
            continue;
        }
        honored.push(HermesDataRequest {
            source: source.to_string(),
            symbols,
            reason,
        });
    }
    (honored, rejected)
}

/// Run the honored refreshes and report per-source outcomes.
///
/// A failed refresh is recorded, never propagated: the advisory round that
/// follows should still happen, just without that source having improved.
pub(crate) async fn execute_hermes_data_requests(
    state: &AppState,
    requests: &[HermesDataRequest],
) -> Vec<JsonValue> {
    let mut outcomes = Vec::new();
    for request in requests {
        info!(
            source = %request.source,
            symbol_count = request.symbols.len(),
            "serving Hermes data refresh request"
        );
        let result = match request.source.as_str() {
            "markov_signals" => {
                if request.symbols.is_empty() {
                    Err(anyhow::anyhow!(
                        "markov_signals refresh needs at least one symbol"
                    ))
                } else {
                    crate::markov_method::refresh_markov_signals_for_symbols(
                        state,
                        &request.symbols,
                    )
                    .await
                }
            }
            "technical_analysis" => {
                crate::daily_indicators::run_daily_indicators_cycle(state).await
            }
            "fx_rates" => crate::fx::run_fx_rate_refresh_cycle(state).await,
            other => Err(anyhow::anyhow!("unsupported refresh source: {other}")),
        };
        outcomes.push(match result {
            Ok(summary) => json!({
                "source": request.source,
                "status": "served",
                "symbols": request.symbols,
                "reason": request.reason,
                "result_status": summary.get("status").cloned().unwrap_or(JsonValue::Null),
            }),
            Err(err) => {
                warn!(
                    source = %request.source,
                    "Hermes data refresh request failed: {err:#}"
                );
                json!({
                    "source": request.source,
                    "status": "failed",
                    "symbols": request.symbols,
                    "reason": request.reason,
                    // Message only; upstream payloads and credentials stay out.
                    "error": format!("{err:#}"),
                })
            }
        });
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prose_names_hermes_actually_uses_map_onto_the_allowlist() {
        for (raw, expected) in [
            ("markov", "markov_signals"),
            ("Markov Signals", "markov_signals"),
            ("technical", "technical_analysis"),
            ("daily-indicators", "technical_analysis"),
            ("FX", "fx_rates"),
        ] {
            assert_eq!(canonical_source(raw), Some(expected), "for {raw}");
        }
    }

    #[test]
    fn an_unknown_source_is_refused_and_recorded_rather_than_run() {
        let raw = json!([
            {"source": "place_order", "reason": "would be nice"},
            {"source": "saxo_session", "reason": "need a token"},
        ]);
        let (honored, rejected) = normalize_hermes_data_requests(Some(&raw));

        assert!(honored.is_empty(), "nothing outside the allowlist may run");
        assert_eq!(rejected.len(), 2);
        assert_eq!(rejected[0]["reason"], "unsupported_source");
    }

    #[test]
    fn repeated_sources_merge_instead_of_recomputing_twice() {
        let raw = json!([
            {"source": "markov", "symbols": ["BMW:xetr"], "reason": "stale"},
            {"source": "markov_signals", "symbols": ["ALV:xetr", "BMW:xetr"]},
        ]);
        let (honored, _) = normalize_hermes_data_requests(Some(&raw));

        assert_eq!(honored.len(), 1);
        assert_eq!(honored[0].symbols, vec!["BMW:xetr", "ALV:xetr"]);
        assert_eq!(honored[0].reason, "stale", "first reason is kept");
    }

    #[test]
    fn requests_and_symbols_are_bounded() {
        let symbols = (0..60)
            .map(|index| json!(format!("SYM{index}:xetr")))
            .collect::<Vec<_>>();
        let raw = json!([{"source": "markov", "symbols": symbols}]);
        let (honored, _) = normalize_hermes_data_requests(Some(&raw));
        assert_eq!(honored[0].symbols.len(), HERMES_MAX_REQUEST_SYMBOLS);

        let many = (0..12)
            .map(|index| json!({"source": format!("unknown_{index}")}))
            .collect::<Vec<_>>();
        let (honored, rejected) = normalize_hermes_data_requests(Some(&json!(many)));
        assert!(honored.is_empty());
        assert_eq!(rejected.len(), 12, "every refusal is auditable");
    }

    #[test]
    fn absent_or_malformed_requests_are_simply_empty() {
        assert_eq!(normalize_hermes_data_requests(None).0.len(), 0);
        assert_eq!(
            normalize_hermes_data_requests(Some(&json!("not-an-array")))
                .0
                .len(),
            0
        );
    }
}
