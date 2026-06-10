---
id: "0055"
title: "SharedParams: add dirty bitsets + wire write sites"
priority: high
created: 2026-06-10
epic: E005
---

## Summary

First ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
Add the dirty-bitset infrastructure to `SharedParams` and route every
write site through it. This is the foundation: until every writer flips
a bit on every Model mutation, no reader can rely on the pump.

Per [ADR 0003](../../adrs/0003-dirty-bitset-diff-pump.md): every
`SharedParams` write follows the two-step pattern — store value
(`Relaxed`), then set the matching bit (`Release`). The reader (next
ticket) pairs with `swap(Acquire)`.

## Acceptance criteria

- [ ] `SharedParams` gains:
  - `dirty_values: [AtomicU64; N_DIRTY_VALUE_WORDS]` where
    `N_DIRTY_VALUE_WORDS = (TOTAL_PARAMS + 63) / 64`.
  - `dirty_matrix: AtomicU64` — one bit per matrix slot meta change
    (16 bits) plus optional headroom for future per-domain bits.
- [ ] `SharedParams::new` initialises both bitset fields to all-ones
  (or all-zeros + an explicit "first tick = full broadcast" trigger —
  pick one and document; all-ones matches the current NaN-seed
  full-broadcast semantics of `last_seen` and keeps the first tick
  behaviour identical).
- [ ] `SharedParams::set(id, v)` and `SharedParams::set_normalised(id, n)`
  perform `values[id].store(...)` followed by
  `dirty_values[id/64].fetch_or(1 << (id%64), Release)`. Out-of-range
  ids short-circuit before any atomic op (current behaviour preserved).
- [ ] `SharedParams::set_matrix_row_raw(slot, row)`:
  - Writes packed meta to `matrix_meta[slot]` (existing behaviour).
  - Sets bit `slot` in `dirty_matrix` via `fetch_or(Release)`.
  - For slots 1-8: the depth side-path still writes `OFF_MTX + slot`
    via `set()`, which already flips the matching `dirty_values` bit
    — verify the call order so both bits land.
  - For slots 9-16: writes `matrix_extra_depth` and sets the same
    `dirty_matrix` slot bit. (Extra depth doesn't need its own bit;
    a slot-meta change is the trigger to push a fresh
    `MatrixSnapshot`.)
- [ ] `SharedParams::reset_to_defaults` flips all relevant bits so the
  next tick re-broadcasts the full table.
- [ ] `SharedParams::load_bytes` (the blob loader) doesn't open-code
  atomic stores — it routes every value through `set` /
  `set_matrix_row_raw` style helpers OR explicitly flips bits after
  the bulk store. Either way the contract holds: state load flips
  bits, no bespoke push required from the caller.
- [ ] Public accessor `SharedParams::take_dirty_values(&self)` returns
  the bitset snapshot via `swap(0, Acquire)` per word. Similar
  `take_dirty_matrix(&self) -> u64`. Tests use these to assert what
  drifted.
- [ ] New tests in `shared.rs`:
  - Writing one id sets exactly one value bit; other bits stay zero.
  - Writing one matrix slot sets exactly one matrix bit; other bits
    stay zero.
  - `set_matrix_row_raw` on a slot 1-8 sets both the slot bit and the
    matching depth value bit.
  - `take_dirty_values` clears the bits — second call returns all
    zeros (no new writes in between).
  - `load_bytes` round-trip leaves both bitsets non-zero (state load
    is observable to the pump).
  - `reset_to_defaults` triggers a full re-broadcast (all bits set).
- [ ] No write site to `SharedParams.values` / `matrix_meta` /
  `matrix_extra_depth` bypasses the helpers — grep for direct
  `.store(` on these fields outside the helper functions; if any
  legitimate site exists (test fixtures), document it and confirm
  the dirty-bit contract still holds.
- [ ] `cargo build -p vxn2-engine` green.
- [ ] `cargo test -p vxn2-engine` green.
- [ ] `cargo bench -p vxn2-osc-bench` runs (no regression on the
  per-block render — the dirty bit is per-write, not per-block).

## Notes

The atomic ordering matters: writers use `fetch_or(Release)` so a
reader's `swap(Acquire)` followed by `get(id)` is guaranteed to see
the value that the writer stored before flipping the bit. `Relaxed`
on the value store is fine because the bit handshake provides the
synchronisation.

Memory layout: 3 × `AtomicU64` for values (192 bits, 180 ids) + 1 ×
`AtomicU64` for matrix. ~32 bytes total. Lives next to the existing
`gestures` bitset on `SharedParams`.

`take_dirty_*` is `&self`-receiver (atomic operations are interior
mutability); document that the swap is single-reader (main thread
only) so concurrent readers don't race each other. Writers are
unrestricted (multiple threads can `fetch_or` safely).

The "all-ones at init" initial state mirrors the `f32::NAN` seed in
`last_seen` today — first tick after open broadcasts the whole table,
hydrating the editor with current values. Document the parallel.
