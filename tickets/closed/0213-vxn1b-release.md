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

## Close-out (2026-08-27)

- Bundle path works end to end: `cargo run -p vxn1b-xtask -- bundle` produces
  `target/bundled/vxn1b.clap`; the release workflow builds macOS universal + the
  Windows VST3 via clap-wrapper, with the `/INCLUDE:clap_entry` fix that stopped
  `/OPT:REF` stripping the whole-archived staticlib ([[vxn-windows-vst3-optref-strip]],
  `6599f54`). Shipped as `vxn-1b-0.0.1` (`9842c0c`); tag line is `vxn-1b-*`
  ([[vxn-release-process]]).
- **clap-validator on `target/bundled/vxn1b.clap`: 20 tests run, 17 passed,
  0 failed, 3 skipped, 1 warning** — clean. The skips are the unimplemented
  preset-discovery factory.
- [README.md](../../vxn-1b/README.md) and
  [PARAMETERS.md](../../vxn-1b/PARAMETERS.md) both land for VXN1b.
- [ADR 0001](../../vxn-1b/adrs/0001-vxn1b-design.md) status is
  **Accepted (2026-08-24, ticket 0213 — shipped as `vxn-1b-0.0.1`)**.
- `cargo test --workspace` 1622 passed, 0 failed.
- **Not verified here:** the Reaper DAW smoke (faceplate, matrix overlay, FX
  tabs, factory bank, play) — [[verify-audio-in-reaper]], user-verified by hand.
- E038's other child (0212, factory preset bank) is already closed, so this is
  the last one — `close-epic E038` now applies.
