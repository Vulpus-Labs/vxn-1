---
id: "0184"
product: vxn-3
title: "vxn-3 new Struck family (BridgedT resonator) + flavours (kick2 / tom2 / claves2 / modal cymbal)"
priority: high
created: 2026-07-05
epic: E034
---

## Summary

The fourth and final voice family (ADR 0005): a brand-new **Struck** engine — the
`patches-drums` "2" resonator school (BridgedT struck resonator) — built directly on the
flavour runtime, and registered into the closed roster. With this the roster is complete:
Driven, Noise, Metal, Struck.

Struck is a small bank of struck partials with **pitch-droop** (the body glides down after
the hit), **Q-as-decay** (one decay time = ring length), and a **selectable excitation
shape** (dirac / exp / half-sine / filtered-click). Pitch follows the sequenced note ×
`Tune`; `Inharm` blends the partials harmonic→inharmonic (drum → modal cymbal).

Design: [ADR 0001](../../vxn-3/adrs/0001-vxn3-architecture.md) §6 (closed roster);
[ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md). Built on the
0180 flavour runtime; mirrors 0181–0183.

## Design

- **New engine** `struck.rs` on the flavour runtime from the start (no flat-patch phase).
  `STRUCK_MODES = 4` (one `f32x4`); SoA mode loop autovectorises; droop + shaped
  excitation are cheap scalar work outside it.
- **Param space (`P = 8`).** Decay (Q), Tune (× note freq), DroopDepth, DroopTime (glide),
  ExcShape (0..3), ExcTime (strike length), Excite, Inharm.
- **Pitch-droop** — a shared multiplier `2^(droop/12)` relaxing to 1 (as in `Kick/Tone`),
  scaling every mode's phase increment per sample (cheap freq modulation).
- **Q-as-decay** — one `decay_coef` sets all modes' ring length.
- **Excitation shapes** — dirac (near-impulse), exp (exponential burst), half-sine
  (windowed), filtered-click (noise × fast decay); the strike transient added on top of
  the struck ring. A per-sample match on the shape sits outside the mode loop.
- **Inharm** blends `HARMONIC → INHARMONIC` mode ratios (kick2/tom2/claves2 harmonic;
  modal cymbal inharmonic).
- **Roster registration.** `EngineKind::Struck` (tag 3, appended never renumbered),
  `make()`, the old value-text `macro_coeffs` fallback, and the ui-web engine picker all
  gain the family.
- **Author flavours** — kick2, tom2, claves2, modal-cymbal via one `struck_flavour(base,
  defaults)` builder + a `struck_flavours()` registry.

## Acceptance criteria

- [ ] `Struck` engine on the flavour runtime: `STRUCK_PARAMS` (P=8), `family_params`,
      `apply_flavour`, flavour serialize/deserialize round-trip (shared cross-engine test).
- [ ] Registered in the closed roster: `EngineKind::Struck` (as_u8/from_u8 stable),
      `make()`, ui-web picker; `clap.state` round-trips a Struck track.
- [ ] Pitch-droop audible (onset higher-pitched than tail); excitation shape changes the
      strike; Q sets ring length; each proven by a test.
- [ ] kick2 / tom2 / claves2 / modal-cymbal authored, audibly distinct, enumerated by
      `struck_flavours()`.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap -p vxn3-ui-web` green; alloc-trap extended
      to a Struck track; clippy clean; `clap-validator` 0 failures.

## Notes

- Roster is now **closed at four families** (ADR 0001 §6). New drums are flavours (data),
  not engines. Mind [[vxn3-flavour-runtime]].
- Flavours Rust-authored; the factory bank ticket (0188+, since 0187 was taken by vxn-2)
  relocates them to `include_dir!` TOML (mind [[vxn2-include-dir-no-rerun]]).

## Close-out (2026-07-05)

New engine [struck.rs](../../vxn-3/crates/vxn3-engine/src/engines/struck.rs); roster
wiring in [track_engine.rs](../../vxn-3/crates/vxn3-engine/src/track_engine.rs),
[engines/mod.rs](../../vxn-3/crates/vxn3-engine/src/engines/mod.rs), and
[ui-web/lib.rs](../../vxn-3/crates/vxn3-ui-web/src/lib.rs).

- **Struck engine** on the flavour runtime from the start; `STRUCK_MODES=4`, `STRUCK_P=8`.
  Pitch-droop = shared `2^(droop/12)` multiplier relaxing to 1, scaling per-mode phase inc;
  Q-as-decay = one `decay_coef`; excitation `dirac/exp/half-sine/filtered-click` as a shaped
  transient (per-sample match outside the SoA mode loop); `Inharm` blends HARMONIC↔INHARMONIC
  ratios; pitch = note × `Tune`.
- **Roster:** `EngineKind::Struck` (tag 3), `make()`, `macro_coeffs` value-text fallback
  (Decay/Excite/Inharm), ui-web `engines` list + `kind_of`. Roster **closed at four**.
- **Flavours** kick2 / tom2 / claves2 / modal-cymbal via `struck_flavour(base, defaults)`
  + `struck_flavours()`.
- **Tests:** `struck_hit_rings_then_decays`, `pitch_droops_downward` (early zc > late),
  `excitation_shape_changes_onset` (dirac ≠ half-sine), `decay_controls_ring_length`
  (late-window ×4), `struck_flavours_are_distinct`, `family_params_are_queryable`.
  Cross-engine `flavour_engine_round_trips_through_rebuild` + `patch_deserialize_tolerances`
  now cover all four; `track_engine` value-text roster tests include Struck.
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap -p vxn3-ui-web` green (55 lib);
  alloc-trap extended to a Struck kick2 track; clippy 0 warnings; `clap-validator` 0 failed.
- Roster complete → 0185 (flavour editor + save-as-flavour) is the next E034 step.
