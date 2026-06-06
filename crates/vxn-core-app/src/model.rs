//! Parameter store trait — the surface the controller programs against.
//!
//! `Send + Sync + 'static`: writes flow from the controller, reads from
//! the audio thread; the synth's concrete impl arranges the lock-free
//! crossing (typically an `AtomicU32` per param). Per-synth shared state
//! that doesn't fit the (id, f32) shape (vxn-1's `KeyMode` / split
//! point, vxn-2's mod-matrix table) lives on an extension trait the
//! synth defines.

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

/// The live parameter store. Audio-thread safe (`Send + Sync`); writes
/// from the controller and reads from the audio thread cross here
/// without locks.
///
/// Beyond the param array the trait also surfaces an opaque save/restore
/// byte channel (`snapshot_bytes` / `restore_from_bytes`) so the
/// controller can serve host save/load without knowing the engine's
/// internal format.
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

    /// Serialise the entire store to bytes for host `state.save`.
    fn snapshot_bytes(&self) -> Vec<u8>;

    /// Apply a previously-saved blob.
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String>;
}
