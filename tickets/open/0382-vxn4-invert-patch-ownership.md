---
id: "0382"
product: vxn-4
title: "vxn-4 patch ownership inverts — shared store and topology ring, audio thread reads only"
priority: high
created: 2026-09-10
epic: E052
depends: ["0381"]
---

## Summary

Move the authoritative patch off the audio thread. Today
[`Engine`](../../vxn-4/crates/vxn4-engine/src/engine.rs#L269) **owns** its
`patch: Patch` ([engine.rs:273](../../vxn-4/crates/vxn4-engine/src/engine.rs#L273))
and [`set_patch`](../../vxn-4/crates/vxn4-engine/src/engine.rs#L449) swaps the
whole thing in from the baked bank. That is fine while the only writer is a
patch-index param and fatal once an editor is writing individual fields at
pointer rate.

The rule this establishes, and which the rest of E052 depends on: **the main
thread owns the model; the audio thread reads it and never owns it.** Truth
flows main → audio and never back.

vxn-1b already solved this and the two halves of the answer are in the tree:
[`shared.rs`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs) (atomics for
values, main-thread-authoritative tables for the rest, a reload flag) and
[`topology.rs`](../../vxn-1b/crates/vxn1b-engine/src/topology.rs) (an SPSC ring,
one `Edit` per field, one `Snapshot` for bulk, the snapshot doubling as the
overflow backstop). Port that split, do not redesign it.

## Acceptance criteria

- [ ] `vxn4-engine::shared::SharedParams` holds one `AtomicU32` per descriptor
      id from [0381](0381-vxn4-patch-descriptor-table.md), plus the
      main-thread-authoritative copy of everything that is not a scalar.
- [ ] `vxn4-engine::topology` is an SPSC ring of `TopoMsg`, with `Edit` (one
      field) and `Snapshot` (whole patch) arms and a documented overflow policy
      that degrades to a snapshot.
- [ ] The audio thread drains the ring at the top of `process`, applies records
      straight onto its tables, and **takes no lock, spins on nothing, and
      allocates nothing** on any patch-edit path.
- [ ] Any mutex guarding the authoritative tables is main-thread-only by
      construction. State this as a doc invariant on the type and enforce it by
      not exposing a guard-taking accessor to the audio side at all.
- [ ] `Engine` no longer owns a `Patch`. It holds the derived, flattened tables
      it actually renders from; `patch_index` remains for the CLAP param.
- [ ] A preset load landing mid-render is coherent: params and topology cannot
      be observed half-applied, whichever order the producer's stores land in.
- [ ] Every existing vxn4-engine test passes unedited, and the seven factory
      patches render bit-identically to the pre-ticket build.

## Notes

The bit-identical requirement is the whole safety net for this ticket, and it is
achievable: nothing here changes what is computed, only who owns the numbers.
Land it with the render maths untouched. If a rendering difference appears, it is
a bug in this ticket, not an improvement.

Ordering between the two channels is the subtle part. vxn-1b's answer: draining
a `Snapshot` implies a param re-sync, so the reload flag and the ring cannot come
apart ([shared.rs:26-31](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L26-L31)).
Copy that property and test it explicitly — a test that sets params and pushes a
snapshot in both orders and asserts the same end state.

vxn-4 has one thing vxn-1b does not: `set_patch` currently panics every sounding
voice, and the CLAP shell guards against calling it redundantly for exactly that
reason ([lib.rs:214-222](../../vxn-4/crates/vxn4-clap/src/lib.rs#L214-L222)). A
field-level edit must **not** panic voices — that is the point of the `Edit` arm.
Which per-field edits are safe to apply to sounding voices, and which genuinely
need a voice reset, wants deciding here and documenting rather than discovering
later. `phase_spread` is the known example: it is applied at note onset, so
editing it cannot affect notes already sounding
([ops.rs:174-177](../../vxn-4/crates/vxn4-dsp/src/ops.rs#L174-L177)), and that is
correct behaviour rather than a bug to fix.
