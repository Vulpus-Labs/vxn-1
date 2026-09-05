---
id: "0351"
product: vxn-3
title: "Per-hit RGB drives the three macro slots; f exposed as a lateness source"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0348"]
---

## Summary

Implements [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §7's
engine half. A hit's `rgb: [f32; 3]` (stored in 0348) drives the track's three
macro slots, resolved at trig time through the existing flavour binding table in
[`flavour.rs`](../../vxn-3/crates/vxn3-engine/src/flavour.rs).

The fit is exact and is why the design landed this way:
[`MACRO_SLOTS`](../../vxn-3/crates/vxn3-engine/src/track_engine.rs) is **3** and
RGB is three channels, so a hit's colour *is* its macro vector. No new routing
mechanism, no widening of [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md)'s
deliberately small matrix — these are new *values* flowing into existing slots.

Also exposes `f` (position within the subdivision slot) as a modulation source:
lateness against the **swung** grid, which is more musical than lateness against
a straight one and is already stored, so it costs nothing.

## Design

Per-hit RGB is an **override of macro values at trig time**, structurally
alongside the p-lock overrides of ADR 0001 §3a — the same shape as
[`LaneState::override_value`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L86),
feeding
`final(p) = clamp(base[p] + Σ curve(macro[slot]) · depth, range(p))` unchanged.

Precedence has to be decided and stated: a per-hit colour and a p-lock on the
same macro slot both want to win. Proposal — **per-hit RGB wins**, because it is
attached to the hit being fired and cannot be an accident of a hold left running
from an earlier position; a latched p-lock on a macro slot then applies only to
hits that carry no colour. Document whichever way it lands, on the resolver.

Macro **values** remain host params (ADR 0003) and automatable; the per-hit
override applies on top for the duration of that trig's resolve and does not
write back to host state.

`f` becomes an addressable source in the same resolve path.

## Acceptance criteria

- [ ] A hit's `rgb` channels drive macro slots 0/1/2 at trig resolve, through the
      unchanged flavour binding table.
- [ ] Values are normalised `0.00–1.00` end to end — no `0–255` representation
      anywhere in the value path.
- [ ] Resolve stays allocation-free and per-trig (not per-sample);
      `tests/groove.rs`'s allocation trap stays green.
- [ ] Precedence between per-hit RGB and a p-lock on the same macro slot is
      implemented, tested both ways round, and documented on the resolver.
- [ ] A per-hit override does not write back to host macro param state — after
      the trig, `get_value` reports the automated value unchanged.
- [ ] `rgb = [0, 0, 0]` sends zero to all three slots (and stays visible on the
      faceplate — 0355 owns the render side).
- [ ] `f` is exposed as a modulation source and reflects position within the
      **swung** slot, verified by asserting it changes with swing amount for a
      fixed hit.
- [ ] `MACRO_SLOTS` is unchanged at 3 and no new destinations are added.

## Notes

Depends only on 0348 (which stores `rgb`), not on the marker or curve tickets —
can run in parallel with 0349/0350.

The palette widget, colour render rules and accessibility redundancy are 0355.
This ticket is the value path only; nothing here is user-visible without it.

ADR 0005's open question — whether macro values belong to the flavour or to
performance state — is not settled here. Per-hit override is a third layer above
both and is compatible with either answer.
