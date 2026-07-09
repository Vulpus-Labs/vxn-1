// AudioContext lifecycle tests for the coordinator (ticket 0043).
//
// Run: node --test crates/vxn-wasm/web/coordinator-lifecycle.test.mjs
//
// These exercise the MAIN-THREAD lifecycle state machine the WebHost (0042
// coordinator) layers on for 0043 — autoplay unlock, suspend/resume voice
// flush, device change / rebuild, and teardown — against a MOCK AudioContext
// (no wasm, no audio clock). The browser-only bits (a real user gesture, a real
// statechange from tab-background, a real devicechange) are structured so the
// LOGIC under them is the unit covered here; the DOM event wiring itself needs a
// manual browser check (see the ticket close-out notes).
//
// The mock context implements just enough of the AudioContext surface the
// coordinator touches: state, audioWorklet.addModule, resume/suspend/close,
// addEventListener/removeEventListener("statechange"), setSinkId, and a
// dispatchStateChange() test hook to fire statechange like the browser would.

import { test } from "node:test";
import assert from "node:assert/strict";
import { WebHost } from "./coordinator.mjs";

// --- mocks ------------------------------------------------------------------

// Records the port messages the coordinator posts to the worklet (reset,
// destroy, keyMode, …) so we can assert the lifecycle's worklet-side effects.
function makeMockNode() {
  const posted = [];
  return {
    posted,
    port: {
      onmessage: null,
      postMessage: (m) => posted.push(m),
    },
    connect() {},
    disconnect() {
      this.disconnected = true;
    },
    disconnected: false,
  };
}

function makeMockContextClass(opts = {}) {
  const created = [];
  class MockAudioContext {
    constructor() {
      this.sampleRate = opts.sampleRate ?? 48000;
      this.destination = {};
      this.state = "suspended";
      this._listeners = { statechange: [] };
      this.closed = false;
      this.sinkId = "";
      this.audioWorklet = { addModule: async () => {} };
      this.setSinkIdSupported = opts.setSinkId !== false;
      if (this.setSinkIdSupported) {
        this.setSinkId = async (id) => {
          this.sinkId = id;
        };
      }
      created.push(this);
    }
    addEventListener(type, fn) {
      (this._listeners[type] ||= []).push(fn);
    }
    removeEventListener(type, fn) {
      const a = this._listeners[type] || [];
      const i = a.indexOf(fn);
      if (i >= 0) a.splice(i, 1);
    }
    _emit(type) {
      for (const fn of (this._listeners[type] || []).slice()) fn({ type });
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
    // Test hook: fire a statechange the browser would (tab background, etc.).
    dispatchStateChange(state) {
      this.state = state;
      this._emit("statechange");
    }
  }
  MockAudioContext.created = created;
  return MockAudioContext;
}

function makeMockMediaDevices() {
  const listeners = [];
  return {
    listeners,
    addEventListener(type, fn) {
      if (type === "devicechange") listeners.push(fn);
    },
    removeEventListener(type, fn) {
      const i = listeners.indexOf(fn);
      if (i >= 0) listeners.splice(i, 1);
    },
    emit() {
      for (const fn of listeners.slice()) fn({ type: "devicechange" });
    },
  };
}

// A WebHost wired to mocks. We must inject wasmBytes (never fetched) and stub
// _seedStoreFromDefaults (it instantiates the real wasm, which these unit tests
// don't need). The node class returns our recording mock.
function makeHost(extra = {}) {
  const Ctx = extra.AudioContextClass || makeMockContextClass(extra.ctxOpts);
  const nodes = [];
  class MockNode {
    constructor() {
      const n = makeMockNode();
      Object.assign(this, n);
      this._mock = n;
      nodes.push(this);
    }
  }
  const states = [];
  const host = new WebHost({
    wasmBytes: new Uint8Array(0),
    AudioContextClass: Ctx,
    AudioWorkletNodeClass: MockNode,
    mediaDevices: extra.mediaDevices || null,
    onState: (s) => states.push(s),
    ...extra.hostOpts,
  });
  // Skip the real-wasm default seeding; lifecycle tests don't need it.
  host._seedStoreFromDefaults = async () => {};
  return { host, Ctx, nodes, states, lastNode: () => nodes[nodes.length - 1] };
}

// --- autoplay unlock --------------------------------------------------------

test("gate starts idle and reaches running only after start() (autoplay unlock)", async () => {
  const { host, states } = makeHost();
  assert.equal(host.gateState, "idle", "no audio before a gesture");
  assert.equal(host.ctx, null);

  await host.start(); // stands in for the user-gesture handler
  assert.equal(host.gateState, "running");
  assert.equal(host.ctx.state, "running");
  // Saw the unlock progression on the UI hook.
  assert.deepEqual(states, ["starting", "running"]);
});

test("key mode / split set before start() replay onto the fresh worklet", async () => {
  const { host, lastNode } = makeHost();
  // Chosen from the UI before the audio graph exists (node is null) — the
  // posts are dropped by `?.` but latched. Without replay the worklet would
  // boot at its Whole default and Split render both layers from Upper.
  host.setKeyMode(2); // Split
  host.setSplitPoint(48);
  assert.equal(host.node, null, "no node before start()");

  await host.start();
  const posted = lastNode()._mock.posted;
  const km = posted.filter((m) => m.type === "keyMode").pop();
  const sp = posted.filter((m) => m.type === "splitPoint").pop();
  assert.equal(km && km.value, 2, "Split replayed onto the new worklet");
  assert.equal(sp && sp.value, 48, "split point replayed onto the new worklet");
});

test("start() after teardown refuses (fresh WebHost required)", async () => {
  const { host } = makeHost();
  await host.start();
  await host.teardown();
  await assert.rejects(() => host.start(), /torn down/);
});

// --- suspend / resume -------------------------------------------------------

test("browser-driven suspend then resume flushes voices and keeps transport", async () => {
  const { host, lastNode } = makeHost();
  await host.start();
  const node = lastNode();
  const ctx = host.ctx;

  // Transport state we expect to survive a suspend/resume cycle (0.5 is exact
  // in f32 — the store round-trips through a Float32Array).
  host.setParam(7, 0.5);
  const ringBefore = host.ringSab;
  const storeBefore = host.storeSab;

  ctx.dispatchStateChange("suspended"); // tab backgrounded
  assert.equal(host.gateState, "suspended");
  // No flush on suspend (nothing rendering).
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 0);

  ctx.dispatchStateChange("running"); // tab foregrounded
  assert.equal(host.gateState, "running");
  // Exactly one reset posted on resume — the voice flush (no stuck notes).
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 1);

  // Transport SABs and param store untouched by the cycle.
  assert.equal(host.ringSab, ringBefore);
  assert.equal(host.storeSab, storeBefore);
  assert.equal(host.readParam(7), 0.5);
});

test("programmatic suspend()/resume() drive the gate and flush once on resume", async () => {
  const { host, lastNode } = makeHost();
  await host.start();
  const node = lastNode();

  await host.suspend();
  assert.equal(host.gateState, "suspended");

  await host.resume();
  assert.equal(host.gateState, "running");
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 1);
});

test("resume from a non-suspended state does not flush", async () => {
  const { host, lastNode } = makeHost();
  await host.start(); // already running
  const node = lastNode();
  await host.resume(); // no-op: not suspended
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 0);
});

// --- device change ----------------------------------------------------------

test("devicechange listener attaches on start and detaches on teardown", async () => {
  const md = makeMockMediaDevices();
  const { host } = makeHost({ mediaDevices: md });
  await host.start();
  assert.equal(md.listeners.length, 1, "listening after start");
  md.emit(); // default handler: no structural change, no throw
  assert.equal(host.gateState, "running");
  await host.teardown();
  assert.equal(md.listeners.length, 0, "detached on teardown");
});

test("setSink re-routes via setSinkId without rebuilding the graph", async () => {
  const { host, nodes } = makeHost();
  await host.start();
  const nodeCountBefore = nodes.length;
  const ok = await host.setSink("device-xyz");
  assert.equal(ok, true);
  assert.equal(host.ctx.sinkId, "device-xyz");
  // No new node/context: the graph stayed up.
  assert.equal(nodes.length, nodeCountBefore);
});

test("setSink returns false when setSinkId is unsupported", async () => {
  const { host } = makeHost({ ctxOpts: { setSinkId: false } });
  await host.start();
  assert.equal(await host.setSink("device-xyz"), false);
});

test("rebuild() makes a new context over the SAME SABs (sample-rate change)", async () => {
  const { host, Ctx, nodes } = makeHost();
  await host.start();
  const ringBefore = host.ringSab;
  const storeBefore = host.storeSab;
  const oldNode = nodes[nodes.length - 1];

  await host.rebuild();

  // New context + node were built; old node got destroy + disconnect.
  assert.equal(Ctx.created.length, 2, "a second context was constructed");
  assert.ok(nodes.length >= 2, "a second node was constructed");
  assert.equal(oldNode.posted.filter((m) => m.type === "destroy").length, 1);
  assert.equal(oldNode.disconnected, true);
  assert.equal(host.ctx.state, "running");
  assert.equal(host.gateState, "running");

  // SAME SABs: transport state carried across the rebuild.
  assert.equal(host.ringSab, ringBefore);
  assert.equal(host.storeSab, storeBefore);
});

// --- teardown ---------------------------------------------------------------

test("teardown closes the context, destroys the worklet, drops SAB refs", async () => {
  const { host, lastNode } = makeHost();
  await host.start();
  const node = lastNode();
  const ctx = host.ctx;

  await host.teardown();

  assert.equal(node.posted.filter((m) => m.type === "destroy").length, 1, "worklet destroyed");
  assert.equal(node.disconnected, true, "node detached");
  assert.equal(ctx.closed, true, "context closed");
  assert.equal(host.ctx, null);
  assert.equal(host.node, null);
  assert.equal(host.ringSab, null, "ring SAB ref dropped");
  assert.equal(host.storeSab, null, "store SAB ref dropped");
  assert.equal(host.ring, null);
  assert.equal(host.store, null);
  assert.equal(host.gateState, "closed");
});

test("dispose() is a teardown alias (0042 back-compat)", async () => {
  const { host } = makeHost();
  await host.start();
  await host.dispose();
  assert.equal(host.gateState, "closed");
  assert.equal(host.ringSab, null);
});

test("a fresh WebHost boots cleanly after a previous one was torn down", async () => {
  const a = makeHost();
  await a.host.start();
  await a.host.teardown();

  // A brand-new WebHost (new SABs, new context) reaches running.
  const b = makeHost();
  await b.host.start();
  assert.equal(b.host.gateState, "running");
  assert.notEqual(b.host.ringSab, a.host.ringSab, "fresh SABs, no shared leak");
});
