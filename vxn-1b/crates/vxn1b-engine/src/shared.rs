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

use crate::engine::{KeyOp, KeyState, MatrixEdit, MatrixField};
use crate::matrix::{Curve, DestId, MatrixTable, SourceId};
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
    /// Non-automatable keyboard state (0219). Written by the controller tick from
    /// a `set_key_mode` / `set_split_point` custom op; the audio thread swaps
    /// `key_dirty` and re-applies it to the engine's [`KeyState`].
    key: Mutex<KeyState>,
    key_dirty: AtomicBool,
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
            key: Mutex::new(KeyState::default()),
            key_dirty: AtomicBool::new(false),
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

    /// Read a CLAP-id param as a **fader position** in `[0, 1]` (`0.0` past the
    /// table). The editor's controller echoes both plain and this position on a
    /// `ParamChanged`, and the faders paint the thumb from it.
    ///
    /// This is `to_fader`, not `to_normalized`: the descriptor's taper is part
    /// of the calibration, not a display flourish. Cutoff is
    /// `Exp { mid: 800 }` over 16.35 Hz … 16 kHz, so a *linear* position would
    /// put 800 Hz at 5% of the travel and spend the top half of the fader
    /// between 8 k and 16 k — the whole usable low end crushed into the bottom
    /// centimetre. `to_fader` pins the midpoint to `mid` and gives each octave
    /// roughly equal travel. VXN1's `SharedParams` has always done this
    /// (`vxn-1/crates/vxn-engine/src/shared.rs`); VXN1b's fork read the linear
    /// pair, which is the entire behavioural difference in fader feel between
    /// the two synths (0243).
    ///
    /// Only the editor path goes through here — CLAP exchanges *plain* values
    /// against the descriptor range, and preset/state I/O is plain — so the
    /// taper never reaches host automation or the wire format.
    #[inline]
    pub fn get_normalized(&self, id: usize) -> f32 {
        match desc_for_clap_id(id) {
            Some(desc) => desc.to_fader(self.get(id)),
            None => 0.0,
        }
    }

    /// Write a CLAP-id param from a fader position in `[0, 1]` (clamped to
    /// range by `set`). Inverse of [`Self::get_normalized`], taper included, so
    /// a drag and the echo that answers it agree. No-op past the table.
    #[inline]
    pub fn set_normalized(&self, id: usize, norm: f32) {
        if let Some(desc) = desc_for_clap_id(id) {
            self.set(id, desc.from_fader(norm));
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

    /// Apply a UI key-op to the shared [`KeyState`] and flag the audio thread to
    /// re-sync (0219). Called on the controller (main) thread.
    pub fn apply_key_op(&self, op: KeyOp) {
        self.key.lock().unwrap_or_else(|e| e.into_inner()).apply(op);
        self.key_dirty.store(true, Ordering::Release);
    }

    /// Apply a UI matrix-topology edit to the per-layer matrix channel and flag a
    /// reload (0219). The audio thread's `take_reload` re-syncs the engine from
    /// `engine_state()`, which reads this topology (depths stay param-authoritative
    /// — the codec re-seeds them). Out-of-range slot indices are ignored.
    pub fn edit_matrix_slot(&self, edit: MatrixEdit) {
        {
            let mut m = self.lock();
            if let Some(slot) = m[edit.layer as usize].slots.get_mut(edit.slot as usize) {
                match edit.field {
                    MatrixField::Source => slot.source = SourceId::from_u8(edit.value),
                    MatrixField::Dest => slot.dest = DestId::from_u8(edit.value),
                    MatrixField::Curve => slot.curve = Curve::from_u8(edit.value),
                    MatrixField::ScaleSrc => slot.scale_src = SourceId::from_u8(edit.value),
                }
            }
        }
        self.reload.store(true, Ordering::Release);
    }

    /// The current keyboard state, if a key-op landed since the last call
    /// (clears the dirty flag). The audio thread applies it via
    /// [`crate::Engine::set_key_state`]. `None` means no change — the common case.
    #[inline]
    pub fn take_key_state(&self) -> Option<KeyState> {
        if self.key_dirty.swap(false, Ordering::Acquire) {
            Some(*self.key.lock().unwrap_or_else(|e| e.into_inner()))
        } else {
            None
        }
    }

    /// The current keyboard state **without** consuming the dirty flag (0221).
    /// The main thread's editor echo reads it every tick and diffs; only the
    /// audio thread's [`Self::take_key_state`] may clear the flag, or a tick that
    /// happened to land first would swallow the re-sync.
    #[inline]
    pub fn key_state(&self) -> KeyState {
        *self.key.lock().unwrap_or_else(|e| e.into_inner())
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
    /// re-seeded from the params so depth stays param-authoritative (0205). The
    /// keyboard record rides along from the key channel (0221), so a `state.save`
    /// carries the routing mode as well as the two patches.
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
        PluginState { layers, key: self.key_state() }
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
        // Keyboard state (0221) travels its own channel to the engine, so the
        // reload flag alone would not carry it: raise `key_dirty` too, and the
        // audio thread's `take_key_state` applies the loaded routing mode.
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = state.key;
        self.key_dirty.store(true, Ordering::Release);
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
    fn key_op_channel_is_dirty_once_and_carries_state() {
        use crate::engine::{KeyMode, KeyOp};
        let sp = SharedParams::new();
        // Clean by default — no spurious re-sync.
        assert!(sp.take_key_state().is_none());

        sp.apply_key_op(KeyOp::SetKeyMode(1)); // enable → Dual
        let k = sp.take_key_state().expect("dirty after a key-op");
        assert_eq!(k.key_mode(), KeyMode::Dual);
        assert!(k.layer2_on);
        // Flag clears after one read.
        assert!(sp.take_key_state().is_none());

        sp.apply_key_op(KeyOp::SetSplitPoint(48));
        sp.apply_key_op(KeyOp::SetKeyMode(2)); // Split, keeps the point
        let k = sp.take_key_state().unwrap();
        assert_eq!(k.key_mode(), KeyMode::Split);
        assert_eq!(k.split_point, 48);
    }

    #[test]
    fn snapshot_restore_carries_key_state_and_flags_the_key_channel() {
        use crate::engine::{KeyMode, KeyOp};
        let sp = SharedParams::new();
        sp.apply_key_op(KeyOp::SetSplitPoint(43));
        sp.apply_key_op(KeyOp::SetKeyMode(2)); // Split
        sp.apply_key_op(KeyOp::SetLfo2Link(true));
        // Drain the dirty flag: the snapshot must read the state itself, not
        // depend on a pending re-sync.
        assert!(sp.take_key_state().is_some());
        let blob = sp.snapshot_bytes();

        let sp2 = SharedParams::new();
        assert!(sp2.take_key_state().is_none(), "fresh store is clean");
        sp2.restore_from_bytes(&blob).unwrap();

        // The restore must flag the key channel as well as the reload — they are
        // separate wires to the audio thread.
        let k = sp2.take_key_state().expect("restore must flag the key channel");
        assert_eq!(k.key_mode(), KeyMode::Split);
        assert_eq!(k.split_point, 43);
        assert!(k.lfo2_link);
        // And the non-consuming peek agrees with what the audio thread got.
        assert_eq!(sp2.key_state(), k);
    }

    #[test]
    fn matrix_edit_updates_the_right_layer_and_flags_reload() {
        use crate::engine::{MatrixEdit, MatrixField};
        use crate::matrix::{DestId, SourceId};
        use crate::params::Layer;
        let sp = SharedParams::new();
        // Edit Layer 2, slot 7: route LFO2 → Cutoff.
        sp.edit_matrix_slot(MatrixEdit {
            layer: Layer::L2,
            slot: 7,
            field: MatrixField::Source,
            value: SourceId::Lfo2 as u8,
        });
        sp.edit_matrix_slot(MatrixEdit {
            layer: Layer::L2,
            slot: 7,
            field: MatrixField::Dest,
            value: DestId::Cutoff as u8,
        });
        assert!(sp.take_reload(), "a topology edit must flag a reload");

        let m = sp.matrix_snapshot();
        assert_eq!(m[1].slots[7].source, SourceId::Lfo2);
        assert_eq!(m[1].slots[7].dest, DestId::Cutoff);
        // Layer 1's same slot is untouched.
        assert!(!m[0].slots[7].is_active());
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
