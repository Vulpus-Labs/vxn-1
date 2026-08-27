//! Shared DSP **components** for VXN synth plugins.
//!
//! The middle of three layers ([ADR 0002](../../../adrs/0002-vxn-core-dsp.md)):
//!
//! - **Leaf utils** — `vxn-core-utils`. A free function or a plain-data struct;
//!   no sample rate, no lifecycle.
//! - **Components** — *here*. Anything with a `Params` struct, a sample-rate
//!   constructor, or an enable/declick lifecycle.
//! - **Hot voice kernels** — per-synth. SoA lane loops stay where they are.
//!
//! Depends on `vxn-core-utils` and nothing else, and must never depend on a
//! synth crate. Consumers keep their existing module paths: `vxn-dsp` and
//! `vxn2-dsp` re-export from here rather than churning call sites.
//!
//! ## Writing code in this crate
//!
//! It is compiled into four products' audio threads, so ADR 0002 §4 applies to
//! everything here:
//!
//! - Plain `#[inline]` on anything in a sample loop.
//! - **No `dyn`, no enum-match inside sample loops.** Resolve runtime choices
//!   once at a block edge, to a marker type or an fn-ptr table — a match inside
//!   a lane loop has measurably dropped NEON to scalar in this repo before.
//! - `[profile.release]` sets *thin* LTO, so the crate boundary is not
//!   guaranteed to vanish. "Vectorisation unchanged" is a claim to verify with
//!   the asm-check harness, not to assume.
//!
//! `control`, `declick`, `fx` and `test_util` landed with ticket 0226; `env`
//! and `os_region` are still stubs, filled by 0238 and 0233.

/// Control-rate vocabulary: `CONTROL_BLOCK`, the `UpdateRate` taxonomy, and the
/// `BaseRate` / `OsRate` / `CtrlRate` newtypes that keep "which sample rate is
/// this?" a compile-time question.
///
/// Filled in by ticket 0226.
pub mod control;

/// Enable/disable declicking. `WetFade` — vxn-2's internal wet-path fade — is
/// the shared idiom for per-FX enables (ADR 0002 §5); whole-span switches build
/// their own weighting on `vxn-core-utils`' `raised_cosine_rise`.
///
/// Filled in by ticket 0226.
pub mod declick;

/// Stereo dynamics: feed-forward peak compressor into a `tanh` saturator, with
/// the wet/dry glide and the gain-reduction metering tap. Moved by 0227; both
/// `vxn-dsp` and `vxn2-dsp` re-export it.
pub mod dynamics;

/// The `FxKernel` trait and the shared effect components built on it, plus the
/// `Bypassable` wrapper carrying the off→on edge-reset glue.
///
/// Filled in by tickets 0226 (trait) and 0228–0232 (the effects).
pub mod fx;

/// One-pole TPT high-pass — the scalar kernel. Moved by 0227; vxn-1's 8-wide
/// `PolyHpf` stays per-synth (SoA body, ADR 0002 §3).
pub mod hpf;

/// OTA-C ladder: kernel, modes, mix tables and coefficients. Moved by 0227.
/// The resonance *cap* policy stays per-synth — see `OtaLadderCoeffs::new_capped`.
pub mod filter;

/// `EnvLifecycle` — the note-on / note-off / tick shape that every envelope
/// family in the repo shares, named without moving any numerics.
///
/// Filled in by ticket 0238.
pub mod env {}

/// Oversampled regions: `OsRegion` and `SpanDelay`. The *mechanics* of running
/// part of a signal path at a multiple of the base rate — the policy of when to
/// do so stays per-synth.
///
/// Filled in by ticket 0233.
pub mod os_region {}

/// Bit-exactness and declick-detection helpers shared by the consumers' test
/// suites (the d4 toolkit and friends).
///
/// Filled in by ticket 0226.
pub mod test_util;
