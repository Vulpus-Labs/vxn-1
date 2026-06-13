---
id: "0091"
title: "Wire lfo2-phase per-lane phase-offset dest"
priority: high
created: 2026-06-12
epic: E008
depends: []
---

## Summary

Second ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md).
`DestId::Lfo2Phase` is routable, cooked, smoothed — and then dropped:
[project_pitch_state](../../crates/vxn2-engine/src/engine.rs#L963) iterates
`PITCH_DESTS` but skips index 1 (`Lfo2Phase`), projecting only `GlobalPitch`
(index 0) and the per-op pitches (index 2+). Consume that smoothed value as a
per-lane LFO2 phase offset. This is the deferred supersaw-shimmer route the LFO2
doc already advertises ([lfo.rs:319-323](../../crates/vxn2-dsp/src/lfo.rs#L319))
but never delivered — `voice-rand → lfo2-phase` is its canonical use.

`Lfo2Phase` is a **per-lane** dest and `lfo2.phase` is already per-lane
([lfo.rs:399](../../crates/vxn2-dsp/src/lfo.rs#L399) advances `phase[k]` per
lane against a shared `inc`), so this is a clean per-lane → per-lane write — no
collapse, no ordering issue.

## Design

The smoothed per-lane `Lfo2Phase` value (in `[-1, 1]` after 0094's
normalization; one cycle = full range) is already available in
`self.pitch_smoothers[i].current()[1]` each block. Apply it as a Q32 phase
offset to the stack's `Lfo2Stack` **before** that block's `lfo2.eval`:

- Add the per-lane offset to `Lfo2Stack.phase[k]` as a wrapping Q32 add
  (`(value * U32_PER_CYCLE) as i64 as u32`), mirroring the math the test at
  [lfo.rs:644-660](../../crates/vxn2-dsp/src/lfo.rs#L644) already simulates.
- Apply it as a **delta vs the previous block's applied offset**, not an
  absolute write — otherwise a static offset would re-add every block and the
  phase would run away. Store `prev_lfo2_phase_off[i][k]` alongside the existing
  per-stack mod state and add `(new - prev)`.
- Ordering: the offset must land before `lfo2.eval` ticks the LFO this block
  ([engine.rs:459-462](../../crates/vxn2-engine/src/engine.rs#L459)). Since the
  smoother target for this block is computed from *last* block's sources, a
  one-block latency is acceptable and consistent with the other deferred dests;
  document it. Alternatively compute `dest_vals` for this stack before the LFO2
  tick — pick whichever keeps the per-stack loop single-pass and note the choice.
- On a fresh note (`fresh` branch, [engine.rs:532](../../crates/vxn2-engine/src/engine.rs#L532)):
  `reseed` already sets the per-lane phases at note-on; snap
  `prev_lfo2_phase_off[i]` to the fresh target (don't glide the offset in from
  the previous voice).

Remove the index-1 skip note in `project_pitch_state`'s doc and the "Not yet
wired: Lfo2Phase" lines at [engine.rs:437](../../crates/vxn2-engine/src/engine.rs#L437)
and [matrix.rs:186](../../crates/vxn2-engine/src/matrix.rs#L186). Fix the stale
[lfo.rs:319-323](../../crates/vxn2-dsp/src/lfo.rs#L319) comment — it currently
credits this route for shimmer that actually comes from the `stack-phase` macro
× `voice_rand` op-phase scatter ([stack.rs:665-670](../../crates/vxn2-dsp/src/stack.rs#L665)).

## Acceptance criteria

- [x] `Lfo2Phase` is consumed: a slot `voice-rand → lfo2-phase` (depth > 0)
  produces per-lane LFO2 outputs that decorrelate across the 8 lanes; with
  depth 0 or no such slot, LFO2 output is bit-identical to today.
  (`matrix_voice_rand_to_lfo2_phase_decorrelates_lanes` +
  `matrix_no_lfo2_phase_slot_keeps_lanes_phase_locked`.)
- [x] Static offset is stable — a constant `lfo2-phase` mod holds a fixed
  per-lane phase scatter across blocks (no runaway), verified by a multi-block
  engine test reading lane phases (`matrix_lfo2_phase_static_offset_does_not_run_away`).
- [x] Fresh note snaps the offset (no glide-in from the prior voice on the
  reused stack) — `prev_lfo2_phase_off` reset to 0 on the `fresh` branch;
  `matrix_lfo2_phase_fresh_note_snaps_offset`.
- [x] `lfo1` / mod-wheel etc. (coarser sources) into `lfo2-phase` also work
  (broadcast same offset to all lanes — coherent, just not decorrelated):
  `matrix_mod_wheel_to_lfo2_phase_broadcasts_equal_offset`.
- [x] Stale "not yet wired" docs removed (engine.rs process_block comment,
  `project_pitch_state` doc, matrix.rs dest doc); the lfo.rs shimmer-attribution
  comment corrected.
- [x] No RT alloc / unwrap / panic; off-path (no `lfo2-phase` slot) cost
  unchanged — guarded by `if delta != 0.0`, the smoother row stays 0 and
  `lfo2.phase` is never written.

## Notes

Cheapest of the three wiring tickets — the dest is already smoothed and the
target field is already per-lane; it's a delta-add in the existing per-stack
loop. Tier metadata (0090) isn't a hard dependency but defines that per-lane
sources are the *intended* drivers here.
