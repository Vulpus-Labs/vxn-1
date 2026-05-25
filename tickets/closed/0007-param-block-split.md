---
id: "0007"
title: "Parameter model: per-patch blocks + global block"
priority: high
created: 2026-05-25
epic: E003
---

## Summary

Split the flat `ParamId` table into a **per-patch block instantiated twice**
(`Upper_*` / `Lower_*`) plus a small **global block**. This is the foundational
change for key modes (ADR 0003 §6): every per-patch parameter becomes
independently automatable per layer, with stable CLAP ids. Everything else in
E003 depends on it.

`KeyMode` and the split point are **not** automatable params — they are
non-automatable **shared state** (setup, not sound; and `KeyMode`'s seed-on-entry
side effect wants a discrete edge, not an automation value stream). They travel
in the plugin-state blob, which is also what presets will serialize — so this
ticket establishes **one canonical serialization** reused by CLAP state and
future preset management (a later ADR), and makes the per-patch block an
**independently serializable unit** so a single-patch preset can later load into
one layer.

## Acceptance criteria

- [x] Define the **per-patch** parameter set (all of: Osc1*, Osc2*, Noise*,
      Cutoff, Resonance, Drive, FilterVariant, HpfCutoff, Osc1Octave,
      Osc2Octave, Env1*, Env2*, the 20 matrix cells, LfoShape, LfoRate,
      LfoDelay, OscSync, CrossMod, ModWheelDest, ModWheelDepth) and the
      **global** param set (MasterTune, MasterVolume, Chorus*, Delay*,
      Oversample). `KeyMode` and split point are **not** in the param table —
      see the shared-state bullet below.
- [x] CLAP id layout: two contiguous per-patch ranges (Upper, then Lower) plus
      the global range. A helper maps `(Layer, PerPatchParam) -> ClapId` and
      `(GlobalParam) -> ClapId`, and the inverse for incoming automation. The
      `MATRIX_BASE` / `matrix_index` scheme (ADR 0001) is preserved **within**
      each per-patch block.
- [x] `ParamValues` (or a successor) exposes a per-layer view: the engine can
      read "layer L's cutoff" cheaply; `SharedParams` mirrors the new layout.
- [x] **One canonical serialization**: a single serializer/deserializer for
      both per-patch blocks + the global params + the shared state (below),
      used by CLAP `state` save/load **and** designed so future preset loading
      reuses it. The per-patch block serializes as a **self-contained unit**
      (so a single-patch preset can target one layer later). Replace today's
      ad-hoc `0..COUNT` iteration. Pre-release: **no backward-compat with old
      saved state** — note it. `count`, `get_info`, `get_value`,
      `value_to_text` updated for the new param id space.
- [x] `KeyMode` (`Whole` / `Dual` / `Split`, default `Whole`) and the split
      point are **non-automatable shared state**, not CLAP params: atomics
      alongside `SharedParams` (audio-thread-readable), persisted via the
      canonical serializer, set discretely from the UI (0013) so the
      seed-on-entry edge (0009) is unambiguous.
- [x] Param-table invariants still hold: ids == indices, defaults in range,
      matrix layout contiguous/ordered (extend the existing tests to the new
      layout).

## Notes

- Biggest blast radius in the codebase: `params.rs`, `vxn-clap` param + state
  impls, `SharedParams`, and every engine read site. Land it as its own change
  before any two-layer behaviour (0008) so the surface is stable.
- Default values per layer are identical (today's defaults); divergence happens
  at runtime via seed-on-entry (0009).
- Keep the per-patch block a single source of truth (one descriptor list,
  instantiated/offset twice) rather than copy-pasting 50 entries.
- **Preset management is a later ADR (0004), not this ticket.** This ticket only
  keeps the doors open: the JP-8 had two tiers — single *Patch* (one layer's
  sound) and *Patch-Preset pair* (both patches + key mode + split). The
  serializable-unit requirement above is what makes both tiers cheap later.
  Don't design preset UX here.
- Validation: `cargo test -p vxn-engine -p vxn-clap`.
