//! A synthetic roster for testing the **mechanism**, with no roster content to
//! confound it.
//!
//! [ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §5 splits the test
//! surface in two. Today's tests conflate the two halves: vxn-1b asserts
//! `out[Cutoff] == 24.0`, which is three claims at once — that the evaluator
//! multiplies correctly, that `DEST_GAIN[Cutoff]` is 48, and that `Cutoff`
//! takes no depth taper. Change any one and an unrelated-looking test fails.
//!
//! [`TestRoster`](crate::test_roster::TestRoster) removes two of the three:
//! **every gain is 1.0 and every taper is the identity**, so a number in a
//! mechanism assertion is the evaluator's arithmetic and nothing else. Roster
//! facts — this dest's gain is 48, these dests take the cubic taper — stay
//! tested per-synth, against the real roster.
//!
//! Built on by
//! [0331](../../../../tickets/open/0331-matrix-golden-vector-harness.md), whose
//! golden-vector cases are declarative records over exactly these endpoints.
//!
//! Available to this crate's own tests always, and to other crates' tests via
//! the `testing` feature.

use crate::roster::{MatrixRoster, Smoothing, Tier};

/// Machine ids for [`TestRoster`]'s sources, indexed by storage index.
///
/// Two bipolar and two unipolar, because the scale VCA's fold has a separate
/// arm for each and a mechanism test wants both reachable.
pub const TEST_SOURCE_NAMES: [&str; 4] = ["bi-a", "bi-b", "uni-a", "uni-b"];

/// Display labels for [`TestRoster`]'s sources.
pub const TEST_SOURCE_LABELS: [&str; 4] = ["Bi A", "Bi B", "Uni A", "Uni B"];

/// Machine ids for [`TestRoster`]'s destinations, indexed by storage index.
pub const TEST_DEST_NAMES: [&str; 4] = ["dest-a", "dest-b", "dest-c", "dest-d"];

/// Display labels for [`TestRoster`]'s destinations.
pub const TEST_DEST_LABELS: [&str; 4] = ["Dest A", "Dest B", "Dest C", "Dest D"];

/// Four sources, four destinations, eight slots. All gains 1.0, no taper.
///
/// The non-arithmetic columns *are* spread deliberately, because they cost the
/// arithmetic nothing and later tickets need the coverage:
///
/// | idx | source | polarity | tier | dest | tier | smoothing |
/// |---|---|---|---|---|---|---|
/// | 0 | `bi-a` | bipolar | `PatchGlobal` | `dest-a` | `PatchGlobal` | `Block` |
/// | 1 | `bi-b` | bipolar | `PerStack` | `dest-b` | `PerStack` | `Quantum` |
/// | 2 | `uni-a` | unipolar | `PerLane` | `dest-c` | `PerLane` | `QuantumCascade` |
/// | 3 | `uni-b` | unipolar | `PerLane` | `dest-d` | `PerLane` | `PerSample` |
///
/// That gives [0336](../../../../tickets/open/0336-coherence-in-the-shared-engine.md)
/// every coherent and incoherent tier pairing, and
/// [0335](../../../../tickets/open/0335-declared-target-smoothing.md) one dest
/// of each smoothing class, without any of it perturbing a route product.
///
/// Eight slots rather than four: a golden-vector case wants several slots
/// sharing a destination (the accumulate-order case) *and* inert slots
/// interleaved (the compaction case), and four is not enough room for both.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TestRoster;

impl MatrixRoster for TestRoster {
    const N_SOURCES: usize = 4;
    const N_DESTS: usize = 4;
    const N_SLOTS: usize = 8;

    #[inline]
    fn source_is_bipolar(src: u8) -> bool {
        match src {
            0 | 1 => true,
            2 | 3 => false,
            _ => panic!("TestRoster has 4 sources; got {src}"),
        }
    }

    /// Unity, always — that is the point of this roster.
    #[inline]
    fn dest_gain(dest: u8) -> f32 {
        assert!(dest < 4, "TestRoster has 4 dests; got {dest}");
        1.0
    }

    /// Identity, always — likewise.
    #[inline]
    fn cook_depth(dest: u8, depth: f32) -> f32 {
        assert!(dest < 4, "TestRoster has 4 dests; got {dest}");
        depth
    }

    /// Out-of-range panics rather than falling through to a catch-all: an
    /// off-by-one dest index in a mechanism test must fail loudly, not come
    /// back with a plausible tier the 4-wide accumulator would never catch.
    #[inline]
    fn dest_tier(dest: u8) -> Tier {
        match dest {
            0 => Tier::PatchGlobal,
            1 => Tier::PerStack,
            2 | 3 => Tier::PerLane,
            _ => panic!("TestRoster has 4 dests; got {dest}"),
        }
    }

    /// Panics out of range, for the reason on
    /// [`dest_tier`](TestRoster::dest_tier).
    #[inline]
    fn source_tier(src: u8) -> Tier {
        match src {
            0 => Tier::PatchGlobal,
            1 => Tier::PerStack,
            2 | 3 => Tier::PerLane,
            _ => panic!("TestRoster has 4 sources; got {src}"),
        }
    }

    /// Panics out of range, for the reason on
    /// [`dest_tier`](TestRoster::dest_tier).
    #[inline]
    fn dest_smoothing(dest: u8) -> Smoothing {
        match dest {
            0 => Smoothing::Block,
            1 => Smoothing::Quantum,
            2 => Smoothing::QuantumCascade,
            3 => Smoothing::PerSample,
            _ => panic!("TestRoster has 4 dests; got {dest}"),
        }
    }

    #[inline]
    fn source_names() -> &'static [&'static str] {
        &TEST_SOURCE_NAMES
    }

    #[inline]
    fn dest_names() -> &'static [&'static str] {
        &TEST_DEST_NAMES
    }

    #[inline]
    fn source_labels() -> &'static [&'static str] {
        &TEST_SOURCE_LABELS
    }

    #[inline]
    fn dest_labels() -> &'static [&'static str] {
        &TEST_DEST_LABELS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{DestLanes, clear_dests};

    /// The property the whole fixture exists for: no roster content can leak
    /// into a mechanism assertion, because there is none.
    #[test]
    fn every_gain_is_unity_and_every_taper_is_the_identity() {
        for dest in 0..TestRoster::N_DESTS as u8 {
            assert_eq!(TestRoster::dest_gain(dest), 1.0, "dest {dest}");
            for depth in [-1.0, -0.25, 0.0, 0.5, 1.0] {
                assert_eq!(TestRoster::cook_depth(dest, depth), depth, "dest {dest}");
            }
        }
    }

    #[test]
    fn tables_are_indexed_by_storage_index_and_sized_to_the_counts() {
        assert_eq!(TestRoster::source_names().len(), TestRoster::N_SOURCES);
        assert_eq!(TestRoster::source_labels().len(), TestRoster::N_SOURCES);
        assert_eq!(TestRoster::dest_names().len(), TestRoster::N_DESTS);
        assert_eq!(TestRoster::dest_labels().len(), TestRoster::N_DESTS);
    }

    #[test]
    fn polarity_covers_both_arms_of_the_scale_fold() {
        let bipolar: Vec<bool> = (0..TestRoster::N_SOURCES as u8)
            .map(TestRoster::source_is_bipolar)
            .collect();
        assert_eq!(bipolar, vec![true, true, false, false]);
    }

    /// One destination of every smoothing class, so 0335's bank has a fixture
    /// that exercises each.
    #[test]
    fn every_smoothing_class_has_a_destination() {
        let classes: Vec<Smoothing> = (0..TestRoster::N_DESTS as u8)
            .map(TestRoster::dest_smoothing)
            .collect();
        assert_eq!(
            classes,
            vec![
                Smoothing::Block,
                Smoothing::Quantum,
                Smoothing::QuantumCascade,
                Smoothing::PerSample,
            ]
        );
    }

    /// Both coherent and incoherent tier pairings are reachable, so 0336 can
    /// test its predicate here rather than against a synth's roster.
    #[test]
    fn tiers_span_coherent_and_incoherent_pairings() {
        // PatchGlobal source → PerLane dest: coherent.
        assert!(TestRoster::source_tier(0).covers(TestRoster::dest_tier(2)));
        // PerLane source → PatchGlobal dest: a lossy collapse.
        assert!(!TestRoster::source_tier(2).covers(TestRoster::dest_tier(0)));
    }

    /// An index past the roster is a caller bug and must say so — the trait's
    /// stated contract, and the reason none of the lookups has a catch-all arm.
    #[test]
    #[should_panic(expected = "TestRoster has 4 dests")]
    fn an_out_of_range_dest_panics() {
        let _ = TestRoster::dest_smoothing(4);
    }

    /// Likewise on the source side.
    #[test]
    #[should_panic(expected = "TestRoster has 4 sources")]
    fn an_out_of_range_source_panics() {
        let _ = TestRoster::source_is_bipolar(4);
    }

    /// The fixture is a roster like any other, so the sizing guard admits it.
    #[test]
    fn sizes_its_own_storage() {
        let mut out: DestLanes<4, 8> = [[1.0; 8]; 4];
        clear_dests::<TestRoster, _, _>(&mut out);
        assert_eq!(out, [[0.0; 8]; 4]);
    }
}
