# Performance Benchmarks

The Performance view can compare the local account-value history with a small
set of Saxo-resolved ETF proxies:

| Reference | Proxy | Purpose |
| --- | --- | --- |
| S&P 500 | `SPY:arcx` | US large-cap reference |
| Nasdaq-100 | `QQQ:xnas` | US technology/growth reference |
| Dow Jones Industrial Average | `DIA:arcx` | US blue-chip reference |
| MSCI World | `EUNL:xetr` | Broad global-equity reference |
| STOXX Europe 600 | `EXSA:xetr` | Broad-Europe reference |
| FTSE 100 | `ISF:xlon` | UK large-cap reference |

The references are deliberately labelled as ETF proxies. They are not claims
that the system owns, trades, or replicates the underlying index.

## Method

The scheduler runs a read-only Saxo chart refresh after the daily-indicator
cycle. It resolves every configured instrument through Saxo reference data,
fetches daily closes, and stores the returned historical close series in
`performance_benchmark_prices`.

For the selected Performance range, the UI uses:

1. The first and latest account-value records in that range.
2. The latest available benchmark close at or before each account timestamp.
3. `return = latest / baseline - 1` for both series.
4. `excess = portfolio return - benchmark return`.

The table displays the actual benchmark dates used. A missing or non-overlapping
series remains `pending_history`; it is never rendered as a zero return.

The Performance range picker controls the comparison horizon. Select `1D` for
the latest available daily account change and `1W` for the week-to-date window;
the panel always shows the aligned portfolio, proxy, and excess return for that
selected period.

`Euronext` and `LSE` are exchange venues, not single comparable indices. The
Europe and UK entries therefore name their underlying indices and use specific
tracker listings rather than treating a venue as a benchmark. A future regional
reference must follow the same rule and be Saxo-SIM validated before it is
configured.

## Comparability Limits

This is an orientation tool, not a broker-verified time-weighted return or a
performance claim:

- The account is valued in DKK and includes cash.
- ETF references are native-currency price returns.
- Dividends, FX effects, fees, tax, and external cash flows are not normalized.
- Saxo chart closes may be delayed or absent in SIM.

The UI therefore carries the caveat with every benchmark result. A future
account-performance endpoint or FX-normalized total-return model should replace
this presentation only after it has an audited baseline and coverage checks.

## Safety Boundary

Benchmark refreshes perform only Saxo reference/chart GET requests. They do not
enter the watchlist or Markov universe and are excluded from Decision Reports,
Hermes, Trading Manager, sizing, protective-stop logic, and broker execution.

The manual refresh endpoint is `POST /api/actions/performance-benchmarks` for
an operator wanting to establish the read-only history before the nightly run.
