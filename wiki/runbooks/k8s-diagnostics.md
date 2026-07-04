---
type: runbook
tags:
  - daytrader/wiki
  - runbooks
  - kubernetes
  - diagnostics
updated: 2026-07-04
---

# Kubernetes Diagnostics One-Liners

Use these one-liners for quick Docker Desktop Kubernetes diagnostics. App resources run in namespace `saxo`; CloudNativePG database resources run in namespace `saxo`; RustFS runs outside Kubernetes in the Docker context so it can persist objects to the local filesystem.

## One-Command Bundle

Start with the read-only diagnostics bundle when you need an operator snapshot:

```bash
rtk make diagnostics
```

The bundle collects pod/service/job status, rollout state, resource usage, recent events, scheduler/API/Hermes logs, RustFS backup status, shared ngrok status, and sanitized app summaries for portfolio performance, Saxo session health, scheduler state, latest decision report, latest Trading Manager run, Hermes advice, Markov freshness, integrity, and recent execution rows. It starts a temporary local port-forward to the API service and cleans it up automatically. It does not trigger reports, process the execution queue, mutate broker state, or print raw Saxo token/account payloads.

To save the same output to a timestamped local artifact for Slack or issue sharing:

```bash
rtk make diagnostics-artifact
```

Artifacts are written to `.diagnostics/daytrader-diagnostics-<utc timestamp>.log` and are ignored by git and Docker builds. To choose an explicit path:

```bash
rtk env DIAGNOSTICS_ARTIFACT=/tmp/daytrader-diagnostics.log make diagnostics
```

To include the public ngrok health check, pass the public base URL or the shared gateway domain:

```bash
rtk env PUBLIC_BASE_URL=https://<domain>/saxo-daytrader make diagnostics
rtk env NGROK_DOMAIN=<domain> make diagnostics
```

## Cluster Snapshot

Current context:

```bash
rtk kubectl config current-context
```

App namespace overview:

```bash
rtk kubectl --context docker-desktop -n saxo get pods,deploy,svc,cronjob,pvc,agentendpoint -o wide
```

Database namespace overview:

```bash
rtk kubectl --context docker-desktop -n saxo get cluster,pods,svc,pvc,backup,scheduledbackup -o wide
```

Recent cluster events:

```bash
rtk kubectl --context docker-desktop -n saxo get events --sort-by=.lastTimestamp
```

Recent database events:

```bash
rtk kubectl --context docker-desktop -n saxo get events --sort-by=.lastTimestamp
```

## Pod Debugging

List non-running app pods:

```bash
rtk kubectl --context docker-desktop -n saxo get pods --field-selector=status.phase!=Running
```

Describe a failing pod:

```bash
rtk kubectl --context docker-desktop -n saxo describe pod <pod-name>
```

Previous logs after a crash:

```bash
rtk kubectl --context docker-desktop -n saxo logs <pod-name> --previous --tail=160
```

API logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-api --tail=160
```

Scheduler logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-scheduler --tail=160
```

Hermes logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs deployment/hermes-agent --tail=160
```

Daytrader MCP logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-mcp --tail=160
```

Follow logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs deployment/daytrader-api -f --tail=80
```

## Rollouts

Check rollout status:

```bash
rtk kubectl --context docker-desktop -n saxo rollout status deployment/daytrader-api --timeout=180s
rtk kubectl --context docker-desktop -n saxo rollout status deployment/daytrader-scheduler --timeout=180s
rtk kubectl --context docker-desktop -n saxo rollout status deployment/daytrader-mcp --timeout=180s
rtk kubectl --context docker-desktop -n saxo rollout status deployment/hermes-agent --timeout=180s
```

Run the read-only post-deploy smoke check, including decision-report schema
health and Hermes-safe MCP tool discovery:

```bash
rtk make post-deploy-smoke
```

Restart app workloads:

```bash
rtk kubectl --context docker-desktop -n saxo rollout restart deployment/daytrader-api deployment/daytrader-scheduler deployment/daytrader-mcp
```

Restart Hermes only:

```bash
rtk kubectl --context docker-desktop -n saxo rollout restart deployment/hermes-agent
```

See current image tags:

```bash
rtk kubectl --context docker-desktop -n saxo get deploy daytrader-api daytrader-scheduler daytrader-mcp hermes-agent -o jsonpath='{range .items[*]}{.metadata.name}{" "}{range .spec.template.spec.containers[*]}{.name}={.image}{" "}{end}{"\n"}{end}'
```

## Network Smoke Tests

App health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo run daytrader-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://daytrader-api.saxo:8000/api/health
```

Hermes health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo run hermes-health-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://hermes-gateway.saxo:8642/health
```

Daytrader MCP health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo run daytrader-mcp-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://daytrader-mcp.saxo:8610/health
```

Service endpoints:

```bash
rtk kubectl --context docker-desktop -n saxo get endpoints daytrader-api daytrader-frontend daytrader-mcp hermes-gateway
```

Port-forward the app:

```bash
rtk kubectl --context docker-desktop -n saxo port-forward svc/daytrader-frontend 18000:8000
```

Port-forward Hermes:

```bash
rtk kubectl --context docker-desktop -n saxo port-forward svc/hermes-gateway 18642:8642
```

Port-forward Daytrader MCP:

```bash
rtk kubectl --context docker-desktop -n saxo port-forward svc/daytrader-mcp 18610:8610
```

## CloudNativePG

Cluster health:

```bash
rtk kubectl --context docker-desktop -n saxo get cluster daytrader-postgres
```

Postgres pods:

```bash
rtk kubectl --context docker-desktop -n saxo get pods -l cnpg.io/cluster=daytrader-postgres -o wide
```

CNPG operator logs:

```bash
rtk kubectl --context docker-desktop -n cnpg-system logs deployment/cnpg-cloudnative-pg --tail=160
```

Database service DNS:

```bash
rtk kubectl --context docker-desktop -n saxo get svc daytrader-postgres-rw
```

Check app-local database secret exists:

```bash
rtk kubectl --context docker-desktop -n saxo get secret daytrader-postgres-app
```

## RustFS Backups

RustFS is the normal S3-compatible storage backend. It runs in the Docker context, not inside Kubernetes, so it can use a local filesystem bind mount for object persistence.

Check RustFS container:

```bash
rtk docker ps --filter name=daytrader_rustfs --format 'table {{.Names}}\t{{.Image}}\t{{.Ports}}'
```

Check port ownership for `9000-9001`:

```bash
rtk docker ps --format 'table {{.Names}}\t{{.Ports}}' | rg '9000|9001'
```

Deploy using RustFS:

```bash
rtk env BACKUP_OBJECT_STORE=rustfs make k8s-deploy
```

Check backup secret exists. The secret name still contains `minio` for historical compatibility, but its endpoint can point at RustFS:

```bash
rtk kubectl --context docker-desktop -n saxo get secret daytrader-minio-backup
```

## ngrok Endpoint

The shared public ngrok endpoint, OAuth policy, and `/saxo-daytrader` route are owned by `/Users/lindau/codex/shared-ngrok-gateway`. This repository owns only the internal `saxo-daytrader.internal` AgentEndpoint.

App-owned internal endpoint:

```bash
rtk kubectl --context docker-desktop -n saxo get agentendpoint saxo-daytrader-internal -o wide
```

Shared gateway status from this repo:

```bash
rtk make shared-ngrok-status
```

Shared gateway status from the owner repo:

```bash
cd /Users/lindau/codex/shared-ngrok-gateway
rtk make status
```

Shared traffic policy, when debugging the public edge:

```bash
rtk kubectl --context docker-desktop -n saxo describe ngroktrafficpolicy daytrader-oauth
```

Apply shared route/OAuth changes only from the shared gateway repo:

```bash
cd /Users/lindau/codex/shared-ngrok-gateway
rtk env ENV_FILE=/Users/lindau/codex/rust_daytrader/.env make apply
```

ngrok operator logs, if the shared gateway resources are unhealthy:

```bash
rtk kubectl --context docker-desktop -n ngrok-operator logs deployment/ngrok-operator --tail=160
```

## Hermes

CronJob status:

```bash
rtk kubectl --context docker-desktop -n saxo get cronjob hermes-daily-reflection
rtk kubectl --context docker-desktop -n saxo get cronjob hermes-weekly-reflection
```

Manual daily EOD reflection job:

```bash
rtk kubectl --context docker-desktop -n saxo create job --from=cronjob/hermes-daily-reflection hermes-daily-reflection-manual
```

Manual weekly reflection job:

```bash
rtk kubectl --context docker-desktop -n saxo create job --from=cronjob/hermes-weekly-reflection hermes-weekly-reflection-manual
```

Reflection job logs:

```bash
rtk kubectl --context docker-desktop -n saxo logs job/hermes-daily-reflection-manual
rtk kubectl --context docker-desktop -n saxo logs job/hermes-weekly-reflection-manual
```

Verify Hermes persisted model config:

```bash
rtk kubectl --context docker-desktop -n saxo exec deployment/hermes-agent -- sh -lc 'grep -nE "^model:|^  default:|^  provider:" /opt/data/config.yaml | sed -n "1,20p"'
```

Verify Hermes persisted MCP config:

```bash
rtk kubectl --context docker-desktop -n saxo exec deployment/hermes-agent -- sh -lc '/opt/hermes/.venv/bin/python - <<'"'"'PY'"'"'
import yaml
data = yaml.safe_load(open("/opt/data/config.yaml")) or {}
server = (data.get("mcp_servers") or {}).get("daytrader") or {}
print("url=" + str(server.get("url")))
print("tools=" + ",".join((server.get("tools") or {}).get("include") or []))
print("has_authorization_header=" + str("Authorization" in (server.get("headers") or {})))
PY'
```

## Cleanup

Delete old completed manual Hermes jobs:

```bash
rtk kubectl --context docker-desktop -n saxo delete job -l app=hermes-agent,component=reflection --ignore-not-found
```

Remove a one-off smoke pod if it sticks:

```bash
rtk kubectl --context docker-desktop -n saxo delete pod daytrader-smoke hermes-health-smoke --ignore-not-found
```
