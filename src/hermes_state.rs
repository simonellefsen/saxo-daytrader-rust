//! Read-only Hermes dashboard projections.
//!
//! This module intentionally contains only deterministic transformations of
//! persisted reflection records. It cannot create experiments, alter runtime
//! configuration, invoke a provider, or reach Saxo.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value as JsonValue, json};

use crate::state::json_text;

pub(crate) const LESSONS_PENDING_REVIEW_REFLECTION_LIMIT: i64 = 50;
pub(crate) const LESSONS_PENDING_REVIEW_LIMIT: usize = 30;
const LESSON_TEXT_MAX_CHARS: usize = 500;
pub(crate) const LEARNING_MEMORY_REFLECTION_LIMIT: i64 = 80;
pub(crate) const LEARNING_MEMORY_LIMIT: usize = 30;
const LEARNING_MEMORY_EMERGING_TTL_DAYS: i64 = 7;
const LEARNING_MEMORY_STABLE_TTL_DAYS: i64 = 21;
const LEARNING_MEMORY_STABLE_MIN_REFLECTIONS: usize = 2;

#[derive(Clone, Debug)]
struct LearningMemoryEntry {
    lesson: String,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    reflection_ids: HashSet<String>,
    cadences: HashSet<String>,
}

/// Convert reflection `proposed_actions` into a bounded, display-safe operator
/// queue. The rows are deliberately derived rather than persisted as a second
/// workflow: an item is advisory context, not an approved experiment or task.
pub(crate) fn lessons_pending_review_from_reflections(
    reflections: &[JsonValue],
    limit: usize,
) -> Vec<JsonValue> {
    let mut lessons = Vec::new();
    let mut seen = HashSet::new();

    for reflection in reflections {
        let reflection_id = reflection
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .trim();
        if reflection_id.is_empty() {
            continue;
        }
        let Some(actions) = reflection.get("proposed_actions_json") else {
            continue;
        };
        for (action_index, action) in proposed_action_entries(actions).into_iter().enumerate() {
            let Some(lesson) = proposed_action_text(action) else {
                continue;
            };
            let normalized = lesson.to_lowercase();
            if !seen.insert(normalized) {
                continue;
            }
            lessons.push(json!({
                "id": format!("{reflection_id}:{action_index}"),
                "reflection_id": reflection_id,
                "created_at": reflection.get("created_at").cloned().unwrap_or(JsonValue::Null),
                "period_start": reflection.get("period_start").cloned().unwrap_or(JsonValue::Null),
                "period_end": reflection.get("period_end").cloned().unwrap_or(JsonValue::Null),
                "goal_version": reflection.get("goal_version").cloned().unwrap_or(JsonValue::Null),
                "lesson": lesson,
                "reflection_summary": reflection.get("summary").cloned().unwrap_or(JsonValue::Null),
                "source_session_id": reflection.get("source_session_id").cloned().unwrap_or(JsonValue::Null),
            }));
            if lessons.len() >= limit.max(1) {
                return lessons;
            }
        }
    }
    lessons
}

/// Compress repeated safe reflection actions into an expiring, read-only
/// learning-memory view. Repetition across separate reflections makes a
/// lesson stable; one-off actions remain emerging and all observations expire
/// deterministically. This is advisory context only, not an experiment or
/// strategy/configuration mutation path.
pub(crate) fn learning_memory_from_reflections(
    reflections: &[JsonValue],
    now: DateTime<Utc>,
    limit: usize,
) -> Vec<JsonValue> {
    let mut entries = HashMap::<String, LearningMemoryEntry>::new();
    for reflection in reflections {
        let reflection_id = json_text(reflection, "id");
        if reflection_id.is_empty() {
            continue;
        }
        let created_at = json_text(reflection, "created_at");
        let Some(created_at) = DateTime::parse_from_rfc3339(&created_at)
            .ok()
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };
        let cadence = reflection_cadence(reflection);
        let Some(actions) = reflection.get("proposed_actions_json") else {
            continue;
        };
        let mut actions_seen_in_reflection = HashSet::new();
        for action in proposed_action_entries(actions) {
            let Some(lesson) = proposed_action_text(action) else {
                continue;
            };
            let normalized = lesson.to_ascii_lowercase();
            if !actions_seen_in_reflection.insert(normalized.clone()) {
                continue;
            }
            let entry = entries
                .entry(normalized)
                .or_insert_with(|| LearningMemoryEntry {
                    lesson: lesson.clone(),
                    first_seen: created_at,
                    last_seen: created_at,
                    reflection_ids: HashSet::new(),
                    cadences: HashSet::new(),
                });
            entry.first_seen = entry.first_seen.min(created_at);
            entry.last_seen = entry.last_seen.max(created_at);
            entry.reflection_ids.insert(reflection_id.clone());
            entry.cadences.insert(cadence.clone());
        }
    }

    let mut memory = entries
        .into_iter()
        .map(|(normalized_lesson, entry)| {
            let observation_count = entry.reflection_ids.len();
            let stable = observation_count >= LEARNING_MEMORY_STABLE_MIN_REFLECTIONS;
            let ttl_days = if stable {
                LEARNING_MEMORY_STABLE_TTL_DAYS
            } else {
                LEARNING_MEMORY_EMERGING_TTL_DAYS
            };
            let expires_at = entry.last_seen + Duration::days(ttl_days);
            let status = if now >= expires_at {
                "stale"
            } else if stable {
                "stable"
            } else {
                "emerging"
            };
            let mut cadences = entry.cadences.into_iter().collect::<Vec<_>>();
            cadences.sort();
            json!({
                "id": format!("lesson-memory:{}:{normalized_lesson}", entry.first_seen.timestamp_micros()),
                "lesson": entry.lesson,
                "status": status,
                "observation_count": observation_count,
                "first_seen": entry.first_seen.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "last_seen": entry.last_seen.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "expires_at": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "cadences": cadences,
                "safety": "derived_reflection_context_not_a_trading_instruction",
            })
        })
        .collect::<Vec<_>>();
    memory.sort_by(|left, right| {
        let rank = |row: &JsonValue| match json_text(row, "status").as_str() {
            "stable" => 0,
            "emerging" => 1,
            _ => 2,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| json_text(right, "last_seen").cmp(&json_text(left, "last_seen")))
    });
    memory.truncate(limit.max(1));
    memory
}

pub(crate) fn safe_display_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }
    if lesson_text_looks_sensitive(&normalized) {
        return "[redacted potentially sensitive Hermes text]".to_string();
    }
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

fn reflection_cadence(reflection: &JsonValue) -> String {
    let session = json_text(reflection, "source_session_id").to_ascii_lowercase();
    if session.contains("weekly") {
        "weekly".to_string()
    } else if session.contains("daily") {
        "daily".to_string()
    } else {
        "other".to_string()
    }
}

fn proposed_action_entries(value: &JsonValue) -> Vec<&JsonValue> {
    match value {
        JsonValue::Array(entries) => entries.iter().collect(),
        JsonValue::Object(object) => object
            .get("actions")
            .and_then(JsonValue::as_array)
            .map(|entries| entries.iter().collect())
            .unwrap_or_else(|| vec![value]),
        JsonValue::String(_) => vec![value],
        _ => Vec::new(),
    }
}

fn proposed_action_text(value: &JsonValue) -> Option<String> {
    let text = match value {
        JsonValue::String(text) => Some(text.as_str()),
        JsonValue::Object(object) => [
            "action",
            "recommendation",
            "proposal",
            "summary",
            "title",
            "detail",
        ]
        .iter()
        .find_map(|field| object.get(*field).and_then(JsonValue::as_str)),
        _ => None,
    };
    let text = text?;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if lesson_text_looks_sensitive(&normalized) {
        return Some("[redacted potentially sensitive reflection action]".to_string());
    }
    Some(safe_display_text(&normalized, LESSON_TEXT_MAX_CHARS))
}

fn lesson_text_looks_sensitive(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "refresh_token",
        "access_token",
        "client_secret",
        "authorization:",
        "bearer ",
        "api_key=",
        "openrouter_api_key",
        "accountkey=",
        "clientkey=",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn learning_memory_makes_repeated_lessons_stable_and_redacts_sensitive_text() {
        let reflections = vec![
            json!({
                "id": "daily-1",
                "created_at": "2026-07-30T17:00:00Z",
                "source_session_id": "daily-reflection",
                "proposed_actions_json": {"actions": [{"action": "Review fills before changing risk."}]},
            }),
            json!({
                "id": "weekly-1",
                "created_at": "2026-07-31T17:00:00Z",
                "source_session_id": "weekly-reflection",
                "proposed_actions_json": {"actions": [
                    {"action": "Review fills before changing risk."},
                    {"action": "Bearer not-a-real-token"}
                ]},
            }),
        ];
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).single().unwrap();
        let memory = learning_memory_from_reflections(&reflections, now, 10);
        assert_eq!(memory[0]["status"], "stable");
        assert_eq!(memory[0]["observation_count"], 2);
        assert!(
            memory.iter().any(|row| {
                row["lesson"] == "[redacted potentially sensitive reflection action]"
            })
        );
    }
}
