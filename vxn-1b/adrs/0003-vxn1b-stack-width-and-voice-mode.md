# ADR 0003 — Voicing is stack width × voice mode, not a four-way assign enum

**Status:** accepted (2026-08-11)
**Supersedes:** ADR 0001 §4's assumption that VXN1's `AssignMode` carries over
unchanged; ticket [0244](../../tickets/closed/0244-vxn1b-assign-modes-unison-detune.md)
**Related:** [0266](../../tickets/open/0266-vxn1b-stack-width-and-voice-mode.md),
[0264](../../tickets/open/0264-vxn1b-32-lanes-unison-16.md) (widening the pool),
[0260](../../tickets/open/0260-vxn1b-pan-as-matrix-dest.md) (the stereo fan)

## Context

VXN1 has a four-way `AssignMode`: **Poly**, **Unison**, **Solo**, **Twin**.
VXN1b inherited the param and 0244 was written to implement it.

That enum conflates two decisions that have nothing to do with each other:

1. **How many lanes a note gets.** Poly and Solo take one; Twin takes two;
   Unison takes the whole pool.
2. **How the keyboard behaves.** Poly and Twin are polyphonic and always
   retrigger; Solo and Unison are monophonic with last-note priority and a
   `Legato` option.

Because the two are welded together, the enum can only express four of the
combinations and silently forbids the rest. The forbidden ones are not exotic:
a 4-lane stack played polyphonically is an ordinary fat pad, and a full-width
stack played *polyphonically* — monophonic by capacity, but retriggering rather
than sliding — is a different instrument from the same width played Solo, with
no way to ask for it.

The enum also makes capacity implicit. "Twin is 8-note polyphonic" is a fact
about the pool size divided by a lane count the player cannot see or change.

## Decision

Replace `assign_mode` with two orthogonal per-layer params:

- **`stack_width`** — lanes per note: `1, 2, 4, 8, 16`. Powers of two, so the
  width divides the lane pool exactly and never orphans lanes. (`32` joins the
  list when [0264](../../tickets/open/0264-vxn1b-32-lanes-unison-16.md) widens
  the pool; the enum is ordered so appending it is additive.)
- **`voice_mode`** — `Poly` or `Solo`. Keyboard behaviour only.

Simultaneous notes = `pool / stack_width`, which is now something the player
sets rather than infers.

The four legacy modes are four points in that space, so nothing is lost:

| Old mode | Width | Mode |
|---|---|---|
| Poly   | 1  | Poly |
| Twin   | 2  | Poly |
| Solo   | 1  | Solo |
| Unison | 16 | Solo |

### Consequences that needed deciding

**Detune means a constant span at every width.** `unison_detune` sets the
*total* spread of the stack; the fan denominator is `width - 1`. Widening a
stack therefore makes it denser, not wider — the same patch is the same chord
at every width, with more copies filling it in. The alternative (a fixed
per-lane offset) would retune a patch every time its width changed, which makes
width unusable as a sound-design control.

**Width 1 ignores detune.** One lane has nothing to beat against. The knob is
inert rather than doing something arbitrary.

**A width change does not re-voice held notes.** It applies from the next
note-on. Re-partitioning the pool underneath sounding voices means a click and a
burst of stolen notes mid-chord, which is a worse failure than the brief
inconsistency of two widths coexisting until release.

**Glide scaling keys on width, not on a mode name.** VXN1 shortens portamento in
Unison and Twin because a detuned stack slides as one body and reads far
stronger than a single voice. That is a property of *being stacked*, so it now
applies from width 2 up regardless of how the keyboard is being played.

**The Twin detune ceiling is retired.** VXN1's editor clamped detune to 20 ct in
Twin (50 ct elsewhere) because Twin was a fixed two-lane mode nobody explicitly
chose. With width an explicit control, the player who selects 2 lanes is
entitled to the whole 0–50 ct range at it.

**The stereo fan must become stack-relative.** VXN1's voice pan is
`pan_position(lane) × spread`, where `pan_position` indexes a fixed table by
lane. That is only correct while a stack *is* the whole lane group: a 2-lane
stack would take two arbitrary points off it, and a stack wider than the table
would repeat positions. The fan has to be computed from the lane's index within
its stack and the stack's width — the same shape as the detune fan. (0260 made
pan a matrix destination fed by a `Spread` source; this is a change to what that
source emits, not to the routing.)

**Legacy presets translate rather than warn.** A pre-0266 preset carries
`assign_mode` by name. The loader maps it through the table above, so an old
patch keeps its voicing instead of falling back to Poly with a warning. The
binary state blob gets a version bump — the param block is positional and two
params replaced one.

## Alternatives considered

- **Keep the enum and add width beside it.** `AssignMode` would keep a name that
  no longer describes what it does (its Unison/Twin variants become width
  settings), and two controls would overlap on the same axis.
- **Keep the four modes as UI shorthands over the two axes.** Familiar, but it
  means maintaining two vocabularies for one thing, and the shorthands stop
  being meaningful the moment a player picks a width the enum cannot name.
- **Make width a continuous 1–32 param.** Non-powers-of-two leave orphaned lanes
  and make the capacity arithmetic a division rather than a shift, for a control
  whose musically distinct settings are the octaves anyway.

## Notes

Precedent: vxn-2 already allocates this way — `Stack` is the allocation unit,
16 stacks × 8 lanes with a density control, and it measures ~3.8% of an M1 at
full poly × density 8.

The allocator currently claims a stack lane-by-lane through the existing
per-voice policy rather than treating the stack as a single allocation unit.
For a full pool the two agree (oldest lanes go first); they can differ when a
partial steal splits a stack across the pool. Making the stack the allocation
unit outright is the remaining half of
[0266](../../tickets/open/0266-vxn1b-stack-width-and-voice-mode.md).
