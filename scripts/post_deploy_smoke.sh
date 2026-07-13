#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KUBE_CONTEXT="${KUBE_CONTEXT:-docker-desktop}"
APP_NAMESPACE="${APP_NAMESPACE:-saxo}"
API_SERVICE="${API_SERVICE:-daytrader-frontend}"
API_LOCAL_PORT="${API_LOCAL_PORT:-18080}"
MCP_LOCAL_PORT="${MCP_LOCAL_PORT:-18610}"
HERMES_LOCAL_PORT="${HERMES_LOCAL_PORT:-18642}"
ROLLOUT_TIMEOUT="${ROLLOUT_TIMEOUT:-180s}"
EXPECTED_DAYTRADER_IMAGE="${EXPECTED_DAYTRADER_IMAGE:-}"
EXPECTED_API_IMAGE="${EXPECTED_API_IMAGE:-$EXPECTED_DAYTRADER_IMAGE}"
EXPECTED_SCHEDULER_IMAGE="${EXPECTED_SCHEDULER_IMAGE:-$EXPECTED_DAYTRADER_IMAGE}"
EXPECTED_MCP_IMAGE="${EXPECTED_MCP_IMAGE:-$EXPECTED_DAYTRADER_IMAGE}"
EXPECTED_HERMES_IMAGE="${EXPECTED_HERMES_IMAGE:-}"
EXPECTED_GIT_SHA="${EXPECTED_GIT_SHA:-}"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/daytrader-smoke.XXXXXX")"
PORT_FORWARD_PIDS=()
WARNINGS=0

cleanup() {
  local pid
  for pid in "${PORT_FORWARD_PIDS[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

info() {
  printf '[smoke] %s\n' "$*"
}

warn() {
  WARNINGS=$((WARNINGS + 1))
  printf '[smoke][warn] %s\n' "$*" >&2
}

fail() {
  printf '[smoke][fail] %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

start_port_forward() {
  local service="$1"
  local local_port="$2"
  local remote_port="$3"
  local log_name="$4"

  kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" port-forward \
    "svc/${service}" "${local_port}:${remote_port}" >"$TMP_DIR/${log_name}.log" 2>&1 &
  PORT_FORWARD_PIDS+=("$!")
}

check_expected_image() {
  local deployment="$1"
  local expected="$2"

  if [[ -z "$expected" ]]; then
    info "image check skipped for deployment/${deployment}; no expected image configured"
    return
  fi

  local actual
  actual="$(
    kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get "deployment/${deployment}" \
      -o 'jsonpath={.spec.template.spec.containers[0].image}' 2>/dev/null || true
  )"
  if [[ -z "$actual" ]]; then
    fail "could not read image for deployment/${deployment}"
  fi
  if [[ "$actual" != "$expected" ]]; then
    fail "deployment/${deployment} image mismatch: expected ${expected}, got ${actual}"
  fi
  info "deployment/${deployment} image matches ${expected}"
}

wait_for_url() {
  local url="$1"
  local out="$2"
  local log_name="$3"
  local attempts=30

  for _ in $(seq 1 "$attempts"); do
    if curl -fsS "$url" -o "$out" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done

  if [[ -s "$TMP_DIR/${log_name}.log" ]]; then
    tail -20 "$TMP_DIR/${log_name}.log" >&2 || true
  fi
  return 1
}

require_cmd kubectl
require_cmd curl
require_cmd base64
require_cmd git

info "checking Kubernetes rollouts in ${APP_NAMESPACE}"
for deployment in daytrader-api daytrader-scheduler daytrader-mcp hermes-agent; do
  kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" rollout status \
    "deployment/${deployment}" --timeout="$ROLLOUT_TIMEOUT"
done

check_expected_image daytrader-api "$EXPECTED_API_IMAGE"
check_expected_image daytrader-scheduler "$EXPECTED_SCHEDULER_IMAGE"
check_expected_image daytrader-mcp "$EXPECTED_MCP_IMAGE"
check_expected_image hermes-agent "$EXPECTED_HERMES_IMAGE"

endpoint_ready="$(
  kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get agentendpoint saxo-daytrader-internal \
    -o 'jsonpath={.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true
)"
if [[ "$endpoint_ready" != "True" ]]; then
  warn "internal ngrok AgentEndpoint is not reporting Ready=True"
else
  info "internal ngrok AgentEndpoint is ready"
fi

info "starting temporary port-forward ${API_SERVICE}:${API_LOCAL_PORT}->8000"
start_port_forward "$API_SERVICE" "$API_LOCAL_PORT" 8000 "api-port-forward"

wait_for_url "http://127.0.0.1:${API_LOCAL_PORT}/api/health" "$TMP_DIR/health.json" "api-port-forward" \
  || fail "local API health endpoint did not become reachable"

health="$(cat "$TMP_DIR/health.json")"
if [[ "$health" != *'"status":"ok"'* ]]; then
  fail "health endpoint returned unexpected payload: ${health}"
fi
info "health endpoint ok"

if [[ ! "$EXPECTED_GIT_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  fail "expected Git SHA is missing or invalid; refusing to verify an unprovenanced rollout"
fi
actual_git_sha="$(printf '%s' "$health" | tr -d '[:space:]' | sed -n 's/.*"git_sha":"\([^"]*\)".*/\1/p')"
if [[ ! "$actual_git_sha" =~ ^[0-9a-f]{40}$ ]]; then
  fail "health endpoint did not return a full Git SHA: ${actual_git_sha:-missing}"
fi
if ! git -C "$ROOT" cat-file -e "${EXPECTED_GIT_SHA}^{commit}" 2>/dev/null; then
  fail "expected Git SHA is not available in the local repository: ${EXPECTED_GIT_SHA}"
fi
if ! git -C "$ROOT" cat-file -e "${actual_git_sha}^{commit}" 2>/dev/null; then
  fail "running Git SHA is not available in the local repository: ${actual_git_sha}"
fi
# The running image must include the requested commit. Equality passes; a later
# descendant also passes, while an older stale build fails closed.
if ! git -C "$ROOT" merge-base --is-ancestor "$EXPECTED_GIT_SHA" "$actual_git_sha"; then
  fail "running Git SHA ${actual_git_sha} does not include requested commit ${EXPECTED_GIT_SHA}"
fi
info "running Git SHA ${actual_git_sha} includes requested commit ${EXPECTED_GIT_SHA}"

curl -fsS "http://127.0.0.1:${API_LOCAL_PORT}/api/overview" -o "$TMP_DIR/overview.json"
overview="$(cat "$TMP_DIR/overview.json")"

if [[ "$overview" != *'"last_cycle_status":"ok"'* ]]; then
  warn "scheduler last_cycle_status is not ok or was not present in overview"
else
  info "scheduler last_cycle_status ok"
fi

if [[ "$overview" == *'Re-authentication is required'* ]] \
  || [[ "$overview" == *'"needs_reauth":true'* ]] \
  || [[ "$overview" == *'"saxo_session":{"error"'* ]] \
  || [[ "$overview" == *'"saxo_session":{"status":"error"'* ]]; then
  warn "Saxo SIM session needs re-authentication; broker refresh and execution will be gated until login is refreshed"
else
  info "Saxo session did not report reauth-required in overview"
fi

if [[ "$overview" != *'"latest_decision"'* ]]; then
  warn "overview did not include latest_decision"
fi

curl -fsS "http://127.0.0.1:${API_LOCAL_PORT}/api/decision/schema" -o "$TMP_DIR/decision-schema.json"
decision_schema="$(cat "$TMP_DIR/decision-schema.json")"
if [[ "$decision_schema" != *'"status":"ok"'* ]] \
  || [[ "$decision_schema" != *'"schema_name":"daytrader_decision_report"'* ]] \
  || [[ "$decision_schema" != *'"strict":true'* ]]; then
  fail "decision-report schema health is not ok: ${decision_schema}"
fi
info "decision-report schema health ok"

info "checking MCP adapter health and tool discovery"
start_port_forward daytrader-mcp "$MCP_LOCAL_PORT" 8610 "mcp-port-forward"
wait_for_url "http://127.0.0.1:${MCP_LOCAL_PORT}/health" "$TMP_DIR/mcp-health.json" "mcp-port-forward" \
  || fail "MCP health endpoint did not become reachable"

mcp_health="$(cat "$TMP_DIR/mcp-health.json")"
if [[ "$mcp_health" != *'"status":"ok"'* ]]; then
  fail "MCP health endpoint returned unexpected payload: ${mcp_health}"
fi

mcp_key="$(
  kubectl --context "$KUBE_CONTEXT" -n "$APP_NAMESPACE" get secret hermes-env \
    -o 'go-template={{index .data "HERMES_DAYTRADER_API_KEY"}}' 2>/dev/null \
    | base64 --decode 2>/dev/null || true
)"
if [[ -z "$mcp_key" ]]; then
  warn "cannot read HERMES_DAYTRADER_API_KEY from hermes-env; skipping authenticated MCP tools/list smoke"
else
  curl -fsS \
    -H "Authorization: Bearer ${mcp_key}" \
    -H "content-type: application/json" \
    --data '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
    "http://127.0.0.1:${MCP_LOCAL_PORT}/mcp" \
    -o "$TMP_DIR/mcp-tools.json"
  mcp_tools="$(cat "$TMP_DIR/mcp-tools.json")"
  for tool_name in get_context get_decision_reports get_end_of_day_reports get_markov_signals get_quiver_signals create_decision_advice; do
    if [[ "$mcp_tools" != *"\"name\":\"${tool_name}\""* ]]; then
      fail "MCP tools/list did not include expected Hermes-safe tool: ${tool_name}"
    fi
  done
  info "MCP tools/list includes expected Hermes-safe tools"
fi

info "checking Hermes gateway health"
start_port_forward hermes-gateway "$HERMES_LOCAL_PORT" 8642 "hermes-port-forward"
wait_for_url "http://127.0.0.1:${HERMES_LOCAL_PORT}/health" "$TMP_DIR/hermes-health.json" "hermes-port-forward" \
  || fail "Hermes gateway health endpoint did not become reachable"
hermes_health="$(cat "$TMP_DIR/hermes-health.json")"
if [[ "$hermes_health" != *'"status": "ok"'* && "$hermes_health" != *'"status":"ok"'* ]]; then
  fail "Hermes gateway health endpoint returned unexpected payload: ${hermes_health}"
fi
info "Hermes gateway health ok"

info "post-deploy smoke completed with ${WARNINGS} warning(s)"
