---
id: "0079"
title: "Recalibrate FB_SCALE_TABLE to DX7 — release through chaos zone clicks"
priority: high
created: 2026-06-11
epic: E006
depends: ["0078"]
---

## Summary

Eighteenth ticket of [E006](../../epics/open/E006-review-remediation.md).
The last click source in the DAW-bounce investigation: every note-off
on the default E.PIANO produced an isolated broadband transient
(|d4| ≈ 0.7, ~1 ms after the off). Per-op state through the click is
completely smooth — EGs march correctly, mods/pan/pitch constant —
the discontinuity is the op6 self-feedback loop itself. At
`fb_scale = 2.0` the loop runs in its chaotic zone; when the
releasing EG sweeps the effective loop gain down through the
stability boundary (~1.0), the oscillation mode collapses within a
couple of samples. A bifurcation — unsmoothable by any ramp.

The structural conflict: `FB_SCALE_TABLE` deliberately extended past
DX7 (`[…, 1.2, 2.0, 3.0]`, "above ~1.0 heads toward noise") while the
default patch copies DX7 ROM FB=6 verbatim onto that hotter scale. A
real DX7 E.PIANO 1 release does not click.

## Design

DX7 feedback is shift-based — exactly ×2 per step — so the table is
now a pure doubling ladder topping out at the saw edge:
`[0, 1/64, 1/32, 1/16, 1/8, 1/4, 1/2, 1]`. ROM transliterations land
on DX7-equivalent loop gains verbatim; the chaotic >1 zone is no
longer reachable (matrix `Feedback` dest still clamps to index 7 =
gain 1.0).

Measured: default-patch note-off worst |d4| drops 0.73 → 0.0014
(≈ 520×), clean at every feedback setting. Side effect: FB 6 energy
is tonal instead of noise-spread, lifting the default patch ~1 dB —
the render test's RMS ceiling moved −9 → −8 dBFS.

## Acceptance criteria

- [x] `note_off_release_is_click_free` — worst post-off |d4| on the
  default patch < 5e-3 for low/mid/high notes (pre-fix ≈ 0.7).
- [x] `fb_scale` unit tests updated to the doubling ladder.
- [x] Full vxn-2 suite green (default-patch RMS window adjusted, see
  above).
- [x] Manual listen: chord sequence releases clean; FB 7 still gives
  the full saw growl.
