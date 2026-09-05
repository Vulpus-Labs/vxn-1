# ADR 0007 — VXN3 continuous lane editor: hits as free points, groove as grid geometry

- **Status:** Accepted
- **Date:** 2026-09-04
- **Scope:** The pattern editing surface and the storage of hit position and
  per-hit modulation. Supersedes [ADR 0006](0006-vxn3-groove.md); retains 0006's
  pooling, assignment and determinism rules. Consumes the RT model specified in
  [ADR 0004](0004-vxn3-micro-timing.md) §3 and finally requires it to be built.
  Routes through the macro binding model of [ADR 0005](0005-vxn3-voice-families-flavours-macros.md).

## Context

ADR 0006 made timing feel a **pooled groove template** — a per-grid-position
table of timing offsets and velocity deltas, scaled by amount knobs, assigned per
track. It kept the pattern editor a pure step editor and removed per-hit timing
outright, closing with: *"If a 'drag one hit' gesture is ever wanted it is a
future ADR; the groove template is the surface now."*

This is that ADR, and it goes further than one gesture: the step grid itself is
the wrong primitive.

### What is actually shipped

Neither 0004's nor 0006's timing model exists in code. Worth stating plainly,
because both ADRs read as though a scheduler with offsets is in place:

- [`Pattern`](../crates/vxn3-engine/src/sequencer.rs#L151) is `[Step; 16]` plus
  `len`, `step_beats` and a `(step, param)` lock table. Position is an **array
  index**. There is no offset field on `Step`, and no `Groove` type anywhere in
  `vxn3-engine`.
- [`LaneState::schedule`](../crates/vxn3-engine/src/lane.rs#L174-L204) walks step
  boundaries that fall inside the current block — `let first = (beat0 / sb).ceil()`
  — evaluates each, and fires at that boundary. There is no continuous timeline
  and no lookahead window. The one thing that *does* schedule off-boundary,
  [`emit_retrig`](../crates/vxn3-engine/src/lane.rs#L211), carries a single
  in-flight window across blocks by hand.

ADR 0004 §Consequences named this: *"the pattern-engine scheduler is a
continuous-timeline lookahead loop from the start, not a per-step 'fire on this
tick' loop. This is the load-bearing implementation constraint; retrofitting
lookahead later is a rewrite."* It was not built from the start. The bill is due
now, and it is due for 0006's design just as much as for this one — no timing
feel of any kind can ship on a boundary-walking scheduler.

That makes this the cheapest moment to revisit the model, because the expensive
half is unbuilt either way.

### Why the step grid is the wrong primitive

0006's groove is an **offset applied to a position**. Two consequences fall out
of that shape, and both are wrong for the genre:

1. **A hand-placed hit and a groove offset compete for the same quantity.** If
   dragging a hit writes an offset, swapping the groove overwrites the drag or
   sums with it; either way the hand-placed hit moves somewhere the user did not
   put it. 0006 avoided this by forbidding the drag, which is a real cost —
   deliberate off-grid placement is core to the idiom, not decoration.
2. **The offset table is indexed by a grid the user cannot see or touch.** Swing
   becomes a number rather than a thing on screen, and anything that is not
   expressible as "position *k* moves by *x*" — an unevenly felt bar, a beat
   pushed as a whole — is inexpressible.

### What the design gets for free

Making the lane a continuous plane hands over a second axis with nothing to spend
on it. vxn-3 already has somewhere to send it: ADR 0005's macro bindings, live in
[`flavour.rs`](../crates/vxn3-engine/src/flavour.rs) as
`final(p) = clamp(base[p] + Σ curve(macro[slot]) · depth, range(p))`, resolved
per trig and allocation-free.

And `MACRO_SLOTS` is **3**, while RGB is three channels. A hit's colour can *be*
its macro vector, exactly, with no new routing layer and no widening of ADR 0005's
deliberately small matrix.

## Decision

A lane is a continuous strip. Hits are freely-positioned points in it. The grid
is **drawn, not stored into** — it is geometry the hit positions are expressed
against, and it is itself editable.

### 1. The lane strip

A rectangular strip per track. **X is time. Y is a modulation value.** A hit is a
draggable diamond at some (x, y). The strip is subdivided by markers (§2); snap
is a toggle, and already-placed hits can be quantised to the nearest marker after
the fact.

Snap and quantise are **editor verbs**, not storage constraints. A quantised hit
is one whose stored fraction happens to be zero; nothing in the data model knows
the difference.

### 2. Two tiers of marker

**Beat markers** are stored and user-draggable. **Subdivision markers** are
derived and never stored. Sub marker `k` of `n` within beat `b`:

```text
sub_pos(b, k) = m[b] + w(k / n) · (m[b+1] - m[b])
```

where `m` is the beat marker array and `w` the swing warp (§3). Beat marker `k=0`
*is* a subdivision marker, so the snap target set is simply every sub marker.

- **Sub-count `n` is per-lane, with a per-beat override.** This is where tuplets
  live: one beat with `n = 3` inside an otherwise-16ths lane, no special case and
  no new concept.
- **Markers cannot cross.** A drag of `m[i]` clamps to
  `(m[i-1] + MIN_SLOT, m[i+1] - MIN_SLOT)`. `MIN_SLOT > 0` is mandatory rather
  than cosmetic: a zero-width slot makes the fraction of §4 unresolvable and the
  inverse mapping divide by ~0.
- **The outer markers are pinned** to the pattern's start and end. Otherwise a
  hit before `m[0]` has no owning slot.
- **Markers are per-lane**, which is what preserves ADR 0001 §2 polymeter, with
  an opt-in lock-together (§8).

### 3. Swing is a warp on the beat's unit interval

> **Amended 2026-09-04 — see [Amendment](#amendment-2026-09-04--the-warp-applies-per-pair-not-per-beat).**
> The interval `w` is applied *to* is the pair, not the beat. `w` itself is
> unchanged; the claim below that classic MPC swing is one `w` holds only at
> `n = 2`.

Swing is a monotonic `w: [0,1] → [0,1]` with `w(0) = 0`, `w(1) = 1`, applied
inside each beat.

This shape rather than 0006's per-position offset table, for one reason:
**it generalises over subdivision count**. Classic MPC swing (a piecewise-linear
pull on the odd subdivisions) is one `w`; a smooth warp is another; `n = 3` uses
the same `w` and simply has fewer points to place. One control stays meaningful
whatever the lane's sub-count, and sub markers stay purely derived — so they
cannot drift out of step with the beat markers that generate them.

An offset table cannot express a marker the user drags, which is the other half
of why 0006's form does not survive.

### 4. Hit position is relative, in two parts

```rust
struct Hit { beat: u16, sub: u8, f: f32, nudge: i16, y: f32, rgb: [f32; 3], /* trig attrs */ }
```

```text
t = sub_pos(beat, sub) + f · (sub_pos_next - sub_pos) + nudge
```

`f ∈ [0, 1)` is proportional; `nudge` is absolute, in ticks. Both are needed, and
the split is the point:

- `f = 0` **welds a hit to its subdivision marker.** Any swing change, any beat
  marker drag, moves the marker and the hit together, exactly. A snapped pattern
  stays snapped through every groove edit, with no re-quantise pass.
- `f > 0` **scales with its slot.** A hit 40% into a slot gets later as swing
  lengthens that slot. Correct for feel.
- `nudge` **does not scale.** A deliberate 3 ms flam, or the output of a humanise
  pass, is an absolute quantity that must survive a swing change unscaled. It is
  clamped (§9) so it cannot reorder hits.

### 5. Marker drag preserves relative; insert and delete preserve absolute

Dragging `m[i]` stretches slot `i-1` and squashes slot `i` at once — hits
rubber-band **in both directions** from a single grab, since all of them are
stored as fractions of a slot whose bounds just moved.

Inserting or deleting a beat marker does the opposite: each affected hit's
`(beat, sub, f)` is **recomputed from its current absolute time**, so splitting or
merging a slot moves nothing.

Both match what a user expects of the respective gesture, but they are opposite
rules. The asymmetry is deliberate and must be explicit in the code, not
emergent.

### 6. Y is absolute in the lane; the groove supplies an interpolated centre curve

The groove carries **Y-centre control points on the beat markers**, interpolated
across the region between them (Catmull-Rom, tangents clamped so a steep pair
cannot overshoot outside the lane) and sampled at the hit's absolute time.

Interpolated, not per-slot, and this is load-bearing. A per-slot step function
would mean a hit dragged horizontally across a marker **jumps vertically** — the
user moves X and Y changes. With a continuous curve there is no discontinuity to
cross, which is what makes it safe to render diamonds *relative* to the curve:
dragging sets offset-from-curve, and editing the curve sweeps the whole lane's
contour as one gesture.

Y is therefore **not** stored symmetrically with X. X is fully relative to a
discrete grid; Y is an offset from a continuous curve. Symmetry here would buy
elegance and cost direct manipulation.

### 7. A hit's colour is its macro vector

Each hit carries `rgb: [f32; 3]`, and the three channels drive ADR 0005's three
macro slots for that track, resolved at trig time through the flavour binding
table. No new routing mechanism: this is a per-hit override of the macro values
that [`flavour::resolve`](../crates/vxn3-engine/src/flavour.rs) already consumes,
sitting alongside the p-lock overrides of ADR 0001 §3a.

Y and `f` are further sources: **Y** is the lane's declared modulation
destination, and **`f`** — position within the subdivision slot — is lateness
against the *swung* grid, which is a more musical modulator than lateness against
a straight one and costs nothing, since it is already stored.

#### The palette selector

Three degrees of freedom do not fit in two dimensions. Any widget must add a
second control, a mode, or lose a channel. And these values are **modulation
sources, not decoration** — the user has to be able to hit `R = 1.0, G = 0,
B = 0.5` on purpose.

That rules out every hue-based widget — HSV wheel, Maxwell triangle, hue hexagon —
because moving one control changes two or three channels. Pretty, unpredictable,
rejected.

Shift-click blooms **three 120° arc sliders around the diamond**, each tinted its
channel, dragged independently, with numeric readouts normalised `0.00–1.00`
(what the matrix wants, not `0–255`). It is compact, in-context, occludes no
lane, and is exactly orthogonal. A three-bar numeric panel is the precise-entry
fallback.

#### Rendering rules, because colour carries data

- `R = G = B = 0` is a legitimate and useful modulation value, and an invisible
  diamond. **Display clamps to a minimum luminance and strokes with fixed
  contrast; the raw value still goes to the macros.** Render and value are
  decoupled.
- Red/green is the worst possible pair to make load-bearing. A redundant
  non-colour channel — notch rotation, or a three-segment ring on the diamond
  edge — is required, not optional.

### 8. Groove keeps 0006's pooling, with a new payload

A **groove** is beat marker positions, sub-counts, the swing warp, and the
Y-centre control points. It stays what 0006 made it: a **named, pooled,
swappable object**, assigned per lane, shared across lanes to lock them to a
common feel.

The pooling survives *better* under this model than under 0006's, because storage
is relative rather than additive: swapping a groove re-times every hit without
touching one of them, and cannot fight a hand-placed position.

**Lock-together covers beat markers only** — never sub-count, never swing. The
valuable case is a shared beat grid with lane A in swung 16ths and lane B in
straight triplets; locking those would destroy it. Two things need stating
outright: on lock, one lane's positions must win **by explicit choice** ("adopt
from lane N"), not implicit first-lane-wins; on unlock, lanes keep their current
positions rather than springing back.

0006's determinism rule is retained unchanged: any randomised humanise derives
from a per-hit deterministic hash of position, never `Math.random`, so loops and
resume reproduce.

### 9. RT model — 0004 §3's lookahead loop, now actually built

The scheduler becomes the continuous-timeline lookahead loop ADR 0004 specified
and [lane.rs](../crates/vxn3-engine/src/lane.rs) does not implement.

- **Monotonic fire order** is what bounds the window, and it comes free from
  `f ∈ [0, 1)`: a hit cannot leave its own slot. `nudge` is the only term that
  can reorder, so it is clamped to ±½ `MIN_SLOT`, preserving 0004's
  no-reordering invariant by the same argument and for the same reason.
- The window is therefore **const-sized**, preallocated, alloc-free in `process`,
  as 0004 §3 requires. Per-lane hit storage is a fixed-capacity array — `MAX_HITS`
  replaces `MAX_STEPS` as the ceiling — and over-capacity drops a hit rather than
  allocating, matching
  [`push_hit`](../crates/vxn3-engine/src/lane.rs#L251)'s existing discipline.
- **Loop wrap, retrig interaction and p-lock interaction are unchanged** from
  0004 §3 / 0006 §4: retrig offsets its window origin to the hit's actual fire
  time; a p-lock resolves on the tick the hit actually fires.
- **`Termination::Revert { n }` counts subdivision slots**, which is what "lane
  tick" now means. With per-beat sub-counts the slot duration varies, so a revert
  hold is a count of grid positions, not a duration — the same semantics
  [`process_locks`](../crates/vxn3-engine/src/lane.rs#L94) has today, now stated
  because the grid is no longer uniform.

## Consequences

- **Pattern storage changes shape**, from an indexed `[Step; 16]` to a hit list
  plus marker geometry, and **the format simply breaks**. vxn-3 is experimental
  with no user base, so the per-track patch blob (ADR 0005 / ticket 0179) is
  redefined rather than migrated — no conversion path, no dual-read. The version
  tag is kept for one purpose only: a stale blob on a developer's own disk must
  be **rejected**, never misparsed into the audio engine.
- **The scheduler rewrite lands first and alone.** It is behaviour-preserving on
  today's straight grid, which makes it independently verifiable against the
  existing lane and groove tests before any editing surface exists.
- **0006's offset/velocity slot table is gone.** Its velocity contour is
  subsumed: the Y-centre curve is the same idea, generalised from "velocity" to
  "whatever this lane's Y is routed to", and its emphasis role is served by
  pointing Y at gain. The two-layer velocity rule of 0006 §Consequences —
  compositional accent from the trig, feel contour from the groove — survives
  intact under that reading.
- **The pattern editor is now a timing surface**, reversing 0006's explicit
  split. What 0006 was protecting against — a fiddly numeric nudge strip
  conflating composition and feel — is not what this is: position *is* the
  gesture, and feel remains a separately-swappable object.
- **ADR 0005's macro matrix stays small.** Per-hit RGB is new *values* flowing
  into the existing three slots, not new destinations or a wider matrix. ADR
  0005's "expands only if play demands it" is not spent here.
- **Colour is data, so accessibility is a correctness requirement**, not polish.
  Any render path that drops the redundant non-colour channel makes the lane
  unreadable for a red/green-deficient user.
- A future ADR could let Y be per-lane routable to any family param rather than a
  fixed destination; nothing here forecloses it, and §7 already treats Y as
  "the lane's declared destination".

## Alternatives considered

**Keep 0006's offset table and add drag as an editor gesture that writes into
it.** The smallest change, and it was the first shape tried. Rejected on §Context
point 1: a dragged hit and a groove offset then occupy the same field, so
swapping the groove either overwrites hand placement or sums with it. Relative
storage is what dissolves the conflict, and relative storage is not expressible
as an offset table.

**Store absolute time per hit.** Simplest possible model and the obvious default.
Rejected because it makes the central feature impossible: marker drag and groove
swap can then re-time nothing, and a groove degenerates back into a set of
numbers with no relationship to what is on screen.

**Store Y relative to its slot, symmetric with X.** Elegant, and wrong — §6: it
makes horizontal drags across a marker jump vertically.

**A hue-based palette (HSV wheel, Maxwell triangle, corner-on RGB cube).** All
look better in a mockup. All rejected on §7: one control moves multiple channels,
so a user cannot address a macro slot deliberately.

**A fourth colour channel (alpha) for Y instead of an axis.** Rejected: alpha is
unreadable against a subdivided strip, and Y already has a whole axis.

**Global markers shared by all lanes, with no per-lane option.** Rejected — it
contradicts ADR 0001 §2's independent lanes and forecloses the phasing the genre
leans on. Sharing is available as a lock (§8), which is the same benefit without
the constraint.

## Amendment (2026-09-04) — the warp applies per pair, not per beat

§3 chose the warp form over an offset table because it "generalises over
subdivision count", and glossed classic MPC swing as *"a piecewise-linear pull on
the odd subdivisions … is one `w`"*. The first claim stands. The parenthetical is
wrong for every `n` but 2, and ticket 0347 shipped the literal reading of §3 —
`w` applied across the whole beat — so the code and the intent parted at `n = 4`.

Measured on the shipped `Grid`, one beat at full swing:

| `n` | shipped gaps (beat interval) | classic shuffle |
|----:|------------------------------|-----------------|
| 2   | `0.75, 0.25`                 | `0.75, 0.25` — exact |
| 4   | `0.375, 0.375, 0.125, 0.125` | `0.375, 0.125, 0.375, 0.125` |
| 3   | `0.5, 0.333, 0.167`          | n/a |

Shuffle is **long-short-long-short**. A beat-wide warp gives
**long-long-short-short** — not a milder shuffle but a different rhythm, the beat
back-loaded. The 16ths land at `0, 0.375, 0.75, 0.875`: subs 1 and 3 are exactly
where shuffle wants them, and it is sub 2 — the on-beat 8th — that is dragged to
0.75 instead of holding at 0.5. Classic swing pulls only the odd subdivisions; a
single knee pulls everything past the midpoint, the 8th included.

This is structural, not a tuning error. Shuffle delays the second of each
**pair**, and a pair is `2/n` of a beat, so pulling only the odd subs needs `n/2`
knees. A two-knee `w` expresses 16th shuffle exactly — and that same `w` at
`n = 2` gives `w(0.5) = 0.5`, i.e. no swing at all. **Any fixed `w(u)` is classic
swing for exactly one `n`.**

The fix keeps `w` exactly as §3 defines it — monotonic on `[0,1]`, fixed
endpoints, no `n` argument — and changes only the interval it is applied to:

```text
p = k / 2            (pair index)
j = k % 2            (position within the pair)
sub_pos(b, k) = m[b] + (p + w(j / 2)) · (2 / n_b) · (m[b+1] - m[b])
```

`n_b` enters only as the pair width, which the grid already holds. Sub markers
stay derived, `w` stays sub-count-agnostic, monotonicity and fixed endpoints are
untouched — every invariant §3 was protecting survives. At `n = 2` it collapses
to the current behaviour, which is the case already correct.

Odd `n` does not pair evenly and needs its own answer: fall back to the beat
interval, or make the swing period an explicit control (beat / pair / custom).
The latter is the better shape — it also buys 8th-note swing on a 16ths lane as a
first-class option rather than an accident — but it widens the swing control that
ticket 0354 has to build, so it is a deliberate choice rather than a detail.
Ticket 0365 carries the change; it must land before 0354, since what the swing
control reads out depends on which interval it drives.
