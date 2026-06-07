//! Per-tick driver for the VXN2 controller.
//!
//! `Controller::tick` requires two closures: one for `UiEvent::Custom`
//! payloads, one for `HostEvent::Custom` payloads. [`tick_vxn2`] wraps
//! that call with the VXN2-specific handlers — translating
//! [`Vxn2UiCustom`] events into [`Vxn2Params`] writes + matching
//! [`Vxn2ViewCustom`] echoes.

use std::any::Any;
use std::path::{Path, PathBuf};

use vxn_core_app::{
    Controller, PresetLoad, PresetMeta, PresetStore, UserFolderEntry, ViewEvent,
};

use crate::events::{MatrixRow, Vxn2UiCustom, Vxn2ViewCustom};
use crate::model::{Layer, Vxn2Params};

/// Read all 16 matrix rows of `layer` from the model. Helper for the
/// snapshot push paths below.
fn snapshot_layer<M: Vxn2Params>(model: &M, layer: Layer) -> [MatrixRow; 16] {
    let mut out = [MatrixRow::default(); 16];
    for slot in 0..16u8 {
        out[slot as usize] = model.matrix_row(layer, slot);
    }
    out
}

fn push_matrix_snapshot<M: Vxn2Params>(ctrl: &mut Controller<M>) {
    let upper = snapshot_layer(ctrl.model().as_ref(), Layer::Upper);
    let lower = snapshot_layer(ctrl.model().as_ref(), Layer::Lower);
    ctrl.push_view_event(ViewEvent::Custom(Box::new(
        Vxn2ViewCustom::MatrixSnapshot { upper, lower },
    )));
}

/// Drain inbound queues against `controller` and apply the VXN2 custom-
/// event handlers. Call once per host timer tick.
pub fn tick_vxn2<M: Vxn2Params>(controller: &mut Controller<M>) {
    let mut on_ui = |ctrl: &mut Controller<M>, payload: Box<dyn Any + Send>| {
        let Ok(boxed) = payload.downcast::<Vxn2UiCustom>() else {
            return;
        };
        match *boxed {
            Vxn2UiCustom::SetEditLayer { layer } => {
                ctrl.model().set_edit_layer(layer);
                ctrl.push_view_event(ViewEvent::Custom(Box::new(
                    Vxn2ViewCustom::EditLayerChanged { layer },
                )));
                // Re-seed the page-side matrix table on every layer
                // toggle so the overlay always renders the
                // now-current layer's rows without polling.
                push_matrix_snapshot(ctrl);
            }
            Vxn2UiCustom::SetOpTab { layer, op } => {
                ctrl.push_view_event(ViewEvent::Custom(Box::new(
                    Vxn2ViewCustom::OpTabChanged { layer, op },
                )));
            }
            Vxn2UiCustom::SetMatrixRow { layer, slot, row } => {
                // Single source of truth: SharedParams::set_matrix_row_raw
                // also writes the CLAP depth for slots 1-8, so depth has
                // two on-the-wire paths (this Custom + the plain
                // SetParam from the UI). CLAP wins during automation;
                // see PARAMETERS.md §"CLAP exposure".
                ctrl.model().set_matrix_row(layer, slot, row);
                ctrl.push_view_event(ViewEvent::Custom(Box::new(
                    Vxn2ViewCustom::MatrixRowChanged { layer, slot, row },
                )));
            }
            Vxn2UiCustom::RequestMatrixSnapshot => {
                push_matrix_snapshot(ctrl);
            }
        }
    };
    // VXN2 doesn't drive any per-synth HostEvent::Custom in E003 — the
    // closure is the no-op pair Controller::tick requires.
    let mut on_host = |_: &mut Controller<M>, _: Box<dyn Any + Send>| {};
    controller.tick(&mut on_ui, &mut on_host);
}

/// Empty preset store. Used by `vxn2-clap` until the preset epic ships;
/// every save / load / list op returns "not supported" or an empty
/// corpus. `Controller::new` requires a `PresetStore` so this fills the
/// hole without committing to a wire format.
pub struct NoopPresetStore;

impl PresetStore for NoopPresetStore {
    fn factory_len(&self) -> usize {
        0
    }
    fn factory_load(&self, _index: usize) -> Result<PresetLoad, String> {
        Err("no factory bank in this build".into())
    }
    fn factory_meta(&self, _index: usize) -> Option<PresetMeta> {
        None
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("no user preset store in this build".into())
    }
    fn user_save(
        &self,
        _name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        // The preset epic on top of E003 wires a real `PresetStore`.
        // Until then both Save and Save As land here — the controller
        // wraps this into a `save failed: …` status the preset bar
        // displays as a toast.
        Err("Save not yet supported in this build".into())
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Err("delete not supported".into())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Err("rename not supported".into())
    }
    fn user_move(&self, _path: &Path, _dest: Option<&str>) -> Result<PathBuf, String> {
        Err("move not supported".into())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Err("create folder not supported".into())
    }
    fn user_rename_folder(&self, _old: &str, _new: &str) -> Result<(PathBuf, String), String> {
        Err("rename folder not supported".into())
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Err("delete folder not supported".into())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}
