// Headless test for the SPSC event ring (0287).
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/event-ring.test.mjs

import test from "node:test";
import assert from "node:assert/strict";

import { EventRing, createRingSAB, readSlot, SLOT_BYTES, CTRL_BYTES } from "./event-ring.mjs";
import {
  EV_NOTE_ON,
  EV_NOTE_OFF,
  EV_PARAM,
  EV_MATRIX_EDIT,
  EV_TEMPO,
  EV_POLY_PRESSURE,
  EV_LFO2_LINK,
  packMatrixAddr,
  unpackMatrixAddr,
  MATRIX_FIELD_SOURCE,
  PARAM_FLAG_NORM,
  LAYER_L2,
} from "./event-codec.mjs";

const ring = (capacity = 8) => new EventRing(createRingSAB(capacity), capacity);

test("capacity must be a power of two", () => {
  assert.throws(() => createRingSAB(3), /power of two/);
  assert.throws(() => new EventRing(createRingSAB(8), 3), /power of two/);
});

test("the SAB is sized for ctrl + capacity slots", () => {
  assert.equal(createRingSAB(8).byteLength, CTRL_BYTES + SLOT_BYTES * 8);
});

test("an empty ring reports nothing pending and drains nothing", () => {
  const r = ring();
  assert.equal(r.pending(), 0);
  assert.deepEqual(r.drainInto([]), []);
});

test("push then drain returns the records in order", () => {
  const r = ring();
  r.pushNoteOn(60, 1.0);
  r.pushNoteOn(64, 0.5, 4);
  r.pushNoteOff(60, 8);
  assert.equal(r.pending(), 3);

  const out = r.drainInto([]);
  assert.equal(out.length, 3);
  assert.deepEqual(
    out.map((x) => [x.type, x.offset, x.note]),
    [
      [EV_NOTE_ON, 0, 60],
      [EV_NOTE_ON, 4, 64],
      [EV_NOTE_OFF, 8, 60],
    ],
  );
  assert.equal(r.pending(), 0, "drain reclaims every slot it read");
});

test("the producer stamps a monotonic sequence so a drop would be detectable", () => {
  const r = ring();
  const first = r.peekSeq();
  r.pushNoteOn(60, 1.0);
  r.pushNoteOn(61, 1.0);
  const out = r.drainInto([]);
  assert.deepEqual(
    out.map((x) => x.seq),
    [first, first + 1],
  );
});

// BLOCK-WRITER, not drop-oldest: dropping would corrupt the slice loop with an
// unpaired note-off or a lost gesture-end, and would mask a dead audio thread.
test("a full ring refuses the push instead of dropping an event", () => {
  const r = ring(4);
  for (let i = 0; i < 4; i++) {
    assert.equal(r.pushNoteOn(60 + i, 1.0), true, `push ${i}`);
  }
  assert.equal(r.pushNoteOn(99, 1.0), false, "the fifth push must fail");
  assert.equal(r.pending(), 4, "and nothing already queued is lost");

  // Draining frees the whole ring again.
  r.drainInto([]);
  assert.equal(r.pushNoteOn(99, 1.0), true);
});

test("slot indices wrap without the records straddling the boundary", () => {
  const r = ring(4);
  // Three full laps through a 4-slot ring.
  for (let lap = 0; lap < 3; lap++) {
    for (let i = 0; i < 4; i++) assert.equal(r.pushNoteOn(60 + i, 1.0, i), true);
    const out = r.drainInto([]);
    assert.equal(out.length, 4, `lap ${lap} drained`);
    assert.deepEqual(
      out.map((x) => x.note),
      [60, 61, 62, 63],
      `lap ${lap} order`,
    );
  }
});

test("drainRawInto copies whole 16-byte slots, verbatim and in order", () => {
  const r = ring();
  r.pushNoteOn(60, 0.5, 7, 3);
  r.pushTempo(120);

  const dst = new Uint8Array(SLOT_BYTES * 8);
  assert.equal(r.drainRawInto(dst), 2);

  const a = readSlot(dst, 0);
  assert.equal(a.type, EV_NOTE_ON);
  assert.equal(a.offset, 7);
  assert.equal(a.note, 60);
  assert.equal(a.flag, 3, "the MIDI channel survives the raw drain");
  assert.equal(a.value, 0.5);

  const b = readSlot(dst, SLOT_BYTES);
  assert.equal(b.type, EV_TEMPO);
  assert.equal(b.value, 120);
});

// A too-small destination must degrade gracefully: the worklet's scratch is
// finite, and losing the overflow would be exactly the drop the block-writer
// policy exists to prevent.
test("drainRawInto reclaims only what it copied, leaving the rest queued", () => {
  const r = ring(8);
  for (let i = 0; i < 5; i++) r.pushNoteOn(60 + i, 1.0, i);

  const small = new Uint8Array(SLOT_BYTES * 2);
  assert.equal(r.drainRawInto(small), 2, "copies only what fits");
  assert.equal(r.pending(), 3, "the remaining three stay queued");

  const rest = new Uint8Array(SLOT_BYTES * 8);
  assert.equal(r.drainRawInto(rest), 3);
  assert.equal(readSlot(rest, 0).note, 62, "resumes where it left off");
});

test("raw drain wraps correctly across the slot boundary", () => {
  const r = ring(4);
  const dst = new Uint8Array(SLOT_BYTES * 4);
  // Advance the indices to just before the wrap.
  for (let i = 0; i < 3; i++) r.pushNoteOn(60, 1.0);
  r.drainRawInto(dst);
  // Now push across the boundary.
  r.pushNoteOn(70, 1.0, 1);
  r.pushNoteOn(71, 1.0, 2);
  assert.equal(r.drainRawInto(dst), 2);
  assert.equal(readSlot(dst, 0).note, 70);
  assert.equal(readSlot(dst, SLOT_BYTES).note, 71);
});

test("param pushes carry the plain/norm discriminant", () => {
  const r = ring();
  r.pushParam(5, 0.5);
  r.pushParamNorm(5, 0.25);
  const out = r.drainInto([]);
  assert.equal(out[0].type, EV_PARAM);
  assert.equal(out[0].flag & PARAM_FLAG_NORM, 0, "plain");
  assert.equal(out[1].flag & PARAM_FLAG_NORM, PARAM_FLAG_NORM, "norm");
});

test("a matrix edit packs the address the codec unpacks", () => {
  const r = ring();
  r.pushMatrixEdit(LAYER_L2, 5, MATRIX_FIELD_SOURCE, 9);
  const [rec] = r.drainInto([]);
  assert.equal(rec.type, EV_MATRIX_EDIT);
  assert.equal(rec.paramIdx, packMatrixAddr(LAYER_L2, 5, MATRIX_FIELD_SOURCE));
  assert.deepEqual(unpackMatrixAddr(rec.paramIdx), {
    layer: LAYER_L2,
    slot: 5,
    field: MATRIX_FIELD_SOURCE,
  });
  assert.equal(rec.flag, 9, "the value byte rides `flag`");
});

test("the non-param domain state has producers that do not touch the store", () => {
  const r = ring();
  r.pushKeyMode(2);
  r.pushSplitPoint(48);
  r.pushLfo2Link(true);
  r.pushScopeTap(1);
  r.pushPolyPressure(60, 0.75, 0, 3);
  const out = r.drainInto([]);
  assert.deepEqual(
    out.map((x) => x.flag),
    [2, 48, 1, 1, 3],
  );
  assert.equal(out[2].type, EV_LFO2_LINK);
  assert.equal(out[4].type, EV_POLY_PRESSURE);
  assert.equal(out[4].value, 0.75);
  assert.equal(out[4].note, 60);
});

test("two views over the same SAB see each other's writes", () => {
  const sab = createRingSAB(8);
  const producer = new EventRing(sab, 8);
  const consumer = new EventRing(sab, 8);
  producer.pushNoteOn(72, 1.0, 3, 2);
  assert.equal(consumer.pending(), 1);
  const [rec] = consumer.drainInto([]);
  assert.equal(rec.note, 72);
  assert.equal(rec.flag, 2);
  assert.equal(producer.pending(), 0, "the consumer's reclaim is visible to the producer");
});
