---
id: "0099"
title: "vxn-2 compiles clean under MSVC on windows-latest"
priority: high
created: 2026-06-13
epic: E009
depends: []
---

## Summary

Second ticket of [E009](../../epics/open/E009-windows-parity.md). The
vxn-2 workspace has only ever been built on macOS/Linux. Get
`cargo build -p vxn2-clap` green under MSVC on `windows-latest` and fix
whatever the first Windows compile surfaces.

## Design

- This is **discovery work** — the exact breaks are unknown until the
  first MSVC build runs. Likely surface:
  - macOS-only `cfg` leakage in vxn2-* crates (the
    `cfg(target_os = "macos")` `objc` code in `vxn-core-ui-web` is
    already gated; confirm nothing in vxn2-* assumes macOS unguarded).
  - Path-separator or `include_str!`/`include_dir!` path assumptions.
  - `windows-sys` feature gaps in `vxn-core-ui-web` (the `WS_POPUP`
    popup) — confirm the feature set compiles.
- Can be developed against the 0100 CI job (push-to-branch, read the
  log) or a local Windows box / VM.
- Independent of 0098 — a clean compile and a cross-platform bundle are
  orthogonal; both feed 0100.

## Acceptance

- `cargo build --release -p vxn2-clap` succeeds on `windows-latest`.
- `cargo test` for the host-agnostic vxn2 crates passes on Windows
  where the tests are platform-independent.
- No new macOS-only `cfg` assumptions introduced; any Windows-specific
  fix is `cfg`-gated, not a regression for macOS.
