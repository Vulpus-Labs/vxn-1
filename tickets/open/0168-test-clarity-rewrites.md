---
id: "0168"
product: monorepo
title: Rewrite buried-setup tests so the asserted property is legible
priority: low
created: 2026-07-01
epic: E031
---

## Summary

A handful of tests hide their asserted property under hand-rolled window
arithmetic, long `if`-fallthrough context setup, or trait-impl boilerplate.
The assertions are sound; the scaffolding obscures them and, in one case,
lets a wrong setup branch silently pass a severed param. Rewrite so "what
is asserted" is separate from "where/how it's set up."

Line numbers are as-reviewed on 2026-07-01; re-grep by name.

## Acceptance criteria

- [ ] `vxn2-engine/src/engine.rs` `default_patch_renders_with_expected_
      envelope` (~1990) — replace the stateful `render_and_rms` closure +
      `blocks_so_far`/`saturating_sub` window bookkeeping with: render the
      whole timeline into one buffer once, then slice named windows by
      sample index. The three asserts (attack in [-24,-8] dBFS, decaying
      tail, tail ≤ -45) should read plainly.
- [ ] `vxn2-clap/tests/smoke.rs` `default_patch_render_one_note` (~222) —
      the manual first-block/`render_seconds`/note-off/tail stitching (~237–
      274) obscures the three properties. Fold note-on/note-off into event-
      injected variants of the existing `render_seconds` helper so the body
      reads as "render 1s held, render 4s after release."
- [ ] `vxn2-engine/tests/param_audibility.rs` `context_override` (~139–357)
      — replace the 220-line positional `if name.starts_with(...)`
      fallthrough with a `[(matcher, fn(&SharedParams) -> Capture)]` table
      so each param's context is a named, greppable entry and a missing
      branch is visible rather than a silent pass.
- [ ] `vxn2-dsp/src/dynamics.rs` `detector_resets_on_inactive_to_active_
      edge` (~498) — factor the "drive env high, fade off, settle" three-
      phase setup into a helper so the two-line reset assertion
      (`detector_env() == 0.0`) stands out.
- [ ] `vxn-app/tests/controller.rs` `step_preset_spans_factory_into_user`
      (~891) — the inline 12-method `MixedStore impl PresetStore` buries the
      forward-step-crosses-boundary scenario. Resolved by the PresetStore
      consolidation in 0166 (reuse the hoisted configurable store with a
      factory seed); this ticket just confirms the scenario reads clearly
      afterward.

- [ ] `cargo test` green; assertions and tolerances unchanged, only the
      setup restructured.

## Notes

`param_audibility`'s `context_override` is inherent to the sweep design and
well-commented, so it is borderline — but it is the densest hidden-setup
site in vxn-2 and a wrong branch silently makes a severed param "pass,"
which defeats the sweep. The table rewrite is the highest-value item here.
Lowest priority in the epic; do after the apparatus tickets so helpers
exist to lean on.
