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

## Close-out (2026-08-28)

Pure extraction, one function per commit, no behaviour change. Every function
in the table, plus the three the Notes named:

| function | was | now |
|---|---|---|
| [dispatch.js `init`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js) | 237 | 24 |
| [`makeWave`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js) | 151 | 72 |
| [`Engine::render_control_block`](../../vxn-1b/crates/vxn1b-engine/src/engine.rs) | 148 | 33 |
| [`routeOpcode`](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs) | 116 | 6 |
| [`bundle_vst3`](../../crates/vxn-xtask-common/src/lib.rs) | 111 | 33 |
| [CLAP `process`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs) | 101 | 56 |
| [`ControllerState::tick`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs) | 98 | 41 |
| [`buildCombo`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js) | 93 | 3 (class) |
| [`decodeViewEvents`](../../vxn-1b/crates/vxn1b-wasm/web/controller.mjs) | 85 | 65 |
| [`on_timer`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs) | 77 | 48 |

- **`dispatch.js`** — `collectCells()` + `wirePanels()` take the sweep and the
  wire block; `VIEW_HANDLERS` is one entry per `ev.kind` (9 entries, zero
  `ev.kind ===` arms left); `installDispatcher()` publishes the two Rust entry
  points. `param_changed`'s two identical partner-refresh blocks collapse into
  `refreshPartner(map, id)`.
- **`routeOpcode`** — `OPCODE_HANDLERS` keyed by op name, each handler taking
  the routing context as one record.
- **`render_control_block`** — the copy-pasted layer-1/layer-2 halves are one
  `render_layer()` over a `LayerParts` record (disjoint `self` field borrows, so
  the caller keeps the bus slices) and a `LayerSpec`. `render_layers_into_bus()`
  holds both calls and the sum; `apply_master()` holds master volume, the finite
  guard, the limiter and the master tap.
- **`process`** — `sync_from_store()` (transport, reload, key, param fold) and
  `write_out()`. The batch loop untouched, as the ticket required.
- **`on_timer`** — the four-way downcast chain is a free `apply_custom_op()`.
- **`bundle_vst3`** — `vst3_preflight()`, `Product::build_vst3_archive()`, and a
  `WrapperBuild` record carrying `configure()` + `build()`.
  `Profile::cmake_config()` replaces two identical Release/Debug matches.
- **`tick()`** — split along its own `(1)`…`(4)` numbering into
  `drain_view_channel` / `pack_pending` / `refresh_rate_partners`, each
  returning its record count instead of mutating a shared `count`. Taken rather
  than left to [[0303]]: 0303 is blocked behind [[0301]], which is unstarted.
- **`decodeViewEvents` / `decodeJournal`** — one `Cursor` class replaces the
  eight reader closures the two re-created per call.
- **`buildCombo`** — a `Combo` class; `buildCombo` stays as the one-line factory
  its four callers already use, and `.value` stays installed on the element.

### Verification

- **Arity.** Widest new signature is 4 positional (`render_layer`,
  `LayerSpec::for_layer`, `apply_master`, `buildWaveGlyphs`). No tuples; the
  only same-typed pairs are the `l`/`r` slice idiom the file already used.
  `write_out` lost its `nch` argument mid-review — two adjacent `usize`s is a
  transposition waiting to happen, and `OutputChannels` already knows.
- **`render_control_block` is bit-identical.** FNV-1a over every rendered sample
  of 200 blocks × 4 configurations (OS off / 4×, limiter off / on, single / dual
  / split, both layers panned and cross-modded, a note released mid-run) matches
  the pre-split engine exactly.
- **Profiles unchanged.** Alternating old/new pairs, to cancel the thermal drift
  a naive before/after shows: `busy_profile` old 6.3/6.4/6.3/6.3 vs new
  6.4/6.3/6.3/6.3 (% of one core, 15.6–16.1x realtime either way).
  `route_profile` 49.4x/2.02% → 49.3–49.5x/2.02–2.03%.
- **`bundle_vst3`** — `target/vxn1b-wrapper-release` deleted and rebuilt clean:
  links, and `VXN1b.vst3` still embeds `labs.vulpus.vxn1b`.
- **`process`** — clap-validator on the bundled `vxn1b.clap`: 20 tests, 17
  passed, 0 failed, 3 skipped. The one warning is the pre-existing 248 ms scan
  time.
- **Suites** ([[0321]]'s four CI commands): `cargo test --workspace` **1389
  pass, 0 fail** (same count as before the work), vitest **318 pass**,
  `xtask web` clean, `node --test` **158 pass**.
- **Manual DAW pass** on the `process` / `render_control_block` changes: done.

### Two things worth carrying forward

- **`makeWave` and the matrix combo popup had no test** (the [[0320]] gap), so
  they got one *before* being touched:
  [`wave.test.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/wave.test.js)
  (6) and
  [`matrix-combo.test.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/matrix-combo.test.js)
  (8). Both pass **unchanged against the pre-refactor files**, which is what
  makes them evidence rather than decoration. `matrix-overlay.test.js` drives
  combos by assigning `.value`, so it skipped every line of the dropdown.
- **`Object.hasOwn` is ES2022 and this bundle declares a macOS 11 minimum**,
  whose WKWebView is Safari 14. Both handler tables were guarded with it at
  first; it would have thrown at first dispatch on the oldest supported host and
  passed every test here. They are `__proto__: null` now, which needs no guard.
  No other shipped faceplate JS reaches past ES2020.
