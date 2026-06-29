---
id: E026
product: vxn-2
title: DX7-faithful operator level curve + per-op EG curve mode
status: closed
created: 2026-06-23
---

## Goal

Ship the DX7-faithful **logarithmic** operator/EG level curve as the engine
default, with a per-operator `Lin | Exp` escape hatch, and rebalance the
factory bank around it.

vxn-2 mapped operator output level **linearly** (`op.rs` cook: `level/99`) and
EG L-values via a **perceptual square** (`eg.rs level_to_amp: (L/99)^2`). DX7 is
**logarithmic** (~0.75 dB/step; both OL and EG levels live in one log domain).
The practical consequence: a modulator at a moderate level (e.g. OL 52) sat at
~0.525 amplitude in vxn-2 vs ~0.017 on a DX7 — **~30× too hot** — so every
mid-level modulator injected far too much FM. That is the root cause behind the
recurring "too bright / buzzy / noisy" reports across the DX7 factory bank
(SAX BC op5 being the smoking gun: an inharmonic 5.8 modulator blaring instead
of shimmering).

A prototype (currently uncommitted in the working tree) put the curve
`amp = 2^((L-99)/8)` behind a global `EG_LOG_LEVELS` const in `eg.rs`, routed
both the EG L-values and the operator output level through it, and was confirmed
by listening as a **massive improvement across the board**. Because faithful DX7
levels now sound correct, the only hand-made preset needing a touch-up was
Mark II E-Piano (tine modulator restored 17 → the real 58).

## Why now

- The prototype is proven good by ear but lives behind a throwaway const with a
  broken regression test and an unbalanced bank — it needs productionizing.
- The fix is **perf-safe**: the level curve is computed in `cook` (control rate,
  scalar per op — `voice.rs eg_tick` runs ~once per block), and the per-sample
  NEON lane loop only reads the precomputed `eg.level` scalar. So the curve
  change, the per-op flag, and even exponential ramps never touch the vectorized
  inner loop. Verified during prototyping; see [[vxn1-soa-match-defeats-simd]].

## Tickets

- 0123 — Ship the DX7 log level curve as the engine default (+ ADR)
- 0124 — Per-op EG curve mode param (`Lin | Exp`, default `Exp`)
- 0125 — Exponential EG ramps for the Exp path (log-domain march)
- 0126 — Re-sweep factory bank master-volume for the log curve
- 0127 — Fix `every_param_sweep_is_audible` under log levels
- 0128 — Faceplate UI selector for op eg-curve (optional)

## When this epic closes

- The log level curve is the shipped default; `EG_LOG_LEVELS` prototype const is
  gone (superseded by the per-op `eg-curve` param, default `Exp`).
- Per-op `Lin | Exp` selectable and round-trips through preset TOML + host state.
- Exp path has DX7-shaped exponential ramps; Lin path preserves legacy behavior.
- The 194-preset factory bank is loudness-rebalanced; no clipping, no
  unintentionally-quiet patches.
- All vxn2-dsp + vxn2-engine tests green (incl. the audibility guard), no SIMD /
  CPU regression on the idle + full-poly benches.

## Notes

Curve calibration cross-checked against DX7 (L=50 ≈ −37 dB at 0.75 dB/step).
The DX7→vxn-2 converter and the uncommitted detune/SAX-ratio preset fixes from
the same work session land alongside this epic. Related: [[vxn2-dx7-factory-translation]].
