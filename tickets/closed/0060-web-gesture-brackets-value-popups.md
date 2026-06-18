---
id: "0060"
product: vxn-1
title: "Gesture brackets + value popups parity on the web bridge"
priority: medium
created: 2026-06-15
epic: E018
depends: ["0059"]
---

## Summary

Confirm gesture brackets (begin/end) and the floating value popup behave on the
web bridge exactly as in the plugin: a knob drag is `begin_gesture` ->
`set_param_norm`* -> `end_gesture`, and the value popup shows/updates/hides off
pointer events. Most of this is reuse — the faceplate already implements gesture
brackets and `valuePop` in bridge.js/panels.js; this ticket verifies they survive
the transport swap and the coalescing doesn't drop the bracket ordering.

## Design

- **Gesture ordering.** The bridge routes `begin_gesture`/`end_gesture` opcodes
  to the controller in arrival order; the ~60 Hz tick coalescing must not reorder
  a bracket relative to its `set_param` writes. Because all three opcodes are
  posted into the controller's bounded queue (FIFO) before the next `tick()`
  drains them, ordering is preserved — assert it headlessly.
- **Gesture-gated echo.** A param under an open gesture suppresses the
  controller's automation echo (native gesture-suppression rule); since the
  readback pump is dormant in web, the only echo is the controller's own
  ParamChanged from the edit, which the dragging cell already filters locally
  (MVC discipline). Verify a begin/setParamNorm/end round-trip still emits the
  ParamChanged the page needs to settle the display.
- **Value popup.** `valuePop` is DOM-only (bridge.js) and untouched by the
  transport swap; it needs a real browser to verify visually. Document that.

## Acceptance criteria

- [ ] A begin/setParamNorm/end opcode sequence reaches the controller in order
      and emits a settled ParamChanged for the param.
- [ ] The value popup show/update/hide path is unchanged from the plugin
      (DOM-only; browser-verified).

## Notes

- Depends on [0059](0059-web-bridge-controller-to-js.md).
- The popup rendering itself can only be confirmed in a real browser.

## Close-out (2026-06-15)

- **Gesture ordering.** begin/set_param(_norm)/end opcodes are posted into the
  controller's bounded queue FIFO before the next `tick()` drains them, so the
  ~60 Hz coalescing never reorders a bracket relative to its writes. The headless
  test ([faceplate-bridge.test.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.test.mjs))
  posts a begin/setParamNorm/end sequence and asserts the value lands in the
  store SAB and the bracket settles to a single deduped param_changed — the
  display the page needs to settle.
- **Gesture-gated echo.** With the readback pump dormant (0044), the only echo is
  the controller's own ParamChanged from the edit; the dragging cell filters it
  locally (MVC discipline, bridge.js/panels.js — unchanged by the transport
  swap).
- **Value popup.** `valuePop` (bridge.js) is DOM-only and untouched; its
  show/update/hide path needs a real browser to verify visually.
- Reuse ticket: no faceplate-asset changes were needed — gesture brackets and
  the value popup already exist; this confirmed they survive the transport swap.
