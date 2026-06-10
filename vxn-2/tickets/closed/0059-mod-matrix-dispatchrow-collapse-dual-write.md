---
id: "0059"
title: "mod-matrix.js dispatchRow: collapse dual-write per slot range"
priority: medium
created: 2026-06-10
epic: E005
depends: ["0058"]
---

## Summary

Fifth ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
The current `dispatchRow` in `mod-matrix.js` fires both `set_matrix_row`
AND `set_param mtxN-depth` for a depth-only edit on slots 1-8. With
the pump unifying Model → View, the rationale for the dual-write
collapses: pick one opcode per slot range and document.

Per [ADR 0003](../../adrs/0003-dirty-bitset-diff-pump.md): slots 1-8
fire `set_param` only (rides CLAP gesture brackets + host echo path);
slots 9-16 fire `set_matrix_row` only (no CLAP id exists). Topology
edits (source, dest, curve, active) always fire `set_matrix_row`
regardless of slot range.

## Acceptance criteria

- [ ] `dispatchRow(slot, partial)` in
  `vxn2-ui-web/assets/panels/mod-matrix.js`:
  - If `partial.depth != null` and no topology field changed: fire
    `set_param { id: mtxN-depth, plain: depth }` for slots 1-8;
    fire `set_matrix_row { slot, row }` for slots 9-16. **Not both.**
  - If any topology field changed (`partial.source`, `partial.dest`,
    `partial.curve`, `partial.active`): fire `set_matrix_row`
    regardless of slot index. Depth rides along inside `row`. (For
    slots 1-8 the engine's `set_matrix_row_raw` already mirrors depth
    into the CLAP `values[OFF_MTX + slot]`; no second dispatch
    needed.)
- [ ] The "optimistic local update" remains: write `vxn.matrix.rows[slot]`
  before dispatching, so the UI doesn't flash between user input
  and the bitset pump's next-tick echo.
- [ ] Document the chosen rule in `dispatchRow`'s leading comment.
  The asymmetry (depth via CLAP for slots 1-8, depth via custom for
  slots 9-16) is intentional and reflects which fields the host
  knows about. Document why.
- [ ] No regression in matrix gesture bracketing — slot 1-8 depth
  drags still emit `gesture_begin` / `gesture_end` to the host (that
  happens via the `set_param` path, which the existing primitive
  bind already handles).
- [ ] Manual test (Reaper):
  - Drag slot 1 depth — the host receives a single CLAP param event
    bracketed by `gesture_begin` / `gesture_end`, not two events.
  - Edit slot 9 depth — the host receives no CLAP event (depth lives
    outside CLAP for slots 9-16); the engine receives the change via
    `set_matrix_row`.
  - Change slot 1 source — one `set_matrix_row` dispatch, the engine
    + UI both update on the next tick.
  - Change slot 1 source AND depth in one widget event (e.g. a
    paste / preset preview that mutates several fields together):
    fire `set_matrix_row` once with the full row; don't double-fire
    a `set_param` chaser.
- [ ] `cargo test -p vxn2-ui-web` green (the existing
  serialise/parse tests still hold — only the dispatch pattern from
  the page changed, not the wire format).

## Notes

This is purely a page-side change. Engine + Controller already accept
both opcodes; nothing has to be added. The win is fewer redundant
round-trips per drag tick and a cleaner mental model: "this opcode
writes this part of the Model."

Edge case: if the user starts a depth drag on slot 1, slot 1 source
gets changed by host automation mid-drag (theoretical), and the depth
release commits — make sure the depth commit doesn't accidentally
write a stale source back through `set_matrix_row`. The cleanest fix
is the rule above: depth-only edit → `set_param` only, never touches
topology. (Topology arrives in the next-tick pump echo.)
