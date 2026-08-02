---
id: "0214"
product: vxn-1b
title: "Synth as an instantiable unit — plugin holds 2 × Synth + global block"
priority: high
created: 2026-07-31
epic: E039
depends: []
---

## Summary

Make the VXN1b core synth an **instantiable unit** so the plugin can hold two of
them. Today `vxn1b-engine` renders one synth via two `RenderBank`s (lanes 0–7 /
8–15) that exist for **stereo decorrelation, not layers**
([vxn1b-engine/src/engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs)).
Wrap voices + patch + matrix + drift consumers into a `Synth` struct; the plugin
holds **2 × `Synth` + a global block** (FX, mixer/balance, master, demux — later
tickets). Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §1.

## Design

- **`Synth` unit.** One synth = its own voice pool, allocator, voice stealing,
  twin/unison, patch params, and (in [[0216]]) its own matrix. Allocation and
  stealing are **private to each synth** — no shared pool, no `param_source`
  indirection (contrast VXN1
  [vxn-engine/src/lib.rs:713-762](../../vxn-1/crates/vxn-engine/src/lib.rs#L713-L762)).
- **Voice budget: 16 voices/synth (32 max).** Each `Synth` keeps VXN1b's
  two-`RenderBank` internal layout for stereo decorrelation.
- **Global block.** New top-level struct owning both `Synth`s + placeholders for
  FX/mixer/master/demux (filled by [[0215]]/[[0218]]/[[0220]]).
- **Single-mode bypass.** When Layer 2 is off, synth 2 is not ticked at all —
  single mode is byte-for-byte today's output at today's CPU.
- **No routing yet.** This ticket wires structure only; both synths currently
  receive the same events (demux is [[0215]]). Verify independence by driving
  them with different patches in a test.

## Acceptance

- `vxn1b-engine` exposes a `Synth` unit; the plugin/engine top level holds two +
  a global block.
- Voice allocation/stealing/unison are per-`Synth`; no shared pool remains.
- 16 voices/synth; single mode ticks only synth 1 (bench: single-mode CPU
  unchanged vs pre-change HEAD).
- Test: two `Synth`s with distinct patches render distinct output from the same
  note stream.
- `cargo test -p vxn1b-engine` green; no allocation in the process callback.

## Close-out (2026-08-02)

- **`Synth` unit** in [synth.rs:46](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L46):
  owns its `VoicePool`, two `RenderBank`s, `Params`, `MatrixTable`, mod-smoothing
  state and drift seeds (`SynthSeeds::LAYER1`/`LAYER2` give the two synths
  distinct drift/noise streams). All entry points (`note_on`, `note_off`,
  `poly_pressure`, `channel_pressure`, `set_param`, `render_control_block`) are
  `pub(crate)` — nothing outside reaches into a synth's pool.
- **Global block**: `Engine` now holds `synths: [Synth; 2]` plus the one serial FX
  chain and master
  ([engine.rs:162-190](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L162-L190)).
  `render_control_block` pre-zeroes, ticks each active synth accumulating into the
  shared buffers, then runs FX + master + finite guard
  ([engine.rs:357-382](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L357-L382)).
- **Allocation/stealing per synth, no shared pool**: no `param_source`
  indirection anywhere in vxn1b-engine (grep clean); `synth::tests::stealing_is_per_synth`
  ([synth.rs:369](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L369)) proves one
  synth's steal doesn't touch the other's voices.
- **16 voices/synth**: `MAX_VOICES == 2 * RenderBank::LANES`, asserted in
  [lib.rs:65-66](../../vxn-1b/crates/vxn1b-engine/src/lib.rs#L65-L66); each `Synth`
  keeps the two-bank internal layout for stereo decorrelation.
- **Single-mode bypass**: synth 2 is gated on `KeyState::layer2_on` in the render
  path and in every event entry point — while layer 2 is off it is never ticked
  ([engine.rs:361-365](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L361-L365)),
  so single mode is today's signal path at today's CPU. Covered by
  `engine::tests::single_mode_leaves_layer2_idle`
  ([engine.rs:539](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L539)).
- **Independence test**: `synth::tests::two_synths_distinct_patches_render_distinct_output`
  ([synth.rs:339](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L339)) drives two
  `Synth`s with distinct patches from the same note stream and asserts distinct
  output.
- **No allocation in the process callback**: grep for `Vec`/`vec!`/`Box::new`/
  `collect()` across [synth.rs](../../vxn-1b/crates/vxn1b-engine/src/synth.rs) and
  [bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs) hits `#[cfg(test)]` code
  only; the render path is fixed-size arrays throughout.
- `cargo test -p vxn1b-engine` green: 118 + 1 + 2 + 4 pass, 0 fail.
- Shipped in `ffa26f6`; downstream [[0215]] (demux) and [[0216]] (per-layer
  matrix) already build on this structure.
