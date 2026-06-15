---
id: "0047"
product: vxn-3
title: "vxn-3 track model + Engine trait + Kick/Tone poly engine + step grid + audio out"
priority: high
created: 2026-06-15
epic: E021
depends: ["0046"]
---

## Summary

The foundational audible slice: the track/engine framework, the first engine,
and a basic sequencer — enough to *hear one programmed track* at host tempo.
Establishes the `Engine` trait and per-track SoA block that every other engine
plugs into (ADR 0001 §4/§5).

## Design

- **Track model.** 8 fixed tracks. Each track = a polymorphic slot holding one
  active engine instance + its patch state, plus a per-track 4-wide SoA voice
  block. Engines are *not* all instantiated per track.
- **`Engine` trait.** The load-bearing abstraction. Covers: per-block `render`,
  trigger handling (`on_trig`), and the declared lane budget. Built so the
  voicing model is the engine's choice (poly here; resonator in 0049) and so
  there is **no enum match inside the lane loop** — dispatch per-block via
  marker/macro (vxn-1/vxn-2 hoist lesson).
- **Off-thread engine swap.** Engine instance built on the main thread,
  pre-allocated, handed to the audio thread over a lock-free channel; the audio
  thread never allocates and the swap must not click.
- **`Kick/Tone` engine (poly).** 4-voice SoA. Sine/FM body + pitch envelope
  (fast sweep) + amp envelope. One engine that covers kick, tom, bass stab, and
  tonal hit — `on_trig` allocates a voice (round-robin / oldest-steal).
- **Basic sequencer.** Per-track fixed-length step grid (16 for now; polymeter
  is 0048), clocked off the 0046 transport, sample-accurate trig scheduling at
  block boundaries (not block-quantised). Plain on/off trigs only.
- **Mix + out.** Sum tracks → stereo output (pan/gain per track, simple).

## Acceptance criteria

- [ ] A track running `Kick/Tone` plays a programmed 4-on-the-floor at host
      tempo, starting/stopping with transport.
- [ ] Pitch-env sweep and amp-env decay are audible; pitch tracks note so the
      same engine yields a tonal stab as well as a kick.
- [ ] Trigs are sample-accurate to the host clock across block boundaries.
- [ ] Swapping a track's engine instance does not click and does not allocate
      on the audio thread.
- [ ] Process callback is allocation-free (alloc-trap clean).

## Notes

- The `Engine` trait shape decided here is reused by 0049; design it for a
  resonator (lanes-as-modes, excite-not-spawn) to slot in without rework.
- Out of scope: polymeter / probability / retrig (0048), other engines (0049),
  p-locks (0050), FX (0051), UI (0052).
- Design: `vxn-3/adrs/0001` §4 (engine slot) + §5 (SoA lane semantics).
