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
//! ## Status: complete
//!
//! Epic [E049](../../../epics/closed/E049-shared-matrix-routing.md) is closed
//! and this crate holds the whole mechanism. Both synths route through it and
//! neither carries a slot type, a curve axis, a scale VCA, a route compiler, a
//! lane loop or a smoother of its own.
//!
//! It landed in steps, each null-tested against the **pre-epic** render, and the
//! end-to-end result is that both synths render **bit-identically** to the code
//! this replaced — the epic budgeted for last-bit reordering and got none. The
//! module list below is roughly the order it arrived in, and each module's docs
//! carry the measurements behind its shape.
//!
//! ## Writing code in this crate
//!
//! It ends up in two products' audio threads, so ADR 0002 §4 applies here as it
//! does to `vxn-core-dsp`:
//!
//! - Plain `#[inline]` on anything in a sample or lane loop.
//! - **No `dyn`, no enum-match inside a lane loop.** Every per-slot decision —
//!   curve polarity, curve shape, scale range map, scale bend — is
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

/// The curve-shaping vocabulary — the mechanism half's smallest piece, and the
/// duplication that prompted the epic: [`Polarity`](curve::Polarity) /
/// [`Shape`](curve::Shape) and their tables, the flat preset codec
/// ([`curve_code`](curve::curve_code) / [`curve_split`](curve::curve_split)),
/// the polarity maps and shape bends, and the scale VCA
/// ([`scale_norm`](curve::scale_norm)).
///
/// Also home to the [`matrix_enum!`](crate::matrix_enum) generator, which both
/// synths now use for their own source and destination enums — it is exported
/// at the crate root, not from this module.
///
/// Landed in ticket 0330.
pub mod curve;

/// Each `(polarity, shape)` pair as an SVG polyline, plotted from
/// [`curve`]'s own arithmetic — what the faceplates draw on a route's curve
/// button and in the 3×3 picker behind it, so the picture cannot drift from the
/// sound.
///
/// Landed in ticket 0340.
pub mod glyph;

/// The coherence predicate: [`Coherence`](coherence::Coherence), the verdict a
/// faceplate paints a row red on, and [`CoherenceRoster`](coherence::CoherenceRoster),
/// the per-synth hook for the special cases the tier rule does not cover.
///
/// Landed in ticket 0336.
pub mod coherence;

/// The roster half of the seam: [`MatrixRoster`](roster::MatrixRoster) and the
/// two shared vocabularies keyed on a destination, [`Tier`](roster::Tier) and
/// [`Smoothing`](roster::Smoothing).
///
/// Landed in ticket 0329; 0332 replaced hand-written implementations with one
/// generated from a row list per enum, and 0334 added `matrix_roster!` so the
/// forwarding impl is generated too.
pub mod roster;

/// The patch's routing table and the block's compiled routes:
/// [`MatrixSlot`](slot::MatrixSlot), [`MatrixTable`](slot::MatrixTable),
/// [`Route`](slot::Route) and [`RouteList`](slot::RouteList), plus the two
/// endpoint traits a synth's own `SourceId` / `DestId` cross the seam through.
///
/// Landed in ticket 0333; [`RouteList::compile`](slot::RouteList::compile)
/// is the single place a slot's on/off switch, its zero-depth skip, its depth
/// taper and its dest gain are resolved, for both synths.
pub mod slot;

/// Lane-major matrix storage sized to a roster — the const-generic scheme, and
/// the `const {}` guard that makes a roster/storage mismatch a compile error.
///
/// Landed in ticket 0329; 0334 put the evaluator on top of it, which is what
/// the sizing scheme was designed against.
pub mod storage;

/// The evaluator: [`eval_dests`](eval::eval_dests), the scalar per-voice
/// reference, and [`eval_dests_bank`](eval::eval_dests_bank), the dest-major
/// lane loop const-generic over the lane count — plus
/// [`slot_topology_gain`](eval::slot_topology_gain) and its two companions, the
/// gain primitives a synth's own fast paths re-apply piecewise.
///
/// Landed in ticket 0334, the last and largest mechanism move: neither synth
/// carries a lane loop after it.
pub mod eval;

/// Post-sum target smoothing: [`OnePoleBank`](smoothing::OnePoleBank) and
/// [`CascadeBank`](smoothing::CascadeBank), plus
/// [`class_rows`](smoothing::class_rows), which turns a roster's declared
/// `Smoothing` column into the rows a bank smooths.
///
/// Landed in ticket 0335. The recurrence, the state, the snap and the settle
/// predicates are shared; *when* to advance a lane stays each synth's render
/// loop's decision.
pub mod smoothing;

/// [`TestRoster`](test_roster::TestRoster) — a synthetic roster with all gains
/// 1.0 and no taper, so a mechanism assertion measures the evaluator's
/// arithmetic and nothing else (ADR 0003 §5).
///
/// Always available to this crate's own tests; exposed to other crates by the
/// `testing` feature. Built on by ticket 0331.
#[cfg(any(test, feature = "testing"))]
pub mod test_roster;

/// The declarative mechanism test surface (ADR 0003 §5): a
/// [`Case`](golden::Case) is *these routes at these depths, these source
/// values, these modulation amounts*, and [`run_case`](golden::run_case) puts
/// every case through every evaluator path the crate offers, requiring them to
/// agree bit-exactly.
///
/// Run against [`test_roster::TestRoster`], so a number in an assertion is the
/// evaluator's arithmetic and nothing else. Same gating as `test_roster` —
/// available to this crate's tests always, to other crates via `testing`.
///
/// Landed in ticket 0331; 0334 registered the shared evaluator as two further
/// paths, so the whole table covers the code both synths ship.
#[cfg(any(test, feature = "testing"))]
pub mod golden;
