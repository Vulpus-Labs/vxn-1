// Binary event codec — the JS half of the browser event wire (0287).
//
// ONE definition, two halves, and they are NOT symmetric (0312). This side
// ENCODES; `vxn-1b/crates/vxn1b-wasm/src/codec.rs` DECODES. Nothing in the
// browser reads a slot back — the ring hands raw bytes to wasm — so there is no
// JS decoder here to drift.
//
// `encodeInto` below is the only thing that writes a wire byte: `EventRing._push`
// calls it, every producer goes through the ring, and the golden table in
// `event-codec.test.mjs` checks it against a row-for-row transcription of the
// Rust golden table. So a layout change in either language fails a test rather
// than silently mis-routing a note.
//
// The slot layout and tags are documented once, in `WIRE-FORMAT.md`. Read that
// before changing anything here.
//
// ===========================================================================
// PARAM ID LAYOUT — mirrors vxn1b-engine's params.rs
// ===========================================================================
//
//   [ 0*P .. 1*P )   Layer 1 patch params   clap_id = patch_index
//   [ 1*P .. 2*P )   Layer 2 patch params   clap_id = P + patch_index
//   [ 2*P .. 2*P+G ) global params          clap_id = 2*P + global_index
//
// ...where P = PATCH_COUNT and G = GLOBAL_COUNT, declared below. Written as
// formulae rather than today's numbers so this block cannot rot against the
// constants two lines under it; WIRE-FORMAT.md carries the worked example.
//
// These constants are a HAND-DECLARED MIRROR of the engine's, and that is the
// dangerous kind of constant: ticket 0285 killed both other browser builds by
// letting exactly this drift behind an engine that had grown two params. Two
// things guard it, and both matter:
//
//   1. the wasm exports `vxn1b_patch_count()` / `vxn1b_global_count()` /
//      `vxn1b_total_params()` (the controller exports the `vxnc_*` triple), and
//      the handshake refuses to start on a mismatch (the check that caught 0285);
//   2. `wasm-agreement.test.mjs` reads those exports out of the BUILT artifact
//      and fails — never skips — if they disagree.
//
// ALL THREE counts are checked, never the sum alone: a drift of +1 patch and
// -2 global leaves TOTAL_PARAMS untouched while `patchClapId(L2, …)` and
// `globalClapId` compute wrong ids on both sides of the wire (0312).
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
// Ordinals are `vxn1b_engine::vocab::MATRIX_FIELD_NAMES` positions, and they are
// FROZEN: 0..3 predate the polarity/shape split, so `shape`, `scale-shape` and
// `enabled` were appended at 4/5/6 rather than inserted in reading order, and
// `scale-polarity` at 7 after them. Match
// `vxn1b_wasm::codec::unpack_matrix_addr`, which decodes exactly this order —
// renumbering here lands edits on the wrong field, silently.
export const MATRIX_FIELD_SOURCE = 0;
export const MATRIX_FIELD_DEST = 1;
// Slot 2 was "curve" before the axes split; `polarity` inherited both the
// ordinal and the meaning, and the shape half became its own field at 4.
export const MATRIX_FIELD_POLARITY = 2;
export const MATRIX_FIELD_SCALE_SRC = 3;
export const MATRIX_FIELD_SHAPE = 4;
export const MATRIX_FIELD_SCALE_SHAPE = 5;
export const MATRIX_FIELD_ENABLED = 6;
// The scale VCA's polarity (0341), appended after `enabled` because the six
// before it were already on the wire — reading order would have put it beside
// MATRIX_FIELD_SCALE_SHAPE, and moving them to get it there re-aims every
// in-flight address.
export const MATRIX_FIELD_SCALE_POLARITY = 7;

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
  if (field > MATRIX_FIELD_SCALE_POLARITY) return null;
  return { layer, slot, field };
}

// ── Encode ─────────────────────────────────────────────────────────────────

/// Encode `event` into `view` at byte `base`, writing exactly 16 bytes.
///
/// The ONE encoder on this wire. `EventRing._push` writes ring slots through it,
/// so every byte the worklet decodes comes from here; there is no second
/// allocating wrapper, because nothing needs a detached slot.
///
/// `seq` (off 10) is owned by the ring writer, not the codec, and is left zero —
/// the ring stamps it after this returns. Throws on an unknown tag rather than
/// publishing a blank slot; `_push` calls this BEFORE it advances the write
/// index, so a throw leaves the ring untouched.
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

// ── Constructors ───────────────────────────────────────────────────────────
//
// Named builders so producers never assemble raw tag objects by hand — every
// `EventRing.push*` is one of these plus the ring's bookkeeping. `offset` sits
// in the same position here, on the ring and on `WebHost`: after the event's own
// fields, defaulted to 0 ("as soon as possible"). The `channel` default of 0 is
// the non-MPE case.

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
