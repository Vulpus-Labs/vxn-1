// The shared Web MIDI decoder, exercised from VXN1b's side (ticket 0294).
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/midi-input.test.mjs
//
// `crates/vxn-core-web/assets/midi-input.mjs` is shared by all three web ports,
// and VXN1b is the one with an MPE-aware producer and NO sustain path. These
// pin the capability detection in both directions: what VXN1b must receive, and
// what a single-timbral host must still not.

import test from "node:test";
import assert from "node:assert/strict";

import { decodeMidiMessage } from "../../../../crates/vxn-core-web/assets/midi-input.mjs";

/// VXN1b's producer shape: channel-aware notes, both pressure messages, and
/// deliberately no `sustain` (its codec reserves CC 64 and decodes it to None,
/// so the web build cannot behave differently from the plugin).
function mpeHost() {
  const calls = [];
  return {
    calls,
    noteOn: (...a) => calls.push(["noteOn", ...a]),
    noteOff: (...a) => calls.push(["noteOff", ...a]),
    polyPressure: (...a) => calls.push(["polyPressure", ...a]),
    channelPressure: (...a) => calls.push(["channelPressure", ...a]),
    pitchBend: (...a) => calls.push(["pitchBend", ...a]),
    modWheel: (...a) => calls.push(["modWheel", ...a]),
  };
}

/// vxn-1's / vxn-2's shape: three-argument notes, a sustain pedal, no pressure.
function singleTimbralHost() {
  const calls = [];
  return {
    calls,
    noteOn: (note, vel, offset) => calls.push(["noteOn", note, vel, offset]),
    noteOff: (note, offset) => calls.push(["noteOff", note, offset]),
    pitchBend: (...a) => calls.push(["pitchBend", ...a]),
    modWheel: (...a) => calls.push(["modWheel", ...a]),
    sustain: (...a) => calls.push(["sustain", ...a]),
  };
}

test("the channel nibble rides note events for an MPE-aware host", () => {
  const h = mpeHost();
  decodeMidiMessage(h, [0x92, 60, 100], 7); // note-on, channel 3
  decodeMidiMessage(h, [0x85, 60, 0], 9); // note-off, channel 6
  assert.deepEqual(h.calls[0], ["noteOn", 60, 100 / 127, 7, 2]);
  assert.deepEqual(h.calls[1], ["noteOff", 60, 9, 5]);
});

test("a second channel is not folded onto the first", () => {
  const h = mpeHost();
  decodeMidiMessage(h, [0x90, 60, 100]);
  decodeMidiMessage(h, [0x93, 64, 100]);
  assert.equal(h.calls[0].at(-1), 0);
  assert.equal(h.calls[1].at(-1), 3, "channel 4's note collapsed onto channel 1");
});

test("note-on with velocity 0 is a note-off, and keeps its channel", () => {
  const h = mpeHost();
  decodeMidiMessage(h, [0x94, 60, 0], 3);
  assert.deepEqual(h.calls[0], ["noteOff", 60, 3, 4]);
});

test("poly and channel aftertouch reach an MPE-aware host", () => {
  const h = mpeHost();
  decodeMidiMessage(h, [0xa2, 60, 64], 5); // poly pressure, channel 3
  decodeMidiMessage(h, [0xd1, 96], 6); // channel pressure, channel 2
  assert.deepEqual(h.calls[0], ["polyPressure", 60, 64 / 127, 5, 2]);
  assert.deepEqual(h.calls[1], ["channelPressure", 96 / 127, 6, 1]);
});

test("aftertouch is NOT sent to a host without those methods", () => {
  // vxn-1 and vxn-2 have no pressure path; the messages must stay ignored, as
  // they were before this decoder learned about them.
  const h = singleTimbralHost();
  decodeMidiMessage(h, [0xa0, 60, 64]);
  decodeMidiMessage(h, [0xd0, 96]);
  assert.deepEqual(h.calls, []);
});

test("a sustain pedal does not throw on a host with no pedal path", () => {
  // The regression this ticket exists to avoid: VXN1b has no `sustain`, and the
  // decoder used to call it unconditionally.
  const h = mpeHost();
  assert.doesNotThrow(() => decodeMidiMessage(h, [0xb0, 64, 127]));
  assert.deepEqual(h.calls, [], "sustain must be dropped, not invented");
});

test("…and still reaches a host that has one", () => {
  const h = singleTimbralHost();
  decodeMidiMessage(h, [0xb0, 64, 127], 4);
  decodeMidiMessage(h, [0xb0, 64, 0], 4);
  assert.deepEqual(h.calls, [
    ["sustain", true, 4],
    ["sustain", false, 4],
  ]);
});

test("a single-timbral host still sees plain three-argument notes", () => {
  // The trailing channel argument must be invisible to the shipped ports: JS
  // drops extra arguments, so their handlers are unchanged.
  const h = singleTimbralHost();
  decodeMidiMessage(h, [0x95, 72, 64], 2);
  assert.deepEqual(h.calls[0], ["noteOn", 72, 64 / 127, 2]);
});

test("mod wheel and pitch bend are unchanged for both shapes", () => {
  for (const h of [mpeHost(), singleTimbralHost()]) {
    decodeMidiMessage(h, [0xb0, 1, 127]);
    decodeMidiMessage(h, [0xe0, 0, 64]); // centre
    assert.deepEqual(h.calls[0], ["modWheel", 1, 0]);
    assert.equal(h.calls[1][0], "pitchBend");
    assert.ok(Math.abs(h.calls[1][1]) < 1e-6, "centre bend must be 0");
  }
});

test("non-status bytes and empty messages are ignored", () => {
  const h = mpeHost();
  decodeMidiMessage(h, []);
  decodeMidiMessage(h, [0x40, 60, 100]); // running status: unsupported
  decodeMidiMessage(h, null);
  assert.deepEqual(h.calls, []);
});
