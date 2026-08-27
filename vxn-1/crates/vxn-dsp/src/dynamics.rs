//! Re-export of `vxn_core_dsp::dynamics`.
//!
//! The kernel moved to `vxn-core-dsp` in ticket 0227. vxn-1's copy was the
//! superset — 0241 added the gain-reduction metering tap here and not to
//! vxn-2's fork — so it is vxn-1's version that moved, unchanged.
//!
//! Kept as a shim so in-crate `crate::dynamics::…` paths still resolve.
pub use vxn_core_dsp::dynamics::{DynamicsBlock, DynamicsParams};
