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

fn read_exports(lib: &Path) -> Option<String> {
    if lib.extension().and_then(OsStr::to_str) == Some("dll") {
        let objdump = pick_objdump_tool()?;
        let out = Command::new(objdump).args(["-p"]).arg(lib).output().ok()?;
        if !out.status.success() {
            return None;
        }
        return Some(String::from_utf8_lossy(&out.stdout).to_string());
    }

    let nm = pick_nm_tool()?;
    let out = Command::new(nm)
        .args(["-g", "--defined-only"])
        .arg(lib)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn dependency_symbol_uses_host_package_prefix_and_writes_sidecar() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let host = root.join("tests").join("host_app");
    let target_dir = host.join("target");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(host.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove("SYMBAKER_PREFIX")
        .env_remove("SYMBAKER_CONFIG")
        .env_remove("SYMBAKER_PRIORITY")
        .env("SYMBAKER_TOP_PACKAGE", "host_app")
        .status()
        .expect("failed to build host_app");
    assert!(status.success(), "host_app build failed");

    let artifact_root = target_dir.join("debug");
    let lib = newest_dynamic_lib(&artifact_root, "host_app").unwrap_or_else(|| {
        panic!(
            "could not find host_app artifact under {}",
            artifact_root.display()
        )
    });

    let exports = read_exports(&lib)
        .unwrap_or_else(|| panic!("failed reading exports from {}", lib.display()));
    assert!(
        exports.contains("host_app__dep_exported"),
        "expected dependency export to use host prefix; artifact: {}",
        lib.display()
    );

    let nro = artifact_root.join("host_app_test.nro");
    fs::copy(&lib, &nro)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", lib.display(), nro.display()));

    let status = Command::new("cargo")
        .args(["run", "--bin", "cargo-symdump", "--", "dump"])
        .arg(&nro)
        .status()
        .expect("failed to run cargo-symdump dump");
    assert!(status.success(), "cargo-symdump dump failed");

    let sidecar = artifact_root.join("host_app_test.nro.exports.txt");
    assert!(
        sidecar.exists(),
        "missing sidecar file: {}",
        sidecar.display()
    );
    let sidecar_body = fs::read_to_string(&sidecar)
        .unwrap_or_else(|e| panic!("failed reading {}: {e}", sidecar.display()));
    assert!(
        sidecar_body.contains("host_app__dep_exported"),
        "sidecar missing host-prefixed dependency export"
    );
}

#[test]
fn workspace_prefix_overrides_dependency_prefix_without_top_package_env() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = root.join("tests").join("workspace_host");
    let target_dir = workspace.join("target");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .arg("-p")
        .arg("host_ws")
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove("SYMBAKER_PREFIX")
        .env_remove("SYMBAKER_CONFIG")
        .env_remove("SYMBAKER_PRIORITY")
        .env_remove("SYMBAKER_TOP_PACKAGE")
        .status()
        .expect("failed to build workspace host");
    assert!(status.success(), "workspace host build failed");

    let artifact_root = target_dir.join("debug");
    let lib = newest_dynamic_lib(&artifact_root, "host_ws").unwrap_or_else(|| {
        panic!(
            "could not find host_ws artifact under {}",
            artifact_root.display()
        )
    });

    let exports = read_exports(&lib)
        .unwrap_or_else(|| panic!("failed reading exports from {}", lib.display()));
    assert!(
        exports.contains("hdr__dep_exported"),
        "expected workspace prefix on dependency export; artifact: {}",
        lib.display()
    );
    assert!(
        !exports.contains("ssbusync__dep_exported"),
        "dependency prefix leaked into host export set; artifact: {}",
        lib.display()
    );
}
