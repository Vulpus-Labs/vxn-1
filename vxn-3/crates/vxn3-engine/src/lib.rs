//! VXN3 engine — the audio-thread synth state the CLAP shell drives.
//!
//! At the 0046 skeleton stage this is an empty vessel: it owns the host
//! transport clock and renders silence, allocation-free. The track model,
//! `Engine` voicing trait, and the three voice engines (`Kick/Tone`, `Metal`,
//! `Noise`) land in 0047 / 0049 (epic E021).

pub mod engine;
pub mod transport;

pub use engine::Engine;
pub use transport::Transport;
