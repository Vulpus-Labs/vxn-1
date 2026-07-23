//! VXN2 — framework-free DSP kernels.
//!
//! Operator core (phase accumulator + sine + 4R/4L EG + key scaling +
//! velocity / amp sens + per-op feedback). Higher-level voice / algorithm /
//! matrix work lives in `vxn2-engine`.

pub mod algo;
pub mod cleanup;
pub mod delay;
pub mod dynamics;
pub mod eg;
pub mod envelope;
pub mod filter;
pub mod halfband;
pub mod hpf;
pub mod ks;
pub mod lfo;
pub mod limiter;
pub mod math;
pub mod op;
pub mod phaser;
pub mod reverb;
pub mod rng;
pub mod sine;
pub mod smoother;
pub mod stack;
pub mod tables;
pub mod voice;

#[cfg(test)]
mod test_util;
