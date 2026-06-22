---
id: "0080"
product: vxn-1
title: vxn-engine — group BlockCtx, extract MasterFx + OutputStage, single reset
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

Two flat god-structs that keep growing, each with a
triple-init drift hazard.

`BlockCtx` (`vxn-engine/src/voice.rs:210-343`) is ~60 flat
fields covering oscillators, filter, four mod routes,
cross-mod, LFO onset, drift, pan, and layer level; `build_
ctx` (`vxn-engine/src/lib.rs:717-815`) is one 100-line
struct literal. Adding any route param means touching the
struct def, the builder, and `render_block`'s consumers with
no compiler grouping. The routes *are* grouped
(pitch/PWM/cutoff/amp) but the type doesn't say so, and the
builder reaches into `self.bend_norm`, `self.sample_rate`,
smoother values, and globals — a denormalized snapshot of
half the engine.

`Synth` (`vxn-engine/src/lib.rs:92-165`) mixes the signal
graph (banks, lfo2, phaser, chorus, delay, reverb, limiter,
two oversamplers) with fast-path bookkeeping
(`limiter_was_on`, `spread_zero_last_block`, `silent_blocks`,
`last_env`, `last_os`, `rr_layer`, `alloc_counter`). `new`,
`set_sample_rate`, and `reset` each re-list and
re-initialize all of it (`lib.rs:173-199, 216-235,
420-437`). A forgotten fast-path flag in `set_sample_rate`
or `reset` is a silent audio glitch — exactly the bug class
the comments around `spread_zero_last_block`/`silent_blocks`
describe fixing.

## Acceptance criteria

- [ ] `BlockCtx` is grouped into named sub-structs (e.g.
      `PitchRoute`, `PwmRoute`, `CutoffRoute`, `AmpRoute`,
      `OscParams`, `FilterParams`); `build_ctx` becomes a
      handful of sub-builders. The grouping matches
      `resolve_mod`'s inputs/outputs and (if 0079 landed)
      the `voice_pitches`/`voice_cutoff_hz` signatures.
- [ ] A `MasterFx` struct owns phaser/chorus/delay/reverb/
      limiter + `limiter_was_on`, with `update` /
      `process_block` / `reset`. The FX on/off edge logic
      moves out of `Synth::process` (`lib.rs:551-625`).
- [ ] An `OutputStage` (or similar) owns the
      `oversampler`/`oversampler_r` pair + `spread_zero_last
      _block` + `silent_blocks` + `last_os`, with the
      mono→stereo `clone_state_from` seeding and skip/
      zero-fill branches folded into one paired API
      (removes the duplicated L/R `.reset()`/`.decimate()`
      calls at `lib.rs:226-227, 431-433, 532-547`).
- [ ] `Synth::reset` and `Synth::set_sample_rate` delegate
      to `MasterFx::reset`/`OutputStage::reset` instead of
      re-listing each fast-path flag; the three-way init
      drift hazard is gone.
- [ ] `cargo test --workspace` green; `tests/baseline.rs`
      render hash unchanged (pure structural regrouping).

## Notes

`BlockCtx` already absorbed E019 spread and E022 drift/trim
and will keep growing — this is the time to group it.
Behaviour-preserving throughout; if the hash moves, the
refactor changed render order and is wrong.

## Close-out (2026-06-22)

- `BlockCtx` regrouped into named sub-structs
  [voice.rs:211-345](../../vxn-1/crates/vxn-engine/src/voice.rs#L211-L345):
  `OscParams`, `CrossMod`, `FilterParams`, plus one struct per `resolve_mod`
  channel — `PitchRoute` / `PwmRoute` / `CutoffRoute` / `AmpRoute`. Route fields
  dropped their redundant prefixes (`pitch_lfo_sel` → `ctx.pitch.lfo_sel`).
  `build_ctx` is now nested sub-builders
  [lib.rs:843-925](../../vxn-1/crates/vxn-engine/src/lib.rs#L843); `resolve_mod`
  /`voice_pitches`/`voice_cutoff_hz` and the 166 unit tests read the grouped
  paths. Grouping mirrors `resolve_mod`'s pitch/pwm/cutoff outputs.
- `MasterFx` [lib.rs:158](../../vxn-1/crates/vxn-engine/src/lib.rs#L158) owns
  phaser/chorus/delay/reverb/limiter + `limiter_was_on`, with
  `new`/`reset`/`update`/`process_block`. The full FX chain and the limiter
  off→on edge reset moved out of `Synth::process` into `MasterFx::process_block`.
- `OutputStage` [lib.rs:311](../../vxn-1/crates/vxn-engine/src/lib.rs#L311) owns
  the `oversampler`/`oversampler_r` pair + `spread_zero_last_block` +
  `silent_blocks` + `last_os`. The mono→stereo `clone_state_from` seed, the
  silent-drain skip and the L/R decimate/zero-fill branches fold into one
  `decimate_block` (+ `on_os_change`); the duplicated `.reset()`/`.decimate()`
  calls are gone.
- `Synth::new`/`set_sample_rate`/`reset` delegate to the `MasterFx`/`OutputStage`
  constructors + `reset` ([lib.rs:450-451](../../vxn-1/crates/vxn-engine/src/lib.rs#L450),
  [lib.rs:647-648](../../vxn-1/crates/vxn-engine/src/lib.rs#L647)) — the
  three-way init drift hazard is closed.
- `cargo test --workspace` green; `tests/baseline.rs`
  `baseline_render_is_stable` unchanged (pure structural regroup). Engine
  clippy clean apart from one pre-existing `GOLDEN` excessive-precision warning.
