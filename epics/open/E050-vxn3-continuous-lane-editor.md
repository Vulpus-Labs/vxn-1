---
id: E050
product: vxn-3
title: "vxn-3 continuous lane editor — free-positioned hits, marker grooves, colour macros"
status: open
created: 2026-09-04
---

> **Design is fixed in [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md)**,
> which supersedes ADR 0006 and finally builds the RT model ADR 0004 §3
> specified. This epic replaces vxn-3's indexed step grid with a continuous lane
> strip: hits are freely-positioned points, the grid is editable geometry, and a
> hit's colour *is* its macro vector.

## Goal

Make the pattern surface a **plane, not a grid**. Each lane is a rectangular
strip where X is time and Y is a modulation value; hits are draggable diamonds;
the beat markers subdividing the strip are themselves draggable, and dragging one
rubber-bands every hit in the two adjacent slots. Swing warps the derived
subdivision markers. A groove is that geometry — marker positions, sub-counts,
swing warp, Y-centre curve — pooled and swappable per ADR 0006's model, but with
hit positions stored *relative* to it, so swapping a groove re-times a pattern
without editing a single hit.

A hit's RGB colour drives ADR 0005's three macro slots. Between position,
height, colour and lateness-within-slot, one diamond carries five continuous
modulation values and reads as a single object.

When this epic closes:

- Lanes render as continuous strips with diamond hits, draggable in X and Y,
  with snap and after-the-fact quantise to subdivision markers.
- Beat markers drag, with two-sided rubber-band on the hits either side; swing
  redistributes the derived subdivision markers.
- Grooves are pooled, named, assigned per lane, with opt-in lock-together on
  beat markers only.
- Shift-click on a diamond opens a three-arc palette; the channels drive the
  track's three macro slots per hit.
- The scheduler is a continuous-timeline lookahead loop, allocation-free, with
  every existing lane/groove/pattern test still green.

## Why now

Two things make this the cheapest possible moment, both recorded in ADR 0007
§Context:

1. **The expensive half is unbuilt either way.** ADR 0004 §3 required a
   continuous-timeline lookahead scheduler and called retrofitting it "a
   rewrite". It was never built —
   [`LaneState::schedule`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L174-L204)
   still walks step boundaries inside the block. ADR 0006's design needs that
   rewrite exactly as much as ADR 0007's does, so choosing between the two
   designs costs nothing extra.
2. **`MACRO_SLOTS` is 3 and RGB is 3.** ADR 0005's macro binding table in
   [`flavour.rs`](../../vxn-3/crates/vxn3-engine/src/flavour.rs) already resolves
   per trig and allocation-free. Per-hit colour needs no new routing layer and
   does not widen ADR 0005's deliberately small matrix.

Nothing in this epic touches the engines or the SoA kernels.

## Scope

**In:**

- Continuous-timeline lookahead scheduler, behaviour-preserving on a straight
  grid (ADR 0007 §9).
- Marker geometry: stored beat markers, derived subdivision markers, swing as a
  warp on the beat's unit interval, per-beat sub-count override for tuplets
  (§2, §3).
- `Pattern` as a hit list — `Hit { beat, sub, f, nudge, y, rgb }` — replacing the
  indexed grid outright (§4).
- Marker edit semantics: drag preserves relative, insert/delete preserves
  absolute, min-slot clamp, pinned outer markers (§5).
- Y-centre curve: control points on beat markers, Catmull-Rom with clamped
  tangents, sampled at fire time (§6).
- Per-hit RGB → the three macro slots, resolved through the flavour binding
  table at trig; `f` exposed as a lateness source (§7).
- Groove object + pool + per-lane assignment + lock-together on beat markers
  (§8).
- Faceplate: lane strip, diamond drag, snap/quantise, marker drag with
  rubber-band feedback, swing control, three-arc palette, Y-centre curve
  editing, groove pool UI.

**Out (deferred):**

- Y as a per-lane *routable* destination (fixed destination for now; ADR 0007
  §Consequences flags the future ADR).
- Widening ADR 0005's macro matrix beyond three slots.
- Humanise / randomised timing generators (the determinism rule is carried
  forward, but no generator ships here).
- Groove extraction from an existing pattern, and groove import.
- Arrangement, kits, presets — untouched.

## Planned tickets

- [ ] 0346 — Continuous-timeline lookahead scheduler (behaviour-preserving).
- [ ] 0347 — Marker geometry: beat markers, derived sub markers, swing warp.
- [ ] 0348 — `Pattern` as a hit list, replacing the indexed grid.
- [ ] 0349 — Marker edit semantics: relative drag, absolute insert/delete.
- [ ] 0350 — Y-centre interpolated curve.
- [ ] 0351 — Per-hit RGB → macro slots; `f` as a lateness source.
- [ ] 0352 — Groove object, pool, per-lane assignment, lock-together.
- [ ] 0353 — Faceplate: lane strip, diamond hits, drag, snap, quantise.
- [ ] 0354 — Faceplate: marker drag, swing control, rubber-band feedback.
- [ ] 0355 — Faceplate: three-arc palette + colour render rules.
- [ ] 0356 — Faceplate: Y-centre curve editing + groove pool UI.
- [ ] 0365 — Swing warp applies per pair, not per beat (corrective; amends ADR
      0007 §3).

0346 and 0347 are independent and can run in parallel; 0348 gates everything
else. 0365 is corrective, opened after 0347 landed, and must precede 0354.

## Risks

- **The scheduler rewrite is the load-bearing step.** 0004 called it a rewrite
  and it is: in-flight retrig state, transport-jump resync and the p-lock
  resolver all currently key off the boundary walk. Mitigated by landing 0346
  alone, behaviour-preserving, verified against the existing tests before any
  new data model exists.
- **Monotonic fire order** is the invariant the bounded window rests on. It comes
  free from `f ∈ [0, 1)`, but `nudge` can break it if the clamp is wrong — and a
  broken clamp shows up as rare out-of-order hits, not a test failure. Needs a
  property test, not an example test.
- **Degenerate slots.** `MIN_SLOT > 0` guards a divide-by-~0 in the inverse
  mapping. Every marker mutation path must go through the clamp; a direct write
  that bypasses it is a silent NaN generator.
- **The patch blob format breaks outright** — vxn-3 is experimental with no user
  base, so this is a licence, not a risk. The only requirement that survives:
  a stale blob must be rejected, never misparsed.
- **Colour as data.** Dropping the redundant non-colour channel, or letting the
  luminance floor leak into the value path, are both correctness bugs and both
  look like cosmetic details in review.
- **Editor scope creep.** The faceplate half (0353–0356) is four tickets of
  direct-manipulation UI, historically where this repo's estimates slip.

## Acceptance

- A lane plays hits placed anywhere in the strip, sample-accurately, with the
  process callback allocation-free and no out-of-order hits under a property
  test over random placements and nudges.
- Dragging a beat marker visibly moves hits in **both** adjacent slots, and hits
  at `f = 0` stay exactly on their subdivision markers through any swing change.
- Inserting or deleting a beat marker moves no hit.
- Swapping a lane's groove re-times its pattern without any hit being edited;
  sharing one groove across lanes locks their feel; lock-together leaves
  per-lane sub-count and swing independent.
- Shift-clicking a diamond opens the three-arc palette; each arc moves exactly
  one macro slot; a hit at `rgb = [0,0,0]` is still visible and still sends zero.
- Editing the Y-centre curve sweeps a lane's contour; dragging a hit across a
  beat marker does not move it vertically.
- 0346 lands alone and behaviour-preserving: with the scheduler rewritten and the
  data model untouched, the E021 demo pattern renders bit-identically to the
  pre-ticket build and every existing test passes unedited.
