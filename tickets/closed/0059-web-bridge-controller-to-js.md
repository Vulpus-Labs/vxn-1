---
id: "0059"
product: vxn-1
title: "Bridge controller->JS: drain ViewEvents to the page dispatcher (coalesced)"
priority: high
created: 2026-06-15
epic: E018
depends: ["0058"]
---

## Summary

Replace the wry `evaluate_script(applyViewEvents(...))` batch with a path that
drains `ViewEvent`s from the controller (`vxnc_view_out_*` via `controller.mjs`)
and calls the page dispatcher (`window.__vxn.applyViewEvents`) directly.
Preserve the plugin's ~60 Hz coalescing and dedupe-ParamChanged-by-id so
automation/preset-load fan-outs can't thrash the DOM. Fed by the controller's
OWN model mutations — NOT the diff readback (dormant in standalone web, 0044).

## Design

- **Tick loop.** `FaceplateBridge.start()` runs a rAF loop (injectable in
  tests). Each frame, if dirty (a UiEvent arrived since the last tick) or always
  at most ~60 Hz, it calls `controller.tick()`, which mutates the model and
  returns the decoded ViewEvent objects.
- **Translate.** The controller's C-ABI drain yields `{type:"ParamChanged"|...}`
  (PascalCase); the faceplate dispatcher consumes `{kind:"param_changed"|...}`.
  Translate per event:
  - `ParamChanged` -> `{kind:"param_changed", id, plain, norm, display}`
  - `KeyModeChanged` -> `{kind:"key_mode_changed", mode}` (int)
  - `SplitPointChanged` -> `{kind:"split_point_changed", note}`
  - `EditLayerChanged` -> `{kind:"edit_layer_changed", layer}` (int -> 'upper'/'lower')
- **Dedupe.** ParamChanged are deduped by id within a batch (latest value wins,
  position of the last occurrence preserved relative to non-ParamChanged), the
  same rule as the native `dedup_param_changes`.
- **Dispatch.** The deduped+translated batch is handed to
  `window.__vxn.applyViewEvents(arr)`. The faceplate's own `init()` swaps that
  function for the real dispatcher; before then bridge.js buffers — so events
  emitted by the first `editorReady` broadcast are not lost.
- **Readback dormant.** No rAF diff-readback pump is wired (0044): standalone web
  has no audio-thread param writer; `pumpReadback` stays callable but unused.

## Acceptance criteria

- [ ] A controller `ParamChanged` (from a model mutation) reaches a fake
      `window.__vxn.applyViewEvents` as a `param_changed` event with id/plain/
      norm/display.
- [ ] Multiple ParamChanged for one id within a tick collapse to one dispatched
      event (dedupe by id).
- [ ] An `EditLayerChanged` is translated to `{kind:'edit_layer_changed',
      layer:'lower'}` for layer 1.
- [ ] No diff-readback poll runs (readback SAB allocated-but-unpolled).

## Notes

- Depends on [0058](0058-web-bridge-js-to-controller.md).
- The ~60 Hz coalescing + dedupe mirror the native timer-tick batch
  (`vxn_core_ui_web::batch_chunks` / `dedup_param_changes`).

## Close-out (2026-06-15)

- **Tick loop.** [faceplate-bridge.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs)
  `FaceplateBridge.start()` runs a free rAF loop (injectable `scheduleFrame`);
  each frame `tick()` calls `controller.tick()` (mutates model, mirrors store SAB)
  and gets the decoded ViewEvents. Coalescing: many opcodes between frames
  collapse to one tick + one dispatch batch, so an automation sweep / fast drag
  can't thrash the DOM.
- **Translate.** `viewEventToFaceplate` maps the controller's PascalCase
  `{type,..}` to the faceplate's snake_case `{kind,..}`: ParamChanged ->
  param_changed; KeyModeChanged -> key_mode_changed (int mode); SplitPointChanged
  -> split_point_changed; EditLayerChanged -> edit_layer_changed (int ->
  'upper'/'lower'). Byte-compatible with the native `view_event_to_json` shapes.
- **Dedupe.** `dedupParamChanged` collapses ParamChanged by id (latest value
  wins, last-occurrence position kept) — the same rule as the native
  `dedup_param_changes`.
- **Dispatch.** The deduped+translated batch goes to
  `window.__vxn.applyViewEvents` directly (no evaluate_script). The faceplate's
  own `init()` swaps that fn for the real dispatcher; bridge.js buffers before
  then, so the first `editorReady` broadcast is not lost.
- **Readback dormant (0044).** No rAF diff-readback pump is wired;
  `pumpReadback` stays callable but unused.
- **Verified headlessly.** [faceplate-bridge.test.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.test.mjs):
  a controller ParamChanged reaches a fake `window.__vxn.applyViewEvents` as a
  param_changed with id/plain/norm/display; a begin/setParamNorm/end bracket
  settles to ONE deduped param_changed; EditLayerChanged layer-1 -> 'lower';
  pure-helper dedupe collapses an id to its latest value; an audio-thread
  readback write does NOT surface (pump dormant). Run manually with `node`
  (see 0058 caveat) — not exercised in the authoring sandbox.
