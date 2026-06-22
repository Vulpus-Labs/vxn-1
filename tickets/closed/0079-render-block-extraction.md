---
id: "0079"
product: vxn-1
title: vxn-engine — extract pure voice_pitches / voice_cutoff_hz from render_block
priority: high
created: 2026-06-21
epic: E024
---

## Summary

`VoiceBank::render_block` (`vxn-engine/src/voice.rs:864-
1283`) is a 420-line god-function: LFO ticking, drift,
glide, per-voice pitch/PWM/cutoff resolution, coefficient
writes, pan, the oversample render loop, three fast-path
predicates, tremolo declick, and voice freeing — all inline
against mutable `self` and DSP state.

This is the highest-value logic in the crate and the only
way to test it is end-to-end through `Synth::process`,
measuring RMS / FFT bins / zero-crossings. Those tests
exist and are thoughtful, but they are coarse oracles: they
prove "sound came out roughly right," not "voice 3's cutoff
trajectory under LFO + envelope + keytrack is correct." The
crate already extracts pure modulation helpers (`resolve_
mod`, `plan`, `block_glide`, `envelopes_static`, `lfo_rate_
from`) for exactly this reason — but the per-voice pitch
assembly (`voice.rs:958-988`, including the `match ctx.
cross_mod_type` osc-routing at `:958` and `:966`) and the
cutoff assembly with drift/trim (`:1001-1014`) are still
tangled inline and untestable in isolation.

Continue the existing extraction pattern. The render loop
and state mutation stay put; only the pure expressions over
locals move out.

## Acceptance criteria

- [ ] A pure `fn voice_pitches(...) -> (f32, f32)` (no
      `&self`, no sample-rate side effects) computes the
      s1/s2 oscillator pitches from the osc-routing match +
      detune + per-osc drift + cross-mod inputs, replacing
      the inline block at `voice.rs:958-988`.
- [ ] A pure `fn voice_cutoff_hz(...) -> f32` computes the
      filter cutoff from base + env/LFO/keytrack mod + drift
      + trim, replacing the inline block at `:1001-1014`.
- [ ] `render_block` calls the two helpers; its behaviour is
      unchanged (the render loop, coefficient writes,
      tremolo state machine, and voice freeing stay inline).
- [ ] Direct unit tests cover the osc-routing matrix (each
      `CrossModType` arm) and the cutoff assembly across
      LFO/env/keytrack combinations — assertions on the
      returned values, no buffer render required.
- [ ] `cargo test --workspace` green; `tests/baseline.rs`
      render hash unchanged (pure-extraction refactor, no
      arithmetic change).

## Notes

This is the keystone testability item: it converts several
of the coarse `lib.rs` integration oracles into precise unit
tests. Pairs naturally with 0080 (BlockCtx grouping) — the
extracted helpers' inputs should line up with the new route
sub-structs, but either can land first.

## Close-out (2026-06-22)

- Pure `voice_pitches(ctx, m, nf, detune, drift1, drift2) -> (f32, f32)`
  ([voice.rs:1402](../../vxn-1/crates/vxn-engine/src/voice.rs#L1402)) — no
  `&self`, no sample rate. Holds the osc-routing match (mod-only: Sync→osc1,
  else→osc2; sweep: Off/Ring→both, Sync→osc1, Pm→osc2) + base/note/osc-semi/
  detune/drift. Replaces the old inline block; `render_block` now calls it and
  applies `note_to_hz` to the result.
- Pure `voice_cutoff_hz(base, cutoff_mod, drift1, drift2, key_track, trim, drift_amount) -> f32`
  ([voice.rs:1440](../../vxn-1/crates/vxn-engine/src/voice.rs#L1440)) — base ×
  `fast_exp2((cutoff_mod + drift_keytrack + trim)/12)`. Replaces inline block.
- `render_block` ([voice.rs:864](../../vxn-1/crates/vxn-engine/src/voice.rs#L864))
  calls both helpers; render loop, coeff writes, tremolo state machine, voice
  freeing stay inline. Bit-exact: `tests/baseline.rs::baseline_render_is_stable`
  green, render hash unchanged.
- Direct unit tests, no buffer render: `voice_pitches_base_assembly_no_mod`,
  `voice_pitches_common_pitch_mod_hits_both_oscs`,
  `voice_pitches_mod_only_routes_per_cross_mod_mode`,
  `voice_pitches_sweep_routes_per_cross_mod_mode` (each `CrossModType` arm),
  `voice_cutoff_neutral_is_base`, `voice_cutoff_mod_is_semitone_exponential`,
  `voice_cutoff_includes_drift_keytrack`, `voice_cutoff_trim_scales_with_drift_amount`.
- `cargo test --workspace` green (69 suites ok).
