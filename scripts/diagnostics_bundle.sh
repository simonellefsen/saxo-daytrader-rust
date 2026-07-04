#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KUBE_CONTEXT="${KUBE_CONTEXT:-docker-desktop}"
APP_NAMESPACE="${APP_NAMESPACE:-saxo}"
DB_NAMESPACE="${DB_NAMESPACE:-saxo}"
API_SERVICE="${API_SERVICE:-daytrader-api}"
API_LOCAL_PORT="${API_LOCAL_PORT:-18080}"
DIAGNOSTICS_CAPTURE="${DIAGNOSTICS_CAPTURE:-0}"
DIAGNOSTICS_OUTPUT_DIR="${DIAGNOSTICS_OUTPUT_DIR:-$ROOT/.diagnostics}"
DIAGNOSTICS_ARTIFACT="${DIAGNOSTICS_ARTIFACT:-}"
PUBLIC_BASE_URL="${PUBLIC_BASE_URL:-}"
if [[ -z "$PUBLIC_BASE_URL" && -n "${NGROK_DOMAIN:-}" ]]; then
  PUBLIC_BASE_URL="https://${NGROK_DOMAIN}/saxo-daytrader"
fi
SHARED_NGROK_GATEWAY_DIR="${SHARED_NGROK_GATEWAY_DIR:-$ROOT/../shared-ngrok-gateway}"

if [[ "$DIAGNOSTICS_CAPTURE" == "1" || -n "$DIAGNOSTICS_ARTIFACT" ]]; then
  if [[ -z "$DIAGNOSTICS_ARTIFACT" ]]; then
    mkdir -p "$DIAGNOSTICS_OUTPUT_DIR"
    DIAGNOSTICS_ARTIFACT="$DIAGNOSTICS_OUTPUT_DIR/daytrader-diagnostics-$(date -u +"%Y%m%dT%H%M%SZ").log"
  else
    mkdir -p "$(dirname "$DIAGNOSTICS_ARTIFACT")"
  fi
  exec > >(tee "$DIAGNOSTICS_ARTIFACT") 2>&1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/daytrader-diagnostics.XXXXXX")"
PORT_FORWARD_PID=""

cleanup() {
  if [[ -n "$PORT_FORWARD_PID" ]] && kill -0 "$PORT_FORWARD_PID" 2>/dev/null; then
    kill "$PORT_FORWARD_PID" 2>/dev/null || true
    wait "$PORT_FORWARD_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

section() {
  printf "\n## %s\n\n" "$1"
}

run() {
  printf '$ %s\n' "$*"
  "$@"
}

optional() {
  printf '$ %s\n' "$*"
  "$@" || printf 'warning: command failed with exit code %s\n' "$?"
}

have() {
  command -v "$1" >/dev/null 2>&1
}

fetch_json() {
  local path="$1"
  local out="$2"
  curl -fsS "http://127.0.0.1:${API_LOCAL_PORT}${path}" -o "$out"
}

public_head_check() {
  local url="$1"
  printf '$ curl -fsSI %s | redact OAuth redirect/session headers\n' "$url"
  curl -fsSI "$url" | sed -E '/^(set-cookie|location):/Id'
}

summarize_api() {
  python3 - "$TMP_DIR/overview.json" "$TMP_DIR/execution.json" "$TMP_DIR/performance.json" <<'PY'
import json
import sys
from pathlib import Path

def load(path):
    try:
        return json.loads(Path(path).read_text())
    except Exception as exc:
        return {"error": str(exc)}

overview = load(sys.argv[1])
execution = load(sys.argv[2])
performance = load(sys.argv[3])

portfolio = overview.get("portfolio_summary") or {}
goals = (overview.get("goal_tracking") or {}).get("periods") or {}
latest_run = (overview.get("trading_manager") or {}).get("latest_run") or {}
manager_json = latest_run.get("manager_json") or {}
hermes = manager_json.get("hermes_decision_advice") or {}
markov = ((overview.get("markov_method") or {}).get("latest_run")) or {}
orders = execution.get("orders") or []
events = execution.get("events") or []
perf_summary = performance.get("summary") or {}

summary = {
    "portfolio": {
        "total_value_dkk": portfolio.get("total_market_value_dkk"),
        "cash_dkk": portfolio.get("cash_balance_dkk"),
        "invested_dkk": portfolio.get("invested_market_value_dkk"),
        "daily_pnl_dkk": portfolio.get("total_daily_pnl_dkk"),
        "unrealised_pnl_dkk": portfolio.get("total_unrealised_pnl_dkk"),
        "positions": portfolio.get("position_count"),
    },
    "performance": {
        "latest_recorded_at": perf_summary.get("latest_recorded_at"),
        "range_change_dkk": perf_summary.get("change_dkk"),
        "range_daily_pnl_dkk": perf_summary.get("daily_pnl_dkk"),
    },
    "goals": {
        "week": goals.get("week"),
        "month": goals.get("month"),
    },
    "saxo": {
        key: (overview.get("saxo_auth") or {}).get(key)
        for key in ["status", "connected", "needs_reauth", "expires_in_minutes", "refresh_expires_in_minutes"]
    },
    "scheduler": {
        key: (overview.get("scheduler_status") or {}).get(key)
        for key in ["last_cycle_status", "last_heartbeat_at", "last_cycle_completed_at"]
    },
    "execution": overview.get("execution"),
    "latest_decision": overview.get("latest_decision"),
    "latest_trading_manager": {
        "id": latest_run.get("id"),
        "report_id": latest_run.get("report_id"),
        "status": latest_run.get("status"),
        "created_at": latest_run.get("created_at"),
        "approved_order_count": manager_json.get("approved_order_count"),
        "skipped_order_count": manager_json.get("skipped_order_count"),
        "queue_status": (latest_run.get("queue_result_json") or {}).get("status"),
        "hermes_status": hermes.get("status"),
        "hermes_mode": hermes.get("mode"),
        "hermes_recommendation": hermes.get("overall_recommendation"),
        "hermes_summary": hermes.get("summary"),
    },
    "markov": {
        "run_date": markov.get("run_date"),
        "status": markov.get("status"),
        "success_count": markov.get("success_count"),
        "error_count": markov.get("error_count"),
        "asset_count": markov.get("asset_count"),
    },
    "integrity": overview.get("integrity"),
    "recent_orders": [
        {
            key: order.get(key)
            for key in [
                "id",
                "created_at",
                "report_id",
                "symbol",
                "action",
                "status",
                "quantity",
                "currency",
                "limit_price_local",
                "broker_order_id",
                "error_text",
            ]
        }
        for order in orders[:12]
    ],
    "recent_events": [
        {
            key: event.get(key)
            for key in [
                "id",
                "created_at",
                "execution_order_id",
                "event_type",
                "broker_status",
                "broker_substatus",
                "broker_quantity",
                "broker_price_local",
            ]
        }
        for event in events[:12]
    ],
}

print(json.dumps(summary, indent=2, sort_keys=True, default=str))
PY
}

printf "# Saxo Daytrader Diagnostics\n\n"
printf -- "- captured_at_utc: %s\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
printf -- "- kube_context: %s\n" "$KUBE_CONTEXT"
printf -- "- app_namespace: %s\n" "$APP_NAMESPACE"
printf -- "- db_namespace: %s\n" "$DB_NAMESPACE"
printf -- "- public_base_url: %s\n" "${PUBLIC_BASE_URL:-not configured}"
if [[ -n "$DIAGNOSTICS_ARTIFACT" ]]; then
  printf -- "- artifact: %s\n" "$DIAGNOSTICS_ARTIFACT"
fi

section "Repository"
optional git -C "$ROOT" rev-parse --short HEAD
optional git -C "$ROOT" status --short

section "Kubernetes Resources"
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get pods,svc,agentendpoint,pvc
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get cronjobs,jobs
optional kubectl --context "$KUBE_CONTEXT" -n "$DB_NAMESPACE" get cluster,svc,pvc

section "Rollouts"
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" rollout status deployment/daytrader-api --timeout=20s
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" rollout status deployment/daytrader-scheduler --timeout=20s
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" rollout status deployment/daytrader-mcp --timeout=20s
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" rollout status deployment/hermes-agent --timeout=20s

section "Resource Usage"
optional kubectl --context "$KUBE_CONTEXT" top nodes
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" top pods

section "Recent Events"
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get events --sort-by=.lastTimestamp

section "Recent Logs"
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" logs deployment/daytrader-scheduler --tail=160
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" logs deployment/daytrader-api --tail=80
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" logs deployment/hermes-agent --tail=80

section "RustFS Backups"
if have docker; then
  optional docker ps --filter name=daytrader_rustfs --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}'
  optional docker inspect daytrader_rustfs --format 'restart_policy={{.HostConfig.RestartPolicy.Name}} state={{.State.Status}}'
else
  printf 'warning: docker not found\n'
fi
optional kubectl --context "$KUBE_CONTEXT" -n "$DB_NAMESPACE" get backup

section "ngrok"
optional kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get agentendpoint saxo-daytrader-internal -o wide
if [[ -d "$SHARED_NGROK_GATEWAY_DIR" ]]; then
  optional make -C "$SHARED_NGROK_GATEWAY_DIR" KUBE_CONTEXT="$KUBE_CONTEXT" status
else
  printf 'warning: shared ngrok gateway dir not found: %s\n' "$SHARED_NGROK_GATEWAY_DIR"
fi
if [[ -n "$PUBLIC_BASE_URL" ]]; then
  optional public_head_check "${PUBLIC_BASE_URL}/api/health"
else
  printf 'warning: PUBLIC_BASE_URL or NGROK_DOMAIN not configured; skipping public health HEAD check\n'
fi

section "Application API Summary"
kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" port-forward "svc/${API_SERVICE}" "${API_LOCAL_PORT}:8000" >"$TMP_DIR/port-forward.log" 2>&1 &
PORT_FORWARD_PID="$!"
sleep 2

if fetch_json /api/overview "$TMP_DIR/overview.json" \
  && fetch_json /api/execution "$TMP_DIR/execution.json" \
  && fetch_json /api/performance "$TMP_DIR/performance.json"; then
  summarize_api
else
  printf 'warning: failed to fetch application API through port-forward\n'
  cat "$TMP_DIR/port-forward.log" || true
fi
