---
id: "0186"
product: vxn-3
title: "vxn-3 MIDI free-play — CLAP note input port, note→track/voice map, audition/jam by hand"
priority: high
created: 2026-07-04
epic: E034
---

## Summary

Add a CLAP **note input port** to VXN3 and route incoming host MIDI notes to tracks /
voices, so voices can be auditioned and jammed **by hand** — not only via the step
sequencer. This is the cheapest path to a playable toy and the whole point of the
E034 pivot: you can't discover what a sound should be if you can't trigger it live.
Independent of the flavour chain (0180–0185) — land it early, in parallel.

Design: [ADR 0001](../../vxn-3/adrs/0001-vxn3-overall-design.md) §voicing (percussion
vs note is envelope + pitch-tracking over a shared engine, not a separate path) +
[ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md) (families /
flavours the notes trigger). No dependency on 0179/0180; composes with them once they
land.

## Design

- **Note input port.** VXN3 currently declares **0 note inputs** (sequencer is the
  sole trig source). Register a CLAP note input port (CLAP + MIDI dialects) on the
  clap shell; handle `NoteOn` / `NoteOff` / choke in the process event loop,
  **sample-accurately** at their event time (same block-slicing discipline as the
  transport/lane scheduler, [lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs)).
- **Note → track/voice map.** A note triggers a track's engine as a trig would.
  Default mapping: a **General-MIDI-drum-ish** note→track layout (kick/snare/hats/…)
  so a standard drum controller "just works", plus a straightforward chromatic option
  for pitched play (the engines already take a fractional MIDI note per step). Keep
  the map a small, explicit table — not hardcoded scattered constants.
- **Velocity + note.** Route note velocity → the trig's velocity (0..1) and note →
  the engine's pitch input, reusing the existing per-step `note`/`velocity` plumbing
  in [sequencer.rs](../../vxn-3/crates/vxn3-engine/src/sequencer.rs) so free-play and
  sequenced trigs share one code path.
- **Coexistence with the sequencer.** Free-play notes and sequencer trigs both feed
  the same voice allocator; a live note must not corrupt sequencer phase or steal a
  playing sequenced voice destructively (respect the existing poly/lane allocation).
  Choke groups (when they land, Phase 1) apply to both sources.
- **RT discipline.** Event handling allocation-free on the audio thread; extend the
  alloc-trap test to cover a note-on/off burst.
- **Scope.** Pattern/bank switching via MIDI note is **out of scope** here (that's the
  Phase-3 arrangement work, needs pattern slots). This ticket is *play a voice by
  hand* only.

## Acceptance criteria

- [ ] VXN3 declares a CLAP note input port (CLAP + MIDI); `NoteOn`/`NoteOff` handled
      sample-accurately in the process loop.
- [ ] A default note→track map triggers the right voice from a standard drum
      controller; a chromatic mode plays a track's engine pitched.
- [ ] Note velocity → trig velocity, note → engine pitch, via the shared
      sequencer trig path (no duplicated trigger logic).
- [ ] Free-play and sequencer trigs coexist without phase corruption or destructive
      voice steal; choke behaviour (where present) applies to both.
- [ ] Event handling allocation-free (alloc-trap test extended); `clap-validator`
      note-ports check passes (it currently skips — intentionally-absent — so this
      flips a skip to a pass); `cargo test -p vxn3-clap -p vxn3-engine` green.

## Notes

- Immediate playability payoff: with this + the sequencer, VXN3 is jammable while the
  flavour work (0180–0185) fills in the sounds.
- The note→track map wants to be user-remappable eventually, but not here — ship a
  sane default table first; remap UI is later.
- Flips the `clap-validator` note-ports skip noted in E032 / 0174's close-out to an
  actual pass.
