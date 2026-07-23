fn main() {
    println!("cargo:rerun-if-env-changed=DAYTRADER_GIT_SHA");

    let git_sha = std::env::var("DAYTRADER_GIT_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=DAYTRADER_GIT_SHA={git_sha}");
}
