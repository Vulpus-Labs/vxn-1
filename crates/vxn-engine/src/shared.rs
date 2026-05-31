//! Thread-safe parameter store shared between the audio thread, the main thread
//! and the UI.
//!
//! Indexed by **CLAP id** (the host/UI boundary speaks ids — see
//! [`crate::params`] for the layout): a flat `[AtomicU32; TOTAL_PARAMS]` of plain
//! `f32` values stored as bits. This is the single source of truth that all
//! writers (host automation, UI edits, state load) update and all readers
//! observe:
//!
//! - **Host → engine/UI:** the CLAP layer applies input param events to this
//!   store; the audio thread snapshots it into the engine each block; the UI
//!   polls it on repaint.
//! - **UI → host:** the UI writes the new value here and raises a *gesture*
//!   ([`set_gesture`]) while a knob is held; the CLAP layer diffs this store
//!   against a per-thread mirror and emits the change (wrapped in gesture
//!   begin/end) to the host as output param events.
//!
//! Alongside the param array it carries the **non-automatable shared state**
//! (key mode + split point — ADR 0003 §3, §8) as atomics: setup state, not
//! sound parameters, set discretely from the UI and persisted via
//! [`crate::state`].
//!
//! Kept in `vxn-engine` (framework-free) so `vxn-clap` and `vxn-ui-web`
//! share one definition without depending on each other.

use crate::params::{
    KeyMode, Layer, ParamValues, PatchParam, TOTAL_PARAMS, desc_for_clap_id, patch_clap_id,
};
use crate::state::PluginState;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

/// Atomic, lock-free parameter store. Intended to live behind an `Arc` so the
/// editor and the plugin can both hold it.
pub struct SharedParams {
    values: [AtomicU32; TOTAL_PARAMS],
    /// Whether the UI is currently holding an edit gesture (e.g. pointer down)
    /// on each param. Read by the plugin to bracket output events in CLAP
    /// gesture begin/end.
    gesture: [AtomicBool; TOTAL_PARAMS],
    /// Key mode (ADR 0003 §3) — non-automatable shared state.
    key_mode: AtomicU8,
    /// Split point as a MIDI note (ADR 0003 §8) — non-automatable shared state.
    split_point: AtomicU8,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    pub fn new() -> Self {
        Self {
            values: std::array::from_fn(|i| {
                AtomicU32::new(desc_for_clap_id(i).map_or(0.0, |d| d.default).to_bits())
            }),
            gesture: std::array::from_fn(|_| AtomicBool::new(false)),
            key_mode: AtomicU8::new(KeyMode::default() as u8),
            split_point: AtomicU8::new(crate::params::DEFAULT_SPLIT_POINT),
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    /// Store `value` (clamped to the param's range) at CLAP id `index`.
    #[inline]
    pub fn set(&self, index: usize, value: f32) {
        if let Some(d) = desc_for_clap_id(index) {
            self.values[index].store(d.clamp(value).to_bits(), Ordering::Relaxed);
        }
    }

    /// Read by normalized `[0, 1]` position (UI convenience).
    #[inline]
    pub fn get_normalized(&self, index: usize) -> f32 {
        desc_for_clap_id(index).map_or(0.0, |d| d.to_normalized(self.get(index)))
    }

    /// Write from a normalized `[0, 1]` position (UI convenience).
    #[inline]
    pub fn set_normalized(&self, index: usize, n: f32) {
        if let Some(d) = desc_for_clap_id(index) {
            self.set(index, d.from_normalized(n));
        }
    }

    #[inline]
    pub fn gesture(&self, index: usize) -> bool {
        self.gesture[index].load(Ordering::Relaxed)
    }

    /// Mark the start (`true`) or end (`false`) of a UI edit gesture.
    #[inline]
    pub fn set_gesture(&self, index: usize, active: bool) {
        if index < TOTAL_PARAMS {
            self.gesture[index].store(active, Ordering::Relaxed);
        }
    }

    // ── Non-automatable shared state ──────────────────────────────────────────

    #[inline]
    pub fn key_mode(&self) -> KeyMode {
        KeyMode::from_u8(self.key_mode.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_key_mode(&self, mode: KeyMode) {
        self.key_mode.store(mode as u8, Ordering::Relaxed);
    }

    /// Set the key mode from a **discrete UI edit**, performing the one-shot
    /// seed-on-entry copy (ADR 0003 §3): the first transition out of Whole copies
    /// layer A (Upper) → layer B (Lower) so Lower starts equal to Upper and then
    /// diverges. Editing in the store (not just the engine) means the copy
    /// persists and the CLAP layer echoes the seeded Lower values to the host.
    /// `Dual ↔ Split` does not re-seed; state load uses [`Self::set_key_mode`].
    pub fn set_key_mode_seeded(&self, mode: KeyMode) {
        if self.key_mode() == KeyMode::Whole && mode != KeyMode::Whole {
            self.seed_lower_from_upper();
        }
        self.set_key_mode(mode);
    }

    /// Copy every Upper per-patch value into the corresponding Lower slot. The
    /// two per-patch blocks are contiguous CLAP-id ranges (0007), so Lower's id
    /// is its Upper id plus one block width.
    fn seed_lower_from_upper(&self) {
        for p in 0..crate::params::PATCH_COUNT {
            let upper = patch_clap_id(Layer::Upper, PatchParam::from_index(p).unwrap());
            let lower = patch_clap_id(Layer::Lower, PatchParam::from_index(p).unwrap());
            self.values[lower].store(
                self.values[upper].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
    }

    /// Reset every per-patch param of `layer` to its descriptor default. Each
    /// write is bracketed by a gesture (like the UI's double-click reset) so the
    /// CLAP layer echoes the jump to the host as a recorded edit. Globals and the
    /// other layer are left untouched; key mode and split point are setup state,
    /// not part of a patch, so they are also left alone.
    pub fn reset_patch_to_defaults(&self, layer: Layer) {
        for p in 0..crate::params::PATCH_COUNT {
            let id = patch_clap_id(layer, PatchParam::from_index(p).unwrap());
            let default = desc_for_clap_id(id).map_or(0.0, |d| d.default);
            self.set_gesture(id, true);
            self.set(id, default);
            self.set_gesture(id, false);
        }
    }

    // ── Preset load (E007 / 0026) ─────────────────────────────────────────────

    /// Load a whole [`PluginState`] into the store (preset load):
    /// both layers and the global block written gesture-bracketed (so the host's
    /// automation/displayed values follow), plus the non-automatable key mode +
    /// split point applied directly — the same path the `state` blob uses.
    ///
    /// Key mode is set **plainly**, not seeded: a Performance carries both layers
    /// explicitly, so the Whole→non-Whole seed-on-entry copy would clobber the
    /// Lower layer the file just supplied. Seeding belongs only to a *discrete
    /// UI* key-mode edit (see [`set_key_mode_seeded`]).
    ///
    /// [`set_key_mode_seeded`]: Self::set_key_mode_seeded
    pub fn load_performance(&self, state: &PluginState) {
        for i in 0..TOTAL_PARAMS {
            self.set_gesture(i, true);
            self.set(i, state.params.get_by_clap_id(i));
            self.set_gesture(i, false);
        }
        self.set_key_mode(state.key_mode);
        self.set_split_point(state.split_point);
    }

    #[inline]
    pub fn split_point(&self) -> u8 {
        self.split_point.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_split_point(&self, note: u8) {
        self.split_point.store(note.min(127), Ordering::Relaxed);
    }

    // ── Engine / state-blob bridges ───────────────────────────────────────────

    /// Copy the whole store into an engine [`ParamValues`] (audio thread),
    /// routing each CLAP id into its layer/global slot.
    pub fn snapshot_into(&self, params: &mut ParamValues) {
        for i in 0..TOTAL_PARAMS {
            params.set_by_clap_id(i, self.get(i));
        }
    }

    /// Build a [`PluginState`] snapshot for serialization.
    pub fn to_state(&self) -> PluginState {
        let mut params = ParamValues::default();
        self.snapshot_into(&mut params);
        PluginState {
            params,
            key_mode: self.key_mode(),
            split_point: self.split_point(),
        }
    }

    /// Apply a deserialized [`PluginState`] back into the store (state load).
    pub fn restore_from(&self, state: &PluginState) {
        for i in 0..TOTAL_PARAMS {
            self.set(i, state.params.get_by_clap_id(i));
        }
        self.set_key_mode(state.key_mode);
        self.set_split_point(state.split_point);
    }
}

// ── ParamModel trait (vxn-app) ───────────────────────────────────────────────
//
// The controller drives the parameter store through `ParamModel`; this is the
// adaptor that lets it. Pure delegation — every method maps to an existing
// inherent method on `SharedParams`. `SharedParams` itself stays trait-free for
// the audio path; the trait surface is for the controller's generic seam.

impl vxn_app::ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get(&self, id: vxn_app::ParamId) -> f32 {
        SharedParams::get(self, id.raw())
    }

    fn set(&self, id: vxn_app::ParamId, plain: f32) {
        SharedParams::set(self, id.raw(), plain);
    }

    fn get_normalized(&self, id: vxn_app::ParamId) -> f32 {
        SharedParams::get_normalized(self, id.raw())
    }

    fn set_normalized(&self, id: vxn_app::ParamId, norm: f32) {
        SharedParams::set_normalized(self, id.raw(), norm);
    }

    fn gesture(&self, id: vxn_app::ParamId) -> bool {
        SharedParams::gesture(self, id.raw())
    }

    fn set_gesture(&self, id: vxn_app::ParamId, on: bool) {
        SharedParams::set_gesture(self, id.raw(), on);
    }

    fn descriptor(&self, id: vxn_app::ParamId) -> Option<&'static vxn_app::ParamDesc> {
        desc_for_clap_id(id.raw())
    }

    fn key_mode(&self) -> KeyMode {
        SharedParams::key_mode(self)
    }

    fn set_key_mode(&self, mode: KeyMode) {
        SharedParams::set_key_mode(self, mode);
    }

    fn set_key_mode_seeded(&self, mode: KeyMode) {
        SharedParams::set_key_mode_seeded(self, mode);
    }

    fn split_point(&self) -> u8 {
        SharedParams::split_point(self)
    }

    fn set_split_point(&self, note: u8) {
        SharedParams::set_split_point(self, note);
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        self.to_state()
            .write(&mut buf)
            .expect("PluginState::write into Vec is infallible");
        buf
    }

    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        self.restore_from(&state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{GlobalParam, Layer, PatchParam, global_clap_id, patch_clap_id};

    #[test]
    fn defaults_match_table_and_clamp() {
        let s = SharedParams::new();
        let vol = global_clap_id(GlobalParam::MasterVolume);
        assert_eq!(s.get(vol), GlobalParam::MasterVolume.desc().default);
        // Out-of-range writes are clamped to the descriptor range.
        let reso = patch_clap_id(Layer::Upper, PatchParam::Resonance);
        s.set(reso, 5.0);
        assert_eq!(s.get(reso), 1.0);
    }

    #[test]
    fn whole_to_dual_seeds_lower_from_upper_once() {
        let s = SharedParams::new();
        let up = patch_clap_id(Layer::Upper, PatchParam::Cutoff);
        let lo = patch_clap_id(Layer::Lower, PatchParam::Cutoff);
        s.set(up, 1234.0);
        s.set(lo, 9999.0);
        // Whole → Dual seeds Lower from Upper.
        s.set_key_mode_seeded(KeyMode::Dual);
        assert_eq!(s.get(lo), 1234.0, "Lower not seeded from Upper");
        assert_eq!(s.key_mode(), KeyMode::Dual);

        // Diverge Lower, then Dual → Split must NOT re-seed.
        s.set(lo, 555.0);
        s.set_key_mode_seeded(KeyMode::Split);
        assert_eq!(s.get(lo), 555.0, "Dual→Split should not re-seed");
    }

    #[test]
    fn returning_to_whole_then_out_seeds_again() {
        let s = SharedParams::new();
        let up = patch_clap_id(Layer::Upper, PatchParam::Cutoff);
        let lo = patch_clap_id(Layer::Lower, PatchParam::Cutoff);
        s.set_key_mode_seeded(KeyMode::Dual);
        s.set_key_mode_seeded(KeyMode::Whole);
        s.set(up, 4000.0);
        s.set(lo, 1.0);
        // Leaving Whole again re-seeds from the current Upper.
        s.set_key_mode_seeded(KeyMode::Split);
        assert_eq!(s.get(lo), 4000.0);
    }

    #[test]
    fn layers_are_independent() {
        let s = SharedParams::new();
        let up = patch_clap_id(Layer::Upper, PatchParam::Cutoff);
        let lo = patch_clap_id(Layer::Lower, PatchParam::Cutoff);
        s.set(up, 1000.0);
        s.set(lo, 2000.0);
        assert_eq!(s.get(up), 1000.0);
        assert_eq!(s.get(lo), 2000.0);
    }

    #[test]
    fn normalized_roundtrip() {
        let s = SharedParams::new();
        let cutoff = patch_clap_id(Layer::Upper, PatchParam::Cutoff);
        s.set_normalized(cutoff, 0.0);
        assert_eq!(s.get(cutoff), PatchParam::Cutoff.desc().min);
        s.set_normalized(cutoff, 1.0);
        assert_eq!(s.get(cutoff), PatchParam::Cutoff.desc().max);
    }

    #[test]
    fn gesture_flag_roundtrips() {
        let s = SharedParams::new();
        assert!(!s.gesture(0));
        s.set_gesture(0, true);
        assert!(s.gesture(0));
    }

    #[test]
    fn key_mode_and_split_default_and_roundtrip() {
        let s = SharedParams::new();
        assert_eq!(s.key_mode(), KeyMode::Whole);
        assert_eq!(s.split_point(), crate::params::DEFAULT_SPLIT_POINT);
        s.set_key_mode(KeyMode::Dual);
        s.set_split_point(72);
        assert_eq!(s.key_mode(), KeyMode::Dual);
        assert_eq!(s.split_point(), 72);
    }

    #[test]
    fn load_performance_restores_everything_without_seeding() {
        let s = SharedParams::new();
        let mut params = ParamValues::default();
        params.layer_mut(Layer::Upper).set(PatchParam::Cutoff, 1111.0);
        params.layer_mut(Layer::Lower).set(PatchParam::Cutoff, 2222.0);
        let state = PluginState {
            params,
            key_mode: KeyMode::Split,
            split_point: 48,
        };
        s.load_performance(&state);

        // Both layers kept their distinct values — no Upper→Lower seed clobbered
        // the explicit Lower the performance supplied.
        assert_eq!(s.get(patch_clap_id(Layer::Upper, PatchParam::Cutoff)), 1111.0);
        assert_eq!(s.get(patch_clap_id(Layer::Lower, PatchParam::Cutoff)), 2222.0);
        assert_eq!(s.key_mode(), KeyMode::Split);
        assert_eq!(s.split_point(), 48);
    }

    #[test]
    fn param_model_trait_roundtrips_through_arc_dyn() {
        use std::sync::Arc;
        use vxn_app::{ParamId, ParamModel};
        let m: Arc<dyn ParamModel> = Arc::new(SharedParams::new());
        let id = ParamId::new(patch_clap_id(Layer::Upper, PatchParam::Cutoff));
        m.set(id, 1234.0);
        assert_eq!(m.get(id), 1234.0);
        m.set_gesture(id, true);
        assert!(m.gesture(id));
        m.set_gesture(id, false);
        let desc = m.descriptor(id).expect("descriptor present");
        assert!((desc.min..=desc.max).contains(&1234.0));
        assert_eq!(m.total(), TOTAL_PARAMS);
    }

    #[test]
    fn param_model_trait_writes_visible_to_concrete() {
        use std::sync::Arc;
        use vxn_app::{ParamId, ParamModel};
        let shared = Arc::new(SharedParams::new());
        let m: Arc<dyn ParamModel> = shared.clone();
        let id = ParamId::new(global_clap_id(GlobalParam::MasterVolume));
        m.set(id, 0.25);
        // Concrete reader sees the trait writer's update — no orphan-rules
        // duplication of the underlying atomic.
        assert_eq!(shared.get(id.raw()), 0.25);
    }

    #[test]
    fn state_roundtrip_through_store() {
        let s = SharedParams::new();
        let up = patch_clap_id(Layer::Upper, PatchParam::Cutoff);
        s.set(up, 4321.0);
        s.set_key_mode(KeyMode::Split);
        s.set_split_point(48);

        let state = s.to_state();
        let s2 = SharedParams::new();
        s2.restore_from(&state);
        assert_eq!(s2.get(up), 4321.0);
        assert_eq!(s2.key_mode(), KeyMode::Split);
        assert_eq!(s2.split_point(), 48);
    }
}
