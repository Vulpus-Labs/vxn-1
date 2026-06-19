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
