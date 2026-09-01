//! Master-bus limiter — re-export of the shared implementation, plus the shared
//! bypass wrapper.
//!
//! `StereoLimiter` comes from `vxn-core-utils::limiter`: it is a leaf util, with
//! no enable, no params snapshot and no lifecycle beyond `reset`. Ticket 0232
//! gave it an `FxKernel` impl in `vxn-core-dsp` so `Bypassable` can hold one,
//! which is where this engine's `limiter_fade` / `limiter_on` /
//! `limiter_primed` trio went.
pub use vxn_core_dsp::fx::{Bypassable, FxKernel};
pub use vxn_core_dsp::limiter::LimiterParams;
pub use vxn_core_utils::limiter::StereoLimiter;
