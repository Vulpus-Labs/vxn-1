---
id: "0062"
title: "Wire amp_sens_coef into per-op level modulation"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Second ticket of [E006](../../epics/open/E006-review-remediation.md).
The review found `amp_sens_coef` is cooked at note-on in both paths —
[op.rs:141](../../crates/vxn2-dsp/src/op.rs#L141) and
[stack.rs:608](../../crates/vxn2-dsp/src/stack.rs#L608) — but never
read by `op_tick`, `stack_tick_stereo`, or `stack_tick_mono`. The
`AmpSens` CLAP param (`op{N}-amp-sens`) is silently inert: routing
LFO1 → op level in the matrix produces identical tremolo at
`AmpSens = 0` and `AmpSens = 7`.

Wire the coefficient in as the per-op gate on incoming level
modulation: the matrix's op-level destination value is scaled by
`amp_sens_coef` before it reaches the op's output gain, so
`AmpSens = 0` means "this op ignores level modulation" and higher
values increase its receptivity, DX7-style.

## Where

- `stack.rs` — the hot path. `op_level_mod` (per-lane level-mod input
  populated from the matrix dest buffer) gets multiplied by the op's
  `amp_sens_coef` either at the point the engine writes it into the
  stack or inside the tick where the level-mod term is applied. Prefer
  the write site (`Engine::process_block` → stack field) if it keeps
  the NEON lane loop untouched — check the asm afterwards per the
  SoA-match lesson (runtime branch in the lane loop kills
  vectorisation).
- `op.rs` — same multiply in the scalar reference path so
  `stack_routing_matches_scalar`-style cross-check tests stay valid.
- Confirm `tables.rs::amp_sens_coef`'s curve (0 → 0.0 gate … 7 → 1.0)
  matches the intended polarity; document the mapping in its doc
  comment.

## Acceptance criteria

- [ ] Engine integration test: default patch + matrix route
  LFO1 → op1-level at fixed depth; render N blocks at
  `op1-amp-sens = 0` and `= 7`. Assert the output amplitude-modulation
  depth differs (e.g. RMS variance over time near zero at 0, clearly
  non-zero at 7).
- [ ] Scalar/SoA parity: existing routing cross-check tests extended
  (or a new one added) covering a non-zero level-mod input with
  differing amp-sens per op.
- [ ] `cargo bench --package vxn2-osc-bench stack` shows no regression
  beyond noise; if the multiply landed inside the lane loop, the asm
  dump still shows `.4s` NEON ops (mind the ARM64 objdump mnemonic
  format — `.4s` sits on the mnemonic, not the operands).
- [ ] No remaining "cooked but unread" state: grep confirms every
  `amp_sens_coef` assignment has a matching read.

## Notes

Velocity sensitivity (`vel_factor`) already works and is separate —
don't conflate. The review also flagged that no test asserts AmpSens
audibility; ticket 0069's sweep test will guard the whole param table,
but this ticket should land its own targeted test regardless.

## Close-out (2026-06-10)

Wired at the engine write site (`Engine::process_block` projection into
`stack.op_level_mod`), per the ticket's preferred option — the NEON lane
loop in `stack_tick_*` is untouched, so no asm/bench risk (stack bench
unaffected by construction; nothing in the per-sample path changed).

Deviations from the ticket text, with reasons:

- Param max is **3**, not 7 (`op{N}-amp-sens` is 0..3, DX7-style); the
  ticket's "= 7" examples read as 0..7 vel-sens conflation.
- "Same multiply in the scalar reference path": not applicable — the
  scalar voice path (`op_tick` / `voice.rs`) has no matrix level-mod
  input at all, so there is nothing to gate. `OpState.amp_sens_coef`
  (cooked-but-unread) was **removed** instead, which is what satisfies
  the "no cooked-but-unread state" criterion; `StackOp.amp_sens_coef`
  is now read by the engine.
- Default patch sets `op2-amp-sens = 3` so the shipped (depth-0)
  Velocity → Op2Level route works when dialed up; with the gate in
  place an op at the default sensitivity 0 ignores level mod entirely.

Tests: `amp_sens_gates_matrix_level_modulation` (op_level_mod zeroed at
0, passes at 3, output waveforms diverge >5% of signal energy);
`matrix_lfo1_to_op_level_modulates_audio` updated to open the gate.
