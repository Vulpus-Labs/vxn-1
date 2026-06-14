---
id: "0015"
product: vxn-1
title: Smoothing policy for chorus/delay/LayerLevel/Spread
priority: high
created: 2026-06-10
epic: E011
---

## Summary

Eight automatable params have no smoothing anywhere in the
chain: ChorusRate, ChorusDepth, ChorusMix, DelayTime,
DelayFeedback, DelayMix (globals), LayerLevel and Spread
(patch). They are absent from both glide tables in
`vxn-engine/src/smoothing.rs:68-98`, and the DSP-side
`set_params` calls snap their targets with no internal ramp
(`vxn-dsp/src/delay.rs:99-116`, `chorus.rs:132-138`).
Host automation therefore steps at control-block rate:

- **DelayTime** is the worst — `delay_samples_l/r` jump
  moves the read pointer instantly, producing a click or
  pitch chirp per step.
- **Spread** recomputes pan for all 8 voice slots; a step is
  an audible image jump.
- Mix/feedback/level params zipper.

Give each param an explicit policy: block-rate glide in
`ParamSmoother` (the existing pattern), or a documented
DSP-internal ramp where glide is the wrong tool (DelayTime
likely wants either a slewed delay-time with the existing
fractional read, or crossfaded taps).

## Acceptance criteria

- [ ] Every param listed above appears in a `patch_glide` /
      `global_glide` arm or carries a comment in
      `smoothing.rs` stating where its ramp lives instead —
      no silent omissions remain.
- [ ] DelayTime automation sweep produces no discontinuity:
      engine-level test renders a sweep over N blocks and
      asserts no sample-to-sample jump above a threshold in
      the wet path (compare against current behaviour first
      to pin the failure).
- [ ] Spread step (0 → 1 in one block) produces per-voice
      pan coefficients that move over the glide window, not
      in one block — unit test on the pan computation or
      rendered L/R energy.
- [ ] Mix-class params (ChorusMix, DelayMix, DelayFeedback,
      LayerLevel) glide with the existing block-rate one-pole
      + snap-epsilon pattern.
- [ ] ChorusRate/ChorusDepth decision documented — rate may
      legitimately stay snapped (LFO phase is continuous
      across rate changes; verify and say so in the comment).
- [ ] `tests/baseline.rs` hash updated if smoothed renders
      change it, with a note in the commit explaining the
      delta is this ticket.
- [ ] Manual listen: automate DelayTime and Spread in a host;
      no clicks.

## Notes

Follow the existing discipline: cutoff/reso/drive are
deliberately excluded from `ParamSmoother` because
`PolyOtaLadder` ramps coefficients internally, and the
comment at `smoothing.rs:17-19` says so. This ticket extends
that explicit-decision style to the FX/mixer params — the
review finding is not "everything must glide", it is "these
eight have no decision recorded and audibly artifact".

The glide-snap epsilon (`GLIDE_SNAP_EPS`) matters for the
silent-skip fast path; keep new glides on the same pattern
so idle cost does not regress (memory: silent-skip freezes
filter state — check interaction if DelayTime ramp keeps the
wet path armed longer).
