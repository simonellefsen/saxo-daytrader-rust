//! Saxo request pacing (U6).
//!
//! Saxo enforces roughly 120 requests per minute per session per service group.
//! Until now the only defence was a fixed 500 ms sleep in the Markov chart
//! loop, chosen when `markov.max_symbols` was 20. Both that job and the daily
//! indicator job were raised to `0` (unlimited, ~199 symbols) on 2026-07-16 and
//! now run back to back at 23:30 and 23:45 against the same limit. A fixed
//! sleep cannot see the other job, so the pacing was a guess that stopped being
//! true the moment the caps were lifted.
//!
//! Two mechanisms, and the second is the one that matters:
//!
//! 1. A sliding-window bucket per service group, defaulting to 100/min -- under
//!    the documented ceiling, because the ceiling is where rejection starts,
//!    not where it is safe to sit.
//! 2. Adaptation from the `X-RateLimit-*` response headers. Saxo reports what
//!    is left and when it resets, so remaining quota divided by remaining time
//!    gives the pace the server is actually willing to accept. That is a
//!    measurement rather than an assumption, and it tightens automatically as
//!    quota depletes instead of waiting for the first 429.
//!
//! Both jobs already share `markov_method::saxo_get_json`, so pacing installed
//! there covers them with one bucket rather than two independent guesses.
//!
//! ## Scope limit
//!
//! State is per process. Saxo's limit is per *session*, and the API and
//! scheduler pods share one session, so they cannot see each other's usage.
//! Coordinating would need the limiter in the database, which is a much heavier
//! thing to put in front of every request. The bulk traffic -- both nightly
//! sweeps -- runs in the scheduler pod, and the API pod's calls are sporadic
//! and operator-driven, so process-local pacing plus header adaptation covers
//! the real exposure. If the API pod ever starts sweeping, this needs revisiting.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use reqwest::header::HeaderMap;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tracing::{debug, warn};

const WINDOW: Duration = Duration::from_secs(60);
const DEFAULT_REQUESTS_PER_MINUTE: usize = 100;

/// Never park a request for longer than this on one wait. A pathological
/// `Reset` value should slow the caller down, not strand a nightly job.
const MAX_SINGLE_WAIT: Duration = Duration::from_secs(30);

/// Saxo's service group is the first path segment: `/chart/v3/charts` is
/// `chart`, `/port/v1/orders` is `port`. Limits are counted per group, so the
/// buckets are keyed the same way.
pub(crate) fn service_group(path: &str) -> String {
    path.split('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("default")
        .to_ascii_lowercase()
}

#[derive(Debug, Default)]
struct GroupState {
    /// Earliest time the next request in this group may go out.
    next_allowed_at: Option<Instant>,
    /// Requests issued through the pacer for this group.
    request_count: u64,
    /// Requests that had to wait for their slot.
    waited_count: u64,
    /// Cumulative time spent waiting, in milliseconds.
    total_waited_ms: u64,
    /// Times a response's headers implied tighter pacing than the configured
    /// baseline, meaning the quota was depleting faster than the floor assumes.
    header_tightened_count: u64,
}

/// The floor spacing implied by a per-minute budget.
///
/// Spacing rather than a token bucket, deliberately. A bucket of 100 lets a
/// sweep fire a hundred requests back to back and then stall for a minute:
/// technically inside the window, but the burstiest possible way to spend the
/// quota, and worse behaviour than the fixed 500 ms sleep this replaces. Even
/// spacing can never burst, and at the default 100/min it works out to 600 ms
/// -- already more conservative than the sleep it supersedes.
fn baseline_spacing(requests_per_minute: usize) -> Duration {
    WINDOW / requests_per_minute.max(1) as u32
}

/// How long a caller must wait before issuing a request, reserving the slot
/// when the answer is "now".
///
/// Pure over `now` so the pacing can be tested without sleeping.
fn plan_delay(group: &mut GroupState, now: Instant, spacing: Duration) -> Option<Duration> {
    if let Some(next_allowed_at) = group.next_allowed_at
        && next_allowed_at > now
    {
        return Some(next_allowed_at.duration_since(now).min(MAX_SINGLE_WAIT));
    }
    // The slot is reserved here rather than by the caller so two tasks cannot
    // both be told "go" against the same opening.
    group.next_allowed_at = Some(now + spacing);
    None
}

/// Translate one dimension's remaining quota and reset time into the spacing
/// the server is telling us it will accept.
///
/// Exhausted quota means wait out the window. Otherwise the remaining seconds
/// spread across the remaining requests is the pace, which tightens on its own
/// as quota runs down -- no threshold to tune, and no waiting for a 429 to
/// learn something the headers already said.
fn spacing_from_quota(remaining: u64, reset_seconds: f64) -> Option<Duration> {
    if !reset_seconds.is_finite() || reset_seconds <= 0.0 {
        return None;
    }
    if remaining == 0 {
        return Some(Duration::from_secs_f64(reset_seconds));
    }
    Some(Duration::from_secs_f64(reset_seconds / remaining as f64))
}

/// The tightest spacing any `X-RateLimit-*` dimension in the response demands.
fn spacing_from_headers(headers: &HeaderMap) -> Option<(String, Duration)> {
    let numeric = |name: &str| -> Option<f64> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<f64>().ok())
    };
    let dimensions = headers
        .keys()
        .filter_map(|name| {
            let lower = name.as_str().to_ascii_lowercase();
            lower
                .strip_prefix("x-ratelimit-")
                .and_then(|rest| rest.strip_suffix("-remaining"))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    dimensions
        .into_iter()
        .filter_map(|dimension| {
            let remaining = numeric(&format!("x-ratelimit-{dimension}-remaining"))?;
            let reset = numeric(&format!("x-ratelimit-{dimension}-reset"))?;
            if remaining < 0.0 {
                return None;
            }
            spacing_from_quota(remaining as u64, reset).map(|spacing| (dimension, spacing))
        })
        .max_by_key(|(_, spacing)| *spacing)
}

/// Requests per minute per service group, from configuration.
///
/// Clamped to Saxo's documented ceiling: a config typo must not be able to
/// raise the pace above what the broker will accept.
pub(crate) fn configured_rate(config: &serde_yaml::Value) -> usize {
    crate::config::yaml_i64(config, &["saxo", "requests_per_minute"])
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_REQUESTS_PER_MINUTE as i64)
        .clamp(1, 120) as usize
}

fn buckets() -> &'static Mutex<HashMap<String, GroupState>> {
    static BUCKETS: OnceLock<Mutex<HashMap<String, GroupState>>> = OnceLock::new();
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Wait until a request against `path` may be issued.
///
/// The lock is released before sleeping so one paced caller never blocks
/// another group, and the plan is recomputed after each sleep because the
/// headers may have moved the target while we waited.
pub(crate) async fn acquire(path: &str, requests_per_minute: usize) {
    let group_key = service_group(path);
    let spacing = baseline_spacing(if requests_per_minute == 0 {
        DEFAULT_REQUESTS_PER_MINUTE
    } else {
        requests_per_minute
    });
    let mut waited = Duration::ZERO;
    loop {
        let delay = {
            let mut guard = match buckets().lock() {
                Ok(guard) => guard,
                // A poisoned lock means another task panicked mid-plan. Pacing
                // is not worth failing a request over; proceed unpaced and say
                // so rather than propagating the panic.
                Err(poisoned) => {
                    warn!(group = %group_key, "Saxo rate-limit state was poisoned; proceeding unpaced");
                    poisoned.into_inner()
                }
            };
            let group = guard.entry(group_key.clone()).or_default();
            plan_delay(group, Instant::now(), spacing)
        };
        match delay {
            Some(delay) => {
                waited += delay;
                sleep(delay).await;
            }
            None => break,
        }
    }
    if let Ok(mut guard) = buckets().lock() {
        let group = guard.entry(group_key.clone()).or_default();
        group.request_count += 1;
        if !waited.is_zero() {
            group.waited_count += 1;
            group.total_waited_ms += waited.as_millis() as u64;
        }
    }
    if waited > Duration::from_millis(250) {
        debug!(
            group = %group_key,
            waited_ms = waited.as_millis() as u64,
            "paced Saxo request against the service-group limit"
        );
    }
}

/// What the pacer did for one service group since the last reset.
///
/// A sweep's own timing cannot distinguish "slow because there are 200 assets"
/// from "slow because something else is spending the same quota". These
/// counters can: a market-hours sweep contending with the price monitor waits
/// more often, and for longer, than the same sweep run against an idle session.
pub(crate) fn snapshot(group_key: &str) -> JsonValue {
    let Ok(guard) = buckets().lock() else {
        return json!({"status": "unavailable"});
    };
    let Some(group) = guard.get(group_key) else {
        return json!({"status": "no_requests", "group": group_key});
    };
    json!({
        "status": "ok",
        "group": group_key,
        "request_count": group.request_count,
        "waited_count": group.waited_count,
        "total_waited_ms": group.total_waited_ms,
        "header_tightened_count": group.header_tightened_count,
    })
}

/// Zero one group's counters so the next sweep measures only itself.
pub(crate) fn reset(group_key: &str) {
    let Ok(mut guard) = buckets().lock() else {
        return;
    };
    if let Some(group) = guard.get_mut(group_key) {
        group.request_count = 0;
        group.waited_count = 0;
        group.total_waited_ms = 0;
        group.header_tightened_count = 0;
    }
}

/// Feed a response's rate-limit headers back into the pacer.
pub(crate) fn observe(path: &str, headers: &HeaderMap) {
    let Some((dimension, spacing)) = spacing_from_headers(headers) else {
        return;
    };
    let group_key = service_group(path);
    let Ok(mut guard) = buckets().lock() else {
        return;
    };
    let group = guard.entry(group_key.clone()).or_default();
    if spacing > baseline_spacing(DEFAULT_REQUESTS_PER_MINUTE) {
        group.header_tightened_count += 1;
    }
    let target = Instant::now() + spacing;
    if group.next_allowed_at.is_none_or(|current| target > current) {
        group.next_allowed_at = Some(target);
    }
    if spacing > Duration::from_secs(1) {
        warn!(
            group = %group_key,
            dimension = %dimension,
            spacing_ms = spacing.as_millis() as u64,
            "Saxo reports low remaining quota; slowing this service group"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                value.parse().expect("header value"),
            );
        }
        headers
    }

    #[test]
    fn the_service_group_is_the_first_path_segment() {
        assert_eq!(service_group("/chart/v3/charts"), "chart");
        assert_eq!(service_group("/port/v1/orders"), "port");
        assert_eq!(service_group("/trade/v2/orders/precheck"), "trade");
        assert_eq!(service_group("ref/v1/instruments/details"), "ref");
        assert_eq!(service_group("/"), "default");
    }

    #[test]
    fn the_budget_becomes_even_spacing_rather_than_a_burst_allowance() {
        // 100/min is one request every 600 ms, sustained. The old fixed sleep
        // was 500 ms, so the default is strictly more conservative than what
        // it replaces -- and unlike a token bucket it cannot spend the whole
        // minute's quota in three seconds.
        assert_eq!(baseline_spacing(100), Duration::from_millis(600));
        assert_eq!(baseline_spacing(120), Duration::from_millis(500));
        // A zero budget must not divide by zero or mean "unlimited".
        assert_eq!(baseline_spacing(0), WINDOW);
    }

    #[test]
    fn consecutive_requests_are_held_apart_by_the_spacing() {
        let mut group = GroupState::default();
        let start = Instant::now();
        let spacing = Duration::from_millis(600);

        assert_eq!(plan_delay(&mut group, start, spacing), None);
        // Immediately after, the next request must wait out the spacing.
        let delay = plan_delay(&mut group, start, spacing).expect("second request waits");
        assert_eq!(delay, spacing);
        // Partway through it still waits, for the remainder only.
        let delay = plan_delay(&mut group, start + Duration::from_millis(400), spacing)
            .expect("still waiting");
        assert_eq!(delay, Duration::from_millis(200));
        // Once the spacing has elapsed it goes out.
        assert_eq!(
            plan_delay(&mut group, start + Duration::from_millis(600), spacing),
            None
        );
    }

    #[test]
    fn a_planned_request_reserves_its_slot_immediately() {
        // If the reservation were left to the caller, two tasks could both be
        // told "go" against the same opening.
        let mut group = GroupState::default();
        let now = Instant::now();
        assert_eq!(plan_delay(&mut group, now, Duration::from_secs(1)), None);
        assert!(plan_delay(&mut group, now, Duration::from_secs(1)).is_some());
    }

    #[test]
    fn an_idle_group_does_not_accumulate_a_burst_allowance() {
        // After a long quiet period exactly one request goes out immediately,
        // not a minute's worth. This is the property a token bucket loses.
        let mut group = GroupState::default();
        let start = Instant::now();
        let spacing = Duration::from_millis(600);
        assert_eq!(plan_delay(&mut group, start, spacing), None);

        let much_later = start + Duration::from_secs(600);
        assert_eq!(plan_delay(&mut group, much_later, spacing), None);
        assert_eq!(
            plan_delay(&mut group, much_later, spacing),
            Some(spacing),
            "the second request after an idle period must still be spaced"
        );
    }

    #[test]
    fn exhausted_quota_waits_out_the_reset_window() {
        assert_eq!(
            spacing_from_quota(0, 42.0),
            Some(Duration::from_secs_f64(42.0))
        );
    }

    #[test]
    fn remaining_quota_is_spread_across_the_time_left() {
        // 10 requests left with 20 seconds to go is one every two seconds.
        assert_eq!(
            spacing_from_quota(10, 20.0),
            Some(Duration::from_secs_f64(2.0))
        );
        // Plenty of quota means effectively no spacing demanded.
        let spacing = spacing_from_quota(1_000, 60.0).expect("spacing");
        assert!(spacing < Duration::from_millis(100), "{spacing:?}");
    }

    #[test]
    fn a_missing_or_nonsensical_reset_demands_no_spacing() {
        assert_eq!(spacing_from_quota(5, 0.0), None);
        assert_eq!(spacing_from_quota(5, -1.0), None);
        assert_eq!(spacing_from_quota(5, f64::NAN), None);
    }

    #[test]
    fn the_tightest_reported_dimension_wins() {
        // Saxo reports several dimensions at once; the pace has to satisfy the
        // most constrained of them, not the average or the first one seen.
        let headers = header_map(&[
            ("X-RateLimit-AppDay-Remaining", "100000"),
            ("X-RateLimit-AppDay-Reset", "3600"),
            ("X-RateLimit-ChartMinute-Remaining", "4"),
            ("X-RateLimit-ChartMinute-Reset", "20"),
        ]);
        let (dimension, spacing) = spacing_from_headers(&headers).expect("spacing");
        assert_eq!(dimension, "chartminute");
        assert_eq!(spacing, Duration::from_secs_f64(5.0));
    }

    #[test]
    fn headers_without_a_matching_reset_are_ignored() {
        // A `Remaining` with no `Reset` cannot be turned into a pace, and
        // guessing one would either throttle for nothing or do nothing at all.
        let headers = header_map(&[("X-RateLimit-ChartMinute-Remaining", "1")]);
        assert!(spacing_from_headers(&headers).is_none());
        assert!(spacing_from_headers(&HeaderMap::new()).is_none());
    }

    #[test]
    fn a_header_driven_hold_delays_the_next_request() {
        let spacing = Duration::from_millis(600);
        let mut group = GroupState::default();
        let now = Instant::now();
        // What `observe` writes when Saxo reports quota running low.
        group.next_allowed_at = Some(now + Duration::from_secs(5));
        let delay = plan_delay(&mut group, now, spacing).expect("held");
        assert_eq!(delay, Duration::from_secs(5));
        // Once the hold has passed the request goes out, and the next one is
        // spaced normally again.
        assert_eq!(
            plan_delay(&mut group, now + Duration::from_secs(6), spacing),
            None
        );
        assert_eq!(
            plan_delay(&mut group, now + Duration::from_secs(6), spacing),
            Some(spacing)
        );
    }

    #[test]
    fn the_header_hold_wins_when_it_is_longer_than_the_baseline_spacing() {
        // Saxo's own accounting must be able to slow us below the configured
        // pace; the configured pace is a ceiling, not a floor.
        let mut group = GroupState::default();
        let now = Instant::now();
        group.next_allowed_at = Some(now + Duration::from_secs(3));
        assert_eq!(
            plan_delay(&mut group, now, Duration::from_millis(600)),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn no_single_wait_strands_a_nightly_job() {
        // A pathological Reset must slow the caller, not park it for an hour.
        // `acquire` loops, so the request still goes out once the hold expires.
        let mut group = GroupState::default();
        let now = Instant::now();
        group.next_allowed_at = Some(now + Duration::from_secs(3_600));
        assert_eq!(
            plan_delay(&mut group, now, Duration::from_millis(600)),
            Some(MAX_SINGLE_WAIT)
        );
    }

    #[test]
    fn the_pacer_reports_how_much_it_made_a_sweep_wait() {
        // A sweep's wall-clock duration cannot separate "200 assets take this
        // long" from "the price monitor is spending the same quota". Both
        // intraday Markov sweeps on 2026-08-31 took about 210s and there was no
        // idle-session control to compare against, which is what made the
        // contention question unanswerable rather than merely unanswered.
        let group = "chart-test-group";
        reset(group);

        let before = snapshot(group);
        assert_eq!(before["status"], "no_requests");

        {
            let mut guard = buckets().lock().expect("bucket lock");
            let state = guard.entry(group.to_string()).or_default();
            state.request_count = 201;
            state.waited_count = 187;
            state.total_waited_ms = 112_000;
            state.header_tightened_count = 3;
        }

        let during = snapshot(group);
        assert_eq!(during["status"], "ok");
        assert_eq!(during["request_count"], 201);
        assert_eq!(during["waited_count"], 187);
        assert_eq!(during["total_waited_ms"], 112_000);
        assert_eq!(
            during["header_tightened_count"], 3,
            "header-derived tightening is the signal that quota was depleting faster than the floor assumes"
        );

        // Reset zeroes the counters so the next sweep measures only itself,
        // without discarding the pacing state that keeps requests spaced.
        reset(group);
        let after = snapshot(group);
        assert_eq!(
            after["status"], "ok",
            "the group still exists after a reset"
        );
        assert_eq!(after["request_count"], 0);
        assert_eq!(after["total_waited_ms"], 0);
    }
}
