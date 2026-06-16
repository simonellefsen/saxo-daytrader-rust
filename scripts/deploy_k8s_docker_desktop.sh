#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTEXT="${KUBE_CONTEXT:-docker-desktop}"
NAMESPACE="saxo"
DB_NAMESPACE="${DB_NAMESPACE:-saxo}"
ENV_FILE="${ENV_FILE:-$ROOT/.env}"
IMAGE_TAG="${IMAGE_TAG:-$(date +%Y%m%d%H%M%S)}"
API_IMAGE="${API_IMAGE:-daytrader-api:$IMAGE_TAG}"
BACKUP_IMAGE="${BACKUP_IMAGE:-daytrader-backup:$IMAGE_TAG}"
SANITIZED_ENV_FILE=""
HERMES_ENV_FILE=""

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf "Missing required command: %s\n" "$1" >&2
    exit 1
  fi
}

require_env() {
  if [ -z "${!1:-}" ]; then
    printf "Missing required environment value: %s\n" "$1" >&2
    exit 1
  fi
}

require_cmd docker
require_cmd kubectl
require_cmd helm
require_cmd python3

if [ ! -f "$ENV_FILE" ]; then
  printf "Missing env file: %s\n" "$ENV_FILE" >&2
  exit 1
fi

cleanup() {
  if [ -n "$SANITIZED_ENV_FILE" ] && [ -f "$SANITIZED_ENV_FILE" ]; then
    rm -f "$SANITIZED_ENV_FILE"
  fi
  if [ -n "$HERMES_ENV_FILE" ] && [ -f "$HERMES_ENV_FILE" ]; then
    rm -f "$HERMES_ENV_FILE"
  fi
}
trap cleanup EXIT

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

SANITIZED_ENV_FILE="$(mktemp)"
HERMES_ENV_FILE="$(mktemp)"
python3 - "$ENV_FILE" "$SANITIZED_ENV_FILE" <<'PY'
import os
import re
import sys

source_path, target_path = sys.argv[1], sys.argv[2]
key_pattern = re.compile(r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=")
keys: list[str] = []
blocked_keys = {"XAI_API_KEY"}
with open(source_path, encoding="utf-8") as handle:
    for line in handle:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        match = key_pattern.match(line)
        if match:
            key = match.group(1)
            if key not in blocked_keys and key not in keys:
                keys.append(key)

with open(target_path, "w", encoding="utf-8") as handle:
    for key in keys:
        if key in os.environ:
            value = os.environ[key].replace("\n", "\\n")
            handle.write(f"{key}={value}\n")
PY
python3 - "$HERMES_ENV_FILE" <<'PY'
import os
import sys

target_path = sys.argv[1]

mappings = {
    "HERMES_API_SERVER_ENABLED": "API_SERVER_ENABLED",
    "HERMES_API_SERVER_HOST": "API_SERVER_HOST",
    "HERMES_API_SERVER_KEY": "API_SERVER_KEY",
    "HERMES_API_SERVER_CORS_ORIGINS": "API_SERVER_CORS_ORIGINS",
    "HERMES_INFERENCE_PROVIDER": "HERMES_INFERENCE_PROVIDER",
    "HERMES_MODEL": "HERMES_MODEL",
    "HERMES_DASHBOARD": "HERMES_DASHBOARD",
    "HERMES_DASHBOARD_TUI": "HERMES_DASHBOARD_TUI",
    "HERMES_DASHBOARD_HOST": "HERMES_DASHBOARD_HOST",
    "HERMES_DASHBOARD_PORT": "HERMES_DASHBOARD_PORT",
    "OPENAI_API_KEY": "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY": "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY": "OPENROUTER_API_KEY",
    "TELEGRAM_BOT_TOKEN": "TELEGRAM_BOT_TOKEN",
    "DISCORD_BOT_TOKEN": "DISCORD_BOT_TOKEN",
    "SLACK_BOT_TOKEN": "SLACK_BOT_TOKEN",
    "HERMES_DAYTRADER_API_KEY": "HERMES_DAYTRADER_API_KEY",
    "HERMES_DAYTRADER_API_BASE_URL": "DAYTRADER_API_BASE_URL",
    "HERMES_DAYTRADER_MCP_URL": "DAYTRADER_MCP_URL",
}

with open(target_path, "w", encoding="utf-8") as handle:
    for source, target in mappings.items():
        value = os.environ.get(source)
        if value:
            sanitized = value.replace("\n", "\\n")
            handle.write(f"{target}={sanitized}\n")
PY

if [ -z "${DAYTRADER_PUBLIC_BASE_URL:-}" ]; then
  require_env NGROK_DOMAIN
  DAYTRADER_PUBLIC_BASE_URL="https://$NGROK_DOMAIN/saxo-daytrader"
fi
BACKUP_OBJECT_STORE="${BACKUP_OBJECT_STORE:-rustfs}"
BACKUP_BUCKET="${BACKUP_BUCKET:-daytrader-cnpg}"
case "$BACKUP_OBJECT_STORE" in
  rustfs)
    RUSTFS_ENDPOINT_URL="${RUSTFS_ENDPOINT_URL:-http://host.docker.internal:9000}"
    RUSTFS_ACCESS_KEY="${RUSTFS_ACCESS_KEY:-rustfsadmin}"
    RUSTFS_SECRET_KEY="${RUSTFS_SECRET_KEY:-rustfsadmin}"
    BACKUP_S3_ENDPOINT_URL="${BACKUP_S3_ENDPOINT_URL:-$RUSTFS_ENDPOINT_URL}"
    BACKUP_S3_ACCESS_KEY_ID="${BACKUP_S3_ACCESS_KEY_ID:-$RUSTFS_ACCESS_KEY}"
    BACKUP_S3_SECRET_ACCESS_KEY="${BACKUP_S3_SECRET_ACCESS_KEY:-$RUSTFS_SECRET_KEY}"
    ;;
  *)
    printf "Unsupported BACKUP_OBJECT_STORE: %s\n" "$BACKUP_OBJECT_STORE" >&2
    printf "Expected: rustfs\n" >&2
    exit 1
    ;;
esac
POSTGRES_APP_USER="${POSTGRES_APP_USER:-daytrader}"
POSTGRES_APP_PASSWORD="${POSTGRES_APP_PASSWORD:-daytrader-postgres-password}"
export BACKUP_BUCKET BACKUP_S3_ENDPOINT_URL BACKUP_S3_ACCESS_KEY_ID BACKUP_S3_SECRET_ACCESS_KEY POSTGRES_APP_USER POSTGRES_APP_PASSWORD API_IMAGE BACKUP_IMAGE DB_NAMESPACE
DATABASE_URL="$(python3 - <<'PY'
import os
from urllib.parse import quote

user = quote(os.environ["POSTGRES_APP_USER"], safe="")
password = quote(os.environ["POSTGRES_APP_PASSWORD"], safe="")
db_namespace = os.environ.get("DB_NAMESPACE", "saxo")
print(f"postgresql://{user}:{password}@daytrader-postgres-rw.{db_namespace}.svc.cluster.local:5432/daytrader")
PY
)"
export DATABASE_URL
export DAYTRADER_PUBLIC_BASE_URL

python3 - "$SANITIZED_ENV_FILE" <<'PY'
import os
import sys

path = sys.argv[1]
key = "DAYTRADER_PUBLIC_BASE_URL"
line = f"{key}={os.environ[key]}\n"
lines: list[str] = []
replaced = False
with open(path, encoding="utf-8") as handle:
    for existing in handle:
        if existing.startswith(f"{key}="):
            lines.append(line)
            replaced = True
        else:
            lines.append(existing)
if not replaced:
    lines.append(line)
with open(path, "w", encoding="utf-8") as handle:
    handle.writelines(lines)
PY

ensure_backup_bucket() {
  printf "Ensuring backup bucket %s at %s...\n" "$BACKUP_BUCKET" "$BACKUP_S3_ENDPOINT_URL"
  docker run --rm --entrypoint /bin/sh quay.io/minio/mc:latest \
    -ec "
      until mc alias set backup '$BACKUP_S3_ENDPOINT_URL' '$BACKUP_S3_ACCESS_KEY_ID' '$BACKUP_S3_SECRET_ACCESS_KEY'; do
        sleep 1
      done
      mc mb --ignore-existing 'backup/$BACKUP_BUCKET'
    " >/dev/null
}

if [ "$(kubectl config current-context)" != "$CONTEXT" ]; then
  printf "Switch kubectl to context %s before deploying.\n" "$CONTEXT" >&2
  printf "Current context: %s\n" "$(kubectl config current-context)" >&2
  exit 1
fi

printf "Building local Docker images...\n"
docker build -f "$ROOT/Dockerfile.api" -t "$API_IMAGE" -t daytrader-api:local "$ROOT"
docker build -f "$ROOT/Dockerfile.backup" -t "$BACKUP_IMAGE" -t daytrader-backup:local "$ROOT"

printf "Installing/upgrading CloudNativePG operator...\n"
helm repo add cnpg https://cloudnative-pg.github.io/charts >/dev/null 2>&1 || true
helm repo update cnpg >/dev/null
helm upgrade --install cnpg cnpg/cloudnative-pg \
  --namespace cnpg-system \
  --create-namespace
kubectl --context "$CONTEXT" -n cnpg-system wait \
  --for=condition=Available deployment \
  -l app.kubernetes.io/name=cloudnative-pg \
  --timeout=180s

printf "Using external RustFS backup target at %s.\n" "$BACKUP_S3_ENDPOINT_URL"
ensure_backup_bucket

printf "Applying app namespace and CloudNativePG resources in %s...\n" "$DB_NAMESPACE"
kubectl --context "$CONTEXT" apply -f "$ROOT/deploy/k8s/base/namespace.yaml"
kubectl --context "$CONTEXT" create namespace "$DB_NAMESPACE" --dry-run=client -o yaml \
  | kubectl --context "$CONTEXT" apply -f -
python3 - "$ROOT/deploy/k8s/postgres/postgres-stack.template.yaml" <<'PY' | kubectl --context "$CONTEXT" apply -f -
import os
import sys

template_path = sys.argv[1]
replacements = {
    "__BACKUP_BUCKET__": os.environ["BACKUP_BUCKET"],
    "__BACKUP_S3_ACCESS_KEY_ID__": os.environ["BACKUP_S3_ACCESS_KEY_ID"],
    "__BACKUP_S3_SECRET_ACCESS_KEY__": os.environ["BACKUP_S3_SECRET_ACCESS_KEY"],
    "__BACKUP_S3_ENDPOINT_URL__": os.environ["BACKUP_S3_ENDPOINT_URL"],
    "__POSTGRES_APP_USER__": os.environ["POSTGRES_APP_USER"],
    "__POSTGRES_APP_PASSWORD__": os.environ["POSTGRES_APP_PASSWORD"],
    "__DATABASE_URL__": os.environ["DATABASE_URL"],
}
rendered = open(template_path, encoding="utf-8").read()
for key, value in replacements.items():
    rendered = rendered.replace(key, value)
sys.stdout.write(rendered)
PY
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" delete scheduledbackup daytrader-postgres-hourly --ignore-not-found
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" delete job daytrader-minio-bucket --ignore-not-found
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" delete deployment daytrader-minio --ignore-not-found
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" delete service daytrader-minio --ignore-not-found
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" delete pvc daytrader-minio-data --ignore-not-found
kubectl --context "$CONTEXT" delete pv daytrader-minio-pv --ignore-not-found
kubectl --context "$CONTEXT" -n "$DB_NAMESPACE" wait --for=condition=Ready cluster/daytrader-postgres --timeout=420s

printf "Applying app Kubernetes resources...\n"
kubectl --context "$CONTEXT" -n "$NAMESPACE" create secret generic daytrader-env \
  --from-env-file="$SANITIZED_ENV_FILE" \
  --dry-run=client \
  -o yaml | kubectl --context "$CONTEXT" apply -f -
if [ -s "$HERMES_ENV_FILE" ]; then
  kubectl --context "$CONTEXT" -n "$NAMESPACE" create secret generic hermes-env \
    --from-env-file="$HERMES_ENV_FILE" \
    --dry-run=client \
    -o yaml | kubectl --context "$CONTEXT" apply -f -
else
  printf "No Hermes-specific env values found; hermes-env secret was not changed.\n"
fi
kubectl --context "$CONTEXT" apply -k "$ROOT/deploy/k8s/base"
kubectl --context "$CONTEXT" -n "$NAMESPACE" set image deployment/daytrader-api "api=$API_IMAGE"
kubectl --context "$CONTEXT" -n "$NAMESPACE" set image deployment/daytrader-scheduler "scheduler=$API_IMAGE"
kubectl --context "$CONTEXT" -n "$NAMESPACE" set image deployment/daytrader-mcp "mcp=$API_IMAGE"
kubectl --context "$CONTEXT" -n "$NAMESPACE" set image cronjob/daytrader-postgres-backup-schedule "backup=$BACKUP_IMAGE"
kubectl --context "$CONTEXT" -n "$NAMESPACE" set image cronjob/daytrader-postgres-backup-retention "retention=$BACKUP_IMAGE"
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete deployment daytrader-frontend --ignore-not-found
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete job daytrader-sqlite-to-postgres-migration --ignore-not-found

kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/daytrader-api
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/daytrader-scheduler
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/daytrader-mcp
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/daytrader-mcp --timeout=180s
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/hermes-agent

printf "Shared ngrok public gateway is owned by ../shared-ngrok-gateway.\n"
printf "This deploy only applies the app-owned saxo-daytrader.internal AgentEndpoint via kustomize.\n"

printf "Waiting for deployments...\n"
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/daytrader-api --timeout=180s
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/daytrader-scheduler --timeout=180s
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/hermes-agent --timeout=180s

printf "Deployment complete.\n"
printf "Rust app: %s\n" "$DAYTRADER_PUBLIC_BASE_URL"
