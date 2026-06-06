//! Event types crossing the controller boundary.
//!
//! `UiEvent` flows UI → controller, `HostEvent` flows host shell →
//! controller, `ViewEvent` flows controller → UI. The controller is the
//! only writer of the model.
//!
//! Per-synth events (vxn-1's `SetKeyMode` / `SetSplitPoint`, vxn-2's
//! mod-matrix row edits) ride the `Custom` variant. The synth supplies
//! a closure to [`crate::Controller::tick`] that downcasts the payload
//! and runs synth-specific logic against the shared helpers
//! ([`crate::Controller::broadcast_all_params`], `push_view_event`, …).

use std::any::Any;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::ParamId;
use crate::preset::PresetMeta;

/// Where a preset is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetSource {
    /// Index into the embedded factory bank.
    Factory { index: usize },
    /// Absolute path under the user preset directory.
    User { path: PathBuf },
}

/// Intent posted by the editor to the controller.
pub enum UiEvent {
    SetParam { id: ParamId, plain: f32 },
    SetParamNorm { id: ParamId, norm: f32 },
    BeginGesture { id: ParamId },
    EndGesture { id: ParamId },

    LoadPreset {
        source: PresetSource,
    },
    /// Walk the combined factory + user preset list by `delta` steps,
    /// wrapping at either end, and load the resulting entry.
    StepPreset {
        delta: i32,
    },
    SavePreset {
        name: String,
        folder: Option<String>,
    },
    RenamePreset {
        path: PathBuf,
        new_name: String,
    },
    DeletePreset {
        path: PathBuf,
    },
    MovePreset {
        path: PathBuf,
        dest_folder: Option<String>,
    },
    RenameFolder {
        old_name: String,
        new_name: String,
    },
    DeleteFolder {
        name: String,
    },
    NewFolder {
        suggested: String,
    },

    /// Editor finished its initial init and is ready to receive view
    /// events. Triggers a full re-broadcast of every param so the page
    /// is correctly seeded after a slow JS bootstrap.
    EditorReady,

    /// Editor asks the backend to pop a floating text-input window
    /// (workaround for hosts that swallow keyboard input). `id` is a
    /// caller-chosen correlation token returned verbatim in the
    /// matching [`UiEvent::TextInputResult`].
    RequestTextInput {
        id: String,
        title: String,
        initial: String,
    },
    /// Floating text-input popup committed (`Some`) or cancelled
    /// (`None`). Controller forwards as [`ViewEvent::TextInputResult`].
    TextInputResult {
        id: String,
        value: Option<String>,
    },

    /// Per-synth event escape hatch. The synth's controller-driver
    /// closure downcasts the payload to its own type.
    Custom(Box<dyn Any + Send>),
}

impl std::fmt::Debug for UiEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetParam { id, plain } => f
                .debug_struct("SetParam")
                .field("id", id)
                .field("plain", plain)
                .finish(),
            Self::SetParamNorm { id, norm } => f
                .debug_struct("SetParamNorm")
                .field("id", id)
                .field("norm", norm)
                .finish(),
            Self::BeginGesture { id } => f.debug_struct("BeginGesture").field("id", id).finish(),
            Self::EndGesture { id } => f.debug_struct("EndGesture").field("id", id).finish(),
            Self::LoadPreset { source } => {
                f.debug_struct("LoadPreset").field("source", source).finish()
            }
            Self::StepPreset { delta } => {
                f.debug_struct("StepPreset").field("delta", delta).finish()
            }
            Self::SavePreset { name, folder } => f
                .debug_struct("SavePreset")
                .field("name", name)
                .field("folder", folder)
                .finish(),
            Self::RenamePreset { path, new_name } => f
                .debug_struct("RenamePreset")
                .field("path", path)
                .field("new_name", new_name)
                .finish(),
            Self::DeletePreset { path } => {
                f.debug_struct("DeletePreset").field("path", path).finish()
            }
            Self::MovePreset { path, dest_folder } => f
                .debug_struct("MovePreset")
                .field("path", path)
                .field("dest_folder", dest_folder)
                .finish(),
            Self::RenameFolder { old_name, new_name } => f
                .debug_struct("RenameFolder")
                .field("old_name", old_name)
                .field("new_name", new_name)
                .finish(),
            Self::DeleteFolder { name } => {
                f.debug_struct("DeleteFolder").field("name", name).finish()
            }
            Self::NewFolder { suggested } => f
                .debug_struct("NewFolder")
                .field("suggested", suggested)
                .finish(),
            Self::EditorReady => f.write_str("EditorReady"),
            Self::RequestTextInput { id, title, initial } => f
                .debug_struct("RequestTextInput")
                .field("id", id)
                .field("title", title)
                .field("initial", initial)
                .finish(),
            Self::TextInputResult { id, value } => f
                .debug_struct("TextInputResult")
                .field("id", id)
                .field("value", value)
                .finish(),
            Self::Custom(_) => f.write_str("Custom(<dyn Any>)"),
        }
    }
}

/// Event extracted from the host's CLAP stream and handed to the controller.
///
/// `StateLoaded` carries the raw blob the host gave us; the model
/// deserialises it via [`crate::ParamModel::restore_from_bytes`]. Keeping
/// the blob opaque here lets this crate stay engine-free.
pub enum HostEvent {
    ParamAutomation { id: ParamId, plain: f32 },
    StateLoaded { blob: Vec<u8> },
    Tempo { bpm: f32 },
    /// Per-synth host event (e.g. mod-wheel CC the synth wants to route).
    Custom(Box<dyn Any + Send>),
}

impl std::fmt::Debug for HostEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParamAutomation { id, plain } => f
                .debug_struct("ParamAutomation")
                .field("id", id)
                .field("plain", plain)
                .finish(),
            Self::StateLoaded { blob } => f
                .debug_struct("StateLoaded")
                .field("blob_len", &blob.len())
                .finish(),
            Self::Tempo { bpm } => f.debug_struct("Tempo").field("bpm", bpm).finish(),
            Self::Custom(_) => f.write_str("Custom(<dyn Any>)"),
        }
    }
}

/// View-bound update the controller emits. The editor drains these on
/// idle and reseeds its widget signals; no other path mutates the
/// view's data.
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
    /// move / new folder). `follow` carries the on-disk path of the
    /// preset that triggered the change so the view can move its cursor
    /// onto that entry; `None` for changes with no single follow target
    /// (delete, new folder).
    PresetCorpusChanged {
        follow: Option<PathBuf>,
    },
    Status {
        line: String,
    },
    /// Backend-bound: open the floating text-input popup. Not forwarded
    /// to the page — the editor backend intercepts in its
    /// `push_view_event` impl and pops a native window.
    OpenTextInput {
        id: String,
        title: String,
        initial: String,
    },
    /// Page-bound result of a text-input popup. The JS dispatcher fires
    /// the pending callback keyed by `id`. `value` is `None` on cancel.
    TextInputResult {
        id: String,
        value: Option<String>,
    },
    /// Per-synth view event escape hatch.
    Custom(Box<dyn Any + Send>),
}

impl std::fmt::Debug for ViewEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParamChanged {
                id, plain, norm, display,
            } => f
                .debug_struct("ParamChanged")
                .field("id", id)
                .field("plain", plain)
                .field("norm", norm)
                .field("display", display)
                .finish(),
            Self::PresetLoaded { meta, source, warnings } => f
                .debug_struct("PresetLoaded")
                .field("meta", meta)
                .field("source", source)
                .field("warnings", warnings)
                .finish(),
            Self::PresetCorpusChanged { follow } => f
                .debug_struct("PresetCorpusChanged")
                .field("follow", follow)
                .finish(),
            Self::Status { line } => f.debug_struct("Status").field("line", line).finish(),
            Self::OpenTextInput { id, title, initial } => f
                .debug_struct("OpenTextInput")
                .field("id", id)
                .field("title", title)
                .field("initial", initial)
                .finish(),
            Self::TextInputResult { id, value } => f
                .debug_struct("TextInputResult")
                .field("id", id)
                .field("value", value)
                .finish(),
            Self::Custom(_) => f.write_str("Custom(<dyn Any>)"),
        }
    }
}

/// `PresetSource` is `serde`-friendly via the helper below — we don't
/// derive directly because `PathBuf` serde behaviour is platform-flavoured
/// and editors want a stable shape.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PresetSourceWire {
    Factory { index: usize },
    User { path: String },
}

impl Serialize for PresetSource {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PresetSource::Factory { index } => {
                PresetSourceWire::Factory { index: *index }.serialize(s)
            }
            PresetSource::User { path } => PresetSourceWire::User {
                path: path.display().to_string(),
            }
            .serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for PresetSource {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match PresetSourceWire::deserialize(d)? {
            PresetSourceWire::Factory { index } => PresetSource::Factory { index },
            PresetSourceWire::User { path } => PresetSource::User { path: path.into() },
        })
    }
}
