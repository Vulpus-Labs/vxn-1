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
//! **Gesture channel (E038).** Each param carries an `AtomicBool` gesture flag
//! the editor raises/lowers around a knob drag, so the controller can bracket a
//! UI drag into one host automation edit and suppress host echo mid-gesture.
//! Off the E038 GUI path this stays all-`false` and costs nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::matrix::MatrixTable;
use crate::params::{
    ClapRef, GLOBAL_PARAMS, Layer, MATRIX_SLOTS, PATCH_PARAMS, Params, TOTAL_PARAMS, clap_ref,
    desc_for_clap_id, global_clap_id, patch_clap_id,
};
use crate::state::{LayerState, PluginState};

/// Lock-free param mirror + topology channel shared by the CLAP main and audio
/// threads. Seeded to the factory-default patch
/// ([`PluginState::factory_default`]).
pub struct SharedParams {
    values: Vec<AtomicU32>,
    /// Per-param live-drag flags the editor raises around a UI gesture (E038).
    gestures: Vec<AtomicBool>,
    /// One matrix topology **per layer** (0216): index 0 = Layer 1, 1 = Layer 2.
    matrix: Mutex<[MatrixTable; 2]>,
    /// Raised on `restore_from_bytes`; the audio thread clears it and re-syncs.
    reload: AtomicBool,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// A store seeded from the factory-default patch (both layers).
    pub fn new() -> Self {
        let factory = PluginState::factory_default();
        let values = (0..TOTAL_PARAMS)
            .map(|id| {
                let r = clap_ref(id).expect("id < TOTAL_PARAMS");
                let layer = match r {
                    ClapRef::Patch(l, _) => l as usize,
                    ClapRef::Global(_) => 0,
                };
                AtomicU32::new(factory.layers[layer].params.get(r.inner()).to_bits())
            })
            .collect();
        let gestures = (0..TOTAL_PARAMS).map(|_| AtomicBool::new(false)).collect();
        Self {
            values,
            gestures,
            matrix: Mutex::new([factory.layers[0].matrix, factory.layers[1].matrix]),
            reload: AtomicBool::new(false),
        }
    }

    #[inline]
    fn lock(&self) -> std::sync::MutexGuard<'_, [MatrixTable; 2]> {
        // Recover a poisoned lock rather than propagating a panic: the guarded
        // topology is a plain value that's still readable after any mid-write
        // panic (plugin code unwinds).
        self.matrix.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build one layer's [`Params`] from the current CLAP value array: its own
    /// patch params plus the shared globals.
    fn layer_params(&self, layer: Layer) -> Params {
        let mut p = Params::default();
        for &inner in PATCH_PARAMS.iter() {
            let id = patch_clap_id(layer, inner).expect("patch param");
            p.set(inner, self.get(id));
        }
        for &g in GLOBAL_PARAMS.iter() {
            p.set(g, self.get(global_clap_id(g).expect("global param")));
        }
        p
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
        if let Some(desc) = desc_for_clap_id(id) {
            self.values[id].store(desc.clamp(value).to_bits(), Ordering::Relaxed);
        }
    }

    /// Read a CLAP-id param as a normalized `[0, 1]` position via its
    /// descriptor's host mapping (`0.0` past the table). The editor's controller
    /// echoes both plain and normalized on a `ParamChanged`.
    #[inline]
    pub fn get_normalized(&self, id: usize) -> f32 {
        match desc_for_clap_id(id) {
            Some(desc) => desc.to_normalized(self.get(id)),
            None => 0.0,
        }
    }

    /// Write a CLAP-id param from a normalized `[0, 1]` position (clamped to
    /// range by `set`). No-op past the table.
    #[inline]
    pub fn set_normalized(&self, id: usize, norm: f32) {
        if let Some(desc) = desc_for_clap_id(id) {
            self.set(id, desc.from_normalized(norm));
        }
    }

    /// Whether the editor is actively dragging `id` (E038). Host automation
    /// echo is suppressed while this is set so the knob doesn't fight the drag.
    #[inline]
    pub fn gesture(&self, id: usize) -> bool {
        self.gestures
            .get(id)
            .is_some_and(|g| g.load(Ordering::Relaxed))
    }

    /// Raise/lower the live-drag flag for `id` (E038). No-op past the table.
    #[inline]
    pub fn set_gesture(&self, id: usize, on: bool) {
        if let Some(g) = self.gestures.get(id) {
            g.store(on, Ordering::Relaxed);
        }
    }

    /// A copy of both layers' matrix topology (for `state.save`).
    pub fn matrix_snapshot(&self) -> [MatrixTable; 2] {
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

    /// Build a two-layer [`PluginState`] from the current store contents. Each
    /// layer's params come from its CLAP block (+ shared globals); its matrix
    /// topology comes from the per-layer topology channel, with slot depths
    /// re-seeded from the params so depth stays param-authoritative (0205).
    pub fn to_state(&self) -> PluginState {
        let matrices = self.matrix_snapshot();
        let layers = [Layer::L1, Layer::L2].map(|layer| {
            let params = self.layer_params(layer);
            let mut matrix = matrices[layer as usize];
            for s in 0..MATRIX_SLOTS {
                matrix.slots[s].depth = params.slot_depth(s);
            }
            LayerState { params, matrix }
        });
        PluginState { layers }
    }

    /// Apply a two-layer `clap.state` blob. On success overwrites every param +
    /// both topologies and raises [`Self::take_reload`]. Returns `Err` (leaving
    /// the store untouched) for an empty/undecodable blob — the 0196 contract: an
    /// invalid restore must report failure, not silently accept.
    pub fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        for layer in [Layer::L1, Layer::L2] {
            let p = &state.layers[layer as usize].params;
            for &inner in PATCH_PARAMS.iter() {
                let id = patch_clap_id(layer, inner).expect("patch param");
                self.values[id].store(p.get(inner).to_bits(), Ordering::Relaxed);
            }
        }
        // Globals are saved identically in both layers; take Layer 1's copy.
        let g = &state.layers[0].params;
        for &gp in GLOBAL_PARAMS.iter() {
            let id = global_clap_id(gp).expect("global param");
            self.values[id].store(g.get(gp).to_bits(), Ordering::Relaxed);
        }
        *self.lock() = [state.layers[0].matrix, state.layers[1].matrix];
        self.reload.store(true, Ordering::Release);
        Ok(())
    }

    /// Sync a freshly-decoded engine-ready [`PluginState`] out of the store (used
    /// by the audio thread on reload / activate).
    pub fn engine_state(&self) -> PluginState {
        self.to_state()
    }
}

// ── ParamModel trait (vxn-core-app) ───────────────────────────────────────────
//
// The E038 editor's [`vxn_core_app::Controller`] drives the parameter store
// through [`vxn_core_app::ParamModel`]; this is the adaptor that lets it. Pure
// delegation to the inherent methods above. `SharedParams` stays trait-free on
// the audio path (0204); the trait surface is only the controller's generic
// seam. Lives in-crate so the orphan rules don't bite (both `SharedParams` and
// the trait would be foreign to the clap crate).

impl vxn_core_app::ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get(self, id.raw())
    }

    fn set(&self, id: vxn_core_app::ParamId, plain: f32) {
        SharedParams::set(self, id.raw(), plain);
    }

    fn get_normalized(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get_normalized(self, id.raw())
    }

    fn set_normalized(&self, id: vxn_core_app::ParamId, norm: f32) {
        SharedParams::set_normalized(self, id.raw(), norm);
    }

    fn gesture(&self, id: vxn_core_app::ParamId) -> bool {
        SharedParams::gesture(self, id.raw())
    }

    fn set_gesture(&self, id: vxn_core_app::ParamId, on: bool) {
        SharedParams::set_gesture(self, id.raw(), on);
    }

    fn descriptor(&self, id: vxn_core_app::ParamId) -> Option<&'static vxn_core_app::ParamDesc> {
        desc_for_clap_id(id.raw())
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        SharedParams::snapshot_bytes(self)
    }

    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        SharedParams::restore_from_bytes(self, blob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ParamId, clap_id_of};

    #[test]
    fn seeds_to_factory_default() {
        let sp = SharedParams::new();
        let factory = PluginState::factory_default();
        for id in 0..TOTAL_PARAMS {
            let r = clap_ref(id).unwrap();
            let layer = match r {
                ClapRef::Patch(l, _) => l as usize,
                ClapRef::Global(_) => 0,
            };
            assert_eq!(
                sp.get(id),
                factory.layers[layer].params.get(r.inner()),
                "param {id}"
            );
        }
        // The seeded Amp depth (0205) is present in the store on both layers.
        assert_eq!(sp.get(clap_id_of(Layer::L1, ParamId::MatrixSlot0Depth)), 1.0);
        assert_eq!(sp.get(clap_id_of(Layer::L2, ParamId::MatrixSlot0Depth)), 1.0);
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
        // Distinct edits on each layer to prove per-layer round-trip.
        sp.set(clap_id_of(Layer::L1, ParamId::Cutoff), 1234.0);
        sp.set(clap_id_of(Layer::L2, ParamId::Cutoff), 220.0);
        {
            let mut m = sp.lock();
            m[1].slots[5] = crate::matrix::MatrixSlot {
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
        assert_eq!(sp2.get(clap_id_of(Layer::L1, ParamId::Cutoff)), 1234.0);
        assert_eq!(sp2.get(clap_id_of(Layer::L2, ParamId::Cutoff)), 220.0);
        // Layer 2's private matrix edit survived on layer 2, not layer 1.
        assert_eq!(sp2.matrix_snapshot()[1].slots[5].source, crate::matrix::SourceId::Lfo2);
        assert!(!sp2.matrix_snapshot()[0].slots[5].is_active());
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
