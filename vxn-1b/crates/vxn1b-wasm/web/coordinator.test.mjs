// Headless tests for the main-thread coordinator (0289).
//
// A fake AudioContext / AudioWorkletNode pair stands in for the browser, and the
// fake node runs the REAL WorkletHostRunner over the REAL SABs — so what is
// exercised here is the actual transport, not a mock of it.
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/coordinator.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { WebHost } from "./coordinator.mjs";
import { WorkletHostRunner } from "./host-runner.mjs";
import { TOTAL_PARAMS } from "./param-store.mjs";
import { EV_NOTE_ON, EV_MATRIX_EDIT, EV_SCOPE_TAP, EV_TEMPO, LAYER_L2 } from "./event-codec.mjs";
import { EventRing } from "./event-ring.mjs";

const WASM = ["release", "debug"]
  .map((p) =>
    fileURLToPath(new URL(`../../../../target/wasm32-unknown-unknown/${p}/vxn1b_wasm.wasm`, import.meta.url)),
  )
  .find((p) => existsSync(p));

function wasmBytes() {
  assert.ok(
    WASM,
    "vxn1b_wasm.wasm not found — this test must not be skipped. Build it:\n" +
      '  RUSTFLAGS="-C target-feature=+simd128" cargo build -p vxn1b-wasm --target wasm32-unknown-unknown --release',
  );
  return readFileSync(WASM);
}

// ── Fake browser audio graph ───────────────────────────────────────────────

class FakeContext {
  constructor() {
    this.state = "suspended";
    this.sampleRate = 48000;
    this.destination = { id: "destination" };
    this.audioWorklet = { addModule: async () => {} };
    this._listeners = {};
    this.closed = false;
  }
  addEventListener(t, fn) {
    (this._listeners[t] ||= []).push(fn);
  }
  removeEventListener(t, fn) {
    this._listeners[t] = (this._listeners[t] || []).filter((f) => f !== fn);
  }
  _emit(t) {
    for (const fn of this._listeners[t] || []) fn();
  }
  async resume() {
    this.state = "running";
    this._emit("statechange");
  }
  async suspend() {
    this.state = "suspended";
    this._emit("statechange");
  }
  async close() {
    this.state = "closed";
    this.closed = true;
    this._emit("statechange");
  }
}

/// A node that actually runs the worklet half. `render()` drives one quantum,
/// the way the browser's audio thread would.
class FakeNode {
  constructor(ctx, name, options) {
    this.ctx = ctx;
    this.name = name;
    this.connected = false;
    const opts = options.processorOptions;
    this.port = {
      onmessage: null,
      postMessage: (m) => this._fromMain(m),
      _toMain: (m) => this.port.onmessage && this.port.onmessage({ data: m }),
    };
    this.runner = new WorkletHostRunner({
      wasmBytes: opts.wasmBytes,
      ringSab: opts.ringSab,
      storeSab: opts.storeSab,
      telemetrySab: opts.telemetrySab,
      capacity: opts.capacity,
      sampleRate: ctx.sampleRate,
      onReady: () => this.port._toMain({ type: "ready" }),
      onTrap: (e, count) => this.port._toMain({ type: "trap", message: String(e.message), count }),
    });
    this.resets = 0;
    this.destroyed = false;
    this.booted = this.runner.init();
  }
  _fromMain(m) {
    if (m.type === "reset") this.resets++;
    if (m.type === "destroy") {
      this.runner.destroy();
      this.destroyed = true;
    }
  }
  connect() {
    this.connected = true;
  }
  disconnect() {
    this.connected = false;
  }
  render(n = 1) {
    const l = new Float32Array(128);
    const r = new Float32Array(128);
    for (let i = 0; i < n; i++) this.runner.process(l, r);
    return l;
  }
}

const peak = (a) => a.reduce((m, y) => Math.max(m, Math.abs(y)), 0);

async function boot(extra = {}) {
  const host = new WebHost({
    wasmBytes: wasmBytes(),
    AudioContextClass: FakeContext,
    AudioWorkletNodeClass: FakeNode,
    mediaDevices: null,
    ...extra,
  });
  await host.start();
  await host.node.booted;
  return host;
}

// ── Construction and boot ──────────────────────────────────────────────────

test("the transport exists before start(), so events can be queued pre-gesture", () => {
  const host = new WebHost({ wasmBytes: wasmBytes(), AudioContextClass: FakeContext, mediaDevices: null });
  assert.ok(host.ringSab && host.storeSab, "the SABs are allocated up front");
  assert.equal(host.gateState, "idle");
  assert.equal(host.noteOn(60, 1.0), true, "a note can be queued before audio exists");
});

test("start() walks the gate to running and connects the node", async () => {
  const states = [];
  const host = await boot({ onState: (s) => states.push(s) });
  assert.deepEqual(states, ["starting", "running"]);
  assert.equal(host.gateState, "running");
  assert.ok(host.node.connected);
  await host.teardown();
});

test("start() refuses a second call and refuses after teardown", async () => {
  const host = await boot();
  await assert.rejects(() => host.start(), /already called/);
  await host.teardown();
  await assert.rejects(() => host.start(), /torn down/);
});

// The seeding order is the whole reason this method exists: the worklet's first
// fold is NaN-seeded and applies EVERY id, so a store still full of zeros when
// the worklet starts would write 0.0 over every param and silence the synth.
test("the param store is seeded with engine defaults before the worklet starts", async () => {
  const host = await boot();
  let nonZero = 0;
  for (let id = 0; id < TOTAL_PARAMS; id++) if (host.readParam(id) !== 0) nonZero++;
  assert.ok(nonZero > 20, `expected engine defaults in the store, only ${nonZero} were non-zero`);

  host.noteOn(60, 1.0);
  assert.ok(peak(host.node.render(2)) > 0, "and the instrument sounds");
  await host.teardown();
});

test("the telemetry SAB is sized from the engine, not from a literal", async () => {
  const host = await boot();
  const x = (await WebAssembly.instantiate(wasmBytes(), {})).instance.exports;
  assert.ok(host.telemetrySab, "allocated during start()");
  assert.equal(host.telemetry.meterLen, x.vxn1b_meter_len());
  assert.equal(host.telemetry.scopeWindow, x.vxn1b_scope_window());
  await host.teardown();
});

test("whenReady resolves once the worklet has instantiated", async () => {
  const host = await boot();
  assert.equal(await host.whenReady, host, "resolves with the host");
  assert.equal(host.ready, true);
  await host.teardown();
});

// ── Producer surface ───────────────────────────────────────────────────────

test("every producer call lands on the ring as the right event", async () => {
  const host = await boot();
  const consumer = new EventRing(host.ringSab, host.capacity);
  consumer.drainInto([]); // clear whatever boot left

  host.noteOn(60, 0.5, 0, 3);
  host.setMatrix(LAYER_L2, 5, 1, 4);
  host.setScopeTap(2);
  host.setTempo(140);

  const out = consumer.drainInto([]);
  assert.deepEqual(
    out.map((r) => r.type),
    [EV_NOTE_ON, EV_MATRIX_EDIT, EV_SCOPE_TAP, EV_TEMPO],
  );
  assert.equal(out[0].flag, 3, "the MIDI channel rides the note event");
  assert.equal(out[3].value, 140);
  await host.teardown();
});

// Key mode / split point ride the RING here, where vxn-1 sends them over the
// port. That is why there is no latched state to replay onto a rebuilt worklet.
test("non-automatable state goes over the ring, not the port", async () => {
  const host = await boot();
  const consumer = new EventRing(host.ringSab, host.capacity);
  consumer.drainInto([]);
  host.setKeyMode(2);
  host.setSplitPoint(48);
  host.setLfo2Link(true);
  const out = consumer.drainInto([]);
  assert.deepEqual(
    out.map((r) => r.flag),
    [2, 48, 1],
  );
  await host.teardown();
});

test("params go to the store and read back", async () => {
  const host = await boot();
  host.setParam(7, 0.25);
  assert.equal(host.readParam(7), 0.25);
  await host.teardown();
});

// ── Lifecycle ──────────────────────────────────────────────────────────────

// The flush exists because a suspended context's clock is stopped: voices that
// were sounding when audio stopped would otherwise resume mid-note, or hang if
// their note-off was eaten while the tab was backgrounded.
test("resume after suspend flushes sounding voices, suspend alone does not", async () => {
  const host = await boot();
  assert.equal(host.node.resets, 0);

  await host.suspend();
  assert.equal(host.gateState, "suspended");
  assert.equal(host.node.resets, 0, "nothing to flush while suspended");

  await host.resume();
  assert.equal(host.gateState, "running");
  assert.equal(host.node.resets, 1, "voices flushed on the way back");
  await host.teardown();
});

test("a browser-driven suspend/resume drives the gate the same way", async () => {
  const states = [];
  const host = await boot({ onState: (s) => states.push(s) });
  host.ctx.state = "suspended";
  host.ctx._emit("statechange");
  host.ctx.state = "running";
  host.ctx._emit("statechange");
  assert.deepEqual(states, ["starting", "running", "suspended", "running"]);
  assert.equal(host.node.resets, 1);
  await host.teardown();
});

test("rebuild() re-boots over the SAME SABs so transport state survives", async () => {
  const host = await boot();
  host.setParam(9, 0.375);
  const ringSab = host.ringSab;
  const storeSab = host.storeSab;
  const oldNode = host.node;

  await host.rebuild();
  await host.node.booted;

  assert.equal(host.ringSab, ringSab, "same ring SAB");
  assert.equal(host.storeSab, storeSab, "same store SAB");
  assert.notEqual(host.node, oldNode, "a fresh worklet node");
  assert.ok(oldNode.destroyed, "the old worklet was told to destroy");
  // The point of rebuilding over the same SABs: the user's patch survives a
  // context change. Re-seeding defaults on every start() would silently reset
  // their sound on a device switch.
  assert.equal(host.readParam(9), 0.375, "param state survived the rebuild");
  assert.equal(host.gateState, "running");
  await host.teardown();
});

test("teardown closes the context, destroys the worklet, and drops the SABs", async () => {
  const host = await boot();
  const node = host.node;
  const ctx = host.ctx;
  await host.teardown();
  assert.ok(node.destroyed);
  assert.equal(node.connected, false);
  assert.ok(ctx.closed);
  assert.equal(host.ringSab, null);
  assert.equal(host.storeSab, null);
  assert.equal(host.telemetrySab, null);
  assert.equal(host.gateState, "closed");
  await assert.rejects(() => host.rebuild(), /torn down/);
});

test("a trap is surfaced to the main thread with its count", async () => {
  const traps = [];
  const host = await boot({ onTrap: (msg, n) => traps.push([msg, n]) });
  host.node.port._toMain({ type: "trap", message: "unreachable executed", count: 1 });
  assert.deepEqual(traps, [["unreachable executed", 1]]);
  assert.equal(host.ready, false, "ready drops until the rebuilt worklet reports in");
  await host.teardown();
});

test("cpu messages reach the observer", async () => {
  const cpu = [];
  const host = await boot({ onCpu: (load, peakLoad) => cpu.push([load, peakLoad]) });
  host.node.port._toMain({ type: "cpu", load: 0.25, peak: 0.4 });
  assert.deepEqual(cpu, [[0.25, 0.4]]);
  await host.teardown();
});

// ── End to end ─────────────────────────────────────────────────────────────

test("a note plays and its audio comes back as a meter frame", async () => {
  const host = await boot();
  host.setScopeTap(1);
  host.noteOn(60, 1.0);

  let meters = null;
  for (let i = 0; i < 64 && !meters; i++) {
    host.node.render(1);
    meters = host.pollMeters();
  }
  assert.ok(meters, "meter frames reach the main thread");
  assert.ok(
    meters.some((v) => v !== 0),
    "the note registers on a tap",
  );
  await host.teardown();
});
