---
id: "0181"
product: vxn-3
title: "vxn-3 enrich Driven family + author flavours (kick / tom / snare-body / claves)"
priority: high
created: 2026-07-05
epic: E034
---

## Summary

First family after the 0180 flavour runtime: grow the **Driven** param space just
enough to reach its four `patches-drums` flavours, and author them as data. Driven
today is a phase-accum sine + pitch sweep + amp env with `P = 4` (Attack / Decay /
Depth / Donk). Add the synthesis controls a kick, tom, snare-body and claves need —
**drive** (saturation → harmonics/punch) and **click** (a broadband onset transient
→ beater/tick) — then author the four flavours over the enriched space and prove two
of them morph into each other by base edits alone.

Design: [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md)
(family = one architecture, flavours = points in its space). Builds on **0180**
(flavour runtime, `resolve`, param-space metadata) and **0179** (the flavour is the
serialised deep patch). Keep the per-sample SoA lane loop branchless + vectorisable
(the vxn-1/2 "no match/branch in the lane loop" lesson).

## Design

- **Param-space growth (`P = 4 → 6`).** Add to `DRIVEN_PARAMS`:
  - **Drive** (`0..1`, default 0) — pre-output saturation of the oscillator. A
    branchless cubic soft-clip `d·(1.5 − 0.5·d²)` on `sine·pre` (pre grows with drive),
    blended `sine + drive·(sat − sine)` so **drive = 0 is bit-clean** (default sound
    unchanged). Adds odd harmonics → kick punch / snare buzz.
  - **Click** (`0..1`, default 0) — a short broadband transient at trig onset: a shared
    white-noise source (xorshift, as `Noise`) gated by a per-voice fast-decaying click
    envelope (fixed ~3 ms). `click = 0` ⇒ silent (default unchanged). Gives claves their
    tick and a kick its beater attack.
- **Defaults stay inert.** Both new params default to 0, so the default Driven flavour
  and all existing engine tests render identically — the enrichment only bites when a
  flavour asks for it. `sweep-start` is already covered by **Depth** (the pitch-sweep
  start above the settled note); no separate param (resist bloat — Risks, E034).
- **Flavour byte layout unchanged.** Only the family's `P` grows; `Flavour`'s
  version-tagged format (0180) already stores `n_params`, so a 6-param Driven flavour
  is self-describing. `FLAVOUR_VERSION` unchanged. (Pre-release: no saved 4-param
  Driven flavours exist to migrate.)
- **Author four flavours** over the enriched space, as named `Flavour` constructors
  (data; 0187 later moves the bank to `include_dir!` TOML): **kick** (low, deep sweep,
  short donk, slight drive + small click), **tom** (mid, moderate sweep, longer decay,
  clean), **snare-body** (mid, short decay, more drive, small click), **claves** (high,
  ~no sweep, very short decay, strong click, no drive). Each keeps the three host-macro
  bindings so the played knobs stay meaningful. Expose a small `driven_flavours()`
  registry (name → flavour) for the editor/bank to enumerate.
- **RT discipline.** Drive + click stay in the branchless 4-wide lane loop (shared
  scalar coefficients; per-lane arrays only). Extend the alloc-trap coverage — a
  flavour with drive + click trigging must not allocate.

## Acceptance criteria

- [ ] `DRIVEN_PARAMS` gains Drive + Click (metadata: name/unit/range/default); `DRIVEN_P
      == 6`; `resolve` fills all six; the per-sample kernel stays branchless (drive/click
      are shared scalars + per-lane arrays, no match/branch added).
- [ ] Drive = 0 **and** Click = 0 reproduce the pre-0181 Driven output bit-for-bit
      (default flavour + existing `kick_tone` tests unchanged).
- [ ] Four flavours authored (kick / tom / snare-body / claves), each audibly distinct;
      a `driven_flavours()` registry enumerates them.
- [ ] Two flavours morph via **base edits alone** (e.g. kick → tom by pitch/decay/sweep
      base), macros held constant — a test asserts the audible difference.
- [ ] Drive adds harmonics (spectral/■ proxy) and click adds onset energy — each proven
      independently by a test.
- [ ] `cargo test -p vxn3-engine` green; alloc-trap (drive+click trig) passes; clippy
      clean; `clap-validator` 0 failures on the VXN3 bundle.

## Notes

- Metal/Noise remain on the flat 0179 patch until 0182/0183; this ticket only touches
  Driven + the flavour runtime it adopted in 0180. Mind [[vxn3-flavour-runtime]].
- Flavours are Rust-authored here for speed; 0187 relocates the factory bank to TOML
  via `include_dir!` (mind [[vxn2-include-dir-no-rerun]]). Keep the flavour values in
  one place so that move is mechanical.

## Close-out (2026-07-05)

All in [kick_tone.rs](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs).

- **Param space `P = 4 → 6`.** Added `P_DRIVE`/`P_CLICK` + two `DRIVEN_PARAMS` entries
  (Drive/Click, Percent, 0..1, default 0). `resolve` fills all six; `resolve_patch`
  assigns them; `KickTonePatch` gains `drive`/`click`. Flavour byte layout unchanged
  (`n_params` self-describes; `FLAVOUR_VERSION` untouched).
- **Drive** = branchless cubic soft-clip `d·(1.5−0.5·d²)` on `sine·pre`
  (`pre = 1 + drive·4`), blended `sn + drive·(sat − sn)` — stays in the 4-wide lane loop
  (shared scalars + per-lane arrays, no branch/match added). `drive = 0` ⇒ `sn` exactly.
- **Click** = shared xorshift white (as `Noise`) × per-voice fast-decaying `click_env`
  (fixed `CLICK_DECAY_S = 3 ms`), seeded to `click_level` at trig, `+ n·click_env·peak`
  in the accumulate. `click = 0` ⇒ contributes 0.
- **Bit-exact default:** with drive/click 0 the accumulate is `sn·amp + 0.0 == sn·amp`;
  the four pre-existing `kick_tone` tests (idle/decay/sweep/overlap) pass unchanged +
  `drive_and_click_inert_at_zero`.
- **Flavours authored** (`flavour_kick/tom/snare_body/claves` + `driven_default_flavour`,
  all via one `driven_flavour(base, macro_defaults)` builder so the 0187 TOML move is
  mechanical) + `driven_flavours()` name→flavour registry.
- **Tests:** `drive_adds_harmonics` (HF-fraction proxy, ×1.5), `click_adds_onset_energy`
  (b−a *is* the click: onset-vs-tail ×4 + broadband), `kick_and_tom_morph_via_base`
  (macros held equal), `authored_flavours_are_distinct` (pairwise via registry),
  `family_params_are_queryable` (P=6, Drive/Click named).
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap` green; alloc-trap
  `driven_flavour_trig_is_allocation_free` extended to drive+click on; clippy 0 warnings;
  `clap-validator` 0 failed.
- Metal/Noise untouched (flat 0179 patch) — 0182/0183.
