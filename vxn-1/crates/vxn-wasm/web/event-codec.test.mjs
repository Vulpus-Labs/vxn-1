// Golden + round-trip tests for the JS event codec (ticket 0037).
//
// Run: node crates/vxn-wasm/web/event-codec.test.mjs
//
// The GOLDEN BYTE TABLE below is byte-for-byte identical to the one in
// src/codec.rs `tests::golden()`. It is THE contract: both languages encode to
// these exact 16-byte arrays, and decode them back to the equivalent event. If
// either side drifts, one of these tests fails. (Cross-checking JS<->Rust is
// the shared literal table: same input -> same bytes in both.)

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  encode,
  decode,
  ev,
  SLOT_BYTES,
  PARAM_FLAG_NORM,
  EV_NOTE_ON,
  EV_NOTE_OFF,
  EV_PARAM,
  EV_PITCH_BEND,
  EV_MOD_WHEEL,
  EV_SUSTAIN,
  EV_KEY_MODE,
  EV_SPLIT_POINT,
  EV_GESTURE_BEGIN,
  EV_GESTURE_END,
  PATCH_COUNT,
  GLOBAL_COUNT,
  TOTAL_PARAMS,
  patchClapId,
  globalClapId,
} from "./event-codec.mjs";

// f32 little-endian bytes helper (so the table is auditable, same as Rust).
function le(v) {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setFloat32(0, v, true);
  return [b[0], b[1], b[2], b[3]];
}
const f1 = le(1.0); //  00 00 80 3F
const f0 = le(0.0); //   00 00 00 00
const fhalf = le(0.5); // 00 00 00 3F
const fneg1 = le(-1.0); // 00 00 80 BF
const f100 = le(100.0); // 00 00 C8 42

// row(type, offset, paramIdx, valBytes[4], note, flag) -> 16-byte array.
// seq (10..12) and reserved (12..16) are always zero.
function row(ty, off, pidx, val, note, flag) {
  return [ty, off, pidx & 0xff, pidx >> 8, val[0], val[1], val[2], val[3], note, flag, 0, 0, 0, 0, 0, 0];
}

// (label, event, expected 16 bytes) — IDENTICAL to src/codec.rs golden().
const GOLDEN = [
  ["note_on n0 v0", ev.noteOn(0, 0.0, 0), row(EV_NOTE_ON, 0, 0, f0, 0, 0)],
  ["note_on n127 v1 off7", ev.noteOn(127, 1.0, 7), row(EV_NOTE_ON, 7, 0, f1, 127, 0)],
  ["note_off n0", ev.noteOff(0, 0), row(EV_NOTE_OFF, 0, 0, f0, 0, 0)],
  ["note_off n127 off127", ev.noteOff(127, 127), row(EV_NOTE_OFF, 127, 0, f0, 127, 0)],
  ["param plain id0 v0.5", ev.setParam(0, 0.5, 0), row(EV_PARAM, 0, 0, fhalf, 0, 0)],
  ["param plain id164 v100", ev.setParam(164, 100.0, 3), row(EV_PARAM, 3, 164, f100, 0, 0)],
  ["param norm id0 n0", ev.setParamNorm(0, 0.0, 0), row(EV_PARAM, 0, 0, f0, 0, PARAM_FLAG_NORM)],
  ["param norm id164 n1", ev.setParamNorm(164, 1.0, 0), row(EV_PARAM, 0, 164, f1, 0, PARAM_FLAG_NORM)],
  ["gesture_begin id12", ev.gestureBegin(12, 0), row(EV_GESTURE_BEGIN, 0, 12, f0, 0, 0)],
  ["gesture_end id12", ev.gestureEnd(12, 0), row(EV_GESTURE_END, 0, 12, f0, 0, 0)],
  ["pitch_bend -1", ev.pitchBend(-1.0, 0), row(EV_PITCH_BEND, 0, 0, fneg1, 0, 0)],
  ["pitch_bend +1", ev.pitchBend(1.0, 0), row(EV_PITCH_BEND, 0, 0, f1, 0, 0)],
  ["mod_wheel 0", ev.modWheel(0.0, 0), row(EV_MOD_WHEEL, 0, 0, f0, 0, 0)],
  ["mod_wheel 1", ev.modWheel(1.0, 0), row(EV_MOD_WHEEL, 0, 0, f1, 0, 0)],
  ["sustain off", ev.sustain(false, 0), row(EV_SUSTAIN, 0, 0, f0, 0, 0)],
  ["sustain on", ev.sustain(true, 0), row(EV_SUSTAIN, 0, 0, f0, 0, 1)],
  ["key_mode whole", ev.keyMode(0, 0), row(EV_KEY_MODE, 0, 0, f0, 0, 0)],
  ["key_mode dual", ev.keyMode(1, 0), row(EV_KEY_MODE, 0, 0, f0, 0, 1)],
  ["key_mode split", ev.keyMode(2, 0), row(EV_KEY_MODE, 0, 0, f0, 0, 2)],
  ["split_point 0", ev.splitPoint(0, 0), row(EV_SPLIT_POINT, 0, 0, f0, 0, 0)],
  ["split_point 60", ev.splitPoint(60, 0), row(EV_SPLIT_POINT, 0, 0, f0, 0, 60)],
  ["split_point 127", ev.splitPoint(127, 0), row(EV_SPLIT_POINT, 0, 0, f0, 0, 127)],
];

test("id layout matches ADR 0009 / vxn-app (165 = 2*69 + 27)", () => {
  assert.equal(PATCH_COUNT, 69);
  assert.equal(GLOBAL_COUNT, 27);
  assert.equal(TOTAL_PARAMS, 165);
  assert.equal(TOTAL_PARAMS, 2 * PATCH_COUNT + GLOBAL_COUNT);
  // forward mappings line up with the ranges
  assert.equal(patchClapId(0, 0), 0); // Upper p0
  assert.equal(patchClapId(0, 68), 68); // Upper last
  assert.equal(patchClapId(1, 0), 69); // Lower p0
  assert.equal(patchClapId(1, 68), 137); // Lower last
  assert.equal(globalClapId(0), 138); // global 0
  assert.equal(globalClapId(26), 164); // global last == TOTAL-1
});

test("encode matches golden bytes (== Rust golden table)", () => {
  for (const [label, event, expected] of GOLDEN) {
    const got = Array.from(encode(event));
    assert.deepEqual(got, expected, `encode mismatch for ${label}: ${got.map((b) => b.toString(16))}`);
  }
});

test("decode of golden bytes yields the equivalent event", () => {
  for (const [label, event, bytes] of GOLDEN) {
    const buf = new Uint8Array(bytes);
    const got = decode(new DataView(buf.buffer), 0);
    assert.ok(got, `decode null for ${label}`);
    // Compare field-by-field (decode adds the `norm` bool / drops absent fields).
    for (const k of Object.keys(event)) {
      assert.equal(got[k], event[k], `decode field ${k} mismatch for ${label}`);
    }
  }
});

test("round-trips every event kind", () => {
  for (const [label, event] of GOLDEN) {
    const buf = encode(event);
    const back = decode(new DataView(buf.buffer), 0);
    for (const k of Object.keys(event)) {
      assert.equal(back[k], event[k], `round-trip field ${k} mismatch for ${label}`);
    }
  }
});

test("unknown tag decodes to null (forward-compat)", () => {
  const buf = new Uint8Array(SLOT_BYTES);
  buf[0] = 200;
  assert.equal(decode(new DataView(buf.buffer), 0), null);
});

test("decode at a non-zero slot base reads the right slot", () => {
  // Two slots in one buffer; decode the second by base offset.
  const buf = new Uint8Array(SLOT_BYTES * 2);
  const view = new DataView(buf.buffer);
  buf.set(encode(ev.noteOn(60, 0.5, 1)), 0);
  buf.set(encode(ev.splitPoint(72, 4)), SLOT_BYTES);
  const second = decode(view, SLOT_BYTES);
  assert.equal(second.type, EV_SPLIT_POINT);
  assert.equal(second.note, 72);
  assert.equal(second.offset, 4);
});
