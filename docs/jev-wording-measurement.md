# Measuring the wording grader

Set `wording-v1-2026-09-22`, 85 cases. Two runs over the **same cases and the
same model**, differing only in the question text:

| run | grading version | results | billed |
|---|---|---|---|
| A | `v15` — the instruction as it stood | `…results-v15-20260922T060840908Z.json` | USD 0.0277 |
| B | `v16` — categorical fields named one by one | `…results-v16-20260922T061120995Z.json` | USD 0.0294 |
| C | `v16` again, unchanged, for variance | `…results-v16-20260922T163422609Z.json` | USD 0.0294 |

Both resolved to `typesafe/jev-1.13-20260917`; both read case set
`a297026f…`; the question fingerprints differ (`f2ff7b9c…` → `7b25e46f…`).
That is the whole of the difference, and it is recorded in each results file so
a later divergence can be attributed to the code, the prompt, the set or the
model rather than to all four at once.

## What can be seeded, and what cannot

Arithmetic settles itself; wording does not. Whether "consolidating securely"
overstates a moderate break risk is a judgement, and a set built from judgements
measures whoever built it. So this set asks only what the evidence answers.

- **A categorical field takes one value.** Cases are built only from candidates
  where every categorical field reads positive — `trend_bias: bullish`,
  `sentiment: BUY`, `state: Bull`, `direction: long`, `break_risk_label: low` —
  so whichever field a flipped phrase refers to, the flip is false.
- **A note of figures alone makes no qualitative claim**, which is the rubric's
  fourth option by construction.
- **A claim with no counterpart in the evidence is unsupported**, and the
  invented claims sit outside the categories the question declares out of scope
  by design.
- **Two readings of the same claim should agree**: the same note asked again,
  and the same note with a synonym substituted.

**There is no arm for `overstated`.** Constructing one needs a threshold — how
small is "modest" — and a threshold I choose is my judgement wearing an answer
key's clothes. A common non-`fair` verdict is therefore one this set cannot
measure.

## Results

| arm | cases | A (`v15`) | B (`v16`) | C (`v16` again) |
|---|---|---|---|---|
| `category_flip` | 30 | 21 detected | **28** | **27** |
| `stripped` | 12 | 12 | 12 | 12 |
| `repeat` | 12 | 12 held | 12 held | 11 held, **1 moved** |
| `paraphrase` | 7 | 6 held, **1 moved** | 7 held | 7 held |
| `invented_evidence` | 12 | 8 flagged, 4 inconclusive | 8, 4 | 9, 3 |
| `control` | 12 | 11 `fair`, 1 `overstated` | 12 `fair` | 12 `fair` |

**Run-to-run variance, from B against C — same version, same cases, same
model:** 3 of 85 verdicts differ. Two of the three are the `control-009`
family. The third, `category_flip-020`, went from `misdescribes_category` to
`fair` **at a margin of 0.34** — a confident reversal rather than a coin
landing the other way.

So the honest statement of the headline is **27 to 28 of 30 on two runs**, not
28.

## The Markov instruction, and what changed

The `v15` misses were one construction:

| flipped phrase | n | A detected | B detected |
|---|---|---|---|
| `bull markov` → `bear markov` | 5 | 1 | **4** |
| `bull-state markov` → `bear-state markov` | 5 | 1 | **4** |
| `bullish` → `bearish` | 5 | 4 | 5 |
| `bullish trend`, `bullish technical` | 10 | 10 | 10 |
| `low break risk`, and two singletons | 5 | 5 | 5 |

The instruction already said "any claim that a categorical field takes a
particular value". Naming the fields one by one — and saying explicitly that the
Markov regime is one of them — took these constructions from **2 of 10 to 8 of
10**, with nothing else in the arm going backwards.

What that establishes is **poor detection on these two Markov phrasings, now
improved**. It does not establish that `v15` never checked Markov wording, and
it does not generalise past the constructions tested.

Two still miss at `v16`: `"+0.300 bear markov signal"` and `"+0.5052 bear-state
markov context"`, both returned `fair`.

## One control moved, and it cannot be attributed

`control-009` went `overstated` at `v15` to `fair` at `v16` — the only real note
the grader had ever flagged.

| | verdict | selected | runner-up | margin |
|---|---|---|---|---|
| `v15` control | `overstated` | 0.47 | 0.40 | 0.07 |
| `v15` repeat | `overstated` | 0.56 | 0.33 | **0.23** |
| `v15` paraphrase | `fair` | 0.44 | 0.42 | 0.02 |
| `v16` control | `fair` | 0.53 | 0.41 | **0.12** |
| `v16` repeat | `fair` | 0.49 | 0.46 | 0.03 |
| `v16` paraphrase | `fair` | 0.50 | 0.45 | 0.05 |

**Two of the six margins exceed 0.1**, so this is not uniformly a near-tie. An
earlier version of this document said "a margin under 0.1 in every reading" with
these numbers printed above it, and then called the move "sampling on a
borderline case, not evidence the instruction suppresses flags". That was an
explanation asserted as a finding.

What the data supports: **the control changed verdict, and these observations
cannot distinguish ordinary variation from an instruction-induced change.**
Its label is unadjudicated, so neither verdict is known to be right.

Run C does add one thing: `repeat-009` came back `overstated` again at `v16`,
with the instruction unchanged. The 009 family therefore varies *within* a
version, which is consistent with variation rather than with the instruction —
but consistent with is not the same as established, and one more run does not
make it so.

## Corrections to the first write-up

Five things the first version of this document got wrong or overstated.

1. **"Contradicted twice over" was wrong.** A positive forward `signed_signal`
   does not contradict a current Bear state; those are different quantities. The
   stored `state: Bull` is the contradiction, and it is enough on its own.
2. **The scorer counted any answer but `fair` as detection**, which would have
   credited `no_qualitative_claim` on a note full of characterisations. Only
   `overstated` and `misdescribes_category` count now, with a separate
   `wrong_category` column and a regression test. No result had used the
   loophole, so 21 of 30 stands.
3. **Sufficiency was scored without its control.** Four of the twelve controls
   were already above 0.5 before anything was added, so those four prove nothing
   about the added claim. The honest figure is **8 newly crossed, 4
   inconclusive** (9 and 3 in run C), not 12 of 12. The useful negative evidence
   is separate: all twelve figures-only notes stayed below 0.5 in every run,
   the highest at **0.36, 0.47 and 0.39** across A, B and C. An earlier version
   of this document gave 0.40, which was the maximum from a run made before the
   case set was regenerated.
4. **"These verdicts don't move between calls" was too strong.** What is
   supported is that nothing moved across 12 repeats and 7 paraphrases at `v16`,
   and that one paraphrase moved at `v15`.
5. **The confidence medians are descriptive, not a review rule.** Two missed
   flips came back at 0.84 and 0.72, well inside the range of the caught ones.
   Selected probability and the margin over the runner-up are the numbers to
   reason about — though `category_flip-020` reversed at a margin of 0.34, so
   they are not a rule either.
6. **Both earlier runs recorded the wrong source commit.** `983db1b` names the
   checkout's base, not the code that ran: `GIT_SHA` was read from
   `git rev-parse HEAD` while the tree was dirty, and the harness fixes and the
   `v16` instruction were uncommitted at the time. Those two files now carry
   the correction. Runs from here record the HEAD commit, whether the tree was
   dirty, a hash of the diff **and the diff itself** beside the results, one
   question hash per candidate count actually used, and a unique run id in the
   filename so a repeat never replaces its predecessor.

## What this still does not establish

- **Two runs of `v16` and one of `v15`.** The `v15` number has no variance
  estimate at all, and two runs of `v16` give a range, not a distribution. The
  seven additional detections are encouraging evidence for the instruction
  change, not an estimate of its repeatable effect.
- **Thirty flips, ten of them Markov.** The improvement is measured on the
  constructions that failed, which is the weakest possible generalisation.
- **The controls have no ground truth.** Twelve `fair` at `v16` is consistent
  with a well-behaved grader and with a permissive one, and they remain
  unadjudicated.
- **The set is not independently labelled.** I built the mutations and I chose
  which candidates qualify.
- **`v16` was measured on the set that motivated it.** It is a development
  baseline, not an evaluation. A fresh, independently labelled set covering both
  mutation directions, mixed and missing evidence, and deliberately excluded
  context is the next thing, and it is not this.

No trading parameter depends on any of this.

## Re-running

```
JEV_PROMPTS_PATH=prompts.json cargo test regenerate_the_wording_set -- --ignored --nocapture
set -a && . ./.env && set +a
GIT_SHA=$(git rev-parse HEAD) cargo test score_the_wording_set -- --ignored --nocapture
```

Results are written to a file named by grading version, so two runs sit side by
side instead of one overwriting the other and taking the comparison with it.
