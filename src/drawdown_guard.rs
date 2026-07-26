//! Portfolio drawdown guardrail (U3).
//!
//! The Hermes goal contract has advertised `max_drawdown: 0.20` since it was
//! written, but nothing in the runtime ever applied it: drawdown was computed
//! for display and for Hermes evidence packs only. Hermes was therefore
//! reasoning about a risk envelope the system did not have, and every
//! experiment it promoted was judged against a limit no gate would defend.
//!
//! This module makes the number real. It mirrors the monthly-loss circuit
//! breaker deliberately -- a soft band that shrinks the cycle BUY budget and a
//! hard floor that suspends new BUYs, with SELLs never blocked -- because the
//! operator already understands that shape and two guardrails with different
//! semantics would be harder to reason about than two with the same one.
//!
//! The two measure genuinely different things. The monthly-loss breaker asks
//! "how much have we lost this calendar month", which resets on the 1st. This
//! one asks "how far are we below our best", which does not reset and which
//! spans months. A book that fell 15% in May and drifted sideways since is
//! invisible to the monthly breaker and visible here.
//!
//! ## Direction of failure
//!
//! Everywhere else in the risk code the safe default is to fail closed. Here it
//! is the opposite, and the asymmetry is worth stating: failing closed means
//! halting all buying, and the inputs that could be missing -- an empty
//! `portfolio_value_history` after a restore, a position batch that has not
//! loaded yet -- occur precisely when no loss has happened. A guardrail that
//! stops the strategy because it cannot see is a worse outcome than one that
//! stays open while the monthly-loss breaker still covers real losses. So thin
//! or unusable history disables the guardrail, loudly, rather than tripping it.
//!
//! ## External cash flows and re-baselining
//!
//! Drawdown is measured on `total_market_value_dkk`, which includes cash. A
//! withdrawal or a reconciliation reset is arithmetically indistinguishable
//! from a loss: the number drops without anything being lost. This is not
//! hypothetical -- in mid-May 2026 a run of operator cash adjustments and a
//! "Live export reset" moved the book from ~351,000 to ~265,000 DKK, and a peak
//! reaching back across that boundary reads as a 27% drawdown of a portfolio
//! that never fell.
//!
//! Those events are recorded in `trade_ledger` as DEPOSIT / WITHDRAWAL /
//! ADJUSTMENT rows, so the window simply starts after the most recent one. A
//! peak from before a re-baselining describes a different portfolio and has no
//! business governing today's decisions.
//!
//! The operator override remains for anything this misses. It pins the peak it
//! was granted against and lapses once the book makes a new high, so it cannot
//! be left switched on forever.

use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;

use crate::{config::yaml_f64, db::value_f64};

/// Trailing window for the peak. Long enough that a genuine drawdown does not
/// age out before it is recovered, short enough that a peak from a materially
/// different portfolio does not govern today's decisions.
const DEFAULT_LOOKBACK_DAYS: i64 = 90;

const DEFAULT_SOFT_REDUCE_PCT: f64 = 0.10;
const DEFAULT_SOFT_BUY_MULTIPLIER: f64 = 0.50;
const DEFAULT_HALT_PCT: f64 = 0.20;

/// Below this many usable observations the window cannot describe a peak, so
/// the guardrail reports `insufficient_history` instead of guessing.
const MIN_OBSERVATIONS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DrawdownPolicy {
    pub lookback_days: i64,
    pub soft_reduce_pct: f64,
    pub soft_buy_multiplier: f64,
    pub halt_pct: f64,
}

impl DrawdownPolicy {
    pub(crate) fn from_config(config: &YamlValue) -> Self {
        let read = |key: &str, fallback: f64| {
            yaml_f64(config, &["strategy", "capital", key])
                .filter(|value| value.is_finite())
                .unwrap_or(fallback)
        };
        let lookback_days =
            crate::config::yaml_i64(config, &["strategy", "capital", "drawdown_lookback_days"])
                .filter(|days| *days > 0)
                .unwrap_or(DEFAULT_LOOKBACK_DAYS)
                .clamp(1, 3_650);
        Self {
            lookback_days,
            soft_reduce_pct: read("drawdown_soft_reduce_pct", DEFAULT_SOFT_REDUCE_PCT),
            soft_buy_multiplier: read("drawdown_soft_buy_multiplier", DEFAULT_SOFT_BUY_MULTIPLIER)
                .clamp(0.0, 1.0),
            halt_pct: read("drawdown_halt_pct", DEFAULT_HALT_PCT),
        }
    }

    /// A non-positive hard floor disables the whole guardrail, matching how a
    /// non-negative `monthly_loss_halt_dkk` disables the loss breaker.
    fn enabled(&self) -> bool {
        self.halt_pct > 0.0
    }

    /// The soft band only exists when it is a real, less severe floor beneath
    /// an enabled hard one. An inverted or absent pair must never silently
    /// change deployment capacity.
    fn soft_band_valid(&self) -> bool {
        self.enabled() && self.soft_reduce_pct > 0.0 && self.soft_reduce_pct < self.halt_pct
    }
}

/// What the trailing window says about where the portfolio stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawdownObservation {
    pub peak_value_dkk: f64,
    pub peak_at: String,
    pub current_value_dkk: f64,
    pub current_at: String,
    /// Positive magnitude below the peak: 0.12 means 12% below.
    pub drawdown_pct: f64,
    pub observation_count: usize,
}

/// Collapse a snapshot window to one closing value per day, oldest first.
///
/// The peak has to come from daily closes rather than every intraday snapshot,
/// and that is not a smoothing preference -- it is what keeps a bad snapshot
/// from becoming a permanent false peak. On 2026-06-10 five consecutive
/// scheduler snapshots recorded 485,094 DKK with *negative* cash, a
/// mid-settlement double-count on a book that was actually worth about 266,000
/// that day. Taken as a peak it implied a 47% drawdown and would have halted
/// all buying on deploy. The day's close was clean. Drawdown is conventionally
/// a close-to-close measure anyway, so the robust choice is also the standard
/// one, and any glitch that does not survive to a daily close cannot set the
/// high-water mark.
///
/// Rows must be ordered oldest-first. Non-positive and non-finite values are
/// dropped rather than treated as a collapse to zero -- a snapshot written
/// while the position batch was still loading reads as a total loss otherwise,
/// which is the single most dangerous input this function can receive.
fn daily_closes(rows: &[JsonValue]) -> Vec<(String, f64)> {
    let mut closes: Vec<(String, f64)> = Vec::new();
    for row in rows {
        let value = value_f64(row, "total_market_value_dkk");
        let recorded_at = row
            .get("recorded_at")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        if !value.is_finite() || value <= 0.0 || recorded_at.is_empty() {
            continue;
        }
        let day = recorded_at.chars().take(10).collect::<String>();
        match closes.last_mut() {
            // Later row, same day: it replaces the day's close.
            Some((last_day, last_value)) if *last_day == day => *last_value = value,
            _ => closes.push((day, value)),
        }
    }
    closes
}

/// Reduce a window of portfolio snapshots to its peak and where we stand now.
pub(crate) fn observe_drawdown(rows: &[JsonValue]) -> Option<DrawdownObservation> {
    let closes = daily_closes(rows);
    if closes.len() < MIN_OBSERVATIONS {
        return None;
    }
    let (current_at, current_value_dkk) = closes.last().cloned().expect("non-empty");
    let (peak_at, peak_value_dkk) = closes
        .iter()
        .cloned()
        .reduce(|best, candidate| {
            if candidate.1 > best.1 {
                candidate
            } else {
                best
            }
        })
        .expect("non-empty");
    if peak_value_dkk <= 0.0 {
        return None;
    }
    Some(DrawdownObservation {
        // A new high is a zero drawdown, never a negative one.
        drawdown_pct: (1.0 - current_value_dkk / peak_value_dkk).max(0.0),
        peak_value_dkk,
        peak_at,
        current_value_dkk,
        current_at,
        observation_count: closes.len(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawdownTier {
    /// Above the soft band, or the guardrail is off / cannot see.
    Clear,
    /// Between the soft band and the hard floor: BUY budget is reduced.
    SoftReduce,
    /// At or beyond the hard floor: new BUYs are suspended.
    Halt,
}

/// Both floors are inclusive, and reaching one has to be decided in floating
/// point. A book exactly 20% down computes as `1.0 - 96.0/120.0`, which is
/// 0.19999999999999996 -- a plain `>=` leaves the guardrail one representation
/// step below its own documented limit. The tolerance is far smaller than any
/// price move and makes the stated semantics true.
const FLOOR_EPSILON: f64 = 1e-9;

fn classify(drawdown_pct: f64, policy: &DrawdownPolicy) -> DrawdownTier {
    if !policy.enabled() || !drawdown_pct.is_finite() {
        return DrawdownTier::Clear;
    }
    if drawdown_pct >= policy.halt_pct - FLOOR_EPSILON {
        return DrawdownTier::Halt;
    }
    if policy.soft_band_valid() && drawdown_pct >= policy.soft_reduce_pct - FLOOR_EPSILON {
        return DrawdownTier::SoftReduce;
    }
    DrawdownTier::Clear
}

/// The guardrail's verdict for one Trading Manager cycle.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawdownGuard {
    pub policy: DrawdownPolicy,
    pub tier: DrawdownTier,
    /// `disabled`, `insufficient_history`, `clear`, `soft_reduce`, `halt`, or
    /// `overridden`. Distinct from the tier so the operator can tell a
    /// guardrail that is watching and content apart from one that is blind.
    pub status: &'static str,
    pub observation: Option<DrawdownObservation>,
    pub override_active: bool,
    pub override_value: JsonValue,
}

impl DrawdownGuard {
    /// True when new BUYs must be suspended outright.
    pub fn halts_buys(&self) -> bool {
        self.tier == DrawdownTier::Halt
    }

    pub fn reduces_buys(&self) -> bool {
        self.tier == DrawdownTier::SoftReduce
    }

    pub fn drawdown_pct(&self) -> Option<f64> {
        self.observation.as_ref().map(|value| value.drawdown_pct)
    }

    pub fn skip_reason(&self) -> String {
        let drawdown = self.drawdown_pct().unwrap_or(0.0) * 100.0;
        format!(
            "Portfolio drawdown guardrail active: the book is {drawdown:.1}% below its {}-day peak, at or beyond the {:.1}% floor; new BUYs are suspended (SELLs unaffected).",
            self.policy.lookback_days,
            self.policy.halt_pct * 100.0
        )
    }

    pub fn to_json(&self) -> JsonValue {
        let observation = self.observation.as_ref();
        json!({
            "status": self.status,
            "active": self.halts_buys(),
            "soft_reduction_active": self.reduces_buys(),
            "drawdown_pct": observation.map(|value| value.drawdown_pct),
            "peak_value_dkk": observation.map(|value| value.peak_value_dkk),
            "peak_at": observation.map(|value| value.peak_at.clone()),
            "current_value_dkk": observation.map(|value| value.current_value_dkk),
            "current_at": observation.map(|value| value.current_at.clone()),
            "observation_count": observation.map(|value| value.observation_count),
            "lookback_days": self.policy.lookback_days,
            "soft_reduce_pct": self.policy.soft_reduce_pct,
            "soft_buy_multiplier": self.policy.soft_buy_multiplier,
            "halt_pct": self.policy.halt_pct,
            "override_active": self.override_active,
            "override": self.override_value.clone(),
        })
    }
}

/// Whether a saved override still applies to the peak now being measured.
///
/// An override is the operator saying "that peak is not a real high-water mark"
/// -- usually because it included cash that has since been withdrawn. Once the
/// book prints a genuinely higher peak, that judgement no longer describes the
/// number the guardrail is using, so the override lapses on its own. This is
/// what stops a one-off exemption from silently becoming permanent.
fn override_applies(saved: &JsonValue, observed_peak_dkk: f64) -> bool {
    if !saved
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    let Some(granted_peak) = saved.get("peak_value_dkk").and_then(JsonValue::as_f64) else {
        // A grant with no recorded peak has no way to expire, so it is not
        // honoured at all rather than lasting forever.
        return false;
    };
    granted_peak.is_finite() && granted_peak > 0.0 && observed_peak_dkk <= granted_peak * 1.000_1
}

/// Build the cycle verdict from config, the snapshot window, and any operator
/// override. Pure so the whole decision is testable without a database.
pub(crate) fn evaluate_drawdown_guard(
    policy: DrawdownPolicy,
    rows: &[JsonValue],
    saved_override: JsonValue,
) -> DrawdownGuard {
    let observed = observe_drawdown(rows);
    let override_active = observed
        .as_ref()
        .is_some_and(|value| override_applies(&saved_override, value.peak_value_dkk));
    let override_value = {
        let mut value = saved_override;
        if let Some(object) = value.as_object_mut() {
            object.insert("active".to_string(), JsonValue::from(override_active));
        }
        value
    };
    if !policy.enabled() {
        return DrawdownGuard {
            policy,
            tier: DrawdownTier::Clear,
            status: "disabled",
            observation: observed,
            override_active,
            override_value,
        };
    }
    let Some(observation) = observed else {
        return DrawdownGuard {
            policy,
            tier: DrawdownTier::Clear,
            status: "insufficient_history",
            observation: None,
            override_active,
            override_value,
        };
    };
    let tier = classify(observation.drawdown_pct, &policy);
    // An override suppresses the restriction but never the measurement, so the
    // report still records how deep the book actually is.
    if override_active && tier != DrawdownTier::Clear {
        return DrawdownGuard {
            policy,
            tier: DrawdownTier::Clear,
            status: "overridden",
            observation: Some(observation),
            override_active,
            override_value,
        };
    }
    let status = match tier {
        DrawdownTier::Clear => "clear",
        DrawdownTier::SoftReduce => "soft_reduce",
        DrawdownTier::Halt => "halt",
    };
    DrawdownGuard {
        policy,
        tier,
        status,
        observation: Some(observation),
        override_active,
        override_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> DrawdownPolicy {
        DrawdownPolicy {
            lookback_days: 90,
            soft_reduce_pct: 0.10,
            soft_buy_multiplier: 0.5,
            halt_pct: 0.20,
        }
    }

    fn window(values: &[f64]) -> Vec<JsonValue> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                json!({
                    "recorded_at": format!("2026-05-{:02}T12:00:00Z", index + 1),
                    "total_market_value_dkk": value,
                })
            })
            .collect()
    }

    fn no_override() -> JsonValue {
        json!({"enabled": false})
    }

    fn override_at_peak(peak_value_dkk: f64) -> JsonValue {
        json!({"enabled": true, "peak_value_dkk": peak_value_dkk})
    }

    #[test]
    fn drawdown_is_measured_from_the_window_peak_not_the_window_start() {
        // Starting low and peaking mid-window is the case a start-to-end
        // comparison gets wrong: it would report a gain.
        let observation = observe_drawdown(&window(&[100.0, 120.0, 200.0, 160.0, 150.0]))
            .expect("enough observations");
        assert_eq!(observation.peak_value_dkk, 200.0);
        assert_eq!(observation.current_value_dkk, 150.0);
        assert!((observation.drawdown_pct - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_new_high_is_a_zero_drawdown_rather_than_a_negative_one() {
        let observation =
            observe_drawdown(&window(&[100.0, 90.0, 95.0, 110.0, 130.0])).expect("observations");
        assert_eq!(observation.drawdown_pct, 0.0);
        assert_eq!(observation.peak_at, observation.current_at);
    }

    /// The production defect this design exists to survive.
    ///
    /// On 2026-06-10 five consecutive scheduler snapshots recorded 485,094 DKK
    /// with negative cash -- a mid-settlement double-count on a book worth
    /// about 264,000 that day. Peaked off intraday snapshots that is a 47%
    /// drawdown and an immediate halt on every position. The day's close was
    /// clean, so a close-to-close peak never sees it.
    #[test]
    fn an_intraday_spike_that_does_not_survive_to_the_close_cannot_set_the_peak() {
        let rows = vec![
            json!({"recorded_at": "2026-06-08T21:00:00Z", "total_market_value_dkk": 266_232.0}),
            json!({"recorded_at": "2026-06-09T21:00:00Z", "total_market_value_dkk": 266_232.0}),
            json!({"recorded_at": "2026-06-10T16:49:00Z", "total_market_value_dkk": 485_094.0}),
            json!({"recorded_at": "2026-06-10T17:18:32Z", "total_market_value_dkk": 485_094.0}),
            json!({"recorded_at": "2026-06-10T21:00:00Z", "total_market_value_dkk": 264_209.0}),
            json!({"recorded_at": "2026-06-11T21:00:00Z", "total_market_value_dkk": 278_437.0}),
            json!({"recorded_at": "2026-06-12T21:00:00Z", "total_market_value_dkk": 279_840.0}),
        ];

        let observation = observe_drawdown(&rows).expect("observations");
        assert_eq!(observation.peak_value_dkk, 279_840.0);
        assert_eq!(observation.observation_count, 5, "one close per day");
        assert!(observation.drawdown_pct < 0.01);

        let guard = evaluate_drawdown_guard(policy(), &rows, no_override());
        assert_eq!(guard.status, "clear");
    }

    #[test]
    fn the_last_snapshot_of_a_day_is_the_one_that_counts() {
        // Including the current, live-valued row the caller appends: it shares
        // today's date with the last stored snapshot and must win.
        let closes = daily_closes(&[
            json!({"recorded_at": "2026-06-10T09:00:00Z", "total_market_value_dkk": 100.0}),
            json!({"recorded_at": "2026-06-10T17:00:00Z", "total_market_value_dkk": 110.0}),
            json!({"recorded_at": "2026-06-10T21:04:11Z", "total_market_value_dkk": 105.0}),
        ]);
        assert_eq!(closes, vec![("2026-06-10".to_string(), 105.0)]);
    }

    #[test]
    fn a_zero_valued_snapshot_is_dropped_instead_of_reading_as_a_total_loss() {
        // A snapshot written while the position batch was still loading is the
        // most dangerous possible input: taken at face value it is a 100%
        // drawdown and would halt the strategy outright.
        let mut rows = window(&[100.0, 105.0, 110.0, 108.0, 112.0]);
        rows.push(json!({
            "recorded_at": "2026-05-06T12:00:00Z",
            "total_market_value_dkk": 0.0,
        }));
        let observation = observe_drawdown(&rows).expect("observations");
        assert_eq!(observation.current_value_dkk, 112.0);
        assert_eq!(observation.drawdown_pct, 0.0);
    }

    #[test]
    fn thin_history_disables_the_guardrail_rather_than_tripping_it() {
        // Fewer observations than the floor means the window cannot describe a
        // peak. Halting here would stop trading on a fresh database.
        let guard = evaluate_drawdown_guard(policy(), &window(&[100.0, 50.0]), no_override());
        assert_eq!(guard.status, "insufficient_history");
        assert!(!guard.halts_buys());
        assert!(!guard.reduces_buys());
    }

    #[test]
    fn the_soft_band_reduces_and_the_hard_floor_halts() {
        let clear = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 118.0, 115.0]),
            no_override(),
        );
        assert_eq!(clear.status, "clear");

        // 12% below the 120 peak: inside the soft band.
        let soft = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 112.0, 105.6]),
            no_override(),
        );
        assert_eq!(soft.status, "soft_reduce");
        assert!(soft.reduces_buys());
        assert!(!soft.halts_buys());

        // 25% below the peak: past the hard floor.
        let halt = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 100.0, 90.0]),
            no_override(),
        );
        assert_eq!(halt.status, "halt");
        assert!(halt.halts_buys());
    }

    #[test]
    fn the_floor_is_inclusive_so_exactly_at_the_limit_halts() {
        // "max_drawdown: 0.20" in the goal contract reads as a limit, so
        // reaching it must trip rather than sit one tick below forever. 96/120
        // is exactly 20% down in arithmetic and 19.999999999999996% in f64,
        // which is why classify() carries a tolerance.
        let guard = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 100.0, 96.0]),
            no_override(),
        );
        assert_eq!(guard.status, "halt");

        // The tolerance must not swallow a real gap: a hair inside the floor
        // still belongs to the soft band.
        let inside = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 100.0, 96.5]),
            no_override(),
        );
        assert_eq!(inside.status, "soft_reduce");
    }

    #[test]
    fn a_non_positive_hard_floor_disables_the_guardrail() {
        let mut disabled = policy();
        disabled.halt_pct = 0.0;
        let guard = evaluate_drawdown_guard(
            disabled,
            &window(&[100.0, 110.0, 120.0, 40.0, 30.0]),
            no_override(),
        );
        assert_eq!(guard.status, "disabled");
        assert!(!guard.halts_buys());
        // The measurement still happens; only the restriction is off.
        assert!(guard.drawdown_pct().expect("measured") > 0.5);
    }

    #[test]
    fn an_inverted_soft_band_never_silently_changes_deployment() {
        let mut inverted = policy();
        inverted.soft_reduce_pct = 0.30;
        let guard = evaluate_drawdown_guard(
            inverted,
            &window(&[100.0, 110.0, 120.0, 112.0, 105.6]),
            no_override(),
        );
        assert_eq!(guard.status, "clear");
        assert!(!guard.reduces_buys());
    }

    #[test]
    fn an_override_suppresses_the_restriction_but_not_the_measurement() {
        let guard = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 100.0, 90.0]),
            override_at_peak(120.0),
        );
        assert_eq!(guard.status, "overridden");
        assert!(!guard.halts_buys());
        assert!((guard.drawdown_pct().expect("measured") - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_override_lapses_once_the_book_prints_a_higher_peak() {
        // Granted against a 120 peak; the window now peaks at 200, so the
        // operator's judgement about the old high no longer describes the
        // number being enforced and the halt must take effect again.
        let guard = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 120.0, 200.0, 170.0, 150.0]),
            override_at_peak(120.0),
        );
        assert!(!guard.override_active);
        assert_eq!(guard.status, "halt");
    }

    #[test]
    fn an_override_without_a_recorded_peak_is_not_honoured() {
        // Nothing anchors such a grant, so it could never expire. Refusing it
        // is safer than honouring an exemption with no end.
        let guard = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 100.0, 90.0]),
            json!({"enabled": true}),
        );
        assert!(!guard.override_active);
        assert_eq!(guard.status, "halt");
    }

    #[test]
    fn an_override_does_not_invent_a_restriction_when_the_book_is_clear() {
        let guard = evaluate_drawdown_guard(
            policy(),
            &window(&[100.0, 110.0, 120.0, 119.0, 118.0]),
            override_at_peak(120.0),
        );
        assert_eq!(guard.status, "clear");
    }

    #[test]
    fn policy_falls_back_when_config_values_are_missing_or_unusable() {
        let config: YamlValue =
            serde_yaml::from_str("strategy:\n  capital:\n    drawdown_lookback_days: 0\n")
                .expect("parses");
        let policy = DrawdownPolicy::from_config(&config);
        assert_eq!(policy.lookback_days, DEFAULT_LOOKBACK_DAYS);
        assert_eq!(policy.halt_pct, DEFAULT_HALT_PCT);
        assert_eq!(policy.soft_reduce_pct, DEFAULT_SOFT_REDUCE_PCT);
    }

    #[test]
    fn policy_reads_the_configured_floors() {
        let config: YamlValue = serde_yaml::from_str(
            "strategy:\n  capital:\n    drawdown_lookback_days: 45\n    drawdown_soft_reduce_pct: 0.08\n    drawdown_soft_buy_multiplier: 0.25\n    drawdown_halt_pct: 0.15\n",
        )
        .expect("parses");
        let policy = DrawdownPolicy::from_config(&config);
        assert_eq!(policy.lookback_days, 45);
        assert!((policy.soft_reduce_pct - 0.08).abs() < 1e-9);
        assert!((policy.soft_buy_multiplier - 0.25).abs() < 1e-9);
        assert!((policy.halt_pct - 0.15).abs() < 1e-9);
    }
}
