use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn manifest_path_from_args(args: &[OsString]) -> Option<PathBuf> {
    find_flag_value(args, "--manifest-path")
}

pub fn discover_top_package_name(args: &[OsString]) -> Option<String> {
    let mut cmd = Command::new("cargo");
    cmd.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest) = manifest_path_from_args(args) {
        cmd.arg("--manifest-path");
        cmd.arg(manifest);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }

    let parsed: Value = serde_json::from_slice(&out.stdout).ok()?;
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

pub fn newest_nro(target_dir: &Path, profile: Option<&str>) -> Result<PathBuf, String> {
    if !target_dir.exists() {
        return Err(format!("target dir does not exist: {}", target_dir.display()));
    }

    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    let mut stack = vec![target_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read_dir entry error: {e}"))?;
            let path = entry.path();
            let meta = entry
                .metadata()
                .map_err(|e| format!("metadata {}: {e}", path.display()))?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("nro") {
                continue;
            }
            if let Some(p) = profile {
                let has_profile_segment = path.components().any(|c| c.as_os_str() == p);
                if !has_profile_segment {
                    continue;
                }
            }
            let mtime = meta
                .modified()
                .map_err(|e| format!("modified {}: {e}", path.display()))?;
            match &best {
                Some((_, t)) if *t >= mtime => {}
                _ => best = Some((path, mtime)),
            }
        }
    }

    best.map(|(p, _)| p)
        .ok_or_else(|| format!("no .nro files found under {}", target_dir.display()))
}

fn pick_nm() -> Option<String> {
    for tool in ["llvm-nm", "nm", "rust-nm", "aarch64-none-elf-nm"] {
        if Command::new(tool).arg("--version").output().is_ok() {
            return Some(tool.to_string());
        }
    }
    None
}

fn pick_objdump() -> Option<String> {
    for tool in ["llvm-objdump", "objdump"] {
        if Command::new(tool).arg("--version").output().is_ok() {
            return Some(tool.to_string());
        }
    }
    None
}

fn parse_nm_symbols(text: &str) -> Vec<String> {
    let mut symbols = Vec::<String>::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let Some(sym) = parts.by_ref().last() {
            if !symbols.iter().any(|s| s == sym) {
                symbols.push(sym.to_string());
            }
        }
    }
    symbols
}

fn parse_objdump_exports(text: &str) -> Vec<String> {
    let mut symbols = Vec::<String>::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0].chars().all(|c| c.is_ascii_digit()) && parts[1].starts_with("0x") {
            let sym = parts[2];
            if !symbols.iter().any(|s| s == sym) {
                symbols.push(sym.to_string());
            }
        }
    }
    symbols
}

pub fn exported_symbols(path: &Path) -> Result<Vec<String>, String> {
    let mut symbols = Vec::<String>::new();
    if let Some(nm) = pick_nm() {
        let output = Command::new(nm)
            .args(["-g", "--defined-only"])
            .arg(path)
            .output()
            .map_err(|e| format!("failed to run nm: {e}"))?;
        if output.status.success() {
            symbols = parse_nm_symbols(&String::from_utf8_lossy(&output.stdout));
        }
    }

    if symbols.is_empty() {
        if let Some(objdump) = pick_objdump() {
            let out = Command::new(objdump)
                .args(["-p"])
                .arg(path)
                .output()
                .map_err(|e| format!("failed to run objdump: {e}"))?;
            if out.status.success() {
                symbols = parse_objdump_exports(&String::from_utf8_lossy(&out.stdout));
            }
        }
    }

    if symbols.is_empty() {
        return Err("could not extract exported symbols; ensure llvm-nm/nm works for this artifact".to_string());
    }
    Ok(symbols)
}

pub fn write_exports_sidecar(path: &Path) -> Result<PathBuf, String> {
    let symbols = exported_symbols(path)?;
    let out_path = path
        .parent()
        .ok_or_else(|| "invalid artifact path".to_string())?
        .join(format!(
            "{}.exports.txt",
            path.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| "invalid artifact file name".to_string())?
        ));
    let body = if symbols.is_empty() {
        String::new()
    } else {
        format!("{}\n", symbols.join("\n"))
    };
    fs::write(&out_path, body).map_err(|e| format!("write {}: {e}", out_path.display()))?;
    Ok(out_path)
}
