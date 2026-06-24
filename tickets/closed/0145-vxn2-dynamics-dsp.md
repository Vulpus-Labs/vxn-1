---
id: "0145"
product: vxn-2
title: "Dynamics FX DSP — compressor + saturator with fade-in/out enable"
priority: medium
created: 2026-06-24
epic: E028
depends: []
---

## Summary

First ticket of [E028](../../epics/open/E028-vxn2-fx-dynamics-block.md).
Add a stereo `DynamicsBlock` to `vxn2-dsp` providing a feed-forward
compressor (threshold / ratio / attack / release / makeup) followed by
a `tanh` saturator (drive), wrapped in a block-level wet/dry smoother
so `set_enabled(false)` fades the wet to 0 (no click) before the
process loop reverts to a bit-exact passthrough — matching the on/off
discipline of phaser / delay / reverb.

## Design

File: `vxn-2/crates/vxn2-dsp/src/dynamics.rs` (new module, declared in
`vxn-2/crates/vxn2-dsp/src/lib.rs:20` alongside `pub mod phaser;`).

**Internal order: comp → sat.** Channel-strip topology — compress
dynamics first, then drive into the saturator for consistent harmonic
content. Reverse order would let saturation peaks defeat the comp;
this order pairs cleanly with the downstream master limiter.

**Compressor** — feed-forward, peak detector, soft knee (~6 dB
internal default, not exposed):

```text
detector: peak = max(|l|, |r|) → one-pole follower with attack/release
          coefficients in samples (att/rel in ms × sample_rate × 1e-3).
gain:     gr_db = -max(0, peak_db - threshold_db) × (1 - 1/ratio)
          smoothed with knee at ±knee/2 around threshold.
makeup:   linear gain from makeup_db, applied post-comp pre-sat.
```

Detector and gain-reduction are linear-domain where possible (one
`log2` for `peak_db`, one `exp2` for the gain multiplier — both per
sample, both behind the bypass gate so they cost nothing when
`dyn-on = 0` and the fade has settled).

**Saturator** — `tanh(drive_lin × x) / tanh(drive_lin)` for
unity-gain at full drive; drive in dB mapped to linear at param
update. At `drive_db = 0` reduces to identity (no harmonic content).

**Wet/dry smoother** — `Smoothed` (`vxn-2/crates/vxn2-dsp/src/smoother.rs`)
priming the same retarget-on-enable / snap-on-first-set pattern as
phaser (`vxn-2/crates/vxn2-dsp/src/phaser.rs:347-394`):

```rust
pub fn set_enabled(&mut self, on: bool) {
    self.enabled = on;
    // mix retargeted in set_from / next param refresh
}

pub fn set_from(&mut self, p: &DynamicsParams) {
    self.set_enabled(p.on);
    let target = if self.enabled { p.mix.clamp(0.0, 1.0) } else { 0.0 };
    if self.first_set { self.mix.snap(target); self.first_set = false; }
    else { self.mix.set_target(target); }
    // comp + sat coefficients updated unconditionally (cheap)
}

pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
    if !self.enabled && self.mix.current() == 0.0 { return (l, r); }
    // run comp + sat, crossfade against dry by mix.tick()
}
```

**State reset on re-enable from fully-faded-out.** When the fade-out
hits 0 the envelope-follower state is stale; on the next `set_enabled(true)`
it would dump a transient gain reduction. Mirror the pattern at
`vxn-2/crates/vxn2-engine/src/engine.rs:1216-1228` (`limiter_was_on`)
— track `was_active` (i.e. `enabled || mix.current() > 0`) and reset
the detector to unity when we transition from inactive back to active.

**Params struct** (engine-facing snapshot, mirrors `PhaserParams`):

```rust
#[derive(Clone, Copy, Debug)]
pub struct DynamicsParams {
    pub on: bool,
    pub threshold_db: f32,  // -60..0
    pub ratio: f32,         // 1..20
    pub attack_ms: f32,     // 0.1..200
    pub release_ms: f32,    // 5..1000
    pub makeup_db: f32,     // 0..24
    pub drive_db: f32,      // 0..36
    pub mix: f32,           // 0..1 (dry/wet)
}
```

`set_params` clamps; the struct just carries the snapshot.

## Acceptance criteria

- [ ] `vxn2_dsp::dynamics::DynamicsBlock` compiles, no clippy warnings.
- [ ] Unit test: `dyn_on = false` from construction → `process` is
      a bit-exact passthrough from sample 0 (no startup fade).
- [ ] Unit test: switch-on after a load-off start → first wet sample
      is at the smoother's first-tick value, not the param target
      (fade-in active).
- [ ] Unit test: switch-off mid-render fades wet down to 0 (no click),
      then `process` reverts to bit-exact passthrough — assert
      identical to a dry reference after settling.
- [ ] Unit test: known threshold/ratio/attack/release on a step input
      hits the expected gain-reduction floor within tolerance (±0.5 dB).
- [ ] Unit test: tanh drive flattens a 1.0-amplitude sine peak at
      `drive_db = 24` (peak < 1.0 vs. linear identity).
- [ ] Unit test: switch-off → wait for full fade → switch-on → first
      active block has no residual gain reduction (envelope-follower
      reset on inactive→active transition).
- [ ] `cargo test -p vxn2-dsp` passes.

## Notes

The comp and sat run per sample; the per-sample cost behind the
bypass gate (`!enabled && mix.current() == 0.0`) must be **zero**
beyond the gate check — match the phaser early-return shape exactly.

Knee width, detector mode (peak vs. RMS), and saturator flavour stay
internal defaults — same discipline as phaser pinning stages /
centre / spread.

Followed by 0146 (params + decode), 0147 (engine bus), 0148 (faceplate).

## Close-out (2026-06-24)

- `DynamicsBlock` + `DynamicsParams` landed at
  [dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs), module declared
  in [lib.rs:10](../../vxn-2/crates/vxn2-dsp/src/lib.rs#L10). Internal order
  comp → sat as designed; feed-forward linked-sidechain peak detector with
  one-pole attack/release; soft-knee quadratic interp (6 dB internal width);
  `tanh(k·x)/tanh(k)` saturator with `k = 10^(drive_db/20) − 1` so
  drive_db = 0 collapses to identity (limit case) and 36 dB hits unity-gain
  at full drive; one log2 / one exp2 per active sample for the static curve.
  Wet/dry smoother (`MIX_SMOOTH_MS = 30`) follows phaser's
  retarget-on-enable / snap-on-first-set pattern. Detector reset on the
  inactive→active edge via `was_active` (mirror of `limiter_was_on`).
- Acceptance test coverage in `dynamics::tests`:
  - `off_from_load_is_bit_exact_from_first_sample` — `set_from(on=false)` at
    load ⇒ `process` bit-exact for 1000 samples (asserts `to_bits()`).
  - `switch_on_after_load_off_glides_up_from_zero` — first tick after
    `on=false → on=true` shows mix between 0 and target (fade-in active).
  - `switch_off_fades_then_settles_to_bit_exact` — switch-off ⇒ smooth fade
    (no jump on the edge), then 600 ms settle ⇒ bit-exact for 1000 samples.
  - `gain_reduction_matches_known_threshold_ratio` — threshold −20 dB,
    ratio 4, 0 dBFS step ⇒ steady-state −15 dB ± 0.5 dB.
  - `tanh_drive_flattens_sine` — drive_db = 24, 1.0 sine ⇒ peak ≤ 1.001,
    RMS > 0.85 (well above the 0.707 sine baseline).
  - `detector_resets_on_inactive_to_active_edge` — env hammered up,
    fade-out under sustained drive, then switch-on ⇒ `detector_env() == 0`
    on the first active sample.
  - `mix_zero_is_dry` — bonus sanity that mix=0 stays dry through comp+sat.
- `cargo test -p vxn2-dsp` — 191 passed, 1 ignored, 0 failed. clippy on
  `dynamics.rs` clean (only pre-existing warnings in `halfband`/`stack`/
  `tables` remain — out of scope).
- Bit-exact passthrough costs one gate check + one branch
  (`!enabled && mix.current() == 0.0`) — comp/sat/log/exp all behind the
  gate, matching phaser's early-return shape.
- Followed by 0146 (params + decode), 0147 (engine bus), 0148 (faceplate).
