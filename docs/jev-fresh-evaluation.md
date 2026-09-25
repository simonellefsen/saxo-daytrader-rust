# Fresh evaluation: the frozen checker on notes nobody tuned against

`fresh-v1-2026-09-25`, protocol `fresh-protocol-v1-2026-09-25`. The method
under evaluation is numeric method **`n15-2026-09-25`, frozen at `cc7b0a4`**.
The scorer refuses to run under any other.

| file | |
|---|---|
| `jev-fresh-v1.json` | the instrument: 51 whole notes and their evidence, with the rubric. **No verdicts, no spans, no strata.** |
| `jev-fresh-v1-key.json` | the frame, and every figure `n15` extracts from each note, with its verdict. Do not read first. |
| `jev-fresh-v1-labels.json` | empty until an independent labeller fills it; then committed as delivered and never edited |
| `jev-fresh-v1-seeded.json` | not yet generated: the seeded cases, generated from the committed labels |
| `jev-fresh-v1-results.json` | not yet written: the single scoring run |

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
