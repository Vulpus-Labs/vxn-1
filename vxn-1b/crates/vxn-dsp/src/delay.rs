//! Re-export of `vxn_core_dsp::delay`.
//!
//! Ticket 0231 (epic E041) unified this kernel with vxn-2's, which was the
//! superset — cubic read tap, tempo sync, DC-blocked feedback, ping-pong flag,
//! in-kernel wet fade — and took vxn-1b's feedback **damping** control across
//! with it. Adopting the superset changes vxn-1b's delay in five known ways:
//!
//! - **Cubic read.** Linear interpolation on the fractional tap became
//!   Catmull-Rom: less HF loss on the repeats, and no interpolation ripple as
//!   the tap sweeps.
//! - **Time glide.** The 40 ms one-pole slew is a 100 ms `Smoothed` glide, so a
//!   `DelayTime` move bends further and slower. The ramp still lives *in the
//!   kernel* — the engine hands it a stepped target per control block, as
//!   before.
//! - **DC blocker.** A ~10 Hz highpass now sits in the feedback path. vxn-1b had
//!   none; long feedback tails no longer accumulate an offset.
//! - **Feedback ceiling.** Clamped at 0.95 rather than 0.99 — the shared
//!   kernel's cap. Maximum regeneration decays a little sooner.
//! - **Ping-pong is a full crossfeed.** vxn-1b crossed only the *feedback*, so
//!   dry L always entered the L line; vxn-2's ping-pong crosses the input too,
//!   which is what makes repeats alternate sides from the first one. `PingPong
//!   = off` is unchanged.
//!
//! Bypass moved inside as well: the 10 ms outer slot fade `FxChain` wrapped this
//! in is gone, the kernel's own 30 ms `WetFade` owns the on/off glide, and the
//! chain true-skips on `is_active`. The stale-tail clear that was
//! `FxChain::clear_slot` is now the kernel honouring `EdgeAction::RisingClear`.
//!
//! `DelayLine` — the bare linear-interpolated line this module used to export —
//! is gone with it. Nothing outside the old `StereoDelay` used it, the shared
//! kernel has its own ring, and `vxn-core-utils`' limiter keeps the private copy
//! it always had.
pub use vxn_core_dsp::delay::{
    MAX_DELAY_MS, MAX_DELAY_S, MAX_FEEDBACK, MIN_DELAY_MS, StereoDelay, StereoDelayParams,
};
pub use vxn_core_dsp::fx::FxKernel;
