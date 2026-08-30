---
id: "0329"
product: monorepo
title: "vxn-core-matrix: crate skeleton and the roster/mechanism seam"
priority: medium
created: 2026-08-30
epic: E049
depends: []
---

## Summary

First ticket of [E049](../../epics/open/E049-shared-matrix-routing.md). Creates
`crates/vxn-core-matrix` and defines the seam [ADR 0003](../../adrs/0003-vxn-core-matrix.md)
sets, with **no consumers yet** — nothing in vxn-1b or vxn-2 changes.

The seam is between the **roster** (what a synth can route) and the
**mechanism** (how a routing is evaluated). This ticket writes down the roster
side as a trait, so later tickets have something to be generic over.

## Design

```rust
pub trait MatrixRoster: Copy {
    const N_SOURCES: usize;
    const N_DESTS: usize;
    const N_SLOTS: usize;

    fn source_is_bipolar(src: u8) -> bool;
    fn dest_gain(dest: u8) -> f32;
    fn cook_depth(dest: u8, depth: f32) -> f32;
    fn dest_tier(dest: u8) -> Tier;
    fn source_tier(src: u8) -> Tier;
    fn dest_smoothing(dest: u8) -> Smoothing;

    fn source_names() -> &'static [&'static str];
    fn dest_names() -> &'static [&'static str];
    // …labels likewise
}
```

Opaque `u8` indices rather than associated enum types: the engine never needs to
know what `dest 7` *is*, and associated types would force every shared function
to carry two more generic parameters. The synth's enums stay its own and convert
at the boundary, exactly as they already do at the wire boundary.

`Tier` and `Smoothing` are shared enums defined here — see
[0332](0332-roster-row-declares-everything.md) for what fills them in and
[0335](0335-declared-target-smoothing.md) for what consumes `Smoothing`.

Whether the trait carries const arrays or functions is an implementation call
for whoever picks this up: const-generic array sizing (`[f32; R::N_DESTS]`)
needs `generic_const_exprs`, which is unstable, so the likely shape is a
fixed-capacity buffer sized by a `const MAX_DESTS` with the roster's real count
as a runtime bound. **Measure that** — vxn-2 has 51 dests and vxn-1b 16, and a
shared 64-wide buffer costs vxn-1b 3× the accumulator clears per block. If the
cost is real, the fallback is a macro that stamps out a monomorphic engine per
roster instead of a generic one, which is uglier but has no such tax.

## Acceptance criteria

- [ ] `crates/vxn-core-matrix` exists, in the workspace, `cargo check` clean.
- [ ] `MatrixRoster`, `Tier`, `Smoothing` defined and documented.
- [ ] A synthetic test roster (`TestRoster`: ~4 sources, ~4 dests, all gains
      1.0, no taper) lives behind `#[cfg(test)]` or a `testing` feature —
      [0331](0331-matrix-golden-vector-harness.md) builds on it.
- [ ] **Nothing outside the new crate changes.** No vxn-1b or vxn-2 file is
      touched by this ticket.
- [ ] The buffer-sizing decision above is recorded in the crate's module docs
      with whatever measurement backed it.

## Notes

- Follow `vxn-core-dsp`'s crate conventions ([ADR 0002](../../adrs/0002-vxn-core-dsp.md)):
  workspace-inherited version/edition/license, no synth deps.
- Deliberately inert. The value of a skeleton ticket is that the seam gets
  reviewed before anything is ported through it — if the trait is wrong, this is
  the cheap moment to find out.
