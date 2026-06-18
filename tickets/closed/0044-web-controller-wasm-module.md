---
id: "0044"
product: vxn-1
title: "Main-thread controller wasm module + JS glue (C-ABI opcode surface)"
priority: high
created: 2026-06-15
epic: E016
depends: ["0042"]
---

## Summary

Promote the throwaway `vxn-app-wasm-probe` (0036) into the real main-thread
controller wasm module and wire its JS glue. Per ADR 0009 the web port reuses
`vxn-app` + `vxn-core-app` verbatim as a main-thread wasm — one source of truth
for model mutation — rather than reimplementing the controller in JS.

## Design

- **Controller wasm crate.** A real `cdylib` (e.g. `vxn-web-controller`)
  depending on `vxn-app`, exposing the narrow C-ABI opcode surface the probe
  proved (ADR 0009 §1): post `UiEvent` in, drain `ViewEvent` out, `tick()`,
  marshalled as opcodes over the boundary — *not* Rust enums across JS. Added
  to the 0041 `xtask web` compile set (second wasm module).
- **JS glue.** A `controller.mjs` that instantiates the controller wasm, posts
  user gestures as `UiEvent` opcodes, ticks it, and drains `ViewEvent`s to the
  view layer (the faceplate bridge is E018; this ticket delivers the transport
  + a smoke view sink).
- **Shared param SAB ownership.** The controller writes param values into the
  same 0039 store SAB the worklet reads lock-free (both wasm memories map it),
  and runs the param-diff pump (port of `push_param_diffs`, ADR 0009 §2:
  `last_seen[165]` mirror, SAB scan) to echo audio-thread writes back as
  `ViewEvent::ParamChanged`.
- **Param addressing.** Read `PATCH_COUNT`/`GLOBAL_COUNT`/`TOTAL_PARAMS` from
  the wasm (not hard-coded); the 165-id layout is fixed in `vxn-app/params.rs`.
- **Delete the probe.** Per ADR 0009, remove `vxn-app-wasm-probe` and its
  workspace member line once this lands — the decision lives in the ADR.

## Acceptance criteria

- [ ] A real controller wasm crate compiles to `wasm32-unknown-unknown` reusing
      `vxn-app`/`vxn-core-app` with no controller-logic changes, and is built by
      `cargo xtask web`.
- [ ] JS posts a `UiEvent` (e.g. a param edit) → the controller mutates the
      model → the value lands in the shared param SAB → the worklet applies it.
- [ ] The param-diff pump echoes an audio-thread / automation write back as a
      `ViewEvent::ParamChanged` to the main thread.
- [ ] `vxn-app-wasm-probe` is deleted from the tree and the workspace.

## Notes

- Depends on [0042](0042-web-main-thread-coordinator.md) (the coordinator that
  hosts this second wasm + owns the shared SABs). Resolves the epic's
  conditional ticket — ADR 0009 picked controller-in-wasm over a JS rewrite.
- Out of scope: the faceplate / full UiEvent↔ViewEvent UI marshalling (E018);
  Web MIDI / keyboard input (E017); IndexedDB presets (E019).

## Close-out (2026-06-15)

- Real controller `cdylib` [vxn-web-controller](../../vxn-1/crates/vxn-web-controller/src/lib.rs)
  built for `wasm32-unknown-unknown`, depends on `vxn-app` (pulls `vxn-core-app`)
  with no controller-logic changes. C-ABI opcode surface (`vxnc_*`): UiEvent in
  (`vxnc_ui_set_param_norm`/`set_param`/`begin_gesture`/`end_gesture`/`editor_ready`
  + per-synth `set_key_mode`/`set_split_point`/`set_edit_layer`/`reset_layer`),
  `vxnc_tick`, ViewEvent drain via `vxnc_view_out_ptr`/`_len`. Counts read from
  wasm (`vxnc_total_params` = 165), never hard-coded. Verified `cargo build -p
  vxn-web-controller --target wasm32-unknown-unknown`.
- Built by `cargo xtask web` as a second wasm module: [xtask/src/main.rs](../../vxn-1/xtask/src/main.rs)
  `build_wasm()` helper compiles both `vxn-wasm` (engine) + `vxn-web-controller`,
  copies both `.wasm`s + `controller.mjs` into `target/web-dist/`. Verified the
  bundle builds both modules.
- JS glue [controller.mjs](../../vxn-1/crates/vxn-wasm/web/controller.mjs): posts
  UiEvent opcodes → controller mutates model → changed values written into the
  0039 store SAB the worklet reads lock-free. The two wasm memories don't share
  linear memory (ADR 0009 §2), so the controller holds authoritative values and
  the glue bridges them into the SAB. Asserted by `controller.test.mjs`
  (UiEvent→model→store SAB, value lands).
- Param-diff pump (port of `push_param_diffs`: NaN-seeded `last_seen[165]` +
  readback scan) echoes audio-thread/automation writes back as
  `ViewEvent::ParamChanged` with correct taper `norm` + `display`. Asserted by
  `controller.test.mjs` (exactly-one-drift, no spurious, gesture-gated).
- `vxn-app-wasm-probe` deleted — crate dir gone and workspace member line removed
  from `Cargo.toml`; `cargo metadata` resolves clean.

## Follow-up note (2026-06-15): diff-pump dormant in standalone web

The param-diff pump (`push_param_diffs` port) was built + tested per the
acceptance criteria, but it is **dormant by design in the standalone web build**.
Recorded here so E018 doesn't wire it into the hot path on reflex.

- **Why it existed (native):** the readback / diff pump caught a *second* param
  writer — a CLAP **host** automating params (and host-driven state restore) —
  that the UI had to catch up to. The NaN-seeded full broadcast on the first tick
  was exactly the host-state-restore case.
- **Why it's dormant (web):** standalone Web Audio has **no host**, so there is
  no second writer. The controller (main thread) is the single source of truth.
  vxn modulation (LFO / envelopes / mod-wheel) is applied in the audio path and
  **never writes back** into the param store, so the audio thread never
  *originates* a param change; `applyStoreToEngine` only echoes the value it just
  read. A readback scan would surface nothing but the controller's own writes.
- **Preset load is NOT a counter-example.** Recall is controller-originated: the
  controller mutates its model, then fans out two ways from main — `writeBulk`
  into the store (for the worklet) **and** 165 `ViewEvent::ParamChanged` to move
  the knobs. The UI updates from the model mutation, same path as one knob ×165;
  nothing comes back from audio. (`writeBulk` is per-slot atomic, non-
  transactional — a worklet fold mid-bulk can see one quantum of mixed old/new
  params; inaudible, identical to native `SharedParams`.)
- **Decision:** keep the readback SAB region allocated (165 i32 = 660 B,
  zero cost if never polled) and keep the pump code, but **do not run the rAF
  poll** in E018 — UI param state flows controller→view. Revive the pump only if
  a genuine audio-thread param writer is ever added (audio-side randomizer,
  MIDI-learn resolved on the audio thread, or an LFO value drawn on a knob). None
  exist today → YAGNI.
- **Scope correction for E018:** the "readback round-trip → knob updates" wiring
  is therefore *not* part of the faceplate work; E018 is that much smaller.
