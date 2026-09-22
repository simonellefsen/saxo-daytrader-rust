# The comparison grammar, and what it cannot read

Method version **`n8-2026-09-22`**, grading version **`v13`**. Frozen as the
evaluation baseline: the limitations below are measured and recorded rather
than fixed, and the method must not move during an evaluation run.

## Why it was rewritten

`n5` read the relation between a figure and a field by looking at the words
*between* them and rejecting a blocklist of bad ones. Anything not on the list
fell through to equality. Against a stored RSI of 71.049 it returned:

| Note | `n5` | |
|---|---|---|
| `It is false that RSI is above 70` | `above` → **matches** | negation sits before the field |
| `RSI is above 70 tomorrow` | `above` → **matches** | tense sits after the figure |
| `RSI >= 70` | `equals` → **differs** | the operator is punctuation, and punctuation was discarded |
| `RSI is above or below 70` | `below` → **differs** | two relations, of which the last won |

Three of the four returned agreement. That is the invisible direction to be
wrong in: a false `differs` gets investigated, a false `matches` never does.

## What `n6` accepts

A closed grammar. A figure is compared only inside one of these forms, and
everything outside them is recorded `unsupported_construction` and left alone.

**The clause must assert plainly.** No negation, no past tense, nothing
prospective or conditional anywhere in the clause holding the figure.
Sentential operators scope over a clause, so reading them from the gap between
field and figure misses every one that sits outside it. Clauses are bounded by
`.` `;` `:` `,` `!` `?` and em/en dashes; a `.` between two digits is a decimal
point.

**And one of:**

- `field <gap> figure` — equality, where every word of the gap is a connective,
  a word of degree, or part of the field's own name.
- `field <gap> <comparator> figure` — `above` or `below`, where the comparator
  is the word immediately before the figure (a trailing `than` is allowed) and
  the rest of the gap is plain.
- `field <gap> <symbol> figure` — `>` `>=` `<` `<=` `=` and the Unicode
  spellings, immediately before the figure. `>=` and `<=` are `at_least` and
  `at_most`, not `above` and `below`: an inclusive bound is satisfied at the
  boundary and folding them together would call a correct note wrong.
- `figure field` — equality. "above 446 EUR support" says 446 *is* the support
  level; reading it relationally would ask whether 446 exceeds itself.

`relation` is now stored on every check and shown to the model, so
`unsupported_construction` (the field is known, the wording was not read) is
distinguishable from `not_read` (no field could be attributed at all), and both
from a comparison that actually happened.

## What it costs, measured

Two different numbers, and they must not be confused.

**Grammar acceptance.** Against all 1,105 stored candidate notes (1,715
figures, 1,320 attributed):

| | |
|---|---|
| accepted as `equals` | 1,254 |
| accepted as `above` | 13 |
| rejected (`unsupported_construction`) | **53 — 4.0% of attributed** |

This measures only whether the grammar could read the claim. The harness has no
evidence to check anything against, so it stops at the parse.

**Figures actually compared.** From production at `n7`, 2,076 stored checks:

| verdict | | |
|---|---|---|
| `matches` | 1,345 | compared |
| `differs` | 25 | compared |
| `unattributed` | 407 | no field |
| `implausible_attribution` | 139 | field an order of magnitude off |
| `uncertain_attribution` | 92 | contested attribution, or wording the grammar refused |
| `not_in_evidence` | 41 | field absent from the snapshot |
| `not_a_field_value` | 27 | a horizon or a share count |

**1,370 of 2,076 compared — 34% of figures found are never checked**, eight
times the grammar's own rejection rate. 179 of those carry a relation the
grammar *did* recognise and still ended uncompared. `relation` is the parsed
claim; `verdict` is the outcome; only `matches` and `differs` mean a comparison
happened. The instruction shown to the model says so explicitly.

Reproduce the first table with the `#[ignore]`d harness in
`src/jev_numeric.rs`:

```
JEV_NOTES_PATH=notes.json cargo test grammar_coverage -- --ignored --nocapture
```

The abstentions divide roughly into thirds. **Correct** (~15): genuine hedges —
"rsi near 74", "markov long signal around 0.55" — and sign words the grammar
deliberately does not read, "markov is positive at 0.1634". **Attribution
noise** (~12): the naming phrase is 40 characters away and the gap begins
mid-word — `gap:fication`, `gap:fied`, `gap:ed`, `gap:st`. **Over-abstention**
(~26): a real claim the grammar declines, either because a substantive word
sits in the gap ("markov edge (signed_signal 0.09)", "markov long bias 0.18")
or because an operator elsewhere in the clause suspends it ("rsi above 75
argues for waiting for a pullback **before** adding").

## Attribution defects, named rather than fixed

Recomputing the whole history at `n6` produced 24 disagreements across 163
reports (1.25% of checks, against 1.21% at `n5` — the grammar neither created
nor cleared them). Reading all 20 distinct ones, **most are this checker's
attribution failures, not report errors.** They are listed here with their
production examples so an independent evaluation can score them as what they
are, instead of rediscovering them one at a time.

| # | Defect | Production example | Effect |
|---|---|---|---|
| A | A field name written with an underscore does not match its keyword: `bull_prob` never matches "bull prob" | #101 AMD `markov bull_prob 0.72 / signed_signal 0.61` | 0.72 given to `signed_signal` (0.612) → false `differs` |
| B | The `N/M` remap needs the word "confluences" adjacent, so a bare ratio is not recognised | #103 BAC `bullish 4/3, markov long 0.555` | the 3 given to `markov.signed_signal` → false `differs` |
| C | "support" as a verb matches the support field | #256 ALV `the 453.0 EUR daily close support a controlled limit` | 453.0 given to `nearest_support` (434.6) → false `differs` |
| D | `~` before a figure is not read as an approximation marker | #106 ARKK `rsi ~55` against 61.34 | compared as an exact equality |
| E | A field named further away can still win when the near phrase is not in the field table | #158 BAC `markov long 0.576 and quiver bullish` | 0.576 given to `quiver.signal` (0.355) → false `differs` |
| F | Attribution ignores clause boundaries; the 40-character window crosses them | seen in testing, not in the corpus | a field in the next clause can claim a figure |

Fixed in `n7`, because it is one rule rather than six: a threshold an order of
magnitude from its field is `implausible_attribution` whatever the relation.
"top held conviction holding trading above 525 DKK with +0.749 Markov Bull
regime" gave a price to `markov.conviction` and, because the magnitude guard
protected equality only, reported a disagreement between 525 and 0.749 rather
than naming the misattribution.

The rest are **left in place deliberately.** Each is small and I could fix them
now, which is exactly the pattern four review rounds have criticised: patch,
declare success, have the next review find what the patch missed. They are
frozen as part of the baseline so the evaluation measures the grader that
exists.

Of the 20, the ones that look like genuine report discrepancies rather than
checker failures are #106 BAC (`rsi 58` against 67.76), #183 DDOG (`5
confluences` against 4), #255 BMW (`6.0% downside-to-support` against 7.00),
and #286 FLS (`593 DKK support` against 551.5, already independently
confirmed). Four out of 1,914 checks. None has been adjudicated, and
`agreement` is not `accuracy`.

## Fixed in `n8`: contradictory bounds both held

Inclusive bounds borrowed equality's rounding and truncation allowance. Against
a stored RSI of 70.9, **`RSI <= 70` and `RSI > 70` were both `matches`** —
because 70 is a legitimate truncation of 70.9, so the boundary test passed while
the strict comparison used the raw value. `RSI >= 71` and `RSI < 71` likewise.

A grader that accepts two mutually exclusive claims agrees with whatever it is
shown, which is the failure this module exists to prevent.

A threshold is now compared at face value: `at_least` is `stored >= written`,
`at_most` is `stored <= written`, with no allowance. The allowance belongs to
equality alone, where a note quotes a stored value short — "295 DKK support"
for 295.733 — and a bound is not a value quoted short. `RSI >= 70` against a
stored exactly 70.0 still matches where `RSI > 70` does not.

## Reading the sufficiency numbers

Grade versions cover overlapping populations. `v11` and `v12` are the **same
120 reports**, so pooling them repeats observations rather than enlarging the
sample; a version-specific, subject-deduplicated count is the only honest one.

`v12` alone: **244 flagged out of 450 answered, 0 unanswered, across 120
reports** — 54%. That is a review workload, not a finding that half the reports
are defective. None of it is adjudicated.

## Known limitations, not fixed

1. **An operator hidden behind commas does not reach its figure.** "RSI was, at
   one point, above 70" reads as present tense. Commas bound clauses because
   these notes are comma-spliced lists of independent observations, and the
   alternative — scoping to the sentence — abstained on "currently 9.9% above
   nearest support, awaiting a tighter entry", a stated fact followed by an
   intention.
2. **An operator joined by `and` still suspends the figure.** "five-day markov
   signal is only 0.138 and does not justify adding exposure now" abstains
   although the figure is stated as fact.
3. **Attribution crosses clause boundaries.** The 40-character window ignores
   punctuation, so a field named in the next clause can claim a figure. Seen in
   testing, not observed in the corpus.
4. **Signs are not read.** "markov is positive at 0.1634" abstains rather than
   check the sign, because the grammar compares magnitudes written as quoted.
5. **The whitelist is English and hand-built.** A new phrasing abstains until
   someone adds it, which is the intended failure direction but means coverage
   drifts as the report model's wording drifts. The harness above is how that
   is detected.

## What this does not establish

Nothing here measures whether the grader is *right*. It measures whether the
comparison grammar reads what a note says. Every stored disagreement it
produces still needs adjudication, and the sensitivity of the whole grader —
whether it catches a note that genuinely misstates the evidence — remains
unmeasured, because every frozen label in both adjudication samples was `fair`.
That needs seeded known-faulty examples and an independent adjudicator, neither
of which exists yet.

No trading parameter depends on any of this.
