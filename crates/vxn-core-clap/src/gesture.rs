//! Outbound gesture-bracket emit helpers.
//!
//! UI-originated parameter writes need to be reported to the host as
//! gesture-bracketed [`ParamValueEvent`]s so DAW automation records the
//! drag as a single edit. The audio thread calls these from inside its
//! `process()` after the main thread queued the UI writes.

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::{
    ParamGestureBeginEvent, ParamGestureEndEvent, ParamValueEvent,
};
use clack_plugin::prelude::{ClapId, OutputEvents};
use clack_plugin::utils::Cookie;

/// Push a `ParamGestureBegin` at sample offset `time`.
#[inline]
pub fn emit_gesture_begin(out: &mut OutputEvents<'_>, id: u32, time: u32) {
    let _ = out.try_push(ParamGestureBeginEvent::new(time, ClapId::new(id)));
}

/// Push a `ParamGestureEnd` at sample offset `time`.
#[inline]
pub fn emit_gesture_end(out: &mut OutputEvents<'_>, id: u32, time: u32) {
    let _ = out.try_push(ParamGestureEndEvent::new(time, ClapId::new(id)));
}

/// Push a `ParamValue` at sample offset `time`.
#[inline]
pub fn emit_param_value(out: &mut OutputEvents<'_>, id: u32, value: f64, time: u32) {
    let _ = out.try_push(ParamValueEvent::new(
        time,
        ClapId::new(id),
        Pckn::match_all(),
        value,
        Cookie::empty(),
    ));
}
