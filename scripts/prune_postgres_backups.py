from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
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


# Point-in-time recovery must always reach at least this far back.
RECOVERY_WINDOW = dt.timedelta(days=7)


def _utc_now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def _parse_time(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(dt.timezone.utc)


def _parse_backup_id(value: str) -> dt.datetime:
    return dt.datetime.strptime(value, "%Y%m%dT%H%M%S").replace(tzinfo=dt.timezone.utc)


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


def _checkpoint_selection(completed: list[BackupRecord]) -> dict[str, str]:
    """Which base backups to keep, as {backup_id: reason}.

    Continuous WAL archiving makes every moment after the oldest kept base
    backup restorable, so further bases only shorten replay and cover one being
    unreadable. What retention must guarantee is a recovery window of at least
    RECOVERY_WINDOW and a bounded footprint. It keeps:
      - the two newest backups;
      - the first backup of the current ISO week and of the previous one;
      - the newest backup at least RECOVERY_WINDOW old (the floor).
    Four or five base backups and seven to fourteen days of WAL.

    Each rule picks a backup that stays put as time passes (a calendar week) or
    that a kept backup grows into (the floor), so the previous week's first
    backup is still there when it comes of age. The rule this replaces kept
    "the newest backup at least 1h/8h/24h/72h/1w old" out of whatever the last
    run left. Everything past the two newest and the 1h slot was pruned at
    about 90 minutes, so no backup ever reached 8h; the 2026-10-10 backup, which
    a seven-hour cluster pause carried straight into the 8h slot, then filled
    every longer slot at once and would never have aged out, nor its WAL.
    """
    now = _utc_now()
    kept: dict[str, str] = {}
    by_time = sorted(completed, key=lambda r: r.started_at, reverse=True)

    for record in by_time[:2]:
        kept[record.backup_id] = "recent"

    weeks = (
        (now.isocalendar()[:2], "first-this-week"),
        ((now - dt.timedelta(weeks=1)).isocalendar()[:2], "first-last-week"),
    )
    for week, label in weeks:
        in_week = [r for r in by_time if r.started_at.isocalendar()[:2] == week]
        if in_week:
            kept.setdefault(in_week[-1].backup_id, label)

    cutoff = now - RECOVERY_WINDOW
    for record in by_time:
        if record.started_at <= cutoff:
            kept.setdefault(record.backup_id, "recovery-floor")
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


# A WAL segment name: timeline, log and segment, 24 hex digits. Archived names
# can carry a suffix (`.gz`, `.partial`, `.00000028.backup`); the 24-digit stem
# orders them. Timeline `.history` files do not match and are never pruned.
_WAL_SEGMENT = re.compile(r"^([0-9A-F]{24})(?:\.|$)")

WAL_PRUNE_MODES = ("off", "dry_run", "delete")


def _begin_wal(s3: Any, bucket: str, server_name: str, backup_id: str) -> str | None:
    """The first WAL segment a base backup needs, from its barman backup.info."""
    try:
        body = s3.get_object(Bucket=bucket, Key=f"{server_name}/base/{backup_id}/backup.info")["Body"].read()
    except Exception:  # noqa: BLE001 - an unreadable backup.info means "do not prune"
        return None
    for line in body.decode("utf-8", errors="replace").splitlines():
        key, _, value = line.partition("=")
        if key.strip() == "begin_wal":
            value = value.strip()
            return value if _WAL_SEGMENT.match(value) else None
    return None


def _wal_objects(s3: Any, bucket: str, server_name: str) -> list[tuple[str, str, int]]:
    """Every archived WAL object as (key, 24-digit segment, size). History files
    and anything else that is not a segment are left out, so they can never be
    selected for deletion."""
    prefix = f"{server_name}/wals/"
    found: list[tuple[str, str, int]] = []
    paginator = s3.get_paginator("list_objects_v2")
    for page in paginator.paginate(Bucket=bucket, Prefix=prefix):
        for item in page.get("Contents", []):
            key = str(item["Key"])
            match = _WAL_SEGMENT.match(key.rsplit("/", 1)[-1])
            if match:
                found.append((key, match.group(1), int(item.get("Size", 0))))
    return found


def prune_wal(
    s3: Any, bucket: str, server_name: str, kept_backup_ids: list[str], mode: str
) -> dict[str, Any]:
    """Removes archived WAL that no kept base backup can use.

    A restore starts from a base backup and replays WAL from that backup's
    begin_wal onwards, so every segment ordered before the oldest kept backup's
    begin_wal serves no restore at all. Base retention deleted old backups but
    never their WAL, which is how 7.9 GB accumulated by 2026-10-18 against
    1.1 GB that a restore could still use.

    Refuses rather than guesses: nothing is pruned without a kept backup whose
    begin_wal can be read, and nothing is pruned if no segment at or after that
    point exists, because that would mean the backup's own WAL is missing.
    """
    report: dict[str, Any] = {"mode": mode}
    if mode == "off":
        return {**report, "status": "off"}
    if not kept_backup_ids:
        return {**report, "status": "skipped_no_kept_backup"}
    oldest = min(kept_backup_ids, key=_parse_backup_id)
    begin = _begin_wal(s3, bucket, server_name, oldest)
    report.update({"oldest_kept_backup": oldest, "begin_wal": begin})
    if begin is None:
        return {**report, "status": "skipped_begin_wal_unreadable"}
    objects = _wal_objects(s3, bucket, server_name)
    obsolete = [(key, size) for key, segment, size in objects if segment < begin]
    needed = [key for key, segment, _ in objects if segment >= begin]
    report.update(
        {
            "obsolete_objects": len(obsolete),
            "obsolete_bytes": sum(size for _, size in obsolete),
            "kept_objects": len(needed),
        }
    )
    if not needed:
        return {**report, "status": "skipped_backup_wal_missing"}
    if mode == "dry_run" or not obsolete:
        return {**report, "status": "dry_run" if mode == "dry_run" else "nothing_to_prune", "deleted_objects": 0}
    deleted = failed = 0
    keys = [key for key, _ in obsolete]
    for start in range(0, len(keys), 1000):
        batch = keys[start : start + 1000]
        response = s3.delete_objects(Bucket=bucket, Delete={"Objects": [{"Key": key} for key in batch], "Quiet": True})
        # Quiet mode lists only the keys that were not deleted.
        errors = len((response or {}).get("Errors", []))
        deleted += len(batch) - errors
        failed += errors
    return {
        **report,
        "status": "deleted" if not failed else "partially_deleted",
        "deleted_objects": deleted,
        "failed_objects": failed,
    }


def _record_backup_heartbeat(valid_completed: list[BackupRecord], dry_run: bool) -> str:
    """Publish the newest verified base backup where the application can read it.

    CNPG records base-backup state in Kubernetes and the trading application has
    no Kubernetes access, so a ten-day gap in base backups during the 2026-08-31
    disk crisis was invisible to every operator-facing surface. This is the one
    process that sees both halves: the Backup resources CNPG reports completed,
    and whether their objects actually exist in the bucket. It writes the newest
    backup that satisfies *both*, because a resource marked completed whose data
    is missing is precisely the failure that hid.

    Never fatal. Retention is the job; the heartbeat is observability, and
    losing it must not stop old backups being pruned.
    """
    database_url = os.getenv("DATABASE_URL")
    if not database_url:
        return "skipped_no_database_url"
    if not valid_completed:
        return "skipped_no_verified_backup"
    if dry_run:
        return "skipped_dry_run"
    newest = max(valid_completed, key=lambda r: r.started_at)
    try:
        import psycopg
    except ImportError:
        return "skipped_psycopg_unavailable"
    now = _utc_now().strftime("%Y-%m-%dT%H:%M:%SZ")
    summary = json.dumps(
        {
            "verified_backup_count": len(valid_completed),
            "newest_backup_name": newest.name,
            "server_name": newest.server_name,
            "source": "prune_postgres_backups",
        }
    )
    try:
        with psycopg.connect(database_url, connect_timeout=10) as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """
                    INSERT INTO backup_status (
                        singleton_key, updated_at, last_verified_backup_at,
                        last_verified_backup_id, verified_backup_count, summary_json
                    ) VALUES ('cnpg_base_backup', %s, %s, %s, %s, %s)
                    ON CONFLICT (singleton_key) DO UPDATE SET
                        updated_at = EXCLUDED.updated_at,
                        last_verified_backup_at = EXCLUDED.last_verified_backup_at,
                        last_verified_backup_id = EXCLUDED.last_verified_backup_id,
                        verified_backup_count = EXCLUDED.verified_backup_count,
                        summary_json = EXCLUDED.summary_json
                    """,
                    (
                        now,
                        newest.started_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
                        newest.backup_id,
                        len(valid_completed),
                        summary,
                    ),
                )
    except Exception as exc:  # noqa: BLE001 - observability must not fail retention
        return f"failed: {type(exc).__name__}"
    return "recorded"


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

    wal_mode = "dry_run" if args.dry_run and args.wal_prune == "delete" else args.wal_prune
    wal_pruning = {
        sn: prune_wal(
            s3,
            args.bucket,
            sn,
            [r.backup_id for r in valid_completed if r.server_name == sn and r.backup_id in kept_ids],
            wal_mode,
        )
        for sn in server_names
    }

    return {
        "status": "ok",
        "dry_run": args.dry_run,
        "wal_pruning": wal_pruning,
        "cluster": args.cluster,
        "namespace": args.namespace,
        "completed_backups": len(completed),
        "valid_backups": len(valid_completed),
        "missing_object_backup_resources": [r.name for r in missing_object_records],
        "kept_backups": kept,
        "pruned_backup_resources": [r.name for r in pruned_records],
        "deleted_backup_resources": deleted_backup_resources,
        "deleted_object_prefixes": deleted_object_prefixes,
        "heartbeat": _record_backup_heartbeat(valid_completed, args.dry_run),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Retention for CNPG base backups and their WAL in S3-compatible storage.")
    parser.add_argument("--namespace", default=os.getenv("NAMESPACE") or _service_account_namespace())
    parser.add_argument("--cluster", default=os.getenv("CLUSTER_NAME", "daytrader-postgres"))
    parser.add_argument("--bucket", default=os.getenv("BACKUP_BUCKET", "daytrader-cnpg"))
    parser.add_argument("--endpoint-url", default=os.getenv("S3_ENDPOINT_URL", "http://host.docker.internal:9000"))
    parser.add_argument("--dry-run", action="store_true")
    # Off unless asked. `dry_run` reports what would go; `delete` removes it.
    parser.add_argument("--wal-prune", choices=WAL_PRUNE_MODES, default=os.getenv("WAL_PRUNE", "off"))
    args = parser.parse_args()

    print(prune(args))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
