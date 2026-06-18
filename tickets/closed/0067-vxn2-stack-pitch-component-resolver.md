---
id: "0067"
product: vxn-2
title: "Wall-aware connected-component resolver over the algo graph"
priority: medium
created: 2026-06-18
epic: E022
depends: []
---

## Summary

First ticket of [E022](../../epics/open/E022-vxn2-stack-pitch-mod.md). Add a
**pure function** that, given an algorithm and the set of fixed-frequency
ops, returns the connected component of a target op in the modulation graph.
This is the core of stack pitch mod — every later ticket consumes it.

## Design

- **Lives in dsp**, next to the graph it reads:
  [algo.rs](../../vxn-2/crates/vxn2-dsp/src/algo.rs). `ALGOS[a].edges` are
  the modulator→carrier wires; treat them **undirected** for connectivity.
- **Signature** (pure, allocation-free, `const`-friendly):

  ```rust
  /// 6-bit op mask. `wall_mask` bit i set = op (i+1) is a wall (fixed-freq):
  /// it is excluded from the result AND traversal cannot cross it.
  pub fn pitch_stack_component(algo: u8, wall_mask: u8, target_op: u8) -> u8;
  ```

  The freq-mode → `wall_mask` translation belongs to the engine cook
  (ticket 0069); dsp stays oblivious to params. Keep this fn graph-only.

- **Algorithm.** Build adjacency from `edges` (both endpoints), delete
  walled nodes, then flood-fill from `target_op` over surviving nodes.
  Return the visited set as a 6-bit mask.
- **Edge cases:**
  - `target_op` itself walled → return `0` (empty). Stack route no-ops; the
    caller decides UI hint vs silent (E022 design: no-op).
  - Isolated op (no edges, e.g. a pure carrier in a 1-op branch) → returns
    just itself.
  - Self-feedback (`AlgoSpec.fb`) is structural and irrelevant to
    connectivity — ignore it here; it is not a modulation edge.
- **Determinism / SoA-safety.** No allocation, no float — pure integer
  graph walk. Safe to call from the cook path.

## Acceptance criteria

- [ ] `pitch_stack_component(algo, wall_mask, target)` implemented in
      `vxn2-dsp`, pure + allocation-free.
- [ ] Walls excluded from the result *and* block traversal (a fixed op
      mid-chain splits the component).
- [ ] Walled target → empty mask.
- [ ] Unit tests over hand-verified algos covering: a simple linear stack
      (e.g. algo 1's `6→5→4→3`), a shared modulator fanning into multiple
      carriers (component pulls in both carriers), a wall mid-chain (two
      sub-components), and walled-target.
- [ ] Tests assert against the published DX7 chart comments already in
      `algo.rs`, not just the code's own output.

## Notes

- Cross-reference the per-algo carrier-mask validation tables already in the
  dsp module's tests — reuse that fixture style.
- No engine/param/UI changes in this ticket; it is a leaf utility.

## Close-out (2026-06-18)

- `pitch_stack_component(algo, wall_mask, target_op) -> u8` added to
  [algo.rs](../../vxn-2/crates/vxn2-dsp/src/algo.rs) — pure `const`,
  allocation-free undirected flood-fill over `ALGOS[].edges`; saturates
  out-of-range algo to 1, out-of-range/walled target → `0`.
- Walls excluded from result *and* sever traversal (an edge touching a wall is
  never crossed), so a fixed op mid-chain splits the component.
- Walled target → empty mask (`component_walled_target_empty`).
- Tests over hand-verified algos: linear stack algo 1 `6→5→4→3`
  (`component_linear_stack`), shared modulator algo 22 fan-out
  (`component_shared_modulator_spread`), wall mid-chain
  (`component_wall_splits_chain`), walled target, isolated op (algo 32), plus
  an exhaustive walls-excluded sweep over all 32 algos
  (`component_excludes_walls_exhaustive`). Assertions reference the published
  DX7 chart comments in `algo.rs`, not just code output. `algo::tests::component_*`.
