---
id: "0044"
product: vxn-2
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
