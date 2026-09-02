---
id: "0345"
product: monorepo
title: "Both browser builds dropped the scale VCA's curve: the JS half of the wire never learned 0340/0341's new topology"
priority: high
created: 2026-09-02
epic: null
depends: ["0341"]
---

## Summary

[0341](0341-scale-vca-polarity-axis.md) gave the scale VCA its own polarity axis
and [0340](../open/0340-matrix-curve-glyph-picker.md) put both of its axes behind the glyph
picker. Every Rust half of that landed: the engines carry `scale_polarity`, the
vocabularies name it, the faceplates send it. **Neither browser build's JS glue
was updated**, so on the web the scale-curve picker moves and the sound does not.

Both failures are silent, and they are silent in the same way — a topology field
has no CLAP id, so there is no param echo to disagree with, and the picker
repaints from its own optimistic local state rather than from the engine.

**vxn-1b** — `vxn1b_engine::vocab::MATRIX_FIELD_NAMES` gained `"scale-polarity"`
at ordinal 7 and `vxn1b_wasm::codec::unpack_matrix_addr` decodes it, but
`event-codec.mjs` stopped at `MATRIX_FIELD_ENABLED = 6` and rejected field 7 as
out of range, and `faceplate-bridge.mjs`'s `MATRIX_FIELD` table had no key for
the name — so `vocabLookup` returned `undefined` and `routeOpcode` dropped the
op. The snapshot leg was missing too: `pack_matrix` wrote seven bytes per slot,
so the field never came back either.

`vocab-agreement.test.mjs` caught exactly this and **the Test workflow was red on
`main` from `d74a5ba`** — three failures, including
`matrixField name "scale-polarity" appears in no page table`.

**vxn-2** — worse, because nothing caught it. The wire had already been widened:
`codec.rs` encodes `scale_curve` into slot byte 13 and decodes it back, and
`vxnc_ui_set_matrix_row` takes it as an eighth argument. But `controller.mjs`
called that export with **seven** arguments — and a wasm export called short
pads the missing `i32` with 0 rather than throwing, so every row arrived with
`scale_curve: 0` and looked entirely normal. `event-ring.mjs`'s `_push` never
wrote byte 13, `faceplate-bridge.mjs` never read `row.scale_curve` off the
message, and the snapshot omitted the field in both the Rust packer and the JS
decoder. vxn-2's `web/*.test.mjs` suites are **not run by CI**, which is why this
one shipped no signal at all.

## Design

Fix the JS half to match the Rust in both ports, and widen the two snapshot legs
that were still a field short. No wire ordinal moves: vxn-1b's `scale-polarity`
stays appended at 7 (past `enabled`) rather than being tidied in beside
`scale-shape`, because renumbering re-aims every in-flight matrix address.

Sites, as fixed:

- vxn-1b send — [event-codec.mjs:117](../../vxn-1b/crates/vxn1b-wasm/web/event-codec.mjs#L117)
  (`MATRIX_FIELD_SCALE_POLARITY = 7`, and the `unpackMatrixAddr` bound follows it),
  [faceplate-bridge.mjs:68](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L68)
  (`MATRIX_FIELD` + `RESEND_FIELDS`).
- vxn-1b snapshot — [vxn1b-web-controller/src/lib.rs:540](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L540)
  packs the byte, [controller.mjs:164](../../vxn-1b/crates/vxn1b-wasm/web/controller.mjs#L164)
  decodes it. Eight bytes per slot now, in `slots_json`'s order.
- vxn-2 send — [event-ring.mjs:125](../../vxn-2/crates/vxn2-wasm/web/event-ring.mjs#L125)
  writes byte 13, [faceplate-bridge.mjs:145](../../vxn-2/crates/vxn2-wasm/web/faceplate-bridge.mjs#L145)
  reads `row.scale_curve`, and `controller.mjs`'s `setMatrixRow` passes the
  eighth argument to both the wasm export and the ring.
- vxn-2 snapshot — [vxn2-web-controller/src/lib.rs:168](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L168)
  packs it, [controller.mjs:95](../../vxn-2/crates/vxn2-wasm/web/controller.mjs#L95)
  decodes it as `scale_curve` (snake_case, matching the native JSON wire the
  same panel reads).

## Acceptance criteria

- [x] `node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs` is green, including
      `vocab-agreement.test.mjs` pinning **eight** matrix fields with
      `scale-polarity` at 7.
- [x] `node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs` is green, including a
      new bridge case asserting `set_matrix_row` carries `scale_curve` through
      to `setMatrixRow`, and a ring case asserting byte 13 survives a
      push/drain/decode round trip.
- [x] `VXN_JS_TESTS=1 cargo test --workspace` passes; the Test workflow is green
      on `main`.
- [ ] In both web builds, picking a scale curve changes the sound and survives a
      preset load (the snapshot repaints the picker rather than resetting it).
      **User-verified by hand** — [[verify-audio-in-reaper]] has no browser
      analogue.

## Notes

- The two synths diverge by design and the fix respects that: vxn-1b addresses
  each matrix field individually by ordinal, vxn-2 sends whole rows with the
  curve as one flat `(polarity, shape)` code. Neither shape is wrong; only the
  JS halves were behind.
- **The real lesson is the missing CI leg.** vxn-1b's web suite is in
  `test.yml`; vxn-2's identically-named suites are not, and that is the whole
  difference between "red on main, found in an afternoon" and "shipped". Adding
  vxn-2's `node --test` to the workflow is out of scope here but is the obvious
  follow-up — it needs `cargo xtask web` to build vxn-2's two wasm artifacts
  first, the same ordering constraint 0321 solved for vxn-1b.
- A wasm export called with too few arguments zero-fills rather than throwing.
  That is the mechanism that made vxn-2's version invisible, and it will do it
  again the next time an export grows a parameter.

## Close-out (2026-09-02)

- **vxn-1b**: `MATRIX_FIELD_SCALE_POLARITY = 7` lands in `event-codec.mjs`
  (and `unpackMatrixAddr`'s bound moves off `ENABLED` onto it), the name joins
  `MATRIX_FIELD` and `RESEND_FIELDS` in `faceplate-bridge.mjs`, and the snapshot
  is eight bytes per slot end to end — `pack_matrix` pushes `scale_polarity`
  between `scale_src` and `scale_shape` (matching `slots_json`'s order) and
  `controller.mjs` decodes it there.
- **vxn-2**: `setMatrixRow` now passes the eighth argument the wasm export has
  been taking all along, `_push`/`pushMatrixRow` write slot byte 13, the bridge
  reads `row.scale_curve` off the message, and the snapshot carries the byte in
  both the Rust packer and the JS decoder. `encodeInto`/`decode` in
  `event-codec.mjs` round-trip it too, so the ring's two encoders agree.
- **Tests**: `VXN_JS_TESTS=1 cargo test --workspace` exit 0 (91 suites ok).
  `node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs` 161/161 —
  `vocab-agreement.test.mjs` now pins eight fields and was the failing gate.
  `node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs` 92/92, up one: a new
  bridge case for `scale_curve`, and the ring's round-trip case now asserts
  bytes 12 and 13 rather than only the packed header.
- Widening the snapshot moved three Rust test decoders that had the old stride
  written as a literal: `vxn1b-web-controller`'s `[u8; 7]` record and vxn-2's
  four `rows * 9` skips. All were consume-the-whole-buffer assertions, which is
  why they failed loudly rather than silently mis-parsing — the property the
  original authors were after.
- **Not verified here**: the browser hand-check (pick a scale curve, hear it
  change, load a preset and see the picker repaint). Needs a served,
  cross-origin-isolated build; user-verified by hand.
- Shipped in **0.3.0**.
