# ADR 0004 — VXN3 micro-timing and lane shift

> **Superseded by [ADR 0006](0006-vxn3-groove.md).** The RT model here (§3:
> continuous-timeline lookahead, `fire = step·spacing + laneShift + offset`,
> loop-wrap, retrig/p-lock interaction) is retained *unchanged*. What 0006
> changes: the per-hit offset source is a **groove template** (per-track,
> pooled), not a per-trig field (§1, §4 nudge strip both dropped); timing feel
> gets its own editor, out of the pattern grid.

- **Status:** Superseded by [ADR 0006](0006-vxn3-groove.md) (2026-07-06)
- **Date:** 2026-07-02
- **Scope:** Rhythmic offset in the VXN3 pattern engine — pushing hits off the
  grid, per-trig and per-lane. Refines [ADR 0001](0001-vxn3-overall-design.md)
  §2/§3a, which name micro-timing as a trig attribute but leave it unspecced.

## Context

ADR 0001 lists **micro-timing** among the trig attributes (§3a: "retrig n/m,
probability, condition, velocity, micro-timing") but never defines its units,
range, RT resolution, or UI. For the v1 genre target (psychedelic / minimal
techno) off-grid feel is not decoration — swing, humanised drift, and lanes
phased against each other are core to the idiom. The polymeter model (ADR 0001
§2: independent per-track length + clock divisor) gives *structural* phase for
free; micro-timing gives *expressive* phase on top.

Two distinct needs surfaced:

- **Per-hit nudge** — swing, flams, humanise, "drag" one snare late.
- **Whole-lane slide** — phase an entire track early/late against the others
  without editing every trig, and independent of the clock divisor.

The divisor cannot express either: it scales step *spacing*, it does not
*translate* the grid, and it is uniform across the lane so it cannot nudge one
hit.

## Decision

Two rhythmic-offset levers, both resolved in the lane's own tick base
(polymeter, ADR 0001 §2 — same tick base as p-lock ramps, §3a). They move
*when* a trig fires; p-locks move *what* value it carries. Orthogonal.

### 1. Per-trig micro-timing

A trig attribute (as ADR 0001 §3a lists it): a signed offset on that one hit,
with no base to revert to.

- **Units:** signed fraction of the lane's step.
- **Range:** clamped to **±50%** of the step, so a nudged trig can never reorder
  past either neighbour. This keeps the trig sequence monotonic in fire time and
  bounds the scheduler lookahead (§3).

### 2. Per-lane shift

A per-track constant offset added to *every* trig on the lane.

- Uniform → preserves trig order, so it **may exceed a step** to phase the whole
  lane arbitrarily against the others (a slow lane-shift p-lock over the loop is
  a supported evolution lever).
- Distinct from the clock divisor (ADR 0001 §2): the divisor *scales* step
  spacing, the shift *translates* it.

### 3. Resolution and RT model

Effective fire tick, on the lane's continuous tick timeline:

```text
fire = step·spacing + laneShift + trigMicro
```

- **Lookahead scheduling.** An early-nudged trig must fire *before* its step's
  block is reached; a late-nudged one *after*. The tick engine therefore
  schedules fire times on a continuous per-lane timeline with a **bounded
  lookahead window** (≥ the max negative offset in play), not the naive
  "evaluate the step, fire now." Because offsets are bounded (per-trig ±50% of a
  step; lane shift reduced modulo the loop), the window is const-sized →
  preallocated, alloc-free in `process`.
- **Loop wrap.** A late trig on the last step — or a positive lane shift — can
  push a fire past the loop boundary; it carries into the next loop's opening
  ticks. Symmetric: an early trig on step 0 carries in from the prior loop's
  tail. Consistent with the ADR 0001 §3a latch-across-wrap rule (steady-state
  loops differ from the cold first loop).
- **Interaction with retrig** (ADR 0001 §2). Micro-timing offsets the retrig
  *window origin*; the n-over-m subdivision runs relative to the shifted origin.
  Lane shift moves the window with everything else.
- **Interaction with p-locks** (§3a). Independent axes: a p-lock resolves on the
  tick the trig actually fires, i.e. after offset. A ramp's tick base is
  unchanged (still the lane grid); only the sample point moves.

### 4. UI — dedicated nudge strip

Editing is via a **nudge strip** per track: a thin per-step offset lane under
the grid, a sibling to the p-lock lanes of ADR 0001 §3a, plus one lane-shift
control for the whole track.

- Edge-drag on a trig cell stays reserved for **gate / retrig-span**. Nudge gets
  its own gesture rather than overloading the edge (edge-drag conventionally
  means resize; collapsing it with offset is ambiguous). This overrides the
  "drag cell edge" sketch that prompted this ADR.

## Consequences

- The pattern-engine scheduler is a continuous-timeline lookahead loop from the
  start, not a per-step "fire on this tick" loop. This is the load-bearing
  implementation constraint; retrofitting lookahead later is a rewrite.
- Trig storage grows one signed field (micro-timing); track storage grows one
  (lane shift). Both bounded, no allocation.
- Lane shift is p-lockable like any continuous track param → automatable phase
  drift over the loop, which the genre wants.
- The ±50% per-trig clamp is a deliberate simplification: no per-hit reordering.
  If a "drag past the neighbour" gesture is ever wanted it is a future ADR, not a
  range widening (it breaks the monotonic-fire-time invariant the scheduler
  relies on).

## Alternatives considered

- **Absolute-time (ms) offsets** instead of step fractions: breaks the polymeter
  invariant (ADR 0001: lane time is per-lane ticks, not a global clock) and
  makes swing tempo-dependent in the wrong way. Rejected.
- **Per-trig only, no lane shift:** whole-lane phase then costs editing every
  trig or abusing the divisor. Rejected — lane shift is cheap and the genre
  leans on it.
- **Lane shift only, no per-trig:** cannot do swing or humanise within a lane.
  Rejected.
- **Edge-drag to nudge** (the prompt's sketch): collides with gate/retrig-span
  editing; rejected in favour of the dedicated strip.
