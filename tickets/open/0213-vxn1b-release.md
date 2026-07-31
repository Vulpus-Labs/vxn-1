---
id: "0213"
product: vxn-1b
title: "VXN1b release — bundle/deploy, docs, clap-validator clean, ADR 0001 → Accepted"
priority: medium
created: 2026-07-29
epic: E038
depends: ["0209", "0210", "0211", "0212"]
---

## Summary

Ship VXN1b. Bundle/deploy via xtask, write the docs, pass `clap-validator`, DAW
smoke, and flip [ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) status
→ **Accepted**. This closes [[E038]] and the VXN1b variant.

## Tasks

- **Bundle/deploy.** `vxn-1b/xtask` builds `target/release/vxn1b.clap` and
  installs to `~/Library/Audio/Plug-Ins/CLAP/vxn1b.clap`
  ([xtask/src/main.rs](../../vxn-1b/xtask/src/main.rs)). Confirm the bundle
  contains the built faceplate assets ([[0209]]–[[0211]]) and factory bank
  ([[0212]]).
- **Docs.** README for VXN1b (what it is; how it differs from VXN1 — compact
  faceplate + matrix routing) and a PARAMETERS.md for the vxn1b param table
  (osc/mixer/filter/LFO/env/voice/master + 16 matrix depths + FX groups).
- **Validation.** `clap-validator validate` clean on the bundle.
- **DAW smoke.** Load in Reaper; play; verify faceplate, matrix overlay, FX tabs,
  factory presets. `verify-audio-in-reaper` — **user verifies manually**; don't
  build a headless audio harness.
- **ADR.** Flip ADR 0001 status → Accepted.

## Acceptance

- Bundle deploys and loads in a DAW; faceplate + overlay + FX tabs + factory bank
  all present and working.
- `clap-validator` reports clean.
- README + PARAMETERS.md land for VXN1b.
- ADR 0001 status → Accepted.
- Epic [[E038]] close-out written; epic closed.
