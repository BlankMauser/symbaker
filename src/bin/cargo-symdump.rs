use std::env;
use std::ffi::OsString;
use std::fs;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use serde::Serialize;
use serde_json::Value;

#[path = "../out.rs"]
mod out;

const DEFAULT_REPO: &str = "https://github.com/BlankMauser/symbaker";

fn usage() {
    eprintln!("cargo-symdump: build then dump exported symbols from newest .nro");
    eprintln!("usage:");
    eprintln!("  cargo symdump init [--prefix <name>] [--force]");
    eprintln!("  cargo symdump --release");
    eprintln!("  cargo symdump build --profile release --target-dir target");
    eprintln!("  cargo symdump skyline build --release");
    eprintln!("  cargo symdump run <cargo-subcommand...>");
    eprintln!("  cargo symdump dump path/to/file.nro");
    eprintln!("  cargo symdump update [--repo <git-url>] [--offline]");
    eprintln!("  outputs:");
    eprintln!("  - .symbaker/sym.log");
    eprintln!("  - .symbaker/resolution.toml");
    eprintln!("  - .symbaker/trace.log");
}

fn find_flag_value(args: &[OsString], flag: &str) -> Option<PathBuf> {
    let mut i = 0usize;
    while i < args.len() {
        let cur = args[i].to_string_lossy();
        if cur == flag && i + 1 < args.len() {
            return Some(PathBuf::from(args[i + 1].clone()));
        }
        let prefix = format!("{flag}=");
        if cur.starts_with(&prefix) {
            return Some(PathBuf::from(cur[prefix.len()..].to_string()));
        }
        i += 1;
    }
    None
}

fn has_flag(args: &[OsString], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn profile_from_args(args: &[OsString]) -> Option<String> {
    if has_flag(args, "--release") {
        return Some("release".to_string());
    }
    let mut i = 0usize;
    while i < args.len() {
        let cur = args[i].to_string_lossy();
        if cur == "--profile" && i + 1 < args.len() {
            return Some(args[i + 1].to_string_lossy().to_string());
        }
        if let Some(v) = cur.strip_prefix("--profile=") {
            return Some(v.to_string());
        }
        i += 1;
    }
    None
}

fn target_dir_from_args(args: &[OsString]) -> PathBuf {
    if let Some(p) = find_flag_value(args, "--target-dir") {
        return p;
    }
    if let Ok(v) = env::var("CARGO_TARGET_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    PathBuf::from("target")
}

fn discover_default_config_path() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let candidate = dir.join("symbaker.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn discover_workspace_root() -> Result<PathBuf, String> {
    let mut dir = env::current_dir().map_err(|e| format!("current_dir: {e}"))?;
    loop {
        if dir.join("Cargo.toml").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("could not find Cargo.toml in current dir or parents".to_string());
        }
    }
}

fn discover_workspace_root_for_args(args: &[OsString]) -> Result<PathBuf, String> {
    if let Some(manifest) = out::manifest_path_from_args(args) {
        let p = if manifest.is_absolute() {
            manifest
        } else {
            env::current_dir()
                .map_err(|e| format!("current_dir: {e}"))?
                .join(manifest)
        };
        if let Some(parent) = p.parent() {
            return Ok(parent.to_path_buf());
        }
    }
    discover_workspace_root()
}

fn symbaker_output_dir(workspace_root: &PathBuf) -> Result<PathBuf, String> {
    let dir = workspace_root.join(".symbaker");
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    Ok(dir)
}

fn extract_quoted(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

#[derive(Default, Clone)]
struct TraceCrate {
    name: String,
    manifest_dir: Option<String>,
    selected_source: Option<String>,
    resolved_prefix: Option<String>,
    symbols: Vec<String>,
}

#[derive(Serialize)]
struct ResolutionCrate {
    name: String,
    manifest_dir: Option<String>,
    selected_source: Option<String>,
    resolved_prefix: Option<String>,
    dependencies: Vec<String>,
    symbols: Vec<String>,
}

#[derive(Serialize)]
struct ResolutionReport {
    generated_unix_utc: u64,
    top_package: Option<String>,
    symbaker_config: Option<String>,
    trace_file: String,
    crates: Vec<ResolutionCrate>,
    overrides_template: BTreeMap<String, String>,
}

fn parse_trace_file(path: &PathBuf) -> Result<BTreeMap<String, TraceCrate>, String> {
    let body = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut map: BTreeMap<String, TraceCrate> = BTreeMap::new();
    let mut current_crate = None::<String>;

    for line in body.lines() {
        if line.contains("env CARGO_PKG_NAME=Some(\"") {
            let crate_name = extract_quoted(line, "CARGO_PKG_NAME=Some(\"");
            let manifest = extract_quoted(line, "CARGO_MANIFEST_DIR=Some(\"");
            if let Some(name) = crate_name {
                current_crate = Some(name.clone());
                let entry = map.entry(name.clone()).or_default();
                entry.name = name;
                entry.manifest_dir = manifest;
            }
            continue;
        }
        if line.contains("selected source=") {
            if let Some(name) = &current_crate {
                let source = line
                    .split("selected source=")
                    .nth(1)
                    .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                    .filter(|s| !s.is_empty());
                let prefix = extract_quoted(line, "sanitized=\"");
                let entry = map.entry(name.clone()).or_default();
                if entry.name.is_empty() {
                    entry.name = name.clone();
                }
                if source.is_some() {
                    entry.selected_source = source;
                }
                if prefix.is_some() {
                    entry.resolved_prefix = prefix;
                }
            }
            continue;
        }
        if line.contains("export_name=\"") {
            if let Some(name) = &current_crate {
                if let Some(export) = extract_quoted(line, "export_name=\"") {
                    let entry = map.entry(name.clone()).or_default();
                    if !entry.symbols.iter().any(|s| s == &export) {
                        entry.symbols.push(export);
                    }
                }
            }
        }
    }

    Ok(map)
}

fn metadata_tree(args: &[OsString]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut cmd = Command::new("cargo");
    cmd.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest) = out::manifest_path_from_args(args) {
        cmd.arg("--manifest-path");
        cmd.arg(manifest);
    }
    let out = cmd.output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Ok(HashMap::new());
    }
    let parsed: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("parse metadata json: {e}"))?;

    let mut id_to_name = HashMap::<String, String>::new();
    if let Some(packages) = parsed.get("packages").and_then(|v| v.as_array()) {
        for p in packages {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            if !id.is_empty() && !name.is_empty() {
                id_to_name.insert(id.to_string(), name.to_string());
            }
        }
    }

    let mut deps_by_name = HashMap::<String, Vec<String>>::new();
    if let Some(nodes) = parsed.get("resolve").and_then(|r| r.get("nodes")).and_then(|v| v.as_array()) {
        for n in nodes {
            let id = n.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let Some(name) = id_to_name.get(id).cloned() else { continue };
            let mut deps = Vec::<String>::new();
            if let Some(d) = n.get("deps").and_then(|v| v.as_array()) {
                for dep in d {
                    if let Some(dep_pkg) = dep.get("pkg").and_then(|v| v.as_str()) {
                        if let Some(dep_name) = id_to_name.get(dep_pkg) {
                            if !deps.iter().any(|x| x == dep_name) {
                                deps.push(dep_name.clone());
                            }
                        }
                    }
                }
            }
            deps.sort();
            deps_by_name.insert(name, deps);
        }
    }
    Ok(deps_by_name)
}

fn write_resolution_report(workspace_root: &PathBuf, args: &[OsString], trace_file: &PathBuf) -> Result<PathBuf, String> {
    if !trace_file.exists() {
        return Err(format!("trace file missing: {}", trace_file.display()));
    }
    let traces = parse_trace_file(trace_file)?;
    let deps = metadata_tree(args).unwrap_or_default();

    let mut crates = Vec::<ResolutionCrate>::new();
    let mut overrides = BTreeMap::<String, String>::new();

    for (name, t) in traces {
        let mut symbols = t.symbols;
        symbols.sort();
        let deps_for = deps.get(&name).cloned().unwrap_or_default();
        if let Some(pref) = &t.resolved_prefix {
            overrides.insert(name.clone(), pref.clone());
        }
        crates.push(ResolutionCrate {
            name,
            manifest_dir: t.manifest_dir,
            selected_source: t.selected_source,
            resolved_prefix: t.resolved_prefix,
            dependencies: deps_for,
            symbols,
        });
    }
    crates.sort_by(|a, b| a.name.cmp(&b.name));

    let report = ResolutionReport {
        generated_unix_utc: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        top_package: env::var("SYMBAKER_TOP_PACKAGE").ok(),
        symbaker_config: env::var("SYMBAKER_CONFIG").ok(),
        trace_file: trace_file.display().to_string(),
        crates,
        overrides_template: overrides,
    };

    let out_dir = symbaker_output_dir(workspace_root)?;
    let out_path = out_dir.join("resolution.toml");
    let encoded = toml::to_string_pretty(&report).map_err(|e| format!("encode report toml: {e}"))?;
    fs::write(&out_path, encoded).map_err(|e| format!("write {}: {e}", out_path.display()))?;
    Ok(out_path)
}

fn parse_init_args(args: &[OsString]) -> Result<(Option<String>, bool), String> {
    let mut prefix = None::<String>;
    let mut force = false;
    let mut i = 0usize;
    while i < args.len() {
        let cur = args[i].to_string_lossy();
        if cur == "--force" {
            force = true;
            i += 1;
            continue;
        }
        if cur == "--prefix" {
            if i + 1 >= args.len() {
                return Err("missing value for --prefix".to_string());
            }
            prefix = Some(args[i + 1].to_string_lossy().to_string());
            i += 2;
            continue;
        }
        if let Some(v) = cur.strip_prefix("--prefix=") {
            prefix = Some(v.to_string());
            i += 1;
            continue;
        }
        return Err(format!("unknown init arg: {}", cur));
    }
    Ok((prefix, force))
}

fn run_init(args: Vec<OsString>) -> Result<(), String> {
    let (prefix, force) = parse_init_args(&args)?;
    let root = discover_workspace_root()?;
    let cfg_path = root.join("symbaker.toml");
    let out_dir = symbaker_output_dir(&root)?;
    let cargo_cfg_dir = root.join(".cargo");
    let cargo_cfg_path = cargo_cfg_dir.join("config.toml");

    if !cfg_path.exists() || force {
        let mut body = String::new();
        if let Some(p) = prefix {
            body.push_str(&format!("prefix = \"{}\"\n", p));
        } else {
        body.push_str("# prefix = \"hdr\"\n");
        }
        body.push_str("sep = \"__\"\n");
        body.push_str("priority = [\"attr\", \"env_prefix\", \"config\", \"top_package\", \"workspace\", \"package\", \"crate\"]\n");
        body.push_str("\n[overrides]\n");
        body.push_str("# ssbusync = \"hdr\"\n");
        fs::write(&cfg_path, body).map_err(|e| format!("write {}: {e}", cfg_path.display()))?;
        println!("wrote {}", cfg_path.display());
    } else {
        println!("kept existing {}", cfg_path.display());
    }

    fs::create_dir_all(&cargo_cfg_dir).map_err(|e| format!("mkdir {}: {e}", cargo_cfg_dir.display()))?;
    let mut doc = if cargo_cfg_path.exists() {
        let text = fs::read_to_string(&cargo_cfg_path)
            .map_err(|e| format!("read {}: {e}", cargo_cfg_path.display()))?;
        toml::from_str::<toml::Value>(&text).unwrap_or_else(|_| toml::Value::Table(Default::default()))
    } else {
        toml::Value::Table(Default::default())
    };

    let table = match doc.as_table_mut() {
        Some(t) => t,
        None => return Err(format!("{} is not a TOML table", cargo_cfg_path.display())),
    };
    let env_entry = table
        .entry("env".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let env_tbl = match env_entry.as_table_mut() {
        Some(t) => t,
        None => return Err(format!("{} has non-table [env]", cargo_cfg_path.display())),
    };
    let cfg_value = cfg_path.to_string_lossy().to_string();

    match env_tbl.get("SYMBAKER_CONFIG") {
        Some(existing) => {
            println!(
                "kept existing [env].SYMBAKER_CONFIG in {}: {}",
                cargo_cfg_path.display(),
                existing
            );
        }
        None => {
            env_tbl.insert("SYMBAKER_CONFIG".to_string(), toml::Value::String(cfg_value));
            println!(
                "added [env].SYMBAKER_CONFIG to {}",
                cargo_cfg_path.display()
            );
        }
    }
    match env_tbl.get("SYMBAKER_REQUIRE_CONFIG") {
        Some(existing) => {
            println!(
                "kept existing [env].SYMBAKER_REQUIRE_CONFIG in {}: {}",
                cargo_cfg_path.display(),
                existing
            );
        }
        None => {
            env_tbl.insert(
                "SYMBAKER_REQUIRE_CONFIG".to_string(),
                toml::Value::String("1".to_string()),
            );
            println!(
                "added [env].SYMBAKER_REQUIRE_CONFIG to {}",
                cargo_cfg_path.display()
            );
        }
    }
    match env_tbl.get("SYMBAKER_ENFORCE_INHERIT") {
        Some(existing) => {
            println!(
                "kept existing [env].SYMBAKER_ENFORCE_INHERIT in {}: {}",
                cargo_cfg_path.display(),
                existing
            );
        }
        None => {
            env_tbl.insert(
                "SYMBAKER_ENFORCE_INHERIT".to_string(),
                toml::Value::String("1".to_string()),
            );
            println!(
                "added [env].SYMBAKER_ENFORCE_INHERIT to {}",
                cargo_cfg_path.display()
            );
        }
    }
    match env_tbl.get("SYMBAKER_INITIALIZED") {
        Some(existing) => {
            println!(
                "kept existing [env].SYMBAKER_INITIALIZED in {}: {}",
                cargo_cfg_path.display(),
                existing
            );
        }
        None => {
            env_tbl.insert(
                "SYMBAKER_INITIALIZED".to_string(),
                toml::Value::String("1".to_string()),
            );
            println!(
                "added [env].SYMBAKER_INITIALIZED to {}",
                cargo_cfg_path.display()
            );
        }
    }

    let encoded = toml::to_string_pretty(&doc)
        .map_err(|e| format!("encode {}: {e}", cargo_cfg_path.display()))?;
    fs::write(&cargo_cfg_path, encoded).map_err(|e| format!("write {}: {e}", cargo_cfg_path.display()))?;
    println!("updated {}", cargo_cfg_path.display());
    println!("output dir: {}", out_dir.display());
    println!("symbaker init complete");
    Ok(())
}

fn apply_symbaker_env(cmd: &mut Command, cargo_args: &[OsString], workspace_root: &PathBuf) {
    if env::var_os("SYMBAKER_TOP_PACKAGE").is_none() {
        if let Some(pkg) = out::discover_top_package_name(cargo_args) {
            cmd.env("SYMBAKER_TOP_PACKAGE", pkg);
        }
    }
    if env::var_os("SYMBAKER_CONFIG").is_none() {
        if let Some(path) = discover_default_config_path() {
            cmd.env("SYMBAKER_CONFIG", path);
        }
    }
    if env::var_os("SYMBAKER_ENFORCE_INHERIT").is_none() {
        cmd.env("SYMBAKER_ENFORCE_INHERIT", "1");
    }
    if env::var_os("SYMBAKER_INITIALIZED").is_none() {
        cmd.env("SYMBAKER_INITIALIZED", "1");
    }
    if env::var_os("SYMBAKER_TRACE").is_none() {
        cmd.env("SYMBAKER_TRACE", "1");
    }
    if env::var_os("SYMBAKER_TRACE_FILE").is_none() {
        let trace_path = workspace_root.join(".symbaker").join("trace.log");
        cmd.env("SYMBAKER_TRACE_FILE", trace_path);
    }
}

fn run_build_then_dump(mut args: Vec<OsString>) -> Result<(), String> {
    // When invoked as `cargo symdump ...`, some environments may still include
    // a leading `symdump` token in argv. Drop it to avoid recursion.
    while args
        .first()
        .map(|s| s.to_string_lossy() == "symdump")
        .unwrap_or(false)
    {
        args.remove(0);
    }

    if args.is_empty() || args[0].to_string_lossy().starts_with('-') {
        args.insert(0, OsString::from("build"));
    }
    let workspace_root = discover_workspace_root_for_args(&args)?;
    let out_dir = symbaker_output_dir(&workspace_root)?;
    let trace_file = out_dir.join("trace.log");
    let _ = fs::remove_file(&trace_file);

    let mut build = Command::new("cargo");
    build.args(&args);
    apply_symbaker_env(&mut build, &args, &workspace_root);
    let status = build.status().map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("cargo {:?} failed", args));
    }

    let target_dir = target_dir_from_args(&args);
    let profile = profile_from_args(&args);
    let nro = out::newest_nro(&target_dir, profile.as_deref())?;
    let out = out::write_exports_sidecar(&nro)?;
    let sym_log = out::write_symbol_log(&nro, &out_dir.join("sym.log"))?;
    let resolution = write_resolution_report(&workspace_root, &args, &trace_file).ok();

    println!("nro: {}", nro.display());
    println!("exports: {}", out.display());
    println!("sym.log: {}", sym_log.display());
    if let Some(report) = resolution {
        println!("resolution: {}", report.display());
    }
    Ok(())
}

fn run_wrapped_cargo(mut args: Vec<OsString>) -> Result<(), String> {
    while args
        .first()
        .map(|s| s.to_string_lossy() == "symdump")
        .unwrap_or(false)
    {
        args.remove(0);
    }
    if args.is_empty() {
        return Err("usage: cargo symdump run <cargo-subcommand...>".to_string());
    }
    let workspace_root = discover_workspace_root_for_args(&args)?;
    let out_dir = symbaker_output_dir(&workspace_root)?;
    let trace_file = out_dir.join("trace.log");
    let _ = fs::remove_file(&trace_file);

    let mut cmd = Command::new("cargo");
    cmd.args(&args);
    apply_symbaker_env(&mut cmd, &args, &workspace_root);
    let status = cmd.status().map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo {:?} failed", args));
    }
    if let Ok(report) = write_resolution_report(&workspace_root, &args, &trace_file) {
        println!("resolution: {}", report.display());
    }
    Ok(())
}

fn run_dump_only(path: PathBuf) -> Result<(), String> {
    let nro = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
    let out = out::write_exports_sidecar(&nro)?;
    let root = discover_workspace_root()?;
    let out_dir = symbaker_output_dir(&root)?;
    let sym_log = out::write_symbol_log(&nro, &out_dir.join("sym.log"))?;
    println!("nro: {}", nro.display());
    println!("exports: {}", out.display());
    println!("sym.log: {}", sym_log.display());
    Ok(())
}

fn run_update(mut args: Vec<OsString>) -> Result<(), String> {
    let mut repo = DEFAULT_REPO.to_string();
    let mut offline = false;
    let mut i = 0usize;
    while i < args.len() {
        let cur = args[i].to_string_lossy();
        if cur == "--repo" && i + 1 < args.len() {
            repo = args[i + 1].to_string_lossy().to_string();
            args.remove(i + 1);
            args.remove(i);
            continue;
        }
        if let Some(v) = cur.strip_prefix("--repo=") {
            repo = v.to_string();
            args.remove(i);
            continue;
        }
        if cur == "--offline" {
            offline = true;
            args.remove(i);
            continue;
        }
        i += 1;
    }

    let mut install_args = vec![
        OsString::from("install"),
        OsString::from("--git"),
        OsString::from(repo.clone()),
        OsString::from("--bin"),
        OsString::from("cargo-symdump"),
        OsString::from("--force"),
    ];
    if offline {
        install_args.push(OsString::from("--offline"));
    }

    if cfg!(windows) {
        let repo_ps = repo.replace('\'', "''");
        let mut script = format!(
            "$ErrorActionPreference='Stop'; Start-Sleep -Milliseconds 1200; cargo install --git '{}' --bin cargo-symdump --force",
            repo_ps
        );
        if offline {
            script.push_str(" --offline");
        }
        let status = Command::new("cmd")
            .args([
                "/C",
                "start",
                "",
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .status()
            .map_err(|e| format!("failed to schedule Windows self-update: {e}"))?;
        if !status.success() {
            return Err("failed to schedule Windows self-update command".to_string());
        }
        println!("scheduled cargo-symdump update from: {repo}");
        println!("close this command and rerun after a moment to use the updated binary");
        if offline {
            println!("mode: offline");
        }
        return Ok(());
    }

    let status = Command::new("cargo")
        .args(&install_args)
        .status()
        .map_err(|e| format!("failed to run cargo install: {e}"))?;
    if !status.success() {
        return Err(format!("cargo install failed for repo: {repo}"));
    }

    println!("updated cargo-symdump from: {repo}");
    if offline {
        println!("mode: offline");
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    while args
        .first()
        .map(|s| s.to_string_lossy() == "symdump")
        .unwrap_or(false)
    {
        args.remove(0);
    }
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
        return ExitCode::SUCCESS;
    }

    let result = if args[0] == "dump" {
        if args.len() < 2 {
            Err("usage: cargo symdump dump path/to/file.nro".to_string())
        } else {
            run_dump_only(PathBuf::from(args.remove(1)))
        }
    } else if args[0] == "init" {
        run_init(args.into_iter().skip(1).collect())
    } else if args[0] == "run" {
        run_wrapped_cargo(args.into_iter().skip(1).collect())
    } else if args[0] == "update" {
        run_update(args.into_iter().skip(1).collect())
    } else {
        run_build_then_dump(args)
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
