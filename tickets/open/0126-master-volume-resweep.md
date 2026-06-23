---
id: "0126"
product: vxn-2
title: "Re-sweep factory bank master-volume for the log level curve"
priority: medium
created: 2026-06-23
epic: E026
depends: ["0123"]
---

## Summary

Fourth ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). The log
curve lowers overall loudness (carriers at OL<99 and any op sustaining below
L3=99 are now log-scaled), so the factory bank's `master-volume` values — set by
a carrier-count heuristic for the old linear engine — are now off (everything is
too quiet). Recompute them.

## Design

- Update the master-volume heuristic in the DX7→vxn-2 converter for the log
  curve, then regenerate; hand-tune the 5 curated presets to match.
- Gain-match the whole bank to a reference (e.g. a sustained full-carrier patch
  at a target peak/RMS) so patches sit at consistent perceived loudness.

## Acceptance criteria

- [ ] Converter master-volume heuristic recomputed for log levels.
- [ ] All 194 factory presets re-swept; consistent perceived loudness, no
      clipping on the loudest (6-carrier organ) nor inaudibly-quiet patches.
- [ ] The 5 hand-made presets rebalanced alongside the 189 auto-generated.
- [ ] Spot-check loudest + quietest categories by ear.

## Notes

The converter (`dx7_to_vxn2.py`, currently in `/tmp`) is now load-bearing for
the bank — fold it into the repo (e.g. `vxn-2/tools/`) as part of this work so
regenerations are reproducible. Depends on the curve being final (0123); if
0125 lands, re-verify loudness didn't shift again.
