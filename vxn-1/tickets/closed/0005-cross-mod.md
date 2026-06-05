---
id: "0005"
title: Cross-mod / linear FM (osc2 → osc1 pitch)
priority: high
created: 2026-05-24
epic: E002
---

## Summary

Add cross-modulation: osc2's output modulates osc1's pitch per sample (the JP-8
Cross Mod slider — VCO-2 output changes VCO-1 pitch). Produces metallic /
ring-mod / sideband tones, and LFO-style wobble when osc2 is slow. Builds on the
coupled osc path from 0004.

## Acceptance criteria

- [x] Extended the coupled path (0004's `process_pair_synced` generalised to
      `process_pair(slave, sync, xmod, …)`): osc1's per-sample increment is
      `inc1 = base_inc1 * fast_exp2(xmod * o2)` (osc2 evaluated first; the
      modulated increment is also the polyblep `dt`, so band-limiting tracks the
      instantaneous frequency).
- [x] New float param `CrossMod` (depth 0..1, default 0), **appended at the end
      of the `ParamId` table**; `cross_mod: f32` on `BlockCtx` via `build_ctx`;
      block-rate smoothed (like the level/PW depth params).
- [x] At `CrossMod == 0` the coupled kernel is bit-identical to the fast path
      (`fast_exp2(0) == 1.0` exactly, no reset) — proven by
      `coupled_xmod_zero_matches_fast_path` — and the engine selects the coupled
      path only when sync is on **or** cross-mod ≠ 0, so plain patches are
      untouched.
- [x] Exponential (semitone-domain) FM keeps the perceived pitch centred as
      depth rises; documented on `process_pair`. Combining with sync is valid and
      stable (`synced_pair_all_lanes_finite` runs sync + heavy cross-mod together).
- [x] Tests: `cross_mod_adds_spectral_content` (DSP) and
      `cross_mod_adds_content_and_stays_finite` (engine) measure the f1+f2
      sideband via a Hann-windowed single-bin DFT (≈0 at depth 0, present at
      depth > 0); `coupled_xmod_zero_matches_fast_path` covers depth-0 equality
      incl. a frozen lane; finite under combined sync + cross-mod.

## Notes

- Exponential (musical) FM via `exp2` keeps perceived pitch centred as depth
  rises; document the choice. (Linear-in-Hz is the alternative; exp matches the
  rest of the engine's semitone-domain pitch maths.)
- This is inherently aliasing-prone at high depth/ratio; same posture as 0004 —
  lean on oversampling for v1, note the limitation.
- Keep the inner loop branchless; `o2` is already in a lane register from the
  osc2 pass, so the extra cost is one `exp2` and a multiply per lane.
- Depends on 0004 (coupled path). Validation:
  `cargo test -p vxn-dsp -p vxn-engine`.
