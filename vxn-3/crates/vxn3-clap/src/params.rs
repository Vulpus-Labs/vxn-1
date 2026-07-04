//! VXN3 fixed host-parameter table (ADR 0003 §1, ticket 0171).
//!
//! A **fixed, deterministic** `clap.params` layout — the same `clap_id`s every
//! session, **never rescanned** on an engine swap. Contents are only the
//! engine-*independent* surface: a master block, then per-track mix + a fixed
//! budget of generic **macro slots** the active engine reinterprets (ADR 0003
//! §2; the mapping lives in [`vxn3_engine::macro_map`]).
//!
//! `clap_id == param_index` (dense, contiguous) so the host enumerates by index
//! and looks up by id through the one positional [`decode`] scheme. Ids are
//! computed, not accreted — stable across sessions without an append-only rule.
//!
//! The engine-aware macro value-text is deliberately *not* here — 0171 renders a
//! generic readout; 0172 upgrades macros to [`vxn3_engine::macro_display`].

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use vxn3_engine::{EngineCommand, MACRO_SLOTS, N_TRACKS};

/// Master params, in id order: master volume, delay feedback, delay time
/// (tempo-synced beats), delay return.
pub const N_MASTER: usize = 4;
/// Params per track: `level`, `pan`, `mute`, `send`, then `MACRO_SLOTS` macros.
pub const PER_TRACK: usize = 4 + MACRO_SLOTS;
/// Total fixed host params (`N_MASTER + N_TRACKS * PER_TRACK`).
pub const TOTAL_PARAMS: usize = N_MASTER + N_TRACKS * PER_TRACK;

/// The decoded meaning of a `clap_id`. Track-scoped variants carry the track;
/// `Macro` also carries the slot index (`0..MACRO_SLOTS`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    MasterVolume,
    DelayFeedback,
    DelayTime,
    DelayReturn,
    Level(u8),
    Pan(u8),
    Mute(u8),
    Send(u8),
    Macro(u8, u8),
}

/// Decode a `clap_id` to its [`Slot`], or `None` if out of range. The single
/// positional scheme every other function in this module reads.
pub fn decode(id: usize) -> Option<Slot> {
    if id < N_MASTER {
        return Some(match id {
            0 => Slot::MasterVolume,
            1 => Slot::DelayFeedback,
            2 => Slot::DelayTime,
            _ => Slot::DelayReturn,
        });
    }
    let q = id - N_MASTER;
    let t = (q / PER_TRACK) as u8;
    if t as usize >= N_TRACKS {
        return None;
    }
    let s = q % PER_TRACK;
    Some(match s {
        0 => Slot::Level(t),
        1 => Slot::Pan(t),
        2 => Slot::Mute(t),
        3 => Slot::Send(t),
        _ => Slot::Macro(t, (s - 4) as u8),
    })
}

/// A slot's `(min, max, default, stepped)` for `param_info`.
pub fn range(slot: Slot) -> (f32, f32, f32, bool) {
    match slot {
        Slot::MasterVolume => (0.0, 1.5, 1.0, false),
        Slot::DelayFeedback => (0.0, 1.25, 0.5, false),
        Slot::DelayTime => (0.0625, 1.5, 0.75, false),
        Slot::DelayReturn => (0.0, 1.0, 0.35, false),
        Slot::Level(_) => (0.0, 1.5, 1.0, false),
        Slot::Pan(_) => (-1.0, 1.0, 0.0, false),
        Slot::Mute(_) => (0.0, 1.0, 0.0, true),
        Slot::Send(_) => (0.0, 1.0, 0.0, false),
        Slot::Macro(..) => (0.0, 1.0, 0.5, false),
    }
}

/// The default value of a param id (used to seed the value cache so
/// `get_value` matches the engine's fresh state).
pub fn default_value(id: usize) -> f32 {
    decode(id).map(|s| range(s).2).unwrap_or(0.0)
}

/// Write the host-facing name for a slot (generic + fixed — a macro is
/// `T3 · M1`, engine-independent; ADR 0003 §2).
pub fn write_name(slot: Slot, out: &mut String) {
    let _ = match slot {
        Slot::MasterVolume => out.write_str("Master Volume"),
        Slot::DelayFeedback => out.write_str("Delay Feedback"),
        Slot::DelayTime => out.write_str("Delay Time"),
        Slot::DelayReturn => out.write_str("Delay Return"),
        Slot::Level(t) => write!(out, "T{} · Level", t + 1),
        Slot::Pan(t) => write!(out, "T{} · Pan", t + 1),
        Slot::Mute(t) => write!(out, "T{} · Mute", t + 1),
        Slot::Send(t) => write!(out, "T{} · Send", t + 1),
        Slot::Macro(t, m) => write!(out, "T{} · M{}", t + 1, m + 1),
    };
}

/// The module (group path) a slot belongs to — `Master` or `Track N`.
pub fn write_module(slot: Slot, out: &mut String) {
    let _ = match slot {
        Slot::MasterVolume | Slot::DelayFeedback | Slot::DelayTime | Slot::DelayReturn => {
            out.write_str("Master")
        }
        Slot::Level(t)
        | Slot::Pan(t)
        | Slot::Mute(t)
        | Slot::Send(t)
        | Slot::Macro(t, _) => write!(out, "Track {}", t + 1),
    };
}

/// Translate a host param write (`id`, normalized/engine value) to the engine
/// [`EngineCommand`] that applies it. `None` for an unknown id.
pub fn to_command(id: usize, value: f32) -> Option<EngineCommand> {
    Some(match decode(id)? {
        Slot::MasterVolume => EngineCommand::SetMasterVolume { value },
        Slot::DelayFeedback => EngineCommand::SetDelayFeedback { value },
        Slot::DelayTime => EngineCommand::SetDelaySyncBeats { beats: value },
        Slot::DelayReturn => EngineCommand::SetDelayReturn { value },
        Slot::Level(t) => EngineCommand::SetGain { track: t, gain: value },
        Slot::Pan(t) => EngineCommand::SetPan { track: t, pan: value },
        Slot::Mute(t) => EngineCommand::SetMute { track: t, muted: value >= 0.5 },
        Slot::Send(t) => EngineCommand::SetSend { track: t, amount: value },
        Slot::Macro(t, m) => EngineCommand::SetMacro { track: t, slot: m, value },
    })
}

/// Render a generic (non-engine-aware) readout for a param value (ticket 0171).
/// Macro slots get an engine-aware readout in 0172; here they show the raw
/// normalized value.
pub fn write_value_text(slot: Slot, value: f32, out: &mut String) {
    let _ = match slot {
        Slot::MasterVolume | Slot::Level(_) => write_db(value, out),
        Slot::Pan(_) => write_pan(value, out),
        Slot::Mute(_) => out.write_str(if value >= 0.5 { "on" } else { "off" }),
        Slot::DelayFeedback | Slot::DelayReturn | Slot::Send(_) => {
            write!(out, "{:.0}%", value * 100.0)
        }
        Slot::DelayTime => write!(out, "{value:.3} beats"),
        Slot::Macro(..) => write!(out, "{value:.2}"),
    };
}

/// Invert [`write_value_text`] for host text edits: parse a slot's display string
/// back to a parameter value. Slot-aware so it round-trips the unit transforms
/// (dB, pan, %) — `value → text → value → text` is stable, which `clap-validator`
/// requires. `None` if the text has no recognisable value.
pub fn parse_value(slot: Slot, text: &str) -> Option<f32> {
    let t = text.trim();
    match slot {
        Slot::MasterVolume | Slot::Level(_) => {
            if t.starts_with("-inf") {
                return Some(0.0);
            }
            let db = leading_number(t)?;
            Some(10f32.powf(db / 20.0))
        }
        Slot::Pan(_) => {
            let head = t.chars().next()?;
            match head {
                'C' | 'c' => Some(0.0),
                'L' | 'l' => Some(-leading_number(&t[1..])? / 100.0),
                'R' | 'r' => Some(leading_number(&t[1..])? / 100.0),
                _ => leading_number(t),
            }
        }
        Slot::Mute(_) => Some(if t.starts_with("on") { 1.0 } else { 0.0 }),
        Slot::DelayFeedback | Slot::DelayReturn | Slot::Send(_) => Some(leading_number(t)? / 100.0),
        Slot::DelayTime | Slot::Macro(..) => leading_number(t),
    }
}

/// The leading numeric token of `s` (sign / digits / dot), ignoring any unit
/// suffix the host echoes back through a text edit.
fn leading_number(s: &str) -> Option<f32> {
    let num: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse::<f32>().ok()
}

fn write_db(lin: f32, out: &mut String) -> std::fmt::Result {
    if lin <= 1e-4 {
        out.write_str("-inf dB")
    } else {
        write!(out, "{:.1} dB", 20.0 * lin.log10())
    }
}

fn write_pan(pan: f32, out: &mut String) -> std::fmt::Result {
    let p = pan.clamp(-1.0, 1.0);
    if p.abs() < 0.005 {
        out.write_str("C")
    } else if p < 0.0 {
        write!(out, "L{:.0}", -p * 100.0)
    } else {
        write!(out, "R{:.0}", p * 100.0)
    }
}

/// Shared, thread-safe cache of the current host-facing param values. The main
/// thread reads it for `get_value`; the audio thread writes it as host
/// automation lands (and 0173's echo pump will read/dirty it). Seeded to each
/// param's default so a fresh instance reports the engine's real starting state.
pub struct ParamCache {
    vals: [AtomicU32; TOTAL_PARAMS],
}

impl ParamCache {
    pub fn new() -> Self {
        Self {
            vals: std::array::from_fn(|i| AtomicU32::new(default_value(i).to_bits())),
        }
    }

    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        f32::from_bits(self.vals[id].load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set(&self, id: usize, v: f32) {
        if id < TOTAL_PARAMS {
            self.vals[id].store(v.to_bits(), Ordering::Relaxed);
        }
    }
}

impl Default for ParamCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_matches_layout() {
        assert_eq!(TOTAL_PARAMS, 4 + 8 * 7);
    }

    #[test]
    fn decode_round_trips_every_id() {
        // Every id in range decodes; nothing past the table does.
        for id in 0..TOTAL_PARAMS {
            assert!(decode(id).is_some(), "id {id} should decode");
        }
        assert_eq!(decode(TOTAL_PARAMS), None);
    }

    #[test]
    fn positional_scheme_is_stable() {
        // Spot-check the fixed layout: track 0 mix block starts right after the
        // 4 master params; macros are the last 3 of each 7-wide track block.
        assert_eq!(decode(0), Some(Slot::MasterVolume));
        assert_eq!(decode(N_MASTER), Some(Slot::Level(0)));
        assert_eq!(decode(N_MASTER + 2), Some(Slot::Mute(0)));
        assert_eq!(decode(N_MASTER + 4), Some(Slot::Macro(0, 0)));
        assert_eq!(decode(N_MASTER + PER_TRACK), Some(Slot::Level(1)));
        assert_eq!(decode(TOTAL_PARAMS - 1), Some(Slot::Macro(7, 2)));
    }

    #[test]
    fn commands_map_by_slot() {
        let macro_id = N_MASTER + 3 * PER_TRACK + 5; // track 3, macro slot 1
        assert_eq!(
            to_command(macro_id, 0.7),
            Some(EngineCommand::SetMacro { track: 3, slot: 1, value: 0.7 })
        );
        assert_eq!(
            to_command(N_MASTER + 2, 1.0), // track 0 mute
            Some(EngineCommand::SetMute { track: 0, muted: true })
        );
        assert_eq!(to_command(TOTAL_PARAMS, 0.0), None);
    }

    #[test]
    fn cache_seeds_defaults() {
        let c = ParamCache::new();
        assert_eq!(c.get(0), 1.0); // master volume default
        assert_eq!(c.get(N_MASTER + 4), 0.5); // a macro default
        c.set(0, 0.25);
        assert_eq!(c.get(0), 0.25);
    }

    #[test]
    fn value_text_round_trips() {
        // value → text → value → text must be stable (clap-validator param-conversions).
        let cases = [
            (Slot::MasterVolume, 1.5),
            (Slot::MasterVolume, 0.0),
            (Slot::Level(0), 0.8),
            (Slot::Pan(0), -0.234),
            (Slot::Pan(0), 0.3),
            (Slot::Pan(0), 0.0),
            (Slot::Mute(0), 1.0),
            (Slot::Send(0), 0.42),
            (Slot::DelayTime, 0.75),
            (Slot::Macro(0, 0), 0.5),
        ];
        for (slot, v) in cases {
            let mut s1 = String::new();
            write_value_text(slot, v, &mut s1);
            let v2 = parse_value(slot, &s1).unwrap_or_else(|| panic!("parse {slot:?} '{s1}'"));
            let mut s2 = String::new();
            write_value_text(slot, v2, &mut s2);
            assert_eq!(s1, s2, "{slot:?} v={v} unstable: '{s1}' vs '{s2}'");
        }
    }

    #[test]
    fn value_text_is_sensible() {
        let mut s = String::new();
        write_value_text(Slot::Pan(0), 0.0, &mut s);
        assert_eq!(s, "C");
        s.clear();
        write_value_text(Slot::Mute(0), 1.0, &mut s);
        assert_eq!(s, "on");
    }
}
