use serde_json::Value as JsonValue;

use crate::models::{DashboardStrategyJournalEntryPayload, StrategyJournalEntryPayload};

/// Decodes the stable outer fields of retained local EOD journals. Detailed
/// metrics, learnings, and diary documents remain staged JSON for the EOD
/// detail view and its read-only benchmark context.
pub(crate) fn dashboard_strategy_journal_entries_from_json(
    entries: Vec<JsonValue>,
) -> serde_json::Result<Vec<DashboardStrategyJournalEntryPayload>> {
    entries
        .into_iter()
        .map(|entry| {
            Ok(DashboardStrategyJournalEntryPayload {
                id: required_i64(&entry, "id")?,
                created_at: required_string(&entry, "created_at")?,
                journal_date: required_string(&entry, "journal_date")?,
                cadence: required_string(&entry, "cadence")?,
                status: required_string(&entry, "status")?,
                summary: required_string(&entry, "summary")?,
                source_report_id: serde_json::from_value(
                    entry
                        .get("source_report_id")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
                metrics_json: entry
                    .get("metrics_json")
                    .cloned()
                    .unwrap_or(JsonValue::Null),
                learnings_json: entry
                    .get("learnings_json")
                    .cloned()
                    .unwrap_or(JsonValue::Null),
                diary_json: entry.get("diary_json").cloned().unwrap_or(JsonValue::Null),
            })
        })
        .collect()
}

/// Decodes the public, stable metadata-only journal list. Detailed retained
/// documents never cross this summary boundary.
pub(crate) fn strategy_journal_summaries_from_json(
    entries: Vec<JsonValue>,
) -> serde_json::Result<Vec<StrategyJournalEntryPayload>> {
    entries
        .into_iter()
        .map(|entry| {
            Ok(StrategyJournalEntryPayload {
                id: required_i64(&entry, "id")?,
                created_at: required_string(&entry, "created_at")?,
                journal_date: required_string(&entry, "journal_date")?,
                cadence: required_string(&entry, "cadence")?,
                status: required_string(&entry, "status")?,
                summary: required_string(&entry, "summary")?,
                source_report_id: serde_json::from_value(
                    entry
                        .get("source_report_id")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                )?,
            })
        })
        .collect()
}

fn required_string(row: &JsonValue, key: &str) -> serde_json::Result<String> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

fn required_i64(row: &JsonValue, key: &str) -> serde_json::Result<i64> {
    serde_json::from_value(row.get(key).cloned().unwrap_or(JsonValue::Null))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        dashboard_strategy_journal_entries_from_json, strategy_journal_summaries_from_json,
    };

    #[test]
    fn dashboard_entries_keep_only_staged_detail_documents() {
        let entries = dashboard_strategy_journal_entries_from_json(vec![json!({
            "id": 17,
            "created_at": "2026-08-26T15:30:00Z",
            "journal_date": "2026-08-26",
            "cadence": "daily",
            "status": "completed",
            "summary": "Bounded EOD summary.",
            "source_report_id": 42,
            "metrics_json": {"total_value_dkk": 250000.0},
            "learnings_json": {"theme": "observation"},
            "diary_json": {"diary": {"benchmark_readthrough": {"status": "ready"}}},
            "runtime_session": {"api_key": "must-not-reach-the-dashboard"}
        })])
        .expect("stable strategy-journal rows decode");

        assert_eq!(entries[0].source_report_id, Some(42));
        assert_eq!(entries[0].journal_date, "2026-08-26");
        assert!(
            !serde_json::to_string(&entries)
                .expect("typed strategy-journal rows serialize")
                .contains("must-not-reach-the-dashboard")
        );
        assert!(dashboard_strategy_journal_entries_from_json(vec![json!({"id": 17})]).is_err());
    }

    #[test]
    fn summaries_exclude_retained_detail_documents() {
        let entries = strategy_journal_summaries_from_json(vec![json!({
            "id": 17,
            "created_at": "2026-08-26T15:30:00Z",
            "journal_date": "2026-08-26",
            "cadence": "daily",
            "status": "completed",
            "summary": "Bounded EOD summary.",
            "source_report_id": 42,
            "metrics_json": {"total_value_dkk": 250000.0},
            "learnings_json": {"theme": "observation"},
            "diary_json": {"api_key": "must-not-reach-the-public-api"}
        })])
        .expect("stable strategy-journal summary rows decode");

        assert_eq!(entries[0].source_report_id, Some(42));
        assert!(
            !serde_json::to_string(&entries)
                .expect("typed strategy-journal summary rows serialize")
                .contains("must-not-reach-the-public-api")
        );
        assert!(strategy_journal_summaries_from_json(vec![json!({"id": 17})]).is_err());
    }

    /// An EOD entry written before its metrics exist carries nulls, and that must
    /// not fail the whole journal.
    #[test]
    fn an_explicit_null_never_blanks_the_strategy_journal() {
        crate::read_model::assert_null_is_never_worse_than_absent(
            &json!([{
                "id": 17,
                "created_at": "2026-08-26T15:30:00Z",
                "journal_date": "2026-08-26",
                "cadence": "daily",
                "status": "completed",
                "summary": "Bounded EOD summary.",
                "source_report_id": 42,
                "metrics_json": {"total_value_dkk": 250000.0},
                "learnings_json": {"theme": "observation"},
                "diary_json": {"diary": {"benchmark_readthrough": {"status": "ready"}}},
                "runtime_session": {"api_key": "must-not-reach-the-dashboard"}
            }]),
            |value| {
                dashboard_strategy_journal_entries_from_json(
                    value.as_array().cloned().expect("fixture is a list"),
                )
            },
        );
    }
}
