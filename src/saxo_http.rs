//! Shared HTTP transport for Saxo OpenAPI calls.
//!
//! A `reqwest::Client` owns the connection pool. Creating one for each Saxo
//! request prevents TLS and HTTP/2 reuse during the scheduler's chart and
//! portfolio sweeps. This module deliberately provides transport only: Saxo
//! OAuth, rate pacing, request ids, retries, and response semantics stay at
//! their existing call sites.

use std::{sync::LazyLock, time::Duration};

static SAXO_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("constructing the static Saxo HTTP client")
});

/// Returns a cheap handle to the process-wide, 30-second Saxo HTTP client.
pub(crate) fn client() -> reqwest::Client {
    SAXO_HTTP_CLIENT.clone()
}
