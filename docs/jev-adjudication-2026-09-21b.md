# Held-out blinded pass — result

Design fixed in `jev-adjudication-2026-09-21b-preregistration.md` (commit
`c4e7726`). Labels recorded in `c110cae`..`0e5d5b3` **before the grader ran on
any of these reports**. Complete data: `jev-adjudication-2026-09-21b-cases.json`.

30 candidates from reports 83–271, none read while building or fixing the
numeric checker. All 30 pre-labelled `fair`.

## The pre-registered hypothesis failed

Carried over unchanged: *flag-level disagreements sit at margin ≤ 0.25.*

| margin | judged | disagreements |
|---|---|---|
| ≤ 0.25 | 6 | 2 |
| > 0.25 | 23 | **1** — predicted 0 |

Case 1 disagrees at **0.299**. On the previous sample the split was 4/5 below
and 0/23 above, which looked like a clean rule. It is not one. The margin still
carries signal — 33% disagreement below the line against 4% above — but as a
review trigger, not a boundary, and the earlier 0/23 was a small-sample artifact
that a second sample dissolved.

This is why the threshold had to be applied unchanged rather than re-fitted.

## Wording: 26 of 29 agree

One case was never judged. Report #101 listed 14 candidates against a
`MAX_JUDGED_CANDIDATES` of 10, so PLTR fell outside the budget and was reported
as `beyond_question_budget`. The coverage mechanism worked; the sampling frame
did not anticipate it.

### The three disagreements

**Corrected.** The first version of this section resolved all three against the
frozen labels after seeing the verdicts. That is deference, not adjudication,
and it is what the pre-registration existed to prevent. The frozen `fair` labels
stand in the totals; what follows is recorded as post-reveal interpretation in
`postreveal_*` fields and excluded from them.

**Case 1 — #83 ADI, `overstated` (p=0.58, margin 0.299).** *"Medium-term
momentum candidate with Markov long bias"* against 2 confluences on a minimum of
3, sentiment HOLD, +0.0976 in a Sideways state.

I wrote that the grader was right and I was wrong. Withdrawn. The note does not
claim entry eligibility, and being below an entry requirement does not by itself
contradict "medium-term candidate" when the trend is bullish and the signal
positive. **Undetermined; needs independent adjudication.**

**Case 5 — #110 NNIT, `overstated` (p=0.46, margin 0.010).** The note is
*"Allowed-market portfolio holding with SELL technical sentiment, bearish trend,
and strongly negative Markov signal; full exit is preferred."*

The first version of this file also attributed *"high support-break risk"* to it.
**It does not say that** — that phrase belongs to case 17, #194 ALK-B. The
misquote invented a second unsupported claim.

The finding survives in a narrower and more useful form. The note asserts a
strongly negative Markov signal and that report's prompt carries no Markov data
at all, so the assertion is unsupported — and it contains **no number**, which
is why the numeric checker cannot reach it. Evidence sufficiency is a separate
axis from wording strength, and the taxonomy has no slot for it, so such a note
can only come back `fair` or `overstated`.

**Case 11 — #163 AAKI, `overstated` (p=0.57, margin 0.209).** *"Markov long
signal just above threshold"* with `signed_signal = 0.1592`.

I called this a confirmed grader false positive because 0.1592 is just above a
0.15 gate. That was unjustified twice over. The gate is **time-varying** —
`min_signed_signal` was 0.15 until it was recalibrated to **0.20 on 2026-08-31**,
and this report is from 2026-07-09, so 0.15 was in force. I happened to be
right, but I recalled the figure rather than checking it, and supplying today's
0.20 would have been wrong.

More to the point, **the stored prompt carries no threshold**: `markov_method`
holds only `signals` and `latest_run`. The grader had no gate to evaluate "just
above threshold" against. Missing context and faulty judgement are different
defects, and this cannot be called the second until the first is fixed.
**Undetermined.**

## Numeric: 2 disagreements, both mine

| | |
|---|---|
| matches | 28 |
| differs | **2 — both attribution failures** |
| not_in_evidence | 1 |
| implausible_attribution | 5 |
| unattributed | 10 |

**#117 BAC** — `rsi14` quoted 70.0 against 71.049. The note says *"RSI is above
70"*. That is a **relational** claim and 71.049 satisfies it. The checker
compares for equality. This is the exact gap identified when case 28 of the
previous run was retracted, now confirmed on data it was never fitted to.

**#185 DSV** — `signed_signal` quoted 5.0 against 0.659. The note says
*"5-day Markov continuation signal of 0.6590"*. The **5 is a horizon**, and it
was attributed to the signal. The 0.6590 matched correctly.

**Neither flagged numeric contradiction was genuine.** That is the claim the
data supports. It does **not** establish how many genuine errors the checker
missed — nothing here measures that. Alongside the two false flags sit 10
`unattributed`, 5 `implausible_attribution` and one excluded candidate, all of
which are figures the checker declined to rule on rather than verified.

One correct catch: **#165 V** returned `not_in_evidence` for *"Markov long
0.542"* where the prompt carries no Markov signal — an unsupported assertion,
surfaced on the numeric side exactly as the rubric says it should be, on a note
never used to build the checker.

## What this establishes

Wording **agreement** of 26/29 on genuinely held-out notes, with blindness
structural rather than procedural — there was no verdict in existence when the
labels were recorded.

Agreement is not accuracy. And every frozen label is `fair`, so this sample
contains no known-faulty wording and therefore **cannot measure sensitivity** to
any. It measures how often two judges concur on notes that look sound.

Three disagreements is also too few to establish a review-priority rule; the
margin split is suggestive and nothing more.

It also establishes that the margin rule does not hold, that the wording
taxonomy cannot express an unsupported assertion, that the relational gap is
real and reachable, and that "5-day" defeats the attributor. Three of those four
are defects this run found and the previous one could not, which is the argument
for held-out data rather than a result about the grader.

The adjudicator remains the agent that wrote the grader. No trading parameter
changed.
