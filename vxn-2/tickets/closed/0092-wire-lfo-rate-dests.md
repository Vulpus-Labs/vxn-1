---
id: "0092"
title: "Wire lfo1-rate / lfo2-rate dests (one-block latency)"
priority: medium
created: 2026-06-12
epic: E008
depends: []
---

## Summary

Third ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). Wire
the two LFO-rate destinations, deferred originally for the rate-on-rate ordering
problem ([matrix.rs:187-188](../../crates/vxn2-engine/src/matrix.rs#L187)):
modulating a source's own rate inside the same block it's evaluated. Resolve it
with a documented **one-block latency** — the rate offset applied this block is
computed from last block's matrix accumulator.

`lfo1-rate` is a **patch-global** dest (one LFO1 in `ModBlock`,
[engine.rs:420-422](../../crates/vxn2-engine/src/engine.rs#L420)); only
patch-global sources are coherent. `lfo2-rate` is a **per-stack** dest (rate
`inc` is shared across a stack's 8 lanes, [lfo.rs:391](../../crates/vxn2-dsp/src/lfo.rs#L391));
patch-global and per-stack sources are coherent. Both apply in the **log/octave
domain** so a fixed depth is musically uniform — mirror `Cutoff`'s `2^value`
treatment ([engine.rs:871](../../crates/vxn2-engine/src/engine.rs#L871),
`DEST_GAIN[Cutoff] = 8.0`); set `DEST_GAIN[Lfo1Rate] = DEST_GAIN[Lfo2Rate]`
to an octave span (recalibrated in 0094, e.g. ±4 oct).

## Design

**`lfo1-rate` (patch-global):** LFO1 is evaluated once per block in
`patch_mod.eval_block` *before* the per-stack loop, so its rate offset must be
known before that call. Aggregate the coherent (patch-global) sources for the
`Lfo1Rate` dest from the previous block's accumulator — patch-global sources
(`lfo1`, `mod-wheel`, `aftertouch`) are lane/stack-invariant, so read any lane
of any active stack's `dest_vals`, or better, evaluate the patch-global slots
into a small pre-loop accumulator that doesn't need a stack. Apply
`rate_hz · 2^(octaves)` inside `eval_block` (or scale `mod_params.lfo1` rate
ahead of it). One-block latency: use the value cached at the end of the previous
`process_block`.

**`lfo2-rate` (per-stack):** in the per-stack loop, before
`self.alloc.stacks[i].lfo2.eval` ([engine.rs:459](../../crates/vxn2-engine/src/engine.rs#L459)),
read this stack's `Lfo2Rate` accumulator (lane 0 — per-stack dest) from last
block and pass a per-stack rate multiplier into `eval` (extend the signature or
stash a `rate_mult` on the `Lfo2Stack`). The `inc` computed at
[lfo.rs:391](../../crates/vxn2-dsp/src/lfo.rs#L391) scales by `2^(octaves)`.

**Gating:** both paths must cost nothing when unused. Track whether any active
slot targets `Lfo1Rate` / `Lfo2Rate` (precompute a per-dest "is targeted" bitset
when the matrix table changes); skip the offset math + keep the previous
bit-identical LFO tick when the bit is clear.

**Self-rate exclusion:** `lfo1 → lfo1-rate` and `lfo2 → lfo2-rate` are flagged
incoherent by 0090. They still won't *crash* (the one-block latency makes them
well-defined feedback), but the validator paints them red; no special engine
handling needed beyond the latency that already breaks the cycle.

## Acceptance criteria

- [x] `mod-wheel → lfo1-rate` sweeps LFO1 speed in the log domain (+4 oct →
  `rate_mult ≈ 16` at unity, `matrix_mod_wheel_to_lfo1_rate_sweeps_log_domain`).
  `aftertouch → lfo1-rate` shares the identical patch-global aggregation path.
  `delay-mix`/`reverb-mix` aggregation untouched (no code change there).
  Note: LFO1-rate (a patch-global dest) updates only while ≥1 voice plays — the
  accumulator is averaged across active stacks, same as the FX-mix dests.
- [x] `velocity → lfo2-rate` sweeps each voice's LFO2 speed per-stack with
  independence (`matrix_velocity_to_lfo2_rate_is_per_stack`: two velocities →
  distinct `rate_mult`). `mod-env → lfo2-rate` shares the same per-stack path.
- [x] One-block latency documented (engine + matrix module docs); the
  self-rate feedback `lfo1 → lfo1-rate` stays finite/bounded over 200 blocks
  (`matrix_lfo1_self_rate_feedback_is_bounded`).
- [x] Gated: with no slot, both `rate_mult` stay exactly 1.0 and the eval
  takes its un-modulated branch. Tests:
  `matrix_no_lfo_rate_slot_keeps_rate_mult_unity` and
  `lfo1_rate_mult_scales_rate_and_stays_bit_identical_at_unity`.
- [x] `DEST_GAIN[Lfo1Rate] = DEST_GAIN[Lfo2Rate] = 4.0` (±4 oct, coords with
  0094); `hz · 2^oct` clamps to `[LFO_RATE_HZ_MIN, LFO_RATE_HZ_MAX]` — finite,
  no denormal/NaN (clamp + extreme-mult dsp test).
- [x] Stale "Not yet wired" lines for the rate dests removed (engine.rs
  process_block comment, matrix.rs dest doc).
- [x] No RT alloc / unwrap / panic.

## Notes

The one-block latency is the cheap, correct resolution of the deferred ordering
problem and is inaudible at musical mod rates (same rationale as the pitch
smoother quantum). True per-sample rate-on-rate is explicitly out of scope for
E008.
