//! Parameter store traits.
//!
//! The generic surface ([`ParamModel`]) lives in `vxn-core-app`. VXN1
//! adds [`Vxn1Params`] for the non-automatable shared state that
//! travels in the state blob rather than the CLAP param table —
//! key mode + split point.

use crate::domain::KeyMode;

pub use vxn_core_app::{ParamId, ParamModel};

/// VXN1-specific extension trait: non-automatable shared state (key
/// mode + split point) and the discrete-edit `set_key_mode_seeded`
/// path that performs Whole → non-Whole one-shot Upper → Lower copy.
pub trait Vxn1Params: ParamModel {
    fn key_mode(&self) -> KeyMode;
    fn set_key_mode(&self, mode: KeyMode);
    /// Set the key mode from a **discrete UI edit**, performing any
    /// one-shot seed-on-entry copy (e.g. Whole → non-Whole seeds
    /// Lower from Upper). Distinct from [`Self::set_key_mode`] which
    /// is used by state load (no seeding).
    fn set_key_mode_seeded(&self, mode: KeyMode);

    fn split_point(&self) -> u8;
    fn set_split_point(&self, note: u8);
}
