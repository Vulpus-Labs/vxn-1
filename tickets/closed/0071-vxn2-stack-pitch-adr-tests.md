---
id: "0071"
product: vxn-2
title: "Stack pitch mod — ADR addendum + ratio-lock / audibility tests"
priority: medium
created: 2026-06-18
epic: E022
depends: ["0069", "0070"]
---

## Summary

Close E022 with the written rationale and the end-to-end tests that lock the
semantics in: ratio preservation, fixed-op walls, shared-modulator spread,
and the no-op edge case.

## Design

- **ADR addendum.** Append a section to
  [0001-vxn2-overall-design.md](../../vxn-2/adrs/0001-vxn2-overall-design.md)
  §6 (mod matrix) — or a short standalone ADR 0005 — recording:
  - Pitch-only by intent: ratio-lock is the entire point; level/pan stack
    mod is explicitly out ([[vxn2-architecture]]).
  - Equal-delta (no depth scaling) is required for ratio preservation.
  - Whole connected component, undirected; fixed-freq ops are connectivity
    walls, not just excluded outputs.
  - Shared modulators legitimately produce large components.
  - Resolver depends on `(algo, ratio-mode×6)`; re-cook on toggle only.
- **Tests** (integration, over the engine cook + a render):
  - *Ratio-lock:* harmonic patch, static stack-pitch offset → all branch op
    frequencies scale by one factor; FM ratios invariant.
  - *Wall split:* fixed op mid-chain → bend applies to one sub-component
    only; the far side is untouched.
  - *Shared-mod spread:* a shared modulator target pulls all its carriers
    into the bend (documented, asserted).
  - *No-op:* stack-pitch route on a Fixed target produces zero pitch change.
  - *Recook gating:* ratio-value change does not re-resolve; Ratio↔Fixed
    toggle does.

## Acceptance criteria

- [ ] ADR addendum (or ADR 0005) merged, covering the five points above.
- [ ] Integration tests for ratio-lock, wall-split, shared-mod spread,
      no-op, and recook gating all pass.
- [ ] [[vxn2-preset-system]] / blob round-trip unaffected (a stack-pitch
      route saves and reloads).
- [ ] Epic E022 checklist complete; ready for `/close-epic E022`.

## Notes

- Reuse the d4-style detector / audibility harness from
  [[vxn2-level-mod-pipeline]] for the audibility assertions if it fits.
- Update the [[vxn2-architecture]] memory's "mod matrix only routing" note
  if stack pitch changes how routing is described.

## Close-out (2026-06-18)

- Standalone [ADR 0005](../../vxn-2/adrs/0005-stack-pitch-mod.md) merged covering
  all five points: pitch-only by intent, equal-delta required, whole undirected
  component, fixed ops as connectivity walls, shared modulators → large
  components, and re-cook on `(algo, ratio-mode×6)` toggle only. Cross-linked
  from [ADR 0001 §6](../../vxn-2/adrs/0001-vxn2-overall-design.md).
- Integration tests over the engine cook + a render: ratio-lock
  (`stack_pitch_ratio_lock_preserves_fm_ratios`), wall-split
  (`stack_pitch_wall_splits_component`), shared-mod spread
  (`stack_pitch_shared_modulator_spreads_to_all_carriers`), fixed-target no-op
  (`stack_pitch_fixed_target_is_noop`), recook gating
  (`stack_pitch_masks_recook_gating`).
- Blob round-trip for a stack-pitch route unaffected
  (`shared::tests::snapshot_round_trips_stack_pitch_route`).
- Audibility note: ratio-lock test asserts a measurable per-op `phase_inc` shift;
  manual DAW listening check tracked in 0070, pending.
