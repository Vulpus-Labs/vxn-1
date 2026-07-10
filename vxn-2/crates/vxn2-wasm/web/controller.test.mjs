// ViewEvent decode drift-guard (ticket 0157). Run: node --test ...
//
// Builds packed ViewEvent bytes by hand in the EXACT layout the Rust packer
// (vxn2-web-controller/src/lib.rs `pack_view_event`) emits, and asserts
// `decodeViewEvents` reproduces the event objects the faceplate's
// `applyViewEvents` consumes. If either side's wire layout drifts, this fails.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  decodeViewEvents,
  VE_PARAM_CHANGED,
  VE_OP_TAB_CHANGED,
  VE_MATRIX_SNAPSHOT,
  VE_KS_CURVE_SNAPSHOT,
  VE_EG_CURVE_SNAPSHOT,
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
      }
    },
  ]);
  const evs = decodeViewEvents(buf, 0, buf.byteLength);
  assert.equal(evs.length, 1);
  assert.equal(evs[0].kind, "matrix_snapshot");
  assert.equal(evs[0].rows.length, 16);
  assert.deepEqual(evs[0].rows[9], { source: 9, dest: 10, curve: 1, active: true, depth: 0.5 });
  assert.equal(evs[0].rows[0].active, false);
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

test("throws on an unknown tag (drift tripwire)", () => {
  const buf = pack([
    ({ u32 }) => {
      u32(999);
    },
  ]);
  assert.throws(() => decodeViewEvents(buf, 0, buf.byteLength), /unknown ViewEvent tag/);
});
