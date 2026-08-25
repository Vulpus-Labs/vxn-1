// Headless test for the JS event codec (0287).
//
// The golden table below is a transcription of the Rust one in
// `src/codec.rs::tests::golden`. That duplication is the point: two independent
// hand-written tables that must agree byte-for-byte, so a layout change in
// either language fails here instead of silently mis-routing events at runtime.
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/event-codec.test.mjs

import test from "node:test";
import assert from "node:assert/strict";

import {
  SLOT_BYTES,
  PATCH_COUNT,
  GLOBAL_COUNT,
  LAYER_COUNT,
  TOTAL_PARAMS,
  patchClapId,
  globalClapId,
  encode,
  decode,
  ev,
  packMatrixAddr,
  unpackMatrixAddr,
  MATRIX_SLOTS,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_CURVE,
  MATRIX_FIELD_SCALE_SRC,
  EV_SUSTAIN_RESERVED,
  LAYER_L1,
  LAYER_L2,
} from "./event-codec.mjs";

// f32 LE byte helpers, so the table is auditable by eye:
//   1.0 = 00 00 80 3F   0.5 = 00 00 00 3F   -1.0 = 00 00 80 BF
//   120.0 = 00 00 F0 42
const f1 = [0x00, 0x00, 0x80, 0x3f];
const fhalf = [0x00, 0x00, 0x00, 0x3f];
const fneg1 = [0x00, 0x00, 0x80, 0xbf];
const f120 = [0x00, 0x00, 0xf0, 0x42];
const f0 = [0, 0, 0, 0];

/// One 16-byte row: type, offset, paramIdx (u16 LE), value (f32 LE), note, flag,
/// then seq (u16) + reserved (f32), both always zero out of `encode`.
const row = (type, offset, paramIdx, value, note, flag) => [
  type,
  offset,
  paramIdx & 0xff,
  (paramIdx >> 8) & 0xff,
  ...value,
  note,
  flag,
  0,
  0,
  0,
  0,
  0,
  0,
];

// Mirrors src/codec.rs::tests::golden(), row for row and in the same order.
const GOLDEN = [
  ["note_on ch0 n60 v1.0", ev.noteOn(60, 1.0), row(1, 0, 0, f1, 60, 0)],
  // The MPE case: channel in `flag` (off 9), which vxn-1 leaves zero.
  ["note_on ch3 n60 v0.5 off7", ev.noteOn(60, 0.5, 7, 3), row(1, 7, 0, fhalf, 60, 3)],
  ["note_off ch3 n60", ev.noteOff(60, 0, 3), row(2, 0, 0, f0, 60, 3)],
  ["param plain id5 v0.5", ev.setParam(5, 0.5), row(3, 0, 5, fhalf, 0, 0)],
  ["param norm id300 n1.0", ev.setParamNorm(300, 1.0), row(3, 0, 300, f1, 0, 1)],
  ["pitch_bend -1.0", ev.pitchBend(-1.0), row(4, 0, 0, fneg1, 0, 0)],
  ["mod_wheel 1.0", ev.modWheel(1.0), row(5, 0, 0, f1, 0, 0)],
  ["key_mode 2 (split)", ev.keyMode(2), row(7, 0, 0, f0, 0, 2)],
  ["split_point 60", ev.splitPoint(60), row(8, 0, 0, f0, 0, 60)],
  ["gesture_begin id12", ev.gestureBegin(12), row(9, 0, 12, f0, 0, 0)],
  ["gesture_end id12", ev.gestureEnd(12), row(10, 0, 12, f0, 0, 0)],
  ["lfo2_link on", ev.lfo2Link(true), row(11, 0, 0, f0, 0, 1)],
  // L2 (1<<12) | slot 5 (5<<8) | field Dest (1) = 0x1501. Value = Cutoff (4).
  [
    "matrix_edit L2 slot5 dest=Cutoff",
    ev.matrixEdit(LAYER_L2, 5, MATRIX_FIELD_DEST, 4),
    row(12, 0, 0x1501, f0, 0, 4),
  ],
  ["scope_tap Layer2", ev.scopeTap(2), row(13, 0, 0, f0, 0, 2)],
  ["tempo 120", ev.tempo(120.0), row(14, 0, 0, f120, 0, 0)],
  ["poly_pressure ch3 n60 1.0", ev.polyPressure(60, 1.0, 0, 3), row(15, 0, 0, f1, 60, 3)],
  ["channel_pressure ch3 1.0", ev.channelPressure(1.0, 0, 3), row(16, 0, 0, f1, 0, 3)],
];

test("encode matches the golden bytes (== the Rust golden table)", () => {
  for (const [label, event, expected] of GOLDEN) {
    assert.deepEqual(Array.from(encode(event)), expected, `encode mismatch for ${label}`);
  }
});

test("decode of golden bytes yields the equivalent event", () => {
  for (const [label, event, expected] of GOLDEN) {
    const got = decode(Uint8Array.from(expected));
    assert.ok(got, `${label} must decode`);
    assert.equal(got.type, event.type, `${label} type`);
    assert.equal(got.offset, event.offset ?? 0, `${label} offset`);
  }
});

test("every event round-trips through encode -> decode", () => {
  for (const [label, event] of GOLDEN) {
    const got = decode(encode(event));
    assert.ok(got, `${label} must round-trip`);
    for (const key of Object.keys(event)) {
      if (key === "type" || key === "offset") continue;
      assert.deepEqual(got[key], event[key], `${label}: field ${key}`);
    }
  }
});

test("every slot is exactly 16 bytes", () => {
  for (const [label, event] of GOLDEN) {
    assert.equal(encode(event).length, SLOT_BYTES, `${label} slot size`);
  }
});

test("unknown and reserved tags decode to null (forward-compat)", () => {
  for (const tag of [0, EV_SUSTAIN_RESERVED, 17, 200, 255]) {
    const buf = new Uint8Array(SLOT_BYTES);
    buf[0] = tag;
    assert.equal(decode(buf), null, `tag ${tag} must not decode`);
  }
});

test("a short slot decodes to null", () => {
  assert.equal(decode(Uint8Array.from([1, 0, 0])), null);
});

test("id layout matches vxn1b-engine (185 = 2*75 + 35)", () => {
  assert.equal(PATCH_COUNT, 75);
  assert.equal(GLOBAL_COUNT, 35);
  assert.equal(LAYER_COUNT, 2);
  assert.equal(TOTAL_PARAMS, 185);
  assert.equal(TOTAL_PARAMS, LAYER_COUNT * PATCH_COUNT + GLOBAL_COUNT);
  assert.ok(TOTAL_PARAMS <= 0xffff, "must fit the u16 paramIdx field");
  // Forward mappings line up with the ranges.
  assert.equal(patchClapId(LAYER_L1, 0), 0);
  assert.equal(patchClapId(LAYER_L1, 74), 74);
  assert.equal(patchClapId(LAYER_L2, 0), 75);
  assert.equal(patchClapId(LAYER_L2, 74), 149);
  assert.equal(globalClapId(0), 150);
  assert.equal(globalClapId(34), 184); // last id == TOTAL - 1
});

test("matrix addresses round-trip for every layer, slot and field", () => {
  for (const layer of [LAYER_L1, LAYER_L2]) {
    for (let slot = 0; slot < MATRIX_SLOTS; slot++) {
      for (const field of [
        MATRIX_FIELD_SOURCE,
        MATRIX_FIELD_DEST,
        MATRIX_FIELD_CURVE,
        MATRIX_FIELD_SCALE_SRC,
      ]) {
        const addr = packMatrixAddr(layer, slot, field);
        assert.deepEqual(unpackMatrixAddr(addr), { layer, slot, field });
      }
    }
  }
});

// Dropping rather than clamping matters: a clamped address lands the edit on a
// real slot the sender never aimed at, silently rewiring a patch.
test("out-of-range matrix addresses unpack to null, never a nearby slot", () => {
  assert.equal(unpackMatrixAddr(packMatrixAddr(2, 0, 0)), null, "layer 2");
  assert.equal(unpackMatrixAddr(0x0001 | (4 << 8) | (0xf << 12)), null, "layer 15");
  assert.equal(unpackMatrixAddr(packMatrixAddr(0, 0, 4)), null, "field 4");
  assert.equal(unpackMatrixAddr(packMatrixAddr(0, 0, 255)), null, "field 255");
});

test("encoding an unknown event type throws rather than emitting a blank slot", () => {
  assert.throws(() => encode({ type: 99, offset: 0 }), /unknown event type/);
});

// ── Cross-language contract ────────────────────────────────────────────────
//
// Everything above asserts the JS encoder against the JS table, and codec.rs
// asserts the Rust encoder against the Rust table. Both can pass while the two
// TABLES disagree — a transcription slip would then ship as a silent
// mis-routing, which is precisely the failure mode 0285 taught us not to leave
// to review.
//
// So: parse the golden rows straight out of codec.rs and compare them to this
// file's. Reading Rust source from a JS test is admittedly crude, but the
// alternative is a build step to export the table, and the contract is worth
// more than the elegance.
test("the Rust golden table and this one are byte-identical", async () => {
  const { readFileSync } = await import("node:fs");
  const { fileURLToPath } = await import("node:url");
  const src = readFileSync(fileURLToPath(new URL("../src/codec.rs", import.meta.url)), "utf8");

  const from = src.indexOf("fn golden()");
  const to = src.indexOf("fn encode_matches_the_golden_table");
  assert.ok(from > 0 && to > from, "could not locate codec.rs's golden table");

  // ("label", Event::Variant { .. }, [ 16 bytes ]) — the label and the array.
  const re = /"([^"]+)",\s*\n\s*Event::[^\n]*(?:\n(?!\s*\[)[^\n]*)*\n\s*\[([^\]]+)\]/g;
  const rust = new Map();
  for (const [, label, arr] of src.slice(from, to).matchAll(re)) {
    rust.set(
      label,
      arr
        .split(",")
        .map((v) => v.trim())
        .filter(Boolean)
        .map((v) => (v.startsWith("0x") ? parseInt(v, 16) : Number(v))),
    );
  }

  // A parse that finds the wrong number of rows means this test can no longer
  // verify the contract — that is a failure, not something to shrug past.
  assert.equal(
    rust.size,
    GOLDEN.length,
    `parsed ${rust.size} rows from codec.rs but this table has ${GOLDEN.length} — ` +
      "either a row is missing on one side, or the parser above needs updating",
  );

  for (const [label, event] of GOLDEN) {
    const theirs = rust.get(label);
    assert.ok(theirs, `codec.rs has no golden row labelled "${label}"`);
    assert.deepEqual(Array.from(encode(event)), theirs, `golden rows differ for ${label}`);
  }
});
