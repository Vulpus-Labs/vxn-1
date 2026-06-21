//! VXN1 controller wrapper.
//!
//! Owns a [`vxn_core_app::Controller`] plus the synth-specific bits the
//! core controller can't know: the `Vxn1UiCustom` payload handler, the
//! `Vxn1ViewCustom` emit on key-mode / split-point changes (polled
//! after every tick because the shared controller's preset-load /
//! state-load paths mutate the model directly), and the
//! `snap-to-upper-if-Whole` edit-layer echo.

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
    last_key_mode: KeyMode,
    last_split_point: u8,
    first_tick: bool,
}

impl<M: ParamModel + Vxn1Params> Controller<M> {
    pub fn new(
        model: Arc<M>,
        presets: Box<dyn PresetStore>,
    ) -> (Self, Receiver<ViewEvent>, CorpusHandle) {
        let initial_key = model.key_mode();
        let initial_split = model.split_point();
        let (inner, view_rx, corpus) = CoreController::new(model, presets);
        let me = Self {
            inner,
            last_key_mode: initial_key,
            last_split_point: initial_split,
            first_tick: true,
        };
        (me, view_rx, corpus)
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

    /// Drain inbound queues and apply their effects. Wraps
    /// [`vxn_core_app::Controller::tick`] with the VXN1 custom handler;
    /// after dispatch, polls the model for key-mode / split-point
    /// changes the shared controller's preset / state load paths did
    /// without an explicit Vxn1Custom event, and emits the matching
    /// `Vxn1ViewCustom::{KeyModeChanged, SplitPointChanged,
    /// EditLayerChanged}` so editors stay in sync.
    pub fn tick(&mut self) {
        self.inner.tick(
            &mut handle_ui_custom::<M>,
            &mut |_, _| {}, // no synth-specific host events
        );
        // A fresh editor attach (incl. webview page reload while the plugin
        // stays alive) re-fires UiEvent::EditorReady; rearm `first_tick` so
        // `publish_keymode_split_diffs` republishes KeyMode / EditLayer /
        // SplitPoint. Without this the keys panel reopens stuck on its
        // cold-start defaults (Whole / Upper / C4) even when the model
        // already holds Dual/Split.
        if self.inner.take_editor_ready_flag() {
            self.first_tick = true;
        }
        self.publish_keymode_split_diffs();
    }

    fn publish_keymode_split_diffs(&mut self) {
        let model = self.inner.model();
        let cur_key = model.key_mode();
        let cur_split = model.split_point();

        // First tick (post-EditorReady or post-state-load): always
        // republish so editors that just attached see the current
        // state without needing a change to fire.
        let force = std::mem::take(&mut self.first_tick);

        if force || cur_key != self.last_key_mode {
            self.inner
                .push_view_event(Vxn1ViewCustom::KeyModeChanged { mode: cur_key }.into_event());
            // Whole reads only Upper-side params; rebind the editor's
            // edit layer onto Upper so the faceplate doesn't paint
            // stale Lower values.
            if cur_key == KeyMode::Whole {
                self.inner.push_view_event(
                    Vxn1ViewCustom::EditLayerChanged { layer: Layer::Upper }.into_event(),
                );
            }
            self.last_key_mode = cur_key;
        }
        if force || cur_split != self.last_split_point {
            self.inner
                .push_view_event(Vxn1ViewCustom::SplitPointChanged { note: cur_split }.into_event());
            self.last_split_point = cur_split;
        }
    }
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
            // KeyModeChanged + Whole→Upper snap are emitted by the
            // post-tick poll in Controller::tick.
            ctrl.broadcast_all_params();
        }
        Vxn1UiCustom::SetSplitPoint { note } => {
            ctrl.model().set_split_point(note);
            ctrl.push_view_event(ViewEvent::Status {
                line: format!("split point: {note}"),
            });
            // SplitPointChanged emitted by post-tick poll.
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
