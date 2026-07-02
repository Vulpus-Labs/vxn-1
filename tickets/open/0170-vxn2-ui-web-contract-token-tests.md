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
