---
id: "0316"
product: vxn-1b
title: "Cross-language tables transcribed 2-4 times with no test binding them"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0309"]
---

## Summary

VXN1b's Rust and JS agree on several tables by hand. Two reviewers found the
same seams from opposite sides, which is itself the argument: the duplication is
discoverable from either end and pinned from neither.

Note what is **not** on this list: parameter descriptor metadata. That has
exactly one source (`vxn1b_engine::params::desc_for_clap_id`) and flows outward
correctly. The problem is confined to the hand-written vocabularies around it.

### 1. The custom-op vocabulary — three transcriptions

The `"lower"` → L2 and `"source"|"dest"|"curve"|"scale"` → `MatrixField`
mappings exist in:

- [vxn1b-ui-web/src/lib.rs:117-163](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L117-L163)
  as string matches,
- [web-controller/src/lib.rs:878-886](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L878-L886)
  (`matrix_field_from_wire`) as ordinals,
- [faceplate-bridge.mjs:88-101](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L88-L101)
  (`LAYER`, `MATRIX_FIELD`) plus its `set_matrix` / `copy_layer` arms at
  [:178-198](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L178-L198)
  as JS tables.

Three independent writings of one enum ordering. Nothing tests them against each
other.

### 2. The telemetry payload shape — reimplemented verbatim in JS

[`serialise_custom_payload`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L181-L232)
and
[`meterEvent` / `scopeEvent`](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L110-L134)
produce the same frames: same key names (`l1`/`l2`/`dynIn`/`dynOut`/`dynGr`/
`master`, `kind:"scope"`, `s`), same ±2.0 clamp, same 3-decimal rounding — and
the comment *"One value, not a pair — the compressor's detector is
stereo-linked"* appears **word for word in both files**. Nothing binds them.

### 3. `MATRIX_SLOTS` and friends

- `LAYERS = 2` / `SLOTS_PER_LAYER = 16` re-declared at
  [controller.mjs:52-56](../../vxn-1b/crates/vxn1b-wasm/web/controller.mjs#L52-L56),
  duplicating `LAYER_COUNT` / `MATRIX_SLOTS` from `event-codec.mjs` which
  `faceplate-bridge.mjs:84` imports properly. The comment justifies it as
  anti-masking; no test compares the two pairs, so it is duplication with the
  rationale of a guard and none of the effect.
- `const MATRIX_SLOTS = 16` at
  [matrix.js:20](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L20)
  duplicates `pub const MATRIX_SLOTS: usize = 16`
  ([params.rs:354](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L354)); it
  could read `matrixData().slots[0].length`.

### 4. Split-point range — four transcriptions

`12` / `96` / `60` appear at
[dispatch.js:722-724](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L722-L724),
in `panels/keys.js:28-32` (dying with [[0310]]), as literal `min`/`max`/`value`
on [faceplate.html:438](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L438),
and as `DEFAULT_SPLIT_POINT: u8 = 60` in
[engine.rs:69](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L69).

### 5. Wave glyph keys

[fader.js:19-36](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js#L19-L36)
— `WAVE_GLYPHS` keys (`Sine`/`Triangle`/`Tri`/`Saw`/`Saw+`/`Saw-`/`Pulse`/
`Square`/`S&H`) must match `WAVE_LABELS` / `LFO_LABELS` at
[params.rs:550,555](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L550) exactly.
A Rust-side relabel yields a **blank glyph**, silently: `glyphPath` returns
`null` and the `d` attribute is skipped. All nine currently match.

### 6. `"Uncategorised"` ×3 and the panel file list ×2

[web-controller:86](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L86),
[ui-web:54](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L54), and the shared
default — with a comment at web-controller:84-86 warning the two *"must not
disagree"*, which is a comment doing a constant's job. And the eight `PANEL_*_JS`
consts at [ui-web:593-616](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L593-L616)
are declared and then re-listed in `PANELS_FILES`, with the splice-order
constraint documented four separate times.

## Design

Not every one of these deserves a single source of truth — crossing a language
boundary costs something, and a fixture test is often the better trade. Decide
per item, and prefer in this order:

1. **Collapse** where it is free within one language: `Uncategorised` → one
   `pub const` in `vxn-core-app`; `PANELS_FILES` → one ordered
   `&[include_str!(...)]` list; `controller.mjs`'s `LAYERS`/`SLOTS_PER_LAYER` →
   import from `event-codec.mjs`; `matrix.js`'s `MATRIX_SLOTS` → read from
   `matrixData()`; the split range → stamp the input's `min`/`max`/`value` from
   the JS constants in `wireSplit`.
2. **Generate** where the page is already generated: the layer/field ordinals
   can be exported from `vxn1b-engine` and spliced into the page the way
   `window.vxn.matrix` vocab already is, leaving one Rust decoder.
3. **Pin with a fixture test** where collapsing is genuinely not worth it: the
   telemetry payload shape (assert the two produce identical JSON for the same
   inputs), and the wave glyphs (assert every enum variant of `osc*_wave` /
   `lfo*_shape` in the params JSON has a `WAVE_GLYPHS` entry).

Whichever is chosen, the outcome must be that **drift fails a test**, not that
drift is documented as forbidden.

## Acceptance criteria

- [ ] Each of the six items has a recorded verdict: collapsed, generated, or
      pinned by a test.
- [ ] A deliberately drifted copy fails CI for every item. Verify by hand at
      least for the custom-op vocabulary and the wave glyphs, which are the two
      that fail *silently* today (dropped opcode; blank glyph).
- [ ] No comment survives that warns two constants "must not disagree" without a
      mechanism enforcing it.
- [ ] `cargo test -p vxn1b-ui-web -p vxn1b-web-controller` and both JS suites
      green under [[0309]].

## Notes

- The wave-glyph case is the sharpest illustration of why these matter: the
  failure mode is a control that renders with no icon, on a synth whose faceplate
  is its whole interface, from a Rust-side rename that touched nothing else.
- [[0312]] handles the *wire format* duplication (slot layout ×3) separately —
  it is the same class of problem but its fix is deleting an encoder, not adding
  a test.
- The `dispatch.js` ↔ `panels/matrix.js` ESM cycle
  ([matrix.js:18](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L18)
  imports `paramIdByNameAtLayer` while `dispatch.js:875` calls back into
  `matrixOverlay`) survives only because the splice loader flattens everything
  into one scope. Extracting a small `model.js` both sides import is the same
  shape of fix and belongs here if it is cheap.
