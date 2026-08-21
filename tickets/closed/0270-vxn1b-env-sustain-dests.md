---
id: "0270"
product: vxn-1b
title: "Env 1 / Env 2 Sustain as mod destinations — additive, cooked at note-on"
priority: medium
created: 2026-08-21
epic: E039
depends: ["0268"]
---

## Summary

`Env 1 Sustain` / `Env 2 Sustain` join the time-scale dests of 0268: a per-voice
offset on the envelope's sustain **level**, latched at note-on. Velocity → Env 2
Sustain is the obvious one (hard notes hold, soft notes decay away); mod wheel,
Spread and Note Random work the same way the time scales do.

**Additive, where the time dests are multiplicative.** Sustain is an absolute
`[0, 1]` level, not a duration. A multiplier can never lift a sustain of 0 and
never reach the ceiling from a low one — precisely what a velocity route wants
to do — so the dest total is added to the patch value and the sum clamped.
Depth 1 spans the full range in either direction.

**Latched at note-on**, for a reason stronger than the time scales': sustain is
the envelope's *held* value and also sets its decay rate
(`decay_inc = (1 − sustain)/…`). Tracking it continuously would step a ringing
note and bend a decay already in flight.

The per-lane drift trim (0218) stays multiplicative on sustain; the matrix
offset lands on top of it and the clamp is applied last, so no combination of
the two can leave `[0, 1]`.

## Acceptance criteria

- [x] `DestId::Env1Sustain` / `Env2Sustain` (wire `env1-sustain` /
      `env2-sustain`), `N_DESTS` 14 → 16, `DEST_GAIN` 1.0, linear `cook_depth`.
- [x] `RenderBank::env_sus_mod` latched in `cook_env_mods` beside the time
      scales; `apply_env_lane` folds it in as
      `(patch × drift trim + offset).clamp(0, 1)`.
- [x] A later envelope/drift param change preserves the lane's offset.
- [x] No route → every lane at exactly the patch sustain, render bit-identical.
- [x] Tests: offset up/down against a known patch sustain, both rails reachable
      and clamped, note-on latching + re-cook, param-change survival.

## Notes

`cook_env_scale` (0268) became `cook_env_mods` — it now latches four values per
lane, not two.
