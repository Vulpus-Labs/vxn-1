//! CLAP event dispatch onto the engine traits.
//!
//! Pure functions over the engine traits — no state. The synth's
//! `process()` walks `events.input.batch()` and calls these once per
//! event before rendering each batch segment.

use std::ops::Bound;

use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, UnknownEvent};

use crate::engine::EngineNotes;

/// Convert a clack event-batch `[start, end)` sample range into concrete
/// frame offsets, capped to the host's frame count. `Unbounded` means
/// "from start" / "to end" of the host block.
pub fn batch_range(bounds: (Bound<usize>, Bound<usize>), frames: usize) -> (usize, usize) {
    let (sb, eb) = bounds;
    let start = match sb {
        Bound::Included(n) => n,
        Bound::Excluded(n) => n + 1,
        Bound::Unbounded => 0,
    }
    .min(frames);
    let end = match eb {
        Bound::Included(n) => n + 1,
        Bound::Excluded(n) => n,
        Bound::Unbounded => frames,
    }
    .min(frames);
    (start, end)
}

/// Dispatch the note + raw-MIDI arms onto `engine`. The param-write seam
/// differs per synth (callback vs. shared-store write-through), so it lives
/// in [`dispatch_event`] / each synth's own wrapper; this carries only the
/// arms that are identical across synths.
///
/// - NoteOn / NoteOff: forwarded to `engine`. CLAP velocity is `[0, 1]`
///   float; we forward it as-is (the engine decides the mapping).
/// - Raw MIDI: note on/off (0x90/0x80), pitch-bend (0xE0), CC1 mod wheel
///   (0xB0), CC64 sustain pedal (0xB0), channel aftertouch (0xD0) forwarded.
///   Note on/off matter for raw-MIDI hosts (the standalone) that never send
///   the typed CLAP note events the NoteOn/NoteOff arms above expect.
/// - `ParamValue` and anything else: silently ignored.
pub fn dispatch_notes<E: EngineNotes>(engine: &mut E, event: &UnknownEvent) {
    match event.as_core_event() {
        Some(CoreEventSpace::NoteOn(e)) => {
            if let Match::Specific(key) = e.key() {
                engine.note_on(key as u8, e.velocity() as f32);
            }
        }
        Some(CoreEventSpace::NoteOff(e)) => {
            if let Match::Specific(key) = e.key() {
                engine.note_off(key as u8);
            }
        }
        Some(CoreEventSpace::Midi(e)) => {
            let [status, d1, d2] = e.data();
            match status & 0xF0 {
                // Note on/off as raw MIDI. Hosts that speak the CLAP note
                // dialect deliver typed NoteOn/NoteOff (handled above), but a
                // raw-MIDI host (e.g. the clap-wrapper standalone) sends these
                // as channel-voice bytes. A 0x90 with velocity 0 is note-off
                // by MIDI convention.
                0x90 if d2 > 0 => {
                    engine.note_on(d1, d2 as f32 / 127.0);
                }
                0x80 | 0x90 => {
                    engine.note_off(d1);
                }
                0xE0 => {
                    // 14-bit bend, centre 8192 → normalised [-1, 1].
                    let raw = ((d2 as u16) << 7) | d1 as u16;
                    engine.pitch_bend((raw as f32 - 8192.0) / 8192.0);
                }
                0xB0 if d1 == 1 => {
                    // CC1 mod wheel. Deadzone the bottom LSB — hardware
                    // wheels rarely rest clean at 0.
                    let wheel = if d2 <= 1 { 0.0 } else { d2 as f32 / 127.0 };
                    engine.mod_wheel(wheel);
                }
                0xD0 => {
                    // Channel aftertouch: single data byte in [0, 127].
                    engine.aftertouch(d1 as f32 / 127.0);
                }
                0xB0 if d1 == 64 => {
                    // CC64 sustain (damper) pedal. MIDI convention: >= 64 on.
                    engine.sustain(d2 >= 64);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Per-event dispatch. `ParamValue` events route to `on_param` (the synth
/// folds them into its audio-thread mirror); every other arm delegates to
/// [`dispatch_notes`].
pub fn dispatch_event<E, F>(engine: &mut E, on_param: &mut F, event: &UnknownEvent)
where
    E: EngineNotes,
    F: FnMut(&UnknownEvent),
{
    if let Some(CoreEventSpace::ParamValue(_)) = event.as_core_event() {
        on_param(event);
    } else {
        dispatch_notes(engine, event);
    }
}
