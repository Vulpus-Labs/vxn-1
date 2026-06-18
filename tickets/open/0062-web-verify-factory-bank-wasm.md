---
id: "0062"
product: vxn-2
title: "Verify the embedded factory preset bank loads under wasm"
priority: medium
created: 2026-06-15
epic: E019
depends: []
---

## Summary

First ticket of [E019](../../epics/open/E019-web-persistence-presets-state.md).
The factory bank is embedded at compile time with `include_dir!`
([factory.rs:24](../../vxn-1/crates/vxn-engine/src/factory.rs#L24)) — no
filesystem, so it should compile and read under `wasm32-unknown-unknown`. The
web controller currently runs with [`NullStore`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L170),
whose `factory_len() == 0`, so the faceplate's factory list is empty. Prove the
real factory read path works in wasm and wire it in (read side only — user
presets are 0063).

## Design

- `vxn-engine`'s `EnginePresetStore` carries `std::fs` user-side code
  ([preset_io.rs](../../vxn-1/crates/vxn-engine/src/preset_io.rs)) that won't
  link under wasm. The factory read methods (`factory_len` / `factory_load` /
  `factory_meta`, [preset_io.rs:181-205](../../vxn-1/crates/vxn-engine/src/preset_io.rs#L181))
  only touch `factory()` + serde + `PluginState::write` — all wasm-safe. Split
  or gate so the factory path compiles for wasm without dragging in `fs`.
- The cleanest shape: a read-only `WasmFactoryStore` (or a `cfg`-gated
  `EnginePresetStore` whose user methods return `Unsupported` under wasm) that
  the web controller holds instead of `NullStore` for this ticket. User-side
  methods stay inert until 0063.
- Confirm `include_dir` itself builds for `wasm32-unknown-unknown` (it's a
  compile-time macro; verify the dep has no host-only features pulled in).

### Blocker found during scoping (2026-06-15): blob-format mismatch

`PresetLoad.blob` from the factory store is the **`PluginState` wire format**
([state.rs:91](../../vxn-1/crates/vxn-engine/src/state.rs#L91)):
`magic "VXN1" + version + global[GLOBAL_COUNT] + upper[PATCH_COUNT] +
lower[PATCH_COUNT] + key_mode + split`.

But the web controller's
[`WebModel::restore_from_bytes`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L125)
expects a **raw flat `f32[TOTAL_PARAMS]`** in CLAP-id order (upper, lower,
global — [params.rs:16](../../vxn-1/crates/vxn-app/src/params.rs#L16)), with no
magic/version and no key-mode/split. That placeholder format was fine under
`NullStore` (no blobs flowed). It is two ways incompatible with the factory
blob: **byte order** (global-first vs upper-first) and **framing** (no
magic/key-mode/split).

So a factory preset cannot load into `WebModel` until `WebModel`'s
snapshot/restore speak the `PluginState` format. `vxn-app` already exposes the
mapping needed to translate (`param_ref` / `patch_clap_id` / `global_clap_id`,
[params.rs:316](../../vxn-1/crates/vxn-app/src/params.rs#L316)); what it lacks is
the `PluginState` serializer itself. Two ways to close that — **decision pending
(see Notes)**:

- **(A) Re-implement the format in `vxn-web-controller`** using vxn-app's
  mapping helpers + the documented layout. Self-contained, no `vxn-engine` dep,
  but duplicates `MAGIC`/`VERSION`/layout — drift risk vs state.rs.
- **(B) Hoist the serializer into `vxn-app`/`vxn-core-app`** so native engine
  and web controller share one `PluginState` codec. Single source of truth, but
  state.rs is coupled to engine param types — needs a shared param-block shape
  or a thin re-derivation over vxn-app's index space.

This also lands the format `WebModel` will use for 0065 full-state save/restore,
so getting it right here pays forward.

### Decisions (2026-06-15)

- **Factory source: build-time baked asset.** `vxn-engine` *does* compile under
  wasm (std::fs symbols link; only error at runtime), but pulling the whole DSP
  engine into the lean main-thread controller wasm violates the ADR 0009
  intent. Instead xtask's web target pre-serializes the factory bank
  (`PluginState` blob + meta per preset) into a flat asset; the JS glue fetches
  it at boot and feeds it to the controller via a new opcode
  (`vxnc_load_factory`), the same boot-hydration shape 0064 uses for user
  presets. Controller wasm keeps depping only `vxn-app`.
- **Blob format: hoist a shared codec.** Both `SharedParams` (engine) and
  `WebModel` (controller) already impl `vxn_app::ParamModel` + `Vxn1Params`
  ([shared.rs:234](../../vxn-1/crates/vxn-engine/src/shared.rs#L234)). Put one
  `PluginState` codec in `vxn-app` over those traits, canonical order
  (magic/version, global, upper, lower, key_mode, split) byte-identical to
  [state.rs](../../vxn-1/crates/vxn-engine/src/state.rs). `WebModel` and
  `SharedParams` both delegate to it; a drift-guard test asserts the codec's
  bytes equal the legacy `PluginState::write` so existing host-state blobs and
  baked factory blobs stay readable.

### Implementation order

1. Shared codec in `vxn-app` + rewire `WebModel` and `SharedParams` + drift
   test. (Self-contained; also the 0065 format.)
2. xtask bake `factory.bin` asset from `vxn-engine::factory()`.
3. Controller `WebPresetStore` (replaces `NullStore`) + `vxnc_load_factory`
   opcode; JS fetches the asset and loads it at boot, publishes corpus.

## Acceptance criteria

- [ ] `cargo build -p vxn-web-controller --target wasm32-unknown-unknown`
      links the factory read path (no `fs` symbols).
- [ ] In the browser, the faceplate's factory list shows all embedded factory
      presets and loading one applies its params (verify the existing factory
      bank count matches the desktop build).
- [ ] User-preset opcodes remain inert/no-throw (still `NullStore`-equivalent)
      until 0063.
- [ ] A test (Rust or JS) asserts `factory_len()` > 0 under the wasm-targeted
      store path.

## Notes

- The vitest at [faceplate-bridge.test.mjs:144](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.test.mjs#L144)
  asserts preset opcodes are inert under `NullStore`; this ticket changes the
  *factory* read expectation only — update that test's framing, keep user-side
  inert.
- Memory: [[vxn1-preset-system]] (name-keyed TOML, embedded factory bank).
- **Decision pending**: blob-format approach A (re-derive in controller) vs B
  (hoist shared serializer) — see Design "Blocker found". This choice also
  fixes the format 0065 uses, so decide before coding restore.
- Out of scope: any user-preset storage (0063), the async bridge (0064).
