# symbaker

`symbaker` proc-macro crate for rewriting exported symbol names.

- `#[symbaker]` on a function sets `#[export_name = "..."]`.
- `#[symbaker_module]` on an inline module applies the same behavior to functions in that module.
- Prefix resolution is configurable and defaults to a priority chain.

Default priority:

1. `attr` (`#[symbaker(prefix = "...")]`)
2. `env_prefix` (`SYMBAKER_PREFIX`)
3. `config` (`SYMBAKER_CONFIG` TOML)
4. `top_package` (top-level package currently being built)
5. `workspace` (`[workspace.metadata.symbaker]`)
6. `package` (`[package.metadata.symbaker]`)
7. `crate` (`CARGO_PKG_NAME`)

## Macro usage

```rust
use symbaker::symbaker;

#[symbaker]
pub extern "C" fn my_export() {}

#[symbaker(prefix = "plugin_name")]
pub extern "C" fn my_export2() {}
```

`symbaker_module` filters:

```rust
use symbaker::symbaker_module;

#[symbaker_module]
mod all_exports {}

#[symbaker_module(
    include_regex = "^keep_,special$",
    exclude_glob = "*skip*",
    template = "{prefix}{sep}{module}_{name}{suffix}",
    suffix = "_v2"
)]
mod custom_rules {}
```

## Recommended one-time setup

Run from workspace/repo root:

```bash
cargo symdump init --prefix hdr
cargo symdump run skyline build
```

This creates:

- `symbaker.toml` (shared Figment config)
- `.symbaker/` (generated outputs)
- `.cargo/config.toml` entries:
  - `SYMBAKER_CONFIG=<abs path to symbaker.toml>`
  - `SYMBAKER_REQUIRE_CONFIG=1`
  - `SYMBAKER_ENFORCE_INHERIT=1`
  - `SYMBAKER_INITIALIZED=1`

`cargo symdump init` does not overwrite existing `[env]` keys in `.cargo/config.toml`; it only adds missing symbaker keys.

Verify outputs:

- `.symbaker/sym.log`
- `.symbaker/resolution.toml`
- `.symbaker/trace.log` (when trace enabled)

For team-wide deterministic behavior, commit:

- `symbaker.toml`
- `.cargo/config.toml`

Optional hard guard in downstream crates (`build.rs`):

```toml
# Cargo.toml
[build-dependencies]
symbaker-build = { git = "https://github.com/BlankMauser/symbaker", package = "symbaker-build" }
```

```rust
// build.rs
fn main() {
    symbaker_build::require_initialized();
}
```

This fails early with a setup message if the user has not run `cargo symdump init`.

## Config

Environment variables:

- `SYMBAKER_PREFIX`
- `SYMBAKER_SEP` (default: `__`)
- `SYMBAKER_PRIORITY` (comma-separated keys from priority list)
- `SYMBAKER_CONFIG` (path to TOML config file)
- `SYMBAKER_TOP_PACKAGE` (explicit top package override)
- `SYMBAKER_REQUIRE_CONFIG` (`1` => compile error if `SYMBAKER_CONFIG` missing)
- `SYMBAKER_ENFORCE_INHERIT` (`1` => dependency crates error if they fall back to local crate/package prefixes)
- `SYMBAKER_INITIALIZED` (`1` marks setup complete; missing value emits warning)
- `SYMBAKER_TRACE` (`1`/`true` enables resolver logs)
- `SYMBAKER_TRACE_FILE` (optional trace file path)
- `SYMBAKER_TRACE_HARD` (`1` => emit compile error with resolved source/prefix)

Example `symbaker.toml`:

```toml
prefix = "plugin_name"
sep = "__"
priority = ["attr", "env_prefix", "config", "top_package", "workspace", "package", "crate"]

[overrides]
# per-crate explicit prefix override
# ssbusync = "hdr"
```

## Troubleshooting and reconfiguration

Symptom: dependency prefixes appear in final exports (for example `ssbusync__*` instead of `hdr__*`).

1. Regenerate reports:

```bash
cargo symdump run skyline build
```

2. Open `.symbaker/resolution.toml` and find crates whose `selected_source` is `package`, `crate`, or `crate_fallback_after_priority`.
3. Add explicit overrides in `symbaker.toml`:

```toml
[overrides]
ssbusync = "hdr"
```

4. Build again:

```bash
cargo symdump run skyline build
```

Useful diagnostics:

- Trace log:
  - `SYMBAKER_TRACE=1`
  - `SYMBAKER_TRACE_FILE=<workspace>/.symbaker/trace.log`
- Hard fail with resolved source/prefix:
  - `SYMBAKER_TRACE_HARD=1`

Reset safely:

```bash
cargo symdump init --force
```

This rewrites `symbaker.toml` template and re-adds missing symbaker env keys.

## Cargo Symdump

Build + dump:

```bash
cargo symdump --release
```

This runs cargo build, finds the newest `.nro`, and writes:

- `<artifact>.nro.exports.txt`
- `.symbaker/sym.log` (NRO format: `address type bind size name`)
- `.symbaker/resolution.toml` (crate resolution + symbols + overrides template)
- `.symbaker/trace.log` (when tracing enabled)

Dump-only mode:

```bash
cargo symdump dump path/to/file.nro
```

Wrap arbitrary cargo subcommands with symbaker env injection:

```bash
cargo symdump run skyline build
```

`cargo symdump run` sets `SYMBAKER_TOP_PACKAGE` (if missing) and `SYMBAKER_CONFIG` (if `symbaker.toml` is found in current dir or parents), then refreshes `.symbaker/resolution.toml`.
