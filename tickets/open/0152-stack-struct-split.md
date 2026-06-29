---
id: "0152"
product: vxn-2
title: "vxn2-dsp — split Stack struct into StackCore/StackMeta/StackModulation"
priority: medium
created: 2026-06-29
epic: E027
depends: []
---

## Summary

Deferred stretch from [0121](../../tickets/closed/0121-stack-tick-ops-extract.md)
(Nth ticket of [E027](../../epics/open/E027-second-maintainability-sweep.md)).
The `Stack` struct ([stack.rs:~230](../../vxn-2/crates/vxn2-dsp/src/stack.rs))
fuses four concerns in one ~30-field block: DSP hot state (`ops`, `prev_outs`,
`phase`), voice metadata (`gate`, `phase`/`VoicePhase`, `density`, lane spread),
mod-matrix scratch (`op_level_mod`, `op_pan_mod`, `op_pitch_mod_st`,
`global_pitch_mod_st`, `op_phase_mod_q32`), and cached layout (`pan_l`/`pan_r`,
`cached_op_pans`, `inv_sqrt_density`). A `StackCore` / `StackMeta` /
`StackModulation` split would make the hot-path state explicit and shrink the
surface each subsystem touches.

## Design

Not obvious — the split is **coupled via
[`apply_pitch_mult`](../../vxn-2/crates/vxn2-dsp/src/stack.rs)**, which reads
matrix scratch (`global_pitch_mod_st`, `op_pitch_mod_st`, `detune_mod_st`) and
voice base state (`bend_st`, `glide_st`, `pitch_eg`) and writes DSP state
(`phase_inc`, `op_nyquist_fade`) in one pass — it straddles all three proposed
sub-structs. Do **not** attempt blind; gate on design review. Options to weigh:
(a) keep `apply_pitch_mult` as a `Stack` method taking `&mut self` over the
sub-structs as fields; (b) pass the sub-structs explicitly. SoA layout for the
hot lane loops must not regress — keep per-lane arrays contiguous so NEON `.4s`
lanes survive (see 0121 close-out: 467 `.4s` ops, must not drop).

## Acceptance criteria

- [ ] Design-review note recorded (which split boundary, how `apply_pitch_mult`
      is handled) before any code moves.
- [ ] `Stack` decomposed into the agreed sub-structs (or the split is rejected
      with a recorded rationale and this ticket closed as wontfix).
- [ ] `cargo test -p vxn2-dsp -p vxn2-engine` green; `tests/baseline.rs` render
      hash unchanged.
- [ ] Post-LTO asm: NEON `.4s` lane count in the stack kernel does not drop
      (0121 baseline = 467); `stack` bench RT figure does not regress.

## Notes

SIMD-sensitive — a runtime enum match hoisted into the lane loop drops NEON to
scalar (memory `vxn1-soa-match-defeats-simd`); per-crate asm misleading pre-LTO
(memory `vxn1-ota-filter-perf`); `.4s` carried on the mnemonic, not operands
(memory `vxn1-neon-grep-pitfall`). The `tick_ops` kernel extracted in 0121 is
the single hot loop to protect.
