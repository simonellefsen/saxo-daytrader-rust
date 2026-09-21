# Adjudication run — 2026-09-21

**Revised after review.** The first version of this file overstated its result
and contained a wrong finding. Corrections are marked below rather than edited
away. Complete case-level data: `jev-adjudication-2026-09-21-cases.json`.

Rubric: `docs/jev-adjudication-rubric.md`, frozen before any example was read.

## What this is, and is not

The adjudicator is **Claude Opus 5 — the same agent that built the grader.**
This is one AI system checking another it wrote. It is not independent human
ground truth, and a self-adjudicated agreement rate is worth less than the
number suggests.

## Method

111 unique (symbol, note) pairs carried v8 grades. Every fourth was taken,
ordered by report id then symbol, giving 28. Each was read against the
technical, Markov and Quiver evidence from that report's own stored prompt, and
a category recorded before the verdict was looked at.

**26 of 28 were blind.** Cases 24 (#309 ALV) and 27 (#311 TSM) were not: those
verdicts had been seen earlier while debugging. They are flagged in the dataset.

## Two tasks, reported separately

v8 instructs the model to judge **wording** and explicitly not to re-check
arithmetic. Numbers are settled by `jev_numeric`. The first version of this
report compared adjudicator labels that mixed both against a model doing only
one, which is not a like-for-like comparison.

### Wording agreement

Computed from `prereveal_label` only. The dataset keeps any later revision in
separate `revised_*` fields with its own blindness flag, so a label changed
after the verdict was known cannot be counted as a blind one.

| basis | exact category | flag vs no-flag |
|---|---|---|
| **26 blind cases** | **23 / 26** | **24 / 26** |
| all 28, including the two not blind | 23 / 28 | 26 / 28 |

| | |
|---|---|
| Grader flagged, adjudicator did not | 2 (cases 8, 19) |
| Adjudicator flagged, grader did not | **0** |

The blind figure is the one that means anything. The two non-blind cases (24,
27) were both flagged by both sides, so including them only inflates the
flag-level agreement.

Cases where both flagged but chose different categories count as category
disagreements, because the adjudicator recorded **ambiguous** and the rubric
says an ambiguous case stays ambiguous rather than being rounded to whichever
category the grader chose.

### Numeric correctness

Not measurable from this run. The v8 grades were produced by **three different
numeric methods** under one label, because the arithmetic was corrected several
times without bumping a version the worker gates on. Production retained
findings the code had already been fixed to stop producing:

| report | stored finding | status |
|---|---|---|
| #311 | 9.9% compared against a support price of 617.21 | fixed in code, stale in storage |
| #304 | 23.13 against 23.135 recorded as a disagreement | fixed in code, stale in storage |
| #279 | "6/3 confluences": the 3 compared against the count of 6 | not fixed at the time |
| #286 | 593 DKK called support where support is 551.5 | **genuine, independently confirmed** |

So "87 figures verified" meant "87 values accepted by whichever parser and
tolerance policy happened to be stored", and cannot be read as a correctness
rate. The method now carries `NUMERIC_METHOD_VERSION` and is recomputed from
stored evidence when it changes, with no provider call.

## The margin

`margin` is the selected option's probability minus the runner-up's.

| margin | cases | flag-level disagreements |
|---|---|---|
| ≤ 0.25 | 5 | **2** (both of them) |
| > 0.25 | 23 | **0** |

Both grader false positives sat at or below 0.25. Treat 0.25 as a **hypothesis
selected from this sample**. Testing it means applying it unchanged to fresh
cases; re-picking a threshold on each new sample would validate nothing.

## The two grader false positives

**Case 8 — #283 TSM** (`misdescribes_category`, p=0.52, margin 0.20).
*"strong Markov Sideways-to-Bull conviction (+0.3506)"* with `state=Sideways`.
The note names Sideways explicitly and describes movement toward Bull, which is
the situation. It is the most precisely worded note in the sample, and it was
the one flagged.

**Case 19 — #299 CRM** (`overstated`, p=0.49, margin 0.07).
Both figures match; the unmentioned moderate break risk is an omission, not a
misstatement.

## Retracted: case 28 was not a finding

The first version called #313 CHEMM — *"trading above 525 DKK"* against a daily
indicator close of 510.0 — a numerical contradiction, and called it the most
useful result of the run. **It is wrong.**

The same stored prompt carries `current_price_local = 525.0`, quoted at
`12:25:01Z`; the report was created at `12:25:17Z`, sixteen seconds later. The
note quotes the live price. There is no 15-DKK discrepancy. What remains is
that "above 525" is not strictly true of a quote of exactly 525, which is
imprecision, not error. Re-adjudicated **ambiguous**.

This also disposes of the conclusion drawn from it. A relational checker that
compared "trading above X" against the daily close would have produced this
error systematically, on every held position quoted at a live price. Relational
assertions need both the right operator **and** the right time-specific price
source; the daily close is the wrong source for a note written against a quote.

The retraction itself was made **after** the verdict and the new evidence were
both known, so it is recorded as `revised_label` with `revision_blind: false`
and is excluded from the blind totals above. Its pre-reveal label,
`numeric_contradiction`, is what the totals use.

## What this establishes

On 28 notes, a self-adjudicating agent agreed with the grader on flagging in 26
cases, both grader false positives sat at low margin, and the grader missed
nothing the adjudicator caught.

It does not establish accuracy on unreviewed notes, numeric correctness at all,
independence, or that any grade should influence a trading decision. The next
run should use fresh cases not used to tune the parser, and ideally an
adjudicator that did not write it.

A recomputation under `n2` makes the stored measurements **consistent**. That is
not a numeric correctness rate, which needs independently checked examples and
has not been produced.

No trading parameter changed.
