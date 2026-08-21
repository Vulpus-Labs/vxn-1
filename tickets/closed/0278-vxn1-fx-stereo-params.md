---
id: "0278"
product: vxn-1
title: "VXN1: Phaser Stereo + Delay Ping-Pong params on the FX bus"
priority: medium
created: 2026-08-21
epic: null
depends: ["0277"]
---

## Summary

Surface the two kernel controls added in
[0277](0277-fx-stereo-kernels-vxn-dsp.md) on VXN1's global param table and
faceplate:

- **Phaser Stereo** — the L/R LFO sweep offset, today pinned anti-phase.
- **Delay Ping-Pong** — feedback crossfeed on/off, today always on.

## Design

Two new `GlobalParam`s in
[params.rs](../../vxn-1/crates/vxn-app/src/params.rs#L785-L814), appended to the
Phaser and Delay blocks:

- `phaser_stereo` — "Phaser Stereo", `0..180`, default **180**, unit `°`,
  `Taper::Linear`. Maps to the kernel's `spread` as `deg / 180`.
- `delay_pingpong` — "Ping-Pong", bool, default **on** (1.0) — preserves the
  sound of every existing patch, and matches how VXN1's delay has always run.
  (VXN2 defaults its `delay-pingpong` *off*; the two differ deliberately,
  each keeping its own history.)

Fan them into the kernels at
[lib.rs:233](../../vxn-1/crates/vxn-engine/src/lib.rs#L233) and
[lib.rs:249](../../vxn-1/crates/vxn-engine/src/lib.rs#L249). Both are stepped
block-rate values, not smoothed: `spread` only re-aims the LFO read offset (no
zipper — the offset feeds a triangle lookup, not a gain), and the crossfeed
toggle changes the write routing. If the toggle proves to click on a live
delay tail, gate it behind the existing FX fade rather than smoothing a bool.

Faceplate: add a fader to `.fx-pane-phaser` and a switch strip to
`.fx-pane-delay` in
[faceplate.html:349-364](../../vxn-1/crates/vxn-ui-web/assets/faceplate.html#L349-L364),
following the `delay_sync` switch-strip idiom already in the delay pane.

## Acceptance criteria

- [ ] `phaser_stereo` and `delay_pingpong` exist in `GLOBAL_PARAMS`, with
      `GlobalParam::COUNT` assertions still green
      ([params.rs:880](../../vxn-1/crates/vxn-app/src/params.rs#L880)).
- [ ] Both reach their kernels: a param sweep changes rendered output, and the
      defaults (180°, ping-pong on) render identically to pre-0277 VXN1.
- [ ] Faceplate shows a Stereo fader in the Phaser pane and a Ping-Pong switch
      in the Delay pane; both round-trip host → UI → host.
- [ ] Preset save/load and host state carry both params; `cargo test -p
      vxn-app -p vxn-engine` and the JS faceplate tests green.

## Notes

- Land with [0279](0279-vxn1b-fx-stereo-params.md) in the same commit if the
  parity oracle is touched — defaults keep it green either way, but the two
  synths' FX surfaces are easier to review together.
- Web build: the wasm faceplate reads the same param table, so no separate
  wiring beyond a rebuild ([[vxn-web-publish-flow]]).
- Listening check in Reaper before close ([[verify-audio-in-reaper]]): sweep
  Stereo 180 → 0 on a phaser patch, toggle Ping-Pong on a long delay tail.


## Close-out (2026-08-21)

- `GlobalParam::PhaserStereo` (`phaser_stereo`, 0–180°, default 180) and
  `GlobalParam::DelayPingPong` (`delay_pingpong`, bool, default on) added to
  the enum and `GLOBAL_PARAMS`
  ([params.rs](../../vxn-1/crates/vxn-app/src/params.rs)); the
  `GlobalParam::COUNT` partition assertions pass unchanged.
- Fanned into the kernels at
  [lib.rs:233](../../vxn-1/crates/vxn-engine/src/lib.rs#L233) (degrees / 180 →
  `spread`) and [lib.rs:249](../../vxn-1/crates/vxn-engine/src/lib.rs#L249).
  `PhaserStereo` joined the block-glide arm in
  [smoothing.rs](../../vxn-1/crates/vxn-engine/src/smoothing.rs) with the other
  phaser knobs; the bool snaps.
- New test file `tests/fx_stereo.rs`: `phaser_stereo_widens_the_image` (side
  RMS at 180° > 1.5× the 0° lockstep case) and
  `delay_pingpong_changes_the_stereo_image` (chorus decorrelates the bus
  upstream, then the two routings diverge). Defaults still render the
  `baseline_render_is_stable` golden.
- Faceplate: Stereo fader in the phaser pane, Ping-Pong switch in the delay
  pane ([faceplate.html](../../vxn-1/crates/vxn-ui-web/assets/faceplate.html));
  the FX pane grid went 4→5 columns with Mix re-pinned to the last column so it
  still lines up across tabs. `control_tallies_match_all_rows` re-tallied to
  64 faders / 15 switches.
- Both params ride the generic codecs: `write_state_bytes` walks the whole id
  space and `preset_toml` iterates `GlobalParam::all()`, so state and sparse
  TOML carry them by name. State `VERSION` bumped 1→2 in **both** codecs
  ([vxn-app](../../vxn-1/crates/vxn-app/src/state.rs#L40),
  [vxn-engine](../../vxn-1/crates/vxn-engine/src/state.rs#L30)) — they must
  write identical bytes, which `codec_matches_legacy_plugin_state` pins.
- Knock-on: `vxn-wasm`'s `id_layout_matches_vxn_app` now asserts 167 = 2×69 +
  29. Docs updated ([effects.md](../../vxn-1/docs/src/panels/effects.md),
  [parameter-reference.md](../../vxn-1/docs/src/parameter-reference.md)).
