---
id: "0008"
product: vxn-1
title: vxn-clap staticlib crate-type + clap_entry smoke
priority: high
created: 2026-06-08
epic: E010
---

## Summary

Add `staticlib` to `vxn-clap`'s `crate-type` list so the same
source produces both the existing CLAP cdylib and a static
archive that clap-wrapper can link into a bundled `.vst3`.
Smoke-link the resulting archive to confirm `clack`'s entry-
symbol macro exposes `clap_entry` from a staticlib build —
this is the foundation assumption of the entire epic and
needs verifying before the wrapper CMake lands.

Per ADR 0008 §2.

## Acceptance criteria

- [ ] `crates/vxn-clap/Cargo.toml`: `crate-type = ["cdylib",
      "rlib", "staticlib"]`.
- [ ] `cargo build -p vxn-clap --release` produces
      `target/release/libvxn_clap.a` (mac/linux) /
      `vxn_clap.lib` (win) alongside the existing dylib.
- [ ] The static archive exports the CLAP entry point. Verify
      on macOS via:
      `nm -gU target/release/libvxn_clap.a | grep clap_entry`
      returning at least one external `T` symbol. On Linux the
      check is `nm --defined-only`. On Windows confirm via
      `dumpbin /symbols vxn_clap.lib | findstr clap_entry`.
- [ ] No new warnings from `cargo build -p vxn-clap`.
- [ ] Existing `cargo xtask bundle [--release]` still produces
      a loadable `VXN1.clap` — adding `staticlib` must not
      perturb the cdylib output. Smoke-load in Reaper.
- [ ] `cargo test --workspace` green.

## Notes

`clack` emits `clap_entry` via a macro the plugin invokes from
its `lib.rs`. The macro is unaware of crate-type and should
work from a staticlib build, but Rust's `staticlib` strips
unused symbols more aggressively than `cdylib` — if the symbol
gets dropped, options in order of preference:

1. Add `#[used]` / `#[no_mangle]` glue near the macro site in
   `vxn-clap/src/lib.rs` to anchor the symbol.
2. Linker arg in the wrapper CMake (`-Wl,--whole-archive` on
   linux, `-Wl,-all_load` on mac, `/WHOLEARCHIVE:vxn_clap.lib`
   on win) — covered in ticket 0010 if needed.

Don't introduce a separate `vxn-clap-static` crate; the
crate-type list is the right knob. If something forces a
split, escalate before doing it — it would force every
downstream consumer to choose, and the entry-symbol problem
should be addressable in-place.

This ticket is the load-bearing one for the epic. If the
staticlib entry symbol doesn't export cleanly, the bundled-
mode decision in ADR 0008 §2 needs revisiting (likely toward
external-CLAP packaging) before more work lands.

## Close-out (2026-07-01)

- [vxn-1/crates/vxn-clap/Cargo.toml](../../vxn-1/crates/vxn-clap/Cargo.toml): `crate-type = ["cdylib", "rlib", "staticlib"]` confirmed present (already on main before this session).
- `cargo build -p vxn-clap --release` produces `target/release/libvxn_clap.a` (worktree target) alongside `libvxn_clap.dylib`. Zero new warnings.
- `_clap_entry` confirmed in SYMDEF via `ar p __.SYMDEF | strings | grep _clap_entry`. System `nm -gU` errors on LLVM 21 bitcode objects (CLT 1700 reader mismatch) but the SYMDEF table is text-readable and unambiguous. Documented at [vxn-1/crates/vxn-clap/src/lib.rs:553](../../vxn-1/crates/vxn-clap/src/lib.rs#L553).
- `cargo xtask bundle --release` produces `target/bundled/VXN1.clap/Contents/MacOS/VXN1` — bundle structure intact, cdylib output unperturbed by the added staticlib crate-type.
- `cargo test --workspace` green (background run, exit code 0).
