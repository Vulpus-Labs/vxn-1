//! Structure-of-arrays poly kernels for the synthesis hot path.
//!
//! Each kernel holds `[f32; CHANNELS_PER_LAYER]` state and processes one
//! layer's channels per sample in a branchless loop the compiler
//! auto-vectorises (NEON is 4-wide f32, so 8 channels = 2 SIMD lanes deep).
//! Waveform / filter variant are *per-layer* parameters, hoisted outside the
//! lane loop — the inner loop has no data-dependent branches. A heterogeneous
//! second layer is simply a second kernel instance with its own hoisted globals.
//!
//! Mirrors the design of `patches-dsp`'s poly kernels. The mono kernels in the
//! sibling modules remain as `pub(crate)` test oracles (see `oscillator.rs`,
//! `ota_ladder.rs`, `hpf.rs`).

pub mod ladder;
pub mod oscillator;

pub use ladder::PolyOtaLadder;
pub use oscillator::{PolyNoiseBank, PolyOscillator, poly_ring_mod, poly_sub_square};
