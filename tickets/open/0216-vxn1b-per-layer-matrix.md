---
id: "0216"
product: vxn-1b
title: "Per-layer mod matrix — private 16-slot matrix per synth, 32 depth params"
priority: high
created: 2026-07-31
epic: E039
depends: ["0214"]
---

## Summary

Give each `Synth` ([[0214]]) its **own private 16-slot mod matrix**. Today VXN1b
has one matrix: topology in blob state, 16 automatable depth params
([vxn1b-engine/src/matrix.rs](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs),
[params.rs:216-231](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L216-L231)).
Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §4.

## Design

- **Two matrix instances**, one per `Synth`. No pooled slots, no cross-layer
  "Both" tag, **no cross-layer routing** — a slot's source and dest are always
  within its own layer.
- **Sources all per-layer**: Env1, Env2, LFO1, **LFO2** (VXN1b has no global
  LFO), plus per-voice/controller sources (Velocity, Key, ModWheel, PitchWheel,
  Aftertouch, NoteRandom). LFO2's optional cross-layer sync is [[0217]].
- **Params**: matrix depths double to **32** — `Layer1 MatrixSlot0..15 Depth`
  + `Layer2 MatrixSlot0..15 Depth`, each bipolar `[-1,1]`. Extend the `ParamId`
  enum + descriptor table; keep the flat CLAP-id layout clean
  ([[vxn1-id-stability-dropped]]).
- **Topology blob ×2**: two 16-slot topology records (source/dest/curve/scale_src)
  in `PluginState`, one per layer.
- **Eval private to each synth**: matrix runs per-voice within each `Synth`; a
  voice only sees its own layer's matrix. No shared source/dest resolution.

## Acceptance

- Each `Synth` has an independent, separately programmable 16-slot matrix.
- 32 depth params exposed (16/layer); topology stored ×2 in blob.
- Sources/dests resolve within-layer only; no cross-layer route is expressible.
- Round-trip test: distinct matrices per layer survive save/reload.
- `cargo test -p vxn1b-engine` green; allocation-free callback.
