# Measuring the wording grader

Set `wording-v1-2026-09-22`, 85 cases. Two runs over the **same cases and the
same model**, differing only in the question text:

| run | grading version | results | billed |
|---|---|---|---|
| A | `v15` — the instruction as it stood | `jev-wording-results-v15.json` | USD 0.0277 |
| B | `v16` — categorical fields named one by one | `jev-wording-results-v16.json` | USD 0.0294 |

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

| arm | cases | A (`v15`) | B (`v16`) |
|---|---|---|---|
| `category_flip` | 30 | 21 detected | **28 detected** |
| `stripped` | 12 | 12 | 12 |
| `repeat` | 12 | 12 held | 12 held |
| `paraphrase` | 7 | 6 held, **1 moved** | 7 held |
| `invented_evidence` | 12 | 8 newly flagged, 4 inconclusive | 8 newly flagged, 4 inconclusive |
| `control` | 12 | 11 `fair`, 1 `overstated` | 12 `fair` |

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
the grader had flagged. But its probabilities say it was never settled:

| | verdict | selected | runner-up |
|---|---|---|---|
| `v15` control | `overstated` | 0.47 | 0.40 |
| `v15` repeat | `overstated` | 0.56 | 0.33 |
| `v15` paraphrase | `fair` | 0.44 | 0.42 |
| `v16` all three | `fair` | 0.49–0.53 | 0.41–0.46 |

A margin under 0.1 in every reading. The `v15` paraphrase flip and the `v16`
move are the same borderline case sampled twice, not evidence that the new
instruction suppresses flags on real notes. With one flagged control out of
twelve there is nothing here to conclude either way.

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
   inconclusive**, not 12 of 12. The useful negative evidence is separate: all
   twelve figures-only notes stayed below 0.5, the highest at 0.40.
4. **"These verdicts don't move between calls" was too strong.** What is
   supported is that nothing moved across 12 repeats and 7 paraphrases at `v16`,
   and that one paraphrase moved at `v15`.
5. **The confidence medians are descriptive, not a review rule.** Two missed
   flips came back at 0.84 and 0.72, well inside the range of the caught ones.
   Selected probability and the margin over the runner-up are the numbers to
   reason about; `control-009` is what that looks like in practice.

## What this still does not establish

- **One run per version.** Each number is a single sample of a probabilistic
  answer, and the borderline control shows what that costs.
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
