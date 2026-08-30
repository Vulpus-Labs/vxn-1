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
per-voice loop — its own comment notes these are pure functions of the patch
that were being re-run per lane per block (ignore that comment's "32-lane"
figure; the bank is 8 lanes — the point stands, the number is stale). vxn-2
already hoists `cook_depth` to table-rebuild time but redoes the sentinel
checks, zero-depth skip and `DEST_GAIN` lookup on every eval.

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

- Cook sites, confirmed 2026-08-30: vxn-2's `MatrixSlot` carries `depth`
  already cooked by `dest.cook_depth` at table-rebuild time (engine.rs:857);
  vxn-1b folds `cook_depth · DEST_GAIN` into `Route.gain` at
  `RouteList::compile` (eval.rs:375–390). Both are per-block — only the *site*
  differs. The shared `RouteList::compile` must take **raw** depths and do the
  cooking itself, and vxn-2's rebuild must stop pre-cooking when it adopts it —
  cooked-twice is the quiet bug here.
- Slot-semantics mismatch, also confirmed: vxn-2 folds `active` into
  `source = None` at rebuild (engine.rs:841–847), so its evaluator never checks
  the flag; vxn-1b's `Route` keeps it live. The shared compile step picks one
  convention and the parity/golden tests prove both synths drop a switched-off
  route identically.
- vxn-2 sources depths from two places — slots 0–7 read the CLAP-automatable
  `mtx_depths`, 8–15 the row's own depth. That mapping stays synth-side,
  outside the shared table.
- Out of scope: the evaluator itself ([0334](0334-share-the-evaluator.md)).
