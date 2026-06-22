---
id: "0084"
product: vxn-1
title: vxn-ui-web — robust HTML assembly via explicit placeholders
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

The faceplate page is assembled by `str::replace` on
placeholder tokens (`__CSS__`, `__BRIDGE_JS__`,
`__PARAMS_JSON__`, …) in `build_faceplate_html`
(`vxn-ui-web/src/lib.rs:280-300`), which is fine — but
`build_web_faceplate_html` (`:372-399`) then does raw
`native.find("<script>\n")` / `find("</script>")` +
`split_at` byte-surgery to inject the web boot head,
guarded by `.expect("faceplate template must contain an
inlined <script>")` panics. Adding a second `<script>` to
`faceplate.html`, or reordering, breaks the page in ways
only the substring tests catch.

Separately, `strip_esm_exports` (`vxn-core-ui-web/src/lib
.rs:59-78`) is a line-prefix hack that turns `export const
X` into `const X` and deletes `import` lines; it
mis-handles any `export`/`import` not at line start,
multi-line imports, or `export { … }` re-export blocks.

The inlining itself is forced by wry's `with_html` having no
custom-protocol asset resolution (documented at
`:273-279`), so this ticket does not remove inlining — it
removes the gratuitous fragility.

## Acceptance criteria

- [ ] `faceplate.html` carries explicit placeholder tokens
      for the web boot head and loader (e.g. `__WEB_BOOT_
      HEAD__` / `__WEB_BOOT_LOADER__`), same as every other
      splice point; `build_web_faceplate_html` does
      `str::replace` on them instead of `find("<script>")` +
      `split_at`. The two `.expect` panics on script
      markers are gone.
- [ ] `strip_esm_exports` either handles (or explicitly
      rejects with a clear error) `export {`/`import`
      blocks not at line start and multi-line imports; its
      contract is documented and covered by a unit test for
      each form it claims to handle.
- [ ] The substring tests that pin assembly structure still
      pass (or are updated to the new placeholders);
      faceplate opens and renders unchanged in both the
      native host and the web build (manual).
- [ ] `cargo test -p vxn-ui-web` green with `VXN_JS_TESTS=1`;
      `cargo test --workspace` green.

## Notes

Sequence with 0077: if the splice/strip logic migrates into
`vxn-core-ui-web` during the editor adoption, do this
placeholder rework there; otherwise it lands after 0077 on
the vxn-1 side. Longer term (out of scope here): evaluate
wry's custom protocol handler so modules load as real ESM
and the strip/concat scheme disappears entirely.

## Close-out (2026-06-22)

- `faceplate.html` now carries `__WEB_BOOT_HEAD__` (before the inlined
  `<script>`) and `__WEB_BOOT_LOADER__` (after `</script>`) placeholder
  tokens, same as every other splice point. [build_web_faceplate_html](../../vxn-1/crates/vxn-ui-web/src/lib.rs#L190)
  is now a single `assemble_faceplate(WEB_BOOT_HEAD, WEB_BOOT_LOADER)`
  call doing `str::replace`; the `.find("<script>\n")` / `find("</script>")`
  + `split_at` byte-surgery and both `.expect` panics on script markers
  are gone. The boot head is spliced before the `__*_JSON__` pass so its
  `__PARAMS_JSON__`/`__SUBDIVISIONS_JSON__`/`__PATCH_COUNT__` tokens pick up
  the same descriptor data as the body (verified byte-identical by the
  existing `web_page_params_are_byte_identical_to_native` test).
- [strip_esm_exports](../../crates/vxn-core-ui-web/src/lib.rs#L59) rewritten:
  trim-aware (handles `export`/`import` not at line start), drops multi-line
  imports, `export { … }` export-lists, and `export … from`/`export *`
  re-exports whole (swallows continuation lines to the terminating `;`).
  Contract documented in the doc-comment; one unit test per claimed form in
  the shared crate: `strip_export_decls_and_default_at_line_start`,
  `strip_preserves_trailing_newline_shape`, `strip_indented_export_keeps_indentation`,
  `strip_single_line_import_drops_to_blank`, `strip_multi_line_import_drops_whole_statement`,
  `strip_export_list_dropped_whole`, `strip_reexport_forms_dropped_whole`,
  `strip_multi_line_export_list_dropped_whole`.
- Structural substring tests still pass unchanged (`web_page_splices_clean_and_wires_boot`,
  `faceplate_esm_exports_stripped`, the 0040-series faceplate checks).
  `gen-web-page` output inspected: no leaked placeholders, boot-head →
  faceplate `<script>` → module-loader order correct, params JSON in the
  boot head.
- `cargo test -p vxn-ui-web` green (56) and with `VXN_JS_TESTS=1` the
  Vitest suite passes (156 in 24 files); `cargo test -p vxn-core-ui-web`
  green (10); `cargo test --workspace` green. Native + web visual render
  is the user's manual DAW/browser check.
