//! UI / host / view event enums (ADR 0007 §3).
//!
//! Channels carry these between threads. `UiEvent` flows UI → controller,
//! `HostEvent` flows host shell → controller, `ViewEvent` flows controller →
//! UI. The controller is the only writer of the model.

use std::path::PathBuf;

use crate::domain::{KeyMode, Layer, PresetMeta};
use crate::model::ParamId;

/// Where a preset is read from.
#[derive(Clone, Debug)]
pub enum PresetSource {
    /// Index into the embedded factory bank (`vxn_engine::factory()`).
    Factory { index: usize },
    /// Absolute path under the user preset directory.
    User { path: PathBuf },
}

/// Intent posted by the editor to the controller.
#[derive(Clone, Debug)]
pub enum UiEvent {
    SetParam { id: ParamId, plain: f32 },
    SetParamNorm { id: ParamId, norm: f32 },
    BeginGesture { id: ParamId },
    EndGesture { id: ParamId },
    /// Reset every per-patch param of `layer` to its descriptor default. Each
    /// write is gesture-bracketed so the host records the jump as one edit.
    /// Globals and the other layer are left untouched.
    ResetLayer { layer: Layer },
    LoadPreset { source: PresetSource },
    SavePreset { name: String, folder: Option<String> },
    RenamePreset { path: PathBuf, new_name: String },
    DeletePreset { path: PathBuf },
    MovePreset { path: PathBuf, dest_folder: Option<String> },
    RenameFolder { old_name: String, new_name: String },
    DeleteFolder { name: String },
    NewFolder { suggested: String },
    SetKeyMode { mode: KeyMode },
    SetSplitPoint { note: u8 },
    SetEditLayer { layer: Layer },
}

/// Event extracted from the host's CLAP stream and handed to the controller.
///
/// `StateLoaded` carries the raw blob the host gave us; the model deserializes
/// it via [`ParamModel::restore_from_bytes`]. Keeping the blob opaque here lets
/// `vxn-app` stay engine-free.
#[derive(Clone, Debug)]
pub enum HostEvent {
    ParamAutomation { id: ParamId, plain: f32 },
    StateLoaded { blob: Vec<u8> },
    Tempo { bpm: f32 },
}

/// View-bound update the controller emits. The editor drains these on idle
/// and reseeds its widget signals; no other path mutates the view's data.
#[derive(Clone, Debug)]
pub enum ViewEvent {
    ParamChanged {
        id: ParamId,
        plain: f32,
        norm: f32,
        display: String,
    },
    PresetLoaded {
        meta: PresetMeta,
        source: Option<PresetSource>,
        warnings: Vec<String>,
    },
    /// The user-preset corpus on disk changed (save / rename / delete /
    /// move / new folder). The editor re-reads the snapshot the controller
    /// publishes via `CorpusHandle`. `follow` carries the on-disk path of the
    /// preset that triggered the change (e.g. just-saved / just-renamed /
    /// just-moved), so the view can move its cursor onto that entry; `None`
    /// for changes with no single follow target (delete, new folder).
    PresetCorpusChanged {
        follow: Option<PathBuf>,
    },
    KeyModeChanged {
        mode: KeyMode,
    },
    Status {
        line: String,
    },
}
