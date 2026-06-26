//! Halfband decimator / oversampler — re-export of the shared implementation.
//!
//! `HalfbandFir`/`Oversampler` (plus the default tap table and the round-trip
//! latency helper) were byte-identical across both synths; E027/0118 promoted
//! them to `vxn-core-utils::halfband`. VXN1 needs only the *decimation* half
//! (the voice path runs directly at the oversampled rate), so there's no
//! interpolation stage here. Re-exported so `vxn_dsp::halfband::…` and the lib
//! re-export keep resolving.
pub use vxn_core_utils::halfband::{
    DEFAULT_CENTRE, DEFAULT_TAPS, HalfbandFir, Oversampler, roundtrip_latency_base_samples,
};
