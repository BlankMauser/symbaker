use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}_{ts}_{}", std::process::id()))
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

#[test]
fn cargo_symdump_dump_accepts_folder_and_writes_sidecars_for_nros() {
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

    let dump_root = unique_temp_dir("symdump_folder_mode");
    let sub_dir = dump_root.join("nested");
    fs::create_dir_all(&sub_dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", sub_dir.display()));

    let nro_a = dump_root.join("alpha.nro");
    let nro_b = sub_dir.join("beta.nro");
    fs::copy(&lib, &nro_a).unwrap_or_else(|e| panic!("copy {} -> {}: {e}", lib.display(), nro_a.display()));
    fs::copy(&lib, &nro_b).unwrap_or_else(|e| panic!("copy {} -> {}: {e}", lib.display(), nro_b.display()));

    let status = Command::new("cargo")
        .args(["run", "--bin", "cargo-symdump", "--", "dump"])
        .arg(&nro_a)
        .status()
        .expect("failed to run cargo-symdump dump");
    assert!(status.success(), "single-file dump failed unexpectedly");

    let status = Command::new("cargo")
        .args(["run", "--bin", "cargo-symdump", "--", "dump"])
        .arg(&dump_root)
        .status()
        .expect("failed to run cargo-symdump folder dump");
    assert!(
        status.success(),
        "folder dump should still succeed while logging duplicate symbols"
    );

    let sidecar_a = dump_root.join("alpha.nro.exports.txt");
    let sidecar_b = sub_dir.join("beta.nro.exports.txt");
    assert!(sidecar_a.exists(), "missing sidecar file: {}", sidecar_a.display());
    assert!(sidecar_b.exists(), "missing sidecar file: {}", sidecar_b.display());

    let dup_log = root.join(".symbaker").join("duplicates.log");
    assert!(dup_log.exists(), "missing duplicate log: {}", dup_log.display());
    let dup_body =
        fs::read_to_string(&dup_log).unwrap_or_else(|e| panic!("read {}: {e}", dup_log.display()));
    assert!(
        dup_body.contains("fixture_app__auto_named"),
        "duplicate report missing expected symbol"
    );
}
