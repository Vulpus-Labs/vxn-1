---
id: "0008"
title: vxn-1 vxn-clap migrate onto vxn-core-clap helpers
priority: medium
created: 2026-06-06
epic: E001
---

## Summary

Rewire vxn-1's `vxn-clap` shell to consume the helpers in
`vxn-core-clap`: dispatch_event, state save/load, gesture emit, the
generic `LocalParams<N>`. Each helper drops vxn-1-local code; the
synth keeps its own `Plugin` impl + `SynthDescriptor` (the
`SynthPlugin<E>` generic shell was deferred in 0005).

## Prerequisites

- 0007 closed: vxn-1's event types are now `vxn_core_app::{UiEvent, ViewEvent}`,
  so the `EngineNotes` trait impl can route ParamValue events
  through `vxn_core_clap::dispatch_event`'s `on_param` closure into a
  `LocalParams<TOTAL_PARAMS>`.

## Acceptance criteria

- [ ] `vxn_engine::SharedParams` implements
      `vxn_core_clap::SharedStore` (get/set on `usize`).
- [ ] `vxn_engine::Synth` (or whatever owns the audio-thread
      surface) implements `vxn_core_clap::{EngineProcess, EngineNotes}`.
      Existing `note_on` / `process_block` / `reset` / `set_sample_rate`
      / `set_tempo` methods become trait-method impls.
- [ ] vxn-1 vxn-clap's local `LocalParams` (~200 LOC) is replaced
      by `vxn_core_clap::LocalParams<TOTAL_PARAMS>`. vxn-1's
      `params/mod.rs` mirror logic (write_to / publish / emit)
      either delegates to the generic LocalParams or is deleted.
- [ ] vxn-1 vxn-clap's `dispatch_event` (note/MIDI dispatch in
      `lib.rs`) is replaced by `vxn_core_clap::dispatch_event`.
- [ ] vxn-1 vxn-clap's state save / load uses
      `vxn_core_clap::state::{save_blob, load_blob}`. Wire format
      (versioned header + flat f32 array, byte-identical to vxn-1
      pre-extraction) is preserved by `SharedParams::snapshot_bytes`
      / `restore_from_bytes`.
- [ ] vxn-1 vxn-clap's outbound gesture emit uses
      `vxn_core_clap::{emit_gesture_begin, emit_gesture_end,
      emit_param_value}`.
- [ ] vxn-1 vxn-clap's `tempo_from_transport`-shaped code uses
      `vxn_core_clap::tempo_from_transport`.
- [ ] vxn-1 tests pass; vxn-1's plugin still loads in `clack-host`
      smoke harness.

## Notes

The migration is largely mechanical: drop in the trait impls, replace
the local helper calls. Each helper is bit-identical to the version
already in vxn-1, so no audio-baseline diff is forced by *this* ticket
alone — but combine it with 0007 / 0009 and re-run the 0010 baseline
diff before declaring done.

`Plugin` impl + `SynthDescriptor` stay vxn-1-local; the generic
`SynthPlugin<E>` from 0005's Notes is still deferred until a third
consumer materialises.
