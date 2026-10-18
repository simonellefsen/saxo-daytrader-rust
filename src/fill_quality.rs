//! Realised execution cost and intraday timing for every fill.
//!
//! The 2026-10-18 edge review found that BUY fills lose 0.69% gross by the
//! first close (interval -1.32% to -0.06%) in every market and both fill
//! windows, and could not say whether that is the spread, slippage, the time
//! of day, or the stocks chosen. Nothing stored could separate them: an
//! order's `price_local` is overwritten with the fill price, and no quote was
//! kept from the moment of submission (`slippage_status:
//! unavailable_no_quote_at_submission`).
//!
//! Two measurements, each from data kept for the purpose:
//!
//! - **Against the quote at submission.** `saxo_order` reads the instrument's
//!   bid and ask just before placement and stores them here. A fill is then
//!   measured against the mid (slippage), the half-spread (what crossing it
//!   costs) and the touch (anything beyond the quoted price).
//! - **Against the session's bar.** `daily_price_bars` gives the day's open,
//!   high, low and close, so a fill can be placed in its day's range and the
//!   move from the fill to the close measured.
//!
//! Capture never decides anything. The quote read is time-boxed and cannot
//! fail or delay an order beyond its bound; an unreadable quote is recorded
//! as unavailable and the order goes ahead unchanged. Nothing here is read by
//! a gate. Errors are stored as a category, never as text: a transport error
//! can carry the request URL, and that URL carries the account key.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value as JsonValue;

use crate::{
    db::{clamp_limit, sql_escape, value_f64},
    models::{FillQualityPayload, FillQualityRowPayload, FillQualitySummaryPayload},
    state::{AppState, json_text},
};

pub fn create_schema_sql() -> &'static [&'static str] {
    &["CREATE TABLE IF NOT EXISTS order_submission_quotes (
        execution_order_id BIGINT PRIMARY KEY,
        observed_at TEXT NOT NULL,
        status TEXT NOT NULL,
        detail TEXT,
        bid DOUBLE PRECISION,
        ask DOUBLE PRECISION,
        price_type_bid TEXT,
        price_type_ask TEXT,
        delayed_by_minutes INTEGER,
        market_state TEXT,
        quote_updated_at TEXT
    )"]
}

/// The quote read just before an order was placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SubmissionQuote {
    /// `ok`, `unusable` (Saxo answered without a usable two-sided price) or
    /// `unavailable` (no answer in time, or no instrument to ask about).
    pub status: &'static str,
    pub detail: Option<String>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub price_type_bid: Option<String>,
    pub price_type_ask: Option<String>,
    pub delayed_by_minutes: Option<i64>,
    pub market_state: Option<String>,
    pub quote_updated_at: Option<String>,
}

impl SubmissionQuote {
    pub(crate) fn unavailable(category: &str) -> Self {
        Self {
            status: "unavailable",
            detail: Some(category.to_string()),
            ..Self::default()
        }
    }
}

/// A stored category for a failed quote read, never the error text.
pub(crate) fn failure_category(error_text: &str) -> String {
    if let Some(index) = error_text.find("HTTP ") {
        let code: String = error_text[index + 5..].chars().take(3).collect();
        if code.len() == 3 && code.chars().all(|c| c.is_ascii_digit()) {
            return format!("http_{code}");
        }
    }
    if error_text.contains("timed out") {
        return "timeout".to_string();
    }
    "request_failed".to_string()
}

fn positive(value: Option<&JsonValue>) -> Option<f64> {
    value
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// Reads a `/trade/v1/infoprices` answer.
pub(crate) fn submission_quote_from_infoprice(payload: &JsonValue) -> SubmissionQuote {
    let quote = payload.get("Quote").unwrap_or(&JsonValue::Null);
    let text = |key: &str| {
        quote
            .get(key)
            .and_then(JsonValue::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let bid = positive(quote.get("Bid"));
    let ask = positive(quote.get("Ask"));
    let usable = matches!((bid, ask), (Some(bid), Some(ask)) if ask >= bid);
    SubmissionQuote {
        status: if usable { "ok" } else { "unusable" },
        detail: (!usable).then(|| {
            text("ErrorCode")
                .filter(|code| code != "None")
                .unwrap_or_else(|| "no_two_sided_price".to_string())
        }),
        bid,
        ask,
        price_type_bid: text("PriceTypeBid"),
        price_type_ask: text("PriceTypeAsk"),
        delayed_by_minutes: quote.get("DelayedByMinutes").and_then(JsonValue::as_i64),
        market_state: text("MarketState"),
        quote_updated_at: payload
            .get("LastUpdated")
            .and_then(JsonValue::as_str)
            .map(str::to_string),
    }
}

fn sql_text(value: Option<&str>) -> String {
    value.map_or_else(
        || "NULL".to_string(),
        |text| format!("'{}'", sql_escape(text)),
    )
}

fn sql_number(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map_or_else(|| "NULL".to_string(), |value| value.to_string())
}

pub(crate) async fn store_submission_quote(
    state: &AppState,
    execution_order_id: i64,
    observed_at: &str,
    quote: &SubmissionQuote,
) -> Result<()> {
    sqlx::query(&format!(
        "INSERT INTO order_submission_quotes (
            execution_order_id, observed_at, status, detail, bid, ask, price_type_bid,
            price_type_ask, delayed_by_minutes, market_state, quote_updated_at
         ) VALUES ({execution_order_id}, '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {})
         ON CONFLICT(execution_order_id) DO UPDATE SET
            observed_at = excluded.observed_at,
            status = excluded.status,
            detail = excluded.detail,
            bid = excluded.bid,
            ask = excluded.ask,
            price_type_bid = excluded.price_type_bid,
            price_type_ask = excluded.price_type_ask,
            delayed_by_minutes = excluded.delayed_by_minutes,
            market_state = excluded.market_state,
            quote_updated_at = excluded.quote_updated_at",
        sql_escape(observed_at),
        quote.status,
        sql_text(quote.detail.as_deref()),
        sql_number(quote.bid),
        sql_number(quote.ask),
        sql_text(quote.price_type_bid.as_deref()),
        sql_text(quote.price_type_ask.as_deref()),
        quote
            .delayed_by_minutes
            .map_or_else(|| "NULL".to_string(), |minutes| minutes.to_string()),
        sql_text(quote.market_state.as_deref()),
        sql_text(quote.quote_updated_at.as_deref()),
    ))
    .execute(&state.pool)
    .await
    .context("storing the submission quote")?;
    Ok(())
}

/// One fill with whatever was recorded about it.
pub(crate) struct Fill<'a> {
    pub symbol: &'a str,
    pub side: &'a str,
    pub filled_at: &'a str,
    pub order_type: Option<String>,
    pub price: f64,
    pub quantity: f64,
    /// `None` when no quote was recorded for the order.
    pub quote: Option<RecordedQuote>,
    /// The session's (open, high, low, close), any of which may be missing.
    pub bar: Option<(Option<f64>, Option<f64>, Option<f64>, f64)>,
}

pub(crate) struct RecordedQuote {
    pub status: String,
    pub detail: Option<String>,
    pub observed_at: String,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub price_type_bid: Option<String>,
    pub price_type_ask: Option<String>,
    pub delayed_by_minutes: Option<i64>,
}

/// +1 for a BUY, -1 for a SELL: multiplying a price move by it makes a
/// positive figure worse for the book on either side.
fn cost_sign(side: &str) -> f64 {
    if side.eq_ignore_ascii_case("SELL") {
        -1.0
    } else {
        1.0
    }
}

fn bps(ratio: f64) -> f64 {
    ratio * 10_000.0
}

pub(crate) fn measure(fill: &Fill<'_>) -> FillQualityRowPayload {
    let sign = cost_sign(fill.side);
    let mut row = FillQualityRowPayload {
        symbol: fill.symbol.to_string(),
        side: fill.side.to_uppercase(),
        filled_at: fill.filled_at.to_string(),
        order_type: fill.order_type.clone(),
        fill_price_local: fill.price,
        quantity: fill.quantity,
        quote_status: "not_recorded".to_string(),
        bar_status: "no_bar".to_string(),
        ..FillQualityRowPayload::default()
    };

    if let Some(quote) = &fill.quote {
        row.quote_status = quote.status.clone();
        row.quote_detail = quote.detail.clone();
        row.quote_observed_at = Some(quote.observed_at.clone());
        row.bid = quote.bid;
        row.ask = quote.ask;
        row.price_type_bid = quote.price_type_bid.clone();
        row.price_type_ask = quote.price_type_ask.clone();
        row.delayed_by_minutes = quote.delayed_by_minutes;
        if let (Some(bid), Some(ask), "ok") = (quote.bid, quote.ask, quote.status.as_str()) {
            let mid = (bid + ask) / 2.0;
            let touch = if sign > 0.0 { ask } else { bid };
            row.slippage_vs_mid_bps = Some(bps(sign * (fill.price / mid - 1.0)));
            row.half_spread_bps = Some(bps((ask - bid) / (2.0 * mid)));
            row.beyond_touch_bps = Some(bps(sign * (fill.price / touch - 1.0)));
        }
    }

    if let Some((open, high, low, close)) = fill.bar {
        row.bar_open = open;
        row.bar_high = high;
        row.bar_low = low;
        row.bar_close = Some(close);
        row.fill_to_close_bps = Some(bps(-sign * (close / fill.price - 1.0)));
        row.open_to_fill_bps = open.map(|open| bps(sign * (fill.price / open - 1.0)));
        row.bar_status = match (high, low) {
            (Some(high), Some(low)) if high > low => {
                // A small tolerance for rounding between the fill and the bar.
                let slack = (high - low) * 0.001;
                if fill.price < low - slack || fill.price > high + slack {
                    // The bar and the fill disagree, so no timing figure from
                    // this bar can be trusted.
                    row.fill_to_close_bps = None;
                    row.open_to_fill_bps = None;
                    "fill_outside_range"
                } else {
                    row.position_in_range =
                        Some(((fill.price - low) / (high - low)).clamp(0.0, 1.0));
                    "matched"
                }
            }
            _ => "no_range",
        }
        .to_string();
    }
    row
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(sorted[n / 2]),
        _ => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

pub(crate) fn summarize(rows: &[FillQualityRowPayload]) -> Vec<FillQualitySummaryPayload> {
    ["BUY", "SELL"]
        .iter()
        .map(|side| {
            let side_rows: Vec<&FillQualityRowPayload> =
                rows.iter().filter(|row| row.side == *side).collect();
            let quoted: Vec<&&FillQualityRowPayload> = side_rows
                .iter()
                .filter(|row| row.slippage_vs_mid_bps.is_some())
                .collect();
            let collect = |pick: &dyn Fn(&FillQualityRowPayload) -> Option<f64>,
                           from: &[&FillQualityRowPayload]| {
                from.iter()
                    .filter_map(|row| pick(row))
                    .collect::<Vec<f64>>()
            };
            let slippage = collect(&|row| row.slippage_vs_mid_bps, &side_rows);
            let matched: Vec<&FillQualityRowPayload> = side_rows
                .iter()
                .copied()
                .filter(|row| row.bar_status == "matched")
                .collect();
            FillQualitySummaryPayload {
                side: side.to_string(),
                fills: side_rows.len() as i64,
                quote_ok: quoted.len() as i64,
                quote_tradable: quoted
                    .iter()
                    .filter(|row| {
                        row.price_type_bid.as_deref() == Some("Tradable")
                            && row.price_type_ask.as_deref() == Some("Tradable")
                    })
                    .count() as i64,
                mean_slippage_vs_mid_bps: mean(&slippage),
                median_slippage_vs_mid_bps: median(&slippage),
                mean_half_spread_bps: mean(&collect(&|row| row.half_spread_bps, &side_rows)),
                mean_beyond_touch_bps: mean(&collect(&|row| row.beyond_touch_bps, &side_rows)),
                bar_matched: matched.len() as i64,
                mean_position_in_range: mean(&collect(&|row| row.position_in_range, &matched)),
                mean_open_to_fill_bps: mean(&collect(&|row| row.open_to_fill_bps, &matched)),
                mean_fill_to_close_bps: mean(&collect(&|row| row.fill_to_close_bps, &matched)),
            }
        })
        .collect()
}

pub(crate) fn definitions() -> BTreeMap<String, String> {
    [
        (
            "sign",
            "every _bps figure is signed so that positive is worse for the book: paying up on a \
             BUY, selling down on a SELL, or the price moving against the position by the close",
        ),
        (
            "slippage_vs_mid_bps",
            "fill price against the mid of the bid and ask read just before placement",
        ),
        (
            "half_spread_bps",
            "half the quoted spread at submission: the cost of crossing it on one side",
        ),
        (
            "beyond_touch_bps",
            "fill price against the quoted touch (the ask for a BUY, the bid for a SELL); \
             positive means the fill was worse than the quoted price",
        ),
        (
            "quote_status",
            "not_recorded for orders placed before submission quotes were captured on \
             2026-10-18; unavailable when the read failed or timed out, with a category only; \
             a delayed or indicative quote is kept and labelled by its price types",
        ),
        (
            "position_in_range",
            "where the fill sat between the session's low (0) and high (1), from \
             daily_price_bars on the fill's UTC date; a fill outside the bar is flagged, not \
             clamped",
        ),
        (
            "fill_to_close_bps",
            "the session close against the fill; the first part of the entry evaluation's \
             one-session return, before the overnight move",
        ),
        (
            "scope",
            "read-only measurement; it gates nothing and no figure here is a realised P/L",
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

fn optional_text(row: &JsonValue, key: &str) -> Option<String> {
    Some(json_text(row, key)).filter(|text| !text.is_empty())
}

fn optional_number(row: &JsonValue, key: &str) -> Option<f64> {
    row.get(key)
        .filter(|value| !value.is_null())
        .map(|_| value_f64(row, key))
        .filter(|value| value.is_finite())
}

/// Every fill, newest first, measured against its submission quote and bar.
pub(crate) async fn fill_quality_payload(
    state: &AppState,
    limit: i64,
) -> Result<FillQualityPayload> {
    let fills = state
        .select_json(&format!(
            "SELECT f.created_at, f.symbol, f.side, f.delta_quantity, f.average_price_local,
                    o.order_type, q.status AS quote_status, q.detail AS quote_detail,
                    q.observed_at AS quote_observed_at, q.bid, q.ask, q.price_type_bid,
                    q.price_type_ask, q.delayed_by_minutes
             FROM execution_fills AS f
             LEFT JOIN execution_orders AS o ON o.id = f.execution_order_id
             LEFT JOIN order_submission_quotes AS q
                    ON q.execution_order_id = f.execution_order_id
             WHERE f.delta_quantity > 0 AND f.average_price_local > 0
             ORDER BY f.created_at DESC
             LIMIT {}",
            clamp_limit(limit, 1, 2_000)
        ))
        .await?;

    let mut rows = Vec::with_capacity(fills.len());
    for fill in &fills {
        let symbol = json_text(fill, "symbol");
        let filled_at = json_text(fill, "created_at");
        let bar = state
            .select_json(&format!(
                "SELECT open, high, low, close FROM daily_price_bars
                 WHERE symbol = '{}' AND bar_date = '{}'",
                sql_escape(&symbol),
                sql_escape(filled_at.get(..10).unwrap_or_default())
            ))
            .await
            .unwrap_or_default()
            .first()
            .map(|bar| {
                (
                    optional_number(bar, "open"),
                    optional_number(bar, "high"),
                    optional_number(bar, "low"),
                    value_f64(bar, "close"),
                )
            })
            .filter(|(_, _, _, close)| *close > 0.0);
        let quote = optional_text(fill, "quote_status").map(|status| RecordedQuote {
            status,
            detail: optional_text(fill, "quote_detail"),
            observed_at: json_text(fill, "quote_observed_at"),
            bid: optional_number(fill, "bid"),
            ask: optional_number(fill, "ask"),
            price_type_bid: optional_text(fill, "price_type_bid"),
            price_type_ask: optional_text(fill, "price_type_ask"),
            delayed_by_minutes: fill.get("delayed_by_minutes").and_then(JsonValue::as_i64),
        });
        rows.push(measure(&Fill {
            symbol: &symbol,
            side: &json_text(fill, "side"),
            filled_at: &filled_at,
            order_type: optional_text(fill, "order_type"),
            price: value_f64(fill, "average_price_local"),
            quantity: value_f64(fill, "delta_quantity"),
            quote,
            bar,
        }));
    }

    Ok(FillQualityPayload {
        summaries: summarize(&rows),
        definitions: definitions(),
        generated_at: Utc::now().to_rfc3339(),
        fills: rows,
        status: None,
        unavailable_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_two_sided_quote_is_ok_and_anything_less_is_unusable() {
        let ok = submission_quote_from_infoprice(&json!({
            "LastUpdated": "2026-10-19T14:30:01.123Z",
            "Quote": {"Bid": 99.9, "Ask": 100.1, "PriceTypeBid": "Tradable",
                      "PriceTypeAsk": "Tradable", "DelayedByMinutes": 0, "MarketState": "Open",
                      "ErrorCode": "None"}
        }));
        assert_eq!(ok.status, "ok");
        assert_eq!((ok.bid, ok.ask), (Some(99.9), Some(100.1)));
        assert_eq!(ok.detail, None);
        assert_eq!(ok.delayed_by_minutes, Some(0));

        let no_access = submission_quote_from_infoprice(&json!({
            "Quote": {"Bid": 0.0, "Ask": 0.0, "ErrorCode": "NoAccess"}
        }));
        assert_eq!(
            (no_access.status, no_access.detail.as_deref()),
            ("unusable", Some("NoAccess"))
        );

        let crossed =
            submission_quote_from_infoprice(&json!({"Quote": {"Bid": 101.0, "Ask": 100.0}}));
        assert_eq!(crossed.status, "unusable");
        assert_eq!(
            submission_quote_from_infoprice(&json!({})).status,
            "unusable"
        );
    }

    /// A transport error can print the request URL, which carries the account
    /// key, so only a category is ever stored.
    #[test]
    fn a_failed_read_is_stored_as_a_category_never_as_text() {
        assert_eq!(
            failure_category("Saxo GET failed: HTTP 404: not found"),
            "http_404"
        );
        assert_eq!(
            failure_category(
                "error sending request for url (https://x/infoprices?AccountKey=SECRET)"
            ),
            "request_failed"
        );
        assert_eq!(failure_category("operation timed out"), "timeout");
    }

    fn quote(bid: f64, ask: f64) -> Option<RecordedQuote> {
        Some(RecordedQuote {
            status: "ok".to_string(),
            detail: None,
            observed_at: "2026-10-19T14:30:00Z".to_string(),
            bid: Some(bid),
            ask: Some(ask),
            price_type_bid: Some("Tradable".to_string()),
            price_type_ask: Some("Tradable".to_string()),
            delayed_by_minutes: Some(0),
        })
    }

    fn fill<'a>(side: &'a str, price: f64) -> Fill<'a> {
        Fill {
            symbol: "AAPL:xnas",
            side,
            filled_at: "2026-10-19T14:30:02Z",
            order_type: Some("Market".to_string()),
            price,
            quantity: 10.0,
            quote: quote(99.0, 101.0),
            bar: Some((Some(98.0), Some(104.0), Some(96.0), 97.0)),
        }
    }

    /// Positive is worse on either side: a BUY paying above the mid and a
    /// SELL receiving below it are both costs.
    #[test]
    fn slippage_and_spread_are_signed_as_costs_on_both_sides() {
        let buy = measure(&fill("BUY", 101.5));
        assert!((buy.slippage_vs_mid_bps.unwrap() - 150.0).abs() < 1e-9);
        assert!((buy.half_spread_bps.unwrap() - 100.0).abs() < 1e-9);
        // 101.5 against an ask of 101: about 49.5 bp beyond the touch.
        assert!((buy.beyond_touch_bps.unwrap() - 49.504_950_495).abs() < 1e-6);

        let sell = measure(&fill("SELL", 99.0));
        assert!((sell.slippage_vs_mid_bps.unwrap() - 100.0).abs() < 1e-9);
        assert!(
            sell.beyond_touch_bps.unwrap().abs() < 1e-9,
            "filled exactly at the bid"
        );
    }

    #[test]
    fn a_fill_is_placed_in_its_sessions_range_and_timed_to_the_close() {
        // Range 96-104, fill 102: three quarters up. Close 97 is 4.9% below
        // the fill, which is against a BUY.
        let row = measure(&fill("BUY", 102.0));
        assert_eq!(row.bar_status, "matched");
        assert!((row.position_in_range.unwrap() - 0.75).abs() < 1e-12);
        assert!((row.open_to_fill_bps.unwrap() - bps(102.0 / 98.0 - 1.0)).abs() < 1e-9);
        assert!((row.fill_to_close_bps.unwrap() - bps(1.0 - 97.0 / 102.0)).abs() < 1e-9);
        assert!(
            row.fill_to_close_bps.unwrap() > 0.0,
            "the close went against the BUY"
        );
    }

    /// A fill the stored bar cannot contain means the two disagree, so the bar
    /// is not used rather than clamped into a plausible answer.
    #[test]
    fn a_fill_outside_its_bar_is_flagged_and_not_timed() {
        let row = measure(&fill("BUY", 110.0));
        assert_eq!(row.bar_status, "fill_outside_range");
        assert_eq!((row.position_in_range, row.fill_to_close_bps), (None, None));

        let mut unranged = fill("BUY", 100.0);
        unranged.bar = Some((None, None, None, 99.0));
        assert_eq!(measure(&unranged).bar_status, "no_range");
    }

    #[test]
    fn fills_before_capture_are_not_recorded_and_left_out_of_the_means() {
        let mut old = fill("BUY", 101.5);
        old.quote = None;
        old.bar = None;
        let rows = vec![
            measure(&old),
            measure(&fill("BUY", 101.5)),
            measure(&fill("SELL", 99.0)),
        ];
        assert_eq!(rows[0].quote_status, "not_recorded");
        assert_eq!(rows[0].bar_status, "no_bar");

        let summaries = summarize(&rows);
        let buy = summaries.iter().find(|s| s.side == "BUY").unwrap();
        assert_eq!(
            (buy.fills, buy.quote_ok, buy.quote_tradable, buy.bar_matched),
            (2, 1, 1, 1)
        );
        assert!((buy.mean_slippage_vs_mid_bps.unwrap() - 150.0).abs() < 1e-9);
        assert_eq!(
            summaries.iter().find(|s| s.side == "SELL").unwrap().fills,
            1
        );
    }

    async fn quality_test_state() -> AppState {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory fill quality database");
        let mut statements = vec![
            "CREATE TABLE execution_fills (id INTEGER PRIMARY KEY, created_at TEXT, symbol TEXT,
                side TEXT, delta_quantity REAL, average_price_local REAL,
                execution_order_id INTEGER)",
            "CREATE TABLE execution_orders (id INTEGER PRIMARY KEY, order_type TEXT)",
        ];
        statements.extend(create_schema_sql());
        statements.extend(crate::price_bars::create_schema_sql());
        for sql in statements {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create fill quality table");
        }
        AppState {
            config_path: std::path::PathBuf::from("fill-quality-test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".to_string(),
            pool,
        }
    }

    /// Stored quote and bar reach the measurement through the builder's joins,
    /// and a fill with neither is still listed.
    #[tokio::test]
    async fn the_builder_joins_each_fill_to_its_quote_and_bar() {
        let state = quality_test_state().await;
        for sql in [
            "INSERT INTO execution_orders (id, order_type) VALUES (7, 'Market'), (8, 'Market')",
            "INSERT INTO execution_fills (id, created_at, symbol, side, delta_quantity,
                 average_price_local, execution_order_id)
             VALUES (1, '2026-10-19T14:30:02Z', 'AAPL:xnas', 'BUY', 10, 101.5, 7),
                    (2, '2026-10-16T14:30:02Z', 'MSFT:xnas', 'BUY', 5, 400, 8)",
            "INSERT INTO daily_price_bars (symbol, bar_date, analysis_symbol, bar_time, open, high,
                 low, close, run_id, updated_at)
             VALUES ('AAPL:xnas', '2026-10-19', 'AAPL:xnas', '2026-10-19T00:00:00Z', 98, 104, 96,
                     97, 'run', '2026-10-19T21:45:00Z')",
        ] {
            sqlx::query(sql).execute(&state.pool).await.expect("seed");
        }
        let captured = submission_quote_from_infoprice(&json!({
            "Quote": {"Bid": 99.0, "Ask": 101.0, "PriceTypeBid": "Tradable", "PriceTypeAsk": "Tradable"}
        }));
        store_submission_quote(&state, 7, "2026-10-19T14:30:00Z", &captured)
            .await
            .unwrap();
        // A second read for the same order replaces the first.
        store_submission_quote(&state, 7, "2026-10-19T14:30:00Z", &captured)
            .await
            .unwrap();

        let payload = fill_quality_payload(&state, 50).await.unwrap();
        assert_eq!(payload.fills.len(), 2);
        let aapl = &payload.fills[0];
        assert_eq!(
            (aapl.quote_status.as_str(), aapl.bar_status.as_str()),
            ("ok", "matched")
        );
        assert!((aapl.slippage_vs_mid_bps.unwrap() - 150.0).abs() < 1e-9);
        let msft = &payload.fills[1];
        assert_eq!(
            (msft.quote_status.as_str(), msft.bar_status.as_str()),
            ("not_recorded", "no_bar")
        );
    }
}
