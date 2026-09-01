//! Re-export of `vxn_core_utils::limiter`, plus the shared bypass wrapper.
//!
//! `StereoLimiter` is a leaf util — no enable, no params, no lifecycle beyond
//! `reset` — so it stays in `vxn-core-utils`. Ticket 0232 gave it an `FxKernel`
//! impl in `vxn-core-dsp` so `Bypassable` can hold one, which is where this
//! engine's `limiter_was_on` edge and vxn-1b's fade both went.
pub use vxn_core_dsp::fx::{Bypassable, FxKernel};
pub use vxn_core_dsp::limiter::LimiterParams;
pub use vxn_core_utils::limiter::StereoLimiter;
