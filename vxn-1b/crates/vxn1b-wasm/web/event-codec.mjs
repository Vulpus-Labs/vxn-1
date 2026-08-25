// Binary event codec — the JS half of the browser event wire (0287).
//
// ONE definition, two implementations. The Rust half is
// `vxn-1b/crates/vxn1b-wasm/src/codec.rs` (0286); this file must stay
// byte-identical to it. The golden table in `event-codec.test.mjs` replicates
// the Rust golden table row for row, so drift in either language fails a test
// rather than silently mis-routing a note.
//
// The slot layout and tags are documented once, in `WIRE-FORMAT.md`. Read that
// before changing anything here.
//
// ===========================================================================
// PARAM ID LAYOUT — mirrors vxn1b-engine's params.rs
// ===========================================================================
//
//   counts:  PATCH_COUNT  = 75   (per-layer patch params)
//            GLOBAL_COUNT = 35   (globals, shared by both layers)
//            LAYER_COUNT  = 2    (L1, L2)
//            TOTAL_PARAMS = 2*75 + 35 = 185
//
//   id ranges:
//     [  0 ..  75 )   Layer 1 patch params   (clap_id = patch_index)
//     [ 75 .. 150 )   Layer 2 patch params   (clap_id = 75 + patch_index)
//     [150 .. 185 )   global params          (clap_id = 150 + global_index)
//
// These constants are a HAND-DECLARED MIRROR of the engine's, and that is the
// dangerous kind of constant: ticket 0285 killed both other browser builds by
// letting exactly this drift behind an engine that had grown two params. Two
// things guard it, and both matter:
//
//   1. the wasm exports `vxn1b_total_params()`, and the controller handshake
//      refuses to start on a mismatch (the check that caught 0285);
//   2. `event-codec.test.mjs` reads that export out of the BUILT artifact and
//      fails — never skips — if the two disagree.
//
// If a param is added to the engine, update the four constants below and the
// counts in WIRE-FORMAT.md. Nothing else in JS hard-codes them.

export const PATCH_COUNT = 75;
export const GLOBAL_COUNT = 35;
export const LAYER_COUNT = 2;
export const TOTAL_PARAMS = LAYER_COUNT * PATCH_COUNT + GLOBAL_COUNT; // 185

// Layer ids (match vxn1b_engine::params::Layer discriminants).
export const LAYER_L1 = 0;
export const LAYER_L2 = 1;

/// Per-layer patch param -> flat CLAP id. Matches `clap_id_of(layer, p)`.
export function patchClapId(layer, patchIndex) {
  return layer * PATCH_COUNT + patchIndex;
}

/// Global param -> flat CLAP id. Globals occupy the tail of the id space.
export function globalClapId(globalIndex) {
  return LAYER_COUNT * PATCH_COUNT + globalIndex;
}

export const SLOT_BYTES = 16;

// ── Event type tags ────────────────────────────────────────────────────────
//
// 1..=10 are common to vxn-1, vxn-2 and VXN1b — and that is the WHOLE of the
// sharing guarantee. 11+ are synth-local and already conflict across the three
// (vxn-2 uses 11 for matrix_row and 12 for patch_swap). Never port a tag by
// number; see WIRE-FORMAT.md.

export const EV_NOTE_ON = 1; // value = velocity, note = key, flag = channel
export const EV_NOTE_OFF = 2; // note = key, flag = channel
export const EV_PARAM = 3; // paramIdx = clap id, value = plain|norm, flag = norm bit
export const EV_PITCH_BEND = 4; // value in [-1, 1]
export const EV_MOD_WHEEL = 5; // value in [0, 1]
export const EV_SUSTAIN_RESERVED = 6; // vxn-1's sustain; VXN1b has no CC64 path
export const EV_KEY_MODE = 7; // flag = mode (0 Single, 1 Dual, 2 Split)
export const EV_SPLIT_POINT = 8; // flag = note
export const EV_GESTURE_BEGIN = 9; // paramIdx = id
export const EV_GESTURE_END = 10; // paramIdx = id
export const EV_LFO2_LINK = 11; // flag 0/1
export const EV_MATRIX_EDIT = 12; // paramIdx = packed address, flag = value byte
export const EV_SCOPE_TAP = 13; // flag = ScopeTap code
export const EV_TEMPO = 14; // value = BPM
export const EV_POLY_PRESSURE = 15; // note = key, value = [0,1], flag = channel
export const EV_CHANNEL_PRESSURE = 16; // value = [0,1], flag = channel

/// `flag` bit on EV_PARAM selecting normalised encoding (0 plain, 1 norm).
export const PARAM_FLAG_NORM = 1;

// ── Matrix address packing ─────────────────────────────────────────────────

export const MATRIX_SLOTS = 16;
export const MATRIX_FIELD_SOURCE = 0;
export const MATRIX_FIELD_DEST = 1;
export const MATRIX_FIELD_CURVE = 2;
export const MATRIX_FIELD_SCALE_SRC = 3;

/// Pack a matrix-slot address into the 16-bit paramIdx field:
/// `layer << 12 | slot << 8 | field`. Mirrors Rust's `pack_matrix_addr`.
export function packMatrixAddr(layer, slot, field) {
  return ((layer & 0xf) << 12) | ((slot & 0xf) << 8) | (field & 0xff);
}

/// Inverse of packMatrixAddr. `null` for a layer, slot or field outside the
/// engine's range — a malformed record is DROPPED, never clamped onto a valid
/// slot it wasn't aimed at, which would silently rewire someone's patch.
export function unpackMatrixAddr(addr) {
  const layer = addr >> 12;
  if (layer > LAYER_L2) return null;
  const slot = (addr >> 8) & 0x0f;
  if (slot >= MATRIX_SLOTS) return null;
  const field = addr & 0xff;
  if (field > MATRIX_FIELD_SCALE_SRC) return null;
  return { layer, slot, field };
}

// ── Encode ─────────────────────────────────────────────────────────────────

/// Encode one event into a fresh 16-byte Uint8Array. `seq` (off 10) is owned by
/// the ring writer, not the codec, so it is left zero here.
export function encode(event) {
  const buf = new Uint8Array(SLOT_BYTES);
  encodeInto(new DataView(buf.buffer), 0, event);
  return buf;
}

/// Encode `event` into `view` at byte `base`, writing exactly 16 bytes.
/// Alloc-free; the hot-path entry point.
export function encodeInto(view, base, event) {
  // Zero the whole slot first so unused fields are deterministic.
  view.setUint8(base + 0, event.type & 0xff);
  view.setUint8(base + 1, (event.offset ?? 0) & 0xff);
  view.setUint16(base + 2, 0, true);
  view.setFloat32(base + 4, 0, true);
  view.setUint8(base + 8, 0);
  view.setUint8(base + 9, 0);
  view.setUint16(base + 10, 0, true); // seq — the ring owns this
  view.setFloat32(base + 12, 0, true); // reserved

  switch (event.type) {
    case EV_NOTE_ON:
      view.setFloat32(base + 4, event.velocity, true);
      view.setUint8(base + 8, event.note & 0xff);
      view.setUint8(base + 9, (event.channel ?? 0) & 0xff);
      break;
    case EV_NOTE_OFF:
      view.setUint8(base + 8, event.note & 0xff);
      view.setUint8(base + 9, (event.channel ?? 0) & 0xff);
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
    case EV_KEY_MODE:
      view.setUint8(base + 9, event.mode & 0xff);
      break;
    case EV_SPLIT_POINT:
      view.setUint8(base + 9, event.note & 0xff);
      break;
    case EV_LFO2_LINK:
      view.setUint8(base + 9, event.on ? 1 : 0);
      break;
    case EV_MATRIX_EDIT:
      view.setUint16(base + 2, event.addr & 0xffff, true);
      view.setUint8(base + 9, event.value & 0xff);
      break;
    case EV_SCOPE_TAP:
      view.setUint8(base + 9, event.tap & 0xff);
      break;
    case EV_TEMPO:
      view.setFloat32(base + 4, event.bpm, true);
      break;
    case EV_POLY_PRESSURE:
      view.setFloat32(base + 4, event.value, true);
      view.setUint8(base + 8, event.note & 0xff);
      view.setUint8(base + 9, (event.channel ?? 0) & 0xff);
      break;
    case EV_CHANNEL_PRESSURE:
      view.setFloat32(base + 4, event.value, true);
      view.setUint8(base + 9, (event.channel ?? 0) & 0xff);
      break;
    default:
      throw new Error(`encode: unknown event type ${event.type}`);
  }
}

// ── Decode ─────────────────────────────────────────────────────────────────

/// Decode the 16-byte slot at `base`. Returns `null` for an unknown or reserved
/// tag (forward-compat) — the same contract as the Rust `decode`.
export function decodeAt(view, base) {
  const type = view.getUint8(base + 0);
  const offset = view.getUint8(base + 1);
  switch (type) {
    case EV_NOTE_ON:
      return {
        type,
        offset,
        channel: view.getUint8(base + 9),
        note: view.getUint8(base + 8),
        velocity: view.getFloat32(base + 4, true),
      };
    case EV_NOTE_OFF:
      return {
        type,
        offset,
        channel: view.getUint8(base + 9),
        note: view.getUint8(base + 8),
      };
    case EV_PARAM:
      return {
        type,
        offset,
        id: view.getUint16(base + 2, true),
        value: view.getFloat32(base + 4, true),
        norm: (view.getUint8(base + 9) & PARAM_FLAG_NORM) !== 0,
      };
    case EV_GESTURE_BEGIN:
    case EV_GESTURE_END:
      return { type, offset, id: view.getUint16(base + 2, true) };
    case EV_PITCH_BEND:
    case EV_MOD_WHEEL:
      return { type, offset, value: view.getFloat32(base + 4, true) };
    case EV_KEY_MODE:
      return { type, offset, mode: view.getUint8(base + 9) };
    case EV_SPLIT_POINT:
      return { type, offset, note: view.getUint8(base + 9) };
    case EV_LFO2_LINK:
      return { type, offset, on: view.getUint8(base + 9) !== 0 };
    case EV_MATRIX_EDIT:
      return {
        type,
        offset,
        addr: view.getUint16(base + 2, true),
        value: view.getUint8(base + 9),
      };
    case EV_SCOPE_TAP:
      return { type, offset, tap: view.getUint8(base + 9) };
    case EV_TEMPO:
      return { type, offset, bpm: view.getFloat32(base + 4, true) };
    case EV_POLY_PRESSURE:
      return {
        type,
        offset,
        channel: view.getUint8(base + 9),
        note: view.getUint8(base + 8),
        value: view.getFloat32(base + 4, true),
      };
    case EV_CHANNEL_PRESSURE:
      return {
        type,
        offset,
        channel: view.getUint8(base + 9),
        value: view.getFloat32(base + 4, true),
      };
    default:
      return null; // unknown or reserved: ignore (forward-compat)
  }
}

/// Decode a standalone 16-byte buffer. `null` if it is short or unknown.
export function decode(bytes) {
  if (!bytes || bytes.length < SLOT_BYTES) return null;
  const view =
    bytes instanceof DataView
      ? bytes
      : new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return decodeAt(view, 0);
}

// ── Constructors ───────────────────────────────────────────────────────────
//
// Named builders so producers never assemble raw tag objects by hand. The
// `channel` default of 0 is the non-MPE case.

export const ev = {
  noteOn: (note, velocity, offset = 0, channel = 0) => ({
    type: EV_NOTE_ON,
    offset,
    channel,
    note,
    velocity,
  }),
  noteOff: (note, offset = 0, channel = 0) => ({ type: EV_NOTE_OFF, offset, channel, note }),
  setParam: (id, plain, offset = 0) => ({ type: EV_PARAM, offset, id, value: plain, norm: false }),
  setParamNorm: (id, norm, offset = 0) => ({ type: EV_PARAM, offset, id, value: norm, norm: true }),
  gestureBegin: (id, offset = 0) => ({ type: EV_GESTURE_BEGIN, offset, id }),
  gestureEnd: (id, offset = 0) => ({ type: EV_GESTURE_END, offset, id }),
  pitchBend: (value, offset = 0) => ({ type: EV_PITCH_BEND, offset, value }),
  modWheel: (value, offset = 0) => ({ type: EV_MOD_WHEEL, offset, value }),
  keyMode: (mode, offset = 0) => ({ type: EV_KEY_MODE, offset, mode }),
  splitPoint: (note, offset = 0) => ({ type: EV_SPLIT_POINT, offset, note }),
  lfo2Link: (on, offset = 0) => ({ type: EV_LFO2_LINK, offset, on }),
  matrixEdit: (layer, slot, field, value, offset = 0) => ({
    type: EV_MATRIX_EDIT,
    offset,
    addr: packMatrixAddr(layer, slot, field),
    value,
  }),
  scopeTap: (tap, offset = 0) => ({ type: EV_SCOPE_TAP, offset, tap }),
  tempo: (bpm, offset = 0) => ({ type: EV_TEMPO, offset, bpm }),
  polyPressure: (note, value, offset = 0, channel = 0) => ({
    type: EV_POLY_PRESSURE,
    offset,
    channel,
    note,
    value,
  }),
  channelPressure: (value, offset = 0, channel = 0) => ({
    type: EV_CHANNEL_PRESSURE,
    offset,
    channel,
    value,
  }),
};
