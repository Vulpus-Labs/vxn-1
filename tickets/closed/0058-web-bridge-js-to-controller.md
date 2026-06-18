---
id: "0058"
product: vxn-1
title: "Bridge JS->controller: route faceplate opcodes into the controller wasm"
priority: high
created: 2026-06-15
epic: E018
depends: ["0057"]
---

## Summary

Replace the faceplate's wry transport (`window.ipc.postMessage(json)`) with a
path that posts the same JSON opcode shapes into the controller wasm
(`controller.mjs`), so a knob/fader edit mutates the controller model and the
new param value lands in the store SAB the worklet reads. The opcode vocabulary
stays byte-compatible with the plugin.

## Design

- **`faceplate-bridge.mjs`.** A `FaceplateBridge` class (headless-testable,
  injectable timer) that owns a `WebController` and exposes
  `handleUiOpcode(jsonString)`. It parses `{op, ...}` and routes to the matching
  controller method:
  - `set_param` -> `setParam(id, plain)`, `set_param_norm` ->
    `setParamNorm(id, norm)`, `begin_gesture`/`end_gesture` ->
    `beginGesture`/`endGesture`, `ready` -> `editorReady`.
  - `set_key_mode` -> `setKeyMode(mode)` (int), `set_split_point` ->
    `setSplitPoint(note)`, `set_edit_layer`/`reset_layer` -> map the
    `'upper'`/`'lower'` string to 0/1 then `setEditLayer`/`resetLayer`.
  - preset/folder ops (`load_factory`, `save_preset`, ...) are accepted and
    forwarded but inert against the controller's NullStore (preset storage is
    E019); they never throw.
  - `request_text_input` is handled JS-side (0061), not forwarded.
- **Store sharing.** The bridge's `WebController` is constructed with the SAME
  `ParamStore` the `WebHost` (coordinator) owns, so a controller model mutation
  mirrored into the store SAB is exactly what the worklet folds — closing the
  UI-edit -> audio path.
- **Coalesced apply.** A `handleUiOpcode` marks the bridge dirty and schedules a
  single tick (0059 owns the ~60 Hz coalescing); the model mutation + SAB mirror
  happen on that tick, not per opcode.
- **Boot queue.** The page's synchronous `window.ipc` stub buffers opcodes
  emitted before the controller is live (the faceplate's `init()` fires `ready`
  during parse); the bridge drains the queue on boot.

## Acceptance criteria

- [ ] A `set_param_norm` opcode posted through the bridge mutates the controller
      model and the resulting plain value lands in the shared store SAB.
- [ ] A `set_edit_layer` `'lower'` opcode is mapped to layer 1 and reaches the
      controller (an `EditLayerChanged` ViewEvent results).
- [ ] The opcode JSON shapes are unchanged from the plugin's vocabulary.
- [ ] Preset opcodes route without throwing (inert under NullStore).

## Notes

- Depends on [0057](0057-web-faceplate-serve-mount.md) for the page that loads
  the bridge module.
- Out of scope: the controller->JS dispatch (0059); gesture/value popups (0060);
  text input (0061).

## Close-out (2026-06-15)

- **Bridge.** [faceplate-bridge.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs)
  `FaceplateBridge.handleUiOpcode(json)` parses the exact `{op,..}` shapes
  bridge.js `_post` builds and routes to the `WebController` C-ABI surface:
  set_param / set_param_norm / begin_gesture / end_gesture / ready ->
  setParam/setParamNorm/begin/end/editorReady; set_key_mode (int) /
  set_split_point / set_edit_layer / reset_layer (with `'upper'`/`'lower'` ->
  0/1 via `layerCode`). Preset/folder ops are accepted but inert (NullStore,
  E019); `request_text_input` is handled in JS (0061), not forwarded; malformed /
  unknown opcodes drop silently (matches the native parser returning None).
- **Store sharing.** `bootFaceplate` constructs the `WebController` with
  `store: host.store` (the coordinator's shared param SAB), so a controller model
  mutation mirrored into the SAB is exactly what the worklet folds. Added
  `WebController.remirrorStore()` so the coordinator's `start()`-time `writeBulk`
  of engine defaults can't clobber a controller value mirrored on the unlock
  gesture (controller is the single source of truth, ADR 0009).
- **Coalesced apply.** `handleUiOpcode` marks dirty + schedules a tick; the model
  mutation + SAB mirror happen on the tick (0059 owns the loop).
- **Boot queue.** The page's synchronous `window.ipc` stub buffers opcodes the
  faceplate emits during parse (its `init()` -> `ready`); `bootFaceplate` drains
  the queue once the controller is live.
- **Verified headlessly.** [faceplate-bridge.test.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.test.mjs)
  asserts: set_param_norm -> model -> store SAB (finite value lands);
  set_edit_layer 'lower' -> EditLayerChanged 'lower'; set_key_mode 1 ->
  key_mode_changed mode 1; preset/malformed/unknown opcodes don't throw. The test
  drives the SAME module + real controller wasm the browser runs.
- **Test runner caveat:** the `.test.mjs` node harness (mirrors
  `controller.test.mjs`) must be run with `node` manually — it is not in the
  cargo/CI path and `node` execution was unavailable in the authoring sandbox, so
  it has not been executed here. Build it first: `cargo build -p
  vxn-web-controller --target wasm32-unknown-unknown`, then `node
  web/faceplate-bridge.test.mjs`.
