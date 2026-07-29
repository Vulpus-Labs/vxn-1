//! Cross-thread parameter store for the CLAP shell (ticket 0204).
//!
//! CLAP calls the `params` extension (`get_value`, `get_info`, `flush`) on the
//! **main thread** while the [`crate::Engine`] runs on the **audio thread**, so
//! the two need a lock-free view of the current param values plus a channel for
//! the non-automatable matrix topology. This store is that bridge:
//!
//! - **Params** — one `AtomicU32` (f32 bits) per CLAP id. Writes cross without
//!   locks: the audio thread stores host automation as it happens, the main
//!   thread reads it back for `get_value`.
//! - **Matrix topology** — behind a `Mutex`. It changes only on state/preset
//!   load (main thread), never per sample, so a lock the audio thread takes
//!   once when the `reload` flag is set is cheap and RT-safe in practice.
//! - **`reload`** — set by [`Self::restore_from_bytes`]; the audio thread swaps
//!   it to `false` at the top of `process` and re-syncs the engine from the
//!   store. This is how a state load that lands while the plugin is active
//!   reaches the running engine.
//!
//! Depth stays param-authoritative (ADR 0001 §5 / 0205): the stored topology's
//! depths already mirror the depth params (the codec seeds them), so a reload
//! that pushes params-then-topology can't disagree.
//!
//! No `gui`/gesture/echo machinery — this shell targets host-generic knobs
//! (0204); the faceplate + its controller land in E038.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::matrix::MatrixTable;
use crate::params::{PARAMS, TOTAL_PARAMS};
use crate::state::PluginState;

/// Lock-free param mirror + topology channel shared by the CLAP main and audio
/// threads. Seeded to the factory-default patch
/// ([`PluginState::factory_default`]).
pub struct SharedParams {
    values: Vec<AtomicU32>,
    matrix: Mutex<MatrixTable>,
    /// Raised on `restore_from_bytes`; the audio thread clears it and re-syncs.
    reload: AtomicBool,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// A store seeded from the factory-default patch.
    pub fn new() -> Self {
        let factory = PluginState::factory_default();
        let values = (0..TOTAL_PARAMS)
            .map(|i| AtomicU32::new(factory.params.get_index(i).to_bits()))
            .collect();
        Self {
            values,
            matrix: Mutex::new(factory.matrix),
            reload: AtomicBool::new(false),
        }
    }

    #[inline]
    fn lock(&self) -> std::sync::MutexGuard<'_, MatrixTable> {
        // Recover a poisoned lock rather than propagating a panic: the guarded
        // topology is a plain value that's still readable after any mid-write
        // panic (plugin code unwinds).
        self.matrix.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Read a CLAP-id param value (`0.0` past the table).
    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        self.values
            .get(id)
            .map_or(0.0, |a| f32::from_bits(a.load(Ordering::Relaxed)))
    }

    /// Write a CLAP-id param value, clamped to the descriptor range.
    #[inline]
    pub fn set(&self, id: usize, value: f32) {
        if let Some(desc) = PARAMS.get(id) {
            self.values[id].store(desc.clamp(value).to_bits(), Ordering::Relaxed);
        }
    }

    /// A copy of the current matrix topology (for `state.save`).
    pub fn matrix_snapshot(&self) -> MatrixTable {
        *self.lock()
    }

    /// Whether the audio thread should re-sync the engine from this store; clears
    /// the flag. Call once at the top of `process`.
    #[inline]
    pub fn take_reload(&self) -> bool {
        self.reload.swap(false, Ordering::Acquire)
    }

    /// Snapshot the whole store to a `clap.state` blob (params + topology).
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let state = self.to_state();
        let mut blob = Vec::with_capacity(TOTAL_PARAMS * 4 + 64);
        // Writing to a `Vec` can't fail.
        let _ = state.write(&mut blob);
        blob
    }

    /// Build a [`PluginState`] from the current store contents.
    pub fn to_state(&self) -> PluginState {
        let mut params = crate::params::Params::default();
        for i in 0..TOTAL_PARAMS {
            params.set_index(i, self.get(i));
        }
        PluginState {
            params,
            matrix: self.matrix_snapshot(),
        }
    }

    /// Apply a `clap.state` blob. On success overwrites every param + the
    /// topology and raises [`Self::take_reload`]. Returns `Err` (leaving the
    /// store untouched) for an empty/undecodable blob — the 0196 contract: an
    /// invalid restore must report failure, not silently accept.
    pub fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        for i in 0..TOTAL_PARAMS {
            self.values[i].store(state.params.get_index(i).to_bits(), Ordering::Relaxed);
        }
        *self.lock() = state.matrix;
        self.reload.store(true, Ordering::Release);
        Ok(())
    }

    /// Sync a freshly-decoded engine-ready [`PluginState`] out of the store (used
    /// by the audio thread on reload / activate).
    pub fn engine_state(&self) -> PluginState {
        self.to_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamId;

    #[test]
    fn seeds_to_factory_default() {
        let sp = SharedParams::new();
        let factory = PluginState::factory_default();
        for i in 0..TOTAL_PARAMS {
            assert_eq!(sp.get(i), factory.params.get_index(i), "param {i}");
        }
        // The seeded Amp depth (0205) is present in the store.
        assert_eq!(sp.get(ParamId::MatrixSlot0Depth as usize), 1.0);
    }

    #[test]
    fn set_clamps_and_reads_back() {
        let sp = SharedParams::new();
        sp.set(ParamId::Resonance as usize, 9.0);
        assert_eq!(sp.get(ParamId::Resonance as usize), 1.0);
        sp.set(ParamId::Cutoff as usize, 500.0);
        assert_eq!(sp.get(ParamId::Cutoff as usize), 500.0);
    }

    #[test]
    fn snapshot_restore_round_trips_and_flags_reload() {
        let sp = SharedParams::new();
        sp.set(ParamId::Cutoff as usize, 1234.0);
        {
            let mut m = sp.lock();
            m.slots[5] = crate::matrix::MatrixSlot {
                source: crate::matrix::SourceId::Lfo2,
                dest: crate::matrix::DestId::Cutoff,
                depth: 0.0,
                curve: crate::matrix::Curve::Lin,
                scale_src: crate::matrix::SourceId::None,
            };
        }
        let blob = sp.snapshot_bytes();

        let sp2 = SharedParams::new();
        assert!(!sp2.take_reload());
        sp2.restore_from_bytes(&blob).unwrap();
        assert!(sp2.take_reload(), "restore must flag a reload");
        assert!(!sp2.take_reload(), "flag clears after one read");
        assert_eq!(sp2.get(ParamId::Cutoff as usize), 1234.0);
        assert_eq!(sp2.matrix_snapshot().slots[5].source, crate::matrix::SourceId::Lfo2);
    }

    #[test]
    fn empty_blob_restore_is_an_error_and_leaves_store() {
        let sp = SharedParams::new();
        sp.set(ParamId::Cutoff as usize, 777.0);
        assert!(sp.restore_from_bytes(&[]).is_err());
        assert_eq!(sp.get(ParamId::Cutoff as usize), 777.0, "store untouched on error");
        assert!(!sp.take_reload(), "a failed restore does not flag reload");
    }
}
