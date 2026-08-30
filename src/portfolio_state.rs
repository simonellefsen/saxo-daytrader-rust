//! Read-only portfolio dashboard and API projections.
//!
//! These decoders narrow persisted position and ledger data to the stable
//! fields rendered by the dashboard and public API. They cannot refresh quotes,
//! alter accounting, change a Decision Report, queue a trade, or reach Saxo.

use serde_json::Value as JsonValue;

use crate::{
    debug_redaction::compact_debug_text,
    models::{DashboardPositionDecisionPayload, DashboardPositionPayload, PortfolioTradePayload},
};

/// Decodes stable overview position fields. The nested advisory decision stays
/// limited to the existing badge and chart fields.
pub(crate) fn dashboard_positions_from_json(
    positions: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardPositionPayload>> {
    positions
        .into_iter()
        .map(|position| {
            let number = |key| optional_f64(&position, key).map(|value| value.unwrap_or(0.0));
            Ok(DashboardPositionPayload {
                instrument_name: optional_string(&position, "instrument_name")?.unwrap_or_default(),
                symbol: required_string(&position, "symbol")?,
                isin: optional_string(&position, "isin")?.unwrap_or_default(),
                quantity: number("quantity")?,
                currency: optional_string(&position, "currency")?.unwrap_or_default(),
                paid_price_local: number("paid_price_local")?,
                open_price_local: number("open_price_local")?,
                cost_basis_local: number("cost_basis_local")?,
                current_price_local: number("current_price_local")?,
                cost_basis_dkk: number("cost_basis_dkk")?,
                market_value_dkk: number("market_value_dkk")?,
                unrealised_pnl_dkk: number("unrealised_pnl_dkk")?,
                daily_pnl_dkk: number("daily_pnl_dkk")?,
                daily_change_pct: number("daily_change_pct")?,
                total_return_pct: number("total_return_pct")?,
                allocation_pct: number("allocation_pct")?,
                asset_class: optional_string(&position, "asset_class")?
                    .unwrap_or_else(|| "Equity".to_string()),
                market_status: optional_string(&position, "market_status")?.unwrap_or_default(),
                change_pct: number("change_pct")?,
                latest_quote_updated_at: optional_string(&position, "latest_quote_updated_at")?
                    .unwrap_or_default(),
                decision: position_decision_from_json(
                    position.get("decision").unwrap_or(&JsonValue::Null),
                ),
            })
        })
        .collect()
}

fn position_decision_from_json(value: &JsonValue) -> Option<DashboardPositionDecisionPayload> {
    (!value.is_null()).then(|| DashboardPositionDecisionPayload {
        sentiment: optional_string(value, "sentiment")
            .ok()
            .flatten()
            .unwrap_or_default(),
        action: optional_string(value, "action")
            .ok()
            .flatten()
            .unwrap_or_default(),
        created_at: optional_string(value, "created_at")
            .ok()
            .flatten()
            .unwrap_or_default(),
        rationale: optional_string(value, "rationale")
            .ok()
            .flatten()
            .map(|text| compact_debug_text(&text, 360))
            .unwrap_or_default(),
        target_rationale: optional_string(value, "target_rationale")
            .ok()
            .flatten()
            .map(|text| compact_debug_text(&text, 360))
            .unwrap_or_default(),
        trend_bias: value
            .get("source")
            .and_then(|source| source.get("technical"))
            .and_then(|technical| technical.get("trend_bias"))
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Decodes the stable portfolio-ledger projection. Free-form notes and
/// retained portfolio, decision, and broker/provider documents do not cross
/// this read-only boundary.
pub(crate) fn portfolio_trades_from_json(
    trades: Vec<JsonValue>,
) -> serde_json::Result<Vec<PortfolioTradePayload>> {
    trades
        .into_iter()
        .map(|trade| {
            let number = |key| optional_f64(&trade, key).map(|value| value.unwrap_or(0.0));
            Ok(PortfolioTradePayload {
                id: required_i64(&trade, "id")?,
                created_at: required_string(&trade, "created_at")?,
                symbol: optional_string(&trade, "symbol")?.unwrap_or_default(),
                isin: optional_string(&trade, "isin")?.unwrap_or_default(),
                instrument_name: optional_string(&trade, "instrument_name")?.unwrap_or_default(),
                side: required_string(&trade, "side")?,
                quantity: number("quantity")?,
                price_local: number("price_local")?,
                currency: optional_string(&trade, "currency")?.unwrap_or_default(),
                gross_amount_dkk: number("gross_amount_dkk")?,
                commission_dkk: number("commission_dkk")?,
                tax_dkk: number("tax_dkk")?,
                realised_gain_dkk: number("realised_gain_dkk")?,
                net_amount_dkk: number("net_amount_dkk")?,
                mode: optional_string(&trade, "mode")?.unwrap_or_default(),
                status: optional_string(&trade, "status")?.unwrap_or_default(),
                batch_id: optional_string(&trade, "batch_id")?.unwrap_or_default(),
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

fn optional_f64(row: &JsonValue, key: &str) -> serde_json::Result<Option<f64>> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dashboard_positions_keep_the_outer_portfolio_contract_typed() {
        let positions = dashboard_positions_from_json(vec![json!({
            "symbol": "EXAMPLE:xnas",
            "instrument_name": "Example Corp",
            "isin": "US0000000001",
            "quantity": 4.0,
            "currency": "USD",
            "paid_price_local": 101.0,
            "open_price_local": 101.0,
            "cost_basis_local": 101.0,
            "current_price_local": 105.0,
            "cost_basis_dkk": 2800.0,
            "market_value_dkk": 2900.0,
            "unrealised_pnl_dkk": 100.0,
            "daily_pnl_dkk": 25.0,
            "daily_change_pct": 0.01,
            "total_return_pct": 0.0357,
            "allocation_pct": 0.12,
            "asset_class": "Stock",
            "market_status": "Saxo broker snapshot",
            "change_pct": 0.01,
            "latest_quote_updated_at": "2026-08-26T12:00:00Z",
            "decision": {"source": {"technical": {"trend_bias": "bullish"}}},
            "broker_payload": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable portfolio position decodes");

        assert_eq!(positions[0].symbol, "EXAMPLE:xnas");
        assert_eq!(positions[0].market_value_dkk, 2900.0);
        assert_eq!(
            positions[0]
                .decision
                .as_ref()
                .map(|decision| decision.trend_bias.as_str()),
            Some("bullish")
        );
        assert!(
            !serde_json::to_string(&positions)
                .expect("typed portfolio positions serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(dashboard_positions_from_json(vec![json!({"quantity": 4.0})]).is_err());
    }

    #[test]
    fn trades_allowlist_excludes_retained_context_documents() {
        let trades = portfolio_trades_from_json(vec![json!({
            "id": 42,
            "created_at": "2026-08-26T12:00:00Z",
            "symbol": "EXAMPLE:xnas",
            "isin": "US0000000001",
            "instrument_name": "Example Corp",
            "side": "SELL",
            "quantity": 4.0,
            "price_local": 105.0,
            "currency": "USD",
            "gross_amount_dkk": 2900.0,
            "commission_dkk": 10.0,
            "tax_dkk": 0.0,
            "realised_gain_dkk": 100.0,
            "net_amount_dkk": 2890.0,
            "mode": "simulation",
            "status": "executed",
            "batch_id": "batch-42",
            "notes": "must-not-reach-the-trades-api",
            "decision_context_json": {"api_key": "must-not-reach-the-trades-api"},
            "portfolio_after_json": {"account": "must-not-reach-the-trades-api"}
        })])
        .expect("stable trade ledger row decodes");

        assert_eq!(trades[0].symbol, "EXAMPLE:xnas");
        assert_eq!(trades[0].realised_gain_dkk, 100.0);
        assert!(
            !serde_json::to_string(&trades)
                .expect("typed trade ledger rows serialize")
                .contains("must-not-reach-the-trades-api")
        );
        assert!(
            portfolio_trades_from_json(vec![json!({
                "id": 42,
                "created_at": "2026-08-26T12:00:00Z"
            })])
            .is_err()
        );
    }
}
