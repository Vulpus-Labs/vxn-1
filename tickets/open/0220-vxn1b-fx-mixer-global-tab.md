---
id: "0220"
product: vxn-1b
title: "FX / Mixer / Global tab — layer balance, split, FX, master (supersedes 0211)"
priority: high
created: 2026-07-31
epic: E039
depends: ["0215", "0217", "0218", "0219"]
---

## Summary

Build **Tab 3 — FX / Mixer / Global**. Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §7–§8. **Supersedes
[[0211]]** (single-patch FX-tab panel) — FX is now one global chain shared by
both layers.

## Design

- **Mixer / layer balance**: control setting the L1↔L2 balance mixed into the
  **single global FX** chain. Both `Synth`s sum here, not doubled FX.
- **Split**: split **enable** toggle + **split point** (MIDI note). Only
  meaningful when Layer 2 is on; drives KeyMode ([[0215]]).
- **FX params**: all FX controls (Chorus/Phaser/Delay/Reverb/Dynamics), reusing
  the E037 FX chain params, with per-effect header on/off. May reuse a tab strip
  *within* this tab for the effects, or lay them out flat — whichever reads
  cleaner in the compact faceplate.
- **Global**: master **level / pitch / drift ([[0218]]) / limiting**.
- **LFO2 sync** control ([[0217]]): the Layer-2-syncs-to-Layer-1 toggle lives
  here (it's a cross-layer/global concern).
- **MVC**: view never reads model; same dirty-bitset pump.

## Acceptance

- Tab 3 exposes: layer balance → global FX, split enable + point, all FX params
  (per-effect on/off), master level/pitch/drift/limiting, LFO2-sync toggle.
- FX is a single global instance; both layers audibly mix through it per the
  balance control.
- Split enable + point drive KeyMode; inert when Layer 2 off.
- Contract/token tests pass; loads without JS errors; opens in a DAW.
