//! Binary event codec — the wire format for the E015 event ring (ticket 0037).
//!
//! ONE definition, two implementations. This is the Rust half; the JS half is
//! `web/event-codec.mjs`. Both are typed encode/decode layers over the **exact
//! 16-byte fixed slot framing frozen by spike 0035** (`web/event-ring.mjs`) —
//! this module does not invent a new layout, it formalises encode/decode of
//! every event kind over the existing slot and keeps it byte-compatible.
//!
//! # Slot layout (16 bytes, little-endian) — frozen by 0035
//!
//! ```text
//! off 0  u8   type      EV_* tag
//! off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
//! off 2  u16  paramIdx  CLAP param id (EV_PARAM only; else 0)
//! off 4  f32  value     velocity / param value / bend / wheel
//! off 8  u8   note      MIDI note number (EV_NOTE_ON/OFF)
//! off 9  u8   flag      small int: sustain 0/1, key mode, split note,
//!                       OR the param-norm bit (see below)
//! off 10 u16  seq       producer sequence (low 16 bits) — owned by the ring,
//!                       not the codec; encode writes 0, decode ignores it.
//! off 12 f32  reserved  zero
//! ```
//!
//! # Decisions made here (close-out)
//!
//! - **norm vs plain param**: a `set_param_norm` rides the SAME `EV_PARAM` tag
//!   with the `flag` byte set to [`PARAM_FLAG_NORM`] (1); a plain `set_param`
//!   has `flag == 0`. This keeps both in the one 16-byte slot, needs no new
//!   tag, and is byte-compatible with the 0035 ring (its `pushParam` writes
//!   `flag = 0`, i.e. plain). The decoder converts norm→plain via the param's
//!   [`ParamDesc::from_normalized`] before calling `Synth::set_param`, because
//!   the engine (and the param SAB, ADR 0009 §2) carry **plain** values.
//! - **gestures**: `gesture_begin`/`gesture_end` get their own tags
//!   ([`EV_GESTURE_BEGIN`], [`EV_GESTURE_END`]) carrying the param id in
//!   `paramIdx`. They are **controller/host-echo concerns, not Synth calls**
//!   (ADR 0009 §1; gesture bracketing lives in the controller / 0039), so
//!   [`apply`] **no-ops** them on the `Synth`. They still round-trip through the
//!   codec so the ring can carry them to whoever cares.
//! - **key-mode / split-point**: non-automatable shared state (ADR 0003 §3),
//!   NOT param ids. Carried in the `flag` byte under their own tags, applied
//!   to the `Synth` once per block (mirrors the native "set once before event
//!   ingestion" rule).
//!
//! # Param addressing — frozen by 0036 / ADR 0009
//!
//! The id-layout constants are re-exported straight from `vxn-app`
//! ([`TOTAL_PARAMS`], [`PATCH_COUNT`], [`GLOBAL_COUNT`]) — **never hard-coded**
//! here — so a future param add/remove flows through. See [`patch_clap_id`] /
//! [`global_clap_id`] for the forward mapping.

use vxn_app::params::{
    desc_for_clap_id, GLOBAL_COUNT as VXN_GLOBAL_COUNT, PATCH_COUNT as VXN_PATCH_COUNT,
    TOTAL_PARAMS as VXN_TOTAL_PARAMS,
};
use vxn_app::KeyMode;
use vxn_engine::Synth;

// Re-export the id-layout constants so downstream (0039 store, 0038 host) reads
// them from one place. These come from vxn-app, not literals.
pub use vxn_app::params::{global_clap_id, patch_clap_id};

/// Total addressable CLAP ids (`2 * PATCH_COUNT + GLOBAL_COUNT`). 165 today.
pub const TOTAL_PARAMS: u16 = VXN_TOTAL_PARAMS as u16;
/// Per-layer patch param count (69 today). Upper = `[0, PATCH_COUNT)`,
/// Lower = `[PATCH_COUNT, 2*PATCH_COUNT)`.
pub const PATCH_COUNT: u16 = VXN_PATCH_COUNT as u16;
/// Global param count (27 today). Globals = `[2*PATCH_COUNT, TOTAL_PARAMS)`.
pub const GLOBAL_COUNT: u16 = VXN_GLOBAL_COUNT as u16;

/// Bytes per slot — must equal the ring's `SLOT_BYTES` (0035).
pub const SLOT_BYTES: usize = 16;

// ── Event type tags (frozen by 0035; gestures added by 0037) ────────────────

/// `note_on { note, velocity }`. `value` = velocity, `note` = key.
pub const EV_NOTE_ON: u8 = 1;
/// `note_off { note }`. `note` = key.
pub const EV_NOTE_OFF: u8 = 2;
/// `set_param`/`set_param_norm`. `paramIdx` = id, `value` = plain or norm,
/// `flag` = [`PARAM_FLAG_NORM`] selects which.
pub const EV_PARAM: u8 = 3;
/// `pitch_bend { norm }`. `value` in `[-1, 1]`.
pub const EV_PITCH_BEND: u8 = 4;
/// `mod_wheel { norm }`. `value` in `[0, 1]`.
pub const EV_MOD_WHEEL: u8 = 5;
/// `sustain { on }`. `flag` 0/1.
pub const EV_SUSTAIN: u8 = 6;
/// `key_mode { mode }`. `flag` = mode (0 Whole, 1 Dual, 2 Split).
pub const EV_KEY_MODE: u8 = 7;
/// `split_point { note }`. `flag` = note.
pub const EV_SPLIT_POINT: u8 = 8;
/// `gesture_begin { id }`. `paramIdx` = id. Decoder no-ops on the Synth.
pub const EV_GESTURE_BEGIN: u8 = 9;
/// `gesture_end { id }`. `paramIdx` = id. Decoder no-ops on the Synth.
pub const EV_GESTURE_END: u8 = 10;

/// `flag` bit on [`EV_PARAM`] selecting the normalised encoding. `0` = plain
/// value (engine-domain f32), `1` = normalised `[0, 1]` (taper not applied —
/// linear position, matching [`ParamDesc::to_normalized`]).
pub const PARAM_FLAG_NORM: u8 = 1;

/// A decoded event. Zero-copy: produced by reading a 16-byte slot view; carries
/// no heap allocation. `offset` is the sample offset within the quantum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    NoteOn { offset: u8, note: u8, velocity: f32 },
    NoteOff { offset: u8, note: u8 },
    /// Plain (engine-domain) param value.
    SetParam { offset: u8, id: u16, plain: f32 },
    /// Normalised `[0, 1]` param value; decoder converts to plain before apply.
    SetParamNorm { offset: u8, id: u16, norm: f32 },
    GestureBegin { offset: u8, id: u16 },
    GestureEnd { offset: u8, id: u16 },
    PitchBend { offset: u8, norm: f32 },
    ModWheel { offset: u8, norm: f32 },
    Sustain { offset: u8, on: bool },
    KeyMode { offset: u8, mode: u8 },
    SplitPoint { offset: u8, note: u8 },
}

impl Event {
    /// The wire tag this event encodes to.
    #[inline]
    pub fn tag(&self) -> u8 {
        match self {
            Event::NoteOn { .. } => EV_NOTE_ON,
            Event::NoteOff { .. } => EV_NOTE_OFF,
            Event::SetParam { .. } | Event::SetParamNorm { .. } => EV_PARAM,
            Event::GestureBegin { .. } => EV_GESTURE_BEGIN,
            Event::GestureEnd { .. } => EV_GESTURE_END,
            Event::PitchBend { .. } => EV_PITCH_BEND,
            Event::ModWheel { .. } => EV_MOD_WHEEL,
            Event::Sustain { .. } => EV_SUSTAIN,
            Event::KeyMode { .. } => EV_KEY_MODE,
            Event::SplitPoint { .. } => EV_SPLIT_POINT,
        }
    }

    /// The sample offset within the quantum (`0..Q`).
    #[inline]
    pub fn offset(&self) -> u8 {
        match *self {
            Event::NoteOn { offset, .. }
            | Event::NoteOff { offset, .. }
            | Event::SetParam { offset, .. }
            | Event::SetParamNorm { offset, .. }
            | Event::GestureBegin { offset, .. }
            | Event::GestureEnd { offset, .. }
            | Event::PitchBend { offset, .. }
            | Event::ModWheel { offset, .. }
            | Event::Sustain { offset, .. }
            | Event::KeyMode { offset, .. }
            | Event::SplitPoint { offset, .. } => offset,
        }
    }
}

// ── Encode ──────────────────────────────────────────────────────────────────

#[inline]
fn put_u16(buf: &mut [u8; SLOT_BYTES], at: usize, v: u16) {
    buf[at] = (v & 0xff) as u8;
    buf[at + 1] = (v >> 8) as u8;
}

#[inline]
fn put_f32(buf: &mut [u8; SLOT_BYTES], at: usize, v: f32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn get_u16(buf: &[u8], at: usize) -> u16 {
    (buf[at] as u16) | ((buf[at + 1] as u16) << 8)
}

#[inline]
fn get_f32(buf: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// Encode `event` into a fresh 16-byte slot. Alloc-free (returns the array by
/// value, on the stack). The `seq` field (off 10) is owned by the ring writer,
/// not the codec, so it is left zero here; the producer stamps it on push.
#[inline]
pub fn encode(event: &Event) -> [u8; SLOT_BYTES] {
    let mut buf = [0u8; SLOT_BYTES];
    encode_into(event, &mut buf);
    buf
}

/// Encode `event` into an existing 16-byte buffer in place (fully overwrites all
/// 16 bytes). The hot-path entry point — no allocation.
#[inline]
pub fn encode_into(event: &Event, buf: &mut [u8; SLOT_BYTES]) {
    *buf = [0u8; SLOT_BYTES];
    buf[0] = event.tag();
    buf[1] = event.offset();
    match *event {
        Event::NoteOn { note, velocity, .. } => {
            put_f32(buf, 4, velocity);
            buf[8] = note;
        }
        Event::NoteOff { note, .. } => {
            buf[8] = note;
        }
        Event::SetParam { id, plain, .. } => {
            put_u16(buf, 2, id);
            put_f32(buf, 4, plain);
            buf[9] = 0; // plain
        }
        Event::SetParamNorm { id, norm, .. } => {
            put_u16(buf, 2, id);
            put_f32(buf, 4, norm);
            buf[9] = PARAM_FLAG_NORM;
        }
        Event::GestureBegin { id, .. } | Event::GestureEnd { id, .. } => {
            put_u16(buf, 2, id);
        }
        Event::PitchBend { norm, .. } => {
            put_f32(buf, 4, norm);
        }
        Event::ModWheel { norm, .. } => {
            put_f32(buf, 4, norm);
        }
        Event::Sustain { on, .. } => {
            buf[9] = on as u8;
        }
        Event::KeyMode { mode, .. } => {
            buf[9] = mode;
        }
        Event::SplitPoint { note, .. } => {
            buf[9] = note;
        }
    }
}

// ── Decode ──────────────────────────────────────────────────────────────────

/// Decode a 16-byte slot view into a typed [`Event`]. Zero-copy: reads the
/// borrowed slice, allocates nothing. Returns `None` for an unknown tag
/// (forward-compat with future event kinds), or if `buf` is too short.
#[inline]
pub fn decode(buf: &[u8]) -> Option<Event> {
    if buf.len() < SLOT_BYTES {
        return None;
    }
    let ty = buf[0];
    let offset = buf[1];
    Some(match ty {
        EV_NOTE_ON => Event::NoteOn {
            offset,
            note: buf[8],
            velocity: get_f32(buf, 4),
        },
        EV_NOTE_OFF => Event::NoteOff {
            offset,
            note: buf[8],
        },
        EV_PARAM => {
            let id = get_u16(buf, 2);
            let value = get_f32(buf, 4);
            if buf[9] & PARAM_FLAG_NORM != 0 {
                Event::SetParamNorm {
                    offset,
                    id,
                    norm: value,
                }
            } else {
                Event::SetParam {
                    offset,
                    id,
                    plain: value,
                }
            }
        }
        EV_GESTURE_BEGIN => Event::GestureBegin {
            offset,
            id: get_u16(buf, 2),
        },
        EV_GESTURE_END => Event::GestureEnd {
            offset,
            id: get_u16(buf, 2),
        },
        EV_PITCH_BEND => Event::PitchBend {
            offset,
            norm: get_f32(buf, 4),
        },
        EV_MOD_WHEEL => Event::ModWheel {
            offset,
            norm: get_f32(buf, 4),
        },
        EV_SUSTAIN => Event::Sustain {
            offset,
            on: buf[9] != 0,
        },
        EV_KEY_MODE => Event::KeyMode {
            offset,
            mode: buf[9],
        },
        EV_SPLIT_POINT => Event::SplitPoint {
            offset,
            note: buf[9],
        },
        _ => return None, // unknown tag: ignore (forward-compat)
    })
}

// ── Apply (dispatch parity with vxn-core-clap::dispatch_event) ───────────────

/// Apply a decoded event to a `Synth`, with semantics **identical** to the
/// plugin's `vxn_core_clap::dispatch_event` + the CLAP batch loop in
/// `vxn-clap/src/lib.rs:335-369`:
///
/// - `NoteOn`  → `Synth::note_on(note, velocity)` — velocity forwarded as-is
///   (CLAP `[0,1]`; the engine owns the mapping), matching `dispatch_event`.
/// - `NoteOff` → `Synth::note_off(note)`.
/// - `SetParam{plain}` → `Synth::set_param(id, plain)` — exactly the CLAP shell's
///   immediate `synth.set_param(idx, value)` inside the event batch.
/// - `SetParamNorm{norm}` → convert to plain via the param's
///   `ParamDesc::from_normalized` (the engine carries plain values), then
///   `set_param`. Unknown ids are dropped (matches CLAP ignoring unknown ids).
/// - `PitchBend` → `Synth::set_pitch_bend(norm)` (the `SynthNotes` adapter's
///   `pitch_bend` impl). Codec carries the already-normalised `[-1,1]` value —
///   the 14-bit→norm conversion `dispatch_event` does for raw MIDI is a wire
///   concern done by the encoder (Web MIDI adapter, E017), not here.
/// - `ModWheel` → `Synth::set_mod_wheel(norm)` (carries normalised `[0,1]`; the
///   CC1 deadzone in `dispatch_event` is likewise an encoder-side concern).
/// - `Sustain` → `Synth::sustain(on)` (CC64 `>=64` decode is encoder-side).
/// - `KeyMode` → `Synth::set_key_mode(KeyMode::from_u8(mode))` (shared state).
/// - `SplitPoint` → `Synth::set_split_point(note)` (shared state).
/// - `GestureBegin`/`GestureEnd` → **no-op on the Synth** (controller / host-echo
///   concern, ADR 0009 §1; they never reach rendering).
#[inline]
pub fn apply(event: &Event, synth: &mut Synth) {
    match *event {
        Event::NoteOn { note, velocity, .. } => synth.note_on(note, velocity),
        Event::NoteOff { note, .. } => synth.note_off(note),
        Event::SetParam { id, plain, .. } => synth.set_param(id as usize, plain),
        Event::SetParamNorm { id, norm, .. } => {
            if let Some(desc) = desc_for_clap_id(id as usize) {
                synth.set_param(id as usize, desc.from_normalized(norm));
            }
        }
        Event::PitchBend { norm, .. } => synth.set_pitch_bend(norm),
        Event::ModWheel { norm, .. } => synth.set_mod_wheel(norm),
        Event::Sustain { on, .. } => synth.sustain(on),
        Event::KeyMode { mode, .. } => synth.set_key_mode(KeyMode::from_u8(mode)),
        Event::SplitPoint { note, .. } => synth.set_split_point(note),
        // Gestures never touch the renderer (ADR 0009 §1).
        Event::GestureBegin { .. } | Event::GestureEnd { .. } => {}
    }
}

/// Decode a raw 16-byte slot and apply it to `synth` in one shot. Unknown tags
/// are ignored (forward-compat). Convenience for the worklet decode loop (0038).
#[inline]
pub fn decode_and_apply(buf: &[u8], synth: &mut Synth) {
    if let Some(ev) = decode(buf) {
        apply(&ev, synth);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Golden byte table ────────────────────────────────────────────────────
    //
    // Hand-written expected 16-byte arrays. This is THE contract: the JS codec
    // (`web/event-codec.test.mjs`) replicates this exact table and asserts its
    // own encode matches byte-for-byte. Any layout drift in either language
    // fails here. Little-endian; seq (off 10..12) and reserved (off 12..16) are
    // always zero from `encode`.
    //
    // Format helper for f32 little-endian bytes is inlined per row as a comment.

    /// (label, event, expected 16 bytes)
    fn golden() -> Vec<(&'static str, Event, [u8; SLOT_BYTES])> {
        // f32 LE byte helpers (so the table is auditable):
        //  1.0   = 00 00 80 3F
        //  0.0   = 00 00 00 00
        //  0.5   = 00 00 00 3F
        // -1.0   = 00 00 80 BF
        //  100.0 = 00 00 C8 42
        let le = |v: f32| v.to_le_bytes();
        let f1 = le(1.0);
        let f0 = le(0.0);
        let fhalf = le(0.5);
        let fneg1 = le(-1.0);
        let f100 = le(100.0);

        let row = |ty: u8,
                   off: u8,
                   pidx: u16,
                   val: [u8; 4],
                   note: u8,
                   flag: u8|
         -> [u8; SLOT_BYTES] {
            [
                ty,
                off,
                (pidx & 0xff) as u8,
                (pidx >> 8) as u8,
                val[0],
                val[1],
                val[2],
                val[3],
                note,
                flag,
                0,
                0,
                0,
                0,
                0,
                0,
            ]
        };

        vec![
            // note_on: note 0 vel 0.0 / note 127 vel 1.0
            (
                "note_on n0 v0",
                Event::NoteOn {
                    offset: 0,
                    note: 0,
                    velocity: 0.0,
                },
                row(EV_NOTE_ON, 0, 0, f0, 0, 0),
            ),
            (
                "note_on n127 v1 off7",
                Event::NoteOn {
                    offset: 7,
                    note: 127,
                    velocity: 1.0,
                },
                row(EV_NOTE_ON, 7, 0, f1, 127, 0),
            ),
            // note_off: note 0 / 127
            (
                "note_off n0",
                Event::NoteOff {
                    offset: 0,
                    note: 0,
                },
                row(EV_NOTE_OFF, 0, 0, f0, 0, 0),
            ),
            (
                "note_off n127 off127",
                Event::NoteOff {
                    offset: 127,
                    note: 127,
                },
                row(EV_NOTE_OFF, 127, 0, f0, 127, 0),
            ),
            // set_param plain: id 0 / id 164 (TOTAL_PARAMS-1)
            (
                "param plain id0 v0.5",
                Event::SetParam {
                    offset: 0,
                    id: 0,
                    plain: 0.5,
                },
                row(EV_PARAM, 0, 0, fhalf, 0, 0),
            ),
            (
                "param plain id164 v100",
                Event::SetParam {
                    offset: 3,
                    id: 164,
                    plain: 100.0,
                },
                row(EV_PARAM, 3, 164, f100, 0, 0),
            ),
            // set_param_norm: flag bit set
            (
                "param norm id0 n0",
                Event::SetParamNorm {
                    offset: 0,
                    id: 0,
                    norm: 0.0,
                },
                row(EV_PARAM, 0, 0, f0, 0, PARAM_FLAG_NORM),
            ),
            (
                "param norm id164 n1",
                Event::SetParamNorm {
                    offset: 0,
                    id: 164,
                    norm: 1.0,
                },
                row(EV_PARAM, 0, 164, f1, 0, PARAM_FLAG_NORM),
            ),
            // gestures: id in paramIdx, no Synth effect
            (
                "gesture_begin id12",
                Event::GestureBegin {
                    offset: 0,
                    id: 12,
                },
                row(EV_GESTURE_BEGIN, 0, 12, f0, 0, 0),
            ),
            (
                "gesture_end id12",
                Event::GestureEnd {
                    offset: 0,
                    id: 12,
                },
                row(EV_GESTURE_END, 0, 12, f0, 0, 0),
            ),
            // pitch bend: -1 / +1
            (
                "pitch_bend -1",
                Event::PitchBend {
                    offset: 0,
                    norm: -1.0,
                },
                row(EV_PITCH_BEND, 0, 0, fneg1, 0, 0),
            ),
            (
                "pitch_bend +1",
                Event::PitchBend {
                    offset: 0,
                    norm: 1.0,
                },
                row(EV_PITCH_BEND, 0, 0, f1, 0, 0),
            ),
            // mod wheel: 0 / 1
            (
                "mod_wheel 0",
                Event::ModWheel {
                    offset: 0,
                    norm: 0.0,
                },
                row(EV_MOD_WHEEL, 0, 0, f0, 0, 0),
            ),
            (
                "mod_wheel 1",
                Event::ModWheel {
                    offset: 0,
                    norm: 1.0,
                },
                row(EV_MOD_WHEEL, 0, 0, f1, 0, 0),
            ),
            // sustain off / on
            (
                "sustain off",
                Event::Sustain {
                    offset: 0,
                    on: false,
                },
                row(EV_SUSTAIN, 0, 0, f0, 0, 0),
            ),
            (
                "sustain on",
                Event::Sustain {
                    offset: 0,
                    on: true,
                },
                row(EV_SUSTAIN, 0, 0, f0, 0, 1),
            ),
            // key modes: all three (0 Whole, 1 Dual, 2 Split)
            (
                "key_mode whole",
                Event::KeyMode {
                    offset: 0,
                    mode: 0,
                },
                row(EV_KEY_MODE, 0, 0, f0, 0, 0),
            ),
            (
                "key_mode dual",
                Event::KeyMode {
                    offset: 0,
                    mode: 1,
                },
                row(EV_KEY_MODE, 0, 0, f0, 0, 1),
            ),
            (
                "key_mode split",
                Event::KeyMode {
                    offset: 0,
                    mode: 2,
                },
                row(EV_KEY_MODE, 0, 0, f0, 0, 2),
            ),
            // split point: 0 / 60 / 127
            (
                "split_point 0",
                Event::SplitPoint {
                    offset: 0,
                    note: 0,
                },
                row(EV_SPLIT_POINT, 0, 0, f0, 0, 0),
            ),
            (
                "split_point 60",
                Event::SplitPoint {
                    offset: 0,
                    note: 60,
                },
                row(EV_SPLIT_POINT, 0, 0, f0, 0, 60),
            ),
            (
                "split_point 127",
                Event::SplitPoint {
                    offset: 0,
                    note: 127,
                },
                row(EV_SPLIT_POINT, 0, 0, f0, 0, 127),
            ),
        ]
    }

    #[test]
    fn id_layout_matches_vxn_app() {
        // The constants must come from vxn-app, and equal the ADR 0009 layout
        // (165 = 2*69 + 27) for today's param table.
        assert_eq!(TOTAL_PARAMS, 165);
        assert_eq!(PATCH_COUNT, 69);
        assert_eq!(GLOBAL_COUNT, 27);
        assert_eq!(TOTAL_PARAMS, 2 * PATCH_COUNT + GLOBAL_COUNT);
    }

    #[test]
    fn encode_matches_golden_bytes() {
        for (label, ev, expected) in golden() {
            let got = encode(&ev);
            assert_eq!(got, expected, "encode mismatch for {label}: {got:02x?}");
        }
    }

    #[test]
    fn decode_matches_golden_bytes() {
        for (label, ev, bytes) in golden() {
            let got = decode(&bytes).unwrap_or_else(|| panic!("decode None for {label}"));
            assert_eq!(got, ev, "decode mismatch for {label}");
        }
    }

    #[test]
    fn unknown_tag_decodes_none() {
        let mut buf = [0u8; SLOT_BYTES];
        buf[0] = 200; // not a known tag
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn short_buffer_decodes_none() {
        let short = [EV_NOTE_ON, 0, 0, 0];
        assert!(decode(&short).is_none());
    }

    #[test]
    fn gestures_no_op_on_synth() {
        // Applying a gesture must leave the synth's audible state untouched:
        // identical output before/after.
        let mut a = Synth::new(48_000.0);
        let mut b = Synth::new(48_000.0);
        a.note_on(60, 0.8);
        b.note_on(60, 0.8);
        for ev in [
            Event::GestureBegin { offset: 0, id: 5 },
            Event::GestureEnd { offset: 0, id: 5 },
        ] {
            apply(&ev, &mut a);
        }
        let (mut la, mut ra) = ([0.0f32; 128], [0.0f32; 128]);
        let (mut lb, mut rb) = ([0.0f32; 128], [0.0f32; 128]);
        a.process(&mut la, &mut ra);
        b.process(&mut lb, &mut rb);
        assert_eq!(la, lb, "gesture changed left output");
        assert_eq!(ra, rb, "gesture changed right output");
    }

    // ── Dispatch-parity behaviour test ───────────────────────────────────────
    //
    // Decode a stream, apply to one Synth via the codec; apply the EQUIVALENT
    // calls to a reference Synth the way vxn-core-clap::dispatch_event +
    // SynthNotes would (note_on/off as-is, pitch_bend->set_pitch_bend,
    // mod_wheel->set_mod_wheel, sustain->sustain, param->set_param). Render
    // both and assert identical audio — proves semantic parity with the plugin.

    fn render(synth: &mut Synth) -> ([f32; 128], [f32; 128]) {
        let (mut l, mut r) = ([0.0f32; 128], [0.0f32; 128]);
        synth.process(&mut l, &mut r);
        (l, r)
    }

    #[test]
    fn dispatch_parity_with_clap_reference() {
        let sr = 48_000.0;

        // ---- codec path: build slots, decode, apply ----
        let mut codec_synth = Synth::new(sr);
        let stream = [
            Event::SetParam {
                offset: 0,
                id: 2,
                plain: 0.6,
            },
            Event::KeyMode { offset: 0, mode: 1 }, // Dual
            Event::SplitPoint {
                offset: 0,
                note: 60,
            },
            Event::NoteOn {
                offset: 0,
                note: 64,
                velocity: 0.9,
            },
            Event::NoteOn {
                offset: 0,
                note: 48,
                velocity: 0.5,
            },
            Event::PitchBend {
                offset: 0,
                norm: 0.25,
            },
            Event::ModWheel {
                offset: 0,
                norm: 0.7,
            },
            Event::Sustain { offset: 0, on: true },
            Event::NoteOff {
                offset: 0,
                note: 48,
            },
        ];
        for ev in &stream {
            let bytes = encode(ev);
            decode_and_apply(&bytes, &mut codec_synth);
        }

        // ---- reference path: the exact calls dispatch_event + SynthNotes make ----
        let mut ref_synth = Synth::new(sr);
        ref_synth.set_param(2, 0.6); // EV_PARAM plain -> set_param
        ref_synth.set_key_mode(KeyMode::from_u8(1));
        ref_synth.set_split_point(60);
        ref_synth.note_on(64, 0.9); // velocity forwarded as-is
        ref_synth.note_on(48, 0.5);
        ref_synth.set_pitch_bend(0.25); // SynthNotes::pitch_bend
        ref_synth.set_mod_wheel(0.7); // SynthNotes::mod_wheel
        ref_synth.sustain(true);
        ref_synth.note_off(48); // deferred under sustain — both see it

        let (cl, cr) = render(&mut codec_synth);
        let (rl, rr) = render(&mut ref_synth);
        assert_eq!(cl, rl, "left output diverges from CLAP reference");
        assert_eq!(cr, rr, "right output diverges from CLAP reference");

        // Render a few more quanta to surface any latent state divergence
        // (envelope, sustain release on a later note-off, etc.).
        for _ in 0..4 {
            assert_eq!(render(&mut codec_synth), render(&mut ref_synth));
        }
    }

    #[test]
    fn param_norm_decodes_to_plain_via_paramdesc() {
        // norm value applied through the codec must equal set_param of the
        // from_normalized plain value for the same id.
        let id: u16 = 5;
        let norm = 0.42f32;
        let desc = desc_for_clap_id(id as usize).unwrap();
        let plain = desc.from_normalized(norm);

        let mut codec_synth = Synth::new(48_000.0);
        apply(
            &Event::SetParamNorm {
                offset: 0,
                id,
                norm,
            },
            &mut codec_synth,
        );

        let mut ref_synth = Synth::new(48_000.0);
        ref_synth.set_param(id as usize, plain);

        codec_synth.note_on(60, 1.0);
        ref_synth.note_on(60, 1.0);
        assert_eq!(render(&mut codec_synth), render(&mut ref_synth));
    }
}
