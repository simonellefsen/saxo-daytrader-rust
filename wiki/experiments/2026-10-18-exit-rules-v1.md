---
type: experiment
tags:
  - daytrader/wiki
  - experiments
  - exits
  - evaluation
updated: 2026-10-18
sources:
  - src/exit_evaluation.rs
  - src/entry_evaluation_state.rs
---

# Exit rules v1: replayed over every entry

**Status:** registered 2026-10-18 before the first read; first in-sample read recorded below. Out-of-sample accumulating; nothing changes live behaviour.

Read-only. `/api/evaluation/exits` replays a frozen set of exit rules over every BUY fill. It proposes nothing, gates nothing and reaches no broker.

## Why

The 2026-10-18 edge review found:

- 96 of the 100 sales since the 2026-07-16 bootstrap were protective stops.
- Lots stopped within 5 or 10 sessions sold 1.5–1.7 points below where the same lot closed at that horizon; the 95% interval excludes zero.
- The live runtime has no take-profit exit ([T2](../todo.md)).

The entry evaluation deliberately simulates no exit policy, so none of this could be compared across policies on the same entries.

## Baseline

`hold_20`: hold to the close of the twentieth trading session after the fill, no stop. It equals the entry evaluation's 20-session return for the same entries, and a test holds the two together.

## Rule set (`exit-v1-2026-10-18`, frozen 2026-10-18T12:00:00Z)

| Key | Rule |
| --- | --- |
| `fixed_stop_6pct` | Stop 6% below the fill, never moved. This is the median initial stop distance of live entries, without the live trailing ladder: an approximation of the current policy, not a replica. |
| `fixed_stop_10pct` | Stop 10% below the fill, never moved. |
| `atr_trail_3x` | Trailing stop 3 × ATR14 below the highest close since the fill, ratcheting up only. |
| `atr_bracket_2x` | Stop 2 × ATR14 below and take-profit 2 × ATR14 above the fill, whichever close comes first. |
| `time_exit_10` | Sell at the close of session 10, no stop. |
| `hold_20` | Baseline. |

Changing any rule, the window or the cost model means `exit-v2`, not an edit.

## Method

- **Unit:** every BUY fill.
- **Window:** 20 sessions. An entry counts only once its whole window has elapsed, under every rule, so early stop-outs cannot be over-represented.
- **Close basis:** a stop, trail or target is tested against each close and fills at that close. Intraday highs and lows are not stored.
- **ATR:** ATR14 comes from the last indicator row before the fill day.
- **Costs:** the entry evaluation's round-trip commission, identical for every rule.
- **Comparison:** each rule against `hold_20`, paired by entry, as a mean difference with a normal-approximation 95% interval.
- **Samples:** `current_book` (2026-07-16 to the freeze) is in-sample, because the edge review saw it before the rules were chosen. `out_of_sample` is every fill after 2026-10-18T12:00:00Z.

## Hypotheses, stated before the first read

1. `fixed_stop_6pct` trails `hold_20`: a negative mean difference.
2. `fixed_stop_10pct` and `atr_trail_3x` trail `hold_20` by less than `fixed_stop_6pct` does.
3. `atr_bracket_2x` carries no directional prediction. It is measured because a take-profit route is what T2 says is missing.
4. `time_exit_10` is within noise of `hold_20`, since the entry evaluation finds no edge at 10 or 20 sessions.

## What would count

- **In-sample results generate hypotheses and nothing more,** whatever their interval.
- **Proposing a live exit change needs out-of-sample evidence:** at least 30 resolved out-of-sample entries, with the rule's 95% interval against `hold_20` excluding zero in the same direction as in-sample.
  - At the current pace of roughly one fill a session, plus the 20-session window, that is not before late December 2026.
- Any such proposal goes through the Hermes experiment workflow as one variable. Simon decides.

## Results

### First in-sample read, 2026-10-18 (`exit-v1-2026-10-18`, release `60b31f2`)

Current book, 68 entries with a full 20-session window (38 pending, 1 unavailable). No out-of-sample entry has resolved yet.

| Rule | Gross | Net | Net > 0 | Mean exit session | vs `hold_20` | 95% interval | Exits |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `fixed_stop_6pct` | −0.12% | −0.63% | 40% | 14.8 | −0.32 | −1.21 to +0.57 | 30 stop, 38 window end |
| `fixed_stop_10pct` | +0.11% | −0.40% | 43% | 18.3 | −0.09 | −0.60 to +0.43 | 13 stop |
| `atr_trail_3x` | +0.27% | −0.24% | 44% | 16.3 | +0.07 | −0.65 to +0.79 | 32 trail |
| `atr_bracket_2x` | +0.24% | −0.27% | 47% | 12.0 | +0.04 | −1.51 to +1.60 | 28 stop, 23 target |
| `time_exit_10` | −0.13% | −0.64% | 46% | 10.0 | −0.33 | −2.08 to +1.43 | 68 time |
| `hold_20` | +0.19% | −0.32% | 43% | 20.0 | — | — | 68 window end |

`hold_20` reproduces the entry evaluation's 20-session gross (+0.19%, n=68).

Against the hypotheses:

1. **`fixed_stop_6pct` trails holding**, as predicted (−0.32 points), but the interval spans zero.
2. **The wider rules trail by less**, as predicted (`fixed_stop_10pct` −0.09, `atr_trail_3x` +0.07), but within noise.
3. **The bracket** is +0.04 with the widest interval of the stop rules. Its target fired on 23 of 68 entries.
4. **`time_exit_10` is within noise of holding**, as predicted.

**Reading.** Over a 20-session window and on closes, the choice of exit rule moves the mean by at most a third of a point, and every rule is net negative. This fits the edge review's headline: with no edge in the entries, a better exit can only lose less.

This does not contradict the review's per-lot finding (1.5–1.7 points at 5–10 sessions). That was conditional on lots a live stop closed early, with intraday fills. Here a stop that never fires contributes zero, and the replay is close-basis.

Next read: when out-of-sample entries resolve. Nothing here supports a live change.
