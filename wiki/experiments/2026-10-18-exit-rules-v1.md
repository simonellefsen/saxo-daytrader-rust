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

**Status:** registered 2026-10-18, before the first result was read. Measuring; nothing changes live behaviour.

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

None read yet. Record the first in-sample read here, dated, with the endpoint's `ruleset_version`.
