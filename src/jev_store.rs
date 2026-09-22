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
            -- Evidence identity and timing, snapshotted when the judgement was
            -- made. editorial_research_items is pruned on a retention schedule
            -- and its last_seen_at is rewritten whenever a feed repeats a
            -- story, so item_id alone becomes a dangling reference to evidence
            -- that has changed or vanished.
            evidence_published_at TEXT,
            evidence_first_seen_at TEXT,
            evidence_sha256 TEXT,
            evidence_timing TEXT,
            evidence_lag_seconds INTEGER,
            -- Denormalised so provenance survives independently of jev_requests.
            model_resolved TEXT,
            measurement_version TEXT,
            answered_question_count INTEGER NOT NULL DEFAULT 0,
            expected_question_count INTEGER NOT NULL DEFAULT 0,
            -- Three distinct moments. `created_at` is kept as the answer time
            -- for continuity; `available_at` is the one availability is judged
            -- against, because a judgement is only usable once it is readable.
            requested_at TEXT,
            answered_at TEXT,
            available_at TEXT,
            -- The sanitized text judged, so a pruned source row does not leave
            -- a judgement that can be matched by hash but never adjudicated.
            evidence_title TEXT,
            evidence_summary TEXT,
            PRIMARY KEY (item_id, symbol)
        )",
        "CREATE INDEX IF NOT EXISTS idx_jev_requests_created
         ON jev_requests(created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_jev_editorial_signals_symbol
         ON jev_editorial_signals(symbol, created_at DESC)",
    ]
}

/// Indexes that depend on columns added after the table first shipped.
///
/// These must run *after* `signal_columns_to_ensure`, never inside
/// `create_schema_sql`. A fresh database gets the column from CREATE TABLE and
/// an index over it succeeds immediately, which is exactly why putting it
/// there passed every test and then failed on the only database that had
/// existed before the column did.
pub fn post_migration_index_sql() -> &'static [&'static str] {
    &["CREATE INDEX IF NOT EXISTS idx_jev_editorial_signals_timing
       ON jev_editorial_signals(evidence_timing, created_at DESC)"]
}

/// Columns added to `jev_editorial_signals` after it first shipped.
///
/// `CREATE TABLE IF NOT EXISTS` does not add columns, and the table was
/// created on the deploy before these existed, so a fresh database and a
/// running one would otherwise disagree about the schema.
pub fn signal_columns_to_ensure() -> &'static [&'static str] {
    &[
        "evidence_published_at TEXT",
        "evidence_first_seen_at TEXT",
        "evidence_sha256 TEXT",
        "evidence_timing TEXT",
        "evidence_lag_seconds INTEGER",
        "model_resolved TEXT",
        "measurement_version TEXT",
        "answered_question_count INTEGER NOT NULL DEFAULT 0",
        "expected_question_count INTEGER NOT NULL DEFAULT 0",
        "requested_at TEXT",
        "answered_at TEXT",
        "available_at TEXT",
        "evidence_title TEXT",
        "evidence_summary TEXT",
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
pub(crate) struct RecordedSignal<'a> {
    pub item_id: &'a str,
    pub symbol: &'a str,
    /// When Jev answered.
    pub created_at: &'a str,
    pub request_id: Option<&'a str>,
    pub marker_screened: bool,
    pub provenance: &'a crate::jev_signals::EvidenceProvenance,
    pub model_resolved: Option<&'a str>,
    pub answered_question_count: i64,
    pub expected_question_count: i64,
}

pub(crate) async fn record_editorial_signal(
    pool: &AnyPool,
    meta: RecordedSignal<'_>,
    signal: &NewsSignal,
) -> Result<()> {
    let (item_id, symbol, created_at, request_id, marker_screened) = (
        meta.item_id,
        meta.symbol,
        meta.created_at,
        meta.request_id,
        meta.marker_screened,
    );
    sqlx::query(
        "INSERT INTO jev_editorial_signals (
            item_id, symbol, created_at, request_id, about_symbol, direction,
            direction_confidence, bullish_probability, bearish_probability, materiality,
            company_specific, instruction_shaped, restates_known, signed_score, marker_screened,
            evidence_published_at, evidence_first_seen_at, evidence_sha256, evidence_timing,
            evidence_lag_seconds, model_resolved, measurement_version,
            answered_question_count, expected_question_count,
            requested_at, answered_at, available_at, evidence_title, evidence_summary
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29)
         ON CONFLICT (item_id, symbol) DO UPDATE SET
           created_at = excluded.created_at, request_id = excluded.request_id,
           about_symbol = excluded.about_symbol, direction = excluded.direction,
           direction_confidence = excluded.direction_confidence,
           bullish_probability = excluded.bullish_probability,
           bearish_probability = excluded.bearish_probability, materiality = excluded.materiality,
           company_specific = excluded.company_specific, instruction_shaped = excluded.instruction_shaped,
           restates_known = excluded.restates_known, signed_score = excluded.signed_score,
           marker_screened = excluded.marker_screened,
           evidence_published_at = excluded.evidence_published_at,
           evidence_first_seen_at = excluded.evidence_first_seen_at,
           evidence_sha256 = excluded.evidence_sha256,
           evidence_timing = excluded.evidence_timing,
           evidence_lag_seconds = excluded.evidence_lag_seconds,
           model_resolved = excluded.model_resolved,
           measurement_version = excluded.measurement_version,
           answered_question_count = excluded.answered_question_count,
           expected_question_count = excluded.expected_question_count,
           requested_at = excluded.requested_at, answered_at = excluded.answered_at,
           available_at = excluded.available_at,
           evidence_title = excluded.evidence_title,
           evidence_summary = excluded.evidence_summary",
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
    .bind(meta.provenance.published_at.as_deref())
    .bind(meta.provenance.first_seen_at.as_deref())
    .bind(meta.provenance.content_sha256.as_str())
    .bind(meta.provenance.timing)
    .bind(meta.provenance.lag_seconds)
    .bind(meta.model_resolved)
    .bind(meta.provenance.measurement_version)
    .bind(meta.answered_question_count)
    .bind(meta.expected_question_count)
    .bind(meta.provenance.requested_at.as_str())
    .bind(meta.provenance.answered_at.as_str())
    // NULL here, deliberately. Availability is stamped after the write
    // succeeds, by `mark_signal_available` below.
    //
    // I previously bound a timestamp taken immediately before the insert and
    // wrote that it "can only understate availability, never claim it early".
    // That is backwards. An earlier stamp makes the row look readable sooner:
    // captured at .100, decision at .200, commit at .300 -- the comparison
    // said available while the row was still being written. Strict ordering
    // fixes equality, not that interval.
    .bind(Option::<&str>::None)
    .bind(meta.provenance.title.as_str())
    .bind(meta.provenance.summary.as_str())
    .execute(pool)
    .await
    .context("recording a Jev editorial signal")?;

    mark_signal_available(pool, item_id, symbol).await
}

/// Stamps a persisted signal as available, using an instant observed after the
/// write that made it readable.
///
/// Kept separate so the timestamp cannot be taken before the row exists. If
/// this fails the column stays NULL, and a NULL is not usable as a feature --
/// the failure mode is a judgement that never counts, which is the safe
/// direction.
pub(crate) async fn mark_signal_available(
    pool: &AnyPool,
    item_id: &str,
    symbol: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE jev_editorial_signals SET available_at = $1
         WHERE item_id = $2 AND symbol = $3",
    )
    .bind(crate::jev_signals::now_rfc3339())
    .bind(item_id)
    .bind(symbol)
    .execute(pool)
    .await
    .context("marking a Jev editorial signal available")?;
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
                instruction_shaped, restates_known, signed_score, marker_screened,
                evidence_published_at, evidence_first_seen_at, evidence_sha256, evidence_timing,
                evidence_lag_seconds, model_resolved, measurement_version,
                answered_question_count, expected_question_count,
                requested_at, answered_at, available_at, evidence_title, evidence_summary
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

/// Stored grades whose numeric half was computed by an older method.
///
/// Returns the request id and subject so the measurement can be re-derived
/// from evidence already on disk, with no provider call.
/// Marker for a grade whose numeric half can never be recomputed.
///
/// Reports 1 to 82 predate the indicator snapshot being stored in the prompt,
/// so there is nothing to recompute against and stamping them with the current
/// method would record a clean revalidation of something never examined. They
/// were skipped instead -- and skipped again on every cycle, because the
/// selection kept handing back the same two hundred. The loop recomputed
/// nothing for hours while grades that *could* be recomputed sat behind them.
pub(crate) const BLOCKED_KEY: &str = "numeric_recompute_blocked";

/// Records that a grade's numeric half cannot be recomputed, and why.
///
/// Permanent by design: a stored prompt does not change, so a later method
/// version must not pick it up again. Nothing about the measurement is
/// altered -- the absence stays an absence.
pub(crate) async fn mark_recompute_blocked(pool: &AnyPool, id: &str, reason: &str) -> Result<()> {
    let row = sqlx::query("SELECT result_json FROM jev_requests WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("reading a grade to mark it unrecomputable")?;
    let Some(row) = row else { return Ok(()) };
    let Ok(text) = row.try_get::<String, _>("result_json") else {
        return Ok(());
    };
    let Ok(mut result) = serde_json::from_str::<JsonValue>(&text) else {
        return Ok(());
    };
    result[BLOCKED_KEY] = serde_json::json!({
        "reason": reason,
        "at": crate::jev_signals::now_rfc3339(),
    });
    sqlx::query("UPDATE jev_requests SET result_json = $1 WHERE id = $2")
        .bind(serde_json::to_string(&result).unwrap_or(text))
        .bind(id)
        .execute(pool)
        .await
        .context("marking a grade unrecomputable")?;
    Ok(())
}

pub(crate) async fn grades_with_stale_numeric_method(
    pool: &AnyPool,
    method_version: &str,
    limit: i64,
) -> Result<Vec<(String, String)>> {
    let limit = limit.clamp(1, 2_000);
    Ok(sqlx::query(&format!(
        "SELECT id, subject FROM jev_requests
         WHERE purpose = $1 AND status = 'completed'
           AND subject IS NOT NULL AND result_json IS NOT NULL
           AND result_json NOT LIKE $2
           AND result_json NOT LIKE $3
         ORDER BY created_at DESC
         LIMIT {limit}"
    ))
    .bind(PURPOSE_REPORT_GRADING)
    .bind(format!("%\"method_version\":\"{method_version}\"%"))
    .bind(format!("%\"{BLOCKED_KEY}\"%"))
    .fetch_all(pool)
    .await
    .context("reading grades with a stale numeric method")?
    .iter()
    .filter_map(|row| {
        Some((
            row.try_get::<String, _>("id").ok()?,
            row.try_get::<String, _>("subject").ok()?,
        ))
    })
    .collect())
}

/// Replaces the numeric half of a stored grade, appending what it replaced.
///
/// The wording verdicts are untouched: the model was not asked again, and its
/// answers are still the answers it gave.
///
/// The history is append-only and holds the whole prior measurement, not a
/// note that one existed. An earlier version stored only a reason and a
/// timestamp while its comment claimed the findings were kept, and its test
/// asserted the reason survived rather than the findings -- so a corrected
/// measurement silently destroyed the one that may have been acted on, and a
/// second correction destroyed even the note.
pub(crate) async fn replace_numeric_measurement(
    pool: &AnyPool,
    id: &str,
    numeric: &JsonValue,
    reason: &str,
) -> Result<()> {
    let row = sqlx::query("SELECT result_json FROM jev_requests WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("reading a grade for numeric recomputation")?;
    let Some(row) = row else { return Ok(()) };
    let Ok(text) = row.try_get::<String, _>("result_json") else {
        return Ok(());
    };
    let Ok(mut result) = serde_json::from_str::<JsonValue>(&text) else {
        return Ok(());
    };
    let previous = result
        .get("numeric_checks")
        .cloned()
        .unwrap_or(JsonValue::Null);
    if !previous.is_null() {
        let entry = serde_json::json!({
            "superseded_at": crate::jev_signals::now_rfc3339(),
            "reason": reason,
            "method_version": previous
                .get("method_version")
                .cloned()
                .unwrap_or(JsonValue::Null),
            // Lifted out of the measurement so a reader can compare identities
            // without unpacking it. Same fingerprint means the method is the
            // only difference; a different one means the evidence moved and
            // the two measurements are not comparable.
            "evidence_sha256": previous
                .get("evidence_sha256")
                .cloned()
                .unwrap_or(JsonValue::Null),
            "measurement": previous,
        });
        match result.get_mut("numeric_checks_history") {
            Some(JsonValue::Array(history)) => history.push(entry),
            _ => result["numeric_checks_history"] = JsonValue::Array(vec![entry]),
        }
    }
    result["numeric_checks"] = numeric.clone();
    sqlx::query("UPDATE jev_requests SET result_json = $1 WHERE id = $2")
        .bind(result.to_string())
        .bind(id)
        .execute(pool)
        .await
        .context("replacing a numeric measurement")?;
    Ok(())
}

/// Subjects already judged for a given purpose, so a sidecar pass never
/// re-spends on work it has done.
pub(crate) async fn judged_subjects_at_version(
    pool: &AnyPool,
    purpose: &str,
    version: &str,
) -> Result<std::collections::HashSet<String>> {
    // A subject judged under an older question set has not been judged under
    // this one. Matching on the stored version stops a re-worded question from
    // silently inheriting answers given to the previous wording.
    Ok(sqlx::query(
        "SELECT subject FROM jev_requests
         WHERE purpose = $1 AND status IN ('completed', 'partial') AND subject IS NOT NULL
           AND result_json IS NOT NULL
           AND result_json LIKE $2",
    )
    .bind(purpose)
    .bind(format!("%\"version\":\"{version}\"%"))
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

    pub(super) async fn pool() -> AnyPool {
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

    /// Records a signal with unremarkable provenance, so a test that is not
    /// about provenance does not have to restate it.
    async fn record_signal(
        pool: &AnyPool,
        item_id: &str,
        symbol: &str,
        created_at: &str,
        signal: &NewsSignal,
        marker_screened: bool,
    ) -> Result<()> {
        let provenance = crate::jev_signals::EvidenceProvenance::new(
            &format!("https://example.test/{item_id}"),
            item_id,
            "",
            Some(created_at),
            Some(created_at),
            created_at,
            created_at,
        );
        record_editorial_signal(
            pool,
            RecordedSignal {
                item_id,
                symbol,
                created_at,
                request_id: None,
                marker_screened,
                provenance: &provenance,
                model_resolved: Some("typesafe/jev-1.13-20260917"),
                answered_question_count: 6,
                expected_question_count: 6,
            },
            signal,
        )
        .await
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
        record_signal(
            &pool,
            "item-1",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
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
            record_signal(
                &pool,
                "item-1",
                "NOVO:xcse",
                "2026-09-20T08:00:00Z",
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
            record_signal(
                &pool,
                id,
                "NOVO:xcse",
                "2026-09-20T08:00:00Z",
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
        record_signal(
            &pool,
            "unjudged",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
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
        record_signal(
            &pool,
            "item-1",
            "NOVO:xcse",
            "2026-09-20T08:00:00Z",
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

    /// The reason provenance is on the signal at all: editorial_research_items
    /// is pruned on a retention schedule, so a judgement whose evidence lived
    /// only behind item_id would become uninterpretable.
    #[tokio::test]
    async fn a_judgement_stays_interpretable_after_its_source_row_is_pruned() {
        let pool = pool().await;
        sqlx::query(
            "INSERT INTO editorial_research_items (id, title, source_name)
             VALUES ('item-1', 'Novo raises guidance', 'Borsen')",
        )
        .execute(&pool)
        .await
        .expect("source row");

        let provenance = crate::jev_signals::EvidenceProvenance::new(
            "https://example.test/a",
            "Novo raises guidance",
            "Outlook lifted.",
            Some("2026-09-20T07:55:00Z"),
            Some("2026-09-20T08:00:00Z"),
            "2026-09-20T08:09:58Z",
            "2026-09-20T08:10:00Z",
        );
        record_editorial_signal(
            &pool,
            RecordedSignal {
                item_id: "item-1",
                symbol: "NOVO:xcse",
                created_at: "2026-09-20T08:10:00Z",
                request_id: None,
                marker_screened: false,
                provenance: &provenance,
                model_resolved: Some("typesafe/jev-1.13-20260917"),
                answered_question_count: 6,
                expected_question_count: 6,
            },
            &NewsSignal {
                about_symbol: Some(0.95),
                ..NewsSignal::default()
            },
        )
        .await
        .expect("signal records");

        sqlx::query("DELETE FROM editorial_research_items")
            .execute(&pool)
            .await
            .expect("prune");

        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals survive the prune");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["evidence_timing"], "decision_time");
        assert_eq!(rows[0]["evidence_lag_seconds"], 600);
        assert_eq!(rows[0]["evidence_published_at"], "2026-09-20T07:55:00Z");
        assert_eq!(rows[0]["model_resolved"], "typesafe/jev-1.13-20260917");
        assert_eq!(
            rows[0]["measurement_version"],
            crate::jev_signals::NEWS_QUESTION_SET_VERSION
        );
        assert_eq!(
            rows[0]["evidence_sha256"].as_str().map(str::len),
            Some(64),
            "the evidence is still identifiable with the source row gone"
        );
    }

    /// An alias rollover produces a different resolved model. Existing
    /// judgements keep the version that made them rather than being restamped,
    /// so a rollover is a visible boundary in the data instead of a silent
    /// recalibration.
    #[tokio::test]
    async fn a_model_rollover_does_not_restamp_earlier_judgements() {
        let pool = pool().await;
        for (item, model, at) in [
            ("old", "typesafe/jev-1.13-20260917", "2026-09-20T08:00:00Z"),
            ("new", "typesafe/jev-1.14-20261001", "2026-10-02T08:00:00Z"),
        ] {
            let provenance = crate::jev_signals::EvidenceProvenance::new(
                "https://example.test/a",
                item,
                "",
                Some(at),
                Some(at),
                at,
                at,
            );
            record_editorial_signal(
                &pool,
                RecordedSignal {
                    item_id: item,
                    symbol: "NOVO:xcse",
                    created_at: at,
                    request_id: None,
                    marker_screened: false,
                    provenance: &provenance,
                    model_resolved: Some(model),
                    answered_question_count: 6,
                    expected_question_count: 6,
                },
                &NewsSignal::default(),
            )
            .await
            .expect("signal records");
        }

        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals");
        let models: Vec<&str> = rows
            .iter()
            .filter_map(|row| row["model_resolved"].as_str())
            .collect();
        assert!(models.contains(&"typesafe/jev-1.13-20260917"));
        assert!(models.contains(&"typesafe/jev-1.14-20261001"));
    }

    /// The upgrade path, which no fresh-database test can exercise.
    ///
    /// This reproduces the shape `jev_editorial_signals` had when it first
    /// shipped, then applies the startup sequence in order. An index over a
    /// migration-added column placed in `create_schema_sql` succeeds on a
    /// fresh database and fails on every database that existed before the
    /// column did -- which is exactly what happened: both the scheduler and
    /// the MCP pod crash-looped on `column "evidence_timing" does not exist`
    /// while every test passed.
    #[tokio::test]
    async fn the_startup_sequence_upgrades_a_table_that_predates_provenance() {
        DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database");

        // The original shape, before any provenance column existed.
        sqlx::query(
            "CREATE TABLE jev_editorial_signals (
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
        )
        .execute(&pool)
        .await
        .expect("the pre-provenance table");
        sqlx::query(
            "INSERT INTO jev_editorial_signals (item_id, symbol, created_at, marker_screened)
             VALUES ('legacy', 'NOVO:xcse', '2026-09-19T08:00:00Z', 1)",
        )
        .execute(&pool)
        .await
        .expect("a row written before the upgrade");

        // The startup sequence, in the order state.rs applies it.
        for sql in create_schema_sql() {
            sqlx::query(sql).execute(&pool).await.unwrap_or_else(|err| {
                panic!("create_schema_sql must be safe on an old table: {err}")
            });
        }
        for column in signal_columns_to_ensure() {
            sqlx::query(&format!(
                "ALTER TABLE jev_editorial_signals ADD COLUMN {column}"
            ))
            .execute(&pool)
            .await
            .unwrap_or_else(|err| panic!("adding {column}: {err}"));
        }
        for sql in post_migration_index_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("post-migration index: {err}"));
        }

        // The pre-existing row survives, with the new columns null rather than
        // back-filled with values nobody measured.
        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("the upgraded table is readable");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["item_id"], "legacy");
        assert!(
            rows[0]["evidence_timing"].is_null(),
            "a judgement made before timing was recorded has unknown timing, not a guessed one"
        );
    }

    /// Guards the ordering rule itself, so a future index over a
    /// migration-added column cannot be put back where this one was.
    #[test]
    fn create_schema_sql_never_indexes_a_migration_added_column() {
        let schema = create_schema_sql().join("\n");
        let indexes: String = schema
            .lines()
            .filter(|line| line.contains("CREATE INDEX") || line.trim().starts_with("ON "))
            .collect::<Vec<_>>()
            .join("\n");
        for column in signal_columns_to_ensure() {
            let name = column.split_whitespace().next().expect("a column name");
            assert!(
                !indexes.contains(name),
                "{name} is added by migration, so an index over it belongs in \
                 post_migration_index_sql, not create_schema_sql"
            );
        }
    }

    /// A version bump has to actually re-ask, or the new wording silently
    /// inherits answers given to the old one and two different measurements
    /// get pooled under one name.
    #[tokio::test]
    async fn a_subject_judged_at_an_older_version_is_not_treated_as_judged() {
        let pool = pool().await;
        record_success(
            &pool,
            RecordedRequest {
                id: "req-v1",
                created_at: "2026-09-20T08:00:00Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("302"),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            &response(),
            Some(&serde_json::json!({"version": "v1", "rationale_supported": 0.04})),
        )
        .await
        .expect("a v1 grade");

        assert!(
            judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v1")
                .await
                .expect("v1 lookup")
                .contains("302"),
            "the subject is judged at the version that judged it"
        );
        assert!(
            !judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v2")
                .await
                .expect("v2 lookup")
                .contains("302"),
            "and is unjudged at a version that has not seen it"
        );
    }

    /// A failed call has no result and no version, so it must never satisfy a
    /// version check -- otherwise one outage would permanently mark a subject
    /// as judged.
    #[tokio::test]
    async fn a_failed_call_never_counts_as_judged_at_any_version() {
        let pool = pool().await;
        record_failure(
            &pool,
            RecordedRequest {
                id: "req-fail",
                created_at: "2026-09-20T08:00:00Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("302"),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            "Jev returned 529: overloaded",
        )
        .await
        .expect("a failure row");

        for version in ["v1", "v2"] {
            assert!(
                !judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, version)
                    .await
                    .expect("lookup")
                    .contains("302"),
                "an outage must not mark {version} as done"
            );
        }
    }

    /// The interval that strict ordering does not fix.
    ///
    /// A timestamp taken before the insert makes the row look readable while
    /// it is still being written: captured .100, decision .200, commit .300 --
    /// and the comparison said available. This pins that a persisted-but-not-
    /// yet-marked row is unusable, which is the state that existed during that
    /// interval.
    #[tokio::test]
    async fn a_row_is_not_available_until_the_write_that_made_it_readable_finished() {
        let pool = pool().await;
        let provenance = crate::jev_signals::EvidenceProvenance::new(
            "https://example.test/a",
            "Novo raises guidance",
            "",
            Some("2026-09-21T08:00:00Z"),
            Some("2026-09-21T08:00:00Z"),
            "2026-09-21T08:00:01Z",
            "2026-09-21T08:00:02Z",
        );

        // The row as it exists between the insert and the availability stamp.
        sqlx::query(
            "INSERT INTO jev_editorial_signals
                (item_id, symbol, created_at, evidence_timing, marker_screened)
             VALUES ('mid-write', 'NOVO:xcse', '2026-09-21T08:00:02Z', 'decision_time', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert");

        let mid = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals");
        assert!(
            mid[0]["available_at"].is_null(),
            "a row still being written has no availability instant"
        );
        assert_eq!(
            crate::jev_signals::retention_summary(&mid, "2026-09-21T08:00:03Z")["features_available"],
            0,
            "and is therefore not usable as a feature by any decision"
        );

        // A fully recorded signal does get one, and it falls inside the window
        // in which the write actually happened. Compared against the real
        // clock rather than the synthetic fixture times above.
        let before = crate::jev_signals::now_rfc3339();
        record_editorial_signal(
            &pool,
            RecordedSignal {
                item_id: "complete",
                symbol: "NOVO:xcse",
                created_at: "2026-09-21T08:00:02Z",
                request_id: None,
                marker_screened: false,
                provenance: &provenance,
                model_resolved: Some("typesafe/jev-1.13-20260917"),
                answered_question_count: 6,
                expected_question_count: 6,
            },
            &NewsSignal::default(),
        )
        .await
        .expect("signal records");

        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals");
        let complete = rows
            .iter()
            .find(|row| row["item_id"] == "complete")
            .expect("the completed row");
        let available_at = complete["available_at"].as_str().expect("an instant");
        let after = crate::jev_signals::now_rfc3339();
        assert!(
            available_at >= before.as_str() && available_at <= after.as_str(),
            "availability is observed during the write, not before it began: \
             {before} <= {available_at} <= {after}"
        );
    }

    /// If the availability stamp fails the column stays NULL, and a NULL is
    /// never usable. The failure mode is a judgement that does not count,
    /// which is the safe direction.
    #[tokio::test]
    async fn a_missing_availability_stamp_fails_closed() {
        let pool = pool().await;
        sqlx::query(
            "INSERT INTO jev_editorial_signals
                (item_id, symbol, created_at, evidence_timing, marker_screened)
             VALUES ('unstamped', 'NOVO:xcse', '2026-09-21T08:00:02Z', 'decision_time', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert");

        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals");
        assert_eq!(
            crate::jev_signals::retention_summary(&rows, "2999-01-01T00:00:00Z")["features_available"],
            0
        );

        mark_signal_available(&pool, "unstamped", "NOVO:xcse")
            .await
            .expect("stamp");
        let rows = recent_signals(&pool, "2026-01-01T00:00:00Z", 10)
            .await
            .expect("signals");
        assert_eq!(
            crate::jev_signals::retention_summary(&rows, "2999-01-01T00:00:00Z")["features_available"],
            1,
            "and becomes usable only once the stamp exists"
        );
    }

    /// A corrected arithmetic must reach measurements already stored, and must
    /// not destroy what it corrects.
    #[tokio::test]
    async fn successive_recomputations_preserve_every_earlier_measurement() {
        let pool = pool().await;
        record_success(
            &pool,
            RecordedRequest {
                id: "req-1",
                created_at: "2026-09-21T06:12:44Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("304"),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            &response(),
            Some(&serde_json::json!({
                "version": "v8",
                "numeric_checks": {
                    "differs": 1,
                    "method_version": "n0-band",
                    "evidence_sha256": "abc123",
                    "per_candidate": [{"symbol": "FORTUM:xhel", "summary": {"differs": 1}}],
                },
                "wording_verdicts": [{"symbol": "FORTUM:xhel", "verdict": "fair"}],
            })),
        )
        .await
        .expect("a grade under the original method");

        let stale = grades_with_stale_numeric_method(&pool, "n2-2026-09-21", 100)
            .await
            .expect("stale lookup");
        assert_eq!(stale, vec![("req-1".to_string(), "304".to_string())]);

        replace_numeric_measurement(
            &pool,
            "req-1",
            &serde_json::json!({"differs": 1, "method_version": "n1-truncation"}),
            "first correction",
        )
        .await
        .expect("first replace");
        replace_numeric_measurement(
            &pool,
            "req-1",
            &serde_json::json!({"differs": 0, "method_version": "n2-2026-09-21"}),
            "second correction",
        )
        .await
        .expect("second replace");

        let rows = results_for(&pool, PURPOSE_REPORT_GRADING, 10)
            .await
            .expect("results");
        let result = &rows[0]["result_json"];

        assert_eq!(result["numeric_checks"]["method_version"], "n2-2026-09-21");
        assert_eq!(result["numeric_checks"]["differs"], 0);

        let history = result["numeric_checks_history"]
            .as_array()
            .expect("an append-only history");
        assert_eq!(history.len(), 2, "both earlier measurements are retained");
        assert_eq!(history[0]["method_version"], "n0-band");
        assert_eq!(
            history[0]["evidence_sha256"], "abc123",
            "the evidence identity travels with the preserved measurement, so the two \
             can be compared with the method as the only variable"
        );
        assert_eq!(
            history[0]["measurement"]["per_candidate"][0]["symbol"], "FORTUM:xhel",
            "the whole prior measurement survives, not a note that one existed"
        );
        assert_eq!(history[0]["measurement"]["differs"], 1);
        assert_eq!(history[1]["method_version"], "n1-truncation");
        assert_eq!(history[1]["reason"], "second correction");

        assert_eq!(
            result["wording_verdicts"][0]["verdict"], "fair",
            "the model was not asked again, so its answers stand"
        );
        assert_eq!(result["version"], "v8", "the wording version is untouched");

        assert!(
            grades_with_stale_numeric_method(&pool, "n2-2026-09-21", 100)
                .await
                .expect("stale lookup")
                .is_empty(),
            "and it is not recomputed again"
        );
    }
}

#[cfg(test)]
mod recompute_selection_tests {
    use super::*;

    /// A grade that can never be recomputed must stop being selected.
    ///
    /// Reports 1 to 82 predate the indicator snapshot being stored, so the
    /// recompute skipped them -- and the selection handed back the same two
    /// hundred on every cycle. Production logged `recomputed=0 unable=200`
    /// for hours while 386 grades that could be recomputed waited behind them.
    #[tokio::test]
    async fn a_grade_marked_unrecomputable_is_not_selected_again() {
        let pool = super::tests::pool().await;
        sqlx::query(
            "INSERT INTO jev_requests
             (id, created_at, purpose, subject, model_requested, question_count, answer_count,
              issue_count, input_tokens, output_tokens, cost_source, latency_ms, attempts,
              status, result_json)
             VALUES ('blocked-1', '2026-09-20T12:00:00Z', $1, '7', 'alias', 1, 1, 0, 10, 0,
                     'not_reported', 1, 1, 'completed', $2)",
        )
        .bind(PURPOSE_REPORT_GRADING)
        .bind(serde_json::json!({"numeric_checks": {"method_version": "n1-old"}}).to_string())
        .execute(&pool)
        .await
        .expect("insert");

        let before = grades_with_stale_numeric_method(&pool, "n2-new", 10)
            .await
            .expect("select");
        assert_eq!(before.len(), 1, "a stale grade is selected");

        mark_recompute_blocked(
            &pool,
            "blocked-1",
            "stored prompt carries no daily indicator snapshot",
        )
        .await
        .expect("mark");

        let after = grades_with_stale_numeric_method(&pool, "n2-new", 10)
            .await
            .expect("select");
        assert!(
            after.is_empty(),
            "and not once it cannot be recomputed: {after:?}"
        );

        // The measurement itself is untouched: the absence stays an absence
        // rather than being stamped with a version it never ran under.
        let row = sqlx::query("SELECT result_json FROM jev_requests WHERE id = 'blocked-1'")
            .fetch_one(&pool)
            .await
            .expect("row");
        let stored: JsonValue =
            serde_json::from_str(&row.try_get::<String, _>("result_json").expect("text"))
                .expect("json");
        assert_eq!(stored["numeric_checks"]["method_version"], "n1-old");
        assert_eq!(
            stored[BLOCKED_KEY]["reason"],
            "stored prompt carries no daily indicator snapshot"
        );
    }
}
