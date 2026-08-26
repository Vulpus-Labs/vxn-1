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

## Notes

- **Renumbered 0308 → 0322.** Filed as 0308 while another session was closing
  its own 0308 (`vxn1b-stack-pos-source`) — a straight collision on the
  worklist's single global counter, from two people filing at once. Theirs moved
  first in history and its id is referenced from closed-ticket history and from
  `bank.rs` / `matrix.rs` / `eval.rs`, so this one moved instead. Commits between
  the two dates cite "0308" meaning the piano; they mean this ticket.

- Deliberately NOT taken here: making the piano aware of VXN1b's split point, so
  the two layers are shaded differently either side of it. That is real polish
  and a real divergence — it would push per-synth knowledge into a shared widget
  — so it wants its own ticket and its own decision.
- vxn-2 is a shipped product. The extraction is mechanical, but land it with both
  suites green and its bundle rebuilt, not on inspection.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
