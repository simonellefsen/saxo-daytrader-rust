---
type: decision
tags:
  - daytrader/wiki
  - decisions
  - decision-reports
  - hermes
  - observability
updated: 2026-08-20
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

Implementation underway. Phase 0, Phase 1, Phase 2, Phase 3's
capture/reference, daily-close maturity, fixed-reference FX/cost estimate,
and the first seven Phase 4 tuning slices are landed.
The two new shadow schedules may create observation-only Decision Reports at
their due times; they cannot queue orders, activate an experiment, or alter
Saxo execution.

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

1. **Landed 2026-08-19:** prevent pending-review experiment values from influencing Hermes allow/reduce/block/review advice. The manager preflight and MCP advisory view now expose only operator-approved or active statuses; pending rows are represented solely by a count, never a changed-variable value. The normal dashboard/lifecycle API remains the operator review surface.
2. **Landed 2026-08-19:** expire unsubmitted discretionary live/Saxo queue rows after `execution.discretionary_queue_max_age_minutes` (360 minutes). The executor applies the guard before obtaining a Saxo session, quote, precheck, or placement; it records `expired_local`, a sanitized queue-expiry event, and a fresh-decision remediation. It never expires `protective_stop` GoodTillCancel rows, broker-submitted rows, or ambiguous broker states.
3. **Landed 2026-08-19:** include current-mode/current-adapter pending and broker-working BUY reservations in symbol exposure and cash calculations across scheduler cycles. The remaining fraction of a partial fill reserves only its remaining DKK value; an active BUY without a reliable DKK value fails closed for new BUYs until reconciled. Terminal and different-execution-environment rows do not reserve the current runtime's budget.
4. **Landed 2026-08-19:** record report-supplied prices only as labelled context, then establish every new Hermes or manager-gate shadow baseline from an immediate read-only Saxo info-price refresh (with background retry while `awaiting_reference`). Persist the Saxo source and timestamp, exclude pre-provenance rows from aggregate learning evidence, and keep legacy rows visible as `legacy_unverified_reference` audit data.
5. **Landed 2026-08-19:** keep protective GoodTillCancel stops outside discretionary-order expiry rules. The stale-discretionary query and its atomic terminal update both exclude `strategy_type=protective_stop`; a regression confirms a stale protective row stays under protective-stop reconciliation while an equivalent discretionary row expires locally.

### Phase 1: Typed Shadow Pulse Contract

1. **Landed 2026-08-19:** add the typed server-owned pulse modes `ExecutionEligible` and `Shadow`.
2. **Landed 2026-08-19:** persist `pulse_mode` and `queue_eligible` with each report; do not infer either value from labels. The manager and manual immediate pipeline both fail closed unless they receive the exact execution-eligible pair.
3. **Landed 2026-08-19:** persist the server-owned pulse provenance used by scheduling: stable key from the pulse's configured local trading date, local and UTC due times, explicit schedule time zone, deterministic market-scope/calendar evidence, and a terminal per-cycle scheduler result (`not_due`, `due`, `missed_due_window`, or `invalid_schedule`). These result rows are scheduler history only, not Decision Reports, and have no execution authority.
4. **Landed 2026-08-19:** add regression coverage for date idempotency across a scheduler restart, empty holiday calendars, shortened sessions that close before the pulse offset, the EU/US DST mismatch, and the existing explicit shadow no-queue/no-Saxo boundaries. A terminal report of any status consumes its local-date key, preventing a retry from duplicating a provider request.
5. **Landed 2026-08-19:** classify every XNAS/XNYS pulse target as `regular`, `pre_market`, `post_market`, `night`, `pause`, or `closed` in America/New_York. Scheduled reports reject every non-regular target, including a calendar-provided continuous Night Session. The persisted pulse provenance records `target_session=regular` and documents extended-hours execution as not assessed. A pure capability check requires both a future Saxo client `AllowedTradingSessions` result and an instrument extended-hours flag; it has no execution effect and cannot alter shadow queue authority.

### Phase 2: Schedule And Generate The Reports

1. **Landed 2026-08-19:** configure the EU shadow report for 14:15 Europe/Copenhagen across the full European scope. The scheduler requires at least one scoped regular market to be currently tradable before provider submission.
2. **Landed 2026-08-19:** configure the US shadow report for 14:15 America/New_York. Its target is derived from the time-zone database, so Copenhagen display time follows DST rather than using a second fixed clock. The report persists `shadow`/`queue_eligible=false` and cannot reach the manager queue.
3. **Landed 2026-08-19:** include current portfolio, cash, scoped positions, active open/pending orders, persisted protective-stop coverage, active approved strategy baseline, Markov/Quiver/editorial/daily support-risk signals, and a bounded same-date opening-report projection in the shadow prompt. The queue/coverage and earlier-report reads are local, allowlisted, and read-only; raw Saxo payloads, account identifiers, raw provider responses, and prior prompt text never enter the provider context. The earlier report is explicitly untrusted analytical data and cannot alter policy, pulse authority, the Trading Manager, or Saxo execution.
4. **Landed 2026-08-19:** require each midpoint shadow report with an available same-date opening reference to declare either one or more concrete `material_change` entries or `no_new_information`. The Rust completion normalizer owns the persisted `shadow_change_assessment`: `no_new_information` clears selected assets, sentiment, suggested trades, and strategy-plan candidates; malformed comparisons do the same under `comparison_invalid`. A missing opening report records `not_available`; non-midpoint reports record `not_applicable`. Neither status can change shadow execution authority.
5. **Landed 2026-08-19:** add separate EU/US shadow scheduler-health rows alongside the existing opening and manual report rows. When an eligible shadow pulse passes its configured due window without a persisted report, the scheduler emits one date-scoped medium-severity operational alert. The alert is deduplicated by pulse key/local date and explicitly performs no provider retry, Trading Manager action, queue insertion, or Saxo request. Closed/non-eligible scope and report-present cases do not alert.

### Phase 3: Shadow Outcome Ledger

**Initial capture/reference slice landed 2026-08-19:** every valid BUY/SELL
suggested by a persisted server-owned shadow report now creates one idempotent
outcome-observation row. It records the report/pulse provenance, provider rank,
same-date opening-pulse presence, proposed quantity, instrument currency,
report-time technical/Markov/cash context, and the provider price as labelled
context only. It immediately enters the existing read-only Saxo info-price loop
as `awaiting_reference`; the first returned quote becomes the provenance-backed
local reference. No Trading Manager gate, Hermes request, execution order,
Saxo precheck, or Saxo order mutation is reachable. The initial record labels
gate/Hermes/FX/Quiver/Support-Risk/cost/maturity fields as not yet evaluated or
captured rather than inferring them.

**Daily-close maturity slice landed 2026-08-19:** after a daily-indicator run,
each provenance-backed reference is compared only with the next distinct
persisted trading-session closes for the same case-normalized symbol. The
ledger persists 1-, 5-, and 20-session directional observations, labelled
`collecting`, `preliminary`, or `mature`; weekends, holidays, missing coverage,
and the reference day itself cannot manufacture a session. This is an
observational price comparison for either BUY or SELL direction, not a fill,
execution simulation, realised P/L, cost estimate, or causal claim. The
maturity job reads/writes local database evidence only and has no Saxo,
provider, Hermes, gate, queue, or order authority.

**FX and estimated after-cost slice landed 2026-08-19:** when the first
read-only Saxo reference quote arrives, the ledger now captures only a fresh
local FX-cache basis (or records it as unavailable), never a static fallback.
It combines that fixed basis with the published exchange-minimum commission
schedule and configured per-side slippage to produce explicitly estimated
1/5/20-session after-cost directional outcomes. These outputs exclude actual
fills, broker fees, tax, later FX movement, and position changes; they are not
realised P/L or an execution simulation. Historical rows without a captured
basis remain unavailable rather than being backfilled from current FX.

**Decision-time context projection landed 2026-08-19:** new candidate rows
now extract a compact, symbol-matched technical/Support-Risk, Markov, and
Quiver snapshot from the exact persisted decision prompt, together with its
cash plan, market-scope position concentration counts, and approved strategy
baseline identity. No later signal, price, position, or prompt is queried, so
the record cannot acquire hindsight. The concentration projection is context
only, not the Trading Manager's portfolio-wide gate; a pre-trade candidate
entry thesis remains explicitly unavailable because no order/fill existed.

**Observed intraday-excursion slice landed 2026-08-19:** each active shadow
candidate now retains its own small trail of the existing read-only Saxo
infoprice samples and derives the maximum favourable and adverse *observed*
directional movement from the reference price. It stops collecting when the
20-session outcome matures. Coverage identifies the number and time span of
samples and explicitly does not claim continuous intraday high/low coverage,
a fill path, realised P/L, or execution quality.

**Decision-time signal-gate slice landed 2026-08-20:** each new candidate now
stores a stable `technical`, `markov`, or explicit not-evaluated code and a
bounded replay over the server-generated technical/Markov prompt snapshots
saved with that report. It can show whether the saved technical direction or
the configured Markov-starter fallback signal would clear its own signal rule.
Market/session, exclusions, order shape, cash, sellability, risk, holdings,
concentration, cost, selection, and Hermes are deliberately omitted and named
in every result. A signal clear is therefore not a Trading Manager evaluation,
queue approval, broker precheck, execution simulation, or Saxo action.

**Hermes record-only slice landed 2026-08-20:** each new shadow report with
candidate rows now submits a separate, server-owned
`shadow-decision-advice-<report-id>` Hermes session. The compact input is
limited to durable report/candidate evidence and report-time approved-policy provenance;
the stored result records only matching/action/effect and self-check state.
The mode is hard-coded record-only even where the normal Trading Manager uses
conservative advice. Neither successful advice nor a timeout can reach a
manager gate, execution queue, broker precheck, or Saxo mutation.

Remaining Phase 3 work, for each suggested BUY or SELL:

- pulse and report provenance;
- candidate rank and whether the symbol appeared in the earlier pulse;
- reference timestamp, local price, currency, DKK conversion basis, and proposed quantity; **landed for newly referenced rows**;
- report-time technical, Markov, Quiver, Support Risk, cash, scoped concentration, and strategy-baseline snapshot; **landed for newly recorded rows**. Candidate entry thesis remains explicitly unavailable before an approved order/fill;
- bounded decision-time technical/Markov signal-gate result and stable gate code; **landed for new rows, with all current-state manager gates explicitly omitted**;
- Hermes record-only effect and report-time approved-policy provenance; **landed for new rows, with unavailable advice retained as missing evidence**;
- maximum adverse and favourable observed excursion when retained intraday samples are available; **landed for new rows, with sampled-coverage limits**;
- an estimated after-cost outcome, explicitly separated from realised P/L; **landed for rows with a captured fresh FX basis**.

Shadow observations never alter holdings, cash, capacity, gates, experiments, or orders.

### Phase 4: Tuning Dashboard

Add a typed, read-only tuning payload and a lazily loaded Tuning tab. Reuse existing evidence builders rather than recomputing incompatible versions in the UI.

**Landed 2026-08-20 (first slice):** a 30-day, typed pulse comparison covers
EU open, EU shadow, US open, and US shadow. It reports per-pulse report count,
terminal report success, shadow candidate/reference coverage, 1/5/20-session
observation counts, and the five-session estimated-after-cost positive rate.
Execution-eligible rows explicitly show that comparable outcome attribution is
not part of this initial shadow-ledger slice; their figures are never blended
with observation-only results. The payload carries its as-of timestamp,
window, denominator counts, `collecting`/`preliminary`/`mature` status, and
read-only safety statement. It creates no composite score and does not invoke
provider, Hermes, broker, gate, or order paths.

**Landed 2026-08-20 (second slice):** the same payload now adds a distinct
execution-evidence lane for the two execution-eligible opening pulses. It uses
only local execution orders, reconciled fills, ledger rows, and persisted daily
closes created within the identical 30-day window. BUY fields show
equal-weighted one/five-session directional movement after reconciled fills;
SELL fields show reconciled local-ledger gain, commission, and tax. Those are
separate evidence types, explicitly not shadow results, not realised BUY P/L,
and not a causal performance claim. The existing bounded Execution-tab query
keeps its original recent-history behaviour; the Tuning call applies its own
window cutoff.

**Landed 2026-08-20 (third slice):** shadow candidates now expose a canonical
symbol novelty/repeat breakdown only when their same-market opening report was
persisted. Candidates without that reference are excluded from the novelty
denominator rather than presumed new. This is candidate overlap evidence, not
a judgement that a zero-candidate report contained no new market information,
and it remains local read-only data with no provider, Hermes, gate, queue, or
Saxo authority.

**Landed 2026-08-20 (fourth slice):** the Tuning payload separately counts the
persisted decision-time gate source (`technical`, Markov fallback, or not
evaluated) and result (clear, blocked, insufficient, or unclassified) for
each shadow pulse. It deliberately exposes unclassified historical records
rather than assigning them a result. These compact replay fields are not a
Trading Manager decision, queue result, broker precheck, execution simulation,
or Saxo action.

**Landed 2026-08-20 (fifth slice):** a separate shadow-only Tuning lane now
counts the persisted record-only Hermes effect, context-self-check coverage,
approved-policy-source coverage, and unknown legacy effects. No category is
treated as approval or prevented quantity, and unavailable/not-requested
evidence remains visible. The lane cannot change a manager gate, queue,
broker precheck, execution simulation, or Saxo action.

**Landed 2026-08-20 (sixth slice):** a shadow-change lane reports only the
server-normalized material-change, `no_new_information`, opening-reference
unavailable, not-applicable, invalid, missing, and unknown assessment states.
Its `no_new_information` rate uses only reports with an available opening
comparison; candidate absence is never used to infer the status. This is local
report evidence only and cannot affect a manager gate, queue, broker precheck,
execution simulation, or Saxo action.

**Landed 2026-08-20 (seventh slice):** a shadow-only Support/Risk lane reports
the candidate's saved decision-time snapshot coverage, low/moderate/high
break-risk bucket, and complete-context average break risk, confidence, and
history coverage. Missing and unknown snapshots remain visible. It is
observational context rather than a forecast, manager risk gate, queue result,
broker precheck, execution simulation, or Saxo action.

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
