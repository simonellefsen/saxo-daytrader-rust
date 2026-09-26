# Where a trade's Markov metadata came from

Recorded by the completion audit (`decision_quality.rs`), `35730e0`. Measured
over every stored report in `decision-metadata-provenance-results.json`.

## Why

The provider writes each suggested trade's `strategy_metadata` itself, and
until now nothing checked that it describes the trade's own symbol. Two cases
prompted this check:
- **Report #259** attached DTE:xetr's Markov signal to its NESTE trade. The
  value, 0.23313425481319427, is identical to all 17 digits, and NESTE had no
  signal in that prompt. Report #260 then quoted it as NESTE's.
- **Report #195** wrote JPM's and JNJ's **Quiver** signals into their
  **Markov** metadata: the right symbol, the wrong source.

Both Markov gate paths in the Trading Manager look up their own signal by the
order's symbol, so neither case reached a gate. But the metadata is what the
report reasoned from, and a figure from the wrong symbol or source means the
reasoning was about something else. Symbol-specific gates do not rule out that
influence on which candidates were chosen.

## What it records

For each suggested trade, `metadata_provenance` traces `markov.signed_signal`
through the decision-time context and names where it came from:

| finding | meaning | passes |
|---|---|---|
| `own_symbol` | the trade's own Markov signal | yes |
| `no_signal_placeholder` | a `0` for a symbol with no signal. The presence check forces the provider to write something. | yes |
| `gate_threshold_as_signal` | the Markov gate's minimum, written as though it were the signal | no |
| `own_symbol_other_source` | the right symbol, the wrong source (#195: Quiver) | no |
| `own_symbol_other_markov_value` | another Markov value for the same symbol | no |
| `other_symbol_markov` | another symbol's Markov signal (#259: DTE) | no |
| `other_symbol_other_source` | another symbol, another source | no |
| `carried_from_earlier_report` | found only in the model's own earlier report | no |
| `no_source` | a full-precision figure found nowhere | no |
| `disagrees_with_own_symbol` | a short figure that is not its own symbol's, or a long one that is its own symbol's cut short rather than rounded | no |
| `no_own_signal` | a short figure for a symbol with no signal | no |

It also lists every piece of metadata that disagrees with the symbol's own
decision-time values: Markov state, direction and run date, and the technical
counts, sentiment and trend.

**Only a precise figure can name a source.** A figure written with six or more
decimals identifies where it came from. A shorter one is checked against its
own symbol only: a written `0.0` "matches" every signal under 0.05, so saying it
came from some other symbol would be a guess. State, direction and counts are
likewise only compared against their own symbol, because too many symbols share
a value of five to say which one it came from.

**Agreement is rounding at the precision written**, plus the difference between
single- and double-precision storage. 0.5828422796683945 against
0.5828422904014587 is one signal stored twice. Without that allowance, 39
correct trades read as disagreements. And #304's 0.429226 is its own
0.4292262494564056, rounded.

**Since v2 (`ea48362`), truncation is not accepted.** Simon settled the
convention as rounding on 2026-09-26, for every figure a report writes.
Rounding is now the numeric checker's own test, `written_by_rounding`, so the
two cannot drift apart. A long figure that is the trade's own signal cut short
reads `disagrees_with_own_symbol`, not `no_source`: it came from its own
symbol, and is written wrongly.

Re-measured over the same stored reports, and over the eleven reports after
them, in `decision-metadata-provenance-v2-results.json`. **Every finding is
identical to v1** across all 361 trades. No stored trade depended on
truncation.

## Where it sits, and what it does not touch

**Beside the audit's checks, not among them.** The audit's status, score and
first twelve checks go to Hermes' preflight, which "may recommend a more
conservative action when it sees a review item". Adding this to the checks
would change what that advisory path sees, and that is a decision for the
operator, not a side effect of building a check. A test pins that the audit's
checks, status and score are identical with and without a finding.

It is observational either way. It cannot block, approve or alter a trade.

## Across the stored reports

226 reports with a decision-time context, 350 suggested trades:

| finding | trades |
|---|---|
| `own_symbol` | 287 |
| `no_signal_placeholder` | 24 |
| **`gate_threshold_as_signal`** | **8** |
| `no_own_signal` | 2 |
| **`own_symbol_other_source`** | **2** |
| **`other_symbol_markov`** | **1** |

Plus 2 trades whose sentiment disagrees with the indicator: #185 wrote BUY
where the indicator said OVERWEIGHT, and SELL for UNDERWEIGHT.

That is 15 flagged trades in 10 reports.

**The threshold pattern.** Ten trades wrote 0.15, the gate's minimum signed
signal, for NVDA, SIE, JNJ, PLTR, UBER or ORCL. Eight are recognised by the
threshold the context recorded. The other two, #211 and #218, predate the
recorded policy; their threshold appears only in the system prompt's prose,
which the audit does not read, so they are flagged `no_own_signal`. All ten
fall in reports #211–#253. That is the period, 2026-08-03 to 09-03, when the
prompt's Markov list was alphabetical and cut off around G and carried no
embedded rows (`jev-markov-provenance.md`). All six symbols sort after G.
Lacking a signal, the model wrote the value its trade needed.

**Nothing is flagged from report #271 on**, which is when the list began to be
ranked by conviction with holdings first. That is 49 reports: too few to call
the problem gone.

## What it does not cover

- **Prose.** A figure in a note or rationale is checked by the numeric checker
  against its own symbol, not traced to another. At two to four decimals, a
  match with one of seventy other symbols is too often chance to name a
  source.
- **What the model was influenced by.** It records where a figure came from,
  not whether that figure changed which candidates were chosen.
- **Reports completed before `35730e0`.** They were not re-audited in place;
  the stored measurement above covers them instead.
