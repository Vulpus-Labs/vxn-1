// Golden-byte parity test for the packed ViewEvent wire protocol (ticket 0083).
//
// The hand-walked binary ViewEvent format crosses two languages WITHOUT a codec:
// the Rust packer (vxn-web-controller/src/lib.rs `pack_view_event`) and the JS
// unpacker (vxn-wasm/web/controller.mjs `decodeViewEvents`). The GOLDEN BYTE
// TABLE below is byte-for-byte identical to the Rust one in lib.rs `view_golden()`:
// Rust asserts its packer EMITS these bytes, and here we assert the JS decoder
// DECODES these bytes back to the equivalent structs. If either side's offset
// math, endianness, length-prefix order, or tag numbering drifts, one of the two
// tests fails in CI rather than silently mis-reading at runtime.
//
// Pattern copied from src/codec.rs `golden()` + web/event-codec.test.mjs.

import { describe, it, expect } from 'vitest';
import {
  decodeViewEvents,
  VE_PARAM_CHANGED,
  VE_KEY_MODE_CHANGED,
  VE_SPLIT_POINT_CHANGED,
  VE_EDIT_LAYER_CHANGED,
  VE_PRESET_LOADED,
  VE_PRESET_CORPUS_CHANGED,
  PRESET_SRC_NONE,
  PRESET_SRC_FACTORY,
  PRESET_SRC_USER,
  KEY_MODE_WHOLE,
  KEY_MODE_DUAL,
  KEY_MODE_SPLIT,
  LAYER_UPPER,
  LAYER_LOWER,
} from '../../../vxn-wasm/web/controller.mjs';

// ── byte builders (mirror the Rust `v*` helpers; little-endian) ──────────────
const enc = new TextEncoder();
function u32(b, v) {
  b.push(v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff);
}
function f32(b, v) {
  const tmp = new Uint8Array(4);
  new DataView(tmp.buffer).setFloat32(0, v, true);
  b.push(tmp[0], tmp[1], tmp[2], tmp[3]);
}
function str(b, s) {
  const bytes = enc.encode(s);
  u32(b, bytes.length);
  for (const x of bytes) b.push(x);
}

// Decode a flat byte array (count header + records) the way controller.mjs does.
function decode(bytes) {
  const u8 = new Uint8Array(bytes);
  return decodeViewEvents(u8.buffer, u8.byteOffset, u8.byteLength);
}

// ── golden records: (label, recordBytes, decodedStruct) ──────────────────────
// IDENTICAL to vxn-web-controller/src/lib.rs `view_golden()`. Each `bytes` is a
// single record WITHOUT the batch count header (added per-test).
const GOLDEN = [
  (() => {
    const b = [];
    u32(b, VE_PARAM_CHANGED);
    u32(b, 42);
    f32(b, 1.5);
    f32(b, 0.25);
    str(b, '1.50 Hz');
    return ['param_changed id42', b, { type: 'ParamChanged', id: 42, plain: 1.5, norm: 0.25, display: '1.50 Hz' }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_KEY_MODE_CHANGED);
    u32(b, KEY_MODE_SPLIT); // 2
    return ['key_mode split', b, { type: 'KeyModeChanged', mode: 2 }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_SPLIT_POINT_CHANGED);
    u32(b, 60);
    return ['split_point 60', b, { type: 'SplitPointChanged', note: 60 }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_EDIT_LAYER_CHANGED);
    u32(b, LAYER_LOWER); // 1
    return ['edit_layer lower', b, { type: 'EditLayerChanged', layer: 1 }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_PRESET_LOADED);
    str(b, 'Init');
    u32(b, PRESET_SRC_FACTORY);
    u32(b, 7);
    u32(b, 0); // warning count
    return [
      'preset_loaded factory#7',
      b,
      { type: 'PresetLoaded', name: 'Init', source: { kind: 'factory', index: 7 }, warnings: [] },
    ];
  })(),
  (() => {
    const b = [];
    u32(b, VE_PRESET_LOADED);
    str(b, 'Deep');
    u32(b, PRESET_SRC_USER);
    str(b, 'Bass/Deep');
    u32(b, 2); // warning count
    str(b, 'clip high');
    str(b, 'old fmt');
    return [
      'preset_loaded user+warnings',
      b,
      {
        type: 'PresetLoaded',
        name: 'Deep',
        source: { kind: 'user', path: 'Bass/Deep' },
        warnings: ['clip high', 'old fmt'],
      },
    ];
  })(),
  (() => {
    const b = [];
    u32(b, VE_PRESET_LOADED);
    str(b, 'X');
    u32(b, PRESET_SRC_NONE);
    u32(b, 1); // warning count
    str(b, 'w');
    return ['preset_loaded none', b, { type: 'PresetLoaded', name: 'X', source: null, warnings: ['w'] }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_PRESET_CORPUS_CHANGED);
    u32(b, 1); // has follow
    str(b, 'Lead/New');
    return ['corpus_changed follow', b, { type: 'PresetCorpusChanged', follow: 'Lead/New' }];
  })(),
  (() => {
    const b = [];
    u32(b, VE_PRESET_CORPUS_CHANGED);
    u32(b, 0); // no follow
    return ['corpus_changed none', b, { type: 'PresetCorpusChanged', follow: null }];
  })(),
];

describe('packed ViewEvent protocol — golden-byte parity with Rust', () => {
  // Protocol constants must equal the Rust `VE_*` / `PRESET_SRC_*` / KeyMode /
  // Layer discriminants verbatim — a renumber on either side is caught here.
  it('protocol constants match lib.rs values', () => {
    expect([VE_PARAM_CHANGED, VE_KEY_MODE_CHANGED, VE_SPLIT_POINT_CHANGED, VE_EDIT_LAYER_CHANGED, VE_PRESET_LOADED, VE_PRESET_CORPUS_CHANGED]).toEqual([1, 2, 3, 4, 5, 6]);
    expect([PRESET_SRC_NONE, PRESET_SRC_FACTORY, PRESET_SRC_USER]).toEqual([0, 1, 2]);
    expect([KEY_MODE_WHOLE, KEY_MODE_DUAL, KEY_MODE_SPLIT]).toEqual([0, 1, 2]);
    expect([LAYER_UPPER, LAYER_LOWER]).toEqual([0, 1]);
  });

  // Every VE_* variant: a one-record batch decodes to the expected struct.
  for (const [label, recordBytes, expected] of GOLDEN) {
    it(`decodes ${label}`, () => {
      const batch = [];
      u32(batch, 1); // record count
      batch.push(...recordBytes);
      expect(decode(batch)).toEqual([expected]);
    });
  }

  it('decodes an empty batch to []', () => {
    expect(decode([0, 0, 0, 0])).toEqual([]);
  });

  it('decodes a multi-record batch to every struct in order', () => {
    const batch = [];
    u32(batch, GOLDEN.length);
    for (const [, recordBytes] of GOLDEN) batch.push(...recordBytes);
    expect(decode(batch)).toEqual(GOLDEN.map(([, , expected]) => expected));
  });

  it('throws on an unknown tag (drift fails loud, not silent)', () => {
    const batch = [];
    u32(batch, 1);
    u32(batch, 999); // no such VE_* tag
    expect(() => decode(batch)).toThrow(/unknown ViewEvent tag/);
  });
});
