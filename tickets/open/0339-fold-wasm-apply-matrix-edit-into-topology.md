---
id: "0339"
product: vxn-1b
title: "vxn1b-wasm's apply_matrix_edit is a second copy of the field-decode match — delegate it to topology::apply_edit"
priority: low
created: 2026-08-30
epic: E049
depends: []
---

## Summary

Follow-up spotted while building
[0338](0338-vxn1b-topology-ring-delete-the-mutex.md), and deliberately left out
of it as out-of-unit.

[`topology::apply_edit`](../../vxn-1b/crates/vxn1b-engine/src/topology.rs#L108)
and
[`codec::apply_matrix_edit`](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L502)
carry **byte-identical** seven-arm matches over `MatrixField`, each writing the
same slot field from the same `from_u8`. 0338 created the first when it moved
the audio thread onto an SPSC ring; the second predates it and was not folded in.

The wasm copy can become a one-liner, since it already has the layer in hand:

```rust
fn apply_matrix_edit(engine: &mut Engine, edit: MatrixEdit) {
    topology::apply_edit(engine.matrix_mut(edit.layer), edit);
}
```

The stale doc comment is the sharper half of this. `apply_matrix_edit`'s
comment points the reader at `SharedParams::edit_matrix_slot` as the other
copy — but 0338 rewrote that method, and it no longer contains a match at all.
So the one signpost telling a maintainer where the parallel copy lives now
points somewhere it isn't.

## Why it is worth a ticket rather than a shrug

Adding a `MatrixField` variant makes the compiler flag both matches, so this is
not a silent-drift hazard in the way the *wire ordinal* tables are. It is worth
doing anyway because the cost is one line and the epic's whole thesis is that a
routing mechanism written twice drifts — and because the misdirecting comment
costs a future reader real time.

## Acceptance criteria

- [ ] `codec::apply_matrix_edit` delegates to `topology::apply_edit`; only one
      `MatrixField` write-match remains in the vxn-1b tree.
- [ ] Its doc comment names a copy that actually exists, or says there is only
      one.
- [ ] `cargo test -p vxn1b-wasm` and `-p vxn1b-engine` green; the wasm decode
      path's existing matrix-edit tests still pass unchanged.
- [ ] Render-hash baseline unchanged — this is a refactor of a decode path, not
      arithmetic.

## Notes

- Scope is the **Rust apply match** only. The parallel drift on the *wire
  ordinal* side — `event-codec.mjs` / `faceplate-bridge.mjs` / `controller.mjs`
  stale against `vocab.rs`'s seven `MATRIX_FIELD_NAMES` — is a separate and much
  more serious bug, since that one fails silently. Do not conflate them.
- Out of scope: moving either function into `vxn-core-matrix`. Where the shared
  slot type ends up is [0333](0333-share-slot-and-route-compilation.md)'s
  question; this ticket only stops vxn-1b keeping two copies of its own.
- `priority: low` — compiler-enforced, no user-visible effect.
