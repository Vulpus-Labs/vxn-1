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

> **Landed in two passes.** The param surface, allocation, detune fan,
> stack-relative stereo fan, legacy-preset mapping and UI shipped first
> (2026-08-11, ADR 0003), followed by stack-granular stealing, width 32 and the
> profile comparison. See the Close-out.

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

## Close-out (2026-08-11)

### Param surface

- `stack_width` (`1/2/4/8/16/32`) and `voice_mode` (`Poly`/`Solo`) are per-layer
  patch params; `assign_mode` is gone from the table and the state version
  bumped. Legacy presets naming `assign_mode` map through the table above —
  `preset::legacy_assign_mode`, covered by
  `legacy_assign_mode_maps_onto_width_and_voice_mode` and
  `unrecognised_legacy_assign_mode_warns` (an unknown value warns and is
  ignored rather than failing the load).
- Faceplate exposes Width as a button group and Voice as a two-way switch, per
  layer. Both read their variants from the Rust label table, so `32` appeared
  with no JS change when 0264 added it.

### Voicing rules

- `the_four_legacy_modes_are_points_in_the_width_mode_space` — Poly=1/Poly,
  Twin=2/Poly, Solo=1/Solo, Unison=N/Solo.
- `full_width_poly_is_monophonic_but_retriggers` vs
  `full_width_solo_slides_under_legato` — the combination the old enum could
  not express.
- `poly_capacity_is_the_pool_divided_by_width` — `32 / width` simultaneous
  notes at every width, the next note stealing rather than sounding.
- `detune_span_is_constant_across_widths` — the outermost lanes sit at ±detune
  whatever the width, so widening a stack makes it denser, not out of tune.
- `a_width_change_leaves_held_stacks_alone` — a width change applies from the
  next note-on (ADR 0003); re-partitioning under held notes would be a click
  and a stolen-note storm.

### Stack-granular allocation

Lanes now carry a `stack_id`; one note-on claims one stack
(`one_note_on_claims_one_stack`). `claim_lanes` takes free lanes first, then
whole victim stacks worst-ranked first — picked by lane, released by *stack*.

The policies only diverge once mixed widths are held, which is exactly what a
width change under held notes produces, so the seam was invisible to every
existing test:

- `a_steal_releases_the_whole_victim_stack` — the case that was wrong. A
  width-4 claim against a pool of width-8 stacks used to slice 4 lanes off a
  held note and leave the other 4 sounding, with a fan missing its outer
  voices and a `level_comp` for a width it no longer had.
- `surplus_lanes_of_a_stolen_stack_ring_out_rather_than_being_cut` — the unused
  half is *released*, not deactivated. Cutting it dead mid-note would click;
  gating it lets the envelope tail finish and ranks it tier 0 for reuse.
- `releasing_a_note_frees_every_lane_of_its_stack`.
- `uniform_widths_steal_exactly_as_the_lane_policy_did` — pins the no-change
  case, which is every patch that never touches Width mid-hold.

### Stereo fan

`SourceId::Spread` reads `stack_pos[v] * ctx.spread`
([bank.rs:514](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L514)), where
`stack_pos` is the lane's position *within its stack* from the same
`stack_spread` helper as the detune fan. This **replaces the 8-entry per-bank
`pan_position` table** [0260](../closed/0260-vxn1b-pan-as-matrix-dest.md) left
behind — correct only while a stack is a bank, and invisible at width 8, which
is why it gets an explicit test:
`the_stereo_fan_spans_the_image_at_every_width` (width 1 centred, width 2 hard
L/R, every width evenly spaced edge to edge). That closes the follow-up logged
on 0260.

### CPU

`busy_profile` ported to vxn-1b
([examples/busy_profile.rs](../../vxn-1b/crates/vxn1b-engine/examples/busy_profile.rs)),
taking a voicing argument so the two cases are one binary. Both run layer 1 at
4× oversample with FX, resonance 0.9, hard sync, detune 12 ct and spread 1, so
the matrix, output stage and pan smoothers are all live.

Six interleaved runs each, 3000 blocks (32 s of audio), min-of-N — the machine
was noisy enough that single runs spread 2.0–4.5 s:

```text
1 x 32 (full poly)              min 2.026 s   6.3% of one core
32 x 1 (one full-width stack)   min 2.038 s   6.4% of one core
```

0.6% apart with fully overlapping distributions: **32 × 1 is no worse than
1 × 32**, as ADR 0003 predicted — 32 lanes are 32 lanes however they are
voiced. The hoped-for vectoriser win from every lane sharing a note did not
materialise either; it is a wash.

### Verification

266 Rust / 240 JS, 0 failures. `tests/parity.rs`, `tests/alloc_free.rs` and
`tests/taper_parity.rs` all green — `claim_lanes` uses two fixed-size stack
arrays and allocates nothing.

### DAW validation (2026-08-12)

Played in Reaper by the user ([[verify-audio-in-reaper]]) — Width and Voice
behave, including 32 × Poly vs 32 × Solo and a width change with notes held.
Every acceptance criterion is now met; nothing outstanding.
