//! Shared **modulation-matrix routing** for VXN synth plugins.
//!
//! The seam ([ADR 0003](../../../adrs/0003-vxn-core-matrix.md)) runs between:
//!
//! - the **roster** — what a synth can route: its source and destination sets,
//!   each dest's native gain, depth taper, granularity tier and smoothing
//!   class. Per-synth by definition, declared through [`roster::MatrixRoster`].
//! - the **mechanism** — how a routing is evaluated: the polarity/shape axes
//!   and their dispatch, the scale VCA, route compilation, the evaluator, the
//!   smoother bank, the coherence predicate. Generic over the roster, and the
//!   reason this crate exists.
//!
//! vxn-1b's matrix is a hand-port of vxn-2's and says so in its own headers.
//! Two copies of one design have drifted in both directions, so adding a
//! routing feature has meant writing the same ~200 lines twice, by hand. None
//! of those lines is specific to subtractive or FM synthesis.
//!
//! Depends on nothing — not `vxn-core-utils`, not a synth crate — and must go
//! on depending on nothing that knows what a destination *means*. Applying a
//! dest total to a filter coefficient, a phase increment or a VCA stays in the
//! synth.
//!
//! ## Status: the seam only
//!
//! Epic [E049](../../../epics/open/E049-shared-matrix-routing.md) ports the
//! mechanism across in steps, each one null-tested at ≤ −100 dBFS against the
//! render it replaces. Ticket
//! [0329](../../../tickets/open/0329-vxn-core-matrix-crate-skeleton.md) — this
//! one — lands the crate and the roster trait **with no consumers**, so the
//! seam gets reviewed before anything is ported through it. If the trait is
//! wrong, that is the cheap moment to find out. The curve vocabulary arrives in
//! 0330, the generated roster row in 0332, slots and route compilation in 0333,
//! the evaluator in 0334 and the smoother bank in 0335.
//!
//! ## Writing code in this crate
//!
//! It ends up in two products' audio threads, so ADR 0002 §4 applies here as it
//! does to `vxn-core-dsp`:
//!
//! - Plain `#[inline]` on anything in a sample or lane loop.
//! - **No `dyn`, no enum-match inside a lane loop.** Every per-slot decision —
//!   curve polarity, curve shape, scale-source polarity, scale bend — is
//!   dispatched *outside* the lane loop. Letting one ride inside is expensive:
//!   hoisting `scale_norm`'s two decisions cut a fully-scaled 16-slot eval by
//!   ~47% in vxn-2.
//! - **Measure vectorisation post-LTO or not at all.** `cargo rustc --emit asm`
//!   on a library crate in this workspace runs no loop vectoriser — with `lto`
//!   set, cargo passes `-C linker-plugin-lto` and defers to link time, so a
//!   trivially vectorisable loop shows up scalar. Use `llvm-objdump` on a
//!   linked bench binary. Two claims in this epic's own tickets were wrong
//!   before that was caught.
//!
//! ## Sizing
//!
//! vxn-2 has 51 destinations and vxn-1b has 16, and each gets storage sized
//! exactly to its own roster, at compile time, with no shared maximum-width
//! buffer and no runtime bound. [`storage`] holds the scheme, the guard that
//! stops a roster and its storage disagreeing, and the measurements behind
//! both — including what per-roster monomorphisation costs and why `L` is
//! generic when nothing in the repo yet uses a second lane count.

/// The roster half of the seam: [`MatrixRoster`](roster::MatrixRoster) and the
/// two shared vocabularies keyed on a destination, [`Tier`](roster::Tier) and
/// [`Smoothing`](roster::Smoothing).
///
/// Filled in by ticket 0329; ticket 0332 replaces hand-written implementations
/// with one generated from a row list per enum.
pub mod roster;

/// Lane-major matrix storage sized to a roster — the const-generic scheme, and
/// the `const {}` guard that makes a roster/storage mismatch a compile error.
///
/// Filled in by ticket 0329; ticket 0334 puts the evaluator on top of it.
pub mod storage;

/// [`TestRoster`](test_roster::TestRoster) — a synthetic roster with all gains
/// 1.0 and no taper, so a mechanism assertion measures the evaluator's
/// arithmetic and nothing else (ADR 0003 §5).
///
/// Always available to this crate's own tests; exposed to other crates by the
/// `testing` feature. Built on by ticket 0331.
#[cfg(any(test, feature = "testing"))]
pub mod test_roster;
