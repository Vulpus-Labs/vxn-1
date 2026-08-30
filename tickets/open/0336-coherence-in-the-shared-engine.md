---
id: "0336"
product: monorepo
title: "Coherence predicate in the shared engine; vxn-1b gains the surface for free"
priority: low
created: 2026-08-30
epic: E049
depends: ["0332"]
---

## Summary

Move vxn-2's tier-coherence predicate into `vxn-core-matrix`, driven by the
`tier` column declared in [0332](0332-roster-row-declares-everything.md).

A routing is **coherent** iff the source's tier is coarser-or-equal to the
dest's: a coarser source broadcasts unambiguously to a finer dest, while a finer
source into a coarser dest is a lossy collapse to lane 0. Plus vxn-2's two
special cases — an LFO into its own rate (`SelfRate`), and `voice-idx` into a
lane-0-collapsed dest, which is a constant zero (`Degenerate`).

vxn-2 exports the verdict table in its matrix descriptor so the faceplate flags
incoherent rows without re-deriving the rule. vxn-1b has no such surface.

## Design

vxn-1b's rosters make every verdict `Ok` today — every destination is per-voice,
so no fine→coarse route is expressible. That is not a reason to skip it: it is
the **degenerate case of the same model**, and the machinery costs vxn-1b
nothing but is live the moment a global destination appears. vxn-1b has no
global FX dests *yet*; `delay-mix` or `reverb-mix` would be exactly the addition
that makes the first incoherent route possible, and the failure mode without
this is silent — a per-voice source driving a global effect, collapsed to
whichever voice happens to be lane 0.

Worth confirming as part of this ticket rather than assuming: walk vxn-1b's
whole source × dest space and assert every verdict is `Ok`. If any isn't, the
"vxn-1b is flat" premise in [ADR 0003](../../adrs/0003-vxn-core-matrix.md) is
wrong and that is worth knowing.

## Acceptance criteria

- [ ] `coherence(src, dst)` in `vxn-core-matrix`, generic over the roster,
      driven by the declared tiers.
- [ ] vxn-2's verdicts are unchanged for every source × dest pair — assert the
      full table against the pre-ticket one.
- [ ] vxn-1b's full source × dest space is asserted all-`Ok`, with the test
      naming the assumption so a future global dest fails it loudly.
- [ ] vxn-2's descriptor export and faceplate warnings behave exactly as before.
- [ ] vxn-1b's matrix descriptor exports the same verdict table. Surfacing it in
      the faceplate is optional here — the export is the deliverable, the UI can
      follow.

## Notes

- `priority: low` — no user-visible change for either synth today. It is here so
  the tier column has a consumer and vxn-1b is not left with a declared-but-dead
  property.
- Out of scope: adding global destinations to vxn-1b. This ticket makes that
  safe to do later; it does not do it.
