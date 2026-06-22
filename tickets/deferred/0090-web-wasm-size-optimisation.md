---
id: "0090"
product: vxn-2
title: "wasm size optimisation (wasm-opt, feature trim)"
priority: medium
created: 2026-06-22
epic: E020
depends: []
---

## Summary

Fourth ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md).
The web bundle ships **two** wasm modules — the engine
(`vxn_wasm.wasm`) and the main-thread controller (`vxn_web_controller.wasm`,
[xtask main.rs:143-149](../../vxn-1/xtask/src/main.rs#L143)) — built release but
otherwise unoptimised for size. This ticket shrinks them (`wasm-opt`, release
profile tuning, feature trimming) to the extent it improves load time, and folds
the step into the `cargo xtask web` pipeline so every bundle is optimised.

## Design

- **wasm-opt pass.** Add an optional `wasm-opt -Oz` (or `-O3` if size-vs-speed
  favours it after 0087) post-build step in `build_wasm`
  ([xtask main.rs:354-397](../../vxn-1/xtask/src/main.rs#L354)), gated on the
  binary being on PATH (skip-with-warning if absent, like `serve_dist`'s node
  check at [main.rs:329-342](../../vxn-1/xtask/src/main.rs#L329)). Run on both
  artifacts before they're copied into `web-dist`
  ([main.rs:174-176](../../vxn-1/xtask/src/main.rs#L174)). Must preserve `+simd128`
  (`wasm-opt` needs `--enable-simd`).
- **Release profile.** Consider `opt-level = "z"`, `lto = true`,
  `codegen-units = 1`, `panic = "abort"`, `strip = true` for the wasm builds
  (workspace `[profile.release]` or a dedicated profile). Measure size *and*
  re-run 0087 perf — `opt-level="z"` can regress the hot path, so this is a
  measured trade, not a default.
- **Feature trim.** Audit what each wasm crate pulls in. The controller deps only
  `vxn-app` by design (ADR 0009); the engine wasm pulls the full DSP. Check for
  host-only features (serde derives, std bits) reachable under
  `wasm32-unknown-unknown` that can be `cfg`-gated or default-off.
- **Measure load.** Record gzipped + raw bytes before/after for each module
  (a static host serves gzip/brotli, so gzipped is the load-time number).

## Acceptance criteria

- [ ] (headless) `cargo xtask web` runs `wasm-opt` on both modules when it is on
      PATH and emits the optimised artifacts into `web-dist`; without `wasm-opt`
      it warns and ships the plain release build (no hard failure).
- [ ] (headless) Record raw + gzipped byte sizes of both modules before and after
      optimisation in the close-out.
- [ ] (headless) `cargo test` + the node web suites still pass against the
      optimised build (no behavioural regression).
- [ ] (MANUAL / cross-ref 0087) If any aggressive size flag is adopted
      (`opt-level="z"`, `panic="abort"`), re-run the 0087 perf harness and confirm
      the hot path did not regress beyond an accepted threshold; otherwise back it
      out.

## Notes

- Cross-references 0087: any size flag that touches codegen must be perf-checked.
- `wasm-opt` ships with `binaryen`; it is a dev tool, not a runtime dep — hence
  the on-PATH gate rather than a hard requirement.
- Out of scope: CDN/compression config on the host (0092 deploy owns that).
