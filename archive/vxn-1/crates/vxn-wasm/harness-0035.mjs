// Node harness for the SAB event-ring spike (ticket 0035).
//
// Headless proof of the main->worklet transport + block-slicing, the way
// harness.mjs (0034) proved the render path. It drives the wasm through the
// EXACT shared code path the AudioWorklet uses (web/event-ring.mjs) — same
// ring, same drain, same slice loop — so what we measure here is what the
// browser runs.
//
//   node harness-0035.mjs
//
// Proves, with hard asserts:
//   1. A note-on written to the ring for sub-block offset N takes measurable
//      effect at offset N (first non-zero output sample == N), for every N.
//   2. Quantifies the jitter/latency difference vs the apply-at-block-start
//      path (the postMessage-equivalent), per offset.
//   3. A dense param + note stream survives drain with zero dropped/reordered
//      records under the block-writer overflow policy (seq continuity check).
//   4. The lock-free primitives (Atomics, SharedArrayBuffer) the worklet relies
//      on are present in this runtime.

import { readFileSync } from "node:fs";
import {
  createRingSAB,
  EventRing,
  renderQuantumSliced,
  renderQuantumBlockStart,
  DEFAULT_CAPACITY,
} from "./web/event-ring.mjs";

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

// Build a fresh synth + engine facade (same shape the worklet constructs).
function newEngine() {
  const s = x.vxn_new(SR);
  return {
    handle: s,
    noteOn: (n, v) => x.vxn_note_on(s, n, v),
    noteOff: (n) => x.vxn_note_off(s, n),
    setParam: (i, val) => x.vxn_set_param(s, i, val),
    processSlice: (a, b) => x.vxn_process_slice(s, a, b),
    destroy: () => x.vxn_destroy(s),
  };
}

// Read the current quantum's L buffer out of linear memory after rendering.
function quantumL(engine) {
  const off = x.vxn_out_l(engine.handle) >>> 2;
  return new Float32Array(x.memory.buffer).slice(off, off + Q);
}

// Index of the first sample whose |value| crosses a tiny threshold, or -1.
// First sample that departs from silence. The engine's silent-skip fast path
// drives un-noted output to EXACTLY 0.0 (see vxn1-silent-skip-filter-state), so
// the true onset is the first strictly-non-zero frame — no arbitrary threshold,
// which matters at a slice boundary where the amp envelope ramps up from 0
// through sub-1e-7 values for the first handful of samples.
function firstNonZero(buf) {
  for (let i = 0; i < buf.length; i++) if (buf[i] !== 0) return i;
  return -1;
}

// The amp envelope STARTS at exactly 0.0 on the attack and ramps up, so the
// first audibly non-zero sample lands one engine-fixed latency AFTER the frame
// the note-on was applied. We measure that latency ONCE (apply at frame 0, find
// first non-zero) and assert every offset reproduces it — that is the real
// sample-accuracy proof: a constant delay relative to the applied offset, NOT a
// per-offset-varying one. (A block-start delivery, by contrast, collapses the
// delay to ~constant near 0 regardless of intended offset — that's the jitter.)
// This is identical behaviour to the CLAP shell; the engine is unchanged.
function measureEnvLatency() {
  const eng = newEngine();
  eng.noteOn(69, 1.0);
  eng.processSlice(0, Q);
  const lat = firstNonZero(quantumL(eng));
  eng.destroy();
  return lat < 0 ? 0 : lat; // frames between apply and first audible sample
}
const ENV_LATENCY = measureEnvLatency();

// Render TWO quanta given a record list, via the SHARED slice loop, through a
// real ring (write -> drain -> slice) so the transport is exercised end-to-end.
// Two quanta so an event near the block tail (offset 127) still has its onset
// captured: returns one buffer of length 2*Q with absolute frame indices.
function renderViaRing(engine, records, renderFn) {
  const sab = createRingSAB(DEFAULT_CAPACITY);
  const ring = new EventRing(sab, DEFAULT_CAPACITY);
  for (const r of records) {
    const ok =
      r.kind === "param"
        ? ring.pushParam(r.offset, r.idx, r.value)
        : ring.pushNoteOn(r.offset, r.note, r.velocity);
    if (!ok) throw new Error("ring overflow during setup (should not happen)");
  }
  const drained = ring.drainInto([]);
  renderFn(engine, drained, Q); // quantum 1: events applied here
  const q1 = quantumL(engine);
  renderFn(engine, [], Q); // quantum 2: tail, no new events
  const q2 = quantumL(engine);
  const both = new Float32Array(2 * Q);
  both.set(q1, 0);
  both.set(q2, Q);
  return both;
}

console.log("=== 0. lock-free primitives present ===");
check(typeof SharedArrayBuffer !== "undefined", "SharedArrayBuffer constructible in this runtime");
check(typeof Atomics !== "undefined" && typeof Atomics.load === "function", "Atomics available");
// crossOriginIsolated is a browser global; in Node it is undefined and SAB is
// unconditionally available. The browser path is proven by serve-coep.mjs.
console.log(`  note: crossOriginIsolated is a browser concept; Node grants SAB unconditionally.`);

console.log(`\n  (engine amp-envelope latency measured = ${ENV_LATENCY} frame; onset = applied-offset + ${ENV_LATENCY})`);

console.log("\n=== 1. sample-accurate onset: note-on @ offset N lands at N ===");
console.log("    (sliced path — the CLAP-parity loop)");
const offsets = [0, 1, 7, 16, 31, 63, 64, 100, 127];
let maxSlicedErr = 0;
for (const N of offsets) {
  const eng = newEngine();
  const buf = renderViaRing(eng, [{ kind: "note", offset: N, note: 69, velocity: 1.0 }], renderQuantumSliced);
  const onset = firstNonZero(buf);
  const expected = N + ENV_LATENCY; // sample-accurate: constant delay off N
  const err = onset < 0 ? 2 * Q : Math.abs(onset - expected);
  maxSlicedErr = Math.max(maxSlicedErr, err);
  check(onset === expected, `offset ${String(N).padStart(3)} -> onset ${String(onset).padStart(3)} (want ${expected})  err ${err} frames`);
  eng.destroy();
}
check(maxSlicedErr === 0, `max sliced onset error across all offsets = ${maxSlicedErr} frames (expect 0 — sample-accurate)`);

console.log("\n=== 2. jitter vs apply-at-block-start (the postMessage-equivalent) ===");
console.log("    block-start applies every event at frame 0 regardless of offset.");
let maxBlockErr = 0;
let sumBlockErr = 0;
for (const N of offsets) {
  const eng = newEngine();
  const buf = renderViaRing(eng, [{ kind: "note", offset: N, note: 69, velocity: 1.0 }], renderQuantumBlockStart);
  const onset = firstNonZero(buf);
  const expected = N + ENV_LATENCY; // where a sample-accurate path would land it
  const err = onset < 0 ? 2 * Q : Math.abs(onset - expected); // intended N, got ~0
  maxBlockErr = Math.max(maxBlockErr, err);
  sumBlockErr += err;
  console.log(`  offset ${String(N).padStart(3)} -> onset ${String(onset).padStart(3)} (want ${expected})  err ${err} frames`);
  eng.destroy();
}
const avgBlockErr = sumBlockErr / offsets.length;
const us = (frames) => ((frames / SR) * 1e6).toFixed(1);
console.log(`\n  sliced     : max onset error = ${maxSlicedErr} frames (${us(maxSlicedErr)} us) — sample-accurate`);
console.log(`  block-start: max onset error = ${maxBlockErr} frames (${us(maxBlockErr)} us), avg ${avgBlockErr.toFixed(1)} frames (${us(avgBlockErr)} us)`);
console.log(`  worst-case quantum jitter (block-start) = up to ${Q - 1} frames = ${us(Q - 1)} us @ ${SR} Hz`);
check(maxSlicedErr < maxBlockErr, "sliced path is strictly tighter than block-start");

console.log("\n=== 3. dense stream: no dropped/reordered events (block-writer policy) ===");
// Pump many quanta of a dense param + note stream through the ring and drain,
// asserting the producer sequence numbers arrive contiguous (none dropped, in
// order). Also force an overflow and confirm block-writer refuses rather than
// dropping silently.
{
  const sab = createRingSAB(DEFAULT_CAPACITY);
  const ring = new EventRing(sab, DEFAULT_CAPACITY);
  const eng = newEngine();
  const drained = [];
  let totalSeen = 0;
  let expectedSeq = 0;
  let seqBreaks = 0;
  const QUANTA = 2000;
  const EV_PER_Q = 24; // dense: 24 events/quantum (notes + params interleaved)
  for (let q = 0; q < QUANTA; q++) {
    for (let e = 0; e < EV_PER_Q; e++) {
      const offset = (e * 5) % Q;
      const ok =
        e % 3 === 0
          ? ring.pushNoteOn(offset, 60 + (e % 12), 0.7)
          : ring.pushParam(offset, e % 165, (e * 0.013) % 1.0);
      if (!ok) throw new Error("unexpected overflow at normal density (ring undersized?)");
    }
    ring.drainInto(drained);
    // Records must be offset-sorted for the slice loop; the producer wrote them
    // in offset order within the quantum, so verify that invariant too.
    for (let i = 1; i < drained.length; i++) {
      if (drained[i].offset < drained[i - 1].offset) seqBreaks++;
    }
    for (const r of drained) {
      if (r.seq !== (expectedSeq & 0xffff)) seqBreaks++;
      expectedSeq++;
      totalSeen++;
    }
    renderQuantumSliced(eng, drained, Q);
  }
  check(totalSeen === QUANTA * EV_PER_Q, `drained ${totalSeen} / ${QUANTA * EV_PER_Q} events (none dropped)`);
  check(seqBreaks === 0, `sequence + offset-order continuity breaks = ${seqBreaks} (expect 0)`);
  eng.destroy();
}

console.log("\n=== 3b. forced overflow: block-writer refuses, never drops ===");
{
  const sab = createRingSAB(DEFAULT_CAPACITY);
  const ring = new EventRing(sab, DEFAULT_CAPACITY);
  let accepted = 0;
  let refused = 0;
  // Write 2x capacity WITHOUT draining -> ring must fill, then refuse.
  for (let i = 0; i < DEFAULT_CAPACITY * 2; i++) {
    if (ring.pushNoteOn(0, 60, 0.5)) accepted++;
    else refused++;
  }
  check(accepted === DEFAULT_CAPACITY, `accepted exactly capacity (${accepted} == ${DEFAULT_CAPACITY}) before blocking`);
  check(refused === DEFAULT_CAPACITY, `refused the overflow (${refused}) — block-writer, no silent drop`);
  // After draining, the refused-then-retried writes succeed: nothing lost.
  const got = ring.drainInto([]).length;
  check(got === DEFAULT_CAPACITY, `drained all ${got} buffered records intact`);
  const retry = ring.pushNoteOn(0, 60, 0.5);
  check(retry === true, "writer can push again after the consumer drained (retry path works)");
}

console.log("\n=== 4. throughput: sliced render of a dense stream vs realtime ===");
{
  const eng = newEngine();
  const sab = createRingSAB(DEFAULT_CAPACITY);
  const ring = new EventRing(sab, DEFAULT_CAPACITY);
  const drained = [];
  const seconds = 5;
  const quanta = Math.ceil((seconds * SR) / Q);
  eng.noteOn(69, 1.0); // a held voice so process() does real work
  const t0 = process.hrtime.bigint();
  for (let q = 0; q < quanta; q++) {
    for (let e = 0; e < 8; e++) ring.pushParam((e * 16) % Q, e % 165, (q * 0.001) % 1);
    ring.drainInto(drained);
    renderQuantumSliced(eng, drained, Q);
  }
  const wall = Number(process.hrtime.bigint() - t0) / 1e9;
  console.log(`  rendered ${seconds}s of audio (8 sliced events/quantum) in ${wall.toFixed(3)}s wall => ${(seconds / wall).toFixed(1)}x realtime`);
  eng.destroy();
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
