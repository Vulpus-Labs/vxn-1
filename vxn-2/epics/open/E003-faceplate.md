---
id: E003
title: VXN2 HTML faceplate editor
status: open
created: 2026-06-06
---

## Goal

Ship the production editor for VXN2: a `wry`-backed HTML faceplate mounted as
a CLAP GUI child window, driven by a `Controller` layer that bridges the audio
thread's `SharedParams` (from E002) to the page over a JSON IPC bridge. The
faceplate covers every patch-level and per-layer parameter in
`PARAMETERS.md`, plus the 16-slot mod matrix, the 32-algorithm picker, and the
6 op tabs with per-op detail.

When this epic closes:

- `vxn2.clap` registers the CLAP `gui` extension and opens the HTML faceplate
  as a child of the host's parent window.
- Every CLAP-automatable parameter is reachable from the faceplate, and host-
  side automation echoes back to the controls without zipper / lag beyond a
  tick.
- The mod matrix overlay edits all 16 slots end-to-end (source / dest / depth
  / smoothing / active).
- Default patch from 0018 is the first thing the user sees — preset bar shows
  "Init", controls reflect the patch, and tweaking a control on the faceplate
  is audible immediately.
- Right-click / double-click on any control opens a native numeric-entry
  popup (macOS) that commits back via the same param event path.

Preset format + factory bank + browser modal are **out** — they're a separate
epic on top of this one (vxn-1 `E007` lineage). This epic carries only the
"Init" patch round-trip + Save/Save-As/Browse buttons as no-op stubs.

## Scope

**In:**

- `vxn2-app` crate: implement `vxn_core_app::ParamModel` for the VXN2 param
  table (380 params, mod matrix, etc.); compose `vxn_core_app::Controller`
  with a synth-specific `Custom` event handler for VXN2-only events
  (mod-matrix row edits, op-tab change, etc.). VXN2's `Layer` / `KeyMode`
  analogues — if any — ride `UiEvent::Custom` / `ViewEvent::Custom`. No
  Controller / event-loop reimplementation; the E001 epic landed the shared
  surface in `vxn-core-app`.
- `vxn2-ui-web` crate: thin HTML / CSS / JS asset bundler. Splices the
  faceplate sources into a single HTML string, passes it to
  `vxn_core_ui_web::open_editor` with a `WebEditorConfig` carrying VXN2's
  `parse_custom_ui` + `serialise_custom_view` closures. WebView lifecycle,
  IPC bridge, batched view-event sink, corpus snapshot JSON, and the
  macOS native text-input popup all come from `vxn-core-ui-web` — no
  reimplementation.
- `vxn2-clap` extensions: register the CLAP `gui` extension, mount the
  shared `vxn_core_ui_web::WebEditor` (via `open_editor`) on `gui_create`,
  run a `timer` extension that drives the UI-echo publish + `ViewEvent`
  flush at ~60 Hz, tear down on `gui_destroy`. Event dispatch + state
  save/load + gesture-bracket emit + `LocalParams` mirror come from
  `vxn-core-clap` helpers.
- HTML/CSS faceplate port from `ui-mockup/index.html`: banner, preset bar,
  op-row (algorithm + op tabs + op detail), gmod-row (LFO1, LFO2, Pitch EG,
  Mod Env), perf-row (Voice, Voice Stack, Delay, Reverb, Master), algorithm
  picker overlay, mod matrix overlay.
- Panels JS primitives: wave-knob, fader, button-group, graph (Pitch EG),
  algo-diagram SVG. Per-panel renderers populate from a `vxn.params` model
  hydrated at first batch and patched by `ViewEvent::Set`.
- Param-id ↔ section mapping shared between Rust and JS (generated table or
  build-time export), so the JS knows which param drives which control and
  the Rust knows which CLAP id corresponds to a UI gesture without a hand-
  maintained switch.
- UI-echo path: every tick, `Engine::poll_local_params` returns the last
  audio-thread parameter snapshot; the controller diffs against the last
  pushed snapshot and emits `ViewEvent::Set` for changed ids. CLAP param
  events also feed this path so host automation moves the controls.
- Native text-input popup (macOS NSWindow / NSTextField subclass) for numeric
  entry, copied in pattern from `vxn-1/crates/vxn-ui-web/src/text_input.rs`.
- xtask update: bundle the `vxn2-ui-web` asset tree into `vxn2.clap` at
  `Contents/Resources/`, so the cdylib finds them at runtime via the bundle
  path (mirrors VXN1 bundling).
- Integration smoke test: spawn the editor headlessly under `wry`'s
  `create_window_for_offscreen` (or equivalent stub), drive a synthetic
  param-change message from JS → controller → SharedParams, assert the
  audio thread sees the change.

**Out (later epics):**

- Preset format + factory bank + browser modal (`vxn2-presets`, follows the
  vxn-1 E007 lineage — ADR 0005 / 0006).
- Drag-and-drop preset operations in the browser.
- Algorithm editor (post-v1, ADR 0001 §12).
- Mod matrix condition fields (ADR 0001 §6, v2).
- MIDI learn / param learn.
- Windows / Linux text-input popup (stubs only; macOS ships first).
- MPE / per-note expression UI.
- High-DPI asset variants beyond CSS `transform: scale()`.

## Tickets

- [ ] [0022 — vxn2-app crate scaffold (Controller, ParamDesc, UiEvent / ViewEvent)](../../tickets/open/0022-vxn2-app-scaffold.md)
- [ ] [0023 — vxn2-ui-web crate scaffold (wry child WebView + IPC bridge)](../../tickets/open/0023-vxn2-ui-web-scaffold.md)
- [ ] [0024 — CLAP gui + timer extensions, editor mount / teardown](../../tickets/open/0024-clap-gui-extension.md)
- [ ] [0025 — Faceplate HTML/CSS port from mockup](../../tickets/open/0025-faceplate-html-css.md)
- [ ] [0026 — Panels JS: wave-knob / fader / button-group + section renderers](../../tickets/open/0026-panels-js.md)
- [ ] [0027 — Op row: algorithm picker overlay + op tabs + op detail](../../tickets/open/0027-op-row.md)
- [ ] [0028 — Mod matrix overlay (16 slots, source / dest / depth / smoothing)](../../tickets/open/0028-mod-matrix-overlay.md)
- [ ] [0029 — Preset bar wiring (Init round-trip; Save / SaveAs / Browse stubs)](../../tickets/open/0029-preset-bar.md)
- [ ] [0030 — Native numeric-entry popup (macOS NSTextField subclass)](../../tickets/open/0030-text-input-popup.md)
- [ ] [0031 — UI-echo: LocalParams publish + ViewEvent::Set diff loop](../../tickets/open/0031-ui-echo.md)
- [ ] [0032 — Bundle assets into vxn2.clap + end-to-end editor smoke test](../../tickets/open/0032-bundle-and-smoke.md)

## Dependency order

```text
E002 (CLAP shell, SharedParams, default patch)
  │
  ├─> 0022 (vxn2-app) ──┬─> 0024 (gui ext) ──> 0025 (HTML/CSS) ──> 0026 (panels JS) ──┬─> 0027 (op row)
  │                     │                                                              ├─> 0028 (mod matrix)
  │                     │                                                              └─> 0029 (preset bar)
  │                     │
  │                     └─> 0031 (UI-echo) ─────────────────────────────────────────────┐
  │                                                                                     │
  ├─> 0023 (vxn2-ui-web) ──> 0024                                                       │
  │                          │                                                          │
  │                          └─> 0030 (text-input popup) ─────────────────────────────┐ │
  │                                                                                   │ │
  └─> 0032 (bundle xtask + smoke) ─────────────────────────────────────────────────────┴─┘
```

- 0022 stands up the controller surface. Until it exists nothing downstream
  has a typed channel to push or receive on.
- 0023 stands up the WebView shell with a placeholder HTML that proves IPC
  in both directions. Decoupled from 0022 enough to land in parallel; merge
  point is 0024 where the gui extension hands the controller into the editor.
- 0024 wires the CLAP `gui` + `timer` extensions, opens the WebView on
  `gui_create`, and runs the per-tick flush. After this the editor opens in
  a host but shows the placeholder page.
- 0025 ports the HTML / CSS verbatim from the mockup. After this the
  faceplate *looks* right but no control is wired.
- 0026 introduces the panel primitives + per-section renderers and binds them
  to the param model hydrated over IPC. After this every static-section
  control (LFO1, LFO2, Pitch EG, Mod Env, Voice, Voice Stack, Delay, Reverb,
  Master) moves the engine.
- 0027 / 0028 / 0029 are independent; 0027 covers the op-row complexity
  (algorithm + op tabs + op detail), 0028 covers the mod matrix overlay,
  0029 covers the preset bar (Init round-trip + Save/SaveAs/Browse stubs).
- 0030 is the native popup primitive; depends on 0023 for the parent
  `*mut c_void`. Lands in parallel with the panel work.
- 0031 closes the echo loop. Can land any time after 0024; gating it after
  0026 means the visible controls reflect host automation immediately.
- 0032 bundles assets + closes the loop with a headless smoke test.

## Acceptance

- Bitwig (or another CLAP host on the dev machine) shows the VXN2 editor at
  1024×772 logical pixels with the banner, preset bar, op-row, gmod-row, and
  perf-row laid out per the mockup.
- The default patch from 0018 is reflected in every visible control on first
  open: algorithm number, op carrier / modulator badges, LFO rates, voice
  stack density, delay / reverb mix, master volume.
- Dragging any fader, clicking any button-group option, picking an algorithm
  from the overlay, editing a mod matrix slot — all produce an audible
  parameter change within one tick.
- Host-side automation (a written automation curve on any of the 174 CLAP
  ids) moves the matching faceplate control smoothly during playback.
- Right-click on any control opens a native numeric-entry popup over the
  host plugin window; pressing return commits, Escape / focus-loss cancels.
- Editor teardown on `gui_destroy` and on plugin unload runs cleanly: no
  leaked WebView, no leaked window subview, no controller-channel panic
  when the engine is torn down before the editor.
- `clack-host` smoke test: instantiate plugin, call `gui_create` + `gui_show`
  against an offscreen parent, post one synthetic IPC param-change message
  per JS opcode, assert `SharedParams` reflects the change.
- No RT allocations in audio thread, no `unwrap` / `expect` across the IPC
  parse, no panics across the FFI boundary from the timer extension or the
  WebView's IPC handler.

## Notes

- `vxn-1/crates/vxn-ui-web` is the closest structural template — copy the
  shape (WebView-child mount, `evaluate_script` batch buffer, IPC opcode set,
  text-input popup pattern) but not the panel JS verbatim. VXN2's params are
  a different set; the JS needs to be retargeted to `vxn-2/PARAMETERS.md`
  and the mod matrix sources/dests from 0008.
- `vxn-1/crates/vxn-app` is the closest template for the controller surface.
  VXN2 inherits the Whole / Layer / Split voicing model + per-layer params,
  so `Layer` + `desc_for_clap_id` translate over with the param table from
  0012 substituted in.
- The ui-mockup HTML uses placeholder values inlined into the markup; the
  port in 0025 must strip those out and let 0026 / 0027 / 0028 fill them
  from the param model so the page can drive any patch.
- Asset bundling: `vxn2-ui-web/assets/` is included via `include_bytes!`
  at build time so the editor doesn't depend on a writable on-disk asset
  path — same trick `vxn-1` uses. 0032's xtask change just unpacks for
  developer iteration; production runs from the embedded bytes.
- pin `wry` and `raw-window-handle` to the same revs as `vxn-1/crates/vxn-ui-web`
  to avoid two competing WebView versions in the same dep graph if both
  workspaces end up vendored together (deploy scripts, dev shell).
- macOS-only popup for now (objc 0.2 NSWindow + NSTextField subclass).
  Windows path is in scope for the popup file structure (so the cfg-gated
  stubs exist) but not for shipping behaviour in this epic.
- 0031 closes a path E002 stubbed at the audio thread side — the publish
  ring already exists; 0031 just adds the consumer + the diff-and-push logic
  on the controller side.
- This epic does **not** introduce a preset format. The "Save" / "Save As" /
  "Browse" buttons in the preset bar fire `UiEvent::PresetSave` etc. that
  the controller logs and discards. The preset epic on top of this one wires
  them to actual disk I/O + the browser modal.
