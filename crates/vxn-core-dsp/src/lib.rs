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
//! Scaffold only as of ticket 0222 — the modules below are placeholders that
//! later E040 tickets fill in. Nothing depends on this crate yet.

/// Control-rate vocabulary: `CONTROL_BLOCK`, the `UpdateRate` taxonomy, and the
/// `BaseRate` / `OsRate` / `CtrlRate` newtypes that keep "which sample rate is
/// this?" a compile-time question.
///
/// Filled in by ticket 0226.
pub mod control {}

/// Enable/disable declicking. `WetFade` — vxn-2's internal wet-path fade — is
/// the shared idiom for per-FX enables (ADR 0002 §5); whole-span switches keep
/// `BypassXfade`, which lives in `vxn-core-utils`.
///
/// Filled in by ticket 0226.
pub mod declick {}

/// The `FxKernel` trait and the shared effect components built on it, plus the
/// `Bypassable` wrapper carrying the off→on edge-reset glue.
///
/// Filled in by tickets 0226 (trait) and 0228–0232 (the effects).
pub mod fx {}

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
pub mod test_util {}
