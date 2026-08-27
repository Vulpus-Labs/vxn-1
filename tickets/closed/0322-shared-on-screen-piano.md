---
id: "0322"
product: monorepo
title: "Hoist the on-screen piano to vxn-core-web and give VXN1b one"
priority: medium
created: 2026-08-26
epic: E045
depends: ["0291"]
---

## Summary

VXN1b's browser build has **no way to play a note without hardware**. Its
faceplate has no playable keys — `keys.js` is the key-*mode* panel (Single /
Dual / Split, split point), not a piano — so a visitor with no MIDI device and
no knowledge of the QWERTY mapping opens the page, sees a synth, and cannot make
a sound. vxn-2 solved this with an on-screen keyboard; VXN1b wants the same.

That widget is ~176 lines living inside
[vxn-2's faceplate-bridge.mjs](../../vxn-2/crates/vxn2-wasm/web/faceplate-bridge.mjs),
and it knows nothing about either synth: it calls `host.noteOn(note, velocity, 0)`
/ `host.noteOff(note, 0)` on the coordinator surface every producer uses. So it
moves to `crates/vxn-core-web/assets/piano-keyboard.mjs` and both ports import
it, rather than becoming the second copy — the rule [[0284]] set.

## Design

### It cannot be a static re-export

vxn-2's bridge kept the functions and its test imported them from there. Leaving
a `export … from "./piano-keyboard.mjs"` in place looks equivalent and is not:
`dist/` is FLAT, the source tree is not, so that specifier resolves in the bundle
and fails in the repo. The piano joins `loadGlue()` with the other shared
modules, and its test imports the shared path directly — the same seam the input
adapters and persistence already use.

### vxn-2's mount stays synchronous

`_mountWebChrome()` runs deliberately early — before the controller-instantiate
await — so the welcome card and piano appear immediately rather than hinging on
a successful async boot. Awaiting a dynamic import there would give that up. The
piano mount becomes fire-and-forget instead: the method stays synchronous, the
piano appears a tick later (still long before audio is live), and a failed import
leaves it absent with a warning rather than throwing out of boot.

### Both bundles must carry it

A dynamic import that the bundle does not ship is a silent 404 — the widget
simply never appears. Both xtasks' `CORE_MODULES` gain it. VXN1b's
closure test (0292) enforces this automatically; **vxn-2 has no such test**, and
that gap is how this nearly shipped broken.

## Acceptance criteria

- [ ] `crates/vxn-core-web/assets/piano-keyboard.mjs` holds `isBlackKey`,
      `pianoLayout` and `createPianoKeyboard`; neither port has a copy.
- [ ] vxn-2's behaviour is unchanged: its 89-test web suite green, its piano
      test passing against the shared module, and `piano-keyboard.mjs` present
      in its bundle.
- [ ] VXN1b mounts the piano at boot; a test asserts 37 keys (C3..C6) and that a
      press reaches the ring.
- [ ] Both bundles ship the module.
- [ ] Manual: clicking the keys plays, and dragging across them glissandos
      monophonically rather than stacking a chord.
- [ ] Keys played by MIDI or the computer keyboard light up, polyphonically, and
      unlight on note-off; a suspend/resume voice flush clears any left lit.
- [ ] In Split mode the keys below the split point are shaded; leaving Split
      clears the shading rather than stranding it.

## Notes

- **Renumbered 0308 → 0322.** Filed as 0308 while another session was closing
  its own 0308 (`vxn1b-stack-pos-source`) — a straight collision on the
  worklist's single global counter, from two people filing at once. Theirs moved
  first in history and its id is referenced from closed-ticket history and from
  `bank.rs` / `matrix.rs` / `eval.rs`, so this one moved instead. Commits between
  the two dates cite "0308" meaning the piano; they mean this ticket.

- **Split shading and note lighting: done (2026-08-26).** The earlier note said
  split awareness "would push per-synth knowledge into a shared widget". That was
  wrong about where the knowledge has to live: the widget takes a bare note
  number and shades below it, and the per-synth half — *when* a split is
  meaningful — stays in VXN1b's bridge, which already receives the key state on
  the `keys` echo. Nothing about layers or key modes reaches the shared module.
  Keys are also lit by any producer now, via a note tap in front of the
  coordinator rather than anything the widget listens to.
- vxn-2 is a shipped product. The extraction is mechanical, but land it with both
  suites green and its bundle rebuilt, not on inspection.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-27)

- `isBlackKey` / `pianoLayout` / `createPianoKeyboard` live once, in
  [piano-keyboard.mjs](../../crates/vxn-core-web/assets/piano-keyboard.mjs).
  Grep for `createPianoKeyboard` across the tree returns only the shared module
  and its three consumers' bridges — no fork in either port.
- vxn-2 unchanged: its web suite is 89 passed / 0 failed / 0 skipped, including
  `piano-keyboard.test.mjs` driving the shared module.
- VXN1b mounts at boot. `faceplate-bridge.test.mjs` — *"boot mounts the
  on-screen piano, and it plays into the ring"* asserts 37 keys (C3..C6, 22
  white + 15 black) and that `_press(60)` advances the ring write index.
- Split shading: *"the split reaches the piano only in Split mode"* pins that
  leaving Split pushes `null` rather than stranding the shading. The widget takes
  a bare note number ([piano-keyboard.mjs:153](../../crates/vxn-core-web/assets/piano-keyboard.mjs#L153));
  when a split is meaningful stays in VXN1b's bridge.
- Lighting by any producer: *"notes from other producers light the keys, and
  still reach the ring"*, *"the tap forwards every other producer call
  untouched"*, *"a piano that throws while painting does not swallow the note"*.
  The suspend/resume flush drops lit keys
  ([piano-keyboard.mjs:214](../../crates/vxn-core-web/assets/piano-keyboard.mjs#L214)).
- Both bundles ship it: `piano-keyboard.mjs` present in `target/web-dist-vxn1b/`
  (via `CORE_MODULES`) and in vxn-2's `target/web-dist/`.
- **Not verified here:** the manual click/glissando feel. Mechanically the
  monophonic drag is [piano-keyboard.mjs:17](../../crates/vxn-core-web/assets/piano-keyboard.mjs#L17)'s
  documented behaviour; confirm by ear in the browser.
