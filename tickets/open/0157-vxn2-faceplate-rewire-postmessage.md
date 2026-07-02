---
id: "0157"
product: vxn-2
title: vxn-2 faceplate rewire — wry evaluate_script → postMessage bridge
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

Rewire the existing `vxn2-ui-web` faceplate assets from the native wry
`evaluate_script` IPC to a browser `postMessage` + SAB bridge, and add a
`gen-web-page` bin that emits a self-contained `index.html` (markup + CSS +
JS modules + param-descriptor JSON spliced in). The assets (index.html,
main.js, 17 panels, style.css) are already complete and modular — this is a
transport swap, not a UI rebuild. Ports `vxn-wasm/web/faceplate-bridge.mjs`
+ `controller.mjs` patterns.

## Acceptance criteria

- [ ] `faceplate-bridge.mjs` (vxn-2): boots the coordinator (0156) +
      controller wasm (0154), routes UiEvent opcodes ↔ ViewEvents using
      vxn-2's existing opcode vocabulary.
- [ ] `vxn2-ui-web/src/bin/gen-web-page.rs`: a `build_web_faceplate_html`
      path that emits standalone `index.html` to stdout (param JSON, default
      patch, subdivisions embedded — same data `bootstrap.js` injects today).
- [ ] Faceplate gestures (knob/dial/fader/op-row/mod-matrix/fx-tabs/
      preset-bar) round-trip through the bridge to the engine and back to
      the DOM — no reliance on `window.ipc.postMessage` wry stub.
- [ ] Gesture bracketing (begin/end) preserved for automation/undo parity
      with the native plugin.
- [ ] Existing vitest suite still green (assets unchanged in behaviour).

## Notes

vxn-2 assets: `vxn2-ui-web/assets/{index.html,main.js,style.css,panels/*}`.
Native IPC today: `vxn2-ui-web/src/lib.rs` `parse_custom_ui` /
`serialise_custom_view` — reuse that opcode set on the web side. Mirror of
vxn-1 E018. Depends on 0154 + 0156.
