---
id: "0235"
product: vxn-1
title: "(stretch) vxn-1 OutputStage adopts OsRegion for decimator + oversample-change declick"
priority: low
created: 2026-08-02
epic: E042
depends: ["0234"]
---

## Summary

Stretch ticket of [E042](../../epics/open/E042-oversampled-region.md),
explicitly skippable. vxn-1 renders the whole voice path at the OS rate and
only decimates — its `OutputStage`
([lib.rs:430](../../vxn-1/crates/vxn-engine/src/lib.rs#L430)) with the
OS-change crossfade-from-held-level (`os_hold_l/r`, `OS_FADE_MS = 5`, E035
close-out) could sit on `OsRegion` for the decimator + fade half. Proves the
shared unit fits the "whole path oversampled, decimator-only" shape, not just
vxn-2's bracketed span.

## Acceptance criteria

- [ ] `OutputStage` uses `OsRegion` for decimators + OS-change fade; the
      crossfade-from-held-level behaviour preserved (not fade-in-from-zero —
      see E035 0191 close-out).
- [ ] vxn-1 baseline + `oversampling_change_is_declicked` byte-identical, or
      ticket is dropped with a note on the mismatch.

## Notes

If `OsRegion`'s shape doesn't fit without contortion, closing this as
"won't-do, boundary confirmed" is a valid outcome — the boundary test says a
shared type must fit without fake parameters.
