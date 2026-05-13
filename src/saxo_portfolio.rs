use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use reqwest::header;
use serde_json::{Value as JsonValue, json};
use tracing::info;

use crate::{
    auth,
    config::yaml_string,
    db::{sql_escape, value_f64},
    state::AppState,
};

#[derive(Clone, Debug, PartialEq)]
struct BrokerPositionRow {
    symbol: String,
    instrument_name: String,
    isin: Option<String>,
    uic: Option<i64>,
    asset_type: Option<String>,
    quantity: f64,
    currency: Option<String>,
    open_price_local: f64,
    open_price_including_costs_local: f64,
    execution_time_open: Option<String>,
    value_date: Option<String>,
    market_state: Option<String>,
    can_be_closed: bool,
    raw_payload: JsonValue,
}

#[derive(Clone, Debug, PartialEq)]
struct BrokerExposureRow {
    symbol: String,
    uic: Option<i64>,
    asset_type: Option<String>,
    quantity: f64,
    average_open_price: f64,
    profit_loss_on_trade: f64,
    instrument_price_day_percent_change: f64,
    currency: Option<String>,
    calculation_reliability: Option<String>,
    can_be_closed: bool,
    raw_payload: JsonValue,
}

pub async fn refresh_broker_snapshots(state: &AppState) -> Result<JsonValue> {
    // Refreshing broker snapshots is read-only against Saxo, but it changes the local
    // read model. Keeping this in Rust means the dashboard no longer depends on the
    // old Python scheduler to discover fills after a rollout.
    state
        .refresh_saxo_session()
        .await
        .context("refreshing Saxo session before broker snapshot refresh")?;
    let session = auth::ensure_session_json(&state.config, &state.config_path)
        .await
        .context("loading Saxo session for broker snapshot refresh")?;

    let positions_payload = saxo_get_json(
        state,
        &session,
        "/port/v1/positions/me",
        &[(
            "FieldGroups",
            "DisplayAndFormat,PositionBase,PositionView".to_string(),
        )],
    )
    .await
    .context("fetching Saxo positions snapshot")?;
    let exposures_payload = saxo_get_json(state, &session, "/port/v1/exposure/instruments/me", &[])
        .await
        .context("fetching Saxo instrument exposures")?;
    let balance_payload = saxo_get_json(state, &session, "/port/v1/balances/me", &[])
        .await
        .context("fetching Saxo balance snapshot")?;
    let accounts_payload = saxo_get_json(state, &session, "/port/v1/accounts/me", &[])
        .await
        .context("fetching Saxo account snapshot")?;

    let updated_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let positions = parse_broker_positions(&positions_payload);
    let exposures = parse_broker_exposures(&exposures_payload);
    save_broker_positions(state, &updated_at, &positions).await?;
    save_broker_exposures(state, &updated_at, &exposures).await?;
    save_broker_balance(state, &updated_at, &balance_payload).await?;
    save_broker_account(state, &updated_at, &session, &accounts_payload).await?;

    info!(
        positions = positions.len(),
        exposures = exposures.len(),
        "Saxo broker read model refreshed"
    );
    Ok(json!({
        "status": "ok",
        "updated_at": updated_at,
        "positions": positions.len(),
        "exposures": exposures.len()
    }))
}

fn parse_broker_positions(payload: &JsonValue) -> Vec<BrokerPositionRow> {
    let mut by_symbol: HashMap<String, BrokerPositionRow> = HashMap::new();
    for parsed in payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let display = row.get("DisplayAndFormat").unwrap_or(&JsonValue::Null);
            let base = row.get("PositionBase").unwrap_or(&JsonValue::Null);
            let view = row.get("PositionView").unwrap_or(&JsonValue::Null);
            let symbol = json_text(display, "Symbol")?;
            let open_price_local = value_f64(base, "OpenPrice");
            Some(BrokerPositionRow {
                symbol,
                instrument_name: json_text(display, "Description")
                    .or_else(|| json_text(display, "InstrumentType"))
                    .unwrap_or_else(|| "Saxo position".to_string()),
                isin: json_text(display, "IsinCode"),
                uic: base.get("Uic").and_then(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                }),
                asset_type: json_text(base, "AssetType"),
                quantity: value_f64(base, "Amount"),
                currency: json_text(display, "Currency"),
                open_price_local,
                open_price_including_costs_local: value_f64(base, "OpenPriceIncludingCosts")
                    .max(open_price_local),
                execution_time_open: json_text(base, "ExecutionTimeOpen"),
                value_date: json_text(base, "ValueDate"),
                market_state: json_text(view, "MarketState"),
                can_be_closed: base
                    .get("CanBeClosed")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                raw_payload: row.clone(),
            })
        })
    {
        let Some(existing) = by_symbol.get_mut(&parsed.symbol) else {
            by_symbol.insert(parsed.symbol.clone(), parsed);
            continue;
        };
        let combined_quantity = existing.quantity + parsed.quantity;
        if combined_quantity > 0.0 {
            existing.open_price_local = weighted_average(
                existing.open_price_local,
                existing.quantity,
                parsed.open_price_local,
                parsed.quantity,
                combined_quantity,
            );
            existing.open_price_including_costs_local = weighted_average(
                existing.open_price_including_costs_local,
                existing.quantity,
                parsed.open_price_including_costs_local,
                parsed.quantity,
                combined_quantity,
            );
        }
        existing.quantity = combined_quantity;
        if existing.execution_time_open.is_none()
            || parsed.execution_time_open.as_deref() < existing.execution_time_open.as_deref()
        {
            existing.execution_time_open = parsed.execution_time_open.clone();
        }
        existing.raw_payload = append_raw_payload(&existing.raw_payload, &parsed.raw_payload);
    }
    by_symbol.into_values().collect()
}

fn parse_broker_exposures(payload: &JsonValue) -> Vec<BrokerExposureRow> {
    let rows = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .or_else(|| payload.as_array());
    rows.into_iter()
        .flatten()
        .filter_map(|row| {
            let display = row.get("DisplayAndFormat").unwrap_or(&JsonValue::Null);
            let symbol = json_text(display, "Symbol")?;
            Some(BrokerExposureRow {
                symbol,
                uic: row.get("Uic").and_then(JsonValue::as_i64),
                asset_type: json_text(row, "AssetType"),
                quantity: value_f64(row, "Amount"),
                average_open_price: value_f64(row, "AverageOpenPrice"),
                profit_loss_on_trade: value_f64(row, "ProfitLossOnTrade"),
                instrument_price_day_percent_change: value_f64(
                    row,
                    "InstrumentPriceDayPercentChange",
                ),
                currency: json_text(display, "Currency"),
                calculation_reliability: json_text(row, "CalculationReliability"),
                can_be_closed: row
                    .get("CanBeClosed")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                raw_payload: row.clone(),
            })
        })
        .collect()
}

async fn save_broker_positions(
    state: &AppState,
    updated_at: &str,
    rows: &[BrokerPositionRow],
) -> Result<()> {
    sqlx::query("DELETE FROM broker_position_snapshots")
        .execute(&state.pool)
        .await
        .context("clearing broker position snapshots")?;
    for row in rows {
        let raw_payload = serde_json::to_string(&raw_payload_array(&row.raw_payload))?;
        let sql = format!(
            "INSERT INTO broker_position_snapshots (
                symbol, updated_at, instrument_name, isin, uic, asset_type, quantity, currency,
                open_price_local, open_price_including_costs_local, execution_time_open, value_date,
                market_state, can_be_closed, raw_payload_json
            ) VALUES (
                '{}', '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, '{}'
            )",
            sql_escape(&row.symbol),
            sql_escape(updated_at),
            sql_escape(&row.instrument_name),
            sql_opt_text(row.isin.as_deref()),
            sql_opt_i64(row.uic),
            sql_opt_text(row.asset_type.as_deref()),
            row.quantity,
            sql_opt_text(row.currency.as_deref()),
            row.open_price_local,
            row.open_price_including_costs_local,
            sql_opt_text(row.execution_time_open.as_deref()),
            sql_opt_text(row.value_date.as_deref()),
            sql_opt_text(row.market_state.as_deref()),
            i32::from(row.can_be_closed),
            sql_escape(&raw_payload)
        );
        sqlx::query(&sql)
            .execute(&state.pool)
            .await
            .with_context(|| format!("saving broker position snapshot for {}", row.symbol))?;
    }
    Ok(())
}

async fn save_broker_exposures(
    state: &AppState,
    updated_at: &str,
    rows: &[BrokerExposureRow],
) -> Result<()> {
    sqlx::query("DELETE FROM broker_instrument_exposures")
        .execute(&state.pool)
        .await
        .context("clearing broker instrument exposures")?;
    for row in rows {
        let raw_payload = serde_json::to_string(&row.raw_payload)?;
        let sql = format!(
            "INSERT INTO broker_instrument_exposures (
                symbol, updated_at, uic, asset_type, quantity, average_open_price,
                profit_loss_on_trade, instrument_price_day_percent_change, currency,
                calculation_reliability, can_be_closed, raw_payload_json
            ) VALUES (
                '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {}, {}, '{}'
            )",
            sql_escape(&row.symbol),
            sql_escape(updated_at),
            sql_opt_i64(row.uic),
            sql_opt_text(row.asset_type.as_deref()),
            row.quantity,
            row.average_open_price,
            row.profit_loss_on_trade,
            row.instrument_price_day_percent_change,
            sql_opt_text(row.currency.as_deref()),
            sql_opt_text(row.calculation_reliability.as_deref()),
            i32::from(row.can_be_closed),
            sql_escape(&raw_payload)
        );
        sqlx::query(&sql)
            .execute(&state.pool)
            .await
            .with_context(|| format!("saving broker exposure for {}", row.symbol))?;
    }
    Ok(())
}

async fn save_broker_balance(
    state: &AppState,
    updated_at: &str,
    payload: &JsonValue,
) -> Result<()> {
    let raw_payload = serde_json::to_string(payload)?;
    let cash_available = first_numeric(
        payload,
        &[
            "CashAvailableForTrading",
            "MarginAvailableForTrading",
            "CashBalance",
            "CollateralAvailable",
        ],
    );
    let sql = format!(
        "INSERT INTO broker_balance_snapshots (
            singleton_key, updated_at, currency, cash_available_for_trading,
            margin_available_for_trading, cash_balance, transactions_not_booked,
            settlement_value, total_value, raw_payload_json
        ) VALUES (
            'main', '{}', {}, {}, {}, {}, {}, {}, {}, '{}'
        )
        ON CONFLICT(singleton_key) DO UPDATE SET
            updated_at = excluded.updated_at,
            currency = excluded.currency,
            cash_available_for_trading = excluded.cash_available_for_trading,
            margin_available_for_trading = excluded.margin_available_for_trading,
            cash_balance = excluded.cash_balance,
            transactions_not_booked = excluded.transactions_not_booked,
            settlement_value = excluded.settlement_value,
            total_value = excluded.total_value,
            raw_payload_json = excluded.raw_payload_json",
        sql_escape(updated_at),
        sql_opt_text(json_text(payload, "Currency").as_deref()),
        sql_opt_f64(cash_available),
        sql_opt_f64(optional_f64(payload, "MarginAvailableForTrading")),
        sql_opt_f64(optional_f64(payload, "CashBalance")),
        sql_opt_f64(optional_f64(payload, "TransactionsNotBooked")),
        sql_opt_f64(optional_f64(payload, "SettlementValue")),
        sql_opt_f64(optional_f64(payload, "TotalValue")),
        sql_escape(&raw_payload)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("saving broker balance snapshot")?;
    Ok(())
}

async fn save_broker_account(
    state: &AppState,
    updated_at: &str,
    session: &JsonValue,
    payload: &JsonValue,
) -> Result<()> {
    let account_key = yaml_string(&state.config, &["saxo", "account_key"])
        .or_else(|| json_text(session, "account_key"))
        .unwrap_or_default();
    let accounts = payload
        .get("Data")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let selected = accounts
        .iter()
        .find(|row| json_text(row, "AccountKey").as_deref() == Some(account_key.as_str()))
        .or_else(|| accounts.first());
    let Some(selected) = selected else {
        return Ok(());
    };
    let raw_payload = serde_json::to_string(selected)?;
    let fractional_asset_types = serde_json::to_string(
        selected
            .get("FractionalOrderEnabledAssetTypes")
            .unwrap_or(&json!([])),
    )?;
    let legal_asset_types =
        serde_json::to_string(selected.get("LegalAssetTypes").unwrap_or(&json!([])))?;
    let sql = format!(
        "INSERT INTO broker_account_snapshots (
            singleton_key, updated_at, account_key, account_id, account_currency, is_trial_account,
            fractional_order_enabled, fractional_order_enabled_asset_types_json,
            can_use_cash_positions_as_margin_collateral, use_cash_positions_as_margin_collateral,
            legal_asset_types_json, raw_payload_json
        ) VALUES (
            'main', '{}', {}, {}, {}, {}, {}, '{}', {}, {}, '{}', '{}'
        )
        ON CONFLICT(singleton_key) DO UPDATE SET
            updated_at = excluded.updated_at,
            account_key = excluded.account_key,
            account_id = excluded.account_id,
            account_currency = excluded.account_currency,
            is_trial_account = excluded.is_trial_account,
            fractional_order_enabled = excluded.fractional_order_enabled,
            fractional_order_enabled_asset_types_json = excluded.fractional_order_enabled_asset_types_json,
            can_use_cash_positions_as_margin_collateral = excluded.can_use_cash_positions_as_margin_collateral,
            use_cash_positions_as_margin_collateral = excluded.use_cash_positions_as_margin_collateral,
            legal_asset_types_json = excluded.legal_asset_types_json,
            raw_payload_json = excluded.raw_payload_json",
        sql_escape(updated_at),
        sql_opt_text(json_text(selected, "AccountKey").as_deref()),
        sql_opt_text(json_text(selected, "AccountId").as_deref()),
        sql_opt_text(json_text(selected, "Currency").as_deref()),
        i32::from(selected.get("IsTrialAccount").and_then(JsonValue::as_bool).unwrap_or(false)),
        i32::from(selected.get("FractionalOrderEnabled").and_then(JsonValue::as_bool).unwrap_or(false)),
        sql_escape(&fractional_asset_types),
        i32::from(selected.get("CanUseCashPositionsAsMarginCollateral").and_then(JsonValue::as_bool).unwrap_or(false)),
        i32::from(selected.get("UseCashPositionsAsMarginCollateral").and_then(JsonValue::as_bool).unwrap_or(false)),
        sql_escape(&legal_asset_types),
        sql_escape(&raw_payload)
    );
    sqlx::query(&sql)
        .execute(&state.pool)
        .await
        .context("saving broker account snapshot")?;
    Ok(())
}

async fn saxo_get_json(
    state: &AppState,
    session: &JsonValue,
    path: &str,
    query: &[(&str, String)],
) -> Result<JsonValue> {
    let access_token = json_text(session, "access_token")
        .ok_or_else(|| anyhow!("Saxo access token is missing from session"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!("{}{}", openapi_base_url(state, session)?, path))
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .query(query)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let payload = serde_json::from_str::<JsonValue>(&body).unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let snippet: String = body.chars().take(300).collect();
        bail!(
            "Saxo GET {path} failed: HTTP {}: {}",
            status.as_u16(),
            snippet
        );
    }
    Ok(payload)
}

fn openapi_base_url(state: &AppState, session: &JsonValue) -> Result<&'static str> {
    let environment = json_text(session, "environment")
        .or_else(|| yaml_string(&state.config, &["saxo", "environment"]))
        .unwrap_or_else(|| "sim".to_string())
        .to_lowercase();
    match environment.as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn json_text(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn optional_f64(value: &JsonValue, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
}

fn first_numeric(value: &JsonValue, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| optional_f64(value, key))
}

fn weighted_average(
    left_value: f64,
    left_quantity: f64,
    right_value: f64,
    right_quantity: f64,
    combined_quantity: f64,
) -> f64 {
    if combined_quantity <= 0.0 {
        return 0.0;
    }
    (left_value * left_quantity + right_value * right_quantity) / combined_quantity
}

fn raw_payload_array(value: &JsonValue) -> JsonValue {
    if value.is_array() {
        value.clone()
    } else {
        json!([value])
    }
}

fn append_raw_payload(left: &JsonValue, right: &JsonValue) -> JsonValue {
    let mut rows = raw_payload_array(left)
        .as_array()
        .cloned()
        .unwrap_or_default();
    rows.push(right.clone());
    JsonValue::Array(rows)
}

fn sql_opt_text(value: Option<&str>) -> String {
    value
        .map(|value| format!("'{}'", sql_escape(value)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_opt_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

fn sql_opt_f64(value: Option<f64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_position_snapshot_rows() {
        let payload = json!({
            "Data": [{
                "DisplayAndFormat": {"Symbol": "AJG:xnys", "Description": "Arthur J. Gallagher & Co.", "Currency": "USD"},
                "PositionBase": {"Amount": 2.0, "OpenPrice": 200.0, "OpenPriceIncludingCosts": 201.0, "Uic": 24411, "AssetType": "Stock", "CanBeClosed": true},
                "PositionView": {"MarketState": "Open"}
            }]
        });

        let rows = parse_broker_positions(&payload);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "AJG:xnys");
        assert_eq!(rows[0].quantity, 2.0);
        assert_eq!(rows[0].open_price_including_costs_local, 201.0);
    }

    #[test]
    fn aggregates_duplicate_position_snapshot_rows() {
        let payload = json!({
            "Data": [
                {
                    "DisplayAndFormat": {"Symbol": "AMD:xnas", "Description": "Advanced Micro Devices Inc.", "Currency": "USD"},
                    "PositionBase": {"Amount": 3.0, "OpenPrice": 100.0, "OpenPriceIncludingCosts": 101.0, "Uic": 1422226, "AssetType": "Stock"},
                    "PositionView": {"MarketState": "Closed"}
                },
                {
                    "DisplayAndFormat": {"Symbol": "AMD:xnas", "Description": "Advanced Micro Devices Inc.", "Currency": "USD"},
                    "PositionBase": {"Amount": 5.0, "OpenPrice": 160.0, "OpenPriceIncludingCosts": 161.0, "Uic": 1422226, "AssetType": "Stock"},
                    "PositionView": {"MarketState": "Closed"}
                }
            ]
        });

        let rows = parse_broker_positions(&payload);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].quantity, 8.0);
        assert_eq!(rows[0].open_price_local, 137.5);
        assert!(rows[0].raw_payload.is_array());
    }

    #[test]
    fn parses_exposure_snapshot_rows() {
        let payload = json!({
            "Data": [{
                "Uic": 24411,
                "AssetType": "Stock",
                "Amount": 2.0,
                "AverageOpenPrice": 200.0,
                "ProfitLossOnTrade": 5.5,
                "InstrumentPriceDayPercentChange": 1.2,
                "CanBeClosed": true,
                "CalculationReliability": "Ok",
                "DisplayAndFormat": {"Symbol": "AJG:xnys", "Currency": "USD"}
            }]
        });

        let rows = parse_broker_exposures(&payload);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "AJG:xnys");
        assert_eq!(rows[0].profit_loss_on_trade, 5.5);
    }
}
