//! VXN2 controller composition layer (ticket 0022 / epic E003).
//!
//! Bridges `vxn2-engine`'s atomic param store to `vxn_core_app::Controller`:
//!
//! - Declares VXN2-specific `UiEvent::Custom` / `ViewEvent::Custom` payloads
//!   ([`Vxn2UiCustom`] / [`Vxn2ViewCustom`]) for things the shared
//!   vocabulary doesn't cover: mod-matrix row edits (source / dest / curve /
//!   active — `depth` rides the normal `SetParam` path for slots 1-8) and
//!   op-tab switches.
//! - Extends the `ParamModel` surface via [`Vxn2Params`] for the non-CLAP
//!   shared state the controller needs to read / write (matrix rows).
//! - Exposes [`tick_vxn2`], the per-tick driver `vxn2-clap`'s timer extension
//!   calls. It wires the `(on_custom_ui, on_custom_host)` closure pair
//!   `Controller::tick` requires, translating `Vxn2UiCustom` into model
//!   writes + matching `Vxn2ViewCustom` echoes.
//!
//! `impl vxn_core_app::ParamModel for SharedParams` and
//! `impl Vxn2Params for SharedParams` both live in `vxn2-engine` itself —
//! orphan rule. This crate is intentionally tiny.

pub mod controller;
pub mod events;
pub mod model;

pub use controller::{
    NoopPresetStore, ks_curve_snapshot_event, matrix_snapshot_event, matrix_snapshot_rows,
    push_ks_curve_snapshot, push_matrix_snapshot, tick_vxn2,
};
pub use events::{MatrixRow, Vxn2UiCustom, Vxn2ViewCustom};
pub use model::Vxn2Params;

pub use vxn_core_app::{
    CHANNEL_CAPACITY, Controller, ControllerHandle, CorpusHandle, EditorBackend, HostEvent,
    ParamDesc, ParamId, ParamKind, ParamModel, PresetCorpus, PresetLoad, PresetMeta,
    PresetSource, PresetStore, Taper, UiEvent, UserFolderEntry, UserPresetEntry, ViewEvent,
};
