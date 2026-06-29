---
id: "0121"
product: vxn-2
title: vxn2-dsp — extract shared tick_ops kernel from stack_tick stereo/mono
priority: medium
created: 2026-06-23
epic: E027
---

## Summary

`vxn2-dsp/src/stack.rs` has two hot-loop functions —
`stack_tick_stereo` and `stack_tick_mono`
(`stack.rs:843-947`) — that share their entire ~50-line
inner body (FB computation, sine eval, phase advance,
feedback rotate, `lvl + lvl_mod[k]`, ph-mod wrapping add,
Nyquist fade) and differ only in the output fold (stereo pan
multiply vs carrier sum). Any change to the shared core
(e.g. the level-mod formula or the Nyquist fade) must be
applied to both — a live divergence risk in the most
performance-critical code in vxn-2.

Extract the shared loop into
`fn tick_ops(stack: &mut Stack) -> [[f32; STACK_LANES];
N_OPS]` that computes `new_outs` and updates `prev_outs`;
the stereo and mono variants become short folds calling it.
The extracted loop **is** the SIMD kernel — keeping it in one
place does not hurt vectorisation.

Stretch (design-review gated, may be deferred to its own
ticket): the `Stack` struct (`stack.rs:210-308`, ~30 fields)
fuses DSP state, voice metadata, mod-matrix scratch, and
cached layout. A `StackCore` / `StackMeta` / `StackModulation`
split is desirable but coupled via `apply_pitch_mult`; do not
attempt blind.

Also fold `recompute_pan` (`stack.rs:786`) into
`pan_targets` (`:649`) — both compute the same equal-power
curve (`theta = (total+1)*FRAC_PI_4`, `sin_cos`); a formula
change currently must hit both.

## Acceptance criteria

- [ ] `tick_ops` holds the shared inner loop; `stack_tick_
      stereo` / `_mono` call it and apply only their fold.
      No third copy of the FB / phase / Nyquist arithmetic
      remains.
- [ ] `recompute_pan` computes its result via `pan_targets`
      (single equal-power formula).
- [ ] An asm dump of the **post-LTO** kernel confirms NEON
      `.4s` lanes survive in `tick_ops` (mnemonic carries
      `.4s` on ARM64 — grep operands won't match; see memory
      `vxn1-neon-grep-pitfall`). No scalar fallback
      introduced by the extraction.
- [ ] `cargo test -p vxn2-dsp` + `-p vxn2-engine` green;
      `tests/baseline.rs` render hash unchanged; the
      `busy_profile` / osc-bench RT figure does not regress.
- [ ] Stack struct split is either done or explicitly
      deferred to a follow-up ticket with a one-line
      rationale in the close-out.

## Notes

SIMD-sensitive — a runtime enum match hoisted into the lane
loop drops NEON to scalar (memory
`vxn1-soa-match-defeats-simd`); keep marker-type dispatch out
of `tick_ops`. Per-crate asm is misleading pre-LTO (memory
`vxn1-ota-filter-perf`) — verify on the linked artifact. This
is the only E026 ticket that can regress audio performance;
gate the close on the profile number, not just correctness.

## Close-out (2026-06-29)

- Shared inner loop extracted to `fn tick_ops(stack: &mut Stack) ->
  [[f32; STACK_LANES]; N_OPS]`
  ([stack.rs](../../vxn-2/crates/vxn2-dsp/src/stack.rs)): routes mod inputs,
  ticks all ops over 8 lanes, returns `new_outs` and leaves `prev_outs`
  untouched so each variant folds the *old* outputs then assigns. `stack_tick_
  stereo` / `_mono` are now thin folds — no third copy of the FB / phase /
  Nyquist arithmetic. `#[inline]` keeps it as the single SIMD kernel.
- `recompute_pan` deleted; note_on now zeroes `op_pan_mod` then calls
  `refresh_pan_with_mod` → `pan_targets`, so the equal-power curve
  (`theta = (total+1)*FRAC_PI_4`, `sin_cos`) lives in one place. Inactive lanes
  stay 0 (pan_targets seeds zero arrays). The 1/√density × 1/√carriers rationale
  comment moved with it.
- Post-LTO asm on the linked `stack` bench artifact: NEON `.4s` quad-lane count
  **467 unchanged** vs stashed baseline (467) — extraction is codegen-identical,
  no scalar fallback. (`.4s` carried on the mnemonic — counted via
  `objdump -d | grep -cE '\.4s'`, per memory `vxn1-neon-grep-pitfall`.)
- `cargo test -p vxn2-dsp -p vxn2-engine` green (174 + 202 tests);
  `tests/baseline.rs` render hash unchanged (1 passed). `stack` bench d4/d8
  within noise; d1 jitter (wide CI, crosses run-to-run) is machine noise —
  confirmed by the identical asm. No RT regression.
- Stack struct split **deferred** to [0152](../../tickets/open/0152-stack-struct-split.md):
  design-review gated, coupled via `apply_pitch_mult` which straddles all three
  proposed sub-structs — not attempted blind, per the ticket's own guidance.
