//! VXN2 engine — voice allocator, voice-stack instantiator, mod-matrix
//! engine, FX chain, block render loop, parameter table.
//!
//! Ticket 0004 deliverable: [`alloc::PolyAlloc`] — 16-voice polyphony with
//! oldest-note stealing, Poly / Solo assignment, glide, and channel-wide
//! pitch bend. Higher-level stacking (0005), modulation (0006–0008), voicing
//! modes (0009), and FX (0010–0012) layer on top.

pub mod alloc;
pub mod default_patch;
pub mod engine;
pub mod factory;
pub mod ftz;
pub mod master;
pub mod matrix;
pub mod modulation;
pub mod params;
pub mod preset;
pub mod preset_io;
pub mod shared;
pub mod sync;

pub use ftz::ScopedFlushToZero;
pub use preset_io::Vxn2PresetStore;
pub use sync::{rate_partner_clap_id, sync_aware_display, sync_pairs, sync_partner_clap_id};
pub use params::{ParamDesc, ParamKind, TOTAL_PARAMS, desc_for_clap_id, module_for_clap_id};
/// Tempo-sync subdivisions re-exported for the CLAP shell's sync-aware
/// display path (ticket 0031). Canonical source lives in `vxn2-dsp::lfo`.
pub use vxn2_dsp::lfo::{SUBDIVISIONS, Subdivision, index_from_norm};
pub use shared::{
    BLOB_HEADER_LEN, BLOB_MAGIC, BLOB_MATRIX_LEN, BLOB_VERSION, EngineParams, FilterParams,
    MatrixRowRaw, N_MATRIX_CLAP_SLOTS, N_MATRIX_SLOTS, ParamLoadError, ParamModel, ParamView,
    Patch, SharedParams, filter_params_of,
};
