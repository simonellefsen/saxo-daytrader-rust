---
type: decision
tags:
  - daytrader/wiki
  - decisions
  - decision-reports
  - hermes
  - observability
updated: 2026-08-19
sources:
  - /Users/lindau/codex/rust_daytrader/deploy/k8s/base/config.k8s.yaml
  - /Users/lindau/codex/rust_daytrader/src/xai_decision.rs
  - /Users/lindau/codex/rust_daytrader/src/trading_manager.rs
  - /Users/lindau/codex/rust_daytrader/docs/hermes-agent.md
  - https://www.nasdaq.com/docs/nasdaq-global-trading-hours-faqs
  - https://www.sec.gov/files/rules/sro/nasdaq/2026/34-105199.pdf
  - https://www.developer.saxo/openapi/referencedocs/port/v1/clients/get__port__clientkey/schema-allowedtradingsessions
  - https://www.developer.saxo/openapi/referencedocs/ref/v1/instruments/get__ref__details/schema-instrumentdetails
---

# Shadow Mid-Session Decision Reports And Tuning Evidence

## Status

Planned. This record defines the implementation and evidence plan; it does not change the running schedule, create Decision Reports, activate an experiment, or alter Saxo execution.

## Context

The running system produces two execution-eligible weekday Decision Reports:

- Nordic/EU open follow-up, 75 minutes after the configured European exchanges open.
- US open follow-up, 75 minutes after XNAS/XNYS open.

Those reports provide only two daily observations. The portfolio needs more evidence about whether conditions later in each session change candidate quality, invalidate an opening thesis, or merely repeat the same nightly Markov, daily-indicator, Support Risk, and Quiver context. Increasing report frequency without isolating the new pulses would increase duplicate advice, provider cost, Hermes interventions, and potential turnover before establishing incremental value.

The dashboard already exposes parts of the required evidence across Decisions, Execution, Performance, and Hermes. A dedicated tuning read model should connect the full path from signal to report, Hermes advice, deterministic gate, shadow or broker outcome, cost, and portfolio result.

Nasdaq has SEC approval for a 23-hour weekday structure and currently targets 6 December 2026, subject to Securities Information Processor readiness and any remaining activation rule changes. The proposed structure retains the 04:00-20:00 ET Day Session, including regular market hours and the Opening/Closing Crosses; adds a 21:00-04:00 ET Night Session; and keeps a daily 20:00-21:00 ET pause. This makes explicit session identity more important, but it does not remove regular market hours or make every broker/account/instrument tradable in every Nasdaq session.

## Decision

Add two scheduled weekday Decision Reports in server-enforced shadow mode:

| Pulse key | Local schedule | Market scope | Required market state |
| --- | --- | --- | --- |
| `europe_mid_session_shadow` | `14:15 Europe/Copenhagen` | `XCSE`, `XSTO`, `XOSL`, `XHEL`, `XLON`, `XETR`, `XFRA`, `XMIL`, `XAMS` | At least one configured exchange is in its regular tradable session |
| `us_mid_session_shadow` | `14:15 America/New_York` (normally `20:15 Europe/Copenhagen`; `19:15` during the US/EU DST mismatch) | `XNAS`, `XNYS` | At least one configured exchange is in its regular tradable session |

The EU wall-clock time is intentionally anchored to `Europe/Copenhagen`. The US pulse is intentionally anchored to `America/New_York` so it stays at the same point in the US trading session. It therefore appears at 20:15 Copenhagen while both regions use matching standard/daylight regimes, and at 19:15 Copenhagen during the short spring and autumn periods when US and European daylight-saving transitions do not align. Time-zone database rules, Saxo exchange calendars, and current market state remain authoritative for DST, holidays, shortened sessions, breaks, halts, and non-trading days. A closed or ambiguous scope records a visible `not_due`/`market_closed` result rather than submitting a provider request.

The existing open +75-minute pulses remain unchanged. The new pulse key must be date-idempotent so a scheduler retry cannot create a duplicate report for the same local trading date.

### Nasdaq 23/5 Compatibility

The US shadow pulse remains anchored to 14:15 America/New_York and requires the regular US session. It should be described as a US afternoon regular-session pulse rather than the midpoint of Nasdaq's future 23-hour venue window. Regular-session liquidity, the Opening/Closing Crosses, and comparison with the earlier US pulse remain the experiment's reference frame.

Before Nasdaq's expected transition, replace any generic boolean `market_open` assumption in this flow with an explicit session classification such as `regular`, `pre_market`, `post_market`, `night`, `pause`, or `closed`. Persist the session observed for every quote, report, candidate, and outcome.

Nasdaq venue availability does not grant Saxo execution availability. The application must independently verify the Saxo client's `AllowedTradingSessions`, the instrument's `IsExtendedTradingHoursEnabled`, the live market/session state, and supported extended-hours order semantics. A future Saxo-visible Night Session may be read as labelled context, but it must not make a shadow pulse queue-eligible or broaden execution authority automatically. Any use of pre-market, post-market, or Night Session execution requires its own SIM experiment, risk limits, evidence window, and decision record.

## Shadow Safety Contract

Shadow status is server-owned metadata, never a provider field or prompt instruction. A shadow report:

- persists the same normalized report and exact context provenance needed for comparison;
- may run the deterministic manager evaluation as a pure gate replay and persist its waterfall;
- may request Hermes advice in record-only mode;
- must carry `queue_eligible = false` through every internal boundary;
- must never insert an `execution_orders` row;
- must never invoke Saxo order precheck, placement, replacement, cancellation, or approval;
- may use the existing read-only Saxo quote, portfolio, market-state, chart, and reconciliation surfaces;
- records a reference quote for every eligible suggested trade so its shadow outcome can mature;
- remains shadow after restarts, configuration reloads, experiment overlays, and future LIVE-environment use.

Tests must prove the negative boundary directly: a shadow report can reach provider parsing, Hermes record-only advice, deterministic evaluation, persistence, EOD aggregation, and dashboard reads while no queue writer or Saxo mutation is reachable.

Pending-review Hermes experiments are excluded from decision authority. The tuning panel must report `pending_policy_influence_count`, whose expected value is zero. Only an explicitly approved/active overlay may affect an execution-eligible manager run, and no overlay may make a shadow report queue-eligible.

## Implementation Plan

### Phase 0: Repair Measurement And Authority Prerequisites

1. Prevent pending-review experiment values from influencing Hermes allow/reduce/block/review advice.
2. Add expiry or full execution-time revalidation for discretionary queued orders.
3. **Landed 2026-08-19:** include current-mode/current-adapter pending and broker-working BUY reservations in symbol exposure and cash calculations across scheduler cycles. The remaining fraction of a partial fill reserves only its remaining DKK value; an active BUY without a reliable DKK value fails closed for new BUYs until reconciled. Terminal and different-execution-environment rows do not reserve the current runtime's budget.
4. Make report-reference pricing reliable for Hermes counterfactuals, manager-gate shadows, and the new pulse candidates.
5. Keep protective GoodTillCancel stops outside discretionary-order expiry rules.

### Phase 1: Typed Shadow Pulse Contract

1. Add a typed server-owned pulse mode such as `ExecutionEligible` versus `Shadow`.
2. Persist pulse mode and queue eligibility with each report; do not infer either value from labels.
3. Add stable pulse keys, local dates, local/UTC due times, market-scope evidence, and terminal scheduler results.
4. Add regressions for date idempotency, holidays, DST changes, shortened sessions, restarts, and the no-queue/no-Saxo boundary.
5. Add explicit US session classification and boundary tests for regular, pre-market, post-market, Night Session, the 20:00-21:00 ET pause, and broker/instrument extended-hours ineligibility.

### Phase 2: Schedule And Generate The Reports

1. Configure the EU shadow report for 14:15 Europe/Copenhagen.
2. Configure the US shadow report for 14:15 America/New_York and derive its Copenhagen display time from the time-zone database rather than storing a second fixed wall-clock schedule.
3. Include current portfolio, cash, positions, open/pending orders, protected quantities, active approved policy, latest signals, and the earlier same-scope report in the context.
4. Require the report to describe what materially changed since the earlier pulse. Persist `no_new_information` when nothing changed instead of manufacturing candidates.
5. Add separate scheduler-health rows and missed-pulse alerts for both schedules.

### Phase 3: Shadow Outcome Ledger

For each suggested BUY or SELL, persist:

- pulse and report provenance;
- candidate rank and whether the symbol appeared in the earlier pulse;
- reference timestamp, local price, currency, DKK conversion basis, and proposed quantity;
- report-time technical, Markov, Quiver, Support Risk, cash, concentration, and thesis snapshot;
- deterministic gate result and stable gate code;
- Hermes record-only effect and the approved-policy source it used;
- next-session, five-session, and twenty-session directional outcomes;
- maximum adverse and favourable excursion when intraday evidence is available;
- an estimated after-cost outcome, explicitly separated from realised P/L.

Shadow observations never alter holdings, cash, capacity, gates, experiments, or orders.

### Phase 4: Tuning Dashboard

Add a typed, read-only tuning payload and a lazily loaded Tuning tab. Reuse existing evidence builders rather than recomputing incompatible versions in the UI.

The first version should contain:

1. **Outcome strip:** 30-day net return versus target, benchmark-relative return, current/max drawdown, after-cost expectancy, evidence completeness, and active experiment progress.
2. **Pulse comparison:** report count, terminal success, candidate novelty, `no_new_information` rate, duplicates, gate outcomes, 1/5/20-session results, and estimated cost by EU open, EU shadow, US open, and US shadow.
3. **Candidate funnel:** considered -> selected -> suggested -> Hermes effect -> deterministic result -> shadow/queued -> prechecked -> filled.
4. **Signal calibration:** technical confluence, Markov signed-signal, Quiver/disclosure age, and Support Risk buckets with sample count, positive rate, forward outcome, and adverse/favourable excursion.
5. **Hermes evidence:** advice distribution, priced prevented quantities, context failures, policy-source compliance, pending-policy influence count, and final manager outcome.
6. **Execution quality:** report-to-queue, queue-to-submit, submit-to-fill, queue age, stale order count, fill rate, slippage, precheck cost, realised cost, and exposure reserved versus cap.
7. **Portfolio risk:** cash/buy budget after all guards, concentration including reservations, risk at protective stops, stop coverage, and realised/unrealised P/L attribution.
8. **Experiment card:** one changed variable, baseline/candidate value, start date, observation target, mature sample count, primary metric, guardrails, and lifecycle status.

Every metric carries environment, observation window, denominator, maturity (`collecting`, `preliminary`, or `mature`), gross/net status, executed/shadow status, and an as-of timestamp. Do not create one composite system score.

### Phase 5: EOD And Hermes Integration

1. Expand the deterministic EOD journal to reconcile all four pulses while separating execution-eligible and shadow recommendations.
2. Surface suggestions with no manager action, unresolved prices, duplicate candidates, stale orders, signal drift, cost, and protection exceptions in `what_did_not_work`.
3. Let the later Hermes EOD reflection interpret the deterministic pack and propose at most one non-duplicate one-variable experiment.
4. Keep all proposals pending until the normal operator-controlled paper/SIM lifecycle approves them.

### Phase 6: Observe, Compare, And Decide

Run both shadow pulses without execution authority until each has:

- at least 20 eligible trading days;
- at least 20 mature five-session candidate observations;
- report-reference coverage high enough that missing prices cannot determine the result;
- no shadow safety violation;
- no unresolved duplicate/idempotency issue.

Review candidate novelty, `no_new_information` rate, after-cost forward outcome, adverse excursion, turnover implied by the shadow recommendations, and drawdown behavior. Compare each shadow pulse with its same-market opening pulse and with doing nothing after the opening decision.

Promotion is a separate operator decision and a separate decision record. It must identify one pulse, one evidence window, and the exact execution authority granted. Neither pulse promotes automatically, and evidence may support retaining both as observation-only or retiring one or both.

## Success And Guardrail Metrics

- Zero `execution_orders`, Saxo prechecks, or Saxo mutations attributable to a shadow pulse.
- Every due pulse reaches one visible terminal state; no silent miss or duplicate report.
- `pending_policy_influence_count = 0`.
- Shadow candidate reference-price coverage is explicit and trends toward complete coverage.
- EOD totals reconcile report, candidate, Hermes, gate, shadow, queue, fill, and failure counts without mixing shadow outcomes into realised P/L.
- Any future promotion improves measured marginal value after estimated costs without worsening drawdown, concentration, stale-order incidence, or protective-stop coverage.

## Consequences

- Provider and Hermes workload grows from two to four scheduled weekday reports, but the new reports cannot create turnover during observation.
- The application gains enough same-day evidence to decide whether a later-session report adds information.
- The EOD journal and Hermes reflection receive a richer but explicitly separated dataset.
- Quote/cost capture, typed read models, and outcome maturity require more storage and bounded retention.
- The roadmap prioritizes measurement and authority repair before further signal or execution automation.

## Alternatives Considered

### Make Both New Reports Execution-Eligible Immediately

Rejected. Current signal calibration is preliminary, pending-order reservation and expiry gaps were observed, and extra cadence would multiply duplicate-order risk before marginal value is measured.

### Trigger At Session Midpoint Instead Of Fixed Local Time

Not selected for the initial experiment. The operator chose 14:15 Europe/Copenhagen for EU and 14:15 America/New_York for US as the hypotheses being tested. The latter normally displays as 20:15 Copenhagen and as 19:15 during the short US/EU DST mismatch. Saxo calendar/state checks still suppress closed or abnormal sessions.

### Add Only The US Shadow Pulse

Not selected. Both regions receive a comparable observation period; the evidence may still support promoting only US, keeping both shadow-only, or retiring either pulse.

## Related

- [Roadmap](../roadmap.md)
- [Hermes self-improvement](../concepts/hermes-self-improvement.md)
- [Current system architecture](../concepts/current-system-architecture.md)
- [Hermes agent contract](/Users/lindau/codex/rust_daytrader/docs/hermes-agent.md)
