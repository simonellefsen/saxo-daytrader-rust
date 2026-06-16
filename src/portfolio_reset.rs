//! SIM Portfolio Reset from Live Positioner CSV
//!
//! This module implements the "Reset SIM from Live Export" feature.
//! It is only intended to be called when running against a Saxo SIM account.

use std::io::Cursor;

use anyhow::{Context, Result};
use chrono::Utc;
use csv::ReaderBuilder;
use serde::Deserialize;

use crate::state::AppState;

/// Row we care about from a Saxo Positioner export (Danish column names).
/// Using Option<String> to be tolerant of varying export formats over time.
#[derive(Debug, Deserialize)]
struct PositionerRow {
    #[serde(rename = "Instrument")]
    instrument: Option<String>,
    #[serde(rename = "L/K")]
    long_short: Option<String>,
    #[serde(rename = "Valuta")]
    currency: Option<String>,
    #[serde(rename = "Antal")]
    quantity: Option<String>,
    #[serde(rename = "Symbol")]
    symbol: Option<String>,
    #[serde(rename = "ISIN")]
    isin: Option<String>,
    #[serde(rename = "Aktivtype")]
    asset_type: Option<String>,
    #[serde(rename = "Kostpris")]
    cost_price: Option<String>,
    #[serde(rename = "Oprindelig værdi (DKK)")]
    original_value_dkk: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
}

/// Parsed clean position ready for import.
#[derive(Debug, Clone)]
pub struct ParsedPosition {
    pub instrument_name: String,
    pub symbol: String,
    pub isin: String,
    pub quantity: f64,
    pub currency: String,
    pub cost_basis_local: f64,
    pub cost_basis_dkk: f64,
    pub asset_type: String,
}

/// Result of a successful SIM reset.
#[derive(Debug, Clone)]
pub struct SimResetResult {
    pub batch_id: String,
    pub imported_positions: usize,
    pub cash_dkk: f64,
}

/// Parse a Saxo Positioner CSV (the format exported from the web platform).
pub fn parse_positioner_csv(bytes: &[u8]) -> Result<Vec<ParsedPosition>> {
    let cursor = Cursor::new(bytes);
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true) // tolerate rows with slightly different column counts (common in Positioner exports)
        .trim(csv::Trim::All)
        .from_reader(cursor);

    let mut positions = Vec::new();

    for result in rdr.deserialize::<PositionerRow>() {
        let row: PositionerRow = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        let long_short = row.long_short.as_deref().unwrap_or("").trim();
        let status = row.status.as_deref().unwrap_or("").trim();

        if long_short != "Lang" || status != "Åben" {
            continue;
        }

        let quantity = parse_dk_number(row.quantity.as_deref().unwrap_or("")).unwrap_or(0.0);
        if quantity <= 0.0 {
            continue;
        }

        let cost_price_local =
            parse_positioner_number(row.cost_price.as_deref().unwrap_or("")).unwrap_or(0.0);
        let cost_basis_local = cost_price_local * quantity;
        let cost_basis_dkk =
            parse_positioner_number(row.original_value_dkk.as_deref().unwrap_or("")).unwrap_or(0.0);

        let symbol = row.symbol.as_deref().unwrap_or("").trim().to_string();
        if symbol.is_empty() {
            continue;
        }

        positions.push(ParsedPosition {
            instrument_name: row.instrument.as_deref().unwrap_or("").trim().to_string(),
            symbol,
            isin: row.isin.as_deref().unwrap_or("").trim().to_string(),
            quantity,
            currency: row.currency.as_deref().unwrap_or("").trim().to_string(),
            cost_basis_local,
            cost_basis_dkk,
            asset_type: row.asset_type.as_deref().unwrap_or("").trim().to_string(),
        });
    }

    Ok(positions)
}

fn parse_dk_number(s: &str) -> Option<f64> {
    let cleaned = s
        .replace(" ", "")
        .replace("\u{a0}", "") // non-breaking space
        .replace(".", "") // thousand separator in DK
        .replace(",", "."); // decimal separator

    cleaned.parse::<f64>().ok()
}

fn parse_positioner_number(s: &str) -> Option<f64> {
    let mut cleaned = s.trim().replace(" ", "").replace("\u{a0}", "");
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.contains(',') && cleaned.contains('.') {
        if cleaned.rfind(',') > cleaned.rfind('.') {
            cleaned = cleaned.replace('.', "").replace(',', ".");
        } else {
            cleaned = cleaned.replace(',', "");
        }
    } else if cleaned.contains(',') {
        cleaned = cleaned.replace('.', "").replace(',', ".");
    }
    cleaned.parse::<f64>().ok()
}

/// Performs a hard reset of the SIM portfolio using a Live Positioner export.
///
/// This will:
/// - Create a new import_batch
/// - Delete previous portfolio lots/snapshots for the SIM environment
/// - Insert fresh position_snapshots + position_lots
/// - Insert synthetic opening trade_ledger entries
/// - Set the provided cash as the new baseline
pub async fn reset_sim_portfolio(
    state: &AppState,
    csv_bytes: &[u8],
    cash_dkk: f64,
    uploaded_filename: &str,
    also_sync_sim_broker: bool,
) -> Result<SimResetResult> {
    let positions = match parse_positioner_csv(csv_bytes) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Positioner CSV parse error: {e:#}");
            anyhow::bail!("failed to parse uploaded Positioner CSV: {e}");
        }
    };

    if positions.is_empty() {
        anyhow::bail!(
            "No open long positions found in the uploaded CSV. Make sure you exported 'Positioner' from your Live account and that it contains open long positions."
        );
    }

    let now = Utc::now();
    let batch_id = format!("live-reset-{}", now.format("%Y%m%dT%H%M%SZ"));
    let imported_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    // We use a simple heuristic: everything in the current database is considered
    // the "SIM environment" for the purpose of this reset. In a multi-environment
    // setup we would filter by environment, but for now we do a full portfolio wipe.
    let mut tx = state
        .pool
        .begin()
        .await
        .context("starting transaction for SIM reset")?;

    // 1. Create the import batch
    sqlx::query(
        "INSERT INTO import_batches (batch_id, imported_at, source_csv, source_position_count, imported_position_count, excluded_position_count, notes)
         VALUES ($1, $2, $3, $4, $5, 0, $6)",
    )
    .bind(&batch_id)
    .bind(&imported_at)
    .bind(uploaded_filename)
    .bind(positions.len() as i64)
    .bind(positions.len() as i64)
    .bind(format!("Live portfolio reset via uploaded Positioner CSV. Cash at export: {:.2} DKK", cash_dkk))
    .execute(&mut *tx)
    .await
    .context("creating import_batch for live reset")?;

    // 2. Hard wipe of previous portfolio state (complete reset).
    // We nuke everything that defined the previous SIM portfolio so the
    // uploaded CSV + the cash number the user typed become the *only*
    // portfolio state.
    sqlx::query("DELETE FROM lot_realizations")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM position_lots")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM position_snapshots")
        .execute(&mut *tx)
        .await?;

    // For a true hard reset, remove *every* existing cash entry so the number
    // the user types becomes the *absolute new cash balance* (not added on top).
    // We delete all rows that affect cash (symbol = 'CASH').
    sqlx::query("DELETE FROM trade_ledger WHERE symbol = 'CASH'")
        .execute(&mut *tx)
        .await?;

    // 3. Insert new snapshots + lots
    for pos in &positions {
        let open_price = if pos.quantity > 0.0 {
            pos.cost_basis_local / pos.quantity
        } else {
            0.0
        };

        // position_snapshots - complete set of columns to satisfy NOT NULL constraints
        sqlx::query(
            "INSERT INTO position_snapshots (
                batch_id, imported_at, instrument_name, symbol, isin, quantity, currency,
                open_price_local, current_price_local, cost_basis_local, cost_basis_dkk,
                market_value_local, market_value_dkk, unrealised_pnl_dkk, daily_pnl_dkk,
                allocation_pct, status, account_name, asset_class, market_status,
                value_date, source_csv, excluded, raw_payload_json
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,0,$23)",
        )
        .bind(&batch_id)
        .bind(&imported_at)
        .bind(&pos.instrument_name)
        .bind(&pos.symbol)
        .bind(&pos.isin)
        .bind(pos.quantity)
        .bind(&pos.currency)
        .bind(open_price)
        .bind(open_price)           // current_price_local at reset time = open
        .bind(pos.cost_basis_local)
        .bind(pos.cost_basis_dkk)
        .bind(pos.cost_basis_local) // market_value_local at reset = cost
        .bind(pos.cost_basis_dkk)   // market_value_dkk at reset = cost
        .bind(0.0)                  // unrealised_pnl_dkk at reset = 0
        .bind(0.0)                  // daily_pnl_dkk
        .bind(0.0)                  // allocation_pct (will be recalculated later)
        .bind("Åben")
        .bind("SIM-Reset")
        .bind(&pos.asset_type)
        .bind("Open")
        .bind(&imported_at)
        .bind(uploaded_filename)
        .bind(serde_json::json!({
            "instrument_name": pos.instrument_name,
            "symbol": pos.symbol,
            "isin": pos.isin,
            "quantity": pos.quantity,
            "currency": pos.currency,
            "cost_basis_local": pos.cost_basis_local,
            "cost_basis_dkk": pos.cost_basis_dkk,
            "asset_type": pos.asset_type
        }).to_string())
        .execute(&mut *tx)
        .await
        .with_context(|| format!("inserting position_snapshot for {}", pos.symbol))?;

        // position_lots (one lot per position for simplicity on reset)
        let lot_id = format!("{}:{}", batch_id, pos.symbol);
        sqlx::query(
            "INSERT INTO position_lots (
                lot_id, batch_id, created_at, acquired_at, symbol, isin, instrument_name,
                quantity_original, currency, cost_basis_total_local, cost_basis_total_dkk,
                fx_rate_to_dkk, source_type, source_reference, raw_payload_json
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
        )
        .bind(&lot_id)
        .bind(&batch_id)
        .bind(&imported_at)
        .bind(&imported_at)
        .bind(&pos.symbol)
        .bind(&pos.isin)
        .bind(&pos.instrument_name)
        .bind(pos.quantity)
        .bind(&pos.currency)
        .bind(pos.cost_basis_local)
        .bind(pos.cost_basis_dkk)
        .bind(1.0) // we treat the provided DKK values as authoritative
        .bind("live_csv_reset")
        .bind(uploaded_filename)
        .bind(
            serde_json::json!({
                "symbol": pos.symbol,
                "isin": pos.isin,
                "instrument_name": pos.instrument_name,
                "quantity": pos.quantity,
                "currency": pos.currency,
                "cost_basis_local": pos.cost_basis_local,
                "cost_basis_dkk": pos.cost_basis_dkk,
                "asset_type": pos.asset_type
            })
            .to_string(),
        )
        .execute(&mut *tx)
        .await
        .context("inserting position_lot")?;
    }

    // 4. Insert a cash baseline trade_ledger entry (using actual table columns)
    sqlx::query(
        "INSERT INTO trade_ledger (
            created_at, symbol, side, quantity, price_local, currency,
            gross_amount_dkk, commission_dkk, tax_dkk, net_amount_dkk,
            mode, status, notes, batch_id
         ) VALUES ($1, 'CASH', 'DEPOSIT', 0, 0, 'DKK', $2, 0, 0, $2, 'simulation', 'executed', 'Initial cash from Live export reset', $3)",
    )
    .bind(&imported_at)
    .bind(cash_dkk)
    .bind(&batch_id)
    .execute(&mut *tx)
    .await
    .context("inserting cash baseline ledger entry")?;

    tx.commit()
        .await
        .context("committing SIM portfolio reset transaction")?;

    // Step 2: If requested, create market orders to bring the actual SIM broker holdings
    // in line with the uploaded Live CSV (close current SIM positions + open the new ones).
    if also_sync_sim_broker {
        // Refresh current SIM broker state so we know what to close
        if let Err(e) = crate::saxo_portfolio::refresh_broker_snapshots(state).await {
            tracing::warn!("Could not refresh SIM broker snapshots before reset sync: {e:#}");
        }

        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        // 1. Close everything currently open in SIM
        let rows = sqlx::query(
            "SELECT symbol, quantity FROM broker_position_snapshots WHERE quantity > 0",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        let current_sim_positions: Vec<serde_json::Value> =
            rows.iter().map(crate::db::row_to_json).collect();

        for pos in &current_sim_positions {
            let symbol = pos
                .get("symbol")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let qty = pos.get("quantity").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if qty <= 0.0 || symbol.is_empty() {
                continue;
            }

            let order_id = format!("{}-close-{}", batch_id, symbol);
            let _ = sqlx::query(
                "INSERT INTO execution_orders (
                    id, created_at, symbol, action, order_type, quantity, status,
                    mode, adapter, strategy_type, strategy_role, notes
                ) VALUES ($1,$2,$3,'SELL','Market',$4,'pending_execution',
                          'simulation','saxo','reset','close','Auto-generated by Live→SIM reset')",
            )
            .bind(&order_id)
            .bind(&now)
            .bind(&symbol)
            .bind(qty)
            .execute(&state.pool)
            .await;
        }

        // 2. Open the positions from the uploaded Live CSV (market buys)
        for pos in &positions {
            let order_id = format!("{}-open-{}", batch_id, pos.symbol);
            let _ = sqlx::query(
                "INSERT INTO execution_orders (
                    id, created_at, symbol, action, order_type, quantity, status,
                    mode, adapter, strategy_type, strategy_role, notes
                ) VALUES ($1,$2,$3,'BUY','Market',$4,'pending_execution',
                          'simulation','saxo','reset','open','Auto-generated by Live→SIM reset')",
            )
            .bind(&order_id)
            .bind(&now)
            .bind(&pos.symbol)
            .bind(pos.quantity)
            .execute(&state.pool)
            .await;
        }

        tracing::info!(
            "SIM reset: created {} close orders + {} open orders (market) for broker sync",
            current_sim_positions.len(),
            positions.len()
        );
    }

    Ok(SimResetResult {
        batch_id,
        imported_positions: positions.len(),
        cash_dkk,
    })
}

impl AppState {
    /// High-level entry point called from the API handler.
    pub async fn perform_sim_reset_from_live_csv(
        &self,
        csv_bytes: &[u8],
        cash_dkk: f64,
        filename: &str,
        also_sync_sim_broker: bool,
    ) -> Result<SimResetResult> {
        reset_sim_portfolio(self, csv_bytes, cash_dkk, filename, also_sync_sim_broker).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioner_number_parser_preserves_dot_decimals() {
        assert_eq!(
            parse_positioner_number("195.75833333333333"),
            Some(195.75833333333333)
        );
        assert_eq!(parse_positioner_number("60,205.23"), Some(60205.23));
        assert_eq!(parse_positioner_number("60.205,23"), Some(60205.23));
    }

    #[test]
    fn parses_positioner_cost_price_as_unit_price() {
        let csv = concat!(
            "\"Instrument\",\"L/K\",\"Valuta\",\"Antal\",\"Åbningskurs\",\"Aktuel kurs\",\"% Total afkast\",\"% 1D afk.\",\"Gevinst/Tab i alt (DKK)\",\"Markedsværdi (DKK)\",\"1-dags gevinst/tab\",\"Status\",\"Optjent rente siden sidste kupondato\",\"Udløb\",\"Oprindelig værdi (DKK)\",\"Kostpris\",\"Handels G/T (DKK)\",\"Handelsgevinst/-tab\",\"Symbol\",\"Nettoafkast %\",\"Bruttoafkast %\",\"Konto\",\"Aktivklasse\",\"Bud/Udbud\",\"Senest opdateret\",\"Markedsstatus\",\"Markedsværdi\",\"Morningstar™\",\"Åbningstidspunkt\",\"Gevinst/tab i alt\",\"Pips/ticks\",\"% af portefølje\",\"Bæredygtighed\",\"Valørdato\",\"ISIN\",\"Udsteder\",\"Aktivtype\",\"1-dags gevinst/tab (DKK)\",\"Accrued Interest (DKK)\"\n",
            "\"Strategy Inc. \",\"Lang\",\"USD\",\"48\",\"195.421875\",\"176.74\",\"-9,56%\",\"-5,47%\",\"-5951.32\",\"54402.21\",\"-33 USD\",\"Åben\",\"\",\"\",\"60205.23\",\"195.75833333333333\",\"-5800.86\",\"-897 USD\",\"MSTR:xnas\",\"-0.09885054836598083\",\"-0.0963873072156689\",\"Hovedkonto\",\"–\",\"177,35 x 177,42\",\"13:39:33\",\"NASDAQ\",\"8.484 USD\",\"\",\"\",\"-920 USD\",\"-1.868\",\"0.06605929232885707\",\"\",\"\",\"US5949724083\",\"Strategy\",\"Aktie\",\"-209.310302\",\"\"\n"
        );
        let positions = parse_positioner_csv(csv.as_bytes()).unwrap();

        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].symbol, "MSTR:xnas");
        assert_eq!(positions[0].quantity, 48.0);
        assert!((positions[0].cost_basis_local - 9396.4).abs() < 1e-6);
        assert!((positions[0].cost_basis_dkk - 60205.23).abs() < 1e-6);
    }
}
