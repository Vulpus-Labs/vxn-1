# ADR 0007 — DX7-faithful logarithmic operator/EG level curve

- **Status:** Accepted
- **Date:** 2026-06-23
- **Scope:** The level→amplitude mapping for both EG L-values and operator
  output level in the VXN2 FM kernel.
- **Epic:** [E026](../../epics/open/E026-dx7-log-level-curve.md)

## Context

VXN2 mapped operator output level **linearly** (`op.rs` / `stack.rs` cook:
`level / 99`) and EG L-values via a **perceptual square** curve
(`eg.rs level_to_amp`: `(L/99)^2`).

The DX7 does neither. On real hardware both operator output level (OL) and EG
levels live in a single **logarithmic** domain — roughly 0.75 dB per step — and
that level controls modulation index, not just final loudness. The consequence
of the linear/square mapping was that a modulator at a moderate level was
dramatically hotter than on a DX7:

| L = OL | VXN2 square `(L/99)^2` | DX7 log (~0.75 dB/step) | error |
|-------:|-----------------------:|------------------------:|------:|
| 52     | ~0.525                 | ~0.017                  | ~30×  |

A modulator ~30× too hot injects far too much FM, which is the root cause of the
recurring "too bright / buzzy / noisy" reports across the DX7 factory bank. The
smoking gun was **SAX BC** op5 — an inharmonic 5.8-ratio modulator that blared
instead of shimmering because its mid-range level sat near full modulation index.

## Decision

### 1. One logarithmic level curve, shared by EG levels and operator output level

`eg::level_to_amp(L)` is the single source of truth:

```
level_to_amp(0)  = 0          (true silence; the curve never reaches 0 otherwise)
level_to_amp(L)  = 2^((L-99)/8)   for L in 1..=99
```

- 0 dB (amplitude 1.0) at L = 99.
- **−6 dB per 8 steps** (the `/8` in the exponent).
- ≈ −74 dB at L = 1; hard 0 at L = 0.

Both the EG L-values (`EgState::cook`) and the **operator output level**
(`op.rs` / `stack.rs` cook → `level_norm`) route through this one function, as on
DX7 where OL and EG levels share a domain.

### 2. Calibration

Cross-checked against the DX7's ~0.75 dB/step at L = 50: `2^((50-99)/8)` ≈
−36.9 dB, i.e. **L = 50 ≈ −37 dB**. The chosen 6 dB / 8 steps = 0.75 dB/step
matches the hardware closely enough that translated ROM voices sound right
without per-patch level fudging.

### 3. This is perf-neutral

The curve is evaluated only in `cook` — control rate, scalar, once per operator
per block (`voice.rs eg_tick`). The per-sample NEON lane loop reads only the
precomputed `eg.level` scalar. The mapping change, the later per-op flag (0124),
and exponential ramps (0125) never enter the vectorized inner loop. See
[[vxn1-soa-match-defeats-simd]].

### 4. Recalibration policy

Faithful DX7 levels are now **correct by construction**. Therefore:

- Translated DX7 ROM voices keep their real OL/EG values — no scaling fudge.
- Any preset that was **hand-tuned for the old linear/square engine** must be
  restored to its true DX7 value. Precedent: **Mark II E-Piano** tine modulator
  was nursed down to OL 17 to tame the over-hot linear engine; under the log
  curve it is restored to the real **58**.
- Overall bank loudness drops (carriers at OL < 99 and ops sustaining below
  L3 = 99 are now log-scaled), so factory `master-volume` is re-swept against the
  new curve (0126) rather than left at the old carrier-count heuristic.

## Consequences

- The bank-wide brightness/buzz is resolved at the source instead of per-patch.
- Old-engine hand-tunings are now wrong and must be reverted to DX7 truth.
- The square curve does not disappear — it survives as the per-op **`Lin`**
  escape hatch (0124), with **`Exp`** (this log curve) the default.
- The level→amplitude *mapping* is fixed here; the EG ramp **shape** stays
  linear-in-amplitude until 0125 adds log-domain (exponential) marching for the
  `Exp` path.

## Status of the prototype const

This curve landed first behind a global `EG_LOG_LEVELS` const for a clean A/B by
ear. That const is a temporary scaffold: it is removed in **0124**, where the
choice becomes a per-operator `eg-curve` field (`Exp` default, `Lin` legacy).
Related: [[vxn2-dx7-factory-translation]].
