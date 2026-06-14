---
id: "0022"
product: vxn-2
title: "xtask: cross-platform bundle (Win/Linux renamed lib → .clap)"
priority: high
created: 2026-06-13
epic: E013
depends: []
---

## Summary

First ticket of [E013](../../epics/open/E013-windows-parity.md). Remove
the hard macOS-only gate in `vxn-2/xtask/src/main.rs` `bundle()` and
adopt vxn-1's cross-platform shape: macOS builds the `Contents/MacOS`
bundle dir; Windows/Linux rename the shared library to `VXN2.clap`.

The blocker today:

```rust
fn bundle() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("bundle currently only supports macOS".into());
    }
```

## Design

- Mirror `vxn-1/xtask/src/main.rs`: add a `lib_path` helper that
  switches `("", "dll")` / `("lib", "dylib")` / `("lib", "so")` on
  `target_os`, and branch `bundle()`:
  - macOS: keep the existing `.app`-shaped dir (`Contents/MacOS` +
    `Info.plist` + `PkgInfo` + `Resources` assets).
  - Windows / Linux: copy/rename the cdylib to `VXN2.clap` (a CLAP is
    just the shared lib with a `.clap` name).
- **Guard the dev-asset staging.** The `Contents/Resources` copy is
  macOS-bundle-shaped; skip it off macOS (Windows/Linux read the
  `include_str!` embed — Windows/Linux dev hot-reload is a separate
  follow-up, see E013 out-of-scope).
- **Align output path** to `target/bundled/VXN2.clap` to match vxn-1's
  CI convention (vxn-2 currently writes `target/release/`). Update
  `bundle_path`, `install`, `uninstall`, and the usage docstring so a
  local install still works.
- Keep `install`/`uninstall` working on macOS unchanged.

## Acceptance

- `cargo xtask bundle --release` produces `target/bundled/VXN2.clap` on
  macOS (bundle dir), Windows (renamed dll), and Linux (renamed so).
- macOS bundle output is byte-equivalent to today's (same Info.plist,
  PkgInfo, embedded Resources).
- `cargo xtask install` / `uninstall` still work on macOS.
