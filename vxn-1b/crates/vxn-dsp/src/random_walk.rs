//! Re-export of `vxn_core_utils::random_walk`.
//!
//! Moved by ticket 0229: `vxn-core-dsp`'s BBD delay line needs it, and a
//! component crate cannot depend on a synth crate. `poly::oscillator` still
//! reaches it through this path.
pub use vxn_core_utils::random_walk::{BoundedRandomWalk, OSCILLATOR_DRIFT_STEP};
