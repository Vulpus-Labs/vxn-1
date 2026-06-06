//! Audio-thread parameter mirror.
//!
//! Bridges the host (CLAP automation), the audio-thread engine, and the
//! UI write path. Follows clack's `gain-gui` pattern: the plugin never
//! writes the shared store directly from host input events; the audio
//! thread keeps a local mirror.
//!
//! 1. [`LocalParams::fetch_ui_changes`] pulls UI-originated writes out
//!    of the shared store (flags them for echo to the host).
//! 2. [`LocalParams::apply_input`] folds a host `ParamValue` event into
//!    the mirror and returns `(idx, value)` so the caller can drive the
//!    engine immediately.
//! 3. [`LocalParams::values`] / `value_at` exposes the working snapshot
//!    the engine reads at the top of each block.
//! 4. [`LocalParams::publish`] writes host-changed slots back to the
//!    shared store so `get_value` reflects automation. Only host-flagged
//!    slots are written — a concurrent UI bulk write (preset load) is
//!    not clobbered.
//! 5. [`LocalParams::emit`] sends UI edits back to the host as
//!    `ParamValue` events.

use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;

use crate::engine::SharedStore;
use crate::gesture::emit_param_value;

pub struct LocalParams<const N: usize> {
    values: [f32; N],
    ui_changed: [bool; N],
    host_changed: [bool; N],
}

impl<const N: usize> LocalParams<N> {
    /// Seed the mirror from `shared`'s current values.
    pub fn new<S: SharedStore>(shared: &S) -> Self {
        Self {
            values: std::array::from_fn(|i| shared.get(i)),
            ui_changed: [false; N],
            host_changed: [false; N],
        }
    }

    /// Pull UI-originated writes from `shared` into the mirror, flagging
    /// each slot for echo to the host. Returns `true` if anything
    /// changed.
    pub fn fetch_ui_changes<S: SharedStore>(&mut self, shared: &S) -> bool {
        let mut any = false;
        for i in 0..N {
            let sv = shared.get(i);
            if sv != self.values[i] {
                self.values[i] = sv;
                self.ui_changed[i] = true;
                any = true;
            }
        }
        any
    }

    /// Fold a host param-value input event into the mirror. Returns
    /// `(idx, value)` so the caller can forward to the engine. Not
    /// flagged as a UI change — never echoed back to the host.
    pub fn apply_input(&mut self, event: &UnknownEvent) -> Option<(usize, f32)> {
        if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
            if let Some(pid) = e.param_id() {
                let i = pid.get() as usize;
                if i < N {
                    let v = e.value() as f32;
                    self.values[i] = v;
                    self.host_changed[i] = true;
                    return Some((i, v));
                }
            }
        }
        None
    }

    /// Publish host-automation changes to `shared`. Only slots flagged
    /// by [`Self::apply_input`] this block are written (then cleared).
    /// Re-publishing the whole mirror would race concurrent UI writes —
    /// preset-load bulk writes would silently revert.
    pub fn publish<S: SharedStore>(&mut self, shared: &S) {
        for i in 0..N {
            if self.host_changed[i] {
                shared.set(i, self.values[i]);
                self.host_changed[i] = false;
            }
        }
    }

    /// Emit UI-originated changes to the host as `ParamValue` events at
    /// sample offset 0. Caller may emit gesture brackets around this
    /// call ([`crate::emit_gesture_begin`] /
    /// [`crate::emit_gesture_end`]).
    pub fn emit(&mut self, out: &mut OutputEvents<'_>) {
        for i in 0..N {
            if !self.ui_changed[i] {
                continue;
            }
            self.ui_changed[i] = false;
            emit_param_value(out, i as u32, self.values[i] as f64, 0);
        }
    }

    #[inline]
    pub fn values(&self) -> &[f32; N] {
        &self.values
    }

    #[inline]
    pub fn value_at(&self, id: usize) -> f32 {
        if id < N { self.values[id] } else { 0.0 }
    }

    #[inline]
    pub fn host_changed(&self, id: usize) -> bool {
        id < N && self.host_changed[id]
    }

    /// Force `id`'s value into the mirror without flagging either
    /// changed bit. Used during preset load: the synth's snapshot apply
    /// writes the shared store; the mirror needs to track that without
    /// echoing back to the host.
    pub fn force_set(&mut self, id: usize, value: f32) {
        if id < N {
            self.values[id] = value;
        }
    }
}
