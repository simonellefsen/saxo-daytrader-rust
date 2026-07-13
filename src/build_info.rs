/// Git revision embedded in release images at Docker build time.
///
/// Local `cargo` invocations intentionally fall back to `unknown`; deployment
/// smoke checks reject that value when verifying a Kubernetes rollout.
pub fn git_sha() -> &'static str {
    option_env!("DAYTRADER_GIT_SHA").unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::git_sha;

    #[test]
    fn build_revision_is_never_empty() {
        assert!(!git_sha().trim().is_empty());
    }
}
