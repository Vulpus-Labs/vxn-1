//! VXN1 controller wrapper.
//!
//! Owns a [`vxn_core_app::Controller`] plus the synth-specific bits the
//! core controller can't know: the `Vxn1UiCustom` payload handler and
//! the `Vxn1ViewCustom` emit on key-mode / split-point changes.
//!
//! Those events fire at the moment of mutation, not from a poll:
//! - direct UI edits (`SetKeyMode` / `SetSplitPoint`) emit from the
//!   custom handler itself,
//! - preset / state loads (which the core mutates silently) emit from
//!   the `on_model_loaded` hook the core invokes from inside the load
//!   path,
//! - an editor (re-)attach republishes the lot via
//!   [`Controller::tick`]'s `take_editor_ready_flag` path — the webview
//!   page can reload without the plugin tearing down, so the per-instance
//!   view state needs reseeding even though nothing in the model changed.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};

use vxn_core_app::Controller as CoreController;
pub use vxn_core_app::{CHANNEL_CAPACITY, ControllerHandle, CorpusHandle};

use crate::domain::{KeyMode, Layer};
use crate::events::{HostEvent, UiEvent, ViewEvent, Vxn1UiCustom, Vxn1ViewCustom};
use crate::model::{ParamId, ParamModel, Vxn1Params};
use crate::params::{PATCH_COUNT, PatchParam, desc_for_clap_id, patch_clap_id};
use vxn_core_app::PresetStore;

/// Main-thread closure that drives the controller's event loop.
pub type Tick = Arc<dyn Fn() + Send + Sync + 'static>;

pub struct Controller<M: ParamModel + Vxn1Params> {
    inner: CoreController<M>,
}

impl<M: ParamModel + Vxn1Params> Controller<M> {
    pub fn new(
        model: Arc<M>,
        presets: Box<dyn PresetStore>,
    ) -> (Self, Receiver<ViewEvent>, CorpusHandle) {
        let (inner, view_rx, corpus) = CoreController::new(model, presets);
        (Self { inner }, view_rx, corpus)
    }

    pub fn handle(&self) -> ControllerHandle {
        self.inner.handle()
    }

    pub fn ui_sender(&self) -> SyncSender<UiEvent> {
        self.inner.ui_sender()
    }

    pub fn host_sender(&self) -> SyncSender<HostEvent> {
        self.inner.host_sender()
    }

    pub fn model(&self) -> &Arc<M> {
        self.inner.model()
    }

    pub fn preset_store(&self) -> &dyn PresetStore {
        self.inner.preset_store()
    }

    pub fn corpus_handle(&self) -> CorpusHandle {
        self.inner.corpus_handle()
    }

    /// Re-read the factory metas from the store into the shared corpus
    /// snapshot. The web controller fills its factory bank after
    /// construction (the baked asset arrives async at boot, E019 / 0062);
    /// this republishes the snapshot once it has.
    pub fn refresh_factory_corpus(&self) {
        self.inner.refresh_factory_corpus();
    }

    /// Re-read the user-side corpus from the store into the shared corpus
    /// snapshot. The web controller seeds its user cache from IndexedDB at
    /// boot (E019 / 0064) *after* construction; this republishes the snapshot
    /// once the cache is hydrated (the native stores never need it — their
    /// `Controller::new` already listed the on-disk corpus).
    pub fn refresh_user_corpus(&self) {
        self.inner.refresh_user_corpus();
    }

    /// Drain inbound queues and apply their effects. Wraps
    /// [`vxn_core_app::Controller::tick`] with the VXN1 custom handler
    /// and the `on_model_loaded` hook that emits
    /// `Vxn1ViewCustom::{KeyModeChanged, SplitPointChanged,
    /// EditLayerChanged}` from inside the preset / state load path.
    pub fn tick(&mut self) {
        self.inner.tick(
            &mut handle_ui_custom::<M>,
            &mut |_, _| {}, // no synth-specific host events
            &mut publish_keymode_split::<M>,
        );
        // A fresh editor attach (incl. webview page reload while the plugin
        // stays alive) re-fires UiEvent::EditorReady; republish KeyMode /
        // EditLayer / SplitPoint so the keys panel doesn't reopen stuck on
        // its cold-start defaults (Whole / Upper / C4) even when the model
        // already holds Dual/Split.
        if self.inner.take_editor_ready_flag() {
            publish_keymode_split(&mut self.inner);
        }
    }
}

/// Emit the current key-mode and split-point as `Vxn1ViewCustom` events
/// so editors stay in sync. Called from the load hook (model mutated by
/// a preset / state load) and on editor re-attach (republish current
/// state for a freshly-bound webview). Unconditional — the caller only
/// invokes it when a republish is actually warranted, so there is no
/// diff to do.
fn publish_keymode_split<M: ParamModel + Vxn1Params>(ctrl: &mut CoreController<M>) {
    let cur_key = ctrl.model().key_mode();
    let cur_split = ctrl.model().split_point();
    ctrl.push_view_event(Vxn1ViewCustom::KeyModeChanged { mode: cur_key }.into_event());
    // Whole reads only Upper-side params; rebind the editor's edit layer
    // onto Upper so the faceplate doesn't paint stale Lower values.
    if cur_key == KeyMode::Whole {
        ctrl.push_view_event(Vxn1ViewCustom::EditLayerChanged { layer: Layer::Upper }.into_event());
    }
    ctrl.push_view_event(Vxn1ViewCustom::SplitPointChanged { note: cur_split }.into_event());
}

fn handle_ui_custom<M: ParamModel + Vxn1Params>(
    ctrl: &mut CoreController<M>,
    payload: Box<dyn std::any::Any + Send>,
) {
    let Ok(custom) = payload.downcast::<Vxn1UiCustom>() else {
        return;
    };
    match *custom {
        Vxn1UiCustom::SetKeyMode { mode } => {
            // Seeded variant: Whole → non-Whole copies Upper → Lower
            // so the lower layer starts equal to the upper before
            // diverging. State load uses plain `set_key_mode` (no
            // seeding).
            ctrl.model().set_key_mode_seeded(mode);
            // Lower-layer params may have just been seeded from
            // Upper — republish them so the editor's signals follow.
            ctrl.broadcast_all_params();
            // Announce the mode change (+ Whole→Upper edit-layer snap).
            ctrl.push_view_event(Vxn1ViewCustom::KeyModeChanged { mode }.into_event());
            if mode == KeyMode::Whole {
                ctrl.push_view_event(
                    Vxn1ViewCustom::EditLayerChanged { layer: Layer::Upper }.into_event(),
                );
            }
        }
        Vxn1UiCustom::SetSplitPoint { note } => {
            ctrl.model().set_split_point(note);
            ctrl.push_view_event(ViewEvent::Status {
                line: format!("split point: {note}"),
            });
            ctrl.push_view_event(Vxn1ViewCustom::SplitPointChanged { note }.into_event());
        }
        Vxn1UiCustom::SetEditLayer { layer } => {
            // No model mutation — the edit layer is pure view state.
            ctrl.push_view_event(
                Vxn1ViewCustom::EditLayerChanged { layer }.into_event(),
            );
        }
        Vxn1UiCustom::ResetLayer { layer } => {
            reset_layer(ctrl, layer);
        }
    }
}

/// Reset every per-patch param of `layer` to its descriptor default.
/// Each write is bracketed by a gesture (like the UI's double-click
/// reset) so the host echoes the jump as a recorded edit.
fn reset_layer<M: ParamModel + Vxn1Params>(ctrl: &CoreController<M>, layer: Layer) {
    for p in 0..PATCH_COUNT {
        let pp = PatchParam::from_index(p).expect("PATCH_COUNT bound by enum");
        let raw = patch_clap_id(layer, pp);
        let id = ParamId::new(raw);
        let default = desc_for_clap_id(raw).map_or(0.0, |d| d.default);
        ctrl.model().set_gesture(id, true);
        ctrl.model().set(id, default);
        ctrl.model().set_gesture(id, false);
        ctrl.emit_param_changed(id);
    }
}
