// The JS half of the telemetry payload contract (0316).
//
// `meterEvent` / `scopeEvent` here and
// `vxn1b_ui_web::serialise_custom_payload` in Rust produce the same frames for
// the same page — same key names, same +/-2 clamp, same 3-dp rounding. They are
// separate implementations because the two hosts hand the frame over
// differently (a Float32Array over a SAB here, a typed `MeterFrame` there), and
// that is a fair trade; transcribing the SHAPE twice with nothing comparing
// them was not.
//
// Both sides assert against `telemetry-payload.fixture.json`. Change either
// serialiser and one of the two tests fails.
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/telemetry-payload.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { meterEvent, scopeEvent } from "./faceplate-bridge.mjs";

const FIXTURE = JSON.parse(
  readFileSync(fileURLToPath(new URL("./telemetry-payload.fixture.json", import.meta.url)), "utf8"),
);

// Through a Float32Array, the way the telemetry reader delivers it — so the
// values this side sees are f32-widened exactly like Rust's.
const f32 = (xs) => Float32Array.from(xs);

test("meterEvent matches the cross-language fixture", () => {
  assert.deepEqual(meterEvent(f32(FIXTURE.meterIn)), FIXTURE.meterOut);
});

test("scopeEvent clamps and rounds to the cross-language fixture", () => {
  assert.deepEqual(scopeEvent(f32(FIXTURE.scopeIn)), FIXTURE.scopeOut);
});

// The clamp is not cosmetic: without it one runaway sample sets the canvas's
// whole vertical scale, and the frame carries a number nobody can see.
test("the scope clamp holds at the rails in both directions", () => {
  const out = scopeEvent(f32([1e9, -1e9]));
  assert.deepEqual(out.s, [2, -2]);
});

// An empty window is a legal frame (the tap just turned on), not an error.
test("an empty scope window serialises to an empty array", () => {
  assert.deepEqual(scopeEvent(f32([])), { kind: "scope", s: [] });
});
