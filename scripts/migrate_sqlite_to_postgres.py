from __future__ import annotations

import argparse
import os
import sqlite3
from pathlib import Path
from typing import Any

from saxo_daytrader_xai.db import connect, init_db


TABLE_ORDER = [
    "trading_environments",
    "trading_accounts",
    "app_users",
    "user_account_access",
    "import_batches",
    "position_snapshots",
    "position_lots",
    "trade_ledger",
    "lot_realizations",
    "decision_reports",
    "execution_orders",
    "execution_fills",
    "execution_order_events",
    "notification_deliveries",
    "notification_channel_state",
    "notification_alert_state",
    "scheduler_status",
    "scheduler_cycle_history",
    "portfolio_price_snapshots",
    "portfolio_value_history",
    "broker_position_snapshots",
    "broker_balance_snapshots",
    "broker_account_snapshots",
    "broker_instrument_exposures",
    "portfolio_reconciliation_adjustments",
    "audit_log",
]

AUTOINCREMENT_TABLES = {
    "audit_log",
    "decision_reports",
    "execution_fills",
    "execution_order_events",
    "execution_orders",
    "lot_realizations",
    "notification_deliveries",
    "portfolio_reconciliation_adjustments",
    "portfolio_value_history",
    "position_snapshots",
    "scheduler_cycle_history",
    "trade_ledger",
}


def _sqlite_columns(connection: sqlite3.Connection, table_name: str) -> list[str]:
    return [row["name"] for row in connection.execute(f"PRAGMA table_info({table_name})").fetchall()]


def _postgres_columns(connection: Any, table_name: str) -> list[str]:
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


def _existing_tables(connection: sqlite3.Connection) -> set[str]:
    rows = connection.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table'"
    ).fetchall()
    return {row["name"] for row in rows}


def _copy_table(source: sqlite3.Connection, target: Any, table_name: str) -> int:
    source_columns = _sqlite_columns(source, table_name)
    target_columns = _postgres_columns(target, table_name)
    columns = [column for column in source_columns if column in target_columns]
    if not columns:
        return 0

    rows = source.execute(f"SELECT {', '.join(columns)} FROM {table_name}").fetchall()
    if not rows:
        return 0

    placeholders = ", ".join("?" for _ in columns)
    column_sql = ", ".join(columns)
    insert_sql = f"INSERT INTO {table_name} ({column_sql}) VALUES ({placeholders}) ON CONFLICT DO NOTHING"
    inserted = 0
    for row in rows:
        cursor = target.execute(insert_sql, tuple(row[column] for column in columns))
        inserted += max(int(cursor.rowcount or 0), 0)
    return inserted


def _reset_identity(target: Any, table_name: str) -> None:
    if table_name not in AUTOINCREMENT_TABLES:
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


def migrate(source_path: Path, target_dsn: str, *, replace: bool = False) -> dict[str, Any]:
    if not source_path.exists():
        raise FileNotFoundError(f"SQLite source database not found: {source_path}")

    source = sqlite3.connect(str(source_path))
    source.row_factory = sqlite3.Row
    target = connect(target_dsn)
    try:
        init_db(source)
        init_db(target)
        source_tables = _existing_tables(source)
        copied: dict[str, int] = {}
        if replace:
            for table_name in reversed(TABLE_ORDER):
                if table_name in source_tables:
                    target.execute(f"DELETE FROM {table_name}")
        for table_name in TABLE_ORDER:
            if table_name not in source_tables:
                continue
            copied[table_name] = _copy_table(source, target, table_name)
            _reset_identity(target, table_name)
        target.commit()
        return {"status": "ok", "source": str(source_path), "copied": copied}
    finally:
        source.close()
        target.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="Migrate the local SQLite ledger into PostgreSQL.")
    parser.add_argument("--source", default="/data/ledger.db", help="Path to the SQLite ledger.db file.")
    parser.add_argument("--target", default="", help="PostgreSQL DSN. Defaults to DATABASE_URL.")
    parser.add_argument("--target-env", default="DATABASE_URL", help="Environment variable containing the PostgreSQL DSN.")
    parser.add_argument("--replace", action="store_true", help="Delete target table rows before copying.")
    args = parser.parse_args()

    target = args.target or os.getenv(args.target_env, "")
    if not target:
        raise SystemExit(f"Missing target PostgreSQL DSN. Set {args.target_env} or pass --target.")
    result = migrate(Path(args.source), target, replace=args.replace)
    print(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
