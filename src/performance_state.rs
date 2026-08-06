//! Read-only Performance dashboard projections.
//!
//! This module contains deterministic transformations of persisted account
//! value snapshots. It cannot query a provider, change trading settings, or
//! interact with Saxo.

use chrono::{DateTime, Utc};
use serde_json::{Value as JsonValue, json};

use crate::db::{value_f64, value_i64};

pub(crate) fn performance_summary_from_history(
    history: &[JsonValue],
    now: DateTime<Utc>,
) -> JsonValue {
    let first = history.first();
    let latest = history.last();
    let first_total = first
        .map(|row| value_f64(row, "total_market_value_dkk"))
        .unwrap_or(0.0);
    let latest_total = latest
        .map(|row| value_f64(row, "total_market_value_dkk"))
        .unwrap_or(0.0);
    let latest_daily = latest
        .map(|row| value_f64(row, "total_daily_pnl_dkk"))
        .unwrap_or(0.0);
    let latest_positions = latest
        .map(|row| value_i64(row, "position_count"))
        .unwrap_or(0);
    let (range_return_pct, range_max_drawdown_pct) = performance_range_metrics(history);
    json!({
        "points": history.len(),
        "first_recorded_at": first.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
        "latest_recorded_at": latest.and_then(|row| row.get("recorded_at")).cloned().unwrap_or(JsonValue::Null),
        "first_total_market_value_dkk": first_total,
        "latest_total_market_value_dkk": latest_total,
        "change_dkk": latest_total - first_total,
        "daily_pnl_dkk": latest_daily,
        "position_count": latest_positions,
        "range_return_pct": range_return_pct,
        "range_max_drawdown_pct": range_max_drawdown_pct,
        "confidence": performance_confidence(history, now),
        // Snapshots between 2026-06-03 and 2026-07-09 stored an arithmetically
        // impossible cost basis. It cannot be recomputed -- snapshots hold only
        // aggregates -- so a range covering it is marked rather than silently
        // plotted. Only cost-basis-derived figures are affected; market value,
        // which the drawdown guardrail reads, is sound throughout.
        "unreliable_cost_basis_points": history
            .iter()
            .filter(|row| {
                !crate::state::cost_basis_is_plausible(
                    value_f64(row, "total_cost_basis_dkk"),
                    value_f64(row, "invested_market_value_dkk"),
                )
            })
            .count(),
    })
}

pub(crate) fn performance_range_metrics(history: &[JsonValue]) -> (Option<f64>, Option<f64>) {
    let values = history
        .iter()
        .filter_map(|row| {
            row.get("total_market_value_dkk")
                .and_then(JsonValue::as_f64)
        })
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect::<Vec<_>>();
    let Some(start_value) = values.first().copied() else {
        return (None, None);
    };
    let Some(end_value) = values.last().copied() else {
        return (None, None);
    };
    if values.len() < 2 {
        return (None, None);
    }

    let mut peak = start_value;
    let mut max_drawdown_pct = 0.0_f64;
    for value in values {
        peak = peak.max(value);
        max_drawdown_pct = max_drawdown_pct.min((value / peak - 1.0) * 100.0);
    }
    (
        Some((end_value / start_value - 1.0) * 100.0),
        Some(max_drawdown_pct),
    )
}

/// Describes the evidence behind the account-value display without making any
/// claim about individual quote, benchmark, or broker-order freshness.
pub(crate) fn performance_confidence(history: &[JsonValue], now: DateTime<Utc>) -> JsonValue {
    let latest = history.last();
    let valid_points = history
        .iter()
        .filter(|row| {
            row.get("total_market_value_dkk")
                .and_then(JsonValue::as_f64)
                .is_some_and(|value| value.is_finite() && value > 0.0)
        })
        .count();
    let latest_value_valid = latest
        .and_then(|row| row.get("total_market_value_dkk"))
        .and_then(JsonValue::as_f64)
        .is_some_and(|value| value.is_finite() && value > 0.0);
    let latest_recorded_at = latest
        .and_then(|row| row.get("recorded_at"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let latest_snapshot_type = latest
        .and_then(|row| row.get("snapshot_type"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let latest_source = latest
        .and_then(|row| row.get("source"))
        .and_then(JsonValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let age_minutes = latest_recorded_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            now.signed_duration_since(value.with_timezone(&Utc))
                .num_minutes()
                .max(0)
        });
    let status = if !latest_value_valid {
        "unavailable"
    } else if valid_points < 2 {
        "partial"
    } else if latest_snapshot_type.as_deref() == Some("runtime_current") {
        "current"
    } else if age_minutes.is_none_or(|minutes| minutes > 90) {
        "stale"
    } else {
        "stored"
    };
    json!({
        "status": status,
        "valid_points": valid_points,
        "latest_recorded_at": latest_recorded_at,
        "latest_snapshot_type": latest_snapshot_type,
        "latest_source": latest_source,
        "age_minutes": age_minutes,
        "scope": "account_value_only",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_preserves_selected_range_and_current_evidence() {
        let now = DateTime::parse_from_rfc3339("2026-07-31T12:00:00Z")
            .expect("parses fixed timestamp")
            .with_timezone(&Utc);
        let summary = performance_summary_from_history(
            &[
                json!({"recorded_at": "2026-07-30T12:00:00Z", "total_market_value_dkk": 100.0}),
                json!({"recorded_at": "2026-07-31T12:00:00Z", "snapshot_type": "runtime_current", "total_market_value_dkk": 120.0, "total_daily_pnl_dkk": 2.0, "position_count": 3}),
            ],
            now,
        );
        assert_eq!(summary["points"], json!(2));
        assert_eq!(summary["change_dkk"], json!(20.0));
        assert_eq!(summary["confidence"]["status"], json!("current"));
    }
}
