# Measuring the wording grader

Set `wording-v1-2026-09-22`, 85 cases, scored against grading version `v15` on
2026-09-22. Frozen in `docs/jev-wording-challenge-v1.json`; results in
`docs/jev-wording-results-2026-09-22.json`. One run, 85 provider calls,
**USD 0.0277 billed**.

## What can be seeded, and what cannot

The numeric checker could be measured because arithmetic settles itself.
Wording cannot: whether "consolidating securely" overstates a moderate break
risk is a judgement, and a set built from judgements measures the person who
built it.

So this set asks only what the evidence already answers.

- **A categorical field takes one value.** Where every categorical field in a
  candidate's evidence reads positive — `trend_bias: bullish`, `sentiment:
  BUY`, `state: Bull`, `direction: long`, `break_risk_label: low` — a note
  calling the setup bearish is wrong, and wrong without anyone deciding how
  strong a word is. Cases are built only from such candidates, so whichever
  field the flipped phrase refers to, the flip is false.
- **A note of figures alone makes no qualitative claim**, which is the rubric's
  fourth option by construction.
- **A claim with no counterpart in the evidence is unsupported.** Analyst
  targets, insider ownership and earnings surprises appear nowhere in a
  candidate's evidence — and they are outside the categories the question
  declares out of scope by design, so "supported" cannot be excused as
  disregarding them.
- **Two readings of the same claim should agree.** The same note asked again,
  and the same note with a synonym substituted, should get the same verdict.

**There is no arm for `overstated`.** Every construction of one needs a
threshold — how small is "modest", how strong is "strong" — and a threshold I
choose is my judgement wearing an answer key's clothes. The consequence is that
a common non-`fair` verdict is the one this set cannot measure.

## Results

| arm | cases | correct | missed | meaning |
|---|---|---|---|---|
| `category_flip` | 30 | **21** | **9** | a seeded categorical error, flagged or called `fair` |
| `stripped` | 12 | 12 | 0 | figures-only note called `no_qualitative_claim` |
| `invented_evidence` | 12 | 12 | 0 | sufficiency ≥ 0.5 on an invented claim |
| `repeat` | 12 | 12 | 0 | same note, second call, same verdict |
| `paraphrase` | 7 | 7 | 0 | synonym substituted, same verdict |
| `control` | 12 | — | — | unmutated; no ground truth, recorded as the baseline |

**70% of seeded categorical errors detected. Perfect stability across repeats
and paraphrases. Every invented claim caught by the sufficiency question.**

## The one finding

The misses are not spread across the arm. They are one phrase:

| flipped phrase | caught | missed |
|---|---|---|
| `bullish trend` → `bearish trend` | 5 | 0 |
| `bullish technical` → `bearish technical` | 5 | 0 |
| `bullish` → `bearish` | 4 | 1 |
| `low break risk` → `high break risk` | 3 | 0 |
| `bullish daily trend`, `low modeled support-break risk` | 2 | 0 |
| **`bull markov` → `bear markov`** | **1** | **4** |
| **`bull-state markov` → `bear-state markov`** | **1** | **4** |

**Markov state: 2 of 10 caught. Everything else: 19 of 20.**

The missed notes are things like "leading +0.821 **bear** markov regime" against
a stored `state: Bull` and `signed_signal: +0.821`. The contradiction is
available twice over — the categorical field and the sign of the figure beside
it — and the grader returned `fair`.

The hypothesis this supports, and it is a hypothesis on thirty cases: **the
grader checks a note's trend and break-risk words against the evidence and does
not check its Markov state words.** Nothing has been changed in response. Fixing
the instruction now would invalidate this baseline, and one measured weakness is
worth more than a quick patch that makes the number go away.

Confidence separates the two groups without being a threshold anyone set:
caught flips have a median confidence of 0.81, missed ones 0.49.

## Stability, and what it means for the production numbers

`repeat` is the same note in a second call: **12 of 12 identical verdicts.**
`paraphrase` substitutes a synonym that changes no characterisation: **7 of 7
held.** Including the one control that came back `overstated` — its repeat and
its paraphrase both returned `overstated` too.

That matters for reading anything else the grader produces. A verdict that moved
between runs would make every stored grade a coin flip; these do not move.

## The sufficiency question

Twelve invented claims, twelve detected, probabilities 0.70 to 0.95 (mean 0.91).
Against the unmutated controls the mean is 0.48 — but the range runs to 0.93, so
**the separation is in the means, not in a clean boundary.** A real note can
score as high as an invented claim. The 0.5 threshold remains hand-picked and
uncalibrated, and the 66% production flag rate should be read with that in mind:
it is a review workload, not a count of defects.

## What this does not establish

- **One run.** Every number is a single sample of a probabilistic answer. The
  `repeat` arm argues the sampling is tight, but it does not make one run into
  many.
- **Thirty flips.** The Markov finding rests on ten cases of two phrases.
- **The controls have no ground truth.** They cannot say whether a `fair` on a
  real note is correct, only that it is stable. Eleven of twelve came back
  `fair`, which is consistent with both a well-behaved grader and a permissive
  one.
- **Detection here is "not `fair`", not the exact category.** Of the 21 caught,
  17 said `misdescribes_category` and 4 said `overstated`. Both are defensible
  on a flipped label, and requiring one would measure the rubric's wording as
  much as the grader's judgement.
- **This is the wording grader and the sufficiency question. It says nothing
  about the numeric checker**, which is measured separately in
  `jev-challenge-set.md`, and nothing about whether any of the three is worth
  acting on.

## Re-running

```
JEV_PROMPTS_PATH=prompts.json cargo test regenerate_the_wording_set -- --ignored --nocapture
set -a && . ./.env && set +a
cargo test score_the_wording_set -- --ignored --nocapture
```

The generator is hermetic and the case invariants run in the build. The scorer
makes real calls and costs about three cents.

The payload is the shape the production grading loop already sends to the same
provider every cycle: a trimmed report, candidate notes, and per-candidate
indicator, Markov and Quiver summaries. `top_events`, which names individuals,
is excluded upstream. No account identifier, position size or credential is in
it.

No trading parameter depends on any of this.
