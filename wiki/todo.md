---
type: capability
tags:
  - daytrader/wiki
  - todo
  - maintained-by-llm
updated: 2026-08-31
---

# Daytrader Todo

Open items that are not defects. [urgent-todo](urgent-todo.md) holds verified gaps between what the runtime claims and what it enforces; [roadmap](roadmap.md) holds the long-horizon map. This page holds work that is decided-in-principle but not yet designed, plus decisions that are the operator's to make.

Raised 2026-08-21 from a review of live production after the weekend close.

## T1 — Decide what closes a review queue

**Status:** open, needs a rule rather than more evidence.

Three review queues now exist and none of them empties.

| Queue | State on 2026-08-21 |
| --- | --- |
| Holding Thesis Reviews | **14 due of 20 held.** Ages 11–25 days, every one carrying a `next 2 weeks` window that has elapsed |
| Hermes experiment proposals | 4 `pending_review` on 2026-08-02, oldest from 07-03, one already closed `expired_stale` without ever being judged |
| Missed-trade shadow book | 15 rows recorded, no review step defined |

Each queue is individually well-built: bounded, read-only, correctly refusing to act on its own. The Holding Thesis Review even states the boundary plainly — "a review queue, not an automated exit path". That restraint was the right call when they were built.

The problem is the aggregate. **A queue nobody empties is indistinguishable from no queue, except it costs compute to fill and it accumulates a backlog that looks like negligence.** The system is good at generating review work and has no mechanism for closing it.

The decision is not which queue to fix. It is what "reviewed" means:

- **Time-based** — a thesis whose window elapses is automatically re-evaluated at the next decision pulse, and the outcome recorded. Removes the operator from the loop entirely.
- **Owner + SLA** — a human reviews within N days; the existing Slack digest already exists for Hermes and could cover all three.
- **Automatic promotion under contract** — for Hermes experiments specifically, `promote_only_if` already encodes the safety envelope; the guardrails could be the reviewer.

**Do not build a fourth queue until this is settled.** The next evidence surface should either plug into an existing closing mechanism or come with its own.

**Checked again 2026-08-31 for the currency concentration panel.** It is a measurement, not a queue: it recomputes from the latest manager run on every render, accumulates nothing, and asks nobody to work through a backlog. It also deliberately does not create the review work a threshold would — the row is unconditional rather than firing past some concentration level, precisely because no threshold has been decided and inventing one would manufacture exactly the kind of queue this item is about.

**Checked against this rule 2026-08-31.** The Hermes data-request loop added that day records refusals and per-source outcomes on the advice record, which is new evidence output — but it is not a fourth queue: every request is served or refused within the same manager cycle and nothing accumulates for a human to work through. It closes itself by construction, which is the shape this item asks for. The `rejected` list is the one thing worth watching: a source Hermes keeps asking for and keeps being refused would be a backlog forming inside an audit field, and should be surfaced rather than left to accumulate quietly.

## T2 — Establish whether the strategy has an edge

**Status:** open. This is an operator decision about capital, not an engineering task.

Live record since the 2026-07-16 reset:

| | |
| --- | --- |
| Closed round trips | 20 |
| Wins / losses | **3 / 17** (15% win rate) |
| Net realised | **−21,372 DKK** (−8.3%) |
| Commission | 415 DKK |
| Since 2026-08-01, stops fully live | 8 sells, 2 wins, −2,263 DKK |

Decision Pulse Outcome Evidence, the system's own forward-movement measure:

- **1 session:** −1.0% forward movement, 38.9% positive, 36 samples
- **5 sessions:** +0.1% forward movement, 56.2% positive, 32 samples
- US Open pulse is the weaker: −1.7% / 23.5% positive at 1 session
- EU Open: −0.4% / 52.6% positive

At five sessions the result rounds to zero. **That is not evidence of an edge, and at n=32 it is not yet evidence of its absence.**

Two things genuinely point the right way and should not be lost in the negative headline:

- **The loss tail has been truncated.** AMAT (−4,469), ASML (−4,236) and ARM (−3,678) all closed 20–30 July, *before* the automatic protective-stop sweep landed on 07-26. Since stops went live the worst single loss is −1,390.
- **Only one exit in the whole period was a protective stop.** Four were discretionary `swing` sells and fifteen have no attributable exit order, so stops are standing guard rather than doing the closing.

**Applied 2026-08-31.** This item decided a real question rather than sitting as advice. A proposed multi-horizon Markov extension (4h / 1d / 5d trends) was measured and rejected — partly on its own evidence, and partly on item 3 below: adding a fourth signal dimension before the existing three can be evaluated makes the evaluation harder. See [concepts/markov-regime-model](concepts/markov-regime-model.md).

**One complication for the baseline, from the same day.** Markov moved from daily to hourly bars and `min_signed_signal` was recalibrated 0.15 → 0.20. Trades from 2026-08-31 onward are therefore decided by a different model than the 20 round trips recorded above, so they do not extend that sample — they start a new one. This is the second model change inside an already-underpowered record, and it is worth being honest that each such change resets the clock on answering this question. That is an argument for holding the model still for a while, not for avoiding the change that was already needed.

**What would change the picture, in preference order:**

1. **Let the shadow evidence collect clean baselines.** The 2026-08-23 diagnostic found that synchronous OpenRouter completions persisted their completed shadow report but returned before the record-only outcome hook; only deferred xAI completions invoked it. The shared completion hook and an idempotent repair pass now cover both paths. Historical reports repaired after the fact retain their candidate context but are explicitly marked as lacking a report-time Saxo quote, rather than receiving an invented later baseline. New shadow reports are the source for evaluable 1/5/20-session evidence.
2. **Wait for n to grow** before concluding anything from 20 trades.
3. **Prefer work that increases the rate of evaluable outcomes** over work that adds new signal sources. The system already has Markov, Quiver, daily indicators, support risk, editorial research and Hermes advice feeding decisions it cannot yet evaluate.

**A confound on the record above, quantified 2026-08-31.** The −21,372 DKK realised figure is a DKK number over a book that was 63% USD through a month in which USD/DKK fell 7.66%. `split_realised_gain` already separates price from currency on each closed trade, so the split is available per row rather than as an estimate; the live concentration is now measured too. Any read of "does the strategy pick well" has to net the currency component out first, and on this sample it is not small relative to the result.

**One thing that does not need netting out.** The Hermes counterfactual and missed-trade shadow ledgers compute entirely in local currency (`estimated_return_pct`, `estimated_pnl_local`), so FX never enters them. The shadow evidence this item leans on in preference order 1 is therefore unaffected by the currency question — verified 2026-08-31 while tracing an unrelated currency-mapping gap.

Whether to keep deploying capital while this is unresolved is the operator's call. The drawdown guardrail was widened to 25% on 2026-08-06 and currently sits at 19.24%, so the automatic floor is not the binding constraint on that decision.

## Related Pages

- [urgent-todo](urgent-todo.md) — verified enforcement gaps
- [roadmap](roadmap.md) — long-horizon map, including the `portfolio_position_snapshots` schema proposal
- [log](log.md) — dated record of what landed
