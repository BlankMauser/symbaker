use proc_macro::TokenStream;
use quote::quote;
use std::{collections::HashMap, fs::OpenOptions, io::Write, sync::OnceLock};
use syn::{parse_macro_input, punctuated::Punctuated, Expr, ExprLit, ItemFn, ItemMod, Lit, Meta, Token};

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::Deserialize;

mod filter;

#[derive(Debug, Deserialize, Default)]
struct Config {
    prefix: Option<String>,
    sep: Option<String>,
    priority: Option<Vec<String>>,
    overrides: Option<HashMap<String, String>>,
}

#[derive(Clone, Copy, Debug)]
enum PrefixSource {
    Override,
    PreferPackagePrefixPackage,
    PreferPackagePrefixCrateFallback,
    Attr,
    EnvPrefix,
    Config,
    TopPackage,
    Workspace,
    Package,
    Crate,
    CrateFallbackAfterPriority,
}
fn sanitize(s: &str) -> String {
    let mut out: String = s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() { out.push('_'); }
    if out.chars().next().unwrap().is_ascii_digit() { out.insert(0, '_'); }
    out
}

fn trace_enabled() -> bool {
    match std::env::var("SYMBAKER_TRACE") {
        Ok(v) => {
            let n = v.trim().to_ascii_lowercase();
            matches!(n.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

fn trace_emit(line: impl AsRef<str>) {
    if !trace_enabled() {
        return;
    }
    let msg = format!("[symbaker] {}", line.as_ref());
    eprintln!("{msg}");

    let path = match std::env::var("SYMBAKER_TRACE_FILE") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{msg}");
    }
}

fn trace_bootstrap() {
    static DID_TRACE: OnceLock<()> = OnceLock::new();
    if DID_TRACE.get().is_some() || !trace_enabled() {
        return;
    }
    let _ = DID_TRACE.set(());
    trace_emit(format!(
        "env CARGO_PKG_NAME={:?} CARGO_MANIFEST_DIR={:?} CARGO_PRIMARY_PACKAGE={:?} SYMBAKER_TOP_PACKAGE={:?} SYMBAKER_PREFIX={:?} SYMBAKER_CONFIG={:?} SYMBAKER_PRIORITY={:?}",
        std::env::var("CARGO_PKG_NAME").ok(),
        std::env::var("CARGO_MANIFEST_DIR").ok(),
        std::env::var("CARGO_PRIMARY_PACKAGE").ok(),
        std::env::var("SYMBAKER_TOP_PACKAGE").ok(),
        std::env::var("SYMBAKER_PREFIX").ok(),
        std::env::var("SYMBAKER_CONFIG").ok(),
        std::env::var("SYMBAKER_PRIORITY").ok(),
    ));
}

fn trace_hard_fail() -> bool {
    matches!(std::env::var("SYMBAKER_TRACE_HARD").as_deref(), Ok("1"))
}

fn truthy_env(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let n = v.trim().to_ascii_lowercase();
            matches!(n.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

fn validate_required_config() -> Result<(), syn::Error> {
    if !truthy_env("SYMBAKER_REQUIRE_CONFIG") {
        return Ok(());
    }
    let path = match std::env::var("SYMBAKER_CONFIG") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "symbaker: SYMBAKER_REQUIRE_CONFIG=1 but SYMBAKER_CONFIG is missing. Run `cargo symdump init` in the workspace root.",
            ))
        }
    };
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "symbaker: SYMBAKER_CONFIG points to a missing file: {}. Run `cargo symdump init` again.",
                path
            ),
        ));
    }
    Ok(())
}

fn warn_if_not_initialized() {
    if truthy_env("SYMBAKER_INITIALIZED") {
        return;
    }
    static DID_WARN: OnceLock<()> = OnceLock::new();
    if DID_WARN.get().is_some() {
        return;
    }
    let _ = DID_WARN.set(());
    eprintln!(
        "warning: symbaker appears uninitialized (SYMBAKER_INITIALIZED not set). Run `cargo symdump init` at workspace root to install deterministic config/inheritance checks."
    );
}

fn trace_compile_error(msg: String) -> TokenStream {
    syn::Error::new(proc_macro2::Span::call_site(), msg)
        .to_compile_error()
        .into()
}

fn enforce_inherited_prefix(source: PrefixSource) -> Result<(), syn::Error> {
    if !truthy_env("SYMBAKER_ENFORCE_INHERIT") {
        return Ok(());
    }
    // Primary package is allowed to resolve with its own crate/package fallback.
    if std::env::var("CARGO_PRIMARY_PACKAGE").is_ok() {
        return Ok(());
    }
    // Explicit per-crate opt-outs or overrides remain valid in strict mode.
    match source {
        PrefixSource::Override
        | PrefixSource::PreferPackagePrefixPackage
        | PrefixSource::PreferPackagePrefixCrateFallback
        | PrefixSource::Attr
        | PrefixSource::EnvPrefix
        | PrefixSource::Config
        | PrefixSource::TopPackage
        | PrefixSource::Workspace => Ok(()),
        PrefixSource::Package | PrefixSource::Crate | PrefixSource::CrateFallbackAfterPriority => {
            let crate_name = std::env::var("CARGO_PKG_NAME").ok();
            Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "symbaker: dependency resolved to local {:?} source ({:?}) while SYMBAKER_ENFORCE_INHERIT=1. This would leak dependency prefixes. Run `cargo symdump init` in the top-level workspace, or set SYMBAKER_CONFIG/SYMBAKER_TOP_PACKAGE for this build, or add [overrides] entry.",
                    crate_name, source
                ),
            ))
        }
    }
}

fn warn_on_dependency_fallback(source: PrefixSource) {
    if truthy_env("SYMBAKER_ENFORCE_INHERIT") {
        return;
    }
    if std::env::var("CARGO_PRIMARY_PACKAGE").is_ok() {
        return;
    }
    match source {
        PrefixSource::Package | PrefixSource::Crate | PrefixSource::CrateFallbackAfterPriority => {
            static DID_WARN: OnceLock<()> = OnceLock::new();
            if DID_WARN.get().is_some() {
                return;
            }
            let _ = DID_WARN.set(());
            let crate_name = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".into());
            eprintln!(
                "warning: symbaker fallback detected in dependency crate {:?}: resolved local {:?} source. This can leak dependency prefixes into final exports. run `cargo symdump init` in workspace root (enables SYMBAKER_REQUIRE_CONFIG=1 and SYMBAKER_ENFORCE_INHERIT=1), or set SYMBAKER_CONFIG/SYMBAKER_TOP_PACKAGE explicitly.",
                crate_name, source
            );
        }
        _ => {}
    }
}

fn load_config() -> Config {
    // Highest-level “shared” config file path
    let cfg_path = std::env::var("SYMBAKER_CONFIG").ok();
    trace_emit(format!("load_config SYMBAKER_CONFIG={:?}", cfg_path));

    let mut fig = Figment::new();

    // Optional file config
    if let Some(p) = cfg_path.clone() {
        let exists = std::path::Path::new(&p).exists();
        trace_emit(format!("load_config merging file path={:?} exists={}", p, exists));
        fig = fig.merge(Toml::file(p));
    }

    // Optional env overrides:
    // SYMBAKER_PREFIX, SYMBAKER_SEP, SYMBAKER_PRIORITY
    fig = fig.merge(Env::prefixed("SYMBAKER_"));

    match fig.extract::<Config>() {
        Ok(cfg) => {
            trace_emit(format!(
                "load_config extracted prefix={:?} sep={:?} priority={:?}",
                cfg.prefix, cfg.sep, cfg.priority
            ));
            cfg
        }
        Err(e) => {
            trace_emit(format!("load_config extract error: {}", e));
            Config::default()
        }
    }
}

fn default_priority() -> Vec<String> {
    vec![
        "attr".into(),
        "env_prefix".into(), // SYMBAKER_PREFIX
        "config".into(),     // SYMBAKER_CONFIG file
        "top_package".into(), // top-level package being built
        "workspace".into(),
        "package".into(),
        "crate".into(),
    ]
}

fn top_level_package_name() -> Option<String> {
    detect_top_level_package_name()
}

fn detect_top_level_package_name() -> Option<String> {
    if let Ok(v) = std::env::var("SYMBAKER_TOP_PACKAGE") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }

    if std::env::var("CARGO_PRIMARY_PACKAGE").is_ok() {
        if let Ok(v) = std::env::var("CARGO_PKG_NAME") {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }

    None
}

fn read_prefix_from_workspace_metadata() -> Option<String> {
    // Only works when the crate being compiled is in/under a workspace
    // (path deps / workspace members). For git deps, this likely won’t find caller workspace.
    let mut dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
    loop {
        let cargo = dir.join("Cargo.toml");
        if cargo.exists() {
            let text = std::fs::read_to_string(&cargo).ok()?;
            let v: toml::Value = toml::from_str(&text).ok()?;
            if let Some(prefix) = v.get("workspace")
                .and_then(|w| w.get("metadata"))
                .and_then(|m| m.get("symbaker"))
                .and_then(|s| s.get("prefix"))
                .and_then(|p| p.as_str()) {
                trace_emit(format!(
                    "workspace metadata prefix found in {}: {:?}",
                    cargo.display(),
                    prefix
                ));
                return Some(prefix.to_string());
            }
        }
        if !dir.pop() { break; }
    }
    trace_emit("workspace metadata prefix not found while walking parent Cargo.toml files");
    None
}

fn read_prefix_from_package_metadata() -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let cargo = std::path::Path::new(&dir).join("Cargo.toml");
    let text = std::fs::read_to_string(cargo).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("symbaker"))
        .and_then(|s| s.get("prefix"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string())
}

fn read_package_prefers_own_prefix() -> bool {
    let dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(v) => v,
        Err(_) => return false,
    };
    let cargo = std::path::Path::new(&dir).join("Cargo.toml");
    let text = match std::fs::read_to_string(cargo) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let v: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("symbaker"))
        .and_then(|s| s.get("prefer_package_prefix"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
}

fn resolve_prefix(attr_prefix: Option<String>) -> (String, String, PrefixSource) {
    trace_bootstrap();

    let cfg = load_config();
    trace_emit(format!(
        "resolve_prefix input attr_prefix={:?} config.prefix={:?} config.sep={:?} config.priority={:?} config.overrides_keys={:?}",
        attr_prefix,
        cfg.prefix,
        cfg.sep,
        cfg.priority,
        cfg.overrides
            .as_ref()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
    ));

    let sep = cfg.sep.clone().unwrap_or_else(|| "__".into());
    let prio = cfg.priority.clone().unwrap_or_else(default_priority);
    let env_prefix = std::env::var("SYMBAKER_PREFIX").ok();
    let top_package = top_level_package_name();
    let workspace_prefix = read_prefix_from_workspace_metadata();
    let crate_name = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "crate".into());
    let package_prefix = read_prefix_from_package_metadata();
    let override_prefix = cfg
        .overrides
        .as_ref()
        .and_then(|m| m.get(&crate_name))
        .cloned();

    trace_emit(format!(
        "resolved candidates env_prefix={:?} top_package={:?} workspace_prefix={:?} package_prefix={:?} override_prefix={:?} crate={:?} sep={:?}",
        env_prefix, top_package, workspace_prefix, package_prefix, override_prefix, crate_name, sep
    ));

    if let Some(p) = &override_prefix {
        let chosen = sanitize(p);
        trace_emit(format!(
            "selected source=override(crate={:?}) raw={:?} sanitized={:?}",
            crate_name, p, chosen
        ));
        return (chosen, sep, PrefixSource::Override);
    }

    // Per-crate opt-out of inherited top-level prefix.
    // If set, package prefix wins (or crate name fallback if no explicit prefix).
    if read_package_prefers_own_prefix() {
        if let Some(p) = &package_prefix {
            let chosen = sanitize(p);
            trace_emit(format!(
                "selected source=prefer_package_prefix(package) raw={:?} sanitized={:?}",
                p, chosen
            ));
            return (chosen, sep, PrefixSource::PreferPackagePrefixPackage);
        }
        let chosen = sanitize(&crate_name);
        trace_emit(format!(
            "selected source=prefer_package_prefix(crate_fallback) raw={:?} sanitized={:?}",
            crate_name, chosen
        ));
        return (chosen, sep, PrefixSource::PreferPackagePrefixCrateFallback);
    }

    // Note: “config” here means the parsed file via SYMBAKER_CONFIG;
    // env overrides come via SYMBAKER_PREFIX.
    for key in prio {
        match key.as_str() {
            "attr" => if let Some(p) = &attr_prefix {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=attr raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::Attr);
            }
            "env_prefix" => if let Some(p) = &env_prefix {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=env_prefix raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::EnvPrefix);
            }
            "config" => if let Some(p) = &cfg.prefix {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=config raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::Config);
            }
            "top_package" => if let Some(p) = &top_package {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=top_package raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::TopPackage);
            }
            "workspace" => if let Some(p) = &workspace_prefix {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=workspace raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::Workspace);
            }
            "package" => if let Some(p) = &package_prefix {
                let chosen = sanitize(p);
                trace_emit(format!("selected source=package raw={:?} sanitized={:?}", p, chosen));
                return (chosen, sep, PrefixSource::Package);
            }
            "crate" => {
                let chosen = sanitize(&crate_name);
                trace_emit(format!("selected source=crate raw={:?} sanitized={:?}", crate_name, chosen));
                return (chosen, sep, PrefixSource::Crate);
            }
            _ => trace_emit(format!("priority key {:?} is unknown and ignored", key)),
        }
    }

    let chosen = sanitize(&crate_name);
    trace_emit(format!(
        "selected source=crate_fallback_after_priority raw={:?} sanitized={:?}",
        crate_name, chosen
    ));
    (chosen, sep, PrefixSource::CrateFallbackAfterPriority)
}

fn parse_attr_prefix(args: &Punctuated<Meta, Token![,]>) -> Option<String> {
    for a in args {
        if let Meta::NameValue(nv) = a {
            if nv.path.is_ident("prefix") {
                if let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = &nv.value {
                    return Some(s.value());
                }
            }
        }
    }
    None
}

fn push_export_name(fn_item: &mut ItemFn, export: String) {
    // Add/override export_name
    fn_item.attrs.retain(|a| !a.path().is_ident("export_name"));
    fn_item.attrs.push(syn::parse_quote!(#[export_name = #export]));
}

#[proc_macro_attribute]
pub fn symbaker(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let mut f = parse_macro_input!(item as ItemFn);

    warn_if_not_initialized();

    if let Err(e) = validate_required_config() {
        return e.to_compile_error().into();
    }

    if !f.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(&f.sig.generics, "symbaker: generic functions not supported")
            .to_compile_error()
            .into();
    }

    let attr_prefix = parse_attr_prefix(&args);
    let (prefix, sep, source) = resolve_prefix(attr_prefix);
    warn_on_dependency_fallback(source);
    if let Err(e) = enforce_inherited_prefix(source) {
        return e.to_compile_error().into();
    }

    let rust_name = f.sig.ident.to_string();
    let export = format!("{prefix}{sep}{rust_name}");
    trace_emit(format!(
        "macro=symbaker function={:?} resolved_prefix={:?} export_name={:?}",
        rust_name, prefix, export
    ));
    if trace_hard_fail() {
        return trace_compile_error(format!(
            "symbaker trace: macro=symbaker crate={:?} function={:?} prefix={:?} export={:?} top_package={:?} workspace={:?} package={:?} env_prefix={:?}",
            std::env::var("CARGO_PKG_NAME").ok(),
            rust_name,
            prefix,
            export,
            top_level_package_name(),
            read_prefix_from_workspace_metadata(),
            read_prefix_from_package_metadata(),
            std::env::var("SYMBAKER_PREFIX").ok(),
        ));
    }
    push_export_name(&mut f, export);

    TokenStream::from(quote!(#f))
}

#[proc_macro_attribute]
pub fn symbaker_module(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let mut m = parse_macro_input!(item as ItemMod);

    warn_if_not_initialized();

    if let Err(e) = validate_required_config() {
        return e.to_compile_error().into();
    }

    let attr_prefix = parse_attr_prefix(&args);
    let module_rules = match filter::parse_module_rules(&args) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };
    let (prefix, sep, source) = resolve_prefix(attr_prefix);
    warn_on_dependency_fallback(source);
    if let Err(e) = enforce_inherited_prefix(source) {
        return e.to_compile_error().into();
    }
    let module_name = m.ident.to_string();

    let items = match &mut m.content {
        Some((_, items)) => items,
        None => {
            return syn::Error::new_spanned(&m, "symbaker_module: must be inline `mod x { ... }`")
                .to_compile_error()
                .into();
        }
    };

    for it in items.iter_mut() {
        if let syn::Item::Fn(f) = it {
            let rust_name = f.sig.ident.to_string();
            if !module_rules.should_prefix(&module_name, &rust_name) { continue; }
            if !f.sig.generics.params.is_empty() { continue; }

            let export = module_rules.render_export_name(&prefix, &sep, &module_name, &rust_name);
            trace_emit(format!(
                "macro=symbaker_module module={:?} function={:?} resolved_prefix={:?} export_name={:?}",
                module_name, rust_name, prefix, export
            ));
            if trace_hard_fail() {
                return trace_compile_error(format!(
                    "symbaker trace: macro=symbaker_module crate={:?} module={:?} function={:?} prefix={:?} export={:?} top_package={:?} workspace={:?} package={:?} env_prefix={:?}",
                    std::env::var("CARGO_PKG_NAME").ok(),
                    module_name,
                    rust_name,
                    prefix,
                    export,
                    top_level_package_name(),
                    read_prefix_from_workspace_metadata(),
                    read_prefix_from_package_metadata(),
                    std::env::var("SYMBAKER_PREFIX").ok(),
                ));
            }
            push_export_name(f, export);
        }
    }

    TokenStream::from(quote!(#m))
}
