//! Audio-thread parameter mirror (ticket 0014).
//!
//! Bridges the host (CLAP automation), the audio-thread engine, and — once
//! the UI epic lands — UI-originated writes. Following clack's gain-gui
//! pattern, the plugin never writes the shared store directly from host
//! input events; each processing thread keeps a local mirror:
//!
//! 1. [`fetch_ui_changes`](LocalParams::fetch_ui_changes) pulls UI-originated
//!    writes out of the shared store (flagging them for echo to the host).
//!    For E002 the UI write path doesn't exist yet, so the diff finds
//!    nothing — but the loop runs so 0015 / 0016 plug in to the final shape.
//! 2. [`apply_input`](LocalParams::apply_input) folds a host `ParamValue`
//!    event into the mirror and reports `(idx, value)` so the caller can
//!    drive the engine immediately.
//! 3. [`write_to`](LocalParams::write_to) pushes the whole mirror into the
//!    engine's working [`EngineParams`] at the top of each block.
//! 4. [`publish`](LocalParams::publish) writes host-changed slots back to the
//!    shared store so `get_value` reflects automation. Only host-flagged
//!    slots are written — a concurrent UI bulk write (preset load) landing
//!    between this block's `fetch_ui_changes` and `publish` survives.
//! 5. [`emit`](LocalParams::emit) sends UI edits back to the host as
//!    `ParamValue` events. Gesture brackets land with the UI epic; for E002
//!    this walks `ui_changed` (always empty) and emits nothing.

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::ParamValueEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::utils::Cookie;

use vxn2_engine::params::PARAMS;
use vxn2_engine::{
    EngineParams, MatrixRowRaw, N_MATRIX_SLOTS, ParamView, SharedParams, TOTAL_PARAMS,
};

/// Per-thread parameter mirror. Lives on the audio thread alongside the
/// engine; carries the same flat shape as [`SharedParams`] but in plain
/// `f32`s — no atomics, no contention.
pub struct LocalParams {
    /// Working values (plain units). Authoritative for this block.
    values: [f32; TOTAL_PARAMS],
    /// Params changed by the UI since the last [`emit`](Self::emit).
    ui_changed: [bool; TOTAL_PARAMS],
    /// Params changed by host automation since the last
    /// [`publish`](Self::publish). Re-publishing the whole mirror would race
    /// concurrent UI bulk writes (a preset load) and silently revert them;
    /// the per-slot flag keeps `publish` to the slots host events touched.
    host_changed: [bool; TOTAL_PARAMS],
    /// Mirror of the shared store's mod-matrix topology + slot 9-16 depths.
    /// Refreshed by [`fetch_ui_changes`] and exposed to the engine through
    /// [`ParamView::matrix_row_raw`] so block-time snapshots see the latest
    /// UI / preset edits. Matrix meta isn't CLAP-automatable, so there's no
    /// host→shared publish path for it.
    matrix_rows: [MatrixRowRaw; N_MATRIX_SLOTS],
}

impl LocalParams {
    pub fn new(shared: &SharedParams) -> Self {
        Self {
            values: std::array::from_fn(|i| shared.get(i)),
            ui_changed: [false; TOTAL_PARAMS],
            host_changed: [false; TOTAL_PARAMS],
            matrix_rows: std::array::from_fn(|s| shared.matrix_row_raw(s)),
        }
    }

    /// Pull UI-originated writes from `shared` into the mirror, flagging them
    /// for echo to the host. Returns whether anything changed.
    ///
    /// Also refreshes the mod-matrix topology mirror — matrix meta isn't
    /// CLAP-automatable, but UI / preset edits still need to reach the audio
    /// thread before the next `write_to`. Topology drift never sets
    /// `ui_changed`; there's no host CLAP id to echo to.
    pub fn fetch_ui_changes(&mut self, shared: &SharedParams) -> bool {
        let mut any = false;
        for i in 0..TOTAL_PARAMS {
            let sv = shared.get(i);
            if sv != self.values[i] {
                self.values[i] = sv;
                self.ui_changed[i] = true;
                any = true;
            }
        }
        for s in 0..N_MATRIX_SLOTS {
            let row = shared.matrix_row_raw(s);
            if row != self.matrix_rows[s] {
                self.matrix_rows[s] = row;
                any = true;
            }
        }
        any
    }

    /// Fold a host param-value input event into the mirror. Returns
    /// `(idx, value)` so the caller can forward to the engine. Not flagged
    /// as a UI change — never echoed back to the host.
    pub fn apply_input(&mut self, event: &UnknownEvent) -> Option<(usize, f32)> {
        if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
            if let Some(pid) = e.param_id() {
                let i = pid.get() as usize;
                if i < TOTAL_PARAMS {
                    let v = e.value() as f32;
                    self.values[i] = v;
                    self.host_changed[i] = true;
                    return Some((i, v));
                }
            }
        }
        None
    }

    /// Push the whole mirror into `engine`'s working [`EngineParams`]. The
    /// engine's smoothers absorb the per-block refresh; no allocation.
    pub fn write_to(&self, engine: &mut EngineParams) {
        engine.snapshot_from(self);
    }

    /// Publish host-automation changes to `shared`. Only slots flagged by
    /// [`apply_input`](Self::apply_input) this block are written (then
    /// cleared). Re-publishing the whole mirror would race concurrent UI
    /// writes — see the type-level doc.
    pub fn publish(&mut self, shared: &SharedParams) {
        for i in 0..TOTAL_PARAMS {
            if self.host_changed[i] {
                shared.set(i, self.values[i]);
                self.host_changed[i] = false;
            }
        }
    }

    /// Emit UI-originated changes to the host. For E002 the UI write path
    /// doesn't exist, so `ui_changed` is always empty and nothing is pushed.
    /// `frame_count` reserves the gesture-end sample offset for the UI
    /// epic; unused now.
    pub fn emit(
        &mut self,
        _shared: &SharedParams,
        out: &mut OutputEvents,
        _frame_count: u32,
    ) {
        for i in 0..TOTAL_PARAMS {
            if !self.ui_changed[i] {
                continue;
            }
            self.ui_changed[i] = false;
            let id = ClapId::new(i as u32);
            let _ = out.try_push(ParamValueEvent::new(
                0,
                id,
                Pckn::match_all(),
                self.values[i] as f64,
                Cookie::empty(),
            ));
        }
    }
}

impl ParamView for LocalParams {
    #[inline]
    fn get(&self, id: usize) -> f32 {
        if id < TOTAL_PARAMS {
            self.values[id]
        } else {
            0.0
        }
    }

    #[inline]
    fn matrix_row_raw(&self, slot: usize) -> MatrixRowRaw {
        if slot < N_MATRIX_SLOTS {
            self.matrix_rows[slot]
        } else {
            MatrixRowRaw::default()
        }
    }
}

/// Normalised read against the mirror — useful where 0015 / 0017 need
/// `[0, 1]` against the mirror rather than the atomic store.
impl LocalParams {
    pub fn get(&self, id: usize) -> f32 {
        ParamView::get(self, id)
    }

    pub fn get_normalised(&self, id: usize) -> f32 {
        if id < TOTAL_PARAMS {
            PARAMS[id].to_normalised(self.values[id])
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn2_engine::params::id_of;

    /// A sequence of host-event applies followed by `publish` lands the
    /// last-written value for each touched id in the shared store; untouched
    /// ids stay at their default.
    #[test]
    fn apply_then_publish_writes_last_value_per_id() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);

        // Two slots get a sequence of writes; the last value should win.
        let vol = id_of("master-volume").unwrap();
        let decay = id_of("reverb-decay").unwrap();
        let untouched = id_of("master-tune").unwrap();
        let untouched_default = shared.get(untouched);

        for v in [-12.0_f32, -8.0, -4.5] {
            local.values[vol] = v;
            local.host_changed[vol] = true;
        }
        for v in [1.0_f32, 3.25] {
            local.values[decay] = v;
            local.host_changed[decay] = true;
        }
        local.publish(&shared);

        assert_eq!(shared.get(vol), -4.5);
        assert_eq!(shared.get(decay), 3.25);
        assert_eq!(shared.get(untouched), untouched_default);
    }

    /// `publish` must only write host-flagged slots — a UI bulk write that
    /// lands in the window between `fetch_ui_changes` and `publish` survives.
    /// (Regression mirror of vxn1's 0027.)
    #[test]
    fn publish_does_not_clobber_concurrent_ui_writes() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("delay-time").unwrap();

        // No host automation this block; UI writes the shared store after
        // the mirror was built (the preset-load race window).
        let loaded = shared.get(id) + 123.0;
        shared.set(id, loaded);

        local.publish(&shared);
        assert_eq!(shared.get(id), loaded);

        // Next `fetch_ui_changes` folds the UI value into the mirror.
        assert!(local.fetch_ui_changes(&shared));
        assert_eq!(local.get(id), loaded);
    }

    /// E002 stub: no UI write path, so `fetch_ui_changes` returns false on a
    /// freshly-built mirror.
    #[test]
    fn fetch_ui_changes_is_false_with_no_ui_writes() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        assert!(!local.fetch_ui_changes(&shared));
    }

    /// `write_to` pushes the mirror through the same section readers as
    /// `EngineParams::snapshot_from(&SharedParams)`, so a mirror diverged
    /// from the store still drives the engine off the mirror's values.
    #[test]
    fn write_to_uses_mirror_not_shared() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);

        // Diverge the mirror: pretend a host event came through.
        let vol = id_of("master-volume").unwrap();
        let op3_num = id_of("op3-num").unwrap();
        local.values[vol] = 0.0;
        local.values[op3_num] = 5.0;

        // Shared store still at defaults.
        let mut engine = EngineParams::default();
        local.write_to(&mut engine);

        assert!((engine.master.volume_db - 0.0).abs() < 1e-6);
        assert_eq!(engine.patch.voice.ops[2].num, 5);
    }
}
