---
id: "0006"
title: MIDI pitch-bend + mod-wheel routing
priority: medium
created: 2026-05-24
epic: E002
---

## Summary

Wire the MIDI expression hooks deferred since ADR 0001. Pitch bend → pitch only
(predictable, deliberately not routable elsewhere). Mod wheel (CC1) → routable
to cutoff or osc2 pitch with a depth — the latter being the expressive gesture
for hard sync (played pitch from osc1, wheel sweeps the synced osc2 formant).

## Acceptance criteria

- [x] `vxn-clap` `process` now handles `CoreEventSpace::Midi`: pitch bend
      (status `0xE0`, 14-bit, centre 8192 → normalised `[-1,1]`) calls
      `set_pitch_bend`; the channel nibble is ignored (global control).
- [x] CC1 (status `0xB0`, controller 1) calls the new engine hook
      `set_mod_wheel(normalized)`, mirroring `set_pitch_bend`. The latest value
      is stored as a control-rate `Smoothed` (`mod_wheel`), so wheel sweeps don't
      zipper the 7-bit CC steps.
- [x] New params `ModWheelDest` (enum `Off` / `Cutoff` / `Osc2 Pitch`, default
      `Off`) and `ModWheelDepth` (float, ±48 st, default 12), **appended at the
      end of the `ParamId` table**; `ModWheelDepth` is block-rate smoothed.
- [x] `build_ctx` applies the smoothed wheel: dest = Cutoff scales `cutoff` by
      `exp2(wheel*depth/12)` (semitone-domain, can't go ≤ 0 Hz; the ladder then
      interpolates the coefficient per sample = no zipper); dest = Osc2 Pitch adds
      `wheel*depth` semitones to `osc2_semi`.
- [x] Pitch bend rides `base_semis` (shared by both oscillators) so it keeps
      working under sync/cross-mod unchanged.
- [x] Tests (engine): `pitch_bend_shifts_rendered_pitch` (+2 st),
      `mod_wheel_osc2_pitch_shifts_osc2` (+12 st → octave on osc2),
      `mod_wheel_off_is_inert`, `mod_wheel_cutoff_moves_cutoff` (filter opens,
      higher RMS, finite).

## Notes

- Engine already has `bend_semis` + `set_pitch_bend` (±2 st); this ticket mostly
  *connects the event* and adds the symmetric mod-wheel path. Bend-range is left
  hard-coded at ±2 st for now (a `BendRange` param is a later nicety).
- Mod wheel is a global (channel) control, applied in `build_ctx`, not
  per-voice — consistent with how bend rides `base_semis`.
- Osc2-pitch routing is most useful with 0004/0005; it works standalone too
  (just detunes osc2).
- Validation: `cargo test -p vxn-engine`; manual MIDI check in a host for the
  event wiring.
