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
    // Kept so tests can assert what the worklet was actually told (e.g. whether
    // the CPU meter was enabled for it — ticket 0309).
    this.processorOptions = opts;
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
    ...extra,
  });
  await host.start();
  await host.node.booted;
  return host;
}

// ── Construction and boot ──────────────────────────────────────────────────

test("the transport exists before start(), so events can be queued pre-gesture", () => {
  const host = new WebHost({ wasmBytes: wasmBytes(), AudioContextClass: FakeContext });
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
});

test("a trap is surfaced to the main thread with its count", async () => {
  const traps = [];
  const host = await boot({ onTrap: (msg, n) => traps.push([msg, n]) });
  host.node.port._toMain({ type: "trap", message: "unreachable executed", count: 1 });
  assert.deepEqual(traps, [["unreachable executed", 1]]);
  assert.equal(host.ready, false, "ready drops until the rebuilt worklet reports in");
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

// `globalThis.navigator` is a getter-only property in Node, so it cannot be
// assigned — only redefined. Used to stand in as Safari / Chrome-on-iOS while
// `isAppleWebKit()` reads it.
// ASYNC on purpose: `fn` boots a host, and a synchronous `finally` around a
// pending promise restores the real navigator before `isAppleWebKit()` is ever
// called — the override then does nothing and the test passes for the wrong
// reason (it did, first time round).
async function withNavigator(nav, fn) {
  const had = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", { value: nav, configurable: true });
  try {
    return await fn();
  } finally {
    if (had) Object.defineProperty(globalThis, "navigator", had);
    else delete globalThis.navigator;
  }
}

// ── Render-load meter (ticket 0309) ────────────────────────────────────────

test("a cpu port message reaches onCpu, and the clock kind is logged once", async () => {
  const seen = [];
  const infos = [];
  const origInfo = console.info;
  console.info = (m) => infos.push(m);
  try {
    const host = await boot({ onCpu: (load, peak) => seen.push([load, peak]) });
    host._onPortMessage({ type: "cpu", load: 0.25, peak: 0.4, clock: "date" });
    host._onPortMessage({ type: "cpu", load: 0.3, peak: 0.45, clock: "date" });
    assert.deepEqual(seen, [
      [0.25, 0.4],
      [0.3, 0.45],
    ]);
    // Which clock the worklet got matters — `date` means the reading is a window
    // mean of 1 ms steps — but it is a boot fact, not a per-frame one.
    assert.equal(infos.filter((m) => /CPU meter clock/.test(m)).length, 1);
  } finally {
    console.info = origInfo;
  }
});

test("the meter is enabled off Safari and the worklet is told so", async () => {
  const host = await boot();
  assert.equal(host._cpuMeterEnabled, true, "no navigator in node → not Safari");
  assert.equal(host.node.processorOptions.cpuMeter, true);
});

test("on Safari the meter is off, and onCpu reports null rather than staying silent", async () => {
  // Safari's AudioWorklet has no render-thread slack, so the timing and the
  // periodic postMessage can cause the very glitching the meter would report
  // ([[vxn1-web-safari-audioworklet]]). A null reading is how the badge shows
  // "n/a" instead of a dash that looks like a measurement still loading.
  const seen = [];
  const host = await withNavigator(
    { vendor: "Apple Computer, Inc.", userAgent: "… Version/17 Safari/605" },
    () => boot({ onCpu: (load, peak) => seen.push([load, peak]) }),
  );
  assert.equal(host._cpuMeterEnabled, false);
  assert.equal(host.node.processorOptions.cpuMeter, false);
  assert.deepEqual(seen, [[null, null]]);
});

test("Chrome on iOS is not treated as Safari", async () => {
  // CriOS carries Apple's vendor string but is Blink; disabling the meter there
  // would be a silent, permanent n/a on a browser that can measure fine.
  const host = await withNavigator(
    { vendor: "Apple Computer, Inc.", userAgent: "… CriOS/120 Mobile Safari" },
    () => boot(),
  );
  assert.equal(host._cpuMeterEnabled, true);
});
