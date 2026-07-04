---
id: "0180"
product: vxn-3
title: "vxn-3 flavour runtime + macro-binding core — family param space, additive-from-base eval, flavour load/apply"
priority: high
created: 2026-07-04
epic: E034
---

## Summary

Build the mechanism at the heart of the voice-roster epic (E034): the runtime that
turns a family's full parameter space plus a **flavour** (base vector + macro-binding
table) into the actual per-trig engine params, allocation-free. No new synthesis and
no UI in this ticket — this is the plumbing all four families (0181–0184) and the
flavour editor (0185) build on. Freezing the `Flavour` data layout here also freezes
the deep-patch bytes ticket 0179 serialises, so land 0180's layout before authoring
any flavours.

Design: [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md)
(Family / Flavour / Macro binding). Depends on the host macro surface (E032: `K = 3`
slots, 0170's `set_macro`) and pairs with **0179** (serialises the `Flavour`).

## Design

- **Family param space.** Give each family a declared parameter table: for every
  param, a stable id, display name, unit, range `[min,max]`, default, and response
  curve. Metadata is data, queryable on the main thread (the flavour editor + 0172
  value-text read it) — not buried in the kernel.
- **`Flavour` type.** `base: [f32; P]` (a value per family param) + `bindings:
  Vec<Binding>` where `Binding = { slot: u8, param: ParamId, depth: f32, curve }`
  + `macro_defaults: [f32; K]`. A param may appear in multiple bindings; unbound
  params use `base`. Keep the byte layout explicit + version-tagged (0179 reads it).
- **Evaluation (additive-from-base), per trig — not per sample:**
  `final(p) = clamp(base[p] + Σ_{b: b.param==p} b.curve(macro[b.slot]) · b.depth, range(p))`
  Compute the resolved param vector once when a voice triggers (macros + flavour are
  stable within a trig); the per-sample SoA kernels consume the resolved values
  unchanged. **Allocation-free**; extend the alloc-trap test.
- **Load / apply.** `Engine::apply_flavour(&Flavour)` sets the base + bindings on the
  live engine; `set_macro(slot, v)` updates a macro value and marks the resolved
  vector dirty (re-resolved on next trig). Applying a flavour must not glitch a
  sounding voice (resolve at next trig boundary, consistent with micro-timing ADR
  0004 scheduling).
- **Curve set.** Start minimal — linear + one exponential — behind a `Curve` enum so
  0185/authoring can widen. ADR 0005 leaves the final set open; pick the smallest
  that lets the first flavours (0181) feel right.
- **Relationship to 0170 macros.** The 3 host macro slots feed `set_macro`; this
  ticket is what makes a slot *mean* something per flavour (replacing the fixed
  per-engine map). `macro_display` (0172) becomes flavour-aware: it reads the
  binding table for the slot. Wire that read here; leave the clap text glue to 0172's
  existing path.
- **Scope.** One family is enough to prove the runtime — do it against the current
  Driven engine's existing params (no enrichment yet; that's 0181). Other families
  adopt the trait in their own tickets.

## Acceptance criteria

- [ ] Family param space declared with per-param metadata (id/name/unit/range/
      default/curve), queryable on the main thread.
- [ ] `Flavour` type (base + binding table + macro defaults) with an explicit,
      version-tagged byte layout that **0179** serialises/deserialises round-trip.
- [ ] Additive-from-base evaluation resolves the param vector per trig, clamped to
      range, allocation-free (alloc-trap test extended); per-sample kernels unchanged.
- [ ] `apply_flavour` + `set_macro` update a live engine without glitching a sounding
      voice; a macro move re-resolves on the next trig.
- [ ] `macro_display` (0172) reads the flavour binding table so a slot's text
      reflects what the current flavour bound it to.
- [ ] Demonstrated on the Driven family with its current params (two hand-set
      flavours differ audibly via base only, and via a macro binding); `cargo test
      -p vxn3-engine` green.

## Notes

- **Freeze the `Flavour` layout here.** Every flavour authored in 0181–0184 and every
  saved user flavour (0185) is these bytes; a change later is format debt across the
  whole roster. Coordinate the version tag with 0179's blob version.
- Resist param-space growth in this ticket — enrichment is per-family (0181–0184).
  Here, only the *mechanism* matters.
- Keep the binding eval readable; it's a constrained mod-matrix, not the vxn-2 general
  matrix ([[vxn2-architecture]]) — don't generalise beyond ADR 0005 without a reason
  that came from playing.

## Close-out (2026-07-04)

- **Flavour runtime module** [flavour.rs](../../vxn-3/crates/vxn3-engine/src/flavour.rs):
  `Curve {Linear, Exp}`, `ParamMeta {name/unit/min/max/default}`, `Binding
  {slot,param,curve,depth}`, `Flavour {base: Vec<f32>, bindings: Vec<Binding>,
  macro_defaults: [f32; K]}`. Explicit version-tagged LE byte layout
  (`FLAVOUR_VERSION=1`) via `Flavour::serialize`/`deserialize`; `deserialize` returns
  `Ok(Some)` / `Ok(None)` (version-or-shape mismatch → keep default) / `Err` (truncated)
  — the 0179 deep-patch contract.
- **Additive-from-base eval** `flavour::resolve(meta, base, bindings, macros, out)` —
  `final(p)=clamp(base[p]+Σ curve(macro[slot])·depth, range)`, writes a caller scratch,
  allocation-free. Tests `resolve_is_additive_from_base_and_clamped`,
  `multiple_bindings_on_one_param_sum`.
- **Driven family adopts the runtime**
  [kick_tone.rs](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs): `DRIVEN_PARAMS`
  (4-param space: Attack/Decay/Depth/Donk) + `driven_default_flavour` (3 host macros as
  editable additive bindings, replacing the fixed 0170 map). `KickTone` holds
  `flavour + macros + dirty`; `on_trig` re-resolves into `patch` + re-cooks only when
  dirty (per-sample SoA kernel untouched). `serialize_patch` = the flavour; `set_macro`
  updates live macro + marks dirty; `apply_flavour` + `family_params` added to
  `TrackEngine` (default no-op / empty). Metal/Noise keep the flat 0179 patch until
  their enrichment tickets (0182/0183).
- **No glitch / next-trig re-resolve** proven: `change_takes_effect_on_next_trig_not_mid_voice`
  (a mid-voice `apply_flavour` leaves the ringing voice byte-identical; the new flavour
  bites on the next trig).
- **Flavour-aware value-text** `flavour::flavour_macro_display` reads the binding table
  (test `display_reflects_the_binding`); shares `track_engine::format_macro_value` with
  the fixed `macro_display` so units can't drift. Clap `value_to_text` rewiring to the
  main-thread flavour is left to 0172's path / the 0185 editor (no live main-thread
  flavour store yet).
- **Driven demonstrated** (acceptance #6): `two_flavours_differ_by_base_only`,
  `macro_binding_drives_sound_only_when_bound`, `driven_flavour_round_trips_through_rebuild`.
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap` green (incl. alloc-trap
  `driven_flavour_trig_is_allocation_free` in [kit.rs](../../vxn-3/crates/vxn3-engine/tests/kit.rs));
  clippy 0 warnings; `clap-validator` 0 failed (state-reproducibility suites PASS).
- **Format note:** this supersedes 0179's flat Kick/Tone patch bytes with the flavour
  layout (pre-release, no presets shipped) — the layout is now **frozen** for 0181–0185.
  Live macro **values** are host state, not serialised in the patch (ADR 0005).
