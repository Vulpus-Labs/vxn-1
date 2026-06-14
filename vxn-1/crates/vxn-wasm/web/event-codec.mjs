// Binary event codec — the wire format for the E015 event ring (ticket 0037).
//
// JS half. Mirror image of the Rust half (`src/codec.rs`); both are typed
// encode/decode layers over the SAME 16-byte fixed slot framing frozen by
// spike 0035 (`event-ring.mjs`). This module does NOT invent a new layout — it
// formalises the encode/decode of every event kind over the existing slot and
// stays byte-compatible (the Rust golden table and the JS golden table in
// `event-codec.test.mjs` are identical literals — that is the contract).
//
// Layered cleanly alongside event-ring.mjs: the ring owns the SPSC SAB
// transport (the `seq` field, atomics, slot addressing); this codec owns the
// per-slot byte format. `encodeInto(view, base, event)` writes the 16 codec
// bytes; the ring's `_push` then stamps `seq` and publishes. `decode(view,
// base)` reads a slot view into a typed event object.
//
// Slot layout (16 bytes, little-endian) — see event-ring.mjs FRAMING:
//   off 0  u8   type      EV_* tag
//   off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
//   off 2  u16  paramIdx  CLAP param id (EV_PARAM / gesture) else 0
//   off 4  f32  value     velocity / param value / bend / wheel
//   off 8  u8   note      MIDI note (note on/off)
//   off 9  u8   flag      sustain 0/1, key mode, split note, OR param-norm bit
//   off 10 u16  seq       producer sequence — owned by the RING, not the codec
//   off 12 f32  reserved  zero
//
// Decisions (match src/codec.rs):
//  - set_param_norm rides EV_PARAM with flag = PARAM_FLAG_NORM (1); plain is
//    flag = 0. Byte-compatible with the 0035 ring (its pushParam writes flag 0).
//  - gestures get their own tags (EV_GESTURE_BEGIN / _END), id in paramIdx;
//    decoder no-ops them on the Synth (controller/host-echo concern, ADR 0009).
//  - key-mode / split-point are non-automatable shared state in `flag`.

// ── Event type tags (frozen by 0035; gestures added by 0037) ────────────────
export const EV_NOTE_ON = 1;
export const EV_NOTE_OFF = 2;
export const EV_PARAM = 3; // plain OR norm (flag bit)
export const EV_PITCH_BEND = 4; // value in [-1, 1]
export const EV_MOD_WHEEL = 5; // value in [0, 1]
export const EV_SUSTAIN = 6; // flag 0/1
export const EV_KEY_MODE = 7; // flag = mode (0 Whole, 1 Dual, 2 Split)
export const EV_SPLIT_POINT = 8; // flag = note
export const EV_GESTURE_BEGIN = 9; // paramIdx = id
export const EV_GESTURE_END = 10; // paramIdx = id

// `flag` bit on EV_PARAM selecting normalised encoding (0 plain, 1 norm).
export const PARAM_FLAG_NORM = 1;

export const SLOT_BYTES = 16; // must equal event-ring.mjs SLOT_BYTES

// ── Param id layout (frozen by 0036 / ADR 0009 §3) ──────────────────────────
//
// Exported so 0039 (param store) and 0038 (host) import the layout from one
// place. These mirror vxn-app's PATCH_COUNT / GLOBAL_COUNT / TOTAL_PARAMS. The
// Rust side reads them from vxn-app (the single source of truth); JS asserts
// them against the wasm exports at host init (0038) so drift is caught. The
// id-layout test in event-codec.test.mjs guards the contract here.
export const PATCH_COUNT = 69; // per-layer patch params
export const GLOBAL_COUNT = 27; // global params
export const LAYER_COUNT = 2; // Upper, Lower
export const TOTAL_PARAMS = LAYER_COUNT * PATCH_COUNT + GLOBAL_COUNT; // 165

// Layer ids (match vxn-app Layer discriminants).
export const LAYER_UPPER = 0;
export const LAYER_LOWER = 1;

/// Forward mapping: per-patch param on a layer -> flat CLAP id.
/// Matches vxn_app::params::patch_clap_id.
export function patchClapId(layer, patchIndex) {
  return layer * PATCH_COUNT + patchIndex;
}

/// Forward mapping: global param -> flat CLAP id.
/// Matches vxn_app::params::global_clap_id.
export function globalClapId(globalIndex) {
  return LAYER_COUNT * PATCH_COUNT + globalIndex;
}

// ── Encode ──────────────────────────────────────────────────────────────────
//
// `encodeInto(view, base, event)` writes exactly 16 bytes at `base` of the
// DataView `view`. Zeroes seq + reserved (the ring overwrites seq on push).
// Returns nothing; alloc-free. `event` is a plain object: `{ type, ...fields }`.

export function encodeInto(view, base, event) {
  // Zero the whole slot first so unused fields are deterministic.
  view.setUint8(base + 0, event.type & 0xff);
  view.setUint8(base + 1, (event.offset ?? 0) & 0xff);
  view.setUint16(base + 2, 0, true);
  view.setFloat32(base + 4, 0, true);
  view.setUint8(base + 8, 0);
  view.setUint8(base + 9, 0);
  view.setUint16(base + 10, 0, true); // seq — ring owns this
  view.setFloat32(base + 12, 0, true); // reserved

  switch (event.type) {
    case EV_NOTE_ON:
      view.setFloat32(base + 4, event.velocity, true);
      view.setUint8(base + 8, event.note & 0xff);
      break;
    case EV_NOTE_OFF:
      view.setUint8(base + 8, event.note & 0xff);
      break;
    case EV_PARAM:
      view.setUint16(base + 2, event.id & 0xffff, true);
      view.setFloat32(base + 4, event.value, true);
      view.setUint8(base + 9, event.norm ? PARAM_FLAG_NORM : 0);
      break;
    case EV_GESTURE_BEGIN:
    case EV_GESTURE_END:
      view.setUint16(base + 2, event.id & 0xffff, true);
      break;
    case EV_PITCH_BEND:
    case EV_MOD_WHEEL:
      view.setFloat32(base + 4, event.value, true);
      break;
    case EV_SUSTAIN:
      view.setUint8(base + 9, event.on ? 1 : 0);
      break;
    case EV_KEY_MODE:
      view.setUint8(base + 9, event.mode & 0xff);
      break;
    case EV_SPLIT_POINT:
      view.setUint8(base + 9, event.note & 0xff);
      break;
    default:
      throw new Error(`encodeInto: unknown event type ${event.type}`);
  }
}

/// Encode into a fresh 16-byte Uint8Array (convenience for tests / one-offs).
/// The hot path uses `encodeInto` against the ring's DataView directly.
export function encode(event) {
  const buf = new Uint8Array(SLOT_BYTES);
  encodeInto(new DataView(buf.buffer), 0, event);
  return buf;
}

// ── Decode ──────────────────────────────────────────────────────────────────
//
// `decode(view, base)` reads a 16-byte slot at `base` of DataView `view` into a
// typed event object. Returns `null` for an unknown tag (forward-compat). The
// `offset` field is included on every event (the ring's slice loop needs it).
// `seq` is deliberately NOT returned — it belongs to the ring layer.

export function decode(view, base = 0) {
  const type = view.getUint8(base + 0);
  const offset = view.getUint8(base + 1);
  switch (type) {
    case EV_NOTE_ON:
      return {
        type,
        offset,
        note: view.getUint8(base + 8),
        velocity: view.getFloat32(base + 4, true),
      };
    case EV_NOTE_OFF:
      return { type, offset, note: view.getUint8(base + 8) };
    case EV_PARAM: {
      const norm = (view.getUint8(base + 9) & PARAM_FLAG_NORM) !== 0;
      return {
        type,
        offset,
        id: view.getUint16(base + 2, true),
        value: view.getFloat32(base + 4, true),
        norm,
      };
    }
    case EV_GESTURE_BEGIN:
    case EV_GESTURE_END:
      return { type, offset, id: view.getUint16(base + 2, true) };
    case EV_PITCH_BEND:
    case EV_MOD_WHEEL:
      return { type, offset, value: view.getFloat32(base + 4, true) };
    case EV_SUSTAIN:
      return { type, offset, on: view.getUint8(base + 9) !== 0 };
    case EV_KEY_MODE:
      return { type, offset, mode: view.getUint8(base + 9) };
    case EV_SPLIT_POINT:
      return { type, offset, note: view.getUint8(base + 9) };
    default:
      return null; // unknown tag: ignore (forward-compat)
  }
}

// ── Typed constructors (ergonomic encoders for E017 input adapters) ──────────
//
// Thin object factories so callers don't hand-build `{ type, ... }` literals.
// They pair with encodeInto / encode.

export const ev = {
  noteOn: (note, velocity, offset = 0) => ({ type: EV_NOTE_ON, offset, note, velocity }),
  noteOff: (note, offset = 0) => ({ type: EV_NOTE_OFF, offset, note }),
  setParam: (id, plain, offset = 0) => ({ type: EV_PARAM, offset, id, value: plain, norm: false }),
  setParamNorm: (id, norm, offset = 0) => ({ type: EV_PARAM, offset, id, value: norm, norm: true }),
  gestureBegin: (id, offset = 0) => ({ type: EV_GESTURE_BEGIN, offset, id }),
  gestureEnd: (id, offset = 0) => ({ type: EV_GESTURE_END, offset, id }),
  pitchBend: (norm, offset = 0) => ({ type: EV_PITCH_BEND, offset, value: norm }),
  modWheel: (norm, offset = 0) => ({ type: EV_MOD_WHEEL, offset, value: norm }),
  sustain: (on, offset = 0) => ({ type: EV_SUSTAIN, offset, on }),
  keyMode: (mode, offset = 0) => ({ type: EV_KEY_MODE, offset, mode }),
  splitPoint: (note, offset = 0) => ({ type: EV_SPLIT_POINT, offset, note }),
};
