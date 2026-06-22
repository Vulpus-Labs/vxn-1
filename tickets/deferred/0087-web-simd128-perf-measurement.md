---
id: "0087"
product: vxn-2
title: "SIMD128 build + 16-voice worst-case perf measurement"
priority: high
created: 2026-06-22
epic: E020
depends: []
---

## Summary

First ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md), the
gate the rest of the epic measures against. The 0034 spike reported ~55×
realtime on **Node, 1 voice** — indicative, not the browser truth. This ticket
builds the perf rig that measures the **16-voice worst case in the
AudioWorklet**: a single-sourced worst-case patch driven by a Rust `Bench`
([bench.rs](../../vxn-1/crates/vxn-wasm/src/bench.rs)) under a JS harness that
brackets a batch of `vxn_bench_render` calls with `performance.now()` and posts
mean / p50 / p95 / max render-time-per-quantum. Primary baseline: **M1 desktop
Chrome**; mobile is a secondary tier (0091 owns the matrix).

The build flag itself already ships — `build_wasm`
([xtask main.rs:373-381](../../vxn-1/xtask/src/main.rs#L373)) appends
`-C target-feature=+simd128` to RUSTFLAGS. What was missing is a way to
*measure* what that flag bought at full poly, and a scalar comparison build.

## Design

- **Worst-case patch single-sourced in Rust.** `Bench` owns a 16-note,
  full-width chord in `KeyMode::Dual`. Dual fires every note on *both* layers
  ([lib.rs:326-329](../../vxn-1/crates/vxn-engine/src/lib.rs#L326)) and each
  layer is 8 channels (`CHANNELS_PER_LAYER = 8`,
  [vxn-dsp lib.rs:44](../../vxn-1/crates/vxn-dsp/src/lib.rs#L44)), so **8 distinct
  notes × 2 layers = 16 voices** — all lanes lit. The FX bus is forced on with a
  long reverb decay and high delay feedback so the tail never collapses to the
  engine's exact-silence fast path
  ([lib.rs:514 `both_silent`](../../vxn-1/crates/vxn-engine/src/lib.rs#L514)),
  keeping every quantum on the hot path. FX params are set by name via
  `vxn_app::GlobalParam::from_name` + `global_clap_id`
  ([params.rs:297,321](../../vxn-1/crates/vxn-app/src/params.rs#L297)) →
  `Synth::set_param` (which clamps to the descriptor's *plain* range, not
  normalised — [shared.rs:69-74](../../vxn-1/crates/vxn-engine/src/shared.rs#L69)),
  so e.g. `reverb_decay = 10.0 s`, `delay_feedback = 0.9`.
- **C-ABI exports mirror the 0034 raw-ABI pattern**
  ([lib.rs:42-160](../../vxn-1/crates/vxn-wasm/src/lib.rs#L42)):
  `vxn_bench_new(sample_rate) -> *mut Bench`, `vxn_bench_destroy`,
  `vxn_bench_render(n_quanta)` (batches N quanta per call — wasm32 has no
  `std::time`, so JS owns the clock), `vxn_bench_out_l` (linear-memory pointer for
  a sanity read), and `vxn_bench_simd128() -> u32` (compile-time `cfg!` flag so
  the harness can label which build it measured).
- **Measurement runs IN the worklet, not Node.** `perf-harness.mjs` boots an
  `AudioContext` + a dedicated `perf-processor.js` worklet that instantiates the
  bench wasm (raw `WebAssembly.instantiate`, no wasm-bindgen) and, inside
  `process()`, brackets `vxn_bench_render(BATCH)` with `performance.now()` —
  modelled on the EMA CPU meter already in
  [vxn-processor-0038.js:88-90](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js#L88).
  It accumulates per-quantum samples and posts `{mean, p50, p95, max}` ms.
  Reason it can't be Node: `wasm32-unknown-unknown` has no `std::time`, and only
  the worklet render thread reflects real audio-callback scheduling.
- **SIMD-vs-scalar toggle.** A `--scalar` flag on `cargo xtask web` omits the
  `+simd128` append, producing a comparison build; `vxn_bench_simd128()` lets the
  harness print which one it measured. The two builds are compared by running the
  harness against each in turn.
- **No fabricated numbers.** This ticket lands the rig and a documented manual
  run procedure; the actual Chrome figures are filled in at close-out by the
  user (see Acceptance, MANUAL).

## Acceptance criteria

- [ ] (headless) `cargo build -p vxn-wasm --target wasm32-unknown-unknown`
      exports `vxn_bench_new/_destroy/_render/_out_l/_simd128`
      (`wasm-objdump -x` or instantiation in the harness shows the symbols).
- [ ] (headless) `cargo test -p vxn-wasm` passes the `bench` unit tests:
      the worst-case patch is audible, the FX tail survives note-off (a
      post-release quantum is still non-silent), and the rendered frame count
      matches `n_quanta * QUANTUM`.
- [ ] (headless) `cargo xtask web` and `cargo xtask web --scalar` both build;
      the scalar bundle's `vxn_bench_simd128()` returns 0, the default returns 1.
- [ ] (MANUAL, M1 Chrome) Run `perf-harness.mjs` against the SIMD build and the
      scalar build; record mean / p50 / p95 / max render-ms-per-quantum for each,
      plus the realtime budget (128 / sampleRate × 1000 ms ≈ 2.67 ms @ 48 k).
      Document SIMD-vs-scalar speedup and headroom (budget ÷ p95).
- [ ] (MANUAL) Note whether p95 stays under the realtime budget at 16 voices; if
      not, flag the headroom gap for 0088 block-size tuning / 0091 mobile voice
      scaling.

## Notes

- WASM SIMD128 auto-vectorisation is weaker than NEON (epic Risks); a measured
  speedup well below the native ratio is expected, not a bug.
- The bench patch is the canonical worst case reused by 0088 (glitch stress) and
  0089 (denormal stress) — keep it single-sourced in `bench.rs`.
- Memory: `vxn1-render-loop-optimized` (native hot-path numbers, for contrast
  only — not a browser target).
- Audio-perf targets are Chrome/Firefox (desktop + Android). Safari and all iOS
  browsers (WebKit) run the faceplate only — the WASM engine is unsupported
  there by the E020 decision — so they are out of perf scope (`0091` records
  them as faceplate-only).
- Out of scope: block-size/latency tuning (0088), denormal flush (0089), wasm
  size (0090).
