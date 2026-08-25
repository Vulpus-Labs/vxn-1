// Headless test for the cross-thread param store (0287).
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/param-store.test.mjs

import test from "node:test";
import assert from "node:assert/strict";

import {
  ParamStore,
  createParamSAB,
  LAYOUT,
  STORE_BYTES,
  TOTAL_PARAMS,
  PATCH_COUNT,
  GLOBAL_COUNT,
  patchClapId,
  globalClapId,
  newWorkletSeen,
  applyStoreToEngine,
} from "./param-store.mjs";
import { LAYER_L1, LAYER_L2 } from "./event-codec.mjs";

const store = () => new ParamStore(createParamSAB());
const f32 = (x) => Math.fround(x);

test("the two-layer layout matches vxn1b-engine's id partition", () => {
  assert.equal(LAYOUT.PATCH_COUNT, 75);
  assert.equal(LAYOUT.GLOBAL_COUNT, 35);
  assert.equal(LAYOUT.LAYER_COUNT, 2);
  assert.equal(LAYOUT.TOTAL_PARAMS, 185);
  assert.equal(LAYOUT.L1_BASE, 0);
  assert.equal(LAYOUT.L2_BASE, PATCH_COUNT);
  assert.equal(LAYOUT.GLOBAL_BASE, 2 * PATCH_COUNT);
  // The three regions tile the id space exactly, with no gap and no overlap.
  assert.equal(LAYOUT.GLOBAL_BASE + GLOBAL_COUNT, TOTAL_PARAMS);
  assert.equal(patchClapId(LAYER_L2, 0), LAYOUT.L2_BASE);
  assert.equal(globalClapId(0), LAYOUT.GLOBAL_BASE);
});

// One direction only: there is no host in a browser, so nothing but the
// controller ever originates a param value and the audio->main readback vxn-1
// carries has nothing to report (0297).
test("the SAB carries exactly one word per param, no readback region", () => {
  assert.equal(STORE_BYTES, TOTAL_PARAMS * 4);
  assert.equal(createParamSAB().byteLength, STORE_BYTES);
});

test("a written value reads back as the same f32", () => {
  const s = store();
  s.write(5, 0.25);
  assert.equal(s.read(5), 0.25);
  // f32 precision, not f64: the store round-trips through 32-bit words.
  s.write(6, 0.1);
  assert.equal(s.read(6), f32(0.1));
});

test("writes to one id never disturb its neighbours", () => {
  const s = store();
  s.write(10, 1.0);
  s.write(11, 2.0);
  s.write(12, 3.0);
  s.write(11, 9.0);
  assert.deepEqual([s.read(10), s.read(11), s.read(12)], [1.0, 9.0, 3.0]);
});

test("a layer-1 write does not leak into its layer-2 twin", () => {
  const s = store();
  const l1 = patchClapId(LAYER_L1, 20);
  const l2 = patchClapId(LAYER_L2, 20);
  s.write(l1, 0.75);
  assert.equal(s.read(l2), 0, "the twin is a separate automation target");
  s.write(l2, 0.25);
  assert.equal(s.read(l1), 0.75);
});

test("two views over the same SAB see each other's writes", () => {
  const sab = createParamSAB();
  const main = new ParamStore(sab);
  const worklet = new ParamStore(sab);
  main.write(42, 0.5);
  assert.equal(worklet.read(42), 0.5);
});

test("writeBulk fills every slot and rejects a wrong-length array", () => {
  const s = store();
  const values = Float32Array.from({ length: TOTAL_PARAMS }, (_, i) => i);
  s.writeBulk(values);
  assert.equal(s.read(0), 0);
  assert.equal(s.read(TOTAL_PARAMS - 1), TOTAL_PARAMS - 1);
  assert.deepEqual(Array.from(s.readAll()), Array.from(values));

  assert.throws(() => s.writeBulk(new Float32Array(TOTAL_PARAMS - 1)), /expects 185 values/);
  assert.throws(() => s.writeBulk(new Float32Array(TOTAL_PARAMS + 1)), /expects 185 values/);
});

// The NaN seed is how the native pump forces a full broadcast on the first tick
// after the editor opens; without it a freshly-opened UI shows stale defaults
// for every control the user has not touched.

// The worklet-side fold. The mirror is what keeps a steady-state quantum from
// re-applying 185 unchanged params; the NaN seed is what makes the first one
// apply all of them.
test("the worklet fold applies every id once, then only what changed", () => {
  const s = store();
  const applied = [];
  const engine = { setParam: (id, v) => applied.push([id, v]) };
  const seen = newWorkletSeen();

  assert.equal(applyStoreToEngine(s, engine, seen), TOTAL_PARAMS, "first fold applies all");
  applied.length = 0;

  assert.equal(applyStoreToEngine(s, engine, seen), 0, "an unchanged store folds nothing");
  assert.deepEqual(applied, []);

  s.write(12, 0.5);
  assert.equal(applyStoreToEngine(s, engine, seen), 1);
  assert.deepEqual(applied, [[12, 0.5]]);
});

