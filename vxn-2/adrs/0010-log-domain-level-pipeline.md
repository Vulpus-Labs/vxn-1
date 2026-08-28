# ADR 0010 — One log-domain level accumulator for the operator amplitude chain

- **Status:** Accepted
- **Date:** 2026-08-29
- **Scope:** How every contributor to an operator's amplitude combines, from the
  patch's output level to the value the per-sample lane loop reads. Epic E048.
  Completes the direction set by [ADR 0007](0007-dx7-log-level-curve.md), which
  put the level→amplitude *mapping* in the log domain but left the contributors
  combining as linear multipliers.

## Context

The reference hardware keeps one integer accumulator in **level units** of
~0.75 dB, adds every amplitude contributor into it, clamps twice, and converts
to a linear gain exactly once, at the end, via a table:

```
outlevel  = scaleoutlevel(OL)              // table: 0..99 → 0..127
outlevel += ScaleLevel(note, bp, …)        // key level scaling, table
outlevel  = min(127, outlevel)             // ceiling, before velocity
outlevel <<= 5                             // → 1/32-level-unit resolution
outlevel += ScaleVelocity(velocity, kvs)   // table, signed — can boost
outlevel  = max(0, outlevel)               // floor only; no second ceiling
gain      = Exp2::lookup(env_level - unity)   // the single conversion
```

VXN2 reached the same result by a different route: each contributor was
converted to a linear multiplier independently and the multipliers were
multiplied together. Mathematically that is the same operation. In practice it
made a specific class of mistake easy to write and hard to see, and we shipped
three of them:

| contributor | what it did | why the domain hid it |
|---|---|---|
| key level scaling | linear amplitude fade, normalised to the keyboard edge | a "depth" that fades a multiplier toward 0 looks reasonable; as a dB-per-semitone slope it is obviously not one |
| velocity | `1 − vs·(1 − v²)`, ceiling 1.0 | a multiplier naturally tops out at unity; a *signed level offset* naturally does not, and the hardware's is +5.25 dB at kvs 7 |
| the ceiling | `.min(1.0)` on the linear product | happens to equal `min(127)` only because `level_to_amp(99) = 1.0` — a coincidence, not a statement |

The first produced −0.1 dB where hardware produced −49 dB (a control that did
nothing). The second leaves a `kvs 7` modulator ~5 dB under hardware at normal
playing velocity — the missing "ting" on every tine patch. Neither is a wrong
constant; both are contributors that were never in the domain the calibration is
expressed in.

The pitch chain already works the hardware's way — `apply_pitch_mult` sums bend,
glide, pitch EG, global/per-op pitch mod and stack detune **in semitones**, then
applies one `2^(st/12)`. It has had no calibration bugs.

## Decision

### 1. One accumulator, in level units, at 1/32 resolution

`OpState::cook` (and `Stack::cook_op`) build a single `i32` level value in the
hardware's post-`<<5` resolution — 1/32 of a level unit, ~0.0235 dB — from:

```
level_units(OL) + ks_level_offset(…)  → clamp to ceiling → ×32
                                      + vel_level_offset(…)
                                      → clamp to floor
```

then convert **once** to the linear `max_amp` that seeds `EgState`. Contributors
may only be *added*. `ks_level_mult` and `vel_factor` are retired as separate
multipliers; `ks_level_offset` already returns units and simply stops being
exponentiated.

### 2. Tables where quantisation is audible, `exp2` where it is not

The hardware used tables because it had no FPU. We reproduce a table only where
its **quantisation is part of the sound**:

- `ScaleCurve` / `ScaleVelocity` / `scaleoutlevel` — integer tables, ported
  exactly, including truncation. The steps are audible at low depths.
- The final level→amplitude conversion — `exp2`, not a table. It runs at control
  rate, where it costs nothing and is *more* accurate than the hardware lookup.
  Reproducing that table's error would be imitation, not fidelity.

### 3. Two contributors stay out

- **Feedback** scales a phase-modulation signal, not an amplitude. It is a
  power-of-two ladder (`2^(fb − 8)`) and so looks log-shaped, but it belongs to
  the phase domain and folding it into the level accumulator would be a category
  error.
- **Matrix level modulation** (`eff = eg·(1 + m)`, clamped) is a VXN2 invention
  with no hardware counterpart; its multiplicative-on-EG semantics and frozen
  tick formula are settled (0074–0078) and stay as they are.

Also out, as not being level-domain concepts at all: pan, the Nyquist carrier
fade, per-lane stack gain, master volume.

### 4. Full scale is the maximum attainable level, not nominal

Hardware clamps the accumulator at the bottom only after velocity, so a `kvs 7`
operator at velocity 127 sits **+5.25 dB above** its nominal level. VXN2 cannot
currently represent that: `cook_stacks_block` stage 8 computes
`eff = (eg·(1 + m)).clamp(0.0, 1.0)`, and that single bound is what allows the
per-sample ramp to skip a clamp of its own — both endpoints in range implies the
interpolation is in range.

Rather than raise that ceiling and re-establish the invariant, **renormalise**:
amplitude `1.0` denotes the maximum attainable level (nominal + maximum velocity
boost, a factor of 1.83), so a nominal `OL 99` carrier sits at `0.546`. The
`[0, 1]` invariant is untouched, the hot loop is unchanged, and the resulting
−5.25 dB of output is absorbed by one master-volume re-sweep.

### 5. Pitch is already conformant

`apply_pitch_mult` stands. The one remaining divergence is `compute_base_hz`,
which applies the ratio as a linear Hz multiply and detune as its own
`2^(cents/1200)` — equivalent, but off the accumulator. Folded in for
consistency, not correction.

## Consequences

- The three shipped bugs become inexpressible: a contributor that is not a
  signed level offset does not type-check into the accumulator.
- Calibration is **not** thereby solved. The feedback ladder was wrong by one
  rung (`2^(fb−7)` for `2^(fb−8)`) on a correctly-logarithmic scale, and would
  have been just as wrong as a table. Each table is still verified individually
  against the reference. This ADR makes calibration errors visible and
  expressible; it does not make them absent.
- Cheaper: three `exp2` per operator per cook become one.
- Every preset's velocity response changes, and unlike the feedback ladder there
  is no mechanical migration — a curve change is not a gain change. Patches
  voiced hard get brighter and louder; patches voiced soft barely move.
- One bank re-audition covers this, the key-scaling fix and the feedback ladder
  together, instead of three passes.

## Prerequisites

The key-scaling port and the feedback ladder land first: this ADR generalises
them, and E048's first ticket consumes `ks_level_offset` directly.
