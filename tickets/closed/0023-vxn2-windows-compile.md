---
id: "0023"
product: vxn-2
title: "vxn-2 compiles clean under MSVC on windows-latest"
priority: high
created: 2026-06-13
epic: E013
depends: []
---

## Summary

Second ticket of [E013](../../epics/open/E013-windows-parity.md). The
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
- Can be developed against the 0024 CI job (push-to-branch, read the
  log) or a local Windows box / VM.
- Independent of 0022 — a clean compile and a cross-platform bundle are
  orthogonal; both feed 0024.

## Acceptance

- `cargo build --release -p vxn2-clap` succeeds on `windows-latest`.
- `cargo test` for the host-agnostic vxn2 crates passes on Windows
  where the tests are platform-independent.
- No new macOS-only `cfg` assumptions introduced; any Windows-specific
  fix is `cfg`-gated, not a regression for macOS.

## Close-out (2026-07-02)

- **#1 clap builds under MSVC.** Already green before this ticket: 0024's
  [build-windows-vxn2.yml](../../.github/workflows/build-windows-vxn2.yml)
  runs `cargo xtask bundle --release` (builds `vxn2-clap`) on
  `windows-latest` and has passed on every push. No compile break
  surfaced — the platform surfaces the ticket flagged (`preset_io`
  per-OS dir, `windows-sys` popup) were already `cfg`-gated. Confirmed
  green in run 28623841042.
- **#2 host-agnostic tests pass on Windows.** Added a `cargo test` step
  to the Windows CI job for `vxn2-dsp`, `vxn2-app`, `vxn2-engine`,
  `vxn2-osc-bench`. First MSVC run surfaced two platform-dependent
  tests, both fixed:
  - `sine::tests::const_table_matches_computed`
    ([sine.rs:88](../../vxn-2/crates/vxn2-dsp/src/sine.rs#L88)) asserted
    the baked `SINE_TABLE` bit-identical to live `f32::sin`; MSVC's
    `sinf` drifts 1 ULP (`SINE_TABLE[24]` 0.14673048 vs 0.14673047).
    Relaxed bit-exact → 1-ULP tolerance (`f32::EPSILON`); a stale table
    still diverges far more, so the guard holds — now on every platform.
  - `render_hash_unchanged`
    ([baseline.rs:99](../../vxn-2/crates/vxn2-engine/tests/baseline.rs#L99))
    folds raw f32 render bits into a golden hash captured on
    macOS/aarch64 (NEON sine path + per-platform libm) — unmatchable
    bit-for-bit under MSVC. `#[cfg_attr(not(macos+aarch64), ignore)]` so
    Windows skips it; still runs+guards on the dev/macOS-CI target.
  - Result on Windows: vxn2-dsp 168 passed, vxn2-engine 202 + integration
    suites passed, render-hash ignored. Run 28623841042 green.
- **#3 no macOS-only regression.** The sine fix is portable (no `cfg`);
  the baseline gate is `cfg`-restricted to non-capture targets, leaving
  macOS behaviour untouched. Both tests still run and pass on
  macOS/aarch64 locally (`cargo test -p vxn2-dsp -p vxn2-engine`).
