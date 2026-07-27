//! Shared HTTP transport for Saxo OpenAPI calls.
//!
//! A `reqwest::Client` owns the connection pool. Creating one for each Saxo
//! request prevents TLS and HTTP/2 reuse during the scheduler's chart and
//! portfolio sweeps. This module deliberately provides transport only: Saxo
//! OAuth, rate pacing, request ids, retries, and response semantics stay at
//! their existing call sites.

use std::{sync::LazyLock, time::Duration};

use anyhow::{Result, bail};

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

/// Maps the explicitly selected Saxo environment to its REST gateway.
///
/// Keep this mapping at the transport boundary. OAuth and all OpenAPI callers
/// must agree on the environment, and an unsupported value must fail closed.
pub(crate) fn openapi_base_url(environment: &str) -> Result<&'static str> {
    match environment.to_lowercase().as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

#[cfg(test)]
mod tests {
    use super::openapi_base_url;

    #[test]
    fn maps_supported_saxo_environments_to_their_gateways() {
        assert_eq!(
            openapi_base_url("SIM").expect("SIM is supported"),
            "https://gateway.saxobank.com/sim/openapi"
        );
        assert_eq!(
            openapi_base_url("live").expect("LIVE is supported"),
            "https://gateway.saxobank.com/openapi"
        );
    }

    #[test]
    fn rejects_an_unknown_saxo_environment() {
        let error = openapi_base_url("staging").expect_err("unknown environments fail closed");
        assert!(
            error
                .to_string()
                .contains("Unsupported Saxo environment: staging")
        );
    }
}
