use std::path::Path;

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn setup_hint() -> &'static str {
    "Run `cargo install --git https://github.com/BlankMauser/symbaker --bin cargo-symdump --force` then `cargo symdump init --prefix <your_prefix>` from workspace root."
}

/// Returns Ok(()) when symbaker one-time init markers are present and valid.
pub fn check_initialized() -> Result<(), String> {
    let initialized = env("SYMBAKER_INITIALIZED")
        .map(|v| truthy(&v))
        .unwrap_or(false);
    if !initialized {
        return Err(format!(
            "symbaker-build: missing SYMBAKER_INITIALIZED=1. {}",
            setup_hint()
        ));
    }

    let cfg = env("SYMBAKER_CONFIG")
        .ok_or_else(|| format!("symbaker-build: missing SYMBAKER_CONFIG. {}", setup_hint()))?;
    if !Path::new(&cfg).exists() {
        return Err(format!(
            "symbaker-build: SYMBAKER_CONFIG points to missing file: {}. {}",
            cfg,
            setup_hint()
        ));
    }

    let require_cfg = env("SYMBAKER_REQUIRE_CONFIG")
        .map(|v| truthy(&v))
        .unwrap_or(false);
    if !require_cfg {
        return Err(format!(
            "symbaker-build: expected SYMBAKER_REQUIRE_CONFIG=1 for deterministic builds. {}",
            setup_hint()
        ));
    }

    let enforce_inherit = env("SYMBAKER_ENFORCE_INHERIT")
        .map(|v| truthy(&v))
        .unwrap_or(false);
    if !enforce_inherit {
        return Err(format!(
            "symbaker-build: expected SYMBAKER_ENFORCE_INHERIT=1 to prevent dependency prefix leaks. {}",
            setup_hint()
        ));
    }

    Ok(())
}

/// Panics with an actionable message when the workspace is not symbaker-initialized.
pub fn require_initialized() {
    // Make changes in setup env/config retrigger build-script checks.
    println!("cargo:rerun-if-env-changed=SYMBAKER_INITIALIZED");
    println!("cargo:rerun-if-env-changed=SYMBAKER_CONFIG");
    println!("cargo:rerun-if-env-changed=SYMBAKER_REQUIRE_CONFIG");
    println!("cargo:rerun-if-env-changed=SYMBAKER_ENFORCE_INHERIT");

    if let Err(msg) = check_initialized() {
        panic!("{msg}");
    }
}
