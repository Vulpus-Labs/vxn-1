---
id: "0172"
product: vxn-3
title: "vxn-3 engine-aware value_to_text — main-thread engine-kind tracking + macro display"
priority: medium
created: 2026-07-02
epic: E032
---

## Summary

Make a macro slot's displayed value **engine-aware** so a generic *name*
(`T3 · M1`) doesn't mean an opaque *readout*: "Decay 0.42 s" under Kick vs
"Ring 1.8 s" under Metal. Needs the main thread to know each track's active engine
kind (swaps are currently fire-and-forget) and to call 0170's pure `macro_display`.

Design: ADR 0003 §2 (dynamic value-text under a fixed name). Depends on 0170
(`macro_display`) + 0171 (params extension).

## Design

- **Main-thread engine-kind cache.** The `SetEngine { track, kind }` swap in
  [vxn3-app/src/lib.rs:108](vxn-3/crates/vxn3-app/src/lib.rs#L108) currently sends
  a freshly-built engine into the swap mailbox and forgets the kind. Record the
  per-track `EngineKind` in main-thread state (in `VxnMainThread` or the controller)
  when the swap is issued, so `value_to_text` can resolve which engine a macro slot
  currently drives. This is the authoritative main-thread mirror of engine kind
  (also consumed by 0174 state).
- **`value_to_text` dispatch.** For a macro `clap_id`, resolve `(track, slot)` via
  the 0171 id map, look up the track's `EngineKind`, and call the pure
  `macro_display(kind, slot, value, out)` from 0170 — no reach into the audio-thread
  engine. If the engine's `macro_count` doesn't cover the slot, render "—".
- **Mix/master text.** level → dB, pan → L/C/R, mute → on/off, send/return →
  percent or dB, delay time → beats. Plain, engine-independent.

## Acceptance criteria

- [ ] Main thread tracks each track's active `EngineKind`, updated on every
      `SetEngine` swap (unit-tested).
- [ ] `value_to_text` on a macro renders engine-aware via `macro_display`; swapping
      a track's engine changes the readout **without** changing the `clap_id`.
- [ ] Unmapped slots (`slot >= macro_count`) render a sentinel ("—"), not garbage.
- [ ] Mix/master params render human-readable units (dB / L-C-R / % / beats).
- [ ] `cargo test -p vxn3-clap -p vxn3-app` green.

## Notes

- Keep `value_to_text` allocation-free where the clack API allows a caller buffer;
  `macro_display` already writes into a caller buffer (0170).
- The engine-kind cache is the same fact 0174 must serialize — share the field,
  don't duplicate it.
