// Headless Node test for the computer-keyboard input adapter (ticket 0055).
//
//   node web/keyboard-input.test.mjs
//
// Node has no DOM; we inject a fake EventTarget and synthesise KeyboardEvents
// (using event.code, the physical-key field the adapter keys off). We assert
// against the REAL E015 EventRing. Covers the 0055 acceptance:
//
//   1. QWERTY -> note mapping (lower + upper rows, correct semitones).
//   2. octave shift moves subsequent notes by 12; held notes unaffected.
//   3. auto-repeat / already-held does NOT retrigger (no double note-on).
//   4. keyup sends the matching note-off at the note actually sent (octave-shift
//      mid-hold can't orphan the off).
//   5. blur / allNotesOff flushes stuck notes.

import {
  createRingSAB,
  EventRing,
  EV_NOTE_ON,
  EV_NOTE_OFF,
} from "./event-ring.mjs";
import { attachKeyboard, DEFAULT_BASE_NOTE } from "./keyboard-input.mjs";

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

function ringHost() {
  const sab = createRingSAB(64);
  const ring = new EventRing(sab, 64);
  return {
    ring,
    noteOn: (n, v, o) => ring.pushNoteOn(o, n, v),
    noteOff: (n, o) => ring.pushNoteOff(o, n),
    drain() {
      const out = [];
      ring.drainInto(out);
      return out;
    },
  };
}

// Fake DOM EventTarget that records listeners and lets us fire events.
function fakeTarget() {
  const listeners = new Map();
  return {
    addEventListener(type, fn) {
      listeners.set(type, fn);
    },
    removeEventListener(type) {
      listeners.delete(type);
    },
    fire(type, ev) {
      const fn = listeners.get(type);
      if (fn) fn(ev);
    },
  };
}

// Synthesise a KeyboardEvent-ish object.
function key(code, { repeat = false, target = null } = {}) {
  return { code, repeat, target, preventDefault() {} };
}

console.log("\n=== 1. QWERTY -> note mapping ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  // KeyZ == base C (48), KeyS == C# (49), KeyM == B (59), KeyQ == C+12 (60).
  t.fire("keydown", key("KeyZ"));
  t.fire("keydown", key("KeyS"));
  t.fire("keydown", key("KeyM"));
  t.fire("keydown", key("KeyQ"));
  const recs = host.drain();
  const notes = recs.filter((r) => r.type === EV_NOTE_ON).map((r) => r.note);
  check(notes[0] === DEFAULT_BASE_NOTE, `KeyZ -> base note ${DEFAULT_BASE_NOTE} (got ${notes[0]})`);
  check(notes[1] === DEFAULT_BASE_NOTE + 1, `KeyS -> base+1 (got ${notes[1]})`);
  check(notes[2] === DEFAULT_BASE_NOTE + 11, `KeyM -> base+11 (got ${notes[2]})`);
  check(notes[3] === DEFAULT_BASE_NOTE + 12, `KeyQ -> base+12 upper octave (got ${notes[3]})`);
  // Non-mapped key -> nothing.
  t.fire("keydown", key("Backquote"));
  check(host.drain().length === 0, "unmapped key produces no note");
  kb.detach();
}

console.log("\n=== 2. octave shift ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  check(kb.getOctave() === 0, "octave starts at 0");
  // Equal key == octave up.
  t.fire("keydown", key("Equal"));
  check(kb.getOctave() === 1, "Equal raised octave to 1");
  t.fire("keydown", key("KeyZ")); // now base + 12
  // Minus key == octave down (twice -> -1).
  t.fire("keydown", key("Minus"));
  t.fire("keydown", key("Minus"));
  check(kb.getOctave() === -1, "Minus lowered octave to -1");
  t.fire("keydown", key("KeyX")); // base + (-12) + 2 (D)
  const recs = host.drain().filter((r) => r.type === EV_NOTE_ON);
  check(recs[0].note === DEFAULT_BASE_NOTE + 12, `note after +1 octave (got ${recs[0].note})`);
  check(recs[1].note === DEFAULT_BASE_NOTE - 12 + 2, `note after -1 octave (got ${recs[1].note})`);
  kb.detach();
}

console.log("\n=== 3. auto-repeat does not retrigger ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  t.fire("keydown", key("KeyZ")); // fresh -> note on
  t.fire("keydown", key("KeyZ", { repeat: true })); // OS auto-repeat
  t.fire("keydown", key("KeyZ")); // repeat WITHOUT the flag (held-set catches it)
  const ons = host.drain().filter((r) => r.type === EV_NOTE_ON);
  check(ons.length === 1, `held key fires exactly one note-on (got ${ons.length})`);
  check(kb.held().length === 1, "one note tracked as held");
  kb.detach();
}

console.log("\n=== 4. keyup -> matching note-off (octave-shift-safe) ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  t.fire("keydown", key("KeyZ")); // note on at base (48)
  host.drain();
  // Shift octave UP while the key is still held.
  t.fire("keydown", key("Equal"));
  // Release: the note-off MUST be at 48 (the note we sent), not 60.
  t.fire("keyup", key("KeyZ"));
  const offs = host.drain().filter((r) => r.type === EV_NOTE_OFF);
  check(offs.length === 1 && offs[0].note === DEFAULT_BASE_NOTE, `note-off at the sent pitch ${DEFAULT_BASE_NOTE} despite octave shift (got ${offs[0] && offs[0].note})`);
  check(kb.held().length === 0, "held cleared after keyup");
  // keyup for a never-pressed key is a no-op.
  t.fire("keyup", key("KeyX"));
  check(host.drain().length === 0, "keyup for un-held key is a no-op");
  kb.detach();
}

console.log("\n=== 5. allNotesOff / blur flush ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  t.fire("keydown", key("KeyZ"));
  t.fire("keydown", key("KeyX"));
  t.fire("keydown", key("KeyC"));
  host.drain();
  check(kb.held().length === 3, "3 notes held");
  kb.allNotesOff();
  const offs = host.drain().filter((r) => r.type === EV_NOTE_OFF);
  check(offs.length === 3, `allNotesOff flushed all 3 (got ${offs.length})`);
  check(kb.held().length === 0, "nothing held after flush");
  kb.detach();
}

console.log("\n=== 6. ignore typing in input fields ===");
{
  const host = ringHost();
  const t = fakeTarget();
  const kb = attachKeyboard(host, { target: t });
  t.fire("keydown", key("KeyZ", { target: { tagName: "INPUT" } }));
  check(host.drain().length === 0, "key in an <input> does not play");
  t.fire("keydown", key("KeyZ", { target: { tagName: "DIV" } }));
  check(host.drain().length === 1, "key outside a field plays");
  kb.detach();
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
