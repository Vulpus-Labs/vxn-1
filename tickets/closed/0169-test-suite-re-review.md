---
id: "0169"
product: monorepo
title: Re-run the five-way test-quality review after remediation
priority: low
created: 2026-07-01
epic: E031
---

## Summary

Closing gate for E031. After 0161–0168 land, re-run the same test-quality
review that produced the epic and confirm the four buckets (non-cogent,
redundant, substring-only, buried setup) are cleared without loss of real
coverage. This ticket is the verification step, not more remediation — any
genuinely new findings become follow-up tickets, they do not block the
close.

## Acceptance criteria

- [x] All of 0161–0168 closed.
- [x] Re-run the five-way review (same slices as 2026-07-01): (1) vxn-1
      engine; (2) vxn-1 dsp/app/ui/clap/wasm; (3) vxn-2 engine; (4) vxn-2
      dsp; (5) vxn-2 app/ui/clap — same four-category rubric (redundant,
      not-cogent, unclear, shared-apparatus).
- [x] Confirm each specific finding cited in 0161–0168 is resolved (spot-
      check by test name, since line numbers will have moved).
- [x] Confirm no coverage regression: workspace `cargo test` green, Vitest
      green, and no production behaviour lost a test (deletions were all
      subsumption-justified in 0162 / behaviour-in-Vitest in 0163).
- [x] Verify a sample of the repaired non-cogent tests (0161) now fail when
      their feature is deliberately broken.
- [x] File any net-new findings as fresh tickets under a follow-up (do not
      reopen E031); record the review summary in the close-out.

## Notes

Line numbers throughout 0161–0168 are as-reviewed on 2026-07-01 and will
drift as earlier tickets land — this re-review keys off test *names*, not
lines. If the re-review is clean, close E031.

## Close-out (2026-07-02)

Re-review run: five fresh Sonnet agents over the same slices as the
2026-07-01 review (vxn-1 engine; vxn-1 dsp/app/ui/clap/wasm; vxn-2 engine;
vxn-2 dsp; vxn-2 app/ui/clap), each cross-checking the 0161–0168 close-outs
against the current tree.

**Verdict: all four buckets cleared, no regressions, no false-confidence
tests introduced.**

- **BUCKETS CLEARED** on four slices (vxn-1 engine, vxn-2 engine, vxn-2 dsp,
  vxn-2 app/ui/clap). Confirmed: deletions all subsumption-justified;
  extracted helpers (clean_sine_synth, assert_all_params_match,
  render_blocks/engine_with_route, worst_d4, test_util, TestPresetStore,
  vxn-core-clap `testing`) don't weaken assertions; the context_override
  TABLE preserves every branch; test-support feature is off by default and
  absent from release (`nm`).
- **Coverage:** full `cargo test --workspace` = 1000 passed, 0 failed, 75
  suites ok (true exit code, not a piped tail).
- **Teeth check:** `resolve_route_clamps` (repaired in 0161) was run against
  a deliberately broken clamp (wrap instead of saturate) and FAILED as it
  should ("algo 33 must clamp to algo 32's route"); reverted clean.
- **Net-new (actioned):** two minor items fixed in `de3f4ff` — an in-code
  note on the deferred `key_mode_applied_before_events`, and a dead `f_slave`
  binding in poly/oscillator.rs.
- **Net-new (deferred):** a cluster of pre-E031 contract-token tests in
  vxn2-ui-web (`mod_matrix_panel_wires_*`, `build_faceplate_html_includes_
  preset_browser`) — out of E031 scope, filed as follow-up ticket 0170.

E031 remediation verified complete.
