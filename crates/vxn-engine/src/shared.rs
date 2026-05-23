//! Thread-safe parameter store shared between the audio thread, the main thread
//! and the UI.
//!
//! Values are plain `f32` (plain units, matching [`ParamValues`]) stored as bits
//! in `AtomicU32`. This is the single source of truth that all three writers
//! (host automation, UI edits, state load) update and all readers observe:
//!
//! - **Host → engine/UI:** the CLAP layer applies input param events to this
//!   store; the audio thread snapshots it into the engine each block; the UI
//!   polls it on repaint.
//! - **UI → host:** the UI writes the new value here and raises a *gesture*
//!   ([`set_gesture`]) while a knob is held; the CLAP layer diffs this store
//!   against a per-thread mirror and emits the change (wrapped in gesture
//!   begin/end) to the host as output param events.
//!
//! Kept in `vxn-engine` (framework-free) so both `vxn-clap` and `vxn-ui` share
//! one definition without depending on each other.

use crate::{PARAMS, ParamId, ParamValues};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Atomic, lock-free parameter store. Intended to live behind an `Arc` so the
/// editor and the plugin can both hold it.
pub struct SharedParams {
    values: [AtomicU32; ParamId::COUNT],
    /// Whether the UI is currently holding an edit gesture (e.g. pointer down)
    /// on each param. Read by the plugin to bracket output events in CLAP
    /// gesture begin/end.
    gesture: [AtomicBool; ParamId::COUNT],
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    pub fn new() -> Self {
        Self {
            values: std::array::from_fn(|i| AtomicU32::new(PARAMS[i].default.to_bits())),
            gesture: std::array::from_fn(|_| AtomicBool::new(false)),
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    /// Store `value` (clamped to the param's range) at `index`.
    #[inline]
    pub fn set(&self, index: usize, value: f32) {
        if index < ParamId::COUNT {
            let clamped = PARAMS[index].clamp(value);
            self.values[index].store(clamped.to_bits(), Ordering::Relaxed);
        }
    }

    /// Read by normalized `[0, 1]` position (UI convenience).
    #[inline]
    pub fn get_normalized(&self, index: usize) -> f32 {
        PARAMS[index].to_normalized(self.get(index))
    }

    /// Write from a normalized `[0, 1]` position (UI convenience).
    #[inline]
    pub fn set_normalized(&self, index: usize, n: f32) {
        if index < ParamId::COUNT {
            self.set(index, PARAMS[index].from_normalized(n));
        }
    }

    #[inline]
    pub fn gesture(&self, index: usize) -> bool {
        self.gesture[index].load(Ordering::Relaxed)
    }

    /// Mark the start (`true`) or end (`false`) of a UI edit gesture.
    #[inline]
    pub fn set_gesture(&self, index: usize, active: bool) {
        if index < ParamId::COUNT {
            self.gesture[index].store(active, Ordering::Relaxed);
        }
    }

    /// Copy the whole store into an engine [`ParamValues`] (audio thread).
    pub fn snapshot_into(&self, params: &mut ParamValues) {
        for i in 0..ParamId::COUNT {
            params.set_index(i, self.get(i));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_table_and_clamp() {
        let s = SharedParams::new();
        assert_eq!(
            s.get(ParamId::MasterVolume.index()),
            PARAMS[ParamId::MasterVolume.index()].default
        );
        // Out-of-range writes are clamped to the descriptor range.
        s.set(ParamId::Resonance.index(), 5.0);
        assert_eq!(s.get(ParamId::Resonance.index()), 1.0);
    }

    #[test]
    fn normalized_roundtrip() {
        let s = SharedParams::new();
        s.set_normalized(ParamId::Cutoff.index(), 0.0);
        assert_eq!(
            s.get(ParamId::Cutoff.index()),
            PARAMS[ParamId::Cutoff.index()].min
        );
        s.set_normalized(ParamId::Cutoff.index(), 1.0);
        assert_eq!(
            s.get(ParamId::Cutoff.index()),
            PARAMS[ParamId::Cutoff.index()].max
        );
    }

    #[test]
    fn gesture_flag_roundtrips() {
        let s = SharedParams::new();
        assert!(!s.gesture(0));
        s.set_gesture(0, true);
        assert!(s.gesture(0));
    }
}
