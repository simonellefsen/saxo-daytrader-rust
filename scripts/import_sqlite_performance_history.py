from __future__ import annotations

import argparse
import os
import sqlite3
from pathlib import Path
from typing import Any

from saxo_daytrader_xai.db import connect, init_db


PERFORMANCE_TABLES = ("portfolio_value_history", "portfolio_price_snapshots")
LEDGER_TABLES = ("trade_ledger",)


def _sqlite_columns(connection: sqlite3.Connection, table_name: str) -> list[str]:
    return [row["name"] for row in connection.execute(f"PRAGMA table_info({table_name})").fetchall()]


def _target_columns(connection: Any, table_name: str) -> list[str]:
    if getattr(connection, "dialect", "sqlite") == "postgres":
        rows = connection.execute(
            """
            SELECT column_name AS name
            FROM information_schema.columns
            WHERE table_schema = current_schema()
              AND table_name = ?
            ORDER BY ordinal_position
            """,
            (table_name,),
        ).fetchall()
        return [row["name"] for row in rows]
    return [row["name"] for row in connection.execute(f"PRAGMA table_info({table_name})").fetchall()]


def _has_table(connection: sqlite3.Connection, table_name: str) -> bool:
    row = connection.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        (table_name,),
    ).fetchone()
    return row is not None


def _insert_row(target: Any, table_name: str, columns: list[str], row: sqlite3.Row) -> int:
    placeholders = ", ".join("?" for _ in columns)
    column_sql = ", ".join(columns)
    cursor = target.execute(
        f"INSERT INTO {table_name} ({column_sql}) VALUES ({placeholders}) ON CONFLICT DO NOTHING",
        tuple(row[column] for column in columns),
    )
    return max(int(cursor.rowcount or 0), 0)


def _reset_identity(target: Any, table_name: str) -> None:
    if getattr(target, "dialect", "sqlite") != "postgres":
        return
    sequence_row = target.execute(
        "SELECT pg_get_serial_sequence(?, 'id') AS sequence_name",
        (table_name,),
    ).fetchone()
    sequence_name = sequence_row["sequence_name"] if sequence_row else None
    if not sequence_name:
        return
    target.execute(
        f"""
        SELECT setval(
            ?::regclass,
            COALESCE((SELECT MAX(id) FROM {table_name}), 1),
            COALESCE((SELECT MAX(id) FROM {table_name}), 0) > 0
        )
        """,
        (sequence_name,),
    )


def _import_value_history(source: sqlite3.Connection, target: Any) -> dict[str, int]:
    source_columns = _sqlite_columns(source, "portfolio_value_history")
    target_columns = _target_columns(target, "portfolio_value_history")
    columns = [column for column in source_columns if column in target_columns and column != "id"]
    if not columns:
        return {"source_rows": 0, "inserted": 0, "skipped": 0}

    source_rows = source.execute(
        f"SELECT {', '.join(source_columns)} FROM portfolio_value_history ORDER BY recorded_at, id"
    ).fetchall()
    inserted = 0
    skipped = 0
    for row in source_rows:
        existing = target.execute(
            """
            SELECT id
            FROM portfolio_value_history
            WHERE recorded_at = ?
              AND snapshot_type = ?
            LIMIT 1
            """,
            (row["recorded_at"], row["snapshot_type"]),
        ).fetchone()
        if existing is not None:
            skipped += 1
            continue
        inserted += _insert_row(target, "portfolio_value_history", columns, row)
    return {"source_rows": len(source_rows), "inserted": inserted, "skipped": skipped}


def _import_price_snapshots(source: sqlite3.Connection, target: Any) -> dict[str, int]:
    source_columns = _sqlite_columns(source, "portfolio_price_snapshots")
    target_columns = _target_columns(target, "portfolio_price_snapshots")
    columns = [column for column in source_columns if column in target_columns]
    if not columns:
        return {"source_rows": 0, "inserted": 0, "skipped": 0}

    source_rows = source.execute(
        f"SELECT {', '.join(columns)} FROM portfolio_price_snapshots ORDER BY symbol"
    ).fetchall()
    inserted = 0
    skipped = 0
    for row in source_rows:
        before = inserted
        inserted += _insert_row(target, "portfolio_price_snapshots", columns, row)
        if inserted == before:
            skipped += 1
    return {"source_rows": len(source_rows), "inserted": inserted, "skipped": skipped}


def _import_simple_table(source: sqlite3.Connection, target: Any, table_name: str) -> dict[str, int]:
    source_columns = _sqlite_columns(source, table_name)
    target_columns = _target_columns(target, table_name)
    columns = [column for column in source_columns if column in target_columns]
    if not columns:
        return {"source_rows": 0, "inserted": 0, "skipped": 0}

    source_rows = source.execute(f"SELECT {', '.join(columns)} FROM {table_name} ORDER BY 1").fetchall()
    inserted = 0
    skipped = 0
    for row in source_rows:
        before = inserted
        inserted += _insert_row(target, table_name, columns, row)
        if inserted == before:
            skipped += 1
    _reset_identity(target, table_name)
    return {"source_rows": len(source_rows), "inserted": inserted, "skipped": skipped}


def import_performance_history(source_path: Path, target_dsn: str, *, include_trade_ledger: bool = False) -> dict[str, Any]:
    if not source_path.exists():
        raise FileNotFoundError(f"SQLite source database not found: {source_path}")

    source = sqlite3.connect(str(source_path))
    source.row_factory = sqlite3.Row
    target = connect(target_dsn)
    try:
        init_db(target)
        result: dict[str, Any] = {"status": "ok", "source": str(source_path), "tables": {}}
        if _has_table(source, "portfolio_value_history"):
            result["tables"]["portfolio_value_history"] = _import_value_history(source, target)
        if _has_table(source, "portfolio_price_snapshots"):
            result["tables"]["portfolio_price_snapshots"] = _import_price_snapshots(source, target)
        if include_trade_ledger and _has_table(source, "trade_ledger"):
            result["tables"]["trade_ledger"] = _import_simple_table(source, target, "trade_ledger")
        target.commit()
        return result
    finally:
        source.close()
        target.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="Import historical performance rows from SQLite into PostgreSQL.")
    parser.add_argument("--source", default="ledger.db", help="Path to the old SQLite ledger.db file.")
    parser.add_argument("--target", default="", help="PostgreSQL DSN. Defaults to DATABASE_URL.")
    parser.add_argument("--target-env", default="DATABASE_URL", help="Environment variable containing the PostgreSQL DSN.")
    parser.add_argument(
        "--include-trade-ledger",
        action="store_true",
        help="Also import historical trade_ledger rows so virtual cash can be recalculated.",
    )
    args = parser.parse_args()

    target = args.target or os.getenv(args.target_env, "")
    if not target:
        raise SystemExit(f"Missing target PostgreSQL DSN. Set {args.target_env} or pass --target.")
    print(import_performance_history(Path(args.source), target, include_trade_ledger=args.include_trade_ledger))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
