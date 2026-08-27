//! Re-export of `vxn_core_dsp::dynamics`.
//!
//! The kernel moved to `vxn-core-dsp` in ticket 0227. This crate's fork had
//! drifted behind vxn-1's: 0241 added the gain-reduction metering tap
//! (`take_gain_reduction_db`) to vxn-1's copy only, so the shared kernel is that
//! superset. The tap is a side channel — `gr_db_min` is never read back into the
//! output — so vxn-2's audio is bit-identical; it simply gains a reading it can
//! publish whenever a meter wants one.
//!
//! Kept as a shim so in-crate `crate::dynamics::…` paths still resolve.
pub use vxn_core_dsp::dynamics::{DynamicsBlock, DynamicsParams};
