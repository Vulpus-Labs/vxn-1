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
//! 5. [`LocalParams::emit`] echoes UI edits back to the host as
//!    `ParamValue` events, each bracketed by a CLAP gesture begin/end
//!    (read from the store's gesture flag) so automation recording and
//!    undo coalesce a drag into one edit.

use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;

use crate::engine::SharedStore;
use crate::gesture::{emit_gesture_begin, emit_gesture_end, emit_param_value};

/// Decide gesture-bracket emission for one param this block from whether
/// its value changed (a UI edit) and the current/previous live-gesture
/// flags. Returns `(begin, emit_value, end)`.
///
/// A held gesture (`cur`) brackets a burst of value edits; a bare value
/// change with no sustained gesture (`changed && !cur && !prev`) is
/// self-bracketed — its own begin *and* end in the same block ("Both"),
/// so a single programmatic set still records as one automation edit.
#[inline]
fn bracket(changed: bool, cur: bool, prev: bool) -> (bool, bool, bool) {
    let bare = changed && !cur && !prev;
    let begin = (cur && !prev) || bare;
    let end = (!cur && prev) || bare;
    (begin, changed, end)
}

pub struct LocalParams<const N: usize> {
    values: [f32; N],
    /// Last-seen UI gesture flag per param, to detect begin/end edges.
    gesture: [bool; N],
    ui_changed: [bool; N],
    host_changed: [bool; N],
}

impl<const N: usize> LocalParams<N> {
    /// Seed the mirror from `shared`'s current values.
    pub fn new<S: SharedStore>(shared: &S) -> Self {
        Self {
            values: std::array::from_fn(|i| shared.get(i)),
            gesture: [false; N],
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

    /// Echo UI-originated changes to the host, each wrapped in a CLAP
    /// gesture begin/end so automation recording and undo coalesce a drag
    /// into one edit. The live gesture flag is read per param from
    /// `shared` ([`SharedStore::gesture`]); `end_time` is the sample offset
    /// for the closing `ParamGestureEnd` (the begin + value land at offset
    /// 0). A sustained drag brackets its burst of values; a bare value
    /// change with no held gesture is self-bracketed (see [`bracket`]).
    pub fn emit<S: SharedStore>(
        &mut self,
        shared: &S,
        out: &mut OutputEvents<'_>,
        end_time: u32,
    ) {
        for i in 0..N {
            let prev = self.gesture[i];
            let cur = shared.gesture(i);
            self.gesture[i] = cur;
            let changed = self.ui_changed[i];
            self.ui_changed[i] = false;

            if !changed && cur == prev {
                continue;
            }
            let (begin, emit_value, end) = bracket(changed, cur, prev);
            let id = i as u32;
            if begin {
                emit_gesture_begin(out, id, 0);
            }
            if emit_value {
                emit_param_value(out, id, self.values[i] as f64, 0);
            }
            if end {
                emit_gesture_end(out, id, end_time);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clack_plugin::events::event_types::{
        ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
    };
    use clack_plugin::events::io::EventBuffer;
    use clack_plugin::events::{Event, Pckn};
    use clack_plugin::prelude::ClapId;
    use clack_plugin::utils::Cookie;
    use std::cell::RefCell;

    const N: usize = 4;

    /// In-memory `SharedStore` for the generic's tests: `get`/`set` over a
    /// `RefCell` array plus a settable per-param gesture flag.
    struct MockStore {
        values: RefCell<[f32; N]>,
        gesture: RefCell<[bool; N]>,
    }
    // The trait requires Send + Sync; the tests are single-threaded, so a
    // RefCell is fine — assert Sync via an explicit unsafe impl scoped to
    // the test store only.
    unsafe impl Sync for MockStore {}
    impl MockStore {
        fn new() -> Self {
            Self {
                values: RefCell::new([0.0; N]),
                gesture: RefCell::new([false; N]),
            }
        }
        fn set_gesture(&self, id: usize, on: bool) {
            self.gesture.borrow_mut()[id] = on;
        }
    }
    impl SharedStore for MockStore {
        fn get(&self, id: usize) -> f32 {
            self.values.borrow()[id]
        }
        fn set(&self, id: usize, value: f32) {
            self.values.borrow_mut()[id] = value;
        }
        fn gesture(&self, id: usize) -> bool {
            self.gesture.borrow()[id]
        }
    }

    /// One ParamValue host-automation event for `id`.
    fn host_event(buf: &mut EventBuffer, id: u32, value: f64) {
        buf.push(&ParamValueEvent::new(
            0,
            ClapId::new(id),
            Pckn::match_all(),
            value,
            Cookie::empty(),
        ));
    }

    /// Tag stream of emitted events, in order: 'b' begin, 'v' value, 'e' end.
    fn emit_tags<const M: usize>(lp: &mut LocalParams<M>, store: &MockStore, end_time: u32) -> String {
        let mut out = EventBuffer::with_capacity(16);
        lp.emit(store, &mut out.as_output(), end_time);
        // clack's `CoreEventSpace::from_unknown` doesn't map the gesture event
        // types (they fall through to None), so match on the raw header type id.
        let mut s = String::new();
        for ev in out.iter() {
            let t = ev.header().type_id();
            if t == ParamGestureBeginEvent::TYPE_ID {
                s.push('b');
            } else if t == ParamValueEvent::TYPE_ID {
                s.push('v');
            } else if t == ParamGestureEndEvent::TYPE_ID {
                s.push('e');
            } else {
                s.push('?');
            }
        }
        s
    }

    #[test]
    fn bracket_decisions() {
        // (changed, cur, prev) → (begin, emit_value, end)
        assert_eq!(bracket(true, true, false), (true, true, false)); // drag start
        assert_eq!(bracket(true, true, true), (false, true, false)); // mid-drag value
        assert_eq!(bracket(false, false, true), (false, false, true)); // drag release
        assert_eq!(bracket(true, false, false), (true, true, true)); // bare set: both
        assert_eq!(bracket(false, true, true), (false, false, false)); // held, no edit
    }

    #[test]
    fn emit_brackets_a_sustained_drag() {
        let store = MockStore::new();
        let mut lp = LocalParams::<N>::new(&store);

        // Gesture begins, first value lands this block → begin + value.
        store.set_gesture(0, true);
        store.set(0, 0.2);
        lp.fetch_ui_changes(&store);
        assert_eq!(emit_tags(&mut lp, &store, 0), "bv");

        // Mid-drag value, gesture still held → value only (no new begin/end).
        store.set(0, 0.4);
        lp.fetch_ui_changes(&store);
        assert_eq!(emit_tags(&mut lp, &store, 0), "v");

        // Release with a final value → value + end.
        store.set(0, 0.6);
        store.set_gesture(0, false);
        lp.fetch_ui_changes(&store);
        assert_eq!(emit_tags(&mut lp, &store, 7), "ve");

        // Quiescent block emits nothing.
        assert_eq!(emit_tags(&mut lp, &store, 0), "");
    }

    #[test]
    fn emit_self_brackets_a_bare_transient_set() {
        let store = MockStore::new();
        let mut lp = LocalParams::<N>::new(&store);
        // A value change with no sustained gesture this block → begin, value, end.
        store.set(1, 0.9);
        lp.fetch_ui_changes(&store);
        assert_eq!(emit_tags(&mut lp, &store, 5), "bve");
    }

    #[test]
    fn host_automation_does_not_echo_and_publishes_once() {
        let store = MockStore::new();
        let mut lp = LocalParams::<N>::new(&store);

        // Host automation event folds into the mirror but is NOT a UI change.
        let mut inbuf = EventBuffer::with_capacity(2);
        host_event(&mut inbuf, 2, 0.5);
        for ev in inbuf.iter() {
            assert_eq!(lp.apply_input(ev), Some((2, 0.5)));
        }
        // Not echoed to the host (no ui_changed flag).
        assert_eq!(emit_tags(&mut lp, &store, 0), "");
        assert!(lp.host_changed(2));

        // Publish writes the host change to the store once, then clears.
        lp.publish(&store);
        assert_eq!(store.get(2), 0.5);
        assert!(!lp.host_changed(2));

        // A later UI write to the same param is not clobbered by a second publish.
        store.set(2, 0.8);
        lp.publish(&store);
        assert_eq!(store.get(2), 0.8);
    }

    #[test]
    fn publish_does_not_clobber_concurrent_ui_write() {
        // A UI bulk write landing between fetch and publish must survive:
        // publish only writes host-flagged slots (regression 0027).
        let store = MockStore::new();
        let mut lp = LocalParams::<N>::new(&store);
        store.set(3, 12.0); // UI write after the mirror was built
        lp.publish(&store); // no host change → must not revert
        assert_eq!(store.get(3), 12.0);
        // Next fetch folds it into the mirror.
        assert!(lp.fetch_ui_changes(&store));
        assert_eq!(lp.value_at(3), 12.0);
    }
}
