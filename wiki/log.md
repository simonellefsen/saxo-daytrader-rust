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

- Read the LLM Wiki source now archived at [wiki/sources/llm-wiki.md](/Users/lindau/codex/rust_daytrader/wiki/sources/llm-wiki.md).
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

## [2026-05-23] runbook | Kubernetes diagnostics

- Added `wiki/runbooks/k8s-diagnostics.md`.
- Documented simple one-liners for Docker Desktop Kubernetes diagnostics, pod debugging, rollouts, in-cluster smoke tests, CloudNativePG, ngrok, Hermes, and RustFS.
- Clarified that RustFS is the normal S3-compatible storage backend and runs in the Docker context to use a local filesystem bind mount.

## [2026-05-23] implementation | Hermes review dashboard

- Added a read-only `Hermes` dashboard tab at `/?view=hermes`.
- Loaded recent `hermes_reflections` and `strategy_experiments` into the server-rendered dashboard model.
- Displayed the latest reflection summary, proposed actions, reflection history, experiment proposal status, one-variable path, and evidence preview.
- Kept the UI review-only; it does not approve, activate, promote, or mutate strategy baselines.

## [2026-05-23] implementation | Hermes SIM/paper overlays

- Added Trading Manager support for one approved Hermes experiment overlay in paper/simulation or Saxo SIM.
- Allowed only `approved_sim`, `active_sim`, `approved_paper`, and `active_paper` experiment statuses.
- Limited overlays to cash buffer, minimum trade value, and daily technical minimum confluence variables.
- Recorded the applied overlay in Trading Manager run JSON and queued order request JSON for auditability.
- Kept overlays disabled for `execution.mode=live` with `saxo.environment=LIVE`.

## [2026-05-23] maintenance | Remove duplicate root LLM Wiki source

- Kept the project copy of the LLM Wiki source note under [wiki/sources/llm-wiki.md](/Users/lindau/codex/rust_daytrader/wiki/sources/llm-wiki.md).
- Removed the duplicate root-level `llm-wiki.md`.
- Updated wiki metadata and docs to point at the wiki source note and original Andrej Karpathy gist.

## [2026-05-23] implementation | Hermes experiment lifecycle

- Added dashboard actions for Hermes experiment lifecycle transitions from `pending_review` through paper, SIM, readiness, rejection/failure, and promotion.
- Added an operator Rust transition path that records actor, action, status transition, notes, timestamp, and promoted baseline id in `approval_json`.
- Promotion creates a `strategy_baselines` audit record and supersedes prior active baseline records.
- Kept promotion as an audit/control-plane record only; it does not activate live broker behavior.

## [2026-05-23] implementation | Hermes baseline context

- Added active baseline visibility to the dashboard `Hermes` tab.
- Included the active `strategy_baselines` audit record in the protected Hermes context adapter.
- Included the active baseline payload in xAI decision prompts and required decision reports to return `strategy_baseline_id`.
- Kept baseline context advisory only; it does not approve orders, mutate Saxo sessions, or enable live overlays.

## [2026-05-23] implementation | Daytrader MCP adapter

- Added `saxo-rust --mcp-http`, an internal MCP endpoint for Hermes-safe daytrader tools.
- Added `Deployment/daytrader-mcp` and `Service/daytrader-mcp` in the `saxo-rust` namespace.
- Configured the Hermes pod startup hook to persist a filtered `daytrader` HTTP MCP server in `/opt/data/config.yaml`.
- Updated the weekly reflection job prompt to prefer MCP tools for context, reflection writes, and one-variable experiment proposals.
- Kept the MCP surface free of Saxo session reads, broker mutation tools, Kubernetes secret tools, and live order approval.
