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

## Close-out (2026-06-18)

Verified in a DAW (user confirmed the faceplate loads, programs, plays, and the
dub throw works). All automated coverage green; live render confirmed manually.

- **Loads in the plugin window.** `vxn3-ui-web` reuses `vxn-core-ui-web`'s wry
  host; `gui` + `timer` extensions in
  [vxn3-clap](../../vxn-3/crates/vxn3-clap/src/gui.rs). `clap-validator` passes
  with the `gui` extension present; confirmed rendering in-DAW.
- **Program a pattern + play.** 8-track grid in
  [app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js) — click toggles trigs,
  shift-click cycles probability, alt-click retrig, per-track length; edits flow
  over the `EngineIo` command queue (0052 backend). Engine-level coverage:
  `faceplate_io::edit_command_programs_a_trig`.
- **Engine selection + knobs.** Per-track selector (Kick/Tone/Metal/Noise) swaps
  via the shared mailbox (`faceplate_io::engine_selection_swaps_via_shared_mailbox`);
  Decay/Tone/Gain/Pan + length knobs. *(The generic knob set is the MVP cut;
  ADR 0003 + ticket 0067 record the proper per-engine/macro model.)*
- **Delay send knob + p-lockable round-trip.** Per-track Send knob lands with
  0051; send amount is `LockParam::Send` so locks round-trip
  (`fx::send_plock_throws_a_hit_into_the_delay`). Master strip: delay
  time/feedback/return + limiter indicator.
- **Per-lane playhead.** Audio thread publishes lane step indices to atomics; the
  GUI timer pushes them to JS (`faceplate_io::playhead_reflects_each_lanes_position`,
  showing polymetric divergence). Confirmed tracking in-DAW.
- **Plumbing.** Reuses the `vxn-core-app` Controller + `UiEvent::Custom` opcode
  surface; controller→engine wiring tested end to end
  (`vxn3_app::tests::edit_event_reaches_the_command_queue`).
- 69 vxn3 tests green; vxn3 crates clippy-clean; `clap-validator` 0 failures.
