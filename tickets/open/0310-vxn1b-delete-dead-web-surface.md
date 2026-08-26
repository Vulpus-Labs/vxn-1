---
id: "0310"
product: vxn-1b
title: "Delete the dead web surface: keys.js ships unmounted, CPU meter burns render-thread time"
priority: high
created: 2026-08-26
epic: E047
depends: ["0307", "0309"]
---

## Summary

Five unreachable things in the shipped web bundle, one of which costs real time
on the audio render thread.

### 1. `panels/keys.js` — 249 lines, spliced into the page, entirely inert

[keys.js:34](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js#L34) opens
its IIFE with `document.querySelector('.panel[data-name="Keys"] .panel-body')`.
`faceplate.html` has no such panel (`grep -c 'data-name="Keys"'` → 0) and the
CSS was already removed (`grep -c 'keys-' faceplate.css` → 0). The guard returns
early, so **every export is a no-op stub**, and six call sites in `dispatch.js`
call into `function(){}`:
[:490 `wireLayerLevels`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L490),
[:965 `setLayer`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L965),
[:975](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L975) /
[:999 `setMode`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L999),
[:985](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L985) /
[:1003 `setSplit`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L1003).

The file is still `include_str!`'d as `PANEL_KEYS_JS` at
[vxn1b-ui-web/src/lib.rs:596](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L596)
and spliced into both the plugin's page and the web build's.

### 2. The CPU meter is dead end-to-end and still runs per quantum

[vxn1b-processor.js:38-45,100-125](../../vxn-1b/crates/vxn1b-wasm/web/vxn1b-processor.js#L38-L45)
accumulates `CPU_CLOCK` timings inside `process()`. The consumer chain is
severed at the top: `onCpu` defaults to a no-op
([coordinator.mjs:83](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L83)),
`boot()` never supplies one
([faceplate-bridge.mjs:626-636](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L626-L636)
passes only `onTrap`), and nothing in `vxn1b-ui-web/assets/` mentions cpu.

This is the one item in [[E047]] with a runtime cost rather than a hygiene cost:
work on the render thread for a number nobody reads. Safari has no render-thread
slack to spare ([[vxn1-web-safari-audioworklet]]).

Dies with it: `_accumCpu`, the `_cpu*` fields, the `cpuMeter` processorOption,
`isAppleWebKit()`, `_onCpu`, the `"cpu"` port case, and the `console.info` at
[coordinator.mjs:265](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L265).

### 3. `makeDropdown` is never instantiated

[discrete.js:179-203](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/discrete.js#L179-L203).
`faceplate.html` has no `data-control="dropdown"` — only `buttongroup`, `dial`,
`fader`, `header-switch`, `rocker`, `switch`, `wave` — and the only JS-created
control kind is `bipolar`
([matrix.js:204](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L204)).
The matrix's picker is `buildCombo`, deliberately not a `<select>`. So
[`case 'dropdown'`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L412)
and the `.ctl-dropdown` rules at
[faceplate.css:1281-1298](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L1281-L1298)
are unreachable too.

### 4. The `src-off` dim-rule kind has no markup

[dispatch.js:257-273](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L257-L273)
collects `data-dim-when-src-off`; nothing in `faceplate.html` carries it (the
only dim marker in the markup is `data-dim-unless-fm` on `cross_mod_amount`,
[faceplate.html:316](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L316)).
Half the 17-line doc block at
[:165-181](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L165-L181)
documents a kind that cannot fire.

### 5. Unreferenced exports and fields

Grep-verified as having no importer in production **or** tests:
`BUILTIN_DIM_SPECS` / `buildParamIndex` (dispatch.js), `STATUS_PILL_FLASH_MS`
(bridge.js), `buildParams` (fixtures/params.js). Test-only or unreferenced on
the wasm side: `peekSeq`, `pushParamNorm`, `pending`, `drainInto`
(event-ring.mjs); `LAYOUT`, `readAll` (param-store.mjs); `unpackMatrixAddr`,
`EV_SUSTAIN_RESERVED` (event-codec.mjs); `destroy`, `factoryLen`, `patchCount`
(controller.mjs); `paramAt`, `gestureBegin`, `gestureEnd`, `setParamsBulk`,
`suspend`, `resume`, `teardown`, `whenReady` (coordinator.mjs). Plus
`entry.layered` + `isLayeredEl`, written into every cell record and never read
(`rebindAllForLayer` rebinds unconditionally; `bindCell` uses `fixedLayer`).

Also dead and cheap: `this._frame`
([faceplate-bridge.mjs:290,566](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L290))
incremented every pump, never read; two CSS custom properties (`--dial`,
`--ctl-value-h`) declared and never `var()`'d; and the duplicated `.meter-mount`
block plus its overridden `.ctl-meter { justify-content: flex-end }` at
[faceplate.css:912-920](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L912-L920),
dead against the `flex-start` rule 500 lines later.

## Design

Straightforward deletion, but **sequence it**:

1. `keys.js` cannot go until [[0307]] decides the Reset button's fate. If 0307
   makes `reset_layer` live, the button and its handler need a new home before
   this ticket removes the file; if 0307 removes it, this ticket subsumes the
   deletion.
2. Remove the six no-op call sites and the `panels.js:49-53` barrel export
   *first*, then `PANEL_KEYS_JS` and the splice slot, then the file — in that
   order, in separate commits. The splice order is load-bearing for
   `css_covers_every_control_primitive` and the orchestration suite.
3. Items 2–5 are independent of each other and of `keys.js`.

For the test-only exports in item 5: where a test is the only caller, the
question the reviewer raised is worth answering rather than routing around —
does that test prove anything about shipped behaviour? Delete both, or keep both
and say why.

## Acceptance criteria

- [ ] `panels/keys.js`, its barrel export, `PANEL_KEYS_JS`, its splice slot and
      all six no-op call sites are gone; the spliced page still passes
      `css_covers_every_control_primitive` and the orchestration suite.
- [ ] No CPU timing work remains inside `process()`; the worklet's per-quantum
      path does no work for an unread value.
- [ ] `makeDropdown`, its `dispatch.js` case and its three CSS rules are gone.
- [ ] The `src-off` dim-rule collector and branch are gone and the doc block
      describes only the kind that exists.
- [ ] Every symbol listed in item 5 is either deleted or has a one-line reason
      recorded for keeping it.
- [ ] The shipped bundle is smaller by roughly the amount deleted — check the
      generated page byte size before and after, and record it in the close-out.
- [ ] Both node suites and the Vitest suite green under [[0309]]'s new CI.

## Notes

- The `keys.js` split-point constants (`SPLIT_MIN=12` / `MAX=96` / `DEFAULT=60`)
  are one of four transcriptions of that range — the others are `dispatch.js:722`,
  the literal `min`/`max`/`value` on
  [faceplate.html:438](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L438),
  and `DEFAULT_SPLIT_POINT: u8 = 60` in
  [engine.rs:69](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L69). Deleting
  keys.js removes one; [[0316]] settles the rest.
- Do not delete `util/drag.js`'s mount form on the same pass. Its comment
  self-declares it dead (*"No production caller takes the mount form… the option
  and its suite stay because the next composite will want it"*) — that is a
  deliberate keep, and re-litigating it belongs in [[0315]], not here.
