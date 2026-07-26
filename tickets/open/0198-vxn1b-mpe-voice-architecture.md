---
id: "0198"
product: vxn-1b
title: "MPE voice architecture: thread MIDI channel through allocation + per-voice pressure state"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

The genuinely novel plumbing for VXN1b, done **early** because it shapes the
voice struct the matrix evaluator later reads
([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §2). VXN1's voice
allocator is channel-agnostic; VXN1b must track the **originating MIDI channel**
through note allocation so **per-note (MPE) pressure** reaches *that note's*
voice, with **channel pressure** as the degenerate broadcast case.

Work in `vxn1b-engine` (forking VXN1's allocator/voice — see
[voice.rs](../../vxn-1/crates/vxn-engine/src/voice.rs)):

- Add a `channel: u8` field carried from note-on → allocator → the assigned
  voice. Voice **stealing must adopt the stealing note's channel**.
- Add per-voice **pressure state** (`f32`), updated by:
  - **poly/per-note pressure** (MPE): applies only to the voice(s) matching that
    note+channel;
  - **channel pressure**: broadcasts to every active voice on that channel.
- Expose the per-voice pressure as an engine-readable value (the matrix source
  hookup lands in 0202's evaluator; the `SourceId::Aftertouch` enum in 0201).

No matrix dependency in this ticket — it is pure voice-allocation architecture.

## Acceptance criteria

- [ ] Note-on carries and stores the MIDI channel on the assigned voice.
- [ ] Per-note pressure updates **only** the matching voice's pressure state;
      it does not leak to voices on other channels or other notes.
- [ ] Channel pressure updates **all** active voices on that channel.
- [ ] A stolen voice re-parents to the stealing note's channel (test).
- [ ] Per-voice pressure is exposed for later matrix consumption.
- [ ] Allocator/voice tests cover: per-note isolation, channel broadcast,
      steal-reparent. Allocation-free on the audio thread.

## Notes

- This is the highest-risk new engine work in E036 — cover with allocator unit
  tests, not just a DAW smoke play.
- Channel-mode (non-MPE) controllers send channel pressure on one channel → the
  broadcast path is the fallback; MPE controllers spread notes across channels →
  the per-note path. Both share this plumbing.
- Keep the pressure fold cheap and per-block-friendly; the evaluator samples it
  at control rate (sr/32).
- Depends on 0197 (scaffold). Feeds 0201 (SourceId) and 0202 (evaluator).
</content>
