---
id: "0280"
product: vxn-2
title: "VXN2: phaser spread param + surface the existing Ping-Pong toggle on the faceplate"
priority: medium
created: 2026-08-21
epic: null
depends: []
---

## Summary

VXN2 is half-done on FX stereoness:

- **Delay ping-pong already exists** end to end — `delay-pingpong` in the param
  table ([params.rs:494](../../vxn-2/crates/vxn2-engine/src/params.rs#L494)),
  decoded at
  [shared.rs:1266](../../vxn-2/crates/vxn2-engine/src/shared.rs#L1266), acted on
  at [delay.rs:274](../../vxn-2/crates/vxn2-dsp/src/delay.rs#L274). It is simply
  **not on the faceplate** — host-automation only. The delay pane in
  [index.html:415-439](../../vxn-2/crates/vxn2-ui-web/assets/index.html#L415-L439)
  has Time/Sync/FB/Mix and nothing else.
- **Phaser has no spread control** — like VXN1's, it pins `SPREAD = 1.0` and
  reads the R cascade at `tick_offset(0.5)`
  ([phaser.rs:375](../../vxn-2/crates/vxn2-dsp/src/phaser.rs#L375),
  [phaser.rs:424](../../vxn-2/crates/vxn2-dsp/src/phaser.rs#L424)).

## Design

**Delay** — UI only. Add a `bgrp-toggle` labelled *Ping-Pong* to the delay
pane, next to the Sync toggle. Default stays **off** (VXN2's existing default);
no param, DSP, or offset change, so the render hash is untouched for existing
patches.

**Phaser** — `phaser-spread`, "Phaser Stereo", `0..180`, default **180**, `°`,
linear, appended to the phaser block. `spread = deg / 180` feeds
`tick_offset(0.5 * spread)` in the kernel, same shape as
[0277](0277-fx-stereo-kernels-vxn-dsp.md) gives VXN1's copy — so the two
kernels stay convergent for E041/0228.

The insert is not free: the phaser block grows from 5 to 6 params, which shifts
every later positional offset. All of these move together —

- the hand-written `OFF_*` literals
  ([params.rs:587-593](../../vxn-2/crates/vxn2-engine/src/params.rs#L587-L593)),
- the `const _: () = assert!(id_eq(...))` anchors
  ([shared.rs:157-174](../../vxn-2/crates/vxn2-engine/src/shared.rs#L157-L174)),
- the `param_group` range test `off >= OFF_PHASER && off < OFF_PHASER + 5`
  ([params.rs:669](../../vxn-2/crates/vxn2-engine/src/params.rs#L669)),
- `TOTAL_PARAMS` / the CLAP surface and the web dispatch wire,
- the group-label table around
  [params.rs:915-918](../../vxn-2/crates/vxn2-engine/src/params.rs#L915-L918).

The const asserts are the safety net: a missed offset fails compilation, not
audio.

## Acceptance criteria

- [ ] Ping-Pong toggle visible in the delay pane, round-trips host → UI → host,
      and audibly bounces the tail L↔R when on.
- [ ] `phaser-spread` exists, is host-visible, decodes positionally, and passes
      [param_audibility](../../vxn-2/crates/vxn2-engine/tests/param_audibility.rs)
      with no exclusion.
- [ ] All `id_eq` const anchors updated; `cargo test -p vxn2-engine -p vxn2-dsp`
      green plus the JS faceplate tests.
- [ ] Existing patches sound unchanged: spread defaults to 180° (today's
      anti-phase sweep) and ping-pong stays off.

## Notes

- Preset compatibility: sparse-TOML presets omit the new key and fall back to
  the default, so the 194 translated DX7 voices and the factory bank need no
  edits — but `include_dir` emits no rerun-if-changed, so touch `factory.rs`
  before an `xtask install` ([[vxn2-include-dir-no-rerun]]).
- E041 treats VXN2's kernels as canon with an unchanged render hash; adding
  spread here *is* the canon change, and 0228 should inherit it rather than
  fight it.


## Close-out (2026-08-21)

- Delay: Ping-Pong is now on the faceplate — a `bgrp-toggle` in a
  `.fader-with-toggle` beside Mix
  ([index.html](../../vxn-2/crates/vxn2-ui-web/assets/index.html)). No param,
  DSP, or offset change; `render_hash_unchanged` still passes. `main.js` binds
  every `.bgrp-toggle[data-vxn-param]` generically, so it round-trips with no
  new wiring.
- Phaser: `spread` added to `PhaserParams` / `set_params` / `set_from` and read
  as `tick_offset(0.5 * spread)` on both process paths
  ([phaser.rs](../../vxn-2/crates/vxn2-dsp/src/phaser.rs)), plus
  `phaser-spread` ("Phaser Stereo", 0–180°, default 180) in the param table.
  New `phaser::tests::spread_zero_recorrelates_channels`; the old
  `stereo_decorrelates_on_mono_input` now shares its correlation helper.
- Positional layout moved with it: `N_PATCH_LEVEL` 39 → 40, `TOTAL_PARAMS`
  208 → 209, `OFF_DYNAMICS` 31 → 32, a new `N_PHASER_PARAMS = 6` driving the
  `module_for_patch` range, the `phaser-spread` `id_eq` const anchor in
  [shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs#L172), the snapshot
  decode at `pb + OFF_PHASER + 5`, and the `filter_section_is_at_table_tail` /
  `total_count_matches_layout` offsets.
- `every_param_sweep_is_audible` passes with `phaser-spread` in the sweep — no
  exclusion entry needed. Existing patches are unchanged: spread defaults to
  180° and ping-pong stays off.
- FX pane grid went 4 → 5 columns with Mix re-pinned to column 5; the delay's
  two `.fader-with-toggle` wrappers gained `fwt-time` / `fwt-mix` classes so
  the old `grid-column: 1` rule no longer grabs both
  ([style.css](../../vxn-2/crates/vxn2-ui-web/assets/style.css#L825)).
- Old host state blobs are rejected by the existing exact-count guard (208 ≠
  209) rather than mis-decoded; `BLOB_VERSION` left alone since the count check
  already fails cleanly.
