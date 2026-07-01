---
id: "0165"
product: vxn-2
title: Extract vxn2-dsp shared test apparatus — ADSR driver, passthrough, tail energy, patch
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

Several fixture patterns repeat across the vxn2-dsp test modules: the ADSR
lifecycle drive (tick-until-stage), the bit-exact passthrough settle loop
(copy-pasted 4×), the "drive a sine, sum tail energy" measurement, and the
`carrier_friendly_patch` helper defined identically in two modules. Extract
each into a shared `#[cfg(test)]` helper so the tested property stands out
from the setup.

Line numbers are as-reviewed on 2026-07-01; re-grep by name.

## Acceptance criteria

- [ ] Add `fn run_until_stage(env, stage, max)` (ADSR lifecycle driver) and
      reuse across `eg.rs` (`attack_then_decay_then_sustain` ~456,
      `release_drops_to_l4` ~486, `exp_rate_zero...` inner closure ~377),
      `envelope.rs` (`mod_env_*`), and `stack.rs` (`mod_env_runs_through_
      adsr...`).
- [ ] Add `fn assert_bit_exact_passthrough(process_fn, n)` (the "settle
      ~0.6 s then assert `to_bits()` bit-exact over 1000 samples" loop) and
      use it in both `phaser.rs` (~494/511) and `dynamics.rs` (~301/357) —
      the loop is currently copy-pasted 4×.
- [ ] Add `fn sine_tail_energy(process_fn, f)` (drive an f-Hz sine, sum
      tail energy) and collapse `filter.rs` `mode_energy` (~327) +
      `chain_energy` (~636) into it, plus `reverb.rs`'s `tail_energy` /
      `rms_with_damp` nested fns (~425/458/494/574 skeleton).
- [ ] Hoist `carrier_friendly_patch()` (algo 32, all ops `r[3]=99`) — defined
      identically in `stack.rs` (~1047) and `voice.rs` (~318) — to one
      shared test-support fn.
- [ ] Add `fn zero_cross_period(samples) -> f64` and reuse in `lfo.rs`
      period/sync tests (~449/513/753); add `fn assert_cooked_hz(params,
      key, expected_hz, tol)` for the four `op.rs` cook tests (~213/273/
      287/302).
- [ ] Optional: `render_peak(stack, n)` / `sustained(stack, level)` for the
      repeated "note_on + force sustain + tick N + measure L/R" in `stack.rs`
      (~1155/1180/1322/1633).

- [ ] `cargo test -p vxn2-dsp` green; assertions unchanged, tolerances
      preserved (do not silently loosen the mismatched S&H thresholds — if
      `lfo1` uses `3..8` and `lfo2` uses `>5`, align them deliberately or
      leave a comment on why they differ).

## Notes

DSP numeric-property tests are legitimately the good pattern here — this
ticket only removes duplicated scaffolding, it does not weaken any numeric
check. Non-cogent trims in these same files (`fresh_stack_is_idle`,
`resolve_route_clamps`, the dead `want` in `bend_scales_all_lane_
increments`) are 0161; redundant pairs (`sine` landmarks, `op` feedback,
`delay` DC, `filter` self-osc) are 0162.
