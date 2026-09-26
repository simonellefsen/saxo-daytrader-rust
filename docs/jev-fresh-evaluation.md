# Fresh evaluation: the frozen checker on notes nobody tuned against

`fresh-v1-2026-09-25`, protocol `fresh-protocol-v1-2026-09-25`. The method
under evaluation is numeric method **`n15-2026-09-25`, frozen at `cc7b0a4`**.
The scorer refuses to run under any other.

**The checker has since moved to `n16`**, which fixes the "supported" defect
this set found (`jev-numeric-grammar.md`). This set remains a measurement of
`n15`, and cannot validate `n16`: its notes shaped the fix. Its reconciliation
test recomputes the seeded readings only under `n15`; on later methods it still
checks the natural, coherence and derivability figures. The seeded figures can
be reproduced at `c45b3aa`.

| file | |
|---|---|
| `jev-fresh-v1.json` | the instrument: 51 whole notes and their evidence, with the rubric. **No verdicts, no spans, no strata.** |
| `jev-fresh-v1-key.json` | the frame, and every figure `n15` extracts from each note, with its verdict. Do not read first. |
| `jev-fresh-v1-labels.json` | 51 labelled notes from a fresh-context AI reviewer, as delivered. Never edited. |
| `jev-fresh-v1-labeller-notes.md` | the labeller's reasoning for every uncertain or contradicting claim |
| `jev-fresh-v1-labelling-provenance.json` | who labelled, from what, with fingerprints |
| `jev-fresh-v1-seeded.json` | 490 seeded cases, generated from the committed labels |
| `jev-fresh-v1-results.json` | the single scoring run, unedited, with fingerprints |
| `jev-fresh-v1-reconciliation.json` | the disagreements, and the seeded evidence's coherence, reconciled beside the labels |

## Why

Every earlier instrument drew on the 233-report dump the checker was developed
against. The controls, the whole-note audit and the claim census all read it,
and every correction from `n11` to `n15` was shaped or checked against them. Review
has said each time that this makes them development and regression evidence,
not validation. None of them can say how the checker does on notes it has never
met.

The seeded challenge set has a second limit, recorded on its own page. It
anchors on figures the checker already reads, so it cannot detect an
attribution that was wrong from the start.

## The frame

**Every completed report after #319, the last in the development dump, up to
and including #362, the last created before `n15` was frozen**: eleven
reports, 2026-09-23 to 2026-09-25. Every distinct note they attached to a
selected symbol is in the set, so the frame is taken whole rather than
sampled.

| | notes |
|---|---|
| shown to the checker in production, with figures found | 28 |
| shown, with no figure found | 0 |
| never shown: no indicator snapshot, or past the question budget | 23 |
| | **51** |

Report #359 attached no notes. Nothing in these reports was read before the
method was frozen. Since then I have seen only the counts above and the key's
totals by verdict. I have not read a note.

**It is small.** Eleven reports from three days is what exists. A later version
can extend the frame, under the same protocol, to reports created after the
freeze. Those are prospective, and none exists yet.

**Almost half the notes never reach the checker.** In the dump it was a
quarter: 279 of 1,116. The key also records what `n15` reads in those 23
notes when given their evidence anyway. The results can therefore say what the
method does with them, beside the production fact that it never sees them.

## Part 1: the natural notes

Give a labeller `jev-fresh-v1.json` and nothing else. It carries its own
rubric. For each note, the labeller lists every **numeric** claim about the
supplied evidence:
- **`quote`**: the exact words, occurring once in the note.
- **`figure`**: the number as written, occurring once in the quote as a whole
  number.
- **`field`**: the dotted evidence path it states, which must hold a number, or
  `none`.
- **`verdict`**: `consistent`, `inconsistent` or `cannot_tell`. Missing
  evidence is `cannot_tell`.

Qualitative claims are not asked for. This evaluates the numeric checker, not
the wording grader.

Each labelled figure is matched to the figure `n15` extracted over it:

| reading | meaning |
|---|---|
| `omitted` | nothing extracted there |
| `abstained` | extracted, not compared |
| `accepted` | compared, and agreed |
| `flagged` | compared, and disagreed |

A comparison against a field other than the labeller's is counted as
`wrong_field` as well.

## Part 2: the seeded cases

Generated from the committed labels, after they arrive and before the score.
An anchor is a claim the labeller reads as `consistent` whose figure the key's
own arithmetic confirms: rounding either way at a half, or truncating, at the
precision written. Each anchor is changed one way at a time:

| change | direction | the changed figure |
|---|---|---|
| `note_value_up`, `note_value_down` | true → false | rewritten ×1.37 or ×0.63; ±2 for a count |
| `sign_flipped` | true → false | the sign of a signed signal |
| `evidence_value_up`, `evidence_value_down` | true → false | the note unchanged, the stored value moved |
| `other_symbol_value` | true → false | the same field's value from another symbol in the same report: the #259 carry-over |
| `both_moved` | stays true | note and evidence moved together |
| `repaired` | false → true | a claim labelled and computed wrong, rewritten to the stored value |
| `evidence_removed` | unsettleable | the field deleted from the evidence |
| `out_of_scope_stop_inserted` | out of scope | ", stop-loss at X" placed after a support level |

**The checker chooses nothing.** Anchors come from the labeller. Each key is
computed by the generator's own arithmetic against the final note and
evidence, not assumed from the kind of change. A change whose result the
arithmetic does not bear out is dropped, not keyed by intention. A test pins
that the generator never calls the checker. Another pins that the committed
cases are exactly what the committed labels generate.

**Every case keys every anchor in its note.** The changed figure tests
detection. The anchors left untouched in the same note test that an error is
placed on the right figure, and not spread to its neighbours.

Not anchored: percentages, grouped thousands, and a claim the arithmetic
contradicts. A contradicted claim goes to reconciliation. The out-of-scope stop
is tested only beside a support level, the one price level the checker reads.

## The analysis plan, fixed before any label exists

**The input is valid or nothing is scored.** A labels file is rejected whole,
checked as sets so the order does not matter, if it has any of these:
- a duplicate or unknown id;
- a quote not in its note, or in it twice;
- a figure not in its quote as a whole number, or in it twice;
- the same figure claimed twice;
- a field that is not a number in the note's evidence;
- a verdict the rubric does not define;
- claims on a note not marked as read.

**Nothing is scored while any note is unlabelled.**

**Natural notes.** Claims are described in full, reading by verdict and
stratum. Claims within a note are not independent, so the headline is per
note:

| outcome | yes when | unresolved when |
|---|---|---|
| `false_alarm` | a claim labelled consistent was flagged | none was, but a `cannot_tell` claim was |
| `accepted_error` | a claim labelled inconsistent was accepted | none was, but a `cannot_tell` claim was |
| `unflagged_error` | a claim labelled inconsistent was not flagged | none was, but a `cannot_tell` claim went unflagged |
| `omission` | a labelled claim had no extracted figure | never |
| `wrong_field` | a claim was compared against another field | never |

Each is reported as `[yes, yes + unresolved]` notes, twice:
- **over the 28 notes production showed the checker**;
- **over all 51**, the method's reading of every note given its evidence.

**The frame is taken whole, so its counts carry no sampling error.** They
describe these eleven reports. No rate is extrapolated to later reports; that
would need a sampling model this set does not have.

**Seeded cases.** Each keyed figure's outcome follows from its truth:

| truth | correct | wrong | neither |
|---|---|---|---|
| `holds` | `matches` on the keyed field | `differs`: a false alarm | anything else: an abstention |
| `fails` | `differs` on the keyed field | `matches`: an error accepted | anything else: an abstention |
| `unsettleable` | not compared | compared anyway | |
| `out_of_scope` | not compared | compared: a misattribution | |

A right verdict on the wrong field is counted apart, not as correct. An
abstention is neither correct nor wrong. It has its own column, so a checker
that abstains everywhere scores nothing, not a clean sheet. Results are
reported per change, split between the changed figure and the untouched ones.

## The order of work

1. **The instrument and its rubric only**, to a fresh-context labeller who
   has not seen the checker, the key, the controls or the whole-note audit. I
   do not label: I wrote the checker.
2. **Labels committed as delivered**, with `labelled_by` and `labelled_at`,
   before anything else runs.
3. **Seeded cases generated** from the committed labels and committed.
4. **Scored once**, by `score_the_fresh_set`. It refuses to overwrite a
   recorded result, and refuses any method but `n15`.
5. **Reconciled afterwards, in a separate file.** The original labels are
   never edited, and reconciled figures are reported beside the originals.

```
cargo test generate_the_seeded_cases -- --ignored
cargo test score_the_fresh_set -- --ignored --nocapture
```

## Results, as they came out

Scored once, at `057006d` with a clean tree and the checker byte-identical to
`cc7b0a4`. The order held:
1. labels committed as delivered (`afcb71f`);
2. seeded cases generated from them (`3843366`);
3. one scoring run (`ddc7781`).

**The labels are one AI review, not human ground truth.** The labeller was a
fresh-context reviewer given the instrument only. Its provenance file records
what it could not see.

**One process fault, before scoring.** `3843366` was committed with its
determinism check failing, and pushed; my command chain did not stop on the
failed test. The file was exactly what the generator wrote. The check compared
it with a regeneration held in memory, and serde_json's default parser reads
some floats back one unit in the last place off. `057006d` compares both as
read from text, and adds a check that every key's truth recomputes against the
evidence as stored. No case or key changed, and nothing had been scored.

### Natural notes

All 51 notes were labelled, and the file was accepted. The labeller found **90
numeric claims**: 72 consistent, 15 `cannot_tell` and 3 inconsistent. Two
notes make no numeric claim.

| per note | the 28 the checker read | all 51, as the method reads them |
|---|---|---|
| `false_alarm` | 0 | 0 |
| `accepted_error` | 1 | 1 |
| `unflagged_error` | 1 to 4 | 1 to 13 |
| `omission` | 1 | 1 |
| `wrong_field` | 1 | 1 |

**No consistent claim was flagged: 0 of the 46 the checker compared in
production**, and 0 of 68 across every note. Every figure the method compared
lies inside a labelled claim. The 25 extracted figures that no claim covers
were all left uncompared.

**The three inconsistent claims**, all in notes the checker read:
- **#360 NOKIA, "6 confluences"** against a count of 5, was flagged. It is the
  one flag in the frame, and the labeller agrees.
- **#321 CHEMM, "502 DKK support"** against 502.89, was accepted. The checker
  accepts truncation and the labeller rounds, to 503. This is the third time
  the convention has decided a case, after LMND in the controls and ASML in
  the whole-note audit.
- **#321 CHEMM, "consolidation at 515.5 DKK"** against a daily close of
  515.0, was not compared. The checker does not read the close.

**The rest:**
- **Three consistent claims were found but not compared:**
  - two "5 confluences", each contested by a nearby "support". In #361 the
    "support" is inside "supported". In #362 it is a separate word, six
    characters away.
  - **#358 UBER**'s "congressional net buying (+0.711)": "net buying" is
    outside the gap grammar.
- **One claim was omitted:** #358 UBER's "near-zero Markov conviction",
  which the labeller gave no field.
- **One claim was compared against another field.** #360 NOKIA's "negative
  Markov conviction (-0.124)" is the signed signal to the labeller and
  `markov.conviction` to the checker. The two hold the same magnitude, so
  both accept the note as written.

The 23 notes production never showed the checker hold 33 claims. The method
accepts all 22 labelled consistent and leaves the 11 `cannot_tell` uncompared.
In production none of them is checked at all.

### Seeded cases

490 cases, from 69 anchors in 47 notes. **Every consistent claim that could
anchor was confirmed by the key's own arithmetic.** The three that could not
anchor were two percentages and one with no field.

| keyed figure | figures | correct | abstained | decided wrongly | other field |
|---|---|---|---|---|---|
| changed, made false | 349 | **329** | 14 | 3 accepted | 3 flagged |
| changed, kept or made true | 71 | **66** | 4 | 1 false alarm | 0 |
| changed, unsettleable | 69 | **68** | — | 1 compared | — |
| changed, out of scope | 1 | 1 | — | 0 | — |
| untouched neighbours | 420 | **391** | 28 | **0** | 1 right verdict |

**All nine wrong or wrong-field outcomes are one anchor**: NOKIA's "negative
Markov conviction (-0.124)" again. Each change moved the field the labeller
named, and the checker compared the one it named. For example, a flipped sign
still matches the conviction's magnitude. Which field the note means is a
question for reconciliation, not something the score settles. The other 68
anchors gave no wrong decision, and no untouched neighbour was ever flagged.

**The 46 abstentions are four figures:** the two contested "5 confluences",
UBER's Quiver figure, and the repaired CHEMM close, a field the checker does
not read.

The 71 figures kept or made true are 69 `both_moved` and 2 repairs: NOKIA's 6
to 5, and CHEMM's close. CHEMM's "502" was not repaired, because the key's
arithmetic accepts truncation too. On that convention, the key and the checker
agree with each other and not with the labeller.

**Review added a qualification: many of these contexts are not coherent.** The
Markov model computes the signed signal as bull less bear, and conviction as
the signal's magnitude. Each seeded change moved one field alone:
- **Kept true:** 37 of the 69 `both_moved` cases hold evidence the model could
  not produce. 35 break the conviction link, 19 of which also break bull less
  bear; nine carry a conviction above one or a signal outside ±1; two carry a
  break risk above one.
- **Made false:** the evidence-side changes have the same fault, in 72 of
  138.

So these are single-field arithmetic tests, not coherent market contexts.
**66 of 71 is acceptance of single-field arithmetic, not acceptance of 71
unambiguously correct notes.** Likewise the nine NOKIA outcomes follow the
registered rules, but they are one disputed reading, not nine independent
checker mistakes. Its evidence-side cases also hold competing values for one
quantity. The reconciliation below separates these.

### What it shows, at its real strength

- **No false alarm was found.** None on the 46 consistent claims the checker
  compared in production, and none on 420 untouched figures beside seeded
  errors. That is these notes: 28 of them, from eleven reports.
- **Seeded single-figure errors were flagged on the right field 329 of 349
  times.** The 490 cases come from 69 anchors, so they are nowhere near 490
  independent trials. They measure sensitivity to one changed figure in claims
  the labeller found, not a natural error rate. Many of them sit in evidence
  the model could not produce.
- **Natural errors are too rare here to measure detection:** 3 in 90 claims.
  One was caught, one was decided by the truncation convention, and one lies
  outside the fields the checker reads.
- **One defect in `n15`, recorded and not fixed:** a keyword matched inside a
  longer word ("supported" contests as "support"). A fix would be `n16`, which
  this set cannot then validate.
- **One open question, which is not a defect:** which field a signed
  "conviction" figure names. It needs an independent adjudication, not an
  automatic change to the checker.

## Reconciliation

`jev-fresh-v1-reconciliation.json`, prompted by review of the results. **No
label, seeded case or recorded score is edited.** A test asserts their hashes,
checks every entry against the labels, the key and the evidence, and recomputes
every reconciled figure. It also shows the per-figure reading reproduces the
recorded totals. I am not independent, since I wrote the checker, so every
reading cites the evidence for someone who is.

Three kinds of disagreement, kept apart:
- **A rounding convention.** CHEMM's "502 DKK support" against 502.89 is
  truncation to the checker and the key, and rounding to 503 to the labeller.
  Left unresolved. It is a policy choice, and the author of the checker should
  not settle it.
- **A disputed attribution.** NOKIA's "negative Markov conviction (-0.124)":
  the word names conviction, a magnitude, and the figure carries the signal's
  sign. Left unresolved for an independent adjudicator.
- **Internally inconsistent seeded evidence**, found by checking every case
  against the model's own links and the fields' bounds.

The seeded figures, as scored and reconciled. Each cell is correct of all, with
the figures decided wrongly or on another field in brackets:

| keyed figure | as scored | coherent evidence only | without the NOKIA anchor | both |
|---|---|---|---|---|
| changed, made false | 329 of 349 (6) | 259 of 277 (4) | 329 of 343 (0) | 259 of 273 (0) |
| changed, kept or made true | 66 of 71 (1) | 30 of 34 (0) | 66 of 70 (0) | 30 of 34 (0) |
| changed, unsettleable | 68 of 69 (1) | 68 of 69 (1) | 68 of 68 (0) | 68 of 68 (0) |
| untouched neighbours | 391 of 420 (1) | 341 of 362 (1) | 391 of 419 (0) | 341 of 361 (0) |

Everything else in those rows is an abstention.
- **Without the disputed anchor, no seeded decision is wrong**, in any
  context.
- **Coherent evidence alone removes the one false alarm**, which was a
  `both_moved` case whose evidence the model could not produce.
- **The abstentions come from the same four figures in every column.** Their
  counts fall only because incoherent cases are dropped.

**"Unsettleable" overstates 35 of the 69 removals.** The model's links still
determine the removed value: all 19 removed signed signals are bull less bear,
and all 16 removed convictions are the signal's magnitude. Leaving them
uncompared is still correct under the protocol, which asks whether the field is
present, but a reader could settle them.

**Natural scenarios.** Read CHEMM's "502" as truncation and the one accepted
error goes. Read NOKIA's figure as conviction and the one wrong field goes. No
false alarm appears under either.

**Two qualifications travel with any summary of these figures**, as review
asked:
1. "No wrong decision without the NOKIA anchor" is a post-hoc subset result,
   not independent validation. It locates the disagreement; it does not
   establish which reading is right.
2. "Coherent evidence" means passing the listed checks in `context_problems`.
   It is not proof that every relationship in a synthetic context is valid.

**The signed-conviction question was adjudicated on 2026-09-26**
(`jev-adjudication-2026-09-26-signed-conviction.md`, amended the same day
after review).
- **NOKIA's figure is the signed signal.** That rests on the note's "negative",
  its minus sign and the gate it cites, so the labeller's field is right.
- **For future labelling, a convention:** a signed figure beside "conviction"
  states the signed signal. Under it, the seeded `sign_flipped` acceptance is a
  miss, because the checker discards a sign it attributes to conviction.
  #358 UBER's "+0.006" is a sign error only under the convention: its stored
  conviction is +0.0064, so it is not an established report error.

The ruling was made by the checker's author, against the checker's current
rule. Nothing was rescored, and no label changed.

**This evaluation is kept as it is.** It is historical evidence. It is not to
be regenerated to get cleaner results, and review recommends no further tuning
on it.

**For a future version**, as review recommended:
- separate valid-context changes from deliberate evidence corruption, and
  report them apart;
- in the former, move linked fields together and keep every field inside its
  bounds;
- key a removed field as unsettleable only when nothing linked still
  determines it;
- state in the rubric the signed-conviction rule and the rounding convention,
  so a labeller applies them rather than choosing. Simon settled the
  convention on 2026-09-26: rounding, not truncation (`n17`). This set's key
  keeps the truncation it was generated with.

## What this cannot establish

- **It is eleven reports.** A clean result on 28 notes says little about the
  rate on later ones, and this page will not turn it into one.
- **The labels will be one review, not ground truth.** The seeded cases are
  keyed by arithmetic, but they inherit the labeller's choice of field for
  each anchor. A field misread there is caught only where the arithmetic then
  fails to confirm the figure.
- **It measures the numeric checker only.** The wording grader, `v16`, is not
  evaluated here. Its fresh evaluation, with repeated, interleaved `v15` and
  `v16` comparisons, is not built.
- **It does not change the method.** A defect it finds is recorded against
  `n15`, and a fix is a new version that this set cannot then validate.
