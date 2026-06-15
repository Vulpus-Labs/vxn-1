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
