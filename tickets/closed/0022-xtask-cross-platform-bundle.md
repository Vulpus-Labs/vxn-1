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

## Close-out (2026-07-01)

- [vxn-2/xtask/src/main.rs](../../vxn-2/xtask/src/main.rs): macOS-only gate removed from `bundle()`. Added `lib_path(profile_dir: &Path)` helper switching `("", "dll")` / `("lib", "dylib")` / `("lib", "so")` on `target_os` (line 130). `bundle()` branches on `cfg!(target_os = "macos")`: macOS builds the `Contents/MacOS` dir + `Info.plist` + `PkgInfo` + Resources; Windows/Linux copies the lib to `bundle_path()`.
- Dev-asset staging (`copy_dir_recursive` into `Contents/Resources`) is macOS-only; Windows/Linux skips it (reads `include_str!` embed).
- Output aligned to `target/bundled/VXN2.clap` via `bundled_dir()` + `BUNDLE_NAME = "VXN2.clap"` (line 22); previously wrote to `target/release/vxn2.clap`. `bundle_path()` and `install_dest()` updated accordingly.
- `install()` calls `bundle(true, false)?` then copies to `~/Library/Audio/Plug-Ins/CLAP/VXN2.clap`; `uninstall()` removes same path. Both still macOS-only via `install_dest()` guard.
- Verified on macOS: `cargo xtask bundle --release` → `target/bundled/VXN2.clap/Contents/MacOS/vxn2`, `Info.plist`, `PkgInfo`, `Resources/` present.
- Also added `--universal` flag (`build_universal` for arm64+x86_64 lipo) to support release.yml (ticket 0026).
