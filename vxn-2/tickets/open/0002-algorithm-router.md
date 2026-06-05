---
id: "0002"
title: Algorithm router (32 algos, carriers, modulators, FB)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

Implement the 32 DX7-canonical algorithm graphs as a data table plus a
per-algorithm router that, given six operator outputs at the previous sample,
returns the modulation input each op should receive at the current sample and
which ops sum to the carrier bus.

Algorithm is a single integer parameter (1..32). The router is a pure
dispatch function: no allocation, no branch on the hot path beyond the
per-algorithm match. With per-op feedback now patch-controlled (ADR §1), the
"FB op" field per algo only indicates which op the *algorithm's structural*
feedback path occupies — per-op FB is independent and orthogonal.

## Acceptance criteria

- [ ] `Algorithm` enum or const table for all 32 DX7 algorithms, capturing:
      modulator→carrier edges, carrier set, structural FB op.
- [ ] `route(algo, prev_outputs: [f32; 6]) -> ([f32; 6] modulation_in,
      f32 carrier_sum)` produces per-op modulation inputs and the summed
      carrier bus for the current sample.
- [ ] Router is branch-free across the 6-op loop body; the algo match is the
      only branch and is hoisted out via a per-algorithm specialised function
      (LUT of fn-pointers or a `match` resolved at the block level). Verify
      with an asm dump for at least 4 representative algos.
- [ ] Per-op `feedback` applies BEFORE the algorithm routing (each op
      receives its own previous output via the feedback term, summed with the
      structural modulation input from `route`).
- [ ] Reference table tested: a "ping" patch in algorithm N (all-zero envs
      except op N peaking briefly) produces audible output if and only if N
      is a carrier in that algorithm.
- [ ] No allocation; constant CPU per sample regardless of algorithm choice.

## Notes

DX7 algorithm reference: the standard 32-graph chart is widely available
(included as `ALGORITHMS.png` to be added in this ticket from a public
reference). The chart's graphical "blocks above blocks" notation maps
directly to our `edges: [[modulator, carrier], …]` list.

Be careful with algorithms that have two-deep modulation chains (e.g. algo 1
has op6 → op5 → op4 → op3). The router must produce all four ops' modulation
inputs in one pass — don't iterate the graph at sample rate.

The choice of per-op-FB *plus* structural-FB is the VXN2 extension and the
one place we deviate from DX7-faithful. Document this clearly in the router's
header comment so anyone consulting the DX7 chart understands the FB op
indicator is purely a structural marker now.
