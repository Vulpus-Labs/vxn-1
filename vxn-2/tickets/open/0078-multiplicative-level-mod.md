---
id: "0078"
title: "Multiplicative level modulation — released voices must close"
priority: high
created: 2026-06-11
epic: E006
depends: ["0077"]
---

## Summary

Seventeenth ticket of [E006](../../epics/open/E006-review-remediation.md).
Level modulation was additive post-EG (`clamp(eg + mod, 0, 1)`, ADR
"additive on a [0,1] base"). A positive mod source could therefore
hold a RELEASED voice open: the LFO refilled what release drained
(state dump from the DAW-bounce investigation: 0.26 s after note-off,
`eg = 0.65` falling, `mod = +0.25` rising, sum pinned ≈ 0.9). The
zombie voice droned at the LFO level until the allocator's idle
detection cut it at full amplitude — a loud click on every chord
release with a level route.

## Design

Level mod is multiplicative on the EG: effective level
`= clamp(eg · (1 + mod), 0, 1)`.

- `eg = 0` stays silent — release always closes, no zombie, no
  idle-cut click. (Verified: post-release tail < 1e-3 under a
  full-depth positive LFO; was ~0.6 amplitude.)
- A full-depth sine gates through zero exactly at its trough, where
  the LFO's own slope is zero — tremolo gating is C¹-smooth by
  construction; the bottom clamp corner of 0076 no longer exists.
- Tremolo depth is proportional to the op's level — the musically
  expected response (DX7 AMS is the same shape).
- dB-flavoured response remains available per route via the matrix
  curve column (Exp).

Implementation is the 0076/0077 target formula only —
`clamp(eg·(1+mod),0,1) − eg` instead of `clamp(eg+mod,0,1) − eg` —
projected into the additive `op_level_mod` offset the tick already
reads.

Follow-on cleanup (2026-06-11, "clean tier"): with multiplicative mod
the 0076 target smoother measured zero effect and was removed (see
0076 SUPERSEDED note). The block-rate `clamp(eg·(1+m),0,1)` is now the
single bound for the whole path — it absorbs boost overflow (eg·(1+m)>1
when eg>0.5) and multi-route m overflow alike. Because both the ramp's
endpoints give eff∈[0,1] (start = previous block's in-range level via
the EG rebase, end = this clamped target), the linear ramp stays in
range, so the **per-sample clamp was removed from `stack_tick_*`**
(perf flat, 6.4% RT at 16 notes × density 8). A rail-targeting form
`eg + m·(rail−eg)` was tried first and rejected: it reopens released
voices (eg=0, m>0 → eff=m, not 0 — the `released_voice` test caught
it). Only `eg·(…)` guarantees release-closes.

Pitch and feedback were audited for the same semantics question and
need no change: pitch dests are semitone-domain (additive st =
multiplicative Hz via `2^(st/12)`), and `FB_SCALE_TABLE` is quasi-log
(≈ ×2 gain per index step), so additive index mod is already
≈ multiplicative loop-gain mod.

## Acceptance criteria

- [x] `released_voice_closes_under_positive_level_mod` — tail silent
  0.25 s after release with full-depth positive LFO on a carrier.
- [x] Convergence + corner + zipper regression suites green with the
  multiplicative formula.
- [ ] Manual listen: chord sequence with LFO → carrier level — no
  click at releases, no post-release drone.

## Notes

ADR amendment required: the matrix ADR's "additive on a [0,1] base"
sentence is superseded by this ticket.
