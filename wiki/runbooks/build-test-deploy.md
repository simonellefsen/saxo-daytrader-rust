---
type: runbook
tags:
  - daytrader/wiki
  - runbooks
  - testing
  - deployment
updated: 2026-07-04
---

# Build, Test, Deploy, And Smoke Runbook

This runbook is the operational checklist for changing `saxo-rust`, validating trading-critical behavior, deploying to Docker Desktop Kubernetes, and keeping the project wiki searchable for future Codex and Hermes sessions.

Use the repository wrapper for commands:

```bash
rtk <command>
```

## Build

Fast Rust compile check:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo check
```

Production-style local image build:

```bash
rtk make docker-build
```

Use `cargo check` for normal code edits and `make docker-build` before Kubernetes deployment or Dockerfile/runtime changes.

Before investigating slow Docker builds, check that the build context is still small. Local database, Cargo, qmd, Obsidian, frontend cache, and RustFS object-store directories must stay outside the Docker context. A healthy Docker build should report a context transfer in the low-megabyte range, not gigabytes:

```bash
rtk env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/opt/homebrew/sbin:/usr/sbin:/sbin docker build --progress=plain -f Dockerfile.api -t saxo-rust:context-check .
```

## Dependency And CVE Hygiene

Check for Rust dependency drift without changing files:

```bash
rtk make deps-dry-run
```

Apply lockfile-only Rust updates when they stay inside existing semver
constraints, then run the full Rust validation set:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo update
rtk make validate
```

Run security scans before releases, after base-image changes, and after adding
new dependencies:

```bash
rtk make docker-build
rtk make security-scan
```

`make security-scan` runs:

- RustSec advisory scan for `Cargo.lock` through `cargo-audit` when installed,
  or a containerized RustSec scanner when Docker is available.
- Trivy filesystem vulnerability scan for high/critical fixed CVEs.
- Trivy image scans for `daytrader-api:local` and `daytrader-backup:local`.
- Trivy secret scan over the repository.

When a scan fails:

- Patch transitive Rust crates with `cargo update` first.
- If a top-level crate must move across a major/minor API boundary, isolate it
  in a focused PR and run trading-critical tests.
- If a CVE is in a base image, rebuild after pulling the latest base image or
  move the image tag/digest forward.
- Do not ignore Saxo token, OpenRouter key, ngrok key, database credential, or
  broker account/key leaks. Remove and rotate the secret.
- Only add scanner ignore rules with an expiry date, CVE id, package, reason,
  and compensating control.

## Formatting

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo fmt --check
```

If formatting fails, run:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo fmt
```

## Unit Tests

Rust unit tests live beside the modules they verify. Run them before committing Rust behavior changes:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home cargo test
```

Current high-value unit coverage is in:

- `src/config.rs`
- `src/db.rs`
- `src/ui.rs`
- Saxo order and portfolio reset helpers where tests are added beside behavior.

When touching trading mutation code, prefer focused tests for the specific guardrail or payload shape over broad snapshot assertions.

## Integration And Regression Tests

Run the full Rust validation set before larger changes:

```bash
rtk make validate
```

For legacy Python behavior that remains the reference for the Rust port, use the narrower validation scripts that match the area changed:

```bash
rtk .venv/bin/python scripts/validate_execution_regressions.py
rtk make validate-phase6
rtk make validate-phase12
rtk make validate-phase30
rtk make validate-phase40
```

Use these when changing tick-size handling, order payloads, broker status sync, replace/cancel logic, fill reconciliation, sell reservations, bracket handling, or Saxo session mechanics.

## Local Smoke Test

Start the Rust app on a non-default port:

```bash
rtk env CARGO_HOME=/Users/lindau/codex/rust_daytrader/.cargo-home BIND_ADDR=127.0.0.1:18001 cargo run --bin saxo-rust
```

In a second terminal:

```bash
rtk curl -sS http://127.0.0.1:18001/api/health
rtk curl -sS http://127.0.0.1:18001/api/overview
```

Stop the smoke-test server before ending the work session.

For scheduler-only smoke checks, prefer mock decisions unless deliberately testing live model calls:

```bash
rtk make scheduler
```

## Kubernetes Manifest Validation

Validate manifests before deployment:

```bash
rtk kubectl apply --dry-run=client -k deploy/k8s/base
rtk bash -n scripts/deploy_k8s_docker_desktop.sh
```

The app namespace is `saxo`. The CloudNativePG database namespace is also `saxo`. Do not change app pods to reference CNPG secrets directly from the `saxo` namespace.

## Docker Desktop Deployment

Deploy the Rust app, scheduler, Hermes Agent, CNPG resources, and the app-owned internal ngrok endpoint:

```bash
rtk make k8s-deploy
```

The shared public ngrok gateway, OAuth policy, and `/saxo-daytrader` route are owned by `/Users/lindau/codex/shared-ngrok-gateway`. Apply shared edge changes there, not from this app deployment:

```bash
cd /Users/lindau/codex/shared-ngrok-gateway
rtk env ENV_FILE=/Users/lindau/codex/rust_daytrader/.env make apply
```

Check status:

```bash
rtk make k8s-status
rtk make k8s-db-status
rtk make shared-ngrok-status
rtk kubectl --context docker-desktop -n saxo get pods,svc,cronjob,agentendpoint,pvc
```

Inspect recent logs:

```bash
rtk make k8s-logs
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-mcp --tail=120
rtk kubectl --context docker-desktop -n saxo logs deployment/hermes-agent --tail=120
```

Stop only app resources:

```bash
rtk make k8s-stop
```

`k8s-stop` should not delete the CNPG database in namespace `saxo`.

## Kubernetes Smoke Test

After deployment, run the read-only smoke target:

```bash
rtk make post-deploy-smoke
```

This checks deployment rollouts, the internal ngrok AgentEndpoint, the health
endpoint, overview/scheduler reachability, Saxo session status, authenticated
decision-report schema health, MCP `tools/list` discovery for Hermes-safe tools,
and Hermes gateway health. A broken rollout, missing health endpoint, invalid
decision-report schema, missing expected MCP tool, or unhealthy Hermes gateway
fails the smoke. A Saxo SIM session that needs reauth is
reported as a warning because it blocks broker refresh/execution but does not
mean the Rust web runtime failed to deploy.

To also fail the smoke check when a deployed image differs from the expected
image, pass one shared daytrader image or per-deployment images:

```bash
rtk env EXPECTED_DAYTRADER_IMAGE=daytrader-api:local make post-deploy-smoke
rtk env EXPECTED_API_IMAGE=daytrader-api:local EXPECTED_SCHEDULER_IMAGE=daytrader-api:local EXPECTED_MCP_IMAGE=daytrader-api:local EXPECTED_HERMES_IMAGE=docker.io/nousresearch/hermes-agent@sha256:<digest> make post-deploy-smoke
```

For a narrower in-cluster service check, verify the app from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo run daytrader-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- \
  curl -fsS http://daytrader-api.saxo:8000/api/health
```

If the pod cannot resolve the service, inspect:

```bash
rtk kubectl --context docker-desktop -n saxo get endpoints daytrader-api daytrader-frontend daytrader-mcp hermes-gateway
```

## Hermes Smoke Test

Hermes is deployed as an internal-only agent. Its daily EOD and weekly reflection CronJobs are suspended by default.

Required `.env` values before enabling scheduled Hermes runs:

```bash
HERMES_API_SERVER_ENABLED=true
HERMES_API_SERVER_HOST=0.0.0.0
HERMES_API_SERVER_KEY=<strong Hermes API key>
OPENROUTER_API_KEY=...
HERMES_INFERENCE_PROVIDER=openrouter
HERMES_MODEL=openai/gpt-5.5
HERMES_DAYTRADER_API_KEY=<strong app adapter key>
HERMES_DAYTRADER_MCP_URL=http://daytrader-mcp.saxo:8610/mcp
HERMES_GATEWAY_URL=http://hermes-gateway.saxo:8642
HERMES_TRADING_MANAGER_ADVISORY_ENABLED=true
HERMES_TRADING_MANAGER_ADVISORY_MODE=conservative
HERMES_TRADING_MANAGER_ADVISORY_WAIT_SECONDS=90
```

Do not place Saxo credentials in `hermes-env`.

RustFS is the normal S3-compatible backup backend. It runs in the Docker context so backup objects can persist to a local filesystem bind mount. Deploy with:

```bash
rtk env BACKUP_OBJECT_STORE=rustfs make k8s-deploy
```

After redeploying, check Hermes:

```bash
rtk kubectl --context docker-desktop -n saxo get deployment hermes-agent
rtk kubectl --context docker-desktop -n saxo get deployment daytrader-mcp
rtk kubectl --context docker-desktop -n saxo logs deployment/hermes-agent --tail=120
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-mcp --tail=120
```

Check the MCP adapter health:

```bash
rtk kubectl --context docker-desktop -n saxo run daytrader-mcp-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- \
  curl -fsS http://daytrader-mcp.saxo:8610/health
```

Trigger one manual daily EOD learning run while keeping the CronJob suspended:

```bash
rtk kubectl --context docker-desktop -n saxo create job --from=cronjob/hermes-daily-reflection hermes-daily-reflection-manual
rtk kubectl --context docker-desktop -n saxo logs job/hermes-daily-reflection-manual
```

Trigger one manual weekly self-improvement learning run while keeping the CronJob suspended:

```bash
rtk kubectl --context docker-desktop -n saxo create job --from=cronjob/hermes-weekly-reflection hermes-weekly-reflection-manual
rtk kubectl --context docker-desktop -n saxo logs job/hermes-weekly-reflection-manual
```

The Hermes CronJobs submit asynchronous `/v1/runs` requests and then wait for a matching `source_session_id` reflection in the daytrader database. If no row appears inside the watchdog window, the job writes a watchdog reflection through `/api/hermes/reflections` so the dashboard records the missed run instead of leaving the latest reflection stale. Experiment proposals are optional audited side effects and remain `pending_review` until an operator acts in the dashboard.

Trading Manager decision advice uses the same Hermes gateway. A fresh decision report submits an advisory run with `source_session_id=decision-advice-<report_id>` and waits for Hermes to call the MCP `create_decision_advice` tool. The response is stored in `hermes_decision_advice` and copied into `trading_manager_runs.manager_json`. Kubernetes runs `HERMES_TRADING_MANAGER_ADVISORY_MODE=conservative`, so Hermes can only block, reduce, or require review; it cannot add or enlarge trades. If conservative advice is missing or times out, the manager records a review-required advisory state instead of silently proceeding.

Only unsuspend the daily recurring schedule after the manual job writes one daily reflection and creates at most one pending-review proposal when evidence supports it:

```bash
rtk kubectl --context docker-desktop -n saxo patch cronjob hermes-daily-reflection -p '{"spec":{"suspend":false}}'
```

Only unsuspend the weekly recurring schedule after the manual job writes one reflection and creates one pending-review proposal when evidence is sufficient and no duplicate proposal exists:

```bash
rtk kubectl --context docker-desktop -n saxo patch cronjob hermes-weekly-reflection -p '{"spec":{"suspend":false}}'
```

The daily job should prefer the configured `daytrader` MCP tools for context, decision reports, EOD reports, Markov signals, recent experiments, reflection writes, and proposal writes. The weekly job should prefer the same tool surface and should convert clear weekly learnings into one-variable proposals. The protected HTTP adapter remains available for manual inspection and fallback.

## Saxo SIM Testing

Saxo SIM testing is still broker-facing. Treat it as a controlled integration test, not a unit test.

Before using SIM:

- Confirm `saxo.environment` is `SIM`.
- Confirm the active Saxo session is a SIM session.
- Confirm `execution.adapter=saxo` only when intentionally testing Saxo broker integration.
- Keep `app.dry_run=true` unless the test explicitly requires SIM order submission.
- Keep `execution.require_approval_live=true` unless the test explicitly verifies auto-submission in SIM.
- Never reuse LIVE `AccountKey`, `ClientKey`, OAuth payloads, or session cache for SIM.

Safe SIM verification order:

1. Session status and refresh.
2. Portfolio and positions read.
3. Instrument lookup for a known small test symbol.
4. Order precheck only.
5. One tiny SIM order submission if explicitly approved for the test.
6. Broker order status sync.
7. Fill reconciliation back into local audit tables.

Every SIM broker mutation should leave local audit records in `execution_orders`, `execution_order_events`, and, when filled, `execution_fills`.

## Hermes SIM/Paper Overlays

Hermes experiment proposals start as `pending_review` and do not affect trading.

For an operator-approved paper or SIM test, use one of these statuses:

- `approved_paper`
- `active_paper`
- `approved_sim`
- `active_sim`

The Rust Trading Manager only loads these overlays when `execution.mode` is not `live`, or when `saxo.environment=SIM`. It will not load overlays for `execution.mode=live` with `saxo.environment=LIVE`.

Current allowlist:

- `execution.min_trade_value_dkk`
- `strategy.capital.min_cash_buffer_pct`
- `strategy.swing.cash_buffer_pct`
- `strategy.swing.daily_indicators.min_confluences`

After a scheduler cycle, verify the applied overlay in `trading_manager_runs.manager_json` and queued order `request_json` before any SIM broker submission.

Promotion flow:

1. Keep new proposals in `pending_review`.
2. Approve and activate paper first from the dashboard `Hermes` tab.
3. Move to SIM only after paper evidence is acceptable.
4. Mark `ready_for_promotion` only after the SIM observation meets the goal contract.
5. Promote from the dashboard to create a `strategy_baselines` audit record.

Promotion records do not activate live broker behavior. Treat a promoted baseline as an audited reference point. After promotion, verify that the baseline appears in the dashboard `Hermes` tab, `/api/hermes/context`, and the next AI decision report `request_json.user.active_strategy_baseline` payload.

## Live Trading Safety Gate

Do not run LIVE broker mutation tests as part of routine validation.

LIVE execution requires an explicit operator decision, a current Saxo session, green local tests, green manifest validation, reviewed queued orders, and a clear rollback/kill path. Hermes must never directly approve, place, replace, or cancel Saxo orders.

## Wiki And Knowledge Maintenance

After changing architecture, runbooks, strategy policy, deployment behavior, or Hermes flow:

1. Update the relevant `wiki/` page.
2. Add an entry to `wiki/log.md`.
3. Refresh the Markdown index:

```bash
rtk qmd update
```

Useful searches:

```bash
rtk qmd search "Saxo SIM testing"
rtk qmd query "Hermes reflection experiment promotion"
```

The wiki is plain Markdown with relative `.md` links, so links work in GitHub previews and remain readable when opened as an Obsidian vault without committing `.obsidian` settings.
