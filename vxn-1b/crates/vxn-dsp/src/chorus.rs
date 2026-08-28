//! Re-export of `vxn_core_dsp::chorus`.
//!
//! Ticket 0229 moved this kernel to `vxn-core-dsp` and resolved its split
//! personality. It had two non-equivalent entry points: a true-stereo block
//! path, and a per-sample `process` that **mono-summed** its input before
//! feeding both BBD lines. vxn-1b's `FxChain` called the per-sample one, so
//! vxn-1b heard the mono-sum voicing. The mono sum is gone — each delay line
//! now takes its own channel — which widens the chorus on stereo material.
//!
//! Bypass also moved inside: the kernel carries its own 30 ms `WetFade` where
//! `FxChain` used to wrap the slot in a 10 ms linear crossfade.
pub use vxn_core_dsp::chorus::{ChorusParams, StereoChorus};
pub use vxn_core_dsp::fx::FxKernel;
