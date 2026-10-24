# Reference Data

Three read-only stores added on 2026-10-24, after the September–October month
review. None of them is read by a Decision Report, Hermes, the Trading Manager,
sizing, the protective-stop sweep or any broker mutation. They feed one
evaluation endpoint and two dashboard panels.

| Store | Source | Endpoint | Dashboard |
| --- | --- | --- | --- |
| `earnings_report_dates` | Alpha Vantage `EARNINGS` | `/api/evaluation/earnings` | none yet |
| `company_profiles` | Financial Modeling Prep `/stable/profile` | `/api/portfolio/concentration` | Performance → Sector Concentration |
| `instrument_trading_schedules` | Saxo `/ref/v1/instruments/tradingschedule/{Uic}/{AssetType}` | `/api/market/trading-schedule` | Market → Saxo Trading Schedule |

## Congress trades

Since 2026-10-24 the congress signal (the `quiver_*` tables and the Congress
tab, named for QuiverQuant, its former paid source) scores disclosures that
Alpha Vantage's free `CONGRESS_TRADES` returns. Unlike the stores above, this
one feeds an advisory input: decision reports, Hermes and the Jev reviews
read the signal, never a gate or the broker path.

- `congress_trades` keeps one stock's House and Senate disclosures from the
  last 400 days, replaced as a whole on each answer. Option trades are left
  out: a disclosure says buy or sell but not call or put.
- `congress_trade_fetches` keeps one row per ticker. A stock is read again
  every 7 days; disclosures arrive weeks after the trade.
- The daily signal cycle (US open + 45 minutes) reads only the store and
  calls no provider. A stock not fetched yet is recorded as
  `not_yet_fetched`, so the signal's freshness reads `partial` until the
  store has covered the universe.
- The score is unchanged from Quiver's: buys count up and sells down,
  weighted by recency and by the amount band's lower bound. Alpha Vantage
  reports no excess return or price change, so those fields are empty.

## Keys

`ALPHAVANTAGE_API_KEY` and `FINANCIALMODELINGPREP_API_KEY` come from `.env`
and reach the pods through the `daytrader-env` secret, like every other key
the deploy script passes. Both providers accept the key only in the query
string, so every request maps its send error through
`debug_redaction::request_error_without_url`; the guard test in
`debug_redaction.rs` covers both files. A failure is stored as a category
(`http_402`, `quota_exhausted`, `timeout`), never as error text.

`FINNHUBIO_API_KEY` is in `.env` but nothing reads it yet. It is the candidate
source for upcoming US report dates if an earnings blackout is built.

`QUIVERQUANT_API_KEY` is no longer read; the deployed config dropped its
`quiver:` provider block on 2026-10-24.

## Budgets

| Store | Daily calls | Per cycle | Refresh |
| --- | --- | --- | --- |
| Congress trades | 20 | 8, 1.3 s apart | every 7 days per stock |
| Report dates | 20 | 8, 1.3 s apart | every 30 days per stock |
| Profiles | 100 (free plan: 250) | 25, 0.3 s apart | every 30 days per stock |
| Trading schedules | about 35 Saxo reads | once a day from 06:30 Copenhagen | daily, weekends included |

Both Alpha Vantage stores share `alpha_vantage.daily_call_budget` (24; a free
key allows 25), and the congress store runs first in each scheduler cycle.
A stock is asked at most once a day, so a stock's row dated today is one of
today's calls. Alpha Vantage refuses with HTTP 200 and a prose note: "per day"
stops the store until the next UTC day, "1 request per second" is retried once.
An answer that will not change by tomorrow (no data, unknown ticker, outside
the plan) waits for the full refresh instead of spending a call every day.

The US universe is ordered by usefulness: held stocks, then anything the book
has traded, then the rest of the watchlist. The report-date backfill of about
80 US stocks takes four days.

## Coverage limits

- **US only** for report dates and profiles. Financial Modeling Prep's free
  plan answers HTTP 402 for European and Danish profiles, and Alpha Vantage's
  earnings data is US-listed only. Non-US holdings show as `Unprofiled`.
- Saxo appends `_NEW` to a relisted symbol (`SHOP_NEW:xnas`); the providers
  get the bare ticker.
- Saxo's trading schedule reaches only a session or two ahead. The first
  read, on Saturday 2026-10-24, covered Friday's close through Tuesday
  morning on every exchange. A holiday is visible the day before at best. An
  early close is judged against `exchange_session_days`: one row per exchange
  per complete trading day, summed over its regular segments (Xetra splits
  its day around a midday auction), kept across reads. With fewer than three
  stored days no early close is claimed, and a day the read ends inside is
  left out.
- Responses carry `Sessions` with `StartTime`, `EndTime` and `State`; the
  regular state is `AutomatedTrading`. The first read returned 30 of 30
  instruments on the SIM account.

## The earnings evaluation

`/api/evaluation/earnings` measures, from stored data only:

- the open-versus-previous-close gap on each report's reaction session (the
  report day for `pre-market`, the next session for `post-market`) against
  every other session. A report with unknown timing is left out, and so are
  the two sessions it could have moved;
- every report the book held a position through since the current book
  opened (2026-07-16), with the gap applied to the quantity held, at today's
  FX rate.

It answers whether an earnings blackout is worth building. The 2026-10-24
month review found that the largest single loss, MDB, gapped on a CEO
departure rather than a report, which a blackout would not have caught.
