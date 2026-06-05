---
id: "0004"
title: Polyphony allocator (16 voices, oldest steal)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

Hand out voices to incoming note-on events. 16-voice polyphony in v1; oldest-
note stealing when full. Supports Poly and Solo `assign_mode`, with glide
between consecutive notes in Solo (optional in Poly via legato).

## Acceptance criteria

- [ ] `PolyAlloc` holds a fixed array of 16 `Voice` slots (no dyn alloc).
- [ ] `note_on(note, vel)` finds an idle voice (EG in release with output
      below silence threshold) or, failing that, steals the oldest still-
      gated voice.
- [ ] `note_off(note)` gates all voices currently sounding that note (handles
      duplicate triggers).
- [ ] Solo mode: a new note-on while another is held re-uses the same voice,
      retriggers EG (unless `legato` is on, in which case EG continues and
      pitch glides).
- [ ] Glide: linear pitch ramp over `glide_time` ms between consecutive
      notes (Solo always; Poly only with `legato` on and notes overlapping).
- [ ] Pitch bend: applied at the voice level, not the allocator — but the
      allocator forwards `set_bend(value)` to every voice.
- [ ] Oldest-steal heuristic: the voice whose note-on timestamp is earliest
      AND whose EG is at or past sustain. Ties broken by lowest played note.
- [ ] Bench: `alloc_held_chord` (8 notes held, render N seconds) and
      `alloc_steal_churn` (rapid note-on/off cycling beyond polyphony cap).

## Notes

VXN1's allocator in `vxn-engine` is a useful reference for the priority queue
shape, but VXN2 has different voice mechanics (6 ops + stacking metadata) so
copy-the-design, not copy-the-code.

Don't ship MPE or per-note expression yet — channel-wide aftertouch / mod
wheel / bend only. MPE is its own epic when we have a user asking for it.

Voice-stealing is a sound design failure when audible. Add an integration
test that holds 16 notes and triggers a 17th, asserting that the new note's
amplitude ramps up smoothly (no click from the stolen voice's mid-cycle
truncation). A short fade-out crossfade on steal may be necessary; defer the
specific implementation until the test exposes the issue.
