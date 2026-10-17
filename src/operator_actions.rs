//! An audit trail of operator actions: every request that changes state.
//!
//! One middleware on the router records each POST, PUT, PATCH or DELETE --
//! settings changes, guardrail and quarantine overrides, experiment
//! transitions, manual report triggers, manual queue processing, protective-
//! stop placements, the portfolio reset, Saxo session actions -- with who made
//! it, which route, and how it ended. Recording at the router rather than in
//! each handler means a write endpoint added later is covered without anyone
//! remembering to add it.
//!
//! What is recorded is deliberately narrow. The path, never the query string
//! or the body: a form can carry an API key, and an OAuth callback carries a
//! code. The caller is the SSO email the ngrok gateway asserts; a Hermes
//! request is recorded as `hermes` from the presence of its key header, and
//! the key itself is never read here. Nothing in this module can change what
//! a request does: the record is written after the handler has answered.

use anyhow::{Context, Result};
use axum::http::{HeaderMap, Method};
use serde_json::Value as JsonValue;
use sqlx::AnyPool;

use crate::db::row_to_json;

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS operator_actions (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            actor TEXT NOT NULL,
            actor_kind TEXT NOT NULL,
            method TEXT NOT NULL,
            route TEXT NOT NULL,
            path TEXT NOT NULL,
            status_code INTEGER NOT NULL,
            outcome TEXT NOT NULL,
            duration_ms INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE INDEX IF NOT EXISTS idx_operator_actions_created
         ON operator_actions(created_at DESC)",
    ]
}

/// Whether a request is one this trail records.
pub(crate) fn is_recorded(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Who made a request, as `(kind, actor)`.
pub(crate) fn actor_from_headers(headers: &HeaderMap) -> (&'static str, String) {
    if let Some(user) = crate::auth::SsoSession::from_headers(headers).user {
        return ("sso", user.email);
    }
    // Presence only. Whether the key was right is the handler's decision,
    // and its answer is in the recorded status.
    let presents_key = headers.contains_key("x-hermes-api-key")
        || headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Bearer "));
    if presents_key {
        return ("api_key", "hermes".to_string());
    }
    ("unauthenticated", String::new())
}

/// How a request ended: 2xx and 3xx (a form post redirects back) did what was
/// asked, 4xx was refused, 5xx failed.
pub(crate) fn outcome_for(status: u16) -> &'static str {
    match status {
        400..=499 => "refused",
        500..=599 => "error",
        _ => "ok",
    }
}

pub(crate) struct OperatorAction<'a> {
    pub created_at: &'a str,
    pub actor_kind: &'a str,
    pub actor: &'a str,
    pub method: &'a str,
    pub route: &'a str,
    pub path: &'a str,
    pub status_code: u16,
    pub duration_ms: i64,
}

pub(crate) async fn record(pool: &AnyPool, action: &OperatorAction<'_>) -> Result<()> {
    let id = format!("op-{}-{:016x}", action.created_at, rand::random::<u64>());
    sqlx::query(
        "INSERT INTO operator_actions
            (id, created_at, actor, actor_kind, method, route, path, status_code, outcome,
             duration_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(action.created_at)
    .bind(action.actor)
    .bind(action.actor_kind)
    .bind(action.method)
    .bind(action.route)
    .bind(action.path)
    .bind(i64::from(action.status_code))
    .bind(outcome_for(action.status_code))
    .bind(action.duration_ms)
    .execute(pool)
    .await
    .context("recording an operator action")?;
    Ok(())
}

pub(crate) async fn recent_rows(pool: &AnyPool, limit: i64) -> Result<Vec<JsonValue>> {
    Ok(sqlx::query(&format!(
        "SELECT created_at, actor, actor_kind, method, route, path, status_code, outcome,
                duration_ms
         FROM operator_actions
         ORDER BY created_at DESC
         LIMIT {}",
        limit.clamp(1, 500)
    ))
    .fetch_all(pool)
    .await
    .context("reading operator actions")?
    .iter()
    .map(row_to_json)
    .collect())
}

/// The recent trail, never failing: an unreadable table is reported as
/// unavailable rather than as nobody having done anything.
pub(crate) async fn recent(pool: &AnyPool, limit: i64) -> crate::models::OperatorActionsPayload {
    match recent_rows(pool, limit).await.and_then(|rows| {
        crate::read_model::decode_each::<crate::models::OperatorActionPayload>(
            "operator actions",
            rows,
        )
        .map_err(Into::into)
    }) {
        Ok(items) => crate::models::OperatorActionsPayload {
            items,
            unavailable: false,
        },
        Err(err) => {
            tracing::warn!("operator actions unavailable: {err:#}");
            crate::models::OperatorActionsPayload {
                items: Vec::new(),
                unavailable: true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    /// The SSO email wins; a presented Hermes key, in either form, is
    /// recorded as `hermes` without its value; anything else is anonymous.
    #[test]
    fn the_actor_is_the_sso_email_then_a_presented_key_then_nobody() {
        assert_eq!(
            actor_from_headers(&headers(&[
                ("x-daytrader-user-email", "simon@example.test"),
                ("x-hermes-api-key", "k"),
            ])),
            ("sso", "simon@example.test".to_string())
        );
        assert_eq!(
            actor_from_headers(&headers(&[("x-hermes-api-key", "k")])),
            ("api_key", "hermes".to_string())
        );
        assert_eq!(
            actor_from_headers(&headers(&[("authorization", "Bearer k")])),
            ("api_key", "hermes".to_string())
        );
        assert_eq!(
            actor_from_headers(&headers(&[("authorization", "Basic k")])),
            ("unauthenticated", String::new())
        );
        assert_eq!(actor_from_headers(&HeaderMap::new()).0, "unauthenticated");
    }

    #[test]
    fn only_writes_are_recorded_and_their_outcome_follows_the_status() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(is_recorded(&method), "{method}");
        }
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(!is_recorded(&method), "{method}");
        }
        assert_eq!(outcome_for(200), "ok");
        assert_eq!(
            outcome_for(303),
            "ok",
            "a form post redirects back on success"
        );
        assert_eq!(outcome_for(422), "refused");
        assert_eq!(outcome_for(503), "error");
    }

    #[tokio::test]
    async fn an_unreadable_trail_is_unavailable_not_empty() {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let trail = recent(&pool, 10).await;
        assert!(trail.unavailable, "no table, so unknown rather than none");
        assert!(trail.items.is_empty());
    }
}
