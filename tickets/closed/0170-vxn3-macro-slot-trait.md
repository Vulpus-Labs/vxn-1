---
id: "0170"
product: vxn-3
title: "vxn-3 macro-slot trait — generalize Knob→macros, pure engine-aware display, fix dead mappings"
priority: medium
created: 2026-07-02
epic: E032
---

## Summary

Replace the MVP `Knob` surface (`set_knob` + `enum Knob { Decay, Tone, Pitch }`)
with a declared **macro mapping** on `TrackEngine`, the foundation the fixed host
param table (0171) reinterprets per engine. `K = 3` slots per track (1:1 with
today's three knobs). Pure engine/app/UI refactor — **no clap**.

Also fixes the dead mappings 0072 called out: `Tone` is a no-op on Kick/Metal and
`Pitch` is absent from the faceplate.

Design: ADR 0003 §2 (macro slots: stable id, engine-reinterpreted value + dynamic
value-text) + ADR 0001 §4/§5.

## Design

- **Trait surface.** On `TrackEngine`
  ([track_engine.rs:26](vxn-3/crates/vxn3-engine/src/track_engine.rs#L26)) replace
  `set_knob(Knob, f32)` with:
  - `fn macro_count(&self) -> usize` — how many of the `K = 3` slots this engine
    maps (`≤ 3`).
  - `fn set_macro(&mut self, slot: usize, value: f32)` — apply a normalized
    (`0..1`) slot value to the patch; ignore out-of-range slots.
- **Pure display, not a `&self` method.** Value-text must render on the **main
  thread**, where the live engine sits on the audio thread. So the display fn is a
  **free, `EngineKind`-dispatched pure fn**, not a trait method:
  `fn macro_display(kind: EngineKind, slot: usize, value: f32, out: &mut impl Write)`
  (or returns a small stack string) — no heap, no reach into the audio-thread
  engine. Both the engine's own mapping and 0172's `value_to_text` share this one
  source of truth for slot semantics + formatting.
- **Slot → control map per engine.** Each of `kick_tone` / `metal` / `noise`
  declares which control each slot drives (its most salient ≤ 3), matching what
  `macro_display` renders. Kill the dead mappings: give Kick/Metal a real slot-1
  target (currently `Tone` is inert), and ensure `Pitch`-equivalent has a slot +
  faceplate control.
- **Command rename.** `EngineCommand::SetKnob { track, knob, value }` →
  `SetMacro { track, slot: u8, value: f32 }` in
  [io.rs:41](vxn-3/crates/vxn3-engine/src/io.rs#L41); update the queue consumer.
- **App + UI.** Update `vxn3-app` view-event → command translation and the
  `ui-web` faceplate (knob widgets emit `SetMacro`; add the missing Pitch control);
  remove `enum Knob`.

## Acceptance criteria

- [ ] `TrackEngine` exposes `macro_count` + `set_macro`; `Knob`/`set_knob` removed.
- [ ] `macro_display(kind, slot, value, …)` is a pure free fn (no `&self`, no
      alloc) and is the sole formatter of slot semantics; unit-tested per engine.
- [ ] All three engines map their `K = 3` slots to real controls — no inert slot
      (Kick/Metal slot that was `Tone`) and no missing control (Pitch on faceplate).
- [ ] `EngineCommand::SetMacro` replaces `SetKnob`; app + `ui-web` wired; the
      faceplate drives every mapped slot end-to-end.
- [ ] `cargo test -p vxn3-engine -p vxn3-app` green; no clap changes in this ticket.

## Notes

- Keep `set_macro` allocation-free and match-hoisted out of any lane loop (the
  vxn-1/vxn-2 "no enum match in the lane loop" lesson) — slots set patch scalars,
  not per-sample state.
- `macro_count < 3` is allowed; surplus host slots for such an engine render as
  a no-op / "—" in 0172. Pin the per-engine slot names in code comments so 0172's
  value-text and 0174's preset docs agree.
- Blocks 0171 (the host table calls `set_macro`) and 0172 (which calls
  `macro_display`).

## Close-out (2026-07-04)

- `TrackEngine` now declares `macro_count` + `set_macro(slot, value)`; `Knob` /
  `set_knob` removed ([track_engine.rs:47](../../vxn-3/crates/vxn3-engine/src/track_engine.rs#L47)).
- `macro_map` (shared linear coeffs) + pure `macro_display` free fn — no `&self`,
  alloc-free into a caller writer; the single source of truth for slot value +
  readout. Unit-tested `track_engine::tests::{every_engine_maps_all_slots,
  display_is_engine_aware, unit_formatting}`.
- Dead mappings fixed — all 3 engines map slot 0/1/2 to real controls: Kick
  Decay/Donk(was inert)/Depth, Metal Ring/Excite(was inert)/Body, Noise
  Decay/Mix/Bright(was inert); faceplate gained the missing Pitch knob
  ([app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js), 3 macro knobs).
- `EngineCommand::SetKnob` → `SetMacro { slot }`; rewired `track.rs` p-lock
  dispatch, `engine.rs` routing, `ui-web` `set_macro` opcode.
- `cargo test -p vxn3-engine -p vxn3-app -p vxn3-ui-web` green; no clap changes.
