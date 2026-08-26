---
id: "0318"
product: vxn-1b
title: "Extract the five long functions whose seams are already written in as comments"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0321"]
---

## Summary

After [[0313]] takes `RenderBank::render` (452 lines), five long functions
remain. The striking thing about all of them is that **the extraction plan is
already in the file as banner comments** — somebody worked out the seams and
stopped there.

| function | lines | seams already marked |
|---|---|---|
| [`dispatch.js init`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L869) | 237 | cell sweep / wire block / nested `dispatch` (159, flat if-chain over 13 `ev.kind`) |
| [`makeWave`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js#L154) | 151 | geometry / glyphs / knob face / indicator / drag |
| [`Engine::render_control_block`](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L627) | 148 | seven comment-blocked phases |
| [`routeOpcode`](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L146) | 116 | preset/folder cases are all one-line forwards |
| [`bundle_vst3`](../../vxn-1b/xtask/src/main.rs#L427) | 111 | preflight / build / configure / cmake / discover / copy / install |
| [`ControllerState::tick`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L534) | 98 | self-numbered `(1)`…`(4)` |
| [CLAP `process`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L586) | 101 | transport / reload / key / params / ports / batch / copy-out |

### Notes per function

**`dispatch.js init`** is the worst of them. The nested `dispatch` is a flat
if-chain over 13 `ev.kind` strings where each arm is already independent and
side-effect-local — a `VIEW_HANDLERS` table keyed by kind. The `param_changed`
arm alone ([:927-958](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L927-L958))
is five concerns (cache, fan-out, sync-partner refresh, cutoff-partner refresh,
dim rules). Extracting `collectCells()`, `wirePanels()` and the handler table
gets `init` to about 30 lines.

**`Engine::render_control_block`** runs at ≤32 frames per call — well outside
the poly hot path — so extraction costs nothing measurable. Its layer-1 and
layer-2 halves are additionally copy-pasted
([:669-679](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L669-L679) vs
[:699-705](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L699-L705)): identical
gain/OS loops, matching `set_target` pairs, matching meter+scope publishes.
A `render_layer(...)` extraction removes the length and the duplication together.

**`CLAP process` is the one to be careful with** — it is on the audio thread.
Extract `sync_from_store(&mut self)` (transport + reload + key + param fold) and
`write_out(&mut out, frames, nch)`; leave the batch loop alone.

**`tick()`** numbers its own seams `(1)` drain, `(2)` pack, `(3)` rate-partner
refresh, `(4)` non-param echoes. Note it may shrink or vanish under [[E046]]'s
dirty-bitset pump ([[0303]]) — check that ticket's state before doing surgery
here, and prefer to let 0303 have it if 0303 is imminent.

**`on_timer`** ([:458](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L458), 77 lines)
is borderline but has an obvious lift: ~35 lines of it are a nested
`on_custom_ui` closure doing a four-way manual downcast chain. Make it a free
`fn apply_custom_op(sink, scope, payload)`.

**`decodeViewEvents`** ([controller.mjs:66-150](../../vxn-1b/crates/vxn1b-wasm/web/controller.mjs#L66-L150),
85 lines) re-creates five reader closures per call, and `decodeJournal` re-declares
four of the same. One small `Cursor` class serves both.

**`buildCombo`** ([matrix.js:33](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L33),
93 lines) holds six nested closures plus an `Object.defineProperty` shim — a
small class or a `popup.js` module.

## Design

Pure extraction, no behaviour change, one function per commit. Two constraints:

- **Nothing here may change audio-thread codegen.** `render_control_block` and
  `process` are the two that touch it; both are per-block, not per-sample, so
  the risk is low — but confirm rather than assume.
- **Extract to named records, not tuples.** The lesson from [[0313]] applies:
  these functions are long partly because they thread many same-typed scalars,
  and an extraction that turns 11 arguments into two 6-tuples has moved the
  hazard rather than removed it.

## Acceptance criteria

- [ ] Every function in the table is under ~80 lines.
- [ ] `dispatch`'s if-chain is a handler table; adding a `ev.kind` means adding
      one entry.
- [ ] No new function takes more than 5 positional parameters or a bare tuple of
      same-typed scalars.
- [ ] `busy_profile` / `route_profile` unchanged within noise — record the
      numbers in the close-out.
- [ ] Full VXN1b suite green (Rust + both JS suites) under [[0321]].
- [ ] One manual DAW pass for the `process` / `render_control_block` changes —
      [[verify-audio-in-reaper]].

## Notes

- Check [[0303]] before touching `tick()`; if the dirty-bitset rewire is close,
  that ticket will restructure it anyway and this one should skip it.
- `makeWave`, `buildCombo` and `decodeViewEvents` have no test of their own
  today (see [[0320]]) — extracting them is easier *after* they have one, not
  before, so sequence those two together if 0320 is being worked.
