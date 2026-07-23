---
id: "0177"
product: vxn-2
title: Mod-matrix scale source — faceplate selector column
priority: medium
created: 2026-07-03
epic: E033
depends: ["0175"]
---

## Summary

Add a per-slot "Scale" source selector to the mod-matrix faceplate panel
(`mod-matrix.js`), reusing the existing source dropdown with a `—`/None default
so unscaled slots read as off at a glance. Selecting a source emits a scale-src
change event to the engine.

## Acceptance criteria

- [ ] Each matrix slot row shows a Scale selector listing the full source roster
      plus a `—` (None) default that is visually distinct from an active source.
- [ ] Changing the selector emits a scale-src change opcode/event; the view
      never reads the model (MVC discipline — the dumb dirty-bitset pump feeds
      state back).
- [ ] Round-trips with 0175's serde: loading a patch with a scale source shows
      it selected; saving preserves it.
- [ ] Contract/token tests added to the `vxn2-ui-web` suite (matching the
      existing mod-matrix panel coverage).

## Notes

Reuse the source-dropdown component and `SOURCE_LABELS`; the only new state is
one selector per slot. Keep the 16-slot layout legible — the None default must
not look like an active route. MVC parity per ADR 0003 / the existing
`mod-matrix.js` conventions. See
[E033](../../epics/open/E033-matrix-scale-source.md).

## Close-out (2026-07-23)

- Per-slot "Scale" selector added to
  [mod-matrix.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/mod-matrix.js)
  (reuses the source roster; `—`/None default), grid column in
  [style.css](../../vxn-2/crates/vxn2-ui-web/assets/style.css). Emits a
  topology change; view never reads the model (MVC).
- Full wire path: codec byte 12 ([codec.rs](../../vxn-2/crates/vxn2-wasm/src/codec.rs)
  + event-codec.mjs), ring `_push`/`pushMatrixRow`, coordinator/faceplate-bridge,
  FFI `vxnc_ui_set_matrix_row` (+scale arg), and both snapshot transports
  (binary VE_MATRIX_SNAPSHOT +u8, native JSON `scale`).
- Round-trips 0175 serde: loading a scale-source patch shows it selected; saving
  preserves it. Tests: `event-codec.test.mjs` (byte-12 contract),
  `faceplate-bridge.test.mjs`, `controller.test.mjs` snapshot decode, Rust codec
  apply/roundtrip. 72 JS tests green. Landed in `27d8823`.
