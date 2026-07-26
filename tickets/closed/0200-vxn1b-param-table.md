---
id: "0200"
product: vxn-1b
title: "VXN1b param table: fork VXN1 flat table, strip fixed mod-panel depths, add 16 slot depths"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

Fork VXN1's flat, index-addressed parameter table into `vxn1b-engine` and
reshape it for matrix modulation
([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §5). Keep the
synthesis params; **remove** the fixed-panel modulation depth/selector params
that the matrix replaces; **add** 16 automatable **slot-depth** params.

Source of truth to fork: VXN1's `PatchParam` enum in
[vxn-app/src/params.rs](../../vxn-1/crates/vxn-app/src/params.rs) and the
accessor layer in [vxn-engine/src/params.rs](../../vxn-1/crates/vxn-engine/src/params.rs).

**Keep:** osc1/osc2 (wave/oct/coarse/fine/PW), cross-mod type+amount, mixer
levels, noise, filter (cutoff/reso/mode/HPF/drive), Env1/Env2 (ADSR+shape),
LFO1/LFO2 (rate/shape/sync/delay/fade/free-run), voice/assign, master. Pitch-bend
range stays a **hardwired** global (§3).

**Remove (matrix replaces them):** `PitchLfoSrc/Depth`, `PitchEnvSrc/Depth`,
`PitchWheelDepth`, `PwmLfoSrc/Depth`, `PwmEnvSrc/Depth`, `CutoffLfo1Depth`,
`CutoffLfo2Depth`, `CutoffEnvDepth`, `VelCutoffDepth`, `AmpLfoSrc/Depth`,
`ModWheelPwm/Cutoff/Reso/CrossModSweep`, `CrossModSweepEnvSrc/Depth`,
`FilterKeyTrack`, and the `LfoSel`/`EnvSel` per-channel selectors.

**Add:** 16 `MatrixSlotNDepth` params (bipolar), CLAP-automatable.

## Acceptance criteria

- [ ] `vxn1b-engine` has its own flat `ParamId`=CLAP-id=index table; `ParamDesc`
      display/format coverage is total (every param formats).
- [ ] All listed fixed-panel mod params are gone; 16 slot-depth params exist and
      are automatable.
- [ ] Synthesis params (osc/mixer/filter/env/LFO/voice/master) carry over with
      VXN1 ranges/defaults; pitch-bend range remains a hardwired global.
- [ ] Enum-index / display round-trip tests pass.

## Notes

- No CLAP id-stability constraint pre-release (`vxn1-id-stability-dropped`) —
  reshape freely; keep the table clean.
- Slot *topology* (source/dest/curve/scale_src) is **not** in this table — it is
  patch state (0201 data model, 0203 persistence). Only slot **depths** are params.
- FX params (per-effect on/off + wet) are added later by [[E037]] 0206, not here.
- Depends on 0197. Feeds 0201 (matrix model references slot indices) and 0202.
</content>

## Close-out (2026-07-26)

- Single flat table (no VXN1 Upper/Lower split), `ParamId` = CLAP id = index:
  [params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs). Fixed mod-panel
  depths/selectors removed, 16 bipolar `MatrixSlotNDepth` params added; pitch-bend
  range kept as the hardwired `PitchBendRange` global (ADR §3). Synthesis params
  carry VXN1 ranges/defaults verbatim. Tests: `params::tests::{table_len_matches_count,
  index_roundtrips_for_every_param, every_param_formats, fixed_mod_panel_params_are_gone,
  sixteen_bipolar_slot_depths_exist, pitch_bend_range_is_a_hardwired_global,
  enum_display_and_parse_roundtrip}`.
