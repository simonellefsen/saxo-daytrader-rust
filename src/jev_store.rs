//! Persistence for Jev requests and the signals they produce.
//!
//! Two tables, and the split between them is deliberate.
//!
//! `jev_requests` is the accounting record: one row per call, whether it
//! succeeded or failed, carrying tokens, latency, cost, and both model names.
//! It exists so Jev appears in the same usage ledger as the Decision Report
//! model rather than being an untracked second spend.
//!
//! `jev_editorial_signals` is the measurement record: one row per (item,
//! symbol) judged. Every judgement column is nullable, because an unanswered
//! question is absent rather than zero -- storing `0.0` for a question Jev
//! never answered would record a confident negative the model never gave.
//!
//! Neither table is read by anything that can create, size, or block an order.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use sqlx::{AnyPool, Row};

use crate::{db::row_to_json, jev::JevResponse, jev_signals::NewsSignal};

pub(crate) const PURPOSE_EDITORIAL_ITEM: &str = "editorial_item";
pub(crate) const PURPOSE_REPORT_GRADING: &str = "report_grading";
pub(crate) const PURPOSE_ERROR_CLASSIFICATION: &str = "error_classification";

/// Where an `instruction_shaped` probability starts counting as a flag.
///
/// Shared by the agreement count and the disagreement list so the two panels
/// can never describe the same item differently. It governs reporting only --
/// no value of this can admit or exclude an item from a prompt, which stays
/// the marker screen's decision alone.
pub(crate) const INSTRUCTION_SHAPED_THRESHOLD: f64 = 0.5;

pub(crate) const STATUS_COMPLETED: &str = "completed";
pub(crate) const STATUS_ERROR: &str = "error";

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        // `DOUBLE PRECISION` rather than `REAL`: a Jev call costs on the order
        // of 1e-6 USD, and float4 would start losing digits once a daily total
        // is accumulated across thousands of them.
        "CREATE TABLE IF NOT EXISTS jev_requests (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            purpose TEXT NOT NULL,
            subject TEXT,
            model_requested TEXT NOT NULL,
            model_resolved TEXT,
            question_count INTEGER NOT NULL DEFAULT 0,
            answer_count INTEGER NOT NULL DEFAULT 0,
            issue_count INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd DOUBLE PRECISION,
            cost_source TEXT NOT NULL,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            attempts INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL,
            error_text TEXT,
            result_json TEXT
        )",
        "CREATE TABLE IF NOT EXISTS jev_editorial_signals (
            item_id TEXT NOT NULL,
            symbol TEXT NOT NULL,
            created_at TEXT NOT NULL,
            request_id TEXT,
            about_symbol DOUBLE PRECISION,
            direction TEXT,
            direction_confidence DOUBLE PRECISION,
            bullish_probability DOUBLE PRECISION,
            bearish_probability DOUBLE PRECISION,
            materiality DOUBLE PRECISION,
            company_specific DOUBLE PRECISION,
            instruction_shaped DOUBLE PRECISION,
            restates_known DOUBLE PRECISION,
            signed_score DOUBLE PRECISION,
            marker_screened INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (item_id, symbol)
        )",
        "CREATE INDEX IF NOT EXISTS idx_jev_requests_created
         ON jev_requests(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_jev_editorial_signals_symbol
         ON jev_editorial_signals(symbol, created_at DESC)",
    ]
}

/// What every recorded call carries, whatever its outcome.
pub(crate) struct RecordedRequest<'a> {
    pub id: &'a str,
    pub created_at: &'a str,
    pub purpose: &'a str,
    /// What the call was about: an item id, a report id, an order id.
    pub subject: Option<&'a str>,
    /// The configured alias. The version that answered is read from the
    /// response and stored beside it.
    pub model_requested: &'a str,
    pub question_count: i64,
}

/// Records one completed call.
pub(crate) async fn record_success(
    pool: &AnyPool,
    meta: RecordedRequest<'_>,
    response: &JevResponse,
    result_json: Option<&JsonValue>,
) -> Result<()> {
    let mut result = result_json
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    result["measurement"] = response.measurement.clone();
    sqlx::query(
        "INSERT INTO jev_requests (
            id, created_at, purpose, subject, model_requested, model_resolved,
            question_count, answer_count, issue_count, input_tokens, output_tokens,
            cost_usd, cost_source, latency_ms, attempts, status, error_text, result_json
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, NULL, $17)",
    )
    .bind(meta.id)
    .bind(meta.created_at)
    .bind(meta.purpose)
    .bind(meta.subject)
    .bind(meta.model_requested)
    .bind(response.model_resolved.as_deref())
    .bind(meta.question_count)
    .bind(response.answers.len() as i64)
    .bind(response.issues.len() as i64)
    .bind(response.usage.input_tokens)
    .bind(response.usage.output_tokens)
    .bind(response.cost_usd)
    .bind(response.cost_source)
    .bind(response.latency_ms)
    .bind(response.attempts as i64)
    .bind(if response.issues.is_empty() { STATUS_COMPLETED } else { "partial" })
    .bind(result.to_string())
    .execute(pool)
    .await
    .context("recording a completed Jev request")?;
    Ok(())
}

/// Records one failed call.
///
/// A failure is a row, not a gap. Without it a Jev outage is invisible in the
/// ledger and indistinguishable from a quiet day with no items to judge.
pub(crate) async fn record_failure(
    pool: &AnyPool,
    meta: RecordedRequest<'_>,
    error_text: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO jev_requests (
            id, created_at, purpose, subject, model_requested, model_resolved,
            question_count, answer_count, issue_count, input_tokens, output_tokens,
            cost_usd, cost_source, latency_ms, attempts, status, error_text, result_json
         ) VALUES ($1, $2, $3, $4, $5, NULL, $6, 0, 0, 0, 0, NULL, $7, 0, 0, $8, $9, NULL)",
    )
    .bind(meta.id)
    .bind(meta.created_at)
    .bind(meta.purpose)
    .bind(meta.subject)
    .bind(meta.model_requested)
    .bind(meta.question_count)
    .bind(crate::jev::COST_SOURCE_NONE)
    .bind(STATUS_ERROR)
    .bind(truncate(error_text, 2_000))
    .execute(pool)
    .await
    .context("recording a failed Jev request")?;
    Ok(())
}

/// Upserts the judgement of one editorial item about one symbol.
///
/// `marker_screened` records whether the existing 31-marker substring screen
/// had already excluded this item, so the two screens can be compared without
/// either one having influenced the other.
pub(crate) async fn record_editorial_signal(
    pool: &AnyPool,
    item_id: &str,
    symbol: &str,
    created_at: &str,
    request_id: Option<&str>,
    signal: &NewsSignal,
    marker_screened: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO jev_editorial_signals (
            item_id, symbol, created_at, request_id, about_symbol, direction,
            direction_confidence, bullish_probability, bearish_probability, materiality,
            company_specific, instruction_shaped, restates_known, signed_score, marker_screened
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (item_id, symbol) DO UPDATE SET
           created_at = excluded.created_at, request_id = excluded.request_id,
           about_symbol = excluded.about_symbol, direction = excluded.direction,
           direction_confidence = excluded.direction_confidence,
           bullish_probability = excluded.bullish_probability,
           bearish_probability = excluded.bearish_probability, materiality = excluded.materiality,
           company_specific = excluded.company_specific, instruction_shaped = excluded.instruction_shaped,
           restates_known = excluded.restates_known, signed_score = excluded.signed_score,
           marker_screened = excluded.marker_screened",
    )
    .bind(item_id)
    .bind(symbol)
    .bind(created_at)
    .bind(request_id)
    .bind(signal.about_symbol)
    .bind(signal.direction.as_deref())
    .bind(signal.direction_confidence)
    .bind(signal.bullish_probability)
    .bind(signal.bearish_probability)
    .bind(signal.materiality)
    .bind(signal.company_specific)
    .bind(signal.instruction_shaped)
    .bind(signal.restates_known)
    .bind(signal.signed_score())
    .bind(i64::from(marker_screened))
    .execute(pool)
    .await
    .context("recording a Jev editorial signal")?;
    Ok(())
}

/// Rows for the shared LLM usage ledger, newest first.
pub(crate) async fn usage_rows(pool: &AnyPool, limit: i64) -> Result<Vec<JsonValue>> {
    let limit = limit.clamp(1, 5_000);
    Ok(sqlx::query(&format!(
        "SELECT created_at, purpose, model_requested, model_resolved, status,
                input_tokens, output_tokens, cost_usd, cost_source
         FROM jev_requests
         ORDER BY created_at DESC
         LIMIT {limit}"
    ))
    .fetch_all(pool)
    .await
    .context("reading Jev usage rows")?
    .iter()
    .map(row_to_json)
    .collect())
}

/// Where the 31-marker substring screen and Jev's judgement disagree.
///
/// Reported in both directions. A marker hit Jev reads as ordinary reporting
/// suggests the list is over-firing and quietly dropping real news; an item
/// the markers passed that Jev reads as instruction-shaped is the more
/// serious direction and is the reason this is measured at all.
pub(crate) async fn screening_disagreements(
    pool: &AnyPool,
    threshold: f64,
    limit: i64,
) -> Result<Vec<JsonValue>> {
    let limit = limit.clamp(1, 500);
    Ok(sqlx::query(&format!(
        "SELECT s.item_id, s.symbol, s.created_at, s.marker_screened, s.instruction_shaped,
                i.title, i.source_name
         FROM jev_editorial_signals s
         LEFT JOIN editorial_research_items i ON i.id = s.item_id
         WHERE s.instruction_shaped IS NOT NULL
           AND ((s.marker_screened = 1 AND s.instruction_shaped < $1)
             OR (s.marker_screened = 0 AND s.instruction_shaped >= $1))
         ORDER BY s.created_at DESC
         LIMIT {limit}"
    ))
    .bind(threshold)
    .fetch_all(pool)
    .await
    .context("reading Jev screening disagreements")?
    .iter()
    .map(row_to_json)
    .collect())
}

/// Signals recent enough to rank on, newest first.
pub(crate) async fn recent_signals(
    pool: &AnyPool,
    since: &str,
    limit: i64,
) -> Result<Vec<JsonValue>> {
    let limit = limit.clamp(1, 5_000);
    Ok(sqlx::query(&format!(
        "SELECT item_id, symbol, created_at, about_symbol, direction, direction_confidence,
                bullish_probability, bearish_probability, materiality, company_specific,
                instruction_shaped, restates_known, signed_score, marker_screened
         FROM jev_editorial_signals
         WHERE created_at >= $1
         ORDER BY created_at DESC
         LIMIT {limit}"
    ))
    .bind(since)
    .fetch_all(pool)
    .await
    .context("reading recent Jev editorial signals")?
    .iter()
    .map(row_to_json)
    .collect())
}

/// How often the marker screen and Jev agree, in both directions.
///
/// Counts only items Jev actually judged on this axis. An unjudged item is
/// not evidence of agreement and must not inflate the denominator.
pub(crate) async fn screening_agreement(pool: &AnyPool, threshold: f64) -> Result<JsonValue> {
    let rows = sqlx::query(
        "SELECT marker_screened, instruction_shaped
         FROM jev_editorial_signals
         WHERE instruction_shaped IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .context("reading Jev screening agreement")?
    .iter()
    .map(row_to_json)
    .collect::<Vec<_>>();

    let mut both_flagged = 0i64;
    let mut both_clear = 0i64;
    let mut marker_only = 0i64;
    let mut jev_only = 0i64;
    for row in &rows {
        let marker = row
            .get("marker_screened")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0)
            != 0;
        let jev = row
            .get("instruction_shaped")
            .and_then(JsonValue::as_f64)
            .is_some_and(|value| value >= threshold);
        match (marker, jev) {
            (true, true) => both_flagged += 1,
            (false, false) => both_clear += 1,
            (true, false) => marker_only += 1,
            (false, true) => jev_only += 1,
        }
    }
    let judged = rows.len() as i64;
    Ok(serde_json::json!({
        "judged_count": judged,
        "both_flagged": both_flagged,
        "both_clear": both_clear,
        "marker_only": marker_only,
        "jev_only": jev_only,
        "threshold": threshold,
        "marker_only_meaning": "the marker list fired where Jev reads ordinary reporting; \
                                if these are real news, the screen is dropping it silently",
        "jev_only_meaning": "the marker list passed text Jev reads as instruction-shaped; \
                             the direction that matters, and the reason this is measured",
        "admission": "observational_only",
    }))
}

/// Subjects already judged for a given purpose, so a sidecar pass never
/// re-spends on work it has done.
pub(crate) async fn judged_subjects(
    pool: &AnyPool,
    purpose: &str,
) -> Result<std::collections::HashSet<String>> {
    Ok(sqlx::query(
        "SELECT subject FROM jev_requests
         WHERE purpose = $1 AND status IN ('completed', 'partial') AND subject IS NOT NULL",
    )
    .bind(purpose)
    .fetch_all(pool)
    .await
    .context("reading judged Jev subjects")?
    .iter()
    .filter_map(|row| row.try_get::<String, _>("subject").ok())
    .collect())
}

/// Stored results for one purpose, newest first.
pub(crate) async fn results_for(
    pool: &AnyPool,
    purpose: &str,
    limit: i64,
) -> Result<Vec<JsonValue>> {
    let limit = limit.clamp(1, 500);
    Ok(sqlx::query(&format!(
        "SELECT created_at, subject, model_resolved, result_json
         FROM jev_requests
         WHERE purpose = $1 AND status IN ('completed', 'partial') AND result_json IS NOT NULL
         ORDER BY created_at DESC
         LIMIT {limit}"
    ))
    .bind(purpose)
    .fetch_all(pool)
    .await
    .context("reading Jev results")?
    .iter()
    .map(row_to_json)
    .collect())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::{JevResponse, JevUsage};
    use std::{collections::BTreeMap, sync::Once};

    static DRIVERS: Once = Once::new();

    #[tokio::test]
    async fn missing_jev_table_does_not_erase_decision_usage() {
        let pool = pool().await;
        sqlx::query("CREATE TABLE decision_reports (id INTEGER, created_at TEXT, model TEXT, status TEXT, request_json TEXT, response_json TEXT)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO decision_reports VALUES (1, '2026-09-20T12:00:00Z', 'model', 'completed', '{}', $1)")
            .bind(serde_json::json!({"usage":{"prompt_tokens":100,"cost":0.01}}).to_string())
            .execute(&pool).await.unwrap();
        sqlx::query("DROP TABLE jev_requests")
            .execute(&pool)
            .await
            .unwrap();
        let state = crate::state::AppState {
            config_path: std::path::PathBuf::from("test.yaml"),
            config: serde_yaml::Value::Null,
            db_url: "sqlite::memory:".into(),
            pool,
        };
        let ledger = state.llm_usage_ledger(10, 30).await.unwrap();
        assert_eq!(ledger.unavailable_sources, vec!["jev"]);
        assert_eq!(ledger.request_count, 1);
        assert_eq!(ledger.cost_usd, Some(0.01));
        assert_eq!(ledger.requests[0].surface, "decision_report");
    }

    #[tokio::test]
    async fn partial_answer_preserves_billing_and_measurement_evidence() {
        let pool = pool().await;
        let mut response = response();
        response.cost_usd = Some(0.125);
        response.cost_source = "billed";
        response.issues.push(crate::jev::JevAnswerIssue {
            id: "direction".into(),
            reason: "missing".into(),
        });
        record_success(
            &pool,
            RecordedRequest {
                id: "partial",
                created_at: "2026-09-20T12:00:00Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("1"),
                model_requested: "~typesafe/jev-latest",
                question_count: 1,
            },
            &response,
            None,
        )
        .await
        .unwrap();
        let rows = usage_rows(&pool, 10).await.unwrap();
        assert_eq!(rows[0]["status"], "partial");
        assert_eq!(rows[0]["cost_source"], "billed");
        assert_eq!(rows[0]["cost_usd"], 0.125);
        let stored = results_for(&pool, PURPOSE_REPORT_GRADING, 10)
            .await
            .unwrap();
        // row_to_json already decodes *_json TEXT columns.
        assert_eq!(
            stored[0]["result_json"]["measurement"]["schema_version"],
            "test"
        );
    }

    async fn pool() -> AnyPool {
        DRIVERS.call_once(sqlx::any::install_default_drivers);
        // One connection: each new connection to `sqlite::memory:` gets its
        // own empty database, so a larger pool would serve queries against a
        // schema that was never created.
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("jev schema applies");
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS editorial_research_items (
                id TEXT PRIMARY KEY, title TEXT, source_name TEXT)",
        )
        .execute(&pool)
        .await
        .expect("stub editorial table");
        pool
    }

    fn response() -> JevResponse {
        JevResponse {
            answers: BTreeMap::new(),
            issues: Vec::new(),
            usage: JevUsage {
                input_tokens: 420,
                output_tokens: 0,
            },
            cost_usd: Some(0.0000176),
            cost_source: crate::jev::COST_SOURCE_RATE_CARD,
            measurement: serde_json::json!({"schema_version":"test"}),
            model_resolved: Some("typesafe/jev-1.13".to_string()),
            latency_ms: 118,
            attempts: 1,
        }
    }

    /// A judgement Jev never made must read back as SQL NULL, not as 0.0.
    /// Zero is a confident "strongly no" for every one of these columns, so a
    /// default would fabricate a negative verdict out of a missing answer.
    #[tokio::test]
    async fn an_unanswered_judgement_is_stored_as_null_not_as_zero() {
        let pool = pool().await;
        let sparse = NewsSignal {
            about_symbol: Some(0.9),
            direction: Some("bullish".to_string()),
            materiality: Some(0.75),
            ..NewsSignal::default()
        };
        record_editorial_signal(
            &pool,
            "item-1",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
            None,
            &sparse,
            false,
        )
        .await
        .expect("signal records");

        let row = sqlx::query(
            "SELECT about_symbol, company_specific, restates_known, instruction_shaped
             FROM jev_editorial_signals WHERE item_id = 'item-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("one row");
        let json = row_to_json(&row);
        assert_eq!(
            json["about_symbol"].as_f64(),
            Some(0.9),
            "a probability must survive the round trip as a float, not truncate to an integer"
        );
        for absent in ["company_specific", "restates_known", "instruction_shaped"] {
            assert!(
                json[absent].is_null(),
                "{absent} was never answered and must stay null, not 0.0"
            );
        }
    }

    #[tokio::test]
    async fn re_judging_an_item_replaces_its_row_rather_than_duplicating_it() {
        let pool = pool().await;
        let signal = NewsSignal {
            about_symbol: Some(0.5),
            ..NewsSignal::default()
        };
        for _ in 0..3 {
            record_editorial_signal(
                &pool,
                "item-1",
                "NOVO:xcse",
                "2026-09-20T08:00:00Z",
                None,
                &signal,
                false,
            )
            .await
            .expect("signal records");
        }
        let rows = sqlx::query("SELECT item_id FROM jev_editorial_signals")
            .fetch_all(&pool)
            .await
            .expect("rows");
        assert_eq!(rows.len(), 1);
    }

    /// A Jev outage must leave a trace. Without a failure row the ledger shows
    /// a quiet day, which is indistinguishable from a day with nothing to judge.
    #[tokio::test]
    async fn a_failed_call_is_recorded_rather_than_leaving_a_gap() {
        let pool = pool().await;
        record_failure(
            &pool,
            RecordedRequest {
                id: "req-1",
                created_at: "2026-09-20T08:00:00Z",
                purpose: PURPOSE_EDITORIAL_ITEM,
                subject: Some("item-1"),
                model_requested: "~typesafe/jev-latest",
                question_count: 6,
            },
            "Jev returned 429: rate limited",
        )
        .await
        .expect("failure records");

        let rows = usage_rows(&pool, 10).await.expect("usage rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], STATUS_ERROR);
        assert_eq!(rows[0]["cost_source"], crate::jev::COST_SOURCE_NONE);
        assert!(
            rows[0]["cost_usd"].is_null(),
            "a failed call has no cost, which is not the same as a cost of zero"
        );
    }

    #[tokio::test]
    async fn a_successful_call_records_the_model_that_actually_answered() {
        let pool = pool().await;
        record_success(
            &pool,
            RecordedRequest {
                id: "req-1",
                created_at: "2026-09-20T08:00:00Z",
                purpose: PURPOSE_EDITORIAL_ITEM,
                subject: Some("item-1"),
                model_requested: "~typesafe/jev-latest",
                question_count: 6,
            },
            &response(),
            None,
        )
        .await
        .expect("success records");

        let rows = usage_rows(&pool, 10).await.expect("usage rows");
        assert_eq!(rows[0]["model_requested"], "~typesafe/jev-latest");
        assert_eq!(
            rows[0]["model_resolved"], "typesafe/jev-1.13",
            "the alias is what we asked for; this is what answered"
        );
        assert_eq!(rows[0]["input_tokens"], 420);
    }

    /// Both directions matter, and for opposite reasons: an over-firing marker
    /// list silently drops real news, while an item the markers passed and Jev
    /// reads as instruction-shaped is a screen that let something through.
    #[tokio::test]
    async fn screening_disagreements_are_reported_in_both_directions() {
        let pool = pool().await;
        for (id, marker_screened, instruction_shaped) in [
            ("agree-clean", false, 0.02),
            ("agree-flagged", true, 0.97),
            ("marker-over-fired", true, 0.03),
            ("marker-missed-it", false, 0.95),
        ] {
            record_editorial_signal(
                &pool,
                id,
                "NOVO:xcse",
                "2026-09-20T08:00:00Z",
                None,
                &NewsSignal {
                    instruction_shaped: Some(instruction_shaped),
                    ..NewsSignal::default()
                },
                marker_screened,
            )
            .await
            .expect("signal records");
        }

        let disagreements = screening_disagreements(&pool, 0.5, 100)
            .await
            .expect("disagreements");
        let ids: Vec<&str> = disagreements
            .iter()
            .filter_map(|row| row["item_id"].as_str())
            .collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(ids.contains(&"marker-over-fired"));
        assert!(ids.contains(&"marker-missed-it"));
    }

    /// An item Jev never judged on this axis is not evidence either way, and
    /// must not be counted as agreement.
    #[tokio::test]
    async fn an_unjudged_item_is_not_counted_as_agreement() {
        let pool = pool().await;
        record_editorial_signal(
            &pool,
            "unjudged",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
            None,
            &NewsSignal::default(),
            true,
        )
        .await
        .expect("signal records");
        assert!(
            screening_disagreements(&pool, 0.5, 100)
                .await
                .expect("disagreements")
                .is_empty()
        );
    }

    /// Pins the float typing of the judgement columns at their boundaries.
    ///
    /// 1.0 and 0.0 are ordinary values here, meaning "certain" and "certainly
    /// not", and `row_to_json` tries its integer arm before its float one. It
    /// currently falls through correctly because a float column rejects an
    /// i64 read on both backends -- this asserts that, rather than assuming
    /// it, and would catch a later change of these columns to INTEGER.
    #[tokio::test]
    async fn a_boundary_probability_reads_back_as_a_float_not_an_integer() {
        let pool = pool().await;
        record_editorial_signal(
            &pool,
            "item-1",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
            None,
            &NewsSignal {
                about_symbol: Some(1.0),
                instruction_shaped: Some(0.0),
                ..NewsSignal::default()
            },
            false,
        )
        .await
        .expect("signal records");

        let row = sqlx::query(
            "SELECT about_symbol, instruction_shaped FROM jev_editorial_signals
             WHERE item_id = 'item-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("one row");
        let json = row_to_json(&row);
        assert!(
            json["about_symbol"].is_f64(),
            "certainty is 1.0, not the integer 1: {}",
            json["about_symbol"]
        );
        assert!(
            json["instruction_shaped"].is_f64(),
            "a confident no is 0.0, not the integer 0: {}",
            json["instruction_shaped"]
        );
        assert_eq!(json["about_symbol"].as_f64(), Some(1.0));
        assert_eq!(json["instruction_shaped"].as_f64(), Some(0.0));
    }
}
