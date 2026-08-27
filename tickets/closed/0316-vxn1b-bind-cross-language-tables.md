---
id: "0316"
product: vxn-1b
title: "Cross-language tables transcribed 2-4 times with no test binding them"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0321"]
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
      green under [[0321]].

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

## Close-out (2026-08-27)

### Verdict per item

| # | item | verdict |
|---|---|---|
| 1 | custom-op vocabulary | **collapsed** (both Rust decoders) + **pinned** (the JS one) |
| 2 | telemetry payload shape | **pinned** by a shared fixture, both ends |
| 3 | `MATRIX_SLOTS` and friends | **collapsed** |
| 4 | split-point range | **collapsed** to dispatch.js + **pinned** to the engine |
| 5 | wave glyph keys | **pinned** by a test |
| 6 | `Uncategorised` ×3, panel file list ×2 | **collapsed** |

### Collapsed

- **The vocabulary is one table.** New
  [`vxn1b_engine::vocab`](../../vxn-1b/crates/vxn1b-engine/src/vocab.rs) owns
  the wire names *and* the ordinals — the ordinal is the table position, so the
  two mappings cannot disagree. `vxn1b-ui-web`'s four string matches and
  `vxn1b-web-controller`'s `matrix_field_from_wire` both read it. Two
  behavioural improvements fell out: `set_matrix` and `copy_layer` now **drop**
  an out-of-range layer instead of folding anything ≠ 0 onto L2 (`layer_from_wire`),
  and `reset_layer` / `set_scope_source` lost their own copies of the same
  match.
- **`UNCATEGORISED_LABEL`** is one `pub const` in `vxn-core-app`, used by
  `vxn1b-ui-web`, `vxn1b-web-controller` and `vxn-core-ui-web`'s default. vxn-2
  still passes its own string, which is what the parameter is for.
- **`PANELS_FILES`** is one ordered `&[include_str!(…)]`. The splice-order
  constraint — the whole content of the constant — is stated once above it
  instead of four times among the consts.
- **`controller.mjs`** imports `LAYER_COUNT` / `MATRIX_SLOTS` from
  `event-codec.mjs`. The local copies were justified as anti-masking; that is a
  real argument only when something compares the two, and nothing did.
- **`matrix.js`** derives its row count from `mx.slots[0].length` — the
  snapshot it is already rendering.
- **The split range** exists once, as dispatch.js's `SPLIT_MIN` / `SPLIT_MAX` /
  `SPLIT_DEFAULT`. `wireSplit` stamps `min`/`max`/`step`/`value` onto the
  element and `faceplate.html` carries none.

### Pinned

- [`vocab-agreement.test.mjs`](../../vxn-1b/crates/vxn1b-wasm/web/vocab-agreement.test.mjs)
  asserts the page's `LAYER` / `MATRIX_FIELD` / `SCOPE_TAP` / `SPLIT_POINT`,
  `event-codec.mjs`'s field constants, and `controller.mjs`'s `VE_*` /
  `PRESET_SRC_*` / `JW_*` tags against the **built** controller wasm, which
  publishes them through the new `vxnc_vocab_json_ptr` / `_len`. Fails rather
  than skips on a missing artefact ([[0295]]).
- [`telemetry-payload.fixture.json`](../../vxn-1b/crates/vxn1b-wasm/web/telemetry-payload.fixture.json)
  is asserted from both ends —
  `vxn1b-ui-web::tests::telemetry_payload_matches_the_fixture` over
  `serialise_custom_payload`, and `telemetry-payload.test.mjs` over
  `meterEvent` / `scopeEvent`. Every fixture input is exactly representable in
  f32, so the two languages agree bit-for-bit with no epsilon. Change either
  serialiser and one test fails.
- `every_wave_label_has_a_glyph` reads **which** params mount a wave knob out of
  `faceplate.html`'s `data-control="wave"`, not from the param name — the first
  cut filtered on `name.contains("shape")` and immediately failed on
  `env1_shape`, which is an enum too and renders as a rocker. Then asserts every
  variant of those four has a `WAVE_GLYPHS` entry.
- `editor_width_matches_the_css` parses `--editor-w` out of the stylesheet.
  `EDITOR_HEIGHT` has no single CSS constant to check against (it is the sum of
  the row variables plus two borders), and the doc now says so instead of
  asking to keep it in sync.

### Drift verified by hand

Every item, and specifically the two that fail silently today:

- **Vocabulary** — swapped `curve`/`scale` in the bridge's `MATRIX_FIELD`:
  `the bridge's matrix-field names and ordinals match the engine's` **fails**.
  Before this ticket the op would have landed, on the wrong field.
- **Wave glyphs** — renamed LFO `"Saw+"` → `"Ramp+"` in `params.rs`:
  `every_wave_label_has_a_glyph` **fails**. Before, the knob would have rendered
  with a blank icon and nothing else would have noticed.
- Telemetry (JS clamp ±2 → ±1: 2 node failures; Rust key `dynGr` → `dynGR`: the
  Rust test fails), split range (`SPLIT_MAX` 96 → 108 fails
  `split_range_matches_the_engine`), `Uncategorised` (-s- → -z- in
  `preset-browser.js` fails `the_uncategorised_label_is_the_shared_one`). All
  restored after.

### A real bug the work caught

Stamping the slider needed `slider.hasAttribute('value')`, not
`slider.value === ''`: a `<input type="range">` with no `value` attribute
reports its range **midpoint** as its value, so the property check never fired.
With the HTML literals removed, the shipped slider would have opened at 54
rather than C4. The vitest added for the stamp caught it before commit.

### No unenforced "must not disagree" comments left

Swept `must not disagree | must match | keep in sync | must agree | mirrored
from | hand-declared | declared mirror` across `vxn-1b/**`. What remains is
either now backed by a test (the event-codec mirror by `wasm-agreement`, the
`VE_*` tags and the vocabulary by `vocab-agreement`, `--editor-w` by
`editor_width_matches_the_css`) or is not a cross-file constant pair at all
(`coordinator.mjs`'s ring `capacity`, which is one value passed to both halves
through `processorOptions`, so it agrees by construction).

### Declined

Extracting a shared `model.js` to break the `dispatch.js` ↔ `panels/matrix.js`
ESM cycle. The Notes ask for it *if it is cheap*, and it is not:
`paramIdByNameAtLayer` reads `_paramIdByName`, an index `dispatch.js` builds in
`init()` and mutates on rebind, so moving the function moves shared mutable
state across the splice boundary whose ordering [[0310]] already flagged as
fragile. Worth its own ticket if the cycle ever costs something — today it is
invisible, because the splice loader flattens both files into one scope.

### Suites

`cargo test --workspace` 1374 pass / 0 fail (was 1365 — 9 new tests).
`node --test .../web/*.test.mjs` **158 pass, 0 skipped** (was 148 — two new
suites). `vitest run` 304 pass / 39 files.
