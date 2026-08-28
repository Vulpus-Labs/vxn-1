//! Re-export of `vxn_core_dsp::delay_line`.
//!
//! Moved by ticket 0229, following its only consumer — the chorus — into
//! `vxn-core-dsp`.
pub use vxn_core_dsp::delay_line::{Complex32, Interp, ModDelayLine, OnePoleLpf};
