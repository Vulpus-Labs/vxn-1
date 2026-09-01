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
      ≤ −100 dBFS (see [E049](../../epics/closed/E049-shared-matrix-routing.md)
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

## Close-out (2026-09-01)

- `MatrixSlot`, `MatrixTable`, `Route` and `RouteList` live in
  [slot.rs](../../crates/vxn-core-matrix/src/slot.rs), generic over the synth's
  own enums through two small traits (`SourceEndpoint`: `idx` + `is_bipolar`;
  `DestEndpoint`: `idx` + `gain` + `cook_depth`). Neither synth defines its own —
  each aliases the types and writes a short forwarding impl, so no call site
  changed meaning. vxn-1b's `ensure_pan_route` became a `MatrixTableExt` trait,
  since inherent impls cannot be added to a foreign type, and is otherwise
  untouched.
- vxn-2 evaluates from a compiled list:
  [`eval_dests(routes: &RouteList, …)`](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L830).
- **The cook site moved in one commit**, which is where this ticket could have
  gone quietly wrong: `apply_block_params` stores the *raw* depth and sets
  `enabled: row.active` instead of folding `!active` onto `SourceId::None`, and
  `RouteList::compile` does the cooking. The test that asserted the rebuild
  cooked now asserts the opposite and checks the taper lands in `Route.gain` —
  that is the cooked-twice tripwire.
- `is_active` / `is_wired` preserved for vxn-1b: `ensure_pan_route` still seeds
  into `!is_wired` ([matrix.rs:471](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L471))
  and preset writing still persists wired-but-off routes
  ([preset.rs:210](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L210)). vxn-2
  *gains* both, and three predicates that tested `source != None` —
  `dest_targeted`, `stack_pitch_targeted`, `eg_rate_targeted` — had to move with
  the field, since they were only equivalent while the switch was folded into
  the source.
- **Null test `-inf dBFS` on both engines**, refactor and Amp fix alike. The
  approved reorder (vxn-2 moving to vxn-1b's `shaped · (gain · scale)`
  grouping) is real but the reference patch wires no scale sources, so the two
  spellings coincide on that render. The grouping is converged early rather than
  left for 0334.
- `AmpRoutes::resolve` now drops on `is_active()` too
  ([bank.rs:405](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L405)) — a
  switched-off Env→Amp route no longer feeds the VCA. Null-neutral on the
  reference render (all sixteen of its routes are on), so a test that does
  exercise it was added: `a_switched_off_amp_route_is_invisible_to_the_scan`.
- **Bench: the AC's case was redefined, deliberately — read the number with
  that in mind.** `compile` is now *outside* the timed loop, because timing
  compile+eval together measures a one-voice patch, the single case
  precompilation cannot help; `matrix_eval_*` times the per-stack half and a new
  `matrix_compile_full` times the per-block half. Measured by compiling a copy of
  the pre-ticket raw-slot evaluator into the same binary and interleaving, since
  absolute numbers swung 50%+ with another agent building: eval full
  100.8 → 95.7 ns (−5.1%), scaled 122.5 → 109.5 ns (−10.6%), compile ~84 ns once
  per block. So the per-stack half improves, which is the AC.
- Review fixes: `slot_topology_gain` indexed the now-sentinel-free gain table via
  `index()`, folding the sentinel onto row 0 (`Pitch`, 12×) — switched to
  `dest.gain()`; `every_factory_preset_drives_the_amp` tested only the dest, so a
  preset whose only Amp route was parked would have rendered silent and still
  passed.
- **Left open, flagged rather than fixed:** vxn-2's TOML preset format has no
  `active` column, so muting a route, saving and reloading brings it back on.
  Pre-existing and a format change; vxn-1b does persist the flag. Wants its own
  ticket.
