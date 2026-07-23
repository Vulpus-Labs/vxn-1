---
id: "0176"
product: vxn-2
title: Mod-matrix scale source — hot-path scaling in eval_dests
priority: medium
created: 2026-07-03
epic: E033
depends: ["0175"]
---

## Summary

Make `scale_src` actually gate a slot's depth. In `eval_dests`, multiply each
slot's per-lane contribution by the **normalised** value of its scale source,
read from the `[lane][source]` table the eval already holds. `None → 1.0`
(identity). Add the shared `scale_norm(SourceId, f32) -> f32` helper defining the
unipolar-vs-bipolar mapping.

## Acceptance criteria

- [ ] `scale_norm` maps unipolar sources (`mod_wheel`, `aftertouch`, `velocity`,
      `key`) as passthrough and bipolar sources (LFOs, `pitch_eg`,
      `voice_spread`, `voice_rand`) via `(x + 1) × 0.5` clamped to `[0, 1]`.
- [ ] `eval_dests` multiplies each slot·lane contribution by
      `scale_norm(slot.scale_src, table[lane][scale_src])`; `None` short-circuits
      to `1.0` without reading the table.
- [ ] Multiply lands outside the per-lane curve dispatch; hot path stays
      allocation-free (alloc-trap test extended to a scale-source patch).
- [ ] Tests: a `mod-wheel` scale route outputs 0 at wheel 0 and full depth at
      wheel 1; a bipolar scale source follows `(x+1)×0.5`; a patch with all
      `scale_src = None` matches the pre-epic render hash (regression).

## Notes

The scale value is available per-lane at the right granularity from the existing
source table (see `engine.rs` `[lane][source]` build ~L246), so a finer scale
source gating a coarser dest needs no extra broadcast — just index the table at
the slot's lane. Keep the `None` path branch-light so the common (unscaled)
case stays hot. Design + normalisation rationale in
[E033](../../epics/open/E033-matrix-scale-source.md).
