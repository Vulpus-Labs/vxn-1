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

- [x] `midi-input.mjs`: Web MIDI API → note-on/off + pitch-bend/mod-wheel/
      sustain pushed to the ring; timestamp→sample-offset mapper; device hotplug;
      graceful denial (resolves `granted:false`, keyboard still plays). Ported
      verbatim from vxn-1 — source-agnostic (only WebHost producer calls).
- [x] `keyboard-input.mjs`: computer-keyboard → MIDI note mapping pushed to the
      ring; held-note tracking (auto-repeat swallowed), octave shift, blur flush
      (no stuck notes), ignore-when-typing. Ported verbatim.
- [~] Key-mode / split-point routing: **N/A for vxn-2** — the FM engine has no
      dual/split layer (established in 0153; tags 7/8 reserved-unused). Dropped.
- [x] Notes from both sources sound through the worklet engine: both adapters
      drive `WebHost.noteOn/off/pitchBend/modWheel/sustain` (the ring); the
      bridge attaches them on boot. The real-wasm tone test (0156) already proves
      a ring note-on renders audio.

## Close-out (2026-07-11)

Done. Both adapters ported verbatim (the WebHost producer surface is identical
across synths), wired into `faceplate-bridge` boot (`_attachInputs` — keyboard
sync, MIDI async/graceful), and added to the xtask bundle → they ship in
`web-dist/`. vxn-1's `key-mode.mjs` was NOT ported (no vxn-2 analogue).

Tests: the vxn-1 suites are self-running `check()` scripts that `process.exit`,
so they'd abort a `node --test` glob — renamed to `*.check.mjs` and folded in via
`input-adapters.test.mjs` (runs each as a subprocess, asserts clean exit). Full
web suite: **46 node tests** green.

Computer-keyboard play is the practical path for the browser click-verify
(play notes with Z/X/C… after the page unlocks audio); Web MIDI works where the
browser supports it.

## Notes

Reference: `vxn-wasm/web/{midi-input,keyboard-input}.mjs` (vxn-1 E017). Depends
on 0155 (ring) + 0156 (worklet live).
