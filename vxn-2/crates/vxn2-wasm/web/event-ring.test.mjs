// SPSC event-ring tests (ticket 0155). Run: node --test crates/vxn2-wasm/web/

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  EventRing,
  createRingSAB,
  SLOT_BYTES,
  EV_NOTE_ON,
  EV_PARAM,
} from "./event-ring.mjs";
import { encodeInto, decode, ev, PARAM_FLAG_NORM } from "./event-codec.mjs";

test("push then drainInto round-trips records in order with seq stamps", () => {
  const ring = new EventRing(createRingSAB(8), 8);
  ring.pushNoteOn(0, 60, 0.8);
  ring.pushParam(16, 5, 0.5);
  ring.pushNoteOff(100, 60);
  const out = [];
  ring.drainInto(out);
  assert.equal(out.length, 3);
  assert.equal(out[0].type, EV_NOTE_ON);
  assert.equal(out[0].note, 60);
  assert.ok(Math.abs(out[0].value - 0.8) < 1e-6);
  assert.equal(out[1].type, EV_PARAM);
  assert.equal(out[1].paramIdx, 5);
  assert.equal(out[2].offset, 100);
  // seq is monotonic from 0.
  assert.deepEqual(out.map((r) => r.seq), [0, 1, 2]);
});

test("pushMatrixRow drains a slot that decodes to the same row (0193)", () => {
  const ring = new EventRing(createRingSAB(8), 8);
  // slot 0, mod-env(4) -> cutoff(28), curve lin(0), active, depth 0.9.
  ring.pushMatrixRow(0, 4, 28, 0, true, 0.9);
  const dst = new Uint8Array(SLOT_BYTES);
  assert.equal(ring.drainRawInto(dst), 1);
  const got = decode(new DataView(dst.buffer), 0);
  assert.equal(got.type, 11); // EV_MATRIX_ROW
  assert.equal(got.slot, 0);
  assert.equal(got.source, 4);
  assert.equal(got.dest, 28);
  assert.equal(got.curve, 0);
  assert.equal(got.active, true);
  assert.ok(Math.abs(got.depth - 0.9) < 1e-6);
});

test("drain reclaims slots — ring is empty afterward", () => {
  const ring = new EventRing(createRingSAB(8), 8);
  ring.pushNoteOn(0, 60, 1.0);
  assert.equal(ring.pending(), 1);
  ring.drainInto([]);
  assert.equal(ring.pending(), 0);
});

test("block-writer: push returns false when full, never overwrites", () => {
  const cap = 4;
  const ring = new EventRing(createRingSAB(cap), cap);
  for (let i = 0; i < cap; i++) assert.equal(ring.pushNoteOn(0, 60 + i, 1.0), true);
  assert.equal(ring.pushNoteOn(0, 99, 1.0), false); // full -> block
  const out = [];
  ring.drainInto(out);
  assert.equal(out.length, cap);
  assert.deepEqual(out.map((r) => r.note), [60, 61, 62, 63]); // the 99 never landed
});

test("drainRawInto copies verbatim bytes and caps at destination capacity", () => {
  const ring = new EventRing(createRingSAB(8), 8);
  ring.pushNoteOn(3, 64, 1.0);
  ring.pushNoteOff(7, 64);
  // Destination sized for only ONE record.
  const dst = new Uint8Array(SLOT_BYTES);
  const n = ring.drainRawInto(dst);
  assert.equal(n, 1);
  // First record's bytes are the note-on: type at off 0, offset at off 1.
  assert.equal(dst[0], EV_NOTE_ON);
  assert.equal(dst[1], 3);
  // The second record stays for the next drain (graceful, not dropped).
  assert.equal(ring.pending(), 1);
  const dst2 = new Uint8Array(SLOT_BYTES);
  assert.equal(ring.drainRawInto(dst2), 1);
  assert.equal(dst2[1], 7); // note-off offset
});

test("drainRawInto handles wrap correctly", () => {
  const cap = 4;
  const ring = new EventRing(createRingSAB(cap), cap);
  // Fill, drain, refill so the read/write indices wrap past capacity.
  for (let i = 0; i < cap; i++) ring.pushNoteOn(i, 60 + i, 1.0);
  ring.drainInto([]);
  for (let i = 0; i < cap; i++) ring.pushNoteOn(i + 10, 70 + i, 1.0);
  const dst = new Uint8Array(SLOT_BYTES * cap);
  const n = ring.drainRawInto(dst);
  assert.equal(n, cap);
  // Offsets 10..13 in arrival order despite the physical wrap.
  for (let i = 0; i < cap; i++) assert.equal(dst[i * SLOT_BYTES + 1], i + 10);
});

test("pushEvent carries codec events (normalised param) and stamps seq", () => {
  const ring = new EventRing(createRingSAB(8), 8);
  ring.pushEvent(ev.setParamNorm(9, 0.75), encodeInto);
  const out = [];
  ring.drainInto(out);
  assert.equal(out.length, 1);
  assert.equal(out[0].type, EV_PARAM);
  assert.equal(out[0].paramIdx, 9);
  assert.equal(out[0].flag & PARAM_FLAG_NORM, PARAM_FLAG_NORM); // norm bit set
  assert.equal(out[0].seq, 0);
});
