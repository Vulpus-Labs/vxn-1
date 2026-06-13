---
id: "0097"
title: "Factory-preset re-audit + matrix tests & benches"
priority: medium
created: 2026-06-12
epic: E008
depends: ["0091", "0092", "0093", "0094"]
---

## Summary

Eighth and final ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md).
Once every sound-affecting change has landed (`lfo2-phase` wired 0091, LFO rates
0092, stack macros 0093, units sanified 0094), audit all factory presets for
dead/incoherent routes — the original review ask — fix or repoint them, confirm
the unit recalibration didn't wreck levels, and add the matrix tests + benches
that lock the new behaviour in.

## Design

**Preset audit.** Walk every `presets/factory/**/*.toml`
([example](../../crates/vxn2-engine/presets/factory/Keys/Mark%20II%20E-Piano.toml))
matrix block. For each `[[matrix]]` slot:

- Flag any slot whose `dest` was previously dead (`lfo2-phase`, `lfo1-rate`,
  `lfo2-rate`, `stack-detune`, `stack-spread`) — these were *inert* when the
  preset was authored and are now *live*. Re-audition: the intended sound was
  whatever the preset made *without* that route. Either (a) the route now does
  something musical and was latent intent (keep, verify it sounds right) or
  (b) it was an accident and now colours the patch (repoint or remove). Decide
  per slot; the `voice-rand → lfo2-phase` in Mark II E-Piano is case (a) — it's
  the supersaw-shimmer route that should now work.
- Flag any slot the 0090 coherence predicate marks `TierCollapse` / `SelfRate` /
  `Degenerate` and repoint to a coherent source/dest.
- Re-check `pitch-eg → *-pitch` routes affected by the 0094 unit fix (the old
  24× collapse) — depths likely need re-scaling to restore the authored pitch
  excursion.

Produce a short audit table in the close-out (preset → slot → old behaviour →
action) so the sound changes are traceable.

**Tests** (extend the matrix + engine suites):

- Coherence predicate grid (may already exist from 0090 — assert it's the one
  the preset audit uses).
- Each newly-wired dest end-to-end: an engine test per dest asserting a slot
  with depth > 0 modulates the expected state and depth 0 is bit-identical.
- A "no factory preset routes to an incoherent or dead dest" test that parses
  every factory TOML and runs each slot through the coherence predicate — fails
  CI if a future preset reintroduces a dead/incoherent route.
- Unit-range assertions (from 0094) and the `pitch-eg` no-double-scale check.

**Benches** (extend the existing busy/render benches):

- Cost of the gated rate dests (0092) and stack re-cook (0093): density-8 stack
  with vs without an active targeting slot, confirming the gate makes the
  unused path free and quantifying the used-path cost.
- Confirm the matrix per-slot eval loop still autovectorises after 0094's source
  normalization (asm spot-check or the existing vectorisation guard).

## Acceptance criteria

- [x] Every factory preset audited (table below); no slot points at an
  incoherent or formerly-dead dest. Locked by
  `factory::tests::no_factory_preset_routes_incoherently`.
- [x] `voice-rand → lfo2-phase` in Mark II E-Piano is now consumed (0091) — the
  intended supersaw shimmer. Audible delivery confirmed by the engine decorrelate
  test; final DAW A/B is the user's check (below).
- [x] CI test parses all factory TOMLs and rejects any slot the coherence
  predicate flags (`no_factory_preset_routes_incoherently`) — the durable guard.
- [x] Per-dest end-to-end + depth-0 bit-identity tests land with their wiring
  tickets and all pass: `lfo2-phase` (0091: decorrelate + off-path lock),
  `lfo1/lfo2-rate` (0092: log sweep + gated-unity), `stack-detune/spread` (0093:
  phase_inc shift + bit-identity), `pitch-eg` units (0094: no double-scale).
  Coherence grid in 0090.
- [x] Gated-cost bench (`vxn2-osc-bench/benches/matrix_gated.rs`): density-8,
  16-note, FX on — `baseline` 291 µs, `lfo2_rate_on` 291 µs, `stack_detune_on`
  292 µs (all ~18.3× RT). On-path cost within noise of baseline; off-path
  bit-identity proven by unit tests. `eval_dests` shape unchanged by 0094
  (normalization at the source-build site), so the vectorised lane loop holds.
- [x] No `pitch-eg → *_pitch` route exists in any factory preset, so the 0094
  unit fix needs no preset re-scale (verified in the audit).
- [ ] **DAW A/B (user's final check):** the two now-live routes in Mark II
  E-Piano change its sound vs the pre-epic (inert) rendering — audition and
  confirm they're musical (see audit). Not auto-claimed.

## Close-out audit

All 5 factory presets walked; every routed slot is **coherent** (the 0090
predicate returns `Ok` for all — confirmed by the CI guard). Note: a *coarser*
source into a *finer* dest (e.g. `mod-wheel` → `op-level`, `velocity`/`mod-env`
→ `op-level`) is **coherent** (it broadcasts) — only finer→coarser collapses.

Slots that changed behaviour this epic (were inert/dead, now live):

| Preset | Slot | Route | Old (pre-E008) | Action |
| --- | --- | --- | --- | --- |
| Mark II E-Piano | 1 | `voice-rand → lfo2-phase` (d 1.0) | cooked + dropped (dead) | **Keep** — now delivers the intended per-lane LFO2 phase scatter (shimmer), 0091. Latent intent. |
| Mark II E-Piano | 3 | `mod-wheel → lfo1-rate` (d 0.6) | cooked + dropped (dead) | **Keep** — now sweeps LFO1 +0…~2.4 oct as the wheel opens (0092). Musical "wheel speeds vibrato" gesture; inert at wheel 0. |

No preset routes to `lfo2-rate`, `stack-detune`, or `stack-spread` (those dests
are wired but unused by the factory bank). No `pitch-eg`-sourced slots, so the
0094 normalization changes no factory sound. Every other slot
(`lfo2 → global-pitch`, `voice-spread → op-pan`, `*-level` routes) was already
live and is unchanged.

## Notes

This ticket is where the epic's headline promise — "no dest silently
cooks-and-drops, no preset routes into a dead/incoherent target" — is actually
verified and locked with a regression test. It must land last because it depends
on every sound-affecting change being in place; the CI preset-coherence test is
the durable guard that keeps the matrix honest as new presets are added.
