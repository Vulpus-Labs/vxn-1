---
id: "0388"
product: vxn-4
title: "vxn-4 faceplate binds the live controls and greys the unbuilt ones"
priority: high
created: 2026-09-10
epic: E052
depends: ["0386", "0387"]
---

## Summary

Wire the faceplate to the model. Every control that corresponds to something the
engine has becomes live — bound by descriptor name from
[0381](0381-vxn4-patch-descriptor-table.md), dispatching through
[0386](0386-vxn4-app-crate.md)'s controller, echoing back on change. Every
control that does not is **visibly greyed and inert**.

More of the faceplate is inert than live at the end of this, and that is the
intended state: a greyed control says "planned, not yet"; an absent one says
"never". The mockup draws vxn-4's intended instrument, not its current one.

## Acceptance criteria

**Live:**

- [ ] The eight macro knobs, with patch-supplied labels and ranges.
- [ ] Patch selection, quality (8x/16x) and master gain — the existing eleven
      CLAP params, driven from the faceplate and echoing host automation back.
- [ ] Per-operator: waveform (the four the engine has), ratio, level, pan,
      damping, phase, phase-decorrelation, and the four-rate/four-level amp EG.
- [ ] The PM grid and the route table: route constants, enables, and each
      route's offset and scaling source and curve.
- [ ] The mod matrix overlay, over the real
      [`SourceId`](../../vxn-4/crates/vxn4-engine/src/matrix.rs#L112) and
      [`DestId`](../../vxn-4/crates/vxn4-engine/src/matrix.rs#L131) rosters —
      **not** the invented lists in the mockup.
- [ ] The scope, from a real capture tap.

**Greyed:**

- [ ] The filter panel entire, both stages.
- [ ] Both LFOs and both ADHSR envelopes on the PERFORM tab.
- [ ] The whole MIXER tab bar master gain and the quality toggle: dynamics,
      phaser, chorus, delay, reverb, and the level meters.
- [ ] Per-operator key scaling, velocity sensitivity, and the rational
      `num`/`den`/`fine`/`fixed-Hz` tuning controls — the engine has a single
      `ratio: f32` ([ops.rs:135](../../vxn-4/crates/vxn4-dsp/src/ops.rs#L135)).
- [ ] The seven proposed waveforms beyond the engine's four.
- [ ] Matrix sources beyond the eight macros, and destinations that name
      unbuilt features.

**Both:**

- [ ] One documented visual treatment for "greyed", applied uniformly, distinct
      from the existing `.dim` used for a bypassed FX section and for an inert
      control in a live panel (a curve with no source behind it).
- [ ] A greyed control cannot be dragged, cannot open a popup, and cannot
      dispatch. Hovering one says why it is greyed.
- [ ] Moving a live control is audible, survives a host save, and round-trips
      through a user preset.
- [ ] Host automation of a macro moves the knob on the faceplate.
- [ ] A test asserts the live/greyed split is **derived from the descriptor
      table**, not hand-maintained — a control with no descriptor greys
      automatically, so building a feature un-greys its control without anyone
      remembering to.

## Notes

That last criterion is the one that matters most for the epic's aftermath.
E052's whole purpose is to make room for the greyed features, and each of them
will be built by a separate later ticket. If the greyed set is a hand-written
list, every one of those tickets has to remember to edit it, and one of them will
not. Deriving it from descriptor presence makes un-greying automatic and makes
this ticket the last time anyone thinks about it.

Decide the greyed treatment before starting rather than during. Reduced opacity
alone will read as "broken" across a whole tab — the MIXER tab is almost
entirely greyed. Consider a distinct treatment for a whole greyed *panel*
(header badge, hatching, a "not yet built" caption) versus an individual greyed
control inside a live panel.

The mockup's mod-matrix source and destination lists are invented and generous.
The real roster is eight macro sources and 80 destinations
([matrix.rs:107](../../vxn-4/crates/vxn4-engine/src/matrix.rs#L107)). The
destination groups the mockup shows for the filter, FX and other mod sources
have no `DestId` behind them at all and must grey rather than appear and fail.
