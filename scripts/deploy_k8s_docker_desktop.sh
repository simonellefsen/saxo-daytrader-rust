#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTEXT="${KUBE_CONTEXT:-docker-desktop}"
NAMESPACE="saxo-rust"
DB_NAMESPACE="${DB_NAMESPACE:-saxo}"
ENV_FILE="${ENV_FILE:-$ROOT/.env}"
IMAGE_TAG="${IMAGE_TAG:-$(date +%Y%m%d%H%M%S)}"
API_IMAGE="${API_IMAGE:-daytrader-api:$IMAGE_TAG}"
MINIO_CONTAINER_NAME="${MINIO_CONTAINER_NAME:-daytrader-minio}"
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
with open(source_path, encoding="utf-8") as handle:
    for line in handle:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        match = key_pattern.match(line)
        if match and match.group(1) not in keys:
            keys.append(match.group(1))

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
    "HERMES_DASHBOARD": "HERMES_DASHBOARD",
    "HERMES_DASHBOARD_TUI": "HERMES_DASHBOARD_TUI",
    "HERMES_DASHBOARD_HOST": "HERMES_DASHBOARD_HOST",
    "HERMES_DASHBOARD_PORT": "HERMES_DASHBOARD_PORT",
    "OPENAI_API_KEY": "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY": "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY": "OPENROUTER_API_KEY",
    "XAI_API_KEY": "XAI_API_KEY",
    "TELEGRAM_BOT_TOKEN": "TELEGRAM_BOT_TOKEN",
    "DISCORD_BOT_TOKEN": "DISCORD_BOT_TOKEN",
    "SLACK_BOT_TOKEN": "SLACK_BOT_TOKEN",
}

with open(target_path, "w", encoding="utf-8") as handle:
    for source, target in mappings.items():
        value = os.environ.get(source)
        if value:
            sanitized = value.replace("\n", "\\n")
            handle.write(f"{target}={sanitized}\n")
PY

require_env NGROK_API_KEY
require_env NGROK_AUTHTOKEN
require_env NGROK_DOMAIN
require_env NGROK_ALLOWED_EMAILS
NGROK_OAUTH_PROVIDER="${NGROK_OAUTH_PROVIDER:-google}"
MINIO_HOST_PATH="${MINIO_HOST_PATH:-$ROOT/minio-data}"
MINIO_ROOT_USER="${MINIO_ROOT_USER:-daytrader}"
MINIO_ROOT_PASSWORD="${MINIO_ROOT_PASSWORD:-daytrader-minio-password}"
MINIO_API_PORT="${MINIO_API_PORT:-9000}"
MINIO_CONSOLE_PORT="${MINIO_CONSOLE_PORT:-9001}"
MINIO_ENDPOINT_URL="${MINIO_ENDPOINT_URL:-http://host.docker.internal:$MINIO_API_PORT}"
BACKUP_OBJECT_STORE="${BACKUP_OBJECT_STORE:-minio}"
BACKUP_BUCKET="${BACKUP_BUCKET:-daytrader-cnpg}"
case "$BACKUP_OBJECT_STORE" in
  minio)
    BACKUP_S3_ENDPOINT_URL="${BACKUP_S3_ENDPOINT_URL:-$MINIO_ENDPOINT_URL}"
    BACKUP_S3_ACCESS_KEY_ID="${BACKUP_S3_ACCESS_KEY_ID:-$MINIO_ROOT_USER}"
    BACKUP_S3_SECRET_ACCESS_KEY="${BACKUP_S3_SECRET_ACCESS_KEY:-$MINIO_ROOT_PASSWORD}"
    MINIO_ENDPOINT_URL="$BACKUP_S3_ENDPOINT_URL"
    MINIO_ROOT_USER="$BACKUP_S3_ACCESS_KEY_ID"
    MINIO_ROOT_PASSWORD="$BACKUP_S3_SECRET_ACCESS_KEY"
    ;;
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
    printf "Expected one of: minio, rustfs\n" >&2
    exit 1
    ;;
esac
POSTGRES_APP_USER="${POSTGRES_APP_USER:-daytrader}"
POSTGRES_APP_PASSWORD="${POSTGRES_APP_PASSWORD:-daytrader-postgres-password}"
export MINIO_HOST_PATH MINIO_ROOT_USER MINIO_ROOT_PASSWORD MINIO_ENDPOINT_URL BACKUP_BUCKET BACKUP_S3_ENDPOINT_URL BACKUP_S3_ACCESS_KEY_ID BACKUP_S3_SECRET_ACCESS_KEY POSTGRES_APP_USER POSTGRES_APP_PASSWORD API_IMAGE DB_NAMESPACE
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

start_minio_container() {
  printf "Starting Docker MinIO backup target at %s...\n" "$MINIO_HOST_PATH"
  mkdir -p "$MINIO_HOST_PATH"

  if docker ps -a --format '{{.Names}}' | grep -qx "$MINIO_CONTAINER_NAME"; then
    docker rm -f "$MINIO_CONTAINER_NAME" >/dev/null
  fi

  docker run -d \
    --name "$MINIO_CONTAINER_NAME" \
    -p "$MINIO_API_PORT:9000" \
    -p "$MINIO_CONSOLE_PORT:9001" \
    -e "MINIO_ROOT_USER=$MINIO_ROOT_USER" \
    -e "MINIO_ROOT_PASSWORD=$MINIO_ROOT_PASSWORD" \
    -v "$MINIO_HOST_PATH:/data" \
    quay.io/minio/minio:latest \
    server /data --console-address :9001 >/dev/null
}

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

printf "Installing/upgrading ngrok Kubernetes operator...\n"
helm repo add ngrok https://charts.ngrok.com >/dev/null 2>&1 || true
helm repo update ngrok >/dev/null
helm upgrade --install ngrok-operator ngrok/ngrok-operator \
  --namespace ngrok-operator \
  --create-namespace \
  --set "credentials.apiKey=$NGROK_API_KEY" \
  --set "credentials.authtoken=$NGROK_AUTHTOKEN"

if [ "$BACKUP_OBJECT_STORE" = "minio" ]; then
  start_minio_container
else
  printf "Using external %s backup target at %s.\n" "$BACKUP_OBJECT_STORE" "$BACKUP_S3_ENDPOINT_URL"
fi
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
kubectl --context "$CONTEXT" -n "$NAMESPACE" create secret generic daytrader-postgres-app \
  --from-literal=database-url="$DATABASE_URL" \
  --dry-run=client \
  -o yaml | kubectl --context "$CONTEXT" apply -f -
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
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete deployment daytrader-frontend --ignore-not-found
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete cronjob daytrader-postgres-backup-schedule daytrader-postgres-backup-retention --ignore-not-found
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete job daytrader-sqlite-to-postgres-migration --ignore-not-found

kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/daytrader-api
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/daytrader-scheduler
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout restart deployment/hermes-agent

printf "Applying ngrok OAuth endpoint for %s...\n" "$NGROK_DOMAIN"
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete ingress daytrader-frontend --ignore-not-found
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete agentendpoint daytrader-rust daytrader-frontend --ignore-not-found --wait=false
kubectl --context "$CONTEXT" -n "$NAMESPACE" delete agentendpoint \
  -l k8s.ngrok.com/controller-name=ngrok-operator-manager \
  --ignore-not-found
python3 - "$ROOT/deploy/k8s/ngrok/ingress.template.yaml" <<'PY' | kubectl --context "$CONTEXT" apply -f -
import json
import os
import sys

template_path = sys.argv[1]
allowed_emails = [
    email.strip()
    for email in os.environ["NGROK_ALLOWED_EMAILS"].split(",")
    if email.strip()
]
if not allowed_emails:
    raise SystemExit("NGROK_ALLOWED_EMAILS must include at least one email address.")

deny_expr = f"!(actions.ngrok.oauth.identity.email in {json.dumps(allowed_emails)})"
with open(template_path, encoding="utf-8") as handle:
    rendered = (
        handle.read()
        .replace("__NGROK_DOMAIN__", os.environ["NGROK_DOMAIN"])
        .replace("__NGROK_OAUTH_PROVIDER__", os.environ.get("NGROK_OAUTH_PROVIDER", "google"))
        .replace("__NGROK_DENY_EXPR__", deny_expr)
    )
sys.stdout.write(rendered)
PY

printf "Waiting for deployments...\n"
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/daytrader-api --timeout=180s
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/daytrader-scheduler --timeout=180s
kubectl --context "$CONTEXT" -n "$NAMESPACE" rollout status deployment/hermes-agent --timeout=180s

printf "Deployment complete.\n"
printf "Rust app: https://%s\n" "$NGROK_DOMAIN"
