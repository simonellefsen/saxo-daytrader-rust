# Jev grading: adjudication rubric

Frozen 2026-09-21, before any example was reviewed under it. Change it by
amending this file with a dated note, never silently — a rubric edited after
seeing the cases it is applied to measures nothing.

## Why this exists

The grader emits categories. Whether those categories are *right* is a separate
question, and it cannot be answered by the grader, by its confidence, or by two
model versions agreeing. It is answered by a human reading the note against the
evidence the report was written from.

This file fixes what the categories mean so that reading is repeatable.

## How to adjudicate

1. Open the note and the candidate's evidence block. **Do not look at the
   grader's verdict first.** Record your own category, then compare.
2. Categorise every assertion in the note separately. One note can contain
   several, in different categories.
3. Assertions about portfolio capital, holdings, unrealised profit, and trading
   costs are **out of scope**. Their supporting material is deliberately not in
   the evidence. Do not count them in any category.
4. If two readings of an assertion are both defensible, record it as
   **ambiguous**. Ambiguity is a result, not a failure to decide, and forcing it
   into another category is how a judgement call becomes a false finding.

## Categories

### 1. Numerical contradiction

A figure quoted in the note disagrees with the evidence field it names, at the
precision the note used.

Settled by `jev_numeric`, not by a model. A figure quoted to three decimals
asserts only what three decimals carry, so `0.061` against `0.06147651902511747`
agrees.

Adjudicate these only to check the *attribution* — that the figure was matched
to the field the note meant. A figure the module could not attribute is
`unattributed` and is not a finding.

### 2. Unsupported assertion

An in-scope assertion with no counterpart in the evidence at all.

This category does **not** by itself say whose fault it is. It may be a gap in
what the grader supplies, or an assertion the report could not have supported.
Record which, in a sentence, when adjudicating.

### 3. Ambiguous or overstated wording

A characterisation stronger than the evidence supports, or one that reads
differently under two defensible interpretations.

Both of the known examples belong here rather than in category 1:

- *"consolidating securely above 446 EUR support"* where `break_risk` is 0.392
  and its label is `moderate`. The price and support figures are exactly right.
  Whether "securely" overstates a moderate break risk depends on what "securely"
  means, and nothing here defines it.
- *"positive Markov state"* where `state` is `Sideways` and `signed_signal` is
  `+0.2467` with `direction: long`. This may assert that the categorical regime
  is positive, which would be wrong, or may be loose phrasing for a positive
  directional signal, which would be right.

Neither is a factual contradiction, and calling either one a confirmed detector
success overstates what has been established.

### 4. No checkable claim

The note makes no in-scope assertion: it is empty, purely numeric with every
figure already covered by category 1, or says only out-of-scope things.

Exists so that a note with nothing to check cannot be recorded as clean, which
is a vacuous success rather than a finding.

## Sampling

Review examples drawn from **every** predicted category, including `fair`. A
review that only reads flagged cases can measure false positives and can never
measure false negatives, and the false-negative direction — wording that should
have been flagged and was not — is the one that matters for trusting a grade.

## What an adjudicated set does and does not establish

It establishes how often the grader's category matches a human's on the cases
reviewed. It does not establish that the reports are accurate, that the grades
predict anything about returns, or that any grade should influence a trading
decision. No trading parameter changes on the strength of this.
