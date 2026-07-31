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
