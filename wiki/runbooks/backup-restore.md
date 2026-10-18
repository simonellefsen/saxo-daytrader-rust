---
type: runbook
tags:
  - daytrader/wiki
  - runbooks
  - backup
  - restore
  - cnpg
  - rustfs
updated: 2026-10-18
---

# CNPG And RustFS Backup/Restore Rehearsal

This runbook covers local Docker Desktop backup checks and restore rehearsals for the `saxo` daytrader deployment. The live app database is CloudNativePG in namespace `saxo`; backup objects are stored in RustFS, an S3-compatible service running in the Docker context with a local filesystem bind mount.

Do not run restore experiments against the live `saxo/daytrader-postgres` cluster. Restore into a separate namespace or throwaway Docker Desktop cluster first.

## Source Of Truth

- App namespace: `saxo`
- Database namespace: `saxo`
- CNPG cluster: `daytrader-postgres`
- Writable service: `daytrader-postgres-rw.saxo.svc.cluster.local`
- App database: `daytrader`
- S3-compatible backend: RustFS container `daytrader_rustfs`
- Bucket: `daytrader-cnpg`
- Secret name: `daytrader-minio-backup`
- Base backup server name: `daytrader-postgres-v2`

The secret name still contains `minio` for historical compatibility, but the configured backend is RustFS.

## Quick Health Check

Run the general diagnostics bundle first:

```bash
rtk make diagnostics-artifact
```

Check CNPG, backup resources, and RustFS:

```bash
rtk kubectl --context docker-desktop -n saxo get cluster daytrader-postgres
rtk kubectl --context docker-desktop -n saxo get backup --sort-by=.metadata.creationTimestamp
rtk kubectl --context docker-desktop -n saxo get cronjob daytrader-postgres-backup-schedule daytrader-postgres-backup-retention
rtk docker ps --filter name=daytrader_rustfs --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
```

Check recent backup job logs:

```bash
rtk kubectl --context docker-desktop -n saxo get jobs --sort-by=.metadata.creationTimestamp | rg 'daytrader-postgres-backup'
rtk kubectl --context docker-desktop -n saxo logs job/<backup-job-name> --tail=120
rtk kubectl --context docker-desktop -n saxo logs job/<retention-job-name> --tail=120
```

## Retention

A base backup is taken every 30 minutes (`daytrader-postgres-backup-schedule`, at :00 and :30) and WAL is archived continuously, so any moment after the oldest kept base backup can be restored. `daytrader-postgres-backup-retention` runs at :05 and :35 and keeps:

- the two newest base backups;
- the first base backup of the current ISO week and of the previous one;
- the newest base backup at least seven days old (the recovery floor).

That is four or five base backups (about 180 MB each, compressed) and seven to fourteen days of WAL (about 60 MB a day): roughly 1–1.7 GB in all, against a recovery window that is never shorter than seven days. The policy is `_checkpoint_selection` in `scripts/prune_postgres_backups.py`; `scripts/test_prune_postgres_backups.py` simulates weeks of runs, including a pause and an outage.

CNPG's own `retentionPolicy` is `3650d`, so CNPG never removes anything; the retention job is the only thing that does. It removes base backups outside the set above and, when `WAL_PRUNE` allows, the archived WAL that precedes the oldest kept base backup's `begin_wal`, which no restore can use. History files are never touched, and nothing is pruned if that `begin_wal` cannot be read or the backup's own WAL is missing.

`WAL_PRUNE` on the retention CronJob (`deploy/k8s/base/postgres-backup-retention.yaml`):

- `off`: WAL is never touched (the script's default).
- `dry_run`: each run reports `wal_pruning` (objects and bytes it would remove) in the job log and removes nothing.
- `delete`: removes them. Permanent; changing the manifest to `delete` and releasing is an operator decision.

Space comes back gradually. RustFS moves deleted objects into `rustfs/.rustfs.sys/tmp/.trash` and purges them in the background. On 2026-10-18, 36,441 deleted WAL objects (7.5 GB) took about 20 minutes to clear, so `du` straight after a run still shows the old size. Leave the trash to RustFS.

Read what a run did or would do:

```bash
rtk kubectl --context docker-desktop -n saxo logs job/<retention-job-name> | rg -o "'wal_pruning': \{[^}]*\}\}?"
```

Lesson, 2026-10-18: the bucket had reached 8.5 GB, 7.9 GB of it WAL older than the oldest kept base backup. Two faults compounded. Retention deleted base backups but never their WAL. And the previous rule kept "the newest backup at least 1h/8h/24h/72h/1w old" out of what the last run left, so every backup was pruned at about 90 minutes and none ever reached 8h: the 2026-10-10 13:30 backup, carried into the 8h slot by a seven-hour cluster pause, held every long slot at once and would never have aged out. Recovery was never at risk (WAL from that backup covers everything since), and the base-backup freshness signal stayed green because the newest backup was always fresh. A retention rule has to be checked over weeks of runs, not one snapshot.

## Manual Backup Rehearsal

Create one on-demand backup job from the scheduled backup CronJob:

```bash
rtk kubectl --context docker-desktop -n saxo create job --from=cronjob/daytrader-postgres-backup-schedule daytrader-postgres-backup-manual-$(date -u +%Y%m%d%H%M%S)
```

Watch for completion:

```bash
rtk kubectl --context docker-desktop -n saxo get jobs,pods | rg 'daytrader-postgres-backup'
rtk kubectl --context docker-desktop -n saxo get backup --sort-by=.metadata.creationTimestamp
```

Inspect the latest backup object status:

```bash
rtk kubectl --context docker-desktop -n saxo get backup -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.status.phase}{"\t"}{.status.startedAt}{"\t"}{.status.stoppedAt}{"\t"}{.status.backupId}{"\n"}{end}'
```

Expected result:

- The backup resource reaches `completed`.
- `status.backupId` is present.
- RustFS is still running.
- No failed backup or retention pods are accumulating.

## RustFS Object Inspection

Use a temporary MinIO client container to inspect the RustFS bucket without installing tools locally:

```bash
rtk docker run --rm --network host -e RUSTFS_ACCESS_KEY -e RUSTFS_SECRET_KEY minio/mc sh -c 'mc alias set backup http://127.0.0.1:9000 "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" && mc ls backup/daytrader-cnpg/daytrader-postgres-v2/base/'
```

If `--network host` is not usable on the local Docker setup, use the host gateway name:

```bash
rtk docker run --rm -e RUSTFS_ACCESS_KEY -e RUSTFS_SECRET_KEY minio/mc sh -c 'mc alias set backup http://host.docker.internal:9000 "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY" && mc ls backup/daytrader-cnpg/daytrader-postgres-v2/base/'
```

Set `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY` from the local `.env` or shell environment. Do not paste real secret values into the wiki.

## Restore Rehearsal Pattern

Restore rehearsals should prove that the latest base backup and WAL archive are usable without risking live trading data.

1. Choose a throwaway namespace, for example `saxo-restore`.
2. Create a copy of the backup secret in that namespace.
3. Create a temporary CNPG `Cluster` that bootstraps from the object store using the desired `backupID` or latest recovery target.
4. Wait for the restored cluster to become Ready.
5. Connect read-only and verify expected tables and recent row counts.
6. Delete the throwaway namespace after recording the result.

Skeleton restore manifest shape:

```yaml
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: daytrader-postgres-restore
  namespace: saxo-restore
spec:
  instances: 1
  bootstrap:
    recovery:
      source: daytrader-postgres-backup
      database: daytrader
      owner: daytrader
      secret:
        name: daytrader-postgres-app
  externalClusters:
    - name: daytrader-postgres-backup
      barmanObjectStore:
        serverName: daytrader-postgres-v2
        destinationPath: s3://daytrader-cnpg/
        endpointURL: http://host.docker.internal:9000
        s3Credentials:
          accessKeyId:
            name: daytrader-minio-backup
            key: ACCESS_KEY_ID
          secretAccessKey:
            name: daytrader-minio-backup
            key: ACCESS_SECRET_KEY
        wal:
          compression: gzip
        data:
          compression: gzip
  storage:
    size: 5Gi
```

Treat this manifest as a rehearsal template, not a checked-in restore manifest. Fill in secrets using Kubernetes secret commands or a temporary local file that is not committed.

## Restore Verification Queries

After the restore cluster is Ready, port-forward it or run a temporary Postgres client pod and verify:

```sql
select count(*) from portfolio_snapshots;
select count(*) from execution_orders;
select count(*) from xai_decision_reports;
select max(created_at) from execution_orders;
select max(created_at) from xai_decision_reports;
select count(*) from saxo_sessions;
```

The `saxo_sessions` table contains tokens. Verify row count only; do not dump token columns into logs or chat.

## Failure Triage

If backups are not completing:

- Check the backup helper image in both CronJobs.
- Check RBAC for `daytrader-backup-retention`.
- Check `daytrader-minio-backup` exists and points at RustFS credentials.
- Check RustFS is reachable from Kubernetes through `http://host.docker.internal:9000`.
- Check CNPG operator logs.

If restore cannot find WAL:

- Confirm `backup.retentionPolicy` and WAL archive objects were not pruned too aggressively.
- Confirm the restore manifest uses the same `serverName` as the backup source.
- Confirm the restore namespace secret values match the RustFS bucket credentials.

If RustFS is down:

- Start or redeploy with `BACKUP_OBJECT_STORE=rustfs`.
- Confirm no other container owns ports `9000-9001`.
- Confirm the RustFS local data directory is not inside the Docker build context.

## Rehearsal Log

After each successful or failed rehearsal, append a short entry to `wiki/log.md` with:

- backup resource name and backup id
- restore namespace
- restore target time or backup id
- verification query summary
- any fixes or follow-up actions
