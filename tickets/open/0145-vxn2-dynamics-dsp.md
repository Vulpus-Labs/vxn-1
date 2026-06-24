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
