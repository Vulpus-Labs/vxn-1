---
id: "0119"
product: vxn-2
title: vxn2-engine — extract cook_stacks_block, collapse parallel ramp Vecs
priority: high
created: 2026-06-23
epic: E027
---

## Summary

Two structural fixes in `vxn2-engine/src/engine.rs`. The
#1 surgery hotspot for any new mod-dest / FX / routing
feature. Behaviour-preserving.

1. **`process_block` is 530 lines, 12 sequential stages**
   (`engine.rs:604-1134`). It serially covers block-rate
   control, LFO1/2-rate gating, the mod-matrix loop over all
   16 stacks with inline ramp computation (678-1033),
   FX-dest aggregation, filter coefficients, pitch-smoother
   dispatch, and the OFF-path render loop. Extract the inner
   stack-matrix loop (678-1033) into
   `cook_stacks_block() -> StackBlockSummary` (ramp_live, FX
   sums, lfo1 oct). That alone removes ~350 lines.

2. **`Engine` carries five lockstep parallel ramp `Vec`s**
   (`engine.rs:160-304`): `level_mod_inc`, `pan_l_inc`,
   `pan_r_inc`, `phase_mod_inc`, `prev_eg_level`. Each must
   be indexed in step in the matrix loop, in `reset`
   (`:431`), and in `advance_mod_ramps` /
   `advance_mod_ramp_one`. Adding one ramp type = five
   parallel edits. Collapse into a single
   `Vec<RampState>` newtype (one struct per stack) with
   `level_mod` / `pan_l` / `pan_r` / `phase_mod` fields.

The matrix loop also has a strict 12-stage ordering whose
intent is documented only in a `:649-666` comment; a reorder
would inject a one-block-latency bug invisible to current
tests.

## Acceptance criteria

- [ ] The 16-stack matrix loop is extracted into
      `cook_stacks_block` returning a summary struct;
      `process_block` shrinks by ≥300 lines and reads as a
      sequence of named stage calls.
- [ ] `level_mod_inc` / `pan_l_inc` / `pan_r_inc` /
      `phase_mod_inc` / `prev_eg_level` are replaced by one
      `Vec<RampState>`; `new`, `reset`, the cook loop, and
      `advance_mod_ramps` index that single Vec. Adding a new
      ramp field touches only `RampState`.
- [ ] A `// STAGE N: <name>` marker precedes each of the 12
      stages and a block-comment table at the top of
      `cook_stacks_block` lists all 12 with their ordering
      constraints.
- [ ] `cargo test -p vxn2-engine` green; `tests/baseline.rs`
      render hash unchanged (proves no stage reordering or
      ramp-index regression).

## Notes

Pure refactor — no per-sample overhead may be added to the
hot loop; `cook_stacks_block` must inline / not allocate per
block (reuse the existing scratch Vecs). The OFF-path render
loop (`:1090-1118`) and the ON-path
`render_block_filtered` are already split and out of scope
here beyond sharing a final FX-tail fn if convenient. Stack
SoA layout is load-bearing for NEON (memory
`vxn2-stack-soa`) — do not change field layout, only group
the ramp scalars.
