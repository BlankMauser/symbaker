use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const DEFAULT_REPO: &str = "https://github.com/BlankMauser/symbaker";

#[cfg(windows)]
fn wait_for_pid(pid: u32) {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const INFINITE: u32 = 0xFFFF_FFFF;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            let _ = WaitForSingleObject(handle, INFINITE);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_pid(_pid: u32) {}

fn usage() {
    eprintln!("cargo-symdump-installer");
    eprintln!("usage:");
    eprintln!("  cargo-symdump-installer [--repo <git-url|commit>] [--offline] [--path <dir>] [--wait-pid <pid>]");
}

fn resolve_repo_arg(raw: &str) -> (String, Option<String>) {
    if let Some((repo, rev)) = raw.rsplit_once('#') {
        if !repo.is_empty() && !rev.is_empty() {
            return (repo.to_string(), Some(rev.to_string()));
        }
    }
    let is_hex = !raw.is_empty()
        && raw.len() >= 7
        && raw.len() <= 40
        && raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'));
    if is_hex {
        return (DEFAULT_REPO.to_string(), Some(raw.to_string()));
    }
    (raw.to_string(), None)
}

fn parse_args(
    args: &[OsString],
) -> Result<(String, Option<String>, bool, Option<PathBuf>, Option<u32>), String> {
    let mut repo_arg = DEFAULT_REPO.to_string();
    let mut offline = false;
    let mut install_root = None::<PathBuf>;
    let mut wait_pid = None::<u32>;
    let mut i = 0usize;
    while i < args.len() {
        let cur = args[i].to_string_lossy();
        if cur == "--repo" && i + 1 < args.len() {
            repo_arg = args[i + 1].to_string_lossy().to_string();
            i += 2;
            continue;
        }
        if let Some(v) = cur.strip_prefix("--repo=") {
            repo_arg = v.to_string();
            i += 1;
            continue;
        }
        if cur == "--offline" {
            offline = true;
            i += 1;
            continue;
        }
        if cur == "--path" && i + 1 < args.len() {
            install_root = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if let Some(v) = cur.strip_prefix("--path=") {
            install_root = Some(PathBuf::from(v.to_string()));
            i += 1;
            continue;
        }
        if cur == "--wait-pid" && i + 1 < args.len() {
            let pid = args[i + 1]
                .to_string_lossy()
                .parse::<u32>()
                .map_err(|_| "invalid --wait-pid value".to_string())?;
            wait_pid = Some(pid);
            i += 2;
            continue;
        }
        if let Some(v) = cur.strip_prefix("--wait-pid=") {
            let pid = v
                .parse::<u32>()
                .map_err(|_| "invalid --wait-pid value".to_string())?;
            wait_pid = Some(pid);
            i += 1;
            continue;
        }
        return Err(format!("unknown arg: {}", cur));
    }
    let (repo, rev) = resolve_repo_arg(&repo_arg);
    Ok((repo, rev, offline, install_root, wait_pid))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return ExitCode::SUCCESS;
    }

    let (repo, rev, offline, install_root, wait_pid) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}");
            usage();
            return ExitCode::FAILURE;
        }
    };

    if let Some(pid) = wait_pid {
        println!("waiting for cargo-symdump (pid {}) to exit...", pid);
        wait_for_pid(pid);
    }

    let mut cmd = Command::new("cargo");
    cmd.args([
        "install",
        "--git",
        &repo,
        "--bin",
        "cargo-symdump",
        "--bin",
        "cargo-symdump-installer",
        "--force",
    ]);
    if let Some(rev) = &rev {
        cmd.arg("--rev");
        cmd.arg(rev);
    }
    if offline {
        cmd.arg("--offline");
    }
    if let Some(root) = &install_root {
        cmd.arg("--root");
        cmd.arg(root);
    }

    println!("updating cargo-symdump from: {repo}");
    if offline {
        println!("mode: offline");
    }
    if let Some(root) = &install_root {
        println!("install root: {}", root.display());
    }

    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to run cargo install: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        eprintln!("error: cargo install failed for repo: {repo}");
        return ExitCode::FAILURE;
    }

    println!("updated cargo-symdump from: {repo}");
    ExitCode::SUCCESS
}
