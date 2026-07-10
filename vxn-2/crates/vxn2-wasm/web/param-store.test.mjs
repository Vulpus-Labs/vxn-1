// Param-store tests (ticket 0155). Run: node --test crates/vxn2-wasm/web/

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ParamStore,
  createParamSAB,
  newLastSeen,
  pollDiffs,
  newWorkletSeen,
  applyStoreToHost,
  TOTAL_PARAMS,
} from "./param-store.mjs";

test("param count is vxn-2's flat 209", () => {
  assert.equal(TOTAL_PARAMS, 209);
});

test("write / read round-trips a plain f32 through the atomic bit-cast", () => {
  const store = new ParamStore(createParamSAB());
  store.write(5, 0.3333);
  assert.ok(Math.abs(store.read(5) - 0.3333) < 1e-6);
  store.write(TOTAL_PARAMS - 1, -6.0);
  assert.ok(Math.abs(store.read(TOTAL_PARAMS - 1) - -6.0) < 1e-6);
});

test("writeBulk sets every slot; wrong length throws", () => {
  const store = new ParamStore(createParamSAB());
  const vals = new Float32Array(TOTAL_PARAMS).map((_, i) => i * 0.01);
  store.writeBulk(vals);
  const all = store.readAll();
  for (let i = 0; i < TOTAL_PARAMS; i++) assert.ok(Math.abs(all[i] - i * 0.01) < 1e-4);
  assert.throws(() => store.writeBulk(new Float32Array(3)));
});

test("pollDiffs: NaN seed forces a full first broadcast, then only drift", () => {
  const store = new ParamStore(createParamSAB());
  const seen = newLastSeen();
  // Worklet echoes some applied values into the readback region.
  for (let i = 0; i < TOTAL_PARAMS; i++) store.publishReadback(i, 0);
  const first = pollDiffs(store, seen);
  assert.equal(first.length, TOTAL_PARAMS, "first poll broadcasts all params");
  // No further readback writes -> no drift.
  assert.equal(pollDiffs(store, seen).length, 0);
  // A single audio-thread write surfaces exactly one record.
  store.publishReadback(42, 0.9);
  const drift = pollDiffs(store, seen);
  assert.equal(drift.length, 1);
  assert.equal(drift[0].id, 42);
  assert.ok(Math.abs(drift[0].plain - 0.9) < 1e-6);
});

test("applyStoreToHost pushes only changed slots and echoes readback", () => {
  const store = new ParamStore(createParamSAB());
  const seen = newWorkletSeen();
  // Seed store with distinct values.
  for (let i = 0; i < TOTAL_PARAMS; i++) store.write(i, i * 0.001);

  const pushed = [];
  const host = { setParam: (id, v) => pushed.push([id, v]) };

  // First fold pushes ALL params (NaN seed).
  const n1 = applyStoreToHost(store, host, seen);
  assert.equal(n1, TOTAL_PARAMS);
  assert.equal(pushed.length, TOTAL_PARAMS);
  // Readback echoed so the main-thread pump can observe applied values.
  assert.ok(Math.abs(store.readReadback(10) - 0.01) < 1e-6);

  // No change -> second fold pushes nothing.
  pushed.length = 0;
  assert.equal(applyStoreToHost(store, host, seen), 0);
  assert.equal(pushed.length, 0);

  // Change one slot -> only that one pushes.
  store.write(7, 1.234);
  assert.equal(applyStoreToHost(store, host, seen), 1);
  assert.deepEqual(pushed, [[7, store.read(7)]]);
});
