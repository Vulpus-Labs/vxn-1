---
id: "0386"
product: vxn-4
title: "vxn4-app: ParamModel implementation and controller wiring"
priority: high
created: 2026-09-10
epic: E052
depends: ["0382"]
---

## Summary

vxn-4 has no app layer. Every other synth in the tree has one — `vxn1b-app`,
`vxn2-app`, [`vxn3-app`](../../vxn-3/crates/vxn3-app/src/lib.rs) — sitting
between the engine and both front ends, implementing
[`vxn_core_app::ParamModel`](../../crates/vxn-core-app/src/model.rs) and driving
the shared [`Controller`](../../crates/vxn-core-app/src/controller.rs).

This is the crate that makes the main thread the owner. The `ParamModel` doc
states the contract this ticket has to satisfy: *"writes flow from the
controller, reads from the audio thread; the synth's concrete impl arranges the
lock-free crossing"*.

## Acceptance criteria

- [ ] New crate `vxn4-app`, depending on `vxn4-engine` and `vxn-core-app`, with
      no clap and no windowing dependency.
- [ ] `ParamModel` implemented over the `SharedParams` store from
      [0382](0382-vxn4-invert-patch-ownership.md): `get` / `set`,
      normalized variants, gesture flags, and `descriptor` resolving against
      the [0381](0381-vxn4-patch-descriptor-table.md) table.
- [ ] `snapshot_bytes` / `restore_from_bytes` delegate to the
      [0384](0384-vxn4-clap-state-v2.md) blob, so the controller serves host
      save/load without knowing the format.
- [ ] Per-synth state that does not fit the `(id, f32)` shape — waveform
      selections, route enables, matrix topology — rides a vxn-4 extension
      trait, as the shared model doc prescribes, and reaches the audio thread
      over the topology ring rather than through `set`.
- [ ] A `Vxn4UiCustom` / `Vxn4ViewCustom` pair for the structured edits the
      faceplate makes that are not param writes (route enable, matrix slot
      edit, patch load), following `vxn3-app`'s shape.
- [ ] `vxn4-clap` drives the engine through this crate rather than calling
      `Engine` setters directly, and its existing lifecycle test still passes.
- [ ] The eleven CLAP params still behave exactly as they do today, including
      the guard that stops a redundant `set_patch` panicking every voice
      ([lib.rs:214-222](../../vxn-4/crates/vxn4-clap/src/lib.rs#L214-L222)).

## Notes

The CLAP param table stays eleven entries. What changes is that those eleven
become a *projection* of the descriptor table rather than a parallel universe —
`Slot::Macro(m)` resolves to a `ParamId` through the mapping 0381 defines. The
argument for keeping the host surface small is unchanged and this ticket must
not quietly widen it.

`vxn-core-app`'s controller is 548 lines of already-solved sequencing —
gestures, echo fan-out, preset corpus, host save/load. The work here is fitting
vxn-4 to it, not extending it. If something genuinely does not fit, widen the
shared crate rather than forking a vxn-4 copy, and say which in the close-out.

Watch the direction of travel. The rule from 0382 is that truth flows main →
audio and never back, which means the audio thread must never write a value the
main thread then reads back as authoritative. Host automation arriving on the
audio thread is the case that looks like an exception and is not: it lands in the
atomic store, which is the *shared* view, and the main-thread model reads it from
there — the same way vxn-1b's shell serves `get_value` off the main thread
([shared.rs:7-11](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L7-L11)).
