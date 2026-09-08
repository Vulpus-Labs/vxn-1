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

## Close-out (2026-09-07)

Shipped as three commits on `main`. The value path is complete inside
`vxn3-engine`; nothing here is user-visible (0355 owns the render side, and there
is still no `EngineCommand` for painting a hit — see *Not done*).

### What shipped

**A hit's colour is its macro vector.** `TrigEvent` and the scheduler's `Pending`
carry a `TrigMod { macros: Option<[f32; 3]>, lateness: f32 }`, resolved with the
fire time at all three hit → trig sites in `lane.rs` (plain hit, retrig
expansion, window emit). `Track::render_with_hits` hands it to the engine through
a new `TrackEngine::on_trig_with`, defaulted to fall through to `on_trig` so
every engine without a flavour runtime — and every test spy — is untouched. The
four families fold it into the vector they hand `flavour::resolve`.

**The binding table is unchanged.** `resolve` has always indexed its source slice
by `Binding::slot` and read a missing slot as zero, so it needed no edit at all:
`0..MACRO_SLOTS` are the host macros as before, and `SRC_LATENESS == 3` is the
new *source*. `MACRO_SLOTS` is still 3, pinned by a `const _: () = assert!`, and
no destination was added.

**Precedence — per-hit colour beats a p-lock — is documented on
`flavour::resolve`** with the reason (a colour is attached to the hit being
fired; a latched lock can be an accident of an earlier position), and tested both
ways round in one pass of one lane: a `Latch` on macro slot 0 that the coloured
hit ignores and the uncoloured hit under it obeys.

**Black is a colour, absence is not.** That distinction has to exist for
`rgb = [0,0,0]` to send zero to three slots *and* for "hits carrying no colour"
to mean anything, so `Hit::rgb` defaults to the out-of-band `NO_COLOUR = -1.0`
and `colour_override` decodes. This is the one change outside the ticket's own
files: two lines in `sequencer.rs`'s `Default for Hit`, plus `set_colour` /
`clear_colour`. Channels clamp into `0.00–1.00` on the way to the slots; there is
no `0–255` representation anywhere in the path.

**No write-back.** The override lives only in the vector passed to `resolve`. It
never reaches `set_macro`, `Track::base` or `Track::applied`, so the host echo
still reports the automated value after a coloured trig — asserted at both the
echo and every `set_macro` the engine saw.

**`f` as a lateness source** is the resolved fraction
`(fire_beat - sub_pos(beat, sub)) / slot_span`, which expands to
`f + clamp(nudge, ±½ span) / span`. The swing AC is tested by holding one hit at
a fixed *absolute* time and re-deriving its `(beat, sub, f)` through
`Grid::locate` per swing amount, so the markers move under a hit that has not
moved. The hit carries a nudge, which is what stops the assertion being
satisfiable by stored `f` alone.

**Allocation-free.** `TrigMod` is `Copy`; `Window` stays a fixed array (entries
grew 24 → 40 bytes, `LaneState` ~7.8 KB → ~12.9 KB, all heap-backed via the
engine's `Vec`s). The trap in `tests/pattern.rs` now drives eight real `KickTone`
lanes where *every* hit carries a different colour — so every trig re-resolves
and re-cooks — and reports zero allocations over 299 blocks. A paint-vs-no-paint
render pins that colour is audible.

### Changed after review

An adversarial review caught two things worth recording:

- The re-resolve gate was a plain `TrigMod` inequality, and `TrigMod` compares
  `lateness`. Lateness differs between any two hits with a different `f` or
  `nudge`, so every *humanised* lane — coloured or not — re-resolved and
  re-cooked its whole flavour on every trig, producing a bit-identical patch.
  `TrigMod::differs_for` now asks whether the change can reach a binding at all.
- `trig_mod`'s doc claimed the resolved fraction and stored `f` were different
  quantities. They differ by exactly the nudge term; the doc now says so, and the
  test was strengthened until it discriminates.

Also tightened: `macro_label` and `flavour_macro_display` no longer answer for
`SRC_LATENESS` as though a fourth macro slot existed, and the precedence doc no
longer implies layers 2 and 3 are per trig — they are per block, via
`Track::apply_effective`, exactly as before this ticket.

### Not done, deliberately

- **No command surface or persistence.** `EngineCommand` has no `SetColour`, and
  hits are not serialised at all (0348 did not add that either), so `set_colour`
  is reachable only in-crate. `io.rs` belongs to another ticket in this wave.
- **A repaint can miss a hit already in the lookahead window** and land from the
  next pass — the same snapshot semantics `note` and `velocity` have had since
  0346. Documented on `set_colour`.
- **The lateness modulator is one-sided**: it is emitted raw and can go negative,
  but `Curve::apply` clamps at the binding, so an early-nudged hit reads as
  dead-on. Widening the curve would be widening ADR 0005's matrix.
- **A pre-0351 flavour blob carrying `slot: 3`** changes meaning rather than
  failing to parse. No shipped flavour does; the format version is deliberately
  not bumped, on ADR 0007's "no user base, redefine rather than migrate" rule.
