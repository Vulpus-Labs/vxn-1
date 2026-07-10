// AudioContext lifecycle tests for the coordinator (ticket 0156).
//
// Run: node --test crates/vxn2-wasm/web/coordinator-lifecycle.test.mjs
//
// Exercise the MAIN-THREAD lifecycle state machine the WebHost layers on —
// autoplay unlock, suspend/resume voice flush, device change / rebuild, and
// teardown — against a MOCK AudioContext (no wasm, no audio clock). Ported from
// vxn-1's coordinator-lifecycle test; the vxn-2 change is that there's no
// key-mode/split replay to cover.

import { test } from "node:test";
import assert from "node:assert/strict";
import { WebHost } from "./coordinator.mjs";

// --- mocks ------------------------------------------------------------------

function makeMockNode() {
  const posted = [];
  return {
    posted,
    port: { onmessage: null, postMessage: (m) => posted.push(m) },
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
      if (opts.setSinkId !== false) {
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
  assert.equal(host.gateState, "idle");
  assert.equal(host.ctx, null);
  await host.start();
  assert.equal(host.gateState, "running");
  assert.equal(host.ctx.state, "running");
  assert.deepEqual(states, ["starting", "running"]);
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

  host.setParam(7, 0.5); // 0.5 is exact in f32
  const ringBefore = host.ringSab;
  const storeBefore = host.storeSab;

  ctx.dispatchStateChange("suspended");
  assert.equal(host.gateState, "suspended");
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 0);

  ctx.dispatchStateChange("running");
  assert.equal(host.gateState, "running");
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 1);

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
  await host.start();
  const node = lastNode();
  await host.resume();
  assert.equal(node.posted.filter((m) => m.type === "reset").length, 0);
});

// --- device change ----------------------------------------------------------

test("devicechange listener attaches on start and detaches on teardown", async () => {
  const md = makeMockMediaDevices();
  const { host } = makeHost({ mediaDevices: md });
  await host.start();
  assert.equal(md.listeners.length, 1);
  md.emit();
  assert.equal(host.gateState, "running");
  await host.teardown();
  assert.equal(md.listeners.length, 0);
});

test("setSink re-routes via setSinkId without rebuilding the graph", async () => {
  const { host, nodes } = makeHost();
  await host.start();
  const before = nodes.length;
  assert.equal(await host.setSink("device-xyz"), true);
  assert.equal(host.ctx.sinkId, "device-xyz");
  assert.equal(nodes.length, before);
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

  assert.equal(Ctx.created.length, 2);
  assert.ok(nodes.length >= 2);
  assert.equal(oldNode.posted.filter((m) => m.type === "destroy").length, 1);
  assert.equal(oldNode.disconnected, true);
  assert.equal(host.ctx.state, "running");
  assert.equal(host.gateState, "running");
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
  assert.equal(node.posted.filter((m) => m.type === "destroy").length, 1);
  assert.equal(node.disconnected, true);
  assert.equal(ctx.closed, true);
  assert.equal(host.ctx, null);
  assert.equal(host.node, null);
  assert.equal(host.ringSab, null);
  assert.equal(host.storeSab, null);
  assert.equal(host.ring, null);
  assert.equal(host.store, null);
  assert.equal(host.gateState, "closed");
});

test("a fresh WebHost boots cleanly after a previous one was torn down", async () => {
  const a = makeHost();
  await a.host.start();
  await a.host.teardown();
  const b = makeHost();
  await b.host.start();
  assert.equal(b.host.gateState, "running");
  assert.notEqual(b.host.ringSab, a.host.ringSab);
});
