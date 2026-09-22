# The seeded challenge set

Two frozen sets, both scored by a hermetic test on every build:

| set | generated against | cases |
|---|---|---|
| `docs/jev-challenge-v1.json` | `n8-2026-09-22` | 240 |
| `docs/jev-challenge-v2.json` | `n9-2026-09-22` | 242 |

**`v1` stays frozen rather than being regenerated.** A regression set rebuilt
on every method change cannot catch a regression, because it has never seen the
method it would catch. `v1` scoring clean at `n9` is the evidence that the `n9`
attribution fix broke nothing it covers.

## Why it exists

Every adjudicated label this project has produced was `fair`. A grader that
never flagged anything would have scored identically on both samples, so
**sensitivity has never been measured** — not once, in either direction.

Adjudication alone cannot fix that. Natural reports supply whatever errors they
happen to contain, and the labelling is a judgement made by whoever is looking.
Seeding can: take a real note and its real decision-time evidence, change one
thing, and the right answer follows from the change. Writing 0.912 where the
snapshot holds 0.612 is false, and it is false whatever the checker says.

Two rules keep it honest.

**The answer key is the truth of the claim, not the verdict the checker ought to
return.** Each case records `holds`, `fails` or `unsettleable`. Nothing in
generation consults a verdict, and the arithmetic that decides whether a written
figure could have come from a stored one is a second implementation, not a call
into the thing under test.

**An abstention is neither a hit nor a miss.** It has its own column. A checker
that abstains everywhere scores zero agreed, not a clean sheet.

## What is in it

240 cases from 80 stored reports: 135 seeded falsehoods, 75 true claims, 30 the
evidence cannot settle. Notes are lowercased, because the checker lowercases
before reading and mutating the same text it will read removes a class of
offset bugs.

| mutation | what it changes | answer key |
|---|---|---|
| `unchanged` | nothing — the control | holds |
| `value_contradiction` | the figure, by 37% | fails |
| `digits_transposed` | two adjacent digits swapped | fails |
| `boundary_truncated` | the figure, to one decimal shorter | holds |
| `boundary_last_place` | one unit in the last place | fails |
| `sign_flipped` | the sign on a signed field | fails |
| `scaled_by_a_hundred` | the units | fails |
| `figures_swapped` | two figures in the note exchanged | fails |
| `comparator_true` / `_false` | inserts `above X` either side of the value | holds / fails |
| `inclusive_bound_holds` / `_fails` | inserts `>= X` / `<= X` | holds / fails |
| `negated_true_claim` / `_false_claim` | wraps the clause in "it is false that" | fails / holds |
| `future_tense` | appends "tomorrow" to the clause | unsettleable |
| `field_removed_from_evidence` | deletes the field from the snapshot | unsettleable |

## Results

`v1` at `n8`, and `v2` at `n9` after the attribution fix the set prompted:

| | `v1` at `n8` | `v2` at `n9` |
|---|---|---|
| **false negatives** — a seeded falsehood recorded as agreement | **0 of 135** | **0 of 135** |
| false positives — a true claim called a disagreement | 0 of 75 | 0 of 77 |
| overreach — an unsettleable claim settled anyway | 0 of 30 | 0 of 30 |
| not checked — the case measured nothing | 0 | 0 |
| agreed | 185 | 189 |
| abstained | 55 | 53 |

Per mutation, the abstentions are not spread evenly. They are three findings:

| mutation | agreed | abstained | |
|---|---|---|---|
| `scaled_by_a_hundred` | 0 | **15** | a unit error is never flagged, only refused |
| `negated_true_claim` | 0 | **15** | a negated claim is never checked |
| `negated_false_claim` | 0 | **15** | nor is a negated false one |
| `figures_swapped` | 8 | **7** | about half of transcription swaps go uncaught |
| everything else | 166 | 1 | |

**A unit error off by a factor of a hundred is never reported.** The magnitude
guard added in `n7` — a threshold an order of magnitude from its field is a
misattribution — cannot tell a misattributed figure from a correctly attributed
one in the wrong units. Both abstain. That is a deliberate trade recorded here
with its price attached.

**A negation is never checked in either direction.** The grammar abstains on
negated claims by design, and the set shows exactly what that buys: it cannot
distinguish "it is false that RSI is above 70" when the claim is true from when
it is false. Abstaining is still better than the alternative — before `n6` those
constructions returned *agreement* — but it is not detection.

## What this does not establish

**It is a regression set, not held-out evidence.** The method has seen it. Every
case was generated against `n8` and the frozen file records `n8`'s answers.

**The anchors are figures the checker already reads correctly.** A mutation
needs a foothold, so every case starts from a figure attributed without
difficulty. The largest abstention class in production — 407 figures at `n8`
with no field attributable at all — is under-represented here by construction.

**Two fields had never been checked once, and now are.** `markov.bull_prob`
and `markov.bear_prob` had zero checks in all of production at `n8`, because
notes write them as `bull_prob` and the keyword was "bull prob". The set found
the hole by being unable to build a single case for them; `n9` reads
underscores, hyphens and slashes as spaces, and `v2` carries the first two
cases those fields have ever had. Two is not coverage — across all 1,105 stored
notes only nineteen name a probability field at all, and most write it as a
percentage or as the word "zero" — but it is no longer nothing.

**This measures the numeric checker, not the grader.** The wording judgement is
a model's opinion about prose and nothing here touches it. Its sensitivity
remains unmeasured.

**Zero false negatives is not zero errors.** It says the checker did not
silently agree with anything this set planted. It abstained on 23% of cases,
and an abstention tells an operator nothing.

## What the set has caught so far

One defect, and it was a silent one: `markov.bull_prob` and `markov.bear_prob`
unverified in their entirety. Adjudication could not have found it, because a
field that is never checked produces no verdict to adjudicate.

It has caught no regression yet, which is the point of keeping `v1` frozen —
that number only becomes meaningful after the method moves under it.

## Regenerating

```
psql -tAc "select jsonb_agg(jsonb_build_object('id', id,
  'report', report_json::jsonb, 'request', request_json::jsonb))
  from (select id, report_json, request_json from decision_reports
  where status='completed' and report_json is not null
  and request_json is not null order by id desc limit 80) t" > prompts.json
JEV_PROMPTS_PATH=prompts.json cargo test regenerate_the_challenge_set -- --ignored --nocapture
```

Regenerating against a changed method rewrites the answer key's *population*,
not its answers — the truth of each claim is fixed by the change that made it.
But it does re-anchor on whatever the new method reads, so a regenerated set is
a new baseline and should be named as one.

No trading parameter depends on any of this.
