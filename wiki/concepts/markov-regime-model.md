---
type: concept
tags:
  - daytrader/wiki
  - strategy
  - markov
updated: 2026-08-31
sources:
  - /Users/lindau/codex/rust_daytrader/src/markov_method.rs
  - /Users/lindau/codex/rust_daytrader/wiki/sources/markov-hedge-fund-method.md
---

# Markov Regime Model As Implemented

How the Markov signal is actually computed in `src/markov_method.rs`, what its
tunings mean, and which model changes have already been tested and rejected.
[sources/markov-hedge-fund-method](../sources/markov-hedge-fund-method.md)
covers the method this is derived from; this page covers our implementation and
the traps in it.

## Pipeline

1. Fetch `sample_count` bars at `horizon_minutes` from Saxo `/chart/v3/charts`.
2. `label_regimes` labels each bar by rolling return over a lookback window:
   `Bull` at or above `threshold`, `Bear` at or below its negative, else
   `Sideways`.
3. `transition_counts` / `transition_matrix` estimate a 3x3 matrix by maximum
   likelihood from consecutive label pairs.
4. `forecast_distribution` raises the matrix to a power from the current regime.
5. `signed_signal = P(Bull) - P(Bear)` at the signal horizon; `conviction` is
   its absolute value, `direction` its sign.
6. The manager gate admits a candidate when `|signed_signal|` reaches
   `strategy.swing.markov_gate.min_signed_signal` and the signal is younger
   than `max_signal_age_days`.

## Every tuning counts bars, not days

This is the single most important thing to know before changing anything here.

`window_days`, `min_labeled_days`, `signal_horizon_days` and `forecast_steps`
are all applied as **index offsets into the bar series** — `label_regimes`
compares `bars[index]` to `bars[index - window]`, and `forecast_distribution`
takes a step count. Their `_days` names are only accurate while
`horizon_minutes` is 1440.

Changing the horizon alone would silently reinterpret a 20-day window as 20
hours. A 5% threshold over roughly two sessions almost never trips, so every
bar labels `Sideways`, the matrix becomes `Sideways -> Sideways`,
`signed_signal` collapses toward zero, and the gate stops approving BUYs — with
**no error raised anywhere**. This was measured, not hypothesised: at a window
matched to a four-hour term, median `|signed_signal|` across 28 symbols was
**0.003**, and **0 of 28** cleared the gate.

`bars_per_session(horizon_minutes, session_minutes)` scales each tuning into
bars and returns `1` for daily-or-coarser horizons, so the historical daily
behaviour is reproduced exactly. `an_unscaled_intraday_window_would_collapse_the_regime_signal`
locks the failure mode down.

## Bars per session vary by exchange

Measured against live SIM at `Horizon=60`:

| Exchange | Bars/session |
| --- | --- |
| NASDAQ / NYSE | 7 |
| Copenhagen | 8 |
| Stockholm, London, XETR | 9 |

`session_minutes: 510` resolves to 9, the maximum. US windows therefore span
*more* calendar days than configured, never fewer — the conservative direction.
A per-exchange session length would be more precise but is not currently
modelled.

## Saxo intraday depth

Saxo returns up to **1200 bars per request at any horizon**, so the calendar
coverage shrinks as the horizon does:

| Horizon | Coverage of 1200 bars |
| --- | --- |
| 1440 (daily) | ~4.7 years |
| 60 (hourly) | ~6.3 months |
| 30 | ~3.2 months |

At hourly, `window_bars` 180 plus `min_labeled_bars` 540 needs 722 bars;
`sample_count: 900` leaves headroom without approaching the 1200 cap.

## Refresh slots

Runs are keyed by named slots, deduplicated on `created_at` within the slot
window rather than by date alone. A config with only `daily_time` collapses to
the previous one-run-per-day behaviour.

Two scheduling facts constrain slot placement:

- The scheduler runs Markov **after** decision reports within one tick, so a
  slot in the same tick as a report will not reach it. Ticks are 10 minutes
  apart (`SCHEDULER_INTERVAL_MINUTES`).
- A slot feeding a report anchored to another timezone must share that
  timezone, or the two drift apart during the weeks each year when US and EU
  daylight saving disagree. `intraday_runs[].time_zone` exists for this.

## Model changes already tested and rejected

Recorded so they are not re-proposed without new evidence. All measured
2026-08-31 on 28 live symbols at hourly bars.

**Multiple look-aheads against one regime window** (4h / 1d / 5d, all on the
20-day window): rejected. Correlation `4h~1d = +0.986`, and all three agreed on
direction in **27 of 28** symbols. They forecast one slow process at different
depths along a monotonic path toward the stationary distribution, so they are
near-duplicates rather than independent views.

**A window matched to each term** (a real 4h, 1d and 5d trend): genuinely
independent (`4h~5d = +0.220`) but inert — median `|signed_signal|` of
0.003 / 0.009 / 0.050, clearing the gate 0, 1 and 3 times out of 28, for the
collapse reason above. A working version needs **a calibrated threshold per
term** (5% suits 20 days; four hours wants perhaps 0.5%), which is a modelling
project with three tuning surfaces rather than a config change.

**14-day and 30-day forecast steps:** rejected. Distance to the stationary
distribution by look-ahead:

| Days | Bars | Median \|signal\| | Gap to stationary |
| --- | --- | --- | --- |
| 1 | 9 | 0.247 | 0.366 |
| 2 | 18 | **0.305** | 0.282 |
| 3 | 27 | **0.286** | 0.222 |
| 5 | 45 | 0.247 | 0.149 |
| 10 | 90 | 0.195 | 0.082 |
| 14 | 126 | 0.182 | 0.067 |
| 20 | 180 | 0.176 | 0.059 |
| 30 | 270 | 0.189 | 0.055 |

By day 10 the forecast is already within 0.082 of the stationary distribution,
which `stationary_json` **already persists exactly**; 14/20/30 restate it less
accurately, and the count clearing the gate is flat at 14/28 from day 10 on.
`forecast_steps` was trimmed to `[1, 2, 3, 5]` for this reason.

Note the peak: information is highest at **2-3 days**, above both 1 and 5. That
makes `signal_horizon_days: 5` a live open question — see
[roadmap](../roadmap.md).

## The gate threshold is coupled to the horizon

`min_signed_signal` is not portable across model changes. Moving from daily to
hourly bars made signals *stronger*, and the same 0.15 threshold went from
admitting 111 of 200 symbols to 132 — a risk gate loosening as a side effect of
a change about freshness. It was recalibrated to 0.20 (admitting 113) the same
day.

**Any change to `horizon_minutes`, `window_days`, `threshold` or
`signal_horizon_days` invalidates the current `min_signed_signal` calibration.**
Re-measure the admission rate against the previous configuration before and
after, and change one of them at a time.

A caution from the same day: an initial 16-symbol sample indicated the opposite
direction and was written into the roadmap before a larger sample corrected it.
Sixteen symbols is not enough to characterise a distribution across 200.

## Related Pages

- [sources/markov-hedge-fund-method](../sources/markov-hedge-fund-method.md) — the source method.
- [concepts/current-system-architecture](current-system-architecture.md) — where this sits in the advisory flow.
- [roadmap](../roadmap.md) — open Markov questions.
- [todo](../todo.md) — T2, whether the strategy has an edge at all.
