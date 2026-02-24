use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[path = "../out.rs"]
mod out;

fn usage() {
    eprintln!("cargo-symdump: build then dump exported symbols from newest .nro");
    eprintln!("usage:");
    eprintln!("  cargo symdump --release");
    eprintln!("  cargo symdump build --profile release --target-dir target");
    eprintln!("  cargo symdump skyline build --release");
    eprintln!("  cargo symdump dump path/to/file.nro");
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

fn run_build_then_dump(mut args: Vec<OsString>) -> Result<(), String> {
    if args.is_empty() || args[0].to_string_lossy().starts_with('-') {
        args.insert(0, OsString::from("build"));
    }

    let mut build = Command::new("cargo");
    build.args(&args);
    if env::var_os("SYMBAKER_TOP_PACKAGE").is_none() {
        if let Some(pkg) = out::discover_top_package_name(&args) {
            build.env("SYMBAKER_TOP_PACKAGE", pkg);
        }
    }
    let status = build.status().map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("cargo {:?} failed", args));
    }

    let target_dir = target_dir_from_args(&args);
    let profile = profile_from_args(&args);
    let nro = out::newest_nro(&target_dir, profile.as_deref())?;
    let out = out::write_exports_sidecar(&nro)?;

    println!("nro: {}", nro.display());
    println!("exports: {}", out.display());
    Ok(())
}

fn run_dump_only(path: PathBuf) -> Result<(), String> {
    let nro = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
    let out = out::write_exports_sidecar(&nro)?;
    println!("nro: {}", nro.display());
    println!("exports: {}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
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
