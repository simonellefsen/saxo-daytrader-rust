---
type: capability
tags:
  - daytrader/wiki
  - todo
  - maintained-by-llm
updated: 2026-09-03
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

Live record since the 2026-07-16 reset. **Restated 2026-09-03** from the realised-sell panel, which had never rendered in production until 2026-09-02 and was scoped to the wrong book until 2026-09-03; the counting unit is the reconciled sale ledger row, so it does not line up with the "closed round trip" count this table used before.

| | |
| --- | --- |
| Closed sales | 32 |
| Wins / losses | **9 / 23** (28.1% win rate) |
| Net realised | **−21,298 DKK** |
| Commission | 673 DKK |
| Average win / average loss | **481 / −1,114 DKK** |
| **Payoff ratio** | **0.43** |
| Median holding time | 24.0 days (winners 31.0, losers 19.6) |
| Cost basis sold | 384,456 DKK |

**The payoff ratio is the number that matters, and it now has a mechanical explanation.** Every one of those 32 exits was a risk-reduction exit — 28 protective stops (−14,225 DKK) and 4 flattens (−7,073) — and **not one was a profit-taking exit**. Nothing in the runtime closes a position because it reached a target, so a win can only happen when a stop that trailed up gets hit. The nine wins are exactly that, which is why the average win is 481 DKK against an average loss of 1,114.

This reframes the question. The earlier reading was "15% win rate, no evidence of an edge, wait for n to grow". The current reading is that **the exit side has no mechanism for realising a gain**, and `strategy.ladder.take_profit_rung_multiple` and `strategy.ladder.max_take_profit_atr_multiple` sit in `config.yaml` marked by the contract audit as "Take-profit targets are not implemented". Waiting for n to grow tests entry quality against an exit policy that structurally caps wins.

Two supporting facts have also moved:

- **The loss tail is genuinely truncated.** Average loss −1,114 DKK against a book of roughly 7–8k DKK clips is stops doing their job, and the roadmap's earlier note that the worst single loss since stops went live is −1,390 still holds.
- **Winners are held longer than losers** (31.0 vs 19.6 days median), so this is not the classic cut-winners-early failure. The book holds winners and still cannot make them large, which points at the exit mechanism rather than at conviction or patience.

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

1. **Let the shadow evidence collect clean baselines.** *(Second defect found and fixed 2026-08-31: the Hermes dimension of that evidence was blank for 13 of 15 candidates because the effect was recorded before the asynchronous advice arrived. Until now, any comparison of Hermes-affected against unaffected shadow candidates would have been reading an artifact of that race rather than a result. The 1/5/20-session price outcomes were unaffected.)* The 2026-08-23 diagnostic found that synchronous OpenRouter completions persisted their completed shadow report but returned before the record-only outcome hook; only deferred xAI completions invoked it. The shared completion hook and an idempotent repair pass now cover both paths. Historical reports repaired after the fact retain their candidate context but are explicitly marked as lacking a report-time Saxo quote, rather than receiving an invented later baseline. New shadow reports are the source for evaluable 1/5/20-session evidence.
2. **Wait for n to grow** before concluding anything from 20 trades.
3. **Prefer work that increases the rate of evaluable outcomes** over work that adds new signal sources. The system already has Markov, Quiver, daily indicators, support risk, editorial research and Hermes advice feeding decisions it cannot yet evaluate.

**A confound on the record above, quantified 2026-08-31.** The −21,372 DKK realised figure is a DKK number over a book that was 63% USD through a month in which USD/DKK fell 7.66%. `split_realised_gain` already separates price from currency on each closed trade, so the split is available per row rather than as an estimate; the live concentration is now measured too. Any read of "does the strategy pick well" has to net the currency component out first, and on this sample it is not small relative to the result.

**One thing that does not need netting out.** The Hermes counterfactual and missed-trade shadow ledgers compute entirely in local currency (`estimated_return_pct`, `estimated_pnl_local`), so FX never enters them. The shadow evidence this item leans on in preference order 1 is therefore unaffected by the currency question — verified 2026-08-31 while tracing an unrelated currency-mapping gap.

Whether to keep deploying capital while this is unresolved is the operator's call. The drawdown guardrail was widened to 25% on 2026-08-06 and currently sits at 19.24%, so the automatic floor is not the binding constraint on that decision.

## T3 — Decide whether the Markov conviction threshold is a gate or a hint

**Status:** open, raised 2026-09-02. The measurement is done and the visibility landed the same day; what is left is a risk decision, not code.

`strategy.swing.markov_gate.min_signed_signal` is configured as a gate threshold and the gate replay reports it `unreachable_in_retained_evidence` — 17 of 17 candidates `not_reached`. That is true of the deterministic gate: `markov_buy_gate` has exactly one call site, inside the branch taken when the *technical* gate rejects a BUY, and the technical gate has rejected no BUY in the retained window.

**The number is not inert, though. It is published to Hermes in the decision preflight, and Hermes applies it as an admission bar in so many words.** From advice recorded since 2026-08-01:

| | |
| --- | --- |
| Order-advice items | 127 |
| Mention Markov | **105 (83%)** |
| Quote `signed_signal` numerically | **54 (43%)** |
| Of 24 `review` holds, mention Markov | 22 |

Verbatim, from three different candidates:

- `GN:xcse`, **allow** — "signed_signal +0.2462 clears both current and pending 0.20 conservative Markov threshold"
- `ALV:xetr`, **review** — "signed_signal +0.1664, below the pending conservative…"
- `CARL-B:xcse`, **stand_down** — "signed_signal only +0.011, below…"

**The clearest case is 2026-09-01.** The US-open report produced two candidates, `DE:xnys` and `PLTR:xnas`. Both passed the technical gate with 5 confluences against a minimum of 3, both fitted the 24,188 DKK budget, and both were skipped `hermes_advice` — Hermes citing signed_signal 0.1686 and 0.1289 as low conviction. Both sit under the configured 0.20. The deterministic gate never ran; the cycle bought nothing.

So the threshold currently binds through an LLM's reading rather than through a gate. It is the most consequential number in the stack and the least deterministic.

Three coherent answers, and they are not equivalent:

- **Make it a real gate.** Apply `min_signed_signal` to every BUY, not just the starter fallback. The advisory and the runtime would then agree, and the replay's reachability verdict would become true. This *tightens* the envelope: at 0.20 it would have blocked the same two candidates deterministically, and it makes the threshold measurable and tunable — a proposal against it would mean something.
- **Tell Hermes what it actually governs.** The preflight publishes `markov.min_signed_signal` with no statement of scope, so reading it as the admission bar for the candidates alongside it is a reasonable inference. Saying it governs only the starter fallback would *loosen* the envelope: yesterday's two candidates would likely have been allowed.
- **Leave it, having named it.** The advisory hold may be doing useful work; the objection is that nothing decided it should.

**Do not treat this as a bug fix.** Both of the first two options change what gets bought, in opposite directions, and neither is obviously right from the evidence — the loss record in T2 does not say whether holding weak-conviction candidates has helped or hurt.

**Landed 2026-09-02 regardless of which is chosen:** the gate replay now reports `advisory_visible` and `effect_path` per proposable variable, so the self-improvement loop is no longer told that an advisory-visible variable "cannot produce a measurable effect at any value". That statement was in the reflection prompt and on the dashboard, and it was false for this variable.

## Related Pages

- [urgent-todo](urgent-todo.md) — verified enforcement gaps
- [roadmap](roadmap.md) — long-horizon map, including the `portfolio_position_snapshots` schema proposal
- [log](log.md) — dated record of what landed
