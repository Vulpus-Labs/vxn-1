//! Halfband decimator / oversampler — re-export of the shared implementation.
//!
//! `HalfbandFir`/`Oversampler` (plus the default tap table and the round-trip
//! latency helper) are re-exported from `vxn-core-utils::halfband`. VXN1 needs
//! only the *decimation* half (the voice path runs directly at the oversampled
//! rate), so there's no interpolation stage here.
pub use vxn_core_utils::halfband::{
    DEFAULT_CENTRE, DEFAULT_TAPS, HalfbandFir, Oversampler, roundtrip_latency_base_samples,
};
