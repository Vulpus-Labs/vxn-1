//! Binary event codec — the wire format for the browser event ring (0286).
//!
//! ONE definition, two halves, and they are **not symmetric**: JS encodes
//! (`web/event-codec.mjs`, driven by the ring's producer) and this module
//! decodes. The framing is the 16-byte fixed slot vxn-1 froze in spike 0035 —
//! this module does not invent a layout, it formalises VXN1b's event set over
//! the existing slot and keeps it byte-compatible across all three synths.
//!
//! **The slot layout is written out in full in one place: `web/WIRE-FORMAT.md`.**
//! Read that before changing anything here; the tag table below states each
//! event's *meaning*, not the byte offsets.
//!
//! # VXN1b's event set vs vxn-1's
//!
//! Tags 1–10 keep vxn-1's meaning byte-for-byte. Three deltas, all forced by
//! what VXN1b *is* rather than chosen:
//!
//! - **Channel rides note events.** VXN1b's CLAP dispatch is deliberately
//!   MPE-aware (per-note pitch and pressure); vxn-1's is channel-agnostic and
//!   its codec has no channel field. `flag` is unused on tags 1/2 in the
//!   existing framing, so the channel goes there — no layout change, and a
//!   producer that writes 0 gets channel 0, which is what a non-MPE source
//!   wants. [`EV_POLY_PRESSURE`] / [`EV_CHANNEL_PRESSURE`] are new for the same
//!   reason: the engine has the surface, so the web path must be able to reach
//!   it.
//! - **Matrix topology has to travel.** Natively the topology lives in
//!   `SharedParams` behind a `Mutex<[MatrixTable; 2]>` that the audio thread can
//!   read directly. The worklet is a separate wasm with its own linear memory
//!   and cannot, so an edit rides the ring as [`EV_MATRIX_EDIT`]. Slot **depth**
//!   deliberately does not: it is an automatable CLAP param and stays on
//!   [`EV_PARAM`], which is the whole point of the 0219 split.
//! - **Tempo is an event.** `sync.rs` resolves LFO and delay subdivisions
//!   against host BPM. The browser has no host transport, so BPM arrives from a
//!   UI control as [`EV_TEMPO`].
//!
//! Two things vxn-1 carries that VXN1b deliberately does **not**:
//!
//! - **Sustain (tag 6) is reserved-unused.** VXN1b's native dispatch has no CC64
//!   path at all — inventing one here would make the web build behave
//!   differently from the plugin. The tag stays reserved so the numbering keeps
//!   lining up across the three synths; it decodes to `None`.
//! - **Layer copy is not on this wire.** `copy_layer` is a `SharedParams`
//!   operation, not an `Engine` one: it rewrites params and topology in the
//!   *model*. On the web that model lives in the controller, so a copy reaches
//!   the worklet as ordinary param writes plus [`EV_MATRIX_EDIT`] records —
//!   there is nothing for a dedicated tag to do.
//!
//! # Param addressing
//!
//! The id-layout constants are re-exported straight from `vxn1b-engine` —
//! **never hard-coded** — so a param add flows through instead of silently
//! desynchronising the JS mirror. That desync is exactly what broke both other
//! web ports (ticket 0285), and [`tests::total_params_matches_the_engine`] is
//! the guard against repeating it here.

use vxn1b_engine::params::{
    GLOBAL_COUNT as ENGINE_GLOBAL_COUNT, Layer, PATCH_COUNT as ENGINE_PATCH_COUNT,
    TOTAL_PARAMS as ENGINE_TOTAL_PARAMS, desc_for_clap_id,
};
use vxn1b_engine::{Engine, KeyOp, MatrixEdit, MatrixField, ScopeTap};

/// Per-layer patch params. Re-exported (never restated) so `vxn1b_patch_count()`
/// can hand JS the engine's own number.
pub const PATCH_COUNT: u16 = ENGINE_PATCH_COUNT as u16;

/// Globals, shared by both layers.
pub const GLOBAL_COUNT: u16 = ENGINE_GLOBAL_COUNT as u16;

/// Total addressable CLAP ids (`2 * PATCH_COUNT + GLOBAL_COUNT`).
///
/// The total alone is a weak guard: a `+1 patch / -2 global` drift leaves it
/// unchanged while `patchClapId` / `globalClapId` compute wrong ids on both
/// sides of the wire. All three counts are exported, and the handshake checks
/// all three.
pub const TOTAL_PARAMS: u16 = ENGINE_TOTAL_PARAMS as u16;

/// Bytes per slot — must equal the ring's `SLOT_BYTES`.
pub const SLOT_BYTES: usize = 16;

// ── Event type tags ─────────────────────────────────────────────────────────
//
// 1..=10 are vxn-1's, unchanged. 11..=16 are VXN1b's.

/// `note_on { note, velocity, channel }`. `value` = velocity, `note` = key,
/// `flag` = MIDI channel.
pub const EV_NOTE_ON: u8 = 1;
/// `note_off { note, channel }`. `note` = key, `flag` = MIDI channel.
pub const EV_NOTE_OFF: u8 = 2;
/// `set_param`/`set_param_norm`. `paramIdx` = id, `value` = plain or norm,
/// `flag` = [`PARAM_FLAG_NORM`] selects which.
pub const EV_PARAM: u8 = 3;
/// `pitch_bend { norm }`. `value` in `[-1, 1]`.
pub const EV_PITCH_BEND: u8 = 4;
/// `mod_wheel { norm }`. `value` in `[0, 1]`.
pub const EV_MOD_WHEEL: u8 = 5;
/// **Reserved, unused.** vxn-1's `sustain`. VXN1b's native dispatch has no CC64
/// path, so this decodes to `None` rather than inventing behaviour the plugin
/// does not have. Kept reserved so tag numbering stays aligned across synths.
pub const EV_SUSTAIN_RESERVED: u8 = 6;
/// `key_mode { mode }`. `flag` = mode (0 Single, 1 Dual, 2 Split) →
/// `KeyOp::SetKeyMode`.
pub const EV_KEY_MODE: u8 = 7;
/// `split_point { note }`. `flag` = note → `KeyOp::SetSplitPoint`.
pub const EV_SPLIT_POINT: u8 = 8;
/// `gesture_begin { id }`. `paramIdx` = id. Controller concern; no-ops here.
pub const EV_GESTURE_BEGIN: u8 = 9;
/// `gesture_end { id }`. `paramIdx` = id. Controller concern; no-ops here.
pub const EV_GESTURE_END: u8 = 10;
/// `lfo2_link { on }`. `flag` 0/1 → `KeyOp::SetLfo2Link`.
pub const EV_LFO2_LINK: u8 = 11;
/// `matrix_edit { layer, slot, field, value }`. `paramIdx` = the packed address
/// (`layer << 12 | slot << 8 | field`; see [`unpack_matrix_addr`]), `flag` = the
/// value byte.
pub const EV_MATRIX_EDIT: u8 = 12;
/// `scope_tap { tap }`. `flag` = `ScopeTap` code (0 Off, 1 Layer1, 2 Layer2).
pub const EV_SCOPE_TAP: u8 = 13;
/// `tempo { bpm }`. `value` = BPM.
pub const EV_TEMPO: u8 = 14;
/// `poly_pressure { note, value, channel }`. `note` = key, `value` in `[0, 1]`,
/// `flag` = MIDI channel.
pub const EV_POLY_PRESSURE: u8 = 15;
/// `channel_pressure { value, channel }`. `value` in `[0, 1]`, `flag` = channel.
pub const EV_CHANNEL_PRESSURE: u8 = 16;

/// `flag` bit on [`EV_PARAM`] selecting normalised encoding (0 plain, 1 norm).
pub const PARAM_FLAG_NORM: u8 = 1;

// ── Matrix address packing ──────────────────────────────────────────────────

/// Pack a matrix-slot address into the 16-bit `paramIdx` field:
/// `layer << 12 | slot << 8 | field`.
///
/// Room to spare — 2 layers, 16 slots, 7 fields — and it keeps `flag` free for
/// the value byte, so the whole edit fits one slot with no second record and no
/// framing change. The field nibble is a full byte wide, so the curve →
/// polarity/shape split cost the layout nothing.
///
/// **Test-only.** Nothing in this crate packs an address: the JS producer does
/// (`packMatrixAddr`), and this side only ever unpacks. It exists so
/// [`unpack_matrix_addr`] can be proven against the packing it inverts.
#[cfg(test)]
#[inline]
pub const fn pack_matrix_addr(layer: u8, slot: u8, field: u8) -> u16 {
    ((layer as u16) << 12) | ((slot as u16) << 8) | (field as u16)
}

/// Unpack a matrix-slot address (`layer << 12 | slot << 8 | field`). `None` for
/// a layer, slot or field outside the engine's range — a malformed record is
/// dropped, never clamped onto a valid slot it wasn't aimed at.
#[inline]
pub fn unpack_matrix_addr(addr: u16) -> Option<(Layer, u8, MatrixField)> {
    let layer = match addr >> 12 {
        0 => Layer::L1,
        1 => Layer::L2,
        _ => return None,
    };
    let slot = ((addr >> 8) & 0x0f) as u8;
    if slot as usize >= vxn1b_engine::matrix::N_SLOTS {
        return None;
    }
    // Ordinals match `vocab::MATRIX_FIELD_NAMES` positions, which are frozen —
    // 0..3 predate the polarity/shape split and must not move.
    let field = match addr & 0xff {
        0 => MatrixField::Source,
        1 => MatrixField::Dest,
        2 => MatrixField::Polarity,
        3 => MatrixField::ScaleSrc,
        4 => MatrixField::Shape,
        5 => MatrixField::ScaleShape,
        6 => MatrixField::Enabled,
        _ => return None,
    };
    Some((layer, slot, field))
}

/// Wire byte for a [`MatrixField`] — the inverse of [`unpack_matrix_addr`]'s
/// field decode, kept beside it so the two can't drift. **Test-only**, for the
/// same reason as [`pack_matrix_addr`].
#[cfg(test)]
#[inline]
pub const fn matrix_field_code(field: MatrixField) -> u8 {
    match field {
        MatrixField::Source => 0,
        MatrixField::Dest => 1,
        MatrixField::Polarity => 2,
        MatrixField::ScaleSrc => 3,
        MatrixField::Shape => 4,
        MatrixField::ScaleShape => 5,
        MatrixField::Enabled => 6,
    }
}

/// A decoded event. Zero-copy: produced by reading a 16-byte slot view; carries
/// no heap allocation. `offset` is the sample offset within the quantum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    NoteOn { offset: u8, channel: u8, note: u8, velocity: f32 },
    NoteOff { offset: u8, channel: u8, note: u8 },
    /// Plain (engine-domain) param value.
    SetParam { offset: u8, id: u16, plain: f32 },
    /// Normalised `[0, 1]` param value; apply converts to plain first.
    SetParamNorm { offset: u8, id: u16, norm: f32 },
    GestureBegin { offset: u8, id: u16 },
    GestureEnd { offset: u8, id: u16 },
    PitchBend { offset: u8, norm: f32 },
    ModWheel { offset: u8, norm: f32 },
    KeyMode { offset: u8, mode: u8 },
    SplitPoint { offset: u8, note: u8 },
    Lfo2Link { offset: u8, on: bool },
    MatrixEditEv { offset: u8, addr: u16, value: u8 },
    ScopeTapEv { offset: u8, tap: u8 },
    Tempo { offset: u8, bpm: f32 },
    PolyPressure { offset: u8, channel: u8, note: u8, value: f32 },
    ChannelPressure { offset: u8, channel: u8, value: f32 },
}

/// **Test-only**, and only because the encoder is. Both of these exist to serve
/// [`encode_into`]; production reads the tag and the offset straight out of the
/// raw slot (`host.rs`'s slice loop indexes bytes 0 and 1) rather than decoding
/// first, so nothing shipping calls either one.
#[cfg(test)]
impl Event {
    /// The wire tag this event encodes to.
    #[inline]
    pub fn tag(&self) -> u8 {
        match self {
            Event::NoteOn { .. } => EV_NOTE_ON,
            Event::NoteOff { .. } => EV_NOTE_OFF,
            Event::SetParam { .. } | Event::SetParamNorm { .. } => EV_PARAM,
            Event::PitchBend { .. } => EV_PITCH_BEND,
            Event::ModWheel { .. } => EV_MOD_WHEEL,
            Event::KeyMode { .. } => EV_KEY_MODE,
            Event::SplitPoint { .. } => EV_SPLIT_POINT,
            Event::GestureBegin { .. } => EV_GESTURE_BEGIN,
            Event::GestureEnd { .. } => EV_GESTURE_END,
            Event::Lfo2Link { .. } => EV_LFO2_LINK,
            Event::MatrixEditEv { .. } => EV_MATRIX_EDIT,
            Event::ScopeTapEv { .. } => EV_SCOPE_TAP,
            Event::Tempo { .. } => EV_TEMPO,
            Event::PolyPressure { .. } => EV_POLY_PRESSURE,
            Event::ChannelPressure { .. } => EV_CHANNEL_PRESSURE,
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
            | Event::KeyMode { offset, .. }
            | Event::SplitPoint { offset, .. }
            | Event::Lfo2Link { offset, .. }
            | Event::MatrixEditEv { offset, .. }
            | Event::ScopeTapEv { offset, .. }
            | Event::Tempo { offset, .. }
            | Event::PolyPressure { offset, .. }
            | Event::ChannelPressure { offset, .. } => offset,
        }
    }
}

// ── Encode (test-only) ──────────────────────────────────────────────────────
//
// This crate never encodes. The bytes the worklet decodes are written by JS —
// `EventRing._push` via `encodeInto` — so an encoder here would be a second
// implementation of a format that already has two. It is kept `#[cfg(test)]`
// because the golden table's round-trip is worth having: it proves this
// module's decode against a table the JS golden table is checked against in
// turn, so a layout slip in either language fails a test.

#[cfg(test)]
#[inline]
fn put_u16(buf: &mut [u8; SLOT_BYTES], at: usize, v: u16) {
    buf[at] = (v & 0xff) as u8;
    buf[at + 1] = (v >> 8) as u8;
}

#[cfg(test)]
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

/// Encode `event` into a fresh 16-byte slot. **Test-only** — see the section
/// banner. The `seq` field (off 10) is owned by the ring writer, not the codec,
/// so it is left zero here.
#[cfg(test)]
#[inline]
pub fn encode(event: &Event) -> [u8; SLOT_BYTES] {
    let mut buf = [0u8; SLOT_BYTES];
    encode_into(event, &mut buf);
    buf
}

/// Encode `event` into an existing 16-byte buffer in place (fully overwrites all
/// 16 bytes). **Test-only** — see the section banner.
#[cfg(test)]
#[inline]
pub fn encode_into(event: &Event, buf: &mut [u8; SLOT_BYTES]) {
    *buf = [0u8; SLOT_BYTES];
    buf[0] = event.tag();
    buf[1] = event.offset();
    match *event {
        Event::NoteOn { channel, note, velocity, .. } => {
            put_f32(buf, 4, velocity);
            buf[8] = note;
            buf[9] = channel;
        }
        Event::NoteOff { channel, note, .. } => {
            buf[8] = note;
            buf[9] = channel;
        }
        Event::SetParam { id, plain, .. } => {
            put_u16(buf, 2, id);
            put_f32(buf, 4, plain);
        }
        Event::SetParamNorm { id, norm, .. } => {
            put_u16(buf, 2, id);
            put_f32(buf, 4, norm);
            buf[9] = PARAM_FLAG_NORM;
        }
        Event::GestureBegin { id, .. } | Event::GestureEnd { id, .. } => put_u16(buf, 2, id),
        Event::PitchBend { norm, .. } | Event::ModWheel { norm, .. } => put_f32(buf, 4, norm),
        Event::KeyMode { mode, .. } => buf[9] = mode,
        Event::SplitPoint { note, .. } => buf[9] = note,
        Event::Lfo2Link { on, .. } => buf[9] = on as u8,
        Event::MatrixEditEv { addr, value, .. } => {
            put_u16(buf, 2, addr);
            buf[9] = value;
        }
        Event::ScopeTapEv { tap, .. } => buf[9] = tap,
        Event::Tempo { bpm, .. } => put_f32(buf, 4, bpm),
        Event::PolyPressure { channel, note, value, .. } => {
            put_f32(buf, 4, value);
            buf[8] = note;
            buf[9] = channel;
        }
        Event::ChannelPressure { channel, value, .. } => {
            put_f32(buf, 4, value);
            buf[9] = channel;
        }
    }
}

// ── Decode ──────────────────────────────────────────────────────────────────

/// Decode a 16-byte slot. Borrowed slice, allocates nothing. Returns `None` for
/// an unknown or reserved tag (forward-compat with future event kinds), or if
/// `buf` is too short.
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
            channel: buf[9],
            note: buf[8],
            velocity: get_f32(buf, 4),
        },
        EV_NOTE_OFF => Event::NoteOff {
            offset,
            channel: buf[9],
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
        EV_GESTURE_BEGIN => Event::GestureBegin { offset, id: get_u16(buf, 2) },
        EV_GESTURE_END => Event::GestureEnd { offset, id: get_u16(buf, 2) },
        EV_PITCH_BEND => Event::PitchBend { offset, norm: get_f32(buf, 4) },
        EV_MOD_WHEEL => Event::ModWheel { offset, norm: get_f32(buf, 4) },
        EV_KEY_MODE => Event::KeyMode { offset, mode: buf[9] },
        EV_SPLIT_POINT => Event::SplitPoint { offset, note: buf[9] },
        EV_LFO2_LINK => Event::Lfo2Link { offset, on: buf[9] != 0 },
        EV_MATRIX_EDIT => Event::MatrixEditEv {
            offset,
            addr: get_u16(buf, 2),
            value: buf[9],
        },
        EV_SCOPE_TAP => Event::ScopeTapEv { offset, tap: buf[9] },
        EV_TEMPO => Event::Tempo { offset, bpm: get_f32(buf, 4) },
        EV_POLY_PRESSURE => Event::PolyPressure {
            offset,
            channel: buf[9],
            note: buf[8],
            value: get_f32(buf, 4),
        },
        EV_CHANNEL_PRESSURE => Event::ChannelPressure {
            offset,
            channel: buf[9],
            value: get_f32(buf, 4),
        },
        // Unknown or reserved (incl. EV_SUSTAIN_RESERVED): ignore.
        _ => return None,
    })
}

// ── Apply (dispatch parity with vxn1b-clap's `dispatch`) ────────────────────

/// Apply a decoded event to an [`Engine`], with semantics identical to the
/// plugin's bespoke `dispatch` + the CLAP batch loop in `vxn1b-clap`:
///
/// - `NoteOn`/`NoteOff` → `Engine::note_on/note_off(channel, note, …)`. Velocity
///   is forwarded as-is (CLAP `[0,1]`; the engine owns the mapping).
/// - `PolyPressure`/`ChannelPressure` → the matching engine call. The MIDI
///   `d2 / 127.0` scaling the plugin does for raw bytes is an **encoder-side**
///   concern (the Web MIDI adapter, 0294); this wire carries normalised values.
/// - `SetParam{plain}` → `Engine::set_param(id, plain)`.
/// - `SetParamNorm{norm}` → convert via the param's `ParamDesc::from_normalized`
///   (the engine carries plain values), then `set_param`. Unknown ids are
///   dropped, matching CLAP ignoring unknown ids.
/// - `PitchBend`/`ModWheel` → the engine setters. Both carry already-normalised
///   values; the 14-bit bend conversion and the CC1 deadzone are encoder-side.
/// - `KeyMode`/`SplitPoint`/`Lfo2Link` → the three `KeyOp`s, folded through the
///   engine's `KeyState`. Non-automatable domain state, not params.
/// - `MatrixEditEv` → retarget one slot's topology on one layer. **Depth is not
///   touched** — it is a CLAP param and arrives on `EV_PARAM`.
/// - `ScopeTapEv` → point the capture ring at a layer (or off).
/// - `Tempo` → `Engine::set_tempo(bpm)` for the synced LFO/delay rates.
/// - `GestureBegin`/`GestureEnd` → **no-op**: controller / host-echo concern,
///   they never reach rendering.
#[inline]
pub fn apply(event: &Event, engine: &mut Engine) {
    match *event {
        Event::NoteOn { channel, note, velocity, .. } => {
            engine.note_on(channel, note, velocity);
        }
        Event::NoteOff { channel, note, .. } => engine.note_off(channel, note),
        Event::PolyPressure { channel, note, value, .. } => {
            engine.poly_pressure(channel, note, value)
        }
        Event::ChannelPressure { channel, value, .. } => engine.channel_pressure(channel, value),
        Event::SetParam { id, plain, .. } => engine.set_param(id as usize, plain),
        Event::SetParamNorm { id, norm, .. } => {
            if let Some(desc) = desc_for_clap_id(id as usize) {
                engine.set_param(id as usize, desc.from_normalized(norm));
            }
        }
        Event::PitchBend { norm, .. } => engine.set_pitch_bend(norm),
        Event::ModWheel { norm, .. } => engine.set_mod_wheel(norm),
        Event::KeyMode { mode, .. } => apply_key_op(engine, KeyOp::SetKeyMode(mode)),
        Event::SplitPoint { note, .. } => apply_key_op(engine, KeyOp::SetSplitPoint(note)),
        Event::Lfo2Link { on, .. } => apply_key_op(engine, KeyOp::SetLfo2Link(on)),
        Event::MatrixEditEv { addr, value, .. } => {
            if let Some((layer, slot, field)) = unpack_matrix_addr(addr) {
                let edit = MatrixEdit { layer, slot, field, value };
                apply_matrix_edit(engine, edit);
            }
        }
        Event::ScopeTapEv { tap, .. } => engine.scope().set_source(ScopeTap::from_code(tap).code()),
        Event::Tempo { bpm, .. } => engine.set_tempo(bpm),
        // Gestures never touch the renderer.
        Event::GestureBegin { .. } | Event::GestureEnd { .. } => {}
    }
}

/// Fold one `KeyOp` through the engine's live `KeyState`.
///
/// Read-modify-write rather than a direct setter: `KeyState::apply` is where the
/// mode→toggles mapping lives (Single clears layer 2, Dual/Split set it and move
/// `split_enabled`, both preserving the split point), and duplicating that here
/// is how the web build would drift from the plugin.
#[inline]
fn apply_key_op(engine: &mut Engine, op: KeyOp) {
    let mut key = engine.key_state();
    key.apply(op);
    engine.set_key_state(key);
}

/// Write one topology field of one matrix slot.
///
/// Resolves the layer and hands the record to
/// [`vxn1b_engine::topology::apply_edit`], which is the **only** `MatrixField`
/// write-match in the tree (0339): the store's main-thread table, the plugin's
/// audio-thread drain and this web decode path all go through it. So an
/// out-of-range wire byte lands on the same `from_u8` fallback the plugin picks
/// by construction, not by two transcriptions happening to agree.
#[inline]
fn apply_matrix_edit(engine: &mut Engine, edit: MatrixEdit) {
    vxn1b_engine::topology::apply_edit(engine.matrix_mut(edit.layer), edit);
}

/// Decode a raw 16-byte slot and apply it in one shot. Unknown tags are ignored
/// (forward-compat). Convenience for the worklet decode loop.
#[inline]
pub fn decode_and_apply(buf: &[u8], engine: &mut Engine) {
    if let Some(ev) = decode(buf) {
        apply(&ev, engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn1b_engine::matrix::{DestId, Polarity, Shape, SourceId};
    use vxn1b_engine::params::{ParamId, clap_id_of};

    // ── Golden byte table ───────────────────────────────────────────────────
    //
    // Hand-written expected 16-byte arrays. This is THE contract: 0287's JS
    // codec replicates this exact table and asserts its own encode matches
    // byte-for-byte, so layout drift in either language fails here.
    // Little-endian; seq (10..12) and reserved (12..16) are always zero.
    //
    // f32 LE helpers, so the table is auditable by eye:
    //   1.0 = 00 00 80 3F   0.5 = 00 00 00 3F   -1.0 = 00 00 80 BF
    //   120.0 = 00 00 F0 42
    fn golden() -> Vec<(&'static str, Event, [u8; SLOT_BYTES])> {
        vec![
            (
                "note_on ch0 n60 v1.0",
                Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 },
                [1, 0, 0, 0, 0x00, 0x00, 0x80, 0x3f, 60, 0, 0, 0, 0, 0, 0, 0],
            ),
            // The MPE case: channel in `flag` (off 9), which vxn-1 leaves zero.
            (
                "note_on ch3 n60 v0.5 off7",
                Event::NoteOn { offset: 7, channel: 3, note: 60, velocity: 0.5 },
                [1, 7, 0, 0, 0x00, 0x00, 0x00, 0x3f, 60, 3, 0, 0, 0, 0, 0, 0],
            ),
            (
                "note_off ch3 n60",
                Event::NoteOff { offset: 0, channel: 3, note: 60 },
                [2, 0, 0, 0, 0, 0, 0, 0, 60, 3, 0, 0, 0, 0, 0, 0],
            ),
            (
                "param plain id5 v0.5",
                Event::SetParam { offset: 0, id: 5, plain: 0.5 },
                [3, 0, 5, 0, 0x00, 0x00, 0x00, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "param norm id300 n1.0",
                Event::SetParamNorm { offset: 0, id: 300, norm: 1.0 },
                [3, 0, 0x2c, 0x01, 0x00, 0x00, 0x80, 0x3f, 0, 1, 0, 0, 0, 0, 0, 0],
            ),
            (
                "pitch_bend -1.0",
                Event::PitchBend { offset: 0, norm: -1.0 },
                [4, 0, 0, 0, 0x00, 0x00, 0x80, 0xbf, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "mod_wheel 1.0",
                Event::ModWheel { offset: 0, norm: 1.0 },
                [5, 0, 0, 0, 0x00, 0x00, 0x80, 0x3f, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "key_mode 2 (split)",
                Event::KeyMode { offset: 0, mode: 2 },
                [7, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0],
            ),
            (
                "split_point 60",
                Event::SplitPoint { offset: 0, note: 60 },
                [8, 0, 0, 0, 0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0],
            ),
            (
                "gesture_begin id12",
                Event::GestureBegin { offset: 0, id: 12 },
                [9, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "gesture_end id12",
                Event::GestureEnd { offset: 0, id: 12 },
                [10, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "lfo2_link on",
                Event::Lfo2Link { offset: 0, on: true },
                [11, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0],
            ),
            // L2 (1<<12 = 0x1000), slot 5 (5<<8 = 0x0500), field Dest (1)
            // => 0x1501 => LE bytes 01 15. Value = DestId::Cutoff (4).
            (
                "matrix_edit L2 slot5 dest=Cutoff",
                Event::MatrixEditEv { offset: 0, addr: 0x1501, value: 4 },
                [12, 0, 0x01, 0x15, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0],
            ),
            (
                "scope_tap Layer2",
                Event::ScopeTapEv { offset: 0, tap: 2 },
                [13, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0],
            ),
            (
                "tempo 120",
                Event::Tempo { offset: 0, bpm: 120.0 },
                [14, 0, 0, 0, 0x00, 0x00, 0xf0, 0x42, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                "poly_pressure ch3 n60 1.0",
                Event::PolyPressure { offset: 0, channel: 3, note: 60, value: 1.0 },
                [15, 0, 0, 0, 0x00, 0x00, 0x80, 0x3f, 60, 3, 0, 0, 0, 0, 0, 0],
            ),
            (
                "channel_pressure ch3 1.0",
                Event::ChannelPressure { offset: 0, channel: 3, value: 1.0 },
                [16, 0, 0, 0, 0x00, 0x00, 0x80, 0x3f, 0, 3, 0, 0, 0, 0, 0, 0],
            ),
        ]
    }

    #[test]
    fn encode_matches_the_golden_table() {
        for (label, ev, want) in golden() {
            assert_eq!(encode(&ev), want, "encode mismatch for {label}");
        }
    }

    #[test]
    fn decode_of_golden_bytes_yields_the_event() {
        for (label, ev, bytes) in golden() {
            assert_eq!(decode(&bytes), Some(ev), "decode mismatch for {label}");
        }
    }

    #[test]
    fn every_event_round_trips() {
        for (label, ev, _) in golden() {
            assert_eq!(decode(&encode(&ev)), Some(ev), "round-trip failed for {label}");
        }
    }

    /// Forward-compat: a tag this build doesn't know is skipped, not
    /// misinterpreted as whatever kind happens to share its layout.
    #[test]
    fn unknown_and_reserved_tags_decode_to_none() {
        let mut buf = [0u8; SLOT_BYTES];
        for tag in [0u8, EV_SUSTAIN_RESERVED, 17, 200, 255] {
            buf[0] = tag;
            assert_eq!(decode(&buf), None, "tag {tag} must not decode");
        }
    }

    #[test]
    fn a_short_slot_decodes_to_none() {
        assert_eq!(decode(&[1, 0, 0]), None);
    }

    /// The drift guard. Ticket 0285 is what happens when a param-space size is
    /// copied by hand and the source of truth then moves; this constant is
    /// re-exported from the engine so it cannot.
    #[test]
    fn total_params_matches_the_engine() {
        assert_eq!(PATCH_COUNT as usize, vxn1b_engine::params::PATCH_COUNT);
        assert_eq!(GLOBAL_COUNT as usize, vxn1b_engine::params::GLOBAL_COUNT);
        assert_eq!(TOTAL_PARAMS as usize, vxn1b_engine::params::TOTAL_PARAMS);
        assert_eq!(TOTAL_PARAMS, 2 * PATCH_COUNT + GLOBAL_COUNT, "the three must be consistent");
        assert!(TOTAL_PARAMS as usize <= u16::MAX as usize, "must fit the u16 paramIdx field");
    }

    #[test]
    fn matrix_addresses_round_trip() {
        for (layer, layer_ix) in [(Layer::L1, 0u8), (Layer::L2, 1u8)] {
            for slot in 0..vxn1b_engine::matrix::N_SLOTS as u8 {
                for field in [
                    MatrixField::Source,
                    MatrixField::Dest,
                    MatrixField::Polarity,
                    MatrixField::ScaleSrc,
                    MatrixField::Shape,
                    MatrixField::ScaleShape,
                    MatrixField::Enabled,
                ] {
                    let addr = pack_matrix_addr(layer_ix, slot, matrix_field_code(field));
                    assert_eq!(unpack_matrix_addr(addr), Some((layer, slot, field)));
                }
            }
        }
    }

    /// A malformed address is dropped rather than clamped: clamping would land
    /// the edit on a real slot the sender never aimed at, silently rewiring a
    /// patch.
    #[test]
    fn out_of_range_matrix_addresses_decode_to_none() {
        assert_eq!(unpack_matrix_addr(pack_matrix_addr(2, 0, 0)), None, "layer 2");
        assert_eq!(unpack_matrix_addr(0x0001 | (4 << 8) | (0xf << 12)), None, "layer 15");
        // Fields 0..=6 are real now that curve split into polarity + shape and
        // the scale bend + on/off switch joined them; 7 is the first past the end.
        assert_eq!(unpack_matrix_addr(pack_matrix_addr(0, 0, 7)), None, "field 7");
        assert_eq!(unpack_matrix_addr(pack_matrix_addr(0, 0, 255)), None, "field 255");
    }

    // ── Apply ───────────────────────────────────────────────────────────────

    fn engine() -> Engine {
        Engine::new(48_000.0)
    }

    /// The 0219 split, enforced on the wire: topology travels as a matrix edit,
    /// depth stays an automatable CLAP param. A matrix edit that quietly reset
    /// depth would silence an automated slot on every topology change.
    #[test]
    fn a_matrix_edit_retargets_the_slot_and_leaves_its_depth_alone() {
        let mut e = engine();
        let depth_id = clap_id_of(Layer::L2, ParamId::MatrixSlot5Depth);
        e.set_param(depth_id, 0.75);

        let addr = pack_matrix_addr(1, 5, matrix_field_code(MatrixField::Dest));
        apply(&Event::MatrixEditEv { offset: 0, addr, value: DestId::Cutoff as u8 }, &mut e);
        apply(
            &Event::MatrixEditEv {
                offset: 0,
                addr: pack_matrix_addr(1, 5, matrix_field_code(MatrixField::Source)),
                value: SourceId::Lfo2 as u8,
            },
            &mut e,
        );

        let slot = e.matrix_mut(Layer::L2).slots[5];
        assert_eq!(slot.dest, DestId::Cutoff, "dest retargeted");
        assert_eq!(slot.source, SourceId::Lfo2, "source retargeted");
        assert_eq!(e.param(depth_id), 0.75, "depth is a param and must be untouched");
        // ...and the edit landed on the layer it was addressed to, not both.
        assert_ne!(
            e.matrix_mut(Layer::L1).slots[5].source,
            SourceId::Lfo2,
            "Layer 1's slot 5 must be unaffected"
        );
    }

    #[test]
    fn a_shape_edit_decodes_through_the_same_from_u8_the_store_uses() {
        let mut e = engine();
        let addr = pack_matrix_addr(0, 2, matrix_field_code(MatrixField::Shape));
        apply(&Event::MatrixEditEv { offset: 0, addr, value: Shape::Exp as u8 }, &mut e);
        assert_eq!(e.matrix_mut(Layer::L1).slots[2].shape, Shape::Exp);
    }

    /// The on/off switch rides the same address wire as every other topology
    /// field — `value` is simply 0 or 1.
    #[test]
    fn an_enabled_edit_toggles_the_slot_without_touching_its_wiring() {
        let mut e = engine();
        let addr = pack_matrix_addr(0, 0, matrix_field_code(MatrixField::Enabled));
        let wired = e.matrix_mut(Layer::L1).slots[0];
        assert!(wired.is_active(), "the default patch's slot 0 is a live route");

        apply(&Event::MatrixEditEv { offset: 0, addr, value: 0 }, &mut e);
        let off = e.matrix_mut(Layer::L1).slots[0];
        assert!(!off.is_active(), "switched off");
        assert!(off.is_wired(), "but still wired");
        assert_eq!((off.source, off.dest), (wired.source, wired.dest));

        apply(&Event::MatrixEditEv { offset: 0, addr, value: 1 }, &mut e);
        assert_eq!(e.matrix_mut(Layer::L1).slots[0], wired, "lossless round trip");
    }

    /// The three key-ops fold through `KeyState`, so the mode→toggles mapping is
    /// the engine's, not a second copy living on this wire.
    #[test]
    fn key_ops_fold_through_key_state() {
        let mut e = engine();

        apply(&Event::KeyMode { offset: 0, mode: 2 }, &mut e);
        let k = e.key_state();
        assert!(k.layer2_on && k.split_enabled, "mode 2 == split");

        apply(&Event::SplitPoint { offset: 0, note: 48 }, &mut e);
        assert_eq!(e.key_state().split_point, 48);

        apply(&Event::Lfo2Link { offset: 0, on: true }, &mut e);
        assert!(e.key_state().lfo2_link);

        // Dropping to Single keeps the split point — the plugin's behaviour, so
        // re-enabling split restores where the player left it.
        apply(&Event::KeyMode { offset: 0, mode: 0 }, &mut e);
        let k = e.key_state();
        assert!(!k.layer2_on, "mode 0 == single");
        assert_eq!(k.split_point, 48, "split point survives a mode change");
    }

    #[test]
    fn a_scope_tap_points_the_capture_ring() {
        let mut e = engine();
        apply(&Event::ScopeTapEv { offset: 0, tap: 2 }, &mut e);
        assert_eq!(e.scope().source(), vxn1b_engine::ScopeTap::Layer2.code());
        apply(&Event::ScopeTapEv { offset: 0, tap: 0 }, &mut e);
        assert_eq!(e.scope().source(), vxn1b_engine::ScopeTap::Off.code());
    }

    #[test]
    fn a_normalised_param_is_converted_to_plain_before_it_reaches_the_engine() {
        let mut e = engine();
        let id = clap_id_of(Layer::L1, ParamId::Cutoff);
        let desc = desc_for_clap_id(id).expect("cutoff has a descriptor");
        apply(&Event::SetParamNorm { offset: 0, id: id as u16, norm: 0.25 }, &mut e);
        let want = desc.from_normalized(0.25);
        assert!((e.param(id) - want).abs() < 1e-4, "got {}, want {want}", e.param(id));
    }

    #[test]
    fn an_unknown_param_id_is_dropped_rather_than_panicking() {
        let mut e = engine();
        apply(&Event::SetParamNorm { offset: 0, id: u16::MAX, norm: 1.0 }, &mut e);
        apply(&Event::SetParam { offset: 0, id: u16::MAX, plain: 1.0 }, &mut e);
    }

    #[test]
    fn gestures_never_reach_the_engine() {
        let mut e = engine();
        let id = clap_id_of(Layer::L1, ParamId::Cutoff);
        let before = e.param(id);
        apply(&Event::GestureBegin { offset: 0, id: id as u16 }, &mut e);
        apply(&Event::GestureEnd { offset: 0, id: id as u16 }, &mut e);
        assert_eq!(e.param(id), before);
    }
}
