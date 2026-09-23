# Clean controls: a blind labelling instrument

`controls-v1-2026-09-22`, 121 cases drawn from the complete history at method
`n10-2026-09-22`. Seed `jev-controls-v1`.

| file | |
|---|---|
| `jev-controls-v1.json` | the instrument — notes, figures, evidence. **No verdicts.** |
| `jev-controls-v1-key.json` | the checker's verdict and attributed field. Do not read first. |
| `jev-controls-v1-labels.json` | empty, waiting for a labeller who is not me |

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

Counts of a stratum are not estimates of the population. 60 of 1,153 `matches`
bounds the false-negative rate loosely; it does not measure it precisely, and
the arithmetic to go from one to the other is the labeller's to do, not mine to
assert.

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

`controls-protocol-v2-2026-09-23`, in the scorer and in the key file.

- **Rates are per stratum.** The strata are sampled at wildly different rates —
  every `implausible_attribution`, six in a thousand `matches`. A total across
  the 121 cases describes the sample and nothing else.
- **Population figures are weighted**, each stratum's rate carrying its
  population share, with the counts behind it printed. An estimate from strata
  of six to sixty, not a measurement.
- **`cannot_tell` and `unlabelled` enter no numerator and no denominator**, and
  are reported beside every rate.
- **Wrong claims among sampled `matches` estimate the error proportion among
  claims the checker accepted.** That is not a conventional false-negative
  rate, which would need a denominator of all errors — including the ones in
  the strata this frame excludes.
- **`missed_by_abstention` stays separately visible** — a coverage failure is
  not a judgement failure — **and also counts toward errors the system did not
  flag end to end.**

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

Duplicate label ids and labels for cases not in the key are **reported, not
absorbed**. A duplicate used to overwrite its predecessor silently.

## What it will still not establish

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
