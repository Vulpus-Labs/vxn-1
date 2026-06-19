---
id: "0074"
product: vxn-2
title: Per-operator phase-offset parameter
priority: medium
created: 2026-06-19
epic: E023
---

## Summary

Add a continuous per-operator phase-offset parameter, applied at note-on, so
the six carriers of algorithm 32 can sum into specific analytic waveforms
(saw needs a π flip on even harmonics; square is phase-0). Today the only
phase control is `StackParams.phase`, which writes the *same* per-lane
decorrelation offset to all six operators — there is no way to set one
operator's phase relative to another. The new per-op offset composes
additively with the existing per-lane offset.

## Acceptance criteria

- [ ] `OpParams` gains a `phase` field (op.rs:36), continuous, stored as a
      fraction in [0,1) (one full cycle = 1.0).
- [ ] Host param table exposes per-operator phase (PARAMETERS.md updated;
      one slot per op).
- [ ] At note-on, `apply_phase_offsets` (stack.rs:684) sets
      `op.phase[k] = lane_offset[k] + op_offset[i]` as a wrapping Q32 add, so
      per-lane decorrelation and per-op shape offset stack.
- [ ] Conversion uses `PM_SCALE_Q32` (2^32) consistently; `frac × 2^32`.
- [ ] No hot-loop cost — offset is applied only at note-on, not per sample.
- [ ] Changing an op's phase offset visibly shifts the summed waveform on a
      scope; with all offsets at 0 the behaviour is unchanged from today.
- [ ] Supersaw width still responds to `StackParams.phase` independently of
      the per-op offsets.

## Notes

- Continuous, not quantized to `2π/N`: quantizing buys nothing in DSP and a
  coarse grid would collide lanes at low density. Shape ergonomics (snap to
  0/¼/½/¾) belong in the UI as detents on a continuous param, not in the
  param domain. See E023 background.
- Precondition already met: the stack (production) path resets phase at
  note-on, so the offset persists rather than washing out. The scalar
  reference path (op.rs) does not reset — keep this a stack-path feature.
- Single-voice steady timbre is phase-deaf (magnitude spectrum unchanged), so
  this is not audible in isolation for most patches. It matters for: correct
  saw time-domain shape, an operator used as an FM modulator (downstream is
  nonlinear), attack transient, and as raw material for 0075.

## Implementation status (code complete)

- `OpParams.phase` (op.rs), fraction `[0,1)`, default 0.0.
- CLAP param `op{n}-phase` appended as the trailing float of each op block
  (`N_PER_OP` 21 → 22, `TOTAL_PARAMS` 189 → 195). Existing per-op read offsets
  unchanged; `read_op` reads `phase: f(21)` (ratio-mode stays index 20).
- `apply_phase_offsets` (stack.rs) now takes `&[OpParams; N_OPS]` and sets
  `op.phase[k] = lane_offset[k].wrapping_add(op_offset_q32)` — per-lane
  decorrelation + per-op shape offset compose by wrapping Q32 add. `frac
  .rem_euclid(1.0) * PM_SCALE_Q32`.
- Host-state blob migration: v11 (`BLOB_VERSION` bump). v≤10 blobs spread the
  op blocks (`migrate_v10_id`, later ids +N_OPS) and seed the six new phase
  slots to 0.0 → an unchanged patch stays bit-identical. New test
  `load_bytes_migrates_v10_param_layout`; v3/v4/v5/v6 migration tests still pass
  (legacy anchors re-based for the second op-block spread).
- UI: `op{n}-phase` fader added to the op-detail Output column (op-row.js).
- PARAMETERS.md updated. Excluded from the min→max audibility sweep (cyclic:
  min 0.0 ≡ max 1.0 = one full cycle), with a dedicated stack.rs test
  (`per_op_phase_shifts_starting_phase_and_waveform`,
  `per_op_phase_composes_with_lane_decorrelation`) proving intermediate offsets
  change the waveform.
- Stack-path only; the scalar reference path (op.rs/voice.rs) does not reset
  phase at note-on, so the offset is intentionally a stack feature.
