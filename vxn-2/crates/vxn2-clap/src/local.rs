//! Audio-thread parameter mirror (ticket 0014, dirty-bit refactor 0056).
//!
//! Bridges the host (CLAP automation), the audio-thread engine, and the
//! UI / state-load write path. Each processing thread keeps a local
//! mirror so block-time reads stay branch-free:
//!
//! 1. [`fetch_ui_changes`](LocalParams::fetch_ui_changes) pulls UI- /
//!    preset-originated writes out of the shared store into the mirror,
//!    flagging them for echo to the host.
//! 2. [`apply_input`](LocalParams::apply_input) folds a host `ParamValue`
//!    event into the mirror AND writes it through to `SharedParams`
//!    immediately. The shared write flips a dirty bit (ADR 0003), which
//!    the main-thread tick drains into a `ParamChanged` view event on
//!    the next pass — no `host_changed` flag, no deferred publish.
//! 3. [`write_to`](LocalParams::write_to) pushes the whole mirror into
//!    the engine's working [`EngineParams`] at the top of each block.
//! 4. [`emit`](LocalParams::emit) sends UI edits back to the host as
//!    `ParamValue` events bracketed by `ParamGestureBegin` / `End`
//!    (driven by the `SharedParams.gestures` bitset the controller
//!    populates — ticket 0065). `ui_changed` survives — different
//!    consumer (plugin → host echo); ADR 0003 §"What survives".

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::{
    ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::utils::Cookie;

use vxn2_engine::params::PARAMS;
use vxn2_engine::{
    EngineParams, KsCurve, MatrixRowRaw, N_KS_CURVES, N_MATRIX_SLOTS, ParamView, SharedParams,
    TOTAL_PARAMS,
};

/// Per-thread parameter mirror. Lives on the audio thread alongside the
/// engine; carries the same flat shape as [`SharedParams`] but in plain
/// `f32`s — no atomics, no contention.
pub struct LocalParams {
    /// Working values (plain units). Authoritative for this block.
    values: [f32; TOTAL_PARAMS],
    /// Params changed by the UI since the last [`emit`](Self::emit).
    /// Drives the plugin → host echo only; Model → View notification
    /// rides the shared store's dirty bitset (ADR 0003).
    ui_changed: [bool; TOTAL_PARAMS],
    /// Mirror of the shared store's mod-matrix topology + slot 9-16 depths.
    /// Refreshed by [`fetch_ui_changes`] and exposed to the engine through
    /// [`ParamView::matrix_row_raw`] so block-time snapshots see the latest
    /// UI / preset edits. Matrix meta isn't CLAP-automatable, so there's no
    /// host→shared publish path for it.
    matrix_rows: [MatrixRowRaw; N_MATRIX_SLOTS],
    /// Mirror of the shared store's per-side KS level-curve selectors,
    /// indexed `op * 2 + side`. Non-CLAP like the matrix topology; refreshed
    /// by [`fetch_ui_changes`] and read by the engine through
    /// [`ParamView::ks_curve`] so block-time snapshots see UI / preset edits.
    ks_curves: [KsCurve; N_KS_CURVES],
    /// Last-seen UI gesture state per param. [`emit`](Self::emit) compares
    /// against `SharedParams.gestures` (populated by the controller on
    /// `BeginGesture` / `EndGesture` UI intents) to push CLAP
    /// `param_gesture_begin` / `param_gesture_end` brackets on the 0→1 / 1→0
    /// transitions (ticket 0065; same pattern as VXN-1's `vxn-clap`).
    gesture: [bool; TOTAL_PARAMS],
}

impl LocalParams {
    pub fn new(shared: &SharedParams) -> Self {
        Self {
            values: std::array::from_fn(|i| shared.get(i)),
            ui_changed: [false; TOTAL_PARAMS],
            matrix_rows: std::array::from_fn(|s| shared.matrix_row_raw(s)),
            ks_curves: std::array::from_fn(|k| ParamView::ks_curve(shared, k / 2, k % 2)),
            gesture: [false; TOTAL_PARAMS],
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
        for k in 0..N_KS_CURVES {
            let curve = ParamView::ks_curve(shared, k / 2, k % 2);
            if curve != self.ks_curves[k] {
                self.ks_curves[k] = curve;
                any = true;
            }
        }
        any
    }

    /// Fold a host param-value input event into the mirror AND write it
    /// through to `shared`. The shared write clamps and flips the matching
    /// dirty bit so the main-thread tick observes the automation without
    /// a deferred `publish` pass.
    ///
    /// The mirror takes the clamped value (reading back from the shared
    /// store after the write) so the mirror and the shared store stay in
    /// lockstep — otherwise an out-of-range host event would leave the
    /// mirror with the raw value and `fetch_ui_changes` on the next block
    /// would flag a spurious UI-side drift.
    pub fn apply_input(
        &mut self,
        shared: &SharedParams,
        event: &UnknownEvent,
    ) -> Option<(usize, f32)> {
        if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
            if let Some(pid) = e.param_id() {
                let i = pid.get() as usize;
                if i < TOTAL_PARAMS {
                    shared.set(i, e.value() as f32);
                    let clamped = shared.get(i);
                    self.values[i] = clamped;
                    return Some((i, clamped));
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

    /// Emit UI-originated changes to the host, bracketed by CLAP gesture
    /// events (ticket 0065). Per id and block:
    ///
    /// - gesture bit 0→1: push `ParamGestureBeginEvent` before any value;
    /// - `ui_changed` (flagged by [`fetch_ui_changes`](Self::fetch_ui_changes)
    ///   for UI / preset writes): push one `ParamValueEvent`;
    /// - gesture bit 1→0: push `ParamGestureEndEvent` after the last value,
    ///   at sample offset `frame_count.saturating_sub(1)` so the bracket
    ///   closes at the end of the block that released it;
    /// - a bare value change with no surrounding gesture (preset load,
    ///   text-entry) gets wrapped in its own begin/end pair so conformant
    ///   hosts record it as a single point edit.
    ///
    /// Host-driven automation never lands here: [`apply_input`](Self::apply_input)
    /// touches neither `ui_changed` nor the gesture bitset, so the host's
    /// own events are not echoed back wrapped in brackets. Gesture state is
    /// read from `SharedParams.gestures` with lock-free atomic loads; no
    /// allocation on this path.
    pub fn emit(
        &mut self,
        shared: &SharedParams,
        out: &mut OutputEvents,
        frame_count: u32,
    ) {
        let end_time = frame_count.saturating_sub(1);
        for i in 0..TOTAL_PARAMS {
            let prev = self.gesture[i];
            let cur = shared.gesture(i);
            self.gesture[i] = cur;
            let changed = self.ui_changed[i];
            self.ui_changed[i] = false;

            if !changed && cur == prev {
                continue;
            }
            // A held gesture brackets a burst of values; a bare value change
            // (no sustained gesture) is wrapped in its own begin/end.
            let bare = changed && !cur && !prev;
            let begin = (cur && !prev) || bare;
            let end = (!cur && prev) || bare;
            let id = ClapId::new(i as u32);
            if begin {
                let _ = out.try_push(ParamGestureBeginEvent::new(0, id));
            }
            if changed {
                let _ = out.try_push(ParamValueEvent::new(
                    0,
                    id,
                    Pckn::match_all(),
                    self.values[i] as f64,
                    Cookie::empty(),
                ));
            }
            if end {
                let _ = out.try_push(ParamGestureEndEvent::new(end_time, id));
            }
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

    #[inline]
    fn ks_curve(&self, op: usize, side: usize) -> KsCurve {
        let k = op * 2 + side;
        if k < N_KS_CURVES {
            self.ks_curves[k]
        } else if side == 0 {
            KsCurve::NegLin
        } else {
            KsCurve::NegExp
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
    use clack_plugin::events::io::EventBuffer;
    use vxn2_engine::params::id_of;

    fn push_param_event(buf: &mut EventBuffer, id: usize, value: f32) {
        buf.push(&ParamValueEvent::new(
            0,
            ClapId::new(id as u32),
            Pckn::match_all(),
            value as f64,
            Cookie::empty(),
        ));
    }

    /// `apply_input` writes through to the shared store on every event —
    /// the last-written value lands per id without any deferred publish.
    #[test]
    fn apply_input_writes_through_each_event() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);

        let vol = id_of("master-volume").unwrap();
        let decay = id_of("reverb-decay").unwrap();
        let untouched = id_of("master-tune").unwrap();
        let untouched_default = shared.get(untouched);

        let mut buf = EventBuffer::with_capacity(8);
        for v in [-12.0_f32, -8.0, -4.5] {
            push_param_event(&mut buf, vol, v);
        }
        for v in [1.0_f32, 3.25] {
            push_param_event(&mut buf, decay, v);
        }
        for ev in buf.iter() {
            let _ = local.apply_input(&shared, ev);
        }

        assert_eq!(shared.get(vol), -4.5);
        assert_eq!(shared.get(decay), 3.25);
        assert_eq!(shared.get(untouched), untouched_default);
        // Mirror tracks the shared store in lockstep.
        assert_eq!(local.get(vol), -4.5);
        assert_eq!(local.get(decay), 3.25);
    }

    /// `apply_input` flips the matching dirty bit on the shared store —
    /// the main-thread tick will observe host automation without a
    /// separate notify path.
    #[test]
    fn apply_input_flips_dirty_bit() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        // Drain the all-ones seed so we observe only this write.
        let _ = shared.take_dirty_values();

        let decay = id_of("reverb-decay").unwrap();
        let mut buf = EventBuffer::with_capacity(1);
        push_param_event(&mut buf, decay, 4.5);
        for ev in buf.iter() {
            let _ = local.apply_input(&shared, ev);
        }

        let bits = shared.take_dirty_values();
        assert!(bits[decay / 64] & (1u64 << (decay % 64)) != 0);
    }

    /// A UI / preset write that lands after `fetch_ui_changes` is folded
    /// in on the next block. The audio thread never overwrites it because
    /// there's no deferred publish — `apply_input` only touches the ids
    /// the host event names. Regression mirror of vxn1's 0027.
    #[test]
    fn fetch_ui_changes_picks_up_concurrent_ui_write() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("delay-time").unwrap();

        // UI writes the shared store after the mirror was built (the
        // preset-load race window).
        let loaded = shared.get(id) + 123.0;
        shared.set(id, loaded);

        // Next `fetch_ui_changes` folds the UI value into the mirror.
        assert!(local.fetch_ui_changes(&shared));
        assert_eq!(local.get(id), loaded);
        assert_eq!(shared.get(id), loaded);
    }

    /// Freshly-built mirror against an untouched shared store: no drift,
    /// `fetch_ui_changes` returns false.
    #[test]
    fn fetch_ui_changes_is_false_with_no_ui_writes() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        assert!(!local.fetch_ui_changes(&shared));
    }

    /// Render the event buffer as compact `(kind, id, time)` tuples for
    /// order assertions. Kind: "begin" / "value" / "end".
    ///
    /// Decodes via typed [`UnknownEvent::as_event`] rather than
    /// `as_core_event()` — the pinned clack rev's `CoreEventSpace::
    /// from_unknown` match table is missing the two gesture TYPE_ID arms
    /// (the enum variants exist; the decoder never produces them). The wire
    /// events are spec-correct regardless — real hosts parse the raw CLAP
    /// structs.
    fn event_log(buf: &EventBuffer) -> Vec<(&'static str, u32, u32)> {
        buf.iter()
            .filter_map(|ev| {
                if let Some(e) = ev.as_event::<ParamGestureBeginEvent>() {
                    Some(("begin", e.param_id().unwrap().get(), e.header().time()))
                } else if let Some(e) = ev.as_event::<ParamValueEvent>() {
                    Some(("value", e.param_id().unwrap().get(), e.header().time()))
                } else if let Some(e) = ev.as_event::<ParamGestureEndEvent>() {
                    Some(("end", e.param_id().unwrap().get(), e.header().time()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// A UI drag emits, in order: `gesture_begin` → `value`×N → `gesture_end`
    /// across the blocks the drag spans (ticket 0065). The end lands at the
    /// last sample of its block.
    #[test]
    fn emit_brackets_ui_drag_with_gesture_events() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("reverb-decay").unwrap();

        // Block 1: controller saw BeginGesture + first drag value.
        shared.set_gesture(id, true);
        shared.set(id, 3.0);
        local.fetch_ui_changes(&shared);
        let mut b1 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b1.as_output(), 256);
        assert_eq!(
            event_log(&b1),
            vec![("begin", id as u32, 0), ("value", id as u32, 0)]
        );

        // Block 2: drag continues — value only, no new bracket.
        shared.set(id, 4.0);
        local.fetch_ui_changes(&shared);
        let mut b2 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b2.as_output(), 256);
        assert_eq!(event_log(&b2), vec![("value", id as u32, 0)]);

        // Block 3: EndGesture, no further value change.
        shared.set_gesture(id, false);
        local.fetch_ui_changes(&shared);
        let mut b3 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b3.as_output(), 256);
        assert_eq!(event_log(&b3), vec![("end", id as u32, 255)]);

        // Block 4: silence.
        let mut b4 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b4.as_output(), 256);
        assert!(event_log(&b4).is_empty());
    }

    /// A bare UI value change (preset load, text entry — no sustained
    /// gesture) is wrapped in its own begin/end pair.
    #[test]
    fn emit_wraps_bare_value_change_in_begin_end() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("master-volume").unwrap();

        shared.set(id, -3.0);
        local.fetch_ui_changes(&shared);
        let mut buf = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut buf.as_output(), 128);
        assert_eq!(
            event_log(&buf),
            vec![
                ("begin", id as u32, 0),
                ("value", id as u32, 0),
                ("end", id as u32, 127),
            ]
        );
    }

    /// Host-driven automation must not echo back wrapped in brackets:
    /// `apply_input` touches neither `ui_changed` nor the gesture bitset.
    #[test]
    fn emit_is_silent_for_host_driven_automation() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("delay-feedback").unwrap();

        let mut input = EventBuffer::with_capacity(1);
        push_param_event(&mut input, id, 0.8);
        for ev in input.iter() {
            let _ = local.apply_input(&shared, ev);
        }
        let mut out = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut out.as_output(), 256);
        assert!(
            event_log(&out).is_empty(),
            "host automation echoed back: {:?}",
            event_log(&out)
        );
    }

    /// A gesture bracket with no value change still emits begin and end —
    /// the host needs the bracket even if the knob never left its value
    /// (touch automation).
    #[test]
    fn emit_brackets_gesture_without_value_change() {
        let shared = SharedParams::new();
        let mut local = LocalParams::new(&shared);
        let id = id_of("reverb-size").unwrap();

        shared.set_gesture(id, true);
        let mut b1 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b1.as_output(), 64);
        assert_eq!(event_log(&b1), vec![("begin", id as u32, 0)]);

        shared.set_gesture(id, false);
        let mut b2 = EventBuffer::with_capacity(4);
        local.emit(&shared, &mut b2.as_output(), 64);
        assert_eq!(event_log(&b2), vec![("end", id as u32, 63)]);
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
