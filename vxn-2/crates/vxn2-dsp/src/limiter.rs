//! Master-bus limiter — re-export of the shared implementation.
//!
//! VXN2's copy was byte-identical to VXN1's; E027/0118 promoted the limiter
//! (`PeakWindow`/`LimiterCore`/`StereoLimiter` + its private `DelayLine`) to
//! `vxn-core-utils::limiter`. Re-exported here so `vxn2_dsp::limiter::…` paths
//! keep resolving.
pub use vxn_core_utils::limiter::StereoLimiter;
