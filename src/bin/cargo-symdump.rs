use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[path = "../out.rs"]
mod out;

const DEFAULT_REPO: &str = "https://github.com/BlankMauser/symbaker";

fn usage() {
    eprintln!("cargo-symdump: build then dump exported symbols from newest .nro");
    eprintln!("usage:");
    eprintln!("  cargo symdump --release");
    eprintln!("  cargo symdump build --profile release --target-dir target");
    eprintln!("  cargo symdump skyline build --release");
    eprintln!("  cargo symdump dump path/to/file.nro");
    eprintln!("  cargo symdump update [--repo <git-url>] [--offline]");
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

    #[cfg(windows)]
    {
        // On Windows, self-updating while this binary is running can fail with
        // access denied. Run install in a detached process after this exits.
        let mut cmdline = String::from("Start-Sleep -Milliseconds 700; cargo");
        for arg in &install_args {
            let s = arg.to_string_lossy().replace('\'', "''");
            cmdline.push(' ');
            cmdline.push('\'');
            cmdline.push_str(&s);
            cmdline.push('\'');
        }
        Command::new("powershell")
            .args(["-NoProfile", "-Command", &cmdline])
            .spawn()
            .map_err(|e| format!("failed to start background update: {e}"))?;
        println!("update started in background from: {repo}");
        if offline {
            println!("mode: offline");
        }
        return Ok(());
    }

    #[cfg(not(windows))]
    let status = Command::new("cargo")
        .args(&install_args)
        .status()
        .map_err(|e| format!("failed to run cargo install: {e}"))?;
    #[cfg(not(windows))]
    if !status.success() {
        return Err(format!("cargo install failed for repo: {repo}"));
    }

    #[cfg(not(windows))]
    println!("updated cargo-symdump from: {repo}");
    #[cfg(not(windows))]
    if offline {
        println!("mode: offline");
    }
    #[cfg(not(windows))]
    return Ok(());
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
