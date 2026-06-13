---
id: "0093"
title: "Wire stack-detune / stack-spread dests (gated per-block re-cook)"
priority: medium
created: 2026-06-12
epic: E008
depends: []
---

## Summary

Fourth ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). Wire
the two stack-macro destinations, deferred because they're cooked into per-lane
offsets at note-on and per-block modulation needs a re-cook inside the audio
loop ([matrix.rs:189-191](../../crates/vxn2-engine/src/matrix.rs#L189)). Both are
**per-stack** dests; patch-global and per-stack sources are coherent (per-lane
sources are flagged by 0090).

`stack-detune` and `stack-spread` are the macros consumed at
[stack.rs:601-637](../../crates/vxn2-dsp/src/stack.rs#L601) — `stack-spread`
scales each lane's symmetric position (`cached_spread`,
[engine.rs:476-482](../../crates/vxn2-engine/src/engine.rs#L476)), `stack-detune`
scales `lane_cents` ([stack.rs:637](../../crates/vxn2-dsp/src/stack.rs#L637)).
Modulating them re-derives the per-lane detune offsets, which feeds `phase_inc`.

## Design

In the per-stack loop, after `eval_dests`, read the per-stack (lane-0)
accumulator for `StackDetune` / `StackSpread`. When either is non-zero for this
stack:

- Re-derive the per-lane detune from `base_detune_cents + detune_mod` and
  `base_spread × (1 + spread_mod)` (clamp to valid ranges), then re-cook the
  affected per-lane state. This rides the **existing block-rate pitch refresh**
  — `apply_pitch_mult` ([engine.rs:654](../../crates/vxn2-engine/src/engine.rs#L654))
  already recomputes `phase_inc` per active stack each block (6×8 powf,
  affordable at ≤16 stacks), so detune modulation folds into that recompute
  rather than adding a second pass. `stack-spread` additionally feeds the pan
  spread / voice-spread source scaling — update `cached_spread` for the block so
  the `VoiceSpread` matrix source ([engine.rs:479](../../crates/vxn2-engine/src/engine.rs#L479))
  tracks the modulated spread.
- Smoothing: these are not pitch-shaped in the zipper-sensitive sense, but
  detune feeds pitch — ramp the re-cooked detune to the new target over the
  block (reuse the pitch-smoother quantum or a per-block one-pole) to avoid a
  block-rate step in tuning. Decide and document; the cheapest acceptable
  option is the existing pitch-smooth quantum applied to the detune offset.

**Gating:** like 0092, precompute whether any active slot targets `StackDetune`
/ `StackSpread`. When neither is targeted, skip the re-cook entirely and keep
the note-on-cooked offsets — the off-path stays bit-identical and pays no powf
it didn't already pay.

**Static-source fast path (optional):** `velocity` / `key` sources are fixed for
the life of a note, so `velocity → stack-spread` only needs the re-cook once at
note-on, not every block. If the only targeting slots use static sources, cook
once on the `fresh` branch and skip the per-block re-derive. Document whether
this optimisation ships now or is left as a follow-up.

## Acceptance criteria

- [x] `key → stack-detune` re-cooks the per-lane detune → `phase_inc` shifts
  (`matrix_key_to_stack_detune_shifts_phase_inc`); `velocity → stack-spread`
  widens the `VoiceSpread` source so a `voice-spread → op-pan` slot pans wider
  (`matrix_velocity_to_stack_spread_widens_voice_spread_source`). `mod-env →
  stack-spread` shares the identical per-stack path (dynamic source → ramped,
  see below).
- [x] Per-stack independence (`matrix_stack_detune_is_per_stack`); with no
  targeting slot the offsets + `phase_inc` are bit-identical and `detune_mod_st`
  stays 0 so `apply_pitch_mult`'s extra term is `+ 0.0` and no re-cook runs
  (`matrix_no_stack_macro_slot_is_bit_identical`).
- [x] Smoothing decision **documented + tested**: fresh notes *snap* (static
  sources — key/velocity — land immediately, zipper-free); block-to-block motion
  from a dynamic source is one-pole ramped (`STACK_MACRO_SMOOTH = 0.5`),
  verified by `matrix_stack_detune_dynamic_change_is_ramped`. The "cook-once for
  static sources" optimisation is **not** shipped — the gated per-block
  re-derive is `set_detune_mod` (8 stores) folded into the always-present
  `apply_pitch_mult`, cheap enough that source-type detection isn't worth it.
- [x] `VoiceSpread` matrix source tracks the modulated spread (the pan-width
  test above drives exactly this path).
- [~] Re-cook cost: analytically the gate makes the off-path free (bit-identity
  test proves no `phase_inc` change + `detune_mod_st == 0`); the used path adds
  one `+ 0.0`→`+ offset` per lane in the already-running `apply_pitch_mult`
  plus `set_detune_mod` (8 stores) + the one-block-latency spread store. The
  **formal criterion bench** (density-8, slot vs none) is folded into
  [0097](0097-preset-reaudit-matrix-tests.md), which owns the bench suite and
  depends on this ticket.
- [x] Stale "Not yet wired" lines for the stack dests removed (engine
  process_block comment + matrix.rs dest doc).
- [x] No RT alloc / unwrap / panic.

## Notes

This is the heaviest of the three wiring tickets — it re-cooks per-lane tuning.
The gating + static-source fast path keep the cost off the common path. Per the
epic, true per-sample stack-macro modulation is not required; block-rate
re-cook (riding the existing `apply_pitch_mult`) is the target.
