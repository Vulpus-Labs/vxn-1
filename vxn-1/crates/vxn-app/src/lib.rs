//! VXN1 controller crate (ADR 0007).
//!
//! Holds the single arbiter of non-audio model mutation — the [`Controller`] —
//! plus the trait surface a UI (`EditorBackend`) and a parameter store
//! ([`ParamModel`]) program against. Engine-agnostic: VXN-2 will plug in its
//! own `ParamModel` impl.
//!
//! Scaffold only; handlers fill in across tickets 0034–0038.

pub mod controller;
pub mod domain;
pub mod events;
pub mod factory_asset;
pub mod model;
pub mod params;
pub mod state;
pub mod sync;

pub use controller::{CHANNEL_CAPACITY, Controller, ControllerHandle, CorpusHandle, Tick};
pub use domain::{DEFAULT_SPLIT_POINT, KeyMode, Layer, PresetMeta, UNCATEGORIZED};
pub use events::{HostEvent, PresetSource, UiEvent, ViewEvent, Vxn1UiCustom, Vxn1ViewCustom};
pub use factory_asset::FactoryEntry;
pub use model::{ParamId, ParamModel, Vxn1Params};
pub use state::{BLOB_LEN, MAGIC, VERSION, read_state_into, write_state_bytes};
pub use params::{
    AssignMode, CrossModType, EnvSel, GLOBAL_PARAMS, GLOBAL_COUNT, GlobalParam, LfoSel,
    PATCH_COUNT, PATCH_PARAMS, ParamDesc, ParamKind, ParamRef, PatchParam,
    TOTAL_PARAMS, Taper, desc_for_clap_id, global_clap_id, module_for_clap_id, param_ref,
    patch_clap_id,
};

// EditorBackend + PresetStore + PresetCorpus + PresetLoad +
// UserFolderEntry + UserPresetEntry all live in `vxn-core-app`
// post-E001/0007. Their signatures now use the shared `UiEvent` /
// `ViewEvent`, so the re-exports are drop-in for vxn-1 callers.
pub use vxn_core_app::{
    EditorBackend, PresetCorpus, PresetLoad, PresetStore, UserFolderEntry, UserPresetEntry,
    corpus_snapshot_json,
};
