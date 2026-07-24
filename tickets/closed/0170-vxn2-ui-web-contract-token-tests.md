---
id: "0170"
product: vxn-2
title: Review pre-E031 vxn2-ui-web contract-token tests for cogency
priority: low
created: 2026-07-02
---

## Summary

Follow-up surfaced by the E031 re-review (ticket 0169). Ticket 0163
collapsed the `faceplate_*_wired` substring change-detector tests, but a
second cluster of `str::contains`-style tests in `vxn2-ui-web/src/lib.rs`
predates E031 (added under E008, ~commit a80740a) and was out of 0163's
scope. Unlike the deleted change-detectors, these assert *named API
contracts* other components depend on (a JS function that must exist, a CSS
class that must toggle) — so they are not obviously worthless, but they
share the same weakness: a token can appear in a comment or dead code and
the test still passes. Decide, per test, whether each guards a real
cross-component contract (keep, maybe with a clearer assertion) or is a
change-detector better served by the Vitest suite (collapse).

## Acceptance criteria

- [ ] Audit `mod_matrix_panel_wires_coherence_validation` and
      `mod_matrix_depth_is_bipolar_fader` (`vxn2-ui-web/src/lib.rs`
      ~1001–1066): for each `PANEL_MOD_MATRIX_JS.contains(...)` assertion,
      determine whether it guards a contract the Rust side actually relies
      on. Keep those; collapse or delete pure token-presence checks whose
      behaviour is covered by the Vitest suite.
- [ ] Audit `build_faceplate_html_includes_preset_browser` (~1126–1184):
      the `html.contains(opcode)` / CSS-class checks — same decision.
      Opcode-dispatch contracts that must stay wired: keep (ideally assert
      against the opcode enum, not a string literal). Arbitrary presence
      checks: collapse.
- [ ] Any kept guard gets a one-line comment noting it is a contract guard
      and that behaviour lives in the Vitest suite (matching the pattern
      0163 established), so it isn't mistaken for a change-detector later.
- [ ] `cargo test -p vxn2-ui-web` green; Vitest suite still green.

## Notes

Not a regression from E031 — these tests predate the epic and were
correctly out of 0163's scope. Filed as a discrete follow-up rather than
reopening E031. Low priority: the tests are harmless as-is, this is a
cogency polish. See E031 / 0163 for the asset-present-guard pattern to
follow.

## Close-out (2026-07-24)

Audited all three test clusters in
[lib.rs](../../vxn-2/crates/vxn2-ui-web/src/lib.rs), keeping cross-component
contracts and collapsing pure token-presence change-detectors. Each kept
guard now carries a contract-guard comment naming its counterpart, matching
the 0163 asset-present-guard pattern.

- `mod_matrix_panel_wires_coherence_validation`: KEPT the Rust↔JS coherence
  contract — `matrix.coherence` table consumption + the reason strings
  `self-rate`/`tier-collapse`/`degenerate`, which `Coherence::reason()` emits
  verbatim ([matrix.rs:136-137](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L136-L137))
  and JS maps to tooltips; plus the `vxn-mm-invalid` JS-toggle↔CSS-rule pair.
  DROPPED `validateRow` (internal JS fn name), the `--vxn-error` CSS-internal
  token, and narrowed `.vxn-mm-invalid select` → `.vxn-mm-invalid`.
- `mod_matrix_depth_is_bipolar_fader`: collapsed to the reuse contract —
  mod-matrix reuses the shared `createBipolar` fader primitive (fader.js
  export + matrix consume). DROPPED `vxn-mm-depth` markup, the
  `!type:"range"` negative check, `.vxn-mm-depth-center` CSS, and the
  `dispatchRow(slot, { depth:` literal — all change-detectors on internal
  formatting.
- `build_faceplate_html_includes_preset_browser`: KEPT the ESM-splice
  contract (module spliced + instantiated + `export` marker stripped so the
  inline `<script>` stays valid), the main.js corpus/highlight/follow routes,
  and the opcode-dispatch loop (canonical consumer =
  [faceplate-bridge.mjs](../../vxn-2/crates/vxn2-wasm/web/faceplate-bridge.mjs)
  `DEFERRED_OPS` + `routeOpcode`). DROPPED the 7-id markup loop and the
  three `.browser-*` two-pane/DnD CSS checks. No Rust opcode enum exists
  (opcodes are JS strings), so the loop stays string-literal but now names
  its consumer.
- Verified green: `cargo test -p vxn2-ui-web` (26 passed, 1 ignored); Vitest
  suite under `assets/__tests__/` (35 passed, 5 files).
