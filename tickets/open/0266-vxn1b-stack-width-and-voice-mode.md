---
id: "0266"
product: vxn-1b
title: "Stack width (1–32 lanes/voice) × voice mode (Poly/Solo), replacing AssignMode"
priority: medium
created: 2026-08-11
epic: E036
depends: ["0264"]
---

## Summary

`AssignMode` is a four-way enum — Poly / Unison / Solo / Twin — that conflates
two independent decisions: **how many lanes a note gets** and **how the keyboard
behaves** (legato/glide vs retrigger). Splitting them is a strict
generalisation. Nothing in the taxonomy is lost:

| Old mode | Stack width | Voice mode |
|---|---|---|
| Poly   | 1  | Poly |
| Twin   | 2  | Poly |
| Solo   | 1  | Solo |
| Unison | 16 | Solo |

...and the combinations the enum cannot express are the ones worth having: 4
lanes × Poly is a fat 8-note stack; 8 × Poly is a 4-note pad that no VXN1 patch
can make; 32 × Poly is one note at a time *without* legato, which is a different
instrument from 32 × Solo.

**Nothing is being unbuilt.** [0244](0244-vxn1b-assign-modes-unison-detune.md)
specced the four-way enum but is not implemented — the allocator is still flat
poly and `AssignMode` is inert in the engine. This ticket **supersedes 0244**,
which should be closed as superseded rather than built first and then replaced.

Precedent: vxn-2 already allocates this way ([[vxn2-stack-soa]]) — `Stack` is
the allocation unit, 16 stacks × 8 lanes with a density control, and it measures
~3.8% of an M1 at full poly × density 8.

## Design

**Params.** `assign_mode` is **replaced** by two params (decided with the user):

- `stack_width` — enum `1, 2, 4, 8, 16, 32`, default `1`. Lanes per note.
  Powers of two only: it divides the 32-lane pool exactly, so there are never
  orphaned lanes, and it keeps the allocator's arithmetic a shift.
- `voice_mode` — enum `Poly | Solo`, default `Poly`. Legato + glide behaviour,
  independent of width.

Simultaneous voices = `32 / stack_width`, so width 32 is monophonic *by
capacity* while still being Poly *by behaviour* (a new note steals the single
stack and retriggers — no legato). That is the case the old enum could not
express at all.

Old presets map through the table above on load. `assign_mode` disappearing
from the param table is a positional change in the state blob → version bump;
the sparse-TOML preset path can translate by name.

**Allocation.** The allocation unit becomes the **stack**, not the lane. The
allocator ([voice.rs](../../vxn-1b/crates/vxn1b-engine/src/voice.rs)) tracks
`32 / width` stacks; note-on claims a whole stack, note-off releases one, and
stealing picks a stack by the existing policy (free → released tail → oldest
held). Per-lane state inside a stack moves together.

Width changes while notes are held: re-partitioning the pool underneath sounding
voices is the messy case. Simplest defensible rule — **the change takes effect
on the next note-on**, held notes keep their current partition until released.
Worth stating explicitly in the code, because the alternative (re-voice
immediately) is a click and a stolen-note storm.

**Detune fan — the rule that needs stating.** `unison_detune` must mean the same
**total span** at every width; changing width changes *density*, not tuning.
Otherwise the same patch at 8 and 32 lanes is two different chords. So the fan
denominator is `width - 1`, not a constant, and width 1 is the degenerate case
(no detune, whatever the param says).

**Stereo fan — collides with 0260.** [`SourceId::Spread`](../../vxn-1b/crates/vxn1b-engine/src/eval.rs)
currently emits `pan_position(lane) × spread`, where `pan_position` is a fixed
8-entry table indexed by lane-within-bank
([bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs)). That is correct only
while a stack *is* a bank. With variable width it must become a function of the
lane's index **within its stack** and the stack's width — evenly spread across
`[-1, 1]` — or a 2-lane stack gets two arbitrary points off an 8-wide table and
a 32-lane stack repeats the same 8 positions four times. Same shape as the
detune fan, and it should use the same helper. Width 1 ⇒ position 0 (centred).

**0264's Unison cap becomes unnecessary.** 0264 caps the Unison fan at 16
because a 32-wide fan over the same detune cents would change the character of
every Unison patch. With width as an explicit control, that concern evaporates —
the player asked for 32. 0264 should keep the 32-lane widening and **drop**
`UNISON_LANES`; this ticket depends on the widening only.

**CPU.** No new worst case: 32 active lanes is 32 active lanes whether they are
32 notes or one 32-lane stack. The idle path is unchanged (inactive lanes are
skipped as now). Worth a `busy_profile` check at 32 × 1 anyway, since a single
stack has every lane on the *same* note — pitch-cascade and filter coefficient
work that usually differs per lane becomes identical, which may or may not help
the vectoriser.

**UI.** The faceplate's `detune-legato` composite cell
([faceplate.html](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html))
bundles detune + legato + mode against the old enum and needs rebuilding around
the two params. Suggest: width as a 6-way button group (it is a voicing
decision, read at a glance), voice mode as a two-way switch beside it.

## Acceptance criteria

- [ ] `stack_width` and `voice_mode` exist as per-layer patch params; `assign_mode`
      is gone from the table, the state version is bumped, and a preset naming
      `assign_mode` still loads by mapping through the table above.
- [ ] Engine test: each old mode's equivalent pair renders as the old mode
      would — Poly=1/Poly, Twin=2/Poly, Solo=1/Solo, Unison=16/Solo.
- [ ] Engine test: simultaneous-voice count is `32 / width` at every width; the
      33rd/17th/9th… note steals rather than sounding.
- [ ] Engine test: 32 × Poly is monophonic **and** retriggers (no legato), while
      32 × Solo glides — the distinction the old enum could not express.
- [ ] Engine test: detune span is constant across widths — the outermost lanes
      of a 4-lane and a 32-lane stack sit at the same cents offset, with the
      32-lane stack denser in between.
- [ ] Engine test: the stereo fan spreads each stack evenly regardless of width
      — a 2-lane stack lands hard L/R at spread 1, a 32-lane stack fills the
      image, and width 1 is centred. (Guards the `pan_position` assumption 0260
      left behind.)
- [ ] Width changes with notes held do not re-voice sounding stacks; the new
      width applies from the next note-on.
- [ ] Allocation/stealing operates on whole stacks: no note is ever left with a
      partial stack, and releasing a note frees all its lanes.
- [ ] `busy_profile` at 32 × 1 (one full-width stack) is no worse than 1 × 32
      (full poly).
- [ ] Faceplate exposes both controls, per layer, and old patches open with the
      mapped values shown.

## Notes

- **Supersedes [0244](0244-vxn1b-assign-modes-unison-detune.md)** — close that as
  superseded when this lands. 0244's *other* half (per-voice unison detune
  actually reaching the engine) is still needed and is folded in here via the
  detune-fan rule.
- **Amends [0264](0264-vxn1b-32-lanes-unison-16.md)**: keep the 32-lane
  widening, drop the `UNISON_LANES = 16` cap.
- **Touches [0260](0260-vxn1b-pan-as-matrix-dest.md)'s `Spread` source**, which
  assumes a stack is a bank. That assumption is only visible at widths ≠ 8, so
  it will not show up in any existing test — hence the explicit criterion above.
- Out of scope: per-stack (rather than per-lane) modulation sources, and
  chord/interval stacking — a stack is one note at N detuned lanes here.
