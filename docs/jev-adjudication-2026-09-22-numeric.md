# Adjudicating the nine numeric findings

Method `n10-2026-09-22`, the complete recomputed history: 12,700 checks, 69
`differs` rows, **nine distinct findings**. Adjudicated 2026-09-22 under the
frozen rubric in `jev-adjudication-rubric.md`.

## Who did this, and what that costs

The rubric says these are "answered by a human reading the note against the
evidence". **I am not that, and I am not independent.** I wrote the checker, and
I have looked at these nine repeatedly across the work that produced them. What
follows is triage — a reading of each note against the stored decision-time
evidence, with the reasoning shown so someone else can disagree with it — and
not an independent verdict.

Under the rubric, a category-1 finding is adjudicated on one question only:
**was the figure matched to the field the note meant?** If yes, the
disagreement is the report's. If no, it is the checker's.

## What settles four of the nine

Four findings turn on whether `N/M` means *count over the minimum required* or
*N of M possible*. Two facts from the stored prompts settle it.

- **`min_confluences` is 3 in all 12,160 indicator snapshots.** It has never
  taken another value.
- **`confluence_count` ranges 1 to 6** (1:64, 2:119, 3:263, 4:4553, 5:7011,
  6:150).

Across 297 uses of the notation in stored notes, the denominator is **3 in 294
of them**. A denominator of 3 cannot be a count of available checks, because
counts reach 6. So the notation is count-over-minimum, and the three deviations
— two `5/5` and one `4/4` — assert a minimum of 5 or 4 where the evidence says
3.

The competing reading, that the writer switched to "N of N" in exactly those
three cases, has no supporting instance in the other 294.

## The nine

| # | report | note fragment | evidence | verdict |
|---|---|---|---|---|
| 1 | 101 AMAT | `reward_risk only 0.65` | 0.5423 | contradiction |
| 2 | 101 AMD | `5/5 confluence` — count | 4 | contradiction |
| 3 | 101 AMD | `5/5 confluence` — minimum | 3 | contradiction |
| 4 | 101 AMGN | `5/5 confluence` — minimum | 3 | contradiction |
| 5 | 101 ARM | `4/4 confluence` — minimum | 3 | contradiction |
| 6 | 106 BAC | `RSI 58` | 67.76 | contradiction |
| 7 | 183 DDOG | `5 confluences` | 4 | contradiction |
| 8 | 255 BMW | `low 6.0% downside-to-support` | 7.003 | contradiction |
| 9 | 286 FLS | `consolidating near 593 DKK support` | 551.5 | contradiction, noted below |

**Nine of nine confirmed. No checker false positive among them.**

Case by case, where the reading needed work:

**1 — AMAT.** The note writes the field name itself, `reward_risk`, so the
attribution is not in question. No rounding or truncation of 0.5423 produces
0.65. The same note's `RSI 70 overbought` against 70.309 is correct and was
matched.

**2 and 3 — AMD.** `confluence_count` is 4 and the note says 5. That is wrong
under *every* reading of the notation, including the one that would rescue the
denominator. The denominator is wrong separately, per the section above.

**4 and 5 — AMGN and ARM.** The numerator is right in both (5 against 5, 4
against 4); only the denominator is wrong. These are the two findings most
likely to be called ambiguous by someone else, and the corpus evidence above is
the whole of my reason for not calling them that.

**6 — BAC.** `RSI 58` against 67.76. Nothing in the note or the evidence makes
58 a way of writing 67.76. The rest of the note is accurate: `R/R 0.75` against
0.74992, `5 confluences` against 5, `Markov long 0.557`.

**7 — DDOG.** `5 confluences` against a count of 4, named directly, no notation
involved.

**8 — BMW.** The note names `downside-to-support` explicitly. 7.003 to one
decimal is 7.0, not 6.0. The same note's `5/3` matches the count and the
minimum exactly, which is one of the 294 instances supporting the notation
reading above.

**9 — FLS.** The note says `consolidating near 593 DKK support`; `nearest_support`
is 551.5. **But `close` is exactly 593.0.** So there is a competing reading in
which 593 is the price and the wording is loose, and the checker matched the
wrong field. I record it as a contradiction because the note names support and
because "consolidating near [its own close]" asserts nothing — but the
coincidence is exact, and this is the one of the nine I would expect an
independent adjudicator to overturn.

## What this establishes

**Precision on this set: 9 of 9.** Every disagreement the checker reported in
the whole recomputed history is a real disagreement between a note and the
evidence it was written from.

**It says nothing about recall.** How often a note misstates the evidence and
the checker stays silent is a different question, measured separately and only
on seeded cases: `jev-challenge-set.md`.

**Four of the nine are in one report.** #101 contributes AMAT, AMD (twice),
AMGN and ARM. That is a signal about one report, not five independent ones, and
a per-report count would read very differently from a per-finding count.

**Six of the nine are two habits.** Writing a confluence count that does not
match the evidence (AMD, DDOG), and writing a denominator that is not the
minimum (AMD, AMGN, ARM).

**Nine findings in 12,700 checks is 0.07%.** On the 1,449 distinct claims the
history contains, it is well under one percent. The reports are, by this
measure, numerically accurate almost all of the time.

**And these nine must not become a held-out test set.** They have been inspected
repeatedly, by the same party that wrote the checker, throughout the work that
produced them. They are triage material and regression material, nothing else.

No trading parameter changes on the strength of this.
