//! Parameter smoothing — re-export of the shared one-pole smoother.
//!
//! The definition was lifted from VXN1 and carried here as a hand-maintained
//! fork; E027/0117 collapsed it back to the single copy in
//! `vxn-core-utils::smoothing`. This module stays as a thin re-export so the
//! mod-matrix and FX blocks keep their `vxn2_dsp::smoother::…` /
//! `crate::smoother::…` paths.
pub use vxn_core_utils::smoothing::{Smoothed, ms_to_samples, one_pole_coeff};
