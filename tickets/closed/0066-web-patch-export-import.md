---
id: "0066"
product: vxn-2
title: "Patch export/import — download/upload file + URL share link"
priority: medium
created: 2026-06-15
epic: E019
depends: ["0065"]
---

## Summary

Fifth and final ticket of
[E019](../../epics/open/E019-web-persistence-presets-state.md). Let a user share
a patch off-device: export the current patch to a downloadable file and/or an
encode-in-URL share link, and import it back. Builds on the snapshot byte
channel
([`snapshot_bytes` / `restore_from_bytes`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L118))
and the corpus storage from 0063/0064.

## Design

- **File export/import.** Download the current patch as a `.toml` (the existing
  name-keyed format, [[vxn1-preset-system]]) via a Blob/anchor; import via a
  file picker that parses it through the same `user_load` path and applies it (or
  offers to save it into the corpus).
- **URL share link.** Encode the snapshot compactly (base64url of the blob, or
  the TOML gzipped) into a URL fragment (`#patch=…`, kept out of the query so it
  isn't sent to any server). On load, if a `#patch=` fragment is present, decode
  and apply it before `EditorReady`. Cap the size; if a patch is too big for a
  practical URL, fall back to file-only and surface that.
- Reuse desktop format so an exported web patch imports on desktop and vice
  versa (no format divergence — epic acceptance).

## Acceptance criteria

- [ ] A patch can be exported to a file and re-imported, reproducing the params.
- [ ] A share-link URL round-trips: open it in a fresh tab and the patch
      applies.
- [ ] An exported file imports on the desktop build (format parity).
- [ ] Malformed file/URL input is rejected gracefully (no crash, user-visible
      message).

## Notes

- Share-link decode runs before `EditorReady` so the seed broadcast carries the
  imported values, same ordering as 0065's restore.
- Depends on 0065 (the full-state codec) and 0063/0064 (corpus + storage).
- Closing this ticket closes E019 — verify the epic's acceptance list end to end.

## Close-out (2026-06-22)

- **TOML codec (format parity, AC3).** New wasm-clean, name-keyed TOML codec
  keyed by `ParamDesc::name` over the `ParamModel`/`Vxn1Params` trait surface:
  [preset_toml.rs](../../vxn-1/crates/vxn-app/src/preset_toml.rs) (`write_toml` /
  `read_toml_into`, `+toml`/`serde` deps). Mirrors `state.rs`'s parallel-impl +
  drift-guard pattern. Byte-for-byte identical to the desktop writer, proved by
  `vxn_engine::preset::tests::app_writer_matches_engine_byte_for_byte`, with both
  cross-direction round-trips (`app_write_parses_on_engine`,
  `engine_write_applies_through_app_reader`) and a sparse-reset test.
- **Controller C-ABI.** `vxnc_export_toml` / `vxnc_import_toml` (+ reused
  `toml_buf`) in
  [vxn-web-controller/lib.rs](../../vxn-1/crates/vxn-web-controller/src/lib.rs);
  `tests::export_import_round_trips_through_toml`,
  `tests::import_rejects_garbage_without_mutating`.
- **File export/import + URL share-link (AC1/AC2).**
  [patch-io.mjs](../../vxn-1/crates/vxn-wasm/web/patch-io.mjs): pure base64url
  codec + `#patch=` fragment parse/build/cap; `exportPatchFile` (.toml download),
  `importPatchFile` (picker → `importToml` → `editorReady`), `shareLinkFor`,
  `applyShareLinkOnBoot` (decodes the compact binary blob, strips the fragment
  after). `exportToml` / `importToml` bindings in
  [controller.mjs](../../vxn-1/crates/vxn-wasm/web/controller.mjs).
- **Boot + UI wiring.**
  [faceplate-bridge.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs):
  share-link apply before the `EditorReady` re-broadcast (precedence over autosave
  restore, same ordering as 0065 — AC2); web-only Export/Import/Share preset-bar
  controls injected like the CPU meter (native plugin + shared faceplate markup
  untouched, since the host owns state on native).
- **Headless proof (AC1/AC4).**
  [patch-io.test.mjs](../../vxn-1/crates/vxn-wasm/web/patch-io.test.mjs) — pure
  codec, fake-controller share glue, and a REAL-wasm export→import round-trip
  (byte-identical re-export, SAB seeding) + malformed file/URL rejection.
  `node web/patch-io.test.mjs` all-pass.
- **Bundling.** `patch-io.mjs` added to the xtask web bundle
  ([main.rs](../../vxn-1/xtask/src/main.rs)); `cargo run -p vxn1-xtask -- web`
  ships it + the rebuilt controller wasm.
- Cargo: vxn-app 18, `vxn_engine::preset` 16, vxn-web-controller 17 pass; Node:
  patch-io / faceplate-bridge / controller suites pass; dependents + xtask
  compile; no new clippy warnings.
- Share-link uses the compact binary state blob (0065) base64url'd; file export
  uses TOML for desktop parity — the deliberate two-channel split from the Design.
