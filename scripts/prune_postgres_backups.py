from __future__ import annotations

import argparse
import datetime as dt
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import boto3
import requests
from botocore.config import Config


@dataclass(frozen=True)
class BackupRecord:
    name: str
    backup_id: str
    started_at: dt.datetime
    phase: str
    server_name: str


# Checkpoint ages: keep the most-recent backup that is at least this old.
_CHECKPOINT_AGES = [
    dt.timedelta(hours=1),
    dt.timedelta(hours=8),
    dt.timedelta(hours=24),
    dt.timedelta(hours=72),
    dt.timedelta(weeks=1),
]


def _utc_now() -> dt.datetime:
    return dt.datetime.now(dt.UTC)


def _parse_time(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(dt.UTC)


def _parse_backup_id(value: str) -> dt.datetime:
    return dt.datetime.strptime(value, "%Y%m%dT%H%M%S").replace(tzinfo=dt.UTC)


def _service_account_namespace() -> str:
    namespace_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
    if namespace_path.exists():
        return namespace_path.read_text(encoding="utf-8").strip()
    return "saxo"


def _k8s_headers() -> dict[str, str]:
    token_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/token")
    token = token_path.read_text(encoding="utf-8").strip()
    return {"Authorization": f"Bearer {token}", "Accept": "application/json"}


def _k8s_verify() -> str | bool:
    ca_path = Path("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
    return str(ca_path) if ca_path.exists() else True


def _k8s_base_url() -> str:
    host = os.environ["KUBERNETES_SERVICE_HOST"]
    port = os.environ.get("KUBERNETES_SERVICE_PORT", "443")
    return f"https://{host}:{port}"


def _list_backup_records(namespace: str, cluster_name: str) -> list[BackupRecord]:
    url = (
        f"{_k8s_base_url()}/apis/postgresql.cnpg.io/v1/namespaces/"
        f"{namespace}/backups"
    )
    response = requests.get(
        url,
        headers=_k8s_headers(),
        params={"labelSelector": f"cnpg.io/cluster={cluster_name}"},
        timeout=30,
        verify=_k8s_verify(),
    )
    response.raise_for_status()
    records: list[BackupRecord] = []
    for item in response.json().get("items", []):
        status = item.get("status") or {}
        backup_id = status.get("backupId")
        started_at = status.get("startedAt") or item.get("metadata", {}).get("creationTimestamp")
        if not backup_id or not started_at:
            continue
        records.append(
            BackupRecord(
                name=str(item["metadata"]["name"]),
                backup_id=str(backup_id),
                started_at=_parse_time(str(started_at)),
                phase=str(status.get("phase") or "unknown"),
                server_name=str(status.get("serverName") or cluster_name),
            )
        )
    return records


def _delete_backup_record(namespace: str, name: str) -> None:
    url = f"{_k8s_base_url()}/apis/postgresql.cnpg.io/v1/namespaces/{namespace}/backups/{name}"
    response = requests.delete(url, headers=_k8s_headers(), timeout=30, verify=_k8s_verify())
    if response.status_code not in {200, 202, 404}:
        response.raise_for_status()


def _age_label(age: dt.timedelta) -> str:
    h = int(age.total_seconds() // 3600)
    return "1w" if h >= 168 else f"{h}h"


def _checkpoint_selection(completed: list[BackupRecord]) -> dict[str, str]:
    """
    Keep:
      - 2 most-recent backups  (provides ~30-min granularity)
      - for each checkpoint age: the most-recent backup that is at least that old
    Returns {backup_id: reason_label}.
    """
    now = _utc_now()
    kept: dict[str, str] = {}
    by_time = sorted(completed, key=lambda r: r.started_at, reverse=True)

    for record in by_time[:2]:
        kept[record.backup_id] = "recent"

    for age in _CHECKPOINT_AGES:
        cutoff = now - age
        label = f"checkpoint-{_age_label(age)}"
        for record in by_time:
            if record.started_at <= cutoff:
                if record.backup_id not in kept:
                    kept[record.backup_id] = label
                break

    return kept


def _s3_client(endpoint_url: str):
    return boto3.client(
        "s3",
        endpoint_url=endpoint_url,
        aws_access_key_id=os.environ["AWS_ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["AWS_SECRET_ACCESS_KEY"],
        region_name=os.environ.get("AWS_DEFAULT_REGION", "us-east-1"),
        config=Config(s3={"addressing_style": "path"}),
    )


def _list_base_backup_prefixes(s3: Any, bucket: str, server_name: str) -> set[str]:
    root_prefix = f"{server_name}/base/"
    prefixes: set[str] = set()
    paginator = s3.get_paginator("list_objects_v2")
    for page in paginator.paginate(Bucket=bucket, Prefix=root_prefix, Delimiter="/"):
        for item in page.get("CommonPrefixes", []):
            prefix = str(item["Prefix"])
            backup_id = prefix.removeprefix(root_prefix).strip("/")
            try:
                _parse_backup_id(backup_id)
            except ValueError:
                continue
            prefixes.add(backup_id)
    return prefixes


def _delete_prefix(s3: Any, bucket: str, prefix: str) -> int:
    deleted = 0
    paginator = s3.get_paginator("list_objects_v2")
    for page in paginator.paginate(Bucket=bucket, Prefix=prefix):
        objects = [{"Key": item["Key"]} for item in page.get("Contents", [])]
        if not objects:
            continue
        s3.delete_objects(Bucket=bucket, Delete={"Objects": objects, "Quiet": True})
        deleted += len(objects)
    return deleted


def prune(args: argparse.Namespace) -> dict[str, Any]:
    records = _list_backup_records(args.namespace, args.cluster)
    completed = [r for r in records if r.phase == "completed"]

    s3 = _s3_client(args.endpoint_url)
    server_names = {r.server_name for r in completed} or {args.cluster}
    base_prefixes_by_server = {
        sn: _list_base_backup_prefixes(s3, args.bucket, sn)
        for sn in server_names
    }

    missing_object_records = [
        r for r in completed
        if r.backup_id not in base_prefixes_by_server.get(r.server_name, set())
    ]
    valid_completed = [
        r for r in completed
        if r.backup_id in base_prefixes_by_server.get(r.server_name, set())
    ]

    kept = _checkpoint_selection(valid_completed)
    kept_ids = set(kept)
    pruned_records = [r for r in valid_completed if r.backup_id not in kept_ids]

    deleted_backup_resources: list[str] = []
    if not args.dry_run:
        for r in [*missing_object_records, *pruned_records]:
            _delete_backup_record(args.namespace, r.name)
            deleted_backup_resources.append(r.name)

    valid_ids_by_server: dict[str, set[str]] = {}
    for r in valid_completed:
        valid_ids_by_server.setdefault(r.server_name, set()).add(r.backup_id)

    deleted_object_prefixes: dict[str, int] = {}
    for sn in server_names:
        base_prefixes = base_prefixes_by_server.get(sn, set())
        valid_ids = valid_ids_by_server.get(sn, set())
        orphan_ids = base_prefixes - valid_ids
        stale_ids = (base_prefixes & valid_ids) - kept_ids
        for backup_id in sorted(orphan_ids | stale_ids):
            prefix = f"{sn}/base/{backup_id}/"
            deleted_object_prefixes[prefix] = 0 if args.dry_run else _delete_prefix(s3, args.bucket, prefix)

    return {
        "status": "ok",
        "dry_run": args.dry_run,
        "cluster": args.cluster,
        "namespace": args.namespace,
        "completed_backups": len(completed),
        "valid_backups": len(valid_completed),
        "missing_object_backup_resources": [r.name for r in missing_object_records],
        "kept_backups": kept,
        "pruned_backup_resources": [r.name for r in pruned_records],
        "deleted_backup_resources": deleted_backup_resources,
        "deleted_object_prefixes": deleted_object_prefixes,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Checkpoint-based retention for CNPG base backups in S3-compatible storage.")
    parser.add_argument("--namespace", default=os.getenv("NAMESPACE") or _service_account_namespace())
    parser.add_argument("--cluster", default=os.getenv("CLUSTER_NAME", "daytrader-postgres"))
    parser.add_argument("--bucket", default=os.getenv("BACKUP_BUCKET", "daytrader-cnpg"))
    parser.add_argument("--endpoint-url", default=os.getenv("S3_ENDPOINT_URL", "http://host.docker.internal:9000"))
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    print(prune(args))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
