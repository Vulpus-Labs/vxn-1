---
id: E018
product: vxn-2
title: "vxn-1 web port — UI faceplate (reuse opcodes, drop wry)"
status: open
created: 2026-06-14
depends-on: E015
---

> **Depends on E015** (transport + controller placement). The single
> biggest reuse win of the whole port: the faceplate is already HTML/JS
> driven by a JSON opcode protocol. This epic serves it in a plain browser
> page and swaps the wry IPC + `evaluate_script` bridge for direct DOM +
> `postMessage`/transport — keeping the opcode vocabulary intact. Detail
> firms up after 0036 decides where the controller lives.

## Goal

Run the existing vxn-1 faceplate in the browser, talking to the controller
through the same opcode vocabulary it uses today, with no wry. Parameter
edits, gestures, and value displays work; the UI sees audio-thread
automation via the E015 diff readback.

When this epic closes:

- The faceplate renders in the page and controls the synth: knobs/faders
  emit `set_param` / gesture brackets, presets load, layer controls work.
- `ViewEvent`s (param changed, preset loaded) reach the DOM and update
  displays — replacing the wry `evaluate_script` batch with a transport-
  driven dispatch.
- The opcode JSON vocabulary is unchanged from the plugin
  (`set_param`, `set_param_norm`, `begin_gesture`, `end_gesture`,
  `load_factory`/`load_user`, `ready`, and the `Vxn1UiCustom` set).

## Why this is mostly reuse

The plugin's UI is already a web app behind a message bridge. The Rust
side parses opcodes (`parse_ui_event_default`,
[vxn-core-ui-web/src/lib.rs:462-542](../../crates/vxn-core-ui-web/src/lib.rs#L462-L542))
and serialises `ViewEvent`s to JSON (`view_event_to_json`,
[lib.rs:607-656](../../crates/vxn-core-ui-web/src/lib.rs#L607-L656)); the
JS side posts via `window.ipc.postMessage` and receives via
`window.__vxn.applyViewEvents`. On the web, only the *transport* changes:
`window.ipc.postMessage` → the controller transport, and
`evaluate_script(...)` → a direct call into the page's dispatcher. The
opcode contract and the faceplate code stay.

## Scope

**In:**

- Serve the faceplate assets in the page (no wry webview).
- Bridge rewrite: JS→controller over the E015/E016 transport instead of
  wry IPC; controller→JS by calling the page dispatcher directly instead
  of `evaluate_script`. Same JSON opcode shapes.
- `ViewEvent` push path: param-changed / preset-loaded → DOM updates,
  fed by the E015 audio→main diff readback (the param-diff pump analogue).
- Gesture brackets and value popups at parity with the plugin.
- The text-input popup (rename/save) re-homed to DOM (the wry/objc/
  windows-sys floating input is desktop-only and dropped).

**Out:**

- Preset *storage* (E019) — this epic emits `load_*`/`save` opcodes; where
  presets live is E019.
- Input devices (E017).
- Visual redesign — port the faceplate as-is; restyling is not in scope.

## Planned tickets

> Ids assigned at scaffold time (after 0036). Provisional set:

- [ ] Serve + mount faceplate assets in the page.
- [ ] Bridge: JS→controller over transport (replace `window.ipc`).
- [ ] Bridge: controller→JS dispatch (replace `evaluate_script` batch),
      fed by the diff readback.
- [ ] Gesture brackets + value popups parity.
- [ ] DOM text-input popup (replace desktop floating input).

## Risks

- **Bridge timing model differs.** The plugin batches ViewEvents once per
  timer tick (~60 Hz) and dedupes ParamChanged by id; the web path should
  preserve that coalescing or risk DOM thrash under automation.
- **Controller placement ripples here.** If 0036 keeps the controller in
  Rust-wasm, the bridge marshals to wasm; if JS, the opcode parsing moves
  to JS. The faceplate is unaffected either way, but the bridge code is.
- **Asset packaging.** The faceplate's JS modules must bundle cleanly into
  E016's `dist/`.

## Acceptance

- The faceplate renders and controls the synth in the browser: param
  edits, gestures, layer controls, preset load all function.
- Audio-thread automation updates the matching UI controls (via diff
  readback).
- The JSON opcode vocabulary is byte-compatible with the plugin's.
- No wry / native webview dependency remains on the web path.
