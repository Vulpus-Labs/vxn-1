---
id: "0212"
product: vxn-1b
title: "Factory preset bank — matrix-idiom init set incl. wheel-vibrato + MPE-pressure demos"
priority: medium
created: 2026-07-29
epic: E038
depends: ["0209", "0210", "0211"]
---

## Summary

Ship a small **factory preset bank** for VXN1b, embedded via `include_dir`. Tune
the set to the matrix routing idiom, and include two demos that showcase the
variant's flexibility: a **wheel-gated vibrato** (scale-source) and an
**MPE-pressure** patch (aftertouch → cutoff/amp). [[E038]].

## Design

**Format.** Name-keyed sparse TOML (ticket 0203,
[preset.rs](../../vxn-1b/crates/vxn1b-engine/src/preset.rs)): only non-default
params written; matrix as `[[matrix]]` array (source/dest/curve/scale-src by
kebab name; slots with `source: none`/`dest: none` omitted).

**Embed.** Add an `include_dir` factory bank (VXN1b has none today — factory is
in-code `PluginState::factory_default()` in
[state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs)). Wire a `factory.rs`
enumerating the bundled TOMLs. **Touch `factory.rs` before install** —
`include_dir!` emits no rerun-if-changed (`vxn2-include-dir-no-rerun`).

**Demos:**

- **Wheel-gated vibrato** — LFO1 → Pitch depth, with mod-wheel as **scale-source**
  on that slot, so vibrato only sounds when the wheel is up.
- **MPE-pressure** — channel/poly pressure (aftertouch) → cutoff and/or amp via
  matrix slots, exercising the E036 MPE source.

**Legality.** All presets original subtractive patches — no DX7 rips, no legal
posture concern (contrast `vxn2-factory-preset-legal-posture`).

## Acceptance

- A factory bank of original presets embeds via `include_dir` and loads in the
  browser.
- Includes the wheel-gated-vibrato and MPE-pressure demos, both audibly doing
  what they claim.
- Each preset round-trips through save → reload (sparse TOML) with no drift.
- `factory.rs` touched in the install path so bank edits actually recompile.
