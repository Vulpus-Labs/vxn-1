//! VXN3 DSP kernels — framework-free synthesis primitives.
//!
//! Lane-parallel SoA kernels live in the engine layer (`vxn3-engine`); this
//! crate carries the shared math they build on: the Q32 phase oscillator
//! ([`sine`]) and the branchless drum-envelope coefficients ([`env`]). The
//! `Metal` (modal resonator) and `Noise` primitives land in 0049.

pub mod env;
pub mod sine;

pub use env::{SILENCE_EPS, attack_coef, decay_coef};
pub use sine::{PHASE_SCALE, fast_sine_01, fast_sine_q32, note_to_freq, phase_inc_hz};
