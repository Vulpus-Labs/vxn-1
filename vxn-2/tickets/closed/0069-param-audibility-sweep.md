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

## Close-out (2026-06-13)

New test `crates/vxn2-engine/tests/param_audibility.rs`.

**Mechanism.** Each param is rendered twice under a deterministic context (two
fresh engines, identical note script, fixed RNG seeds) — param at min, param at
max — capturing the full L+R buffer across attack → sustain → release. A
relative-L1 fingerprint compares them; a *severed* param yields a bit-identical
pair (rel-diff exactly 0), a *wired* one moves the output. Threshold 1e-4 (the
smallest real effect — a transient EG attack-stage level — sits at ≈3e-4, an
order of magnitude above; severed = 0).

**Context.** Rich base patch (algo 32 = six parallel carriers, eight active
matrix routes, FX engaged, moving EG/mod-env/pitch-EG) makes most params
audible; `context_override` handles the stragglers — Fixed-mode for op fixed-hz,
side-isolated KS with distant notes, zig-zag EGs reaching every stage, a
VoiceSpread route for `stack-spread` (it is the gain on that source, not a
direct knob), stereo panning for `delay-pingpong` (a no-op on a mono sum),
filter-in-circuit for filter params, long windows for FX tails / LFO2 fade.

**Acceptance.**
- Enumerates the full 188-param table (TOTAL_PARAMS) — a new param with no
  context fails rather than passing silently.
- Teeth verified by hand: forcing `depth = 0.0 * mtx_depths[s]` in engine.rs
  made it fail (mtx1-8-depth, stack-spread, + the LFO2/mod-env params routed
  through those slots), passing again on restore. Documented in the test.
- Exclusions = 4 (≤10), each with a reason: `assign-mode`, `legato`,
  `glide-time` (all need overlapping-note scripts), `filter-cutoff-tuned`
  (UI-only — engine never reads it, shared.rs `read_filter`).
- Fast run ≈ 9 s (under the ~10 s budget); `#[ignore]`d 3× thorough variant
  also green. CI wiring is ticket 0070's scope.

Deferred matrix dests (`Lfo2Phase`/`Lfo1Rate`/`Lfo2Rate`/`StackDetune`/
`StackSpread`) are dest-enum entries, not params (and now wired via E008), so
they don't trip the sweep — noted in the test header.
