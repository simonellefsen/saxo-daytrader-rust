use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use quick_xml::de::from_str;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tracing::{info, warn};

use crate::{
    config::{yaml_at, yaml_bool, yaml_i64},
    db::{clamp_limit, row_to_json},
    state::AppState,
};

const DEFAULT_REFRESH_INTERVAL_MINUTES: i64 = 240;
const DEFAULT_REQUEST_TIMEOUT_SECONDS: i64 = 15;
const DEFAULT_MAX_ITEMS_PER_SOURCE: usize = 12;
const DEFAULT_MAX_SUMMARY_CHARS: usize = 800;
const DEFAULT_RETENTION_DAYS: i64 = 90;
const HERMES_RESEARCH_ITEM_LIMIT: usize = 20;

#[derive(Debug, Clone)]
struct EditorialResearchConfig {
    enabled: bool,
    refresh_interval: Duration,
    request_timeout: StdDuration,
    max_items_per_source: usize,
    max_summary_chars: usize,
    retention: Duration,
    sources: Vec<EditorialResearchSource>,
}

#[derive(Debug, Clone)]
struct EditorialResearchSource {
    name: String,
    url: String,
    access_level: String,
    symbol_aliases: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Deserialize)]
struct RssFeed {
    channel: RssChannel,
}

#[derive(Debug, Deserialize)]
struct RssChannel {
    #[serde(default)]
    item: Vec<RssItem>,
}

#[derive(Debug, Deserialize)]
struct RssItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    guid: String,
    #[serde(rename = "pubDate", default)]
    published_at: String,
    #[serde(default)]
    description: String,
}

pub fn create_schema_sql() -> &'static [&'static str] {
    &[
        "CREATE TABLE IF NOT EXISTS editorial_research_runs (
            id TEXT PRIMARY KEY,
            source_name TEXT NOT NULL,
            started_at TEXT NOT NULL,
            completed_at TEXT NOT NULL,
            status TEXT NOT NULL,
            fetched_count INTEGER NOT NULL DEFAULT 0,
            stored_count INTEGER NOT NULL DEFAULT 0,
            error_summary TEXT
        )",
        "CREATE TABLE IF NOT EXISTS editorial_research_items (
            id TEXT PRIMARY KEY,
            source_name TEXT NOT NULL,
            source_url TEXT NOT NULL,
            canonical_url TEXT NOT NULL,
            title TEXT NOT NULL,
            published_at TEXT,
            access_level TEXT NOT NULL,
            summary TEXT NOT NULL,
            matched_symbols_json TEXT NOT NULL,
            first_seen_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_editorial_research_runs_source_completed
         ON editorial_research_runs(source_name, completed_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_editorial_research_items_published
         ON editorial_research_items(published_at DESC, last_seen_at DESC)",
    ]
}

pub async fn run_editorial_research_cycle(state: &AppState) -> Result<JsonValue> {
    let config = editorial_research_config(state);
    if !config.enabled {
        return Ok(json!({"status": "disabled"}));
    }
    if config.sources.is_empty() {
        return Ok(json!({"status": "disabled", "reason": "no_sources_configured"}));
    }

    let client = reqwest::Client::builder()
        .timeout(config.request_timeout)
        .user_agent("saxo-daytrader-rust/0.1 public-editorial-research")
        .build()
        .context("building editorial research HTTP client")?;
    let mut source_results = Vec::new();
    let mut fetched_count = 0usize;
    let mut stored_count = 0usize;
    let mut attempted_sources = 0usize;

    for source in &config.sources {
        if !source_due(state, source, config.refresh_interval).await? {
            source_results.push(json!({
                "source": source.name,
                "status": "skipped",
                "reason": "refresh_interval",
            }));
            continue;
        }
        attempted_sources += 1;
        let started_at = Utc::now();
        match fetch_source_items(
            &client,
            source,
            config.max_items_per_source,
            config.max_summary_chars,
        )
        .await
        {
            Ok(items) => {
                let item_count = items.len();
                let mut source_stored_count = 0usize;
                for item in items {
                    if store_item(state, &item).await? {
                        source_stored_count += 1;
                    }
                }
                record_run(
                    state,
                    source,
                    started_at,
                    "ok",
                    item_count,
                    source_stored_count,
                    None,
                )
                .await?;
                fetched_count += item_count;
                stored_count += source_stored_count;
                source_results.push(json!({
                    "source": source.name,
                    "status": "ok",
                    "fetched_count": item_count,
                    "stored_count": source_stored_count,
                }));
            }
            Err(err) => {
                let error_summary = compact_error(&err.to_string());
                warn!(source = %source.name, "editorial research source fetch failed: {error_summary}");
                record_run(
                    state,
                    source,
                    started_at,
                    "error",
                    0,
                    0,
                    Some(&error_summary),
                )
                .await?;
                source_results.push(json!({
                    "source": source.name,
                    "status": "error",
                    "error": error_summary,
                }));
            }
        }
    }

    let status = if attempted_sources == 0 {
        "skipped"
    } else if source_results
        .iter()
        .all(|result| result.get("status").and_then(JsonValue::as_str) == Some("error"))
    {
        "error"
    } else {
        "ok"
    };
    let pruned = prune_old_records(state, config.retention).await?;
    info!(
        attempted_sources,
        fetched_count, stored_count, pruned, status, "editorial research cycle completed"
    );
    Ok(json!({
        "status": status,
        "attempted_sources": attempted_sources,
        "fetched_count": fetched_count,
        "stored_count": stored_count,
        "pruned": pruned,
        "sources": source_results,
        "safety": "public_rss_only_sanitized_editorial_context_no_broker_or_manager_mutation",
    }))
}

pub async fn compact_editorial_research_context(state: &AppState, limit: i64) -> Result<JsonValue> {
    let item_limit = clamp_limit(limit, 1, HERMES_RESEARCH_ITEM_LIMIT as i64);
    let rows = sqlx::query(&format!(
        "SELECT source_name, canonical_url, title, published_at, access_level, summary, matched_symbols_json, last_seen_at
         FROM editorial_research_items
         ORDER BY COALESCE(published_at, last_seen_at) DESC, last_seen_at DESC
         LIMIT {item_limit}"
    ))
    .fetch_all(&state.pool)
    .await
    .context("reading compact editorial research context")?
    .iter()
    .map(row_to_json)
    .collect::<Vec<_>>();
    let items = rows
        .into_iter()
        .map(|row| {
            let matched_symbols = row
                .get("matched_symbols_json")
                .and_then(|value| {
                    value.as_array().cloned().or_else(|| {
                        value
                            .as_str()
                            .and_then(|value| serde_json::from_str::<JsonValue>(value).ok())
                            .and_then(|value| value.as_array().cloned())
                    })
                })
                .unwrap_or_default();
            json!({
                "source": value_text(&row, "source_name"),
                "url": value_text(&row, "canonical_url"),
                "title": value_text(&row, "title"),
                "published_at": value_text(&row, "published_at"),
                "access_level": value_text(&row, "access_level"),
                "summary": value_text(&row, "summary"),
                "matched_symbols": matched_symbols,
                "last_seen_at": value_text(&row, "last_seen_at"),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "status": if items.is_empty() { "no_public_research_recorded" } else { "ok" },
        "items": items,
        "safety": "public_feed_metadata_and_bounded_summary_only_editorial_secondary_context_not_a_trading_signal",
        "interpretation": "Items are attributable editorial research. They do not verify a claim, create a manager gate, or authorize, size, block, place, amend, or cancel a Saxo order.",
    }))
}

#[derive(Debug)]
struct EditorialResearchItem {
    id: String,
    source_name: String,
    source_url: String,
    canonical_url: String,
    title: String,
    published_at: Option<String>,
    access_level: String,
    summary: String,
    matched_symbols: Vec<String>,
}

async fn fetch_source_items(
    client: &reqwest::Client,
    source: &EditorialResearchSource,
    max_items: usize,
    max_summary_chars: usize,
) -> Result<Vec<EditorialResearchItem>> {
    let response = client
        .get(&source.url)
        .send()
        .await
        .with_context(|| format!("fetching public feed for {}", source.name))?
        .error_for_status()
        .with_context(|| format!("public feed returned an error for {}", source.name))?;
    let body = response
        .text()
        .await
        .with_context(|| format!("reading public feed for {}", source.name))?;
    let feed: RssFeed =
        from_str(&body).with_context(|| format!("parsing RSS feed for {}", source.name))?;
    Ok(feed
        .channel
        .item
        .into_iter()
        .filter_map(|item| sanitized_item(source, item, max_summary_chars))
        .take(max_items)
        .collect())
}

fn sanitized_item(
    source: &EditorialResearchSource,
    item: RssItem,
    max_summary_chars: usize,
) -> Option<EditorialResearchItem> {
    let title = normalize_text(&item.title, 240);
    let canonical_url = item.link.trim().to_string();
    if title.is_empty() || canonical_url.is_empty() {
        return None;
    }
    let summary = normalize_text(&item.description, max_summary_chars);
    let matched_symbols = match_symbols(&source.symbol_aliases, &title, &summary);
    let published_at = parse_published_at(&item.published_at);
    let identity = if item.guid.trim().is_empty() {
        format!("{}\n{}\n{}", source.name, canonical_url, title)
    } else {
        format!("{}\n{}", source.name, item.guid.trim())
    };
    Some(EditorialResearchItem {
        id: stable_id("editorial", &identity),
        source_name: source.name.clone(),
        source_url: source.url.clone(),
        canonical_url,
        title,
        published_at,
        access_level: source.access_level.clone(),
        summary,
        matched_symbols,
    })
}

async fn store_item(state: &AppState, item: &EditorialResearchItem) -> Result<bool> {
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let updated = sqlx::query(
        "UPDATE editorial_research_items
         SET source_url = ?, canonical_url = ?, title = ?, published_at = ?, access_level = ?,
             summary = ?, matched_symbols_json = ?, last_seen_at = ?
         WHERE id = ?",
    )
    .bind(&item.source_url)
    .bind(&item.canonical_url)
    .bind(&item.title)
    .bind(&item.published_at)
    .bind(&item.access_level)
    .bind(&item.summary)
    .bind(serde_json::to_string(&item.matched_symbols)?)
    .bind(&now)
    .bind(&item.id)
    .execute(&state.pool)
    .await
    .context("updating sanitized editorial research item")?;
    if updated.rows_affected() > 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO editorial_research_items (
            id, source_name, source_url, canonical_url, title, published_at, access_level,
            summary, matched_symbols_json, first_seen_at, last_seen_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&item.id)
    .bind(&item.source_name)
    .bind(&item.source_url)
    .bind(&item.canonical_url)
    .bind(&item.title)
    .bind(&item.published_at)
    .bind(&item.access_level)
    .bind(&item.summary)
    .bind(serde_json::to_string(&item.matched_symbols)?)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .context("inserting sanitized editorial research item")?;
    Ok(true)
}

async fn source_due(
    state: &AppState,
    source: &EditorialResearchSource,
    interval: Duration,
) -> Result<bool> {
    let row = sqlx::query(
        "SELECT completed_at
         FROM editorial_research_runs
         WHERE source_name = ? AND status = 'ok'
         ORDER BY completed_at DESC
         LIMIT 1",
    )
    .bind(&source.name)
    .fetch_optional(&state.pool)
    .await
    .context("reading editorial research source freshness")?;
    let Some(row) = row else {
        return Ok(true);
    };
    let completed_at = row.try_get::<String, _>("completed_at").unwrap_or_default();
    let Ok(completed_at) = DateTime::parse_from_rfc3339(&completed_at) else {
        return Ok(true);
    };
    Ok(Utc::now() >= completed_at.with_timezone(&Utc) + interval)
}

async fn record_run(
    state: &AppState,
    source: &EditorialResearchSource,
    started_at: DateTime<Utc>,
    status: &str,
    fetched_count: usize,
    stored_count: usize,
    error_summary: Option<&str>,
) -> Result<()> {
    let completed_at = Utc::now();
    let id = stable_id(
        "editorial-run",
        &format!(
            "{}:{}:{}",
            source.name,
            started_at.timestamp_millis(),
            status
        ),
    );
    sqlx::query(
        "INSERT INTO editorial_research_runs (
            id, source_name, started_at, completed_at, status, fetched_count, stored_count, error_summary
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&source.name)
    .bind(started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .bind(completed_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .bind(status)
    .bind(fetched_count as i64)
    .bind(stored_count as i64)
    .bind(error_summary)
    .execute(&state.pool)
    .await
    .context("recording editorial research source run")?;
    Ok(())
}

async fn prune_old_records(state: &AppState, retention: Duration) -> Result<usize> {
    let cutoff = (Utc::now() - retention).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let item_result = sqlx::query("DELETE FROM editorial_research_items WHERE last_seen_at < ?")
        .bind(&cutoff)
        .execute(&state.pool)
        .await
        .context("pruning expired editorial research items")?;
    let run_result = sqlx::query("DELETE FROM editorial_research_runs WHERE completed_at < ?")
        .bind(&cutoff)
        .execute(&state.pool)
        .await
        .context("pruning expired editorial research runs")?;
    Ok((item_result.rows_affected() + run_result.rows_affected()) as usize)
}

fn editorial_research_config(state: &AppState) -> EditorialResearchConfig {
    let base = &["market_data", "editorial_research"];
    let refresh_minutes = yaml_i64(
        &state.config,
        &[base[0], base[1], "refresh_interval_minutes"],
    )
    .unwrap_or(DEFAULT_REFRESH_INTERVAL_MINUTES)
    .max(1);
    let timeout_seconds = yaml_i64(
        &state.config,
        &[base[0], base[1], "request_timeout_seconds"],
    )
    .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECONDS)
    .clamp(1, 60) as u64;
    let max_items_per_source = yaml_i64(&state.config, &[base[0], base[1], "max_items_per_source"])
        .unwrap_or(DEFAULT_MAX_ITEMS_PER_SOURCE as i64)
        .clamp(1, 30) as usize;
    let max_summary_chars = yaml_i64(&state.config, &[base[0], base[1], "max_summary_chars"])
        .unwrap_or(DEFAULT_MAX_SUMMARY_CHARS as i64)
        .clamp(120, 1_500) as usize;
    let retention_days = yaml_i64(&state.config, &[base[0], base[1], "retention_days"])
        .unwrap_or(DEFAULT_RETENTION_DAYS)
        .clamp(1, 365);
    let sources = yaml_at(&state.config, &[base[0], base[1], "sources"])
        .and_then(|value| value.as_sequence())
        .map(|sources| sources.iter().filter_map(parse_source).collect::<Vec<_>>())
        .unwrap_or_default();
    EditorialResearchConfig {
        enabled: yaml_bool(&state.config, &[base[0], base[1], "enabled"]).unwrap_or(false),
        refresh_interval: Duration::minutes(refresh_minutes),
        request_timeout: StdDuration::from_secs(timeout_seconds),
        max_items_per_source,
        max_summary_chars,
        retention: Duration::days(retention_days),
        sources,
    }
}

fn parse_source(value: &serde_yaml::Value) -> Option<EditorialResearchSource> {
    let name = value.get("name")?.as_str()?.trim().to_string();
    let url = value.get("url")?.as_str()?.trim().to_string();
    if name.is_empty() || url.is_empty() || !url.starts_with("https://") {
        return None;
    }
    let mut symbol_aliases = value
        .get("symbol_aliases")
        .and_then(|value| value.as_mapping())
        .map(|aliases| {
            aliases
                .iter()
                .filter_map(|(symbol, aliases)| {
                    let symbol = symbol.as_str()?.trim().to_string();
                    let aliases = aliases
                        .as_sequence()?
                        .iter()
                        .filter_map(|alias| alias.as_str())
                        .map(|alias| alias.trim().to_string())
                        .filter(|alias| !alias.is_empty())
                        .collect::<Vec<_>>();
                    (!symbol.is_empty() && !aliases.is_empty()).then_some((symbol, aliases))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    symbol_aliases.sort_by(|left, right| left.0.cmp(&right.0));
    Some(EditorialResearchSource {
        name,
        url,
        access_level: value
            .get("access_level")
            .and_then(|value| value.as_str())
            .unwrap_or("public_feed")
            .trim()
            .to_ascii_lowercase(),
        symbol_aliases,
    })
}

fn match_symbols(
    symbol_aliases: &[(String, Vec<String>)],
    title: &str,
    summary: &str,
) -> Vec<String> {
    let haystack = format!(
        "{} {}",
        title.to_ascii_lowercase(),
        summary.to_ascii_lowercase()
    );
    symbol_aliases
        .iter()
        .filter_map(|(symbol, aliases)| {
            aliases
                .iter()
                .any(|alias| contains_term(&haystack, &alias.to_ascii_lowercase()))
                .then(|| symbol.clone())
        })
        .collect()
}

fn contains_term(haystack: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    let mut start = 0usize;
    while let Some(relative) = haystack[start..].find(term) {
        let index = start + relative;
        let end = index + term.len();
        let before = haystack[..index].chars().next_back();
        let after = haystack[end..].chars().next();
        if !before.is_some_and(|ch| ch.is_ascii_alphanumeric())
            && !after.is_some_and(|ch| ch.is_ascii_alphanumeric())
        {
            return true;
        }
        start = end;
    }
    false
}

fn normalize_text(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    let mut previous_was_space = true;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            _ if ch.is_whitespace() => {
                if !previous_was_space {
                    output.push(' ');
                    previous_was_space = true;
                }
            }
            _ => {
                output.push(ch);
                previous_was_space = false;
            }
        }
        if output.chars().count() >= max_chars {
            break;
        }
    }
    output.trim().chars().take(max_chars).collect()
}

fn parse_published_at(value: &str) -> Option<String> {
    DateTime::parse_from_rfc2822(value)
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|value| {
            value
                .with_timezone(&Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
}

fn stable_id(prefix: &str, value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{prefix}-{:x}", digest)
}

fn compact_error(value: &str) -> String {
    normalize_text(value, 240)
}

fn value_text(value: &JsonValue, key: &str) -> String {
    match value.get(key) {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Number(number)) => number.to_string(),
        Some(JsonValue::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sanitizes_public_rss_item() {
        let source = EditorialResearchSource {
            name: "Example".to_string(),
            url: "https://example.com/feed".to_string(),
            access_level: "public_feed".to_string(),
            symbol_aliases: vec![("TSLA:xnas".to_string(), vec!["Tesla".to_string()])],
        };
        let item = RssItem {
            title: "Tesla: <b>Cash flow</b>".to_string(),
            link: "https://example.com/tesla".to_string(),
            guid: "guid-1".to_string(),
            published_at: "Fri, 24 Jul 2026 12:00:00 +0000".to_string(),
            description: "<p>Free cash flow weakened.</p>".to_string(),
        };
        let item =
            sanitized_item(&source, item, DEFAULT_MAX_SUMMARY_CHARS).expect("sanitized item");
        assert_eq!(item.title, "Tesla: Cash flow");
        assert_eq!(item.summary, "Free cash flow weakened.");
        assert_eq!(item.matched_symbols, vec!["TSLA:xnas"]);
        assert_eq!(item.published_at.as_deref(), Some("2026-07-24T12:00:00Z"));
    }

    #[test]
    fn matcher_requires_term_boundaries() {
        let aliases = vec![("TSLA:xnas".to_string(), vec!["Tesla".to_string()])];
        assert_eq!(
            match_symbols(&aliases, "Tesla results", ""),
            vec!["TSLA:xnas"]
        );
        assert!(match_symbols(&aliases, "Teslastic", "").is_empty());
    }

    #[test]
    fn rss_parser_reads_public_item_list() {
        let feed: RssFeed = from_str(
            "<rss><channel><item><title>Test</title><link>https://example.com/a</link><description>Summary</description></item></channel></rss>",
        )
        .expect("valid RSS");
        assert_eq!(feed.channel.item.len(), 1);
        assert_eq!(feed.channel.item[0].title, "Test");
    }

    #[tokio::test]
    async fn persists_sanitized_items_as_bounded_context() {
        static INSTALL_DRIVERS: std::sync::Once = std::sync::Once::new();
        INSTALL_DRIVERS.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory editorial research database");
        for sql in create_schema_sql() {
            sqlx::query(sql)
                .execute(&pool)
                .await
                .expect("create editorial research table");
        }
        let state = AppState {
            config_path: std::path::PathBuf::from("editorial-research-test.yaml"),
            config: serde_yaml::from_str("{}").expect("parse test config"),
            db_url: "sqlite::memory:".to_string(),
            pool,
        };
        let source = EditorialResearchSource {
            name: "Example".to_string(),
            url: "https://example.com/feed".to_string(),
            access_level: "public_feed_metadata".to_string(),
            symbol_aliases: vec![("TSLA:xnas".to_string(), vec!["Tesla".to_string()])],
        };
        let item = sanitized_item(
            &source,
            RssItem {
                title: "Tesla update".to_string(),
                link: "https://example.com/tesla".to_string(),
                guid: "test-guid".to_string(),
                published_at: "Fri, 24 Jul 2026 12:00:00 +0000".to_string(),
                description: "A short public summary.".to_string(),
            },
            DEFAULT_MAX_SUMMARY_CHARS,
        )
        .expect("sanitized item");
        assert!(store_item(&state, &item).await.expect("store item"));
        assert!(!store_item(&state, &item).await.expect("deduplicate item"));

        let context = compact_editorial_research_context(&state, 50)
            .await
            .expect("compact context");
        assert_eq!(context["status"], "ok");
        assert_eq!(context["items"][0]["source"], "Example");
        assert_eq!(context["items"][0]["matched_symbols"], json!(["TSLA:xnas"]));
    }
}
