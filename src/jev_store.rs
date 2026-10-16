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
/// Refused before any request was sent, because the input could never fit.
/// Not a provider call: it costs nothing and says nothing about the provider.
pub(crate) const STATUS_REFUSED: &str = "refused";

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
            result_json TEXT,
            attempts_json TEXT,
            request_bytes INTEGER,
            estimated_input_tokens INTEGER
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
        // One row per worker step plus one for the cycle, overwritten every
        // cycle. The worker writes it so the panel can tell a quiet day from
        // a stopped worker, which the request ledger alone cannot: no
        // subjects and no worker both leave it without new rows.
        "CREATE TABLE IF NOT EXISTS jev_worker_status (
            step TEXT PRIMARY KEY,
            updated_at TEXT NOT NULL,
            status TEXT NOT NULL,
            last_clean_at TEXT,
            last_failed_at TEXT,
            summary_json TEXT NOT NULL
        )",
        // The provider gate, one row. It persists a wait across worker cycles
        // and pod restarts: a provider that answers 429 with an hour's
        // Retry-After is not asked again in ten minutes because the worker
        // happened to wake up.
        "CREATE TABLE IF NOT EXISTS jev_provider_state (
            singleton_key TEXT PRIMARY KEY,
            updated_at TEXT NOT NULL,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            not_before TEXT,
            last_failure_at TEXT,
            last_failure_kind TEXT,
            last_retry_after_seconds INTEGER
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

/// Columns added to `jev_requests` after it first shipped.
pub fn request_columns_to_ensure() -> &'static [&'static str] {
    &[
        "attempts_json TEXT",
        "request_bytes INTEGER",
        "estimated_input_tokens INTEGER",
    ]
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

/// One `jev_requests` row, owned.
///
/// Owned so that a row whose write failed can be held and written on a later
/// cycle: a call the provider billed is spent whether or not this process
/// managed to record it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestRow {
    pub id: String,
    pub created_at: String,
    pub purpose: String,
    pub subject: Option<String>,
    pub model_requested: String,
    pub model_resolved: Option<String>,
    pub question_count: i64,
    pub answer_count: i64,
    pub issue_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    pub latency_ms: i64,
    pub attempts: i64,
    pub status: String,
    pub error_text: Option<String>,
    pub result_json: Option<String>,
    /// Every HTTP attempt the call made, as `jev::JevAttempt` values.
    pub attempts_json: String,
    /// The request body sent, and the tokens the budget estimated for it.
    pub request_bytes: Option<i64>,
    pub estimated_input_tokens: Option<i64>,
}

fn attempts_json(attempt_log: &[crate::jev::JevAttempt]) -> String {
    serde_json::to_string(attempt_log).unwrap_or_else(|_| "[]".to_string())
}

/// The row for a call that answered, fully or partly.
pub(crate) fn completed_row(
    meta: RecordedRequest<'_>,
    response: &JevResponse,
    result_json: Option<&JsonValue>,
) -> RequestRow {
    let mut result = result_json
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    result["measurement"] = response.measurement.clone();
    RequestRow {
        id: meta.id.to_string(),
        created_at: meta.created_at.to_string(),
        purpose: meta.purpose.to_string(),
        subject: meta.subject.map(str::to_string),
        model_requested: meta.model_requested.to_string(),
        model_resolved: response.model_resolved.clone(),
        question_count: meta.question_count,
        answer_count: response.answers.len() as i64,
        issue_count: response.issues.len() as i64,
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cost_usd: response.cost_usd,
        cost_source: response.cost_source.to_string(),
        latency_ms: response.latency_ms,
        attempts: response.attempts as i64,
        status: if response.issues.is_empty() {
            STATUS_COMPLETED
        } else {
            "partial"
        }
        .to_string(),
        error_text: None,
        result_json: Some(result.to_string()),
        attempts_json: attempts_json(&response.attempt_log),
        request_bytes: Some(response.request_bytes).filter(|bytes| *bytes > 0),
        estimated_input_tokens: Some(response.estimated_input_tokens).filter(|tokens| *tokens > 0),
    }
}

/// The row for a call that produced no response.
///
/// Its attempts are counted from the log rather than written as zero: a call
/// that failed after three tries made three requests of the provider.
pub(crate) fn failed_row(
    meta: RecordedRequest<'_>,
    error_text: &str,
    attempt_log: &[crate::jev::JevAttempt],
) -> RequestRow {
    RequestRow {
        id: meta.id.to_string(),
        created_at: meta.created_at.to_string(),
        purpose: meta.purpose.to_string(),
        subject: meta.subject.map(str::to_string),
        model_requested: meta.model_requested.to_string(),
        model_resolved: None,
        question_count: meta.question_count,
        answer_count: 0,
        issue_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: None,
        cost_source: crate::jev::COST_SOURCE_NONE.to_string(),
        latency_ms: attempt_log.iter().map(|attempt| attempt.latency_ms).sum(),
        attempts: attempt_log.len() as i64,
        status: STATUS_ERROR.to_string(),
        // The text can carry a provider's body; anything token-shaped in it
        // is masked before it is stored.
        error_text: Some(truncate(
            &crate::debug_redaction::redact_debug_text(error_text),
            2_000,
        )),
        result_json: None,
        attempts_json: attempts_json(attempt_log),
        request_bytes: None,
        estimated_input_tokens: None,
    }
}

/// The row for a subject refused before sending, at a question version.
///
/// Recorded so the subject counts as judged at that version and is not
/// selected again: a refusal is a property of the input, so asking again
/// next cycle would refuse again, and with newest-first selection it would
/// stop its step's batch on every cycle. A new question version re-selects
/// it, since the questions are half of what did not fit.
pub(crate) fn refused_row(
    meta: RecordedRequest<'_>,
    reason: &str,
    question_version: &str,
) -> RequestRow {
    RequestRow {
        status: STATUS_REFUSED.to_string(),
        result_json: Some(
            serde_json::json!({"version": question_version, "refused": reason}).to_string(),
        ),
        ..failed_row(meta, reason, &[])
    }
}

pub(crate) async fn insert_request_row(pool: &AnyPool, row: &RequestRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO jev_requests (
            id, created_at, purpose, subject, model_requested, model_resolved,
            question_count, answer_count, issue_count, input_tokens, output_tokens,
            cost_usd, cost_source, latency_ms, attempts, status, error_text, result_json,
            attempts_json, request_bytes, estimated_input_tokens
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                   $17, $18, $19, $20, $21)",
    )
    .bind(&row.id)
    .bind(&row.created_at)
    .bind(&row.purpose)
    .bind(row.subject.as_deref())
    .bind(&row.model_requested)
    .bind(row.model_resolved.as_deref())
    .bind(row.question_count)
    .bind(row.answer_count)
    .bind(row.issue_count)
    .bind(row.input_tokens)
    .bind(row.output_tokens)
    .bind(row.cost_usd)
    .bind(&row.cost_source)
    .bind(row.latency_ms)
    .bind(row.attempts)
    .bind(&row.status)
    .bind(row.error_text.as_deref())
    .bind(row.result_json.as_deref())
    .bind(&row.attempts_json)
    .bind(row.request_bytes)
    .bind(row.estimated_input_tokens)
    .execute(pool)
    .await
    .with_context(|| format!("recording a Jev request ({})", row.status))?;
    Ok(())
}

/// Records one completed call. The worker writes through `record_or_hold`
/// instead, so a failed write keeps the row; tests record directly.
#[cfg(test)]
pub(crate) async fn record_success(
    pool: &AnyPool,
    meta: RecordedRequest<'_>,
    response: &JevResponse,
    result_json: Option<&JsonValue>,
) -> Result<()> {
    insert_request_row(pool, &completed_row(meta, response, result_json)).await
}

/// Records one failed call.
///
/// A failure is a row, not a gap. Without it a Jev outage is invisible in the
/// ledger and indistinguishable from a quiet day with no items to judge.
#[cfg(test)]
pub(crate) async fn record_failure(
    pool: &AnyPool,
    meta: RecordedRequest<'_>,
    error_text: &str,
    attempt_log: &[crate::jev::JevAttempt],
) -> Result<()> {
    insert_request_row(pool, &failed_row(meta, error_text, attempt_log)).await
}

/// Rows the worker could not write, held for a later cycle.
///
/// Process-local, so a restart loses what is held; the warning logged when a
/// row is held is then the only trace. Bounded: past the cap the oldest row
/// is dropped and counted rather than growing without limit.
#[derive(Debug, Default)]
pub(crate) struct Unrecorded {
    rows: Vec<RequestRow>,
    dropped: usize,
}

const UNRECORDED_CAP: usize = 200;

impl Unrecorded {
    pub(crate) fn hold(&mut self, row: RequestRow) {
        if self.rows.len() >= UNRECORDED_CAP {
            let oldest = self.rows.remove(0);
            self.dropped += 1;
            tracing::warn!(
                id = %oldest.id,
                cost_usd = ?oldest.cost_usd,
                "dropping an unrecorded Jev request: the hold is full"
            );
        }
        self.rows.push(row);
    }

    pub(crate) fn pending(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn dropped(&self) -> usize {
        self.dropped
    }

    /// Cost the provider reported for held rows, which no ledger total
    /// includes until they are written.
    pub(crate) fn pending_cost_usd(&self) -> Option<f64> {
        self.rows
            .iter()
            .filter_map(|row| row.cost_usd)
            .reduce(|left, right| left + right)
    }

    /// Writes what it can, oldest first, and keeps the rest. Returns how many
    /// were written.
    pub(crate) async fn flush(&mut self, pool: &AnyPool) -> usize {
        let mut written = 0;
        let mut kept = Vec::new();
        for row in std::mem::take(&mut self.rows) {
            if !kept.is_empty() {
                kept.push(row);
                continue;
            }
            match insert_request_row(pool, &row).await {
                Ok(()) => written += 1,
                Err(err) => {
                    tracing::warn!(error = %err, "writing a held Jev request");
                    kept.push(row);
                }
            }
        }
        self.rows = kept;
        written
    }
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
         WHERE status <> 'refused'
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
///
/// The title is the one snapshotted when Jev judged the item, falling back to
/// the source row: that row is pruned on a retention schedule, and a
/// disagreement whose text has gone cannot be adjudicated.
pub(crate) async fn screening_disagreements(
    pool: &AnyPool,
    threshold: f64,
    limit: i64,
) -> Result<Vec<JsonValue>> {
    let limit = limit.clamp(1, 500);
    Ok(sqlx::query(&format!(
        "SELECT s.item_id, s.symbol, s.created_at, s.marker_screened, s.instruction_shaped,
                COALESCE(NULLIF(s.evidence_title, ''), i.title) AS title, i.source_name
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
/// Partial grades are selected too.
///
/// `partial` describes the *model's* answers: some question came back missing.
/// The numeric half is arithmetic over the stored prompt and owes nothing to
/// those answers, and the read path shows partial grades to anyone looking --
/// so excluding them left 37 rows carrying findings from six superseded
/// methods, with no cycle able to reach them. Recomputing changes
/// `numeric_checks` and nothing else: the status stays `partial` and the
/// missing answers stay missing.
///
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
         WHERE purpose = $1 AND status IN ('completed', 'partial')
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
         WHERE purpose = $1 AND status IN ('completed', 'partial', 'refused')
           AND subject IS NOT NULL
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

/// Subjects refused at a question version. Editorial items are judged per
/// (item, symbol) pair, so "judged" there cannot be a subject set; a refusal
/// can, because it is a property of the item's text.
pub(crate) async fn refused_subjects_at_version(
    pool: &AnyPool,
    purpose: &str,
    version: &str,
) -> Result<std::collections::HashSet<String>> {
    Ok(sqlx::query(
        "SELECT subject FROM jev_requests
         WHERE purpose = $1 AND status = 'refused' AND subject IS NOT NULL
           AND result_json LIKE $2",
    )
    .bind(purpose)
    .bind(format!("%\"version\":\"{version}\"%"))
    .fetch_all(pool)
    .await
    .context("reading refused Jev subjects")?
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

/// Every resolved Jev version that has answered, from the request ledger.
///
/// The ledger is never pruned, so each rollover stays a dated boundary.
/// `created_at` holds RFC 3339 text in more than one fractional style;
/// lexical `MIN`/`MAX` order those correctly to the second, which is the
/// precision reported.
pub(crate) async fn model_epochs(
    pool: &AnyPool,
) -> Result<Vec<crate::models::JevModelEpochPayload>> {
    let rows = sqlx::query(
        "SELECT model_resolved, purpose, COUNT(*) AS requests,
                MIN(created_at) AS first_seen_at, MAX(created_at) AS last_seen_at
         FROM jev_requests
         WHERE model_resolved IS NOT NULL AND model_resolved <> ''
         GROUP BY model_resolved, purpose",
    )
    .fetch_all(pool)
    .await
    .context("reading Jev model epochs")?
    .iter()
    .map(row_to_json)
    .collect::<Vec<_>>();
    Ok(epochs_from_rows(&rows))
}

/// Folds per-(version, purpose) rows into one epoch per version, oldest first.
pub(crate) fn epochs_from_rows(rows: &[JsonValue]) -> Vec<crate::models::JevModelEpochPayload> {
    use chrono::{DateTime, Utc};
    let parse = |row: &JsonValue, key: &str| {
        row.get(key)
            .and_then(JsonValue::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|at| at.with_timezone(&Utc))
    };
    let mut by_model: std::collections::BTreeMap<
        String,
        (
            DateTime<Utc>,
            DateTime<Utc>,
            std::collections::BTreeMap<String, i64>,
        ),
    > = std::collections::BTreeMap::new();
    for row in rows {
        let Some(model) = row
            .get("model_resolved")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            continue;
        };
        let (Some(first), Some(last)) = (parse(row, "first_seen_at"), parse(row, "last_seen_at"))
        else {
            continue;
        };
        let purpose = row
            .get("purpose")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown")
            .to_string();
        let requests = row.get("requests").and_then(JsonValue::as_i64).unwrap_or(0);
        let entry = by_model.entry(model.to_string()).or_insert((
            first,
            last,
            std::collections::BTreeMap::new(),
        ));
        entry.0 = entry.0.min(first);
        entry.1 = entry.1.max(last);
        *entry.2.entry(purpose).or_default() += requests;
    }
    let seconds = |at: DateTime<Utc>| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut epochs: Vec<_> = by_model
        .into_iter()
        .map(|(model, (first, last, by_purpose))| {
            (
                first,
                crate::models::JevModelEpochPayload {
                    model,
                    first_seen_at: seconds(first),
                    last_seen_at: seconds(last),
                    requests: by_purpose.values().sum(),
                    by_purpose,
                },
            )
        })
        .collect();
    epochs.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.model.cmp(&right.1.model))
    });
    epochs.into_iter().map(|(_, epoch)| epoch).collect()
}

/// The configured alias, the version answering now, and every epoch.
///
/// The current version is the one behind the latest recorded request. Epochs
/// overlap when an older version answered again after a newer one began,
/// which a staged provider rollout can do; the boundary is then a span.
pub(crate) fn models_payload(
    configured_model: &str,
    epochs: Vec<crate::models::JevModelEpochPayload>,
) -> crate::models::JevModelsPayload {
    let current_model = epochs
        .iter()
        .max_by(|left, right| left.last_seen_at.cmp(&right.last_seen_at))
        .map(|epoch| epoch.model.clone());
    let overlapping = epochs
        .windows(2)
        .any(|pair| pair[0].last_seen_at > pair[1].first_seen_at);
    crate::models::JevModelsPayload {
        configured_model: configured_model.to_string(),
        current_model,
        epochs,
        overlapping,
        policy: crate::jev::ALIAS_ROLLOVER_POLICY.to_string(),
        unavailable: false,
    }
}

const PROVIDER_STATE_KEY: &str = "provider";
/// The first deferral after a failure: one worker cycle.
const BACKOFF_BASE_SECONDS: i64 = 600;
/// Consecutive failures stop doubling the wait here.
const BACKOFF_CAP_SECONDS: i64 = 4 * 3_600;
/// A provider's Retry-After is honoured up to a day. Beyond that it is more
/// likely a malformed header than a real instruction, and a gate shut for
/// longer would hide a recovered provider.
const RETRY_AFTER_CAP_SECONDS: u64 = 24 * 3_600;

fn seconds_text(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// When provider calls may resume after `consecutive_failures` in a row.
///
/// The longer of the provider's own Retry-After and an exponential backoff:
/// one cycle after the first failure, doubling to four hours. Never earlier
/// than the provider asked.
pub(crate) fn backoff_until(
    now: chrono::DateTime<chrono::Utc>,
    consecutive_failures: i64,
    retry_after_seconds: Option<u64>,
) -> chrono::DateTime<chrono::Utc> {
    let doublings = (consecutive_failures - 1).clamp(0, 30) as u32;
    let backoff = BACKOFF_BASE_SECONDS
        .saturating_mul(2_i64.saturating_pow(doublings))
        .min(BACKOFF_CAP_SECONDS);
    let asked = retry_after_seconds
        .unwrap_or(0)
        .min(RETRY_AFTER_CAP_SECONDS) as i64;
    now + chrono::Duration::seconds(backoff.max(asked))
}

pub(crate) async fn provider_state_row(pool: &AnyPool) -> Result<Option<JsonValue>> {
    Ok(sqlx::query(
        "SELECT updated_at, consecutive_failures, not_before, last_failure_at,
                last_failure_kind, last_retry_after_seconds
         FROM jev_provider_state WHERE singleton_key = $1",
    )
    .bind(PROVIDER_STATE_KEY)
    .fetch_optional(pool)
    .await
    .context("reading the Jev provider state")?
    .as_ref()
    .map(row_to_json))
}

pub(crate) fn provider_state_from_row(
    row: Option<&JsonValue>,
    now: chrono::DateTime<chrono::Utc>,
) -> crate::models::JevProviderStatePayload {
    let Some(row) = row else {
        return crate::models::JevProviderStatePayload::default();
    };
    let text = |key: &str| {
        row.get(key)
            .and_then(JsonValue::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let not_before = text("not_before");
    let deferred = not_before
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| at.with_timezone(&chrono::Utc) > now);
    crate::models::JevProviderStatePayload {
        consecutive_failures: row
            .get("consecutive_failures")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        not_before,
        deferred,
        last_failure_at: text("last_failure_at"),
        last_failure_kind: text("last_failure_kind"),
        last_retry_after_seconds: row
            .get("last_retry_after_seconds")
            .and_then(JsonValue::as_i64),
    }
}

/// When provider calls are deferred until, if they are deferred now.
pub(crate) async fn provider_deferred_until(
    pool: &AnyPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<String>> {
    let state = provider_state_from_row(provider_state_row(pool).await?.as_ref(), now);
    Ok(state.deferred.then_some(state.not_before).flatten())
}

/// An answer closes nothing and reopens the gate. Written only when there is
/// something to reset, so a healthy run costs one no-op UPDATE per call.
pub(crate) async fn note_provider_success(
    pool: &AnyPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE jev_provider_state
         SET consecutive_failures = 0, not_before = NULL, updated_at = $1
         WHERE singleton_key = $2 AND (consecutive_failures <> 0 OR not_before IS NOT NULL)",
    )
    .bind(seconds_text(now))
    .bind(PROVIDER_STATE_KEY)
    .execute(pool)
    .await
    .context("reopening the Jev provider gate")?;
    Ok(())
}

/// Counts a provider failure and shuts the gate until the backoff ends.
/// Returns the time it shuts until.
pub(crate) async fn note_provider_failure(
    pool: &AnyPool,
    now: chrono::DateTime<chrono::Utc>,
    kind: &str,
    retry_after_seconds: Option<u64>,
) -> Result<String> {
    let previous = provider_state_from_row(provider_state_row(pool).await?.as_ref(), now);
    let consecutive = previous.consecutive_failures + 1;
    let not_before = seconds_text(backoff_until(now, consecutive, retry_after_seconds));
    let now = seconds_text(now);
    sqlx::query(
        "INSERT INTO jev_provider_state
            (singleton_key, updated_at, consecutive_failures, not_before, last_failure_at,
             last_failure_kind, last_retry_after_seconds)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (singleton_key) DO UPDATE SET
            updated_at = excluded.updated_at,
            consecutive_failures = excluded.consecutive_failures,
            not_before = excluded.not_before,
            last_failure_at = excluded.last_failure_at,
            last_failure_kind = excluded.last_failure_kind,
            last_retry_after_seconds = excluded.last_retry_after_seconds",
    )
    .bind(PROVIDER_STATE_KEY)
    .bind(&now)
    .bind(consecutive)
    .bind(&not_before)
    .bind(&now)
    .bind(kind)
    .bind(retry_after_seconds.map(|seconds| seconds.min(i64::MAX as u64) as i64))
    .execute(pool)
    .await
    .context("shutting the Jev provider gate")?;
    Ok(not_before)
}

/// The worker step that re-derives numeric findings. It calls no provider, so
/// it has no purpose in the request ledger; the other three steps are named
/// by the purpose they record under.
pub(crate) const STEP_NUMERIC_RECOMPUTE: &str = "numeric_recompute";
/// The row that records the cycle itself.
pub(crate) const WORKER_CYCLE: &str = "cycle";

/// Worker steps in the order one cycle runs them.
const WORKER_STEPS: [&str; 4] = [
    STEP_NUMERIC_RECOMPUTE,
    PURPOSE_REPORT_GRADING,
    PURPOSE_ERROR_CLASSIFICATION,
    PURPOSE_EDITORIAL_ITEM,
];

/// Whether a step summary ran clean, and whether anything in it failed.
///
/// A step that judged nine subjects and then lost the tenth to an outage
/// reports `ok`, so `ok` alone would date the last clean run to that cycle.
/// A disabled step is neither.
fn step_outcome(summary: &JsonValue) -> (bool, bool) {
    let status = summary.get("status").and_then(JsonValue::as_str);
    let failed = summary
        .get("failed")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    let any_failure = status == Some("error") || failed > 0;
    (status == Some("ok") && !any_failure, any_failure)
}

/// Records one finished worker cycle: each step's summary, then the cycle.
pub(crate) async fn record_worker_cycle(
    pool: &AnyPool,
    started_at: &str,
    finished_at: &str,
    steps: &[(&str, &JsonValue)],
    unrecorded: &Unrecorded,
) -> Result<()> {
    let mut any_failure = false;
    for (step, summary) in steps {
        let (clean, failed) = step_outcome(summary);
        any_failure |= failed;
        let status = summary
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown");
        upsert_worker_status(
            pool,
            step,
            finished_at,
            status,
            clean.then_some(finished_at),
            failed.then_some(finished_at),
            summary,
        )
        .await?;
    }
    upsert_worker_status(
        pool,
        WORKER_CYCLE,
        finished_at,
        "ok",
        (!any_failure).then_some(finished_at),
        any_failure.then_some(finished_at),
        &serde_json::json!({
            "started_at": started_at,
            "finished_at": finished_at,
            "unrecorded_pending": unrecorded.pending(),
            "unrecorded_pending_cost_usd": unrecorded.pending_cost_usd(),
            "unrecorded_dropped": unrecorded.dropped(),
        }),
    )
    .await
}

async fn upsert_worker_status(
    pool: &AnyPool,
    step: &str,
    updated_at: &str,
    status: &str,
    clean_at: Option<&str>,
    failed_at: Option<&str>,
    summary: &JsonValue,
) -> Result<()> {
    // COALESCE keeps the earlier moment when this cycle did not set one, so
    // a failing cycle cannot erase when the step last ran clean.
    sqlx::query(
        "INSERT INTO jev_worker_status
            (step, updated_at, status, last_clean_at, last_failed_at, summary_json)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (step) DO UPDATE SET
            updated_at = excluded.updated_at,
            status = excluded.status,
            last_clean_at = COALESCE(excluded.last_clean_at, jev_worker_status.last_clean_at),
            last_failed_at = COALESCE(excluded.last_failed_at, jev_worker_status.last_failed_at),
            summary_json = excluded.summary_json",
    )
    .bind(step)
    .bind(updated_at)
    .bind(status)
    .bind(clean_at)
    .bind(failed_at)
    .bind(summary.to_string())
    .execute(pool)
    .await
    .with_context(|| format!("recording Jev worker status for {step}"))?;
    Ok(())
}

pub(crate) async fn worker_status_rows(pool: &AnyPool) -> Result<Vec<JsonValue>> {
    Ok(sqlx::query(
        "SELECT step, updated_at, status, last_clean_at, last_failed_at, summary_json
         FROM jev_worker_status",
    )
    .fetch_all(pool)
    .await
    .context("reading Jev worker status")?
    .iter()
    .map(row_to_json)
    .collect())
}

/// Per-purpose call outcomes, cost sources and the latest failure.
///
/// Three reads: the grouped counts, the failures after each purpose's latest
/// answer, and each purpose's latest failure text. Each joins a per-purpose
/// aggregate rather than a correlated subquery, which would rescan the ledger
/// once per failure row. Timestamps compare as text, which is exact here
/// because one purpose always writes one format.
pub(crate) async fn purpose_call_rows(
    pool: &AnyPool,
) -> Result<(Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>)> {
    let read = |sql: &'static str| async move {
        sqlx::query(sql)
            .fetch_all(pool)
            .await
            .map(|rows| rows.iter().map(row_to_json).collect::<Vec<_>>())
    };
    let grouped = read(
        "SELECT purpose, status, cost_source, COUNT(*) AS requests,
                MAX(created_at) AS last_at, SUM(cost_usd) AS cost_usd,
                SUM(issue_count) AS issues, SUM(attempts) AS attempts,
                SUM(CASE WHEN attempts > 1 THEN 1 ELSE 0 END) AS retried,
                MAX(input_tokens) AS max_input_tokens,
                MIN(CASE WHEN request_bytes > 0 AND input_tokens > 0
                         THEN CAST(request_bytes AS DOUBLE PRECISION) / input_tokens END)
                    AS min_bytes_per_token
         FROM jev_requests
         GROUP BY purpose, status, cost_source",
    )
    .await
    .context("reading Jev call outcomes")?;
    let since_answer = read(
        "SELECT r.purpose, COUNT(*) AS failures
         FROM jev_requests r
         LEFT JOIN (SELECT purpose, MAX(created_at) AS answered_at
                    FROM jev_requests WHERE status IN ('completed', 'partial')
                    GROUP BY purpose) a ON a.purpose = r.purpose
         WHERE r.status = 'error' AND r.created_at > COALESCE(a.answered_at, '')
         GROUP BY r.purpose",
    )
    .await
    .context("reading Jev failures since the latest answer")?;
    let latest_failures = read(
        "SELECT r.purpose, r.error_text
         FROM jev_requests r
         JOIN (SELECT purpose, MAX(created_at) AS failed_at
               FROM jev_requests WHERE status = 'error'
               GROUP BY purpose) l ON l.purpose = r.purpose AND l.failed_at = r.created_at
         WHERE r.status = 'error'",
    )
    .await
    .context("reading the latest Jev failure per purpose")?;
    Ok((grouped, since_answer, latest_failures))
}

/// A bounded label for a stored failure.
///
/// The stored text can carry two thousand characters of a provider's body,
/// which has no place on a dashboard; the label says which failure it was
/// and the scheduler log keeps the rest.
pub(crate) fn failure_kind(error_text: &str) -> String {
    let text = error_text.trim();
    if text.contains("non-JSON body") {
        return "non_json_response".to_string();
    }
    if let Some(status) = text
        .strip_prefix("Jev returned ")
        .and_then(|rest| rest.get(..3))
        .and_then(|code| code.parse::<u16>().ok())
    {
        return format!("http_{status}");
    }
    if text.starts_with("transport error") {
        return "transport".to_string();
    }
    if text.contains("above the configured maximum") {
        return "over_budget".to_string();
    }
    "other".to_string()
}

pub(crate) fn purpose_calls_from_rows(
    grouped: &[JsonValue],
    since_answer: &[JsonValue],
    latest_failures: &[JsonValue],
) -> Vec<crate::models::JevPurposeCallsPayload> {
    let text =
        |row: &JsonValue, key: &str| row.get(key).and_then(JsonValue::as_str).map(str::to_string);
    let count = |row: &JsonValue, key: &str| row.get(key).and_then(JsonValue::as_i64).unwrap_or(0);
    let later = |current: Option<String>, candidate: Option<String>| match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (current, candidate) => current.or(candidate),
    };

    let mut by_purpose: std::collections::BTreeMap<String, crate::models::JevPurposeCallsPayload> =
        std::collections::BTreeMap::new();
    for row in grouped {
        let Some(purpose) = text(row, "purpose") else {
            continue;
        };
        let calls = by_purpose.entry(purpose.clone()).or_insert_with(|| {
            crate::models::JevPurposeCallsPayload {
                purpose,
                ..Default::default()
            }
        });
        let requests = count(row, "requests");
        let status = text(row, "status").unwrap_or_default();
        if status == STATUS_REFUSED {
            // Never sent: not a call, an attempt, or a cost.
            calls.refused += requests;
            continue;
        }
        calls.requests += requests;
        if let Some(tokens) = row.get("max_input_tokens").and_then(JsonValue::as_i64) {
            calls.max_input_tokens = Some(calls.max_input_tokens.unwrap_or(0).max(tokens));
        }
        if let Some(ratio) = row.get("min_bytes_per_token").and_then(JsonValue::as_f64) {
            calls.min_bytes_per_token = Some(
                calls
                    .min_bytes_per_token
                    .map_or(ratio, |known| known.min(ratio)),
            );
        }
        calls.attempts += count(row, "attempts");
        calls.retried += count(row, "retried");
        match status.as_str() {
            STATUS_COMPLETED => calls.completed += requests,
            "partial" => {
                calls.partial += requests;
                calls.partial_issue_count += count(row, "issues");
            }
            STATUS_ERROR => {
                calls.failed += requests;
                calls.last_failed_at = later(calls.last_failed_at.take(), text(row, "last_at"));
            }
            _ => {}
        }
        if !matches!(status.as_str(), STATUS_COMPLETED | "partial") {
            continue;
        }
        calls.last_answered_at = later(calls.last_answered_at.take(), text(row, "last_at"));
        let priced = match text(row, "cost_source").as_deref() {
            Some("billed") | Some("upstream_byok") => {
                calls.billed += requests;
                true
            }
            Some(crate::jev::COST_SOURCE_RATE_CARD) => {
                calls.rate_card += requests;
                true
            }
            _ => {
                calls.unpriced += requests;
                false
            }
        };
        if let Some(cost) = priced
            .then(|| row.get("cost_usd").and_then(JsonValue::as_f64))
            .flatten()
        {
            calls.known_cost_usd = Some(calls.known_cost_usd.unwrap_or(0.0) + cost);
        }
    }
    for row in since_answer {
        if let Some(calls) = text(row, "purpose").and_then(|purpose| by_purpose.get_mut(&purpose)) {
            calls.failures_since_answer = count(row, "failures");
        }
    }
    for row in latest_failures {
        if let Some(calls) = text(row, "purpose").and_then(|purpose| by_purpose.get_mut(&purpose)) {
            if calls.last_failure_kind.is_none() {
                calls.last_failure_kind = Some(failure_kind(
                    row.get("error_text")
                        .and_then(JsonValue::as_str)
                        .unwrap_or_default(),
                ));
            }
        }
    }
    by_purpose.into_values().collect()
}

/// Reads one worker status row. The summary fields are the step's own
/// report of its latest cycle.
fn summary_of(row: &JsonValue) -> JsonValue {
    row.get("summary_json")
        .and_then(|value| match value {
            JsonValue::String(text) => serde_json::from_str::<JsonValue>(text).ok(),
            other => Some(other.clone()),
        })
        .unwrap_or(JsonValue::Null)
}

fn worker_step_from_row(row: &JsonValue) -> crate::models::JevWorkerStepPayload {
    let text = |key: &str| {
        row.get(key)
            .and_then(JsonValue::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let summary = summary_of(row);
    let number = |key: &str| summary.get(key).and_then(JsonValue::as_i64);
    crate::models::JevWorkerStepPayload {
        step: text("step").unwrap_or_default(),
        updated_at: text("updated_at").unwrap_or_default(),
        status: text("status").unwrap_or_default(),
        last_clean_at: text("last_clean_at"),
        last_failed_at: text("last_failed_at"),
        processed: number("processed").unwrap_or(0),
        failed: number("failed").unwrap_or(0),
        waiting: number("waiting"),
        remaining: number("remaining"),
        waiting_is_lower_bound: summary
            .get("waiting_is_lower_bound")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        stage: summary
            .get("stage")
            .and_then(JsonValue::as_str)
            .map(str::to_string),
        refused: number("refused").unwrap_or(0),
    }
}

fn screening_from(
    agreement: &JsonValue,
    disagreements: &[JsonValue],
) -> crate::models::JevScreeningPayload {
    let count = |key: &str| agreement.get(key).and_then(JsonValue::as_i64).unwrap_or(0);
    crate::models::JevScreeningPayload {
        threshold: agreement
            .get("threshold")
            .and_then(JsonValue::as_f64)
            .unwrap_or(INSTRUCTION_SHAPED_THRESHOLD),
        judged_count: count("judged_count"),
        both_flagged: count("both_flagged"),
        both_clear: count("both_clear"),
        marker_only: count("marker_only"),
        jev_only: count("jev_only"),
        disagreements: disagreements
            .iter()
            .filter_map(|row| {
                let text = |key: &str| {
                    row.get(key)
                        .and_then(JsonValue::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                };
                Some(crate::models::JevScreeningDisagreementPayload {
                    item_id: text("item_id")?,
                    symbol: text("symbol").unwrap_or_default(),
                    created_at: text("created_at").unwrap_or_default(),
                    marker_screened: row
                        .get("marker_screened")
                        .and_then(JsonValue::as_i64)
                        .unwrap_or(0)
                        != 0,
                    instruction_shaped: row.get("instruction_shaped")?.as_f64()?,
                    title: text("title"),
                    source_name: text("source_name"),
                })
            })
            .collect(),
    }
}

/// How many cycle intervals may pass before an enabled worker counts as silent.
const SILENT_AFTER_INTERVALS: i64 = 3;

/// Every read the panel makes, as it came back.
pub(crate) struct OperatorReads {
    pub worker: Result<Vec<JsonValue>>,
    pub provider: Result<Option<JsonValue>>,
    pub calls: Result<(Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>)>,
    pub agreement: Result<JsonValue>,
    pub disagreements: Result<Vec<JsonValue>>,
}

/// Builds the panel from its reads. A read that failed is named, and its
/// section stays empty rather than reading as nothing to report.
pub(crate) fn operator_payload(
    status: &str,
    now: chrono::DateTime<chrono::Utc>,
    cycle_interval_seconds: i64,
    budget: crate::models::JevBudgetPayload,
    reads: OperatorReads,
) -> crate::models::JevOperatorPayload {
    let mut unavailable_sources = Vec::new();
    let mut note = |source: &str, err: &anyhow::Error| {
        tracing::warn!("Jev operator panel: {source} unavailable: {err:#}");
        unavailable_sources.push(source.to_string());
    };

    let mut cycle_summary = JsonValue::Null;
    let (last_cycle_at, steps) = match &reads.worker {
        Ok(rows) => {
            let mut last_cycle_at = None;
            let mut steps = Vec::new();
            for row in rows {
                let step = worker_step_from_row(row);
                if step.step == WORKER_CYCLE {
                    last_cycle_at = Some(step.updated_at);
                    cycle_summary = summary_of(row);
                } else {
                    steps.push(step);
                }
            }
            let rank = |step: &str| {
                WORKER_STEPS
                    .iter()
                    .position(|known| *known == step)
                    .unwrap_or(WORKER_STEPS.len())
            };
            steps.sort_by(|left, right| {
                rank(&left.step)
                    .cmp(&rank(&right.step))
                    .then_with(|| left.step.cmp(&right.step))
            });
            (last_cycle_at, steps)
        }
        Err(err) => {
            note("worker status", err);
            (None, Vec::new())
        }
    };
    let provider = match &reads.provider {
        Ok(row) => provider_state_from_row(row.as_ref(), now),
        Err(err) => {
            note("provider state", err);
            crate::models::JevProviderStatePayload::default()
        }
    };
    let purposes = match &reads.calls {
        Ok((grouped, since_answer, latest_failures)) => {
            purpose_calls_from_rows(grouped, since_answer, latest_failures)
        }
        Err(err) => {
            note("request ledger", err);
            Vec::new()
        }
    };
    let agreement = match &reads.agreement {
        Ok(agreement) => agreement.clone(),
        Err(err) => {
            note("screening agreement", err);
            JsonValue::Null
        }
    };
    let disagreements = match &reads.disagreements {
        Ok(rows) => rows.as_slice(),
        Err(err) => {
            note("screening disagreements", err);
            &[]
        }
    };

    let worker_unreadable = reads.worker.is_err();
    let worker_silent = status == "ok"
        && !worker_unreadable
        && last_cycle_at
            .as_deref()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_none_or(|at| {
                (now - at.with_timezone(&chrono::Utc)).num_seconds()
                    > SILENT_AFTER_INTERVALS * cycle_interval_seconds
            });
    crate::models::JevOperatorPayload {
        status: status.to_string(),
        last_cycle_at,
        cycle_interval_seconds,
        worker_silent,
        steps,
        provider,
        unrecorded_pending: cycle_summary
            .get("unrecorded_pending")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        unrecorded_pending_cost_usd: cycle_summary
            .get("unrecorded_pending_cost_usd")
            .and_then(JsonValue::as_f64),
        unrecorded_dropped: cycle_summary
            .get("unrecorded_dropped")
            .and_then(JsonValue::as_i64)
            .unwrap_or(0),
        purposes,
        budget,
        screening: screening_from(&agreement, disagreements),
        unavailable_sources,
    }
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
            attempt_log: Vec::new(),
            request_bytes: 2_310,
            estimated_input_tokens: 1_540,
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
            &[],
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
            &[],
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

    /// Records one completed request answered by `model` at `created_at`.
    async fn answered_by(
        pool: &AnyPool,
        id: &str,
        created_at: &str,
        purpose: &str,
        subject: &str,
        model: &str,
    ) {
        let mut answered = response();
        answered.model_resolved = Some(model.to_string());
        record_success(
            pool,
            RecordedRequest {
                id,
                created_at,
                purpose,
                subject: Some(subject),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            &answered,
            Some(&serde_json::json!({"version": "v16"})),
        )
        .await
        .expect("a recorded request");
    }

    /// The 2026-10-15 rollover as the ledger records it: two versions, one
    /// boundary, each request kept under the version that answered it, and
    /// the ledger's two timestamp styles read as one.
    #[tokio::test]
    async fn the_request_ledger_yields_one_epoch_per_resolved_version() {
        let pool = pool().await;
        let old = "typesafe/jev-1.13-20260917";
        let new = "typesafe/jev-1.14-20261014";
        answered_by(
            &pool,
            "a",
            "2026-09-20T12:24:23.149293045+00:00",
            PURPOSE_REPORT_GRADING,
            "300",
            old,
        )
        .await;
        answered_by(
            &pool,
            "b",
            "2026-09-20T12:24:27.083538005+00:00",
            PURPOSE_ERROR_CLASSIFICATION,
            "e1",
            old,
        )
        .await;
        answered_by(
            &pool,
            "c",
            "2026-10-14T18:21:53.325Z",
            PURPOSE_REPORT_GRADING,
            "301",
            old,
        )
        .await;
        answered_by(
            &pool,
            "d",
            "2026-10-15T08:55:12.398Z",
            PURPOSE_REPORT_GRADING,
            "302",
            new,
        )
        .await;

        let epochs = model_epochs(&pool).await.expect("epochs");
        let models = models_payload("~typesafe/jev-latest", epochs);

        assert_eq!(models.epochs.len(), 2);
        assert_eq!(models.epochs[0].model, old);
        assert_eq!(models.epochs[0].first_seen_at, "2026-09-20T12:24:23Z");
        assert_eq!(models.epochs[0].last_seen_at, "2026-10-14T18:21:53Z");
        assert_eq!(models.epochs[0].requests, 3);
        assert_eq!(models.epochs[0].by_purpose[PURPOSE_REPORT_GRADING], 2);
        assert_eq!(models.epochs[0].by_purpose[PURPOSE_ERROR_CLASSIFICATION], 1);
        assert_eq!(models.epochs[1].model, new);
        assert_eq!(models.epochs[1].first_seen_at, "2026-10-15T08:55:12Z");
        assert_eq!(models.current_model.as_deref(), Some(new));
        assert!(!models.overlapping);
        assert_eq!(models.configured_model, "~typesafe/jev-latest");
        assert_eq!(models.policy, crate::jev::ALIAS_ROLLOVER_POLICY);
    }

    /// A failed call names no version, so it cannot invent an epoch.
    #[test]
    fn a_request_without_a_resolved_version_is_no_epoch() {
        let rows = vec![
            serde_json::json!({"model_resolved": null, "purpose": "report_grading", "requests": 4,
                               "first_seen_at": "2026-09-21T05:00:00Z", "last_seen_at": "2026-09-21T06:00:00Z"}),
            serde_json::json!({"model_resolved": "  ", "purpose": "report_grading", "requests": 1,
                               "first_seen_at": "2026-09-21T05:00:00Z", "last_seen_at": "2026-09-21T05:00:00Z"}),
        ];
        assert!(epochs_from_rows(&rows).is_empty());
        let models = models_payload("~typesafe/jev-latest", Vec::new());
        assert_eq!(models.current_model, None);
        assert!(!models.overlapping);
    }

    /// A staged provider rollout can serve the older version after the newer
    /// one began. The current version is then the older one, and the
    /// boundary is a span the payload has to admit to.
    #[test]
    fn an_older_version_answering_again_is_reported_as_an_overlap() {
        let rows = vec![
            serde_json::json!({"model_resolved": "typesafe/jev-1.13-20260917", "purpose": "report_grading",
                               "requests": 10, "first_seen_at": "2026-09-20T12:00:00Z",
                               "last_seen_at": "2026-10-16T09:00:00Z"}),
            serde_json::json!({"model_resolved": "typesafe/jev-1.14-20261014", "purpose": "report_grading",
                               "requests": 2, "first_seen_at": "2026-10-15T08:55:12Z",
                               "last_seen_at": "2026-10-15T18:00:00Z"}),
        ];
        let models = models_payload("~typesafe/jev-latest", epochs_from_rows(&rows));
        assert!(models.overlapping);
        assert_eq!(
            models.current_model.as_deref(),
            Some("typesafe/jev-1.13-20260917")
        );
        assert_eq!(
            models.epochs[1].model, "typesafe/jev-1.14-20261014",
            "epochs stay oldest first"
        );
    }

    /// The rollover policy, pinned: a subject judged by an older resolved
    /// version still counts as judged at its question set, so a rollover
    /// spends nothing re-asking it. Only a question-set change re-asks.
    #[tokio::test]
    async fn a_rollover_does_not_reselect_a_subject_for_regrading() {
        let pool = pool().await;
        answered_by(
            &pool,
            "old",
            "2026-10-14T18:21:53Z",
            PURPOSE_REPORT_GRADING,
            "399",
            "typesafe/jev-1.13-20260917",
        )
        .await;
        answered_by(
            &pool,
            "new",
            "2026-10-15T08:55:12Z",
            PURPOSE_REPORT_GRADING,
            "400",
            "typesafe/jev-1.14-20261014",
        )
        .await;

        let judged = judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v16")
            .await
            .expect("judged subjects");

        assert!(
            judged.contains("399"),
            "judged by the previous version, still judged"
        );
        assert!(judged.contains("400"));
    }

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .expect("a timestamp")
            .with_timezone(&chrono::Utc)
    }

    fn reads(
        worker: Result<Vec<JsonValue>>,
        calls: Result<(Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>)>,
    ) -> OperatorReads {
        OperatorReads {
            worker,
            provider: Ok(None),
            calls,
            agreement: Ok(serde_json::json!({})),
            disagreements: Ok(Vec::new()),
        }
    }

    /// A step that judged nine subjects and lost the tenth reports `ok`, and
    /// must still date its last clean run to before that cycle. A later
    /// failing cycle must not erase when it last ran clean either.
    #[tokio::test]
    async fn a_failing_cycle_keeps_when_the_step_last_ran_clean() {
        let pool = pool().await;
        let clean = serde_json::json!({"status": "ok", "failed": 0, "processed": 3});
        let partly_failed = serde_json::json!({"status": "ok", "failed": 1, "processed": 9});
        record_worker_cycle(
            &pool,
            "2026-10-16T08:00:00Z",
            "2026-10-16T08:01:00Z",
            &[(PURPOSE_REPORT_GRADING, &clean)],
            &Unrecorded::default(),
        )
        .await
        .expect("first cycle records");
        record_worker_cycle(
            &pool,
            "2026-10-16T08:11:00Z",
            "2026-10-16T08:12:00Z",
            &[(PURPOSE_REPORT_GRADING, &partly_failed)],
            &Unrecorded::default(),
        )
        .await
        .expect("second cycle records");

        let panel = operator_payload(
            "ok",
            at("2026-10-16T08:15:00Z"),
            600,
            Default::default(),
            reads(worker_status_rows(&pool).await, Ok(Default::default())),
        );
        let grading = &panel.steps[0];
        assert_eq!(grading.status, "ok");
        assert_eq!(grading.processed, 9);
        assert_eq!(grading.failed, 1);
        assert_eq!(
            grading.last_clean_at.as_deref(),
            Some("2026-10-16T08:01:00Z")
        );
        assert_eq!(
            grading.last_failed_at.as_deref(),
            Some("2026-10-16T08:12:00Z")
        );
        assert_eq!(panel.last_cycle_at.as_deref(), Some("2026-10-16T08:12:00Z"));
        assert!(!panel.worker_silent);
    }

    /// A disabled step did not run, so it is neither a clean run nor a failure.
    #[test]
    fn a_disabled_step_is_neither_clean_nor_failed() {
        assert_eq!(
            step_outcome(&serde_json::json!({"status": "disabled"})),
            (false, false)
        );
        assert_eq!(
            step_outcome(&serde_json::json!({"status": "error", "stage": "select"})),
            (false, true),
            "a step that failed before any work still failed"
        );
        assert_eq!(
            step_outcome(&serde_json::json!({"status": "ok", "failed": 0})),
            (true, false)
        );
    }

    /// The backlog is each step's own report, in the order a cycle runs the
    /// steps, and a selection that hit its limit says so.
    #[tokio::test]
    async fn the_panel_shows_the_backlog_each_step_saw() {
        let pool = pool().await;
        let editorial = serde_json::json!({"status": "ok", "failed": 0, "processed": 60,
            "waiting": 60, "remaining": 0, "waiting_is_lower_bound": true});
        let grading = serde_json::json!({"status": "ok", "failed": 0, "processed": 10,
            "waiting": 14, "remaining": 4, "waiting_is_lower_bound": false});
        let numeric = serde_json::json!({"status": "ok", "failed": 0, "processed": 0,
            "waiting": 0, "remaining": 0, "waiting_is_lower_bound": false});
        record_worker_cycle(
            &pool,
            "2026-10-16T08:00:00Z",
            "2026-10-16T08:02:00Z",
            &[
                (PURPOSE_EDITORIAL_ITEM, &editorial),
                (PURPOSE_REPORT_GRADING, &grading),
                (STEP_NUMERIC_RECOMPUTE, &numeric),
            ],
            &Unrecorded::default(),
        )
        .await
        .expect("cycle records");

        let panel = operator_payload(
            "ok",
            at("2026-10-16T08:05:00Z"),
            600,
            Default::default(),
            reads(worker_status_rows(&pool).await, Ok(Default::default())),
        );
        let steps: Vec<&str> = panel.steps.iter().map(|step| step.step.as_str()).collect();
        assert_eq!(
            steps,
            [
                STEP_NUMERIC_RECOMPUTE,
                PURPOSE_REPORT_GRADING,
                PURPOSE_EDITORIAL_ITEM
            ],
            "the cycle row is not a step"
        );
        assert_eq!(panel.steps[1].waiting, Some(14));
        assert_eq!(panel.steps[1].remaining, Some(4));
        assert!(!panel.steps[1].waiting_is_lower_bound);
        assert!(panel.steps[2].waiting_is_lower_bound);
    }

    /// Silence is judged against the cycle interval, and only for an enabled
    /// worker whose status could be read. An unreadable table is named, not
    /// reported as a stopped worker.
    #[tokio::test]
    async fn an_enabled_worker_with_no_recent_cycle_is_silent() {
        let pool = pool().await;
        let clean = serde_json::json!({"status": "ok", "failed": 0});
        record_worker_cycle(
            &pool,
            "2026-10-16T08:00:00Z",
            "2026-10-16T08:01:00Z",
            &[(PURPOSE_REPORT_GRADING, &clean)],
            &Unrecorded::default(),
        )
        .await
        .expect("cycle records");
        let panel_at = |status: &str, now: &str, worker: Result<Vec<JsonValue>>| {
            operator_payload(
                status,
                at(now),
                600,
                Default::default(),
                reads(worker, Ok(Default::default())),
            )
        };
        let rows = worker_status_rows(&pool).await.expect("rows");

        assert!(!panel_at("ok", "2026-10-16T08:30:00Z", Ok(rows.clone())).worker_silent);
        assert!(panel_at("ok", "2026-10-16T08:31:01Z", Ok(rows.clone())).worker_silent);
        assert!(
            !panel_at("disabled", "2026-10-17T08:00:00Z", Ok(rows)).worker_silent,
            "a disabled worker is not expected to run"
        );
        assert!(
            panel_at("ok", "2026-10-16T08:00:00Z", Ok(Vec::new())).worker_silent,
            "enabled with no cycle ever recorded"
        );
        let unreadable = panel_at(
            "ok",
            "2026-10-17T08:00:00Z",
            Err(anyhow::anyhow!("no such table")),
        );
        assert!(!unreadable.worker_silent);
        assert_eq!(unreadable.unavailable_sources, ["worker status"]);
    }

    async fn call(pool: &AnyPool, id: &str, created_at: &str, response: &JevResponse) {
        record_success(
            pool,
            RecordedRequest {
                id,
                created_at,
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some(id),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            response,
            None,
        )
        .await
        .expect("a recorded call");
    }

    async fn failure(pool: &AnyPool, id: &str, created_at: &str, error_text: &str) {
        record_failure(
            pool,
            RecordedRequest {
                id,
                created_at,
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some(id),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            error_text,
            &[],
        )
        .await
        .expect("a recorded failure");
    }

    /// Partial answers, failures and an answer with no cost are each counted
    /// apart, the known cost sums only priced calls, and a failure is shown
    /// by a bounded label rather than the provider's body.
    #[tokio::test]
    async fn call_outcomes_keep_partial_failed_and_unpriced_apart() {
        let pool = pool().await;
        let mut billed = response();
        billed.cost_source = "billed";
        billed.cost_usd = Some(0.00002);
        let rate_card = response();
        let mut unpriced = response();
        unpriced.cost_source = crate::jev::COST_SOURCE_NONE;
        unpriced.cost_usd = None;
        let mut partial = response();
        partial.issues = vec![
            crate::jev::JevAnswerIssue {
                id: "claim_0".into(),
                reason: "missing".into(),
            },
            crate::jev::JevAnswerIssue {
                id: "claim_1".into(),
                reason: "missing".into(),
            },
        ];

        failure(
            &pool,
            "f0",
            "2026-10-15T09:00:00Z",
            "transport error: timed out",
        )
        .await;
        call(&pool, "a", "2026-10-15T10:00:00Z", &billed).await;
        call(&pool, "b", "2026-10-15T11:00:00Z", &rate_card).await;
        call(&pool, "c", "2026-10-15T12:00:00Z", &unpriced).await;
        call(&pool, "d", "2026-10-15T13:00:00Z", &partial).await;
        failure(
            &pool,
            "f1",
            "2026-10-16T07:00:00Z",
            "transport error: timed out",
        )
        .await;
        failure(
            &pool,
            "f2",
            "2026-10-16T08:00:00Z",
            "Jev returned 429: {\"error\":\"secret-shaped provider body\"}",
        )
        .await;

        let (grouped, since_answer, latest) = purpose_call_rows(&pool).await.expect("ledger reads");
        let calls = purpose_calls_from_rows(&grouped, &since_answer, &latest);
        assert_eq!(calls.len(), 1);
        let calls = &calls[0];
        assert_eq!(calls.requests, 7);
        assert_eq!((calls.completed, calls.partial, calls.failed), (3, 1, 3));
        assert_eq!(calls.partial_issue_count, 2);
        assert_eq!(
            (calls.billed, calls.rate_card, calls.unpriced),
            (1, 2, 1),
            "the partial call was rate-card priced"
        );
        let known = calls.known_cost_usd.expect("a known subtotal");
        assert!(
            (known - (0.00002 + 2.0 * 0.0000176)).abs() < 1e-12,
            "{known}"
        );
        assert_eq!(
            calls.last_answered_at.as_deref(),
            Some("2026-10-15T13:00:00Z")
        );
        assert_eq!(
            calls.last_failed_at.as_deref(),
            Some("2026-10-16T08:00:00Z")
        );
        assert_eq!(
            calls.failures_since_answer, 2,
            "the failure before the first answer is not one of them"
        );
        assert_eq!(calls.last_failure_kind.as_deref(), Some("http_429"));
        assert!(
            !serde_json::to_string(&calls)
                .unwrap()
                .contains("provider body"),
            "the provider's text never reaches the panel"
        );
    }

    #[test]
    fn a_failure_label_is_drawn_from_a_bounded_set() {
        for (text, kind) in [
            ("Jev returned 503: upstream overloaded", "http_503"),
            (
                "Jev returned a non-JSON body: expected value",
                "non_json_response",
            ),
            ("transport error: connection reset", "transport"),
            (
                "Jev state is 90000 characters, above the configured maximum of 64000",
                "over_budget",
            ),
            ("database is locked", "other"),
            ("", "other"),
        ] {
            assert_eq!(failure_kind(text), kind, "{text}");
        }
    }

    /// Both directions are counted, and the title shown is the one Jev
    /// judged, not whatever the pruned-and-rewritten source row holds now.
    #[tokio::test]
    async fn the_screening_section_counts_both_directions_with_the_judged_title() {
        let pool = pool().await;
        for (id, marker_screened, instruction_shaped) in [
            ("agree-clean", false, 0.02),
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
        sqlx::query(
            "INSERT INTO editorial_research_items (id, title, source_name)
             VALUES ('marker-missed-it', 'rewritten later', 'Example feed')",
        )
        .execute(&pool)
        .await
        .expect("source row");

        let panel = operator_payload(
            "ok",
            at("2026-10-16T08:00:00Z"),
            600,
            Default::default(),
            OperatorReads {
                worker: Ok(Vec::new()),
                provider: Ok(None),
                calls: Ok(Default::default()),
                agreement: screening_agreement(&pool, INSTRUCTION_SHAPED_THRESHOLD).await,
                disagreements: screening_disagreements(&pool, INSTRUCTION_SHAPED_THRESHOLD, 20)
                    .await,
            },
        );
        let screening = &panel.screening;
        assert_eq!(screening.judged_count, 3);
        assert_eq!(
            (
                screening.both_clear,
                screening.marker_only,
                screening.jev_only
            ),
            (1, 1, 1)
        );
        let missed = screening
            .disagreements
            .iter()
            .find(|row| row.item_id == "marker-missed-it")
            .expect("the Jev-only item");
        assert!(!missed.marker_screened);
        assert_eq!(missed.title.as_deref(), Some("marker-missed-it"));
        assert_eq!(missed.source_name.as_deref(), Some("Example feed"));
        assert!(panel.unavailable_sources.is_empty());
    }

    /// A refusal is judged at its version, so it is not asked again, and is
    /// not a call: it costs nothing, counts as no answer, and stays out of
    /// the usage ledger.
    #[tokio::test]
    async fn a_refused_subject_is_judged_at_its_version_and_is_not_a_call() {
        let pool = pool().await;
        let meta = |id, created_at| RecordedRequest {
            id,
            created_at,
            purpose: PURPOSE_REPORT_GRADING,
            subject: Some("401"),
            model_requested: "~typesafe/jev-latest",
            question_count: 22,
        };
        failure(
            &pool,
            "f0",
            "2026-10-16T07:00:00Z",
            "transport error: reset",
        )
        .await;
        insert_request_row(
            &pool,
            &refused_row(
                meta("too-big", "2026-10-16T08:00:00Z"),
                "Jev request is an estimated 70000 tokens",
                "v16",
            ),
        )
        .await
        .expect("a refusal records");

        let judged = judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v16")
            .await
            .unwrap();
        assert!(judged.contains("401"), "not selected again at this version");
        assert!(
            judged_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v17")
                .await
                .unwrap()
                .is_empty(),
            "a new question set re-selects it"
        );
        assert_eq!(
            refused_subjects_at_version(&pool, PURPOSE_REPORT_GRADING, "v16")
                .await
                .unwrap()
                .len(),
            1
        );
        let usage = usage_rows(&pool, 100).await.unwrap();
        assert!(
            usage.iter().all(|row| row["status"] != STATUS_REFUSED),
            "no request was sent, so none is in the usage ledger"
        );

        let (grouped, since_answer, latest) = purpose_call_rows(&pool).await.unwrap();
        let calls = &purpose_calls_from_rows(&grouped, &since_answer, &latest)[0];
        assert_eq!(calls.refused, 1);
        assert_eq!(calls.requests, 1, "only the failure was a call");
        assert_eq!(
            calls.failures_since_answer, 1,
            "a refusal is not an answer, so it does not reset the count"
        );
    }

    /// Each answered call keeps its request size beside the provider's count,
    /// so whether the estimate overcounts is visible per purpose.
    #[tokio::test]
    async fn the_panel_shows_the_largest_call_and_the_tightest_bytes_per_token() {
        let pool = pool().await;
        call(&pool, "a", "2026-10-16T08:00:00Z", &response()).await;
        let mut bigger = response();
        bigger.usage.input_tokens = 1_000;
        bigger.request_bytes = 1_800;
        call(&pool, "b", "2026-10-16T08:10:00Z", &bigger).await;

        let (grouped, since_answer, latest) = purpose_call_rows(&pool).await.unwrap();
        let calls = &purpose_calls_from_rows(&grouped, &since_answer, &latest)[0];
        assert_eq!(calls.max_input_tokens, Some(1_000));
        let ratio = calls.min_bytes_per_token.expect("a ratio");
        assert!((ratio - 1.8).abs() < 1e-9, "{ratio}");
    }

    /// One cycle after the first failure, doubling to four hours, and never
    /// sooner than the provider asked. A day caps a Retry-After that is more
    /// likely malformed than meant.
    #[test]
    fn the_backoff_doubles_to_a_cap_and_never_undercuts_retry_after() {
        let now = at("2026-10-16T08:00:00Z");
        let wait = |failures: i64, retry_after: Option<u64>| {
            (backoff_until(now, failures, retry_after) - now).num_seconds()
        };
        assert_eq!(wait(1, None), 600);
        assert_eq!(wait(2, None), 1_200);
        assert_eq!(wait(3, None), 2_400);
        assert_eq!(wait(6, None), 14_400);
        assert_eq!(wait(40, None), 14_400, "capped, and no overflow");
        assert_eq!(wait(1, Some(3_600)), 3_600, "the provider's wait wins");
        assert_eq!(wait(6, Some(60)), 14_400, "the backoff wins");
        assert_eq!(wait(1, Some(10 * 86_400)), 86_400);
    }

    /// The gate is durable: a failure shuts it until the backoff ends, the
    /// count carries across failures, and one answer reopens it while keeping
    /// the record of what last failed.
    #[tokio::test]
    async fn the_provider_gate_shuts_on_failure_and_reopens_on_an_answer() {
        let pool = pool().await;
        let now = at("2026-10-16T08:00:00Z");
        assert_eq!(provider_deferred_until(&pool, now).await.unwrap(), None);

        let until = note_provider_failure(&pool, now, "http_429", Some(3_600))
            .await
            .expect("the gate shuts");
        assert_eq!(until, "2026-10-16T09:00:00Z");
        assert_eq!(
            provider_deferred_until(&pool, at("2026-10-16T08:59:59Z"))
                .await
                .unwrap()
                .as_deref(),
            Some("2026-10-16T09:00:00Z"),
            "held for the whole hour, across however many cycles"
        );
        assert_eq!(
            provider_deferred_until(&pool, at("2026-10-16T09:00:01Z"))
                .await
                .unwrap(),
            None
        );

        let later = at("2026-10-16T09:00:01Z");
        note_provider_failure(&pool, later, "transport", None)
            .await
            .expect("a second failure");
        let state =
            provider_state_from_row(provider_state_row(&pool).await.unwrap().as_ref(), later);
        assert_eq!(state.consecutive_failures, 2);
        assert_eq!(state.not_before.as_deref(), Some("2026-10-16T09:20:01Z"));
        assert_eq!(state.last_retry_after_seconds, None);

        note_provider_success(&pool, later).await.expect("reopens");
        let state =
            provider_state_from_row(provider_state_row(&pool).await.unwrap().as_ref(), later);
        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.deferred);
        assert_eq!(state.not_before, None);
        assert_eq!(
            state.last_failure_kind.as_deref(),
            Some("transport"),
            "history kept"
        );
    }

    /// A failed call counts the attempts it made, and its latency is theirs.
    #[tokio::test]
    async fn a_failed_call_records_the_attempts_it_made() {
        let pool = pool().await;
        let attempt = |n: u32, outcome: &str| crate::jev::JevAttempt {
            attempt: n,
            latency_ms: 100,
            http_status: Some(503),
            outcome: outcome.to_string(),
            retry_after_seconds: None,
        };
        let log = [
            attempt(1, "retried"),
            attempt(2, "retried"),
            attempt(3, "failed"),
        ];
        record_failure(
            &pool,
            RecordedRequest {
                id: "three-tries",
                created_at: "2026-10-16T08:00:00Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("401"),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            "Jev returned 503: overloaded",
            &log,
        )
        .await
        .expect("a recorded failure");

        let row = sqlx::query("SELECT attempts, latency_ms, attempts_json FROM jev_requests")
            .fetch_one(&pool)
            .await
            .expect("the row");
        let row = row_to_json(&row);
        assert_eq!(row["attempts"], 3);
        assert_eq!(row["latency_ms"], 300);
        // `row_to_json` decodes JSON-shaped text, so either form may arrive.
        let stored = match &row["attempts_json"] {
            JsonValue::String(text) => serde_json::from_str(text).expect("attempts json"),
            other => other.clone(),
        };
        let stored: Vec<crate::jev::JevAttempt> =
            serde_json::from_value(stored).expect("attempts decode");
        assert_eq!(stored, log);
    }

    async fn refuse_request_writes(pool: &AnyPool) {
        sqlx::query(
            "CREATE TRIGGER refuse_jev_requests BEFORE INSERT ON jev_requests
             BEGIN SELECT RAISE(ABORT, 'disk full'); END",
        )
        .execute(pool)
        .await
        .expect("trigger");
    }

    /// A row whose write failed is held, written once writes work again, and
    /// counted with its cost while it waits. Past the cap the oldest goes.
    #[tokio::test]
    async fn a_held_row_is_written_later_and_the_hold_is_bounded() {
        let pool = pool().await;
        let meta = |id| RecordedRequest {
            id,
            created_at: "2026-10-16T08:00:00Z",
            purpose: PURPOSE_REPORT_GRADING,
            subject: Some("401"),
            model_requested: "~typesafe/jev-latest",
            question_count: 3,
        };
        refuse_request_writes(&pool).await;
        let row = completed_row(meta("billed-1"), &response(), None);
        assert!(insert_request_row(&pool, &row).await.is_err());

        let mut held = Unrecorded::default();
        held.hold(row);
        assert_eq!(held.flush(&pool).await, 0, "still refused");
        assert_eq!(held.pending(), 1);
        assert_eq!(held.pending_cost_usd(), Some(0.0000176));

        sqlx::query("DROP TRIGGER refuse_jev_requests")
            .execute(&pool)
            .await
            .expect("drop trigger");
        assert_eq!(held.flush(&pool).await, 1);
        assert_eq!(held.pending(), 0);
        let cost: f64 = sqlx::query("SELECT cost_usd FROM jev_requests WHERE id = 'billed-1'")
            .fetch_one(&pool)
            .await
            .expect("written")
            .try_get("cost_usd")
            .expect("a cost");
        assert!((cost - 0.0000176).abs() < 1e-12);

        let mut full = Unrecorded::default();
        for n in 0..=UNRECORDED_CAP {
            let id = format!("row-{n}");
            full.hold(completed_row(
                RecordedRequest {
                    id: &id,
                    ..meta("unused")
                },
                &response(),
                None,
            ));
        }
        assert_eq!(full.pending(), UNRECORDED_CAP);
        assert_eq!(full.dropped(), 1);
    }

    /// The panel reports the gate and the held rows the latest cycle saw.
    #[tokio::test]
    async fn the_panel_shows_the_gate_and_the_rows_still_held() {
        let pool = pool().await;
        let now = at("2026-10-16T08:00:00Z");
        note_provider_failure(&pool, now, "http_429", Some(3_600))
            .await
            .expect("the gate shuts");
        let mut held = Unrecorded::default();
        held.hold(completed_row(
            RecordedRequest {
                id: "held",
                created_at: "2026-10-16T08:00:00Z",
                purpose: PURPOSE_REPORT_GRADING,
                subject: Some("401"),
                model_requested: "~typesafe/jev-latest",
                question_count: 3,
            },
            &response(),
            None,
        ));
        let deferred = serde_json::json!({"status": "deferred", "failed": 0});
        record_worker_cycle(
            &pool,
            "2026-10-16T08:00:00Z",
            "2026-10-16T08:01:00Z",
            &[(PURPOSE_REPORT_GRADING, &deferred)],
            &held,
        )
        .await
        .expect("cycle records");

        let panel = operator_payload(
            "ok",
            at("2026-10-16T08:05:00Z"),
            600,
            Default::default(),
            OperatorReads {
                worker: worker_status_rows(&pool).await,
                provider: provider_state_row(&pool).await,
                calls: Ok(Default::default()),
                agreement: Ok(serde_json::json!({})),
                disagreements: Ok(Vec::new()),
            },
        );
        assert!(panel.provider.deferred);
        assert_eq!(
            panel.provider.not_before.as_deref(),
            Some("2026-10-16T09:00:00Z")
        );
        assert_eq!(
            panel.provider.last_failure_kind.as_deref(),
            Some("http_429")
        );
        assert_eq!(panel.unrecorded_pending, 1);
        assert_eq!(panel.unrecorded_pending_cost_usd, Some(0.0000176));
        assert_eq!(
            (
                panel.steps[0].last_clean_at.as_deref(),
                panel.steps[0].last_failed_at.as_deref()
            ),
            (None, None),
            "a deferred step neither ran clean nor failed"
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

        // A partial grade is selected too, and recomputing it must not make it
        // look complete.
        sqlx::query(
            "INSERT INTO jev_requests
             (id, created_at, purpose, subject, model_requested, question_count, answer_count,
              issue_count, input_tokens, output_tokens, cost_source, latency_ms, attempts,
              status, result_json)
             VALUES ('partial-1', '2026-09-20T12:00:00Z', $1, '8', 'alias', 4, 2, 2, 10, 0,
                     'not_reported', 1, 1, 'partial', $2)",
        )
        .bind(PURPOSE_REPORT_GRADING)
        .bind(serde_json::json!({"numeric_checks": {"method_version": "n1-old"}}).to_string())
        .execute(&pool)
        .await
        .expect("insert partial");

        let with_partial = grades_with_stale_numeric_method(&pool, "n2-new", 10)
            .await
            .expect("select");
        assert_eq!(
            with_partial.len(),
            2,
            "a partial grade's numeric half is arithmetic and owes nothing to the missing \
             answers: {with_partial:?}"
        );

        replace_numeric_measurement(
            &pool,
            "partial-1",
            &serde_json::json!({"method_version": "n2-new"}),
            "recomputed",
        )
        .await
        .expect("replace");
        let partial = sqlx::query(
            "SELECT status, answer_count, result_json FROM jev_requests WHERE id = 'partial-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(
            partial.try_get::<String, _>("status").expect("status"),
            "partial",
            "recomputing arithmetic must not turn an incomplete model response into a complete one"
        );
        assert_eq!(
            partial.try_get::<i64, _>("answer_count").expect("answers"),
            2
        );

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
