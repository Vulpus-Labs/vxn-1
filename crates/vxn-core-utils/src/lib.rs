//! Shared low-level utilities for VXN synth plugins.
//!
//! Trivially-shared surface lifted out of vxn-1 + vxn-2: a denormal/FTZ
//! guard, a one-pole parameter smoother, MIDI note → Hz, and the
//! host-tempo subdivision table. Zero external dependencies; uses `std`
//! (matches both consumer synths — strict `no_std` would force `libm`
//! and bring no real win at this layer).

pub mod ftz;
pub mod midi;
pub mod smoothing;
pub mod sync;

pub use ftz::ScopedFlushToZero;
pub use midi::{MIDI_0_HZ, note_to_hz};
pub use smoothing::{Smoothed, ms_to_samples, one_pole_coeff};
pub use sync::{
    DEFAULT_TEMPO_BPM, SUBDIVISIONS, Subdivision, index_from_norm, subdivision_hz,
    subdivision_label, subdivision_seconds,
};
