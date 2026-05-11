from __future__ import annotations

import argparse
import datetime as dt
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

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


def _tier_selection(
    completed: list[BackupRecord],
    *,
    hourly: int,
    daily: int,
    weekly: int,
    monthly: int,
    yearly: int,
) -> dict[str, str]:
    kept: dict[str, str] = {}
    remaining = sorted(completed, key=lambda record: record.started_at, reverse=True)

    for record in remaining[:hourly]:
        kept[record.backup_id] = "hourly"
    remaining = [record for record in remaining if record.backup_id not in kept]

    daily_dates: set[dt.date] = set()
    next_remaining: list[BackupRecord] = []
    for record in remaining:
        key = record.started_at.date()
        if len(daily_dates) < daily and key not in daily_dates:
            daily_dates.add(key)
            kept[record.backup_id] = "daily"
        else:
            next_remaining.append(record)
    remaining = next_remaining

    weekly_keys: set[tuple[int, int]] = set()
    next_remaining = []
    for record in remaining:
        iso = record.started_at.isocalendar()
        key = (iso.year, iso.week)
        if len(weekly_keys) < weekly and key not in weekly_keys:
            weekly_keys.add(key)
            kept[record.backup_id] = "weekly"
        else:
            next_remaining.append(record)
    remaining = next_remaining

    monthly_keys: set[tuple[int, int]] = set()
    next_remaining = []
    for record in remaining:
        key = (record.started_at.year, record.started_at.month)
        if len(monthly_keys) < monthly and key not in monthly_keys:
            monthly_keys.add(key)
            kept[record.backup_id] = "monthly"
        else:
            next_remaining.append(record)
    remaining = next_remaining

    yearly_keys: set[int] = set()
    for record in remaining:
        key = record.started_at.year
        if len(yearly_keys) < yearly and key not in yearly_keys:
            yearly_keys.add(key)
            kept[record.backup_id] = "yearly"

    return kept


def _is_weekend_backup(record: BackupRecord, timezone_name: str) -> bool:
    local_started_at = record.started_at.astimezone(ZoneInfo(timezone_name))
    return local_started_at.weekday() >= 5


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
    completed = [record for record in records if record.phase == "completed"]
    weekend_records: list[BackupRecord] = []
    if args.purge_weekends:
        weekday_records = [
            record
            for record in completed
            if not _is_weekend_backup(record, args.local_timezone)
        ]
        # Keep weekend backups as a temporary safety net until at least one
        # weekday backup exists under the new local-time schedule.
        if weekday_records:
            weekend_records = [
                record
                for record in completed
                if _is_weekend_backup(record, args.local_timezone)
            ]
            completed = weekday_records
    s3 = _s3_client(args.endpoint_url)
    server_names = {record.server_name for record in completed} or {args.cluster}
    base_prefixes_by_server = {
        server_name: _list_base_backup_prefixes(s3, args.bucket, server_name)
        for server_name in server_names
    }
    missing_object_records = [
        record
        for record in completed
        if record.backup_id not in base_prefixes_by_server.get(record.server_name, set())
    ]
    valid_completed = [
        record
        for record in completed
        if record.backup_id in base_prefixes_by_server.get(record.server_name, set())
    ]
    kept = _tier_selection(
        valid_completed,
        hourly=args.hourly,
        daily=args.daily,
        weekly=args.weekly,
        monthly=args.monthly,
        yearly=args.yearly,
    )
    valid_ids_by_server: dict[str, set[str]] = {}
    for record in valid_completed:
        valid_ids_by_server.setdefault(record.server_name, set()).add(record.backup_id)
    kept_ids = set(kept)
    pruned_records = [record for record in valid_completed if record.backup_id not in kept_ids]

    deleted_backup_resources: list[str] = []
    if not args.dry_run:
        for record in [*weekend_records, *missing_object_records, *pruned_records]:
            _delete_backup_record(args.namespace, record.name)
            deleted_backup_resources.append(record.name)

    deleted_object_prefixes: dict[str, int] = {}
    for server_name in server_names:
        base_prefixes = base_prefixes_by_server.get(server_name, set())
        valid_ids = valid_ids_by_server.get(server_name, set())
        orphan_ids = base_prefixes - valid_ids
        stale_ids = (base_prefixes & valid_ids) - kept_ids
        for backup_id in sorted(orphan_ids | stale_ids):
            prefix = f"{server_name}/base/{backup_id}/"
            deleted_object_prefixes[prefix] = 0 if args.dry_run else _delete_prefix(s3, args.bucket, prefix)

    tier_counts: dict[str, int] = {}
    for tier in kept.values():
        tier_counts[tier] = tier_counts.get(tier, 0) + 1

    return {
        "status": "ok",
        "dry_run": args.dry_run,
        "cluster": args.cluster,
        "namespace": args.namespace,
        "completed_backups": len(completed),
        "weekend_backup_resources": [record.name for record in weekend_records],
        "valid_backups": len(valid_completed),
        "missing_object_backup_resources": [record.name for record in missing_object_records],
        "kept_backups": len(kept_ids),
        "tier_counts": tier_counts,
        "pruned_backup_resources": [record.name for record in pruned_records],
        "deleted_backup_resources": deleted_backup_resources,
        "deleted_object_prefixes": deleted_object_prefixes,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Apply tiered retention to CNPG base backups in MinIO.")
    parser.add_argument("--namespace", default=os.getenv("NAMESPACE") or _service_account_namespace())
    parser.add_argument("--cluster", default=os.getenv("CLUSTER_NAME", "daytrader-postgres"))
    parser.add_argument("--bucket", default=os.getenv("BACKUP_BUCKET", "daytrader-cnpg"))
    parser.add_argument("--endpoint-url", default=os.getenv("S3_ENDPOINT_URL", "http://host.docker.internal:9000"))
    parser.add_argument("--hourly", type=int, default=int(os.getenv("BACKUP_KEEP_HOURLY", "24")))
    parser.add_argument("--daily", type=int, default=int(os.getenv("BACKUP_KEEP_DAILY", "7")))
    parser.add_argument("--weekly", type=int, default=int(os.getenv("BACKUP_KEEP_WEEKLY", "4")))
    parser.add_argument("--monthly", type=int, default=int(os.getenv("BACKUP_KEEP_MONTHLY", "12")))
    parser.add_argument("--yearly", type=int, default=int(os.getenv("BACKUP_KEEP_YEARLY", "10")))
    parser.add_argument("--local-timezone", default=os.getenv("BACKUP_LOCAL_TIMEZONE", "Europe/Copenhagen"))
    parser.add_argument(
        "--purge-weekends",
        action=argparse.BooleanOptionalAction,
        default=os.getenv("BACKUP_PURGE_WEEKENDS", "true").lower() in {"1", "true", "yes"},
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    print(prune(args))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
