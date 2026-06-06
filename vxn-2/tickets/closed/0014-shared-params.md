---
id: "0014"
title: SharedParams + LocalParams (atomic store, mirror)
priority: high
created: 2026-06-05
epic: E002
---

## Summary

The lockless parameter bridge between the main thread (CLAP `flush` +
later, the UI), the audio thread (`process`), and the host (`get_value`).
Implements the VXN1 idiom: `SharedParams` is an `Arc`-wrapped table of
atomics — one slot per CLAP-automatable param — and `LocalParams` is an
audio-thread mirror that diffs against it each block.

For E002 only host automation writes the store; the UI epic will plug in
UI-originated writes later. The diff / publish / emit machinery is wired
end-to-end now so the UI epic adds the consumer, not new plumbing.

Lives in `vxn2-engine::shared` (the store + `ParamModel` trait surface)
and `vxn2-clap::local` (the audio-thread mirror). Mirrors VXN1's
`vxn-engine::shared` + `vxn-clap::local` split.

## Acceptance criteria

- [ ] `vxn2-engine::shared::SharedParams` holds `[AtomicU32; TOTAL_PARAMS]`
      — one `AtomicU32` per CLAP id, storing the plain-units value as
      `f32::to_bits`. Loads/stores use `Ordering::Relaxed`; the store
      is per-slot lockless and allocation-free after construction.
- [ ] `SharedParams::new()` seeds every slot from the corresponding
      `ParamDesc::default` (from 0012's `ParamTable`).
- [ ] `SharedParams::get(idx) -> f32`, `set(idx, value: f32)`,
      `get_normalized(idx) -> f32` (looks up the descriptor's
      plain↔norm converter).
- [ ] `ParamModel` trait in `vxn2-engine` exposes `total()`, `get(id)`,
      `get_normalized(id)`, `snapshot_bytes() -> Vec<u8>` and
      `load_bytes(&[u8]) -> Result<…>`. `SharedParams` implements it.
      The trait surface is what 0015 (CLAP params extension) and 0017
      (state) bind against.
- [ ] `TOTAL_PARAMS` constant exported from `vxn2-engine` (= 174 per
      0012's count, but derived from `ParamTable::len()` so it can't
      drift).
- [ ] `vxn2-clap::local::LocalParams`:
      - `new(shared: &SharedParams)` seeds from the shared store.
      - `apply_input(event: &UnknownEvent) -> Option<(usize, f32)>` —
        folds a CLAP `ParamValue` event into the mirror, returns
        `(idx, value)` so the caller drives the engine.
      - `fetch_ui_changes(&SharedParams) -> bool` — diffs the shared
        store against the mirror; for E002 always returns false (no UI
        writes yet), but the diff loop is present and tested with a
        stub.
      - `write_to(engine_params: &mut EngineParams)` — pushes the
        whole mirror into the engine's working param set at the top
        of each block (cheap; the engine's smoothers absorb it).
      - `publish(&SharedParams)` — writes host-changed entries back to
        the shared store so `get_value` reflects automation.
      - `emit(&SharedParams, output_events, frame_count)` — stub now,
        gesture-bracketed `ParamValue` events later; for E002 it walks
        `ui_changed` (always empty) and emits nothing.
- [ ] No allocation in `apply_input`, `fetch_ui_changes`, `write_to`,
      `publish`, or `emit`.
- [ ] Property test: a sequence of `apply_input` writes followed by
      `publish` makes `SharedParams::get` return the last-written value
      for each touched id. Untouched ids stay at their default.
- [ ] Property test: round-tripping a `SharedParams` through
      `snapshot_bytes` → `load_bytes` reproduces every slot
      bit-identically.

## Notes

VXN1's `vxn-engine::shared::SharedParams` is the structural reference;
copy the atomic-array shape and the `f32::to_bits` packing. Don't copy
the layer-mode / split-point / key-mode side channels — VXN2's voicing
mode is a regular CLAP param per 0012 / 0009, so it lives in the param
table, not as a separate shared field.

`ParamModel` exists so the CLAP shell and the state extension don't
import `SharedParams` directly — the UI epic will introduce a separate
implementation (UI-side mirror with view-event emission) and both
implementations satisfy the same trait. Keeps 0015 / 0017 swappable.

The `ui_changed` / `host_changed` bookkeeping in `LocalParams` looks
overbuilt for an E002-only path (which has no UI writes) but it is the
exact surface the UI epic plugs into; ship it now so 0015 / 0016 are
written against the final shape.

`AtomicU32` over `AtomicF32`: `f32::to_bits` / `f32::from_bits` are
free, and `AtomicF32` isn't stable. Use `Relaxed` ordering — params are
independent slots, no inter-param ordering matters, and the audio
thread tolerates a few-sample skew on a multi-param change.
