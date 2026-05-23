---
type: wiki-log
tags:
  - daytrader/wiki
  - maintained-by-llm
updated: 2026-05-23
---

# Wiki Log

Append-only timeline for project wiki maintenance. Use headings with the format `## [YYYY-MM-DD] kind | summary` so agents and shell tools can parse the log.

## [2026-05-23] ingest | LLM Wiki pattern

- Read [llm-wiki.md](/Users/lindau/codex/rust_daytrader/llm-wiki.md).
- Created the initial project wiki structure under [wiki/](/Users/lindau/codex/rust_daytrader/wiki).
- Added schema, index, source note, and concept pages.
- Added [docs/project-wiki.md](/Users/lindau/codex/rust_daytrader/docs/project-wiki.md) for repo-level workflow documentation.

## [2026-05-23] attribution | LLM Wiki source credit

- Credited Andrej Karpathy as the author of the copied LLM Wiki idea file.
- Added the original gist URL: [karpathy/442a6bf555914893e9891c11519de94f](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).

## [2026-05-23] implementation | Hermes Kubernetes base

- Added initial Kubernetes support for Hermes Agent in `saxo-rust`.
- Added `hermes-agent`, `hermes-data`, `hermes-gateway`, and `hermes-daytrader-context`.
- Updated deployment scripting to create a separate `hermes-env` secret from a whitelist so Saxo credentials are not passed to Hermes.
- Documented that Hermes is internal-only and not yet connected to a daytrader MCP adapter or strategy promotion flow.

## [2026-05-23] implementation | Hermes HTTP adapter

- Added protected `/api/hermes/*` endpoints in `saxo-rust`.
- Added sanitized context, capabilities, reflection writes, and strategy experiment proposal writes.
- Added runtime tables for `hermes_reflections`, `strategy_experiments`, and `strategy_baselines`.
- Required `HERMES_DAYTRADER_API_KEY` for the adapter so these endpoints are not exposed as normal dashboard API routes.

## [2026-05-23] implementation | Hermes weekly reflection CronJob

- Added suspended `CronJob/hermes-weekly-reflection`.
- The CronJob submits a run to Hermes' `/v1/runs` API instead of writing reflections directly.
- The prompt instructs Hermes to fetch the protected daytrader context, create one reflection, and optionally create one one-variable experiment proposal.
- The job requires `HERMES_API_SERVER_KEY` and `HERMES_DAYTRADER_API_KEY`, and remains suspended until explicitly enabled.

## [2026-05-23] runbook | Build, test, deploy, and Saxo SIM checks

- Added `wiki/runbooks/build-test-deploy.md`.
- Documented Rust build, formatting, unit tests, integration/regression tests, local smoke tests, Kubernetes deployment and smoke tests, Hermes smoke tests, Saxo SIM testing order, live trading safety gates, and qmd/Obsidian-compatible wiki maintenance.

## [2026-05-23] smoke | Hermes in-cluster reflection

- Deployed Hermes Agent to Docker Desktop Kubernetes in namespace `saxo-rust`.
- Used `BACKUP_OBJECT_STORE=rustfs` because the local `daytrader_rustfs` container already owns ports `9000-9001`.
- Verified `daytrader-api` health from inside the cluster.
- Enabled Hermes API server with cluster-only generated keys and verified `/health`, `/v1/capabilities`, and the protected daytrader `/api/hermes/capabilities` endpoint.
- First Hermes run failed because the persisted Hermes default model was inaccessible; switching `/opt/data/config.yaml` to provider `xai` and model `grok-4` fixed model execution.
- Added a Hermes pod startup hook that applies `HERMES_MODEL` and `HERMES_INFERENCE_PROVIDER` to `/opt/data/config.yaml`.
- Manual reflection run `run_d56aacdb4f0e45b0abfda8dfd2145957` completed after approving internal HTTP adapter calls for the session.
- The run wrote reflection `hermes-reflection-1779537409085596` and created no experiment because closed-trade evidence was insufficient.
