// Binary event codec — the wire format for the E030 event ring (ticket 0155).
//
// JS half. Mirror image of the Rust half (`../src/codec.rs`); both are typed
// encode/decode layers over the SAME 16-byte fixed slot framing (carried
// verbatim from vxn-1's spike-0035 layout so the two synths share a wire
// format). This module does NOT invent a new layout — it formalises the
// encode/decode of every event kind over the existing slot and stays byte-
// compatible. The Rust golden table (`codec.rs tests::golden`) and the JS
// golden table (`event-codec.test.mjs`) are identical literals — that is the
// contract. The one-page spec both halves point at is `WIRE-FORMAT.md`.
//
// Slot layout (16 bytes, little-endian):
//   off 0  u8   type      EV_* tag
//   off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
//   off 2  u16  paramIdx  CLAP param id (EV_PARAM / gesture) else 0
//   off 4  f32  value     velocity / param value / bend / wheel
//   off 8  u8   note      MIDI note (note on/off)
//   off 9  u8   flag      sustain 0/1, OR param-norm bit (EV_PARAM)
//   off 10 u16  seq       producer sequence — owned by the RING, not the codec
//   off 12 f32  reserved  zero
//
// vxn-2 divergences from vxn-1's codec (see ../src/codec.rs header):
//  - No key-mode / split-point events (tags 7/8): the vxn-2 FM engine has no
//    dual/split layer. Those tags stay reserved so notes/params/gestures keep
//    vxn-1's byte numbering.
//  - Flat param id space (TOTAL_PARAMS = vxn-2's param table size); no
//    per-layer patch/global split, so no patchClapId/globalClapId helpers.

// ── Event type tags (numbering carried from vxn-1's 0035/0037 layout) ────────
export const EV_NOTE_ON = 1;
export const EV_NOTE_OFF = 2;
export const EV_PARAM = 3; // plain OR norm (flag bit)
export const EV_PITCH_BEND = 4; // value in [-1, 1]
export const EV_MOD_WHEEL = 5; // value in [0, 1]
export const EV_SUSTAIN = 6; // flag 0/1
// tags 7 (key_mode) and 8 (split_point) reserved-unused in vxn-2
export const EV_GESTURE_BEGIN = 9; // paramIdx = id
export const EV_GESTURE_END = 10; // paramIdx = id

// `flag` bit on EV_PARAM selecting normalised encoding (0 plain, 1 norm).
export const PARAM_FLAG_NORM = 1;

export const SLOT_BYTES = 16; // must equal event-ring.mjs SLOT_BYTES

// ── Param id layout ──────────────────────────────────────────────────────────
//
// vxn-2's param space is flat: CLAP id == param index, `0 .. TOTAL_PARAMS`. The
// Rust side owns the authoritative count (`vxn2_engine::TOTAL_PARAMS`, re-checked
// by `codec.rs tests::total_params_matches_vxn2_engine`); this JS constant MUST
// match it. The host-init handshake (ticket 0156) asserts it against the wasm
// param count so drift is caught at load. 209 today.
export const TOTAL_PARAMS = 209;

// ── Encode ──────────────────────────────────────────────────────────────────
//
// `encodeInto(view, base, event)` writes exactly 16 bytes at `base` of the
// DataView `view`. Zeroes seq + reserved (the ring overwrites seq on push).
// Alloc-free. `event` is a plain object: `{ type, ...fields }`.

export function encodeInto(view, base, event) {
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
// `decode(view, base)` reads a 16-byte slot at `base` into a typed event object.
// Returns `null` for an unknown tag (forward-compat). `offset` is on every
// event (the slice loop needs it). `seq` is NOT returned — it belongs to the
// ring layer.

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
    default:
      return null; // unknown tag: ignore (forward-compat)
  }
}

// ── Typed constructors (ergonomic encoders for the input adapters, 0160) ─────

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
};
