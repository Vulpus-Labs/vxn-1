---
id: "0104"
title: Engine — voice sum stereo bus + FX entry
priority: high
created: 2026-06-07
epic: E019
---

## Summary

Wire the new `Spread` param into the voice-render loop. Voice sum
unconditionally accumulates per-lane samples into L/R buses using
fixed per-slot pan coefficients × spread. The FX bus always feeds
phaser and chorus via their `process_block_stereo` variants
(0101, 0102). Spread=0 reproduces current output bit-identically.

Delete the now-unused mono `process_block` from `StereoPhaser` and
`StereoChorus` — no longer called.

## Acceptance criteria

- [ ] Const per-slot pan table: `PAN_POSITIONS: [f32; N]` for N=8
      lanes per layer, evenly spread across `[-1.0..+1.0]`:
      `[-1.0, -0.714, -0.428, -0.143, +0.143, +0.428, +0.714, +1.0]`.
      Same table used for both layers.
- [ ] Per-block equal-power coeff derivation: for each lane,
      `pos = PAN_POSITIONS[v] * spread`, then `gL = cos((pos+1)*π/4)`
      and `gR = sin((pos+1)*π/4)`. Computed once per block at the
      same point `level_comp` and `layer_level` are picked up.
      Spread=0 → all lanes pos=0 → gL=gR=1/√2.
- [ ] `VoiceBank::render_block`: dual-accumulator sum
      `sumL += filt[v] * amp[v] * gL[v];`
      `sumR += filt[v] * amp[v] * gR[v];`
      Output written into L and R oversampled buffers.
- [ ] Both layers accumulate into the same L/R buses.
- [ ] FX bus in the top-level `render_block`: oversample → stereo
      L/R → `process_block_stereo` on phaser and chorus. Delay +
      reverb downstream unchanged.
- [ ] Existing `mono_os` and `mono` buffers replaced (or
      supplemented) by L/R siblings. Avoid heap churn — extend the
      per-engine scratch arrays.
- [ ] Delete `StereoPhaser::process_block` and
      `StereoChorus::process_block` (mono-in variants). Confirm no
      callers remain via grep.
- [ ] Bit-identity test: Spread=0 on all factory presets — output
      matches pre-E019 output sample for sample. (Justified by
      0101 + 0102 tests showing L=R stereo input produces output
      identical to the deleted mono path.)
- [ ] Smoke test: Spread=1 + poly chord → L and R output diverge
      visibly. Voice 0 dominates L, voice 7 dominates R.
- [ ] Bench: dry-path RT factor within 10% of the
      [[vxn1-render-loop-optimized]] baseline (51× RT for dry_4x).
- [ ] `cargo test --workspace` green.

## Notes

Files: `crates/vxn-engine/src/voice.rs` (sum loop ~lines 1001-1005,
FX wiring ~lines 440-505), `crates/vxn-dsp/src/phaser.rs` and
`crates/vxn-dsp/src/chorus.rs` (delete dead mono variants + their
tests if any).

Equal-power pan: `(pos + 1) * π / 4` maps `pos ∈ [-1, +1]` to angle
`[0, π/2]`. At centre (`pos=0`): `gL = gR = cos(π/4) = sin(π/4) =
1/√2`. Hard left: `gL=1, gR=0`. Hard right: `gL=0, gR=1`.

The SoA sum loop is already auto-vectorised by the compiler at
N=8; the dual-accumulator path should vectorise equivalently since
`(filt[v]*amp[v])` is a common subexpression — let the compiler
hoist it, but check the asm dump if NEON usage looks off
([[vxn1-soa-match-defeats-simd]], [[vxn1-neon-grep-pitfall]]).

Per [[vxn1-render-loop-optimized]], the hot path uses block-level
silent-osc skip — keep that path live (skip both L and R
accumulation when the layer is silent).

Smoothing on Spread: smooth the value, not the per-slot coeffs.
Compute coeffs once per block from the smoothed value.

The 0101 phaser test confirmed L=R stereo input produces L and R
outputs each matching the mono path's L channel exactly (no
tolerance). Same for the 0102 chorus test. That's the foundation
for the bit-identity claim at Spread=0.
