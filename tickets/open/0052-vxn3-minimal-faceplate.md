---
id: "0052"
product: vxn-3
title: "vxn-3 minimal HTML faceplate"
priority: medium
created: 2026-06-15
epic: E021
depends: ["0048", "0049"]
---

## Summary

A minimal HTML faceplate to program and play the MVP: an 8-track step grid,
per-track engine select + a few engine knobs, a send knob, master controls, and
a transport-reactive playhead. Reuses the line's `vxn-core-ui-web` faceplate
idiom.

## Design

- **Grid.** 8 track rows × step cells; click to toggle trigs. Reflects per-track
  length (polymeter) — rows can differ in length. Per-trig probability and
  retrig editable at a basic level (e.g. cell context / a small editor).
- **Per-track strip.** Engine selector (`Kick/Tone` / `Metal` / `Noise`) + a
  small set of that engine's knobs, plus level/pan and the delay send amount.
- **Master strip.** Delay controls (time/feedback) + limiter on/off-ish; minimal.
- **Playhead.** Transport-reactive position indicator per lane (lane-local), so
  the polymetric drift is visible.
- **Plumbing.** Reuse the established edit→param opcode surface
  (`vxn-core-ui-web` / controller pattern from the vxn-1 web work) so UI edits
  mutate the model and round-trip; no bespoke transport.

## Acceptance criteria

- [ ] The faceplate loads in the plugin window in a CLAP host.
- [ ] A pattern can be programmed from the UI (toggle trigs, set per-track
      length) and plays.
- [ ] Engine selection per track works and exposes that engine's knobs.
- [ ] The delay send knob is editable (and p-lockable values round-trip if a
      lock is set — display can be basic).
- [ ] The playhead tracks each lane's local position during playback.

## Notes

- Polish, theming, and full p-lock/automation editing UI are post-MVP. This is
  the minimum to play the instrument without a host's generic knob panel.
- Design: `vxn-3/adrs/0001` (overall); reuse `vxn-core-ui-web`.
