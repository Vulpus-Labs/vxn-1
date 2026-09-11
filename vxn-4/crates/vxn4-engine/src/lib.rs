//! VXN4 engine — enough machinery to hear the operator block.
//!
//! Scope is deliberately the brief's: voice allocation with vxn-2's note
//! selection and trimming behaviour, per-operator envelopes, six hardwired
//! patches, and the oversampling chain down through a limiter. No FX, no
//! faceplate, no parameter automation, no preset format.
//!
//! ```no_run
//! use vxn4_engine::{Engine, Quality};
//!
//! let mut e = Engine::new(48_000.0);
//! e.set_patch(1);
//! e.set_quality(Quality::X16);
//! e.note_on(60, 100);
//!
//! let (mut l, mut r) = ([0.0; 512], [0.0; 512]);
//! e.process(&mut l, &mut r);
//! ```
//!
//! ## Layout
//!
//! - [`alloc`] — 16 explicit voices + 4 declick spares, quietest-voice
//!   stealing. The behavioural port from vxn-2.
//! - [`eg`] — 4-rate/4-level envelopes, one per operator per voice.
//! - [`patch`] — the six hardwired patches, graded by routing density.
//! - [`matrix`] — the modulation roster: 8 macro sources, 80 destinations.
//! - [`engine`] — banks, rate plan, limiter.
//! - [`params`] — the descriptor table: every patch field named and ranged.
//! - [`shared`] — the authoritative patch, on the main thread.
//! - [`topology`] — the lock-free channel that carries the rest of it.
//!
//! ## Who owns the patch
//!
//! **The main thread owns the model; the audio thread reads it and never owns
//! it. Truth flows main → audio and never back.** [`shared::SharedParams`] is
//! the authority — scalars in atomics, matrix topology behind a mutex the audio
//! thread never takes — and [`Engine`] holds only the flattened tables it
//! renders from. See [`shared`] for the split and why it is shaped that way.
//!
//! ## Modulation
//!
//! Eight macro knobs are the only modulation sources, and the only thing about
//! modulation a host ever sees ([`matrix`] says why). Their totals are **added**
//! to the patch's authored depths at control rate, so all macros at zero is the
//! patch exactly as the table writes it.
//!
//! ## Known placeholder
//!
//! The self-feedback diagonal still averages 2 *ticks*, as the brief specifies,
//! which at 8x puts its Nyquist zero at 192 kHz where it does nothing. See
//! `vxn4_dsp::ops` for why, and `patch::bell` for the patch that will change
//! character when it is fixed. Left visible rather than silently corrected,
//! because which way to fix it is an ear decision.

pub mod alloc;
pub mod eg;
pub mod engine;
pub mod matrix;
pub mod params;
pub mod patch;
pub mod preset;
pub mod shared;
pub mod topology;

pub use alloc::{Alloc, N_ACTIVE, N_DECLICK, N_SLOTS, Phase, Voice};
pub use eg::{Eg, EgParams, Stage};
pub use engine::{Engine, HOST_LATENCY_SAMPLES, MAX_MASTER_GAIN, Quality, latency_samples};
pub use matrix::{DestId, Matrix, N_DESTS, N_MACROS, N_MATRIX_SLOTS, Roster, SourceId};
pub use params::{N_PARAMS, PATCH_PARAMS, Param, ParamId, all_ids, desc, id_for_name, patch_ids};
pub use patch::{N_PATCHES, Patch, PatchTables, patch, patch_names};
pub use preset::{MacroSpec, Macros, Meta, Preset, PresetError, read_preset, write_preset};
pub use shared::{Drain, SharedParams};
pub use topology::{SlotEdit, SlotField, TopoMsg};
