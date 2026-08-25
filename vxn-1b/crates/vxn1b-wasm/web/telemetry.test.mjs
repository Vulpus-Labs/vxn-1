// Headless test for the audio -> view telemetry channel (0288).
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/telemetry.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import {
  createTelemetrySAB,
  telemetryBytes,
  TelemetryWriter,
  TelemetryReader,
  CTRL_BYTES,
  I_METER_SEQ,
  I_SCOPE_SEQ,
  I_SCOPE_LEN,
} from "./telemetry.mjs";

// f32 round-trip: the SAB stores 32-bit floats, so 0.9 comes back as
// 0.8999999761581421. Compare against the rounded value, not the literal.
const f32 = (x) => Math.fround(x);

const METER_LEN = 11;
const SCOPE_WINDOW = 384;

/// A stand-in for the wasm shim. `peaks` is what a drain returns; it clears
/// afterwards, mirroring the read-and-clear contract of MeterFrame::drain.
function fakeEngine({ scope = null } = {}) {
  const meter = new Float32Array(METER_LEN);
  const scopeBuf = new Float32Array(SCOPE_WINDOW);
  let pending = new Float32Array(METER_LEN);
  return {
    // test-side: publish a peak into the bus
    publish(index, value) {
      pending[index] = Math.max(pending[index], value);
    },
    setScope(fill) {
      scopeBuf.set(typeof fill === "function" ? Array.from(scopeBuf, (_, i) => fill(i)) : fill);
    },
    drains: 0,
    drainMeters() {
      this.drains++;
      meter.set(pending);
      pending = new Float32Array(METER_LEN); // read-and-clear
    },
    meterFrame: () => meter,
    readScope: () => (scope === false ? 0 : SCOPE_WINDOW),
    scopeSamples: () => scopeBuf,
  };
}

const writerFor = (sab, engine, sampleRate = 48000) =>
  new TelemetryWriter(sab, { meterLen: METER_LEN, scopeWindow: SCOPE_WINDOW, engine, sampleRate });

const readerFor = (sab) =>
  new TelemetryReader(sab, { meterLen: METER_LEN, scopeWindow: SCOPE_WINDOW });

const sab = () => createTelemetrySAB(METER_LEN, SCOPE_WINDOW);

test("the SAB is sized for ctrl plus both regions", () => {
  assert.equal(telemetryBytes(METER_LEN, SCOPE_WINDOW), CTRL_BYTES + (METER_LEN + SCOPE_WINDOW) * 4);
  assert.equal(sab().byteLength, telemetryBytes(METER_LEN, SCOPE_WINDOW));
});

test("a published meter frame reaches the reader", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);

  engine.publish(9, 0.5); // masterL
  w.publishMeters();

  const frame = r.readMeters();
  assert.ok(frame, "a non-silent frame must be delivered");
  assert.equal(frame.length, METER_LEN);
  assert.equal(frame[9], 0.5);
});

test("the reader returns null until something new is published", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);

  engine.publish(9, 0.5);
  w.publishMeters();
  assert.ok(r.readMeters(), "first read delivers");
  assert.equal(r.readMeters(), null, "an unchanged frame is not re-delivered");

  engine.publish(9, 0.75);
  w.publishMeters();
  assert.equal(r.readMeters()[9], f32(0.75), "a new publish delivers again");
});

// ── The seqlock ────────────────────────────────────────────────────────────

test("a reader will not return a frame while the writer is mid-update", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);
  engine.publish(9, 0.5);
  w.publishMeters();
  r.readMeters(); // settle

  // Simulate the writer having entered its critical section and not left:
  // counter odd, region half-updated.
  const ctrl = new Int32Array(s, 0, 4);
  const meter = new Float32Array(s, CTRL_BYTES, METER_LEN);
  Atomics.store(ctrl, I_METER_SEQ, Atomics.load(ctrl, I_METER_SEQ) + 1); // odd
  meter[0] = 999; // a torn value the reader must never surface

  assert.equal(r.readMeters(), null, "an odd counter must yield nothing, not partial data");
});

test("a frame overwritten during the copy is retried, never returned torn", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);
  engine.publish(9, 0.25);
  w.publishMeters();
  r.readMeters();

  // Writer completes a whole new publish "between" the reader's two counter
  // reads: bump by 2 so the counter is even but different.
  const ctrl = new Int32Array(s, 0, 4);
  Atomics.store(ctrl, I_METER_SEQ, Atomics.load(ctrl, I_METER_SEQ) + 2);
  const frame = r.readMeters();
  // The value it settles on is whatever is actually in the region — the point
  // is that it is one coherent frame, and the read completed.
  assert.ok(frame, "a completed republish is readable");
  assert.equal(frame.length, METER_LEN);
});

// ── Rate division ──────────────────────────────────────────────────────────

test("tick() publishes at ~60 Hz, not once per quantum", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine, 48000);
  // 48000 / 128 / 60 = 6.25 -> 6 quanta per publish.
  assert.equal(w.everyN, 6);

  for (let i = 0; i < 5; i++) assert.equal(w.tick(), false, `quantum ${i} must not publish`);
  assert.equal(w.tick(), true, "the 6th quantum publishes");
  assert.equal(engine.drains, 1, "exactly one drain per publish");
});

// The reason the division exists: read-and-clear means a drain discards
// everything it reports. Draining every quantum would throw away the peaks of
// the five quanta the reader never sees.
test("a published frame covers every quantum since the last publish", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine, 48000);
  const r = readerFor(s);

  // A transient in an early quantum, quiet afterwards.
  engine.publish(9, 0.9);
  for (let i = 0; i < 5; i++) w.tick();
  engine.publish(9, 0.1);
  w.tick(); // the 6th: publishes

  assert.equal(
    r.readMeters()[9],
    f32(0.9),
    "the transient must survive to the UI, not be discarded",
  );
});

test("the scope publishes at half the meter rate", () => {
  const s = sab();
  const engine = fakeEngine();
  engine.setScope((i) => Math.sin(i));
  const w = writerFor(s, engine, 48000);
  const r = readerFor(s);

  for (let i = 0; i < w.everyN; i++) w.tick(); // publish #1
  assert.equal(r.readScope(), null, "no scope on the first publish");
  for (let i = 0; i < w.everyN; i++) w.tick(); // publish #2
  assert.ok(r.readScope(), "scope arrives on the second");
});

test("a scope read with no full window publishes nothing", () => {
  const s = sab();
  const engine = fakeEngine({ scope: false }); // readScope() -> 0
  const w = writerFor(s, engine);
  const r = readerFor(s);
  assert.equal(w.publishScope(), false);
  assert.equal(Atomics.load(new Int32Array(s, 0, 4), I_SCOPE_LEN), 0);
  assert.equal(r.readScope(), null, "the tap being off must not fabricate a flat trace");
});

// ── Silence suppression ────────────────────────────────────────────────────

test("one silent frame is delivered, then silence is suppressed", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);

  // The view needs this one: it is the zero that starts the decay falling.
  w.publishMeters();
  assert.ok(r.readMeters(), "the first silent frame IS delivered");

  w.publishMeters();
  assert.equal(r.readMeters(), null, "a second silent frame is suppressed");
  w.publishMeters();
  assert.equal(r.readMeters(), null, "and stays suppressed");

  engine.publish(9, 0.4);
  w.publishMeters();
  assert.equal(r.readMeters()[9], f32(0.4), "audio resumes delivery");

  w.publishMeters();
  assert.ok(r.readMeters(), "and the next silent frame is delivered once more");
  w.publishMeters();
  assert.equal(r.readMeters(), null, "before suppressing again");
});

test("writer and reader over the same SAB allocate nothing per frame", () => {
  const s = sab();
  const engine = fakeEngine();
  const w = writerFor(s, engine);
  const r = readerFor(s);
  engine.publish(0, 0.1);
  w.publishMeters();
  const a = r.readMeters();
  engine.publish(0, 0.2);
  w.publishMeters();
  const b = r.readMeters();
  assert.equal(a.buffer, b.buffer, "the reader hands back a view over its reused scratch");
});

// ── End to end, through the real engine ────────────────────────────────────

const WASM = ["release", "debug"]
  .map((p) => fileURLToPath(new URL(`../../../../target/wasm32-unknown-unknown/${p}/vxn1b_wasm.wasm`, import.meta.url)))
  .find((p) => existsSync(p));

test("a sounding note reaches the main thread as meter and scope frames", async () => {
  assert.ok(
    WASM,
    "vxn1b_wasm.wasm not found — this test must not be skipped. Build it:\n" +
      '  RUSTFLAGS="-C target-feature=+simd128" cargo build -p vxn1b-wasm --target wasm32-unknown-unknown --release',
  );
  const { instance } = await WebAssembly.instantiate(readFileSync(WASM), {});
  const x = instance.exports;
  const { EventRing, createRingSAB } = await import("./event-ring.mjs");
  const { ev, encode, SLOT_BYTES } = await import("./event-codec.mjs");

  const meterLen = x.vxn1b_meter_len();
  const scopeWindow = x.vxn1b_scope_window();
  assert.equal(meterLen, METER_LEN, "the JS region size must match the engine's tap count");
  assert.equal(scopeWindow, SCOPE_WINDOW);

  const h = x.vxn1b_host_new(48000);
  // The wasm-export shim the writer drives.
  const engine = {
    drainMeters: () => x.vxn1b_host_drain_meters(h),
    meterFrame: () => new Float32Array(x.memory.buffer, x.vxn1b_host_meters_ptr(h), meterLen),
    readScope: () => x.vxn1b_host_read_scope(h),
    scopeSamples: (n) => new Float32Array(x.memory.buffer, x.vxn1b_host_scope_ptr(h), n),
  };

  const t = createTelemetrySAB(meterLen, scopeWindow);
  const w = new TelemetryWriter(t, { meterLen, scopeWindow, engine, sampleRate: 48000 });
  const r = new TelemetryReader(t, { meterLen, scopeWindow });

  // Point the scope at layer 1, then hold a note.
  const ring = new EventRing(createRingSAB());
  ring.pushScopeTap(0, 1);
  ring.pushNoteOn(0, 60, 1.0);
  const scratch = new Uint8Array(x.memory.buffer, x.vxn1b_host_events_ptr(h), SLOT_BYTES * 16);
  x.vxn1b_host_render(h, ring.drainRawInto(scratch));
  w.tick();

  let meters = null;
  let scope = null;
  for (let q = 0; q < 400 && (!meters || !scope); q++) {
    x.vxn1b_host_render(h, 0);
    w.tick();
    meters = meters || r.readMeters();
    scope = scope || r.readScope();
  }

  assert.ok(meters, "meter frames must reach the main thread");
  assert.ok(meters[9] > 0 || meters[10] > 0, `master meter must register audio, got ${meters}`);
  assert.ok(scope, "a scope frame must reach the main thread");
  assert.ok(
    scope.some((v) => v !== 0),
    "the captured window must not be flat while a note is sounding",
  );
  x.vxn1b_host_destroy(h);
});

// Regression: a reader constructed before the writer has ever published must
// report "nothing yet", not hand back its own zeroed region as a silent frame.
// That fabricated frame would consume the single silent frame the suppression
// rule allows, so the engine's real first frame would be the one dropped.
test("a reader with no publish yet returns null, not a fabricated silent frame", () => {
  const s = sab();
  const r = readerFor(s);
  assert.equal(r.readMeters(), null, "nothing has been published");
  assert.equal(r.readScope(), null);

  // ...and the first genuine (silent) frame still gets through afterwards.
  const w = writerFor(s, fakeEngine());
  w.publishMeters();
  assert.ok(r.readMeters(), "the engine's first frame is delivered");
});
