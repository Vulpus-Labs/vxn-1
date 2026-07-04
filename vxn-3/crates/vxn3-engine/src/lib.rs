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
pub mod flavour;
pub mod io;
pub mod lane;
pub mod patch;
pub mod sequencer;
pub mod swap;
pub mod track;
pub mod track_engine;
pub mod transport;

pub use engine::{Engine, LIMITER_LOOKAHEAD, N_TRACKS};
pub use engines::{KickTone, KickTonePatch, Metal, MetalPatch, Noise, NoisePatch, make};
pub use flavour::{Binding, Curve, Flavour, ParamMeta, flavour_macro_display, resolve};
pub use io::{EditQueue, EngineCommand, EngineIo, PlayheadState, TrackKinds};
pub use lane::{Hit, LaneState};
pub use sequencer::{
    EIGHTH, EIGHTH_TRIPLET, Lock, LockParam, MAX_STEPS, N_LOCK_PARAMS, Pattern, Retrig,
    RetrigCurve, SIXTEENTH, Step, Termination,
};
pub use swap::EngineSwap;
pub use track::Track;
pub use track_engine::{
    EngineKind, LANES, MACRO_SLOTS, MacroReadout, MacroUnit, TrackEngine, macro_display, macro_map,
    macro_parse,
};
pub use transport::Transport;
