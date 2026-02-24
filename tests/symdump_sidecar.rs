use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
fn cargo_symdump_writes_sidecar_txt_next_to_nro() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests").join("fixture_app");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .status()
        .expect("failed to build fixture_app");
    assert!(status.success(), "fixture_app build failed");

    let artifact_root = fixture.join("target").join("debug");
    let lib = newest_dynamic_lib(&artifact_root, "fixture_app")
        .unwrap_or_else(|| panic!("could not find fixture dynamic library under {}", artifact_root.display()));

    let nro = artifact_root.join("fixture_app_test.nro");
    fs::copy(&lib, &nro).unwrap_or_else(|e| panic!("copy {} -> {}: {e}", lib.display(), nro.display()));

    let status = Command::new("cargo")
        .args(["run", "--bin", "cargo-symdump", "--", "dump"])
        .arg(&nro)
        .status()
        .expect("failed to run cargo-symdump");
    assert!(status.success(), "cargo-symdump dump failed");

    let sidecar = artifact_root.join("fixture_app_test.nro.exports.txt");
    assert!(sidecar.exists(), "missing sidecar file: {}", sidecar.display());
    let body = fs::read_to_string(&sidecar)
        .unwrap_or_else(|e| panic!("failed reading {}: {e}", sidecar.display()));
    assert!(
        body.contains("fixture_app__auto_named"),
        "sidecar missing expected symbol"
    );
}
