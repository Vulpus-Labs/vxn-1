---
id: "0219"
product: vxn-1b
title: "3-tab UI shell + Layer 1 / Layer 2 tabs (supersedes 0209/0210 single-patch)"
priority: high
created: 2026-07-31
epic: E039
depends: ["0215", "0216"]
---

## Summary

Build the three-tab faceplate shell and the two **Layer** tabs. Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §8. **Supersedes [[0209]]**
(single-patch 3-row faceplate) **and [[0210]]** (single matrix overlay) — see
[[E039]] "Relationship to E038." Resolve that overlap before starting.

## Design

- **Tab strip**: Layer 1 · Layer 2 · FX/Mixer/Global ([[0220]]). Tabs are pure
  UI (not signal routing).
- **Tab 1 — Layer 1**: full synth patch — Osc1/2, Mixer, Filter, Env1/2, LFO1,
  LFO2 — bound to **Layer 1** param names. Plus Layer 1's **matrix overlay**.
  Always on.
- **Tab 2 — Layer 2**: identical surface bound to **Layer 2** params + Layer 2's
  matrix overlay. Plus a **Layer 2 on/off toggle** that drives KeyMode ([[0215]])
  — off → Single (synth 2 bypassed), on → Dual/Split.
- **Per-layer matrix overlay**: the 16-slot editor (source/dest/depth/curve/
  scale-src) lives **on each layer tab**, bound to that layer's 32-of-total depth
  params + topology. Clean — no cross-layer "Both" rows to render twice.
- **MVC discipline**: view never reads the model; per-layer dirty-bitset pump
  ([[vxn2-mvc-discipline]]). Two overlays = keep the parity test **per layer**.
- Reuse VXN1b's ported HTML widgets (fader, wave-rotary, button-group, segmented
  switch). Bind `data-param` to the two-layer param names from [[0216]].

## Acceptance

- 3-tab shell; Layer 1 / Layer 2 tabs each carry the full patch surface + a
  private matrix overlay, bound to the correct layer's params.
- Layer 2 on/off toggle drives KeyMode; off leaves synth 2 silent.
- MVC parity test passes for **both** overlays (view never reads model mid-drag).
- Contract/token tests (control→param map per layer) pass.
- `vxn1b-clap` GUI extension opens the faceplate in a DAW; loads without JS
  errors.
