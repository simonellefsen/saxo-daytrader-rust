# Retained analysis scripts

Reproduce the exit-rule and entry-separation results quoted in the 2026-09-20
review. Kept because the conclusions were used to argue against changing risk
parameters, and an argument that cannot be re-run is not evidence.

## Inputs

Export from the production database into the same directory:

- `trips.psv` — one row per reconciled SELL with its matched prior BUY:
  `symbol|exit_at|exit_px|qty|realised_gain_dkk|entry_at|entry_px`
- `series.psv` — daily close and ATR per symbol: `symbol|run_date|close|atr14`
- `features.psv` — decision-time indicator snapshot per round trip
  (see `sep.py` `COLS` for the column order)

## Scripts

| script | question |
|---|---|
| `sim.py`  | trailing stop at 2/3/4/5 ATR on daily closes |
| `sim2.py` | trailing stops vs fixed-horizon exits |
| `sim3.py` | gross price return by horizon, no costs or FX |
| `sep.py`  | winner/loser effect sizes across decision-time features |
| `perm.py` | permutation test on the strongest feature |
| `dow2.py` | leave-one-week-out on the day-of-week effect |

## Known limits

These are the caveats that belong with any number they produce.

- **The simulator does not reproduce its own baseline.** At 2 ATR it returns
  −21,425 DKK against an actual −15,617. Daily closes miss the intraday highs
  the real ladder ratchets on, and miss intraday stop triggers and gaps
  entirely. An earlier version of this file claimed "only the ordering is
  usable"; that was too generous. A model that misses the baseline by 37% can
  misrank alternatives too, because the same missing intraday mechanics do not
  bias every variant equally — a wider stop is triggered by intraday extremes
  more often than a tight one, so the error grows with the parameter. Treat the
  ordering as a hypothesis, not a result.
- **Sale-ledger rows are not independent entry-to-exit experiments.** They lack
  partial-sale handling and explicit acquisition matching, and they exclude
  still-open positions, so the population is survivor-shaped.
- **Counts differ by design and should be stated whenever quoted**: 64
  reconciled SELLs, 59 with a matched entry and a usable price series, 52 also
  matched to an entry report carrying decision metadata.
- **A large permutation p-value is not evidence of no effect.** `perm.py`
  returning p = 0.842 means no detectable separation at this sample size, not
  that the features are uninformative.
- **Winners vs losers among admitted positions cannot measure the filter.**
  Rejected candidates are absent, so nothing here says whether the gate helped.
- **The period is not one strategy.** Markov scaling, sizing, the commission
  threshold and the prompts all changed inside it, and long-horizon rows
  necessarily describe older entries.
