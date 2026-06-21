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
