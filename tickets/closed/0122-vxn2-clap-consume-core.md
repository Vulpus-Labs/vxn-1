---
id: "0122"
product: vxn-2
title: vxn2-clap — consume vxn-core-clap helpers, add controller factory
priority: medium
created: 2026-06-23
epic: E027
---

## Summary

`vxn2-clap` (2072 lines) re-copies helpers that
`vxn-core-clap` already exports, each drifting independently.

1. **`batch_range`** (`vxn2-clap/src/lib.rs:414-429`) is a
   verbatim copy of `vxn_core_clap::batch_range` (same body,
   "Mirrors VXN1" comment). Delete; import core's.
2. **`dispatch_event`** (`lib.rs:435-488`) duplicates
   `vxn_core_clap::dispatch_event` — the NoteOn/NoteOff/MIDI
   bend/CC1/CC64/aftertouch arms are byte-identical. The
   genuine difference is vxn-2 writes through to the shared
   store (`local.apply_input(shared, …)`) instead of using
   the `on_param` callback. Extract the MIDI/note arms into a
   shared `dispatch_notes` in core that both call, leaving
   only the param-write seam per-synth.
3. **`LocalParams::emit`** (`local.rs:164-203`) re-implements
   the gesture bracket logic from
   `vxn_core_clap::local::LocalParams::emit`. The pure
   decision `bracket(changed, cur, prev) -> (bare, begin,
   end)` is already unit-tested in core — extract it there
   and call from vxn-2's `emit` (vxn-2's `end_time =
   frame_count.saturating_sub(1)` guard is correct; keep it).
4. **Controller construction is repeated in 3 sites** —
   `new_main_thread` (`lib.rs:124`), the `mk_main` test helper
   (`:669`), each calling `set_echo_param_writes(false)` +
   `set_init_preset_meta`. Extract
   `make_vxn2_controller(shared) -> Controller<SharedParams>`.

## Acceptance criteria

- [ ] `vxn2-clap`'s private `batch_range` is deleted; it
      imports `vxn_core_clap::batch_range`.
- [ ] The NoteOn/NoteOff/bend/CC1/CC64/aftertouch arms live
      once in `vxn-core-clap` (`dispatch_notes` or equivalent)
      and are called from both synths; vxn-2 keeps only its
      shared-store write-through.
- [ ] The gesture-bracket decision is a single pure fn in
      `vxn-core-clap`, consumed by both `LocalParams::emit`
      impls.
- [ ] A `make_vxn2_controller` factory is the one
      construction path used by `new_main_thread` and
      `mk_main`.
- [ ] `cargo test -p vxn2-clap` green; gesture begin/end
      balancing and the dispatch arms keep their existing
      test coverage (add parity tests if the extraction lacks
      them).

## Notes

The write-through-to-shared-store approach is architecturally
different from vxn-1's `on_param`-callback path — do not
collapse the two; only the MIDI/note arms (which are
identical) move to core. The deep review confirmed vxn-2's
inactive-flush path is **not** buggy (all-ones dirty seed
masks it — see epic Notes); do not "fix" `flush` here.
`drain_dirty_bits` (`lib.rs:210`) and its `force_rate_refresh`
hot-path `Vec` alloc are noted but out of scope — they belong
in vxn2-app, ticket separately if pursued.

## Close-out (2026-06-29)

- Private `batch_range` deleted; `vxn2-clap` imports
  [`vxn_core_clap::batch_range`](../../crates/vxn-core-clap/src/events.rs#L17).
  Existing parity tests (`tests::batch_range_*`) retained, now
  exercising the imported fn.
- Note/MIDI arms (NoteOn/NoteOff/bend/CC1/CC64/aftertouch)
  extracted to [`dispatch_notes`](../../crates/vxn-core-clap/src/events.rs#L43)
  in core; `dispatch_event` (core) routes `ParamValue`→callback,
  else delegates. vxn-2's
  [`dispatch_event`](../../vxn-2/crates/vxn2-clap/src/lib.rs#L441)
  keeps only the shared-store write-through seam and calls
  `dispatch_notes` through `EngineNotesAdapter` (orphan rule
  blocks impl on `Engine`; adapter carries the `[0,1]`→`1..=127`
  velocity quirk). Dispatch tests
  (`tests::dispatch_*`) green both sides.
- Gesture-bracket decision is the single pure fn
  [`bracket`](../../crates/vxn-core-clap/src/local.rs#L39) in core
  (unit-tested `local::tests::bracket_decisions`); vxn-2's
  [`emit`](../../vxn-2/crates/vxn2-clap/src/local.rs#L177) consumes
  it, retaining the `end_time = frame_count.saturating_sub(1)`
  guard.
- [`make_vxn2_controller`](../../vxn-2/crates/vxn2-clap/src/lib.rs#L132)
  factory is the one construction path; `new_main_thread` and the
  test `mk_main` both call it.
- `cargo test -p vxn-core-clap -p vxn2-clap` green; clippy clean
  for both crates (pre-existing dsp/engine warnings untouched).
