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

## Close-out (2026-06-15)

- **Track model + `Engine` trait.** Named `TrackEngine`
  ([track_engine.rs](../../vxn-3/crates/vxn3-engine/src/track_engine.rs)) to
  avoid colliding with the instrument-level `Engine`. One active
  `Box<dyn TrackEngine>` per track, per-block vtable dispatch; `render()` runs a
  monomorphic SoA lane loop with **no in-loop match** (LANES=4). `on_trig` is
  engine-defined voicing (poly voice-alloc here; resonator excite reuses the
  same surface in 0049). 8 tracks in
  [engine.rs](../../vxn-3/crates/vxn3-engine/src/engine.rs) (`N_TRACKS`).
- **`Kick/Tone` poly engine.**
  [kick_tone.rs](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs):
  4-wide `[f32;4]`/`[u32;4]` SoA, branchless one-pole-attack × exp-decay amp
  env + multiplicative pitch-sweep (no per-lane stage branch), round-robin /
  oldest-steal alloc. Tests `trig_produces_sound_then_decays` (audible body +
  decay, lane freed), `pitch_sweeps_downward` (higher note → more zero
  crossings, i.e. pitch tracks note → tonal stab), `voices_overlap_up_to_lane_budget`.
- **Sample-accurate sequencer.**
  [sequencer.rs](../../vxn-3/crates/vxn3-engine/src/sequencer.rs) 16-step grid
  (`Pattern::len` per-track, ready for 0048 polymeter);
  [track.rs](../../vxn-3/crates/vxn3-engine/src/track.rs) `render_block` slices
  each block at 16th boundaries (half-open interval → no double-fire). Test
  `trig_scheduling_is_sample_accurate_and_block_size_invariant`: recorded trig
  sample positions are exact (`i*6000` @120BPM/48k) and **identical across block
  sizes 64/512/1000** — block-size invariance is the sample-accuracy proof.
- **4-on-floor at host tempo, transport-gated.** Test
  `four_on_the_floor_is_audible_and_transport_gated` — audible (rms>0.02) while
  playing, silent (<1e-5) when stopped (no trigs fire). Driven by a simulated
  host transport feeding `song_pos_beats` per block (the 0046 CLAP path already
  proven to surface transport). *NB:* programming a pattern through the CLAP
  boundary needs the param/UI path (0052) — so the audible proof is engine-level
  with simulated transport; the plugin is silent-by-default in a DAW until the
  faceplate lands.
- **Off-thread engine swap, no audio alloc/free.**
  [swap.rs](../../vxn-3/crates/vxn3-engine/src/swap.rs): two fixed SPSC rings;
  install **moves** boxes (never alloc/free on the audio thread), old engines
  retired to the main thread to drop, swap deferred when the retired ring is
  full. Tests `swap::install_swaps_and_retires`, `swap::defers_when_retired_unreclaimed`,
  and `engine_swap_is_alloc_free_and_does_not_click` (0 allocs via counting
  allocator; peak < 1e-6 at the swap point). *NB:* click-free is verified for a
  swap at silence; swapping a sounding track cuts its tail — a crossfade is
  post-MVP.
- **Allocation-free process callback.** Test
  `process_block_is_allocation_free` — 0 allocs over 200 blocks with all 8
  tracks triggering (per-binary thread-local counting allocator).
- Workspace builds; all vxn3 tests pass (dsp 6, engine 6 + groove 4, clap 3 +
  smoke 3); vxn3 crates clippy-clean; `clap-validator validate` 0 failures.
