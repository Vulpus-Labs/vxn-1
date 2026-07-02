---
id: "0174"
product: vxn-3
title: "vxn-3 clap.state — save/restore fixed table + engine kind + patch blob; integration pass"
priority: medium
created: 2026-07-02
epic: E032
---

## Summary

Persist VXN3's host-facing state into `clap.state` so a project reload restores it,
and freeze the **state blob format** the future preset epic will reuse. State must
round-trip **through** an engine swap: restore rebuilds the right engine (from the
saved kind) before applying macro/mix values. Closes E032 with a full
`clap-validator` + id-stability integration pass.

Design: ADR 0003 §Consequences (preset stores per track: engine kind + patch + macro
values). Depends on 0171 (value cache), 0172 (engine-kind cache), 0173 (echo).

## Design

- **Extension.** Register `PluginState`; implement `save` / `load` on the main
  thread.
- **Blob format (frozen here).** Versioned, forward-tolerant. Contains:
  - Format version tag.
  - The fixed table's current values (mix + master + macro slots) — from the 0171
    value cache.
  - Per track: `EngineKind` (from the 0172 main-thread cache) + the engine's
    **patch blob** (the deep per-engine params not in the host table — the
    faceplate-only layer). Define a per-engine patch (de)serialization; keep it
    small and explicit, not `#[derive]`-on-everything, so the format is reviewable.
  - The pattern/sequencer state is **out of scope** here unless it already
    round-trips elsewhere — note where sequencer state lives and whether it needs
    inclusion (coordinate with the preset epic; do not silently drop it).
- **Restore order.** On `load`: for each track, rebuild the engine from the saved
  kind via `make(kind, sr)` and queue the swap, update the main-thread kind cache,
  then apply the saved patch + macro/mix values. Restoring before the audio thread
  installs the swap must not glitch.
- **Round-trip through swap.** A saved project whose track later had its engine
  swapped restores the *saved* engine + patch, and its macro slots mean what that
  engine says (ties 0170/0172 together).

## Acceptance criteria

- [ ] `clap.state` `save`/`load` round-trips the fixed table + per-track engine kind
      + patch blob; a reload reproduces the audible state.
- [ ] Restore rebuilds the saved engine per track and applies patch + macro/mix
      values in the correct order (no glitch, no lost values) — integration tested.
- [ ] Blob is versioned; an older version tag loads or degrades cleanly (documented).
- [ ] Ids stable across re-instantiation **and** a save/load cycle; no rescan.
- [ ] `clap-validator` reports **0 failures** across the full VXN3 sweep.
- [ ] `cargo test -p vxn3-clap` green; alloc-trap tests still pass.

## Notes

- This freezes the format the preset epic persists — get the version tag + per-engine
  patch layout right; a later preset system reads the same bytes.
- Document the format (a short doc comment or `vxn-3/adrs` addendum) so the preset
  epic doesn't reverse-engineer it.
- Closes E032: on merge, verify the epic acceptance list and close via the epic
  workflow.
