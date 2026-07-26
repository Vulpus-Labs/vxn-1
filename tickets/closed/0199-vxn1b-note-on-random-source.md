---
id: "0199"
product: vxn-1b
title: "Note-on random: per-voice RNG value latched at note-on"
priority: medium
created: 2026-07-25
epic: E036
---

## Summary

Add the second source VXN1 lacks: a per-voice **random** value in `[0,1)`,
latched once at note-on and fixed for the note's lifetime, used to decorrelate
stacked/adjacent voices ([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md)
§2). Small, self-contained engine plumbing in `vxn1b-engine`.

- On note-on, latch a per-voice `f32` in `[0,1)` from a cheap
  deterministic-per-voice RNG (no audio-thread allocation, no global mutable RNG
  contention — a per-voice seed advanced deterministically is fine).
- Hold it constant until the voice is released/reused.
- Expose it as an engine-readable per-voice value (the `SourceId::NoteRandom`
  enum lands in 0201; the evaluator reads it in 0202).

## Acceptance criteria

- [ ] Each voice latches a random value in `[0,1)` at note-on.
- [ ] The value is **constant across the note's lifetime** (test).
- [ ] Values **differ across concurrent voices** (test).
- [ ] No audio-thread allocation; deterministic enough to be testable.
- [ ] Exposed for later matrix consumption.

## Notes

- Determinism: avoid `Math.random`-style nondeterminism (also see the harness
  note that `Date.now`/`Math.random` are unavailable in some contexts) — use a
  per-voice counter/seed so tests are reproducible.
- Depends on 0197. Feeds 0201 (SourceId) and 0202 (evaluator). Independent of
  0198 (MPE) — can proceed in parallel.
</content>

## Close-out (2026-07-26)

- Per-voice `note_random[]` in `[0,1)` latched at note-on from a single
  deterministic xorshift64 stream: [voice.rs](../../vxn-1b/crates/vxn1b-engine/src/voice.rs)
  (`note_random_draw`, `note_on`), exposed via `note_random(v)`. Tests:
  `voice::tests::{note_random_in_unit_interval, note_random_constant_over_note_lifetime,
  note_random_differs_across_concurrent_voices, note_random_reproducible_from_seed,
  note_random_relatched_on_reuse}`. Consumed as `SourceId::NoteRandom` by the evaluator.
