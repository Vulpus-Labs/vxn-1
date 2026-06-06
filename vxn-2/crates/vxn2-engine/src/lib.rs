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
pub mod ftz;
pub mod master;
pub mod matrix;
pub mod modulation;
pub mod params;
pub mod shared;
pub mod voicing;

pub use ftz::ScopedFlushToZero;
pub use params::{ParamDesc, ParamKind, TOTAL_PARAMS, desc_for_clap_id, module_for_clap_id};
pub use shared::{
    BLOB_HEADER_LEN, BLOB_MAGIC, BLOB_VERSION, EngineParams, ParamLoadError, ParamModel,
    ParamView, SharedParams,
};
