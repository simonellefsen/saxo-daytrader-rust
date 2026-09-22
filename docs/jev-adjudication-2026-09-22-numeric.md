# Adjudicating the nine numeric findings

Method `n10-2026-09-22`, the complete recomputed history. Adjudicated
2026-09-22 under the frozen rubric in `jev-adjudication-rubric.md`, and
**amended the same day** after review — the first version recorded nine
contradictions and a precision claim, and neither survived.

## Who did this, and what that costs

The rubric says these are "answered by a human reading the note against the
evidence". **I am not that, and I am not independent.** I wrote the checker and
have looked at these nine repeatedly across the work that produced them. The
review that corrected this document was a second agent, which is not blinded
human adjudication either.

Under the rubric a category-1 finding is adjudicated on one question: **was the
figure matched to the field the note meant?** And rule 4 says that where two
readings are both defensible, the result is *ambiguous* — ambiguity being a
result rather than a failure to decide.

## Verdicts

**Five confirmed contradictions. Four unresolved.**

| # | report | note says | evidence | verdict |
|---|---|---|---|---|
| 1 | 101 AMAT | `reward_risk only 0.65` | 0.5423 | **contradiction** |
| 2 | 101 AMD | `5/5 confluence` — numerator | count 4 | **contradiction** |
| 3 | 106 BAC | `RSI 58` | 67.7608 | **contradiction** |
| 4 | 183 DDOG | `5 confluences` | count 4 | **contradiction** |
| 5 | 255 BMW | `low 6.0% downside-to-support` | 7.003% | **contradiction** |
| 6 | 101 AMD | `5/5` — denominator | minimum 3 | *ambiguous* |
| 7 | 101 AMGN | `5/5` — denominator | minimum 3 | *ambiguous* |
| 8 | 101 ARM | `4/4` — denominator | minimum 3 | *ambiguous* |
| 9 | 286 FLS | `near 593 DKK support` | support 551.5, **close 593.0** | *ambiguous* |

### The five

**AMAT** writes the field name itself, `reward_risk`, so attribution is not in
question, and no rounding or truncation of 0.5423 produces 0.65. **AMD's
numerator** says five confluences where the count is four — wrong under every
reading of the notation, including the one that would rescue its denominator.
**BAC** says `RSI 58` against 67.7608; the rest of that note is accurate.
**DDOG** says five confluences against a count of four, named directly.

**BMW** says `low 6.0% downside-to-support` where the downside is 7.003% — and
its stored `break_risk` is 0.06002732, which is 6.0027%. The report appears to
have put the break-risk probability where the distance to support goes. That is
a plausible mechanism for the error, not a separate finding.

### The four

**The three denominators.** The corpus shows count-over-minimum is the dominant
convention: `min_confluences` is 3 in all 12,160 indicator snapshots,
`confluence_count` runs 1 to 6, and 294 of 297 notation uses in stored notes
write 3. A denominator of 3 cannot be a count of available checks when counts
reach 6.

That establishes the convention. It does not settle these three, and the first
version of this document argued that it did — by saying the reading "has no
supporting instance in the other 294", as though three independent
counterexamples had been weighed against 294. **All three occur in report 101.**
They are one writer's choice on one occasion, not three deviations, and a report
switching to "N out of N" for its own candidates is exactly what one occasion
looks like. That wording may still be misleading or unsupported; it does not
unambiguously assert a minimum of 5 or 4.

**FLS.** The note says `consolidating near 593 DKK support` against a
`nearest_support` of 551.5 — but `close` is **exactly 593.0**. The competing
attribution is grounded in the evidence rather than hypothetical. The first
version recorded this as a contradiction while stating in the same paragraph
that an independent adjudicator would likely overturn it. That is not applying
rule 4; it is hedging while keeping the score.

## What this establishes, and what was withdrawn

**Precision is not established at 9 of 9.** Five of the nine flags are
confirmed; four are unresolved. Unresolved does not mean the checker was wrong —
it means the evidence does not settle it either way.

**The accuracy conclusion is withdrawn.** The first version said "nine findings
in 12,700 checks is 0.07%" and concluded the reports are "numerically accurate
almost all of the time". Three things are wrong with that:

- It divided deduplicated findings by repeated check rows. The comparable
  figures are **69 flag rows in 12,716 check rows — 0.54%** — or **9 distinct
  findings in 1,445 distinct claims — 0.62%**.
- A flag rate is not an error rate. It counts what the checker noticed.
- **283 of the 1,445 distinct claims, 19.6%, were never compared at all.** An
  error in any of them could not have been flagged.

Even perfect precision among flagged cases would say nothing about the claims
that were never flagged, which is where a missed error would be.

**Two counting corrections.** Report 101 contributes **five** of the nine, not
four — AMAT, AMD's numerator, AMD's denominator, AMGN and ARM. Confluence counts
and denominators account for **five** findings, not six: two counts (AMD, DDOG)
and three denominators.

**These nine must not become a held-out test set.** They have now been inspected
repeatedly by the party that wrote the checker and once by a reviewing agent.
They are triage and regression material.

What remains necessary, and is not this: independently labelled clean controls
and unflagged examples. Reviewing only the cases the checker flagged can measure
its false positives and can never measure its false negatives.

No trading parameter changes on the strength of this.
