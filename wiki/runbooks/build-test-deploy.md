---
type: runbook
tags:
  - daytrader/wiki
  - runbooks
  - testing
  - deployment
updated: 2026-05-23
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

The app namespace is `saxo-rust`. The CloudNativePG database namespace is `saxo`. Do not change app pods to reference CNPG secrets directly from the `saxo` namespace.

## Docker Desktop Deployment

Deploy the Rust app, scheduler, Hermes Agent, CNPG resources, and ngrok endpoint:

```bash
rtk make k8s-deploy
```

Check status:

```bash
rtk make k8s-status
rtk make k8s-db-status
rtk kubectl --context docker-desktop -n saxo-rust get pods,svc,cronjob,agentendpoint,ngroktrafficpolicy,pvc
```

Inspect recent logs:

```bash
rtk make k8s-logs
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/hermes-agent --tail=120
```

Stop only app resources:

```bash
rtk make k8s-stop
```

`k8s-stop` should not delete the CNPG database in namespace `saxo`.

## Kubernetes Smoke Test

After deployment, verify the app from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo-rust run daytrader-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- \
  curl -fsS http://daytrader-api.saxo-rust:8000/api/health
```

If the pod cannot resolve the service, inspect:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get endpoints daytrader-api daytrader-frontend hermes-gateway
```

## Hermes Smoke Test

Hermes is deployed as an internal-only agent. Its weekly reflection CronJob is suspended by default.

Required `.env` values before enabling scheduled Hermes runs:

```bash
HERMES_API_SERVER_ENABLED=true
HERMES_API_SERVER_HOST=0.0.0.0
HERMES_API_SERVER_KEY=<strong Hermes API key>
HERMES_INFERENCE_PROVIDER=xai
HERMES_MODEL=grok-4
HERMES_DAYTRADER_API_KEY=<strong app adapter key>
```

Do not place Saxo credentials in `hermes-env`.

If a local rustFS container already owns ports `9000-9001`, deploy with:

```bash
rtk env BACKUP_OBJECT_STORE=rustfs make k8s-deploy
```

After redeploying, check Hermes:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get deployment hermes-agent
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/hermes-agent --tail=120
```

Trigger one manual reflection run while keeping the CronJob suspended:

```bash
rtk kubectl --context docker-desktop -n saxo-rust create job --from=cronjob/hermes-weekly-reflection hermes-weekly-reflection-manual
rtk kubectl --context docker-desktop -n saxo-rust logs job/hermes-weekly-reflection-manual
```

Only unsuspend the recurring schedule after the manual job writes one reflection and, when evidence is sufficient, at most one experiment proposal:

```bash
rtk kubectl --context docker-desktop -n saxo-rust patch cronjob hermes-weekly-reflection -p '{"spec":{"suspend":false}}'
```

The current HTTP adapter can trigger Hermes terminal approval prompts for internal HTTP calls. For a fully unattended weekly schedule, either add a native MCP adapter or explicitly review and configure a narrow approval policy for the protected `daytrader-api` adapter calls.

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

The wiki is plain Markdown with Obsidian-style `[[links]]`, so it can be opened as a vault even without committing `.obsidian` settings.
