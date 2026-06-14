// Node harness for worklet lifecycle + trap safety (ticket 0040).
//
// Drives the shared WorkletHostRunner (web/host-runner.mjs) — the exact code the
// AudioWorklet runs — headlessly, asserting the lifecycle hardening:
//
//   node harness-0040.mjs   (build the wasm + copy to web/ first; see README)
//
//   1. Silence-until-ready: process() before init() resolves outputs exact
//      silence and does NOT drain the ring, so events written pre-ready survive
//      and apply in order on the first ready quantum.
//   2. Sample-rate: a context sample-rate change leaves the host rendering.
//   3. Reset + teardown: reset() clears sounding voices without dropping ring
//      state; destroy() releases the host and SAB references.
//   4. Trap safety: a forced render-thread trap is caught at the runner
//      boundary (process() never throws), surfaced via onTrap, and audio
//      RECOVERS after async re-instantiate over the same SABs.

import { readFileSync } from "node:fs";
import { WorkletHostRunner } from "./web/host-runner.mjs";
import { createRingSAB, EventRing } from "./web/event-ring.mjs";

const WASM = new URL(
  "../../../target/wasm32-unknown-unknown/release/vxn_wasm.wasm",
  import.meta.url,
);
const SR = 48000;
const wasmBytes = readFileSync(WASM);
// Q is fixed at 128 in Web Audio; read it once from a throwaway instance.
const Q = (await WebAssembly.instantiate(wasmBytes, {})).instance.exports.vxn_quantum();

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};
const onset = (buf) => {
  for (let i = 0; i < buf.length; i++) if (buf[i] !== 0) return i;
  return -1;
};
const tick = () => new Promise((r) => setTimeout(r, 0));
const l = new Float32Array(Q);
const r = new Float32Array(Q);

console.log("\n=== 1. silence-until-ready + pre-ready events buffer in the ring ===");
{
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  const runner = new WorkletHostRunner({ wasmBytes, ringSab, sampleRate: SR });

  // Producer writes a note BEFORE the worklet is live.
  ring.pushNoteOn(10, 64, 1.0);

  // Render a quantum while still not-ready: must be exact silence AND must not
  // consume the ring.
  const rendered = runner.process(l, r);
  check(rendered === false, "pre-ready process() reports no audio");
  check(l.every((s) => s === 0) && r.every((s) => s === 0), "pre-ready output is exact silence");
  check(ring.pending() === 1, "pre-ready process did NOT drain the ring (event preserved)");

  // Now go live and render: the buffered note applies at its offset (10 -> 11).
  await runner.init();
  runner.process(l, r);
  check(onset(l) === 11, `buffered pre-ready note applied at its offset (onset ${onset(l)}, want 11)`);
  runner.destroy();
}

console.log("\n=== 2. sample-rate change leaves the host rendering ===");
{
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  const runner = new WorkletHostRunner({ wasmBytes, ringSab, sampleRate: SR });
  await runner.init();
  runner.setSampleRate(44100);
  ring.pushNoteOn(0, 60, 1.0);
  runner.process(l, r);
  check(onset(l) >= 0, "host still renders audio after a sample-rate change");
  runner.destroy();
}

console.log("\n=== 3. reset clears voices; teardown releases refs ===");
{
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  const runner = new WorkletHostRunner({ wasmBytes, ringSab, sampleRate: SR });
  await runner.init();
  ring.pushNoteOn(0, 60, 1.0);
  runner.process(l, r);
  check(onset(l) >= 0, "note sounds before reset");
  runner.reset(); // all-notes-off, ring untouched
  runner.process(l, r);
  check(l.every((s) => s === 0) && r.every((s) => s === 0), "reset cleared the sounding voice");

  runner.destroy();
  check(runner.host === null && runner.ringSab === null, "teardown released host + SAB references");
  const after = runner.process(l, r);
  check(after === false && l.every((s) => s === 0), "post-destroy process() is safe silence");
}

console.log("\n=== 4. trap safety: caught, surfaced, and audio recovers ===");
{
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  let trapMsg = null;
  const runner = new WorkletHostRunner({
    wasmBytes,
    ringSab,
    sampleRate: SR,
    onTrap: (e) => { trapMsg = String(e && e.message || e); },
  });
  await runner.init();

  // Arm a forced trap on the next render and process — must NOT throw out.
  runner.host.armTrap();
  let threw = false;
  try {
    const ret = runner.process(l, r);
    check(ret === false, "trapped quantum reports no audio (caught at the boundary)");
  } catch {
    threw = true;
  }
  check(!threw, "trap did NOT propagate out of process() (worklet boundary held)");
  check(l.every((s) => s === 0) && r.every((s) => s === 0), "trapped quantum output exact silence");
  check(trapMsg !== null, `trap surfaced to main via onTrap (\"${trapMsg}\")`);
  check(runner.ready === false, "runner marked not-ready after trap (instance poisoned)");

  // Recovery is async (re-instantiate over the same SABs); let it settle.
  await tick();
  await tick();
  check(runner.ready === true, "host re-instantiated after the trap (recovered)");

  // Ring state survived the re-init (same SAB): a fresh note renders again.
  ring.pushNoteOn(5, 67, 1.0);
  runner.process(l, r);
  check(onset(l) === 6, `audio resumes after recovery (onset ${onset(l)}, want 6)`);
  runner.destroy();
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
