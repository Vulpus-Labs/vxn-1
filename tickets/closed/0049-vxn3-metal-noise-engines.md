---
id: "0049"
product: vxn-3
title: "vxn-3 Metal (resonator) + Noise engines — validate voicing-model split"
priority: high
created: 2026-06-15
epic: E021
depends: ["0047"]
---

## Summary

Add the two engines that prove the `Engine` trait (0047) generalises beyond
poly: a modal resonator and a noise percussion engine. Completes a credible
three-engine minimal-techno kit and validates engine-defined lane semantics,
trigger semantics, and choke (ADR 0001 §5) — the load-bearing novelty.

## Design

- **`Metal` (modal resonator).** Lanes = ~6–8 inharmonic modal partials of
  *one* body, not parallel voices. `on_trig` **injects excitation** into the
  persistent modal state (a re-hit rides the decaying state, partly re-excites)
  rather than allocating a fresh voice. Covers hats, ride, cymbal. Choke =
  **damping** (raise loss / collapse decay), so a closed-hat trig chokes an
  open-hat ring on the same body — *not* a voice kill.
- **`Noise` (perc).** Filtered noise burst + optional short tuned body, per-
  component decay. Covers snare, clap, misc percussion. Poly-style voicing
  (independent short tails) — simplest case.
- **Trait validation.** Both engines plug into the same track slot / per-block
  dispatch / SoA block as `Kick/Tone`. `Metal` exercises the resonator branch
  of the trait (lanes-as-modes, excite, damp-choke); `Noise` the poly branch.
  Confirm the lane budget is engine-declared (Metal may want 8 lanes; Kick 4).

## Acceptance criteria

- [ ] `Metal` produces a metallic ring; a re-trigger re-excites the same body
      (audibly *not* a second independent voice), and a closed-hat trig chokes
      an open-hat ring via damping.
- [ ] `Noise` produces usable snare/clap/perc hits.
- [ ] Both run through the unchanged track/dispatch/SoA framework from 0047;
      the trait needed no poly-specific assumptions.
- [ ] Per-block dispatch keeps the lane loop free of per-sample engine-type
      matching (verify hot path).
- [ ] A pattern using all three engines plays a recognisable minimal-techno
      loop; process stays allocation-free.

## Notes

- If the trait needs reshaping to fit the resonator, that reshape belongs here
  and should flow back into 0047's `Kick/Tone`.
- Out of scope: additional engines, oversampling, FX, p-locks.
- Design: `vxn-3/adrs/0001` §5 (lane semantics, trigger, choke).

## Close-out (2026-06-15)

- **`Metal` (modal resonator).**
  [metal.rs](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs): 8 inharmonic
  modal partials of one body (`METAL_MODES = 8`, the engine-declared lane budget
  ≠ the poly cap of 4 — lane budget is the engine's choice). Each mode is a
  decay-scaled complex rotation (SoA arrays). `on_trig` injects excitation into
  the persistent ring; choke = switching the shared decay coefficient short. GM
  note map: `< closed_below` (44) damps, `≥` rings open. Tests `open_hit_rings`,
  `rehit_reexcites_the_same_body` (post-rehit level > 1.5× the decaying
  pre-hit level → additive re-excitation, not a fresh voice),
  `closed_hit_chokes_open_ring_via_damping` (choked tail < 0.25× the open tail).
- **`Noise` (poly perc).**
  [noise.rs](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs): 4-voice white
  noise + tuned body, per-component exponential decay, one-pole output highpass.
  Poly voicing (independent tails). Tests `idle_is_silent`,
  `trig_produces_perc_then_decays`, `voices_are_independent`.
- **Trait validation (no reshape).** Both engines `impl TrackEngine` and plug
  into the unchanged 0047 `Track` slot / per-block dispatch / SoA block — the
  trait needed no poly-specific assumptions, so 0047's `Kick/Tone` was untouched.
  `engines::make(kind, sr)` builds any engine for the swap channel.
- **Per-block dispatch, no per-sample type match.** Each engine's `render` is a
  single monomorphic function; the only dynamic dispatch is one `Box<dyn TrackEngine>`
  vtable call per track per block (outside the lane loop). Verified structurally
  (no `match` on engine kind anywhere in a render loop).
- **Three-engine kit + alloc-free.**
  [kit.rs](../../vxn-3/crates/vxn3-engine/tests/kit.rs): kick (poly) + hats
  (Metal resonator, incl. an open hat) + snare (Noise) on one engine; test
  `three_engine_kit_plays_a_loop` (audible, finite, kind assertions) and
  `three_engine_kit_is_allocation_free` (0 allocs over ~300 blocks).
- vxn3 tests: dsp 6, engine 12 unit + groove 4 + pattern 8 + kit 2, clap 3 +
  smoke 3 — all pass; vxn3 crates clippy-clean; `clap-validator` 0 failures.
