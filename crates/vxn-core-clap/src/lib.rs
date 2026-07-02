//! Shared clack helpers for VXN synth plugins.
//!
//! Each synth keeps its own `Plugin` impl (the type-system gymnastics of
//! generalising `clack_plugin::Plugin` over a synth-specific engine
//! outweigh the duplication payoff at two synths). This crate carries
//! the bits that *are* synth-agnostic given a small engine-trait
//! surface: CLAP event dispatch, transport tempo extraction, state I/O,
//! gesture-bracket emit, and a generic `LocalParams<N>` audio-thread
//! mirror.
//!
//! Re-exports `vxn_core_app::ParamModel` so the state helpers compose
//! with the controller surface.
//!
//! ## Feature flags
//!
//! - `test-support` (off by default): exposes [`testing`], a module of
//!   synth-agnostic helpers for the CLAP event-buffer ritual
//!   (`push_param_event`, `event_log`). Not compiled into release
//!   builds; crates add it under `[dev-dependencies]` only.

pub mod engine;
pub mod events;
pub mod gesture;
pub mod local;
pub mod state;
pub mod transport;

#[cfg(feature = "test-support")]
pub mod testing;

pub use engine::{EngineNotes, EngineProcess, SharedStore};
pub use events::{batch_range, dispatch_event, dispatch_notes};
pub use gesture::{emit_gesture_begin, emit_gesture_end, emit_param_value};
pub use local::{LocalParams, bracket};
pub use state::{load_blob, save_blob};
pub use transport::{playing_from_transport, tempo_from_transport};
pub use vxn_core_app::ParamModel;
