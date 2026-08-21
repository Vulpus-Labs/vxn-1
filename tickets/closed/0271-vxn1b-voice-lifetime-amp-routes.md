---
id: "0271"
product: vxn-1b
title: "Voice lifetime belongs to the Amp routes, and freeing declicks"
priority: high
created: 2026-08-21
epic: E039
---

## Summary

A released lane used to stay allocated until **both** envelopes went idle,
regardless of what those envelopes were routed to. That predicate is inherited
from VXN1, where Env 2 *was* the VCA — but in VXN1b `Amp` is an ordinary matrix
destination, and the assumption "an envelope finishing means the note is
finished" stopped being true.

Two bugs came out of it, found by playing a patch with LFO → Amp:

1. **An unrouted envelope held voices open.** Env 1 with a long release kept a
   lane allocated long after Env 2 had closed it — invisible in a sparse patch,
   a polyphony leak in a dense one.
2. **The held lane kept sounding.** With `LFO → Amp` contributing to the VCA,
   the lane held open by Env 1 stayed *audible* after its amp envelope had
   finished. Note-off did not end the note.

And latent behind both: freeing a lane sets `active = false`, so the VCA drops
to zero on the next sample with no ramp — an audible click at whatever level
the tail was at.

## Decisions

- **Only envelopes routed to `Amp` hold a lane open.** They are the only ones
  that can end a note, so they own the lifetime. An envelope routed elsewhere
  (filter sweep) or nowhere at all no longer keeps a silent lane allocated.
- **Freeing always goes through an 8 ms declick ramp.** In the ordinary case the
  VCA is already at zero when the amp envelope idles, so the ramp costs 8 ms of
  silence; in the LFO-held case it is the difference between a fade and a step.
- Together these give the degenerate case a sensible answer: a patch with **no**
  Env→Amp route at all has nothing an envelope can close, the predicate is
  vacuously true at gate-off, and the note ends on the ramp.

## Acceptance criteria

- [x] `amp_envelopes(&MatrixTable)` — which envelopes reach `Amp` at all.
      Topology + depth, **not** `AmpCoeffs::e1/e2`: those collect only
      `Lin`-curve Env→Amp slots (curved ones fold into `stat`), and a curved
      Env→Amp route must silence its note like a linear one.
- [x] The free predicate holds a released lane only while an Amp-routed
      envelope is still running.
- [x] `RenderBank::free_fade` per-lane declick, `FREE_FADE_SECS = 8 ms`,
      applied to the VCA in both the per-frame and block-constant paths; a
      stolen lane mid-ramp resets to full gain on trigger.
- [x] Any re-gate cancels a ramp in flight, checked in the render loop rather
      than only in `trigger_lane`: a legato slide onto a widened Solo stack
      re-gates a lane with no trigger, and since the ramp only advances while
      the gate is low, a part-faded lane would otherwise stay quiet for the
      whole note.
- [x] Tests: unrouted envelope doesn't hold, filter-only envelope doesn't hold,
      Amp-routed envelope does hold under every curve, an LFO-only Amp patch
      ends its note, and the tail ramps out over ~8 ms rather than stepping.

## Notes

A `scale_src` sitting at zero is deliberately ignored when deciding whether an
Env→Amp route exists: a route that exists counts, so a momentarily gated VCA
cannot make a note un-endable.

A re-gate snaps the gain back to 1.0 rather than ramping up. That is a step, but
only reachable on a legato slide onto a lane inside its 8 ms ramp — an
articulation boundary, where the alternative (a permanently quiet lane) is
clearly worse.

`amp_env_bypass` (organ mode) is untouched — it returns `gate ? 1 : 0`, so its
note-off step is unaffected by the ramp. Worth a separate look if it clicks.

## Close-out

Landed 2026-08-21 in `fafef73`. Files touched: `vxn1b-engine/src/bank.rs`.

Both decisions shipped as written. A released lane is now held open only by the
envelopes actually routed to `Amp`, and freeing always goes through an 8 ms
declick ramp (`FREE_FADE_SECS`).

The routed-envelope test is topology + depth, deliberately **not** the per-lane
`AmpCoeffs`: those collect only `Lin`-curve Env→Amp slots (the rest fold into
`stat`), and a curved Env→Amp route must silence its note exactly like a linear
one. A `scale_src` sitting at zero is likewise ignored — a route that exists
counts as a route, so a momentarily gated VCA cannot make a note un-endable.

A re-gate cancels a ramp in flight, and that reset lives in the per-frame check
rather than only in `trigger_lane`: a legato slide onto a widened stack can
re-gate a lane *without* a trigger, and since the ramp only advances while the
gate is low, a lane left part-faded there would have stayed quiet for the whole
note.

Tests: `an_unrouted_envelope_no_longer_holds_a_lane_open`,
`a_filter_only_envelope_does_not_hold_a_lane_open`,
`an_lfo_only_amp_patch_still_ends_its_note`,
`an_amp_routed_envelope_holds_its_lane_whatever_the_curve`,
`a_regated_lane_recovers_full_gain`, `freeing_a_sounding_lane_declicks`.

**Refactored on landing.** 0274 replaced the standalone `amp_envelopes` walk
with `AmpRoutes::resolve`, which carries `holds_env1`/`holds_env2` off the
block's single Amp scan; 0276 lifted the per-frame lifetime block out of the
render loop into `free_released_lanes`. The behaviour and all six tests are
unchanged — see [0274](0274-vxn1b-amp-slot-walk-single-pass.md) and
[0276](0276-vxn1b-bank-render-decomposition.md).

**Not verified by automated test:** the audible result — in particular that the
declick is inaudible on an ordinary patch and effective on an LFO→Amp one.
Needs a listen in Reaper.
