---
id: "0041"
product: vxn-2
title: "xtask web: build + bundle wasm/JS/assets to dist/ (release, SIMD128)"
priority: high
created: 2026-06-15
epic: E016
depends: []
---

## Summary

A reproducible `cargo xtask web` that compiles the wasm crate(s) for
`wasm32-unknown-unknown` (release, SIMD128) and assembles a self-contained,
servable `dist/` — wasm + JS glue + worklet + faceplate assets — with one
command. The foundation every other [E016](../../epics/open/E016-web-host-shell-and-build.md)
ticket plugs into.

## Design

- **New `web` subcommand** in [xtask/src/main.rs](../../vxn-1/xtask/src/main.rs),
  alongside `bundle`. Args: `[--serve] [--debug]` (release is the default,
  mirroring how a real deploy ships).
- **Compile targets.** Per ADR 0009 the web port runs *two* wasm modules:
  the engine in the worklet (the 0034/E015 `vxn-wasm` cdylib) and the
  controller on the main thread (the real crate lands in 0044). This ticket
  builds whatever wasm crates exist today (engine) and is structured so the
  controller crate slots in without reshaping the pipeline.
- **SIMD128.** Build with `RUSTFLAGS="-C target-feature=+simd128"` (perf
  measurement is E020; the flag belongs to the pipeline). Confirm the engine
  still compiles + the harness still renders correct audio with it on.
- **Bundle layout.** Assemble `target/web-dist/` (or `dist/`): the `.wasm`
  artifact(s), the E015 `web/*.mjs` modules (event-ring, event-codec,
  param-store, audio-host, host-runner), the worklet processor, `index.html`,
  and faceplate assets (placeholder until E018). Copy, don't symlink, so the
  output is portable + servable by any static host.
- **Determinism.** Pin the toolchain expectation (rust-toolchain / documented
  rustc) and avoid a bespoke post-processor; a plain `cargo build` + file copy
  is the whole chain (no wasm-bindgen, per the 0034 finding).

## Acceptance criteria

- [ ] `cargo xtask web` produces a self-contained servable dir in one command.
- [ ] The wasm is built release with `+simd128`; the engine compiles and the
      0040 harness still renders correct audio against the SIMD build.
- [ ] The bundle contains the wasm, all E015 JS modules, the worklet, and an
      `index.html` — opening it over a COOP/COEP server boots (verified in
      0042/0045, but the assets are all present here).
- [ ] The build is reproducible from a clean `target/` with no manual copy steps.

## Notes

- Foundation ticket: no deps. 0042 (coordinator) and 0045 (serving) consume
  the dist/ output; 0044 adds the controller wasm crate to the compile set.
- Reuses the spike's manual recipe in
  [vxn-wasm/README.md](../../vxn-1/crates/vxn-wasm/README.md) (`cargo build
  --target wasm32-unknown-unknown --release` + copy) — this ticket automates it.
- Out of scope: wasm size optimisation beyond release+SIMD, CI artifact builds
  (both E020); the faceplate assets themselves (E018).

## Close-out (2026-06-15)

- **`web` subcommand** added to
  [xtask/src/main.rs](../../vxn-1/xtask/src/main.rs) (`web(release)` +
  `web_index_html()`), alongside `bundle`. `cargo xtask web [--debug]` →
  `target/web-dist/` in one command; defaults release (a deploy ships
  release+SIMD), `--debug` opts into a debug wasm. (AC1)
- **Release + SIMD128** (AC2): compiles `vxn-wasm` for
  `wasm32-unknown-unknown` with `-C target-feature=+simd128` *appended* to any
  caller `RUSTFLAGS` (not clobbered). Verified the flag actually fires —
  SIMD build carries **9297** `0xFD`-prefixed v128 bytes vs **84** in a no-SIMD
  control build (same crate, empty RUSTFLAGS). The 0040 harness renders correct
  audio against the exact SIMD artifact: `node harness-0040.mjs` → ALL CHECKS
  PASSED (it loads `target/wasm32-unknown-unknown/release/vxn_wasm.wasm`, the
  file the build produces).
- **Curated bundle** (AC3): `target/web-dist/` holds `vxn_wasm.wasm`, the five
  E015 transport modules (event-ring, event-codec, param-store, audio-host,
  host-runner), the production runner-based worklet copied to the stable name
  `vxn-processor.js`, and a generated `index.html` that reports
  `crossOriginIsolated`. Test suites, Node harnesses, and the 0034/0035 spike
  processors are excluded by an explicit copy manifest, not a glob.
- **Reproducible** (AC4): each run `remove_dir_all`s `web-dist/` then rebuilds +
  copies — no manual steps, clean from an empty `target/`. A missing source
  module is a hard error, not a silent skip.
- Out of scope as planned: the AudioContext/coordinator boot that drives these
  assets (0042) and the COOP/COEP server that serves them (0045).
