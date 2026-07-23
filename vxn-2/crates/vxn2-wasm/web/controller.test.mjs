// ViewEvent decode drift-guard (ticket 0157). Run: node --test ...
//
// Builds packed ViewEvent bytes by hand in the EXACT layout the Rust packer
// (vxn2-web-controller/src/lib.rs `pack_view_event`) emits, and asserts
// `decodeViewEvents` reproduces the event objects the faceplate's
// `applyViewEvents` consumes. If either side's wire layout drifts, this fails.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  WebController,
  decodeViewEvents,
  VE_PARAM_CHANGED,
  VE_OP_TAB_CHANGED,
  VE_MATRIX_SNAPSHOT,
  VE_KS_CURVE_SNAPSHOT,
  VE_EG_CURVE_SNAPSHOT,
  VE_PRESET_LOADED,
} from "./controller.mjs";

// Little-endian packer mirroring the Rust `push_u32` / `push_f32` / `push_str`.
function pack(records) {
  const bytes = [];
  const u32 = (v) => {
    bytes.push(v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff);
  };
  const f32 = (v) => {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, v, true);
    bytes.push(...b);
  };
  const str = (s) => {
    const enc = new TextEncoder().encode(s);
    u32(enc.length);
    bytes.push(...enc);
  };
  u32(records.length);
  for (const r of records) r({ u32, f32, str, u8: (v) => bytes.push(v & 0xff) });
  const buf = new Uint8Array(bytes);
  return buf.buffer;
}

test("decodes a param_changed record", () => {
  const buf = pack([
    ({ u32, f32, str }) => {
      u32(VE_PARAM_CHANGED);
      u32(42);
      f32(0.25);
      f32(0.5);
      str("1.50 dB");
    },
  ]);
  const evs = decodeViewEvents(buf, 0, buf.byteLength);
  assert.deepEqual(evs, [{ kind: "param_changed", id: 42, plain: 0.25, norm: 0.5, display: "1.50 dB" }]);
});

test("decodes op_tab_changed", () => {
  const buf = pack([
    ({ u32 }) => {
      u32(VE_OP_TAB_CHANGED);
      u32(3);
    },
  ]);
  assert.deepEqual(decodeViewEvents(buf, 0, buf.byteLength), [{ kind: "op_tab_changed", op: 3 }]);
});

test("decodes a 16-row matrix_snapshot", () => {
  const buf = pack([
    ({ u32, u8, f32 }) => {
      u32(VE_MATRIX_SNAPSHOT);
      u32(16);
      for (let i = 0; i < 16; i++) {
        u8(i); // source
        u8(i + 1); // dest
        u8(i % 4); // curve
        u8(i === 9 ? 1 : 0); // active
        f32(i === 9 ? 0.5 : 0.0); // depth
        u8(i === 9 ? 5 : 0); // scale (E033)
      }
    },
  ]);
  const evs = decodeViewEvents(buf, 0, buf.byteLength);
  assert.equal(evs.length, 1);
  assert.equal(evs[0].kind, "matrix_snapshot");
  assert.equal(evs[0].rows.length, 16);
  assert.deepEqual(evs[0].rows[9], { source: 9, dest: 10, curve: 1, active: true, depth: 0.5, scale: 5 });
  assert.equal(evs[0].rows[0].active, false);
  assert.equal(evs[0].rows[0].scale, 0);
});

test("decodes ks_curve_snapshot (6×[L,R]) and eg_curve_snapshot (6)", () => {
  const buf = pack([
    ({ u32, u8 }) => {
      u32(VE_KS_CURVE_SNAPSHOT);
      for (let i = 0; i < 6; i++) {
        u8(i % 4);
        u8((i + 2) % 4);
      }
    },
    ({ u32, u8 }) => {
      u32(VE_EG_CURVE_SNAPSHOT);
      for (let i = 0; i < 6; i++) u8(i % 2);
    },
  ]);
  const evs = decodeViewEvents(buf, 0, buf.byteLength);
  assert.equal(evs.length, 2);
  assert.equal(evs[0].kind, "ks_curve_snapshot");
  assert.deepEqual(evs[0].curves[0], [0, 2]);
  assert.deepEqual(evs[0].curves[5], [1, 3]);
  assert.equal(evs[1].kind, "eg_curve_snapshot");
  assert.deepEqual(evs[1].curves, [0, 1, 0, 1, 0, 1]);
});

test("decodes a factory preset_loaded record", () => {
  const buf = pack([
    ({ u32, str }) => {
      u32(VE_PRESET_LOADED);
      str("Init Bell");
      u32(1); // PRESET_SRC_FACTORY
      u32(7); // index
      u32(1); // warning count
      str("clamped X");
    },
  ]);
  assert.deepEqual(decodeViewEvents(buf, 0, buf.byteLength), [
    { kind: "preset_loaded", name: "Init Bell", source: { kind: "factory", index: 7 }, warnings: ["clamped X"] },
  ]);
});

test("decodes a preset_loaded with no source", () => {
  const buf = pack([
    ({ u32, str }) => {
      u32(VE_PRESET_LOADED);
      str("None");
      u32(0); // PRESET_SRC_NONE
      u32(0); // no warnings
    },
  ]);
  assert.deepEqual(decodeViewEvents(buf, 0, buf.byteLength), [
    { kind: "preset_loaded", name: "None", source: null, warnings: [] },
  ]);
});

test("throws on an unknown tag (drift tripwire)", () => {
  const buf = pack([
    ({ u32 }) => {
      u32(999);
    },
  ]);
  assert.throws(() => decodeViewEvents(buf, 0, buf.byteLength), /unknown ViewEvent tag/);
});

test("mirrors a matrix_snapshot to the ring, one pushMatrixRow per slot (0193)", () => {
  // Preset loads / reset restore the model and surface only a matrix_snapshot
  // (no setMatrixRow call), so the tick must fan the whole table to the worklet.
  const pushed = [];
  const ring = { pushMatrixRow: (...args) => pushed.push(args), pushPatchSwap: () => {} };
  const c = new WebController({ ring });
  c._mirrorControlToRing([
    { kind: "param_changed", id: 1, plain: 0.5 }, // ignored
    {
      kind: "matrix_snapshot",
      rows: [
        { source: 4, dest: 28, curve: 0, active: true, depth: 0.9, scale: 5 },
        { source: 2, dest: 29, curve: 3, active: false, depth: 0.5 },
      ],
    },
  ]);
  // Trailing arg is the E033 scale source (absent → 0).
  assert.deepEqual(pushed, [
    [0, 4, 28, 0, true, 0.9, 5],
    [1, 2, 29, 3, false, 0.5, 0],
  ]);
});

test("preset_loaded pushes a patchSwap BEFORE the matrix rows (0193 silence)", () => {
  const order = [];
  const ring = {
    pushPatchSwap: () => order.push("swap"),
    pushMatrixRow: () => order.push("row"),
  };
  const c = new WebController({ ring });
  c._mirrorControlToRing([
    { kind: "matrix_snapshot", rows: [{ source: 1, dest: 2, curve: 0, active: true, depth: 0 }] },
    { kind: "preset_loaded", name: "X", source: { kind: "factory", index: 0 }, warnings: [] },
  ]);
  // Swap first (silence the old patch), then the new topology — regardless of
  // the events' arrival order in the drain.
  assert.deepEqual(order, ["swap", "row"]);
});

test("_mirrorControlToRing is a no-op without a ring", () => {
  const c = new WebController({}); // no ring
  assert.doesNotThrow(() =>
    c._mirrorControlToRing([
      { kind: "preset_loaded", name: "X", source: null, warnings: [] },
      { kind: "matrix_snapshot", rows: [{ source: 1, dest: 2, curve: 0, active: true, depth: 0 }] },
    ]),
  );
});
