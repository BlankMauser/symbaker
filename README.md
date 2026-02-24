# symbaker

`symbaker` is a proc-macro crate that rewrites exported symbol names with a prefix.

It is intended for Switch/homebrew-style workflows where symbol namespace collisions matter.

## What it does

- `#[symbaker]` on a function sets `#[export_name = "..."]`.
- `#[symbaker_module]` on an inline module applies the same behavior to export-like functions in that module.
- Prefix resolution is configurable and defaults to a priority chain.

Default priority:

1. `attr` (`#[symbaker(prefix = "...")]`)
2. `env_prefix` (`SYMBAKER_PREFIX`)
3. `config` (`SYMBAKER_CONFIG` TOML)
4. `top_package` (top-level package currently being built)
5. `workspace` (`[workspace.metadata.symbaker]`)
6. `package` (`[package.metadata.symbaker]`)
7. `crate` (`CARGO_PKG_NAME`)

## Install in a user crate

```toml
[dependencies]
symbaker = { git = "https://github.com/your-org/symbaker" }
```

## Macro usage

```rust
use symbaker::symbaker;

#[symbaker]
pub extern "C" fn my_export() {}

#[symbaker(prefix = "my_mod")]
pub extern "C" fn my_export2() {}
```

## Config

Environment variables:

- `SYMBAKER_PREFIX`
- `SYMBAKER_SEP` (default: `__`)
- `SYMBAKER_PRIORITY` (comma-separated keys from priority list above)
- `SYMBAKER_CONFIG` (path to TOML config file)
- `SYMBAKER_TOP_PACKAGE` (explicit top package override)

Example TOML file:

```toml
prefix = "mygame"
sep = "__"
priority = ["attr", "env_prefix", "config", "top_package", "workspace", "package", "crate"]
```

## Export dump tool

This repo includes two ways to dump exported symbols from a built `.nro` into a sidecar text file:

- Python script: `scripts/dump_nro_exports.py`
- Cargo subcommand binary: `cargo-symdump`

### Recommended workflow

`cargo` does not support a built-in `cargo build --symdump` post-build hook.
Use the subcommand instead:

```bash
cargo symdump --release
```

Update the tool later with:

```bash
cargo symdump update
```

`cargo-symdump` automatically sets `SYMBAKER_TOP_PACKAGE` for that build when it can resolve the top package via `cargo metadata`.

This runs `cargo build --release`, finds the newest `.nro` in the target dir, and writes:

- `<artifact>.nro.exports.txt`

Dump-only mode:

```bash
cargo symdump dump path/to/file.nro
```

Result:

- The `.nro` stays in place.
- A sibling text file is emitted in the same folder (same base filename + `.exports.txt`).

## Rust testing workflow

Rust already has a standard test workflow:

- Unit tests in source files via `#[cfg(test)]`.
- Integration tests in `tests/`.
- Run all tests with:

```bash
cargo test
```

This repo includes an integration test that builds a fixture crate and validates exported symbols.

To also verify sidecar emission behavior, run:

```bash
cargo test --test symdump_sidecar
```

This test leaves `fixture_app_test.nro` and `fixture_app_test.nro.exports.txt` in `tests/fixture_app/target/debug/`.

To verify host-prefix propagation for a dependency export and sidecar output, run:

```bash
cargo test --test host_prefix_propagation
```

This test builds `tests/host_app` (which depends on `tests/dep_lib`) and leaves:

- `tests/host_app/target/debug/host_app_test.nro`
- `tests/host_app/target/debug/host_app_test.nro.exports.txt`
