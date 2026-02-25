# symbaker

`symbaker` proc-macro crate meant for easily rewriting symbols. Currently just for prefixes but may get more functionality later

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

## Macro usage

```rust
use symbaker::symbaker;

#[symbaker]
pub extern "C" fn my_export() {}

#[symbaker(prefix = "plugin_name")]
pub extern "C" fn my_export2() {}
```

`symbaker_module` custom filters (simple):

```rust
use symbaker::symbaker_module;

// 1) Prefix every function in the module (default behavior)
#[symbaker_module]
mod all_exports {}

// 2) Make your own rules
#[symbaker_module(
    include_regex = "^keep_,special$",
    exclude_glob = "*skip*",
    template = "{prefix}{sep}{module}_{name}{suffix}",
    suffix = "_v2"
)]
mod custom_rules {}
```

How rules work:

- Default is all functions in the module.
- `include_regex` / `include_glob` keeps only matching functions.
- `exclude_regex` / `exclude_glob` removes matching functions.
- `template` controls final symbol name.
- `suffix` is available in template as `{suffix}`.

Template placeholders:

- `{prefix}` resolved prefix
- `{sep}` separator (default `__`)
- `{module}` module name
- `{name}` function name
- `{suffix}` optional suffix

Tip:

- Multiple regex/glob patterns are comma-separated, for example:
  - `include_glob = "init_*,main_*"`

## Config

Environment variables:

- `SYMBAKER_PREFIX`
- `SYMBAKER_SEP` (default: `__`)
- `SYMBAKER_PRIORITY` (comma-separated keys from priority list above)
- `SYMBAKER_CONFIG` (path to TOML config file)
- `SYMBAKER_TOP_PACKAGE` (explicit top package override)

Example TOML file:

```toml
prefix = "plugin_name"
sep = "__"
priority = ["attr", "env_prefix", "config", "top_package", "workspace", "package", "crate"]
```

### Cargo.toml metadata inheritance examples

Top-level app forcing one shared prefix for everything:

```toml
# app/Cargo.toml
[package]
name = "my_plugin"

[package.metadata.symbaker]
prefix = "my_plugin"
```

Workspace-level shared prefix:

```toml
# workspace Cargo.toml
[workspace.metadata.symbaker]
prefix = "mods"
```

Child dependency opting out and using its own prefix:

```toml
# dependency crate Cargo.toml
[package]
name = "child_crate"

[package.metadata.symbaker]
prefix = "child_crate"
prefer_package_prefix = true
```

Notes:

- `prefer_package_prefix = true` makes that crate ignore inherited top-level prefix and keep its own.
- Without that flag top-level prefix is used first by default.

## Cargo Symdump

Building:

```bash
cargo symdump --release
```

`cargo-symdump` automatically sets `SYMBAKER_TOP_PACKAGE` for that build when it can resolve the top package via `cargo metadata`.

This runs `cargo build --release`, finds the newest `.nro` in the target dir, and writes:

- `<artifact>.nro.exports.txt`

Dump-only mode:

```bash
cargo symdump dump path/to/file.nro
```

- Text file is emitted in the same folder (same base filename + `.exports.txt`).
