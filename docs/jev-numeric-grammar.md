# The comparison grammar, and what it cannot read

Method version **`n6-2026-09-22`**, grading version **`v11`**. Frozen as the
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

Against all 1,105 stored candidate notes (1,715 figures, 1,320 attributed):

| | |
|---|---|
| compared as `equals` | 1,254 |
| compared as `above` | 13 |
| abstained (`unsupported_construction`) | **53 — 4.0% of attributed** |

Reproduce with the `#[ignore]`d harness in `src/jev_numeric.rs`:

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
