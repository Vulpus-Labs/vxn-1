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

## Close-out (2026-09-01)

- `coherence()` lives in
  [coherence.rs:152](../../crates/vxn-core-matrix/src/coherence.rs#L152), driven
  by the declared tiers. The shape — empty-slot short circuit, then special
  cases, then `!covers` — is written once in the shared crate, so the precedence
  cannot drift per synth.
- **Special cases are a per-synth hook, and vxn-1b supplies none** (decided this
  session): `CoherenceRoster` has `source_tier`/`dest_tier` returning
  `Option<Tier>` (where `None` *is* the sentinel) plus a `special_case` that
  defaults to `None`. Deliberately not keyed on `MatrixRoster`: that trait speaks
  storage indices with the sentinel excluded, while the verdict table is a UI
  descriptor addressed by wire discriminant *with* the sentinel at 0 — a
  faceplate's first pick-list entry is "—" and has to be answerable.
- `matrix_enum!`'s source form gained a mandatory `tier =` column, with a
  `compile_fail` doctest pinning the forcing function; vxn-2's hand-written
  `SourceId::tier` match is deleted — the last hand-kept parallel list in its
  roster.
- vxn-2's verdicts are asserted against a **frozen** 12 × 52 table captured from
  the pre-ticket build (`coherence_grid_matches_the_pre_0336_table`). The test it
  replaces re-derived the expectation from a copy of the rule, so a mistyped tier
  column would have moved both sides together and still passed.
- vxn-1b's whole 13 × 17 space, sentinels included, is asserted all-`Ok`
  ([matrix.rs:955](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L955)), with a
  companion test naming *why* (every routable endpoint declares `per_lane`) and
  one pinning that `lfo1 → lfo1-rate` stays `Ok` here — the verdict a future
  "share the special cases too" tidy-up would break.
- vxn-1b's descriptor exports the verdict table at top level
  ([lib.rs:325](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L325)). Surfacing it
  in that faceplate's UI stays optional, per the ticket.
- **The dead-wiring fix turned out to be a live regression, not a missing
  warning.** `bootstrap.js` never copied `coherence`, `shapes`, `polarities` or
  `curve_stride` into `window.__vxn.matrix`, so since the curve-axis split
  (`bbff167`) `buildSelect` threw on `undefined.length` and took the whole
  `bind()` with it: **vxn-2's mod-matrix overlay rendered no rows at all**, in
  plugin and web faceplates alike. Verified independently by generating both
  pages and driving them headless — pre-fix `48d54f8`: 0 `li.vxn-mm-row` nodes
  and `Uncaught TypeError: Cannot read properties of undefined (reading
  'length')`; post-fix: 16 rows, clean console.
- The Rust-side guard is now structural rather than a substring grep: it extracts
  every `window.__vxn.matrix.<key>` read from all 16 bundled JS assets and
  requires each to be assigned in bootstrap's literal *and* emitted by the
  descriptor. Mutation-checked — it fails on the pre-fix bootstrap.
- Headless DOM check, 18 assertions: `voice-idx → cutoff` flags the row
  `vxn-mm-invalid` with the collapsed-lane tooltip and `velocity → cutoff` clears
  it, both on load and through the edit path. Confirmed in Reaper by the user;
  one follow-up landed separately — the overlay needed right padding so the
  macOS overlay scrollbar stops sitting on the bin column.
- Null test `-inf dBFS` on both engines; no audio path touched.
- **Left open, deliberately:** vxn-1b's `mod-wheel`/`pitch-wheel` are
  patch-global scalars declaring `tier = per_lane`. Harmless while vxn-1b has no
  coarse destination, but the day it grows one, a coherent `mod-wheel →
  delay-mix` would score `TierCollapse`. Kept as declared because ADR 0003 states
  every vxn-1b endpoint is `PerLane` — roster and ADR have to move together.
  Recorded in a note on the source row block.
