//! Re-export of `vxn_core_utils::halfband`.
//!
//! Both halves of the 2× halfband pair live in `vxn-core-utils` as of ticket
//! 0224 — the decimator (`HalfbandFir`, `Oversampler`) and the interpolator
//! (`HalfbandInterp`, `Interpolator`), sharing one `DEFAULT_TAPS` /
//! `DEFAULT_CENTRE` table. The interpolating half lived here only because vxn-2
//! was the sole upsampler.
//!
//! Kept as a shim so in-crate `vxn2_dsp::halfband::…` paths still resolve.
pub use vxn_core_utils::halfband::{
    DEFAULT_CENTRE, DEFAULT_TAPS, HalfbandFir, HalfbandInterp, Interpolator, Oversampler,
    roundtrip_latency_base_samples,
};
