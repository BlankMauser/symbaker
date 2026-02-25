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
    match path.extension().and_then(OsStr::to_str) {
        Some("dll") | Some("so") | Some("dylib") => true,
        _ => false,
    }
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
fn exported_symbols_are_prefixed() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests").join("fixture_app");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .env_remove("SYMBAKER_PREFIX")
        .env_remove("SYMBAKER_CONFIG")
        .env_remove("SYMBAKER_PRIORITY")
        .env_remove("SYMBAKER_TOP_PACKAGE")
        .status()
        .expect("failed to run cargo build for fixture_app");
    assert!(status.success(), "fixture_app build failed");

    let artifact_root = fixture.join("target").join("debug");
    let lib = newest_dynamic_lib(&artifact_root, "fixture_app").unwrap_or_else(|| {
        panic!(
            "could not find built dynamic library under {}",
            artifact_root.display()
        )
    });

    let text = if lib.extension().and_then(OsStr::to_str) == Some("dll") {
        let Some(objdump) = pick_objdump_tool() else {
            eprintln!("skipping: no objdump-compatible tool found in PATH");
            return;
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
            eprintln!("skipping: no nm-compatible tool found in PATH");
            return;
        };
        let out = Command::new(nm)
            .args(["-g", "--defined-only"])
            .arg(&lib)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {nm}: {e}"));
        assert!(out.status.success(), "nm failed for {}", lib.display());
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    assert!(
        text.contains("fixture_app__auto_named"),
        "missing default top-package-prefixed symbol in {}",
        lib.display()
    );
    assert!(
        text.contains("custom__attr_named"),
        "missing attribute-prefixed symbol in {}",
        lib.display()
    );
}
