// Headless tests for the worklet render loop and its lifecycle policy (0289).
//
// The render path runs against the REAL wasm; the failure paths run against a
// fake exports object, because the only way to reach them with real wasm would
// be to add a force-trap export to the shipped ABI purely for testing.
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/host-runner.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { WorkletHostRunner } from "./host-runner.mjs";
import { AudioHost } from "./audio-host.mjs";
import { EventRing, createRingSAB } from "./event-ring.mjs";
import { ParamStore, createParamSAB, TOTAL_PARAMS } from "./param-store.mjs";
import { createTelemetrySAB, TelemetryReader } from "./telemetry.mjs";

const WASM = ["release", "debug"]
  .map((p) =>
    fileURLToPath(new URL(`../../../../target/wasm32-unknown-unknown/${p}/vxn1b_wasm.wasm`, import.meta.url)),
  )
  .find((p) => existsSync(p));

// Asserted, never skipped — a missing artifact is a setup problem with a known
// fix, and skipping turns "I could not check this" into a green tick.
function wasmBytes() {
  assert.ok(
    WASM,
    "vxn1b_wasm.wasm not found — this test must not be skipped. Build it:\n" +
      '  RUSTFLAGS="-C target-feature=+simd128" cargo build -p vxn1b-wasm --target wasm32-unknown-unknown --release',
  );
  return readFileSync(WASM);
}

const Q = 128;
const buffers = () => [new Float32Array(Q), new Float32Array(Q)];
const peak = (a) => a.reduce((m, y) => Math.max(m, Math.abs(y)), 0);

async function runner(extra = {}) {
  const r = new WorkletHostRunner({
    wasmBytes: wasmBytes(),
    sampleRate: 48000,
    ...extra,
  });
  await r.init();
  return r;
}

// ── Render path, against the real engine ───────────────────────────────────

test("the runner renders silence before it is ready", () => {
  const r = new WorkletHostRunner({ wasmBytes: wasmBytes(), sampleRate: 48000 });
  const [l, right] = buffers();
  l.fill(0.5); // prove it actively zeroes rather than leaving the buffer alone
  assert.equal(r.process(l, right), false, "not ready yet");
  assert.equal(peak(l), 0, "output must be silent, not stale");
});

test("a note pushed onto the ring before ready is not lost", async () => {
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  // Written while the runner is still instantiating: the ring's read index is
  // untouched until the worklet drains, so nothing is dropped.
  ring.pushNoteOn(0, 60, 1.0);

  const r = await runner({ ringSab });
  const [l, right] = buffers();
  r.process(l, right);
  assert.ok(peak(l) > 0, "the buffered note must sound on the first live quantum");
});

test("the store fold seeds the engine before the first render", async () => {
  const storeSab = createParamSAB();
  const store = new ParamStore(storeSab);
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);

  // A zeroed store is the pathological case the coordinator's seeding exists to
  // prevent: the NaN-seeded fold applies every id, so every param goes to 0.
  const r = await runner({ ringSab, storeSab });
  ring.pushNoteOn(0, 60, 1.0);
  const [l, right] = buffers();
  r.process(l, right);
  // With every param zeroed the instrument is silent — which is exactly why
  // start() seeds defaults before the worklet can read the store.
  assert.equal(peak(l), 0, "an unseeded store folds zeros over every param");

  // Seeded with real defaults, the same note sounds.
  const x = (await WebAssembly.instantiate(wasmBytes(), {})).instance.exports;
  const h = x.vxn1b_host_new(48000);
  const vals = new Float32Array(TOTAL_PARAMS);
  for (let id = 0; id < TOTAL_PARAMS; id++) vals[id] = x.vxn1b_host_get_param(h, id);
  x.vxn1b_host_destroy(h);
  store.writeBulk(vals);

  const r2 = await runner({ ringSab, storeSab });
  ring.pushNoteOn(0, 60, 1.0);
  const [l2, r2out] = buffers();
  r2.process(l2, r2out);
  assert.ok(peak(l2) > 0, "a seeded store lets the note sound");
});

test("the steady-state render does not rebuild its memory views", async () => {
  const r = await runner({ ringSab: createRingSAB() });
  const [l, right] = buffers();
  r.process(l, right);
  const host = r.host;
  const views = [host._eventsU8, host._outLview, host._outRview];
  for (let i = 0; i < 32; i++) r.process(l, right);
  assert.equal(host._eventsU8, views[0], "event scratch view reused");
  assert.equal(host._outLview, views[1], "left output view reused");
  assert.equal(host._outRview, views[2], "right output view reused");
});

test("telemetry ticks with the render and reaches a reader", async () => {
  const x = (await WebAssembly.instantiate(wasmBytes(), {})).instance.exports;
  const meterLen = x.vxn1b_meter_len();
  const scopeWindow = x.vxn1b_scope_window();
  const telemetrySab = createTelemetrySAB(meterLen, scopeWindow);
  const reader = new TelemetryReader(telemetrySab, { meterLen, scopeWindow });

  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  const r = await runner({ ringSab, telemetrySab });

  ring.pushNoteOn(0, 60, 1.0);
  const [l, right] = buffers();
  let meters = null;
  for (let i = 0; i < 64 && !meters; i++) {
    r.process(l, right);
    meters = reader.readMeters();
  }
  assert.ok(meters, "meter frames must reach the reader");
  assert.ok(
    meters.some((v) => v !== 0),
    "a sounding note must register on some tap",
  );
});

// ── Failure policy, against a fake ─────────────────────────────────────────
//
// A fake rather than a force-trap export: the catch is JS-level, so a throwing
// `vxn1b_host_render` proves the same boundary without putting a test-only
// function in the shipped ABI (which is what vxn-1 had to do).

function throwingWasm({ throwOn = 1 } = {}) {
  let calls = 0;
  const mem = new WebAssembly.Memory({ initial: 4 });
  return {
    memory: mem,
    vxn1b_host_new: () => 1,
    vxn1b_host_destroy: () => {},
    vxn1b_quantum: () => Q,
    vxn1b_host_max_events: () => 8,
    vxn1b_host_events_ptr: () => 0,
    vxn1b_host_out_l: () => 4096,
    vxn1b_host_out_r: () => 8192,
    vxn1b_host_set_param: () => {},
    vxn1b_host_reset: () => {},
    vxn1b_host_set_sample_rate: () => {},
    vxn1b_host_render: () => {
      if (++calls === throwOn) throw new Error("unreachable executed");
    },
  };
}

test("a render trap goes silent, reports, and does not throw out of process()", async () => {
  const traps = [];
  const r = new WorkletHostRunner({ sampleRate: 48000, onTrap: (e, n) => traps.push([String(e.message), n]) });
  // Hand the runner a live host built over the throwing fake.
  r.host = new AudioHost(throwingWasm(), { sampleRate: 48000 });
  r.ready = true;

  const [l, right] = buffers();
  l.fill(0.5);
  assert.doesNotThrow(() => r.process(l, right), "a trap must not escape the worklet boundary");
  assert.equal(peak(l), 0, "output goes silent on a trap");
  assert.equal(traps.length, 1);
  assert.equal(traps[0][1], 1, "trap count reported");
  assert.equal(r.ready, false, "the poisoned instance is dropped");
});

test("after a trap the runner re-instantiates over the same SABs and recovers", async () => {
  const ringSab = createRingSAB();
  const ring = new EventRing(ringSab);
  const r = await runner({ ringSab });

  // Poison the live host, then render: the runner catches, drops it, and kicks
  // an async rebuild.
  r.host = new AudioHost(throwingWasm(), { ringSab, sampleRate: 48000 });
  const [l, right] = buffers();
  r.process(l, right);
  assert.equal(r.ready, false);

  // Let the async re-instantiate settle.
  for (let i = 0; i < 10 && !r.ready; i++) await new Promise((res) => setTimeout(res, 5));
  assert.equal(r.ready, true, "the runner rebuilds itself");

  // The SABs survived, so a note pushed now still sounds.
  ring.pushNoteOn(0, 60, 1.0);
  r.process(l, right);
  assert.ok(peak(l) > 0, "audio recovers over the same transport");
});

test("the default trap handler is loud rather than silent", () => {
  const r = new WorkletHostRunner({ sampleRate: 48000 });
  const seen = [];
  const orig = console.warn;
  console.warn = (...a) => seen.push(a.join(" "));
  try {
    r.host = new AudioHost(throwingWasm(), { sampleRate: 48000 });
    r.ready = true;
    r.process(...buffers());
  } finally {
    console.warn = orig;
  }
  // A trap rebuilds the engine and loses the non-automatable state; that must
  // not pass unremarked just because nobody registered a handler.
  assert.ok(
    seen.some((s) => /render trap/.test(s) && /re-broadcast/.test(s)),
    `expected a warning naming the consequence, got ${JSON.stringify(seen)}`,
  );
});

test("destroy releases the engine and the SAB references", async () => {
  const r = await runner({ ringSab: createRingSAB(), storeSab: createParamSAB() });
  r.destroy();
  assert.equal(r.ready, false);
  assert.equal(r.host, null);
  assert.equal(r.ringSab, null);
  assert.equal(r.storeSab, null);
  assert.equal(r.wasmBytes, null);
  // Still safe to call — the worklet keeps calling process() until the node dies.
  const [l, right] = buffers();
  assert.equal(r.process(l, right), false);
  assert.equal(peak(l), 0);
});
