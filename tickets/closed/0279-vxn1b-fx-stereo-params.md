---
id: "0279"
product: vxn-1b
title: "VXN1b: Phaser Stereo + Delay Ping-Pong params on the global FX chain"
priority: medium
created: 2026-08-21
epic: null
depends: ["0277"]
---

## Summary

VXN1b's half of the FX-stereo work — the same two controls as
[0278](0278-vxn1-fx-stereo-params.md), on VXN1b's flat param table and
dual-layer CLAP surface.

FX are **global** (not per-layer), so both params sit in the globals block
alongside `phaser_mix` / `delay_sync`
([params.rs:679-691](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L679-L691)).

## Design

- `ParamId::PhaserStereo` — `phaser_stereo`, "Phaser Stereo", `0..180`,
  default **180**, `°`, linear. `ParamId::DelayPingPong` — `delay_pingpong`,
  "Ping-Pong", bool, default **on**. Both added to the `ParamId` list and the
  id → descriptor table, and to the ordering list at
  [params.rs:419-420](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L419-L420).
- Two new fields on `FxParams`
  ([fx.rs:78-90](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L78-L90)), read in
  `from_params`, passed through in `set_params`
  ([fx.rs:214-224](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L214-L224)).
- Outer dual-layer map: globals count grows by 2 (160 → 162). Tests must keep
  using `clap_id_of`, never `as usize` ([[vxn1b-two-layer-param-map]]).
- State v2 and the sparse-TOML preset path pick both up by name; absent keys
  fall back to the defaults, so existing presets and saved host state stay
  sound-identical.
- Faceplate: Stereo fader in the phaser pane
  ([faceplate.html:478](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L478))
  and a Ping-Pong switch strip beside `delay_sync`
  ([faceplate.html:504](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L504)).

## Acceptance criteria

- [ ] Both params exist, are host-visible on the outer map at the grown count,
      and round-trip through state + preset TOML.
- [ ] Defaults render bit-identically to today, and the VXN1 render-parity
      oracle ([parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs))
      stays green.
- [ ] Sweeping Stereo and toggling Ping-Pong each audibly change the render
      (param-audibility style check, no exclusions).
- [ ] Faceplate controls present and round-tripping; `cargo test -p
      vxn1b-engine -p vxn1b-clap` green.

## Notes

- Factory bank presets keep their current stereo behaviour by omitting both
  keys; only new/edited presets carry them. Remember `include_dir` emits no
  rerun-if-changed — touch `factory.rs` before an `xtask install`
  ([[vxn2-include-dir-no-rerun]]).
- Mod-matrix destinations for either control are out of scope.


## Close-out (2026-08-21)

- `ParamId::PhaserStereo` and `ParamId::DelayPingPong` added to the enum, the
  descriptor table, and `GLOBAL_PARAMS` (33 → 35)
  ([params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs)); `TOTAL_PARAMS`
  is now 2×71 + 35 = 177 and `patch_and_global_partition_every_param` passes.
- `FxParams` carries `phaser_stereo` (already normalised to the kernel's
  `spread`) and `delay_pingpong`, read in `from_params` and passed through in
  `set_params` ([fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs)).
- New test file `tests/fx_stereo.rs`:
  `phaser_stereo_widens_the_image` and `delay_pingpong_changes_the_stereo_image`
  — the audibility check the ticket asked for, no exclusions. Defaults keep the
  VXN1 parity oracle green (`default_patch_render_matches_vxn1`).
- Faceplate: Stereo fader in the Phaser panel, Ping-Pong switch under Mix in
  the Delay panel (the same `.ctl-col-center` idiom as Time ↔ Sync)
  ([faceplate.html](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html));
  the Phaser panel's flex-grow went 1.15 → 1.4 to keep fader columns even
  across the row at five faders.
- State `VERSION` bumped 9 → 10
  ([state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L63)) — the layer
  param block is positional, so the layout change needs the gate. Presets are
  name-keyed and sparse (`PARAMS.iter()`), so existing factory/user presets
  load unchanged and fall back to the defaults.
