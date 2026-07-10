//! Binary event codec — the wire format for the E030 event ring (ticket 0153).
//!
//! ONE definition, two implementations. This is the Rust half; the JS half is
//! `web/event-codec.mjs` (ticket 0155). Both are typed encode/decode layers over
//! a **16-byte fixed slot framing** carried verbatim from vxn-1's frozen spike-
//! 0035 layout — so the two synths' web transports share a byte format and the
//! JS ring/codec ports over unchanged. The one-page spec both halves point at
//! is `web/WIRE-FORMAT.md`.
//!
//! # Slot layout (16 bytes, little-endian)
//!
//! ```text
//! off 0  u8   type      EV_* tag
//! off 1  u8   offset    sample offset within the upcoming quantum, 0..Q-1
//! off 2  u16  paramIdx  CLAP param id (EV_PARAM / gestures; else 0)
//! off 4  f32  value     velocity / param value / bend / wheel
//! off 8  u8   note      MIDI note number (EV_NOTE_ON/OFF)
//! off 9  u8   flag      sustain 0/1, OR the param-norm bit (EV_PARAM)
//! off 10 u16  seq       producer sequence (low 16 bits) — owned by the ring,
//!                       not the codec; encode writes 0, decode ignores it.
//! off 12 f32  reserved  zero
//! ```
//!
//! # vxn-2 divergences from vxn-1's `vxn-wasm::codec`
//!
//! - **No key-mode / split-point events.** vxn-1's `EV_KEY_MODE` (7) /
//!   `EV_SPLIT_POINT` (8) drove its dual/split layer shared state. The vxn-2 FM
//!   engine has no such layering, so those tags and event kinds are dropped.
//!   The remaining tags keep their vxn-1 numbering (no renumbering) so a note /
//!   param / gesture record is byte-identical across both synths.
//! - **Param apply targets the atomic store, not the engine.** vxn-2 has no
//!   per-id `Engine::set_param`; a param edit writes the plain value into the
//!   [`SharedParams`] store, which the host folds into the engine once per block
//!   via `Engine::snapshot_params` (see [`crate::host`]). So [`apply`] takes both
//!   `&mut Engine` (notes / MIDI, applied immediately) and `&SharedParams`
//!   (params, block-granular). This matches the `vxn2-clap` dispatch split.
//! - **Velocity is `[0, 1]` on the wire, mapped to `1..=127` on apply** — the
//!   same mapping `vxn2-clap`'s `EngineNotesAdapter` does.

use vxn2_engine::engine::Engine;
use vxn2_engine::shared::SharedParams;
use vxn2_engine::TOTAL_PARAMS as VXN2_TOTAL_PARAMS;

/// Total addressable CLAP ids (the vxn-2 param table size). 209 today — well
/// inside the `u16` `paramIdx` field. Re-exported from `vxn2-engine`, never
/// hard-coded, so a param add/remove flows through.
pub const TOTAL_PARAMS: u16 = VXN2_TOTAL_PARAMS as u16;

/// Bytes per slot — must equal the ring's `SLOT_BYTES` (0155).
pub const SLOT_BYTES: usize = 16;

// ── Event type tags (numbering carried from vxn-1's 0035/0037 layout) ────────

/// `note_on { note, velocity }`. `value` = velocity `[0,1]`, `note` = key.
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
// Tags 7 (key_mode) and 8 (split_point) are intentionally unused in vxn-2 —
// reserved so the numbering stays aligned with vxn-1's wire format.
/// `gesture_begin { id }`. `paramIdx` = id. Decoder no-ops on the engine.
pub const EV_GESTURE_BEGIN: u8 = 9;
/// `gesture_end { id }`. `paramIdx` = id. Decoder no-ops on the engine.
pub const EV_GESTURE_END: u8 = 10;

/// `flag` bit on [`EV_PARAM`] selecting the normalised encoding. `0` = plain
/// value (engine-domain f32), `1` = normalised `[0, 1]` (taper applied on decode
/// via `ParamDesc::from_normalised`).
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
            | Event::Sustain { offset, .. } => offset,
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
                Event::SetParamNorm { offset, id, norm: value }
            } else {
                Event::SetParam { offset, id, plain: value }
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
        _ => return None, // unknown tag: ignore (forward-compat)
    })
}

// ── Apply (dispatch parity with vxn2-clap::dispatch_event) ───────────────────

/// Map a wire velocity (`[0, 1]`) to the engine's `1..=127` — the exact mapping
/// `vxn2-clap`'s `EngineNotesAdapter::note_on` performs.
#[inline]
fn vel_to_u8(velocity: f32) -> u8 {
    ((velocity * 127.0) as i32).clamp(1, 127) as u8
}

/// Apply a decoded event, with semantics **identical** to `vxn2-clap`'s
/// `dispatch_event` split:
///
/// - Notes / raw MIDI act on `engine` immediately (sample-accurate within the
///   quantum): `NoteOn` maps velocity `[0,1] → 1..=127`; `PitchBend` /
///   `ModWheel` forward the already-normalised value; `Sustain` toggles CC64.
/// - `SetParam{plain}` / `SetParamNorm{norm}` write the plain value into the
///   atomic [`SharedParams`] store. The engine folds the store at block start
///   (`Engine::snapshot_params`), so a mid-quantum param edit lands at the next
///   quantum — matching the plugin, which folds params per block and documents
///   the same one-block latency. Unknown ids are dropped by `SharedParams::set`.
/// - `GestureBegin` / `GestureEnd` → **no-op** (controller / host-echo concern;
///   they never reach rendering).
#[inline]
pub fn apply(event: &Event, engine: &mut Engine, shared: &SharedParams) {
    match *event {
        Event::NoteOn { note, velocity, .. } => engine.note_on(note, vel_to_u8(velocity)),
        Event::NoteOff { note, .. } => engine.note_off(note),
        Event::SetParam { id, plain, .. } => shared.set(id as usize, plain),
        Event::SetParamNorm { id, norm, .. } => shared.set_normalised(id as usize, norm),
        Event::PitchBend { norm, .. } => engine.set_pitch_bend(norm),
        Event::ModWheel { norm, .. } => engine.set_mod_wheel(norm),
        Event::Sustain { on, .. } => engine.set_sustain(on),
        // Gestures never touch the renderer.
        Event::GestureBegin { .. } | Event::GestureEnd { .. } => {}
    }
}

/// Decode a raw 16-byte slot and apply it in one shot. Unknown tags are ignored
/// (forward-compat). Convenience for the worklet decode loop (host render).
#[inline]
pub fn decode_and_apply(buf: &[u8], engine: &mut Engine, shared: &SharedParams) {
    if let Some(ev) = decode(buf) {
        apply(&ev, engine, shared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Golden byte table ────────────────────────────────────────────────────
    //
    // Hand-written expected 16-byte arrays. This is THE contract: the JS codec
    // (`web/event-codec.test.mjs`, ticket 0155) replicates this exact table and
    // asserts its own encode matches byte-for-byte. Any layout drift in either
    // language fails here. Little-endian; seq (off 10..12) and reserved
    // (off 12..16) are always zero from `encode`.

    /// (label, event, expected 16 bytes)
    fn golden() -> Vec<(&'static str, Event, [u8; SLOT_BYTES])> {
        let le = |v: f32| v.to_le_bytes();
        let f1 = le(1.0);
        let f0 = le(0.0);
        let fhalf = le(0.5);
        let fneg1 = le(-1.0);
        let f100 = le(100.0);

        let row = |ty: u8, off: u8, pidx: u16, val: [u8; 4], note: u8, flag: u8| -> [u8; SLOT_BYTES] {
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
            (
                "note_on n0 v0",
                Event::NoteOn { offset: 0, note: 0, velocity: 0.0 },
                row(EV_NOTE_ON, 0, 0, f0, 0, 0),
            ),
            (
                "note_on n127 v1 off7",
                Event::NoteOn { offset: 7, note: 127, velocity: 1.0 },
                row(EV_NOTE_ON, 7, 0, f1, 127, 0),
            ),
            (
                "note_off n0",
                Event::NoteOff { offset: 0, note: 0 },
                row(EV_NOTE_OFF, 0, 0, f0, 0, 0),
            ),
            (
                "note_off n127 off127",
                Event::NoteOff { offset: 127, note: 127 },
                row(EV_NOTE_OFF, 127, 0, f0, 127, 0),
            ),
            (
                "param plain id0 v0.5",
                Event::SetParam { offset: 0, id: 0, plain: 0.5 },
                row(EV_PARAM, 0, 0, fhalf, 0, 0),
            ),
            (
                "param plain id208 v100",
                Event::SetParam { offset: 3, id: 208, plain: 100.0 },
                row(EV_PARAM, 3, 208, f100, 0, 0),
            ),
            (
                "param norm id0 n0",
                Event::SetParamNorm { offset: 0, id: 0, norm: 0.0 },
                row(EV_PARAM, 0, 0, f0, 0, PARAM_FLAG_NORM),
            ),
            (
                "param norm id208 n1",
                Event::SetParamNorm { offset: 0, id: 208, norm: 1.0 },
                row(EV_PARAM, 0, 208, f1, 0, PARAM_FLAG_NORM),
            ),
            (
                "gesture_begin id12",
                Event::GestureBegin { offset: 0, id: 12 },
                row(EV_GESTURE_BEGIN, 0, 12, f0, 0, 0),
            ),
            (
                "gesture_end id12",
                Event::GestureEnd { offset: 0, id: 12 },
                row(EV_GESTURE_END, 0, 12, f0, 0, 0),
            ),
            (
                "pitch_bend -1",
                Event::PitchBend { offset: 0, norm: -1.0 },
                row(EV_PITCH_BEND, 0, 0, fneg1, 0, 0),
            ),
            (
                "pitch_bend +1",
                Event::PitchBend { offset: 0, norm: 1.0 },
                row(EV_PITCH_BEND, 0, 0, f1, 0, 0),
            ),
            (
                "mod_wheel 0",
                Event::ModWheel { offset: 0, norm: 0.0 },
                row(EV_MOD_WHEEL, 0, 0, f0, 0, 0),
            ),
            (
                "mod_wheel 1",
                Event::ModWheel { offset: 0, norm: 1.0 },
                row(EV_MOD_WHEEL, 0, 0, f1, 0, 0),
            ),
            (
                "sustain off",
                Event::Sustain { offset: 0, on: false },
                row(EV_SUSTAIN, 0, 0, f0, 0, 0),
            ),
            (
                "sustain on",
                Event::Sustain { offset: 0, on: true },
                row(EV_SUSTAIN, 0, 0, f0, 0, 1),
            ),
        ]
    }

    #[test]
    fn total_params_matches_vxn2_engine() {
        // The constant must come from vxn2-engine and fit the u16 paramIdx field.
        assert_eq!(TOTAL_PARAMS as usize, VXN2_TOTAL_PARAMS);
        assert!(VXN2_TOTAL_PARAMS <= u16::MAX as usize);
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
        // The two vxn-1 tags dropped in vxn-2 are likewise unknown here.
        buf[0] = 7; // was EV_KEY_MODE
        assert!(decode(&buf).is_none());
        buf[0] = 8; // was EV_SPLIT_POINT
        assert!(decode(&buf).is_none());
    }

    #[test]
    fn short_buffer_decodes_none() {
        let short = [EV_NOTE_ON, 0, 0, 0];
        assert!(decode(&short).is_none());
    }

    #[test]
    fn gestures_no_op_on_engine() {
        // Applying a gesture must leave the engine's audible state untouched:
        // identical output before/after.
        let sr = 48_000.0;
        let mut a = Engine::new(sr, crate::CONTROL_BLOCK);
        let mut b = Engine::new(sr, crate::CONTROL_BLOCK);
        let shared = SharedParams::new();
        a.snapshot_params(&shared);
        b.snapshot_params(&shared);
        a.note_on(60, 100);
        b.note_on(60, 100);
        for ev in [
            Event::GestureBegin { offset: 0, id: 5 },
            Event::GestureEnd { offset: 0, id: 5 },
        ] {
            apply(&ev, &mut a, &shared);
        }
        let (mut la, mut ra) = ([0.0f32; crate::CONTROL_BLOCK], [0.0f32; crate::CONTROL_BLOCK]);
        let (mut lb, mut rb) = ([0.0f32; crate::CONTROL_BLOCK], [0.0f32; crate::CONTROL_BLOCK]);
        a.process_block(&mut la, &mut ra);
        b.process_block(&mut lb, &mut rb);
        assert_eq!(la, lb, "gesture changed left output");
        assert_eq!(ra, rb, "gesture changed right output");
    }

    #[test]
    fn param_norm_decodes_to_plain_via_paramdesc() {
        // A norm write through the codec must equal a shared.set of the
        // from_normalised plain value for the same id.
        use vxn2_engine::desc_for_clap_id;
        let id: u16 = 5;
        let norm = 0.42f32;
        let desc = desc_for_clap_id(id as usize).unwrap();
        let plain = desc.from_normalised(norm);

        let via_codec = SharedParams::new();
        apply(
            &Event::SetParamNorm { offset: 0, id, norm },
            &mut Engine::new(48_000.0, crate::CONTROL_BLOCK),
            &via_codec,
        );

        let via_ref = SharedParams::new();
        via_ref.set(id as usize, plain);

        assert_eq!(via_codec.get(id as usize), via_ref.get(id as usize));
    }
}
