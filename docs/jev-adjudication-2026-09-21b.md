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

### The three disagreements, adjudicated

**Case 1 — #83 ADI, `overstated` (p=0.58, margin 0.299).** *"Medium-term
momentum candidate with Markov long bias"* against 2 confluences on a minimum of
3, sentiment HOLD, and +0.0976 in a Sideways state. **The grader is right and I
was wrong.** I noted the sub-minimum confluence count while labelling and still
wrote `fair`.

**Case 5 — #110 NNIT, `overstated` (p=0.46, margin 0.010).** *"Bearish technical
trend, SELL sentiment, high support-break risk, and strongly negative Markov
signal."* Trend and sentiment are exact. The other two assert values the stored
prompt does not contain at all — that report carries neither break-risk nor
Markov data.

Neither of us is right. The correct category is **unsupported assertion**, and
the wording taxonomy has no slot for it, so such a note can only come back
`fair` or `overstated`. That is a gap in my design, not a grader error, and it
is the most useful thing this run produced.

**Case 11 — #163 AAKI, `overstated` (p=0.57, margin 0.209).** *"Markov long
signal just above threshold"* against 0.1592 and a 0.15 gate. That is precise,
and the allocation comment is out of scope. **Grader false positive.**

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

**Zero genuine numeric findings in 30 held-out cases.** Every disagreement the
checker produced on material it had not been tuned against was its own.

One correct catch: **#165 V** returned `not_in_evidence` for *"Markov long
0.542"* where the prompt carries no Markov signal — an unsupported assertion,
surfaced on the numeric side exactly as the rubric says it should be, on a note
never used to build the checker.

## What this establishes

Wording agreement of 26/29 on genuinely held-out notes, with blindness
structural rather than procedural — there was no verdict in existence when the
labels were recorded.

It also establishes that the margin rule does not hold, that the wording
taxonomy cannot express an unsupported assertion, that the relational gap is
real and reachable, and that "5-day" defeats the attributor. Three of those four
are defects this run found and the previous one could not, which is the argument
for held-out data rather than a result about the grader.

The adjudicator remains the agent that wrote the grader. No trading parameter
changed.
