---
id: E002
title: Oscillator interaction & expressive control
status: closed
created: 2026-05-24
---

## Goal

Bring the two oscillators into relationship and make that relationship playable:
**hard sync** (osc2 slaved to osc1), **cross-modulation / linear FM** (osc2 →
osc1 pitch), and the MIDI **pitch-bend / mod-wheel** routing that makes them
expressive — pitch bend → pitch, mod wheel → cutoff or osc2. These are ADR
0002's #1/#2 features plus the bend hook deferred since ADR 0001.

Sync and cross-mod are grouped deliberately: both couple osc2 → osc1 *within a
single sample*, which breaks the independent, vectorised
`osc1.process(); osc2.process()` loop in the same way. We pay that hot-path
refactor once and build both on it. The mod-wheel → osc2 route is the payoff
gesture for sync (played pitch comes from osc1; the wheel sweeps the synced osc2
to move the formant), so it ships in the same epic.

## Scope

**In:**

- `vxn-dsp`: a coupled oscillator-pair process path that computes osc2 first,
  then osc1, supporting (a) hard sync — reset osc2 phase when osc1 wraps — and
  (b) cross-mod — osc2 output modulates osc1's per-sample phase increment.
- The existing independent path stays as the fast path when sync is off and
  cross-mod depth is zero (no perf regression for plain patches).
- Engine params: sync on/off, cross-mod depth; `BlockCtx` plumbing.
- MIDI: wire pitch-bend events to the existing `set_pitch_bend` hook; add a
  mod-wheel (CC1) hook routable to cutoff or osc2 pitch with a depth.

**Out (deferred):**

- BLEP/minBLEP anti-aliasing of the sync discontinuity — v1 leans on
  oversampling; band-limited sync correction is a documented follow-up.
- Editor controls for the new params (rides the next UI pass).
- Unison, portamento, env time-scaling, second LFO, key modes — later epics.
- Per-note-expression / MPE bend; per-channel anything. Global bend + CC1 only.

## Tickets

- [x] [0004 — Hard sync (coupled osc path)](../../tickets/closed/0004-hard-sync.md)
- [x] [0005 — Cross-mod / linear FM](../../tickets/closed/0005-cross-mod.md)
- [x] [0006 — MIDI pitch-bend + mod-wheel routing](../../tickets/closed/0006-midi-bend-modwheel.md)

## Dependency order

```text
0004 (hard sync) ──> 0005 (cross-mod)      both build on the coupled osc path
0006 (MIDI bend/wheel)                     independent (event layer)
```

0004 introduces the coupled osc2→osc1 path; 0005 extends it with the FM term.
0006 is independent and can land in parallel, but mod-wheel → osc2 is only
*audibly interesting* once 0004 lands.

## Acceptance

- Enabling sync produces the classic hard-sync timbre; sweeping osc2 pitch (by
  knob or mod wheel) sweeps the synced formant.
- Cross-mod adds metallic/sideband content scaling with depth; at depth 0 the
  output is bit-identical to today.
- Pitch bend bends pitch (±2 st default range); mod wheel moves cutoff or osc2
  pitch per its routing, smoothed, no zipper.
- Plain (no-sync, zero cross-mod) patches keep the vectorised fast path — no
  measurable CPU regression.
- No RT allocation; no `unwrap`/`expect` in DSP; poly kernels stay finite for
  inactive/frozen voices.
