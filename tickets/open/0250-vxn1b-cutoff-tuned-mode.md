---
id: "0250"
product: vxn-1b
title: "Port VXN1's Cutoff \"Tuned\" mode — note-precise cutoff setting"
priority: medium
created: 2026-08-07
epic: E038
depends: ["0245"]
---

## Summary

VXN1's Filter panel carries a **Tuned** strip toggle: with it on, the Cutoff
fader stops being an exponential Hz sweep and becomes a semitone-snapped note
selector over MIDI C0..C4, reading out as a note name ("C2", "A3"). It is how
you set the cutoff to an exact pitch by eye — which matters most with resonance
up, where the filter's own ring is a note, and alongside key-track at unity
(0245), where the cutoff *is* the played pitch.

VXN1b dropped it in 0209 when the compact faceplate was cut down. Everything
except the param survived the fork: the shared math is already spliced into the
page ([cutoff-tuned.js](../../crates/vxn-core-ui-web/assets/cutoff-tuned.js)),
and `dispatch.js` still carries the three overrides plus the repaint-on-toggle
path. They were inert only because `paramIdByNameAtLayer('cutoff_tuned')`
resolved to null, so the `tunedOfCutoff` / `cutoffOfTuned` maps stayed empty.

## Design

- **`ParamId::CutoffTuned`** — bool, default off, in the filter block, per layer
  (patch param) as in VXN1, so the display mode travels with presets and state.
  The **engine never reads it**; cutoff stays a plain Hz param.
- **`locateSyncPartners`** pairs cutoff ↔ cutoff_tuned per layer again, which is
  all the dormant override machinery needed.
- **Faceplate**: a `Tuned` strip switch beside `Slope`, matching VXN1's Filter
  panel.
- **State `VERSION` 4 → 5** (positional param block).

## Acceptance criteria

- [x] `cutoff_tuned` exists as a bool patch param, default `0.0`.
- [x] Engine ignores it: blocks rendered with the toggle off and on are
      bit-identical (`cutoff_tuned_never_reaches_the_engine`).
- [x] Tuned on — drag snaps to a semitone over MIDI 12..60, thumb position
      derives from the snapped Hz, readout is a note name; tuned off — every
      override defers to the default exp-Hz fader
      (`dispatch-orchestration.test.js`, driving the real core helpers rather
      than stand-ins).
- [x] Toggling repaints the cutoff fader (the existing `cutoffOfTuned` path in
      dispatch).
- [ ] **In a DAW:** flip Tuned, sweep the cutoff, confirm it steps semitone by
      semitone with note names; check it survives a preset save/load.

## Notes

- Fixed in passing: `tests/parity.rs` zeroed the vibrato route by slot index
  (`slots[2]`), but 0245 removed the pre-wired Key→Cutoff slot and slid the
  LFO1→Pitch route to slot 1 — so the line had quietly become a no-op and the
  parity gate was running *with* vibrato. Now found by dest, not index.
- The tuned range (C0..C4 = MIDI 12..60) is the shared core constant, and its
  floor is the same C0 the cutoff param's minimum and the key-track pivot use.
