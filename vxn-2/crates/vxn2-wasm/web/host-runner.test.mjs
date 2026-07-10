// End-to-end runner test (ticket 0156) over the REAL engine wasm.
//
// Run (after `cargo build -p vxn2-wasm --target wasm32-unknown-unknown --release
// -C target-feature=+simd128`):
//   node --test crates/vxn2-wasm/web/host-runner.test.mjs
//
// Boots the actual `vxn2_wasm.wasm` through the WorkletHostRunner over a real
// event-ring SAB, drives a note through the ring, and asserts audible output —
// the headless proxy for "the served page renders a test tone driven through the
// ring". Also proves silence-until-ready and render-thread trap recovery. Skips
// (not fails) when the wasm artifact isn't built, so the suite stays green in a
// checkout that hasn't run the wasm build.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WorkletHostRunner } from "./host-runner.mjs";
import { EventRing, createRingSAB, DEFAULT_CAPACITY } from "./event-ring.mjs";

const WASM_PATH = fileURLToPath(
  new URL("../../../../target/wasm32-unknown-unknown/release/vxn2_wasm.wasm", import.meta.url),
);
const HAVE_WASM = existsSync(WASM_PATH);
const wasmBytes = HAVE_WASM ? readFileSync(WASM_PATH) : null;

const Q = 128;

/// Peak absolute sample across the stereo pair.
function peak(l, r) {
  let m = 0;
  for (let i = 0; i < l.length; i++) m = Math.max(m, Math.abs(l[i]), Math.abs(r[i]));
  return m;
}

test("silence-until-ready then a ring note-on renders audible output", { skip: !HAVE_WASM }, async () => {
  const ringSab = createRingSAB(DEFAULT_CAPACITY);
  const ring = new EventRing(ringSab, DEFAULT_CAPACITY); // producer
  const runner = new WorkletHostRunner({
    wasmBytes,
    ringSab,
    sampleRate: 48000,
    capacity: DEFAULT_CAPACITY,
  });

  const l = new Float32Array(Q);
  const r = new Float32Array(Q);

  // Before init resolves: process outputs silence and returns false.
  assert.equal(runner.process(l, r), false);
  assert.ok(peak(l, r) === 0, "pre-ready output must be exact silence");

  await runner.init(); // instantiate the real engine
  assert.equal(runner.ready, true);

  // No engine store passed → the engine renders with its own SharedParams::new
  // defaults, so a plain note sounds without seeding. Push a note-on at offset 0.
  ring.pushNoteOn(0, 60, 1.0);

  // Render a few quanta for the FM attack to open up.
  let loud = 0;
  for (let i = 0; i < 8; i++) {
    runner.process(l, r);
    loud = Math.max(loud, peak(l, r));
  }
  assert.ok(loud > 1e-4, `expected audible tone through the ring, got peak ${loud}`);
});

test("render-thread trap is caught and the engine recovers", { skip: !HAVE_WASM }, async () => {
  const traps = [];
  const runner = new WorkletHostRunner({
    wasmBytes,
    sampleRate: 48000,
    onTrap: (e, count) => traps.push(count),
  });
  await runner.init();

  const l = new Float32Array(Q);
  const r = new Float32Array(Q);

  // Arm a forced wasm trap on the next process(); the worklet boundary must
  // catch it, go silent, and NOT throw out to the caller.
  runner.host.armTrap();
  assert.doesNotThrow(() => runner.process(l, r));
  assert.equal(peak(l, r), 0, "output is silence on the trapping quantum");
  assert.equal(traps.length, 1, "onTrap fired exactly once");
  assert.equal(runner.ready, false, "runner marked not-ready after the trap");

  // Async re-instantiate over the same (absent here) SABs restores readiness.
  await new Promise((res) => setTimeout(res, 0));
  assert.equal(runner.ready, true, "engine recovered after the trap");
  assert.doesNotThrow(() => runner.process(l, r));
});
