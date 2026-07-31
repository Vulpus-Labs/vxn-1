---
id: "0221"
product: vxn-1b
title: "Two-layer state + preset format — versioned, with single-patch migration"
priority: high
created: 2026-07-31
epic: E039
depends: ["0214", "0216"]
---

## Summary

Extend VXN1b's host-state blob and preset TOML to carry **two layers +
KeyMode/split**, versioned, with migration for any single-patch presets already
saved. Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §4 "Consequences."

## Design

- **Host state (`PluginState`)**: carry two `Synth` patches + two matrix topology
  records + KeyMode + split point + LFO2-sync flag + global (drift/FX/mixer).
  Bump the state version tag; add a decode path that reads the old single-patch
  layout and lifts it into Layer 1 with Layer 2 off (= Single).
- **Preset TOML**: name-keyed sparse format extended with a `[layer2]` section
  (and layer-scoped matrix), plus keymode/split/global. A preset with no
  `[layer2]` loads as single-patch (Layer 2 off) — so pre-existing presets remain
  valid without rewrite.
- **Factory bank**: the E038/0212 bank rebases onto this format (single-patch
  presets stay valid via the no-`[layer2]` default; add a couple of split/dual
  demo presets when convenient — not required to close this ticket).
- Mind `include_dir` no-rerun ([[vxn2-include-dir-no-rerun]]) when touching
  factory TOMLs.

## Acceptance

- Host state + preset TOML round-trip two full layers + KeyMode/split/LFO2-sync.
- **Migration**: a pre-change single-patch VXN1b preset / saved host state loads
  as Layer 1 + Layer 2 off, bit-identical to its old sound.
- A preset with no `[layer2]` section loads as single-patch (no error).
- Round-trip tests: dual and split presets survive save → reload; legacy preset
  migrates. `cargo test -p vxn1b-engine` green.
