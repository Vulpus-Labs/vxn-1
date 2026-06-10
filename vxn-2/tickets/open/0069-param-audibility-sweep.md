---
id: "0069"
title: "Param audibility sweep test: every param must do something"
priority: high
created: 2026-06-10
epic: E006
depends: ["0061", "0062", "0063"]
---

## Summary

Ninth ticket of [E006](../../epics/open/E006-review-remediation.md).
The review's headline pattern — `lfo1-depth`, `AmpSens`,
`PitchSmoother` all structurally complete, functionally inert — has a
mechanical guard: an integration test asserting that **every param,
swept min → max under a patch where it should matter, changes the
output**. Three of the five review bugs would have been caught by it.

Lives next to the existing
[param_sweep.rs](../../crates/vxn2-engine/tests/param_sweep.rs) (which
checks finiteness, not effect).

## Design

- For each param id: render N blocks at min, render at max, compare a
  cheap fingerprint (RMS + spectral centroid, or block-wise sample
  diff) — assert the fingerprints differ beyond epsilon.
- The hard part is the **per-param context**: many params are inaudible
  under the default patch (a modulator's level when its algo doesn't
  route it; delay-feedback with delay-mix at 0). The test needs a small
  table of context overrides:
  `param → (patch tweaks, note, render length)` that put the param in
  a position to matter. Build the table incrementally — start with
  everything under a context-rich patch (algo 1, matrix routes from
  LFO1/ModEnv to pitch+level, FX mixes up) and add overrides for the
  stragglers.
- Some params legitimately need exclusion (e.g. `assign-mode` needs a
  note-overlap script, not a sweep; sync toggles change display not
  sound when rate maps to the same value). Keep an explicit
  `EXCLUDED: &[(&str, &str)]` list — **name + reason string** — so an
  exclusion is a documented decision, not a silent skip. Review goal:
  the excluded list stays short and every entry is justifiable.
- Runtime budget: keep the default-run variant under ~10 s (short
  renders, coarse fingerprint); a `#[ignore]`d thorough variant can
  render longer.

## Acceptance criteria

- [ ] Test enumerates the full param table (173 post-0073) — new
  params are swept automatically; adding a param without audibility
  context fails the test rather than passing silently.
- [ ] Test fails when any wired param is severed (verify once by
  hand: comment out a matrix projection multiply, watch it fail,
  restore).
- [ ] Exclusion list ≤ ~10 entries, each with a reason.
- [ ] Runs in CI (ticket 0070) within budget.

## Notes

Depends on 0061/0062/0063 because the sweep fails against the current
inert params — landing it first would mean landing it with exclusions
that the epic exists to remove. The deferred matrix destinations
(`Lfo2Phase`, `Lfo1Rate`, `Lfo2Rate`, `StackDetune`, `StackSpread`)
are dest-enum entries, not params, so they don't trip this test — but
note them in the test header as known-inert UI surface.
