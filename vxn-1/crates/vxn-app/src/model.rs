//! Generic parameter model the controller programs against (ADR 0007 §4).
//!
//! `vxn-engine`'s `SharedParams` implements [`ParamModel`]. The trait exists so
//! a future VXN-2 plugs in its own atomic store without changes here.

use crate::domain::KeyMode;
use crate::params::ParamDesc;

/// Stable id for a parameter — the CLAP id, newtyped. Indexes the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamId(pub usize);

impl ParamId {
    #[inline]
    pub const fn new(raw: usize) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> usize {
        self.0
    }
}

impl From<usize> for ParamId {
    fn from(raw: usize) -> Self {
        Self(raw)
    }
}

impl From<ParamId> for usize {
    fn from(id: ParamId) -> Self {
        id.0
    }
}

/// The live parameter store. Audio-thread safe (`Send + Sync`); writes from
/// the controller and reads from the audio thread cross here without locks.
///
/// Beyond the param array, the trait also surfaces non-automatable shared
/// state (key mode, split point) and an opaque save/restore byte channel
/// (state blob), so the controller can serve host save/load without knowing
/// the engine's internal format.
pub trait ParamModel: Send + Sync + 'static {
    fn total(&self) -> usize;

    fn get(&self, id: ParamId) -> f32;
    fn set(&self, id: ParamId, plain: f32);

    fn get_normalized(&self, id: ParamId) -> f32;
    fn set_normalized(&self, id: ParamId, norm: f32);

    fn gesture(&self, id: ParamId) -> bool;
    fn set_gesture(&self, id: ParamId, on: bool);

    /// Static descriptor for a param id (range, formatting, fader mapping).
    fn descriptor(&self, id: ParamId) -> Option<&'static ParamDesc>;

    // ── Non-automatable shared state ─────────────────────────────────────────

    fn key_mode(&self) -> KeyMode;
    fn set_key_mode(&self, mode: KeyMode);
    /// Set the key mode from a **discrete UI edit**, performing any one-shot
    /// seed-on-entry copy (e.g. Whole → non-Whole seeds Lower from Upper).
    /// Distinct from [`set_key_mode`] which is used by state load (no seeding).
    fn set_key_mode_seeded(&self, mode: KeyMode);

    fn split_point(&self) -> u8;
    fn set_split_point(&self, note: u8);

    // ── State blob (CLAP state save/load) ────────────────────────────────────

    /// Serialize the entire store to bytes for host `state.save`.
    fn snapshot_bytes(&self) -> Vec<u8>;

    /// Apply a previously-saved blob.
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String>;
}
