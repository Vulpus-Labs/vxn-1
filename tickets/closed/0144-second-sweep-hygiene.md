---
id: "0144"
product: monorepo
title: Second-sweep hygiene batch — dead trait method, defensive comments
priority: low
created: 2026-06-23
epic: E027
---

## Summary

Small, low-risk items the second sweep enumerated. Group into
one batch; land last to avoid conflicting with the larger
tickets. Behaviour-preserving.

1. **Delete dead `EditorBackend::open`.** Deep review:
   `vxn-core-app/src/backend.rs:31` `open` has one impl
   (`WebEditor`, `vxn-core-ui-web/src/lib.rs:380`, which just
   returns `Err`) and **zero callers** — every synth opens
   editors via the concrete free fn `open_editor`. Delete the
   trait method and the matching `WebEditor::open` impl. The
   trait's `close` / `push_view_event` / `flush_view_events`
   are used (via `EditorHandle`) and stay.
2. **xorshift cross-ref comment** (if not already done in
   `0117`): note on vxn-1 `vxn-dsp/src/math.rs:10` that its
   `xorshift64` is intentionally a different variant from
   vxn-2's `xorshift64*` (`vxn2-dsp/src/rng.rs:12`) — do not
   merge them (different output mappings).
3. **`SharedParams` threading SAFETY doc.** Add a crate-level
   SAFETY comment on `SharedParams::set`/`get`
   (`vxn2-engine/src/shared.rs`) documenting the CLAP
   audio/main-thread non-overlap guarantee that makes the
   atomic-free write-through (`local.rs:121`) sound — so a
   future reviewer adding a non-atomic field is warned.

## Acceptance criteria

- [ ] `EditorBackend::open` and `WebEditor::open` are
      deleted; the workspace builds; no caller existed
      (confirm with grep in the close-out).
- [ ] The xorshift cross-ref comment exists (here or `0117`).
- [ ] A SAFETY comment documents the `SharedParams`
      threading guarantee.
- [ ] `cargo test --workspace` green.

## Notes

Each item is independent. **`EG_LOG_LEVELS` is NOT in this
ticket** — the level-curve flag is being productionized by
the concurrent epic E026 (DX7-faithful level curve, ticket
0123); leave `eg.rs` to that epic. The `recompute_pan` →
`pan_targets` fold lives in `0121`; the `static mut STATE`
forward-compat note lives in `0142`. No audio behaviour
changes here — comments + dead-code removal only; do not
touch any hot-path or EG code.

## Close-out (2026-07-01)

- Deleted `EditorBackend::open` trait method from
  [backend.rs](../../crates/vxn-core-app/src/backend.rs) and
  `WebEditor::open` impl from
  [lib.rs:407](../../crates/vxn-core-ui-web/src/lib.rs#L407). Removed now-unused
  `use std::error::Error` and `{ControllerHandle, CorpusHandle}` imports from
  `backend.rs`. Grep sweep confirms zero callers in the workspace.
- Xorshift cross-ref comment already present at
  [math.rs:10](../../vxn-1/crates/vxn-dsp/src/math.rs#L10): "Intentionally a
  *different* generator from vxn-2's xorshift64*" — criterion satisfied.
- Added SAFETY comment on `SharedParams::get`/`set` at
  [shared.rs:455](../../vxn-2/crates/vxn2-engine/src/shared.rs#L455) documenting
  the CLAP audio/main-thread non-overlap guarantee and the `LocalParams` plain-`[f32]`
  consequence; warns future reviewers adding non-atomic fields.
- `cargo test --workspace` green (exit code 0).
