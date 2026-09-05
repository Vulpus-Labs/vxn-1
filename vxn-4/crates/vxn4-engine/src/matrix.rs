//! VXN4's modulation roster, on the shared mechanism.
//!
//! The routing *mechanism* — polarity and shape axes, the scale VCA, route
//! compilation, the evaluator, the coherence predicate — lives in
//! [`vxn_core_matrix`] and is shared with vxn-1b and vxn-2 (ADR 0003, epic
//! E049). This module is only vxn-4's **roster**: what it can route from and
//! to. Nothing here knows how a route is evaluated, and the shared crate knows
//! nothing about operators.
//!
//! ## Sources: 8 macros, and only 8
//!
//! The synth has 72 modulatable outs and the brief allows two sources on each.
//! Exposing that to a host would mean hundreds of automation lanes, and would
//! bake the routing topology into every saved project.
//!
//! So the host sees **8 macro knobs and nothing else about modulation**. The
//! knobs are matrix *sources*; which routes each one drives, and how far, is
//! patch state. A host automates intent, not wiring — and a saved lane that
//! says "macro 3" stays valid when the patch behind it is rewired.
//!
//! The brief's other sources (two LFOs, two envelopes) are not here yet.
//! Adding them is new rows in [`SourceId`], not a new mechanism.
//!
//! ## The two-source pair is already in the shared slot
//!
//! The brief asks for each out to take "two sources: additive and scaling, as
//! per the vxn-1b/2 mod matrices". That is exactly
//! [`vxn_core_matrix::slot::MatrixSlot`]'s `source` plus `scale_src`, with its
//! own polarity and shape axes on the VCA. It came with the shared crate; there
//! is nothing for vxn-4 to implement.
//!
//! ## Destinations: the brief's 72
//!
//! 64 inter-operator PM depths (the diagonal is self-feedback) plus 8 sum-bus
//! sends. Generated rather than hand-written, because 72 near-identical rows is
//! exactly the duplication `matrix_enum!` exists to absorb.
//!
//! Every row is `tier = patch_global`, which is the truth today and not a
//! convenience: vxn-4's route depths are patch-wide, shared across every voice
//! in a bank, which is what lets the operator kernel keep broadcast scalar
//! gains (the op-bench measured per-voice gains at 2-6%, so this is a deliberate
//! deferral rather than a limit). **Revisit the whole column the day a
//! per-voice source lands** — an envelope or velocity is `per_lane`, and a
//! `per_lane` source into a `patch_global` dest is precisely the tier collapse
//! the coherence predicate exists to flag.
//!
//! `smooth = block` likewise says what actually happens: totals are applied once
//! per control block and held, so a fast macro sweep zippers at the control
//! rate. Wiring up the shared smoother bank is the follow-up; the column is
//! declared honestly meanwhile rather than claiming a filter that is not
//! running.
//!
//! PM depths take a **cubic** taper for the same reason vxn-1b's `Pitch` does:
//! the musically useful range of a modulation index is the bottom of the fader,
//! and a linear depth puts every usable setting in the first few percent.

use vxn_core_matrix::{matrix_enum, matrix_roster};
use vxn4_dsp::ops::NOPS;

/// Macro knobs exposed to the host as modulation sources.
pub const N_MACROS: usize = 8;

/// Matrix slots per patch. 16, matching vxn-1b and vxn-2.
///
/// Named `N_MATRIX_SLOTS` rather than `N_SLOTS` because `alloc::N_SLOTS` is the
/// *voice* slot count. Two different 16-ish numbers with the same name in one
/// crate is a bug waiting to happen.
pub const N_MATRIX_SLOTS: usize = 16;

/// Routable destinations: 64 PM depths + 8 sum-bus sends.
pub const N_DESTS: usize = NOPS * NOPS + NOPS;

matrix_enum! {
    /// Modulation source. `None` is the empty-slot sentinel.
    ///
    /// Eight macro knobs, unipolar, one value per patch. These are the only
    /// modulation controls the host can automate — see the module docs.
    SourceId, fallback = None, names = SOURCE_NAMES,
    labels = SOURCE_LABELS, roster_names = ROSTER_SOURCE_NAMES,
    roster_labels = ROSTER_SOURCE_LABELS, polarity;
    sentinel None = 0, "none", "—";
    Macro1 = 1, "macro1", "Macro 1", uni, tier = patch_global;
    Macro2 = 2, "macro2", "Macro 2", uni, tier = patch_global;
    Macro3 = 3, "macro3", "Macro 3", uni, tier = patch_global;
    Macro4 = 4, "macro4", "Macro 4", uni, tier = patch_global;
    Macro5 = 5, "macro5", "Macro 5", uni, tier = patch_global;
    Macro6 = 6, "macro6", "Macro 6", uni, tier = patch_global;
    Macro7 = 7, "macro7", "Macro 7", uni, tier = patch_global;
    Macro8 = 8, "macro8", "Macro 8", uni, tier = patch_global;
}

matrix_enum! {
    /// Modulation destination: every out the brief makes modulatable.
    ///
    /// `PmDS` is the phase-modulation depth into operator `D` from operator
    /// `S`; `PmDD` (D == S) is that operator's self-feedback. `OutD` is
    /// operator `D`'s send into the stereo sum bus.
    DestId, fallback = None, names = DEST_NAMES,
    labels = DEST_LABELS, roster_names = ROSTER_DEST_NAMES,
    roster_labels = ROSTER_DEST_LABELS, roster_gains = ROSTER_DEST_GAIN;
    sentinel None = 0, "none", "—";
    Pm00 = 1, "pm-0-0", "Op0 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm01 = 2, "pm-0-1", "Op0 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm02 = 3, "pm-0-2", "Op0 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm03 = 4, "pm-0-3", "Op0 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm04 = 5, "pm-0-4", "Op0 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm05 = 6, "pm-0-5", "Op0 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm06 = 7, "pm-0-6", "Op0 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm07 = 8, "pm-0-7", "Op0 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm10 = 9, "pm-1-0", "Op1 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm11 = 10, "pm-1-1", "Op1 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm12 = 11, "pm-1-2", "Op1 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm13 = 12, "pm-1-3", "Op1 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm14 = 13, "pm-1-4", "Op1 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm15 = 14, "pm-1-5", "Op1 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm16 = 15, "pm-1-6", "Op1 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm17 = 16, "pm-1-7", "Op1 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm20 = 17, "pm-2-0", "Op2 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm21 = 18, "pm-2-1", "Op2 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm22 = 19, "pm-2-2", "Op2 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm23 = 20, "pm-2-3", "Op2 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm24 = 21, "pm-2-4", "Op2 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm25 = 22, "pm-2-5", "Op2 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm26 = 23, "pm-2-6", "Op2 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm27 = 24, "pm-2-7", "Op2 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm30 = 25, "pm-3-0", "Op3 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm31 = 26, "pm-3-1", "Op3 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm32 = 27, "pm-3-2", "Op3 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm33 = 28, "pm-3-3", "Op3 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm34 = 29, "pm-3-4", "Op3 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm35 = 30, "pm-3-5", "Op3 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm36 = 31, "pm-3-6", "Op3 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm37 = 32, "pm-3-7", "Op3 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm40 = 33, "pm-4-0", "Op4 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm41 = 34, "pm-4-1", "Op4 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm42 = 35, "pm-4-2", "Op4 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm43 = 36, "pm-4-3", "Op4 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm44 = 37, "pm-4-4", "Op4 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm45 = 38, "pm-4-5", "Op4 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm46 = 39, "pm-4-6", "Op4 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm47 = 40, "pm-4-7", "Op4 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm50 = 41, "pm-5-0", "Op5 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm51 = 42, "pm-5-1", "Op5 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm52 = 43, "pm-5-2", "Op5 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm53 = 44, "pm-5-3", "Op5 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm54 = 45, "pm-5-4", "Op5 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm55 = 46, "pm-5-5", "Op5 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm56 = 47, "pm-5-6", "Op5 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm57 = 48, "pm-5-7", "Op5 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm60 = 49, "pm-6-0", "Op6 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm61 = 50, "pm-6-1", "Op6 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm62 = 51, "pm-6-2", "Op6 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm63 = 52, "pm-6-3", "Op6 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm64 = 53, "pm-6-4", "Op6 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm65 = 54, "pm-6-5", "Op6 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm66 = 55, "pm-6-6", "Op6 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm67 = 56, "pm-6-7", "Op6 <- Op7", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm70 = 57, "pm-7-0", "Op7 <- Op0", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm71 = 58, "pm-7-1", "Op7 <- Op1", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm72 = 59, "pm-7-2", "Op7 <- Op2", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm73 = 60, "pm-7-3", "Op7 <- Op3", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm74 = 61, "pm-7-4", "Op7 <- Op4", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm75 = 62, "pm-7-5", "Op7 <- Op5", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm76 = 63, "pm-7-6", "Op7 <- Op6", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Pm77 = 64, "pm-7-7", "Op7 self", gain = 1.0, taper = cubic, tier = patch_global, smooth = block;
    Out0 = 65, "out-0", "Op0 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out1 = 66, "out-1", "Op1 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out2 = 67, "out-2", "Op2 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out3 = 68, "out-3", "Op3 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out4 = 69, "out-4", "Op4 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out5 = 70, "out-5", "Op5 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out6 = 71, "out-6", "Op6 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
    Out7 = 72, "out-7", "Op7 Out", gain = 1.0, taper = linear, tier = patch_global, smooth = block;
}

matrix_roster! {
    /// VXN4's roster: 8 macro sources, 72 destinations, 16 slots.
    ///
    /// Pure forwarding to the generated enums, as in vxn-1b and vxn-2. It exists
    /// so the shared evaluator can size its accumulators through the `const`
    /// guards in [`vxn_core_matrix::storage`], which turn a wrong-width buffer
    /// into a compile error rather than a silent overrun.
    Roster, source = SourceId, dest = DestId, slots = 16,
    source_names = ROSTER_SOURCE_NAMES, source_labels = ROSTER_SOURCE_LABELS,
    dest_names = ROSTER_DEST_NAMES, dest_labels = ROSTER_DEST_LABELS,
}

/// Storage index, or `None` for the sentinel.
///
/// `matrix_enum!` generates the tables and the columns but not this — the
/// sentinel-to-`None` mapping is the seam ADR 0003 §2 describes, and both other
/// synths spell it out the same way.
impl SourceId {
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }
}

impl DestId {
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }
}

impl vxn_core_matrix::slot::SourceEndpoint for SourceId {
    #[inline]
    fn idx(self) -> Option<usize> {
        SourceId::idx(self)
    }

    #[inline]
    fn is_bipolar(self) -> bool {
        SourceId::is_bipolar(self)
    }
}

impl vxn_core_matrix::slot::DestEndpoint for DestId {
    #[inline]
    fn idx(self) -> Option<usize> {
        DestId::idx(self)
    }

    #[inline]
    fn gain(self) -> f32 {
        DestId::gain(self)
    }

    #[inline]
    fn cook_depth(self, depth: f32) -> f32 {
        DestId::cook_depth(self, depth)
    }
}

/// Storage index of the PM-depth destination for route `dest <- src`.
///
/// The generated enum is a flat table, so the engine needs the same
/// `dest * NOPS + src` layout the rows were written in to read a total back
/// out. Asserted against the enum in `tests::dest_indices_match_the_enum`.
#[inline]
pub const fn pm_dest_index(dest: usize, src: usize) -> usize {
    dest * NOPS + src
}

/// Storage index of operator `op`'s sum-bus send destination.
#[inline]
pub const fn out_dest_index(op: usize) -> usize {
    NOPS * NOPS + op
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn_core_matrix::roster::MatrixRoster;
    use vxn_core_matrix::slot::DestEndpoint;

    #[test]
    fn the_roster_is_the_size_the_brief_asks_for() {
        assert_eq!(Roster::N_SOURCES, N_MACROS);
        assert_eq!(Roster::N_DESTS, N_DESTS);
        assert_eq!(N_DESTS, 72, "64 inter-op routes + 8 sum-bus sends");
        assert_eq!(Roster::N_SLOTS, N_MATRIX_SLOTS);
    }

    /// The engine reads dest totals positionally, so the index helpers and the
    /// generated enum must agree exactly. If they drift, modulation silently
    /// lands on the wrong route.
    #[test]
    fn dest_indices_match_the_enum() {
        for (i, d) in DestId::ALL.iter().skip(1).enumerate() {
            assert_eq!(d.idx(), Some(i), "{d:?} storage index");
        }
        assert_eq!(DestId::Pm00.idx(), Some(pm_dest_index(0, 0)));
        assert_eq!(DestId::Pm35.idx(), Some(pm_dest_index(3, 5)));
        assert_eq!(DestId::Pm77.idx(), Some(pm_dest_index(7, 7)));
        assert_eq!(DestId::Out0.idx(), Some(out_dest_index(0)));
        assert_eq!(DestId::Out7.idx(), Some(out_dest_index(7)));
    }

    /// Every source is coarser-or-equal to every dest, so no route is a tier
    /// collapse. This holds only while modulation is patch-wide; the day a
    /// per-voice source lands it will fail, which is the point.
    #[test]
    fn every_route_is_tier_coherent() {
        for s in 0..Roster::N_SOURCES as u8 {
            for d in 0..Roster::N_DESTS as u8 {
                assert!(
                    Roster::source_tier(s).covers(Roster::dest_tier(d)),
                    "source {s} -> dest {d} collapses tiers"
                );
            }
        }
    }

    /// The cubic taper is what makes low modulation indices dialable.
    #[test]
    fn pm_depths_take_a_cubic_taper_and_sends_do_not() {
        assert_eq!(DestId::Pm01.cook_depth(0.5), 0.125);
        assert_eq!(DestId::Out0.cook_depth(0.5), 0.5);
        // Sign-preserving, so a negative depth still subtracts.
        assert_eq!(DestId::Pm01.cook_depth(-0.5), -0.125);
    }

    #[test]
    fn wire_names_are_unique_and_stable() {
        let mut seen: Vec<&str> = ROSTER_DEST_NAMES.to_vec();
        seen.sort_unstable();
        let n = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), n, "duplicate dest wire name");
        assert_eq!(ROSTER_DEST_NAMES[pm_dest_index(2, 6)], "pm-2-6");
        assert_eq!(ROSTER_DEST_NAMES[out_dest_index(4)], "out-4");
        assert_eq!(ROSTER_SOURCE_NAMES[0], "macro1");
    }
}
