"""Tests for WAL pruning in prune_postgres_backups.py.

Run with: python3 -m unittest discover -s scripts -p 'test_*.py'

The third-party modules the script imports at the top (boto3, botocore,
requests) are stubbed before import, so these run on a plain Python with no
network and no bucket: the pruning logic takes the S3 client as an argument and
a fake stands in for it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import io
import os
import sys
import types
import unittest
from pathlib import Path
from unittest import mock


def _stub_modules() -> None:
    for name in ("boto3", "requests"):
        sys.modules.setdefault(name, types.ModuleType(name))
    botocore = sys.modules.setdefault("botocore", types.ModuleType("botocore"))
    config = types.ModuleType("botocore.config")
    config.Config = object  # type: ignore[attr-defined]
    sys.modules.setdefault("botocore.config", config)
    botocore.config = config  # type: ignore[attr-defined]


_stub_modules()
sys.path.insert(0, str(Path(__file__).resolve().parent))
import prune_postgres_backups as prune  # noqa: E402

SERVER = "daytrader-postgres-v2"


class FakeS3:
    """Enough of the S3 client for listing, reading and batch deletion."""

    def __init__(self, objects: dict[str, bytes]):
        self.objects = dict(objects)
        self.delete_batches: list[int] = []

    def get_object(self, Bucket: str, Key: str):  # noqa: N803 - boto3 casing
        if Key not in self.objects:
            raise KeyError(Key)
        return {"Body": io.BytesIO(self.objects[Key])}

    def get_paginator(self, _name: str):
        fake = self

        class Paginator:
            def paginate(self, Bucket: str, Prefix: str, Delimiter: str = ""):  # noqa: N803
                keys = sorted(key for key in fake.objects if key.startswith(Prefix))
                if Delimiter:
                    tops = {Prefix + key[len(Prefix) :].split(Delimiter, 1)[0] + Delimiter for key in keys}
                    yield {"CommonPrefixes": [{"Prefix": top} for top in sorted(tops)]}
                    return
                for start in range(0, len(keys), 1000):
                    yield {
                        "Contents": [
                            {"Key": key, "Size": len(fake.objects[key])}
                            for key in keys[start : start + 1000]
                        ]
                    }

        return Paginator()

    refuse: frozenset[str] = frozenset()

    def delete_objects(self, Bucket: str, Delete: dict):  # noqa: N803
        keys = [item["Key"] for item in Delete["Objects"]]
        assert len(keys) <= 1000, "S3 deletes at most 1,000 keys a call"
        self.delete_batches.append(len(keys))
        errors = []
        for key in keys:
            if key in self.refuse:
                errors.append({"Key": key, "Code": "AccessDenied"})
            else:
                del self.objects[key]
        return {"Errors": errors} if errors else {}


def wal(segment: str, suffix: str = ".gz") -> str:
    return f"{SERVER}/wals/{segment[:16]}/{segment}{suffix}"


def backup_info(backup_id: str, begin_wal: str) -> dict[str, bytes]:
    return {
        f"{SERVER}/base/{backup_id}/backup.info": f"backup_id={backup_id}\nbegin_wal={begin_wal}\n".encode(),
        f"{SERVER}/base/{backup_id}/data.tar.gz": b"x" * 10,
    }


OLD_TL3 = "000000030000003A00000001"
OLD_TL4 = "00000004000000AA00000001"
BEGIN = "00000004000000AA00000010"
AFTER = "00000004000000AB00000002"


def bucket() -> dict[str, bytes]:
    objects = {
        wal(OLD_TL3): b"a" * 100,
        wal(OLD_TL4): b"b" * 100,
        wal(BEGIN): b"c" * 100,
        wal(BEGIN, ".00000028.backup.gz"): b"label",
        wal(AFTER): b"d" * 100,
        f"{SERVER}/wals/00000003.history.gz": b"history",
        f"{SERVER}/wals/00000004.history.gz": b"history",
    }
    objects.update(backup_info("20261010T133001", BEGIN))
    objects.update(backup_info("20261018T090001", AFTER))
    return objects


class PruneWalTests(unittest.TestCase):
    def test_only_wal_before_the_oldest_kept_backup_is_obsolete(self):
        s3 = FakeS3(bucket())
        report = prune.prune_wal(s3, "b", SERVER, ["20261018T090001", "20261010T133001"], "delete")
        self.assertEqual(report["status"], "deleted")
        self.assertEqual(report["oldest_kept_backup"], "20261010T133001")
        self.assertEqual(report["begin_wal"], BEGIN)
        self.assertEqual(report["deleted_objects"], 2)
        self.assertEqual(report["obsolete_bytes"], 200)
        self.assertNotIn(wal(OLD_TL3), s3.objects, "an older timeline's WAL goes")
        self.assertNotIn(wal(OLD_TL4), s3.objects)
        for kept in (
            wal(BEGIN),
            wal(BEGIN, ".00000028.backup.gz"),
            wal(AFTER),
            f"{SERVER}/wals/00000003.history.gz",
            f"{SERVER}/wals/00000004.history.gz",
        ):
            self.assertIn(kept, s3.objects, f"{kept} must survive")

    def test_a_dry_run_reports_and_deletes_nothing(self):
        s3 = FakeS3(bucket())
        before = dict(s3.objects)
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "dry_run")
        self.assertEqual(report["status"], "dry_run")
        self.assertEqual((report["obsolete_objects"], report["obsolete_bytes"]), (2, 200))
        self.assertEqual(s3.objects, before)
        self.assertEqual(s3.delete_batches, [])

    def test_off_touches_nothing(self):
        s3 = FakeS3(bucket())
        self.assertEqual(prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "off")["status"], "off")
        self.assertEqual(s3.delete_batches, [])

    def test_an_unreadable_backup_info_prunes_nothing(self):
        objects = bucket()
        del objects[f"{SERVER}/base/20261010T133001/backup.info"]
        s3 = FakeS3(objects)
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001", "20261018T090001"], "delete")
        self.assertEqual(report["status"], "skipped_begin_wal_unreadable")
        self.assertEqual(s3.delete_batches, [])

    def test_missing_wal_for_the_kept_backup_prunes_nothing(self):
        objects = {key: value for key, value in bucket().items() if "/wals/" not in key or OLD_TL3 in key}
        s3 = FakeS3(objects)
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "delete")
        self.assertEqual(report["status"], "skipped_backup_wal_missing")
        self.assertIn(wal(OLD_TL3), s3.objects, "nothing goes when the backup's own WAL is absent")

    def test_no_kept_backup_prunes_nothing(self):
        s3 = FakeS3(bucket())
        self.assertEqual(prune.prune_wal(s3, "b", SERVER, [], "delete")["status"], "skipped_no_kept_backup")

    def test_deletion_is_batched_at_the_s3_limit(self):
        objects = backup_info("20261010T133001", "00000004000000FF00000000")
        objects[wal("00000004000000FF00000000")] = b"keep"
        for n in range(2500):
            objects[wal(f"00000004000000A0{n:08X}")] = b"x"
        s3 = FakeS3(objects)
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "delete")
        self.assertEqual(report["deleted_objects"], 2500)
        self.assertEqual(s3.delete_batches, [1000, 1000, 500])

    def test_a_refused_delete_is_reported_not_counted(self):
        s3 = FakeS3(bucket())
        s3.refuse = frozenset({wal(OLD_TL3)})
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "delete")
        self.assertEqual(report["status"], "partially_deleted")
        self.assertEqual((report["deleted_objects"], report["failed_objects"]), (1, 1))
        self.assertIn(wal(OLD_TL3), s3.objects)

    def test_a_malformed_begin_wal_is_not_trusted(self):
        objects = bucket()
        objects[f"{SERVER}/base/20261010T133001/backup.info"] = b"begin_wal=not-a-segment\n"
        s3 = FakeS3(objects)
        report = prune.prune_wal(s3, "b", SERVER, ["20261010T133001"], "delete")
        self.assertEqual(report["status"], "skipped_begin_wal_unreadable")
        self.assertEqual(s3.delete_batches, [])


def record(backup_id: str) -> "prune.BackupRecord":
    return prune.BackupRecord(
        name=f"backup-{backup_id.lower()}",
        backup_id=backup_id,
        started_at=prune._parse_backup_id(backup_id),
        phase="completed",
        server_name=SERVER,
    )


class PruneRunTests(unittest.TestCase):
    """The whole retention run, with Kubernetes and the bucket faked.

    Four completed backups on Sunday 2026-10-18 12:00Z: retention keeps the two
    newest and the previous week's first, which is also the recovery floor
    (2026-10-10), and prunes the 2026-10-01 backup. WAL must be judged against
    what is kept, so the WAL the pruned backup would have needed goes too.
    """

    NOW = dt.datetime(2026, 10, 18, 12, 0, tzinfo=dt.timezone.utc)
    IDS = ("20261001T000001", "20261010T133001", "20261018T090001", "20261018T113001")

    def run_prune(self, *, dry_run: bool, wal_prune: str):
        objects = bucket()
        objects.update(backup_info("20261001T000001", OLD_TL4))
        objects.update(backup_info("20261018T113001", AFTER))
        s3 = FakeS3(objects)
        deleted_resources: list[str] = []
        args = argparse.Namespace(
            namespace="saxo",
            cluster="daytrader-postgres",
            bucket="b",
            endpoint_url="http://s3.invalid",
            dry_run=dry_run,
            wal_prune=wal_prune,
        )
        env = {key: value for key, value in os.environ.items() if key != "DATABASE_URL"}
        with mock.patch.object(prune, "_utc_now", return_value=self.NOW), mock.patch.object(
            prune, "_list_backup_records", return_value=[record(i) for i in self.IDS]
        ), mock.patch.object(prune, "_s3_client", return_value=s3), mock.patch.object(
            prune, "_delete_backup_record", side_effect=lambda _ns, name: deleted_resources.append(name)
        ), mock.patch.dict(os.environ, env, clear=True):
            report = prune.prune(args)
        return report, s3, deleted_resources

    def test_wal_is_judged_against_the_backups_retention_keeps(self):
        report, s3, deleted_resources = self.run_prune(dry_run=False, wal_prune="delete")
        self.assertEqual(sorted(report["kept_backups"]), sorted(self.IDS[1:]))
        self.assertEqual(deleted_resources, ["backup-20261001t000001"])
        wal_report = report["wal_pruning"][SERVER]
        self.assertEqual(wal_report["oldest_kept_backup"], "20261010T133001")
        self.assertEqual(wal_report["status"], "deleted")
        self.assertNotIn(wal(OLD_TL4), s3.objects, "the pruned backup's WAL goes with it")
        self.assertNotIn(f"{SERVER}/base/20261001T000001/backup.info", s3.objects)
        self.assertIn(wal(BEGIN), s3.objects)

    def test_a_dry_run_forces_wal_pruning_to_report_only(self):
        report, s3, deleted_resources = self.run_prune(dry_run=True, wal_prune="delete")
        self.assertEqual(report["wal_pruning"][SERVER]["status"], "dry_run")
        self.assertEqual(report["wal_pruning"][SERVER]["obsolete_objects"], 2)
        self.assertEqual(s3.delete_batches, [])
        self.assertEqual(deleted_resources, [])

    def test_wal_pruning_is_off_unless_asked(self):
        report, s3, _ = self.run_prune(dry_run=False, wal_prune="off")
        self.assertEqual(report["wal_pruning"][SERVER]["status"], "off")
        self.assertIn(wal(OLD_TL3), s3.objects)
        self.assertIn(wal(OLD_TL4), s3.objects)


def at(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value).replace(tzinfo=dt.timezone.utc)


def backup_at(started: dt.datetime) -> "prune.BackupRecord":
    return record((started + dt.timedelta(seconds=1)).strftime("%Y%m%dT%H%M%S"))


def simulate(survivors, start, end, pauses=()):
    """Runs the two CronJobs as scheduled: a backup at :00 and :30 and
    retention at :05 and :35, nothing at all during a pause. Returns each
    retention run as (time, survivors)."""
    survivors = list(survivors)
    runs = []
    now = start
    while now < end:
        if not any(a <= now < b for a, b in pauses):
            if now.minute in (0, 30):
                survivors.append(backup_at(now))
            elif now.minute in (5, 35):
                with mock.patch.object(prune, "_utc_now", return_value=now):
                    kept = prune._checkpoint_selection(survivors)
                survivors = [r for r in survivors if r.backup_id in kept]
                runs.append((now, list(survivors)))
        now += dt.timedelta(minutes=5)
    return runs


class RetentionPolicyTests(unittest.TestCase):
    """Retention over weeks of runs, not a single snapshot: the rule it
    replaced looked right at any one moment and was wrong over time."""

    def assert_window_holds(self, runs, first_backup, slack):
        """At least seven days of recovery (or all the history there is), and
        at most fourteen plus `slack`: an outage delays the backup that would
        have become the next floor, so the old one is held for that long."""
        for now, survivors in runs:
            window = now - min(r.started_at for r in survivors)
            history = now - first_backup
            self.assertGreaterEqual(
                window, min(prune.RECOVERY_WINDOW, history), f"window too short at {now}"
            )
            self.assertLessEqual(window, dt.timedelta(days=14) + slack, f"window unbounded at {now}")
            self.assertLessEqual(len(survivors), 5, f"too many bases kept at {now}")

    def test_a_month_of_backups_keeps_a_bounded_seven_day_window(self):
        start = at("2026-09-01T00:00:00")
        pauses = (
            # A seven-hour pause, like the 2026-10-10 macOS upgrade...
            (at("2026-09-10T13:57:00"), at("2026-09-10T20:53:00")),
            # ...and a three-day outage across a week boundary.
            (at("2026-09-20T06:00:00"), at("2026-09-23T06:00:00")),
        )
        runs = simulate([], start, at("2026-10-06T00:00:00"), pauses)
        self.assert_window_holds(runs, backup_at(start).started_at, slack=dt.timedelta(days=3))
        _, last = runs[-1]
        self.assertLess(
            prune.RECOVERY_WINDOW,
            runs[-1][0] - min(r.started_at for r in last),
        )

    def test_the_2026_10_18_state_keeps_its_old_floor_until_a_successor_comes_of_age(self):
        # What the old rule left: the 2026-10-10 backup and this morning's.
        survivors = [backup_at(at(value)) for value in (
            "2026-10-10T13:30:00", "2026-10-18T08:00:00", "2026-10-18T08:30:00", "2026-10-18T09:00:00",
        )]
        runs = simulate(survivors, at("2026-10-18T09:05:00"), at("2026-11-08T00:00:00"))
        # The old rule left nothing between 2026-10-10 and this morning, so the
        # 2026-10-10 backup is held until this morning's comes of age: a window
        # of fourteen days and eighteen hours at its widest.
        self.assert_window_holds(runs, survivors[0].started_at, slack=dt.timedelta(days=1))

        def kept_at(when: str) -> set[str]:
            return {r.backup_id for now, kept in runs if now == at(when) for r in kept}

        self.assertIn("20261010T133001", kept_at("2026-10-25T07:35:00"))
        # This morning's 08:00 backup turns seven days old and becomes the
        # floor, and only then is the 2026-10-10 backup released.
        self.assertNotIn("20261010T133001", kept_at("2026-10-25T08:05:00"))
        self.assertIn("20261018T080001", kept_at("2026-10-25T08:05:00"))


if __name__ == "__main__":
    unittest.main()
