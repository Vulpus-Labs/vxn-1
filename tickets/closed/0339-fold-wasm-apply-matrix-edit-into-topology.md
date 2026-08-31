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

## Close-out (2026-08-31)

- `codec::apply_matrix_edit` is now a single delegation to
  [`topology::apply_edit`](../../vxn-1b/crates/vxn1b-engine/src/topology.rs#L108):
  7 insertions, 16 deletions, one file. The seven-arm `MatrixField` match and
  the function-local `use vxn1b_engine::matrix::{DestId, Polarity, Shape,
  SourceId}` are gone —
  [codec.rs:497](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L497).
- Grep sweep `MatrixField::.* => slot\.` across `vxn-1b/`, `vxn-2/` and
  `crates/` returns exactly seven lines, all in `topology.rs:113-119`. The only
  other match over the field is `codec.rs`'s `matrix_field_code`, which is
  `#[cfg(test)]` and encodes a wire byte rather than writing a slot.
- Doc comment rewritten: it now names `topology::apply_edit` as the only
  `MatrixField` write-match and lists the three callers that funnel through it.
  The stale pointer to `SharedParams::edit_matrix_slot` is gone — 0338 had
  already converted that method to a delegation
  ([shared.rs:363](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L363)).
- **Correction to the ticket's premise:** `MatrixField` does *not* become an
  unused import. `unpack_matrix_addr`
  ([codec.rs:151](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L151)) still
  constructs all seven variants from the wire byte, so the `codec.rs:60` import
  stays. No new warnings.
- Tests green, unchanged: `codec::tests::a_matrix_edit_retargets_the_slot_and_leaves_its_depth_alone`,
  `topology::tests::apply_edit_ignores_an_out_of_range_slot`, and the wasm
  round-trip cases. Full `cargo test --workspace` on the merged tree: 91 test
  binaries, 0 failures. Node web suite 161/161.
- Render hash unchanged and the null test is `-inf dBFS` on both synths — the
  renders are bit-identical, as a decode-path refactor should be.
- Landed via `e049/0339-fold-apply-edit` @ `f5dd46a`, merged to `main` in
  `c58167c`.
