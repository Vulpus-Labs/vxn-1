//! VXN1 editor (Vizia), embedded into the host window via baseview.
//!
//! Parameter flow (see `vxn_engine::SharedParams` and `vxn-clap`'s
//! `LocalParams`):
//!
//! - **UI → host:** a control's `on_change` writes the new value into the shared
//!   store. The plugin's audio thread diffs the store each `process` and emits
//!   the change to the host (wrapped in a CLAP gesture), so DAW automation
//!   recording and the host's generic UI stay in sync.
//! - **host → UI:** [`Application::on_idle`] polls the shared store and pushes
//!   any host-side changes into the reactive [`SyncSignal`]s, so the knobs track
//!   live automation playback. `SyncSignal` is `Send + Sync`, which is why the
//!   `on_idle` (`Send`) closure can hold them.
//!
//! Values bound to the controls are *normalized* `[0, 1]`; the shared store
//! converts to/from plain units via the parameter descriptors.
//!
//! Scope (first slice): a representative subset of controls is wired, spanning
//! the smoothing classes (cutoff → ladder coeff interpolation; resonance/drive;
//! master volume → per-sample smoothing; osc levels → block-rate smoothing).
//! Sustained drag gestures (`SharedParams::set_gesture`) are not yet emitted on
//! pointer down/up — until then each edit is sent as a self-contained
//! gesture, which the host still records correctly.

use std::ffi::c_void;
use std::sync::Arc;

use vizia::ParentWindow;
use vizia::prelude::*;
use vxn_engine::{PARAMS, ParamId, SharedParams};

/// Handle to the live editor window. Call [`WindowHandle::close`] when the host
/// destroys the GUI.
pub type EditorHandle = WindowHandle;

pub const EDITOR_WIDTH: u32 = 540;
pub const EDITOR_HEIGHT: u32 = 240;

/// Controls shown in this first editor, one per smoothing class.
const CONTROLS: &[ParamId] = &[
    ParamId::Cutoff,
    ParamId::Resonance,
    ParamId::Drive,
    ParamId::Osc1Level,
    ParamId::Osc2Level,
    ParamId::MasterVolume,
];

/// Open the editor parented to `parent` (a platform window pointer — on macOS
/// the host `NSView`). The editor reads and writes `shared`.
pub fn open_editor(parent: *mut c_void, shared: Arc<SharedParams>) -> EditorHandle {
    // One reactive signal per control, holding the normalized [0, 1] position.
    let signals: Vec<(usize, SyncSignal<f32>)> = CONTROLS
        .iter()
        .map(|id| {
            let i = id.index();
            (i, SyncSignal::new(shared.get_normalized(i)))
        })
        .collect();

    let build_signals = signals.clone();
    let build_shared = Arc::clone(&shared);
    let idle_signals = signals;
    let idle_shared = shared;

    let parent = ParentWindow(parent);
    Application::new(move |cx| build_editor(cx, build_signals.clone(), Arc::clone(&build_shared)))
        .on_idle(move |_cx| {
            // Host automation → UI: reflect shared values back into the knobs.
            for (i, sig) in &idle_signals {
                let n = idle_shared.get_normalized(*i);
                if (sig.get() - n).abs() > f32::EPSILON {
                    sig.set(n);
                }
            }
        })
        .inner_size((EDITOR_WIDTH, EDITOR_HEIGHT))
        .title("VXN1")
        .open_parented(&parent)
}

fn build_editor(
    cx: &mut Context,
    signals: Vec<(usize, SyncSignal<f32>)>,
    shared: Arc<SharedParams>,
) {
    VStack::new(cx, move |cx| {
        Label::new(cx, "VXN1");
        HStack::new(cx, move |cx| {
            for (i, sig) in signals {
                let sh = Arc::clone(&shared);
                VStack::new(cx, move |cx| {
                    Label::new(cx, PARAMS[i].label);
                    Slider::new(cx, sig).on_change(move |_cx, v| {
                        sh.set_normalized(i, v);
                    });
                })
                .width(Pixels(80.0))
                .vertical_gap(Pixels(6.0));
            }
        })
        .horizontal_gap(Pixels(12.0));
    })
    .padding(Pixels(16.0))
    .vertical_gap(Pixels(12.0));
}
