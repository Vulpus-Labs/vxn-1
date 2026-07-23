---
id: "0178"
product: vxn-2
title: Mod-matrix scale source — ADR, docs, demo preset, validation
priority: medium
created: 2026-07-03
epic: E033
depends: ["0176", "0177"]
---

## Summary

Close out E033: document the scale-source semantics in an ADR, update
PARAMETERS.md / README, ship a demo preset that uses wheel-gated LFO vibrato
(the DX7 mod-wheel-vibrato case that motivated the epic), and run the validation
pass.

## Acceptance criteria

- [ ] New ADR (or section) records the matrix scale-source model and the
      unipolar/bipolar `scale_norm` mapping table.
- [ ] PARAMETERS.md and README describe the per-slot scale source.
- [ ] A demo/factory preset routes LFO→pitch with `scale_src = mod-wheel`:
      audibly no vibrato at wheel 0, vibrato in at wheel up.
- [ ] `clap.state` round-trips `scale_src` through save → reload (incl. a
      pre-epic fixture defaulting to None); `clap-validator` reports 0 failures
      and no new param ids.
- [ ] Manual DAW check logged: mod wheel controls vibrato depth on the demo
      preset in Reaper.

## Notes

No new `clap.params` (topology is patch state), so validation is about state
round-trip + zero regressions, not param-count changes. The demo preset doubles
as the acceptance artefact for the whole epic. Depends on 0176 (eval) for audio
and 0177 (UI) so the preset is authorable on the faceplate. See
[E033](../../epics/open/E033-matrix-scale-source.md).

## Close-out (2026-07-23)

- [ADR 0009](../../vxn-2/adrs/0009-matrix-scale-source.md) records the scale-
  source model + the unipolar/bipolar `scale_norm` table (and the `voice_rand`
  polarity call). [PARAMETERS.md](../../vxn-2/PARAMETERS.md) + README updated.
- Demo factory preset **EP Wheel Vibrato** (Keys): LFO1→global-pitch,
  `scale-src = mod-wheel` — silent at wheel 0, ~0.65 st vibrato at wheel up.
  Passes `factory::tests::every_factory_preset_parses_cleanly` +
  `…::no_factory_preset_routes_incoherently`.
- No new `clap.params` (topology is patch state; TOTAL_PARAMS still 208 —
  asserted in the JS codec suite); state round-trips `scale_src` incl. a
  pre-epic blob defaulting to None. clap-validator + Reaper mod-wheel-vibrato
  check confirmed by the user. Landed in `27d8823`.
