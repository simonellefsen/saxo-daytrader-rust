from __future__ import annotations

import json
from calendar import monthrange
from datetime import UTC, datetime, time
from typing import Any
from zoneinfo import ZoneInfo

import requests

from saxo_daytrader_xai.market_benchmarks import fetch_benchmark_index_snapshot
from saxo_daytrader_xai.portfolio import fetch_goal_tracking
from saxo_daytrader_xai.xai_request import apply_response_options, timeout_seconds


DIARY_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "executive_summary": {"type": "string"},
        "what_went_well": {"type": "array", "items": {"type": "string"}},
        "what_went_wrong": {"type": "array", "items": {"type": "string"}},
        "missed_opportunities": {"type": "array", "items": {"type": "string"}},
        "risk_notes": {"type": "array", "items": {"type": "string"}},
        "benchmark_readthrough": {"type": "string"},
        "next_session_adjustments": {"type": "array", "items": {"type": "string"}},
        "decision_report_instructions": {"type": "array", "items": {"type": "string"}},
    },
    "required": [
        "executive_summary",
        "what_went_well",
        "what_went_wrong",
        "missed_opportunities",
        "risk_notes",
        "benchmark_readthrough",
        "next_session_adjustments",
        "decision_report_instructions",
    ],
    "additionalProperties": False,
}

DIARY_SYSTEM_PROMPT = "You are the trading diary reviewer. Return strict JSON only."

DIARY_INSTRUCTION = (
    "Write an end-of-day trading diary for the operator and for future decision reports. "
    "Be specific about what worked, what failed, whether trades aligned with the Decision Reports, "
    "how the portfolio performed versus UK/EU/Nordic/US benchmark indices, and what the next "
    "Decision Report should remember. Do not invent trades that are not in the metrics payload. "
    "If portfolio_valuation_warnings says a period has no fresh valuation, state that limitation "
    "directly and do not describe that stale value as a gain or loss for the journal date."
)


def _journal_cfg(config: dict[str, Any]) -> dict[str, Any]:
    return config.get("strategy", {}).get("swing", {}).get("journal", {})


def _timezone(config: dict[str, Any]) -> ZoneInfo:
    return ZoneInfo(str(_journal_cfg(config).get("timezone") or "Europe/Copenhagen"))


def _parse_time(value: Any, default: str) -> time:
    raw = str(value or default)
    hour_text, minute_text = raw.split(":", 1)
    return time(hour=int(hour_text), minute=int(minute_text))


def _journal_exists(connection, *, journal_date: str, cadence: str) -> bool:
    row = connection.execute(
        """
        SELECT id
        FROM strategy_journal_entries
        WHERE journal_date = ?
          AND cadence = ?
        LIMIT 1
        """,
        (journal_date, cadence),
    ).fetchone()
    return row is not None


def fetch_recent_journal_learnings(connection, limit: int = 6) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM strategy_journal_entries
        ORDER BY journal_date DESC, id DESC
        LIMIT ?
        """,
        (int(limit),),
    ).fetchall()
    output: list[dict[str, Any]] = []
    for row in rows:
        item = dict(row)
        item["metrics_json"] = json.loads(item["metrics_json"]) if item.get("metrics_json") else {}
        item["learnings_json"] = json.loads(item["learnings_json"]) if item.get("learnings_json") else []
        item["diary_json"] = json.loads(item["diary_json"]) if item.get("diary_json") else None
        output.append(item)
    return output


def fetch_strategy_journal_entries(connection, limit: int = 20) -> list[dict[str, Any]]:
    rows = connection.execute(
        """
        SELECT *
        FROM strategy_journal_entries
        ORDER BY journal_date DESC, id DESC
        LIMIT ?
        """,
        (int(limit),),
    ).fetchall()
    output: list[dict[str, Any]] = []
    for row in rows:
        item = dict(row)
        item["metrics_json"] = json.loads(item["metrics_json"]) if item.get("metrics_json") else {}
        item["learnings_json"] = json.loads(item["learnings_json"]) if item.get("learnings_json") else []
        item["diary_json"] = json.loads(item["diary_json"]) if item.get("diary_json") else None
        output.append(item)
    return output


def _decision_metrics(connection, config: dict[str, Any], *, since_date: str, reference_time: datetime | None = None) -> dict[str, Any]:
    report_rows = connection.execute(
        """
        SELECT id, created_at, status, analysis_pulse_key, analysis_pulse_label, report_json
        FROM decision_reports
        WHERE report_date >= ?
        ORDER BY id DESC
        LIMIT 20
        """,
        (since_date,),
    ).fetchall()
    suggested_trades = 0
    swing_orders = 0
    strategy_statuses: list[str] = []
    report_summaries: list[dict[str, Any]] = []
    source_report_id = None
    for row in report_rows:
        source_report_id = source_report_id or int(row["id"])
        report_json = json.loads(row["report_json"]) if row.get("report_json") else {}
        suggested_trades += len(report_json.get("suggested_trades") or [])
        strategy_plan = dict(report_json.get("strategy_plan") or {})
        swing_orders += len(strategy_plan.get("swing_orders") or [])
        if strategy_plan.get("status"):
            strategy_statuses.append(str(strategy_plan["status"]))
        report_summaries.append(
            {
                "id": int(row["id"]),
                "created_at": row.get("created_at"),
                "status": row.get("status"),
                "pulse_key": row.get("analysis_pulse_key"),
                "pulse_label": row.get("analysis_pulse_label"),
                "suggested_trade_count": len(report_json.get("suggested_trades") or []),
                "swing_order_count": len(strategy_plan.get("swing_orders") or []),
                "strategy_status": strategy_plan.get("status"),
                "market_regime": report_json.get("market_regime"),
                "portfolio_assessment": report_json.get("portfolio_assessment"),
                "execution_notes": report_json.get("execution_notes"),
            }
        )
    execution_rows = connection.execute(
        """
        SELECT status, COUNT(*) AS count
        FROM execution_orders
        WHERE created_at >= ?
        GROUP BY status
        """,
        (since_date,),
    ).fetchall()
    ledger_row = connection.execute(
        """
        SELECT
            COUNT(*) AS trade_count,
            COALESCE(SUM(realised_gain_dkk), 0) AS realised_gain_dkk
        FROM trade_ledger
        WHERE created_at >= ?
        """,
        (since_date,),
    ).fetchone()
    trade_rows = connection.execute(
        """
        SELECT id, created_at, symbol, side, quantity, price_local, currency,
               gross_amount_dkk, commission_dkk, tax_dkk, net_amount_dkk,
               mode, status, notes
        FROM trade_ledger
        WHERE created_at >= ?
        ORDER BY created_at DESC, id DESC
        LIMIT 50
        """,
        (since_date,),
    ).fetchall()
    order_rows = connection.execute(
        """
        SELECT id, created_at, report_id, symbol, action, order_type, mode, status,
               quantity, price_local, currency, estimated_value_dkk, strategy_type,
               strategy_session, strategy_role, error_text
        FROM execution_orders
        WHERE created_at >= ?
        ORDER BY created_at DESC, id DESC
        LIMIT 80
        """,
        (since_date,),
    ).fetchall()
    manager_rows = connection.execute(
        """
        SELECT id, created_at, manager_key, manager_label, report_id, status,
               open_exchange_codes_json, manager_json, error_text
        FROM trading_manager_runs
        WHERE created_at >= ?
        ORDER BY created_at DESC, id DESC
        LIMIT 20
        """,
        (since_date,),
    ).fetchall()
    goal_tracking = fetch_goal_tracking(connection, config, reference_time=reference_time)
    portfolio_valuation_warnings = _portfolio_valuation_warnings(goal_tracking, journal_date=since_date)
    benchmark_indices = fetch_benchmark_index_snapshot(
        config,
        timeout_seconds=int(config.get("market_data", {}).get("request_timeout_seconds", 10) or 10),
    )
    return {
        "report_count": len(report_rows),
        "suggested_trade_count": suggested_trades,
        "swing_order_count": swing_orders,
        "strategy_statuses": strategy_statuses[:5],
        "decision_reports": report_summaries[:10],
        "execution_status_counts": {str(row["status"]): int(row["count"]) for row in execution_rows},
        "trade_count": int(ledger_row["trade_count"] if ledger_row else 0),
        "realised_gain_dkk": float(ledger_row["realised_gain_dkk"] if ledger_row else 0.0),
        "trades": [dict(row) for row in trade_rows],
        "execution_orders": [dict(row) for row in order_rows],
        "trading_manager_runs": [
            {
                **dict(row),
                "open_exchange_codes": json.loads(row["open_exchange_codes_json"]) if row.get("open_exchange_codes_json") else [],
                "manager": json.loads(row["manager_json"]) if row.get("manager_json") else {},
            }
            for row in manager_rows
        ],
        "goal_tracking": goal_tracking,
        "portfolio_valuation_warnings": portfolio_valuation_warnings,
        "benchmark_indices": benchmark_indices,
        "source_report_id": source_report_id,
    }


def _portfolio_valuation_warnings(goal_tracking: dict[str, Any], *, journal_date: str) -> list[str]:
    periods = (goal_tracking or {}).get("periods") or {}
    day = periods.get("day") or {}
    warnings: list[str] = []
    if not bool(day.get("available")):
        current_value = float(day.get("current_value_dkk") or 0.0)
        valuation_at = day.get("current_valuation_at")
        if current_value > 0 and valuation_at:
            warnings.append(
                f"No fresh portfolio valuation was recorded for {journal_date}; "
                f"the latest available valuation is {current_value:.2f} DKK from {valuation_at}. "
                "Do not describe this as portfolio performance for the journal date."
            )
        else:
            warnings.append(
                f"No portfolio valuation was recorded for {journal_date}; "
                "do not describe portfolio performance for the journal date."
            )
    return warnings


def _learning_points(metrics: dict[str, Any]) -> list[str]:
    learnings: list[str] = []
    learnings.extend(str(item) for item in metrics.get("portfolio_valuation_warnings") or [])
    if metrics["report_count"] == 0:
        learnings.append("No decision reports were available for this journal period; keep the next analysis conservative.")
    if metrics["suggested_trade_count"] == 0:
        learnings.append("No high-conviction trades were suggested; market or portfolio constraints likely dominated.")
    if metrics["swing_order_count"] > 0:
        learnings.append("Review each swing order against daily confluence count and tomorrow-morning ownership quality.")
    if metrics["trade_count"] == 0:
        learnings.append("No closed trades were available for expectancy scoring; defer performance conclusions.")
    elif metrics["realised_gain_dkk"] < 0:
        learnings.append("Closed trades were net negative; inspect whether stop discipline or entry confluence failed.")
    else:
        learnings.append("Closed trades were non-negative; preserve the setup tags that worked.")
    goal_week = (metrics.get("goal_tracking") or {}).get("periods", {}).get("week", {})
    if goal_week:
        learnings.append(
            f"Weekly goal progress is {float(goal_week.get('pnl_dkk') or 0.0):.0f} DKK "
            f"versus {float(goal_week.get('target_dkk') or 0.0):.0f} DKK target-to-date."
        )
    benchmarks = (metrics.get("benchmark_indices") or {}).get("regions", {})
    if benchmarks:
        strongest = sorted(
            (
                (region, payload.get("average_change_pct"))
                for region, payload in benchmarks.items()
                if payload.get("average_change_pct") is not None
            ),
            key=lambda item: float(item[1]),
            reverse=True,
        )
        if strongest:
            learnings.append(
                f"Benchmark context: strongest region was {strongest[0][0]} "
                f"at {float(strongest[0][1]) * 100:.2f}% average index move."
            )
    return learnings


def _extract_output_text(response_json: dict[str, Any]) -> str:
    for item in response_json.get("output", []):
        if item.get("type") != "message":
            continue
        for content in item.get("content", []):
            if content.get("type") == "output_text":
                return str(content.get("text") or "")
    return ""


def _request_xai_diary(config: dict[str, Any], *, cadence: str, metrics: dict[str, Any], fallback_learnings: list[str]) -> dict[str, Any]:
    api_key = config.get("xai", {}).get("api_key")
    if not api_key:
        raise ValueError("XAI_API_KEY is missing")
    prompt = {
        "cadence": cadence,
        "performance_metrics": metrics,
        "deterministic_learnings": fallback_learnings,
        "instruction": DIARY_INSTRUCTION,
    }
    request_json = {
        "model": config["xai"]["model"],
        "input": [
            {
                "role": "system",
                "content": DIARY_SYSTEM_PROMPT,
            },
            {"role": "user", "content": json.dumps(prompt, ensure_ascii=False, indent=2, default=str)},
        ],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "strategy_diary",
                "schema": DIARY_SCHEMA,
                "strict": True,
            }
        },
    }
    request_json = apply_response_options(request_json, config)
    response = requests.post(
        f"{config['xai']['base_url'].rstrip('/')}/responses",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        json=request_json,
        timeout=timeout_seconds(config),
    )
    response.raise_for_status()
    response_json = response.json()
    output_text = _extract_output_text(response_json)
    if not output_text:
        raise ValueError("xAI diary response did not contain structured output text")
    return {
        "status": "xai_completed",
        "response_id": response_json.get("id"),
        "diary": json.loads(output_text),
    }


def build_diary_prompt_preview(config: dict[str, Any]) -> dict[str, Any]:
    prompt = {
        "cadence": "daily",
        "performance_metrics": {
            "preview": "Live end-of-day runs include realised trades, broker fills, decision reports, trading manager runs, goal tracking, and benchmark index moves.",
        },
        "deterministic_learnings": [],
        "instruction": DIARY_INSTRUCTION,
    }
    return {
        "kind": "eod_diary",
        "title": "End-of-Day Diary",
        "description": "Prompt used after US close to turn trading performance and benchmark context into lessons for future Decision Reports.",
        "system_prompt": DIARY_SYSTEM_PROMPT,
        "instruction": DIARY_INSTRUCTION,
        "user_prompt": json.dumps(prompt, ensure_ascii=False, indent=2, default=str),
        "schema": DIARY_SCHEMA,
        "model": config.get("xai", {}).get("model"),
    }


def _fallback_diary(*, cadence: str, metrics: dict[str, Any], learnings: list[str], error: str | None = None) -> dict[str, Any]:
    benchmarks = (metrics.get("benchmark_indices") or {}).get("regions", {})
    benchmark_summary = ", ".join(
        f"{region} {float(payload.get('average_change_pct') or 0.0) * 100:.2f}%"
        for region, payload in benchmarks.items()
    ) or "No benchmark data was available."
    valuation_warnings = [str(item) for item in metrics.get("portfolio_valuation_warnings") or [] if str(item).strip()]
    executive_bits = [
        f"{cadence.title()} diary: {metrics.get('report_count', 0)} report(s)",
        f"{metrics.get('suggested_trade_count', 0)} suggested trade(s)",
        f"{metrics.get('trade_count', 0)} closed trade(s)",
        f"{float(metrics.get('realised_gain_dkk') or 0.0):.0f} DKK realised gain",
    ]
    if valuation_warnings:
        executive_bits.append(valuation_warnings[0])
    return {
        "status": "deterministic_fallback" if error else "deterministic",
        "error": error,
        "diary": {
            "executive_summary": ". ".join(executive_bits) + ".",
            "what_went_well": [item for item in learnings if "non-negative" in item or "preserve" in item] or learnings[:1],
            "what_went_wrong": [item for item in learnings if "negative" in item or "No " in item] or [],
            "missed_opportunities": [],
            "risk_notes": [*valuation_warnings, *[item for item in learnings if "goal progress" in item]],
            "benchmark_readthrough": benchmark_summary,
            "next_session_adjustments": learnings,
            "decision_report_instructions": learnings,
        },
    }


def record_strategy_journal_entry(
    connection,
    *,
    journal_date: str,
    cadence: str,
    status: str,
    summary: str,
    metrics: dict[str, Any],
    learnings: list[str],
    diary: dict[str, Any],
    source_report_id: int | None,
) -> int:
    cursor = connection.execute(
        """
        INSERT INTO strategy_journal_entries (
            created_at, journal_date, cadence, status, summary,
            metrics_json, learnings_json, diary_json, source_report_id
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            datetime.now(UTC).isoformat(timespec="seconds"),
            journal_date,
            cadence,
            status,
            summary,
            json.dumps(metrics, ensure_ascii=False, sort_keys=True),
            json.dumps(learnings, ensure_ascii=False, sort_keys=True),
            json.dumps(diary, ensure_ascii=False, sort_keys=True),
            source_report_id,
        ),
    )
    connection.commit()
    return int(cursor.lastrowid)


def generate_strategy_journal_entry(
    connection,
    *,
    config: dict[str, Any],
    cadence: str,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    now = (reference_time or datetime.now(UTC)).astimezone(_timezone(config))
    journal_date = now.date().isoformat()
    if _journal_exists(connection, journal_date=journal_date, cadence=cadence):
        return {"status": "skipped", "reason": f"{cadence} journal already exists for {journal_date}"}
    metrics = _decision_metrics(connection, config, since_date=journal_date, reference_time=now)
    learnings = _learning_points(metrics)
    try:
        diary_result = _request_xai_diary(config, cadence=cadence, metrics=metrics, fallback_learnings=learnings)
    except Exception as exc:  # noqa: BLE001
        diary_result = _fallback_diary(cadence=cadence, metrics=metrics, learnings=learnings, error=str(exc))
    diary = dict(diary_result.get("diary") or {})
    diary_instructions = [
        str(item)
        for item in diary.get("decision_report_instructions", [])
        if str(item).strip()
    ]
    merged_learnings = [*learnings]
    for item in diary_instructions:
        if item not in merged_learnings:
            merged_learnings.append(item)
    metrics["diary_status"] = diary_result.get("status")
    if diary_result.get("response_id"):
        metrics["diary_response_id"] = diary_result.get("response_id")
    if diary_result.get("error"):
        metrics["diary_error"] = diary_result.get("error")
    week = metrics.get("goal_tracking", {}).get("periods", {}).get("week", {})
    month = metrics.get("goal_tracking", {}).get("periods", {}).get("month", {})
    summary = (
        f"{cadence.title()} strategy journal: {metrics['report_count']} report(s), "
        f"{metrics['suggested_trade_count']} suggested trade(s), "
        f"{metrics['swing_order_count']} swing order(s), "
        f"{metrics['trade_count']} closed trade(s). "
        f"Week {float(week.get('pnl_dkk') or 0.0):.0f}/{float(week.get('target_dkk') or 0.0):.0f} DKK, "
        f"month {float(month.get('pnl_dkk') or 0.0):.0f}/{float(month.get('target_dkk') or 0.0):.0f} DKK before tax."
    )
    entry_id = record_strategy_journal_entry(
        connection,
        journal_date=journal_date,
        cadence=cadence,
        status="completed",
        summary=summary,
        metrics=metrics,
        learnings=merged_learnings,
        diary=diary_result,
        source_report_id=metrics.get("source_report_id"),
    )
    return {"status": "completed", "id": entry_id, "cadence": cadence, "journal_date": journal_date}


def generate_due_strategy_journals(
    connection,
    config: dict[str, Any],
    *,
    reference_time: datetime | None = None,
) -> dict[str, Any]:
    cfg = _journal_cfg(config)
    if not bool(cfg.get("enabled", True)):
        return {"status": "disabled", "entries": []}
    now = (reference_time or datetime.now(UTC)).astimezone(_timezone(config))
    entries: list[dict[str, Any]] = []
    daily_time = _parse_time(cfg.get("daily_time"), "22:30")
    if now.time() >= daily_time:
        entries.append(generate_strategy_journal_entry(connection, config=config, cadence="daily", reference_time=now))
    weekly_time = _parse_time(cfg.get("weekly_time"), "20:00")
    weekly_day = int(cfg.get("weekly_weekday", 6) or 6)
    if now.weekday() == weekly_day and now.time() >= weekly_time:
        entries.append(generate_strategy_journal_entry(connection, config=config, cadence="weekly", reference_time=now))
    monthly_time = _parse_time(cfg.get("monthly_time"), "22:45")
    if now.day == monthrange(now.year, now.month)[1] and now.time() >= monthly_time:
        entries.append(generate_strategy_journal_entry(connection, config=config, cadence="monthly", reference_time=now))
    return {"status": "ok", "entries": entries}
