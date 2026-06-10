---
id: "0068"
title: "RT hardening: Engine::reset in place, SINE_TABLE const static"
priority: medium
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Eighth ticket of [E006](../../epics/open/E006-review-remediation.md).
Two latent RT-safety findings from the review:

1. **`Engine::reset` constructs a fresh `PolyAlloc`**
   ([engine.rs:146](../../crates/vxn2-engine/src/engine.rs#L146)).
   CLAP marks `reset` as an audio-thread method; constructing the
   struct can touch the heap depending on layout/moves. Replace with a
   `PolyAlloc::clear(&mut self)` that zeroes fields in place — no
   construction, no allocation.

2. **`SINE_TABLE` is `LazyLock<[f32; 1024]>`**
   ([sine.rs:20](../../crates/vxn2-dsp/src/sine.rs#L20)). First deref
   initialises under an internal once-lock — allocation + lock if it
   ever happens on the audio thread. No production caller today
   (`lookup_sine_q32` is test-only), but it's an armed trap for the
   first person who reaches for the table in hot code. Convert to a
   const-initialised `static SINE_TABLE: [f32; 1024]` (const fn with a
   const-evaluable sine approximation, or a `build.rs`/macro-generated
   literal — whichever is least code).

## Acceptance criteria

- [ ] `Engine::reset` performs no construction of `PolyAlloc`;
  `clear()` resets every field the constructor would (stacks gated
  off, held list empty, glides cleared, counters zeroed). Test:
  play notes, `reset()`, assert allocator state equals a fresh
  instance's observable state and next note-on behaves identically.
- [ ] No `LazyLock` / `OnceLock` / `lazy_static` remains in
  `vxn2-dsp`. `SINE_TABLE` values bit-identical to the current
  runtime-initialised table (test compares against a freshly computed
  table).
- [ ] Grep sweep recorded in the ticket close-out: no other
  lazy-init/lock/alloc primitives reachable from `process_block` /
  `stack_tick_*` (the review found none besides these two — confirm
  still true at landing).

## Notes

If const-evaluating `sin` proves awkward (f32 transcendentals in const
context), an `include!`-ed generated literal beats pulling in a const
math crate — the table is 1024 floats, ~12 KB of source, generated
once by a small `#[test]`-gated writer or xtask subcommand.
