//! VXN1 DSP kernels.
//!
//! Pure, allocation-free-on-the-hot-path DSP building blocks for the VXN1
//! synthesizer. Adapted from the `patches` / `patches-bundles` codebases and
//! rewritten for VXN1's static signal flow.
//!
//! ## Processing model
//!
//! Kernels expose per-sample `next()` / `tick()` methods. The recurrences
//! (phase accumulation, envelope state machines, ladder integrators) are
//! inherently serial, so the per-sample form is the natural one and is kept
//! bit-faithful to the originals.
//!
//! The *block* optimisation lives one level up in `vxn-engine`: control-rate
//! quantities (modulation, filter coefficients, smoothed parameters) are
//! recomputed once per fixed control block ([`CONTROL_BLOCK`]) and the inner
//! per-sample loop stays branch-light.
//!
//! Nothing here depends on the plugin framework or the UI.

pub mod adsr;
pub mod chorus;
pub mod delay;
pub mod delay_line;
pub mod fdn_reverb;
pub mod halfband;
pub mod hpf;
pub mod lfo;
pub mod limiter;
pub mod math;
pub mod noise;
pub mod oscillator;
pub mod ota_ladder;
pub mod phase;
pub mod phaser;
pub mod poly;
pub mod random_walk;
// `smoothing` lifted to `vxn-core-utils`; re-exported below for back-compat.

/// Channels (DSP voices) per layer. The poly kernels are sized to this: one
/// homogeneous layer renders together, which is what the vectorised lane loop
/// needs (ADR 0003 §10). Fixed so per-voice arrays live on the stack and the
/// compiler can unroll/vectorise voice loops.
pub const CHANNELS_PER_LAYER: usize = 8;

/// Maximum total polyphony across both always-present layers (ADR 0003 §2).
pub const MAX_VOICES: usize = 2 * CHANNELS_PER_LAYER;

/// Maximum oversampling factor for the synthesis path. Bounds the size of the
/// oversampled scratch buffer (`CONTROL_BLOCK * MAX_OVERSAMPLE`).
pub const MAX_OVERSAMPLE: usize = 8;

/// Engine control-block size in samples. Modulation and coefficients are
/// recomputed once per block; the per-sample inner loop runs this many times.
/// 32 @ 48 kHz ≈ 0.67 ms — well below any audible zipper threshold for the
/// modulation depths VXN1 uses.
pub const CONTROL_BLOCK: usize = 32;

pub use adsr::{AdsrCore, AdsrShape, AdsrStage};
pub use chorus::StereoChorus;
pub use delay::{DelayLine, StereoDelay};
pub use fdn_reverb::{FdnReverb, FdnReverbParams};
pub use halfband::{HalfbandFir, Oversampler};
pub use hpf::PolyHpf;
pub use lfo::{LfoCore, LfoShape};
pub use limiter::StereoLimiter;
pub use math::{fast_exp2, fast_sine, fast_tanh, lookup_sine, xorshift64};
pub use noise::{NoiseColor, PolyNoise};
pub use oscillator::Waveform;
pub use ota_ladder::{FilterMode, FilterSlope, OtaLadderCoeffs};
pub use phaser::StereoPhaser;
pub use poly::{PolyNoiseBank, PolyOscillator, PolyOtaLadder, poly_ring_mod, poly_sub_square};
pub use vxn_core_utils::smoothing::{self as smoothing, Smoothed, ms_to_samples, one_pole_coeff};
pub use vxn_core_utils::ScopedFlushToZero;
pub use vxn_core_utils::ftz::flush_denormal;

/// Flush x86/ARM denormals-to-zero on the current thread, without restoring the
/// previous mode. Denormal arithmetic can cost 100× and silently wreck
/// real-time deadlines in filter/delay feedback paths.
///
/// Prefer [`ScopedFlushToZero`] at the top of each `process()` call: it is
/// robust to the host running `process` on a different thread than `activate`,
/// and restores the host's FP mode on the way out so it doesn't perturb other
/// plugins in the chain. This bare setter is kept for tests and one-shot setup.
#[inline]
pub fn enable_flush_to_zero() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // MXCSR bit 15 (FTZ): flush denormal SSE/AVX results to zero.
        let mut mxcsr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut mxcsr, options(nostack, preserves_flags));
        mxcsr |= 0x8000;
        std::arch::asm!("ldmxcsr [{}]", in(reg) &mxcsr, options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // FPCR bit 24 (FZ): flush-to-zero.
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr);
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr);
    }
    // Other targets (0019): no portable FTZ control word, so this is a no-op and
    // denormals are NOT flushed — the phaser/BBD/reverb feedback paths can hit
    // denormal slowdowns on a held-quiet tail. The only shipping targets are
    // x86_64 (CLAP/VST3/standalone) and aarch64 (Apple Silicon), both handled
    // above; this arm exists so a NEW target compiles with an explicit,
    // greppable "denormals unhandled here" instead of silently doing nothing.
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        // Intentionally empty. Adding a target where the denormal tails matter?
        // Wire its FTZ/DAZ control word here (and in `ScopedFlushToZero`).
    }
}

// `ScopedFlushToZero` + `flush_denormal` lifted to `vxn-core-utils`; the
// re-export above keeps existing `vxn_dsp::ScopedFlushToZero` callsites
// working without touching them.

/// Reference frequency for V/oct: MIDI note 0 (C-1) ≈ 8.1758 Hz.
pub const MIDI_0_HZ: f32 = 8.175_799;

/// Convert a MIDI note number (with fractional cents/bend) to frequency in Hz.
#[inline]
pub fn note_to_hz(note: f32) -> f32 {
    MIDI_0_HZ * fast_exp2(note / 12.0)
}
