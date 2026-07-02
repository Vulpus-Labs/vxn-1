---
id: E031
product: monorepo
title: Test-suite remediation (2026-07-01 test review) — kill non-cogent tests, prune redundancy, extract shared apparatus
status: closed
created: 2026-07-01
---

## Goal

Remediate the findings of the 2026-07-01 test-quality review (five
agent sweeps over vxn-1 and vxn-2: engine, dsp, app/ui/clap for each).
The suites are healthy overall — DSP numeric-property tests, golden-byte
codec tables, and the alloc-fixture pattern are exemplary — so this is
refinement, not rescue. The review sorted every finding into four
buckets, and this epic clears them in priority order:

1. **Non-cogent tests (highest value).** ~12 tests assert setup,
   tautologies, or constants rather than behaviour — they pass whether
   or not the feature is correctly implemented. These are the only tests
   that give false confidence, so they go first (0161).

2. **Redundant coverage.** A stronger test already subsumes a weaker
   one, or the same literal edit-list is duplicated across two layers
   and can drift silently. Prune the subset, dedupe the literal (0162).

3. **The vxn-ui-web substring block.** ~30 `faceplate_*_wired` tests
   assert only that a JS/CSS token *appears* in an assembled string —
   presence, not behaviour; a token in a comment passes as readily as a
   live call site. The real behavioural net is the gated Vitest suite.
   Collapse to a few asset-present guards (0163).

4. **Shared apparatus.** The densest copy-paste: the vxn-2 engine
   render loop (~25×), the 4th-difference click detector (5 files), the
   ADSR lifecycle driver, the bit-exact-passthrough loop, four synth
   builders, three `PresetStore` impls, and the CLAP event-buffer ritual
   that both vxn-1 and vxn-2 open-code. Extract per-crate helpers
   (0164–0166) and one cross-crate CLAP test-support module (0167).

5. **Buried setup.** A handful of tests hide the asserted property under
   hand-rolled window arithmetic or long `if`-fallthrough context setup;
   rewrite so the property is legible (0168).

When the work lands, re-run the same five-way review to confirm the
buckets are cleared and no coverage was lost (0169).

## Scope

**In:** deleting/repairing non-cogent tests; pruning subsumed tests and
deduping shared literals; collapsing the ui-web substring tests to
asset-present guards; extracting per-crate and cross-crate test helpers;
rewriting the buried-setup tests; a closing re-review.

**Out:** adding new feature coverage; changing production (non-test)
code, except where a test helper legitimately lives in a `#[cfg(test)]`
module of a production crate; touching vxn-3 (excluded from the review).

## Constraint

No net loss of real coverage. Every deletion must be justified by a named
test that already covers the property. Every repaired test must, after
the change, *fail* if its feature is broken (that is the whole point of
the non-cogent bucket). The full `cargo test` workspace stays green.

## Tickets

Independent quick wins (do first, any order):
- 0161 — repair/delete non-cogent tests + stale-framing rename
- 0162 — prune redundant tests + dedupe duplicated literals
- 0163 — collapse vxn-ui-web / vxn2-ui-web substring "wired" tests

Shared-apparatus extractions (parallel):
- 0164 — vxn-2 engine: render/click/route helpers
- 0165 — vxn-2 dsp: lifecycle/passthrough/energy/patch helpers
- 0166 — vxn-1 engine + app: synth-builder / preset / PresetStore helpers
- 0167 — cross-crate CLAP test-support module (vxn-1 + vxn-2)

Clarity rewrites:
- 0168 — rewrite buried-setup tests (timeline-slice, context table)

Closing gate:
- 0169 — re-run the five-way test-quality review

## Notes

Source review: 2026-07-01, five general-purpose agents (vxn-1 engine;
vxn-1 dsp/app/ui/clap/wasm; vxn-2 engine; vxn-2 dsp; vxn-2 app/ui/clap).
Every ticket cites specific `file:line` findings from that review. Line
numbers are as-reviewed on 2026-07-01 — re-grep by test name before
editing, since earlier tickets in this epic will shift later line numbers.

## Close-out (2026-07-02)

All nine tickets (0161–0169) closed. The 2026-07-01 test-quality review's
four buckets are cleared:

- **Non-cogent (0161):** 14 tests repaired/deleted/trimmed, 1 documented
  skip; the repaired tests now read real output (one verified to fail on a
  deliberately broken feature during the 0169 re-review).
- **Redundant (0162):** 9 subsumed tests deleted (each covering test
  verified), 2 pairs merged, the state-round-trip EDITS literal deduped.
- **Substring-only (0163):** ~18 ui-web `faceplate_*_wired` token tests
  collapsed to asset-present guards; behaviour left to the Vitest suite.
- **Shared apparatus (0164–0167):** per-crate helpers (render_blocks,
  worst_d4, test_util, clean_sine_synth, TestPresetStore) + a cross-crate
  `test-support` feature on vxn-core-clap; single-source EDITS.
- **Clarity (0168):** timeline-slice envelope test, render_with_note_on/off,
  and the param_audibility context TABLE.

Verified: full `cargo test --workspace` = 1000 passed / 0 failed / 75 suites;
five-way re-review (0169) found no regressions and no new false-confidence
tests. One follow-up filed outside this epic: **0170** (pre-existing
vxn2-ui-web contract-token tests, out of the original review scope).

Delivered by fanning the tickets out to Sonnet implementation agents in
waves (0161→0162 serial; 0163–0166 parallel; 0167→0168 serial), each gated
(test-only diff check + green tests) before commit.
