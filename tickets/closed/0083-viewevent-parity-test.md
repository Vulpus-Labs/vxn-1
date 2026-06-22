---
id: "0083"
product: vxn-1
title: web — golden-byte parity test for the packed ViewEvent protocol
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

The ViewEvent wire protocol exists in three hand-synchronised
encodings: the packed binary in `vxn-web-controller/src/lib
.rs:259-470` (`VE_*` consts + `drain_view_events`), the JS
decoder in `controller.mjs:33-334` (`_drainViewEvents`,
which re-declares `VE_*`/`PRESET_SRC_*`/`KEY_MODE_*` as JS
constants with a "MUST match lib.rs" comment), and a third
JSON encoding in `vxn-core-ui-web/src/lib.rs:607-656`
(`view_event_to_json`).

The byte offsets, little-endianness, and length-prefix order
are duplicated by hand on both sides of a manual `off += 4`
cursor walk. Unlike the *event codec* (`vxn-wasm/src/codec
.rs:367-639` + `event-codec.test.mjs`), the packed
ViewEvent format has **no golden-byte cross-language test** —
the only safety net is a runtime `throw new Error("unknown
ViewEvent tag")` (controller.mjs:330). A field added to a
record requires synchronised edits to the Rust packer, the
JS unpacker's offset math, and the doc comment; drift
produces silently misaligned reads, not a clean failure.

## Acceptance criteria

- [ ] A golden-byte parity test mirrors the codec's
      `golden()` table: Rust emits a known set of ViewEvent
      records (one per `VE_*` tag, including the multi-field
      `VE_PRESET_LOADED`) → the bytes are decoded by the JS
      unpacker → assert the decoded structs equal the
      originals. Layout drift fails in CI, not at runtime.
- [ ] The test exercises every `VE_*` variant currently
      packed by `drain_view_events`, plus an empty-batch and
      a multi-record batch.
- [ ] The `VE_*`/`PRESET_SRC_*`/`KEY_MODE_*` constants are
      either generated for JS from the Rust source, or the
      parity test asserts the JS constant values match the
      Rust ones (so a renumber is caught).
- [ ] Test runs under the existing gated JS suite
      (`VXN_JS_TESTS=1`) alongside `event-codec.test.mjs`.
- [ ] `cargo test --workspace` green; gated vitest suite
      green.

## Notes

This is the one place a hand-walked binary protocol crosses
two languages without the codec's golden treatment — highest
drift risk in the web layer. Pattern to copy:
`vxn-wasm/src/codec.rs` `golden()` + `assets/__tests__/
event-codec.test.mjs`.

## Close-out (2026-06-22)

- Extracted the per-record packer `pack_view_event(buf, &ev) -> bool`
  out of `drain_view_events` so the golden test exercises the REAL
  packer; `drain` now loops over it
  ([lib.rs:326](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L326),
  [lib.rs:520](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L520)).
- Rust golden-byte table `view_golden()` — one row per `VE_*` tag,
  incl. all three `VE_PRESET_LOADED` source kinds (none / factory /
  user-with-warnings) and `VE_PRESET_CORPUS_CHANGED` with/without a
  follow path. Asserted by `vxn_web_controller::tests::pack_view_event_matches_golden`;
  empty + multi-record batch layout by
  `tests::drain_layout_empty_and_multi_batch`; skip semantics by
  `tests::pack_view_event_skips_other_channel_variants`.
- Extracted a standalone `decodeViewEvents(buffer, ptr, len)` export
  (clean u32/f32/str cursor helpers); `_drainViewEvents` now delegates,
  so the test hits production decode code
  ([controller.mjs:64](../../vxn-1/crates/vxn-wasm/web/controller.mjs#L64)).
  Exported `PRESET_SRC_*` for the constant cross-check.
- JS parity test mirrors the Rust golden table byte-for-byte, decodes
  each record and asserts the structs, plus empty-batch, multi-record
  batch, an unknown-tag `throw`, and a constants-match assertion
  (`VE_*` / `PRESET_SRC_*` / `KEY_MODE_*` / `LAYER_*` == Rust values) —
  `assets/__tests__/viewevent-parity.test.js` (13 cases).
- Placed the JS test in the Vitest suite rather than next to
  `vxn-wasm/web/event-codec.test.mjs`: the `node:test` files in
  `vxn-wasm/web/` are NOT run by CI; the only JS suite CI executes under
  `VXN_JS_TESTS=1` is Vitest (`vxn-ui-web` `js_suite_passes` → `npm test`).
  So drift now genuinely fails in CI.
- Verified: `cargo test --workspace` green (0 failures);
  `npx vitest run` green (24 files, 169 tests); `node --test
  controller.test.mjs` green (refactored decoder, real wasm).
