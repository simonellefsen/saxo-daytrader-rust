//! Declared exit rules replayed over every entry's first sessions.
//!
//! Built 2026-10-18 after the edge review found that 96 of the 100 sales since
//! the bootstrap were protective stops, and that lots stopped within 5 or 10
//! sessions sold 1.5-1.7 points below where the same lot closed at that
//! horizon. `entry_evaluation` measures what happened after each entry and
//! deliberately simulates no exit policy; this module is that simulation, with
//! the same discipline:
//!
//! **The rule set is declared before any result is computed, and versioned.**
//! `RULES` was written down and frozen at `FROZEN_AT`. The review that
//! motivated it had already seen the current book, so those entries are
//! in-sample whatever this module says about them; only entries filled after
//! the freeze are out-of-sample, and they are reported as their own sample.
//! Changing a rule means a new `RULESET_VERSION`, never an edit in place.
//!
//! **Every rule is measured over the same window, and an entry counts only
//! once the whole window has elapsed.** Counting entries whose stop has fired
//! while leaving out those still running would over-represent early exits,
//! which for a stop are the losers. A rule that exits early is held to the
//! same denominator as one that holds.
//!
//! **Rules are compared on the same entries.** Each summary reports the mean
//! difference against holding to the end of the window, paired by entry, so
//! the comparison is between policies and not between samples.
//!
//! **It is close-basis.** Only daily closes are stored, so a stop, trail or
//! target is tested against each session's close and fills at that close. A
//! resting stop that would have triggered intraday on a day that recovered is
//! not modelled. That bias is the same for every rule with a stop, which is
//! why the comparison between rules is more trustworthy than any one level.

use std::collections::BTreeMap;

use crate::models::{
    ExitEvaluationRowPayload, ExitRuleOutcomePayload, ExitRulePayload, ExitRuleSummaryPayload,
};

/// Bumped whenever a rule, the window or the cost model changes meaning.
pub(crate) const RULESET_VERSION: &str = "exit-v1-2026-10-18";

/// When `RULES` was frozen. Entries filled after this are out-of-sample.
pub(crate) const FROZEN_AT: &str = "2026-10-18T12:00:00Z";

/// Trading sessions every rule is replayed over. Twenty is the longest
/// horizon `entry_evaluation` declared, so holding to the end of the window
/// is exactly that evaluation's 20-session return.
pub(crate) const WINDOW_SESSIONS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rule {
    /// Close-basis stop a fixed fraction below the fill, never moved.
    FixedStop { fraction: f64 },
    /// Stop `multiple` x ATR14 below the highest close since the fill,
    /// ratcheting up only.
    AtrTrail { multiple: f64 },
    /// Stop and take-profit `multiple` x ATR14 either side of the fill.
    AtrBracket { multiple: f64 },
    /// Sell at the close of a fixed session.
    TimeExit { session: usize },
    /// Hold to the end of the window: the baseline.
    Hold,
}

pub(crate) struct RuleSpec {
    pub key: &'static str,
    pub rule: Rule,
    pub description: &'static str,
}

pub(crate) const HOLD_KEY: &str = "hold_20";

/// The declared rule set. Frozen at `FROZEN_AT`; see the module notes.
pub(crate) const RULES: &[RuleSpec] = &[
    RuleSpec {
        key: "fixed_stop_6pct",
        rule: Rule::FixedStop { fraction: 0.06 },
        description: "Stop 6% below the fill, never moved: the median initial stop distance of \
                      live entries since the bootstrap, without the live trailing ladder. It \
                      approximates the current policy; it does not reproduce it.",
    },
    RuleSpec {
        key: "fixed_stop_10pct",
        rule: Rule::FixedStop { fraction: 0.10 },
        description: "Stop 10% below the fill, never moved.",
    },
    RuleSpec {
        key: "atr_trail_3x",
        rule: Rule::AtrTrail { multiple: 3.0 },
        description: "Trailing stop 3 x ATR14 below the highest close since the fill, \
                      ratcheting up only.",
    },
    RuleSpec {
        key: "atr_bracket_2x",
        rule: Rule::AtrBracket { multiple: 2.0 },
        description: "Stop 2 x ATR14 below the fill and take-profit 2 x ATR14 above it, \
                      whichever close comes first. The live runtime has no take-profit exit.",
    },
    RuleSpec {
        key: "time_exit_10",
        rule: Rule::TimeExit { session: 10 },
        description: "Sell at the close of session 10, no stop.",
    },
    RuleSpec {
        key: HOLD_KEY,
        rule: Rule::Hold,
        description: "Hold to the close of session 20, no stop: the baseline every rule is \
                      compared with.",
    },
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Exit {
    /// 1-based trading session of the exit.
    pub session: usize,
    pub reason: &'static str,
    pub date: String,
    pub close: f64,
}

/// Replays one rule over the window. `None` when the rule cannot be judged:
/// the window has not fully elapsed, or the rule needs an ATR the entry lacks.
pub(crate) fn replay(
    rule: Rule,
    fill_price: f64,
    atr: Option<f64>,
    sessions: &[(String, f64)],
) -> Option<Exit> {
    if !(fill_price.is_finite() && fill_price > 0.0) || sessions.len() < WINDOW_SESSIONS {
        return None;
    }
    let window = &sessions[..WINDOW_SESSIONS];
    let exit_at = |index: usize, reason: &'static str| {
        let (date, close) = window[index].clone();
        Some(Exit {
            session: index + 1,
            reason,
            date,
            close,
        })
    };
    let usable_atr = atr.filter(|value| value.is_finite() && *value > 0.0);
    match rule {
        Rule::FixedStop { fraction } => {
            let stop = fill_price * (1.0 - fraction);
            match window.iter().position(|(_, close)| *close <= stop) {
                Some(index) => exit_at(index, "stop"),
                None => exit_at(WINDOW_SESSIONS - 1, "window_end"),
            }
        }
        Rule::AtrTrail { multiple } => {
            let distance = multiple * usable_atr?;
            let mut stop = fill_price - distance;
            for (index, (_, close)) in window.iter().enumerate() {
                // Tested against the stop as it stood before this close, then
                // ratcheted: a close cannot raise the stop it is judged by.
                if *close <= stop {
                    return exit_at(index, "trail");
                }
                stop = stop.max(close - distance);
            }
            exit_at(WINDOW_SESSIONS - 1, "window_end")
        }
        Rule::AtrBracket { multiple } => {
            let distance = multiple * usable_atr?;
            for (index, (_, close)) in window.iter().enumerate() {
                if *close <= fill_price - distance {
                    return exit_at(index, "stop");
                }
                if *close >= fill_price + distance {
                    return exit_at(index, "target");
                }
            }
            exit_at(WINDOW_SESSIONS - 1, "window_end")
        }
        Rule::TimeExit { session } => exit_at(session.clamp(1, WINDOW_SESSIONS) - 1, "time"),
        Rule::Hold => exit_at(WINDOW_SESSIONS - 1, "window_end"),
    }
}

/// Which sample an entry belongs to.
pub(crate) fn sample_for(filled_at: &str) -> &'static str {
    if filled_at > FROZEN_AT {
        "out_of_sample"
    } else if crate::entry_evaluation::epoch_for(filled_at) == "a_pre_bootstrap" {
        "pre_bootstrap"
    } else {
        "current_book"
    }
}

pub(crate) struct Entry<'a> {
    pub symbol: &'a str,
    pub filled_at: &'a str,
    pub fill_price_local: f64,
    pub atr14_before_fill: Option<f64>,
    /// The symbol's trading sessions strictly after the fill date, oldest first.
    pub sessions: &'a [(String, f64)],
    pub sessions_elapsed: i64,
    /// Modelled round-trip commission as a fraction of notional.
    pub round_trip_cost: f64,
}

/// Replays every declared rule over one entry.
pub(crate) fn evaluate_entry(entry: &Entry<'_>) -> ExitEvaluationRowPayload {
    let window = WINDOW_SESSIONS as i64;
    let outcomes: Vec<ExitRuleOutcomePayload> = RULES
        .iter()
        .filter_map(|spec| {
            let exit = replay(
                spec.rule,
                entry.fill_price_local,
                entry.atr14_before_fill,
                entry.sessions,
            )?;
            let gross = exit.close / entry.fill_price_local - 1.0;
            Some(ExitRuleOutcomePayload {
                rule: spec.key.to_string(),
                exit_session: exit.session as i64,
                exit_reason: exit.reason.to_string(),
                exit_date: exit.date,
                gross_return: gross,
                net_return: gross - entry.round_trip_cost,
            })
        })
        .collect();
    let status = if !outcomes.is_empty() {
        "resolved"
    } else if entry.sessions_elapsed < window {
        "pending"
    } else {
        "unavailable"
    };
    ExitEvaluationRowPayload {
        symbol: entry.symbol.to_string(),
        filled_at: entry.filled_at.to_string(),
        epoch: crate::entry_evaluation::epoch_for(entry.filled_at).to_string(),
        fill_price_local: entry.fill_price_local,
        atr14_before_fill: entry.atr14_before_fill,
        sessions_elapsed: entry.sessions_elapsed,
        status: status.to_string(),
        sample: sample_for(entry.filled_at).to_string(),
        outcomes,
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// Per rule and sample, with the paired comparison against holding.
///
/// Pre-bootstrap entries are another account's book and are not summarised.
pub(crate) fn summarize(entries: &[ExitEvaluationRowPayload]) -> Vec<ExitRuleSummaryPayload> {
    let mut summaries = Vec::new();
    for sample in ["current_book", "out_of_sample"] {
        let rows: Vec<&ExitEvaluationRowPayload> = entries
            .iter()
            .filter(|row| row.sample == sample && row.status == "resolved")
            .collect();
        for spec in RULES {
            let mut gross = Vec::new();
            let mut net = Vec::new();
            let mut sessions = Vec::new();
            let mut differences = Vec::new();
            let mut exit_reasons: BTreeMap<String, i64> = BTreeMap::new();
            for row in &rows {
                let Some(outcome) = row.outcomes.iter().find(|o| o.rule == spec.key) else {
                    continue;
                };
                gross.push(outcome.gross_return);
                net.push(outcome.net_return);
                sessions.push(outcome.exit_session as f64);
                *exit_reasons.entry(outcome.exit_reason.clone()).or_default() += 1;
                if let Some(hold) = row.outcomes.iter().find(|o| o.rule == HOLD_KEY) {
                    differences.push(outcome.gross_return - hold.gross_return);
                }
            }
            let mean_difference = mean(&differences);
            let half_width = (differences.len() >= 2).then(|| {
                let m = mean_difference.unwrap_or_default();
                let variance = differences.iter().map(|d| (d - m).powi(2)).sum::<f64>()
                    / (differences.len() - 1) as f64;
                1.96 * (variance / differences.len() as f64).sqrt()
            });
            summaries.push(ExitRuleSummaryPayload {
                rule: spec.key.to_string(),
                sample: sample.to_string(),
                resolved: net.len() as i64,
                mean_gross_return: mean(&gross),
                mean_net_return: mean(&net),
                net_positive_rate: (!net.is_empty())
                    .then(|| net.iter().filter(|v| **v > 0.0).count() as f64 / net.len() as f64),
                mean_exit_session: mean(&sessions),
                exit_reasons,
                mean_difference_vs_hold: mean_difference,
                difference_interval_low: mean_difference.zip(half_width).map(|(m, h)| m - h),
                difference_interval_high: mean_difference.zip(half_width).map(|(m, h)| m + h),
                better_than_hold_count: differences.iter().filter(|d| **d > 0.0).count() as i64,
            });
        }
    }
    summaries
}

pub(crate) fn rules() -> Vec<ExitRulePayload> {
    RULES
        .iter()
        .map(|spec| ExitRulePayload {
            key: spec.key.to_string(),
            description: spec.description.to_string(),
        })
        .collect()
}

pub(crate) fn definitions() -> BTreeMap<String, String> {
    [
        (
            "window",
            "every rule is replayed over the same first 20 trading sessions after the fill; an \
             entry counts only once its whole window has elapsed, so early exits cannot be \
             over-represented",
        ),
        (
            "close_basis",
            "stops, trails and targets are tested against each session's close and fill at that \
             close; intraday highs and lows are not stored, so a resting stop that would have \
             triggered intraday on a day that recovered is not modelled",
        ),
        (
            "atr",
            "ATR14 from the last indicator row before the fill day, so no rule sees the fill \
             day's own range; ATR rules are skipped for an entry without one",
        ),
        (
            "costs",
            "the same modelled round-trip commission as the entry evaluation, identical for every \
             rule, so a difference between rules is a difference in gross",
        ),
        (
            "difference_vs_hold",
            "mean of (rule return - hold_20 return) over the entries both resolved, with a \
             normal-approximation 95% interval",
        ),
        (
            "out_of_sample",
            "the rule set was frozen at 2026-10-18T12:00:00Z, after the edge review had seen the \
             current book; only entries filled after that are out-of-sample",
        ),
        (
            "not_a_backtest_of_live",
            "the live book exits through a trailing stop ladder and decided SELLs; no rule here \
             reproduces that, and none of these figures is a realised result",
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(closes: &[f64]) -> Vec<(String, f64)> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| (format!("2026-09-{:02}", index + 1), *close))
            .collect()
    }

    /// A window of `n` closes at `level`, with `overrides` at 1-based sessions.
    fn flat(level: f64, overrides: &[(usize, f64)]) -> Vec<(String, f64)> {
        let mut closes = vec![level; WINDOW_SESSIONS];
        for (session, close) in overrides {
            closes[session - 1] = *close;
        }
        series(&closes)
    }

    #[test]
    fn a_fixed_stop_exits_at_the_first_close_at_or_below_it() {
        // Closes sit off the stop levels: 100 x (1 - 0.06) is not exactly 94
        // in floating point, and the test is about order, not rounding.
        let sessions = flat(100.0, &[(4, 95.0), (6, 93.5), (7, 89.5)]);
        let exit = replay(Rule::FixedStop { fraction: 0.06 }, 100.0, None, &sessions).unwrap();
        assert_eq!((exit.session, exit.reason, exit.close), (6, "stop", 93.5));

        let untouched = replay(Rule::FixedStop { fraction: 0.10 }, 100.0, None, &sessions).unwrap();
        assert_eq!((untouched.session, untouched.reason), (7, "stop"));

        let never = flat(100.0, &[(20, 103.0)]);
        let exit = replay(Rule::FixedStop { fraction: 0.06 }, 100.0, None, &never).unwrap();
        assert_eq!(
            (exit.session, exit.reason, exit.close),
            (20, "window_end", 103.0)
        );
    }

    /// The trail follows the highest close up, never down, and is judged
    /// against the level it stood at before the close that tests it.
    #[test]
    fn a_trailing_stop_ratchets_up_and_never_loosens() {
        // ATR 2, multiple 3: distance 6. Highest close 110 at session 3 puts the
        // stop at 104; 105 holds, 104 exits.
        let sessions = flat(
            105.0,
            &[
                (1, 102.0),
                (2, 106.0),
                (3, 110.0),
                (4, 108.0),
                (5, 105.0),
                (6, 104.0),
            ],
        );
        let exit = replay(
            Rule::AtrTrail { multiple: 3.0 },
            100.0,
            Some(2.0),
            &sessions,
        )
        .unwrap();
        assert_eq!((exit.session, exit.reason, exit.close), (6, "trail", 104.0));

        // A dip below the fill that stays above the initial stop (94) does not
        // exit, and a later fall is judged against the raised stop.
        let dip = flat(100.0, &[(1, 95.0), (2, 101.0)]);
        let exit = replay(Rule::AtrTrail { multiple: 3.0 }, 100.0, Some(2.0), &dip).unwrap();
        assert_eq!(exit.reason, "window_end");
    }

    #[test]
    fn a_bracket_takes_whichever_side_a_close_reaches_first() {
        let up = flat(100.0, &[(3, 104.5), (5, 95.0)]);
        let exit = replay(Rule::AtrBracket { multiple: 2.0 }, 100.0, Some(2.0), &up).unwrap();
        assert_eq!((exit.session, exit.reason), (3, "target"));

        let down = flat(100.0, &[(2, 96.0), (5, 105.0)]);
        let exit = replay(Rule::AtrBracket { multiple: 2.0 }, 100.0, Some(2.0), &down).unwrap();
        assert_eq!((exit.session, exit.reason), (2, "stop"));
    }

    #[test]
    fn time_and_hold_exit_at_their_session() {
        let sessions = series(
            &(1..=WINDOW_SESSIONS)
                .map(|s| 100.0 + s as f64)
                .collect::<Vec<_>>(),
        );
        let time = replay(Rule::TimeExit { session: 10 }, 100.0, None, &sessions).unwrap();
        assert_eq!((time.session, time.reason, time.close), (10, "time", 110.0));
        let hold = replay(Rule::Hold, 100.0, None, &sessions).unwrap();
        assert_eq!(
            (hold.session, hold.reason, hold.close),
            (20, "window_end", 120.0)
        );
    }

    /// Nothing is judged before the whole window has elapsed, so a stop that
    /// fired early cannot enter the sample ahead of entries still running.
    #[test]
    fn an_entry_counts_only_once_its_whole_window_has_elapsed() {
        let short = series(&[90.0, 89.0, 88.0]);
        assert_eq!(
            replay(Rule::FixedStop { fraction: 0.06 }, 100.0, None, &short),
            None
        );

        let row = evaluate_entry(&Entry {
            symbol: "NOVO-B:xcse",
            filled_at: "2026-10-10T09:00:00Z",
            fill_price_local: 100.0,
            atr14_before_fill: Some(2.0),
            sessions: &short,
            sessions_elapsed: 3,
            round_trip_cost: 0.005,
        });
        assert_eq!(row.status, "pending");
        assert!(row.outcomes.is_empty());

        let gap = evaluate_entry(&Entry {
            symbol: "NOVO-B:xcse",
            filled_at: "2026-09-01T09:00:00Z",
            fill_price_local: 100.0,
            atr14_before_fill: Some(2.0),
            sessions: &short,
            sessions_elapsed: 25,
            round_trip_cost: 0.005,
        });
        assert_eq!(
            gap.status, "unavailable",
            "elapsed but the series has a hole"
        );
    }

    #[test]
    fn atr_rules_are_skipped_without_an_atr_and_the_rest_still_resolve() {
        let sessions = flat(100.0, &[]);
        let row = evaluate_entry(&Entry {
            symbol: "X:xnas",
            filled_at: "2026-09-01T14:00:00Z",
            fill_price_local: 100.0,
            atr14_before_fill: None,
            sessions: &sessions,
            sessions_elapsed: 30,
            round_trip_cost: 0.005,
        });
        assert_eq!(row.status, "resolved");
        let rules: Vec<&str> = row.outcomes.iter().map(|o| o.rule.as_str()).collect();
        assert_eq!(
            rules,
            [
                "fixed_stop_6pct",
                "fixed_stop_10pct",
                "time_exit_10",
                "hold_20"
            ]
        );
        assert!(
            row.outcomes
                .iter()
                .all(|o| (o.net_return - (o.gross_return - 0.005)).abs() < 1e-12)
        );
    }

    /// Holding to the end of the window is exactly the entry evaluation's
    /// 20-session return, so the two modules cannot drift apart unnoticed.
    #[test]
    fn holding_matches_the_entry_evaluations_twenty_session_return() {
        let sessions = series(
            &(1..=25)
                .map(|s| 100.0 + (s as f64 * 0.7).sin() * 5.0)
                .collect::<Vec<_>>(),
        );
        let hold = replay(Rule::Hold, 97.0, None, &sessions).unwrap();
        let evaluated = crate::entry_evaluation::evaluate_entry(
            &crate::entry_evaluation::Entry {
                symbol: "X:xnas",
                filled_at: "2026-08-31T14:00:00Z",
                fill_price_local: 97.0,
                quantity: 10.0,
                model: "m",
                forward_closes: &sessions,
                benchmark_closes: &[],
                min_commission_local: 1.0,
                rate_per_side: crate::entry_evaluation::COMMISSION_RATE_PER_SIDE,
            },
            25,
        );
        let twenty = evaluated["horizons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["horizon_sessions"] == 20)
            .unwrap()["gross_return"]
            .as_f64()
            .unwrap();
        assert!((hold.close / 97.0 - 1.0 - twenty).abs() < 1e-12);
    }

    #[test]
    fn summaries_pair_each_rule_with_holding_and_split_out_of_sample_entries() {
        // Entry A falls to 93 at session 2 and recovers to 101: the 6% stop
        // sells at 93, holding ends at 101. Entry B never falls.
        let a = flat(101.0, &[(2, 93.0)]);
        let b = flat(100.0, &[(20, 104.0)]);
        let row = |filled_at: &'static str, sessions: &[(String, f64)]| {
            evaluate_entry(&Entry {
                symbol: "X:xnas",
                filled_at,
                fill_price_local: 100.0,
                atr14_before_fill: Some(2.0),
                sessions,
                sessions_elapsed: 30,
                round_trip_cost: 0.0,
            })
        };
        let rows = vec![
            row("2026-09-01T14:00:00Z", &a),
            row("2026-09-02T14:00:00Z", &b),
            row("2026-10-19T14:00:00Z", &b),
            row("2026-07-01T14:00:00Z", &a),
        ];
        assert_eq!(rows[2].sample, "out_of_sample");
        assert_eq!(rows[3].sample, "pre_bootstrap");

        let summaries = summarize(&rows);
        let find = |rule: &str, sample: &str| {
            summaries
                .iter()
                .find(|s| s.rule == rule && s.sample == sample)
                .unwrap()
        };
        let stop = find("fixed_stop_6pct", "current_book");
        assert_eq!(
            stop.resolved, 2,
            "the pre-bootstrap entry is not summarised"
        );
        assert_eq!(stop.exit_reasons.get("stop"), Some(&1));
        // A: 93 vs 101 is -8 points; B: 104 vs 104 is 0. Mean -4.
        assert!((stop.mean_difference_vs_hold.unwrap() + 0.04).abs() < 1e-12);
        assert_eq!(stop.better_than_hold_count, 0);
        assert!(stop.difference_interval_low.unwrap() < stop.difference_interval_high.unwrap());

        let hold = find(HOLD_KEY, "current_book");
        assert_eq!(hold.mean_difference_vs_hold, Some(0.0));
        assert_eq!(find(HOLD_KEY, "out_of_sample").resolved, 1);
    }

    #[test]
    fn the_rule_set_is_declared_once_with_unique_keys_and_a_hold_baseline() {
        let mut keys: Vec<&str> = RULES.iter().map(|spec| spec.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), RULES.len());
        assert!(
            RULES
                .iter()
                .any(|spec| spec.key == HOLD_KEY && spec.rule == Rule::Hold)
        );
        assert!(FROZEN_AT.starts_with(RULESET_VERSION.trim_start_matches("exit-v1-")));
    }
}
