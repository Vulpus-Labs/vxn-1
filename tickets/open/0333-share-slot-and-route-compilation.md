---
id: "0333"
product: monorepo
title: "Share MatrixSlot/MatrixTable and RouteList precompilation"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0330", "0332"]
---

## Summary

Move the slot and table types, and vxn-1b's per-block route compilation, into
`vxn-core-matrix`. vxn-2 adopts the precompilation it currently lacks.

Both synths already have the same slot: source, dest, depth, polarity, shape,
enabled, scale source, scale shape — with `is_active()` = switched on **and**
both endpoints real, and `is_wired()` for the "has the player set this up"
question that persistence asks.

vxn-1b additionally compiles a `RouteList` once per block, hoisting the sentinel
checks, the zero-depth skip, `cook_depth` and the `DEST_GAIN` lookup out of the
per-voice loop — its own comment notes a 32-lane synth was otherwise running
them 32 times a block. vxn-2 redoes that work on every eval.

## Design

`RouteList::compile(&table)` becomes the single entry point, generic over the
roster. Slot order is preserved, and that is the load-bearing contract: dests
accumulate additively, float addition is not associative, so "same routes in the
same order" is what keeps any two evaluator paths bit-exact.

The `enabled` switch is honoured **at compile time**, not per lane — a
switched-off route never reaches a lane loop, exactly like an unwired one. Note
that both `RouteList::compile` and the scalar evaluator must drop on the same
predicate: they diverged during 868faef (the bank path honoured `enabled`, the
scalar path did not) and the parity test caught it. Sharing the compile step is
what makes that class of bug structurally impossible rather than merely tested.

## Acceptance criteria

- [ ] `MatrixSlot`, `MatrixTable`, `Route`, `RouteList` live in
      `vxn-core-matrix`; neither synth defines its own.
- [ ] vxn-2 evaluates from a compiled `RouteList` rather than walking raw slots.
- [ ] `is_active` / `is_wired` semantics preserved exactly, including that
      persistence writes `is_wired` slots (a switched-off route keeps its
      wiring) and that vxn-1b's `ensure_pan_route` seeds into `!is_wired` slots
      so it cannot evict a parked route.
- [ ] Null test against the pre-ticket render passes: difference peak
      ≤ −100 dBFS (see [E049](../../epics/open/E049-shared-matrix-routing.md)
      §"The bar"). Re-capture the render hashes if they moved, and say so.
- [ ] `matrix_eval_full` improves or holds on vxn-2 — the precompilation is
      supposed to be free or better. If it regresses, say why in the close-out.

## Notes

- vxn-2's `MatrixSlot` carries `depth` already cooked by `dest.cook_depth`
  at snapshot time while vxn-1b cooks inside `slot_topology_gain`. Check which
  before assuming they compose the same way — this is exactly the sort of
  quiet difference that survives a "these look identical" reading.
- Out of scope: the evaluator itself ([0334](0334-share-the-evaluator.md)).
