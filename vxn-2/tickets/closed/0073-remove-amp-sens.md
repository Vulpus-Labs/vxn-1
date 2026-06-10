---
id: "0073"
title: "Remove AmpSens — matrix slot depth is the only level-mod attenuator"
priority: medium
created: 2026-06-10
depends: ["0062"]
---

## Summary

Reverses the design 0062 wired in. AmpSens was a per-op receive gate on
incoming mod-matrix level modulation, defaulting to 0 — so any
LFO → OpNLevel route was silently inert until the user also found and
opened the gate (observed in practice: LFO routed to a carrier's level,
no tremolo, no hint why). The gate adds nothing the matrix doesn't
already provide:

- per-route strength is the slot's `depth`;
- velocity → level has its own per-op `vel_sens` path, cooked at
  note-on, independent of the matrix.

Unlike the DX7 (where AMS gated only the single global LFO's AMD), our
gate sat on *all* matrix sources targeting op level — broader than the
hardware precedent and redundant with slot depth. Removed rather than
defaulted open.

## Changes

- `vxn2-dsp`: drop `OpParams::amp_sens`, `StackOp::amp_sens_coef`,
  `AMP_SENS_TABLE` / `amp_sens_coef()`; matrix level dests now project
  into `op_level_mod` unscaled.
- `vxn2-engine`: drop the six `opN-amp-sens` params — `N_PER_OP`
  21 → 20, `TOTAL_PARAMS` 179 → 173; remove the gate at the
  `op_level_mod` write site; default patch no longer opens op 2's gate.
- Host-state blob bumped to v5; v≤4 blobs migrate on load by dropping
  the stored amp-sens values and compressing the op-block ids
  (`migrate_v4_id`). Migration test renders a v4 blob and verifies
  bit-exact landing either side of the removed slots.
- `vxn2-ui-web`: AMS fader removed from the op row's Sensitivity group.
- The 0062 regression test `amp_sens_gates_matrix_level_modulation` is
  deleted; `matrix_lfo1_to_op_level_modulates_audio` still covers the
  route end-to-end (and no longer needs the gate opened).

## Acceptance criteria

- [x] LFO1 → OpNLevel at non-zero depth produces tremolo on a carrier
  with no other setup.
- [x] Workspace tests pass; param-count asserts and PARAMETERS.md
  totals updated (173).
- [x] v4 (and v≤3) blobs load with all surviving values bit-exact.
