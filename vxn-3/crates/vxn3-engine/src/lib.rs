//! VXN3 engine — the audio-thread synth state the CLAP shell drives.
//!
//! 0047 makes it audible: eight heterogeneous [`track::Track`]s, each holding one
//! active [`track_engine::TrackEngine`] over a per-track SoA voice block
//! (ADR 0001 §4/§5), driven by a step [`sequencer`] off the host
//! [`transport`] clock and summed to stereo by the instrument [`engine::Engine`].
//! The first engine is [`engines::KickTone`] (poly); `Metal` / `Noise` land in
//! 0049. Engines hot-swap off-thread via [`swap::EngineSwap`].

pub mod engine;
pub mod engines;
pub mod sequencer;
pub mod swap;
pub mod track;
pub mod track_engine;
pub mod transport;

pub use engine::{Engine, N_TRACKS};
pub use engines::{KickTone, KickTonePatch};
pub use sequencer::{MAX_STEPS, Pattern, STEP_BEATS, STEPS_PER_BEAT, Step};
pub use swap::EngineSwap;
pub use track::Track;
pub use track_engine::{EngineKind, LANES, TrackEngine};
pub use transport::Transport;
