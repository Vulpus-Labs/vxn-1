// 0036 param-store probe (Node prototype, throwaway).
//
// Compares the two cross-thread param-store mechanisms ADR 0009 must pick
// between, on the two cases the ticket calls out:
//   (i)  bulk preset load  — 165 params written at once (main thread)
//   (ii) audio->main diff readback — the `push_param_diffs` pump: scan the
//        store for audio-thread writes the controller never saw, emit a
//        ParamChanged for each drifted id.
//
// Option (a): SharedArrayBuffer of 165 Int32 atomics (f32 bit-cast),
//             indexed by CLAP id. Lock-free latest-value-wins. This is the
//             direct analogue of vxn-clap's `SharedParams`.
// Option (b): param-set events carried on the 0035-style SPSC byte ring
//             (one length-prefixed record per param).
//
// Node has SharedArrayBuffer + Atomics with no isolation headers, so this
// models the mechanics (not the browser COOP/COEP gate, which is 0035's job).

const N = 165;            // CLAP param count: 69*2 + 27
const ITERS = 100_000;    // repeat each scenario for a stable mean

// ---------------------------------------------------------------------------
// Option (a): SAB atomic array
// ---------------------------------------------------------------------------
const sab = new SharedArrayBuffer(N * 4);
const ai = new Int32Array(sab);           // atomic view (Int32 = f32 bits)
const af = new Float32Array(sab);         // convenience float view
const lastSeen = new Float32Array(N);     // main-thread mirror for the diff pump

function f2i(x) { const b = new Float32Array(1); b[0] = x; return new Int32Array(b.buffer)[0]; }

// (i) bulk preset load: write all 165 atomically (main thread writer)
function sab_bulkLoad(preset) {
  for (let i = 0; i < N; i++) Atomics.store(ai, i, preset[i]);
}

// (ii) diff readback: scan store vs lastSeen, collect drifted ids
function sab_diffReadback(out) {
  let n = 0;
  for (let i = 0; i < N; i++) {
    const bits = Atomics.load(ai, i);
    const v = (af[i]); // float reinterpret of the same word
    if (v !== lastSeen[i]) { lastSeen[i] = v; out[n++] = i; }
  }
  return n;
}

// ---------------------------------------------------------------------------
// Option (b): SPSC byte ring carrying param-set records
//   record = [u16 id][f32 value] = 6 bytes (here padded to 8 for alignment)
// ---------------------------------------------------------------------------
const RING_CAP = 4096;                       // records
const REC = 8;                               // bytes/record (id:u32 + f32)
const ringSab = new SharedArrayBuffer(RING_CAP * REC);
const ringU32 = new Uint32Array(ringSab);
const ringF32 = new Float32Array(ringSab);
const ctl = new Int32Array(new SharedArrayBuffer(8)); // [head, tail] atomic
const HEAD = 0, TAIL = 1;
const ringLastSeen = new Float32Array(N);

function ring_push(id, valBits) {
  const tail = Atomics.load(ctl, TAIL);
  const slot = tail % RING_CAP;
  ringU32[slot * 2] = id;
  ringU32[slot * 2 + 1] = valBits;
  Atomics.store(ctl, TAIL, tail + 1);
}

// (i) bulk load over the ring: one record per param
function ring_bulkLoad(preset) {
  for (let i = 0; i < N; i++) ring_push(i, preset[i]);
}

// audio side: drain the ring into a local f32 mirror (what the worklet does)
const audioMirror = new Float32Array(N);
function ring_drainAudio() {
  let head = Atomics.load(ctl, HEAD);
  const tail = Atomics.load(ctl, TAIL);
  while (head < tail) {
    const slot = head % RING_CAP;
    const id = ringU32[slot * 2];
    const bits = ringU32[slot * 2 + 1];
    audioMirror[id] = f32FromBits(bits);
    head++;
  }
  Atomics.store(ctl, HEAD, head);
}
function f32FromBits(b) { const a = new Int32Array(1); a[0] = b; return new Float32Array(a.buffer)[0]; }

// (ii) diff readback over the ring: there is NO shared latest-value store —
// the audio thread holds the authoritative copy in its own linear memory.
// To observe audio-thread drift on the main thread you need a SECOND ring
// (audio->main) carrying echo records, then drain+diff that. Model the cost
// as: audio thread emits an echo record per changed param, main drains them.
const echoSab = new SharedArrayBuffer(RING_CAP * REC);
const echoU32 = new Uint32Array(echoSab);
const echoCtl = new Int32Array(new SharedArrayBuffer(8));
function echo_emit(id, bits) {
  const tail = Atomics.load(echoCtl, TAIL);
  const slot = tail % RING_CAP;
  echoU32[slot * 2] = id;
  echoU32[slot * 2 + 1] = bits;
  Atomics.store(echoCtl, TAIL, tail + 1);
}
function ring_diffReadback(changedIds, out) {
  // audio side: emit one echo per changed param
  for (const id of changedIds) echo_emit(id, f2i(audioMirror[id]));
  // main side: drain echoes
  let head = Atomics.load(echoCtl, HEAD);
  const tail = Atomics.load(echoCtl, TAIL);
  let n = 0;
  while (head < tail) {
    const slot = head % RING_CAP;
    out[n++] = echoU32[slot * 2];
    head++;
  }
  Atomics.store(echoCtl, HEAD, head);
  return n;
}

// ---------------------------------------------------------------------------
// Benchmark driver
// ---------------------------------------------------------------------------
function bench(label, fn) {
  fn(); fn(); fn(); // warm
  const t0 = process.hrtime.bigint();
  for (let k = 0; k < ITERS; k++) fn();
  const t1 = process.hrtime.bigint();
  const nsPer = Number(t1 - t0) / ITERS;
  console.log(`  ${label.padEnd(42)} ${(nsPer).toFixed(0).padStart(8)} ns/op  (${(nsPer/1000).toFixed(3)} µs)`);
  return nsPer;
}

// Build a representative preset (165 random f32 bit patterns)
const preset = new Int32Array(N);
for (let i = 0; i < N; i++) preset[i] = f2i(Math.random());

console.log(`\nparam-store bench — N=${N} params, ${ITERS.toLocaleString()} iters/op\n`);

console.log("(i) bulk preset load (165 params at once):");
const aBulk = bench("(a) SAB atomic store ×165", () => sab_bulkLoad(preset));
const bBulk = bench("(b) ring push ×165", () => { Atomics.store(ctl, HEAD, 0); Atomics.store(ctl, TAIL, 0); ring_bulkLoad(preset); ring_drainAudio(); });

// Scenario (ii): audio thread changed K params (automation/LFO drift);
// main thread must observe them. Model a typical pump tick: K=8 drifted.
const K = 8;
const changed = [];
for (let i = 0; i < K; i++) changed.push((i * 19) % N);

console.log("\n(ii) audio->main diff readback (typical: 8 drifted params/tick):");
// SAB: mutate K atomics from "audio side", then main scans all 165
const sabOut = new Int32Array(N);
const aDiff = bench("(a) SAB full scan ×165 (+8 drift)", () => {
  for (const id of changed) Atomics.store(ai, id, f2i(Math.random()));
  sab_diffReadback(sabOut);
});
// ring: audio emits 8 echoes, main drains
const ringOut = new Int32Array(N);
const bDiff = bench("(b) ring echo emit×8 + drain", () => {
  Atomics.store(echoCtl, HEAD, 0); Atomics.store(echoCtl, TAIL, 0);
  for (const id of changed) audioMirror[id] = Math.random();
  ring_diffReadback(changed, ringOut);
});

console.log("\n(ii-worst) diff readback when ALL 165 drift (preset load echo):");
const aDiffAll = bench("(a) SAB full scan ×165 (+165 drift)", () => {
  for (let i = 0; i < N; i++) Atomics.store(ai, i, f2i(Math.random()));
  sab_diffReadback(sabOut);
});
const allIds = Array.from({length: N}, (_, i) => i);
const bDiffAll = bench("(b) ring echo emit×165 + drain", () => {
  Atomics.store(echoCtl, HEAD, 0); Atomics.store(echoCtl, TAIL, 0);
  for (let i = 0; i < N; i++) audioMirror[i] = Math.random();
  ring_diffReadback(allIds, ringOut);
});

console.log("\nsummary (lower = faster):");
console.log(`  bulk load:        SAB ${aBulk.toFixed(0)} ns vs ring ${bBulk.toFixed(0)} ns`);
console.log(`  diff (8 drift):   SAB ${aDiff.toFixed(0)} ns vs ring ${bDiff.toFixed(0)} ns`);
console.log(`  diff (165 drift): SAB ${aDiffAll.toFixed(0)} ns vs ring ${bDiffAll.toFixed(0)} ns`);
console.log("");
