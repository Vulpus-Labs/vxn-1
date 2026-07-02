//! Generic CLAP test-support helpers (feature = "test-support").
//!
//! Provides synth-agnostic scaffolding for unit tests (in `src/*.rs`
//! `#[cfg(test)]` modules) and integration tests (in `tests/*.rs`) that
//! exercise the param-event ritual:
//!
//! - [`push_param_event`] — push one `ParamValueEvent` into an `EventBuffer`.
//! - [`event_log`] — decode an `EventBuffer` into a compact `(kind, id,
//!   time)` tuple stream; "kind" is one of `"begin"` / `"value"` / `"end"`.
//!
//! Both helpers are re-exported by `vxn-core-clap` when the
//! `test-support` feature is active. Neither touches production code
//! paths; the feature is off by default.

use clack_plugin::events::event_types::{
    ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::events::io::EventBuffer;
use clack_plugin::events::Pckn;
use clack_plugin::prelude::{ClapId, Event};
use clack_plugin::utils::Cookie;

/// Push a single `ParamValueEvent` for `id` (CLAP index) with `value` at
/// sample offset 0 into `buf`.
///
/// The helper exists so every test file that sets up host-automation events
/// uses the same shape rather than open-coding the `ParamValueEvent::new`
/// call. It is deliberately thin — no assertions on range, no side effects.
pub fn push_param_event(buf: &mut EventBuffer, id: usize, value: f32) {
    buf.push(&ParamValueEvent::new(
        0,
        ClapId::new(id as u32),
        Pckn::match_all(),
        value as f64,
        Cookie::empty(),
    ));
}

/// Decode all param-related events in `buf` into `(kind, param_id, time)`
/// tuples.
///
/// `kind` is one of `"begin"` / `"value"` / `"end"`, matching
/// `ParamGestureBegin`, `ParamValue`, and `ParamGestureEnd` respectively.
/// Events of other types are silently skipped.
///
/// The returned vec preserves the insertion order inside `buf`, allowing
/// tests to assert on both the sequence and the per-param bracketing
/// without parsing raw event headers by hand.
///
/// ## Why `as_event` rather than `as_core_event`
///
/// The pinned clack revision's `CoreEventSpace::from_unknown` match table
/// does not map the two gesture `TYPE_ID` arms (the enum variants exist; the
/// decoder never produces them). The typed `as_event::<T>` accessor reads
/// the raw CLAP header type id, so it works correctly regardless.
pub fn event_log(buf: &EventBuffer) -> Vec<(&'static str, u32, u32)> {
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
