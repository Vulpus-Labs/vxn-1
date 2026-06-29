//! Master-bus limiter — re-export of the shared implementation.
//!
//! `PeakWindow`/`LimiterCore`/`StereoLimiter` (and their private `DelayLine`)
//! were byte-identical in vxn-1 and vxn-2; E027/0118 promoted them to
//! `vxn-core-utils::limiter`. This module re-exports `StereoLimiter` so
//! `vxn_dsp::limiter::StereoLimiter` and the lib re-export keep resolving.
pub use vxn_core_utils::limiter::StereoLimiter;
