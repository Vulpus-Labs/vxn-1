//! VXN2 — framework-free DSP kernels.
//!
//! Ticket 0001 deliverables: operator core (phase accumulator + sine + 4R/4L
//! EG + key scaling + velocity / amp sens + per-op feedback). Higher-level
//! voice / algorithm / matrix work lives in `vxn2-engine` (next epics).

pub mod algo;
pub mod cleanup;
pub mod delay;
pub mod eg;
pub mod envelope;
pub mod filter;
pub mod halfband;
pub mod ks;
pub mod lfo;
pub mod limiter;
pub mod math;
pub mod op;
pub mod reverb;
pub mod rng;
pub mod sine;
pub mod smoother;
pub mod stack;
pub mod tables;
pub mod voice;
