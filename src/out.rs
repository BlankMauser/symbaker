use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DT_NULL: u64 = 0;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_STRSZ: u64 = 10;

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

pub fn all_nros(target_dir: &Path, profile: Option<&str>) -> Result<Vec<PathBuf>, String> {
    if !target_dir.exists() {
        return Err(format!(
            "target dir does not exist: {}",
            target_dir.display()
        ));
    }

    let mut out = Vec::<PathBuf>::new();
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
            if !has_nro_extension(&path) {
                continue;
            }
            if let Some(p) = profile {
                let has_profile_segment = path.components().any(|c| c.as_os_str() == p);
                if !has_profile_segment {
                    continue;
                }
            }
            out.push(path);
        }
    }

    out.sort();
    if out.is_empty() {
        return Err(format!(
            "no .nro files found under {}",
            target_dir.display()
        ));
    }
    Ok(out)
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

fn run_nm(tool: &str, path: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let output = Command::new(tool)
        .args(args)
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run {tool}: {e}"))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_nm_symbols(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_objdump_exports(text: &str) -> Vec<String> {
    let mut symbols = Vec::<String>::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && parts[1].starts_with("0x")
        {
            let sym = parts[2];
            if !symbols.iter().any(|s| s == sym) {
                symbols.push(sym.to_string());
            }
        }
    }
    symbols
}

fn has_nro_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("nro"))
        .unwrap_or(false)
}

fn read_u32_le(bytes: &[u8], off: usize) -> Option<u32> {
    let end = off.checked_add(4)?;
    let chunk = bytes.get(off..end)?;
    Some(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn cstr_at(bytes: &[u8], off: usize, max_end: usize) -> Option<String> {
    if off >= max_end || off >= bytes.len() {
        return None;
    }
    let mut end = off;
    while end < max_end && end < bytes.len() {
        if bytes[end] == 0 {
            break;
        }
        end += 1;
    }
    if end <= off {
        return None;
    }
    std::str::from_utf8(&bytes[off..end])
        .ok()
        .map(|s| s.to_string())
}

#[derive(Clone, Debug)]
struct NroSymbol {
    name: String,
    value: u64,
    st_type: u8,
    st_bind: u8,
    size: u64,
    shndx: u16,
}

fn type_name(st_type: u8) -> &'static str {
    match st_type {
        0 => "NOTYPE",
        1 => "OBJECT",
        2 => "FUNC",
        3 => "SECTION",
        4 => "FILE",
        5 => "COMMON",
        6 => "TLS",
        _ => "UNKNOWN",
    }
}

fn bind_name(st_bind: u8) -> &'static str {
    match st_bind {
        0 => "LOCAL",
        1 => "GLOBAL",
        2 => "WEAK",
        _ => "UNKNOWN",
    }
}

fn read_u64_le(bytes: &[u8], off: usize) -> Option<u64> {
    let end = off.checked_add(8)?;
    let chunk = bytes.get(off..end)?;
    Some(u64::from_le_bytes([
        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
    ]))
}

fn read_u16_le(bytes: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let chunk = bytes.get(off..end)?;
    Some(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn parse_nro_symbols(path: &Path) -> Result<Vec<NroSymbol>, String> {
    let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let magic = data
        .get(0x10..0x14)
        .ok_or_else(|| "short file".to_string())?;
    if magic != b"NRO0" {
        return Ok(Vec::new());
    }

    // NRO section descriptors match the nxo64 loader layout:
    // tloc/tsize @ 0x20, rloc/rsize @ 0x28, dloc/dsize @ 0x30.
    let tloc = read_u32_le(&data, 0x20).ok_or_else(|| "invalid text offset".to_string())? as usize;
    let tsize = read_u32_le(&data, 0x24).ok_or_else(|| "invalid text size".to_string())? as usize;
    let rloc = read_u32_le(&data, 0x28).ok_or_else(|| "invalid ro offset".to_string())? as usize;
    let rsize = read_u32_le(&data, 0x2c).ok_or_else(|| "invalid ro size".to_string())? as usize;
    let dloc = read_u32_le(&data, 0x30).ok_or_else(|| "invalid data offset".to_string())? as usize;
    let dsize = read_u32_le(&data, 0x34).ok_or_else(|| "invalid data size".to_string())? as usize;

    let text_end = tloc.saturating_add(tsize);
    let ro_end = rloc.saturating_add(rsize);
    let data_end = dloc.saturating_add(dsize);
    if text_end > data.len() || ro_end > data.len() || data_end > data.len() {
        return Ok(Vec::new());
    }

    let text = &data[tloc..text_end];
    let ro = &data[rloc..ro_end];
    let dataseg = &data[dloc..data_end];

    let mut full = Vec::<u8>::new();
    full.extend_from_slice(text);
    if rloc > full.len() {
        full.resize(rloc, 0);
    } else if rloc < full.len() {
        full.truncate(rloc);
    }
    full.extend_from_slice(ro);
    if dloc > full.len() {
        full.resize(dloc, 0);
    } else if dloc < full.len() {
        full.truncate(dloc);
    }
    full.extend_from_slice(dataseg);

    let modoff = read_u32_le(&full, 4).ok_or_else(|| "missing MOD0 offset".to_string())? as usize;
    let mod_magic = full
        .get(modoff..modoff.saturating_add(4))
        .ok_or_else(|| "invalid MOD0 offset".to_string())?;
    if mod_magic != b"MOD0" {
        return Ok(Vec::new());
    }

    let dynamic_rel = read_u32_le(&full, modoff + 4)
        .ok_or_else(|| "invalid dynamic offset".to_string())? as usize;
    let dynamic_off = modoff.saturating_add(dynamic_rel);
    if dynamic_off >= full.len() {
        return Ok(Vec::new());
    }

    let mut strtab = None::<usize>;
    let mut strsz = None::<usize>;
    let mut symtab = None::<usize>;
    let mut off = dynamic_off;
    while off.saturating_add(16) <= full.len() {
        let tag = read_u64_le(&full, off).unwrap_or(DT_NULL);
        let val = read_u64_le(&full, off + 8).unwrap_or(0);
        off += 16;
        if tag == DT_NULL {
            break;
        }
        match tag {
            DT_STRTAB => strtab = Some(val as usize),
            DT_STRSZ => strsz = Some(val as usize),
            DT_SYMTAB => symtab = Some(val as usize),
            _ => {}
        }
    }

    let (dynstr_off, dynstr_size, dynsym_off) = match (strtab, strsz, symtab) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => return Ok(Vec::new()),
    };

    if dynstr_size == 0
        || dynstr_off >= full.len()
        || dynsym_off >= full.len()
        || dynsym_off >= dynstr_off
    {
        return Ok(Vec::new());
    }
    let dynstr_end = dynstr_off.saturating_add(dynstr_size).min(full.len());
    if dynstr_end <= dynstr_off {
        return Ok(Vec::new());
    }

    let entry_size = 24usize;
    let count = (dynstr_off - dynsym_off) / entry_size;
    let mut out = Vec::<NroSymbol>::new();
    for i in 0..count {
        let base = dynsym_off + i * entry_size;
        let name_idx = read_u32_le(&full, base).unwrap_or(0) as usize;
        if name_idx == 0 {
            continue;
        }
        let st_info = full.get(base + 4).copied().unwrap_or(0);
        let st_shndx = read_u16_le(&full, base + 6).unwrap_or(0);
        let st_value = read_u64_le(&full, base + 8).unwrap_or(0);
        let st_size = read_u64_le(&full, base + 16).unwrap_or(0);
        if st_shndx == 0 {
            continue;
        }
        let name_off = dynstr_off.saturating_add(name_idx);
        if let Some(name) = cstr_at(&full, name_off, dynstr_end) {
            if !name.is_empty() {
                out.push(NroSymbol {
                    name,
                    value: st_value,
                    st_type: st_info & 0x0f,
                    st_bind: st_info >> 4,
                    size: st_size,
                    shndx: st_shndx,
                });
            }
        }
    }

    out.sort_by(|a, b| {
        a.value
            .cmp(&b.value)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.shndx.cmp(&b.shndx))
    });
    Ok(out)
}

fn parse_nro_exports(path: &Path) -> Result<Vec<String>, String> {
    let rows = parse_nro_symbols(path)?;
    let mut names = Vec::<String>::new();
    for row in rows {
        if !names.iter().any(|n| n == &row.name) {
            names.push(row.name);
        }
    }
    Ok(names)
}

fn alt_symbol_source_for_nro(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let mut candidates = Vec::<PathBuf>::new();

    let explicit = [
        format!("{stem}.nso"),
        format!("{stem}.so"),
        format!("{stem}.elf"),
        format!("lib{stem}.nso"),
        format!("lib{stem}.so"),
        format!("lib{stem}.elf"),
    ];
    for name in explicit {
        let p = parent.join(name);
        if p.exists() {
            candidates.push(p);
        }
    }

    let scan_dirs = [parent.to_path_buf(), parent.join("deps")];
    for dir in scan_dirs {
        if !dir.exists() || !dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or_default();
            if !matches!(ext, "so" | "nso" | "elf" | "dll" | "dylib") {
                continue;
            }
            let fst = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if fst.contains(&stem) || stem.contains(fst.trim_start_matches("lib")) {
                candidates.push(p);
            }
        }
    }

    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for p in candidates {
        let Ok(meta) = fs::metadata(&p) else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        match &newest {
            Some((_, t)) if *t >= mtime => {}
            _ => newest = Some((p, mtime)),
        }
    }
    newest.map(|(p, _)| p)
}

pub fn exported_symbols(path: &Path) -> Result<Vec<String>, String> {
    let mut symbols = Vec::<String>::new();
    if path.extension().and_then(|s| s.to_str()) == Some("nro") {
        symbols = parse_nro_exports(path)?;
    }
    if symbols.is_empty() {
        if let Some(nm) = pick_nm() {
            let tries: [&[&str]; 4] = [
                &["-g", "--defined-only"],
                &["-D", "--defined-only"],
                &["-gD"],
                &["-g"],
            ];
            for t in tries {
                symbols = run_nm(&nm, path, t)?;
                if !symbols.is_empty() {
                    break;
                }
            }
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

    if symbols.is_empty() && path.extension().and_then(|s| s.to_str()) == Some("nro") {
        symbols = parse_nro_exports(path)?;
    }

    if symbols.is_empty() {
        return Err(
            "could not extract exported symbols from artifact (nm/objdump/nro parser found nothing)".to_string(),
        );
    }
    Ok(symbols)
}

pub fn write_exports_sidecar(path: &Path) -> Result<PathBuf, String> {
    let symbols = match exported_symbols(path) {
        Ok(s) => s,
        Err(original_err) => {
            if path.extension().and_then(|s| s.to_str()) == Some("nro") {
                if let Some(alt) = alt_symbol_source_for_nro(path) {
                    exported_symbols(&alt).map_err(|e| {
                        format!(
                            "{original_err}; fallback '{}' also failed: {e}",
                            alt.display()
                        )
                    })?
                } else {
                    return Err(original_err);
                }
            } else {
                return Err(original_err);
            }
        }
    };
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

pub fn write_symbol_log(path: &Path, out_path: &Path) -> Result<PathBuf, String> {
    let mut body = String::new();
    body.push_str("# symbaker sym.log\n");
    body.push_str(&format!("# source={}\n", path.display()));
    if path.extension().and_then(|s| s.to_str()) == Some("nro") {
        let rows = parse_nro_symbols(path)?;
        body.push_str("# format: address type bind size name\n");
        for row in rows {
            body.push_str(&format!(
                "0x{0:016X} {1} {2} 0x{3:X} {4}\n",
                row.value,
                type_name(row.st_type),
                bind_name(row.st_bind),
                row.size,
                row.name
            ));
        }
    } else {
        let symbols = exported_symbols(path)?;
        body.push_str("# format: name\n");
        for sym in symbols {
            body.push_str(&sym);
            body.push('\n');
        }
    }

    fs::write(out_path, body).map_err(|e| format!("write {}: {e}", out_path.display()))?;
    Ok(out_path.to_path_buf())
}
