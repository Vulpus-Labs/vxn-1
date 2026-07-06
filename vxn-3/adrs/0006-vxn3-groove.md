# ADR 0006 — VXN3 groove: timing feel as a pooled template

- **Status:** Accepted
- **Date:** 2026-07-06
- **Scope:** Where rhythmic *feel* (swing, timing deviation, emphasis) lives in
  the VXN3 editor and data model. Supersedes [ADR 0004](0004-vxn3-micro-timing.md);
  refines [ADR 0001](0001-vxn3-overall-design.md) §2/§3a.

## Context

ADR 0004 put timing feel inside the pattern editor: a per-track **nudge strip**
(per-step offset lane) plus a per-lane shift knob, with per-trig micro-timing
stored as a trig attribute (±50% of the step).

Two problems surfaced in the editor design:

1. **The pattern editor is a step editor.** Its job is defining *what* fires and
   attaching p-lock values to steps — trigs, gates, retrig, probability/condition,
   p-locks. Small per-hit timing offsets are a poor fit for that surface: a thin
   per-step numeric strip is fiddly to read and edit, and it conflates two
   orthogonal concerns (composition vs feel) on one grid.
2. **Feel is reusable; patterns are not.** Swing and humanised drift are a
   *style* you apply across tracks and patterns. Baking the offset onto each
   trig makes feel un-shareable and un-swappable — you re-edit every hit to
   change the groove.

Ableton's groove abstraction (Groove Pool: a groove is a per-grid-position
timing + velocity template with Timing / Velocity / Random / Quantize amount
knobs, pooled and assigned per clip) solves both: feel is a first-class,
reusable object, edited apart from the notes.

## Decision

Pattern and groove are **orthogonal editors**.

- **Pattern editor** — pure step editor. Trigs, gate, retrig n/m,
  probability/condition, per-step **velocity/accent**, and p-locks (ADR 0001
  §3a). No timing surface, no nudge strip.
- **Groove editor** — a separate surface owning timing feel and feel-based
  emphasis, via a **groove** object.

### 1. The groove object

A groove is a reusable template, indexed by position within a **base grid**:

```text
Groove {
  base,                       // grid the template is defined over, e.g. 1/16
  slots: [ { timing, vel } ], // per-position offset + emphasis delta, length = base cycle
  amount: { timing, vel, random },  // scaling knobs, 0..1 (vel may be signed)
}
```

- **`timing`** per slot — signed fraction of the base-grid step, the per-hit
  offset. Scaled by `amount.timing`.
- **`vel`** per slot — signed emphasis delta applied to the trig's velocity.
  Scaled by `amount.vel`. This is the groove's *feel* contour — **distinct from
  the trig's own velocity/accent** (ADR 0001 §3a), which is compositional and
  stays in the pattern editor. Final velocity = trig accent, then groove `vel`
  on top.
- **`random`** — per-hit random timing humanise, magnitude scaled by
  `amount.random`. (Randomness derives from a per-trig deterministic hash of
  position, not `Math.random` — reproducible across loops and resume.)
- **Swing is a parametric groove**, not a special case: the even/odd offset
  pattern is one built-in groove (or a one-knob generator that fills `slots`).
  No separate swing code path.

The per-trig micro-timing field of ADR 0004 §1 is **removed** — all timing feel
comes from the assigned groove. There is no per-hit manual nudge on the trig.
(If a "drag one hit" gesture is ever wanted it is a future ADR; the groove
template is the surface now.)

### 2. Assignment: pooled, per track

Grooves live in a **pool** — named, reusable objects. Each **track** points at
one groove (or none = straight grid). Swapping a track's groove restyles its
feel without touching its pattern; sharing one groove across tracks locks them
to a common feel. Mirrors Ableton's Groove Pool.

- **Base-grid vs lane grid.** A track's groove `base` need not equal the lane's
  polymetric step spacing (ADR 0001 §2). The groove slot index is
  `(fire position on the groove's base grid) mod base-cycle`; the offset it
  yields is still expressed as a fraction of the *lane* step for the fire math
  below. This keeps groove templates portable across tracks of different
  divisors.

### 3. Per-lane shift stays — as a track/groove-timing property

The per-lane constant shift (ADR 0004 §2) survives unchanged in semantics but
moves out of the pattern editor into track timing (alongside groove
assignment). Uniform per-lane, may exceed a step, p-lockable for slow phase
drift over the loop. It is *not* part of the groove template (a groove is
feel; lane shift is gross phase), but both feed the same fire math.

### 4. RT model — unchanged from ADR 0004 §3

The scheduler is identical; only the *source* of the per-hit offset changes.
Effective fire tick on the lane's continuous timeline:

```text
fire = step·spacing + laneShift + groove.timing[pos]·amount.timing + grooveRandom
```

- Continuous-timeline **lookahead** loop (not per-step "fire now"), bounded
  window ≥ max negative offset. Groove `timing` slots are clamped to ±50% of the
  step (as ADR 0004 §1 clamped per-trig micro-timing) → the window stays
  const-sized, preallocated, alloc-free in `process`.
- **Loop wrap**, **retrig interaction** (groove offsets the retrig window
  origin), **p-lock interaction** (a p-lock resolves on the tick the trig
  actually fires; ramp tick base unchanged) — all exactly as ADR 0004 §3.

The load-bearing constraint from ADR 0004 stands: the pattern-engine scheduler
is a continuous-timeline lookahead loop from the start.

## Consequences

- **Trig storage shrinks** — the micro-timing field is gone. Trig keeps
  velocity/accent (compositional). Timing feel is a per-track pointer into the
  groove pool + the pool itself.
- **Two editors, one fire path.** Pattern editor and groove editor are separate
  UI surfaces; they converge only at the fire-tick math (§4), which is
  unchanged. No nudge strip in the pattern grid.
- **Feel is a first-class, swappable object.** Restyle a track's groove or share
  one across tracks without editing patterns. Grooves are pool state
  (host/project state), not pattern data.
- **Velocity has two layers** — trig accent (pattern) + groove vel contour
  (feel), summed. Documented so neither editor silently owns the other's job.
- **Determinism preserved.** Groove random is hash-derived per position, so
  loops and resume reproduce (consistent with ADR 0001 §3a steady-state rules
  and the no-`Math.random` constraint).

## Alternatives considered

- **Keep per-trig micro-timing as a manual override alongside the groove**
  (Ableton keeps both note-nudge and groove). Rejected for v1: the pattern
  editor is explicitly not the surface for small timing, and a trig field we
  don't expose in that editor is dead weight. Revisit as a future ADR if a
  deliberate one-hit drag is wanted.
- **Groove per pattern** (one groove, all tracks). Rejected — can't phase tracks
  with different feels, which the genre leans on.
- **Groove baked inline per track** (not pooled). Rejected — loses sharing and
  swap, the whole point of the abstraction.
- **Groove owns all velocity** (no per-step accent in the pattern editor).
  Rejected — conflates compositional accent with feel; forces faking accents via
  p-locks.
