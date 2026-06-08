---
id: "0105"
title: Faceplate — Voice block Spread fader
priority: medium
created: 2026-06-07
epic: E019
---

## Summary

Add a Spread fader column to the Voice panel of the faceplate.
Rebalance row-4 flex-grow ratios to claim horizontal space from FX
and Master.

## Acceptance criteria

- [ ] `crates/vxn-ui-web/assets/faceplate.html`: new `.ctl` div in
      the Voice panel's `.panel-body`, appended after the Glide
      fader column. Standard fader column markup matching Glide.
- [ ] Fader binds to `PatchParam::Spread`.
- [ ] `crates/vxn-ui-web/assets/faceplate.css`: adjust panel
      flex-grow ratios on row 4 — Voice 1.05 → 1.25, FX 1.80 → 1.70,
      Master 1.75 → 1.65. Confirm FX tab strip + 4-column body and
      Master's 3 faders + 2-control strip still fit visually.
- [ ] Snapshot / pixel-diff test (if the JS test framework from
      E015 covers it): voice panel layout updated, no regressions
      in FX or Master.
- [ ] Loads in CLAP, fader drives the underlying param, value
      persists across reload.

## Notes

The flex rebalance numbers preserve the row-4 sum (4.60). FX takes
0.10 off, Master takes 0.10 off — smaller pull than the original
plan since only a fader is added (no buttongroup row).

Single fader column, no toggle below it — the Mono/Stereo switch
was dropped (no CPU saving justifies the extra surface; spread=0
already gives mono behaviour bit-identically). See epic E019
background for the rationale.
