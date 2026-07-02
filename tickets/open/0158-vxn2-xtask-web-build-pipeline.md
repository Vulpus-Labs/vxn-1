---
id: "0158"
product: vxn-2
title: vxn-2 cargo xtask web — build + bundle pipeline
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

A `cargo xtask web` subcommand for vxn-2 that builds both wasm crates,
generates the faceplate page, bakes the factory bank, and assembles a
self-contained `dist/` with COOP/COEP `_headers`. Mirrors vxn-1
`xtask/src/main.rs` `web` (lines ~533–631), retargeted to the vxn-2 crates.

## Acceptance criteria

- [ ] `cargo xtask web [--debug] [--serve] [--port N]` in vxn-2 builds
      `vxn2-wasm` + `vxn2-web-controller` for `wasm32-unknown-unknown`
      (release + `-C target-feature=+simd128` by default).
- [ ] Generates `index.html` via the `gen-web-page` bin (ticket 0157).
- [ ] Bakes the factory bank to `factory.bin` via a `vxn2-engine`
      `bake-factory` bin (preset store serialization).
- [ ] Assembles `dist/`: both `.wasm`, the curated JS transport/glue
      modules, `index.html`, `factory.bin`, and a generated `_headers`
      (COOP `same-origin` + COEP `require-corp`).
- [ ] `--serve` runs a COOP/COEP dev server; `SharedArrayBuffer` is
      available on the served page.
- [ ] Build works with the rustup toolchain — the subcommand forces the
      right `RUSTC`/PATH or the repo pins a `rust-toolchain.toml` (memory
      `wasm-build-toolchain`).

## Notes

Reference: `vxn-1/xtask/src/main.rs` web subcommand + `compile_wasm`
helper (~line 721), `serve-coep.mjs`. vxn-2 already has a `vxn2-xtask`
(`.cargo/config` alias). Depends on 0153/0154 (wasm crates) + 0157
(gen-web-page). Needs a `bake-factory` bin in vxn2-engine.
