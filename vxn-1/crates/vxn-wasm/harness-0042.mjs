// Node harness for the main-thread coordinator (ticket 0042).
//
//   node harness-0042.mjs   (build the wasm + copy to web/ first; see README)
//
// Drives the REAL WebHost (web/coordinator.mjs) — the exact main-side code the
// browser runs — headlessly. There is no AudioWorklet in Node, so we inject a
// fake AudioContext whose AudioWorkletNode runs the REAL WorkletHostRunner
// (host-runner.mjs) over the SAME SABs the coordinator allocated. That is the
// E015 one-code-path discipline applied to the boot: every byte of the boot +
// transport (SAB alloc, processorOptions hand-off, ring producer, store fold,
// port ready/trap) is the production path; only the audio-clock pump is faked,
// and we pump it by hand to assert what the speaker would hear.
//
//   1. Boot: start() reaches "audio live" (worklet posts `ready`); the SABs the
//      coordinator allocated are the ones the worklet mapped (same identity).
//   2. Note: a note written from the main thread via WebHost.noteOn sounds at
//      its sample offset on the audio thread.
//   3. Param: a value written to the store via WebHost.setParam takes effect on
//      the audio thread (observed via the readback echo / pollParamDiffs).
//   4. Trap: a render-thread trap is surfaced to the coordinator's onTrap.

import { readFileSync } from "node:fs";
import { WebHost } from "./web/coordinator.mjs";
import { WorkletHostRunner } from "./web/host-runner.mjs";

const WASM = new URL(
  "../../../target/wasm32-unknown-unknown/release/vxn_wasm.wasm",
  import.meta.url,
);
const SR = 48000;
const wasmBytes = readFileSync(WASM);
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

// ---------------------------------------------------------------------------
// Fake AudioContext: the audio-thread stand-in. Its node builds the REAL runner
// from the coordinator's processorOptions and exposes a hand-cranked render()
// the harness pumps in place of the browser's audio clock. The runner wires its
// onReady/onTrap to the node's port, exactly as vxn-processor.js does — so
// the coordinator's port.onmessage sees the genuine ready/trap stream.
// ---------------------------------------------------------------------------
function makeFakeAudio(sampleRate) {
  let lastNode = null;

  class FakeWorkletNode {
    constructor(_ctx, _name, opts) {
      const po = opts.processorOptions;
      // The port: worklet end posts to .onmessage (coordinator listens); the
      // coordinator posts to .postMessage (we forward to the runner like the
      // real processor's port.onmessage switch does).
      const portMain = { onmessage: null };
      this.port = {
        onmessage: null, // unused on the node's own end here
        postMessage: (m) => this._onControl(m),
      };
      // The runner surfaces ready/trap to "the main thread" == coordinator.
      this.runner = new WorkletHostRunner({
        wasmBytes: po.wasmBytes,
        ringSab: po.ringSab,
        storeSab: po.storeSab,
        sampleRate,
        capacity: po.capacity,
        onReady: () => this._post({ type: "ready" }),
        onTrap: (e, count) =>
          this._post({ type: "trap", message: String((e && e.message) || e), count }),
      });
      this._portMain = portMain;
      this.runner.init(); // async; render() is silence until it resolves
      lastNode = this;
    }
    // Deliver a worklet->main message to whoever the coordinator hooked up.
    _post(m) {
      if (this.port.onmessage) this.port.onmessage({ data: m });
    }
    // main->worklet control messages (keyMode/splitPoint/reset/destroy).
    _onControl(m) {
      switch (m.type) {
        case "keyMode": this.runner.setKeyMode(m.value); break;
        case "splitPoint": this.runner.setSplitPoint(m.value); break;
        case "reset": this.runner.reset(); break;
        case "destroy": this.runner.destroy(); break;
        default: break;
      }
    }
    connect() {}
    disconnect() {}
    // Harness-only: pump one quantum through the real runner.
    render(l, r) {
      return this.runner.process(l, r);
    }
  }

  class FakeAudioContext {
    constructor() {
      this.sampleRate = sampleRate;
      this.destination = {};
      this.state = "suspended";
      this.audioWorklet = { addModule: async () => {} };
    }
    async resume() {
      this.state = "running";
    }
    async close() {
      this.state = "closed";
    }
  }

  return { FakeAudioContext, FakeWorkletNode, lastNode: () => lastNode };
}

const l = new Float32Array(Q);
const r = new Float32Array(Q);

console.log("\n=== 1. boot reaches 'audio live' over the coordinator's SABs ===");
let host, fake;
{
  fake = makeFakeAudio(SR);
  let readyFired = false;
  host = new WebHost({
    wasmBytes,
    AudioContextClass: fake.FakeAudioContext,
    AudioWorkletNodeClass: fake.FakeWorkletNode,
    onReady: () => (readyFired = true),
  });
  // SABs exist before start() (producer usable immediately).
  check(host.ringSab != null && host.storeSab != null, "transport SABs allocated at construction");

  await host.start();
  const node = fake.lastNode();
  // Same SAB identity main<->worklet: the coordinator's ring IS the worklet's.
  check(node.runner.ringSab === host.ringSab, "worklet mapped the coordinator's ring SAB (same identity)");
  check(node.runner.storeSab === host.storeSab, "worklet mapped the coordinator's store SAB (same identity)");

  await host.whenReady; // the async wasm instantiate
  check(host.ready === true && readyFired, "coordinator observed worklet `ready` (audio live)");
  check(host.ctx.state === "running", "context resumed");
}

console.log("\n=== 2. a note from the main thread sounds on the audio thread ===");
{
  const node = fake.lastNode();
  host.noteOn(64, 1.0, 10); // offset 10 -> onset at 11 (slice boundary)
  node.render(l, r);
  check(onset(l) === 11, `main-thread note sounded at its offset (onset ${onset(l)}, want 11)`);
  host.noteOff(64, 0);
}

console.log("\n=== 3. a param written to the store takes effect on the audio thread ===");
{
  const node = fake.lastNode();
  // Drain any first-quantum full-broadcast readback so we measure THIS edit.
  node.render(l, r);
  host.pollParamDiffs();

  // Write a distinctive value to a param id and render: the worklet's store fold
  // applies it and echoes it into the readback region.
  const ID = 5;
  const VAL = 0.731;
  host.setParam(ID, VAL);
  node.render(l, r);
  const diffs = host.pollParamDiffs();
  const got = diffs.find((d) => d.id === ID);
  check(got != null, `param ${ID} surfaced in readback after the audio thread applied it`);
  check(got != null && Math.abs(got.plain - VAL) < 1e-6, `applied value echoed back (${got && got.plain}, want ${VAL})`);
}

console.log("\n=== 4. a render-thread trap is surfaced to the coordinator ===");
{
  fake = makeFakeAudio(SR);
  let trapMsg = null;
  host = new WebHost({
    wasmBytes,
    AudioContextClass: fake.FakeAudioContext,
    AudioWorkletNodeClass: fake.FakeWorkletNode,
    onTrap: (msg) => (trapMsg = msg),
  });
  await host.start();
  await host.whenReady;
  const node = fake.lastNode();

  node.runner.host.armTrap(); // force a render-thread trap next quantum
  node.render(l, r); // caught at the runner boundary; must not throw
  check(trapMsg !== null, `trap surfaced to coordinator.onTrap ("${trapMsg}")`);
  check(host.ready === false, "coordinator marked not-ready after trap");

  // Recovery is async (re-instantiate over the same SABs); then audio resumes.
  await tick();
  await tick();
  host.noteOn(67, 1.0, 5);
  node.render(l, r);
  check(onset(l) === 6, `audio recovers after the trap (onset ${onset(l)}, want 6)`);
  await host.dispose();
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
