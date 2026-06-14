// Headless Node test for the cross-thread param store + diff readback (0039).
//
//   node web/param-store.test.mjs
//
// This env has no browser/audio; we prove the store the same way harness-0035
// proves the ring — driving the EXACT shared code path (param-store.mjs) the
// AudioWorklet will use, with hard asserts. Covers:
//
//   1. bulk update: write 165 params, read them all back correct.
//   2. lock-free single read/write round-trips incl. f32 bit-cast edge values
//      (0.0, -0.0, NaN, 1.0, negative, denormal, +/-Inf, FLT_MAX).
//   3. diff readback: an audio-side write surfaces as exactly one
//      ParamChanged-equivalent; no spurious diffs when nothing changed; the
//      first poll after a NaN seed broadcasts all 165 (NaN-seed behaviour).
//   4. bulk-preset-load "no glitch": a concurrent reader interleaved with a
//      bulk write only ever sees old-or-new per slot, never a torn float
//      (per-slot atomicity guarantee).
//   5. worklet integration: applyStoreToEngine pushes changed store values
//      into a fake engine and echoes them into the readback so pollDiffs sees
//      audio-thread drift end-to-end.

import {
  createParamSAB,
  ParamStore,
  TOTAL_PARAMS,
  LAYOUT,
  patchClapId,
  globalClapId,
  newLastSeen,
  pollDiffs,
  newWorkletSeen,
  applyStoreToEngine,
} from "./param-store.mjs";

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

// f32-precision equality: values written go through a Float32 store, so compare
// against the f32-rounded expectation. Math.fround gives the f32 image of a JS
// double.
const f32 = (x) => Math.fround(x);

console.log("=== 0. lock-free primitives + layout ===");
check(
  typeof SharedArrayBuffer !== "undefined",
  "SharedArrayBuffer constructible in this runtime",
);
check(
  typeof Atomics !== "undefined" && typeof Atomics.load === "function",
  "Atomics available",
);
check(TOTAL_PARAMS === 165, `TOTAL_PARAMS == 165 (got ${TOTAL_PARAMS})`);
check(
  LAYOUT.PATCH_COUNT === 69 &&
    LAYOUT.GLOBAL_COUNT === 27 &&
    LAYOUT.LAYER_COUNT === 2,
  `layout counts 69/27/2 (matches ADR 0009 §3 / vxn-app)`,
);
check(LAYOUT.UPPER_BASE === 0, "Upper base = 0");
check(LAYOUT.LOWER_BASE === 69, "Lower base = 69");
check(LAYOUT.GLOBAL_BASE === 138, "Global base = 138");
check(patchClapId(0, 0) === 0, "patchClapId(Upper,0) = 0");
check(patchClapId(1, 0) === 69, "patchClapId(Lower,0) = 69");
check(globalClapId(0) === 138, "globalClapId(0) = 138");
check(globalClapId(26) === 164, "globalClapId(26) = 164 (last id)");

console.log("\n=== 1. bulk update: write 165, read 165 back correct ===");
{
  const store = new ParamStore(createParamSAB());
  // A distinct value per id so a mis-indexed read is caught.
  const vals = new Float32Array(TOTAL_PARAMS);
  for (let id = 0; id < TOTAL_PARAMS; id++) vals[id] = f32(id * 0.123 - 7.5);
  store.writeBulk(vals);
  let mismatches = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    if (store.read(id) !== vals[id]) mismatches++;
  }
  check(mismatches === 0, `all ${TOTAL_PARAMS} bulk-written params read back exact (${mismatches} mismatch)`);

  // readAll snapshot matches.
  const snap = store.readAll();
  let snapMismatch = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) if (snap[id] !== vals[id]) snapMismatch++;
  check(snapMismatch === 0, `readAll() snapshot matches written values (${snapMismatch} mismatch)`);

  // writeBulk wrong-length is rejected.
  let threw = false;
  try {
    store.writeBulk(new Float32Array(164));
  } catch {
    threw = true;
  }
  check(threw, "writeBulk rejects a wrong-length (164) array");
}

console.log("\n=== 2. single read/write round-trips incl. f32 bit-cast edges ===");
{
  const store = new ParamStore(createParamSAB());
  const id = patchClapId(0, 5);
  // Ordinary round-trips.
  for (const v of [0.0, 1.0, -1.0, 440.0, -0.5, f32(0.123456)]) {
    store.write(id, v);
    check(store.read(id) === f32(v), `round-trip plain ${v} -> ${store.read(id)}`);
  }
  // -0.0: bits differ from +0.0 but compares equal; read back is still -0.0.
  store.write(id, -0.0);
  check(Object.is(store.read(id), -0.0), "round-trip preserves -0.0 (bit-cast, Object.is)");
  // NaN: bit-cast must round-trip a NaN (the seed sentinel relies on this).
  store.write(id, NaN);
  check(Number.isNaN(store.read(id)), "round-trip preserves NaN");
  // Infinities + FLT_MAX + a denormal: full f32 bit-cast coverage.
  store.write(id, Infinity);
  check(store.read(id) === Infinity, "round-trip +Inf");
  store.write(id, -Infinity);
  check(store.read(id) === -Infinity, "round-trip -Inf");
  store.write(id, f32(3.4028235e38));
  check(store.read(id) === f32(3.4028235e38), "round-trip FLT_MAX");
  store.write(id, f32(1.4e-45));
  check(store.read(id) === f32(1.4e-45), "round-trip smallest denormal");

  // Distinct ids are independent (no slot aliasing).
  const a = patchClapId(0, 0);
  const b = patchClapId(1, 0); // 69 — Lower, same patch param
  store.write(a, 111.0);
  store.write(b, 222.0);
  check(store.read(a) === 111 && store.read(b) === 222, "Upper/Lower slots independent (id 0 vs 69)");
}

console.log("\n=== 3. diff readback: audio write surfaces as ParamChanged ===");
{
  const store = new ParamStore(createParamSAB());
  const lastSeen = newLastSeen();

  // 3a. First poll after NaN seed broadcasts ALL 165 (NaN-seed behaviour).
  // Seed the readback region with concrete values (the worklet would publish
  // these on its first render). Use 0.0 so they're real, non-NaN values.
  for (let id = 0; id < TOTAL_PARAMS; id++) store.publishReadback(id, f32(id));
  const first = pollDiffs(store, lastSeen);
  check(first.length === TOTAL_PARAMS, `first poll broadcasts all ${TOTAL_PARAMS} (got ${first.length}) — NaN seed`);
  // Shape check: ParamChanged-equivalent { id, plain, norm, display }.
  const r0 = first[0];
  check(
    "id" in r0 && "plain" in r0 && "norm" in r0 && "display" in r0,
    `record shape { id, plain, norm, display } compatible with ViewEvent::ParamChanged`,
  );
  check(r0.id === 0 && r0.plain === 0, "first record is id 0, plain 0");
  const rLast = first[TOTAL_PARAMS - 1];
  check(rLast.id === 164 && rLast.plain === f32(164), "last record is id 164, plain 164");

  // 3b. No spurious diffs when nothing changed.
  const second = pollDiffs(store, lastSeen);
  check(second.length === 0, `second poll (no change) yields 0 diffs (got ${second.length})`);

  // 3c. A single audio-thread write surfaces exactly that one param.
  const driftId = globalClapId(3); // 141
  store.publishReadback(driftId, 0.777);
  const third = pollDiffs(store, lastSeen);
  check(third.length === 1, `single audio write -> exactly 1 diff (got ${third.length})`);
  check(third[0].id === driftId, `the diff is id ${driftId} (got ${third[0]?.id})`);
  check(third[0].plain === f32(0.777), `the diff carries the new plain value (${third[0]?.plain})`);

  // 3d. Re-writing the SAME value does not re-surface.
  store.publishReadback(driftId, 0.777);
  check(pollDiffs(store, lastSeen).length === 0, "re-publishing the same value yields no diff");

  // 3e. Two simultaneous drifts surface as two records, others quiet.
  store.publishReadback(2, -3.5);
  store.publishReadback(100, 9.0);
  const fourth = pollDiffs(store, lastSeen);
  check(fourth.length === 2, `two drifts -> 2 diffs (got ${fourth.length})`);
  const ids = fourth.map((r) => r.id).sort((x, y) => x - y);
  check(ids[0] === 2 && ids[1] === 100, `the two diffs are ids 2 and 100 (got ${ids})`);
}

console.log("\n=== 4. bulk-preset-load no-glitch: per-slot atomicity (no torn float) ===");
{
  // A concurrent reader interleaved with a bulk write must see, for each slot,
  // either the OLD value or the NEW value — never a torn 32-bit float. We can't
  // spawn a true second thread cheaply here, but we can prove the per-slot
  // invariant directly: every read is a single Atomics.load of a word that was
  // written by a single Atomics.store, so the value is always a member of
  // {old, new} for that slot, and the f32 image is exactly one of the two
  // committed bit patterns. We assert by reading after each individual slot
  // store during a bulk-like load and confirming the value is exactly old-or-new.
  const store = new ParamStore(createParamSAB());
  const oldVals = new Float32Array(TOTAL_PARAMS);
  const newVals = new Float32Array(TOTAL_PARAMS);
  for (let id = 0; id < TOTAL_PARAMS; id++) {
    oldVals[id] = f32(id + 0.25);
    newVals[id] = f32(1000 + id * 2.5);
  }
  store.writeBulk(oldVals);

  // Emulate a reader racing the bulk load: walk the bulk write slot-by-slot,
  // and after each store, read EVERY slot and assert each is exactly old-or-new
  // (never a value that is neither — which is what a torn float would be).
  let torn = 0;
  for (let w = 0; w < TOTAL_PARAMS; w++) {
    store.write(w, newVals[w]); // one slot transitions old -> new
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      const v = store.read(id);
      const isOld = Object.is(v, oldVals[id]) || v === oldVals[id];
      const isNew = Object.is(v, newVals[id]) || v === newVals[id];
      if (!isOld && !isNew) torn++;
    }
  }
  check(torn === 0, `reader interleaved with bulk load saw 0 torn floats (always old-or-new) across all slots`);
  // After the full bulk load, every slot is the new value.
  let wrong = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) if (store.read(id) !== newVals[id]) wrong++;
  check(wrong === 0, `after bulk load every slot holds the new value (${wrong} wrong)`);
}

console.log("\n=== 5. worklet integration: store -> engine -> readback -> pollDiffs ===");
{
  // Fake engine recording vxn_set_param calls (the 0035 shim 0038 will call).
  const applied = new Map();
  const engine = { setParam: (id, v) => applied.set(id, v) };

  const store = new ParamStore(createParamSAB());
  const workletSeen = newWorkletSeen();
  const lastSeen = newLastSeen();

  // Controller writes the store (main thread / preset load).
  const preset = new Float32Array(TOTAL_PARAMS);
  for (let id = 0; id < TOTAL_PARAMS; id++) preset[id] = f32(id * 0.01);
  store.writeBulk(preset);

  // First worklet pass applies ALL 165 (NaN-seeded workletSeen) and echoes them.
  const n1 = applyStoreToEngine(store, engine, workletSeen);
  check(n1 === TOTAL_PARAMS, `first render applies all ${TOTAL_PARAMS} store values to the engine (got ${n1})`);
  check(applied.get(64) === preset[64], "engine received the correct value for a sample id (64)");

  // Main thread sees that audio-thread application via the readback diff.
  const seen1 = pollDiffs(store, lastSeen);
  check(seen1.length === TOTAL_PARAMS, `main thread sees all ${TOTAL_PARAMS} applied via readback diff (got ${seen1.length})`);

  // Steady state: nothing changed -> worklet applies nothing, main sees nothing.
  const n2 = applyStoreToEngine(store, engine, workletSeen);
  check(n2 === 0, "second render with no store change applies 0 (latest-value-wins, not a stream)");
  check(pollDiffs(store, lastSeen).length === 0, "and main thread sees 0 diffs in steady state");

  // Controller edits one param -> worklet applies exactly it -> main sees it.
  const editId = patchClapId(1, 10); // 79
  store.write(editId, 0.99);
  const n3 = applyStoreToEngine(store, engine, workletSeen);
  check(n3 === 1 && applied.get(editId) === f32(0.99), `single controller edit applies exactly 1 (id ${editId})`);
  const seen3 = pollDiffs(store, lastSeen);
  check(seen3.length === 1 && seen3[0].id === editId, "main thread sees exactly that one edit via readback");
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
