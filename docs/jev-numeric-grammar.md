# The comparison grammar, and what it cannot read

Method version **`n12-2026-09-24`**, grading version **`v16`**. `n10-2026-09-22`
was the evaluation baseline; the controls are keyed to it, and its v4 result
stands as scored. `n11` widened the evidence and `n12` added two names (both
below); each change is measured claim by claim against the version before it.

## `n12`: two names the checker did not know

Both were found by the controls reconciliation. They are measured over all
1,445 claims in the frame in `jev-numeric-n12-changes.json`, from two recorded
runs of committed code: `n11` at `427a44f`, `n12` at `3e88dfc`.

- **"R/R" is `reward_risk`.** The separator rule turned "R/R" into "r r", which
  no keyword spelled. The fix adds a general rule: an initialism — a keyword
  made only of single letters — names a field only when it stands alone,
  because "r r" also sits inside "higher rsi". Single letters never become gap
  words.
- **The Markov horizon, written as a duration, is `markov.horizon_days`.** A
  figure followed by "day" stays a quantity everywhere else, and that
  exclusion is load-bearing. The horizon is read by a closed rule instead: a
  whole, unsigned `N-day` / `N day(s)` followed by `horizon`, `markov`,
  `signal` or `signed`, or written `<figure> over N days`, in a clause that
  names a Markov field. It is settled before attribution and uses no phrase.
  The gap grammar sets aside only a horizon the checker itself read this way.

| `n11` → `n12` | claims |
|---|---|
| `not_a_field_value` → `matches` (horizon) | 8 |
| `unattributed` → `matches` (R/R) | 4 |
| `uncertain_attribution` → `matches` (the Markov signal beside a horizon) | 2 |
| `uncertain_attribution` → `matches` (R/R; was contested by `rsi`) | 1 |
| to `differs`, or a `matches` lost | **0** |

Compared claims go from 1,213 to **1,228 of 1,445 (85.0%)**.
- **"R/R":** all 5 in the notes are now read.
- **Horizons:** all 13 in the notes fit the rule. 8 are in the frame and match;
  the other 5 sit in candidates that never reached the checker.
- **Still quantities:** the 14 remaining `not_a_field_value` are share counts,
  and holding periods like "1–3 month" and "2-week".

**What it can still get wrong.** A Markov clause that says "5-day signal" and
means a lookback rather than the horizon would be compared against the horizon.
No stored note does this. "Over N days" is read only straight after a quoted
figure, which is the narrowest form that covers the one stored case.

## `n11`: the evidence

`n11` changes what the evidence contains, not how anything is read. A symbol
missing from the prompt's compact Markov list is now read from the full signal
rows the prompt embedded under `markov_method.latest_run` until 2026-08-03.
Each candidate's `markov_source` records which list its evidence came from.
The effect, claim by claim, is in `jev-markov-provenance.md`. (An earlier
version of this header said grading `v15`; it has been `v16` since the Markov
state instruction was fixed.)

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
figures, 1,456 attributed):

| | |
|---|---|
| accepted as `equals` | 1,401 |
| accepted as `above` | 13 |
| rejected (`unsupported_construction`) | **42 — 2.9% of attributed** |

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

## Attribution defects, all six now fixed

Recomputing the history at `n6` produced 24 disagreements across 163 reports.
Reading all 20 distinct ones, most were this checker's attribution failures
rather than report errors. They were recorded here first and fixed afterwards,
one at a time, each with its production example.

| | defect | production example | fixed |
|---|---|---|---|
| A | a name written with an underscore does not match its keyword | #101 AMD `markov bull_prob 0.72` | `n9` |
| B | the `N/M` remap needs the word "confluences" adjacent | #103 BAC `bullish 4/3, markov long 0.555` | `n10` |
| C | "support" as a verb matches the support field | #256 ALV `the 453.0 EUR daily close support a controlled limit` | `n10` |
| D | `~` before a figure is not read as an approximation marker | #106 ARKK `rsi ~55` against 61.34 | `n10` |
| E | a field named further away wins when the near phrase is taken | #158 BAC `markov long 0.576 and quiver bullish` | `n10` |
| F | attribution ignores sentence boundaries | #197 AJG `+0.404 signal. Quiver is supportive only` | `n10` |

### What is left at `n10`

Across the reports recomputed at `n10` so far, **nine distinct disagreements**
remain, down from twenty at `n6`. None is obviously a checker artefact:

| | note | evidence |
|---|---|---|
| #101 AMAT | `reward_risk only 0.65` | 0.542 |
| #101 AMD | `5/5 confluence` | count 4, minimum 3 |
| #101 AMGN | `5/5 confluence` | count 5, minimum 3 |
| #101 ARM | `4/4 confluence` | count 4, minimum 3 |
| #106 BAC | `rsi 58` | 67.76 |
| #183 DDOG | `5 confluences` | 4 |
| #255 BMW | `6.0% downside-to-support` | 7.00 |
| #286 FLS | `593 DKK support` | 551.5 |

Three of them — AMGN, ARM, and the minimum half of AMD — turn on how `N/N`
should be read. The system's own prompt writes `N/M` as count over minimum, but
a report writing `5/5` where the count is 5 and the minimum 3 may be saying
"five of five checks passed". That is an ambiguity in the notation, not
necessarily an error in the note, and it is not resolved here.

The rest look like genuine report discrepancies. None has been adjudicated.

**The census is now complete.** Every grade that can be recomputed is at `n10`:
1,745 completed and 42 partial, around 12,700 checks. The 210 that cannot are
marked and stay at their original method version — reports 1 to 82, whose stored
prompts predate the indicator snapshot, so there is nothing to recompute them
against.

Of the 47 partial grades, **10 were already at `n10`, 32 were brought there by
the repair, and 5 were marked unrecomputable.** All kept their partial status.

| verdict | | |
|---|---|---|
| `matches` | 10,258 | compared |
| `differs` | 69 | compared — **9 distinct findings**, the table above |
| `unattributed` | 1,318 | no field |
| `uncertain_attribution` | 432 | contested, or wording the grammar refused |
| `not_in_evidence` | 398 | field absent from the snapshot |
| `not_a_field_value` | 176 | a horizon or a share count |
| `implausible_attribution` | 60 | field an order of magnitude off |

**10,258 matches and 69 disagreements are 81% of the checks compared.** That is
comparison coverage, not correctness: it counts how often the checker reached a
verdict, and says nothing about whether the verdict was right. The 69 `differs`
rows are the same nine findings repeated across grade versions of the same
reports, so the row count weights some reports far more than others.

### Measuring the attribution fixes properly

Comparing 12,700 checks now against 2,076 earlier is not a measurement of
anything: different denominators, and repeated grades weighting some reports
many times over. The retained history makes a real comparison possible, because
each superseded measurement is kept whole.

Taking every claim measured under **both** `n7` and `n10` — deduplicated by
report, symbol, figure and excerpt, so each distinct claim counts once:

| | `n7` | `n10` |
|---|---|---|
| shared claims | 1,429 | 1,429 |
| compared | 955 — 66.8% | **1,146 — 80.2%** |

195 claims went from abstention to a verdict; 4 went the other way. Of those
four, **three were false positives the fixes were meant to remove**: `rsi ~55`
read as an exact equality, `markov long 0.576` compared against Quiver, and
`453.0 EUR daily close support` compared against the support level. Each is now
an abstention, which is the correct reading.

The fourth is a genuine loss: #161 AAKI's `rsi 51.8` was a correct `matches` at
`n7` and is a contested `uncertain_attribution` at `n10` — another field's
phrase now falls inside the attribution margin. One correct comparison traded
for three false ones removed and 195 gained.

The single new disagreement is #101 AMAT's `reward_risk only 0.65` against
0.542, which `n7` could not attribute at all because the note writes the field
name with an underscore.

### A, in `n9`

The keyword search was a plain substring match, so `bull_prob` never matched
`bull prob`. **`markov.bull_prob` and `markov.bear_prob` had zero checks in all
of production** — two of twelve fields silently unverified, found by the
challenge set being unable to build a single case for them.

Underscores, hyphens and slashes are now read as spaces on both sides of the
match, which handles every spelling with one rule rather than a list of
variants. Each is a single ASCII byte, so offsets are preserved exactly. The
explicit `break-risk` and `reward/risk` keyword entries became redundant and are
gone.

That fix alone would have broken another attribution: with `bull_prob` matching,
the nearest phrase to the 0.61 in `markov bull_prob 0.72 / signed_signal 0.61`
became `bull_prob`. `signed signal` is now a name in its own right.

### B, E and F, in `n10` — one cause behind three symptoms

The `N/M` rule ran **last**, and only when one side had already been attributed
to the count. So "bullish 4/3, markov long 0.555" left the 3 compared against a
Markov signal.

The worse consequence was indirect. While the notation went unrecognised, those
figures competed for phrases in the ordinary way — and in #158 the **4 claimed
`markov` from sixteen characters away**, because its own name had already been
taken by the 3. A phrase another figure holds cannot contest anything, so
`markov` could not contest the 0.576 six characters from it, and Quiver won by a
single character. The false disagreement in E was caused by B.

The notation is now settled from its own shape, before any phrase is read: two
whole unsigned numbers, 1 to 12, separated by one slash that is not part of a
longer chain. Across all 1,105 notes, every one of the 298 occurrences of that
shape is a confluence count. It consumes the word where the note writes it —
"6/3 technical confluences" — so `confluences,` does not then contest the 2.71
two characters after it.

F is separate and simple: a phrase in the next sentence names nothing here. Full
stops and semicolons only. Commas were tried and cost more than they saved,
because these notes are comma-spliced lists and a figure is routinely separated
from its own field name by one.

`#158` now **abstains** rather than answering: `markov` and `quiver` really are
within a character of each other there, and the note does not settle which one
owns the figure.

### C and D, in `n10`

A name followed by a determiner is a verb. English does not put a determiner
after a noun, so "close support a controlled limit" is a narrow syntactic test
rather than a guess, and `above 446 EUR support` still reads as the noun.

`~` and `≈` before a figure are hedges written as punctuation, and now abstain
the way "near" and "around" do.

### The magnitude guard, in `n7`

One rule rather than a list: a threshold an order of
magnitude from its field is `implausible_attribution` whatever the relation.

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

Grade versions cover overlapping populations, so pooling them repeats
observations rather than enlarging the sample. A version-specific,
subject-deduplicated count is the only honest one.

**And no count taken while a backfill is running is stable.** Measured at 120
reports, `v12` read 244 flagged of 450 answered — 54%. The same version
complete, at all 229 reports, reads **545 flagged of 830 answered — 66%**, zero
unanswered. Nothing changed but the sample. Every figure in an evaluation must
come from a version whose backfill has finished; the observation loop works
through 120 reports a cycle and a version takes two cycles to complete.

66% of candidate notes flagged as asserting something the evidence does not
contain is a review workload, not a finding that two thirds of the reports are
defective. None of it is adjudicated, and the question itself is uncalibrated.

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
3. **Attribution crosses commas, though no longer sentences.** A field named
   in the next clause of the same sentence can still claim a figure. Commas
   were tried as barriers and cost more than they saved: these notes are
   comma-spliced lists and a figure is routinely separated from its own field
   name by one.
4. **`N/M` is read as a count over its minimum from its shape alone.** A
   genuine small fraction — "1/2 position" — would be misread. Every one of the
   298 occurrences of that shape in the stored corpus is a confluence count,
   and the rule is bounded to whole numbers from 1 to 12, but the note model's
   wording could change.
5. **A break risk written as a percentage is not attributed.**
   `support.break_risk` is stored as a fraction and declared as quoted in its
   own units, so "very low 3.7% modeled support-break risk" against a stored
   0.037 goes unattributed rather than being converted. A unit declaration
   problem, not an attribution one, and not fixed.
6. **Signs are not read.** "markov is positive at 0.1634" abstains rather than
   check the sign, because the grammar compares magnitudes written as quoted.
7. **The whitelist is English and hand-built.** A new phrasing abstains until
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
