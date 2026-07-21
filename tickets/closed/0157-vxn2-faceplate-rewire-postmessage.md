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

- [x] `faceplate-bridge.mjs` (vxn-2): boots the coordinator (0156) +
      controller wasm (0154) via `controller.mjs`, installs `window.ipc`, routes
      `{op,...}` opcodes → controller C-ABI, runs the rAF pump (tick → mirror
      values → decode ViewEvents → `applyViewEvents`), unlocks audio on the first
      user gesture. 8 routing tests.
- [x] `vxn2-ui-web/src/bin/gen-web-page.rs` + `build_web_faceplate_html`: emits
      a standalone `index.html` (309 KB) to stdout — same faceplate JS + spliced
      param/matrix/default-patch/subdivision JSON as the native page, plus a
      `window.ipc` boot-queue stub and the `<script type="module">` bridge boot.
      All placeholders spliced; 26 native lib tests still pass.
- [x] Faceplate gestures round-trip through the bridge: `controller.mjs`
      decodes the packed ViewEvent drain into the exact `{kind:...}` objects
      `applyViewEvents` consumes (param_changed / op_tab_changed / matrix / ks /
      eg snapshots) — a golden-byte decode test guards Rust↔JS drift.
- [x] Gesture bracketing preserved: `begin_gesture` / `end_gesture` route to
      `vxnc_ui_begin/end_gesture` → the controller's gesture bitset → CLAP-style
      brackets.
- [x] Existing vitest suite unaffected: **zero** asset files changed (only new
      `web/` transport modules + a bin + a Rust HTML-builder refactor were
      added), so the suite's behaviour is untouched by construction.

## Close-out (2026-07-10)

Done (headless). Files: `vxn-2/crates/vxn2-wasm/web/{controller.mjs,
faceplate-bridge.mjs}` + `.test.mjs` each; `vxn2-ui-web/src/bin/gen-web-page.rs`
+ a `pub build_web_faceplate_html` (the `faceplate_js_bundle` was factored out of
`build_faceplate_html` so native + web share one JS stack). Tests: 44 node
(`vxn2-wasm/web`) + 26 Rust lib = green.

**Notable wire quirk (documented in `routeOpcode`):** the faceplate's
`dispatch` merges `{op: opcode}` with the payload, so the op-indexed customs
(`set_op_tab` / `set_ks_curve` / `set_eg_curve`) arrive with a NUMBER in `op`
(the operator) and the opcode string gone. The bridge recovers intent by field
presence (unambiguous — only these three put a number in `op`, and they differ by
`side`/`curve`). Aside: the native `parse_ui_event` reads `op.as_str()` and thus
silently DROPS these three — a latent native bug the optimistic UI paint hides;
the web path handles them correctly.

**Boot ordering:** the generated page runs a classic `window.ipc` boot-queue
stub before the (classic) faceplate bundle, so `ready` + any boot dispatch is
captured; the ES-module bridge replaces `window.ipc`, drains the queue, and
starts the pump.

**Browser verification pending (your call):** the served-page click→audio check
rides 0158 (`cargo xtask web --serve`). Preset / text-input / status view events
are deferred to 0159 (`DEFERRED_OPS`).

## Notes

vxn-2 assets: `vxn2-ui-web/assets/{index.html,main.js,style.css,panels/*}`.
Native IPC today: `vxn2-ui-web/src/lib.rs` `parse_custom_ui` /
`serialise_custom_view` — reuse that opcode set on the web side. Mirror of
vxn-1 E018. Depends on 0154 + 0156.
