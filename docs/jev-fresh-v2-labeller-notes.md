# fresh-v2 labeller notes

Labeller: Fresh-context Claude AI subagent (claude-opus-5-5), blind fresh-v2 labeller.
Instrument version: `fresh-v2-2026-09-26`.

## Provenance and scope

All work happened inside `.../scratchpad/fresh-v2-labelling/`. I read only these files:

- `jev-fresh-v2.json`: the instrument. I read `what_this_is`, `how_to_label`, `conventions` and `out_of_scope` in full, then all 143 cases one at a time as pretty JSON, five or six per display, each note and its evidence in full. No display was truncated.
- `jev-fresh-v2-labels.json`: the empty output template. I read its structure and then wrote the completed labels into it.
- My own working files `progress.json` and `batch.json`, which held the labels I wrote by hand as I went. I deleted both at the end.

I did not open, list or search anything outside this directory: no repository, git history, wiki, memory, web or other agents. The session harness put some project instruction text into my context on its own at startup. I did not open it and did not use it.

I used code only to display cases, write the labels I had authored to JSON, count verdicts, and run the structural checks. I identified every claim, field and verdict by hand, and did any arithmetic in my head.

## Interpretations and judgement calls

1. **Signed Markov figures.** I mapped every Markov figure written with a sign to `markov.signed_signal`. That covers the word "conviction" as the convention says, and also "signal", "regime", "drift", "momentum" and "support". Every Markov row a note cites with a sign has direction `long`, so `signed_signal` equals `conviction` and no verdict depends on this choice. I found no unsigned Markov figures.
2. **How figures are written.** Each figure is recorded exactly as the note writes it, sign included (`+0.791`, `-0.635`). I dropped `%` and currency units. `~12.2k DKK` is recorded as `12.2k`, judged as 12,200 to the precision written.
3. **Missing evidence.** When `daily_indicators` is null, figures for confluences, support break-risk and support levels are `cannot_tell` with field `none`. That follows the missing-evidence convention.
4. **Price levels.** A standalone price level ("trading at", "consolidating near/at", "holding above", "performing well at") is judged against the stored closes, `daily_indicators.close` or `markov.close`, whichever the figure states under rounding. It is consistent if either close matches. It is inconsistent if neither does, or if the stated comparison fails against them.
5. **Prices at the end of a live move.** Some prices are the end point of a stated live move: intraday, midday, "from fill", or a note of the form "+x% to P". The evidence holds no live quote and no change field, and `markov.close` is not described as a prior close. So these prices and their percentage moves are `cannot_tell`, with one exception: a price that exactly equals a stored price is consistent. That happened once, `584.0` = `markov.close` in fresh-64870e3ee5.
6. **Support levels.** "Above X support" states the support level, judged against `daily_indicators.support.nearest_support` under the rounding convention.
7. **`5/3`** (fresh-81850743d1) is read as `confluence_count` over `min_confluences`: two claims sharing one quote, on different figure occurrences.
8. **Rounding at an exact half.** `276.42` against a close of 276.415 is consistent, rounding either way at an exact half.
9. **Ambiguous figures I kept.** "performing well at 226.40 DKK" (fresh-39aa4957a7) is read as the current price, not the execution price, and matches `markov.close`. "room to 276.42 USD" (fresh-b1521c2ba7) matches the daily close. "+5.99% extension" (fresh-06678d7d9e) is read as a price extension or move rather than position gain. Each has a note on its claim.

## Inconsistent claims (7)

- **fresh-0e85deb7ab**, `281` ("consolidating near 281 EUR"): the close is 292.2 (markov 294.0), more than two ATRs away. No price field is near 281.
- **fresh-6aca487519**, `281`: the same note text and evidence as above, with the same reason.
- **fresh-284a298277**, `317.00` ("holding above 317.00 DKK"): the closes are 314.0 and 315.0, both below 317.00.
- **fresh-5e56403935**, `570` ("consolidating at 570 DKK"): `markov.close` is 576.0, which does not round to 570.
- **fresh-6a6646bff8**, `566` ("above 566 NOK support"): `nearest_support` is 566.8133, which rounds to 567. 566 is a truncation.
- **fresh-87a32e4717**, `310.65` ("310.65 DKK nearest support"): `nearest_support` is 318.075 and next is 308.85. Neither matches.
- **fresh-893ae01c00**, `323.8` ("trading stably at 323.8 DKK"): `markov.close` is 325.2, which does not round to 323.8.

## Cannot-tell claims (41)

**`daily_indicators` is null, so there is no confluence, break-risk or support evidence:**

- fresh-0773035379 `713.49` (support)
- fresh-0c66c2090d `0.347` (break risk)
- fresh-189a8ae3a8 `0.192` (break risk)
- fresh-3b85e03349 `0.101` (break risk), `248.92` (support)
- fresh-57bbe35eab `23.50` (support)
- fresh-65a5a82c70 `5` (confluences), `713.49` (support)
- fresh-7f1f2454b5 `0.347` (break risk)
- fresh-893ae01c00 `5` (confluences), `0.24` (break risk)
- fresh-9b0ffbb3de `713` (support)
- fresh-9c47b1eca9 `0.101` (break risk), `248.92` (support)
- fresh-ac6ba8e72f `23.5` (support)
- fresh-c2c8ab6a8c `5` (confluences)
- fresh-e159859cce `0.154` (break risk)
- fresh-f239144933 `0.192` (break risk)

**A percentage move with no change field in the evidence:**

- fresh-06678d7d9e `+5.99` (extension)
- fresh-100a6ba9d0 `+1.51` (intraday)
- fresh-1f16082a6a `+2.50` (intraday)
- fresh-2d82a4e677 `+5.79` (intraday)
- fresh-32f576cbd5 `+1.80` (midday)
- fresh-3b15685062 `+5.34` (intraday)
- fresh-61bc8ac66f `+3.1` (intraday): 419.41 is in fact 3.1% above the daily close, but neither figure is held.
- fresh-64870e3ee5 `+1.92` (intraday)
- fresh-b51df2fde3 `+3.07` (midday)
- fresh-d6955d6a45 `-2.34` (pullback)
- fresh-e2656ddddc `+3.6` (intraday)
- fresh-e36d0ab688 `+2.58` (move)
- fresh-fc76be3d04 `+2.52` (intraday rally)

**A live price (intraday, midday, end point of a stated move, or since a fill) that matches no stored close:**

- fresh-1f16082a6a `533` (markov close 519.0)
- fresh-2d82a4e677 `584.50` (closes 550.5 and 554.5)
- fresh-32f576cbd5 `594` (markov close 598.5)
- fresh-61bc8ac66f `419.41` (closes 406.76 and 424.17)
- fresh-93069cce5f `160.80` (closes 158.61 and 157.23; a price since the fill)
- fresh-b51df2fde3 `587.5` (markov close 575.5)
- fresh-d6955d6a45 `104.15` (closes 105.4 and 104.5)
- fresh-e2656ddddc `654.79` (closes 631.75 and 645.30)
- fresh-e36d0ab688 `326` (closes 319.2 and 323.0)
- fresh-fc76be3d04 `391.50` (closes 374.74 and 380.52)

## Zero-claim notes (1)

- **fresh-10d3e1b96f**: wholly qualitative ("strong upward momentum, low break risk, and net positive congressional accumulation"). It has no figure in digits or words.

## Figures left out as out of scope

- **Stops:**
  - 540.30 in fresh-02fbfb894e, 06678d7d9e, 32f576cbd5, 541ebd1768, 5e56403935, 70183131f0
  - 546.88 in 05fad4aba8, b51df2fde3
  - 430.19 in 100a6ba9d0 (the same number as SMA20 430.19, but stated as a stop)
  - 300.26 in 10dc8bec04, 284a298277, e261808d53, e62f95655a
  - 253.41 in 113421de29
  - 484.76 in 187f7608d3, f448ece20c
  - 373.45 in 1c39ac782f, 61bc8ac66f
  - 212.84 in 39aa4957a7, c72a116a08, d355e582f6
  - 176.20 in 4058bf858b, c2c8ab6a8c
  - 104.47 in 5338ba627d
  - 157.55 in 692c4c2db9
  - 305.75 in 87a32e4717
  - 151.59 in 887ffb94f1
  - 311.05 in 893ae01c00, c86e75e284
  - 22.87 in 8ff91755d4
  - 140.87 in 93069cce5f
  - 581.20 in e2656ddddc
  - 501.55 in e840662836
  - 101.21 in f32bd315a0
- **Unrealised profit:**
  - `+11.1%` total return (fresh-1c39ac782f)
  - `+12.1%` unrealised gain (fresh-dc3b3c994b)
  - `+3.15%` from fill (fresh-93069cce5f)
- **Executions:** `108.35 DKK` fill price (fresh-f32bd315a0).
- **Borderline figures I included rather than excluded** (each claim carries a note):
  - `226.40` (fresh-39aa4957a7): could be an execution price.
  - `160.80` (fresh-93069cce5f): the end point of a from-fill gain.
  - `+5.99` (fresh-06678d7d9e): could be position gain.
  - `12.2k` (fresh-078c696777): a share price in DKK, given while deferring an allocation.
  - `317.00` (fresh-284a298277): could be an entry level.

## Counts

143 notes labelled, 319 claims: 271 consistent, 7 inconsistent, 41 cannot_tell. One zero-claim note.
