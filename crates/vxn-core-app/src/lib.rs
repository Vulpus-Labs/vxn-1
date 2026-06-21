//! Shared MVC controller for VXN synth plugins.
//!
//! Lifts the synth-agnostic surface of vxn-1's `vxn-app` crate (ADR 0007):
//! the parameter schema ([`ParamDesc`], [`ParamModel`]), the
//! UI / host / view event types ([`UiEvent`], [`HostEvent`], [`ViewEvent`])
//! with a `Custom` escape hatch for per-synth events, the [`Controller`]
//! event loop, the [`EditorBackend`] trait, and the [`PresetStore`] trait
//! plus [`PresetCorpus`] model.
//!
//! Per-synth concerns (vxn-1's `KeyMode` / `Layer`, vxn-2's mod-matrix
//! rows) ride `UiEvent::Custom` / `ViewEvent::Custom` and are handled by
//! a closure the synth supplies to [`Controller::tick`].

pub mod backend;
pub mod controller;
pub mod events;
pub mod model;
pub mod params;
pub mod preset;

pub use backend::EditorBackend;
pub use controller::{CHANNEL_CAPACITY, Controller, ControllerHandle, CorpusHandle};
pub use events::{HostEvent, PresetSource, UiEvent, ViewEvent};
pub use model::{ParamId, ParamModel};
pub use params::{ParamDesc, ParamKind, Taper};
pub use preset::{
    PresetCorpus, PresetLoad, PresetMeta, PresetStore, UserFolderEntry, UserPresetEntry,
    corpus_snapshot_json,
};
