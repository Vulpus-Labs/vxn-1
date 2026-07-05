---
id: "0183"
product: vxn-3
title: "vxn-3 enrich Metal family + flavours (closed/open hat, ride, cymbal); migrate Metal onto the flavour runtime"
priority: high
created: 2026-07-05
epic: E034
---

## Summary

Third existing family onto the flavour runtime: move **Metal** off the flat 0179 patch
and enrich it to reach its `patches-drums` flavours — closed hat, open hat, ride,
cymbal. After this, all three existing engines are flavour-based; only the new **Struck**
family (0184) remains. Add an **XOR-pair metallic** tone source alongside the modal bank,
a **shimmer** LFO, and keep the note-split open/closed choke.

Design: [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md); mirrors
0181/0182. The ADR leaves open/closed as "two flavours or one choke-driven decay" — this
ticket keeps the existing **note-split choke** (one flavour rings open ≥ split, chokes
< split), so a single hat flavour covers both hits on one body.

## Design

- **Adopt the flavour runtime.** `Metal` gains `flavour + macros + dirty`, `from_flavour`
  / `resolve_patch` / `apply_flavour` / `family_params`, dirty re-resolve at trig,
  `serialize_patch` = the flavour. Replaces the flat 0179 `MetalPatch` serialization.
- **Param space (`P = 9`).** Body, Open, Closed, Excite, Split (note threshold), **Metal**
  (modal↔XOR blend), **Shimmer**, **Rate**, **Bright** (XOR-path HP).
- **XOR-pair metallic** — six square oscillators at inharmonic multiples of Body; their
  sign-**parity** is a cheap 808/909 metallic buzz, enveloped (shares the choke decay)
  and high-passed (Bright) for the hat's noisy character. Blended with the modal ring by
  Metal (`0` = pure modal, `1` = pure XOR).
- **Shimmer LFO** — a slow sine tremolo on the output (Shimmer depth, Rate) for
  cymbal/ride movement. `Shimmer = 0` ⇒ no modulation.
- **Choke unchanged** — note < Split → closed decay damps the shared ring.
- **Author flavours** — closed-hat, open-hat, ride, cymbal via one `metal_flavour(base,
  defaults)` builder + a `metal_flavours()` registry.
- **RT discipline.** The modal loop stays the untouched SoA vector loop; XOR (6 oscs) +
  LFO are cheap scalar work outside it. `Metal = 0, Shimmer = 0` ⇒ pure modal
  bit-for-bit. Extend the alloc-trap to a cymbal.

## Acceptance criteria

- [ ] `Metal` on the flavour runtime: `METAL_PARAMS` (P=9), `family_params`,
      `apply_flavour`, flavour serialize/deserialize round-trip (shared cross-engine
      test); flat 0179 `MetalPatch` serialization removed. **All three existing engines
      now flavour-based** — no flat engines remain.
- [ ] XOR source adds metallic brightness (HF-richer than pure modal); shimmer LFO
      amplitude-modulates the output; note-split choke still damps an open ring — each
      proven by a test (existing choke/re-hit tests kept).
- [ ] closed-hat / open-hat / ride / cymbal authored, audibly distinct, enumerated by
      `metal_flavours()`.
- [ ] Modal loop unchanged (`Metal = 0, Shimmer = 0` ⇒ pure modal); alloc-trap extended
      to a cymbal and passes.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; `clap-validator`
      0 failures.

## Notes

- New `MacroUnit::Ratio` (0182) reused for the Split note threshold.
- Flavours Rust-authored; 0187/0188 relocate the bank to `include_dir!` TOML (mind
  [[vxn2-include-dir-no-rerun]]). Mind [[vxn3-flavour-runtime]].

## Close-out (2026-07-05)

Rewrite in [metal.rs](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs); cross-engine
tests in [engines/mod.rs](../../vxn-3/crates/vxn3-engine/src/engines/mod.rs).

- **Migrated to the flavour runtime.** `Metal` mirrors the other engines; `METAL_PARAMS`
  P=9; flat serialization gone; `MetalPatch` kept as the cooked struct. **All three
  existing engines are now on the flavour runtime — no flat-patch engines remain.**
- **XOR metallic** — six square oscillators (`XOR_RATIOS`), sign-parity `∏ ±1` × a
  choke-shared `xor_env`, high-passed (`Bright`), blended with the modal ring by `Metal`.
- **Shimmer** — sine LFO (`Shimmer` depth, `Rate`) tremolos the output; `Shimmer=0` ⇒ 1.0.
- **Choke** kept: note < `Split` → closed decay. Modal loop byte-identical when
  `Metal=0, Shimmer=0`.
- **Flavours** closed-hat / open-hat / ride / cymbal via `metal_flavour(base, defaults)`
  + `metal_flavours()` registry.
- **Tests:** `xor_source_adds_metallic_brightness` (HF-fraction ×1.3),
  `shimmer_modulates_amplitude` (windowed-RMS coefficient-of-variation ×1.5),
  `metal_flavours_are_distinct`, `family_params_are_queryable`; existing open-ring /
  re-hit / choke tests kept. Cross-engine `flavour_engine_round_trips_through_rebuild`
  now covers all three; `patch_deserialize_tolerances` loops all three (flat test
  removed).
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap` green (49 lib); alloc-trap
  `driven_flavour_trig_is_allocation_free` extended to a cymbal on the Metal track;
  clippy 0 warnings; `clap-validator` 0 failed.
- Next: 0184 adds the new Struck (BridgedT) resonator family — the 4th family.
