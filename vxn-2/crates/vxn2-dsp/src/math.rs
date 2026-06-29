//! Fast math approximations — re-export of the shared scalar `fast_tanh`.
//!
//! VXN2's only nonlinearity is the per-stage `tanh` of the OTA-C ladder
//! ([`crate::filter`]); VXN2's operators are pure sine. The branched-scalar
//! `fast_tanh` was byte-identical to VXN1's, so E027/0118 folded both into
//! `vxn-core-utils::math`. Re-exported here so `crate::math::fast_tanh`
//! keeps resolving.
pub use vxn_core_utils::math::fast_tanh;
