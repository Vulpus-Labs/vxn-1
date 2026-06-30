---
id: "0140"
product: monorepo
title: Shared web widgets — value-pop, cutoff-tuned math, wireDrag
priority: medium
created: 2026-06-23
epic: E027
---

## Summary

The two synths' web faceplates were built separately and now
re-implement three widget primitives, each with observable
divergence. Lift each into one shared JS asset (served from
`vxn-core-ui-web`) that both consume.

1. **Floating value-pop singleton** — copy-pasted verbatim:
   vxn-1 `bridge.js:120-134` (`valuePop.show/update/hide`) vs
   vxn-2 `panels/fader.js:86-103`
   (`ensurePop/showPop/updatePop/hidePop`). Same class name
   `value-pop`, same `x+12, y-8` offsets, same display
   toggling.
2. **Cutoff-tuned math** — three-way drift with confirmed
   provenance: `midiToHz` / `hzToMidi` /
   `cutoffTunedNormToHz` / `cutoffTunedHzToNorm` /
   `noteName` / `CUTOFF_TUNED_MIDI_MIN/MAX` exist in vxn-1
   `panels.js:163-187`, vxn-2 `main.js:183-207` (comment:
   "Mirrors VXN-1's panels.js"), and again in vxn-2
   `bootstrap.js:42-46`.
3. **Pointer-drag lifecycle** — re-implemented ≥4×:
   vxn-1 `wireDrag` (`panels.js:472`), vxn-2 fader `create`
   (`fader.js:194`), fader `createBipolar` (`fader.js:338`),
   knob `bindDrag` (`knob.js:140`). Divergences: vxn-2 fader
   has a RAF-throttle, knob omits it; the
   `shift = 0.1× sensitivity` convention is in
   `fader.js:219` / `knob.js:161` / `op-row.js:622` but
   missing from vxn-1.

## Acceptance criteria

- [ ] One `ValuePop` (class or factory) in a shared asset;
      both synths import it; the per-synth copies are deleted;
      one CSS `value-pop` ruleset.
- [ ] One module owns the cutoff-tuned math + `noteName`;
      vxn-1 and vxn-2 import it; the `bootstrap.js` and
      `main.js` duplicates are removed.
- [ ] One `wireDrag(el, opts)` with an optional RAF-throttle
      flag and the `shift = 0.1×` convention baked in; the
      vxn-1 monolith and all vxn-2 panels consume it; no
      panel re-implements pointerdown→capture→move→up.
- [ ] vitest green; add direct tests for `cutoffTunedNormToHz`
      / `hzToMidi` round-trip and `wireDrag` taper math (the
      Rust sync tests exist; the JS side currently does not
      cover these).

## Notes

Land before `0141` (the god-file splits consume these shared
widgets). Confirm the shared-asset serving path —
`vxn-core-ui-web` is a Rust crate that bundles assets; the
splice/concat build step (vxn-1) and the ES-module loader
(vxn-2) differ, so the shared file must work under both
(plain ES module + named exports is the safe shape). This is
the JS half of epic E008's "reusable primitives" intent;
cross-link if E008 is still open. Coordinate with the
concurrent E026 faceplate work (ticket 0128 adds an EG-curve
selector to the same op-row faceplate) to avoid editing the
same DOM region simultaneously.

## Close-out (2026-06-30)

- **ValuePop** lifted to shared singleton
  [value-pop.js](../../crates/vxn-core-ui-web/assets/value-pop.js) with one CSS
  ruleset [value-pop.css](../../crates/vxn-core-ui-web/assets/value-pop.css).
  vxn-1 re-exports from [panels.js:25](../../vxn-1/crates/vxn-ui-web/assets/panels.js#L25);
  old `bridge.js` copy removed. vxn-2 consumes via
  [panels/dial.js:16](../../vxn-2/crates/vxn2-ui-web/assets/panels/dial.js#L16);
  old `fader.js`/`style.css` copies removed.
- **Cutoff-tuned math + noteName** in shared
  [cutoff-tuned.js](../../crates/vxn-core-ui-web/assets/cutoff-tuned.js) — all six
  symbols (`midiToHz`/`hzToMidi`/`cutoffTunedNormToHz`/`cutoffTunedHzToNorm`/
  `noteName`/`CUTOFF_TUNED_MIDI_MIN/MAX`). vxn-1 re-exports
  [panels.js:27](../../vxn-1/crates/vxn-ui-web/assets/panels.js#L27); vxn-2
  consumes via [bootstrap.js:39](../../vxn-2/crates/vxn2-ui-web/assets/bootstrap.js#L39).
  `main.js`/`bootstrap.js` local dupes removed (grep: 0 occurrences).
- **wireDrag** in shared
  [wire-drag.js](../../crates/vxn-core-ui-web/assets/wire-drag.js) with `raf` flag +
  `shift = 0.1` default. vxn-1 wraps as `wireFaderDrag`
  [util/drag.js:30](../../vxn-1/crates/vxn-ui-web/assets/util/drag.js#L30); vxn-2
  fader [fader.js:169](../../vxn-2/crates/vxn2-ui-web/assets/panels/fader.js#L169)
  and knob [knob.js:149](../../vxn-2/crates/vxn2-ui-web/assets/panels/knob.js#L149)
  (`shift: 0.25` override) consume it. No panel re-implements the pointer lifecycle
  (grep: 0 local defs across 15 vxn-2 panels).
- Build splices shared assets via `vxn_core_ui_web::shared_widgets_js()` /
  `VALUE_POP_CSS` in both [vxn-1 lib.rs:1752](../../vxn-1/crates/vxn-ui-web/src/lib.rs#L1752)
  and [vxn-2 lib.rs:111](../../vxn-2/crates/vxn2-ui-web/src/lib.rs#L111).
- **Tests** green both sides: vxn-1 `cutoff-tuned.test.js` (round-trip + clamp),
  `wire-drag.test.js` (relative-delta + 0.1× shift + rAF coalesce) — 188 passed;
  vxn-2 same coverage incl. knob 0.25 quantiser — 35 passed.
