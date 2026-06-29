---
id: "0149"
product: vxn-1
title: voice.rs NoteOn/Trigger param structs — kill the per-note data clump
priority: medium
created: 2026-06-28
epic: E029
---

## Summary

Four functions in `vxn-1/crates/vxn-engine/src/voice.rs` —
`note_on`, `mono_voice`, `trigger`, `mono_note_off` — thread
the same per-note parameter tuple by hand and each carries an
`#[allow(clippy::too_many_arguments)]`. The repeated tuple is
a data clump: arg-order drift is silent (transpose `velocity`
/ `detune_cents` and nothing type-errors) and adding a
per-note field means editing four signatures and every call
site. Group the cluster into two small structs; the lint
allows fall out as a side effect of the real fix.

Behaviour-preserving — the control (event-rate) path only, no
audio sample-loop code, no render change.

## The clusters

- Per-note (`note_on` / `mono_voice` / `mono_note_off`):
  `note: u8`, `velocity: f32`, `alloc_tick: u64`,
  `lfo1: Lfo1Trigger`. (`mode`, `legato`/`slide`,
  `unison_detune` vary per layer — keep as separate args.)
- Per-trigger (`trigger`): `note`, `velocity`, `alloc_tick`,
  `detune_cents: f32`, `start_phase: f32`, `lfo1`.

## Proposed shape

```rust
struct NoteOn { note: u8, velocity: f32, alloc_tick: u64, lfo1: Lfo1Trigger }
struct Trigger { note: u8, velocity: f32, alloc_tick: u64,
                 detune_cents: f32, start_phase: f32, lfo1: Lfo1Trigger }
```

`note_on(&mut self, mode, NoteOn, legato)`,
`trigger(&mut self, v, Trigger)`, etc. The `plan` loop in
`note_on` builds a `Trigger` per assignment from the `NoteOn`
plus the assignment's `detune_cents` / `start_phase`.

## Acceptance criteria

- [ ] `NoteOn` and `Trigger` structs exist; `note_on`,
      `mono_voice`, `trigger`, `mono_note_off` consume them.
- [ ] All four `#[allow(clippy::too_many_arguments)]` in
      `voice.rs` are removed and `cargo clippy -p vxn-engine`
      is clean.
- [ ] No call site copies the per-note tuple field-by-field
      more than once (struct is built once, passed down).
- [ ] `cargo test --workspace` green; render baseline
      unchanged (behaviour-preserving).

## Notes

Event-rate control path — not the audio loop, so a plain
struct is free (no SIMD/borrow concern). Pure refactor; do
not change any allocation, retrigger, or legato behaviour. The
`oscillator.rs` SIMD kernels are a separate matter (kept, see
`0151`).
