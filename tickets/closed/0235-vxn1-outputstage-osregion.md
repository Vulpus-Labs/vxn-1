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

## Close-out (2026-08-27) — won't-do, vxn-1 retired

Closed unbuilt. This ticket's entire subject is vxn-1's `OutputStage`, and vxn-1
was retired on 2026-08-27 (archived under `archive/vxn-1/`, out of the
workspace, not expected to compile).

It was already marked "(stretch), explicitly skippable", and its stated purpose
was to *prove* `OsRegion` fits the "whole path oversampled, decimator-only"
shape as well as vxn-2's bracketed span. That evidence is now unobtainable from
vxn-1 — but the shape has another live instance: **vxn-1b's `output.rs` does
exactly the same thing** (whole voice path at OS rate, decimate + crossfade on
an OS change, `OS_FADE_MS = 5`). If E042 still wants the second shape proven,
re-file it against vxn-1b rather than resurrecting this.

E042's remaining chain is unaffected: 0233 → 0234 stand.
