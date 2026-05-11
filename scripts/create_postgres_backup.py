from __future__ import annotations

import argparse
import datetime as dt
import os
from pathlib import Path
from typing import Any

import requests


def _service_account_namespace() -> str:
    namespace_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
    if namespace_path.exists():
        return namespace_path.read_text(encoding="utf-8").strip()
    return "saxo"


def _k8s_headers() -> dict[str, str]:
    token_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/token")
    token = token_path.read_text(encoding="utf-8").strip()
    return {
        "Authorization": f"Bearer {token}",
        "Accept": "application/json",
        "Content-Type": "application/json",
    }


def _k8s_verify() -> str | bool:
    ca_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
    return str(ca_path) if ca_path.exists() else True


def _k8s_base_url() -> str:
    host = os.environ["KUBERNETES_SERVICE_HOST"]
    port = os.environ.get("KUBERNETES_SERVICE_PORT", "443")
    return f"https://{host}:{port}"


def create_backup(args: argparse.Namespace) -> dict[str, Any]:
    now = dt.datetime.now(dt.UTC)
    backup_name = f"{args.cluster}-scheduled-{now:%Y%m%d%H%M}"
    url = (
        f"{_k8s_base_url()}/apis/postgresql.cnpg.io/v1/namespaces/"
        f"{args.namespace}/backups"
    )
    payload = {
        "apiVersion": "postgresql.cnpg.io/v1",
        "kind": "Backup",
        "metadata": {
            "name": backup_name,
            "labels": {
                "cnpg.io/cluster": args.cluster,
                "daytrader.backup/schedule": "weekday-local",
            },
        },
        "spec": {
            "cluster": {"name": args.cluster},
            "method": "barmanObjectStore",
            "target": "prefer-standby",
        },
    }
    response = requests.post(
        url,
        headers=_k8s_headers(),
        json=payload,
        timeout=30,
        verify=_k8s_verify(),
    )
    if response.status_code == 409:
        return {
            "status": "exists",
            "backup": backup_name,
            "cluster": args.cluster,
            "namespace": args.namespace,
        }
    response.raise_for_status()
    body = response.json()
    return {
        "status": "created",
        "backup": body.get("metadata", {}).get("name", backup_name),
        "cluster": args.cluster,
        "namespace": args.namespace,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Create an on-demand CloudNativePG backup.")
    parser.add_argument("--namespace", default=os.getenv("NAMESPACE") or _service_account_namespace())
    parser.add_argument("--cluster", default=os.getenv("CLUSTER_NAME", "daytrader-postgres"))
    print(create_backup(parser.parse_args()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
