// Headless Node test for the Web MIDI input adapter (ticket 0053 + 0054).
//
//   node web/midi-input.test.mjs
//
// Node has no Web MIDI / DOM, so we inject fakes: a fake requestMIDIAccess that
// hands back fake input ports, and synthetic MIDI byte arrays. We assert against
// the REAL E015 EventRing — attachMidi writes through a tiny host whose producer
// surface IS the ring's push*, and we drain the ring to check the decoded
// events byte-for-byte. Covers the 0053/0054 acceptance:
//
//   1. decode: note on/off + velocity, vel-0 == note-off, pitch bend, mod
//      wheel (CC1), sustain (CC64), via the producer surface into the ring.
//   2. timestamp -> sample-offset mapping (and its documented degenerates).
//   3. enumeration: all present inputs get subscribed on attach.
//   4. hotplug: statechange connect adds an input, disconnect removes it.
//   5. graceful denial: no requestMIDIAccess / a rejected prompt resolves a
//      controller with granted=false (never throws) — keyboard fallback intact.

import {
  createRingSAB,
  EventRing,
  EV_NOTE_ON,
  EV_NOTE_OFF,
  EV_PITCH_BEND,
  EV_MOD_WHEEL,
  EV_SUSTAIN,
} from "./event-ring.mjs";
import { attachMidi, decodeMidiMessage, makeOffsetMapper } from "./midi-input.mjs";

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};
const approx = (a, b, eps = 1e-5) => Math.abs(a - b) <= eps;

// A minimal host whose producer surface is the real EventRing (same methods the
// WebHost exposes). attachMidi/decode only ever call these five.
function ringHost(ctx = null) {
  const sab = createRingSAB(64);
  const ring = new EventRing(sab, 64);
  return {
    ctx,
    ring,
    noteOn: (n, v, o) => ring.pushNoteOn(o, n, v),
    noteOff: (n, o) => ring.pushNoteOff(o, n),
    pitchBend: (v, o) => ring.pushPitchBend(o, v),
    modWheel: (v, o) => ring.pushModWheel(o, v),
    sustain: (on, o) => ring.pushSustain(o, on),
    drain() {
      const out = [];
      ring.drainInto(out);
      return out;
    },
  };
}

// Fake MIDI port + access mirroring the Web MIDI shapes attachMidi touches.
function fakePort(id, type = "input", state = "connected") {
  return { id, type, state, name: id, onmidimessage: null };
}
function fakeAccess(ports) {
  const map = new Map(ports.map((p) => [p.id, p]));
  return {
    inputs: { values: () => map.values() },
    onstatechange: null,
    _map: map,
    // helper to fire a statechange like the browser would
    _fireStateChange(port) {
      if (this.onstatechange) this.onstatechange({ port });
    },
  };
}

console.log("\n=== 1. decode: note/velocity, bend, mod wheel, sustain ===");
{
  const host = ringHost();
  // Note on, note 60, vel 100 -> unit 100/127.
  decodeMidiMessage(host, [0x90, 60, 100], 7);
  // Note on vel 0 == note off.
  decodeMidiMessage(host, [0x90, 60, 0], 0);
  // Explicit note off.
  decodeMidiMessage(host, [0x80, 64, 40], 3);
  // Pitch bend centre -> 0.
  decodeMidiMessage(host, [0xe0, 0x00, 0x40], 0);
  // Pitch bend max -> +1.
  decodeMidiMessage(host, [0xe0, 0x7f, 0x7f], 0);
  // Pitch bend min -> -1.
  decodeMidiMessage(host, [0xe0, 0x00, 0x00], 0);
  // Mod wheel CC1 = 64 -> ~0.5039.
  decodeMidiMessage(host, [0xb0, 1, 64], 0);
  // Sustain CC64 = 127 -> on.
  decodeMidiMessage(host, [0xb0, 64, 127], 0);
  // Sustain CC64 = 0 -> off.
  decodeMidiMessage(host, [0xb0, 64, 0], 0);
  // Unhandled CC -> ignored.
  decodeMidiMessage(host, [0xb0, 7, 100], 0);
  // Program change -> ignored.
  decodeMidiMessage(host, [0xc0, 5], 0);

  const recs = host.drain();
  const noteOn = recs.find((r) => r.type === EV_NOTE_ON);
  check(noteOn && noteOn.note === 60 && noteOn.offset === 7, "note-on decoded (note 60, offset 7)");
  check(noteOn && approx(noteOn.value, 100 / 127), `note-on velocity scaled (${noteOn && noteOn.value})`);

  const noteOffs = recs.filter((r) => r.type === EV_NOTE_OFF);
  check(noteOffs.length === 2, `two note-offs (explicit + vel-0), got ${noteOffs.length}`);
  check(noteOffs.some((r) => r.note === 60 && r.offset === 0), "vel-0 note-on became note-off (note 60)");
  check(noteOffs.some((r) => r.note === 64 && r.offset === 3), "explicit note-off decoded (note 64, offset 3)");

  const bends = recs.filter((r) => r.type === EV_PITCH_BEND);
  check(bends.length === 3, `three pitch bends, got ${bends.length}`);
  check(approx(bends[0].value, 0), `bend centre -> 0 (${bends[0].value})`);
  check(approx(bends[1].value, 1), `bend max -> +1 (${bends[1].value})`);
  check(approx(bends[2].value, -1), `bend min -> -1 (${bends[2].value})`);

  const mod = recs.find((r) => r.type === EV_MOD_WHEEL);
  check(mod && approx(mod.value, 64 / 127), `CC1 mod wheel scaled (${mod && mod.value})`);

  const sustains = recs.filter((r) => r.type === EV_SUSTAIN);
  check(sustains.length === 2, `two sustain events, got ${sustains.length}`);
  check(sustains[0].flag === 1, "CC64=127 -> sustain on");
  check(sustains[1].flag === 0, "CC64=0 -> sustain off");

  // 11 input messages, but only 9 produce a ring record (CC7 + program change ignored).
  check(recs.length === 9, `unhandled messages ignored (9 records, got ${recs.length})`);
}

console.log("\n=== 2. timestamp -> sample-offset mapping ===");
{
  // No context -> always offset 0 (safe degenerate).
  const m0 = makeOffsetMapper({ ctx: null }, 128);
  check(m0(123.4) === 0, "no AudioContext -> offset 0");

  // timeStamp 0 (unsupported) -> 0 even with a context.
  const ctx = {
    sampleRate: 48000,
    currentTime: 10.0,
    getOutputTimestamp: () => ({ contextTime: 10.0, performanceTime: 1000.0 }),
  };
  const m1 = makeOffsetMapper({ ctx }, 128);
  check(m1(0) === 0, "timeStamp 0 (unsupported) -> offset 0");

  // Event exactly 'now' (perf 1000 maps to context 10.0 == currentTime) -> 0.
  check(m1(1000.0) === 0, "event at currentTime -> offset 0");

  // Event 1ms in the FUTURE -> 48 frames at 48k (1ms = 48 samples).
  check(m1(1001.0) === 48, `event +1ms -> 48 frames (got ${m1(1001.0)})`);

  // Event in the PAST -> clamps to 0 (never a stale block).
  check(m1(999.0) === 0, "past event clamps to offset 0");

  // Event far in the future -> clamps to Q-1 = 127.
  check(m1(2000.0) === 127, `+1s event clamps to Q-1 (got ${m1(2000.0)})`);

  // getOutputTimestamp absent -> 0.
  const m2 = makeOffsetMapper({ ctx: { sampleRate: 48000, currentTime: 0 } }, 128);
  check(m2(500) === 0, "no getOutputTimestamp -> offset 0");
}

console.log("\n=== 3. enumeration: present inputs subscribed on attach ===");
{
  const host = ringHost();
  const a = fakeAccess([fakePort("in-1"), fakePort("in-2"), fakePort("out-1", "output")]);
  const ctl = await attachMidi(host, { requestMIDIAccess: async () => a });
  check(ctl.state.granted === true, "access granted");
  check(ctl.inputs().length === 2, `2 inputs subscribed (output skipped), got ${ctl.inputs().length}`);
  check(typeof a._map.get("in-1").onmidimessage === "function", "input port got an onmidimessage handler");

  // A message through a subscribed port reaches the ring.
  a._map.get("in-1").onmidimessage({ data: [0x90, 72, 64], timeStamp: 0 });
  const recs = host.drain();
  check(recs.length === 1 && recs[0].type === EV_NOTE_ON && recs[0].note === 72, "live port message reached the ring");
  ctl.detach();
  check(a._map.get("in-1").onmidimessage === null, "detach unsubscribed the port");
}

console.log("\n=== 4. hotplug: connect adds, disconnect removes ===");
{
  const host = ringHost();
  const a = fakeAccess([fakePort("in-1")]);
  let lastEvent = null;
  const ctl = await attachMidi(host, {
    requestMIDIAccess: async () => a,
    onStateChange: (p) => (lastEvent = p),
  });
  check(ctl.inputs().length === 1, "1 input at attach");

  // Hotplug ADD.
  const newPort = fakePort("in-2", "input", "connected");
  a._map.set("in-2", newPort);
  a._fireStateChange(newPort);
  check(ctl.inputs().length === 2, `hotplug connect added input, got ${ctl.inputs().length}`);
  check(lastEvent && lastEvent.id === "in-2", "onStateChange fired for the added port");
  check(typeof newPort.onmidimessage === "function", "added port subscribed");

  // Added port plays.
  newPort.onmidimessage({ data: [0x90, 48, 100], timeStamp: 0 });
  check(host.drain().length === 1, "hotplugged port reaches the ring");

  // Hotplug REMOVE.
  const gone = fakePort("in-1", "input", "disconnected");
  a._fireStateChange(gone);
  check(ctl.inputs().length === 1, `hotplug disconnect removed input, got ${ctl.inputs().length}`);
  ctl.detach();
}

console.log("\n=== 5. graceful denial ===");
{
  const host = ringHost();
  // No requestMIDIAccess available (Safari w/o Web MIDI).
  let errSeen = null;
  const ctl1 = await attachMidi(host, { requestMIDIAccess: null, onError: (e) => (errSeen = e) });
  check(ctl1.state.granted === false, "no Web MIDI -> granted=false (no throw)");
  check(errSeen != null, "onError called when Web MIDI unavailable");
  check(ctl1.inputs().length === 0, "no inputs when unavailable");

  // Permission denied (requestMIDIAccess rejects).
  let errSeen2 = null;
  const ctl2 = await attachMidi(host, {
    requestMIDIAccess: async () => {
      throw new Error("SecurityError: permission denied");
    },
    onError: (e) => (errSeen2 = e),
  });
  check(ctl2.state.granted === false, "denied prompt -> granted=false (no throw)");
  check(errSeen2 != null && /denied/.test(String(errSeen2)), "onError called with the denial");
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
