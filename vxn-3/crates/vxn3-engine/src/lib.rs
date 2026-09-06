//! VXN3 engine — the audio-thread synth state the CLAP shell drives.
//!
//! 0047 makes it audible: eight heterogeneous [`track::Track`]s, each holding one
//! active [`track_engine::TrackEngine`] over a per-track SoA voice block
//! (ADR 0001 §4/§5), driven by a hit-list [`sequencer`] over [`grid`] geometry off
//! the host [`transport`] clock and summed to stereo by [`engine::Engine`].
//! The first engine is [`engines::KickTone`] (poly); `Metal` / `Noise` land in
//! 0049. Engines hot-swap off-thread via [`swap::EngineSwap`].

pub mod engine;
pub mod engines;
pub mod flavour;
pub mod grid;
pub mod io;
pub mod lane;
pub mod patch;
pub mod sequencer;
pub mod swap;
pub mod track;
pub mod track_engine;
pub mod transport;

pub use engine::{Engine, LIMITER_LOOKAHEAD, N_TRACKS};
pub use engines::{
    KickTone, KickTonePatch, Metal, MetalPatch, Noise, NoisePatch, Struck, StruckPatch,
    default_flavour_for, flavours_for, make, params_for,
};
pub use flavour::{Binding, Curve, Flavour, ParamMeta, flavour_macro_display, resolve};
pub use grid::{
    Grid, GridPos, MAX_BEATS, MAX_MARKERS, MAX_SUBS, MIN_SLOT, Swing, SwingPeriod, SwingShape,
};
pub use io::{EditQueue, EngineCommand, EngineIo, PlayheadState, TrackKinds};
pub use lane::{LaneState, TrigEvent};
pub use sequencer::{
    EIGHTH, EIGHTH_TRIPLET, Hit, Lock, LockParam, MAX_HITS, MAX_NUDGE_TICKS, N_LOCK_PARAMS,
    Pattern, Retrig, RetrigCurve, SIXTEENTH, TICKS_PER_BEAT, Termination,
};
pub use swap::EngineSwap;
pub use track::Track;
pub use track_engine::{
    EngineKind, LANES, MACRO_SLOTS, MacroReadout, MacroUnit, TrackEngine, macro_display, macro_map,
    macro_parse,
};
pub use transport::Transport;
