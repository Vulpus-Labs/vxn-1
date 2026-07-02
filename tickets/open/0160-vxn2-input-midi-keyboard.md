---
id: "0160"
product: vxn-2
title: vxn-2 browser input — Web MIDI + computer keyboard
priority: low
created: 2026-06-30
epic: E030
---

## Summary

Browser input adapters that turn Web MIDI events and computer-keyboard
keypresses into note events on the SAB event ring (ticket 0155). Ports
`vxn-wasm/web/midi-input.mjs`, `keyboard-input.mjs`, `key-mode.mjs`.

## Acceptance criteria

- [ ] `midi-input.mjs`: Web MIDI API → note-on/off + pitch-bend/mod-wheel
      events pushed to the ring; device hotplug handled.
- [ ] `keyboard-input.mjs`: computer-keyboard → MIDI note mapping pushed
      to the ring (held-note tracking, no stuck notes).
- [ ] Key-mode / split-point routing wired the same way as the native
      build (shared-state events, applied once per block).
- [ ] Notes from both sources sound through the worklet engine.

## Notes

Reference: `vxn-wasm/web/{midi-input,keyboard-input,key-mode}.mjs`
(vxn-1 E017). Depends on 0155 (ring) + 0156 (worklet live). Key-mode /
split-point are the EV_KEY_MODE / EV_SPLIT_POINT shared-state events in
the codec (ticket 0153).
