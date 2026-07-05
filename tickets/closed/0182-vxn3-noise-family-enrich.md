---
id: "0182"
product: vxn-3
title: "vxn-3 enrich Noise family + flavours (snare-noise / clap); migrate Noise onto the flavour runtime"
priority: high
created: 2026-07-05
epic: E034
---

## Summary

Second family after 0180: move **Noise** off the flat 0179 patch onto the flavour
runtime (like Driven in 0181) and enrich its param space to reach its `patches-drums`
flavours — **snare-noise** and **clap**. Add the synthesis controls those need: a
**state-variable bandpass** (freq + Q) to shape the noise colour, a **snap** onset
transient, and a **multi-tap burst gate** (the thing that makes a clap a clap).

Design: [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md);
builds on **0180** (flavour runtime) and mirrors **0181**'s Driven enrichment. Keep the
per-sample SoA lane loop branchless so it stays vectorisable — the multi-tap gate is a
compare→mask, not a per-lane branch.

## Design

- **Adopt the flavour runtime.** `Noise` gains `flavour + macros + dirty`,
  `from_flavour` / `resolve_patch` / `apply_flavour` / `family_params`, `set_macro`
  stores + marks dirty (re-resolves at next trig), `serialize_patch` = the flavour.
  Replaces the flat 0179 `NoisePatch` serialization.
- **Param space (`P = 8`).** NoiseDecay, ToneDecay, ToneMix, **BandFreq**, **BandQ**,
  **Snap**, **TapCount**, **TapSpacing**. (New `MacroUnit::Ratio` for Q / tap count.)
- **Bandpass** — a TPT/Cytomic state-variable filter on the **noise sum only** (the
  tuned body passes unfiltered, so a snare keeps its low thud). freq + Q from the patch.
- **Snap** — a bright, ~2 ms broadband transient at onset (shared white × per-voice fast
  env), added **post-filter** so it stays crisp. `snap = 0` ⇒ silent.
- **Multi-tap burst gate** — the noise envelope re-fires `tap-count` times at
  `tap-spacing`. Implemented **branchless**: per lane per sample, `fire = (timer ≤ 0) ·
  (taps_left > 0)` (compares → 0/1), re-seed env to 1, decrement. `tap-count = 1` ⇒ the
  old single burst.
- **Author flavours** — snare-noise (bright filtered burst + tuned body + snap, one tap)
  and clap (focused mid band, no body, four rapid taps). Named `Flavour` constructors +
  a `noise_flavours()` registry (0187 moves the bank to TOML).
- **RT discipline.** SVF is mono (outside the lane loop); the gate + snap stay in the
  branchless lane loop. Extend the alloc-trap to a clap (multi-tap + SVF + snap).

## Acceptance criteria

- [ ] `Noise` on the flavour runtime: `NOISE_PARAMS` (P=8) with metadata, `family_params`,
      `apply_flavour`, flavour serialize/deserialize round-trip (via the shared
      cross-engine tests); flat 0179 `NoisePatch` serialization removed.
- [ ] Bandpass shapes noise colour (a high-centre flavour is HF-richer than a low one);
      snap adds isolated onset energy; multi-tap re-fires the burst (a 4-tap clap carries
      far more late-window energy than a single burst) — each proven by a test.
- [ ] snare-noise + clap authored, audibly distinct, enumerated by `noise_flavours()`.
- [ ] Per-sample lane loop stays branchless (gate is compare→mask); alloc-trap extended
      to a clap and passes.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; `clap-validator`
      0 failures.

## Notes

- Metal is now the only engine on the flat 0179 patch → 0183. Mind [[vxn3-flavour-runtime]].
- Flavours Rust-authored here; 0187 relocates the bank to `include_dir!` TOML (mind
  [[vxn2-include-dir-no-rerun]]). Kept in one `noise_flavour(base, defaults)` builder so
  that move is mechanical.

## Close-out (2026-07-05)

Rewrite in [noise.rs](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs); shared unit
in [track_engine.rs](../../vxn-3/crates/vxn3-engine/src/track_engine.rs); cross-engine
tests in [engines/mod.rs](../../vxn-3/crates/vxn3-engine/src/engines/mod.rs).

- **Migrated to the flavour runtime.** `Noise` mirrors Kick/Tone: `flavour/macros/dirty`,
  `from_flavour`/`resolve_patch`/`apply_flavour`/`family_params`, dirty re-resolve at
  trig, `serialize_patch` = the flavour. `NOISE_PARAMS` P=8. Flat patch serialization
  gone; `NoisePatch` kept as the cooked struct.
- **Bandpass** = TPT-SVF (`bandpass()`, mono) on the noise sum; tone body + snap bypass
  it. `MacroUnit::Ratio` added (Q / tap count) + arms in `format_macro_value`/`macro_parse`.
- **Snap** = per-voice fast (`SNAP_DECAY_S = 2 ms`) env × shared white, post-filter.
- **Multi-tap gate** — branchless `fire = (tap_timer ≤ 0) · (tap_left > 0.5)` re-seeds
  `noise_env`; `tap_left`/`tap_timer` per lane; free-lane guard waits for taps to drain.
- **Flavours** `flavour_snare_noise` / `flavour_clap` via one `noise_flavour(base,
  defaults)` builder + `noise_flavours()` registry.
- **Tests:** `snap_adds_onset_energy` (b−a isolates snap), `multitap_refires_the_burst`
  (25–45 ms window ×2 vs single), `bandpass_shapes_noise_colour` (HF-fraction ×1.3),
  `noise_flavours_are_distinct`, `family_params_are_queryable`. Cross-engine
  `flavour_engine_round_trips_through_rebuild` now covers Kick **and** Noise;
  `patch_deserialize_tolerances` splits Metal(flat)/Kick+Noise(flavour). Existing
  idle/perc/voices tests kept.
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap` green (46 lib); alloc-trap
  `driven_flavour_trig_is_allocation_free` extended to a clap on the Noise track; clippy
  0 warnings; `clap-validator` 0 failed.
- Metal is the last flat-patch engine → 0183.
