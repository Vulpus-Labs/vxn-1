---
id: "0304"
product: vxn-1
title: "vxn-1: dirty bits on both model impls (SharedParams and WebModel)"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0299"]
---

## Summary

Ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md): the
vxn-1 model half. vxn-1 is the origin of the poll-and-diff idiom
([ADR 0007](../../vxn-1/adrs/0007-vxn1-mvc-architecture.md), 2026-05-30) that
vxn-2 superseded ten days later and VXN1b inherited.

**vxn-1 has two `ParamModel` impls, and both need bits** or the web build
silently keeps the old path:

- [`vxn-engine::SharedParams`](../../vxn-1/crates/vxn-engine/src/shared.rs#L34)
  — the native model.
- [`vxn-web-controller::WebModel`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L70)
  — a second, parallel store in the controller wasm (separate linear memory from
  the worklet, so it cannot share the engine's).

Model-side only; nothing drains until [[0305]].

## Design

Both structs gain `DirtyBits` from [[0299]] (165 ids → 3 words), marked in `set`
([shared.rs:71](../../vxn-1/crates/vxn-engine/src/shared.rs#L71)) and
`set_normalized` ([shared.rs:90](../../vxn-1/crates/vxn-engine/src/shared.rs#L90))
and the `WebModel` equivalents, seeded all-set.

### The non-CLAP state is the coverage gap

`key_mode` and `split_point`
([shared.rs:41-43](../../vxn-1/crates/vxn-engine/src/shared.rs#L41-L43)) are
plain atomics with **no change flag at all**. vxn-1's answer was a third
mechanism, distinct from both the poll and VXN1b's memos: the `on_model_loaded`
hook republishes them after a *known* load
([controller.rs:118](../../vxn-1/crates/vxn-app/src/controller.rs#L118)) — correct
for loads, silent for every other writer. Give them view bits.

Note `set_key_mode_seeded` vs `set_key_mode`
([shared.rs:117-131](../../vxn-1/crates/vxn-engine/src/shared.rs#L117-L131)) are
deliberately different writers (UI edit vs state load, the layer re-seed rule);
both mark, and the ADR amendment should say the bit is about *notification*, not
about which re-seed semantics ran.

### ADR amendment, not a new ADR

vxn-1's ADR 0007 is the MVC decision and stays; amend its change-detection
section to point at the pump, and cross-reference vxn-2's ADR 0003 rather than
restating it. A second vxn-1 ADR saying "actually, like vxn-2" would fragment the
record.

## Acceptance criteria

- [ ] `SharedParams` and `WebModel` both carry value bits + `key_mode` /
      `split_point` view bits, with the same ordering contract.
- [ ] A write to either model, in either impl, surfaces exactly that id on the
      next drain; a second drain yields nothing.
- [ ] Seeded-full construction drains all 165 on first read, in both impls.
- [ ] `restore_from_bytes` on both marks the full table plus key mode and split.
- [ ] ADR 0007's change-detection section is amended and dated.
- [ ] `cargo test -p vxn-engine`, `-p vxn-web-controller`, `--workspace` green —
      both shells still on their old paths, so nothing should move yet.

## Notes

- Additive; one `fetch_or(Release)` per write on the audio thread's publish path.
- The duplicated `WebModel` is itself a smell (two hand-maintained stores that
  must agree on layout, already the subject of [[0285]]). Not this ticket's job —
  but if the two are ever merged, the bits should be part of the merged type, not
  re-added on top.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]].
- Blocks 0305.
