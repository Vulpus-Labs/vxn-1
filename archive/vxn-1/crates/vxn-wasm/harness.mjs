// Node harness for the WASM spike (ticket 0034). Instantiates the raw
// cdylib exactly as an AudioWorklet would, plays one note, and checks the
// rendered buffers are audible. Also times render throughput vs realtime
// for a sustained note (denormal/CPU sanity).
//
// Run: node harness.mjs
import { readFileSync } from "node:fs";

const WASM = new URL(
  "../../../target/wasm32-unknown-unknown/release/vxn_wasm.wasm",
  import.meta.url,
);
const SR = 48000;

const { instance } = await WebAssembly.instantiate(readFileSync(WASM), {});
const x = instance.exports;
const mem = () => new Float32Array(x.memory.buffer);

const Q = x.vxn_quantum();
const synth = x.vxn_new(SR);

// Read a quantum out of linear memory after a process() call.
function renderQuantum() {
  x.vxn_process(synth);
  const lOff = x.vxn_out_l(synth) >>> 2; // byte ptr -> f32 index
  const rOff = x.vxn_out_r(synth) >>> 2;
  const m = mem();
  return [m.slice(lOff, lOff + Q), m.slice(rOff, rOff + Q)];
}

function peak(buf) {
  let p = 0;
  for (const s of buf) p = Math.max(p, Math.abs(s));
  return p;
}

// --- 1. silence before note-on ---
let [l0] = renderQuantum();
console.log(`pre-note peak:      ${peak(l0).toExponential(2)} (expect ~0)`);

// --- 2. note-on, render ~0.5s, confirm audible ---
x.vxn_note_on(synth, 69, 1.0); // A4, full velocity
let maxPeak = 0;
const quanta = Math.ceil((0.5 * SR) / Q);
for (let i = 0; i < quanta; i++) {
  const [l, r] = renderQuantum();
  maxPeak = Math.max(maxPeak, peak(l), peak(r));
}
console.log(`note-on peak (0.5s): ${maxPeak.toFixed(4)} (expect > 0.01)`);
console.log(maxPeak > 0.01 ? "AUDIBLE — synth renders sound ✓" : "SILENT — FAIL ✗");

// --- 3. throughput: sustained note, 5s of audio, wall-clock ratio ---
const renderSeconds = 5.0;
const totalQuanta = Math.ceil((renderSeconds * SR) / Q);
const t0 = process.hrtime.bigint();
for (let i = 0; i < totalQuanta; i++) x.vxn_process(synth);
const t1 = process.hrtime.bigint();
const wall = Number(t1 - t0) / 1e9;
console.log(
  `\nsustained: rendered ${renderSeconds}s audio in ${wall.toFixed(3)}s wall ` +
    `=> ${(renderSeconds / wall).toFixed(1)}x realtime (1 voice)`,
);

// --- 4. denormal probe: note-off, window the decay tail per-second.
// A denormal cliff would show as a slow window mid-decay (signal tiny but
// nonzero, before any silent-skip fast path engages).
x.vxn_note_off(synth, 69);
console.log(`\ndecay tail per-second (FTZ absent on wasm):`);
const qPerSec = Math.ceil(SR / Q);
for (let s = 0; s < 12; s++) {
  const w0 = process.hrtime.bigint();
  let p = 0;
  for (let i = 0; i < qPerSec; i++) {
    const [l] = renderQuantum();
    p = Math.max(p, peak(l));
  }
  const w1 = process.hrtime.bigint();
  const wall = Number(w1 - w0) / 1e9;
  console.log(
    `  t=${s}s  peak=${p.toExponential(2)}  ${(1.0 / wall).toFixed(0)}x realtime`,
  );
}

x.vxn_destroy(synth);
