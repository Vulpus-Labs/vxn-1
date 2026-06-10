---
id: "0071"
title: "DSP hygiene: dedupe base-Hz + xorshift, annotate reference paths"
priority: low
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Eleventh ticket of [E006](../../epics/open/E006-review-remediation.md).
Mechanical dedup/annotation pass over `vxn2-dsp` from the review's
divergence-risk findings. No behaviour change; every item should be
provable by existing tests passing unchanged.

## Items

1. **Base-Hz computation duplicated** —
   [op.rs:113-122](../../crates/vxn2-dsp/src/op.rs#L113) and
   [stack.rs:575-584](../../crates/vxn2-dsp/src/stack.rs#L575) both
   reimplement ratio-mode → Hz (`ratio_mode` match → `midi_to_hz` →
   cents factor). Extract `fn compute_base_hz(params: &OpParams,
   key: u8) -> f32` in `op.rs`; both call it (stack applies per-lane
   detune on top).
2. **`xorshift_step` duplicated** —
   [stack.rs:288](../../crates/vxn2-dsp/src/stack.rs#L288) and
   [lfo.rs:136](../../crates/vxn2-dsp/src/lfo.rs#L136), identical
   constants, same top-24-bit extraction and non-zero seed guard.
   Single private `rng` helper module (or pub(crate) fns in one of the
   two), both call sites converge.
3. **`voice.rs` reference-path annotation** — module doc still says
   "once stacking (0005) lands … becomes the default path"; 0005
   landed long ago and `Stack` is the production path. Rewrite the doc
   as "scalar reference + bench path; production uses `stack.rs`".
   Same for the `VoiceMod` stub comment
   ([voice.rs:67-74](../../crates/vxn2-dsp/src/voice.rs#L67)) — 0008
   shipped; either delete `VoiceMod` if truly unused or state its
   actual role.
4. **`set_algo_live` doc** —
   [stack.rs:319-327](../../crates/vxn2-dsp/src/stack.rs#L319): note
   that feedback routing must be refreshed separately via
   `set_feedback_live` (the engine does both each block; the API
   contract should say so).
5. **Bhaskara polynomial third copy** —
   [reverb.rs:285-287](../../crates/vxn2-dsp/src/reverb.rs#L285):
   extract `fn fast_sine_01(p: f32) -> f32` into `sine.rs` and call it
   from the reverb LFO. Skip if the float-phase variant resists a
   clean shared signature — then just cross-reference the copies in
   comments.

## Acceptance criteria

- [ ] `cargo test --workspace` green with zero test modifications
  (pure refactor — if a test needs touching, the refactor changed
  behaviour; stop and check).
- [ ] `cargo bench --package vxn2-osc-bench` stack/op benches within
  noise (the extracted fns must inline; spot-check asm if stack
  regresses).
- [ ] Greps clean: one definition each of the base-Hz computation and
  xorshift step in `vxn2-dsp`.
- [ ] No stale "will land / forthcoming" language remains in
  `vxn2-dsp` module docs (grep for ticket numbers in comments and
  verify each referenced ticket's status matches the tense).

## Notes

Explicitly OUT (review judged not worth churn): `eg.rs`/`envelope.rs`
merge, the duplicate `EgStage`/`EnvStage4` enums, reverb's f32-phase
idiom. Leave them unless item 5 makes one trivial.
