# Clean controls: a blind labelling instrument

`controls-v1-2026-09-22`, 121 cases drawn from the complete history at method
`n10-2026-09-22`. Seed `jev-controls-v1`.

| file | |
|---|---|
| `jev-controls-v1.json` | the instrument — notes, figures, evidence. **No verdicts.** |
| `jev-controls-v1-key.json` | the checker's verdict and attributed field. Do not read first. |
| `jev-controls-v1-labels.json` | 121 labels from a fresh-context AI reviewer, as delivered. Never edited. |
| `jev-controls-v1-labelling-provenance.json` | who labelled, from what, under which rubric clarification |
| `jev-controls-v1-results-v4.json` | the single scoring run under protocol v4, unedited, with hashes |
| `jev-controls-v1-reconciliation.json` | every disagreement, reconciled against the evidence, beside the labels |

## Why this is not another measurement

Everything measured so far runs one way. The seeded sets plant an error and ask
whether it is caught — that is **detection of known errors on examples I
constructed**, not precision, and an earlier version of this page called it
precision. The adjudication reads the nine claims the checker flagged and asks
whether it was right, which *is* precision. Either way, every case that reaches
a desk is one the checker already chose to raise.

A miss never reaches a desk. **283 of the 1,445 distinct claims in the history —
19.6% — were never compared at all**, and an error in any of them could not have
been flagged. Nothing built so far can see into that.

This module does not fix that. It builds what fixing it requires: a frozen,
drawn, blind sample of natural claims with the evidence beside each one, and
somebody else's judgement written into the empty file.

## What makes the sample worth labelling

**The verdict is not in the instrument.** A test serialises the cases and
searches the text for every verdict name and for the word `stratum`. A labeller
cannot be anchored by an answer they have not been shown.

**The sample is drawn, not chosen.** Ordering within each stratum is
`sha256(seed + report + symbol + figure + offset)`. The seed is recorded in the
instrument, so the draw reproduces — and I could not have preferred a claim I
liked the look of.

**Abstentions are over-sampled on purpose.** Proportional sampling would have
drawn six `implausible_attribution` cases out of every thousand and said nothing
about the class. The strata:

| stratum | population | drawn |
|---|---|---|
| `matches` | 1,153 | 60 |
| `unattributed` | 151 | 18 |
| `uncertain_attribution` | 48 | 12 |
| `not_in_evidence` | 56 | 8 |
| `not_a_field_value` | 22 | 8 |
| `implausible_attribution` | 6 | 6 |
| `differs` | 9 | 9 |

Counts of a stratum are not estimates of the population. How to get from one to
the other is fixed in the analysis plan below, before any label exists — and so
is how little the best possible result would say.

**The nine `differs` are in there, unmarked.** A disagreement with them is a
**reconciliation point, not a verdict on the labeller.** Those five
confirmations are supported triage by the party that wrote the checker, already
amended once under review, and they are not gold labels. Where the two differ,
the evidence settles it.

Give a labeller the instrument and the rubric below, and nothing else. The key
and the earlier adjudication stay unavailable until labels are committed.

## Labelling

Each case gives the note in full, the figure as written, its offset, and
everything the report was given about that candidate. Record three things:

- **`verdict`** — `consistent` if the figure agrees with the evidence at the
  precision the note used, `inconsistent` if it does not, `cannot_tell` if two
  readings are both defensible. Ambiguity is a result. The adjudication that
  preceded this was amended precisely because it recorded a definite verdict
  while expecting to be overturned.
- **`field`** — the dotted evidence field the figure names, `none` if it names
  no field, empty if you could not tell. A right verdict about the wrong field
  is scored separately from agreement.
- **`note`** — anything a reader of the result would need.

Out of scope, per the frozen rubric: portfolio capital, holdings, unrealised
profit and trading costs. Their supporting material is deliberately not in the
evidence.

Fill in `labelled_by` and `labelled_at`. The build asserts that labels never
arrive without somebody named as having made them — which records **authorship,
not independence**. Nothing in this repository can establish the latter; that is
the reader's judgement.

## What the frame excludes

Cases reach the instrument through `grading_inputs` and `numeric_checks`, so the
sample inherits both. Counted across the history rather than asserted:

| | |
|---|---|
| candidates selected by reports | 1,116 |
| reached the checker | 837 |
| excluded — no indicator snapshot | 271 |
| excluded — past the question budget | 8 |
| of those that reached it, yielded an extracted claim | 608 |

**This is an audit of extracted claims from 608 notes, not of every way the
system can miss an error.** A figure the scanner never recognises cannot enter
the sample and cannot be labelled wrong. Extraction omissions need a whole-note
audit — a different instrument, not built.

And re-deriving the claims from the prompts rather than reading them out of the
stored grades is a **consistency check on the same code**, not an independent
implementation. An earlier version of this page called it an independent
reproduction. It is not one.

## The analysis plan, fixed before any label exists

`controls-protocol-v4-2026-09-23`, in the scorer and in the key file; a test
fails if the two disagree. Every revision so far came from review, before any
label arrived:

| | defect | corrected in |
|---|---|---|
| v2 | a duplicate label was reported, then whichever came last was scored | v3 |
| v2 | a settled-only rate was extrapolated to the whole stratum: one settled label of sixty spoke for 1,153 claims, and a stratum with nothing settled left the denominator | v3 |
| v3 | Wilson bounds were called "about 95%". Two wrong claims among the 1,153 `matches` covered the truth only **89.9%** of the time | v4 |

**The input is valid or nothing is scored.** A duplicate id, a label for a case
not in the key, or a verdict other than `consistent`, `inconsistent` or
`cannot_tell` rejects the file. No rates, no tables — only the problems, listed
as sets so the report is the same whatever order the file is in. Identical
duplicates are rejected too: one rule, no judgement about which conflicts
matter. An empty verdict is a case not yet labelled.

**Two kinds of unresolved, kept apart.**

| | means | effect |
|---|---|---|
| `unlabelled` | unfinished work | the population estimate is **withheld** until there are none |
| `cannot_tell` | a finding — two readings are defensible | enters the interval **both ways** |

**Rates among settled cases are conditional, and named so** —
`among_settled_only`. They describe the settled cases and say nothing about the
`cannot_tell` ones, which may be exactly the hard claims.

**The population figure is an interval, and the headline is the one that
assumes nothing about unresolved cases.**

- Within each stratum, every `cannot_tell` is counted once as consistent and
  once as wrong.
- A stratum drawn from a larger population is widened at each end by an
  **exact hypergeometric bound** — the distribution of errors found when
  drawing without replacement from this fixed population — at 1 − 0.05/S for
  S sampled strata. Five of the seven are sampled, so each stratum's bound is
  at 99%, and each tail at 0.5%. `differs` and `implausible_attribution` are
  taken whole and come out exact.
- **Coverage is at least 95%, and is checked rather than asserted.** A test
  enumerates every error count each of the five sampled strata could hold and
  confirms the stratum's bound covers it at least 99% of the time; the worst
  case is 99.01%. Bonferroni over the five then gives at least 95% for the
  population sum. A second test runs the case that broke v3 — two wrong
  `matches`, 2 of 1,445 overall — through the scorer: it now covers 99.7%.
- The guarantee assumes the hash-ordered draw behaves as a simple random
  sample within each stratum, and that the labels are right. It covers
  sampling error only. Label error is outside it.
- Strata are weighted by population share, and **every stratum stays in the
  denominator**. One with nothing settled contributes its full width instead
  of leaving.

**A point estimate appears only under a stated assumption** — that the
unresolved cases in each stratum resemble the settled ones. It is printed
beside the interval, never instead of it, and not at all when some stratum has
nothing settled for the assumption to extrapolate from.

**Wrong claims among sampled `matches` estimate the error proportion among
claims the checker accepted.** That is not a conventional false-negative rate,
which would need a denominator of all errors — including the ones in the strata
this frame excludes. **`missed_by_abstention` stays separately visible** — a
coverage failure is not a judgement failure — **and also counts toward errors
the system did not flag end to end.**

### What this method would report from a perfect result

Computed now, from the frozen strata, and pinned by a test. If every one of the
121 labels came back `consistent` with nothing unresolved:

| | wrong claims in the population | interval |
|---|---|---|
| `matches` — error proportion among accepted claims | 0 to 94 of 1,153 | 0 to **8.2%** |
| `unattributed` | 0 to 36 of 151 | 0 to 23.8% |
| `uncertain_attribution` | 0 to 15 of 48 | 0 to 31.3% |
| `not_in_evidence` | 0 to 25 of 56 | 0 to 44.6% |
| `not_a_field_value` | 0 to 8 of 22 | 0 to 36.4% |
| `differs`, `implausible_attribution` | exactly 0 | taken whole |
| **population** | **0 to 178 of 1,445** | 0 to **12.3%** |

These are what this pre-registered method would print, **not a limit on what
the sample could establish**. Bonferroni and the discreteness of exact bounds
are both conservative, and another valid method could be narrower. The method
is fixed now so that the choice cannot be made after seeing the labels. An
earlier version of this page gave 14.4% and 10.0% as "how little the best
result would say"; those were outputs of the Wilson method, which did not
cover.

A clean result bounds the error proportion; it cannot show that it is small.
Every `cannot_tell` widens these.

## What the score will say

The scorer was written before any label existed, so it cannot be tuned to the
labels it meets:

**Two axes, scored apart.** Folding attribution into the verdict let a right
answer about the wrong field read as agreement, and a blank field read as a
match — an unvalidated attribution becoming a verified one by silence.

| verdict axis | |
|---|---|
| **`false_negative`** | the checker compared and agreed; the labeller says the claim is wrong. **The quantity nothing so far could reach.** |
| `missed_by_abstention` | a wrong claim the checker never compared |
| `false_positive` | the checker flagged; the labeller says the claim holds |
| `agreed_on_error` | both call it wrong |
| `agreed` | both say it holds |
| `benign_abstention` | never compared, nothing to catch |
| `unsettled` | `cannot_tell` |
| `unlabelled` | **not a pass** |

| attribution axis | |
|---|---|
| `same` | both name the same field |
| `different` | they do not |
| `unknown` | the labeller left it blank — **not agreement** |
| `not_labelled` | no label |

A duplicate label id, a label for a case not in the key, or an undefined
verdict **rejects the input**. The version before this reported a duplicate
and then scored whichever label came last, so two files differing only in
order gave different results under the same warning.

## After the labels

`controls-protocol-v4-2026-09-23` is frozen. The order from here:

1. **The labeller gets the instrument and the rubric only.** Not the key, not
   the earlier adjudication, not this repository's history of findings.
2. **The labels are committed as delivered**, with `labelled_by` and
   `labelled_at`, before anyone opens the key.
3. **The score is computed once, on those labels,** and reported as it comes
   out.
4. **Disagreements are reconciled afterwards, in a separate file.** The
   original labels are never edited. Where reconciliation changes a reading,
   the reconciled figures are reported beside the original ones, not instead of
   them.

A change to the scorer after the labels are in is a new protocol version, and
its result is reported beside v4's, not in place of it.

## Results under protocol v4, as they came out

Scored once, at `9304c1d` with a clean tree, on labels committed and pushed
before the run; the hashes are in the results file. **The labels are an AI
review, not human ground truth**, from one labeller. The labeller was given only
the instrument and the rubric, plus one clarification disclosed in the
provenance: *missing evidence is `cannot_tell`, not proof that a claim is
false.*

All 121 labelled; no duplicate, unknown or unreadable label. 77 `consistent`,
7 `inconsistent`, 37 `cannot_tell`.

| stratum | sampled | agreed | labelled wrong | `cannot_tell` | wrong claims in population |
|---|---|---|---|---|---|
| `matches` | 60 of 1,153 | 59 | **1** (false negative) | 0 | 1 to 132 |
| `differs` | 9 of 9 | — | 6 (agreed on error) | 3 | 6 to 9 |
| `uncertain_attribution` | 12 of 48 | 10 benign | 0 | 2 | 0 to 25 |
| `implausible_attribution` | 6 of 6 | 5 benign | 0 | 1 | 0 to 1 |
| `unattributed` | 18 of 151 | 2 benign | 0 | 16 | 0 to 149 |
| `not_a_field_value` | 8 of 22 | 1 benign | 0 | 7 | 0 to 21 |
| `not_in_evidence` | 8 of 56 | 0 | 0 | 8 | 0 to 56 |

Sampled strata's ranges are at 99% each; strata taken whole are exact apart from
`cannot_tell`. The population range holds at 95% or more.

- **Error proportion among claims the checker accepted:** 1 of 60 sampled
  labelled wrong. Population range **1 to 132 of 1,153 — 0.09% to 11.4%.**
- **Claims labelled wrong, whole frame:** 7 to 393 of 1,445 — **0.5% to
  27.2%.**
- **Wrong and not flagged, end to end:** 1 to 384 of 1,445 — **0.07% to
  26.6%.**
- **No point estimate.** Nothing in `not_in_evidence` was settled, so the
  assumption has nothing to extrapolate from, and the protocol withholds it.
- **No false positive among settled flags.** Six of the nine flags were
  labelled wrong, three `cannot_tell`, none `consistent`.

**The width comes from `cannot_tell`, and almost all of it sits where the
checker abstained:** 34 of the 37, including all 8 `not_in_evidence` and 16 of
18 `unattributed`. The labeller and the checker largely agree that these claims
cannot be checked against the evidence. Under the frozen protocol, an
unresolved case still counts both ways. That is the rule as registered, and
this result is reported under it, not re-scored under a kinder one.

**The attribution axis:** 103 same, 13 different, 5 unknown. All 60 accepted
claims name the same field on both sides. The 13 differences are all in cases
the checker did not compare:

- In 5 `implausible_attribution` cases, the labeller names a field other than
  the one the checker rejected as implausible. The rejection was right; the
  tentative attribution was not.
- 4 claims about a "5-day" Markov horizon, which the labeller reads as
  `markov.horizon_days` and the checker treats as not a field value.
- 2 claims written as "R/R", which the labeller reads as `reward_risk` and the
  checker does not attribute.
- 2 `uncertain_attribution` cases where the two readings differ.

These are coverage observations. The parser stays frozen at `n10`.

### For reconciliation — listed, not resolved

Reconciliation goes in a separate file, and the labels above stay as they are.

1. **The one false negative is a question of convention.** Report #101, LMND,
   "RSI 60" against `rsi14` 60.66. The checker accepts truncation by design,
   because reports write "295" for 295.7. The labeller rounds, which gives 61.
   The rubric's "at the precision the note used" does not say which applies.
2. **FLS, report #286, "593 DKK support".** The labeller reads it as
   inconsistent: `nearest_support` is 551.5, and 593 is the close. The earlier
   adjudication, by the checker's author, left it ambiguous. The other eight
   flags agree with that adjudication — five wrong, three denominators
   unresolved.
3. **The rubric clarification.** Missing evidence is `cannot_tell`, which
   drives most of the width. It was disclosed before labelling and is part of
   what these labels mean.

## Reconciliation

`jev-controls-v1-reconciliation.json`, 2026-09-24, **by the checker's author**:
not independent. Every reading cites the evidence in its case so that someone
who is independent can check it. The rule: where the label and the checker
differ, the evidence settles it and the rubric settles how to read it. Where
neither can, the reading stays unresolved.

**No label changes, and the reconciled figures equal the v4 figures.** A test
checks, on every build:
- the delivered labels are byte-identical to the hashed file;
- every entry quotes the label and the key exactly;
- each of the 37 `cannot_tell` is classified exactly once;
- the figures are what the frozen scorer gives.

It covers 20 entries — every disagreement on either axis, plus FLS:

| point | cases | reconciled |
|---|---|---|
| false negative, LMND "RSI 60" against 60.66 | 1 | **the label stands.** The rubric reads precision as rounding, which gives 61. It is the only accepted claim in the sample that matches by truncation alone, so the labeller applied the rule consistently. |
| flags left `cannot_tell` — the report #101 denominators | 3 | **unresolved.** Read as observed/required, each disagrees. Read as a criteria total, none can be checked, because `confluences` is null. |
| wording with two readings — CHEMM 525, NVDA 224 | 2 | **unresolved.** Two closes from different moments; one satisfies each relation and one does not. |
| checker and labeller named different fields | 13 | **the labeller's field holds in every case.** The checker never compared any of them. |
| FLS 593 — label against the earlier adjudication | 1 | **not revised.** See below. |

The 13 attribution differences break down as follows:
- **Five: the checker guessed a field and its guard rejected the guess.** Its
  plausibility guard refused a wrong tentative field, so no wrong verdict came
  of it.
- **Two more: the checker guessed a field and abstained.**
- **Six are coverage gaps.** "R/R" is not read as `reward_risk` (2), and a
  "5-day" horizon is not read as `markov.horizon_days` (4). Three of those six
  claims were checkable and consistent, and none was compared.

**FLS.** The label and the checker agree it is wrong, so the score is
unaffected. The earlier adjudication, revised under review, left it ambiguous,
because 593.0 is exactly the close. The blind label considered the close and
rejected that reading. The adjudication is **not revised**: it stays at five
confirmed and four unresolved. The label would make it six and three. The
checker's author should not upgrade his own checker's confirmed count on the
strength of one label that agrees with it; an independent reader can settle it.

**What the 37 `cannot_tell` are:**

| reason | cases |
|---|---|
| the claim names a real field that is null in the evidence — Markov, in all twelve | 12 |
| portfolio capital, holdings, unrealised profit or trading costs — out of scope by the rubric | 14 |
| stops, limit references, executions — no order state in the evidence | 4 |
| a recommended holding period, not a claim of fact | 2 |
| two defensible readings | 5 |

Only 5 of the 37 are ambiguity in the reading. 20 are claims about things
outside the evidence, and 12 are claims the evidence should have covered but
did not. Under v4 all 37 count both ways, and that is most of the population
range's width.

**Truncation, as a scenario, not a reconciled reading.** If the rubric accepted
truncation as the checker does, only LMND moves. Accepted claims wrong would be
**0 to 94 of 1,153** instead of 1 to 132, and all claims wrong 6 to 355 of
1,445 instead of 7 to 393.

**Open, and not settled here:**
- **Truncation.** Whether the checker should keep accepting it is a parser
  change, and the parser is frozen at `n10`.
- **Coverage gaps.** The "R/R" and "N-day" gaps above; not fixed.
- **Missing Markov evidence.** Twelve claims quote Markov figures while the
  candidate's evidence has `markov` null — all 8 sampled `not_in_evidence`
  among them. Whether those figures came from elsewhere in the prompt or were
  invented cannot be answered from the instrument, and it is the next thing
  worth measuring.
  *Measured since, 2026-09-24, in `jev-markov-provenance.md`:* not invented.
  52 of the 53 such `not_in_evidence` claims in the frame match, by rounding,
  the value the model was shown for the same symbol and field. They sat in
  debugging rows the evidence builder does not read, because the evidence list
  was alphabetical and cut off around G. The other one, NESTE in report #260,
  is DTE:xetr's signal, attached to NESTE by report #259 and copied forward.
- **Scope.** Excluding the 20 claims outside the evidence would be a new
  protocol, and would need the frame's out-of-scope share measured, not
  assumed.

## What it will still not establish

**This validates the measurement procedure, not the checker's accuracy.** The
coverage guarantee is about sampling error only. Label error, extraction
omissions (the frame above) and how the checker performs on future reports are
all outside it.

- **It measures the numeric checker only.** The wording grader and the
  sufficiency question are measured, thinly, elsewhere.
- **One labeller is one judgement.** Two independent passes would give an
  agreement rate; one gives a reading.
- **121 cases from one method version.** Frozen at `n10`; a later method needs
  its own draw, and this one becomes regression material the moment it is
  labelled.
- **It cannot make the reports accurate.** It can only say how often the
  checker's answer and a labeller's agree, on claims the checker mostly never
  raised.

No trading parameter changes on the strength of this, and none has.
