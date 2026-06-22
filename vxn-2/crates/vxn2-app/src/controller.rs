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
use crate::model::Vxn2Params;

/// Read all 16 matrix rows from `model`. Used by both the controller-
/// scoped push and the CLAP shell's dirty-bitset pump (ADR 0003) so they
/// build the same snapshot shape.
pub fn matrix_snapshot_rows<M: Vxn2Params>(model: &M) -> [MatrixRow; 16] {
    let mut out = [MatrixRow::default(); 16];
    for slot in 0..16u8 {
        out[slot as usize] = model.matrix_row(slot);
    }
    out
}

/// Build a `Vxn2ViewCustom::MatrixSnapshot` view event from `model` without
/// the controller wrapper. The CLAP shell's pump (ADR 0003) calls this
/// when `SharedParams.dirty_matrix` is non-zero on a tick drain so it can
/// push directly to the editor handle without re-entering the controller.
pub fn matrix_snapshot_event<M: Vxn2Params>(model: &M) -> ViewEvent {
    ViewEvent::Custom(Box::new(Vxn2ViewCustom::MatrixSnapshot {
        rows: matrix_snapshot_rows(model),
    }))
}

/// Push a fresh `MatrixSnapshot` view event into the controller's queue.
/// Used by the UI on `RequestMatrixSnapshot` so the page never displays
/// stale rows after an explicit refresh request.
pub fn push_matrix_snapshot<M: Vxn2Params>(ctrl: &mut Controller<M>) {
    let event = matrix_snapshot_event(ctrl.model().as_ref());
    ctrl.push_view_event(event);
}

/// Build a `Vxn2ViewCustom::KsCurveSnapshot` from `model`. Shared by the
/// controller-scoped push and the CLAP pump (ADR 0003) so both produce the
/// same shape, the KS-curve analogue of [`matrix_snapshot_event`].
pub fn ks_curve_snapshot_event<M: Vxn2Params>(model: &M) -> ViewEvent {
    ViewEvent::Custom(Box::new(Vxn2ViewCustom::KsCurveSnapshot {
        curves: model.ks_curves(),
    }))
}

/// Push a fresh `KsCurveSnapshot` into the controller's queue (UI
/// `RequestKsCurveSnapshot` path).
pub fn push_ks_curve_snapshot<M: Vxn2Params>(ctrl: &mut Controller<M>) {
    let event = ks_curve_snapshot_event(ctrl.model().as_ref());
    ctrl.push_view_event(event);
}

/// Drain inbound queues against `controller` and apply the VXN2 custom-
/// event handlers. Call once per host timer tick.
pub fn tick_vxn2<M: Vxn2Params>(controller: &mut Controller<M>) {
    let mut on_ui = |ctrl: &mut Controller<M>, payload: Box<dyn Any + Send>| {
        let Ok(boxed) = payload.downcast::<Vxn2UiCustom>() else {
            return;
        };
        match *boxed {
            Vxn2UiCustom::SetOpTab { op } => {
                // Pure UI mode state — no Model backing, so the
                // dirty-bitset pump (ADR 0003) doesn't carry it. The
                // echo is the only path; keep it.
                ctrl.push_view_event(ViewEvent::Custom(Box::new(
                    Vxn2ViewCustom::OpTabChanged { op },
                )));
            }
            Vxn2UiCustom::SetMatrixRow { slot, row } => {
                // Write the Model and stop. The dirty-bitset pump
                // catches the matrix-slot bit on the next tick and
                // pushes a `MatrixSnapshot`. The optimistic UI paint
                // in `dispatchRow` covers the one-tick latency.
                ctrl.model().set_matrix_row(slot, row);
            }
            Vxn2UiCustom::RequestMatrixSnapshot => {
                push_matrix_snapshot(ctrl);
            }
            Vxn2UiCustom::SetKsCurve { op, side, curve } => {
                // Write the Model and stop. The dirty-bitset pump catches
                // the KS-curve dirty flag on the next tick and pushes a
                // `KsCurveSnapshot`; the op-row graph paints optimistically
                // in the meantime.
                ctrl.model().set_ks_curve(op, side, curve);
            }
            Vxn2UiCustom::RequestKsCurveSnapshot => {
                push_ks_curve_snapshot(ctrl);
            }
            Vxn2UiCustom::RequestFullRebroadcast => {
                // Flip every dirty bit on the Model; the CLAP shell's
                // main-thread tick drains them next time round and the
                // editor handle receives a full table broadcast plus a
                // MatrixSnapshot. No view event from this handler.
                ctrl.model().mark_all_dirty();
            }
        }
    };
    // VXN2 doesn't drive any per-synth HostEvent::Custom in E003 — the
    // closure is the no-op pair Controller::tick requires.
    let mut on_host = |_: &mut Controller<M>, _: Box<dyn Any + Send>| {};
    // No post-load hook: vxn-2's dirty-bitset pump (ADR 0003) already
    // re-emits the whole table after a load, and it has no non-param view
    // state (key-mode / split) to announce the way vxn-1 does.
    let mut on_loaded = |_: &mut Controller<M>| {};
    controller.tick(&mut on_ui, &mut on_host, &mut on_loaded);
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
