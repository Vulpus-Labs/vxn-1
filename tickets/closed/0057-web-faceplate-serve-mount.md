---
id: "0057"
product: vxn-1
title: "Serve + mount the faceplate assets in the browser page (no wry)"
priority: high
created: 2026-06-15
epic: E018
depends: ["0044"]
---

## Summary

Serve the existing vxn-1 faceplate in a plain browser page instead of a wry
WebView, reusing the native splice so the HTML/JS/CSS + param-descriptor JSON
are byte-identical to the plugin. `cargo xtask web` bundles the assembled
faceplate page into `target/web-dist/index.html` and copies the bridge module.

## Design

- **Single source of truth for the page.** The native plugin already assembles
  the faceplate via `vxn_ui_web::build_faceplate_html` (splices CSS + the four
  JS modules with ESM stripped + `__PARAMS_JSON__` / `__SUBDIVISIONS_JSON__` /
  `__PATCH_COUNT__`). The faceplate JS modules reference each other across module
  boundaries in one shared scope (dispatch.js uses `keysPanel`/`presetBar`,
  panels.js uses `paramIdByNameAtLayer`), so the splice-into-one-blob is
  load-bearing — they cannot be served as independent ESM. The web page reuses
  the *same* splice.
- **Build-time generator, xtask stays dep-free.** `vxn-ui-web` (host-only,
  wry-bound) gains a `gen-web-page` bin that prints the assembled web faceplate
  HTML to stdout. xtask `web()` runs `cargo run -p vxn-ui-web --bin gen-web-page`
  and writes the output to `dist/index.html` — no new xtask dependency, no
  duplicated JSON-shaping logic, byte-identical descriptors.
- **Web boot wiring.** The web page differs from the native page only in its
  transport head: a classic inline `<script>` installs a synchronous queuing
  `window.ipc` stub (so the faceplate's `init()` -> `ready()` opcode buffers
  before the controller is live) and a `<script type="module"
  src="./faceplate-bridge.mjs">` that boots `WebHost` + `WebController`, drains
  the queue, and runs the bridge (0058/0059). E017 plugs MIDI/keyboard in via a
  clearly-marked hook.
- **Asset copy.** xtask `web()` copies `faceplate-bridge.mjs` into dist next to
  the existing E015 modules; the MODULES copy-list edit is kept localized so the
  concurrent E017 edit to the same list merges cleanly.

## Acceptance criteria

- [ ] `cargo xtask web` emits `target/web-dist/index.html` carrying the full
      faceplate markup + inlined CSS/JS, with the param JSON spliced.
- [ ] The page's descriptor JSON is byte-identical to the plugin's
      (same `build_params_json` path).
- [ ] `faceplate-bridge.mjs` lands in dist and the page loads it as a module.
- [ ] No wry / native-webview dependency is referenced on the web path.

## Notes

- xtask `web()` is also edited by the concurrent E017 (input adapters appended
  to the MODULES copy-list). Both edits touch `web()` — flagged for merge.
- Out of scope: the bridge transport itself (0058/0059), gesture/value-popup
  parity (0060), the DOM text-input popup (0061), preset storage (E019), input
  devices (E017).

## Close-out (2026-06-15)

- **Page generator.** Added `vxn_ui_web::build_web_faceplate_html` + the
  `gen-web-page` bin ([vxn-1/crates/vxn-ui-web/src/bin/gen-web-page.rs](../../vxn-1/crates/vxn-ui-web/src/bin/gen-web-page.rs)).
  It reuses the native `build_faceplate_html` splice verbatim, then swaps the wry
  IPC head for a web boot head (queuing `window.ipc` stub + descriptor globals)
  and a `faceplate-bridge.mjs` module loader, injected around the inlined
  faceplate `<script>` by string surgery on the single `<script>` boundary.
  Param/subdivision/patch-count placeholders are spliced in the boot head too, so
  the descriptor table is byte-identical to the plugin.
- **xtask bundling.** [xtask web()](../../vxn-1/xtask/src/main.rs) now copies
  `faceplate-bridge.mjs` into the MODULES list and writes `index.html` from
  `gen_faceplate_page()` (runs `cargo run -p vxn-ui-web --bin gen-web-page` as a
  subprocess — xtask keeps zero deps, no wry pulled in). The 0042 coordinator-
  smoke page (`web_index_html`) was retired.
- **Verified.** `cargo run -p vxn1-xtask -- web` assembles `target/web-dist/`
  with the 234 KB faceplate `index.html`, `faceplate-bridge.mjs`, both wasm
  modules, and all E015 transport modules. Two new Rust tests
  (`web_page_splices_clean_and_wires_boot`,
  `web_page_params_are_byte_identical_to_native`) pass: no placeholder leaks,
  boot head + loader + text-input CSS present, faceplate markup intact, and the
  web page carries the SAME `build_params_json` output as the native page.
- **Merge note:** xtask `web()` is also edited by E017 (MODULES copy-list); the
  faceplate-bridge entry is appended as one localized list item for a clean merge.
- Browser-only: the page actually rendering + cross-origin isolation need a real
  browser via `cargo xtask web --serve` (0045).
