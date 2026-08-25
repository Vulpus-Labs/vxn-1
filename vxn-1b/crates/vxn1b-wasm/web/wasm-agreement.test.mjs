// The JS mirror vs the real engine (0287).
//
// `event-codec.mjs` hand-declares VXN1b's param-space size, because the browser
// has no build step that could read the Rust table. Ticket 0285 is what that
// costs when it rots: vxn-1 and vxn-2 BOTH shipped browser builds that could not
// boot, for weeks, because their declared counts drifted behind engines that had
// grown params. The runtime handshake caught it immediately — nobody ran it.
//
// So this test reads the count out of the BUILT wasm artifact and asserts the
// mirror agrees.
//
// It deliberately FAILS, not skips, when the artifact is missing. vxn-2's
// wasm-backed tests skip on a missing artifact, and because both ports' `xtask
// web` write and wipe the same `target/web-dist`, a normal run reported "89
// pass" with 13 tests silently skipped — including every one that would have
// caught 0285. A test that quietly opts out of its own coverage is worse than no
// test, because it reads as green.
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/wasm-agreement.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { TOTAL_PARAMS, SLOT_BYTES } from "./event-codec.mjs";

const BUILD_HINT =
  'RUSTFLAGS="-C target-feature=+simd128" cargo build -p vxn1b-wasm ' +
  "--target wasm32-unknown-unknown --release";

// The crate's OWN artifact, under its own target dir — not a shared bundle
// directory another product's build can delete out from under it.
const CANDIDATES = ["release", "debug"].map((profile) =>
  fileURLToPath(
    new URL(`../../../../target/wasm32-unknown-unknown/${profile}/vxn1b_wasm.wasm`, import.meta.url),
  ),
);

function wasmPath() {
  const found = CANDIDATES.find((p) => existsSync(p));
  assert.ok(
    found,
    `vxn1b_wasm.wasm not found. This test must not be skipped — build it first:\n  ${BUILD_HINT}`,
  );
  return found;
}

async function exports_() {
  const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath()), {});
  return instance.exports;
}

test("the JS param mirror matches the engine's TOTAL_PARAMS", async () => {
  const x = await exports_();
  assert.equal(
    TOTAL_PARAMS,
    x.vxn1b_total_params(),
    "event-codec.mjs's declared count has drifted from the engine — update " +
      "PATCH_COUNT / GLOBAL_COUNT there and the counts in WIRE-FORMAT.md",
  );
});

test("the JS quantum and slot size match the engine", async () => {
  const x = await exports_();
  assert.equal(x.vxn1b_quantum(), 128, "Web Audio render quantum");
  // The scratch is sized in records; the ring's slot stride must agree or the
  // raw drain would write misaligned records into linear memory.
  assert.equal(SLOT_BYTES, 16);
  assert.ok(x.vxn1b_host_max_events() > 0);
});

test("the module needs no imports (it must instantiate in an AudioWorklet)", async () => {
  const mod = await WebAssembly.compile(readFileSync(wasmPath()));
  assert.deepEqual(
    WebAssembly.Module.imports(mod),
    [],
    "an AudioWorkletGlobalScope has no DOM, no fetch, and nothing to satisfy an import with",
  );
});

test("every export the JS transport calls is present", async () => {
  const mod = await WebAssembly.compile(readFileSync(wasmPath()));
  const got = new Set(
    WebAssembly.Module.exports(mod)
      .filter((e) => e.kind === "function")
      .map((e) => e.name),
  );
  for (const name of [
    "vxn1b_host_new",
    "vxn1b_host_destroy",
    "vxn1b_host_events_ptr",
    "vxn1b_host_max_events",
    "vxn1b_host_set_param",
    "vxn1b_host_get_param",
    "vxn1b_host_render",
    "vxn1b_host_out_l",
    "vxn1b_host_out_r",
    "vxn1b_host_reset",
    "vxn1b_quantum",
    "vxn1b_total_params",
  ]) {
    assert.ok(got.has(name), `missing export ${name}`);
  }
  assert.ok(
    WebAssembly.Module.exports(mod).some((e) => e.kind === "memory"),
    "linear memory must be exported — the ring drains raw bytes into it",
  );
});

// End-to-end through the real artifact: the ring's wire bytes ARE the codec's
// input, so a record the JS producer wrote must render as audio in Rust, at the
// sample offset it asked for.
test("a JS-encoded note-on renders in the real engine at its sample offset", async () => {
  const { EventRing, createRingSAB } = await import("./event-ring.mjs");
  const x = await exports_();
  const Q = x.vxn1b_quantum();

  const ring = new EventRing(createRingSAB());
  assert.ok(ring.pushNoteOn(64, 60, 1.0, 3), "push must succeed on an empty ring");

  const h = x.vxn1b_host_new(48000);
  const scratch = new Uint8Array(x.memory.buffer, x.vxn1b_host_events_ptr(h), SLOT_BYTES * 8);
  const n = ring.drainRawInto(scratch);
  assert.equal(n, 1, "one record drained");

  x.vxn1b_host_render(h, n);
  const out = new Float32Array(x.memory.buffer, x.vxn1b_host_out_l(h), Q);
  const peak = (a) => a.reduce((m, y) => Math.max(m, Math.abs(y)), 0);

  assert.equal(peak(out.subarray(0, 64)), 0, "silent before the note-on offset");
  assert.ok(peak(out.subarray(64)) > 0, "sounding after it");
  x.vxn1b_host_destroy(h);
});
