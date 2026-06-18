---
id: E022
product: vxn-2
title: "vxn-2 stack pitch modulation — ratio-locked pitch mod across an FM branch"
status: closed
created: 2026-06-18
depends-on: none
---

> Adds a pitch-only modulation destination that targets an operator **and
> its whole ratio-coherent stack** at once, so a single pitch route bends a
> carrier and every operator feeding it by the same amount. The FM ratios
> stay locked → the tone shifts pitch without shifting timbre.

## Goal

A mod-matrix pitch route can target an operator's **stack** instead of a
single op. At cook time the target expands to the whole connected component
of that op in the algorithm's modulation graph, and the route's modulated
pitch value is applied identically (same semitone delta) to every op in the
component.

When this epic closes:

- A pitch route to "Op N stack" bends op N and all ops connected to it
  (ancestors *and* descendants) by the same semitone offset.
- FM ratios within the branch are preserved under the modulation — pitch
  moves, timbre holds.
- Fixed-frequency ops act as walls: they are never pitch-modded and
  traversal cannot cross them, so each ratio-coherent sub-region is its own
  component.
- The target set re-resolves when the algorithm changes or any op crosses
  the Ratio↔Fixed boundary.

## Why this feature

Per-op pitch mod already exists, but modding a single op in an FM stack
detunes it *relative to* its neighbours → the ratio breaks → the timbre
smears instead of the pitch bending cleanly. To bend a whole tone in tune
you must apply the *same* pitch delta to every op in the branch. Doing that
by hand is N routes that must be kept in lock-step. This makes it one route.

## Semantics (settled in design)

- **Pitch only.** Same mod domain/curve/depth as the existing per-op pitch
  dest — this is pure target fan-out, no new mod-value code.
- **Equal delta, not depth-scaled.** Ratio-lock *requires* every op shift by
  the same semitones; depth-scaling would break it. The simplest law is the
  correct one.
- **Whole connected component**, undirected over modulation edges. Targeting
  a mid-stack op propagates both up (ancestors/modulators) and down
  (descendants/carriers).
- **Fixed-freq ops are connectivity walls.** A fixed-Hz op does not track
  key, so tuning coherence genuinely stops there. Remove it as a graph
  *node* (not just from the result) so a fixed op mid-chain splits the graph
  into independent components.
- **Shared modulators blow the component wide** (DX7 algos share modulators
  across carriers). Targeting a shared mod can sweep most of the patch —
  correct for ratio-lock, but a documented user-facing surprise.
- **Target op itself fixed** → component is just-itself / empty → no-op the
  stack route (optionally a UI hint).

## Architecture impact

One new input to an existing cook step. The mod-target resolver now reads
op freq-mode in addition to the algorithm, so the dirty-bitset pump
(ADR 0003) gains two trigger sources: `algo` and per-op `ratio-mode`. Only
the **Ratio↔Fixed toggle** re-cooks, not ratio-value tweaks. Resolver stays
a pure function `(algo, [ratio_mode; 6], target_op) → op bitset`.

## Scope

**In:**

- A pure connected-component resolver over the algo graph, wall-aware.
- New pitch-stack mod destination(s) on the matrix surface + wire/blob
  migration.
- Cook-time scatter of stack-pitch value across the resolved component into
  per-op pitch, with re-cook wired to algo + ratio-mode dirty bits.
- Faceplate matrix UI exposing the new dest.
- ADR addendum + audibility/round-trip tests.

**Out:**

- Stack mod for any non-pitch dest (level, pan, etc.) — pitch is the whole
  point ([[vxn2-architecture]]).
- Depth-scaled / weighted propagation.
- Any change to the algorithm graph table itself.

## Planned tickets

> Scaffolded 2026-06-18.

- [x] [0067](../../tickets/closed/0067-vxn2-stack-pitch-component-resolver.md) —
      Pure wall-aware connected-component resolver over the algo graph (dsp).
- [x] [0068](../../tickets/closed/0068-vxn2-stack-pitch-mod-dest.md) — Op-N
      stack-pitch mod destination(s) + wire/blob migration.
- [x] [0069](../../tickets/closed/0069-vxn2-stack-pitch-cook-scatter.md) —
      Cook-time scatter + re-cook on algo / ratio-mode dirty bits.
- [x] [0070](../../tickets/closed/0070-vxn2-stack-pitch-matrix-ui.md) — Expose
      the stack-pitch dest in the faceplate mod matrix.
- [x] [0071](../../tickets/closed/0071-vxn2-stack-pitch-adr-tests.md) — ADR
      addendum + audibility / ratio-lock tests.
