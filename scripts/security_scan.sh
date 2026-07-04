#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-home}"
TRIVY_IMAGE="${TRIVY_IMAGE:-aquasec/trivy:latest}"
RUSTSEC_IMAGE="${RUSTSEC_IMAGE:-ghcr.io/rustsec/audit-check:latest}"
API_IMAGE="${API_IMAGE:-daytrader-api:local}"
BACKUP_IMAGE="${BACKUP_IMAGE:-daytrader-backup:local}"
SEVERITY="${SEVERITY:-HIGH,CRITICAL}"

section() {
  printf '\n## %s\n\n' "$1"
}

have() {
  command -v "$1" >/dev/null 2>&1
}

run_cargo_audit() {
  section "RustSec cargo audit"
  if have cargo-audit; then
    CARGO_HOME="$CARGO_HOME" cargo audit --deny warnings
    return
  fi

  if have docker; then
    docker run --rm \
      -v "$ROOT:/workspace:ro" \
      -w /workspace \
      "$RUSTSEC_IMAGE" --deny warnings
    return
  fi

  printf 'warning: neither cargo-audit nor docker is available; skipped RustSec audit\n' >&2
}

run_trivy_fs() {
  section "Trivy filesystem scan"
  if have trivy; then
    trivy fs --severity "$SEVERITY" --ignore-unfixed --exit-code 1 .
    return
  fi

  if have docker; then
    docker run --rm \
      -v "$ROOT:/workspace:ro" \
      -v /var/run/docker.sock:/var/run/docker.sock \
      "$TRIVY_IMAGE" fs --severity "$SEVERITY" --ignore-unfixed --exit-code 1 /workspace
    return
  fi

  printf 'warning: neither trivy nor docker is available; skipped filesystem scan\n' >&2
}

run_trivy_image() {
  local image="$1"

  section "Trivy image scan: ${image}"
  if have trivy; then
    trivy image --severity "$SEVERITY" --ignore-unfixed --exit-code 1 "$image"
    return
  fi

  if have docker; then
    docker run --rm \
      -v /var/run/docker.sock:/var/run/docker.sock \
      "$TRIVY_IMAGE" image --severity "$SEVERITY" --ignore-unfixed --exit-code 1 "$image"
    return
  fi

  printf 'warning: neither trivy nor docker is available; skipped image scan for %s\n' "$image" >&2
}

run_secret_scan() {
  section "Trivy secret scan"
  if have trivy; then
    trivy fs --scanners secret --exit-code 1 .
    return
  fi

  if have docker; then
    docker run --rm \
      -v "$ROOT:/workspace:ro" \
      "$TRIVY_IMAGE" fs --scanners secret --exit-code 1 /workspace
    return
  fi

  printf 'warning: neither trivy nor docker is available; skipped secret scan\n' >&2
}

run_cargo_audit
run_trivy_fs
run_trivy_image "$API_IMAGE"
run_trivy_image "$BACKUP_IMAGE"
run_secret_scan
