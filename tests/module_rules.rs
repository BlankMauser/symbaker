use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn pick_nm_tool() -> Option<&'static str> {
    for tool in ["llvm-nm", "nm", "rust-nm", "aarch64-none-elf-nm"] {
        if Command::new(tool).arg("--version").output().is_ok() {
            return Some(tool);
        }
    }
    None
}

fn pick_objdump_tool() -> Option<&'static str> {
    for tool in ["llvm-objdump", "objdump"] {
        if Command::new(tool).arg("--version").output().is_ok() {
            return Some(tool);
        }
    }
    None
}

fn is_dynamic_lib(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("dll") | Some("so") | Some("dylib")
    )
}

fn newest_dynamic_lib(root: &Path, stem: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let path = entry.path();
            let meta = entry.metadata().ok()?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !is_dynamic_lib(&path) {
                continue;
            }
            let fname = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            if !fname.contains(stem) {
                continue;
            }
            let mtime = meta.modified().ok()?;
            match &best {
                Some((_, t)) if *t >= mtime => {}
                _ => best = Some((path, mtime)),
            }
        }
    }
    best.map(|(p, _)| p)
}

#[test]
fn module_rules_control_prefixing_and_template() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests").join("rules_app");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .env_remove("SYMBAKER_PREFIX")
        .env_remove("SYMBAKER_CONFIG")
        .env_remove("SYMBAKER_PRIORITY")
        .env_remove("SYMBAKER_TOP_PACKAGE")
        .status()
        .expect("failed to build rules_app");
    assert!(status.success(), "rules_app build failed");

    let artifact_root = fixture.join("target").join("debug");
    let lib = newest_dynamic_lib(&artifact_root, "rules_app")
        .unwrap_or_else(|| panic!("could not find rules_app artifact under {}", artifact_root.display()));

    let text = if lib.extension().and_then(OsStr::to_str) == Some("dll") {
        let Some(objdump) = pick_objdump_tool() else {
            panic!("no objdump-compatible tool found");
        };
        let out = Command::new(objdump)
            .args(["-p"])
            .arg(&lib)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {objdump}: {e}"));
        assert!(out.status.success(), "objdump failed for {}", lib.display());
        String::from_utf8_lossy(&out.stdout).to_string()
    } else {
        let Some(nm) = pick_nm_tool() else {
            panic!("no nm-compatible tool found");
        };
        let out = Command::new(nm)
            .args(["-g", "--defined-only"])
            .arg(&lib)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {nm}: {e}"));
        assert!(out.status.success(), "nm failed for {}", lib.display());
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    assert!(text.contains("rules_app__exports_keep_one_x"), "missing keep_one export");
    assert!(text.contains("rules_app__exports_special_x"), "missing special export");
    assert!(!text.contains("rules_app__exports_keep_skip_x"), "exclude glob failed");
    assert!(!text.contains("rules_app__exports_other_x"), "include regex failed");
}
