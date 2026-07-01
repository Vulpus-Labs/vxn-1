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

- [ ] All of 0161–0168 closed.
- [ ] Re-run the five-way review (same slices as 2026-07-01): (1) vxn-1
      engine; (2) vxn-1 dsp/app/ui/clap/wasm; (3) vxn-2 engine; (4) vxn-2
      dsp; (5) vxn-2 app/ui/clap — same four-category rubric (redundant,
      not-cogent, unclear, shared-apparatus).
- [ ] Confirm each specific finding cited in 0161–0168 is resolved (spot-
      check by test name, since line numbers will have moved).
- [ ] Confirm no coverage regression: workspace `cargo test` green, Vitest
      green, and no production behaviour lost a test (deletions were all
      subsumption-justified in 0162 / behaviour-in-Vitest in 0163).
- [ ] Verify a sample of the repaired non-cogent tests (0161) now fail when
      their feature is deliberately broken.
- [ ] File any net-new findings as fresh tickets under a follow-up (do not
      reopen E031); record the review summary in the close-out.

## Notes

Line numbers throughout 0161–0168 are as-reviewed on 2026-07-01 and will
drift as earlier tickets land — this re-review keys off test *names*, not
lines. If the re-review is clean, close E031.
