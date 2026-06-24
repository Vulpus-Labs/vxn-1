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

## Implementation note (2026-06-24) — code landed, **pending ear verification**

Tooling + auto-bank re-sweep landed; **ticket stays open until loudness is
verified by ear** (and re-verified after 0125's ear-check, since the exponential
ramps shift loudness — see that dependency below).

- **Converter folded into the repo**: [vxn-2/tools/](../../vxn-2/tools/) —
  `dx7_to_vxn2.py` + `dx7decode.py` + `README.md` + `.gitignore`. Paths are now
  repo-relative; ROM dumps are read from `$VXN2_DX7_ROMS` / `tools/roms/` /
  `/tmp` and are **not committed** (Yamaha data, gitignored). Regen is one
  command (README) and deterministic (`clean()` + 15 `KEEP` presets preserved).
- **Master-volume heuristic recomputed for the log curve**: replaced the
  carrier-*count* rule (`-8 - 2.2·(ncar-1)`, tuned for the retired square curve,
  which left the bank too quiet) with a log-curve carrier-**loudness** estimate
  — `Σ 2^((OL-99)/8)` over the algorithm's carriers — gain-matched to
  `TARGET_PEAK_DB = -3` dBFS, clamped `[-24, +6]`.
- **189 auto presets re-swept**: regenerated; the diff is **master-volume-only**
  (verified line-by-line vs the prior bank — no other field moved). New spread:
  median ≈ -9 dBFS, loudest ≈ -3, the quietest single-carrier voices hit the
  +6 clamp. Bank validates: `factory` lib tests green (parses cleanly, zero
  warnings, no incoherent routes); loudest = the +6 clamp, within the
  master-volume range (no parse clamp).
- Remaining for the ear pass (why this stays open):
  - The estimate ignores FM brightness / EG sustain / feedback, so bright or
    sustained patches read quieter than they sound and percussive ones louder —
    tune `TARGET_PEAK_DB` or hand-edit outliers after listening.
  - **The 5 hand-made `KEEP` presets are NOT auto-swept** (AC item) — they need
    hand gain-matching by ear alongside the bank.
  - **0125 dependency**: exponential ramps change perceived loudness; do the
    final gain-match *after* 0125 is ear-verified, then re-run the converter
    (one command) if the target shifts.
  - Spot-check loudest (6-carrier organ) vs quietest categories by ear.
