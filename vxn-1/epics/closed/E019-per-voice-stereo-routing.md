---
id: E019
title: Per-voice stereo spread
status: open
created: 2026-06-07
---

## Goal

Add a per-patch `Spread` control that pans each voice slot to a
fixed position across the L/R field. The FX block always runs in
stereo; spread=0 reproduces the current behaviour exactly (L=R
input → existing FX state machinery produces identical output to
the pre-E019 mono-in path, confirmed by tickets 0101 + 0102 tests).

1. **Spread fader** (0..1, default 0.0) scales per-slot pan
   positions. Default 0 preserves all existing presets bit-identical.
2. **Engine**: voice sum branches into L/R accumulators using fixed
   per-slot pan coefficients × spread. Phaser and chorus always
   receive a stereo bus via their `process_block_stereo` variants
   (0101, 0102).
3. **UI**: new fader column at the end of the Voice panel. Voice
   panel claims width from FX and Master via flex-grow rebalance.

## Background

The current chain sums all voices to a mono bus before the FX block;
stereo width comes entirely from the anti-phase LFO inside phaser
and chorus. That's a fine baseline but leaves the natural per-voice
spread of a polysynth (Jupiter, Prophet) on the table. Lane data
already lives in SoA arrays per layer (8 lanes × 2 layers = 16
slots), so per-slot pan is a cheap branchless extension of the sum
loop.

A `Mono | Stereo` routing switch was considered and dropped:
`StereoPhaser` and `StereoChorus` already hold separate L/R chain
state internally (the pre-E019 mono path was just feeding identical
input to both chains). So there's no CPU saving from a dedicated
mono path — the only cost difference between "mono mode" and
"stereo mode with spread=0" was one extra buffer copy, which isn't
worth a user-facing toggle. Spread=0 IS mono mode.

Per-slot layout: evenly spread across `[-1..+1]`, deterministic by
slot index. Voice 0 → -1.0, voice 7 → +1.0 within each layer, both
layers using the same table. Stable across note-ons (no re-pan when
voices steal) and simpler than activation-order or key-tracked
alternatives.

Pan law: equal-power (sin/cos). At centre (spread=0 or slot 3.5),
each lane contributes `1/√2` to both buses — sum reproduces the
mono bus level exactly.

## In scope

- **Per-slot pan table**: const `[f32; 8]` of normalised positions
  in `vxn-engine`. Equal-power coeffs derived at param-update time
  given `spread`.
- **Voice sum** in `voice.rs` `render_block`: unconditional
  dual-accumulator (sumL, sumR) using per-slot (gL, gR). No mode
  branch.
- **FX entry** in the top-level `render_block`: always stereo L/R
  buffer pair fed into `process_block_stereo` on phaser and chorus.
- **Dead code removal**: delete `StereoPhaser::process_block` and
  `StereoChorus::process_block` (mono-in) — no longer called.
- **Params** (`PatchParam`): add `Spread` (0..1, default 0.0). Per
  [[vxn1-id-stability-dropped]], append cleanly.
- **Faceplate**: new `.ctl` column at end of Voice panel with a
  Spread fader. Adjust panel flex-grow ratios.
- **Preset format**: new `spread` key defaults to 0.0 on load per
  [[vxn1-preset-system]]. Existing presets unchanged.

## Out of scope

- Per-voice pan modulation (LFO/env routes to spread). Spread is
  a static value; revisit if it feels stiff.
- Note-driven pan (key-tracked, random-at-note-on, voice-stealing
  aware). Fixed slot mapping is simpler.
- Per-layer spread. Both layers share the table and the value.
- Reverb/delay stereo entry changes. Delay is already
  sample-by-sample stereo; reverb already stereo-in/out.
- ADR. Straightforward extension of existing topology.

## Phasing

- **0101** ✅ DSP — `StereoPhaser::process_block_stereo` added.
- **0102** ✅ DSP — `StereoChorus::process_block_stereo` added.
- **0103** Params — add `PatchParam::Spread` (0..1, default 0.0).
  Preset round-trip new key default-fills.
- **0104** Engine — unconditional voice sum stereo branch in
  `render_block`, per-slot pan table, equal-power coeff derivation,
  FX entry always stereo. Delete dead mono `process_block` from
  phaser + chorus. Spread=0 bit-identical to current output.
- **0105** Faceplate — add Spread fader column to Voice panel.
  Flex-grow rebalance.

## Dependency order

```text
0101 ✅  ┐
0102 ✅  ┤
         ├── 0103 (param) ── 0104 (engine bus) ── 0105 (faceplate)
```

## Acceptance

- `cargo test --workspace` passes.
- `cargo build -p vxn-clap --release` produces a CLAP that loads
  with a Spread fader visible on the Voice panel.
- Spread=0 (default for all existing presets) produces output
  bit-identical to pre-E019 behaviour.
- Spread=1: voice 0 fully left, voice 7 fully right, smooth pan
  across slots. Verified by ear with a poly chord on a factory
  preset.
- Bench: dry-path RT factor stays within 10% of the
  [[vxn1-render-loop-optimized]] baseline (51× RT for dry_4x). The
  per-block coeff derivation and dual accumulator are cheap; no
  meaningful FX delta since both chains were already running.
- Existing presets load unchanged; new presets save with the
  `spread` key.
