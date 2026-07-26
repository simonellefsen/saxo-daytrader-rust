//! Automatic protective-stop maintenance (U1 slice 3b).
//!
//! Slice 3a made a stop a real `execution_orders` row, so a fill is finally
//! visible. This module is the other half of the operator requirement: stops
//! must appear without anyone clicking, follow the position size through every
//! trade, and ratchet upward as a holding appreciates.
//!
//! The design is deliberately *declarative* rather than event-driven. Nothing
//! here hooks a particular fill. Each cycle the sweep compares the desired
//! protective state against the actual one and closes the gap. That covers a
//! new BUY fill, a partial exit that left a residual holding, a stop released
//! for a discretionary sell, and a failed placement on an earlier cycle -- all
//! through one path, with no event that can be missed while the process is
//! restarting. A missed event is silent; a missed reconciliation is corrected
//! on the next cycle.

use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use tracing::{info, warn};

use crate::{
    api::{StopPlacementOutcome, place_one_protective_stop},
    config::yaml_f64,
    db::value_f64,
    saxo_order::cancel_protective_stop_for_replacement,
    state::AppState,
};

/// Trimmed string field from a JSON row.
fn row_text(row: &JsonValue, key: &str) -> String {
    row.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Saxo permits one order per second per session.
const PLACEMENT_SPACING_MS: u64 = 1_100;

/// A systemic fault -- bad indicator data, a broken position snapshot -- must
/// not be able to turn into an unbounded run of broker orders. The sweep is
/// self-healing across cycles, so a low bound costs only time.
const MAX_ACTIONS_PER_CYCLE: usize = 5;

const DEFAULT_STOP_LOSS_ATR_MULTIPLE: f64 = 2.0;
const DEFAULT_TRAIL_STOP_ATR_MULTIPLE: f64 = 1.25;

/// How far the trailing stop must improve before the sweep will cancel and
/// replace a resting order. Without it, every tick of ATR drift would rewrite
/// twelve broker orders a day for no protective gain, and each rewrite has a
/// real cost: a window where the position carries no stop at all.
const DEFAULT_MIN_RATCHET_ATR_FRACTION: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StopPolicy {
    pub stop_loss_atr_multiple: f64,
    pub trail_stop_atr_multiple: f64,
    pub min_ratchet_atr_fraction: f64,
}

impl StopPolicy {
    pub(crate) fn from_config(config: &YamlValue) -> Self {
        // A non-positive multiple would put the stop at or above the last
        // close, which fires the moment the market opens and sells the position
        // at market. Config is not trusted to be sane here.
        let positive_or = |path: &[&str], fallback: f64| {
            yaml_f64(config, path)
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(fallback)
        };
        Self {
            stop_loss_atr_multiple: positive_or(
                &["strategy", "ladder", "stop_loss_atr_multiple"],
                DEFAULT_STOP_LOSS_ATR_MULTIPLE,
            ),
            trail_stop_atr_multiple: positive_or(
                &["strategy", "ladder", "trail_stop_atr_multiple"],
                DEFAULT_TRAIL_STOP_ATR_MULTIPLE,
            ),
            min_ratchet_atr_fraction: positive_or(
                &["strategy", "ladder", "min_ratchet_atr_fraction"],
                DEFAULT_MIN_RATCHET_ATR_FRACTION,
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExistingStop {
    pub broker_order_id: String,
    pub quantity: f64,
    pub stop_price: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StopTarget {
    pub symbol: String,
    pub position_quantity: f64,
    pub existing: Option<ExistingStop>,
    pub close: f64,
    pub atr14: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StopAction {
    /// Nothing to do, or nothing that can be done safely.
    Hold { reason: &'static str },
    Place {
        quantity: f64,
        stop_price: f64,
        reason: &'static str,
    },
    /// Cancel the resting stop and place a corrected one. Saxo permits a single
    /// resting sell per holding, so this is the only way to change a stop.
    Replace {
        broker_order_id: String,
        quantity: f64,
        stop_price: f64,
        reason: &'static str,
    },
}

/// Decides what a single position's protective stop should be.
///
/// Two invariants hold for every value this returns, and both are enforced here
/// rather than at the call site, because this is the only place that computes a
/// stop price:
///
/// 1. **A stop never moves down.** A replacement price is always at least the
///    resting price. The resting order is therefore its own high-water mark and
///    the ratchet needs no separate stored peak to be monotonic.
/// 2. **A stop is always below the last close.** A stop at or above the market
///    triggers immediately and turns protection into an unplanned market sell.
///
/// It fails closed: unusable indicator data yields `Hold`, never a guessed
/// level. An unprotected position is visible in the coverage audit; a stop
/// placed at a fabricated price is not.
pub(crate) fn decide_stop_action(target: &StopTarget, policy: &StopPolicy) -> StopAction {
    if !target.close.is_finite() || target.close <= 0.0 {
        return StopAction::Hold {
            reason: "no_usable_close",
        };
    }
    if !target.atr14.is_finite() || target.atr14 <= 0.0 {
        return StopAction::Hold {
            reason: "no_usable_atr",
        };
    }
    let quantity = target.position_quantity.floor();
    if quantity < 1.0 {
        return StopAction::Hold {
            reason: "no_whole_share_position",
        };
    }

    let initial = target.close - target.atr14 * policy.stop_loss_atr_multiple;
    let trail = target.close - target.atr14 * policy.trail_stop_atr_multiple;

    let Some(existing) = target.existing.as_ref() else {
        return match usable_stop_price(initial, target.close) {
            Some(stop_price) => StopAction::Place {
                quantity,
                stop_price,
                reason: "position_has_no_resting_stop",
            },
            None => StopAction::Hold {
                reason: "computed_stop_is_not_below_the_last_close",
            },
        };
    };

    let quantity_mismatch = (existing.quantity - quantity).abs() > 1e-6;
    // Never lower a stop. On a quantity change the resting level is kept unless
    // the trail has already earned a higher one, so re-sizing after a partial
    // exit cannot quietly give back protection the position already had.
    let ratcheted = trail.max(existing.stop_price);
    let ratchet_step = target.atr14 * policy.min_ratchet_atr_fraction;
    let ratchet_due = trail > existing.stop_price + ratchet_step;

    if !quantity_mismatch && !ratchet_due {
        return StopAction::Hold {
            reason: "resting_stop_matches_position_and_level",
        };
    }
    let reason = if quantity_mismatch && ratchet_due {
        "position_size_changed_and_trail_advanced"
    } else if quantity_mismatch {
        "position_size_changed"
    } else {
        "trail_advanced"
    };
    match usable_stop_price(ratcheted, target.close) {
        Some(stop_price) => StopAction::Replace {
            broker_order_id: existing.broker_order_id.clone(),
            quantity,
            stop_price,
            reason,
        },
        // Refusing here leaves the existing stop resting, which is the safe
        // outcome: the position keeps the protection it already has.
        None => StopAction::Hold {
            reason: "replacement_stop_would_not_sit_below_the_last_close",
        },
    }
}

fn usable_stop_price(candidate: f64, close: f64) -> Option<f64> {
    (candidate.is_finite() && candidate > 0.0 && candidate < close).then_some(candidate)
}

/// Reads the desired-versus-actual picture for every held position.
async fn stop_targets(state: &AppState) -> anyhow::Result<Vec<StopTarget>> {
    let positions = state
        .select_json(
            "SELECT symbol, quantity FROM broker_position_snapshots
             WHERE quantity > 0 ORDER BY symbol ASC",
        )
        .await?;
    // Only a broker-confirmed stop counts as existing protection. A submitted
    // or ambiguous one must not be treated as replaceable: cancelling an order
    // whose state is unknown is how a position ends up with none.
    let resting = state
        .select_json(
            "SELECT symbol, quantity, stop_price_local, broker_order_id
             FROM execution_orders
             WHERE action = 'SELL'
               AND COALESCE(strategy_type, '') = 'protective_stop'
               AND status = 'broker_working'
               AND broker_order_id IS NOT NULL
               AND broker_order_id <> ''
             ORDER BY id DESC",
        )
        .await?;
    let indicators = state
        .select_json(
            "SELECT symbol, close, atr14 FROM daily_indicator_signals
             WHERE close IS NOT NULL AND atr14 IS NOT NULL
             ORDER BY run_date DESC, id DESC LIMIT 600",
        )
        .await
        .unwrap_or_default();

    let key = |row: &JsonValue| row_text(row, "symbol").trim().to_ascii_uppercase();
    let mut targets = Vec::new();
    for position in &positions {
        let symbol = row_text(position, "symbol").trim().to_string();
        if symbol.is_empty() {
            continue;
        }
        let upper = symbol.to_ascii_uppercase();
        // Rows arrive newest-first, so the first match per symbol wins.
        let indicator = indicators.iter().find(|row| key(row) == upper);
        let stops = resting
            .iter()
            .filter(|row| key(row) == upper)
            .collect::<Vec<_>>();
        // More than one resting stop should be impossible -- Saxo permits a
        // single sell per holding -- so treat it as a state this sweep must not
        // act on rather than guessing which order to cancel.
        if stops.len() > 1 {
            warn!(
                symbol,
                count = stops.len(),
                "skipping automatic stop maintenance: more than one resting protective stop"
            );
            continue;
        }
        targets.push(StopTarget {
            symbol,
            position_quantity: value_f64(position, "quantity"),
            existing: stops.first().map(|row| ExistingStop {
                broker_order_id: row_text(row, "broker_order_id").trim().to_string(),
                quantity: value_f64(row, "quantity"),
                stop_price: value_f64(row, "stop_price_local"),
            }),
            close: indicator.map(|row| value_f64(row, "close")).unwrap_or(0.0),
            atr14: indicator.map(|row| value_f64(row, "atr14")).unwrap_or(0.0),
        });
    }
    Ok(targets)
}

/// True when the symbol's exchange will accept an order right now. A stop
/// rejected because the market is shut is indistinguishable at the broker from
/// a stop rejected for a real reason, and the sweep halts on failure, so an
/// out-of-hours attempt would stall protection for every symbol behind it.
fn exchange_is_tradable(state: &AppState, symbol: &str) -> bool {
    let exchange = crate::state::exchange_code_for(symbol).to_ascii_lowercase();
    state.market_exchange_rows().iter().any(|row| {
        row_text(row, "code").eq_ignore_ascii_case(&exchange)
            && row
                .get("is_tradable")
                .or_else(|| row.get("is_open"))
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
    })
}

/// Brings every held position's protective stop to its desired size and level.
///
/// This is the first path in the runtime that places a broker order without an
/// operator confirming it, so the guards matter more than the mechanism. It is
/// off unless `strategy.ladder.submit_stop_loss_after_fill` is set; it is
/// SIM-only, inherited from the verified-SIM-session check every protective
/// stop placement already runs through; it acts only on exchanges currently
/// accepting orders; it is bounded per cycle; and it halts on the first
/// failure rather than working down a list of positions repeating a mistake.
pub(crate) async fn run_automatic_protective_stop_sweep(state: &AppState) -> JsonValue {
    if !crate::config::yaml_bool(
        &state.config,
        &["strategy", "ladder", "submit_stop_loss_after_fill"],
    )
    .unwrap_or(false)
    {
        return json!({
            "status": "disabled",
            "reason": "strategy.ladder.submit_stop_loss_after_fill is not enabled"
        });
    }
    let policy = StopPolicy::from_config(&state.config);
    let targets = match stop_targets(state).await {
        Ok(targets) => targets,
        Err(err) => {
            warn!("automatic protective-stop sweep could not read state: {err:#}");
            return json!({"status": "error", "error": err.to_string()});
        }
    };

    let mut placed = 0usize;
    let mut replaced = 0usize;
    let mut acted = 0usize;
    let mut halted = None;
    let mut skipped_closed = 0usize;
    let mut holds = Vec::new();
    for target in &targets {
        let action = decide_stop_action(target, &policy);
        let StopAction::Hold { reason } = &action else {
            if acted >= MAX_ACTIONS_PER_CYCLE {
                halted = Some("per_cycle_action_limit_reached".to_string());
                break;
            }
            if !exchange_is_tradable(state, &target.symbol) {
                skipped_closed += 1;
                continue;
            }
            if acted > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(PLACEMENT_SPACING_MS)).await;
            }
            acted += 1;
            match run_stop_action(state, target, &action).await {
                Ok(true) => match action {
                    StopAction::Place { .. } => placed += 1,
                    StopAction::Replace { .. } => replaced += 1,
                    StopAction::Hold { .. } => unreachable!(),
                },
                Ok(false) => {
                    halted = Some(format!("placement_not_confirmed_for_{}", target.symbol));
                    break;
                }
                Err(err) => {
                    warn!(
                        symbol = target.symbol,
                        "automatic protective-stop action failed; halting sweep: {err:#}"
                    );
                    halted = Some(format!("{}: {err}", target.symbol));
                    break;
                }
            }
            continue;
        };
        holds.push(json!({"symbol": target.symbol, "reason": reason}));
    }

    if placed > 0 || replaced > 0 {
        info!(
            placed,
            replaced, "automatic protective-stop sweep adjusted broker-hosted stops"
        );
    }
    json!({
        "status": if halted.is_some() { "halted" } else { "ok" },
        "considered": targets.len(),
        "placed": placed,
        "replaced": replaced,
        "skipped_market_closed": skipped_closed,
        "halted_reason": halted,
        "held": holds
    })
}

/// Performs one decided action. Returns `Ok(false)` when the broker did not
/// confirm the placement, which halts the sweep without treating the position
/// as protected.
async fn run_stop_action(
    state: &AppState,
    target: &StopTarget,
    action: &StopAction,
) -> anyhow::Result<bool> {
    let (quantity, stop_price, reason) = match action {
        StopAction::Hold { .. } => return Ok(true),
        StopAction::Place {
            quantity,
            stop_price,
            reason,
        } => (*quantity, *stop_price, *reason),
        StopAction::Replace {
            broker_order_id,
            quantity,
            stop_price,
            reason,
        } => {
            // Saxo allows one resting sell per holding, so the old stop has to
            // go before the new one can exist. That leaves a genuine unprotected
            // window; it is kept as short as possible and the replacement is
            // attempted immediately. If the placement below fails, the next
            // sweep sees an unprotected position and places a fresh stop.
            cancel_protective_stop_for_replacement(state, &target.symbol, broker_order_id).await?;
            warn!(
                symbol = target.symbol,
                broker_order_id,
                reason,
                "cancelled a resting protective stop to replace it; the position is unprotected until the replacement is confirmed"
            );
            (*quantity, *stop_price, *reason)
        }
    };
    let outcome = place_one_protective_stop(
        state,
        &target.symbol,
        quantity,
        stop_price,
        "automatic_protective_stop_sweep",
    )
    .await;
    match outcome {
        StopPlacementOutcome::Placed {
            test_id,
            broker_order_id,
        } => {
            info!(
                symbol = target.symbol,
                quantity,
                stop_price,
                reason,
                test_id,
                broker_order_id,
                "automatic protective stop placed"
            );
            Ok(true)
        }
        other => {
            warn!(
                symbol = target.symbol,
                outcome = other.label(),
                "automatic protective stop was not confirmed"
            );
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> StopPolicy {
        StopPolicy {
            stop_loss_atr_multiple: 2.0,
            trail_stop_atr_multiple: 1.25,
            min_ratchet_atr_fraction: 0.25,
        }
    }

    fn target(quantity: f64, existing: Option<ExistingStop>, close: f64, atr14: f64) -> StopTarget {
        StopTarget {
            symbol: "AMD:xnas".to_string(),
            position_quantity: quantity,
            existing,
            close,
            atr14,
        }
    }

    fn resting(quantity: f64, stop_price: f64) -> Option<ExistingStop> {
        Some(ExistingStop {
            broker_order_id: "5100".to_string(),
            quantity,
            stop_price,
        })
    }

    #[test]
    fn an_unprotected_position_gets_a_stop_at_the_wider_initial_multiple() {
        let action = decide_stop_action(&target(7.0, None, 500.0, 10.0), &policy());
        assert_eq!(
            action,
            StopAction::Place {
                quantity: 7.0,
                stop_price: 480.0,
                reason: "position_has_no_resting_stop"
            },
            "a new stop uses stop_loss_atr_multiple (2.0), not the tighter trail multiple"
        );
    }

    #[test]
    fn a_resting_stop_is_left_alone_until_the_trail_advances_far_enough() {
        // Trail candidate is 500 - 1.25*10 = 487.5. The ratchet step is
        // 0.25*10 = 2.5, so a stop already at 486 has not earned a rewrite:
        // 487.5 is not more than 486 + 2.5.
        assert_eq!(
            decide_stop_action(&target(7.0, resting(7.0, 486.0), 500.0, 10.0), &policy()),
            StopAction::Hold {
                reason: "resting_stop_matches_position_and_level"
            },
            "a sub-threshold improvement must not cancel and replace a live broker order"
        );
        assert_eq!(
            decide_stop_action(&target(7.0, resting(7.0, 480.0), 500.0, 10.0), &policy()),
            StopAction::Replace {
                broker_order_id: "5100".to_string(),
                quantity: 7.0,
                stop_price: 487.5,
                reason: "trail_advanced"
            }
        );
    }

    #[test]
    fn the_ratchet_is_monotonic_even_when_the_price_falls() {
        // Close has dropped, so the trail candidate (400 - 12.5 = 387.5) is far
        // below the resting stop. A stop that follows price down is not a stop.
        let action = decide_stop_action(&target(7.0, resting(7.0, 487.5), 400.0, 10.0), &policy());
        assert_eq!(
            action,
            StopAction::Hold {
                reason: "resting_stop_matches_position_and_level"
            },
            "a protective stop may only ever move up for a long position"
        );
    }

    #[test]
    fn a_size_change_rewrites_the_stop_without_giving_back_level() {
        // A partial exit leaves 4 of 7 shares. The stop must be re-sized, and
        // the level may only move up: the trail candidate (487.5) is taken over
        // the resting 486 rather than the other way round.
        let action = decide_stop_action(&target(4.0, resting(7.0, 486.0), 500.0, 10.0), &policy());
        assert_eq!(
            action,
            StopAction::Replace {
                broker_order_id: "5100".to_string(),
                quantity: 4.0,
                stop_price: 487.5,
                reason: "position_size_changed"
            }
        );
        // A follow-on buy is the mirror case: the stop is undersized and must
        // grow to cover the whole holding.
        let action = decide_stop_action(&target(11.0, resting(7.0, 480.0), 500.0, 10.0), &policy());
        assert_eq!(
            action,
            StopAction::Replace {
                broker_order_id: "5100".to_string(),
                quantity: 11.0,
                stop_price: 487.5,
                reason: "position_size_changed_and_trail_advanced"
            }
        );
    }

    #[test]
    fn unusable_indicator_data_never_produces_a_guessed_stop() {
        for (close, atr, expected) in [
            (0.0, 10.0, "no_usable_close"),
            (f64::NAN, 10.0, "no_usable_close"),
            (500.0, 0.0, "no_usable_atr"),
            (500.0, f64::NAN, "no_usable_atr"),
        ] {
            assert_eq!(
                decide_stop_action(&target(7.0, None, close, atr), &policy()),
                StopAction::Hold { reason: expected },
                "an unprotected position is visible in the audit; a fabricated stop level is not"
            );
        }
        // A volatile instrument can compute a negative level. Refusing is the
        // only safe answer -- a stop at or above the close sells at market.
        assert_eq!(
            decide_stop_action(&target(7.0, None, 10.0, 8.0), &policy()),
            StopAction::Hold {
                reason: "computed_stop_is_not_below_the_last_close"
            }
        );
    }

    #[test]
    fn a_stop_already_above_the_close_is_left_alone_rather_than_rewritten() {
        // This state is self-contradictory -- a stop at 487.5 cannot still be
        // resting with the close at 400, it would have triggered -- so it means
        // stale or inconsistent data. Re-placing at the retained level would
        // sell the position at market the moment the order was accepted, and
        // re-placing lower would give back protection. Holding leaves the
        // existing order to do its job and leaves the mismatch visible in the
        // coverage audit, which is the only outcome that cannot make things
        // worse.
        assert_eq!(
            decide_stop_action(&target(4.0, resting(7.0, 487.5), 400.0, 10.0), &policy()),
            StopAction::Hold {
                reason: "replacement_stop_would_not_sit_below_the_last_close"
            }
        );
    }

    #[test]
    fn a_fractional_or_empty_position_is_never_acted_on() {
        assert_eq!(
            decide_stop_action(&target(0.4, None, 500.0, 10.0), &policy()),
            StopAction::Hold {
                reason: "no_whole_share_position"
            }
        );
    }

    #[test]
    fn a_non_positive_configured_multiple_falls_back_instead_of_selling_at_market() {
        // A zero multiple would put the stop exactly at the last close, which
        // fires immediately. Config is not trusted to be sane.
        let config: YamlValue = serde_yaml::from_str(
            "strategy:\n  ladder:\n    stop_loss_atr_multiple: 0\n    trail_stop_atr_multiple: -1\n",
        )
        .unwrap();
        let policy = StopPolicy::from_config(&config);
        assert_eq!(
            policy.stop_loss_atr_multiple,
            DEFAULT_STOP_LOSS_ATR_MULTIPLE
        );
        assert_eq!(
            policy.trail_stop_atr_multiple,
            DEFAULT_TRAIL_STOP_ATR_MULTIPLE
        );
    }
}
