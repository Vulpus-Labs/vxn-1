---
id: "0210"
product: vxn-1b
title: "Mod-matrix overlay — 16-slot editor (source/dest/depth/curve/scale-src) under MVC"
priority: high
created: 2026-07-29
epic: E038
depends: ["0209"]
---

> **SUPERSEDED (2026-07-31) by [[E039]] / [[0219]].** The single global matrix
> overlay is replaced by a **per-layer** overlay (one per layer tab), each bound
> to its layer's 16-of-32 depth params + topology
> ([ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md)). The overlay widget
> itself is **inherited**. Closed as superseded.

## Summary

Build the **mod-matrix overlay**: a scrollable 16-slot editor, one row per slot,
triggered from the preset bar (`Mod Matrix · N` where N = active slot count).
Each row edits source / dest / bipolar depth / curve / scale-source. This is the
routing surface that replaces VXN1's fixed mod panels ([[0209]] deleted them).
[ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §7; [[E038]].

## Design

**Data.** 16 slots (`MATRIX_SLOTS = 16`,
[matrix.rs](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs)). Per slot:

- `source: SourceId` — enum (topology, not a CLAP param)
- `dest: DestId` — enum (topology)
- `depth: f32` — automatable CLAP param `matrix_slot{0..15}_depth`, bipolar
  `[-1,1]` ([params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs))
- `curve: Curve` — enum (topology)
- `scale_src: SourceId` — enum; `—`/None default = off

Depth edits go through the normal automatable-param path; source/dest/curve/
scale-src are **topology** edits carried as state opcodes (not CLAP params) —
mirror the VXN2 topology-edit wire.

**Row per slot:** source selector · dest selector · bipolar depth fader · curve
selector · scale-source selector. Reuse the shared segmented/discrete selectors
from [[0209]]'s ported panels.

**MVC discipline (epic risk).** View emits change events, **never reads the
model**; reflects state on the idle poll (VXN2 dirty-bitset pump). A view that
reads the model mid-drag reintroduces the input-stomp bug class
(`vxn1-vizia-automation-relayout-input-stomp` lineage). Enforce with a parity
test: simulate a mid-drag depth edit + concurrent state push, assert the drag
value is not stomped.

**Calm-when-sparse (epic risk).** Empty rows and `—` scale-source must read as
"off" at a glance — dim/collapse inactive rows so a 3-slot patch looks calm.

## Acceptance

- Overlay opens from the preset-bar `Mod Matrix · N` trigger and lists all 16
  slots.
- Each row edits source/dest/depth/curve/scale-src; depth automates as a CLAP
  param; topology edits round-trip to state and preset TOML.
- MVC parity test passes (no mid-drag stomp; view never reads model).
- Sparse patches render calm; empty/`—` rows read as off.
- Contract/token tests pass.
