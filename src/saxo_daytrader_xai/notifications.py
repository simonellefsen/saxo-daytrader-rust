from __future__ import annotations

import hashlib
import json
import smtplib
from datetime import UTC, date, datetime, time, timedelta
from email.message import EmailMessage
from typing import Any

import pytz
import requests

from saxo_daytrader_xai.db import append_audit_log
from saxo_daytrader_xai.portfolio import (
    fetch_latest_batch_id,
    fetch_portfolio_positions,
    fetch_portfolio_summary,
    fetch_realised_tax_summary,
)
from saxo_daytrader_xai.xai_decision import fetch_latest_decision_report


def _notification_timezone(config: dict[str, Any]):
    return pytz.timezone(config.get("notifications", {}).get("timezone", "Europe/Copenhagen"))


def _notification_now(config: dict[str, Any], reference_time: datetime | None = None) -> datetime:
    now_utc = (reference_time or datetime.now(UTC)).astimezone(UTC)
    return now_utc.astimezone(_notification_timezone(config))


def _day_bounds_utc(config: dict[str, Any], summary_date: date) -> tuple[str, str]:
    timezone = _notification_timezone(config)
    local_start = timezone.localize(datetime.combine(summary_date, time(0, 0)), is_dst=None)
    local_end = timezone.localize(datetime.combine(summary_date, time(23, 59, 59)), is_dst=None)
    return (
        local_start.astimezone(UTC).isoformat(timespec="seconds"),
        local_end.astimezone(UTC).isoformat(timespec="seconds"),
    )


def _period_bounds_utc(config: dict[str, Any], start_date: date, end_date: date) -> tuple[str, str]:
    timezone = _notification_timezone(config)
    local_start = timezone.localize(datetime.combine(start_date, time(0, 0)), is_dst=None)
    local_end = timezone.localize(datetime.combine(end_date, time(23, 59, 59)), is_dst=None)
    return (
        local_start.astimezone(UTC).isoformat(timespec="seconds"),
        local_end.astimezone(UTC).isoformat(timespec="seconds"),
    )


def _summary_trade_stats(connection, config: dict[str, Any], start_date: date, end_date: date) -> dict[str, Any]:
    start_utc, end_utc = _period_bounds_utc(config, start_date, end_date)
    row = connection.execute(
        """
        SELECT
            COUNT(*) AS trade_count,
            COALESCE(SUM(net_amount_dkk), 0) AS net_amount_dkk,
            COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_dkk,
            COALESCE(SUM(tax_dkk), 0) AS tax_dkk,
            COALESCE(SUM(commission_dkk), 0) AS commission_dkk
        FROM trade_ledger
        WHERE created_at >= ? AND created_at <= ?
        """,
        (start_utc, end_utc),
    ).fetchone()
    return dict(row)


def _summary_execution_stats(connection, config: dict[str, Any], start_date: date, end_date: date) -> dict[str, Any]:
    start_utc, end_utc = _period_bounds_utc(config, start_date, end_date)
    rows = connection.execute(
        """
        SELECT status, COUNT(*) AS count_rows
        FROM execution_orders
        WHERE created_at >= ? AND created_at <= ?
        GROUP BY status
        ORDER BY status
        """,
        (start_utc, end_utc),
    ).fetchall()
    return {row["status"]: row["count_rows"] for row in rows}


def _summary_top_positions(connection, config: dict[str, Any]) -> list[dict[str, Any]]:
    batch_id = fetch_latest_batch_id(connection)
    prefer_broker_cash = (
        str(config.get("execution", {}).get("mode")) == "live"
        and str(config.get("execution", {}).get("adapter")) == "saxo"
    )
    positions = fetch_portfolio_positions(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0),
        prefer_broker_cash=prefer_broker_cash,
    )
    return positions[:5]


def _period_descriptor(kind: str, local_now: datetime, config: dict[str, Any]) -> tuple[date, date, str]:
    current_date = local_now.date()
    if kind == "daily":
        start_date = current_date
        end_date = current_date
        label = current_date.isoformat()
    elif kind == "weekly":
        end_date = current_date - timedelta(days=current_date.weekday() + 1)
        start_date = end_date - timedelta(days=6)
        label = f"{start_date.isoformat()}_to_{end_date.isoformat()}"
    elif kind == "monthly":
        first_of_current_month = current_date.replace(day=1)
        end_date = first_of_current_month - timedelta(days=1)
        start_date = end_date.replace(day=1)
        label = f"{start_date.isoformat()}_to_{end_date.isoformat()}"
    elif kind == "quarterly":
        first_of_current_quarter = date(current_date.year, ((current_date.month - 1) // 3) * 3 + 1, 1)
        end_date = first_of_current_quarter - timedelta(days=1)
        start_date = date(end_date.year, ((end_date.month - 1) // 3) * 3 + 1, 1)
        label = f"{start_date.isoformat()}_to_{end_date.isoformat()}"
    elif kind == "ytd":
        first_of_current_month = current_date.replace(day=1)
        end_date = first_of_current_month - timedelta(days=1)
        start_date = date(end_date.year, 1, 1)
        label = f"{start_date.isoformat()}_to_{end_date.isoformat()}"
    else:
        raise ValueError(f"Unsupported summary kind '{kind}'")
    return start_date, end_date, label


def build_summary(
    connection,
    config: dict[str, Any],
    *,
    summary_kind: str = "daily",
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    local_now = _notification_now(config, reference_time)
    start_date, end_date, summary_label = _period_descriptor(summary_kind, local_now, config)
    batch_id = fetch_latest_batch_id(connection)
    initial_cash_dkk = float(config.get("portfolio", {}).get("initial_cash_dkk", 0.0) or 0.0)
    prefer_broker_cash = (
        str(config.get("execution", {}).get("mode")) == "live"
        and str(config.get("execution", {}).get("adapter")) == "saxo"
    )
    portfolio_summary = fetch_portfolio_summary(
        connection,
        batch_id=batch_id,
        initial_cash_dkk=initial_cash_dkk,
        prefer_broker_cash=prefer_broker_cash,
    )
    tax_summary = fetch_realised_tax_summary(connection, tax_year=end_date.year)
    trade_stats = _summary_trade_stats(connection, config, start_date, end_date)
    execution_stats = _summary_execution_stats(connection, config, start_date, end_date)
    latest_report = fetch_latest_decision_report(connection)
    top_positions = _summary_top_positions(connection, config)

    suggested_trade_count = 0
    if latest_report and latest_report.get("report_json"):
        suggested_trade_count = len(latest_report["report_json"].get("suggested_trades", []))

    payload = {
        "summary_kind": summary_kind,
        "summary_date": summary_label,
        "period_start": start_date.isoformat(),
        "period_end": end_date.isoformat(),
        "generated_at_local": local_now.isoformat(timespec="seconds"),
        "portfolio": portfolio_summary,
        "period": {
            "trade_count": int(trade_stats["trade_count"]),
            "net_amount_dkk": float(trade_stats["net_amount_dkk"]),
            "realised_gain_dkk": float(trade_stats["realised_gain_dkk"]),
            "tax_dkk": float(trade_stats["tax_dkk"]),
            "commission_dkk": float(trade_stats["commission_dkk"]),
            "execution_status_counts": execution_stats,
        },
        "year_to_date_tax": tax_summary,
        "latest_decision_report": {
            "created_at": latest_report.get("created_at") if latest_report else None,
            "status": latest_report.get("status") if latest_report else None,
            "model": latest_report.get("model") if latest_report else None,
            "suggested_trade_count": suggested_trade_count,
        },
        "top_positions": [
            {
                "symbol": row["symbol"],
                "market_value_dkk": row["market_value_dkk"],
                "allocation_pct": row["allocation_pct"],
                "daily_pnl_dkk": row["daily_pnl_dkk"],
            }
            for row in top_positions
        ],
    }
    subject = f"saxo-daytrader-xai {summary_kind} summary {summary_label}"
    style = _summary_style(config, summary_kind)
    if style == "compact":
        lines = [
            subject,
            f"Portfolio {portfolio_summary['total_market_value_dkk']:.2f} DKK | Daily P/L {portfolio_summary['total_daily_pnl_dkk']:.2f} DKK | Trades {int(trade_stats['trade_count'])}",
            f"Realised {float(trade_stats['realised_gain_dkk']):.2f} DKK | Tax {float(trade_stats['tax_dkk']):.2f} DKK | Commission {float(trade_stats['commission_dkk']):.2f} DKK",
            f"Decision report {payload['latest_decision_report']['status'] or 'none'} | Suggested trades {suggested_trade_count}",
        ]
    else:
        lines = [
            subject,
            "",
            f"Period: {start_date.isoformat()} to {end_date.isoformat()}",
            "",
            "Portfolio:",
            f"- Value: {portfolio_summary['total_market_value_dkk']:.2f} DKK",
            f"- Daily P/L: {portfolio_summary['total_daily_pnl_dkk']:.2f} DKK",
            f"- Unrealised P/L: {portfolio_summary['total_unrealised_pnl_dkk']:.2f} DKK",
            "",
            "Trading:",
            f"- Trades: {int(trade_stats['trade_count'])}",
            f"- Realised gain: {float(trade_stats['realised_gain_dkk']):.2f} DKK",
            f"- Net amount: {float(trade_stats['net_amount_dkk']):.2f} DKK",
            f"- Tax: {float(trade_stats['tax_dkk']):.2f} DKK",
            f"- Commission: {float(trade_stats['commission_dkk']):.2f} DKK",
            "",
            "Decision Engine:",
            f"- Latest report: {payload['latest_decision_report']['status'] or 'none'}",
            f"- Suggested trades: {suggested_trade_count}",
        ]
        if execution_stats:
            lines.append("- Execution status counts:")
            lines.extend(f"  - {status}: {count}" for status, count in sorted(execution_stats.items()))
        if top_positions:
            lines.extend(
                [
                    "",
                    "Top Positions:",
                    *[
                        f"- {row['symbol']}: {float(row['market_value_dkk']):.2f} DKK ({float(row['allocation_pct']) * 100:.2f}%), daily {float(row['daily_pnl_dkk'] or 0):.2f} DKK"
                        for row in top_positions[:3]
                    ],
                ]
            )
    formatted_subject, formatted_message = _format_delivery_content(config, summary_kind, subject, "\n".join(lines))
    return {
        "summary_date": payload["summary_date"],
        "subject": formatted_subject,
        "message_text": formatted_message,
        "payload": payload,
    }


def build_daily_summary(connection, config: dict[str, Any], reference_time: datetime | None = None) -> dict[str, Any]:
    return build_summary(connection, config, summary_kind="daily", reference_time=reference_time)


def _record_notification_delivery(
    connection,
    *,
    summary_date: str,
    summary_kind: str,
    channel: str,
    status: str,
    subject: str,
    message_text: str,
    payload: dict[str, Any],
    error_text: str | None = None,
) -> int:
    cursor = connection.execute(
        """
        INSERT INTO notification_deliveries (
            created_at, summary_date, summary_kind, channel, status, subject, message_text, payload_json, error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            summary_date,
            summary_kind,
            channel,
            status,
            subject,
            message_text,
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
            error_text,
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


def _already_sent(connection, summary_date: str, summary_kind: str, channel: str) -> bool:
    row = connection.execute(
        """
        SELECT 1
        FROM notification_deliveries
        WHERE summary_date = ? AND summary_kind = ? AND channel = ? AND status = 'sent'
        ORDER BY id DESC
        LIMIT 1
        """,
        (summary_date, summary_kind, channel),
    ).fetchone()
    return row is not None


def _state_key(summary_kind: str, channel: str) -> str:
    return f"{summary_kind}:{channel}"


def _notification_state(connection, summary_kind: str, channel: str) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT *
        FROM notification_channel_state
        WHERE channel = ?
        """,
        (_state_key(summary_kind, channel),),
    ).fetchone()
    return dict(row) if row else None


def _upsert_notification_state(
    connection,
    *,
    summary_kind: str,
    channel: str,
    summary_date: str,
    last_attempt_at: str,
    next_attempt_after: str | None,
    attempt_count: int,
    last_status: str,
    last_error_text: str | None,
) -> None:
    connection.execute(
        """
        INSERT INTO notification_channel_state (
            channel, summary_date, last_attempt_at, next_attempt_after, attempt_count, last_status, last_error_text
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(channel) DO UPDATE SET
            summary_date = excluded.summary_date,
            last_attempt_at = excluded.last_attempt_at,
            next_attempt_after = excluded.next_attempt_after,
            attempt_count = excluded.attempt_count,
            last_status = excluded.last_status,
            last_error_text = excluded.last_error_text
        """,
        (
            _state_key(summary_kind, channel),
            summary_date,
            last_attempt_at,
            next_attempt_after,
            attempt_count,
            last_status,
            last_error_text,
        ),
    )
    connection.commit()


def _channel_ready(
    connection,
    config: dict[str, Any],
    *,
    summary_kind: str,
    channel: str,
    summary_date: str,
    reference_time: datetime,
    force: bool,
) -> tuple[bool, str]:
    if force:
        return True, "forced"
    if _already_sent(connection, summary_date, summary_kind, channel):
        return False, "already_sent"
    state = _notification_state(connection, summary_kind, channel)
    if not state or state.get("summary_date") != summary_date:
        return True, "fresh"
    max_attempts = int(config.get("notifications", {}).get("max_attempts_per_day", 3))
    if int(state.get("attempt_count") or 0) >= max_attempts:
        return False, "max_attempts_reached"
    next_attempt_after = state.get("next_attempt_after")
    if next_attempt_after:
        next_dt = datetime.fromisoformat(str(next_attempt_after))
        if reference_time < next_dt:
            return False, "backoff_active"
    cooldown_minutes = int(config.get("notifications", {}).get("channel_cooldown_minutes", 240))
    last_attempt_at = state.get("last_attempt_at")
    if last_attempt_at and state.get("last_status") == "sent":
        last_dt = datetime.fromisoformat(str(last_attempt_at))
        if reference_time < last_dt + timedelta(minutes=cooldown_minutes):
            return False, "cooldown_active"
    return True, "ready"


def _route_config(config: dict[str, Any], summary_kind: str) -> dict[str, Any]:
    routes = config.get("notifications", {}).get("routes", {})
    route = routes.get(summary_kind, {})
    if not isinstance(route, dict):
        return {}
    profile_name = route.get("profile")
    profile_cfg: dict[str, Any] = {}
    if profile_name:
        profiles = config.get("notifications", {}).get("route_profiles", {})
        candidate = profiles.get(profile_name, {})
        if isinstance(candidate, dict):
            profile_cfg = candidate
    route_overrides = {key: value for key, value in route.items() if key != "profile"}
    return {**profile_cfg, **route_overrides}


def _summary_style(config: dict[str, Any], summary_kind: str) -> str:
    route_cfg = _route_config(config, summary_kind)
    return str(route_cfg.get("summary_style") or config.get("notifications", {}).get("summary_style", "structured")).lower()


def _format_delivery_content(
    config: dict[str, Any],
    summary_kind: str,
    subject: str,
    message_text: str,
) -> tuple[str, str]:
    route_cfg = _route_config(config, summary_kind)
    subject_prefix = str(route_cfg.get("subject_prefix") or "").strip()
    message_preamble = str(route_cfg.get("message_preamble") or "").strip()

    formatted_subject = f"{subject_prefix} {subject}".strip() if subject_prefix else subject
    formatted_message = f"{message_preamble}\n\n{message_text}" if message_preamble else message_text
    return formatted_subject, formatted_message


def _resolve_slack_webhook(config: dict[str, Any], summary_kind: str) -> str:
    route_cfg = _route_config(config, summary_kind)
    webhook_url = route_cfg.get("slack_webhook_url") or config.get("notifications", {}).get("slack", {}).get("webhook_url")
    if not webhook_url:
        raise ValueError("Slack webhook URL is missing")
    return str(webhook_url)


def _resolve_email_to_addresses(config: dict[str, Any], summary_kind: str) -> list[str]:
    route_cfg = _route_config(config, summary_kind)
    raw_to = route_cfg.get("email_to_addresses_csv") or config.get("notifications", {}).get("email", {}).get("to_addresses_csv") or ""
    return [part.strip() for part in str(raw_to).split(",") if part.strip()]


def _send_slack(
    config: dict[str, Any],
    subject: str,
    message_text: str,
    payload: dict[str, Any],
    *,
    summary_kind: str,
) -> dict[str, Any]:
    webhook_url = _resolve_slack_webhook(config, summary_kind)
    response = requests.post(
        webhook_url,
        json={
            "text": f"*{subject}*\n```{message_text}```",
            "metadata": {"event_type": summary_kind, "event_payload": payload},
        },
        timeout=20,
    )
    response.raise_for_status()
    return {"status_code": response.status_code, "webhook_url": webhook_url}


def _send_email(config: dict[str, Any], subject: str, message_text: str, *, summary_kind: str) -> dict[str, Any]:
    email_cfg = config.get("notifications", {}).get("email", {})
    host = str(email_cfg.get("smtp_host") or "")
    if not host:
        raise ValueError("SMTP host is missing")
    port = int(email_cfg.get("smtp_port") or 587)
    username = str(email_cfg.get("username") or "")
    password = str(email_cfg.get("password") or "")
    from_address = str(email_cfg.get("from_address") or "")
    to_addresses = _resolve_email_to_addresses(config, summary_kind)
    if not from_address or not to_addresses:
        raise ValueError("SMTP from/to addresses are missing")

    message = EmailMessage()
    message["Subject"] = subject
    message["From"] = from_address
    message["To"] = ", ".join(to_addresses)
    message.set_content(message_text)

    with smtplib.SMTP(host, port, timeout=20) as smtp:
        if bool(email_cfg.get("use_starttls", True)):
            smtp.starttls()
        if username:
            smtp.login(username, password)
        smtp.send_message(message)
    return {"to": to_addresses}


def fetch_notification_deliveries(connection, limit: int = 100) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM notification_deliveries
        ORDER BY id DESC
        LIMIT ?
        """,
        (limit,),
    ).fetchall()
    output: list[dict[str, Any]] = []
    for row in rows:
        record = dict(row)
        record["payload_json"] = json.loads(record["payload_json"]) if record.get("payload_json") else None
        output.append(record)
    return output


def _alert_severity(summary_kind: str) -> str:
    return {
        "alert_execution_success": "medium",
        "alert_execution_warning": "low",
        "alert_broker_fill": "medium",
        "alert_broker_reject": "high",
        "alert_broker_cancel": "low",
        "alert_broker_grouped": "medium",
        "alert_execution_failed": "high",
        "alert_broker_management_failed": "high",
        "alert_saxo_session_failed": "high",
    }.get(summary_kind, "medium")


def _alert_scope_key(summary_kind: str, record: dict[str, Any]) -> str:
    execution_order_id = record.get("execution_order_id") or record.get("id")
    return f"{summary_kind}:order:{execution_order_id}"


def _alert_cooldown_minutes(config: dict[str, Any], severity: str) -> int:
    suppression_cfg = config.get("notifications", {}).get("alert_suppression", {})
    return int(
        suppression_cfg.get(
            f"{severity}_cooldown_minutes",
            {"low": 240, "medium": 60, "high": 0}.get(severity, 60),
        )
    )


def _alert_state(connection, scope_key: str) -> dict[str, Any] | None:
    row = connection.execute(
        """
        SELECT *
        FROM notification_alert_state
        WHERE scope_key = ?
        """,
        (scope_key,),
    ).fetchone()
    return dict(row) if row else None


def _upsert_alert_state(
    connection,
    *,
    scope_key: str,
    severity: str,
    last_sent_at: str,
    last_alert_key: str,
    last_summary_kind: str,
    last_delivery_id: int | None,
) -> None:
    connection.execute(
        """
        INSERT INTO notification_alert_state (
            scope_key, severity, last_sent_at, last_alert_key, last_summary_kind, last_delivery_id
        ) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(scope_key) DO UPDATE SET
            severity = excluded.severity,
            last_sent_at = excluded.last_sent_at,
            last_alert_key = excluded.last_alert_key,
            last_summary_kind = excluded.last_summary_kind,
            last_delivery_id = excluded.last_delivery_id
        """,
        (scope_key, severity, last_sent_at, last_alert_key, last_summary_kind, last_delivery_id),
    )
    connection.commit()


def _suppressed_alert_reason(
    connection,
    config: dict[str, Any],
    *,
    alert: dict[str, Any],
    reference_time: datetime,
    force: bool,
) -> str | None:
    suppression_cfg = config.get("notifications", {}).get("alert_suppression", {})
    if force or not bool(suppression_cfg.get("enabled", True)):
        return None
    severity = alert["severity"]
    cooldown_minutes = _alert_cooldown_minutes(config, severity)
    if cooldown_minutes <= 0:
        return None
    state = _alert_state(connection, alert["scope_key"])
    if not state or not state.get("last_sent_at"):
        return None
    last_sent_at = datetime.fromisoformat(str(state["last_sent_at"]))
    if reference_time < last_sent_at + timedelta(minutes=cooldown_minutes):
        return f"suppressed_{severity}"
    return None


def _alerts_enabled(config: dict[str, Any]) -> bool:
    alerts_cfg = config.get("notifications", {}).get("alerts", {})
    return any(
        bool(alerts_cfg.get(key, False))
        for key in (
            "execution_success_enabled",
            "execution_warning_enabled",
            "broker_fill_enabled",
            "broker_reject_enabled",
            "broker_cancel_enabled",
            "execution_failure_enabled",
            "broker_management_failure_enabled",
            "saxo_session_failure_enabled",
        )
    )


def _severity_rank(severity: str) -> int:
    return {"low": 1, "medium": 2, "high": 3}.get(severity, 2)


def _execution_source_label(record: dict[str, Any]) -> str:
    if str(record.get("strategy_type") or "") == "portfolio_sync":
        return "SIM portfolio sync"
    if str(record.get("strategy_type") or "") in {"swing", "ladder"}:
        return "Trading Manager"
    return "Execution"


def _execution_success_prefix(record: dict[str, Any]) -> str:
    source = _execution_source_label(record)
    if str(record.get("status") or "") == "executed":
        return f"{source} executed"
    return f"{source} submitted to broker"


def _build_broker_alert_candidates(connection, config: dict[str, Any], limit: int = 25) -> list[dict[str, Any]]:
    alerts_cfg = config.get("notifications", {}).get("alerts", {})
    alerts_by_scope: dict[str, dict[str, Any]] = {}

    if alerts_cfg.get("saxo_session_failure_enabled", False):
        session_failure_rows = connection.execute(
            """
            SELECT *
            FROM audit_log
            WHERE event_type = 'saxo_session_keepalive_failed'
            ORDER BY id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        for row in session_failure_rows:
            record = dict(row)
            try:
                payload = json.loads(record.get("event_json") or "{}")
            except ValueError:
                payload = {}
            error_text = str(payload.get("error") or "Unknown Saxo session error")
            environment = str(payload.get("environment") or "unknown")
            event_date = str(record.get("created_at") or "")[:10] or datetime.now(UTC).date().isoformat()
            fingerprint = hashlib.sha1(f"{environment}:{error_text}".encode("utf-8")).hexdigest()[:12]
            alert_key = f"saxo_session_failed:{event_date}:{fingerprint}"
            scope_key = f"alert_saxo_session_failed:{environment}:{fingerprint}"
            if scope_key in alerts_by_scope:
                continue
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": "alert_saxo_session_failed",
                "severity": _alert_severity("alert_saxo_session_failed"),
                "scope_key": scope_key,
                "execution_order_id": None,
                "subject": f"Saxo session connection failed ({environment})",
                "message_text": "\n".join(
                    [
                        f"Saxo session connection failed ({environment})",
                        "",
                        f"Audit event ID: {record['id']}",
                        f"Detected at: {record.get('created_at') or 'n/a'}",
                        f"Error: {error_text}",
                        "",
                        "Trading actions that require Saxo access will not be able to refresh session state until this is fixed.",
                    ]
                ),
                "payload": {
                    "alert_type": "saxo_session_failed",
                    "record": record,
                    "session_error": payload,
                },
            }

    if alerts_cfg.get("execution_success_enabled", False):
        success_rows = connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE status IN ('executed', 'submitted_to_broker')
            ORDER BY id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        for row in success_rows:
            record = dict(row)
            alert_key = f"execution_success:{record['id']}:{record['status']}"
            scope_key = _alert_scope_key("alert_execution_success", record)
            if scope_key in alerts_by_scope:
                continue
            quantity = record.get("quantity")
            quantity_text = f"{float(quantity):.0f}" if quantity is not None else "n/a"
            subject_prefix = _execution_success_prefix(record)
            source_label = _execution_source_label(record)
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": "alert_execution_success",
                "severity": _alert_severity("alert_execution_success"),
                "scope_key": scope_key,
                "execution_order_id": record["id"],
                "subject": f"{subject_prefix} for {record['symbol']}",
                "message_text": "\n".join(
                    [
                        f"{subject_prefix} for {record['symbol']}",
                        "",
                        f"Execution order ID: {record['id']}",
                        f"Mode: {record.get('mode') or 'n/a'}",
                        f"Action: {record.get('action') or 'n/a'}",
                        f"Source: {source_label}",
                        f"Quantity: {quantity_text}",
                        f"Status: {record['status']}",
                        f"Estimated value DKK: {float(record.get('estimated_value_dkk') or 0.0):.2f}",
                        f"Broker Order ID: {record.get('broker_order_id') or 'n/a'}",
                    ]
                ),
                "payload": {
                    "alert_type": "execution_success",
                    "record": record,
                },
            }

    if alerts_cfg.get("execution_warning_enabled", False):
        warning_rows = connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE status IN ('pending_approval', 'blocked_by_dry_run', 'invalid_quantity', 'waiting_for_market_open', 'waiting_for_cash_settlement')
            ORDER BY id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        for row in warning_rows:
            record = dict(row)
            alert_key = f"execution_warning:{record['id']}:{record['status']}"
            scope_key = _alert_scope_key("alert_execution_warning", record)
            if scope_key in alerts_by_scope:
                continue
            quantity = record.get("quantity")
            quantity_text = f"{float(quantity):.0f}" if quantity is not None else "n/a"
            warning_reason = record.get("error_text") or record["status"]
            source_label = _execution_source_label(record)
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": "alert_execution_warning",
                "severity": _alert_severity("alert_execution_warning"),
                "scope_key": scope_key,
                "execution_order_id": record["id"],
                "subject": f"Execution warning for {record['symbol']}",
                "message_text": "\n".join(
                    [
                        f"Execution warning for {record['symbol']}",
                        "",
                        f"Execution order ID: {record['id']}",
                        f"Mode: {record.get('mode') or 'n/a'}",
                        f"Action: {record.get('action') or 'n/a'}",
                        f"Source: {source_label}",
                        f"Quantity: {quantity_text}",
                        f"Status: {record['status']}",
                        f"Warning: {warning_reason}",
                    ]
                ),
                "payload": {
                    "alert_type": "execution_warning",
                    "record": record,
                },
            }

    if alerts_cfg.get("execution_failure_enabled", False):
        failure_rows = connection.execute(
            """
            SELECT *
            FROM execution_orders
            WHERE status = 'execution_failed'
            ORDER BY id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        for row in failure_rows:
            record = dict(row)
            alert_key = f"execution_failed:{record['id']}"
            scope_key = _alert_scope_key("alert_execution_failed", record)
            if scope_key in alerts_by_scope:
                continue
            error_text = record.get("error_text") or "Unknown execution error"
            quantity = record.get("quantity")
            quantity_text = f"{float(quantity):.0f}" if quantity is not None else "n/a"
            source_label = _execution_source_label(record)
            subject_prefix = f"{source_label} failed" if source_label != "Execution" else "Execution failed"
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": "alert_execution_failed",
                "severity": _alert_severity("alert_execution_failed"),
                "scope_key": scope_key,
                "execution_order_id": record["id"],
                "subject": f"{subject_prefix} for {record['symbol']}",
                "message_text": "\n".join(
                    [
                        f"{subject_prefix} for {record['symbol']}",
                        "",
                        f"Execution order ID: {record['id']}",
                        f"Mode: {record.get('mode') or 'n/a'}",
                        f"Action: {record.get('action') or 'n/a'}",
                        f"Source: {source_label}",
                        f"Quantity: {quantity_text}",
                        f"Broker Order ID: {record.get('broker_order_id') or 'n/a'}",
                        f"Error: {error_text}",
                    ]
                ),
                "payload": {
                    "alert_type": "execution_failed",
                    "record": record,
                },
            }

    if alerts_cfg.get("broker_fill_enabled", False):
        fill_rows = connection.execute(
            """
            SELECT *
            FROM execution_fills
            ORDER BY id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        for row in fill_rows:
            record = dict(row)
            alert_key = f"fill:{record['id']}"
            scope_key = _alert_scope_key("alert_broker_fill", record)
            if scope_key in alerts_by_scope:
                continue
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": "alert_broker_fill",
                "severity": _alert_severity("alert_broker_fill"),
                "scope_key": scope_key,
                "execution_order_id": record["execution_order_id"],
                "subject": f"Broker fill confirmed for {record['symbol']}",
                "message_text": "\n".join(
                    [
                        f"Broker fill confirmed for {record['symbol']}",
                        "",
                        f"Order ID: {record['execution_order_id']}",
                        f"Broker Order ID: {record.get('broker_order_id') or 'n/a'}",
                        f"Side: {record['side']}",
                        f"Status: {record['fill_status']}",
                        f"Delta quantity: {float(record['delta_quantity']):.4f}",
                        f"Cumulative quantity: {float(record['cumulative_quantity']):.4f}",
                        f"Average price: {float(record['average_price_local']):.4f} {record['currency']}",
                        f"Ledger ID: {record.get('ledger_id') or 'n/a'}",
                    ]
                ),
                "payload": {
                    "alert_type": "broker_fill",
                    "record": record,
                },
            }

    event_type_map = {}
    if alerts_cfg.get("broker_reject_enabled", False):
        event_type_map["broker_rejected"] = ("alert_broker_reject", "Broker order rejected")
    if alerts_cfg.get("broker_cancel_enabled", False):
        event_type_map["broker_cancelled"] = ("alert_broker_cancel", "Broker order cancelled")
        event_type_map["broker_expired"] = ("alert_broker_cancel", "Broker order expired")
    if alerts_cfg.get("broker_management_failure_enabled", False):
        event_type_map["broker_cancel_failed"] = ("alert_broker_management_failed", "Broker cancel failed")
        event_type_map["broker_replace_failed"] = ("alert_broker_management_failed", "Broker replace failed")

    if event_type_map:
        placeholders = ", ".join("?" for _ in event_type_map)
        event_rows = connection.execute(
            f"""
            SELECT *
            FROM execution_order_events
            WHERE event_type IN ({placeholders})
            ORDER BY id DESC
            LIMIT ?
            """,
            (*event_type_map.keys(), limit),
        ).fetchall()
        for row in event_rows:
            record = dict(row)
            summary_kind, subject_prefix = event_type_map[record["event_type"]]
            alert_key = f"event:{record['id']}"
            scope_key = _alert_scope_key(summary_kind, record)
            if scope_key in alerts_by_scope:
                continue
            alerts_by_scope[scope_key] = {
                "alert_key": alert_key,
                "summary_kind": summary_kind,
                "severity": _alert_severity(summary_kind),
                "scope_key": scope_key,
                "execution_order_id": record["execution_order_id"],
                "subject": f"{subject_prefix} for order {record['execution_order_id']}",
                "message_text": "\n".join(
                    [
                        f"{subject_prefix} for execution order {record['execution_order_id']}",
                        "",
                        f"Broker Order ID: {record.get('broker_order_id') or 'n/a'}",
                        f"Event type: {record['event_type']}",
                        f"Broker status: {record.get('broker_status') or 'n/a'}",
                        f"Broker substatus: {record.get('broker_substatus') or 'n/a'}",
                        f"Quantity: {record.get('broker_quantity') if record.get('broker_quantity') is not None else 'n/a'}",
                        f"Price: {record.get('broker_price_local') if record.get('broker_price_local') is not None else 'n/a'}",
                    ]
                ),
                "payload": {
                    "alert_type": record["event_type"],
                    "record": record,
                },
            }

    alerts = list(alerts_by_scope.values())
    alerts.sort(key=lambda item: item["alert_key"])
    return alerts[:limit]


def _group_broker_alert_candidates(config: dict[str, Any], alerts: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouping_cfg = config.get("notifications", {}).get("alert_grouping", {})
    if not bool(grouping_cfg.get("enabled", True)):
        return alerts

    grouped_by_order: dict[int, list[dict[str, Any]]] = {}
    non_order_alerts: list[dict[str, Any]] = []
    for alert in alerts:
        if alert.get("execution_order_id") is None:
            non_order_alerts.append(alert)
            continue
        grouped_by_order.setdefault(int(alert["execution_order_id"]), []).append(alert)

    max_items = int(grouping_cfg.get("max_items_per_group", 5))
    output: list[dict[str, Any]] = []
    for execution_order_id, items in grouped_by_order.items():
        items.sort(key=lambda item: item["alert_key"])
        if len(items) == 1:
            output.extend(items)
            continue
        highest = max(items, key=lambda item: _severity_rank(str(item["severity"])))
        preview_items = items[:max_items]
        preview_lines = []
        for item in preview_items:
            preview_lines.append(f"- {item['summary_kind']}: {item['subject']}")
        if len(items) > max_items:
            preview_lines.append(f"- ... and {len(items) - max_items} more broker updates")
        output.append(
            {
                "alert_key": f"group:{execution_order_id}:{items[-1]['alert_key']}",
                "summary_kind": "alert_broker_grouped",
                "severity": highest["severity"],
                "scope_key": f"alert_broker_grouped:order:{execution_order_id}",
                "execution_order_id": execution_order_id,
                "subject": f"Broker updates for order {execution_order_id}",
                "message_text": "\n".join(
                    [
                        f"Broker updates for execution order {execution_order_id}",
                        "",
                        f"Grouped events: {len(items)}",
                        f"Highest severity: {highest['severity']}",
                        "",
                        *preview_lines,
                    ]
                ),
                "payload": {
                    "alert_type": "broker_grouped",
                    "execution_order_id": execution_order_id,
                    "grouped_items": [
                        {
                            "alert_key": item["alert_key"],
                            "summary_kind": item["summary_kind"],
                            "severity": item["severity"],
                            "subject": item["subject"],
                            "payload": item["payload"],
                        }
                        for item in items
                    ],
                },
            }
        )
    output.sort(key=lambda item: item["alert_key"])
    return sorted([*non_order_alerts, *output], key=lambda item: item["alert_key"])


def _pending_broker_alerts(connection, config: dict[str, Any], limit: int = 25) -> list[dict[str, Any]]:
    candidates = _build_broker_alert_candidates(connection, config, limit=limit)
    grouped = _group_broker_alert_candidates(config, candidates)
    return grouped[:limit]


def _summary_due(config: dict[str, Any], summary_kind: str, local_now: datetime, *, force: bool) -> bool:
    if force:
        return True
    notifications_cfg = config.get("notifications", {})
    if summary_kind == "daily":
        return bool(notifications_cfg.get("daily_summary_enabled", False))
    if summary_kind == "weekly":
        return bool(notifications_cfg.get("weekly_summary_enabled", False)) and local_now.weekday() == int(
            notifications_cfg.get("weekly_dispatch_weekday_local", 0)
        )
    if summary_kind == "monthly":
        return bool(notifications_cfg.get("monthly_summary_enabled", False)) and local_now.day == int(
            notifications_cfg.get("monthly_dispatch_day_local", 1)
        )
    if summary_kind == "quarterly":
        return (
            bool(notifications_cfg.get("quarterly_summary_enabled", False))
            and local_now.day == int(notifications_cfg.get("quarterly_dispatch_day_local", 1))
            and local_now.month in {1, 4, 7, 10}
        )
    if summary_kind == "ytd":
        return bool(notifications_cfg.get("ytd_summary_enabled", False)) and local_now.day == int(
            notifications_cfg.get("ytd_dispatch_day_local", 1)
        )
    return False


def dispatch_summary_if_due(
    connection,
    config: dict[str, Any],
    *,
    summary_kind: str = "daily",
    reference_time: datetime | None = None,
    force: bool = False,
) -> dict[str, Any]:
    notifications_cfg = config.get("notifications", {})
    local_now = _notification_now(config, reference_time)
    if not _summary_due(config, summary_kind, local_now, force=force):
        return {"status": "disabled" if summary_kind == "daily" and not notifications_cfg.get("daily_summary_enabled", False) and not force else "not_due", "sent": [], "summary_kind": summary_kind}

    dispatch_time = local_now.replace(
        hour=int(notifications_cfg.get("dispatch_hour_local", 18)),
        minute=int(notifications_cfg.get("dispatch_minute_local", 15)),
        second=0,
        microsecond=0,
    )
    if not force and local_now < dispatch_time:
        return {"status": "not_due", "sent": [], "summary_kind": summary_kind}

    summary = build_summary(connection, config, summary_kind=summary_kind, reference_time=reference_time)
    now_utc = (reference_time or datetime.now(UTC)).astimezone(UTC)
    channels: list[str] = []
    if notifications_cfg.get("slack", {}).get("enabled"):
        channels.append("slack")
    if notifications_cfg.get("email", {}).get("enabled"):
        channels.append("email")
    if not channels:
        channels.append("audit_log")

    sent: list[dict[str, Any]] = []
    for channel in channels:
        is_ready, reason = _channel_ready(
            connection,
            config,
            summary_kind=summary_kind,
            channel=channel,
            summary_date=summary["summary_date"],
            reference_time=now_utc,
            force=force,
        )
        if not is_ready:
            sent.append({"channel": channel, "status": "skipped", "reason": reason})
            continue
        previous_state = _notification_state(connection, summary_kind, channel) or {}
        attempt_count = int(previous_state.get("attempt_count") or 0) + 1
        try:
            if channel == "slack":
                delivery_meta = _send_slack(
                    config,
                    summary["subject"],
                    summary["message_text"],
                    summary["payload"],
                    summary_kind=summary_kind,
                )
            elif channel == "email":
                delivery_meta = _send_email(config, summary["subject"], summary["message_text"], summary_kind=summary_kind)
            else:
                delivery_meta = {"status": "stored_only"}
            delivery_id = _record_notification_delivery(
                connection,
                summary_date=summary["summary_date"],
                summary_kind=summary_kind,
                channel=channel,
                status="sent",
                subject=summary["subject"],
                message_text=summary["message_text"],
                payload={**summary["payload"], "delivery_meta": delivery_meta},
            )
            _upsert_notification_state(
                connection,
                summary_kind=summary_kind,
                channel=channel,
                summary_date=summary["summary_date"],
                last_attempt_at=now_utc.isoformat(timespec="seconds"),
                next_attempt_after=None,
                attempt_count=attempt_count,
                last_status="sent",
                last_error_text=None,
            )
            append_audit_log(
                connection,
                "daily_summary_sent",
                {"summary_date": summary["summary_date"], "summary_kind": summary_kind, "channel": channel, "delivery_id": delivery_id},
            )
            sent.append({"channel": channel, "status": "sent", "delivery_id": delivery_id})
        except Exception as exc:  # noqa: BLE001
            delivery_id = _record_notification_delivery(
                connection,
                summary_date=summary["summary_date"],
                summary_kind=summary_kind,
                channel=channel,
                status="failed",
                subject=summary["subject"],
                message_text=summary["message_text"],
                payload=summary["payload"],
                error_text=str(exc),
            )
            next_attempt = now_utc + timedelta(minutes=int(config.get("notifications", {}).get("retry_backoff_minutes", 30)))
            _upsert_notification_state(
                connection,
                summary_kind=summary_kind,
                channel=channel,
                summary_date=summary["summary_date"],
                last_attempt_at=now_utc.isoformat(timespec="seconds"),
                next_attempt_after=next_attempt.isoformat(timespec="seconds"),
                attempt_count=attempt_count,
                last_status="failed",
                last_error_text=str(exc),
            )
            append_audit_log(
                connection,
                "daily_summary_failed",
                {
                    "summary_date": summary["summary_date"],
                    "summary_kind": summary_kind,
                    "channel": channel,
                    "delivery_id": delivery_id,
                    "error": str(exc),
                },
            )
            sent.append({"channel": channel, "status": "failed", "delivery_id": delivery_id, "error": str(exc)})

    return {"status": "ok", "sent": sent, "summary": summary, "summary_kind": summary_kind}


def dispatch_summaries_if_due(
    connection,
    config: dict[str, Any],
    reference_time: datetime | None = None,
    *,
    force: bool = False,
) -> dict[str, Any]:
    results = []
    for summary_kind in ("daily", "weekly", "monthly", "quarterly", "ytd"):
        results.append(
            dispatch_summary_if_due(
                connection,
                config,
                summary_kind=summary_kind,
                reference_time=reference_time,
                force=force,
            )
        )
    return {"status": "ok", "results": results}


def dispatch_broker_alerts_if_due(
    connection,
    config: dict[str, Any],
    reference_time: datetime | None = None,
    *,
    force: bool = False,
    limit: int = 25,
) -> dict[str, Any]:
    if not _alerts_enabled(config) and not force:
        return {"status": "disabled", "sent": [], "alerts": []}

    now_utc = (reference_time or datetime.now(UTC)).astimezone(UTC)
    channels: list[str] = []
    notifications_cfg = config.get("notifications", {})
    if notifications_cfg.get("slack", {}).get("enabled"):
        channels.append("slack")
    if notifications_cfg.get("email", {}).get("enabled"):
        channels.append("email")
    if not channels:
        channels.append("audit_log")

    pending_alerts = _pending_broker_alerts(connection, config, limit=limit)
    sent: list[dict[str, Any]] = []
    for alert in pending_alerts:
        suppressed_reason = _suppressed_alert_reason(
            connection,
            config,
            alert=alert,
            reference_time=now_utc,
            force=force,
        )
        if suppressed_reason:
            for channel in channels:
                sent.append(
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "severity": alert["severity"],
                        "channel": channel,
                        "status": "skipped",
                        "reason": suppressed_reason,
                    }
                )
            continue
        for channel in channels:
            is_ready, reason = _channel_ready(
                connection,
                config,
                summary_kind=alert["summary_kind"],
                channel=channel,
                summary_date=alert["alert_key"],
                reference_time=now_utc,
                force=force,
            )
            if not is_ready:
                sent.append(
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "channel": channel,
                        "status": "skipped",
                        "reason": reason,
                    }
                )
                continue
            previous_state = _notification_state(connection, alert["summary_kind"], channel) or {}
            attempt_count = int(previous_state.get("attempt_count") or 0) + 1
            formatted_subject, formatted_message = _format_delivery_content(
                config,
                alert["summary_kind"],
                alert["subject"],
                alert["message_text"],
            )
            try:
                if channel == "slack":
                    delivery_meta = _send_slack(
                        config,
                        formatted_subject,
                        formatted_message,
                        alert["payload"],
                        summary_kind=alert["summary_kind"],
                    )
                elif channel == "email":
                    delivery_meta = _send_email(
                        config,
                        formatted_subject,
                        formatted_message,
                        summary_kind=alert["summary_kind"],
                    )
                else:
                    delivery_meta = {"status": "stored_only"}
                delivery_id = _record_notification_delivery(
                    connection,
                    summary_date=alert["alert_key"],
                    summary_kind=alert["summary_kind"],
                    channel=channel,
                    status="sent",
                    subject=formatted_subject,
                    message_text=formatted_message,
                    payload={**alert["payload"], "delivery_meta": delivery_meta},
                )
                _upsert_notification_state(
                    connection,
                    summary_kind=alert["summary_kind"],
                    channel=channel,
                    summary_date=alert["alert_key"],
                    last_attempt_at=now_utc.isoformat(timespec="seconds"),
                    next_attempt_after=None,
                    attempt_count=attempt_count,
                    last_status="sent",
                    last_error_text=None,
                )
                append_audit_log(
                    connection,
                    "broker_alert_sent",
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "severity": alert["severity"],
                        "channel": channel,
                        "delivery_id": delivery_id,
                    },
                )
                _upsert_alert_state(
                    connection,
                    scope_key=alert["scope_key"],
                    severity=alert["severity"],
                    last_sent_at=now_utc.isoformat(timespec="seconds"),
                    last_alert_key=alert["alert_key"],
                    last_summary_kind=alert["summary_kind"],
                    last_delivery_id=delivery_id,
                )
                sent.append(
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "severity": alert["severity"],
                        "channel": channel,
                        "status": "sent",
                        "delivery_id": delivery_id,
                    }
                )
            except Exception as exc:  # noqa: BLE001
                delivery_id = _record_notification_delivery(
                    connection,
                    summary_date=alert["alert_key"],
                    summary_kind=alert["summary_kind"],
                    channel=channel,
                    status="failed",
                    subject=formatted_subject,
                    message_text=formatted_message,
                    payload=alert["payload"],
                    error_text=str(exc),
                )
                next_attempt = now_utc + timedelta(minutes=int(config.get("notifications", {}).get("retry_backoff_minutes", 30)))
                _upsert_notification_state(
                    connection,
                    summary_kind=alert["summary_kind"],
                    channel=channel,
                    summary_date=alert["alert_key"],
                    last_attempt_at=now_utc.isoformat(timespec="seconds"),
                    next_attempt_after=next_attempt.isoformat(timespec="seconds"),
                    attempt_count=attempt_count,
                    last_status="failed",
                    last_error_text=str(exc),
                )
                append_audit_log(
                    connection,
                    "broker_alert_failed",
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "severity": alert["severity"],
                        "channel": channel,
                        "delivery_id": delivery_id,
                        "error": str(exc),
                    },
                )
                sent.append(
                    {
                        "alert_key": alert["alert_key"],
                        "summary_kind": alert["summary_kind"],
                        "channel": channel,
                        "status": "failed",
                        "delivery_id": delivery_id,
                        "error": str(exc),
                    }
                )
    return {"status": "ok", "alerts": pending_alerts, "sent": sent}


def dispatch_daily_summary_if_due(
    connection,
    config: dict[str, Any],
    reference_time: datetime | None = None,
    *,
    force: bool = False,
) -> dict[str, Any]:
    return dispatch_summary_if_due(
        connection,
        config,
        summary_kind="daily",
        reference_time=reference_time,
        force=force,
    )
