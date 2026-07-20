// On-screen piano keyboard (task: vxn-2 browser). Run: node --test
//
// Two layers: the pure layout helpers (isBlackKey / pianoLayout) and the DOM
// producer (createPianoKeyboard) driven through a tiny createElement stub + a
// fake host that records noteOn / noteOff. Verifies key counts, that a press
// sounds a note and highlights the key, and that a drag is monophonic (each new
// press releases the previous note — a glissando, not a chord).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createPianoKeyboard, pianoLayout, isBlackKey } from "./faceplate-bridge.mjs";

test("isBlackKey marks the five accidentals per octave", () => {
  // C .. B for octave starting at 60 (C4). Black = C#,D#,F#,G#,A#.
  const expected = [false, true, false, true, false, false, true, false, true, false, true, false];
  for (let i = 0; i < 12; i++) assert.equal(isBlackKey(60 + i), expected[i], `note ${60 + i}`);
});

test("pianoLayout covers the inclusive range with correct colouring", () => {
  const layout = pianoLayout(60, 72); // C4..C5 inclusive
  assert.equal(layout.length, 13);
  assert.equal(layout[0].note, 60);
  assert.equal(layout[0].black, false);
  assert.equal(layout[12].note, 72);
  const blacks = layout.filter((k) => k.black).length;
  assert.equal(blacks, 5, "five black keys in one octave");
});

function makeEl() {
  return {
    style: { cssText: "" },
    dataset: {},
    className: "",
    children: [],
    append(...c) { this.children.push(...c); },
    appendChild(c) { this.children.push(c); return c; },
    addEventListener() {},
    removeEventListener() {},
    remove() { this._removed = true; },
  };
}

function fakeDoc() {
  return {
    _byId: {},
    getElementById(id) { return this._byId[id] || null; },
    createElement() { return makeEl(); },
    addEventListener() {},
    removeEventListener() {},
    body: makeEl(),
  };
}

function fakeHost() {
  const events = [];
  return {
    events,
    noteOn(note, vel, off) { events.push(["on", note, vel, off]); },
    noteOff(note, off) { events.push(["off", note, off]); },
  };
}

function keysOf(piano) {
  const bed = piano.el.children[0];
  return bed.children;
}

test("returns a no-op keyboard with no document", () => {
  const p = createPianoKeyboard(null, fakeHost());
  assert.equal(p.el, null);
  assert.doesNotThrow(() => p.detach());
});

test("mounts three octaves of keys (C3..C6)", () => {
  const p = createPianoKeyboard(fakeDoc(), fakeHost());
  const keys = keysOf(p);
  const whites = keys.filter((k) => k.className === "vxn-piano-white").length;
  const blacks = keys.filter((k) => k.className === "vxn-piano-black").length;
  // 48..84 inclusive = 37 notes: 22 white, 15 black.
  assert.equal(whites, 22);
  assert.equal(blacks, 15);
});

test("press sounds a note-on; release sounds the matching note-off", () => {
  const host = fakeHost();
  const p = createPianoKeyboard(fakeDoc(), host);
  p._press(60);
  assert.deepEqual(host.events, [["on", 60, 0.8, 0]]);
  p._release();
  assert.deepEqual(host.events[1], ["off", 60, 0]);
});

test("drag is monophonic: a new press releases the previous note first", () => {
  const host = fakeHost();
  const p = createPianoKeyboard(fakeDoc(), host);
  p._press(60);
  p._press(62); // slide up a whole tone
  assert.deepEqual(host.events, [
    ["on", 60, 0.8, 0],
    ["off", 60, 0],
    ["on", 62, 0.8, 0],
  ]);
});

test("re-pressing the sounding note is a no-op (no retrigger)", () => {
  const host = fakeHost();
  const p = createPianoKeyboard(fakeDoc(), host);
  p._press(60);
  p._press(60);
  assert.equal(host.events.length, 1, "still just the one note-on");
});

test("allNotesOff releases the sounding note", () => {
  const host = fakeHost();
  const p = createPianoKeyboard(fakeDoc(), host);
  p._press(64);
  p.allNotesOff();
  assert.deepEqual(host.events[1], ["off", 64, 0]);
});
