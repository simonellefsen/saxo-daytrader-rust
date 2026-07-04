# QuiverQuant Advisory Signals

The Quiver integration adds an advisory alternative-data layer for US portfolio and watchlist assets. It is implemented in Rust in `src/quiver.rs`.

Quiver data is never an execution trigger by itself. The decision report prompt treats it as corroborating or risk-reducing context only; Trading Manager order gates still enforce market scope, cash, technical, Markov, and Saxo execution constraints.

## Data Source

Runtime config:

```yaml
quiver:
  api_key: ENV:QUIVERQUANT_API_KEY
  base_url: https://api.quiverquant.com

strategy:
  quiver:
    enabled: true
    timezone: Europe/Copenhagen
    daily_time: "23:10"
    run_weekdays_only: true
    lookback_days: 120
    max_symbols: 60
```

The first source is Quiver's ticker-specific Congress trading endpoint:

```text
GET /beta/historical/congresstrading/{ticker}
Authorization: Bearer ${QUIVERQUANT_API_KEY}
```

Only US Saxo symbols are included in the first pass:

- `NVDA:xnas` -> `NVDA`
- `BAC:xnys` -> `BAC`
- `BRK.B:xnys` -> `BRK-B`

Non-US Saxo symbols are skipped until Quiver coverage and ticker mapping are explicitly added.

## Scoring

For each ticker, recent Congress transactions inside the configured lookback window are weighted by:

- transaction direction: purchases positive, sales negative
- disclosed amount lower bound or range lower bound
- recency from transaction/report date

The stored signal is continuous in `[-1.0, 1.0]`:

- `bullish`: signal > 0.15
- `bearish`: signal < -0.15
- `neutral`: otherwise

The confidence value is derived from event count and absolute signal strength. It is not a probability.

## Persistence

Runtime tables:

- `quiver_signal_runs`: one row per run.
- `quiver_asset_signals`: one row per asset in that run.

The stored rows include normalized top events and source status, not API keys or request headers.

## Surfaces

- Dashboard: `/?view=quiver`
- API: `/api/quiver/signals`
- Manual refresh: `POST /api/actions/quiver-signals`
- Scheduler: runs after `strategy.quiver.daily_time`
- Decision prompt context: `quiver_signals`
- Hermes context/MCP: `quiver_signals`, `get_quiver_signals`

The manual refresh endpoint returns a compact run summary with ranked signal
rows. Use `GET /api/quiver/signals` for the full latest table and stored top
event details.

## Current Status

As of 2026-07-04, the Kubernetes deployment in namespace `saxo` has live
Quiver subscription access. Manual verification produced completed runs with 60
US portfolio/watchlist assets, 60 successes, and 0 errors.

## Safety

Quiver should answer "does alternative data support or weaken this already-valid setup?" It should not answer "what should we buy now?".

Current prompt rule:

```text
Never create a BUY solely because of Quiver data; use it to strengthen,
weaken, or explain a setup that already has technical, Markov, capital,
and market-scope support.
```
