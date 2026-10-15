---
type: source-note
tags:
  - daytrader/wiki
  - jev
  - markov-method
  - measurement
  - maintained-by-llm
updated: 2026-10-15
source:
  title: jev-markov-provenance
  path: docs/jev-markov-provenance.md
---

# Where the missing Markov figures came from

Raw source: [jev-markov-provenance.md](/Users/lindau/codex/rust_daytrader/docs/jev-markov-provenance.md),
with `jev-markov-provenance-results.json` and `jev-markov-n11-changes.json`.
Measured on 2026-09-24 at `c80c6d9`, over the 233-report dump the
[controls](jev-controls-v1.md) were drawn from.

## The question and the answer

The controls left a question open. Some notes quote a Markov figure where the
candidate's grading evidence has no Markov block. Were the figures in the
prompt, or invented?

**They were in the prompt.** There are 54 such claims in the frame, over 34
reports. Every one is in the prompt the model was given, for the same symbol
and field:
- **53** sit in the embedded debugging rows,
  `markov_method.latest_run.summary_json.signals[]`;
- **1** sits in the model's own earlier report.

**All 54 round to the value found.** That is strong evidence of where they
came from. It is not a hallucination rate, and it shows nothing about how the
model reasoned.
- **Control:** the same search finds all 340 Markov claims the checker had
  matched.
- **Coincidence:** of 428 perturbed figures, only 3 (0.7%) land on the same
  symbol and field.

## Why the evidence missed them

**The grading evidence read only one of the prompt's two Markov lists.** That
list was alphabetical and cut off at about 64 to 70 symbols, around GS or GSK,
until 2026-09-03. Every affected symbol sorts after the cut: V, ORSTED, NNIT,
LMND, NVDA, JPM and NESTE.

| reports | dates | Markov in the prompt |
|---|---|---|
| #71–#201 | 2026-06-04 to 07-31 | the list cut around G, plus 20 embedded rows |
| #202–#270 | 2026-08-03 to 09-03 | the cut list only: **symbols after the cut had no Markov data at all** |
| #271 on | from 2026-09-04 | ranked by conviction, holdings first (`47c9731`) |

This is the same truncated block the
[roadmap](../roadmap.md) records under 2026-09-03.

## The one wrong figure

**Report #260 quotes NESTE at "+0.2331".** That is DTE:xetr's signal,
identical to 17 digits. Report #259 attached it to its NESTE trade, and #260
read it back. **The wrong figure reached report text but not the gate.** Both
Markov gate paths look up the signal by the order's own symbol.

## Since: `n11` reads the embedded rows

On 2026-09-24 (`0cb1565`), at the operator's request, the evidence builder
began reading the embedded rows for symbols missing from the compact list.
The grammar was unchanged.

**93 candidates in 50 reports changed:**
- 51 claims moved from `not_in_evidence` to `matches`;
- none moved to `differs`;
- compared claims rose from 80.4% to 83.9% of the frame.

**Three things did not change:**
- **The controls.** They stay keyed at `n10`.
- **The stored wording grades.** They were given without the Markov block
  and are not re-asked.
- **What the 51 comparisons show.** They show the figures match what the
  model was shown, not that the reports using them were right.
