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

- [x] `cargo xtask web [--debug] [--serve] [--port N]` builds `vxn2-wasm` +
      `vxn2-web-controller` for `wasm32-unknown-unknown` (release +
      `-C target-feature=+simd128` by default; `--debug` for a debug build).
- [x] Generates `index.html` via the `gen-web-page` bin (0157) as a subprocess.
- [x] Bakes the factory bank to `factory.bin` via a new `vxn2-engine`
      `bake-factory` bin (length-prefixed `Vxn2PresetStore` serialization —
      204 presets, ~199 KB).
- [x] Assembles `target/web-dist/`: both `.wasm`, the 9 curated transport/glue
      modules (`*.test.mjs` + preset/input modules excluded), `index.html`,
      `factory.bin`, and a generated `_headers` (COOP `same-origin` + COEP
      `require-corp` + CORP `same-origin`).
- [x] `--serve` runs `serve-coep.mjs`; verified headless: `/` returns the three
      isolation headers (⇒ `crossOriginIsolated` ⇒ SAB constructible), `.wasm`
      served as `application/wasm`, `.mjs` as `text/javascript`, `factory.bin`
      as `application/octet-stream`.
- [x] Builds under the rustup toolchain: the workspace pins 1.95.0
      (`rust-toolchain.toml`); `wasm32-unknown-unknown` target added for it.
      `build_wasm` appends `-C target-feature=+simd128` without clobbering a
      caller's RUSTFLAGS.

## Close-out (2026-07-11)

Done. `cargo run -p vxn2-xtask --release -- web` → `target/web-dist/` complete
(14 files). Added: `vxn-2/xtask/src/main.rs` `web` subcommand + `build_wasm` /
`run_capture` / `serve_dist` helpers; `vxn2-engine/src/bin/bake-factory.rs`;
`vxn-2/crates/vxn2-wasm/serve-coep.mjs`.

**`factory.bin` format** (read by the browser loader in 0159): `u32 count`, then
per preset `str name`, `str category`, `u32 blob_len` + canonical state blob —
all little-endian. It's baked now (bundle-complete) but not consumed until 0159
wires the controller's factory load.

**Browser click→audio is the one manual step left** — run
`cargo xtask web --serve` and open `http://localhost:8080/`. Everything the page
needs is served correctly (headers/MIME verified); the in-browser
gesture→sound confirmation is yours.

## Notes

Reference: `vxn-1/xtask/src/main.rs` web subcommand + `build_wasm` helper,
`serve-coep.mjs`. Depends on 0153/0154 (wasm crates) + 0157 (gen-web-page).
