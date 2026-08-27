//! Re-export of `vxn_core_dsp::hpf`.
//!
//! The scalar kernel moved to `vxn-core-dsp` in ticket 0227 — vxn-1 held a
//! body-identical copy as its `PolyHpf` test oracle. Kept as a shim so in-crate
//! `crate::hpf::…` paths still resolve.
pub use vxn_core_dsp::hpf::HpfKernel;
