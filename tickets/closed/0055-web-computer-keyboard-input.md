---
id: "0055"
product: vxn-1
title: "Computer-keyboard note input (QWERTY map, octave shift, held tracking)"
priority: high
created: 2026-06-15
epic: E017
depends: ["0042"]
---

## Summary

A computer-keyboard → note-number input adapter so users without a MIDI device
can play the web synth. QWERTY tracker-style layout, octave shift, and held-note
tracking that suppresses OS auto-repeat retrigger and sends the correct
note-off on keyup. Pure E015-ring producer, like the MIDI adapter.

## Design

- **Module** `vxn-1/crates/vxn-wasm/web/keyboard-input.mjs` —
  `attachKeyboard(host, opts)`.
- **Layout.** Key off `KeyboardEvent.code` (physical key), NOT `.key`, so the
  mapping is layout-independent (AZERTY/QWERTZ/Dvorak get the same piano shape).
  Two overlapping octaves: lower row `KeyZ`=C … upper row `KeyQ`=C+12 …, black
  keys on the row above. Base note default 48 (C3).
- **Octave shift.** `Minus`/`Equal` (− / =) keys shift the base by ±12, clamped
  `minOctave..maxOctave` (default ±4). Shift affects only notes pressed AFTER
  it.
- **Held-note tracking / auto-repeat.** A `code → sentNote` Map is both the
  held-set (presence ⇒ held, so a repeated `keydown` is swallowed — no double
  note-on) AND the record of the exact note sent, so a note-off on keyup is
  always at the pitch we played even if the octave shifted mid-hold (no
  orphaned off). `event.repeat` is a fast-path; the held-set is authoritative.
- **Stuck-note guard.** `window` `blur` flushes all held notes (focus loss can
  swallow keyup). `allNotesOff()` exposed.
- **Typing guard.** `opts.ignoreWhenTyping` (default true) ignores key events
  whose target is `<input>/<textarea>/<select>`/contenteditable so typing in a
  field (e.g. preset name) doesn't play.
- **Return** `{ detach, octaveUp, octaveDown, setOctave, getOctave, held,
  allNotesOff }`.

## Acceptance criteria

- [ ] QWERTY keys map to the correct note numbers (lower + upper rows,
      semitone-accurate) from the base note.
- [ ] Octave shift moves subsequent notes by ±12 and clamps; sounding notes are
      unaffected.
- [ ] Holding a key (OS auto-repeat) fires exactly one note-on; keyup fires one
      note-off at the sent pitch (octave-shift-safe).
- [ ] `allNotesOff` / blur flushes held notes; keyup for an un-held key is a
      no-op; keys in a text field don't play.

## Notes

- Velocity is a fixed default (no pressure on a computer keyboard).
- Single producer into the same ring as MIDI; the worklet can't tell the source.

## Close-out (2026-06-15)

- **Module.** `vxn-1/crates/vxn-wasm/web/keyboard-input.mjs` —
  `attachKeyboard(host, opts)`. Keys off `event.code` (physical key,
  layout-independent), two overlapping octaves (`KeyZ`=base C … `KeyQ`=base+12),
  base note 48, fixed velocity 0.8.
- **Octave shift.** `Minus`/`Equal` shift ±12, clamped ±4; affects only
  subsequent presses.
- **Held / auto-repeat.** `code → sentNote` Map is both held-set (swallows
  repeated keydown — exactly one note-on) and the sent-pitch record (keyup
  note-off at the played pitch even after a mid-hold octave shift). `blur` +
  `allNotesOff()` flush stuck notes. `ignoreWhenTyping` skips input/textarea/
  select/contenteditable targets.
- **Tests.** `web/keyboard-input.test.mjs` §1 (QWERTY→note mapping, lower+upper
  rows, unmapped key no-op), §2 (octave ±1 shift correctness), §3 (auto-repeat
  via `event.repeat` AND held-set → one note-on), §4 (keyup note-off at sent
  pitch despite octave shift; un-held keyup no-op), §5 (allNotesOff flush), §6
  (typing-in-field guard). Fakes an EventTarget + synthetic KeyboardEvents,
  asserts via `EventRing.drainInto`.
- **Build.** `keyboard-input.mjs` added to xtask MODULES; bundles to dist.
- **Headless run note.** Harness written + self-reviewed; manual `node` run
  pending (script execution blocked in this sandbox).
