//! Per-thread parameter mirror used to bridge the engine, the host and the UI.
//!
//! Following clack's gain-gui pattern, the plugin never writes the shared store
//! directly from host input events. Instead each processing thread keeps a
//! local mirror:
//!
//! 1. [`fetch_ui_changes`](LocalParams::fetch_ui_changes) pulls UI-originated
//!    writes out of the shared store (flagging them for echo to the host).
//! 2. [`apply_input`](LocalParams::apply_input) folds host automation events
//!    into the mirror (and reports them so the engine can be updated).
//! 3. [`publish`](LocalParams::publish) writes the mirror back to the shared
//!    store so the host (`get_value`) and the UI observe host-side changes.
//! 4. [`emit`](LocalParams::emit) sends UI edits back to the host, each wrapped
//!    in a CLAP gesture begin/end so automation recording and undo coalesce.
//!
//! Because the shared store only ever changes via the UI or via `publish`, the
//! `fetch_ui_changes` diff sees *only* UI edits — host automation is never
//! echoed back to the host (no feedback loop).

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::{
    ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::utils::Cookie;
use vxn_engine::{ParamValues, SharedParams, TOTAL_PARAMS};

pub struct LocalParams {
    /// Working values (plain units), the authoritative set for this thread.
    values: [f32; TOTAL_PARAMS],
    /// Last-seen UI gesture state per param (to detect begin/end transitions).
    gesture: [bool; TOTAL_PARAMS],
    /// Params changed by the UI since the last [`emit`](Self::emit).
    ui_changed: [bool; TOTAL_PARAMS],
}

impl LocalParams {
    pub fn new(shared: &SharedParams) -> Self {
        Self {
            values: std::array::from_fn(|i| shared.get(i)),
            gesture: [false; TOTAL_PARAMS],
            ui_changed: [false; TOTAL_PARAMS],
        }
    }

    /// Pull UI-originated writes from `shared` into the mirror, flagging them
    /// for echo to the host. Returns whether anything changed.
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
        any
    }

    /// Fold a host param-value input event into the mirror. Returns the
    /// `(index, value)` so the caller can forward it to the engine. Not flagged
    /// as a UI change, so it is never echoed back to the host.
    pub fn apply_input(&mut self, event: &UnknownEvent) -> Option<(usize, f32)> {
        if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
            if let Some(pid) = e.param_id() {
                let i = pid.get() as usize;
                if i < TOTAL_PARAMS {
                    let v = e.value() as f32;
                    self.values[i] = v;
                    return Some((i, v));
                }
            }
        }
        None
    }

    /// Copy the working values into the engine's parameter table.
    pub fn write_to(&self, params: &mut ParamValues) {
        for (i, &v) in self.values.iter().enumerate() {
            params.set_by_clap_id(i, v);
        }
    }

    /// Publish the working values to `shared` so the host and UI observe
    /// host-side automation changes.
    pub fn publish(&self, shared: &SharedParams) {
        for (i, &v) in self.values.iter().enumerate() {
            shared.set(i, v);
        }
    }

    /// Emit UI-originated changes to the host, each bracketed by a gesture
    /// begin/end. `end_time` is the sample offset for the closing gesture.
    pub fn emit(&mut self, shared: &SharedParams, out: &mut OutputEvents, end_time: u32) {
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
            // (no sustained gesture) is wrapped in its own begin/end ("Both").
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
