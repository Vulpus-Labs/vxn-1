//! Lane-major matrix storage, sized exactly to a roster at compile time.
//!
//! # The sizing scheme
//!
//! Two synths with very different roster sizes — vxn-2 has 51 destinations,
//! vxn-1b has 16 — share one evaluator. Each must get storage sized to its own
//! roster, with the size known at compile time: no shared `MAX_DESTS` buffer
//! (which would tax vxn-1b with 3× the accumulator clears per block) and no
//! runtime bound in a lane loop.
//!
//! The trick is that the width is **its own const generic parameter, inferred
//! from the argument**, rather than derived from the roster's associated const:
//!
//! ```text
//! pub fn eval<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(
//!     src: &SourceLanes<NS, L>,
//!     out: &mut DestLanes<ND, L>,
//! ) {
//!     assert_source_width::<R, NS>();
//!     assert_dest_width::<R, ND>();
//!     // …
//! }
//! ```
//!
//! Writing `[f32; R::N_DESTS]` — deriving the length *from* the associated
//! const — is what needs unstable `generic_const_exprs`. Taking the length as a
//! parameter does not. The two read almost the same and behave completely
//! differently; this one compiles on the pinned stable toolchain.
//!
//! # …and the hole it would otherwise leave
//!
//! `ND` is inferred from whatever array the caller happens to pass, so nothing
//! about the *types* stops a 16-dest roster being handed a 51-wide accumulator.
//! That is exactly the failure this scheme invites, and it is why
//! [`assert_dest_width`](crate::storage::assert_dest_width) exists: its
//! `const {}` block is checked at monomorphisation and it fires, turning the
//! mismatch into a compile error rather than a silently half-used buffer. The
//! guard is not decoration — see the `compile_fail` doctest on
//! [`clear_dests`](crate::storage::clear_dests).
//!
//! A `compile_fail` doctest is a weak assertion on its own: it passes on *any*
//! compile error, so a fixture that stops type-checking keeps it green while no
//! longer exercising the guard. (The `,E0080` annotation does not close that —
//! rustdoc on the pinned 1.95.0 accepts the code without verifying it; it is
//! there to say which error is meant, not to check for it.) What does close it
//! is the **passing** doctest immediately above, which uses the byte-identical
//! roster fixture: break the fixture and that one fails loudly, so the pair
//! cannot both rot silently. Keep them in step.
//!
//! **The check is post-monomorphisation, so `cargo check` does not see it.**
//! `check` stops at metadata and never instantiates the generic, so a
//! mismatched width passes it silently and only errors under `cargo build` /
//! `cargo test` (`error[E0080]: evaluation panicked: dest storage width does
//! not match this roster's N_DESTS`). That is inherent — a value that depends
//! on a generic parameter cannot be checked before the generic is instantiated
//! — and the only alternative is to make the width an associated *type*, which
//! ADR 0003 rejects for the two extra generic parameters it would push through
//! every shared function. Worth knowing when a "check is clean" report is taken
//! as evidence.
//!
//! # What it costs
//!
//! Each instantiation monomorphises to its exact size, so several copies of the
//! evaluator exist: good for speed, potentially bad for compile time and
//! instruction cache. Measured on a representative 16-slot dest-major
//! accumulate (vxn-1b's `eval_dests_bank` shape) built at the release profile's
//! settings on aarch64:
//!
//! | Instantiations | `__text` | Δ |
//! |---|---|---|
//! | 1 roster (16 dests, `L = 8`) | 220 432 B | — |
//! | + a 51-dest roster at `L = 8` | 222 872 B | **+2 440 B** |
//! | + both rosters at a second `L` | 226 324 B | **+3 452 B** (~1 726 B each) |
//!
//! So a roster's copy of the evaluator is ~2.4 KB. The number that matters is
//! smaller still: **the two rosters never link into the same binary.** A synth's
//! plugin contains its own roster and no other, so per-roster monomorphisation
//! costs each shipped binary nothing over a hand-written evaluator — for scale,
//! `libvxn1b_clap.dylib` is 2.0 MB. The only artifact that pays for both is this
//! crate's test binary.
//!
//! # Should `L` be generic at all?
//!
//! On today's evidence, it is paying for nothing. Both synths use `L = 8`
//! (`vxn2_dsp::stack::STACK_LANES`, and vxn-1b's `RenderBank::LANES =
//! CHANNELS_PER_LAYER`), and vxn-1b's `eval_dests_bank` — already const-generic
//! over the lane count — is instantiated at `L = 8` by its one caller *and* by
//! its one test. There is not a second value of `L` anywhere in the repo, so
//! the genericity is currently not even buying test convenience.
//!
//! It is kept anyway, for two reasons that are about the seam rather than about
//! speed: `L` is the one width that plausibly diverges later (a synth changing
//! its unison width should not touch the shared evaluator), and a mechanism
//! test that wants to check the lane loop's edges wants a small `L` — which
//! [0331](../../../../tickets/closed/0331-matrix-golden-vector-harness.md) is
//! expected to use. If neither materialises, `L` is a cheap thing to fix later:
//! the second lane count is the ~1.7 KB row above, and it is only ever paid by
//! a build that actually instantiates one.

use crate::roster::MatrixRoster;

/// Source values, **source-major**: `[source][lane]`, so one source's lanes are
/// contiguous. `NS` is the roster's source count, `L` the lane count.
///
/// This is vxn-1b's `SourceLanesSoa` layout, not vxn-2's — vxn-2 stores
/// `[[f32; N_SOURCES]; STACK_LANES]`, lane-major, where reading one source
/// across lanes strides a whole row and the accumulate becomes a
/// gather/scatter that cannot vectorise. The two converge in
/// [0328](../../../../tickets/closed/0328-matrix-dest-major-lane-accumulators.md),
/// which is a prerequisite for sharing the evaluator at all.
pub type SourceLanes<const NS: usize, const L: usize> = [[f32; L]; NS];

/// Destination accumulator, **dest-major**: `[dest][lane]`, the mirror of
/// [`SourceLanes`] and the layout 0328 moves vxn-2 to.
pub type DestLanes<const ND: usize, const L: usize> = [[f32; L]; ND];

/// Compile-time proof that `NS`-wide source storage belongs to roster `R`.
///
/// Call this at the top of any roster-generic function that takes caller-sized
/// source storage. It generates no code; the check runs when the enclosing
/// function is monomorphised.
#[inline(always)]
pub fn assert_source_width<R: MatrixRoster, const NS: usize>() {
    const {
        assert!(
            NS == R::N_SOURCES,
            "source storage width does not match this roster's N_SOURCES"
        );
    }
}

/// Compile-time proof that `ND`-wide destination storage belongs to roster `R`.
///
/// See [`assert_source_width`]. This is the guard that closes the hole in the
/// sizing scheme — `ND` is inferred from the caller's array, so without it a
/// roster and its storage could silently disagree.
#[inline(always)]
pub fn assert_dest_width<R: MatrixRoster, const ND: usize>() {
    const {
        assert!(
            ND == R::N_DESTS,
            "dest storage width does not match this roster's N_DESTS"
        );
    }
}

/// Zero a destination accumulator sized to roster `R`.
///
/// The first thing every evaluator does, and — until
/// [0334](../../../../tickets/closed/0334-share-the-evaluator.md) moves the rest
/// of the mechanism here — the only shared mechanism in this crate. It is here
/// now because the sizing scheme needs a real caller to be reviewed against.
///
/// Storage sized to the roster is accepted:
///
/// ```
/// # use vxn_core_matrix::roster::{MatrixRoster, Smoothing, Tier};
/// # use vxn_core_matrix::storage::{DestLanes, clear_dests};
/// # #[derive(Clone, Copy)]
/// # struct Tiny;
/// # impl MatrixRoster for Tiny {
/// #     const N_SOURCES: usize = 2;
/// #     const N_DESTS: usize = 2;
/// #     const N_SLOTS: usize = 16;
/// #     fn source_is_bipolar(src: u8) -> bool { src == 0 }
/// #     fn dest_gain(_dest: u8) -> f32 { 1.0 }
/// #     fn cook_depth(_dest: u8, depth: f32) -> f32 { depth }
/// #     fn dest_tier(_dest: u8) -> Tier { Tier::PerLane }
/// #     fn source_tier(_src: u8) -> Tier { Tier::PerLane }
/// #     fn dest_smoothing(_dest: u8) -> Smoothing { Smoothing::Block }
/// #     fn source_names() -> &'static [&'static str] { &["a", "b"] }
/// #     fn dest_names() -> &'static [&'static str] { &["x", "y"] }
/// #     fn source_labels() -> &'static [&'static str] { &["A", "B"] }
/// #     fn dest_labels() -> &'static [&'static str] { &["X", "Y"] }
/// # }
/// let mut out: DestLanes<2, 8> = [[1.0; 8]; 2];
/// clear_dests::<Tiny, 2, 8>(&mut out);
/// assert_eq!(out, [[0.0; 8]; 2]);
/// ```
///
/// Storage that is not is a **compile** error, not a runtime one — this is the
/// `const {}` guard firing:
///
/// ```compile_fail,E0080
/// # use vxn_core_matrix::roster::{MatrixRoster, Smoothing, Tier};
/// # use vxn_core_matrix::storage::{DestLanes, clear_dests};
/// # #[derive(Clone, Copy)]
/// # struct Tiny;
/// # impl MatrixRoster for Tiny {
/// #     const N_SOURCES: usize = 2;
/// #     const N_DESTS: usize = 2;
/// #     const N_SLOTS: usize = 16;
/// #     fn source_is_bipolar(src: u8) -> bool { src == 0 }
/// #     fn dest_gain(_dest: u8) -> f32 { 1.0 }
/// #     fn cook_depth(_dest: u8, depth: f32) -> f32 { depth }
/// #     fn dest_tier(_dest: u8) -> Tier { Tier::PerLane }
/// #     fn source_tier(_src: u8) -> Tier { Tier::PerLane }
/// #     fn dest_smoothing(_dest: u8) -> Smoothing { Smoothing::Block }
/// #     fn source_names() -> &'static [&'static str] { &["a", "b"] }
/// #     fn dest_names() -> &'static [&'static str] { &["x", "y"] }
/// #     fn source_labels() -> &'static [&'static str] { &["A", "B"] }
/// #     fn dest_labels() -> &'static [&'static str] { &["X", "Y"] }
/// # }
/// // A 51-wide accumulator handed to a 2-dest roster: `ND` is inferred from
/// // the argument, so only the guard catches this.
/// let mut out: DestLanes<51, 8> = [[0.0; 8]; 51];
/// clear_dests::<Tiny, 51, 8>(&mut out);
/// ```
#[inline]
pub fn clear_dests<R: MatrixRoster, const ND: usize, const L: usize>(out: &mut DestLanes<ND, L>) {
    assert_dest_width::<R, ND>();
    for row in out.iter_mut() {
        row.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{Smoothing, Tier};

    /// A roster that is nothing but its sizes. The tables are correctly
    /// *lengthed* (the trait's contract) but carry no names: these fixtures
    /// exist to prove the sizing scheme, and the golden-vector roster with real
    /// content is [`crate::test_roster::TestRoster`].
    macro_rules! sizing_roster {
        ($name:ident, $ns:literal, $nd:literal) => {
            #[derive(Clone, Copy)]
            struct $name;

            impl MatrixRoster for $name {
                const N_SOURCES: usize = $ns;
                const N_DESTS: usize = $nd;
                const N_SLOTS: usize = 16;

                fn source_is_bipolar(_src: u8) -> bool {
                    false
                }
                fn dest_gain(_dest: u8) -> f32 {
                    1.0
                }
                fn cook_depth(_dest: u8, depth: f32) -> f32 {
                    depth
                }
                fn dest_tier(_dest: u8) -> Tier {
                    Tier::PerLane
                }
                fn source_tier(_src: u8) -> Tier {
                    Tier::PerLane
                }
                fn dest_smoothing(_dest: u8) -> Smoothing {
                    Smoothing::Block
                }
                fn source_names() -> &'static [&'static str] {
                    &[""; $ns]
                }
                fn dest_names() -> &'static [&'static str] {
                    &[""; $nd]
                }
                fn source_labels() -> &'static [&'static str] {
                    &[""; $ns]
                }
                fn dest_labels() -> &'static [&'static str] {
                    &[""; $nd]
                }
            }
        };
    }

    // The two real sizings, mirroring `vxn2_engine::matrix` and
    // `vxn1b_engine::matrix`. Copied numbers, not a dependency — this crate
    // must not depend on a synth.
    sizing_roster!(Vxn2Sized, 11, 51);
    sizing_roster!(Vxn1bSized, 12, 16);

    /// The seam's whole claim, in one build: both real sizings instantiate the
    /// same generic function, each against storage of its own exact width.
    #[test]
    fn both_real_sizings_instantiate_in_one_build() {
        let mut wide: DestLanes<51, 8> = [[7.0; 8]; 51];
        clear_dests::<Vxn2Sized, 51, 8>(&mut wide);
        assert!(wide.iter().flatten().all(|&v| v == 0.0));
        assert_eq!(wide.len(), Vxn2Sized::N_DESTS);

        let mut narrow: DestLanes<16, 8> = [[7.0; 8]; 16];
        clear_dests::<Vxn1bSized, 16, 8>(&mut narrow);
        assert!(narrow.iter().flatten().all(|&v| v == 0.0));
        assert_eq!(narrow.len(), Vxn1bSized::N_DESTS);
    }

    /// Widths inferred from the argument, no turbofish — the ergonomic case
    /// the scheme exists for, and the one the guard has to police.
    #[test]
    fn widths_infer_from_the_argument() {
        let mut out = [[1.0f32; 8]; 16];
        clear_dests::<Vxn1bSized, _, _>(&mut out);
        assert_eq!(out[15], [0.0; 8]);
    }

    /// The source-side guard, which has no consumer until the evaluator lands
    /// and so is only reachable by turbofish today.
    #[test]
    fn source_width_guard_accepts_each_roster_width() {
        assert_source_width::<Vxn2Sized, 11>();
        assert_source_width::<Vxn1bSized, 12>();
        let src: SourceLanes<12, 8> = [[0.5; 8]; 12];
        assert_eq!(src.len(), Vxn1bSized::N_SOURCES);
    }

    /// A second lane count, to confirm `L` really is free of the roster —
    /// nothing in the repo uses one today (see the module docs).
    #[test]
    fn lane_count_is_independent_of_the_roster() {
        let mut four: DestLanes<16, 4> = [[3.0; 4]; 16];
        clear_dests::<Vxn1bSized, 16, 4>(&mut four);
        assert_eq!(four, [[0.0; 4]; 16]);
    }

    #[test]
    fn sizing_fixture_tables_match_their_counts() {
        assert_eq!(Vxn2Sized::source_names().len(), Vxn2Sized::N_SOURCES);
        assert_eq!(Vxn2Sized::dest_names().len(), Vxn2Sized::N_DESTS);
        assert_eq!(Vxn1bSized::source_labels().len(), Vxn1bSized::N_SOURCES);
        assert_eq!(Vxn1bSized::dest_labels().len(), Vxn1bSized::N_DESTS);
    }
}
