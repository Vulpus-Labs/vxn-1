// Bridge opcode routing (ticket 0157). Run: node --test ...
//
// Exercises `routeOpcode` against a mock controller — proving every faceplate
// dispatch opcode maps to the right C-ABI call, including the numeric-`op`
// collision cases (set_op_tab / set_ks_curve / set_eg_curve) that the
// `bootstrap.js.dispatch` merge produces on the wire.

import { test } from "node:test";
import assert from "node:assert/strict";
import { routeOpcode } from "./faceplate-bridge.mjs";

function mockController() {
  const calls = [];
  const rec = (name) => (...args) => calls.push([name, ...args]);
  return {
    calls,
    beginGesture: rec("beginGesture"),
    endGesture: rec("endGesture"),
    setParam: rec("setParam"),
    setParamNorm: rec("setParamNorm"),
    editorReady: rec("editorReady"),
    requestFullRebroadcast: rec("requestFullRebroadcast"),
    setOpTab: rec("setOpTab"),
    setMatrixRow: rec("setMatrixRow"),
    setKsCurve: rec("setKsCurve"),
    setEgCurve: rec("setEgCurve"),
    requestMatrixSnapshot: rec("requestMatrixSnapshot"),
    requestKsCurveSnapshot: rec("requestKsCurveSnapshot"),
    requestEgCurveSnapshot: rec("requestEgCurveSnapshot"),
    loadFactory: rec("loadFactory"),
    stepPreset: rec("stepPreset"),
  };
}

test("string opcodes route to the matching C-ABI call", () => {
  const c = mockController();
  routeOpcode(c, { op: "begin_gesture", id: 5 });
  routeOpcode(c, { op: "set_param_norm", id: 5, norm: 0.4 });
  routeOpcode(c, { op: "set_param", id: 5, plain: 12.5 });
  routeOpcode(c, { op: "end_gesture", id: 5 });
  routeOpcode(c, { op: "ready" });
  routeOpcode(c, { op: "request_full_rebroadcast" });
  routeOpcode(c, { op: "request_matrix_snapshot" });
  assert.deepEqual(c.calls, [
    ["beginGesture", 5],
    ["setParamNorm", 5, 0.4],
    ["setParam", 5, 12.5],
    ["endGesture", 5],
    ["editorReady"],
    ["requestFullRebroadcast"],
    ["requestMatrixSnapshot"],
  ]);
});

test("set_matrix_row unpacks the row fields", () => {
  const c = mockController();
  routeOpcode(c, { op: "set_matrix_row", slot: 9, source: 2, dest: 3, curve: 1, active: true, depth: 0.5 });
  assert.deepEqual(c.calls, [["setMatrixRow", 9, 2, 3, 1, true, 0.5]]);
});

test("numeric op == set_op_tab (operator index, no side/curve)", () => {
  const c = mockController();
  // dispatch('set_op_tab', {op: 3}) merges to {op: 3}.
  routeOpcode(c, { op: 3 });
  assert.deepEqual(c.calls, [["setOpTab", 3]]);
});

test("numeric op + side + curve == set_ks_curve", () => {
  const c = mockController();
  // dispatch('set_ks_curve', {op: 2, side: 1, curve: 3}) → {op: 2, side: 1, curve: 3}.
  routeOpcode(c, { op: 2, side: 1, curve: 3 });
  assert.deepEqual(c.calls, [["setKsCurve", 2, 1, 3]]);
});

test("numeric op + curve (no side) == set_eg_curve", () => {
  const c = mockController();
  // dispatch('set_eg_curve', {op: 4, curve: 1}) → {op: 4, curve: 1}.
  routeOpcode(c, { op: 4, curve: 1 });
  assert.deepEqual(c.calls, [["setEgCurve", 4, 1]]);
});

test("side/curve at operator 0 still disambiguate (falsy op index)", () => {
  const c = mockController();
  routeOpcode(c, { op: 0, side: 0, curve: 0 });
  routeOpcode(c, { op: 0, curve: 1 });
  routeOpcode(c, { op: 0 });
  assert.deepEqual(c.calls, [
    ["setKsCurve", 0, 0, 0],
    ["setEgCurve", 0, 1],
    ["setOpTab", 0],
  ]);
});

test("factory preset opcodes route (minimal 0159)", () => {
  const c = mockController();
  routeOpcode(c, { op: "load_factory", index: 3 });
  routeOpcode(c, { op: "step_preset", delta: -1 });
  routeOpcode(c, { op: "step_preset", dir: "next" }); // delta inferred
  assert.deepEqual(c.calls, [
    ["loadFactory", 3],
    ["stepPreset", -1],
    ["stepPreset", 1],
  ]);
});

test("a known-but-deferred opcode is accepted without a controller call", () => {
  const c = mockController();
  assert.equal(routeOpcode(c, { op: "request_text_input", id: "x" }), true);
  assert.equal(routeOpcode(c, { op: "save_preset", name: "x" }), true); // user op deferred
  assert.equal(c.calls.length, 0);
});

test("an unknown opcode returns false", () => {
  const c = mockController();
  assert.equal(routeOpcode(c, { op: "explode" }), false);
});
