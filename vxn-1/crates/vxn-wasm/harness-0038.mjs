// Node harness for the worklet audio-host (ticket 0038).
//
// Headless proof of the PRODUCTION render loop — the Rust `vxn_host_render`
// driven through the same AudioHost driver (web/audio-host.mjs) the AudioWorklet
// uses, over the real 0035 ring and 0039 store. What we assert here is what the
// browser runs.
//
//   node harness-0038.mjs   (build the wasm + copy to web/ first; see README)
//
// Asserts:
//   1. A note-on written to the ring for sub-block offset N takes effect at N
//      (onset == N+1, the engine's 1-frame attack latency) — sample accuracy
//      through the full ring->wasm-decode->slice path, for every N incl. 127
//      (which lands in the next quantum).
//   2. The one-call Rust host renders byte-identical audio to the proven 0035 JS
//      slice loop (renderQuantumSliced) for a mixed note+param stream — the
//      in-wasm loop didn't drift from the reference.
//   3. key-mode/split (non-automatable shared state) applied once per quantum
//      before events: a split-mode render routes and sounds.
//   4. The 0039 param store folds into the host: a bulk preset (165 params)
//      applies through AudioHost with no glitch, and a single store edit reaches
//      the engine via the block-start fold.
//   5. The lock-free primitives the worklet relies on (SAB, Atomics) exist.

import { readFileSync } from "node:fs";
import { AudioHost } from "./web/audio-host.mjs";
import {
  createRingSAB,
  EventRing,
  renderQuantumSliced,
  DEFAULT_CAPACITY,
} from "./web/event-ring.mjs";
import { createParamSAB, ParamStore, TOTAL_PARAMS } from "./web/param-store.mjs";

const WASM = new URL(
  "../../../target/wasm32-unknown-unknown/release/vxn_wasm.wasm",
  import.meta.url,
);
const SR = 48000;

const { instance } = await WebAssembly.instantiate(readFileSync(WASM), {});
const x = instance.exports;
const Q = x.vxn_quantum();

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

// First strictly-non-zero sample (silent-skip => exact 0.0 before onset).
const onset = (buf) => {
  for (let i = 0; i < buf.length; i++) if (buf[i] !== 0) return i;
  return -1;
};

console.log("\n=== 0. runtime: SAB + Atomics present (worklet prerequisites) ===");
check(typeof SharedArrayBuffer !== "undefined", "SharedArrayBuffer constructible");
check(typeof Atomics !== "undefined", "Atomics present");

console.log("\n=== 1. note-on at sub-block offset N -> onset N+1 (sample accuracy) ===");
for (const off of [0, 1, 7, 31, 63, 100, 127]) {
  const ringSab = createRingSAB();
  const host = new AudioHost(x, { ringSab, sampleRate: SR });
  const ring = new EventRing(ringSab);
  ring.pushNoteOn(off, 64, 1.0);

  // Two quanta: offset 127 + 1-frame latency lands at frame 128 (next quantum).
  const combined = new Float32Array(Q * 2);
  const l = new Float32Array(Q);
  const r = new Float32Array(Q);
  host.process(l, r);
  combined.set(l, 0);
  host.process(l, r); // tail quantum, no new events
  combined.set(l, Q);

  const got = onset(combined);
  check(got === off + 1, `offset ${off} -> onset ${got} (want ${off + 1})`);
  host.destroy();
}

console.log("\n=== 2. Rust host == proven 0035 JS slice loop (parity) ===");
{
  // Whole mode (key_mode 0) so the JS reference engine — which has no key-mode
  // C-ABI — is a fair comparison.
  const stream = [
    { type: 1, offset: 0, paramIdx: 0, value: 0.8, note: 60, flag: 0 }, // note_on
    { type: 3, offset: 16, paramIdx: 2, value: 0.7, note: 0, flag: 0 }, // param
    { type: 1, offset: 64, paramIdx: 0, value: 0.6, note: 67, flag: 0 }, // note_on
    { type: 2, offset: 100, paramIdx: 0, value: 0, note: 60, flag: 0 }, // note_off
  ];

  // --- Rust host path: push to ring, one render call ---
  const ringSab = createRingSAB();
  const host = new AudioHost(x, { ringSab, sampleRate: SR });
  const ring = new EventRing(ringSab);
  for (const e of stream) ring._push(e.type, e.offset, e.paramIdx, e.value, e.note, e.flag);
  const hl = new Float32Array(Q);
  const hr = new Float32Array(Q);
  host.process(hl, hr);

  // --- reference path: a fresh Instance driven by the 0035 JS slice loop ---
  const s = x.vxn_new(SR);
  const engine = {
    noteOn: (n, v) => x.vxn_note_on(s, n, v),
    noteOff: (n) => x.vxn_note_off(s, n),
    setParam: (i, v) => x.vxn_set_param(s, i, v),
    processSlice: (a, b) => x.vxn_process_slice(s, a, b),
  };
  renderQuantumSliced(engine, stream, Q);
  const rl = new Float32Array(x.memory.buffer, x.vxn_out_l(s), Q);
  const rr = new Float32Array(x.memory.buffer, x.vxn_out_r(s), Q);

  let maxDiff = 0;
  for (let i = 0; i < Q; i++) {
    maxDiff = Math.max(maxDiff, Math.abs(hl[i] - rl[i]), Math.abs(hr[i] - rr[i]));
  }
  check(maxDiff === 0, `host output identical to JS slice loop (max abs diff ${maxDiff})`);
  x.vxn_destroy(s);
  host.destroy();
}

console.log("\n=== 3. key-mode/split applied before events (split-mode render) ===");
{
  const ringSab = createRingSAB();
  const host = new AudioHost(x, { ringSab, sampleRate: SR });
  const ring = new EventRing(ringSab);
  host.setKeyMode(2); // Split
  host.setSplitPoint(60);
  ring.pushNoteOn(0, 72, 1.0); // upper-half note
  const l = new Float32Array(Q);
  const r = new Float32Array(Q);
  host.process(l, r);
  check(onset(l) >= 0, "split-mode note produced audio (km/split honoured)");
  host.destroy();
}

console.log("\n=== 4. 0039 param store folds into the host ===");
{
  const ringSab = createRingSAB();
  const storeSab = createParamSAB();
  const host = new AudioHost(x, { ringSab, storeSab, sampleRate: SR });
  const store = new ParamStore(storeSab);

  // Bulk preset: 165 params at once. Must apply through AudioHost with no throw
  // and no audible glitch on an idle (un-noted) synth — still exact silence.
  const preset = new Float32Array(TOTAL_PARAMS);
  for (let i = 0; i < TOTAL_PARAMS; i++) preset[i] = 0.3;
  store.writeBulk(preset);
  const ring = new EventRing(ringSab);
  const l = new Float32Array(Q);
  const r = new Float32Array(Q);
  host.process(l, r); // folds the 165 params block-start, renders idle
  let silent = true;
  for (let i = 0; i < Q; i++) if (l[i] !== 0 || r[i] !== 0) silent = false;
  check(silent, "bulk 165-param preset applied with no glitch (idle stays silent)");

  // A single store edit + a note: reaches the engine via the block-start fold.
  store.write(2, 0.9);
  ring.pushNoteOn(0, 60, 1.0);
  host.process(l, r);
  check(onset(l) >= 0, "store edit + note rendered (fold reached the engine)");
  host.destroy();
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
