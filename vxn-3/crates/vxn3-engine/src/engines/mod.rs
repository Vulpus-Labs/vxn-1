//! The engine roster (ADR 0001 §6). `Kick/Tone` (poly) ships in 0047; `Metal`
//! (modal resonator) and `Noise` land in 0049.

pub mod kick_tone;

pub use kick_tone::{KickTone, KickTonePatch};
