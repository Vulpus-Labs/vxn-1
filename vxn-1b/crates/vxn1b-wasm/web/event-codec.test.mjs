// Headless test for the JS event codec (0287, retargeted by 0312).
//
// The golden table below is a transcription of the Rust one in
// `src/codec.rs::tests::golden`. That duplication is the point: two independent
// hand-written tables that must agree byte-for-byte, so a layout change in
// either language fails here instead of silently mis-routing events at runtime.
//
// WHAT THIS PROVES CHANGED IN 0312. It used to check an `encode` wrapper that
// nothing called, against a `decode` that nothing called either; the bytes the
// worklet actually saw came from `EventRing._push`, which no golden table
// touched. Now `_push` writes through `encodeInto` and the table below drives
// that same function, so the encoder under test is the encoder that ships. The
// decode direction is checked once, in Rust, by the half that does the decoding.
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
  encodeInto,
  ev,
  packMatrixAddr,
  unpackMatrixAddr,
  MATRIX_SLOTS,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_POLARITY,
  MATRIX_FIELD_SCALE_SRC,
  MATRIX_FIELD_SHAPE,
  MATRIX_FIELD_SCALE_SHAPE,
  MATRIX_FIELD_ENABLED,
  MATRIX_FIELD_SCALE_POLARITY,
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

/// Encode one event the way the ring does — into a slot of a larger buffer,
/// at a non-zero base — and hand back just those 16 bytes. `base = SLOT_BYTES`
/// rather than 0 so a `base`-ignoring write would fail here rather than only in
/// a wrapped ring.
function slotBytes(event) {
  const buf = new Uint8Array(SLOT_BYTES * 2);
  encodeInto(new DataView(buf.buffer), SLOT_BYTES, event);
  return Array.from(buf.subarray(SLOT_BYTES));
}

/// One 16-byte row: type, offset, paramIdx (u16 LE), value (f32 LE), note, flag,
/// then seq (u16) + reserved (f32). Both are zero out of the codec — `seq` is
/// the RING's to stamp, after `encodeInto` returns.
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

test("encodeInto matches the golden bytes (== the Rust golden table)", () => {
  for (const [label, event, expected] of GOLDEN) {
    assert.deepEqual(slotBytes(event), expected, `encode mismatch for ${label}`);
  }
});

// Every unused field must be written, not merely left alone: the ring reuses
// slots, so a field the codec skips would carry the previous event's bytes
// round the wrap.
test("encodeInto overwrites all 16 bytes, leaving nothing from a prior event", () => {
  const buf = new Uint8Array(SLOT_BYTES * 2).fill(0xaa);
  const view = new DataView(buf.buffer);
  for (const [label, event, expected] of GOLDEN) {
    encodeInto(view, SLOT_BYTES, event);
    assert.deepEqual(Array.from(buf.subarray(SLOT_BYTES)), expected, `stale bytes after ${label}`);
    assert.ok(
      buf.subarray(0, SLOT_BYTES).every((b) => b === 0xaa),
      `${label} wrote outside its slot`,
    );
    buf.fill(0xaa);
  }
});

// The reserved tag has no encoder — the assertion that it (and any unknown tag)
// decodes to nothing lives with the decoder, in `src/codec.rs`.
test("the reserved tag is not encodable", () => {
  assert.throws(() => slotBytes({ type: EV_SUSTAIN_RESERVED, offset: 0 }), /unknown event type/);
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
        MATRIX_FIELD_POLARITY,
        MATRIX_FIELD_SCALE_SRC,
        MATRIX_FIELD_SHAPE,
        MATRIX_FIELD_SCALE_SHAPE,
        MATRIX_FIELD_ENABLED,
        MATRIX_FIELD_SCALE_POLARITY,
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
  // 7 is the last real field (`scale-polarity`); 8 is the first that does not
  // exist.
  assert.equal(unpackMatrixAddr(packMatrixAddr(0, 0, 8)), null, "field 8");
  assert.equal(unpackMatrixAddr(packMatrixAddr(0, 0, 255)), null, "field 255");
});

test("encoding an unknown event type throws rather than emitting a blank slot", () => {
  assert.throws(() => slotBytes({ type: 99, offset: 0 }), /unknown event type/);
});

// ── Cross-language contract ────────────────────────────────────────────────
//
// Everything above asserts the JS encoder against the JS table, and codec.rs
// asserts its (test-only) encoder against the Rust table. Both can pass while
// the two TABLES disagree — a transcription slip would then ship as a silent
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
    assert.deepEqual(slotBytes(event), theirs, `golden rows differ for ${label}`);
  }
});
