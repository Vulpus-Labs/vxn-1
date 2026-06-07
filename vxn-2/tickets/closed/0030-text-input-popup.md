---
id: "0030"
title: Native numeric-entry popup (macOS NSTextField subclass)
priority: medium
created: 2026-06-06
closed: 2026-06-07
epic: E003
---

## Summary

Wire the macOS native text-input popup end-to-end for VXN2: JS
emits `request_text_input { id, title, initial }` →
`UiEvent::RequestTextInput` → controller forwards as
`ViewEvent::OpenTextInput` → the WebView backend intercepts and
calls `vxn_core_ui_web::prompt_text` (the existing NSWindow /
NSTextField primitive, 722 lines in core) → popup commits /
cancels → `UiEvent::TextInputResult` → `ViewEvent::TextInputResult`
→ JS dispatcher fires the pending callback keyed by `id`.

The native popup primitive itself ALREADY exists in
`vxn-core-ui-web::text_input`. This ticket's job is the end-to-end
wiring + JS pending-callback registry, NOT a reimplementation.

## Acceptance criteria

- [x] `assets/main.js` (or a new `assets/text-input.js` module)
      maintains a `pendingTextInput: Map<string, (value:
      string|null) => void>` registry. `dispatchTextInput(title,
      initial)` returns a Promise; under the hood it picks a UUID,
      stashes the resolver, dispatches `request_text_input { id,
      title, initial }`, and resolves the promise when
      `applyViewEvents` delivers `text_input_result { id,
      value }`.
- [x] Double-click on any wave-knob / fader (0026) calls
      `dispatchTextInput(desc.label, desc.display(currentValue))`;
      on commit, parses the numeric token (strips unit suffix
      same as `vxn2-clap::text_to_value`), clamps via
      `desc.clamp`, and dispatches
      `set_param { id, plain: <parsed> }` bracketed by
      `begin_gesture` / `end_gesture`.
- [x] Save As popup (0029) reuses the same primitive — no
      special-case path; the `id` field distinguishes which
      callback to fire.
- [x] Escape / focus-loss cancels (`value: None`), Return
      commits, the popup centres over the parent NSView (the
      `vxn-core-ui-web::text_input` primitive does this already;
      this ticket just verifies the path).
- [x] Manual smoke on macOS: in Bitwig (or Reaper), right-click
      a fader (or double-click — whichever the mockup
      specifies), type a value, press Return; the engine's
      audible output reflects the new value within one tick.
- [x] Windows / Linux: dispatchTextInput falls back to an
      in-page `<dialog>` element styled to look native-ish
      until the platform popup ships. The fallback ALSO fires
      `request_text_input`; the core's `prompt_text` is
      cfg-gated to no-op + immediately deliver
      `UiEvent::TextInputResult { id, value: None }` so the JS
      always sees a `text_input_result` and resolves the
      Promise. (The in-page dialog is JS-only and dispatches
      its result via the same opcode.)

## Notes

- `vxn_core_ui_web::text_input::prompt_text` is already battle-
  tested in VXN1. Reading the source is enough; do not copy /
  reimplement.
- The Promise-resolution pattern is essential: without the
  per-id callback registry the JS can't tell which control was
  edited when two popups race (e.g. preset bar Save As + a
  control popup queued from before).
- Numeric token parsing: lean on the engine — the
  `vxn2-clap::text_to_value` helper from E002 already handles
  the leading-numeric / strip-unit case. Don't duplicate parsing
  in JS beyond a simple `parseFloat` after stripping the unit
  suffix.
- Don't ship a Windows-native popup in this epic. The fallback
  `<dialog>` works for save / save-as / numeric entry on
  Windows hosts; a real popup waits for a Windows-specific
  follow-up (cfg-gated `prompt_text` on Win32 is a known
  follow-up in core).
