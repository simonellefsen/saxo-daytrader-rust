---
type: runbook
tags:
  - daytrader/wiki
  - runbooks
  - kubernetes
  - diagnostics
updated: 2026-05-23
---

# Kubernetes Diagnostics One-Liners

Use these one-liners for quick Docker Desktop Kubernetes diagnostics. App resources run in namespace `saxo-rust`; CloudNativePG database resources run in namespace `saxo`; RustFS runs outside Kubernetes in the Docker context so it can persist objects to the local filesystem.

## Cluster Snapshot

Current context:

```bash
rtk kubectl config current-context
```

App namespace overview:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get pods,deploy,svc,cronjob,pvc,agentendpoint,ngroktrafficpolicy -o wide
```

Database namespace overview:

```bash
rtk kubectl --context docker-desktop -n saxo get cluster,pods,svc,pvc,backup,scheduledbackup -o wide
```

Recent cluster events:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get events --sort-by=.lastTimestamp
```

Recent database events:

```bash
rtk kubectl --context docker-desktop -n saxo get events --sort-by=.lastTimestamp
```

## Pod Debugging

List non-running app pods:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get pods --field-selector=status.phase!=Running
```

Describe a failing pod:

```bash
rtk kubectl --context docker-desktop -n saxo-rust describe pod <pod-name>
```

Previous logs after a crash:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs <pod-name> --previous --tail=160
```

API logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/daytrader-api --tail=160
```

Scheduler logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/daytrader-scheduler --tail=160
```

Hermes logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/hermes-agent --tail=160
```

Daytrader MCP logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/daytrader-mcp --tail=160
```

Follow logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs deployment/daytrader-api -f --tail=80
```

## Rollouts

Check rollout status:

```bash
rtk kubectl --context docker-desktop -n saxo-rust rollout status deployment/daytrader-api --timeout=180s
rtk kubectl --context docker-desktop -n saxo-rust rollout status deployment/daytrader-scheduler --timeout=180s
rtk kubectl --context docker-desktop -n saxo-rust rollout status deployment/daytrader-mcp --timeout=180s
rtk kubectl --context docker-desktop -n saxo-rust rollout status deployment/hermes-agent --timeout=180s
```

Restart app workloads:

```bash
rtk kubectl --context docker-desktop -n saxo-rust rollout restart deployment/daytrader-api deployment/daytrader-scheduler deployment/daytrader-mcp
```

Restart Hermes only:

```bash
rtk kubectl --context docker-desktop -n saxo-rust rollout restart deployment/hermes-agent
```

See current image tags:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get deploy daytrader-api daytrader-scheduler daytrader-mcp hermes-agent -o jsonpath='{range .items[*]}{.metadata.name}{" "}{range .spec.template.spec.containers[*]}{.name}={.image}{" "}{end}{"\n"}{end}'
```

## Network Smoke Tests

App health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo-rust run daytrader-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://daytrader-api.saxo-rust:8000/api/health
```

Hermes health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo-rust run hermes-health-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://hermes-gateway.saxo-rust:8642/health
```

Daytrader MCP health from inside the cluster:

```bash
rtk kubectl --context docker-desktop -n saxo-rust run daytrader-mcp-smoke --rm -i --restart=Never --image=curlimages/curl:8.17.0 -- curl -fsS http://daytrader-mcp.saxo-rust:8610/health
```

Service endpoints:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get endpoints daytrader-api daytrader-frontend daytrader-mcp hermes-gateway
```

Port-forward the app:

```bash
rtk kubectl --context docker-desktop -n saxo-rust port-forward svc/daytrader-frontend 18000:8000
```

Port-forward Hermes:

```bash
rtk kubectl --context docker-desktop -n saxo-rust port-forward svc/hermes-gateway 18642:8642
```

Port-forward Daytrader MCP:

```bash
rtk kubectl --context docker-desktop -n saxo-rust port-forward svc/daytrader-mcp 18610:8610
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
rtk kubectl --context docker-desktop -n saxo-rust get secret daytrader-postgres-app
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

Endpoint status:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get agentendpoint daytrader-frontend -o wide
```

Traffic policy:

```bash
rtk kubectl --context docker-desktop -n saxo-rust describe ngroktrafficpolicy daytrader-oauth
```

ngrok operator logs:

```bash
rtk kubectl --context docker-desktop -n ngrok-operator logs deployment/ngrok-operator --tail=160
```

## Hermes

CronJob status:

```bash
rtk kubectl --context docker-desktop -n saxo-rust get cronjob hermes-daily-reflection
rtk kubectl --context docker-desktop -n saxo-rust get cronjob hermes-weekly-reflection
```

Manual daily EOD reflection job:

```bash
rtk kubectl --context docker-desktop -n saxo-rust create job --from=cronjob/hermes-daily-reflection hermes-daily-reflection-manual
```

Manual weekly reflection job:

```bash
rtk kubectl --context docker-desktop -n saxo-rust create job --from=cronjob/hermes-weekly-reflection hermes-weekly-reflection-manual
```

Reflection job logs:

```bash
rtk kubectl --context docker-desktop -n saxo-rust logs job/hermes-daily-reflection-manual
rtk kubectl --context docker-desktop -n saxo-rust logs job/hermes-weekly-reflection-manual
```

Verify Hermes persisted model config:

```bash
rtk kubectl --context docker-desktop -n saxo-rust exec deployment/hermes-agent -- sh -lc 'grep -nE "^model:|^  default:|^  provider:" /opt/data/config.yaml | sed -n "1,20p"'
```

Verify Hermes persisted MCP config:

```bash
rtk kubectl --context docker-desktop -n saxo-rust exec deployment/hermes-agent -- sh -lc '/opt/hermes/.venv/bin/python - <<'"'"'PY'"'"'
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
rtk kubectl --context docker-desktop -n saxo-rust delete job -l app=hermes-agent,component=reflection --ignore-not-found
```

Remove a one-off smoke pod if it sticks:

```bash
rtk kubectl --context docker-desktop -n saxo-rust delete pod daytrader-smoke hermes-health-smoke --ignore-not-found
```
