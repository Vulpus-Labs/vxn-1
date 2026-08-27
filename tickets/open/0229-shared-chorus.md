---
id: "0229"
product: monorepo
title: "Shared StereoChorus, true-stereo only — mono-sum process deleted"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

Second ticket of [E041](../../epics/open/E041-shared-fx-unification.md).
vxn-1's `StereoChorus` has two non-equivalent entry points:
`process_block_stereo`
([chorus.rs:150](../../vxn-1b/crates/vxn-dsp/src/chorus.rs#L150), true stereo —
what vxn-1's engine uses) and per-sample `process`
([chorus.rs:201](../../vxn-1b/crates/vxn-dsp/src/chorus.rs#L201), **mono-sums
the input** — what vxn-1b's FxChain uses). A naive per-sample trait with a
blanket block impl would silently change vxn-1's sound. Resolution: the shared
kernel keeps only the true-stereo path; `FxKernel::process` becomes a
length-1 call into the stereo body; the mono-sum variant is deleted.

## Acceptance criteria

- [ ] `StereoChorus` in `vxn-core-dsp::chorus` implementing `FxKernel` with
      WetFade; block override tested sample-identical to per-sample form.
- [ ] vxn-1 adoption: block-stereo body moved verbatim — target **no**
      re-baseline (verify baseline + declick byte-identical; if the WetFade
      swap from `chorus_fade` shifts the toggle envelope, that lands as the
      flagged part).
- [ ] vxn-1b adoption (same commit as vxn-1): mono-sum → true-stereo is
      audible → `REBASELINE:` vxn-1b goldens with A/B notes.
- [ ] Outer fades deleted both sides; grep check.

## Notes

Chorus is vxn-1-lineage (vxn-2 has none) — the shared kernel makes it
importable to vxn-2/vxn-3 later by constructing its Params.
