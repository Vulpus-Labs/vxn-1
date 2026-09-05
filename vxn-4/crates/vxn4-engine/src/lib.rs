//! VXN4 engine — enough machinery to hear the operator block.
//!
//! Scope is deliberately the brief's: voice allocation with vxn-2's note
//! selection and trimming behaviour, per-operator envelopes, five hardwired
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
//! - [`patch`] — the five hardwired patches, graded by routing density.
//! - [`engine`] — banks, rate plan, limiter.
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
pub mod patch;

pub use alloc::{Alloc, N_ACTIVE, N_DECLICK, N_SLOTS, Phase, Voice};
pub use eg::{Eg, EgParams, Stage};
pub use engine::{Engine, Quality, latency_samples};
pub use matrix::{DestId, N_DESTS, N_MACROS, N_MATRIX_SLOTS, Roster, SourceId};
pub use patch::{N_PATCHES, Patch, patch, patch_names};
