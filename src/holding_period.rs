//! Holding-period evidence, matched FIFO from the trade ledger at read time.
//!
//! The realised-sell panel reported holding time as
//! `unavailable_no_lot_sale_linkage`, and the roadmap deferred the fix as
//! "FIFO `lot_realizations` at SELL time". Persisting realizations would only
//! describe *future* sales; deriving them covers the seventy already recorded
//! and can be recomputed whenever the matching improves. Danish share income
//! uses gennemsnitsmetoden -- the average method -- so the per-lot table the
//! schema reserves is not needed for tax either, which leaves this the only
//! consumer that ever wanted the linkage.
//!
//! The naive shortcut does not work: a per-symbol `MIN(acquired_at)` spans two
//! trading episodes for the thirteen of forty symbols that were re-bought after
//! selling, and overstates their holding period. Walking each symbol's own
//! timeline and consuming acquisitions FIFO is what makes the answer honest.

use serde_json::{Value as JsonValue, json};

use crate::db::value_f64;
use crate::state::json_text;

/// Minimum matched sales before the aggregate is worth reading.
const MIN_SAMPLE: usize = 20;

/// One FIFO-matched slice of a sale: shares acquired at one moment, sold at
/// another. A single sale can produce several when it spans acquisitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HoldingSlice {
    pub(crate) days: f64,
    pub(crate) quantity: f64,
    /// The sale's realised gain, allocated to this slice pro rata by quantity.
    pub(crate) realised_gain_dkk: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HoldingPeriodEvidence {
    pub(crate) slices: Vec<HoldingSlice>,
    pub(crate) matched_sale_count: usize,
    pub(crate) unmatched_sale_count: usize,
    pub(crate) matched_quantity: f64,
    pub(crate) unmatched_quantity: f64,
}

/// One acquisition still open for matching.
#[derive(Clone, Debug)]
struct OpenLot {
    acquired_at: String,
    remaining: f64,
}

fn parse_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

/// Match sales against acquisitions FIFO, per symbol, in time order.
///
/// `seed_lots` carry positions that entered the book without a BUY ledger row:
/// the 2026-05-18 CSV import and the 2026-07-16 broker bootstrap. They are the
/// difference between matching 46 of 70 sales and matching 68 -- omitting them
/// would not merely lose coverage, it would bias the sample toward positions
/// this system opened itself.
pub(crate) fn fifo_holding_periods(
    ledger: &[JsonValue],
    seed_lots: &[JsonValue],
) -> HoldingPeriodEvidence {
    let mut by_symbol: std::collections::HashMap<String, Vec<(String, bool, f64, f64)>> =
        std::collections::HashMap::new();
    for row in ledger {
        let symbol = json_text(row, "symbol");
        let created_at = json_text(row, "created_at");
        let quantity = value_f64(row, "quantity");
        if symbol.is_empty() || created_at.is_empty() || !(quantity > 0.0) {
            continue;
        }
        let side = json_text(row, "side").to_ascii_uppercase();
        let is_buy = match side.as_str() {
            "BUY" => true,
            "SELL" => false,
            _ => continue,
        };
        by_symbol.entry(symbol).or_default().push((
            created_at,
            is_buy,
            quantity,
            value_f64(row, "realised_gain_dkk"),
        ));
    }

    let mut seeds: std::collections::HashMap<String, Vec<OpenLot>> =
        std::collections::HashMap::new();
    for lot in seed_lots {
        let symbol = json_text(lot, "symbol");
        let acquired_at = json_text(lot, "acquired_at");
        let quantity = value_f64(lot, "quantity_original");
        if symbol.is_empty() || acquired_at.is_empty() || !(quantity > 0.0) {
            continue;
        }
        seeds.entry(symbol).or_default().push(OpenLot {
            acquired_at,
            remaining: quantity,
        });
    }

    let mut evidence = HoldingPeriodEvidence::default();
    let mut symbols: Vec<&String> = by_symbol.keys().collect();
    symbols.sort();
    for symbol in symbols {
        let mut events = by_symbol[symbol].clone();
        events.sort_by(|left, right| left.0.cmp(&right.0));
        let mut open = seeds.remove(symbol).unwrap_or_default();
        open.sort_by(|left, right| left.acquired_at.cmp(&right.acquired_at));

        for (sold_at, is_buy, quantity, realised_gain_dkk) in events {
            if is_buy {
                open.push(OpenLot {
                    acquired_at: sold_at,
                    remaining: quantity,
                });
                continue;
            }
            let Some(sold_time) = parse_time(&sold_at) else {
                evidence.unmatched_sale_count += 1;
                evidence.unmatched_quantity += quantity;
                continue;
            };
            let mut outstanding = quantity;
            let mut matched_here = Vec::new();
            while outstanding > f64::EPSILON {
                let Some(lot) = open.first_mut() else { break };
                let take = lot.remaining.min(outstanding);
                if let Some(acquired) = parse_time(&lot.acquired_at) {
                    let days = (sold_time - acquired).num_seconds() as f64 / 86_400.0;
                    // A sale dated before its acquisition is a data fault, not
                    // a negative holding period; drop the slice rather than
                    // let it drag a median downward.
                    if days >= 0.0 {
                        matched_here.push((days, take));
                    }
                }
                lot.remaining -= take;
                outstanding -= take;
                if lot.remaining <= f64::EPSILON {
                    open.remove(0);
                }
            }
            let matched_quantity = quantity - outstanding;
            if outstanding > f64::EPSILON {
                evidence.unmatched_sale_count += 1;
                evidence.unmatched_quantity += outstanding;
            } else {
                evidence.matched_sale_count += 1;
            }
            evidence.matched_quantity += matched_quantity;
            for (days, take) in matched_here {
                evidence.slices.push(HoldingSlice {
                    days,
                    quantity: take,
                    // Pro rata by quantity: a sale that spans two acquisitions
                    // cannot say which of them earned the gain.
                    realised_gain_dkk: if quantity > 0.0 {
                        realised_gain_dkk * take / quantity
                    } else {
                        0.0
                    },
                });
            }
        }
    }
    evidence
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The payload the realised-sell panel reads.
///
/// Reports coverage beside the medians on purpose: a holding period computed
/// over 68 of 70 sales and one computed over 46 are different claims, and the
/// second would be quietly biased toward positions this system opened itself.
pub(crate) fn holding_period_evidence_json(evidence: &HoldingPeriodEvidence) -> JsonValue {
    let winners: Vec<f64> = evidence
        .slices
        .iter()
        .filter(|slice| slice.realised_gain_dkk > 0.0)
        .map(|slice| slice.days)
        .collect();
    let losers: Vec<f64> = evidence
        .slices
        .iter()
        .filter(|slice| slice.realised_gain_dkk < 0.0)
        .map(|slice| slice.days)
        .collect();
    let total_quantity = evidence.matched_quantity + evidence.unmatched_quantity;
    let status = if evidence.matched_sale_count == 0 {
        "unavailable_no_matched_sales"
    } else if evidence.matched_sale_count < MIN_SAMPLE {
        "collecting"
    } else {
        "available"
    };
    json!({
        "status": status,
        "method": "fifo_matched_from_trade_ledger_at_read_time",
        "counting_unit": "matched acquisition-to-sale slice",
        "sample_requirement": MIN_SAMPLE,
        "matched_sale_count": evidence.matched_sale_count,
        "unmatched_sale_count": evidence.unmatched_sale_count,
        "matched_quantity_pct": if total_quantity > 0.0 {
            json!(round1(100.0 * evidence.matched_quantity / total_quantity))
        } else {
            JsonValue::Null
        },
        "slice_count": evidence.slices.len(),
        "winner_slice_count": winners.len(),
        "loser_slice_count": losers.len(),
        "median_days_held": median(evidence.slices.iter().map(|slice| slice.days).collect()).map(round1),
        "winner_median_days_held": median(winners).map(round1),
        "loser_median_days_held": median(losers).map(round1),
        "safety": "read_only_local_ledger_derivation_no_broker_call_and_not_a_trading_gate",
        "interpretation": "Acquisitions are consumed FIFO along each symbol's own timeline, so a symbol bought, sold and bought again is measured per episode rather than from its first purchase. Positions that entered without a BUY ledger row are seeded from their import or bootstrap lot. A sale whose acquisitions cannot be found is counted as unmatched rather than estimated. Gains are allocated to a slice pro rata by quantity; this is accounting evidence, not a backtest or a trading gate.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buy(symbol: &str, at: &str, quantity: f64) -> JsonValue {
        json!({"symbol": symbol, "side": "BUY", "created_at": at, "quantity": quantity, "realised_gain_dkk": 0.0})
    }

    fn sell(symbol: &str, at: &str, quantity: f64, gain: f64) -> JsonValue {
        json!({"symbol": symbol, "side": "SELL", "created_at": at, "quantity": quantity, "realised_gain_dkk": gain})
    }

    fn seed(symbol: &str, at: &str, quantity: f64) -> JsonValue {
        json!({"symbol": symbol, "acquired_at": at, "quantity_original": quantity})
    }

    /// The case that rules out the cheap answer. `MIN(acquired_at)` for this
    /// symbol is 2026-01-01, so the naive read calls the second sale a
    /// 90-day hold when it was five days. Thirteen of forty traded symbols
    /// were re-bought after selling, so this is not a corner case.
    #[test]
    fn a_symbol_bought_again_after_selling_is_measured_per_episode() {
        let evidence = fifo_holding_periods(
            &[
                buy("AMD:xnas", "2026-01-01T10:00:00Z", 10.0),
                sell("AMD:xnas", "2026-01-11T10:00:00Z", 10.0, 500.0),
                buy("AMD:xnas", "2026-03-26T10:00:00Z", 10.0),
                sell("AMD:xnas", "2026-03-31T10:00:00Z", 10.0, -200.0),
            ],
            &[],
        );

        assert_eq!(evidence.matched_sale_count, 2);
        assert_eq!(evidence.unmatched_sale_count, 0);
        let days: Vec<f64> = evidence.slices.iter().map(|slice| slice.days).collect();
        assert_eq!(
            days,
            vec![10.0, 5.0],
            "the second episode starts at its own buy"
        );
    }

    /// Positions from the 2026-05-18 CSV import and the 2026-07-16 broker
    /// bootstrap have no BUY ledger row. Without seeding, only 46 of 70 sales
    /// match and the sample skews toward positions this system opened itself.
    #[test]
    fn a_position_that_predates_the_ledger_matches_from_its_import_lot() {
        let evidence = fifo_holding_periods(
            &[sell("NOVOb:xcse", "2026-06-17T10:00:00Z", 100.0, 1_000.0)],
            &[seed("NOVOb:xcse", "2026-05-18T10:00:00Z", 235.0)],
        );

        assert_eq!(evidence.matched_sale_count, 1);
        assert_eq!(evidence.slices.len(), 1);
        assert!((evidence.slices[0].days - 30.0).abs() < 0.01);
    }

    /// A sale whose acquisitions cannot be found is reported, not estimated.
    #[test]
    fn a_sale_with_no_acquisition_is_counted_as_unmatched() {
        let evidence = fifo_holding_periods(
            &[sell("GOOGL:xnas", "2026-06-01T10:00:00Z", 18.0, 250.0)],
            &[],
        );

        assert_eq!(evidence.matched_sale_count, 0);
        assert_eq!(evidence.unmatched_sale_count, 1);
        assert_eq!(evidence.unmatched_quantity, 18.0);
        assert!(evidence.slices.is_empty(), "nothing may be invented for it");

        let payload = holding_period_evidence_json(&evidence);
        assert_eq!(payload["status"], "unavailable_no_matched_sales");
        assert_eq!(payload["matched_quantity_pct"], 0.0);
    }

    /// One sale spanning two acquisitions yields two slices, and the gain is
    /// split by quantity because the sale cannot say which purchase earned it.
    #[test]
    fn a_sale_spanning_two_buys_splits_its_gain_by_quantity() {
        let evidence = fifo_holding_periods(
            &[
                buy("FLS:xcse", "2026-08-18T07:01:51Z", 10.0),
                buy("FLS:xcse", "2026-08-28T07:01:52Z", 14.0),
                sell("FLS:xcse", "2026-09-01T15:03:27Z", 24.0, 1_488.0),
            ],
            &[],
        );

        assert_eq!(evidence.matched_sale_count, 1);
        assert_eq!(evidence.slices.len(), 2);
        let total: f64 = evidence
            .slices
            .iter()
            .map(|slice| slice.realised_gain_dkk)
            .sum();
        assert!(
            (total - 1_488.0).abs() < 0.01,
            "allocation must be lossless"
        );
        assert!((evidence.slices[0].realised_gain_dkk - 1_488.0 * 10.0 / 24.0).abs() < 0.01);
        // The older acquisition is consumed first, so it carries the longer hold.
        assert!(evidence.slices[0].days > evidence.slices[1].days);
    }

    /// A sale dated before its acquisition is a data fault. Recording it as a
    /// negative holding period would drag the median toward a number no
    /// position ever held for.
    #[test]
    fn a_sale_dated_before_its_acquisition_contributes_no_slice() {
        let evidence = fifo_holding_periods(
            &[
                buy("XX:xnas", "2026-06-10T10:00:00Z", 5.0),
                sell("XX:xnas", "2026-06-01T10:00:00Z", 5.0, 10.0),
            ],
            &[],
        );
        assert!(evidence.slices.is_empty());
    }

    /// Under the sample floor the aggregate says it is still collecting rather
    /// than presenting a median of a handful of trades as a finding.
    #[test]
    fn a_thin_sample_reports_collecting_rather_than_a_median_to_act_on() {
        let evidence = fifo_holding_periods(
            &[
                buy("AMD:xnas", "2026-01-01T10:00:00Z", 10.0),
                sell("AMD:xnas", "2026-01-11T10:00:00Z", 10.0, 500.0),
            ],
            &[],
        );
        let payload = holding_period_evidence_json(&evidence);
        assert_eq!(payload["status"], "collecting");
        assert_eq!(payload["sample_requirement"], MIN_SAMPLE as i64);
        assert_eq!(payload["winner_median_days_held"], 10.0);
        assert!(payload["loser_median_days_held"].is_null());
    }
}
