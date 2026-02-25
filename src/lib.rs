use proc_macro::TokenStream;
use quote::quote;
use serde_json::Value;
use std::{process::Command, sync::OnceLock};
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
}
fn sanitize(s: &str) -> String {
    let mut out: String = s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() { out.push('_'); }
    if out.chars().next().unwrap().is_ascii_digit() { out.insert(0, '_'); }
    out
}

fn load_config() -> Config {
    // Highest-level “shared” config file path
    let cfg_path = std::env::var("SYMBAKER_CONFIG").ok();

    let mut fig = Figment::new();

    // Optional file config
    if let Some(p) = cfg_path {
        fig = fig.merge(Toml::file(p));
    }

    // Optional env overrides:
    // SYMBAKER_PREFIX, SYMBAKER_SEP, SYMBAKER_PRIORITY
    fig = fig.merge(Env::prefixed("SYMBAKER_"));

    fig.extract::<Config>().unwrap_or_default()
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
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(detect_top_level_package_name)
        .clone()
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

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let parsed: Value = serde_json::from_slice(&output.stdout).ok()?;
    let root_id = parsed
        .get("resolve")
        .and_then(|r| r.get("root"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            parsed
                .get("workspace_default_members")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;

    parsed
        .get("packages")
        .and_then(|v| v.as_array())?
        .iter()
        .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(root_id.as_str()))
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn read_prefix_from_workspace_metadata() -> Option<String> {
    // Only works when the crate being compiled is in/under a workspace
    // (path deps / workspace members). For git deps, this likely won’t find caller workspace.
    let mut dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
    loop {
        let cargo = dir.join("Cargo.toml");
        if cargo.exists() {
            let text = std::fs::read_to_string(&cargo).ok()?;
            if text.contains("[workspace]") && text.contains("[workspace.metadata.symbaker]") {
                let v: toml::Value = toml::from_str(&text).ok()?;
                return v.get("workspace")
                    .and_then(|w| w.get("metadata"))
                    .and_then(|m| m.get("symbaker"))
                    .and_then(|s| s.get("prefix"))
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string());
            }
        }
        if !dir.pop() { break; }
    }
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

fn resolve_prefix(attr_prefix: Option<String>) -> (String, String) {
    let cfg = load_config();

    let sep = cfg.sep.clone().unwrap_or_else(|| "__".into());
    let prio = cfg.priority.clone().unwrap_or_else(default_priority);
    let package_prefix = read_prefix_from_package_metadata();

    // Per-crate opt-out of inherited top-level prefix.
    // If set, package prefix wins (or crate name fallback if no explicit prefix).
    if read_package_prefers_own_prefix() {
        if let Some(p) = &package_prefix {
            return (sanitize(p), sep);
        }
        let p = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "crate".into());
        return (sanitize(&p), sep);
    }

    // Note: “config” here means the parsed file via SYMBAKER_CONFIG;
    // env overrides come via SYMBAKER_PREFIX.
    for key in prio {
        match key.as_str() {
            "attr" => if let Some(p) = &attr_prefix { return (sanitize(p), sep); }
            "env_prefix" => if let Ok(p) = std::env::var("SYMBAKER_PREFIX") { return (sanitize(&p), sep); }
            "config" => if let Some(p) = &cfg.prefix { return (sanitize(p), sep); }
            "top_package" => if let Some(p) = top_level_package_name() { return (sanitize(&p), sep); }
            "workspace" => if let Some(p) = read_prefix_from_workspace_metadata() { return (sanitize(&p), sep); }
            "package" => if let Some(p) = &package_prefix { return (sanitize(p), sep); }
            "crate" => {
                let p = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "crate".into());
                return (sanitize(&p), sep);
            }
            _ => {}
        }
    }

    let p = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "crate".into());
    (sanitize(&p), sep)
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

    if !f.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(&f.sig.generics, "symbaker: generic functions not supported")
            .to_compile_error()
            .into();
    }

    let attr_prefix = parse_attr_prefix(&args);
    let (prefix, sep) = resolve_prefix(attr_prefix);

    let rust_name = f.sig.ident.to_string();
    let export = format!("{prefix}{sep}{rust_name}");
    push_export_name(&mut f, export);

    TokenStream::from(quote!(#f))
}

#[proc_macro_attribute]
pub fn symbaker_module(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let mut m = parse_macro_input!(item as ItemMod);

    let attr_prefix = parse_attr_prefix(&args);
    let module_rules = match filter::parse_module_rules(&args) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };
    let (prefix, sep) = resolve_prefix(attr_prefix);
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
            push_export_name(f, export);
        }
    }

    TokenStream::from(quote!(#m))
}
