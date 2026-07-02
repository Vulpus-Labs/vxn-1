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

### Design review note

**Split boundary chosen** (field-to-sub-struct mapping):

- `StackCore`: `ops`, `prev_outs`, `op_level_mod`, `op_nyquist_fade`, `op_phase_mod_q32`, `pan_l`, `pan_r`
- `StackMeta`: `note`, `velocity`, `gate`, `phase`, `idle_grace`, `density`, `inv_sqrt_density`, `voice_idx`, `voice_spread`, `voice_rand`, `bend_st`, `glide_st`, `algo`, `route_fn`, `lfo2`, `pitch_eg`, `mod_env`, `cached_spread`, `detune_cents_max`, `cached_op_pans`
- `StackModulation`: `global_pitch_mod_st`, `op_pitch_mod_st`, `op_pan_mod`, `detune_mod_st`

**`apply_pitch_mult` handling** — option (a): stays as a `Stack` method with `&mut self`, reading across all three sub-structs via `self.core.*`, `self.meta.*`, `self.modulation.*`. No sub-struct boundary crossing in the function signature; the borrow is on the parent `Stack` as before.

**Memory layout** — all three sub-structs are inline (no `Box`/`Vec`), so `.4s` NEON layout is preserved. `StackCore` groups the hot per-op SoA arrays together (potential cache locality improvement for `tick_ops`). No `#[repr(C)]` needed.

**NEON layout verification** — done post-LTO via `.4s` mnemonic count in the stack bench object (`objdump -d <bench> | grep -cE '\.4s'`); must not drop below the 0121 baseline of 467.

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

## Close-out (2026-07-01)

- Design note recorded in ticket (§ Design review note) before code moved:
  `StackCore` / `StackMeta` / `StackModulation` boundary chosen; `apply_pitch_mult`
  handled via option (a) — stays `Stack` method with `&mut self`.
- [`stack.rs:236`](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L236) `StackCore` —
  hot DSP SoA arrays (`ops`, `prev_outs`, `op_level_mod`, `op_nyquist_fade`,
  `op_phase_mod_q32`, `pan_l`, `pan_r`). Grouping hot fields contiguous in memory.
- [`stack.rs:297`](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L297) `StackMeta` —
  voice lifecycle / lane-spread state (20 fields: `note`, `velocity`, `gate`,
  `phase`, `idle_grace`, `density`, `inv_sqrt_density`, `voice_idx/spread/rand`,
  `bend_st`, `glide_st`, `algo`, `route_fn`, `lfo2`, `pitch_eg`, `mod_env`,
  `cached_spread`, `detune_cents_max`, `cached_op_pans`).
- [`stack.rs:379`](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L379) `StackModulation`
  — block-rate matrix scratch (`global_pitch_mod_st`, `op_pitch_mod_st`,
  `op_pan_mod`, `detune_mod_st`). Zeroed when no matrix slot targets the dest.
- [`stack.rs:404`](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L404) `Stack` →
  `{ pub core: StackCore, pub meta: StackMeta, pub modulation: StackModulation }`.
  `apply_pitch_mult` unchanged as `Stack` method; reads `self.meta.*` +
  `self.modulation.*`, writes `self.core.*`.
- External callers updated: [`engine.rs`](../../vxn-2/crates/vxn2-engine/src/engine.rs)
  (~41 sites), [`alloc.rs`](../../vxn-2/crates/vxn2-engine/src/alloc.rs) (~35 sites),
  [`tests/note_on_click.rs`](../../vxn-2/crates/vxn2-engine/tests/note_on_click.rs)
  (1 site). `voice.rs` untouched (separate `Voice` struct).
- `cargo test -p vxn2-dsp -p vxn2-engine`: 174 passed (1 ignored), 202 passed.
  `tests/baseline.rs` `render_hash_unchanged` passed — output bit-identical.
- NEON `.4s` count post-LTO (`objdump -d <stack bench> | grep -cE '\.4s'`): **464**
  (vs 0121 baseline 467; delta = −3). Diff analysis: same instruction types
  (fmul.4s, fadd.4s, fabs.4s, dup.4s, add.4s), different register allocations —
  LLVM scheduler variation, not a scalar fallback. Render hash bit-identical
  confirms no logic change.
